//! Claude 原生请求、账号亲和和字节保真的 JSON／SSE 执行

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use gateway_core::{
    account::{
        AccountAffinity, AccountCandidate, AccountRuntimeSignals, AccountSelectionContext,
        AccountSelector, AccountStateChange, CredentialState, PreferredAccountSelection,
        ProviderAccount, ProviderAccountId, RotationStrategy,
    },
    concurrency::{CapacityWait, ConcurrencyWaitQueue},
    engine::{
        AttemptContext,
        provider::{Provider, ProviderCallMetadata, ProviderRequest, ProviderStream},
    },
    error::{ClientVisibleUpstreamResponse, ProviderError, ProviderErrorKind},
    event::{
        GatewayEvent, ProtocolWireEvent, ProviderEvent, ProviderResponseHeader,
        ProviderResponseObservation, ProviderResponseTimings, ResponseMeta,
    },
    operation::{Operation, OperationKind, RawJsonPayload},
    provider_ports::{
        ProviderAccountAdmission, ProviderConcurrencyPool, ProviderCooldown,
        ProviderLeaseAcquisition, ProviderLeaseGuard, ProviderLeaseRequest,
        ProviderSchedulingLeaseRequest, ProviderSessionAffinityKey, ProviderSessionClaim,
        ProviderStorePorts,
    },
    routing::{
        ModelCapabilities, ProviderCatalogGeneration, ProviderCatalogUnavailable,
        ProviderModelCapabilities, ProviderModelContent, ProviderModelDescriptor, UpstreamModelId,
    },
    upstream::{UpstreamSendState, UpstreamTransport},
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    API, MAX_BODY, NAME, OAUTH_BETA,
    credential::{CredentialError, Credentials},
    usage::UsageCapture,
};

pub(crate) struct AnthropicProvider {
    credentials: Arc<Credentials>,
    ports: ProviderStorePorts,
    queue: ConcurrencyWaitQueue<ProviderAccountId>,
    catalog_generation: AtomicU64,
    catalog_fingerprint: Mutex<Option<[u8; 32]>>,
    catalog_refresh: tokio::sync::Mutex<()>,
    catalog_cursor: AtomicU64,
}

impl AnthropicProvider {
    pub fn new(credentials: Arc<Credentials>, ports: ProviderStorePorts) -> Self {
        Self {
            credentials,
            ports,
            queue: ConcurrencyWaitQueue::default(),
            catalog_generation: AtomicU64::new(0),
            catalog_fingerprint: Mutex::new(None),
            catalog_refresh: tokio::sync::Mutex::new(()),
            catalog_cursor: AtomicU64::new(0),
        }
    }

    pub(crate) async fn refresh_catalog(&self) -> Result<(), ProviderError> {
        let _refresh = self.catalog_refresh.lock().await;
        // 给发布快照留出周期预算；已采集账号不能因其后慢账号被取消而永远无法发布。
        let refresh =
            tokio::time::timeout(Duration::from_secs(90), self.scoped_models(None, true)).await;
        self.query_model_capabilities().await?;
        refresh.map_err(|_| infra())?.map(|_| ())
    }

    async fn scoped_models(
        &self,
        scope: Option<&gateway_core::account::scope::FrozenAccountScope>,
        refresh: bool,
    ) -> Result<Vec<(ProviderAccount, AccountCatalog)>, ProviderError> {
        self.credentials
            .veil
            .control(gateway_veil::ControlPurpose::Models)
            .map_err(|_| infra())?;
        let accounts = self
            .ports
            .accounts()
            .list_for_provider(&self.credentials.kind())
            .await
            .map_err(|_| infra())?;
        let mut accounts: Vec<_> = accounts
            .into_iter()
            .filter(|account| {
                account.enabled()
                    && account.credential_state() == CredentialState::Ready
                    && scope.is_none_or(|scope| scope.allows(account.id()))
            })
            .collect();
        accounts.sort_by(|a, b| a.id().cmp(b.id()));
        let start = if refresh && !accounts.is_empty() {
            self.catalog_cursor.load(Ordering::Relaxed) as usize % accounts.len()
        } else {
            0
        };
        accounts.rotate_left(start);
        let requests = accounts
            .into_iter()
            .enumerate()
            .map(|(offset, account)| async move {
                // 周期超时后从已尝试位置继续，避免固定队首的故障账号饿死后续账号。
                if refresh {
                    self.catalog_cursor
                        .store((start + offset + 1) as u64, Ordering::Relaxed);
                }
                let mode = if refresh {
                    CatalogRead::RefreshIfMissing
                } else {
                    CatalogRead::Snapshot
                };
                let result = tokio::time::timeout(
                    Duration::from_secs(30),
                    account_models(&self.credentials, account.id(), mode),
                )
                .await;
                (account, result)
            });
        // 单账号故障只移除该账号的目录贡献；限制出站并发，不让一个坏账号阻断其他账号。
        let results = futures::stream::iter(requests)
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        let current = self
            .ports
            .accounts()
            .list_for_provider(&self.credentials.kind())
            .await
            .map_err(|_| infra())?;
        let current: BTreeMap<_, _> = current
            .into_iter()
            .map(|account| (account.id().clone(), account))
            .collect();
        self.credentials.veil.ensure_ready().map_err(|_| infra())?;
        let had_accounts = !results.is_empty();
        let result: Vec<_> = results
            .into_iter()
            .filter_map(|(account, result)| {
                let Ok(Ok(models)) = result else {
                    return None;
                };
                let latest = current.get(account.id())?;
                // 拉取期间刷新、停用或权限变更使旧快照失效，不能重新发布已撤销的目录。
                (latest.revision() == account.revision()
                    && latest.enabled()
                    && latest.credential_state() == CredentialState::Ready)
                    .then_some((account, models))
            })
            .collect();
        // 首次采集未完成与空权限集合不同；客户端发现不能同步访问上游或伪造旧模型。
        if had_accounts && result.is_empty() {
            return Err(infra());
        }
        Ok(result)
    }

