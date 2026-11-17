//! 上游拒绝的派生证据与重放锁；账号健康不参与内容风险判断。

use super::*;
use gateway_admin::ports::prompt_guard::GuardIncident;
use gateway_core::engine::prompt_guard::{UpstreamPolicyAssessment, upstream_policy_code};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(super) struct Feedback {
    protocol: String,
    model: String,
    fingerprint: String,
    pub(super) lock_scopes: Vec<String>,
    // 仅活在当前请求内，不实现 Debug；收到拒绝后才脱敏，禁止直接入库。
    current_text: Option<String>,
    learning_enabled: bool,
    lock_hours: Option<u32>,
}

fn digest(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl HostPromptGuard {
    pub(super) fn prepare_feedback(
        &self,
        operation: &Operation,
        envelope: &Envelope,
        config: &PromptGuardConfig,
    ) -> Result<Feedback, &'static str> {
        let original: Vec<_> = envelope
            .segments
            .iter()
            .filter(|s| s.origin == envelope::Origin::CurrentUser && s.representation == "original")
            .map(|s| s.text.as_str())
            .collect();
        let text = original.join("\n");
        let body: Value = serde_json::from_slice(
            &operation
                .wire_body()
                .map_err(|_| "prompt_guard_unsupported_content")?,
        )
        .map_err(|_| "prompt_guard_unsupported_content")?;
        let fingerprint = digest(&[operation.protocol(), &text]);
        // 空用户文本不能合并成一个全局锁；此时只使用完整请求的不可逆指纹。
        let replay = if text.is_empty() {
            digest(&[operation.protocol(), &body.to_string()])
        } else {
            fingerprint.clone()
        };
        let mut scopes = vec![format!("prompt:{replay}")];
        let session = match operation {
            Operation::Generate(request) => request
                .client_turn()
                .map(|turn| turn.thread_id())
                .or_else(|| {
                    request
                        .protocol_payload()
                        .context()
                        .get("session_id")
                        .and_then(Value::as_str)
                }),
            Operation::CountTokens(request) => request
                .payload()
                .context()
                .get("session_id")
                .and_then(Value::as_str),
            _ => None,
        };
        if let Some(session) = session.filter(|value| !value.is_empty()) {
            scopes.push(format!("session:{}", digest(&[session])));
        }
        // 无文本事件共用待补证候选，重放锁仍按完整请求区分，避免生成无穷的不可学习候选。
        Ok(Feedback {
            protocol: operation.protocol().into(),
            model: body["model"].as_str().unwrap_or("").into(),
            fingerprint: if text.is_empty() {
                format!("insufficient:{}", operation.protocol())
            } else {
                fingerprint
            },
            lock_scopes: scopes,
            current_text: (config.learning_enabled && !text.is_empty()).then_some(text),
            learning_enabled: config.learning_enabled,
            lock_hours: (config.conversation_lock_enabled && config.mode == "enforce")
                .then_some(config.conversation_lock_ttl_hours),
        })
    }

    pub(super) async fn capture_failure(
        &self,
        context: PromptGuardContext<'_>,
        error: &ProviderError,
    ) {
        let Some(code) = upstream_policy_code(error) else {
            return;
        };
        self.capture_rejection(context, code, true).await;
    }

    pub(super) async fn capture_refusal(&self, context: PromptGuardContext<'_>) {
        // refusal 不证明违反某项策略；保留独立来源供人工学习，不继承策略拒绝的会话锁。
        self.capture_rejection(context, "refusal", false).await;
    }

    async fn capture_rejection(
        &self,
        context: PromptGuardContext<'_>,
        code: &'static str,
        allow_lock: bool,
    ) {
        let Some(value) = context.state.take_feedback() else {
            return;
        };
        let Ok(feedback) = serde_json::from_value::<Feedback>(value) else {
            tracing::error!("invalid internal prompt guard feedback");
            return;
        };
        let lock_hours = feedback.lock_hours.filter(|_| allow_lock);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut evidence = None;
        let mut quality = if feedback.learning_enabled {
            "no_current_user"
        } else {
            "disabled"
        };
        if let Some(text) = feedback.current_text {
            quality = "unavailable";
            // 只有拒绝样本进入映射空间；先完整脱敏再裁剪。为锁定元数据预留写入预算。
            let redacted = tokio::time::timeout(Duration::from_millis(500), async {
                self.veil
                    .session(gateway_veil::Scope::Review(context.request_id))
                    .map_err(|_| ())?
                    .redact(json!({"text":text}))
                    .await
                    .map_err(|_| ())
            })
            .await;
            if let Ok(Ok(value)) = redacted
                && let Some(text) = value["text"].as_str()
            {
                quality = if text.chars().count() > 8192 {
                    "truncated"
                } else {
                    "complete"
                };
                evidence = Some(text.chars().take(8192).collect());
            }
        }
        let incident = GuardIncident {
            request_id: context.request_id.into(),
            client_key_id: context.client_key_id.into(),
            protocol: feedback.protocol,
            model: feedback.model,
            fingerprint: if code == "refusal" {
                digest(&[code, &feedback.fingerprint])
            } else {
                feedback.fingerprint
            },
            lock_scopes: feedback.lock_scopes,
            evidence,
            evidence_quality: quality.into(),
            code: code.into(),
            learning_enabled: feedback.learning_enabled,
        };
        let recorded = if let Some(store) = &self.store {
            matches!(
                tokio::time::timeout_at(deadline, store.record_incident(incident, lock_hours))
                    .await,
                Ok(Ok(()))
            )
        } else {
            false
        };
        if !recorded {
            tracing::warn!(
                request_id = context.request_id,
                "prompt guard upstream feedback persistence was not confirmed"
            );
        }
        context.state.upstream_policy(UpstreamPolicyAssessment {
            code: code.into(),
            recorded,
            lock_applied: recorded && lock_hours.is_some(),
        });
    }
}
