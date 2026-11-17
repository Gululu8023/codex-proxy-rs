//! 最终待发送内容的宿主审查端口，凭据和 HTTP 实现留在外层

use crate::{
    error::ProviderError,
    lifecycle::{CancellationToken, Deadline},
    operation::Operation,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// 安全摘要只包含受控分类与数字，不保存正文、匹配片段或审核模型的自由文本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptGuardAssessment {
    pub policy_version: String,
    pub rules_version: String,
    pub normalization_version: String,
    pub mode: String,
    pub action: GuardAction,
    pub would_action: GuardAction,
    pub raw_score: u32,
    pub signal_score: u32,
    pub audit_score: u32,
    pub decision_score: u32,
    pub categories: Vec<CategoryScore>,
    pub hits: Vec<RuleHit>,
    #[serde(default)]
    pub omitted_hits: u32,
    pub coverage: Coverage,
    pub reason_code: Option<String>,
    pub review: Option<ReviewAssessment>,
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub upstream_policy: Option<UpstreamPolicyAssessment>,
    pub local_ms: u64,
    pub total_ms: u64,
    pub evaluation_count: u32,
    pub reuse_count: u32,
}

impl PromptGuardAssessment {
    pub fn pending() -> Self {
        Self {
            policy_version: String::new(),
            rules_version: String::new(),
            normalization_version: String::new(),
            mode: "enforce".into(),
            action: GuardAction::Unavailable,
            would_action: GuardAction::Unavailable,
            raw_score: 0,
            signal_score: 0,
            audit_score: 0,
            decision_score: 0,
            categories: Vec::new(),
            hits: Vec::new(),
            omitted_hits: 0,
            coverage: Coverage::Incomplete,
            reason_code: None,
            review: None,
            local_ms: 0,
            total_ms: 0,
            evaluation_count: 0,
            reuse_count: 0,
            terminal: false,
            upstream_policy: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardAction {
    Disabled,
    Allow,
    Warn,
    Block,
    CoverageError,
    Unavailable,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamPolicyAssessment {
    pub code: String,
    pub recorded: bool,
    pub lock_applied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Complete,
    TextOnly,
    Incomplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryScore {
    pub category: String,
    pub raw_score: u32,
    pub decision_score: u32,
    pub observation_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleHit {
    pub rule_id: String,
    pub category: String,
    pub origin: String,
    /// 固定协议路径和数组位置，绝不使用用户定义的对象键。
    pub source: String,
    pub representation: String,
    pub weight: u32,
    pub effective_weight: u32,
    pub signal_only: bool,
    pub observation_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewAssessment {
    pub result: String,
    pub confidence_bps: Option<u32>,
    pub threshold_bps: u32,
    pub evaluated_categories: Vec<String>,
    pub input_chars: u32,
    pub selected_blocks: u32,
    pub omitted_blocks: u32,
    pub omitted_chars: u64,
    pub projection: String,
    pub redacted: bool,
    #[serde(default)]
    pub request_mode: String,
    #[serde(default)]
    pub category_scores: std::collections::BTreeMap<String, u32>,
}

#[derive(Debug, Default)]
struct State {
    assessment: Option<PromptGuardAssessment>,
    fingerprint: Option<[u8; 32]>,
    evaluations: u32,
    reuses: u32,
    feedback: Option<Feedback>,
}

struct Feedback(serde_json::Value);
impl std::fmt::Debug for Feedback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Feedback([REDACTED])")
    }
}

/// 每个逻辑请求单独创建，attempt 只克隆引用；不建立跨请求审核缓存。
#[derive(Debug, Clone, Default)]
pub struct PromptGuardState(Arc<Mutex<State>>);

impl PromptGuardState {
    /// 仅保存当前请求的临时反馈材料，回调取走后不再持有；禁止进入日志或请求摘要。
    pub fn set_feedback(&self, value: serde_json::Value) {
        self.0.lock().expect("prompt guard state").feedback = Some(Feedback(value));
    }
    pub fn take_feedback(&self) -> Option<serde_json::Value> {
        self.0
            .lock()
            .expect("prompt guard state")
            .feedback
            .take()
            .map(|value| value.0)
    }
    pub fn upstream_policy(&self, value: UpstreamPolicyAssessment) {
        let mut state = self.0.lock().expect("prompt guard state");
        if let Some(assessment) = &mut state.assessment {
            assessment.upstream_policy = Some(value);
        }
    }
    /// 在复审等待期间保留已完成的本地分析，取消时仍能形成终态摘要。
    pub fn progress(&self, assessment: PromptGuardAssessment) {
        let mut state = self.0.lock().expect("prompt guard state");
        state.assessment = Some(assessment);
        state.fingerprint = None;
    }
    pub fn reuse(&self, fingerprint: [u8; 32]) -> Option<PromptGuardAssessment> {
        let mut state = self.0.lock().expect("prompt guard state");
        if state.fingerprint != Some(fingerprint) {
            return None;
        }
        state.reuses = state.reuses.saturating_add(1);
        let mut assessment = state.assessment.clone()?;
        assessment.reuse_count = state.reuses;
        Some(assessment)
    }

    pub fn record(&self, mut assessment: PromptGuardAssessment, fingerprint: Option<[u8; 32]>) {
        let mut state = self.0.lock().expect("prompt guard state");
        state.evaluations = state.evaluations.saturating_add(1);
        assessment.evaluation_count = state.evaluations;
        assessment.reuse_count = state.reuses;
        state.assessment = Some(assessment);
        state.fingerprint = fingerprint;
    }

    pub fn snapshot(&self) -> Option<PromptGuardAssessment> {
        let state = self.0.lock().expect("prompt guard state");
        state.assessment.clone().map(|mut assessment| {
            assessment.evaluation_count = state.evaluations;
            assessment.reuse_count = state.reuses;
            assessment
        })
    }
}

pub struct PromptGuardContext<'a> {
    pub request_id: &'a str,
    pub client_key_id: &'a str,
    pub state: &'a PromptGuardState,
}

#[async_trait]
pub trait PromptGuard: Send + Sync + std::fmt::Debug {
    /// 每次业务调用同步完成审查，失败时必须保持 NotSent 且禁止重试
    async fn check(
        &self,
        operation: &Operation,
        context: PromptGuardContext<'_>,
        cancellation: &CancellationToken,
        deadline: Deadline,
    ) -> Result<(), ProviderError>;
    /// 上游明确的策略拒绝是终态反馈，不能据此轮换账号或归因成账号故障。
    async fn observe_failure(&self, _context: PromptGuardContext<'_>, _error: &ProviderError) {}
    /// 原生成功响应中的拒绝只产生业务观测，不能转成 ProviderError 或账号故障。
    async fn observe_refusal(&self, _context: PromptGuardContext<'_>) {}
}

pub fn upstream_policy_code(error: &ProviderError) -> Option<&'static str> {
    upstream_policy_code_from_fields(error.upstream_failure_fields()?)
}

/// Provider 在账号冷却或凭据恢复之前复用同一策略分类，不能把内容拒绝当作账号故障。
pub fn upstream_policy_code_from_fields(
    fields: &crate::upstream::UpstreamFailureFields,
) -> Option<&'static str> {
    for code in [fields.code.as_deref(), fields.error_type.as_deref()]
        .into_iter()
        .flatten()
    {
        let code = code.trim();
        if code.eq_ignore_ascii_case("cyber_policy") {
            return Some("cyber_policy");
        }
        if code.eq_ignore_ascii_case("content_policy_violation") {
            return Some("content_policy_violation");
        }
    }
    None
}