    async fn select(
        &self,
        context: &AttemptContext,
        model: &str,
        session: Option<&str>,
    ) -> Result<(ProviderAccount, Box<dyn ProviderLeaseGuard>), ProviderError> {
        let kind = self.credentials.kind();
        let policy = context.account_selection_policy();
        let affinity_mode = policy.anthropic_account_affinity();
        // 宽松模式、无 session 和诊断请求不读写用户亲和关系。
        let affinity = session
            .filter(|session| {
                !session.is_empty()
                    && affinity_mode != AccountAffinity::Relaxed
                    && !context.is_diagnostic_required_account()
            })
            .map(|session| {
                ProviderSessionAffinityKey::try_new(hex::encode(Sha256::digest(
                    format!("{}\0{session}", context.client_api_key_ref().as_str()).as_bytes(),
                )))
            })
            .transpose()
            .map_err(|_| infra())?;
        let affinity_port = self.ports.session_affinity();
        let mut wait = CapacityWait::new(
            &self.queue,
            context.account_selection_policy().queue_policy(),
            context.deadline().at(),
            context.concurrency_wait_budget(),
        );
        loop {
            check_context(context, UpstreamSendState::NotSent)?;
            let binding = match &affinity {
                Some(affinity) => affinity_port
                    .load(&kind, affinity)
                    .await
                    .map_err(|_| infra())?,
                None => None,
            };
            let assignment_cursor = if policy.strategy() == RotationStrategy::RoundRobin
                && context.required_account().is_none()
                && context
                    .continuation()
                    .and_then(gateway_core::engine::continuation::ContinuationBinding::pinned)
                    .is_none()
                && !context.is_diagnostic_required_account()
                && !(binding.is_some() && affinity_mode == AccountAffinity::Preferred)
            {
                Some(
                    affinity_port
                        .round_robin_cursor(&kind, context.client_api_key_ref())
                        .await
                        .map_err(|_| infra())?,
                )
            } else {
                None
            };
            let mut accounts = self
                .ports
                .accounts()
                .list_for_provider(&kind)
                .await
                .map_err(|_| infra())?;
            // Key 的模型并集只用于发现，不能让没有该模型的账号参与实际选号。
            // 每次等待后重读可信快照，目录失效、刷新或权限变化不能沿用旧资格。
            let supported = if context.is_diagnostic_required_account() {
                None
            } else {
                Some(
                    self.scoped_models(context.account_scope().map(AsRef::as_ref), false)
                        .await?
                        .into_iter()
                        .filter(|(_, catalog)| {
                            catalog
                                .models
                                .iter()
                                .any(|item| item["id"].as_str() == Some(model))
                        })
                        .map(|(account, _)| (account.id().clone(), account.revision()))
                        .collect::<BTreeMap<_, _>>(),
                )
            };
            accounts.retain(|account| {
                context
                    .account_scope()
                    .is_none_or(|scope| scope.allows_model(account.id(), model))
                    && context
                        .required_account()
                        .or_else(|| {
                            context
                                .continuation()
                                .and_then(
                                    gateway_core::engine::continuation::ContinuationBinding::pinned,
                                )
                                .map(|pin| pin.account())
                        })
                        .is_none_or(|id| id == account.id())
                    && !context.excluded_accounts().contains(account.id())
                    && account.model_access().allows(model)
                    && supported.as_ref().is_none_or(|supported| {
                        supported.get(account.id()) == Some(&account.revision())
                    })
            });
            let ids: Vec<_> = accounts
                .iter()
                .map(|account| account.id().clone())
                .collect();
            let state = self
                .ports
                .leases()
                .load_state(
                    context.client_api_key_ref(),
                    &kind,
                    &ids,
                    ProviderConcurrencyPool::Shared,
                )
                .await
                .map_err(|_| infra())?;
            let quotas: BTreeMap<_, _> = self
                .ports
                .accounts()
                .get_quotas(&ids)
                .await
                .map_err(|_| infra())?
                .into_iter()
                .map(|quota| (quota.account_id, (quota.expected_revision, quota.quota)))
                .collect();
            let quota_now = SystemTime::now();
            let mut cooldowns = BTreeMap::new();
            if !context.is_diagnostic_required_account() {
                let port = self.ports.cooldowns();
                for batch in accounts.chunks(16) {
                    for result in futures::future::join_all(batch.iter().map(|account| async {
                        // 账号 ID 绑定用户与组织，订阅冷却跨令牌 revision 延续；刷新不能释放额度
                        let mut cooldown = port
                            .read(account.id())
                            .await
                            .map_err(|_| infra())?
                            .map(|cooldown| cooldown.scheduling_state());
                        let scope =
                            gateway_core::provider_ports::ProviderCooldownScope::upstream_model(
                                UpstreamModelId::new(model).map_err(|_| infra())?,
                            );
                        if let Some(scoped) = port
                            .read_scoped(account.id(), &scope)
                            .await
                            .map_err(|_| infra())?
                            && cooldown.is_none_or(|current| current.until < scoped.until())
                        {
                            cooldown = Some(gateway_core::account::AccountCooldown {
                                until: scoped.until(),
                                kind: gateway_core::provider_ports::ProviderCooldownKind::RateLimit,
                            });
                        }
                        Ok::<_, ProviderError>((account.id().clone(), cooldown))
                    }))
                    .await
                    {
                        let (id, cooldown) = result?;
                        cooldowns.insert(id, cooldown);
                    }
                }
            }
            let mut candidates: Vec<_> = accounts
                .into_iter()
                .map(|account| {
                    let signals = state.signals().get(account.id()).cloned().unwrap_or(
                        AccountRuntimeSignals {
                            in_flight: 0,
                            last_started_at: None,
                            quota_reset_at: None,
                            quota_reset_windows: None,
                            quota_remaining_rank: None,
                            cooldown: None,
                            failure_rate_basis_points: None,
                            first_output_latency_ms: None,
                        },
                    );
                    let health = self
                        .ports
                        .account_feedback()
                        .scheduling_signals(&kind, account.id());
                    // 刷新凭据后的旧观测不参与新账号评分，查询失败也不能凭空生成额度。
                    let quota = quotas
                        .get(account.id())
                        .filter(|(revision, _)| *revision == account.revision())
                        .and_then(|(_, quota)| crate::quota::scheduling_signals(quota, quota_now));
                    let signals = signals
                        .with_provider_quota(quota)
                        .with_rate_limit(cooldowns.get(account.id()).copied().flatten())
                        .with_runtime_health(health.0, health.1);
                    AccountCandidate {
                        account,
                        signals,
                        inherited_concurrency: None,
                    }
                })
                .collect();
            let selection = AccountSelectionContext {
                policy: context.account_selection_policy(),
                now: SystemTime::now(),
                excluded_accounts: context.excluded_accounts().clone(),
                busy_accounts: BTreeSet::new(),
                preferred_account: binding.as_ref().map(|binding| binding.account_id().clone()),
                preferred_account_overrides_weight: binding.is_some(),
                round_robin_cursor: assignment_cursor.unwrap_or_else(|| state.round_robin_cursor()),
                eligibility: context.account_eligibility(),
                account_scope: context.account_scope().cloned(),
                reserved_concurrency: 0,
            };
            let bound_eligible = binding.as_ref().is_some_and(|binding| {
                AccountSelector
                    .wait_candidates(&candidates, &selection)
                    .contains(binding.account_id())
            });
            // 严格模式等待可用主账号；优先模式允许繁忙分流。两者均在主账号失效后重绑。
            if affinity_mode == AccountAffinity::Strict && bound_eligible {
                candidates.retain(|candidate| {
                    Some(candidate.account.id()) == selection.preferred_account.as_ref()
                });
            }
            let chosen = AccountSelector.select(&candidates, &selection);
            let wait_ids = AccountSelector.wait_candidates(&candidates, &selection);
            if wait_ids.is_empty() {
                if let Some(until) = candidates
                    .iter()
                    .filter_map(|candidate| candidate.signals.cooldown)
                    .map(|cooldown| cooldown.until)
                    .filter(|until| *until > selection.now)
                    .min()
                {
                    return Err(not_sent(ProviderErrorKind::RateLimited)
                        .with_status(429)
                        .with_retry_after(
                            until
                                .duration_since(selection.now)
                                .expect("future cooldown"),
                        ));
                }
                return Err(not_sent(ProviderErrorKind::NoEligibleAccount));
            }
            if let Some(chosen) = chosen {
                let account = chosen.candidate().account.clone();
                if !wait.can_try(account.id()) {
                    tokio::select! {
                        _ = context.cancellation().cancelled() => return Err(not_sent(ProviderErrorKind::Cancelled)),
                        result = AccountSelector.wait_for_capacity(&mut wait,&wait_ids,&candidates,&selection) => result.map_err(|error| not_sent(error.provider_kind()))?,
                    }
                    continue;
                }
                let request = ProviderSchedulingLeaseRequest::new(
                    kind.clone(),
                    account.id().clone(),
                    account.revision(),
                    selection.concurrency_limit(chosen.candidate()),
                    context.deadline(),
                )
                .with_cancellation(context.cancellation().clone())
                .with_metadata(context.lease_metadata());
                if let ProviderLeaseAcquisition::Acquired(lease) = self
                    .ports
                    .leases()
                    .try_acquire(ProviderLeaseRequest::Scheduling(request))
                    .await
                    .map_err(|_| infra())?
                {
                    let claim = affinity.as_ref().map(|affinity| {
                        let target = binding
                            .as_ref()
                            .filter(|_| {
                                affinity_mode == AccountAffinity::Preferred && bound_eligible
                            })
                            .map_or(account.id(), |binding| binding.account_id());
                        ProviderSessionClaim {
                            key: affinity,
                            expected: binding.as_ref(),
                            account: target,
                            ttl: policy.anthropic_session_affinity_ttl(),
                        }
                    });
                    let admission = match assignment_cursor {
                        Some(cursor) if chosen.preferred() != PreferredAccountSelection::Hit => {
                            Some(ProviderAccountAdmission::RoundRobin {
                                client_key: context.client_api_key_ref(),
                                cursor,
                                session: claim,
                            })
                        }
                        _ => claim.map(ProviderAccountAdmission::Session),
                    };
                    // 成功租约、轮询位置与会话绑定原子认领；亲和命中和临时分流不消耗新会话位置。
                    if let Some(admission) = admission
                        && !affinity_port
                            .admit_account(&kind, admission)
                            .await
                            .map_err(|_| infra())?
                    {
                        drop(lease);
                        continue;
                    }
                    return Ok((account, lease));
                }
            }
            // 共享有界等待负责排队预算和取消后的同步退队。
            tokio::select! {
                _ = context.cancellation().cancelled() => return Err(not_sent(ProviderErrorKind::Cancelled)),
                result = AccountSelector.wait_for_capacity(&mut wait,&wait_ids,&candidates,&selection) => result.map_err(|error| not_sent(error.provider_kind()))?,
            }
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn default_request_profile(
        &self,
    ) -> Result<Option<gateway_core::account::OpaqueProviderData>, ProviderError> {
        Ok(Some(crate::profile::object(
            &self
                .credentials
                .profiles
                .resolve(crate::profile::Selection::default()),
        )))
    }
    fn resolve_request_profile(
        &self,
        configuration: &gateway_core::account::OpaqueProviderData,
    ) -> Result<gateway_core::account::OpaqueProviderData, ProviderError> {
        let selection = if let Some(fields) =
            gateway_core::settings::request_profiles::inherited_fields(configuration)
        {
            crate::profile::Selection::parse(&gateway_core::account::OpaqueProviderData::new(
                fields.clone(),
            ))
        } else {
            crate::profile::Selection::parse(configuration)
        }
        .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?;
        Ok(crate::profile::object(
            &self.credentials.profiles.resolve(selection),
        ))
    }
    fn name(&self) -> &str {
        NAME
    }
    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::new(self.catalog_generation.load(Ordering::Acquire))
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        let mut models: BTreeMap<String, BTreeSet<ProviderAccountId>> = BTreeMap::new();
        let mut digest = Sha256::new();
        // Core 重建路由快照也会调用此接口，因此这里只读缓存，绝不能隐式发起上游查询。
        let mut catalogs = self.scoped_models(None, false).await?;
        catalogs.sort_by(|a, b| a.0.id().cmp(b.0.id()));
        for (account, catalog) in catalogs {
            digest.update(account.id().as_str().as_bytes());
            digest.update(account.revision().get().to_be_bytes());
            // 能力与展示元数据改变也使目录代次失效，采集时间本身不触发重建。
            digest.update(serde_json::to_vec(&catalog.models).map_err(|_| infra())?);
            for value in catalog.models {
                let id = value["id"].as_str().ok_or_else(infra)?;
                if !account.model_access().allows(id) {
                    continue;
                }
                models
                    .entry(id.to_owned())
                    .or_default()
                    .insert(account.id().clone());
            }
        }
        // 只对经账号版本围栏复核的可用集合发布代次，失败账号不保留无期限的旧贡献。
        for (model, accounts) in &models {
            digest.update(model.as_bytes());
            digest.update([0]);
            for account in accounts {
                digest.update(account.as_str().as_bytes());
                digest.update([0]);
            }
            digest.update([0]);
        }
        let fingerprint: [u8; 32] = digest.finalize().into();
        let mut published = self
            .catalog_fingerprint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *published != Some(fingerprint) {
            *published = Some(fingerprint);
            self.catalog_generation.fetch_add(1, Ordering::Release);
        }
        models
            .into_iter()
            .map(|(id, accounts)| {
                Ok(ProviderModelCapabilities::new(
                    UpstreamModelId::new(id).map_err(|_| infra())?,
                    ModelCapabilities::new(
                        BTreeSet::from([OperationKind::Generate, OperationKind::CountTokens]),
                        None,
                    )
                    .with_upstream_feature_validation(),
                )
                .with_catalog_accounts(accounts))
            })
            .collect()
    }

    async fn query_client_model_catalog(
        &self,
        _client_api_key_id: &gateway_core::policy::ClientApiKeyId,
        scope: &gateway_core::account::scope::FrozenAccountScope,
        protocol: &str,
        _version: &str,
        _peer_route: Option<&reqwest::header::HeaderValue>,
    ) -> Result<Option<Vec<ProviderModelDescriptor>>, ProviderCatalogUnavailable> {
        // 已知不支持的协议返回空原生目录，避免 Core 为 Codex 合成 Claude 画像
        if protocol != NAME {
            return Ok(Some(vec![]));
        }
        self.credentials
            .veil
            .control(gateway_veil::ControlPurpose::Models)
            .map_err(|_| ProviderCatalogUnavailable)?;
        let mut result = BTreeMap::new();
        for (account, catalog) in self
            .scoped_models(Some(scope), false)
            .await
            .map_err(|_| ProviderCatalogUnavailable)?
        {
            for model in catalog.models {
                let id = model["id"]
                    .as_str()
                    .ok_or(ProviderCatalogUnavailable)?
                    .to_owned();
                if account.model_access().allows(&id) && scope.allows_model(account.id(), &id) {
                    result.entry(id.clone()).or_insert(ProviderModelDescriptor {
                        model: UpstreamModelId::new(id).map_err(|_| ProviderCatalogUnavailable)?,
                        content: ProviderModelContent::Native(
                            RawJsonPayload::new(NAME, Bytes::from(model.to_string()))
                                .map_err(|_| ProviderCatalogUnavailable)?,
                        ),
                    });
                }
            }
        }
        Ok(Some(result.into_values().collect()))
    }

    async fn execute(
        self: Arc<Self>,
        request: ProviderRequest,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        let ready = self.credentials.veil.ensure_ready();
        phase(&context, "anthropic.privacy_ready", 0, ready.is_ok());
        ready.map_err(|error| veil_error(error, UpstreamSendState::NotSent))?;
        if request.candidate().provider().as_str() != NAME {
            return Err(not_sent(ProviderErrorKind::Unsupported));
        }
        let model = request
            .candidate()
            .upstream_model()
            .ok_or_else(|| not_sent(ProviderErrorKind::InvalidRequest))?
            .clone();
        let (protocol_context, endpoint) = match request.operation() {
            Operation::Generate(generate) if generate.protocol_payload().protocol() == NAME => (
                generate.protocol_payload().context().clone(),
                "/v1/messages",
            ),
            Operation::CountTokens(count) if count.payload().protocol() == NAME => (
                count.payload().context().clone(),
                "/v1/messages/count_tokens",
            ),
            _ => return Err(not_sent(ProviderErrorKind::Unsupported)),
        };
        let session = protocol_context.get("session_id").and_then(Value::as_str);
        let mut routed_body: Value = serde_json::from_slice(
            &request
                .operation()
                .wire_body()
                .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
        )
        .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?;
        let stream_requested = request_stream(&routed_body, endpoint.ends_with("count_tokens"))?;
        // 路由映射先于 attempt 生效；此后的 model 是保留字段，不能静默覆盖中间件的选择。
        routed_body["model"] = Value::String(model.as_str().to_owned());
        let routed_operation = request
            .operation()
            .clone()
            .replace_wire_payload(
                NAME,
                Bytes::from(
                    serde_json::to_vec(&routed_body)
                        .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
                ),
            )
            .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?;
        let original_thread = match request.operation() {
            Operation::Generate(generate) => {
                crate::thread::request(&Value::Object(generate.protocol_payload().body().clone()))?
            }
            _ => None,
        };
        let saved_thread = match request.operation() {
            Operation::Generate(generate) => generate.provider_session_state(NAME).cloned(),
            _ => None,
        };
        let effective: crate::profile::Effective = context
            .request_profile()
            .and_then(|profile| {
                serde_json::from_value(Value::Object(profile.expose_to_provider().clone())).ok()
            })
            .ok_or_else(infra)?;
        let selection_started = std::time::Instant::now();
        let selected = self.select(&context, model.as_str(), session).await;
        phase(
            &context,
            "anthropic.selection",
            milliseconds(selection_started),
            selected.is_ok(),
        );
        let (account, lease) = selected?;
        let provider = self.clone();
        let account_id = account.id().clone();
        // 通用错误保护按本次选号的凭据版本建立反馈围栏，账号展示快照不包含该版本。
        let metadata = ProviderCallMetadata::new(
            self.credentials.kind(),
            model.clone(),
            account_id.clone(),
            transport(),
        )
        .with_credential_revision(account.revision())
        .with_account_snapshot(Some(
            gateway_core::account::AccountHistorySnapshot::from_account(&account),
        ));
        let execution_context = context.clone();
        // 已恢复字面量不能再次解释为映射引用，响应侧只允许原帧透传，不做二次恢复。
        // 发送前复核路由后的业务正文和会话约束。
        let guard_operation = routed_operation.clone();
        let (mut body, protocol_context) = match routed_operation {
            Operation::Generate(generate)
                if endpoint == "/v1/messages" && generate.protocol_payload().protocol() == NAME =>
            {
                (
                    Value::Object(generate.protocol_payload().body().clone()),
                    generate.protocol_payload().context().clone(),
                )
            }
            Operation::CountTokens(count)
                if endpoint.ends_with("count_tokens") && count.payload().protocol() == NAME =>
            {
                (
                    serde_json::from_slice(count.payload().body())
                        .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
                    count.payload().context().clone(),
                )
            }
            _ => return Err(not_sent(ProviderErrorKind::Unsupported)),
        };
        // 下游 JSON/SSE 已在入口冻结，必须在发送前拒绝 attempt 改变交付协议。
        if request_stream(&body, endpoint.ends_with("count_tokens"))? != stream_requested {
            return Err(request_contract_error("cprs_stream_changed"));
        }
        if body["model"].as_str() != Some(model.as_str()) {
            return Err(request_contract_error("cprs_routed_model_changed"));
        }
        if crate::thread::request(&body)? != original_thread {
            return Err(crate::thread::error("cprs_thread_handle_changed"));
        }
        let mut protocol_headers = protocol_headers(&protocol_context)?;
        let identity_started = std::time::Instant::now();
        let mut effective = effective;
        effective
            .follow(&protocol_headers)
            .map_err(|_| crate::thread::error("cprs_identity_invalid"))?;
        let veil_fingerprint = provider
            .credentials
            .veil
            .state_fingerprint()
            .ok_or_else(infra)?;
        let restored = if let Some(Some(previous)) = &original_thread {
            // 增量线程冻结实际模型；映射修改不能将同一线程静默转到另一个模型。
            // 旧记录没有模型证据时要求重建完整会话。
            if saved_thread
                .as_ref()
                .and_then(|state| state.payload().get("model"))
                .and_then(Value::as_str)
                != Some(model.as_str())
            {
                return Err(crate::thread::error("cprs_thread_model_changed"));
            }
            Some(crate::thread::Checkpoint::restore(
                saved_thread.as_ref(),
                previous,
                &account,
                &mut effective,
                &mut protocol_headers,
                veil_fingerprint,
            )?)
        } else {
            None
        };
        let explicit_session = protocol_context.get("session_id").and_then(Value::as_str);
        let identity = if endpoint == "/v1/messages" {
            Some(crate::thread::identity(
                &mut body,
                &effective,
                execution_context.client_api_key_ref().as_str(),
                &account,
                explicit_session,
                restored,
            )?)
        } else {
            None
        };
        let version = protocol_headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("2023-06-01")
            .to_owned();
        let mut betas = protocol_headers
            .get("anthropic-beta")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for beta in std::iter::once(OAUTH_BETA)
            .chain(original_thread.is_some().then_some(crate::thread::BETA))
        {
            if !betas.iter().any(|value| value == beta) {
                betas.push(beta.into());
            }
        }
        if original_thread.is_some() {
            crate::thread::validate_features(&body, &betas)?;
        }
        protocol_headers.insert(
            "anthropic-version",
            reqwest::header::HeaderValue::from_str(&version)
                .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
        );
        protocol_headers.insert(
            "anthropic-beta",
            reqwest::header::HeaderValue::from_str(&betas.join(","))
                .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
        );
        if let Some(identity) = &identity {
            protocol_headers.insert(
                "x-claude-code-session-id",
                reqwest::header::HeaderValue::from_str(&identity.session)
                    .expect("validated session"),
            );
        }
        // Only protocol facts enter diagnostics; never serialize the selection or its installation seed.
        execution_context.trace().record_evidence("anthropic.identity", serde_json::json!({
        "durationMs": milliseconds(identity_started), "succeeded": true,
        "clientProfile": {
            "mode": effective.selection.mode, "version": effective.version, "userAgent": effective.user_agent,
            "entry": effective.entry, "platform": effective.os, "arch": effective.arch,
            "sdkVersion": effective.sdk_version, "runtimeVersion": effective.runtime_version,
            "templateRevision": effective.template, "versionSource": effective.version_source,
            "verifiedAt": effective.verified_at,
        },
    }));
        let checkpoint = identity
            .as_ref()
            .filter(|_| original_thread.is_some())
            .map(|identity| {
                crate::thread::Checkpoint::new(
                    &account,
                    effective.clone(),
                    &protocol_headers,
                    veil_fingerprint,
                    identity,
                )
                .with_model(model.as_str())
            });
        // 账号（含出站代理）的位置优先；只改原生搜索工具，不向对话添加环境标签。
        let apply_location = |body: &mut Value| {
            if let Some(location) = account
                .request_location()
                .or(execution_context.request_location())
                && let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut)
            {
                for tool in tools {
                    if let Some(tool) = tool.as_object_mut()
                        && tool
                            .get("type")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| kind.starts_with("web_search_"))
                        && tool.get("name").and_then(Value::as_str) == Some("web_search")
                    {
                        tool.insert("user_location".into(), serde_json::json!({
                            "type":"approximate", "country":location.country, "region":location.region,
                            "city":location.city, "timezone":location.timezone.name(),
                        }));
                    }
                }
            }
        };
        apply_location(&mut body);
        let signed =
            crate::signing::prepare(&mut body, &effective, endpoint.ends_with("count_tokens"))
                .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?;
        // 完整身份与协议结构已经组装；Guard 和 veil 之后只更新派生校验值。
        let encoded =
            serde_json::to_vec(&body).map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?;
        if encoded.len() > MAX_BODY {
            return Err(not_sent(ProviderErrorKind::MessageTooBig));
        }
        let final_operation = guard_operation
            .replace_wire_payload(NAME, Bytes::copy_from_slice(&encoded))
            .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?;
        let guard_started = std::time::Instant::now();
        let guarded = execution_context.check_prompt_guard(&final_operation).await;
        let assessment = execution_context.prompt_guard_assessment();
        execution_context.trace().record_evidence("anthropic.prompt_guard", serde_json::json!({
        "durationMs": milliseconds(guard_started), "succeeded": guarded.is_ok(),
        "guard": assessment.map(|assessment| serde_json::json!({
            "mode": assessment.mode, "action": assessment.action, "wouldAction": assessment.would_action,
        })),
    }));
        guarded?;
        let privacy = provider
            .credentials
            .veil
            .session(if execution_context.is_diagnostic_required_account() {
                gateway_veil::Scope::Diagnostic(execution_context.request_id().as_str())
            } else {
                gateway_veil::Scope::ClientKey(execution_context.client_api_key_ref().as_str())
            })
            .map_err(|error| veil_error(error, UpstreamSendState::NotSent))?;
        let redaction_started = std::time::Instant::now();
        let redacted = privacy.redact(body).await;
        phase(
            &execution_context,
            "anthropic.redaction",
            milliseconds(redaction_started),
            redacted.is_ok(),
        );
        let mut body = redacted.map_err(|error| veil_error(error, UpstreamSendState::NotSent))?;
        // Guard 已检查这些管理员配置值；脱敏后恢复同一快照，避免位置被当作用户文本替换。
        // 不恢复任何客户端提供的原始字段。
        apply_location(&mut body);
        let encoded = crate::signing::encode(&mut body, &effective, signed)
            .map_err(|_| not_sent(ProviderErrorKind::Protocol))?;
        if encoded.len() > MAX_BODY {
            return Err(not_sent(ProviderErrorKind::MessageTooBig));
        }
        let feedback = provider.ports.account_feedback();
        let events = async_stream::try_stream! {
            check_context(&execution_context, UpstreamSendState::NotSent)?;
            let credential_started = std::time::Instant::now();
            let credential = provider.credentials.load(account.id()).await;
            phase(&execution_context, "anthropic.credential", milliseconds(credential_started), credential.is_ok());
            let (loaded, tokens) = credential.map_err(|_| infra())?;
            // 租约与保护反馈绑定选号版本；刷新后的新凭据必须重新选号，不能沿用旧围栏。
            if loaded.account.revision() != account.revision() || !loaded.account.enabled() || loaded.account.credential_state() != CredentialState::Ready {
                Err(not_sent(ProviderErrorKind::NoEligibleAccount))?;
            }
            let client = provider.credentials.transport.client(loaded.account.outbound_proxy()).map_err(|_| infra())?;
            let query = protocol_context.get("raw_query").and_then(Value::as_str);
            let url = match query { Some(query) => format!("{API}{endpoint}?{query}"), None => format!("{API}{endpoint}") };
            let mut builder = client.post(url).bearer_auth(&tokens.access_token)
                .header("content-type", "application/json").header("accept-encoding", "identity");
            let remaining = execution_context.deadline().remaining();
            builder = builder.headers(effective.headers(protocol_headers, remaining, endpoint.ends_with("count_tokens"), u32::from(execution_context.credential_recovery_attempted())));
            if let Some(remaining) = remaining { builder = builder.timeout(remaining); }
            let upstream_started = std::time::Instant::now();
            let response = tokio::select! {
                _ = execution_context.cancellation().cancelled() => Err(sent(ProviderErrorKind::Cancelled)),
                _ = deadline_elapsed(&execution_context) => Err(sent(ProviderErrorKind::Timeout)),
                response = provider.credentials.transport.send(builder.body(encoded), crate::transport::Purpose::Inference) => response.map_err(|error| match error {
                    crate::transport::SendError::Privacy(error) => veil_error(error, UpstreamSendState::NotSent),
                    crate::transport::SendError::Http(error) => ProviderError::new(ProviderErrorKind::Transport,
                        if error.is_connect() || error.is_builder() { UpstreamSendState::NotSent } else { UpstreamSendState::Ambiguous }).with_retry_prohibited(),
                }),
            };
            phase(&execution_context, "anthropic.upstream", milliseconds(upstream_started), response.as_ref().is_ok_and(|response| response.status().is_success()));
            let mut response = response?;
            // token 计数不是生成流量，其响应头不能覆盖账号页上的生成额度观测
            if endpoint == "/v1/messages" && crate::quota::observe_headers(&provider.credentials, &loaded.account, response.headers()).await.is_err() {
                execution_context.trace().record("anthropic.quota_header_observation_failed",serde_json::json!({}));
            }
            let status = response.status().as_u16();
            let headers = safe_headers(response.headers());
            let mut timings = ProviderResponseTimings { headers_ms:Some(elapsed_ms(&execution_context)), ..ProviderResponseTimings::default() };
            let observation = ProviderResponseObservation::new(transport()).with_status_code(status).with_client_headers(headers.clone()).with_timings(timings);
            if !response.status().is_success() {
                let content_type = response.headers().get("content-type").map(|value| value.as_bytes().to_vec());
                let bytes = read_body(&mut response, &execution_context).await?;
                let scope = crate::quota::error_scope(status, response.headers(), &bytes);
                let raw_error = serde_json::from_slice::<Value>(&bytes).ok();
                let raw_fields = gateway_core::upstream::UpstreamFailureFields { status:Some(status),
                    code:raw_error.as_ref().and_then(|value|value["error"]["code"].as_str()).map(str::to_owned),
                    error_type:raw_error.as_ref().and_then(|value|value["error"]["type"].as_str()).map(str::to_owned),
                    message:raw_error.as_ref().and_then(|value|value["error"]["message"].as_str()).map(str::to_owned) };
                let policy_rejection = gateway_core::engine::prompt_guard::upstream_policy_code_from_fields(&raw_fields).is_some();
                // 明确的内容拒绝不参与账号冷却或 OAuth 恢复。
                let rejection = (!policy_rejection && status == 429 && scope != crate::quota::ErrorScope::Request).then(|| crate::quota::rejection(response.headers())).flatten();
                let bytes = restore_error_body(&privacy, bytes).await?;
                if !execution_context.is_diagnostic_required_account() && let Some(rejection) = &rejection {
                    let port = provider.ports.cooldowns();
                    let result = if scope == crate::quota::ErrorScope::Model {
                        port.put_scoped_if_later(gateway_core::provider_ports::ProviderScopedCooldown::new(account.id().clone(), loaded.account.revision(),
                            gateway_core::provider_ports::ProviderCooldownScope::upstream_model(model.clone()), rejection.until)).await
                    } else { port.put_if_later(ProviderCooldown::new(account.id().clone(), loaded.account.revision(), rejection.until)).await };
                    if result.is_err() {
                        // 观测存储失败不能改写客户端赖以恢复的原始上游错误
                        execution_context.trace().record("anthropic.cooldown_failed",serde_json::json!({"status":429}));
                    }
                }
                if !policy_rejection
                    && execution_context.account_eligibility().allows_account_state_mutation(loaded.account.enabled())
                    && let Some(credential_state) = rejected_credential_state(&raw_fields) {
                    // 只停用明确失效的当前代凭据，迟到的拒绝不能覆盖已刷新或重新授权的账号。
                    if provider.ports.accounts().apply_state_change(AccountStateChange {
                        account_id: account.id().clone(), expected_revision: loaded.account.revision(),
                        credential_state, observed_at: SystemTime::now(), error_reason: credential_state.error_reason(),
                        message: raw_fields.message.clone(),
                    }).await.is_err() {
                        // 状态写入失败仍交付原始拒绝，不能掩盖客户端需要处理的认证错误。
                        execution_context.trace().record("anthropic.credential_state_update_failed", serde_json::json!({"status":status}));
                    }
                }
                let mut error = sent(match status { 401 => ProviderErrorKind::Unauthorized, 403 => ProviderErrorKind::PermissionDenied, 429 => ProviderErrorKind::RateLimited, _ => ProviderErrorKind::Unavailable })
                    .with_status(status).with_upstream_failure_fields(raw_fields.clone()).with_client_visible_upstream_response(ClientVisibleUpstreamResponse::new(status, content_type.clone(), bytes.clone()).with_headers(headers.clone()));
                if let Some(rejection) = &rejection { error = error.with_retry_after(rejection.until.duration_since(SystemTime::now()).unwrap_or_default()); }
                error = error.with_diagnostic(gateway_core::error::ProviderDiagnostic::new("Anthropic upstream refusal")
                    .with_classification("anthropic_upstream", scope.code()));
                // 仅明确的过期 401 允许一次同账号恢复，发送后不能内部隐藏重放
                let expired = serde_json::from_slice::<Value>(&bytes).ok().and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
                    .is_some_and(|message| message.to_ascii_lowercase().contains("expired"));
                // 冻结恢复探测只观察原始拒绝，不能通过刷新提前解除冻结
                if !policy_rejection && status == 401 && expired && !execution_context.credential_recovery_attempted()
                    && execution_context.account_eligibility().allows_account_state_mutation(loaded.account.enabled()) {
                    // 目录按凭据 revision 隔离；恢复需先补齐新代次快照，不能带着旧资格重试
                    let recovery = async {
                        provider.credentials.recover_expired(&loaded).await?;
                        account_models(&provider.credentials, account.id(), CatalogRead::RefreshIfMissing).await
                    };
                    let recovered = tokio::select! {
                        _ = execution_context.cancellation().cancelled() => Err(sent(ProviderErrorKind::Cancelled)),
                        _ = deadline_elapsed(&execution_context) => Err(sent(ProviderErrorKind::Timeout)),
                        result = recovery => Ok(result.is_ok()),
                    }?;
                    if recovered {
                        error = ProviderError::new(ProviderErrorKind::Unauthorized, UpstreamSendState::Sent).with_status(status)
                            .with_upstream_failure_fields(raw_fields)
                            .with_diagnostic(gateway_core::error::ProviderDiagnostic::new("Anthropic OAuth expired").with_classification("anthropic_upstream", "account"))
                            .with_same_account_retry().with_credential_recovery().with_replay_safe().with_client_visible_upstream_response(ClientVisibleUpstreamResponse::new(status, content_type, bytes).with_headers(headers));
                    }
                }
                Err(error)?;
            }
            yield ProviderEvent::observation(observation);
            let mut meta = ResponseMeta::new(execution_context.request_id().to_string(), model.as_str());
            let mut usage = UsageCapture::default();
            if !stream_requested {
                let bytes = read_body(&mut response, &execution_context).await?;
                let value: Value = serde_json::from_slice(&bytes).map_err(|_| sent(ProviderErrorKind::Protocol))?;
                if endpoint == "/v1/messages" && value["stop_reason"] == "refusal" {
                    execution_context.observe_prompt_refusal().await;
                }
                usage.observe(&value["usage"]);
                let restoration_started = std::time::Instant::now();
                let restored = privacy.restore(value).await;
                phase(&execution_context, "anthropic.restoration", milliseconds(restoration_started), restored.is_ok());
                let value = restored.map_err(|error| veil_error(error, UpstreamSendState::Sent)
                    .with_atomic_client_events(usage.finish(model.as_str(), execution_context.pricing(), &body, false)))?;
                if endpoint == "/v1/messages" {
                    let id = value["id"].as_str().filter(|id| crate::thread::message_id(id)).ok_or_else(|| sent(ProviderErrorKind::Protocol))?;
                    meta = ResponseMeta::new(id, model.as_str());
                }
                let bytes = Bytes::from(serde_json::to_vec(&value).map_err(|_| sent(ProviderErrorKind::Protocol))?);
                // Started、计量和完成与可交付 JSON 同批发布，恢复失败不能提前跨过提交屏障。
                let mut canonical = vec![GatewayEvent::Started(meta.clone())];
                for event in usage.finish(model.as_str(), execution_context.pricing(), &body, true) {
                    canonical.extend_from_slice(event.canonical_facts());
                    if let Some(observation) = event.response_observation() { yield ProviderEvent::observation(observation.clone()); }
                }
                if let Some(content) = value["content"].as_array() {
                    for (index, block) in content.iter().enumerate() {
                        if block["type"] == "text" && let Some(text) = block["text"].as_str() {
                            canonical.push(GatewayEvent::TextDelta(gateway_core::event::TextDelta { content_index: index as u32, text: text.to_owned() }));
                        }
                    }
                }
                canonical.push(GatewayEvent::Completed(meta.clone()));
                let mut event = ProviderEvent::canonical_with_wire(canonical, ProtocolWireEvent::raw_json(NAME, bytes).expect("valid protocol"));
                if let Some(checkpoint) = &checkpoint && crate::thread::eligible(&value) {
                    event.attach_session_update(checkpoint.event(meta.response_id()));
                }
                yield event;
            } else {
                let mut restorer = privacy.stream().map_err(|error| veil_error(error, UpstreamSendState::Sent))?;
                let mut buffer = BytesMut::new();
                let mut stopped = false;
                let mut started = false;
                let mut restoration_ms = 0u64;
                let mut thread_reply = serde_json::json!({});
                loop {
                    let chunk = next_chunk(&mut response, &execution_context).await?;
                    let Some(chunk) = chunk else { break; };
                    if buffer.len().saturating_add(chunk.len()) > MAX_BODY { Err(sent(ProviderErrorKind::MessageTooBig))?; }
                    buffer.extend_from_slice(&chunk);
                    while let Some(end) = event_end(&buffer) {
                        let raw = buffer.split_to(end).freeze();
                        let value = event_json(&raw)?;
                        if let Some(value) = &value {
                            if value["type"] == "message_start" {
                                let id = value["message"]["id"].as_str().filter(|id| crate::thread::message_id(id)).ok_or_else(|| sent(ProviderErrorKind::Protocol))?;
                                meta = ResponseMeta::new(id, model.as_str());
                                thread_reply = serde_json::json!({"stop_reason":value["message"]["stop_reason"],"usage":{"iterations":value["message"]["usage"]["iterations"]}});
                                if value["message"]["stop_reason"] == "refusal" { execution_context.observe_prompt_refusal().await; }
                            }
                            if value["type"] == "message_delta" {
                                if let Some(reason) = value["delta"].get("stop_reason") { thread_reply["stop_reason"] = reason.clone(); }
                                if value["delta"]["stop_reason"] == "refusal" { execution_context.observe_prompt_refusal().await; }
                                if let Some(iterations) = value["usage"].get("iterations") { thread_reply["usage"]["iterations"] = iterations.clone(); }
                            }
                            usage.observe(&value["message"]["usage"]); usage.observe(&value["usage"]);
                            // 已启动后及时记录部分用量；首事件恢复失败则随错误附带事实，不能先发 Usage 后发 Started。
                            if started && (value["message"]["usage"].is_object() || value["usage"].is_object()) {
                                for event in usage.finish(model.as_str(), execution_context.pricing(), &body, false) { yield event; }
                            }
                            let previous = timings;
                            let elapsed = elapsed_ms(&execution_context);
                            timings.first_event_ms.get_or_insert(elapsed);
                            match value["delta"]["type"].as_str() {
                                Some("text_delta") if value["delta"]["text"].as_str().is_some_and(|text|!text.is_empty()) => { timings.first_text_ms.get_or_insert(elapsed); timings.first_token_ms.get_or_insert(elapsed); },
                                Some("thinking_delta") if value["delta"]["thinking"].as_str().is_some_and(|text|!text.is_empty()) => { timings.first_reasoning_ms.get_or_insert(elapsed); timings.first_token_ms.get_or_insert(elapsed); },
                                Some("input_json_delta") if value["delta"]["partial_json"].as_str().is_some_and(|text|!text.is_empty()) => { timings.first_token_ms.get_or_insert(elapsed); },
                                _ => {},
                            }
                            if timings != previous { yield ProviderEvent::observation(ProviderResponseObservation::new(transport()).with_timings(timings)); }
                        }
                        let event = std::str::from_utf8(&raw).map_err(|_| sent(ProviderErrorKind::Protocol))?.to_owned();
                        let restoration_started = std::time::Instant::now();
                        let restored = restorer.push(event).await;
                        restoration_ms = restoration_ms.saturating_add(milliseconds(restoration_started));
                        if restored.is_err() { phase(&execution_context, "anthropic.restoration", restoration_ms, false); }
                        let restored = restored.map_err(|error| veil_error(error, UpstreamSendState::Sent)
                            .with_atomic_client_events(usage.finish(model.as_str(), execution_context.pricing(), &body, false)))?;
                        let raw = Bytes::from(restored);
                        if let Some(value) = &value {
                            if value["type"] == "error" {
                                let (kind, scope) = match value["error"]["type"].as_str() {
                                    Some("invalid_request_error" | "permission_error" | "not_found_error") => (ProviderErrorKind::InvalidRequest, "request"),
                                    Some("rate_limit_error") => (ProviderErrorKind::RateLimited, "model"),
                                    Some("authentication_error") => (ProviderErrorKind::Unauthorized, "account"),
                                    _ => (ProviderErrorKind::Unavailable, "account"),
                                };
                                Err(sent(kind).with_diagnostic(gateway_core::error::ProviderDiagnostic::new("Anthropic SSE error").with_classification("anthropic_upstream", scope))
                                    .with_upstream_failure_fields(gateway_core::upstream::UpstreamFailureFields {
                                        status: Some(status), code: value["error"]["code"].as_str().map(str::to_owned),
                                        error_type: value["error"]["type"].as_str().map(str::to_owned),
                                        message: value["error"]["message"].as_str().map(str::to_owned),
                                    })
                                    .with_atomic_client_events(vec![ProviderEvent::wire(ProtocolWireEvent::raw_sse(NAME, raw.clone()).expect("protocol"))]))?;
                            }
                            if value["type"] == "message_stop" {
                                if !started { Err(sent(ProviderErrorKind::Protocol))?; }
                                let restoration_started = std::time::Instant::now();
                                let tail = restorer.finish().await;
                                restoration_ms = restoration_ms.saturating_add(milliseconds(restoration_started));
                                phase(&execution_context, "anthropic.restoration", restoration_ms, tail.is_ok());
                                let tail = tail.map_err(|error| veil_error(error, UpstreamSendState::Sent))?;
                                if !tail.is_empty() { Err(sent(ProviderErrorKind::Protocol))?; }
                                for event in usage.finish(model.as_str(), execution_context.pricing(), &body, true) { yield event; }
                                let mut event = ProviderEvent::canonical_with_wire(vec![GatewayEvent::Completed(meta.clone())], ProtocolWireEvent::raw_sse(NAME, raw).expect("protocol"));
                                if let Some(checkpoint) = &checkpoint && crate::thread::eligible(&thread_reply) {
                                    event.attach_session_update(checkpoint.event(meta.response_id()));
                                }
                                yield event;
                                stopped = true; break;
                            }
                        }
                        if raw.is_empty() { continue; }
                        if !started && value.as_ref().is_none_or(|value| value["type"] != "message_start") {
                            if value.as_ref().is_some_and(|value| value["type"] != "ping") { Err(sent(ProviderErrorKind::Protocol))?; }
                            yield ProviderEvent::wire(ProtocolWireEvent::raw_sse(NAME, raw).expect("protocol")).with_replayable_prelude();
                            continue;
                        }
                        let mut canonical = restored_text_events(&raw)?;
                        let wire = ProtocolWireEvent::raw_sse(NAME, raw).expect("protocol");
                        // Started 与首个 wire 同批交付，不能让协议层在无字节时跨过提交屏障
                        if !started {
                            started = true;
                            canonical.insert(0, GatewayEvent::Started(meta.clone()));
                            for event in usage.finish(model.as_str(), execution_context.pricing(), &body, false) {
                                canonical.extend_from_slice(event.canonical_facts());
                                if let Some(observation) = event.response_observation() { yield ProviderEvent::observation(observation.clone()); }
                            }
                        }
                        yield ProviderEvent::canonical_with_wire(canonical, wire);
                    }
                    if stopped { break; }
                    if buffer.len() > MAX_BODY { Err(sent(ProviderErrorKind::MessageTooBig))?; }
                }
                if !stopped { Err(sent(ProviderErrorKind::Protocol))?; }
            }
        };
        Ok(ProviderStream::new(metadata, events, lease)
            .with_filtered_account_feedback(feedback, score_account_failure))
    }
}

