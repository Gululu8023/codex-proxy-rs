//! Claude OAuth 材料校验、账号身份与租约保护的令牌轮换

use std::time::{Duration, SystemTime};

use gateway_core::{
    account::{
        AccountErrorReason, AccountStateChange, CredentialCasOutcome, CredentialCasUpdate,
        CredentialRevision, CredentialState, LoadedCredential, OpaqueProviderData,
        PlaintextCredential, ProviderAccountId, ProviderAccountUpdate,
    },
    provider_ports::{
        ProviderLeaseAcquisition, ProviderLeaseGuard, ProviderLeaseRequest,
        ProviderRefreshCapacityRequest, ProviderRefreshLeaseRequest, ProviderStorePorts,
        provider_refresh_backoff_at,
    },
    routing::ProviderKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{API, CLIENT_ID, MAX_BODY, NAME, TOKEN_URL};

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialError {
    #[error("Claude credential is invalid or requires authorization")]
    Invalid,
    #[error("Claude credential service is temporarily unavailable")]
    Unavailable,
    #[error("Claude control request was rejected")]
    Rejected,
    #[error("Claude access token was rejected by the profile endpoint")]
    AccessUnauthorized,
    #[error("Claude credential is being refreshed")]
    Busy,
    #[error("Claude refresh grant was explicitly revoked")]
    Revoked,
    #[error("Claude token exchange outcome is uncertain; authorization is required")]
    Ambiguous,
}

// 敏感材料不派生 Debug，错误也不能包含 reqwest 的 URL 或响应正文
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub scopes: Vec<String>,
}

impl Tokens {
    pub fn parse(value: &Value) -> Result<Self, CredentialError> {
        let value = value.get("claudeAiOauth").unwrap_or(value);
        let tokens: Self =
            serde_json::from_value(value.clone()).map_err(|_| CredentialError::Invalid)?;
        if tokens.access_token.trim().is_empty()
            || tokens.refresh_token.as_ref().is_some_and(|token| token.trim().is_empty())
            || !tokens.scopes.iter().any(|scope| scope == "user:inference")
            // 原生 expiresAt 使用 Unix 毫秒；拒绝秒数或无效日期，不能把它们当作未设置。
            || tokens.expires_at.is_some_and(|ms| ms < 1_000_000_000_000 || chrono::DateTime::from_timestamp_millis(ms).is_none())
        {
            return Err(CredentialError::Invalid);
        }
        Ok(tokens)
    }

