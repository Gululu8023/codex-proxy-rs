//! 独立 Prompt 审查控制面；规则发布和证据状态由同一仓储事务维护。

use crate::model::{AdminError, MutationContext, prompt_guard::PromptGuardConfig};
use async_trait::async_trait;
use gateway_core::engine::prompt_guard::PromptGuardAssessment;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardRule {
    pub id: String,
    pub pattern: String,
    pub category: String,
    pub weight: u32,
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub signal_only: bool,
    #[serde(default)]
    pub all_patterns: Vec<String>,
    #[serde(default)]
    pub any_patterns: Vec<String>,
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RuleSnapshot {
    pub revision: i64,
    pub rules: Vec<GuardRule>,
}

#[derive(Clone)]
pub struct GuardIncident {
    pub request_id: String,
    pub client_key_id: String,
    pub protocol: String,
    pub model: String,
    pub fingerprint: String,
    pub lock_scopes: Vec<String>,
    /// 仅允许经过隐私运行时处理的当前用户文本；缺失时只能归档，不能自动生成规则。
    pub evidence: Option<String>,
    pub evidence_quality: String,
    pub code: String,
    pub learning_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleCandidate {
    pub id: i64,
    pub revision: i64,
    pub status: String,
    pub source: String,
    pub protocol: String,
    pub evidence_count: i64,
    pub evidence: Option<String>,
    pub evidence_quality: String,
    pub rule: Option<GuardRule>,
    pub rationale: String,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardListQuery {
    pub page: u32,
    pub page_size: u16,
    pub protocol: Option<String>,
    pub status: Option<String>,
}
impl GuardListQuery {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.page == 0
            || self.page_size == 0
            || self.page_size > 100
            || u64::from(self.page) * u64::from(self.page_size) > 100_000
            || self
                .protocol
                .as_deref()
                .is_some_and(|p| !matches!(p, "openai" | "anthropic"))
            || self
                .status
                .as_deref()
                .is_some_and(|s| !matches!(s, "pending" | "published" | "dismissed" | "disabled"))
        {
            return Err(AdminError::invalid("Prompt 审查分页或筛选条件无效"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidatePage {
    pub items: Vec<RuleCandidate>,
    pub total: i64,
    pub page: u32,
    pub page_size: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateUpdate {
    pub id: i64,
    pub revision: i64,
    pub rule: GuardRule,
    pub rationale: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateAction {
    pub id: i64,
    pub revision: i64,
    pub action: String,
}

#[async_trait]
pub trait PromptGuardStore: Send + Sync {
    async fn rules(&self) -> Result<RuleSnapshot, AdminError>;
    async fn is_locked(
        &self,
        key: &str,
        protocol: &str,
        scopes: &[String],
    ) -> Result<bool, AdminError>;
    async fn record_incident(
        &self,
        incident: GuardIncident,
        lock_hours: Option<u32>,
    ) -> Result<(), AdminError>;
    async fn candidates(&self, query: GuardListQuery) -> Result<CandidatePage, AdminError>;
    async fn candidate(&self, id: i64) -> Result<RuleCandidate, AdminError>;
    async fn save_candidate(
        &self,
        context: &MutationContext,
        update: CandidateUpdate,
    ) -> Result<RuleCandidate, AdminError>;
    async fn transition(
        &self,
        context: &MutationContext,
        action: CandidateAction,
    ) -> Result<RuleCandidate, AdminError>;
    async fn profiles(&self, query: GuardListQuery) -> Result<serde_json::Value, AdminError>;
    async fn locks(&self, query: GuardListQuery) -> Result<serde_json::Value, AdminError>;
    async fn release_lock(
        &self,
        context: &MutationContext,
        id: i64,
        revision: i64,
    ) -> Result<(), AdminError>;
}

/// API 依赖能力端口，规则编译、复审请求及隐私处理继续归 Host 所有。
#[async_trait]
pub trait PromptGuardControl: Send + Sync {
    async fn policy(&self) -> Result<serde_json::Value, AdminError>;
    async fn validate_config(&self, config: &PromptGuardConfig) -> Result<(), AdminError>;
    async fn evaluate(
        &self,
        text: String,
        origin: String,
    ) -> Result<PromptGuardAssessment, AdminError>;
    async fn candidates(&self, query: GuardListQuery) -> Result<CandidatePage, AdminError>;
    async fn save_candidate(
        &self,
        context: &MutationContext,
        update: CandidateUpdate,
    ) -> Result<RuleCandidate, AdminError>;
    async fn candidate_action(
        &self,
        context: &MutationContext,
        action: CandidateAction,
    ) -> Result<RuleCandidate, AdminError>;
    async fn profiles(&self, query: GuardListQuery) -> Result<serde_json::Value, AdminError>;
    async fn locks(&self, query: GuardListQuery) -> Result<serde_json::Value, AdminError>;
    async fn release_lock(
        &self,
        context: &MutationContext,
        id: i64,
        revision: i64,
    ) -> Result<(), AdminError>;
}