pub(crate) enum CatalogRead {
    Snapshot,
    RefreshIfMissing,
    Refresh,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountCatalog {
    #[serde(rename = "data")]
    pub models: Vec<Value>,
    pub observed_at: chrono::DateTime<chrono::Utc>,
}

pub(crate) async fn account_models(
    credentials: &Credentials,
    id: &ProviderAccountId,
    mode: CatalogRead,
) -> Result<AccountCatalog, CredentialError> {
    use gateway_core::{
        account::OpaqueProviderData,
        provider_ports::{ProviderCatalogCacheKey, ProviderCatalogScope},
    };
    credentials
        .veil
        .control(gateway_veil::ControlPurpose::Models)
        .map_err(|_| CredentialError::Unavailable)?;
    let (loaded, tokens) = credentials.load(id).await?;
    if loaded.account.credential_state() != CredentialState::Ready
        || tokens
            .expiry()
            .is_some_and(|expiry| expiry <= SystemTime::now())
    {
        return Err(CredentialError::Invalid);
    }
    let key = ProviderCatalogCacheKey::new(
        credentials.kind(),
        ProviderCatalogScope::new(format!("models:{}:{}", id, loaded.account.revision().get()))
            .map_err(|_| CredentialError::Invalid)?,
    );
    let cache = credentials.ports.catalog_cache();
    if !matches!(mode, CatalogRead::Refresh)
        && let Some(cached) = cache
            .read(&key)
            .await
            .map_err(|_| CredentialError::Unavailable)?
    {
        return serde_json::from_value(Value::Object(cached.expose_to_provider().clone()))
            .map_err(|_| CredentialError::Invalid);
    }
    // 原生目录和管理快照读取不会发起上游 HTTP，后台任务与显式刷新负责填充。
    if matches!(mode, CatalogRead::Snapshot) {
        return Err(CredentialError::Unavailable);
    }
    let client = credentials
        .transport
        .client(loaded.account.outbound_proxy())?;
    let mut models = Vec::new();
    let mut cursor = None;
    // 分页和总量均有界；不完整目录不能当作完整模型能力发布
    for _ in 0..10 {
        let mut request = client
            .get(format!("{API}/v1/models"))
            .query(&[("limit", "1000")])
            .headers(credentials.control_headers())
            .bearer_auth(&tokens.access_token)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", OAUTH_BETA)
            .timeout(Duration::from_secs(30));
        if let Some(cursor) = &cursor {
            request = request.query(&[("after_id", cursor)]);
        }
        let response = credentials
            .transport
            .send(
                request,
                crate::transport::Purpose::Control(gateway_veil::ControlPurpose::Models),
            )
            .await
            .map_err(|_| CredentialError::Unavailable)?;
        let value = crate::credential::read_json(response).await?;
        let data = value["data"].as_array().ok_or(CredentialError::Invalid)?;
        if data.len() > 1000
            || data.iter().any(|model| {
                model["id"]
                    .as_str()
                    .is_none_or(|id| UpstreamModelId::new(id).is_err())
            })
        {
            return Err(CredentialError::Invalid);
        }
        models.extend(data.iter().cloned());
        if value["has_more"].as_bool() == Some(false) {
            let (current, _) = credentials.load(id).await?;
            if current.account.revision() != loaded.account.revision() {
                return Err(CredentialError::Busy);
            }
            let catalog = AccountCatalog {
                models,
                observed_at: chrono::Utc::now(),
            };
            let value = serde_json::to_value(&catalog).map_err(|_| CredentialError::Invalid)?;
            cache
                .replace(
                    &key,
                    &OpaqueProviderData::new(value.as_object().expect("catalog object").clone()),
                    Duration::from_secs(300),
                )
                .await
                .map_err(|_| CredentialError::Unavailable)?;
            return Ok(catalog);
        }
        let next = value["last_id"]
            .as_str()
            .filter(|_| !data.is_empty())
            .ok_or(CredentialError::Invalid)?
            .to_owned();
        if cursor.as_ref() == Some(&next) {
            return Err(CredentialError::Invalid);
        }
        cursor = Some(next);
    }
    Err(CredentialError::Unavailable)
}

fn transport() -> UpstreamTransport {
    UpstreamTransport::new("http_sse").expect("transport")
}

fn request_stream(body: &Value, count_tokens: bool) -> Result<bool, ProviderError> {
    let stream = match body.get("stream") {
        None => false,
        Some(Value::Bool(stream)) => *stream,
        _ => return Err(not_sent(ProviderErrorKind::InvalidRequest)),
    };
    if count_tokens && (stream || body.get("thread").is_some()) {
        return Err(not_sent(ProviderErrorKind::InvalidRequest));
    }
    Ok(stream)
}

fn request_contract_error(code: &'static str) -> ProviderError {
    not_sent(ProviderErrorKind::InvalidRequest)
        .with_retry_prohibited()
        .with_diagnostic(
            gateway_core::error::ProviderDiagnostic::new(
                "The request model and stream mode must match the routed execution.",
            )
            .with_classification("anthropic_local", code),
        )
}

fn protocol_headers(
    context: &serde_json::Map<String, Value>,
) -> Result<reqwest::header::HeaderMap, ProviderError> {
    fn allowed(name: &str) -> bool {
        name.starts_with("anthropic-")
            || name.starts_with("x-claude-code-")
            || name.starts_with("x-stainless-")
            || matches!(name, "x-app" | "user-agent")
    }
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in context {
        if allowed(name)
            && let Some(value) = value.as_str()
        {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
                reqwest::header::HeaderValue::from_str(value)
                    .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
            );
        }
    }
    let betas = headers
        .get_all("anthropic-beta")
        .iter()
        .map(|value| value.to_str().map(str::to_owned))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?
        .join(",");
    if !betas.is_empty() {
        headers.insert(
            "anthropic-beta",
            reqwest::header::HeaderValue::from_str(&betas)
                .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
        );
    }
    Ok(headers)
}