    pub fn expiry(&self) -> Option<SystemTime> {
        self.expires_at
            .and_then(|ms| u64::try_from(ms).ok())
            .and_then(|ms| SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(ms)))
    }

    pub fn material(&self) -> OpaqueProviderData {
        OpaqueProviderData::new(
            json!({"claudeAiOauth": self})
                .as_object()
                .expect("object")
                .clone(),
        )
    }

    fn from_response(value: &Value, previous: Option<&Self>) -> Result<Self, CredentialError> {
        let access_token = value["access_token"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or(CredentialError::Invalid)?
            .to_owned();
        let expires_in = value["expires_in"]
            .as_u64()
            .filter(|seconds| *seconds > 0)
            .ok_or(CredentialError::Invalid)?;
        let expires_at = chrono::Utc::now()
            .timestamp_millis()
            .checked_add(
                i64::try_from(expires_in)
                    .map_err(|_| CredentialError::Invalid)?
                    .checked_mul(1000)
                    .ok_or(CredentialError::Invalid)?,
            )
            .ok_or(CredentialError::Invalid)?;
        let scopes = value["scope"]
            .as_str()
            .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
            .or_else(|| previous.map(|tokens| tokens.scopes.clone()))
            .ok_or(CredentialError::Invalid)?;
        let refresh_token = value["refresh_token"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| previous.and_then(|tokens| tokens.refresh_token.clone()));
        let tokens = Self {
            access_token,
            refresh_token,
            expires_at: Some(expires_at),
            scopes,
        };
        Self::parse(&json!(tokens))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Profile {
    pub user: String,
    pub organization: String,
    pub email: Option<String>,
    pub plan: Option<String>,
}

impl Profile {
    // 只能由成功的官方控制响应调用；上传凭据里的身份字段从不进入这个信任路径。
    fn from_response(value: &Value) -> Option<Self> {
        Some(Self {
            user: value["account"]["uuid"]
                .as_str()
                .filter(|id| crate::profile::uuid(id))?
                .to_owned(),
            organization: value["organization"]["uuid"]
                .as_str()
                .filter(|id| crate::profile::uuid(id))?
                .to_owned(),
            email: value["account"]["email"]
                .as_str()
                .or_else(|| value["account"]["email_address"].as_str())
                .map(str::to_owned),
            // 与官方客户端一致，优先读取当前组织的套餐；旧布尔字段仅作兼容，不把缺失推断为免费套餐。
            plan: match value["organization"]["organization_type"].as_str() {
                Some("claude_max") => Some("max".into()),
                Some("claude_pro") => Some("pro".into()),
                Some("claude_team") => Some("team".into()),
                Some("claude_enterprise") => Some("enterprise".into()),
                _ if value["account"]["has_claude_max"].as_bool() == Some(true) => {
                    Some("max".into())
                }
                _ if value["account"]["has_claude_pro"].as_bool() == Some(true) => {
                    Some("pro".into())
                }
                _ => None,
            },
        })
    }
    pub fn id(&self) -> ProviderAccountId {
        let digest = Sha256::digest(format!("{}\0{}", self.user, self.organization).as_bytes());
        // 账号 ID 必须带核心要求的 acct_ 前缀；保留用户与组织的确定性映射。
        ProviderAccountId::new(format!("acct_anthropic_{}", hex::encode(digest)))
            .expect("valid account id")
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Exchanged {
    pub tokens: Tokens,
    pub profile: Option<Profile>,
}

pub(crate) struct Rotation {
    pub loaded: LoadedCredential,
    pub tokens: Tokens,
    pub guards: Vec<Box<dyn ProviderLeaseGuard>>,
}

impl Rotation {
    pub fn update(&self) -> Result<CredentialCasUpdate, CredentialError> {
        let account = &self.loaded.account;
        CredentialCasUpdate::new(
            account.id().clone(),
            account.revision(),
            ProviderAccountUpdate {
                account_id: account.id().clone(),
                name: account.name().to_owned(),
                email: account.email().map(str::to_owned),
                plan_type: account.plan_type().map(str::to_owned),
            },
            PlaintextCredential::new(self.tokens.material().expose_to_provider().clone()),
            self.tokens.refresh_token.is_some(),
            self.tokens.expiry(),
            None,
        )
        .map(|update| {
            update.preserving_profile().with_account_state(
                CredentialState::Ready,
                SystemTime::now(),
                None,
                None,
            )
        })
        .map_err(|_| CredentialError::Invalid)
    }
}

pub(crate) struct Credentials {
    pub ports: ProviderStorePorts,
    pub veil: std::sync::Arc<gateway_veil::Runtime>,
    pub transport: crate::transport::Transport,
    pub profiles: std::sync::Arc<crate::profile::Profiles>,
    pub catalog_changed: tokio::sync::Notify,
}

impl Credentials {
    pub fn new(ports: ProviderStorePorts, veil: std::sync::Arc<gateway_veil::Runtime>) -> Self {
        Self {
            profiles: std::sync::Arc::new(crate::profile::Profiles::new(ports.artifact_profiles())),
            ports,
            transport: crate::transport::Transport::new(veil.clone()),
            veil,
            catalog_changed: tokio::sync::Notify::new(),
        }
    }

    pub fn kind(&self) -> ProviderKind {
        ProviderKind::new(NAME).expect("valid provider")
    }

    pub fn control_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = self
            .profiles
            .resolve(crate::profile::Selection::default())
            .headers(reqwest::header::HeaderMap::new(), None, true, 0);
        headers.remove("anthropic-dangerous-direct-browser-access");
        headers
    }

    pub async fn load(
        &self,
        id: &ProviderAccountId,
    ) -> Result<(LoadedCredential, Tokens), CredentialError> {
        let loaded = self
            .ports
            .accounts()
            .load_current_credential(id)
            .await
            .map_err(|_| CredentialError::Unavailable)?;
        if loaded.account.provider() != &self.kind() {
            return Err(CredentialError::Invalid);
        }
        let tokens = Tokens::parse(&Value::Object(
            loaded.credential.expose_to_provider().clone(),
        ))?;
        Ok((loaded, tokens))
    }

    pub async fn profile(
        &self,
        tokens: &Tokens,
        proxy: Option<&gateway_core::account::OutboundProxy>,
    ) -> Result<Profile, CredentialError> {
        self.veil
            .control(gateway_veil::ControlPurpose::Profile)
            .map_err(|_| CredentialError::Unavailable)?;
        let request = self
            .transport
            .client(proxy)?
            .get(format!("{API}/api/oauth/profile"))
            .headers(self.control_headers())
            .bearer_auth(&tokens.access_token)
            .timeout(Duration::from_secs(30));
        let response = self
            .transport
            .send(
                request,
                crate::transport::Purpose::Control(gateway_veil::ControlPurpose::Profile),
            )
            .await
            .map_err(|_| CredentialError::Unavailable)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(CredentialError::AccessUnauthorized);
        }
        let value = read_json(response).await?;
        Profile::from_response(&value).ok_or(CredentialError::Invalid)
    }

    pub async fn exchange(
        &self,
        grant: Value,
        proxy: Option<&gateway_core::account::OutboundProxy>,
        previous: Option<&Tokens>,
    ) -> Result<Exchanged, CredentialError> {
        self.veil
            .control(gateway_veil::ControlPurpose::OAuth)
            .map_err(|_| CredentialError::Unavailable)?;
        let request = self
            .transport
            .client(proxy)?
            .post(TOKEN_URL)
            .headers(self.control_headers())
            .json(&grant)
            .timeout(Duration::from_secs(30));
        let response = self
            .transport
            .send(
                request,
                crate::transport::Purpose::Control(gateway_veil::ControlPurpose::OAuth),
            )
            .await
            .map_err(|error| {
                if error.ambiguous() {
                    CredentialError::Ambiguous
                } else {
                    CredentialError::Unavailable
                }
            })?;
        let status = response.status();
        // code 和 RT 都是一次性材料；成功回包或服务端响应丢失时不能重放。
        let value = read_json(response).await.map_err(|error| {
            if status.is_success() || status.is_server_error() {
                CredentialError::Ambiguous
            } else {
                error
            }
        })?;
        let tokens =
            Tokens::from_response(&value, previous).map_err(|_| CredentialError::Ambiguous)?;
        Ok(Exchanged {
            tokens,
            profile: Profile::from_response(&value),
        })
    }

    pub async fn import_tokens(
        &self,
        tokens: Tokens,
        proxy: Option<&gateway_core::account::OutboundProxy>,
    ) -> Result<Exchanged, CredentialError> {
        if tokens
            .expiry()
            .is_none_or(|expiry| expiry > SystemTime::now())
        {
            match self.profile(&tokens, proxy).await {
                Ok(profile) => {
                    return Ok(Exchanged {
                        tokens,
                        profile: Some(profile),
                    });
                }
                // expiresAt 可省略且不作为身份凭证；只有明确的 401 才允许用 RT 恢复。
                Err(CredentialError::AccessUnauthorized) if tokens.refresh_token.is_some() => {}
                Err(error) => return Err(error),
            }
        }
        if tokens.refresh_token.is_none() {
            return Err(CredentialError::Invalid);
        }
        let policy = self
            .ports
            .runtime_policy()
            .load_refresh_policy()
            .await
            .map_err(|_| CredentialError::Unavailable)?;
        let _capacity = self
            .acquire(ProviderLeaseRequest::RefreshCapacity(
                ProviderRefreshCapacityRequest::new(policy.concurrency()),
            ))
            .await?;
        self.exchange_refresh(&tokens, proxy).await
    }

    async fn exchange_refresh(
        &self,
        previous: &Tokens,
        proxy: Option<&gateway_core::account::OutboundProxy>,
    ) -> Result<Exchanged, CredentialError> {
        let refresh = previous
            .refresh_token
            .as_ref()
            .ok_or(CredentialError::Invalid)?;
        // 同一 RT 的导入和后台刷新共享领取权；持有原始 RT 才能构造此内部 owner。
        let fingerprint = hex::encode(Sha256::digest(refresh.as_bytes()));
        let flow = format!("refresh:{fingerprint}");
        let initial = json!({"phase":"ready"});
        crate::checkpoint::Claim::put(
            self,
            flow.clone(),
            fingerprint.clone(),
            &initial,
            Duration::from_secs(1800),
        )
        .await?;
        let (claim, mut state) = crate::checkpoint::Claim::acquire(self, flow, fingerprint).await?;
        let result = async {
            match state["phase"].as_str() {
                Some("exchanged") => {
                    let exchanged: Exchanged = serde_json::from_value(state["exchanged"].clone()).map_err(|_| CredentialError::Invalid)?;
                    if exchanged.tokens.expiry().is_none_or(|expiry| expiry <= SystemTime::now()) { return Err(CredentialError::Invalid); }
                    return Ok(exchanged);
                }
                Some("exchanging") => return Err(CredentialError::Ambiguous),
                Some("revoked") => return Err(CredentialError::Revoked),
                Some("ready") => {},
                _ => return Err(CredentialError::Invalid),
            }
            state["phase"] = json!("exchanging");
            claim.save(&state).await?;
            match self.exchange(json!({"grant_type":"refresh_token", "client_id":CLIENT_ID, "refresh_token":refresh}), proxy, Some(previous)).await {
                Ok(exchanged) => {
                    state["phase"] = json!("exchanged");
                    state["exchanged"] = serde_json::to_value(&exchanged).map_err(|_| CredentialError::Ambiguous)?;
                    // 新 RT 必须先可恢复，才能进入 profile 或整份导入事务；后续失败仍保留检查点。
                    claim.save(&state).await.map_err(|_| CredentialError::Ambiguous)?;
                    Ok(exchanged)
                }
                Err(error) => {
                    if !matches!(error, CredentialError::Ambiguous) {
                        state["phase"] = json!(if matches!(error, CredentialError::Revoked) { "revoked" } else { "ready" });
                        claim.save(&state).await.map_err(|_| CredentialError::Ambiguous)?;
                    }
                    Err(error)
                }
            }
        }.await;
        let released = claim.release().await;
        // 结果未知优先于清理故障，其他清理错误正常上报；绝不把一次性轮换当作可安全重试。
        match (result, released) {
            (Err(CredentialError::Ambiguous), _) => Err(CredentialError::Ambiguous),
            (_, Err(error)) => Err(error),
            (result, Ok(())) => result,
        }
    }

    async fn acquire(
        &self,
        request: ProviderLeaseRequest,
    ) -> Result<Box<dyn ProviderLeaseGuard>, CredentialError> {
        match self
            .ports
            .leases()
            .try_acquire(request)
            .await
            .map_err(|_| CredentialError::Unavailable)?
        {
            ProviderLeaseAcquisition::Acquired(guard) => Ok(guard),
            ProviderLeaseAcquisition::Busy { .. } => Err(CredentialError::Busy),
        }
    }

    pub async fn prepare_refresh(
        &self,
        id: &ProviderAccountId,
        rejected_revision: Option<CredentialRevision>,
    ) -> Result<Rotation, CredentialError> {
        let (before, _) = self.load(id).await?;
        // 在途旧请求的 401 不能发起新代次令牌轮换；锁后重读仍会复核同一 revision
        if rejected_revision.is_some_and(|revision| revision != before.account.revision()) {
            return Err(CredentialError::Busy);
        }
        let policy = self
            .ports
            .runtime_policy()
            .load_refresh_policy()
            .await
            .map_err(|_| CredentialError::Unavailable)?;
        let capacity = self
            .acquire(ProviderLeaseRequest::RefreshCapacity(
                ProviderRefreshCapacityRequest::new(policy.concurrency()),
            ))
            .await?;
        let account = self
            .acquire(ProviderLeaseRequest::Refresh(
                ProviderRefreshLeaseRequest::new(id.clone(), before.account.revision()),
            ))
            .await?;
        // 取得刷新权后重读，不能用排队前的 refresh token 发起轮换
        let (loaded, previous) = self.load(id).await?;
        // 租约按 revision 隔离；旧代次租约不能保护新代次的刷新
        if loaded.account.revision() != before.account.revision() {
            return Err(CredentialError::Busy);
        }
        if matches!(
            loaded.account.credential_state(),
            CredentialState::Invalid | CredentialState::Banned
        ) {
            return Err(CredentialError::Invalid);
        }
        if previous.refresh_token.is_none() {
            return Err(CredentialError::Invalid);
        }
        // 在不可逆轮换前持久暂停调度，进程取消、回包丢失或提交失败都不能继续使用旧 RT
        self.ports
            .accounts()
            .apply_state_change(AccountStateChange {
                account_id: id.clone(),
                expected_revision: loaded.account.revision(),
                credential_state: CredentialState::Invalid,
                observed_at: SystemTime::now(),
                error_reason: Some(AccountErrorReason::CredentialInvalid),
                message: Some(
                    "Claude OAuth refresh pending or uncertain; reauthorize if it cannot complete"
                        .into(),
                ),
            })
            .await
            .map_err(|_| CredentialError::Unavailable)?;
        let (fenced, _) = self.load(id).await?;
        if fenced.account.revision() != loaded.account.revision()
            || fenced.account.credential_state() != CredentialState::Invalid
        {
            return Err(CredentialError::Busy);
        }
        let result = self
            .exchange_refresh(&previous, loaded.account.outbound_proxy())
            .await;
        let tokens = match result {
            Ok(exchanged) => exchanged.tokens,
            Err(error) => {
                self.persist_refresh_failure(&loaded, &error).await?;
                return Err(error);
            }
        };
        Ok(Rotation {
            loaded,
            tokens,
            guards: vec![capacity, account],
        })
    }

    pub async fn refresh(&self, id: &ProviderAccountId) -> Result<(), CredentialError> {
        self.refresh_revision(id, None).await
    }

    pub async fn recover_expired(
        &self,
        rejected: &LoadedCredential,
    ) -> Result<(), CredentialError> {
        self.refresh_revision(rejected.account.id(), Some(rejected.account.revision()))
            .await
    }

    async fn refresh_revision(
        &self,
        id: &ProviderAccountId,
        rejected_revision: Option<CredentialRevision>,
    ) -> Result<(), CredentialError> {
        let rotation = match self.prepare_refresh(id, rejected_revision).await {
            Err(CredentialError::Busy) if rejected_revision.is_some() => {
                let (latest, _) = self.load(id).await?;
                // 并发刷新已提交时仅复用新凭据；刷新仍在途或账号被停用时保留原拒绝
                if Some(latest.account.revision()) != rejected_revision
                    && latest.account.enabled()
                    && latest.account.credential_state() == CredentialState::Ready
                    && latest
                        .account
                        .access_token_expires_at()
                        .is_none_or(|expiry| expiry > SystemTime::now())
                {
                    return Ok(());
                }
                return Err(CredentialError::Busy);
            }
            result => result?,
        };
        // CAS 与 lease 共同保护轮换，迟到的结果不得覆盖较新的 credential revision
        match self
            .ports
            .accounts()
            .compare_and_swap_credential(rotation.update()?)
            .await
            .map_err(|_| CredentialError::Unavailable)?
        {
            CredentialCasOutcome::Updated(_) => {}
            CredentialCasOutcome::Conflict => return Err(CredentialError::Busy),
        }
        // 新代次已提交后立即唤醒采集；不能复用旧 revision 的目录，也不把采集失败当成令牌轮换失败。
        self.catalog_changed.notify_one();
        self.ports
            .credential_state()
            .clear_refresh_backoff(id)
            .await
            .map_err(|_| CredentialError::Unavailable)?;
        Ok(())
    }

    async fn persist_refresh_failure(
        &self,
        loaded: &LoadedCredential,
        error: &CredentialError,
    ) -> Result<(), CredentialError> {
        let account = &loaded.account;
        let now = SystemTime::now();
        if matches!(error, CredentialError::Ambiguous) {
            return Ok(());
        }
        if matches!(error, CredentialError::Revoked) {
            self.ports
                .accounts()
                .apply_state_change(AccountStateChange {
                    account_id: account.id().clone(),
                    expected_revision: account.revision(),
                    credential_state: CredentialState::Expired,
                    observed_at: now,
                    error_reason: Some(AccountErrorReason::CredentialExpired),
                    message: Some("Claude OAuth authorization expired; sign in again".into()),
                })
                .await
                .map_err(|_| CredentialError::Unavailable)?;
        } else {
            let attempt = self
                .ports
                .credential_state()
                .record_refresh_backoff(account.id(), Duration::from_secs(1800))
                .await
                .map_err(|_| CredentialError::Unavailable)?;
            let retry =
                provider_refresh_backoff_at(account.id(), now, attempt, "anthropic-refresh")
                    .map_err(|_| CredentialError::Unavailable)?;
            let update = CredentialCasUpdate::new(
                account.id().clone(),
                account.revision(),
                ProviderAccountUpdate {
                    account_id: account.id().clone(),
                    name: account.name().into(),
                    email: account.email().map(str::to_owned),
                    plan_type: account.plan_type().map(str::to_owned),
                },
                loaded.credential.clone(),
                account.has_refresh_token(),
                account.access_token_expires_at(),
                Some(retry),
            )
            .map_err(|_| CredentialError::Invalid)?
            .preserving_profile()
            .with_account_state(
                account.credential_state(),
                now,
                account.credential_state().error_reason(),
                None,
            );
            self.ports
                .accounts()
                .compare_and_swap_credential(update)
                .await
                .map_err(|_| CredentialError::Unavailable)?;
        }
        Ok(())
    }
}

pub(crate) async fn read_json(mut response: reqwest::Response) -> Result<Value, CredentialError> {
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CredentialError::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
            return Err(CredentialError::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice::<Value>(&bytes);
    if !status.is_success() {
        // 403 并不证明授权失效；只有 OAuth 的明确 invalid_grant 才停用刷新
        return Err(
            if status.as_u16() == 400
                && value.as_ref().is_ok_and(|value| {
                    value["error"].as_str() == Some("invalid_grant")
                        || value["error"]["type"].as_str() == Some("invalid_grant")
                })
            {
                CredentialError::Revoked
            } else {
                CredentialError::Rejected
            },
        );
    }
    value.map_err(|_| CredentialError::Unavailable)
}
