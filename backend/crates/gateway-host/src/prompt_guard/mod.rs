//! 本地评分与独立语义复审；Claude 派生请求复用强制 veil 边界。

mod control;
mod envelope;
mod feedback;
mod normalize;
mod projection;
mod rules;

use async_trait::async_trait;
use envelope::Envelope;
use gateway_admin::ports::prompt_guard::PromptGuardStore;
use gateway_admin::{model::prompt_guard::PromptGuardConfig, ports::store::SettingsStore};
use gateway_core::{
    engine::prompt_guard::{
        Coverage, GuardAction, PromptGuard, PromptGuardAssessment, PromptGuardContext,
        ReviewAssessment,
    },
    error::{ClientVisibleUpstreamResponse, ProviderDiagnostic, ProviderError, ProviderErrorKind},
    lifecycle::{CancellationToken, Deadline},
    operation::{Operation, OperationKind},
    upstream::UpstreamSendState,
};
use rules::{Classification, POLICY_VERSION, Rules, VERSION};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub struct HostPromptGuard {
    settings: Arc<dyn SettingsStore>,
    rules: Arc<Rules>,
    slots: Arc<tokio::sync::Semaphore>,
    veil: Arc<gateway_veil::Runtime>,
    client: reqwest::Client,
    store: Option<Arc<dyn PromptGuardStore>>,
    rule_cache: RwLock<(i64, Arc<Rules>)>,
    review_active: Arc<AtomicUsize>,
    review_key_cursor: AtomicUsize,
}
impl std::fmt::Debug for HostPromptGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostPromptGuard")
    }
}
impl HostPromptGuard {
    /// 页面读取正在执行的规则目录，不维护第二份权重或类别配置。
    pub fn policy_catalog(&self) -> Value {
        self.rules.catalog()
    }
    pub fn new(
        settings: Arc<dyn SettingsStore>,
        veil: Arc<gateway_veil::Runtime>,
    ) -> Result<Self, &'static str> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "prompt_guard_client")?;
        let rules = Arc::new(Rules::new()?);
        Ok(Self {
            settings,
            rules: rules.clone(),
            slots: Arc::new(tokio::sync::Semaphore::new(32)),
            veil,
            client,
            store: None,
            rule_cache: RwLock::new((0, rules)),
            review_active: Arc::new(AtomicUsize::new(0)),
            review_key_cursor: AtomicUsize::new(0),
        })
    }
    pub fn with_store(mut self, store: Arc<dyn PromptGuardStore>) -> Self {
        self.store = Some(store);
        self
    }

    async fn active_rules(&self) -> Result<(i64, Arc<Rules>), &'static str> {
        let store = self
            .store
            .as_ref()
            .ok_or("prompt_guard_store_unavailable")?;
        let snapshot = store
            .rules()
            .await
            .map_err(|_| "prompt_guard_store_unavailable")?;
        {
            let cached = self.rule_cache.read().expect("prompt rules cache");
            if cached.0 == snapshot.revision {
                return Ok((cached.0, cached.1.clone()));
            }
        }
        let revision = snapshot.revision;
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "prompt_guard_review_busy")?;
        let rules = tokio::task::spawn_blocking(move || {
            let _slot = slot;
            Rules::with_custom(&snapshot.rules).map(Arc::new)
        })
        .await
        .map_err(|_| "prompt_guard_unavailable")??;
        let mut cached = self.rule_cache.write().expect("prompt rules cache");
        if cached.0 < revision {
            *cached = (revision, rules.clone());
        }
        Ok((revision, rules))
    }

    async fn inspect(
        &self,
        operation: &Operation,
        context: &PromptGuardContext<'_>,
    ) -> (PromptGuardAssessment, Option<[u8; 32]>, bool) {
        let started = Instant::now();
        let mut assessment = pending();
        let result = async {
            let config = self
                .settings
                .load_runtime_settings()
                .await
                .map_err(|_| "prompt_guard_unavailable")?
                .values
                .prompt_guard;
            assessment.mode = config.mode.clone();
            if !config.enabled {
                assessment.action = GuardAction::Disabled;
                assessment.would_action = GuardAction::Disabled;
                assessment.coverage = Coverage::Incomplete;
                return Ok((assessment.clone(), None, false));
            }
            let (rules_revision, rules) = self.active_rules().await?;
            let mut digest = Sha256::new();
            for fact in [
                context.request_id,
                context.client_key_id,
                operation.protocol(),
                POLICY_VERSION,
                VERSION,
                normalize::VERSION,
                self.veil.rules_revision().unwrap_or("unavailable"),
            ] {
                digest.update(fact.as_bytes());
                digest.update([0]);
            }
            digest.update(config.to_storage().to_string().as_bytes());
            digest.update(rules_revision.to_le_bytes());
            digest.update(
                operation
                    .wire_body()
                    .map_err(|_| "prompt_guard_unsupported_content")?,
            );
            let fingerprint: [u8; 32] = digest.finalize().into();
            if let Some(reused) = context.state.reuse(fingerprint) {
                return Ok((reused, Some(fingerprint), true));
            }
            context.state.progress(assessment.clone());
            let slot = self
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| "prompt_guard_review_busy")?;
            let input = operation.clone();
            let local_config = config.clone();
            // 本地扫描覆盖全部分析文本，32K 只约束派生复审材料；取消不提前释放阻塞槽位。
            let (envelope, classification, _slot) = tokio::task::spawn_blocking(move || {
                let mut envelope =
                    Envelope::extract(&input).map_err(|_| "prompt_guard_unsupported_content")?;
                let classification = rules.classify(&mut envelope, &local_config);
                Ok::<_, &'static str>((envelope, classification, slot))
            })
            .await
            .map_err(|_| "prompt_guard_unavailable")??;
            let mut classification = classification;
            assessment = local_decision(&classification, &config);
            assessment.rules_version = format!("{VERSION}:{rules_revision}");
            assessment.mode = config.mode.clone();
            assessment.local_ms = elapsed(started);
            context.state.progress(assessment.clone());
            if envelope.unclassified {
                return Err("prompt_guard_unsupported_content");
            }
            let feedback = self.prepare_feedback(operation, &envelope, &config)?;
            let locked = config.conversation_lock_enabled
                && self
                    .store
                    .as_ref()
                    .ok_or("prompt_guard_store_unavailable")?
                    .is_locked(
                        context.client_key_id,
                        operation.protocol(),
                        &feedback.lock_scopes,
                    )
                    .await
                    .map_err(|_| "prompt_guard_store_unavailable")?;
            context.state.set_feedback(
                serde_json::to_value(feedback).map_err(|_| "prompt_guard_unavailable")?,
            );
            if locked {
                assessment.action = GuardAction::Block;
                assessment.terminal = true;
                assessment.reason_code = Some("prompt_guard_conversation_locked".into());
            } else if !assessment.terminal
                && config.review_enabled
                && (config.scope == "all" || !classification.categories.is_empty())
            {
                if config.scope == "all" {
                    classification.categories = Rules::execution_categories();
                }
                let local_action = assessment.action;
                if let Err(code) = self
                    .review(
                        &config,
                        &envelope,
                        &classification,
                        operation.protocol() == "anthropic",
                        context.request_id,
                        &mut assessment,
                    )
                    .await
                {
                    if let Some(review) = &mut assessment.review {
                        review.result = code.into();
                    }
                    if local_action != GuardAction::Block {
                        assessment.reason_code = Some(code.into());
                    }
                    // 失败放行只作用于外部复审，不能抹掉独立的本地阻断或告警。
                    assessment.action =
                        if config.review_fail_closed && local_action != GuardAction::Block {
                            if code == "prompt_guard_review_input_too_large" {
                                assessment.coverage = Coverage::Incomplete;
                                GuardAction::CoverageError
                            } else {
                                GuardAction::Unavailable
                            }
                        } else {
                            local_action
                        };
                }
            }
            assessment.would_action = assessment.action;
            if config.mode == "observe" {
                assessment.action = GuardAction::Allow;
            }
            assessment.total_ms = elapsed(started);
            Ok((assessment.clone(), Some(fingerprint), false))
        }
        .await;
        match result {
            Ok(result) => result,
            Err(code) => {
                assessment.reason_code = Some(code.into());
                assessment.would_action = if matches!(
                    code,
                    "prompt_guard_unsupported_content" | "prompt_guard_review_input_too_large"
                ) {
                    assessment.coverage = Coverage::Incomplete;
                    GuardAction::CoverageError
                } else {
                    GuardAction::Unavailable
                };
                assessment.action = if assessment.mode == "observe" {
                    GuardAction::Allow
                } else {
                    assessment.would_action
                };
                assessment.total_ms = elapsed(started);
                (assessment, None, false)
            }
        }
    }

    async fn review(
        &self,
        config: &PromptGuardConfig,
        envelope: &Envelope,
        classification: &Classification,
        claude: bool,
        request_id: &str,
        assessment: &mut PromptGuardAssessment,
    ) -> Result<(), &'static str> {
        if !config.ready() {
            return Err("prompt_guard_not_configured");
        }
        let projection = if config.review_request_mode == "moderations" {
            projection::moderation(envelope, &config.review_model)
        } else {
            projection::build(
                envelope,
                classification,
                &config.review_model,
                config.max_text_chars as usize,
            )?
        };
        let mut payload = projection.payload;
        let mut review = ReviewAssessment {
            result: "pending".into(),
            confidence_bps: None,
            threshold_bps: config.confidence_threshold_bps,
            evaluated_categories: Vec::new(),
            input_chars: 0,
            selected_blocks: projection.selected_blocks,
            omitted_blocks: projection.omitted_blocks,
            omitted_chars: projection.omitted_chars,
            projection: "current_turn_with_context".into(),
            redacted: false,
            request_mode: config.review_request_mode.clone(),
            category_scores: Default::default(),
        };
        if config.review_request_mode == "moderations" {
            // Moderations 没有来源语义合同，不能把系统/历史/工具材料当作用户输入评分。
            if review.selected_blocks == 0 {
                return Ok(());
            }
            review.projection = "current_user_only".into();
        }
        if claude {
            // 复审仅使用派生空间，认证在脱敏之后注入；不恢复或存储自由文本理由。
            payload = self
                .veil
                .session(gateway_veil::Scope::Review(request_id))
                .map_err(|_| "prompt_guard_veil_unavailable")?
                .redact(payload)
                .await
                .map_err(|_| "prompt_guard_veil_unavailable")?;
            review.redacted = true;
        }
        review.input_chars = payload.to_string().chars().count() as u32;
        assessment.review = Some(review);
        if assessment
            .review
            .as_ref()
            .expect("review initialized")
            .input_chars
            > config.max_text_chars
        {
            return Err("prompt_guard_review_input_too_large");
        }
        if claude {
            self.veil
                .ensure_ready()
                .map_err(|_| "prompt_guard_veil_unavailable")?;
        }
        let value = self.review_request(config, &payload).await?;
        if config.review_request_mode == "moderations" {
            let results = value["results"]
                .as_array()
                .filter(|values| values.len() == 1)
                .ok_or("prompt_guard_invalid_response")?;
            let scores = results[0]["category_scores"]
                .as_object()
                .ok_or("prompt_guard_invalid_response")?;
            let review = assessment.review.as_mut().expect("review initialized");
            let mut flagged = false;
            let mut maximum = 0;
            let mut selected_flagged = false;
            for (category, threshold) in &config.moderation_thresholds {
                let score = scores
                    .get(category)
                    .and_then(Value::as_f64)
                    .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                    .ok_or("prompt_guard_invalid_response")?;
                let bps = (score * 10000.0).round() as u32;
                let exceeds = score >= f64::from(*threshold) / 10000.0;
                if exceeds && !selected_flagged || exceeds == selected_flagged && bps > maximum {
                    maximum = bps;
                    review.threshold_bps = *threshold;
                    selected_flagged = exceeds;
                }
                flagged |= exceeds;
                review.category_scores.insert(category.clone(), bps);
            }
            review.result = "judged".into();
            review.confidence_bps = Some(maximum);
            review.evaluated_categories = config.moderation_thresholds.keys().cloned().collect();
            assessment.action = if assessment.terminal || flagged {
                GuardAction::Block
            } else {
                GuardAction::Allow
            };
            assessment.reason_code =
                (assessment.action == GuardAction::Block).then(|| "prompt_guard_blocked".into());
            return Ok(());
        }
        let choices = value["choices"]
            .as_array()
            .filter(|choices| choices.len() == 1)
            .ok_or("prompt_guard_invalid_response")?;
        let choice = &choices[0];
        if choice["finish_reason"] != "stop"
            || choice["message"]["refusal"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
            || choice["message"]
                .get("tool_calls")
                .is_some_and(|value| !value.is_null())
        {
            return Err("prompt_guard_invalid_response");
        }
        let decision: ReviewDecision = serde_json::from_str(
            choice["message"]["content"]
                .as_str()
                .ok_or("prompt_guard_invalid_response")?,
        )
        .map_err(|_| "prompt_guard_invalid_response")?;
        let evaluated: BTreeSet<_> = decision.evaluated_categories.iter().cloned().collect();
        if decision.policy_version != POLICY_VERSION
            || evaluated != classification.categories
            || decision.reason.len() > 8192
            || !matches!(decision.result.as_str(), "judged" | "inconclusive")
        {
            return Err("prompt_guard_invalid_response");
        }
        let review = assessment.review.as_mut().expect("review initialized");
        review.result = decision.result.clone();
        review.evaluated_categories = decision.evaluated_categories;
        if decision.result == "inconclusive" {
            return Err("prompt_guard_review_inconclusive");
        }
        let confidence = decision
            .confidence
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or("prompt_guard_invalid_response")?;
        review.confidence_bps = Some((confidence * 10000.0).round() as u32);
        assessment.action = if assessment.terminal
            || confidence >= f64::from(config.confidence_threshold_bps) / 10000.0
        {
            GuardAction::Block
        } else {
            GuardAction::Allow
        };
        assessment.reason_code =
            (assessment.action == GuardAction::Block).then(|| "prompt_guard_blocked".into());
        Ok(())
    }

    async fn review_request(
        &self,
        config: &PromptGuardConfig,
        payload: &Value,
    ) -> Result<Value, &'static str> {
        self.review_active
            .try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < config.review_max_concurrent as usize).then_some(active + 1)
            })
            .map_err(|_| "prompt_guard_review_busy")?;
        let _slot = ReviewSlot(self.review_active.clone());
        let keys = config.api_keys();
        if !config.ready() {
            return Err("prompt_guard_not_configured");
        }
        let key = keys[self.review_key_cursor.fetch_add(1, Ordering::Relaxed) % keys.len()];
        let mut response = self
            .client
            .post(&config.review_url)
            .bearer_auth(key)
            .timeout(Duration::from_secs(u64::from(config.timeout_seconds)))
            .json(payload)
            .send()
            .await
            .map_err(|_| "prompt_guard_unavailable")?;
        if response.status().as_u16() == 413 {
            return Err("prompt_guard_review_input_too_large");
        }
        if !response.status().is_success() {
            return Err("prompt_guard_unavailable");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "prompt_guard_unavailable")?
        {
            if chunk.len() > (64usize * 1024).saturating_sub(body.len()) {
                return Err("prompt_guard_invalid_response");
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| "prompt_guard_invalid_response")
    }
}