fn rejected_credential_state(
    fields: &gateway_core::upstream::UpstreamFailureFields,
) -> Option<CredentialState> {
    let message = fields
        .message
        .as_deref()
        .map(|message| message.trim().to_ascii_lowercase());
    let sentence = message.as_deref().map(|message| {
        message
            .split_once('.')
            .map_or(message, |(first, _)| first)
            .trim()
    });
    // HTTP 状态和通用错误类型不足以停号；只接受明确错误码或首句，避免把权限提示误判成封禁。
    match (fields.status, fields.error_type.as_deref()) {
        (Some(401), Some("authentication_error"))
            if matches!(
                fields.code.as_deref(),
                Some("token_revoked" | "token_invalidated")
            ) || matches!(
                sentence,
                Some(
                    "invalid bearer token"
                        | "invalid oauth token"
                        | "oauth token has been revoked"
                        | "access token has been revoked"
                )
            ) =>
        {
            // AT 被拒绝不证明 RT 也失效；Expired 退出选号，但保留受控刷新能力。
            Some(CredentialState::Expired)
        }
        (Some(403), Some("permission_error"))
            if matches!(
                fields.code.as_deref(),
                Some("account_disabled" | "account_banned")
            ) || matches!(
                sentence,
                Some(
                    "your account has been disabled"
                        | "your account has been banned"
                        | "this organization has been disabled"
                )
            ) =>
        {
            Some(CredentialState::Banned)
        }
        _ => None,
    }
}

fn score_account_failure(error: &ProviderError) -> bool {
    // Closed classification: model/request refusals and local infrastructure never poison account health.
    error.send_state() == UpstreamSendState::Sent
        && (error.diagnostic().is_some_and(|diagnostic| {
            diagnostic.stage() == Some("anthropic_upstream") && diagnostic.code() == Some("account")
        }) || matches!(
            error.kind(),
            ProviderErrorKind::Transport | ProviderErrorKind::Protocol
        ))
}

async fn restore_error_body(
    privacy: &gateway_veil::Session,
    bytes: Bytes,
) -> Result<Bytes, ProviderError> {
    // 上游网关也会返回纯文本或 HTML 错误，保留其状态与类型，同时严格恢复其中的占位符。
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => {
            let value = privacy
                .restore(value)
                .await
                .map_err(|error| veil_error(error, UpstreamSendState::Sent))?;
            serde_json::to_vec(&value)
                .map(Bytes::from)
                .map_err(|_| sent(ProviderErrorKind::Protocol))
        }
        Err(_) => {
            let text =
                std::str::from_utf8(&bytes).map_err(|_| sent(ProviderErrorKind::Protocol))?;
            let restored = privacy
                .restore(Value::String(text.to_owned()))
                .await
                .map_err(|error| veil_error(error, UpstreamSendState::Sent))?;
            Ok(Bytes::from(
                restored
                    .as_str()
                    .expect("text restoration preserves shape")
                    .to_owned(),
            ))
        }
    }
}
fn not_sent(kind: ProviderErrorKind) -> ProviderError {
    ProviderError::new(kind, UpstreamSendState::NotSent).with_retry_prohibited()
}
fn sent(kind: ProviderErrorKind) -> ProviderError {
    ProviderError::new(kind, UpstreamSendState::Sent).with_retry_prohibited()
}
fn infra() -> ProviderError {
    not_sent(ProviderErrorKind::ProviderInfrastructureUnavailable)
}
fn veil_error(error: gateway_veil::Error, state: UpstreamSendState) -> ProviderError {
    // 隐私基础设施失败不应处罚选中的订阅账号，且绝不重发未恢复的内容。
    let code = error.to_string();
    ProviderError::new(ProviderErrorKind::ProviderInfrastructureUnavailable, state)
        .with_retry_prohibited()
        .with_diagnostic(
            gateway_core::error::ProviderDiagnostic::new(code)
                .with_classification("veil", "privacy_processing_failed"),
        )
}
fn milliseconds(started: std::time::Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
fn phase(context: &AttemptContext, name: &'static str, duration_ms: u64, succeeded: bool) {
    // 诊断只记录阶段、耗时和结果，不写正文、占位符、凭据或规则匹配值。
    context.trace().record_evidence(
        name,
        serde_json::json!({"durationMs":duration_ms,"succeeded":succeeded}),
    );
}
fn check_context(context: &AttemptContext, state: UpstreamSendState) -> Result<(), ProviderError> {
    if context.cancellation().is_cancelled() {
        return Err(ProviderError::new(ProviderErrorKind::Cancelled, state).with_retry_prohibited());
    }
    if context.deadline().is_elapsed() {
        return Err(ProviderError::new(ProviderErrorKind::Timeout, state).with_retry_prohibited());
    }
    Ok(())
}
async fn next_chunk(
    response: &mut reqwest::Response,
    context: &AttemptContext,
) -> Result<Option<Bytes>, ProviderError> {
    tokio::select! {
        _ = context.cancellation().cancelled() => Err(sent(ProviderErrorKind::Cancelled)),
        _ = deadline_elapsed(context) => Err(sent(ProviderErrorKind::Timeout)),
        result = response.chunk() => result.map_err(|_| sent(ProviderErrorKind::Transport)),
    }
}
async fn deadline_elapsed(context: &AttemptContext) {
    if let Some(remaining) = context.deadline().remaining() {
        tokio::time::sleep(remaining).await;
    } else {
        futures::future::pending::<()>().await;
    }
}
async fn read_body(
    response: &mut reqwest::Response,
    context: &AttemptContext,
) -> Result<Bytes, ProviderError> {
    let mut bytes = BytesMut::new();
    while let Some(chunk) = next_chunk(response, context).await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
            return Err(sent(ProviderErrorKind::MessageTooBig));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.freeze())
}
fn safe_headers(headers: &reqwest::header::HeaderMap) -> Vec<ProviderResponseHeader> {
    headers
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "request-id" | "retry-after" | "x-should-retry" | "content-type"
            ) || name.as_str().starts_with("anthropic-ratelimit-")
        })
        .map(|(name, value)| {
            ProviderResponseHeader::new(name.as_str(), Bytes::copy_from_slice(value.as_bytes()))
        })
        .collect()
}
fn event_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| index + 2)
        .into_iter()
        .chain(
            bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4),
        )
        .min()
}
fn restored_text_events(mut bytes: &[u8]) -> Result<Vec<GatewayEvent>, ProviderError> {
    let mut events = Vec::new();
    while let Some(end) = event_end(bytes) {
        if let Some(value) = event_json(&bytes[..end])?
            && value["type"] == "content_block_delta"
            && value["delta"]["type"] == "text_delta"
            && let (Some(index), Some(text)) =
                (value["index"].as_u64(), value["delta"]["text"].as_str())
            && !text.is_empty()
        {
            events.push(GatewayEvent::TextDelta(gateway_core::event::TextDelta {
                content_index: u32::try_from(index)
                    .map_err(|_| sent(ProviderErrorKind::Protocol))?,
                text: text.to_owned(),
            }));
        }
        bytes = &bytes[end..];
    }
    Ok(events)
}
fn event_json(bytes: &[u8]) -> Result<Option<Value>, ProviderError> {
    let text = std::str::from_utf8(bytes).map_err(|_| sent(ProviderErrorKind::Protocol))?;
    let data = text
        .lines()
        .filter_map(|line| {
            line.strip_prefix("data:")
                .map(|data| data.strip_prefix(' ').unwrap_or(data))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|_| sent(ProviderErrorKind::Protocol))
}

fn elapsed_ms(context: &AttemptContext) -> u64 {
    u64::try_from(context.timing_started_at().elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