struct ReviewSlot(Arc<AtomicUsize>);
impl Drop for ReviewSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn local_decision(
    classification: &Classification,
    config: &PromptGuardConfig,
) -> PromptGuardAssessment {
    let mut assessment = classification.assessment.clone();
    assessment.mode = config.mode.clone();
    assessment.terminal = classification.terminal;
    assessment.action = if classification.terminal
        || (assessment.decision_score >= config.threshold && config.local_action == "block")
    {
        GuardAction::Block
    } else if assessment.decision_score >= config.threshold {
        GuardAction::Warn
    } else {
        GuardAction::Allow
    };
    assessment.would_action = assessment.action;
    if assessment.action == GuardAction::Block {
        assessment.reason_code = Some("prompt_guard_blocked".into());
    }
    assessment
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewDecision {
    result: String,
    policy_version: String,
    evaluated_categories: Vec<String>,
    confidence: Option<f64>,
    reason: String,
}

#[async_trait]
impl PromptGuard for HostPromptGuard {
    async fn observe_failure(&self, context: PromptGuardContext<'_>, error: &ProviderError) {
        self.capture_failure(context, error).await;
    }
    async fn observe_refusal(&self, context: PromptGuardContext<'_>) {
        self.capture_refusal(context).await;
    }
    async fn check(
        &self,
        operation: &Operation,
        context: PromptGuardContext<'_>,
        cancellation: &CancellationToken,
        deadline: Deadline,
    ) -> Result<(), ProviderError> {
        if !matches!(
            operation.kind(),
            OperationKind::Generate | OperationKind::CountTokens
        ) {
            return Ok(());
        }
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(ProviderErrorKind::Cancelled),
            _ = deadline.wait() => Err(ProviderErrorKind::Timeout),
            result = self.inspect(operation, &context) => Ok(result),
        };
        let (assessment, fingerprint, reused) = match result {
            Ok(result) => result,
            Err(kind) => {
                let mut assessment = context.state.snapshot().unwrap_or_else(pending);
                assessment.action = GuardAction::Cancelled;
                assessment.would_action = GuardAction::Cancelled;
                assessment.reason_code = Some(
                    if kind == ProviderErrorKind::Timeout {
                        "prompt_guard_deadline"
                    } else {
                        "prompt_guard_cancelled"
                    }
                    .into(),
                );
                context.state.record(assessment, None);
                return Err(
                    ProviderError::new(kind, UpstreamSendState::NotSent).with_retry_prohibited()
                );
            }
        };
        if !reused {
            context.state.record(assessment.clone(), fingerprint);
        }
        match assessment.action {
            GuardAction::Disabled | GuardAction::Allow | GuardAction::Warn => Ok(()),
            action => {
                let code = assessment
                    .reason_code
                    .as_deref()
                    .unwrap_or("prompt_guard_unavailable");
                Err(reject(
                    operation,
                    code,
                    if matches!(action, GuardAction::Block | GuardAction::CoverageError) {
                        400
                    } else {
                        503
                    },
                ))
            }
        }
    }
}

fn pending() -> PromptGuardAssessment {
    let mut assessment = PromptGuardAssessment::pending();
    assessment.policy_version = POLICY_VERSION.into();
    assessment.rules_version = VERSION.into();
    assessment.normalization_version = normalize::VERSION.into();
    assessment
}
fn elapsed(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
fn reject(operation: &Operation, code: &str, status: u16) -> ProviderError {
    let error_type = if status == 400 {
        "invalid_request_error"
    } else {
        "api_error"
    };
    let mut body = json!({"error":{"type":error_type,"code":code,"message":code}});
    if operation.protocol() == "anthropic" {
        body["type"] = json!("error");
    }
    ProviderError::new(
        if status == 503 {
            ProviderErrorKind::ProviderInfrastructureUnavailable
        } else if code == "prompt_guard_blocked" {
            ProviderErrorKind::RequestPolicyDenied
        } else {
            ProviderErrorKind::InvalidRequest
        },
        UpstreamSendState::NotSent,
    )
    .with_retry_prohibited()
    .with_diagnostic(
        ProviderDiagnostic::new(code.to_owned()).with_classification(
            "prompt_guard",
            if status == 503 {
                "review_unavailable"
            } else {
                "rejected"
            },
        ),
    )
    .with_client_visible_upstream_response(ClientVisibleUpstreamResponse::new(
        status,
        Some(b"application/json".to_vec()),
        serde_json::to_vec(&body).expect("error JSON").into(),
    ))
}
