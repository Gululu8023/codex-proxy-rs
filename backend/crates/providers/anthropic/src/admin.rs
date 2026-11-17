//! Anthropic 账号导入、浏览器授权与控制面事务准备

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use gateway_admin::{
    model::{
        Revision,
        observability::{CalculatedBillingBreakdown, DashboardWireProfile, ProviderBillingInput},
        provider_credentials::*,
    },
    ports::provider::{ProviderAdmin, ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::{
    account::{OpaqueProviderData, ProviderAccountId},
    operation::{GenerateRequest, Operation, ProtocolPayload},
    provider_ports::{OAuthPendingPutOutcome, ProviderLeaseGuard},
    routing::{ProviderKind, UpstreamModelId},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::{
    CLIENT_ID, NAME, REDIRECT_URI, SCOPES,
    checkpoint::Claim,
    credential::{CredentialError, Credentials, Exchanged, Profile, Rotation, Tokens},
};

const MAX_CREDENTIALS: usize = 200;

pub(crate) struct AnthropicAdmin {
    credentials: Arc<Credentials>,
    kind: ProviderKind,
}

impl AnthropicAdmin {
    pub fn new(credentials: Arc<Credentials>) -> Self {
        Self {
            kind: credentials.kind(),
            credentials,
        }
    }

    async fn prepare_create(
        &self,
        exchanged: Exchanged,
        name: Option<String>,
        proxy: Option<gateway_core::account::OutboundProxy>,
    ) -> Result<PreparedCredentialCreate, ProviderAdminError> {
        let profile = match exchanged.profile {
            Some(profile) => profile,
            None => self
                .credentials
                .profile(&exchanged.tokens, proxy.as_ref())
                .await
                .map_err(admin_error)?,
        };
        Ok(create(exchanged.tokens, profile, name, proxy))
    }

    async fn authorization_result(
        &self,
        value: &mut Value,
        command: &CompleteAuthorization,
        claim: &Claim,
    ) -> Result<PreparedAuthorizationCommit, ProviderAdminError> {
        let pending = PendingAuthorizationMutation::from_storage_v1(value["mutation"].clone())
            .map_err(|_| invalid())?;
        if !pending.owner_binding().matches_context(&command.context)
            || value["expires_at"]
                .as_i64()
                .is_none_or(|expires| expires <= Utc::now().timestamp())
        {
            return Err(invalid());
        }
        let expected_state = value["state"].as_str().ok_or_else(invalid)?.to_owned();
        let (code, state) = parse_callback(&command.callback_url)?;
        if !bool::from(state.as_bytes().ct_eq(expected_state.as_bytes())) {
            return Err(invalid());
        }
        let code_digest = hex::encode(Sha256::digest(code.as_bytes()));
        if value
            .get("code_digest")
            .is_some_and(|digest| digest.as_str() != Some(code_digest.as_str()))
        {
            return Err(invalid());
        }
        let exchanged = match value["phase"].as_str() {
            Some("exchanged") => serde_json::from_value::<Exchanged>(value["exchanged"].clone())
                .map_err(|_| invalid())?,
            Some("exchanging") => return Err(admin_error(CredentialError::Ambiguous)),
            Some("revoked") => return Err(invalid()),
            Some("ready") => {
                let verifier = value["verifier"].as_str().ok_or_else(invalid)?.to_owned();
                value["phase"] = json!("exchanging");
                value["code_digest"] = json!(code_digest);
                // 先记录不可重放阶段；取消或回包丢失后也不能再次兑换同一 code。
                claim.save(value).await.map_err(admin_error)?;
                let result = self.credentials.exchange(json!({"grant_type":"authorization_code", "code":code,
                    "state":state, "client_id":CLIENT_ID, "redirect_uri":REDIRECT_URI, "code_verifier":verifier}),
                    pending.outbound_proxy(), None).await;
                match result {
                    Ok(exchanged) => {
                        value["phase"] = json!("exchanged");
                        value["exchanged"] = serde_json::to_value(&exchanged)
                            .map_err(|_| admin_error(CredentialError::Ambiguous))?;
                        value
                            .as_object_mut()
                            .expect("pending object")
                            .remove("verifier");
                        claim
                            .save(value)
                            .await
                            .map_err(|_| admin_error(CredentialError::Ambiguous))?;
                        exchanged
                    }
                    Err(error) => {
                        if !matches!(error, CredentialError::Ambiguous) {
                            value["phase"] = json!(if matches!(error, CredentialError::Revoked) {
                                "revoked"
                            } else {
                                "ready"
                            });
                            value
                                .as_object_mut()
                                .expect("pending object")
                                .remove("code_digest");
                            claim
                                .save(value)
                                .await
                                .map_err(|_| admin_error(CredentialError::Ambiguous))?;
                        }
                        return Err(admin_error(error));
                    }
                }
            }
            _ => return Err(invalid()),
        };
        let created = self
            .prepare_create(
                exchanged,
                match pending.target() {
                    AuthorizationMutationTarget::Create { name } => Some(name.clone()),
                    _ => None,
                },
                pending.outbound_proxy().cloned(),
            )
            .await?;
        let credential = match pending.target() {
            AuthorizationMutationTarget::Create { .. } => {
                PreparedAuthorizationCredential::Create(Box::new(created))
            }
            AuthorizationMutationTarget::Reauthorize { account_id } => {
                let (loaded, _) = self
                    .credentials
                    .load(account_id)
                    .await
                    .map_err(admin_error)?;
                if loaded.account.upstream_user_id() != created.upstream_user_id.as_deref()
                    || loaded.account.upstream_account_id()
                        != created.upstream_account_id.as_deref()
                {
                    return Err(invalid());
                }
                let tokens = Tokens::parse(&Value::Object(
                    created
                        .provider_material
                        .expose_to_provider()
                        .expose_to_provider()
                        .clone(),
                ))
                .map_err(admin_error)?;
                PreparedAuthorizationCredential::Reauthorize(Box::new(rotation(Rotation {
                    loaded,
                    tokens,
                    guards: vec![],
                })?))
            }
        };
        Ok(PreparedAuthorizationCommit::new(pending, credential))
    }
}

#[async_trait]
impl ProviderAdmin for AnthropicAdmin {
    fn default_client_profile(&self) -> Option<OpaqueProviderData> {
        Some(crate::profile::object(&crate::profile::Selection::default()))
    }
    fn client_profile_options(&self) -> Result<OpaqueProviderData, ProviderAdminError> {
        Ok(self.credentials.profiles.options())
    }
    fn preview_client_profile(
        &self,
        configuration: &OpaqueProviderData,
    ) -> Result<OpaqueProviderData, ProviderAdminError> {
        // 默认画像会序列化空种子；null 等同未设置，只有实际安装标识禁止用于全局画像。
        if configuration
            .expose_to_provider()
            .get("installationSeed")
            .is_some_and(|value| !value.is_null())
            || gateway_core::settings::request_profiles::inherited_fields(configuration).is_some()
        {
            return Err(invalid());
        }
        let selection = crate::profile::Selection::parse(configuration).map_err(|_| invalid())?;
        Ok(self.credentials.profiles.preview(selection))
    }
    fn preview_key_client_profile(
        &self,
        key: &OpaqueProviderData,
        global: Option<&OpaqueProviderData>,
    ) -> Result<OpaqueProviderData, ProviderAdminError> {
        if let Some(fields) = gateway_core::settings::request_profiles::inherited_fields(key)
            && fields.keys().any(|key| key != "installationSeed")
        {
            return Err(invalid());
        }
        let resolved = gateway_core::settings::request_profiles::resolve_key_profile(key, global);
        let selection = if let Some(fields) =
            gateway_core::settings::request_profiles::inherited_fields(&resolved)
        {
            crate::profile::Selection::parse(&OpaqueProviderData::new(fields.clone()))
        } else {
            crate::profile::Selection::parse(&resolved)
        }
        .map_err(|_| invalid())?;
        Ok(self.credentials.profiles.preview(selection))
    }
    fn provider_kind(&self) -> &ProviderKind {
        &self.kind
    }
    fn account_capabilities(
        &self,
        _id: &ProviderAccountId,
        authentication: &str,
    ) -> gateway_admin::model::accounts::ProviderAccountCapabilities {
        gateway_admin::model::accounts::ProviderAccountCapabilities {
            quota: authentication == "oauth",
            quota_refresh: authentication == "oauth",
            ..Default::default()
        }
    }
    fn pricing_catalog(&self) -> gateway_admin::model::pricing::ProviderPricingCatalog {
        crate::usage::pricing_catalog()
    }
    async fn account_unavailable(&self, _account_id: &ProviderAccountId) {}
    fn dashboard_wire_profile(&self) -> Option<DashboardWireProfile> {
        Some(
            self.credentials
                .profiles
                .dashboard(crate::profile::Selection::default()),
        )
    }
    fn configured_wire_profile(
        &self,
        configuration: &OpaqueProviderData,
    ) -> Option<DashboardWireProfile> {
        let selection = crate::profile::Selection::parse(configuration).ok()?;
        Some(self.credentials.profiles.dashboard(selection))
    }
    fn calculated_billing(
        &self,
        _input: &ProviderBillingInput,
    ) -> Result<Option<CalculatedBillingBreakdown>, ProviderAdminError> {
        Ok(None)
    }

    async fn connection_test_operation(
        &self,
        _model: &UpstreamModelId,
        input: &str,
    ) -> Result<Operation, ProviderAdminError> {
        let payload = ProtocolPayload::json_object(
            NAME,
            json!({"messages":[{"role":"user","content":input}],"max_tokens":32,"stream":true})
                .as_object()
                .expect("object")
                .clone(),
        )
        .map_err(|_| invalid())?;
        Ok(Operation::Generate(
            GenerateRequest::from_protocol_payload(payload).with_native_requirements(
                gateway_core::operation::CapabilityRequirements::new(
                    gateway_core::operation::OperationKind::Generate,
                )
                .with_requested_output_tokens(Some(32)),
            ),
        ))
    }

    async fn prepare_import(
        &self,
        command: PrepareCredentialImport,
    ) -> Result<PreparedCredentialImport, ProviderAdminError> {
        let document = Value::Object(
            command
                .document
                .expose_to_provider()
                .expose_to_provider()
                .clone(),
        );
        let entries = match document.get("credentials") {
            Some(Value::Array(entries)) => entries.clone(),
            Some(_) => return Err(invalid()),
            None => vec![document],
        };
        if entries.is_empty() || entries.len() > MAX_CREDENTIALS {
            return Err(invalid());
        }
        // 先校验整份文档，避免尾部格式错误触发前部一次性 RT 轮换；账号身份仍由上游证明。
        let mut seen_tokens = std::collections::BTreeSet::new();
        let entries = entries
            .into_iter()
            .map(|entry| {
                let tokens = Tokens::parse(&entry).map_err(admin_error)?;
                if !seen_tokens.insert(hex::encode(Sha256::digest(tokens.access_token.as_bytes())))
                {
                    return Err(invalid());
                }
                Ok((tokens, entry["name"].as_str().map(str::to_owned)))
            })
            .collect::<Result<Vec<_>, ProviderAdminError>>()?;
        let mut credentials = Vec::with_capacity(entries.len());
        for (tokens, name) in entries {
            let exchanged = self
                .credentials
                .import_tokens(tokens, command.default_outbound_proxy.as_ref())
                .await
                .map_err(admin_error)?;
            let prepared = self
                .prepare_create(exchanged, name, command.default_outbound_proxy.clone())
                .await?;
            if credentials
                .iter()
                .any(|value: &PreparedCredentialCreate| value.account_id == prepared.account_id)
            {
                return Err(invalid());
            }
            credentials.push(prepared);
        }
        Ok(PreparedCredentialImport {
            provider_kind: self.kind.clone(),
            credentials,
        })
    }

    async fn start_authorization(
        &self,
        pending: PendingAuthorizationMutation,
    ) -> Result<AuthorizationStarted, ProviderAdminError> {
        self.credentials
            .veil
            .control(gateway_veil::ControlPurpose::OAuth)
            .map_err(|_| admin_error(CredentialError::Unavailable))?;
        let flow_id = random()?;
        let state = random()?;
        let verifier = random()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let expires_at = Utc::now() + chrono::Duration::minutes(10);
        let mutation = pending.to_storage_v1();
        let owner = owner_digest(pending.owner_binding().owner());
        let payload = json!({"mutation":mutation,"state":state,"verifier":verifier,"expires_at":expires_at.timestamp(),"phase":"ready"});
        if Claim::put(
            &self.credentials,
            flow_id.clone(),
            owner,
            &payload,
            Duration::from_secs(600),
        )
        .await
        .map_err(admin_error)?
            != OAuthPendingPutOutcome::Stored
        {
            return Err(unavailable());
        }
        let mut url =
            url::Url::parse("https://claude.ai/oauth/authorize").expect("static OAuth URL");
        url.query_pairs_mut()
            .append_pair("code", "true")
            .append_pair("client_id", CLIENT_ID)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", SCOPES)
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(AuthorizationStarted {
            flow_id,
            authorization_url: url.into(),
            expires_at,
        })
    }

    async fn complete_authorization(
        &self,
        command: CompleteAuthorization,
    ) -> Result<PreparedAuthorizationCommit, ProviderAdminError> {
        let owner = owner_digest(AuthorizationOwnerBinding::from_context(&command.context).owner());
        let (guard, mut value) = Claim::acquire(&self.credentials, command.flow_id.clone(), owner)
            .await
            .map_err(admin_error)?;
        match self
            .authorization_result(&mut value, &command, &guard)
            .await
        {
            Ok(prepared) => Ok(prepared.with_authorization_guard(Box::new(guard))),
            Err(error) => {
                // 清理失败不能把结果未知降级成普通暂时故障；claim 仍由原 TTL 兜底释放。
                if let Err(cleanup) = guard.release().await
                    && error.kind() != ProviderAdminErrorKind::Ambiguous
                {
                    return Err(admin_error(cleanup));
                }
                Err(error)
            }
        }
    }

    async fn prepare_rotation(
        &self,
        command: PrepareCredentialRotation,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        let (loaded, _) = self
            .credentials
            .load(&ProviderAccountId::new(command.account.id).map_err(|_| invalid())?)
            .await
            .map_err(admin_error)?;
        let tokens = Tokens::parse(&Value::Object(
            command
                .provider_material
                .expose_to_provider()
                .expose_to_provider()
                .clone(),
        ))
        .map_err(admin_error)?;
        let profile = self
            .credentials
            .profile(&tokens, loaded.account.outbound_proxy())
            .await
            .map_err(admin_error)?;
        if loaded.account.upstream_user_id() != Some(profile.user.as_str())
            || loaded.account.upstream_account_id() != Some(profile.organization.as_str())
        {
            return Err(invalid());
        }
        rotation(Rotation {
            loaded,
            tokens,
            guards: vec![],
        })
    }

    async fn prepare_refresh(
        &self,
        command: PrepareCredentialRefresh,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        rotation(
            self.credentials
                .prepare_refresh(
                    &ProviderAccountId::new(command.account.id).map_err(|_| invalid())?,
                    None,
                )
                .await
                .map_err(admin_error)?,
        )
    }

    async fn quota(
        &self,
        request: ProviderQuotaRequest,
    ) -> Result<ProviderQuota, ProviderAdminError> {
        crate::quota::read(&self.credentials, request)
            .await
            .map_err(admin_error)
    }

    async fn models(
        &self,
        id: &ProviderAccountId,
        refresh: bool,
    ) -> Result<ProviderModels, ProviderAdminError> {
        let mode = if refresh {
            crate::provider::CatalogRead::Refresh
        } else {
            crate::provider::CatalogRead::Snapshot
        };
        let catalog = crate::provider::account_models(&self.credentials, id, mode)
            .await
            .map_err(admin_error)?;
        let models = catalog
            .models
            .into_iter()
            .map(|value| {
                Ok(ProviderModel {
                    id: UpstreamModelId::new(value["id"].as_str().ok_or_else(invalid)?)
                        .map_err(|_| invalid())?,
                    name: value["display_name"]
                        .as_str()
                        .or_else(|| value["id"].as_str())
                        .ok_or_else(invalid)?
                        .to_owned(),
                })
            })
            .collect::<Result<_, ProviderAdminError>>()?;
        Ok(ProviderModels {
            models,
            observed_at: Some(catalog.observed_at),
        })
    }

    async fn model_catalog_document(
        &self,
        id: &ProviderAccountId,
    ) -> Result<ProviderModelCatalogDocument, ProviderAdminError> {
        let catalog = crate::provider::account_models(
            &self.credentials,
            id,
            crate::provider::CatalogRead::Snapshot,
        )
        .await
        .map_err(admin_error)?;
        Ok(ProviderModelCatalogDocument {
            model_count: catalog.models.len(),
            observed_at: catalog.observed_at,
            document: gateway_core::operation::RawJsonPayload::new(
                NAME,
                bytes::Bytes::from(serde_json::to_vec(&catalog).map_err(|_| invalid())?),
            )
            .map_err(|_| invalid())?,
        })
    }

    async fn export_credentials(
        &self,
        credentials: Vec<ProviderExportCredentialInput>,
    ) -> Result<ProviderExport, ProviderAdminError> {
        if credentials.is_empty() || credentials.len() > MAX_CREDENTIALS {
            return Err(invalid());
        }
        let account_ids = credentials
            .iter()
            .map(|entry| ProviderAccountId::new(entry.account.id.clone()).map_err(|_| invalid()))
            .collect::<Result<_, _>>()?;
        let values: Vec<_> = credentials
            .into_iter()
            .map(|entry| {
                Value::Object(
                    entry
                        .provider_material
                        .into_provider_data()
                        .expose_to_provider()
                        .clone(),
                )
            })
            .collect();
        Ok(ProviderExport {
            provider_kind: self.kind.clone(),
            account_ids,
            document: ProviderDocument::new(OpaqueProviderData::new(
                json!({"provider":NAME,"credentials":values})
                    .as_object()
                    .expect("object")
                    .clone(),
            )),
        })
    }
}

fn create(
    tokens: Tokens,
    profile: Profile,
    name: Option<String>,
    proxy: Option<gateway_core::account::OutboundProxy>,
) -> PreparedCredentialCreate {
    PreparedCredentialCreate {
        model_access: None,
        outbound_proxy: proxy,
        account_id: profile.id(),
        provider_kind: ProviderKind::new(NAME).expect("valid provider"),
        name: name
            .or_else(|| profile.email.clone())
            .unwrap_or_else(|| "Claude".into()),
        email: profile.email,
        upstream_user_id: Some(profile.user),
        upstream_account_id: Some(profile.organization),
        plan_type: profile.plan,
        authentication_kind: "oauth".into(),
        provider_material: ProviderDocument::new(tokens.material()),
        has_refresh_token: tokens.refresh_token.is_some(),
        access_token_expires_at: tokens.expiry().map(DateTime::<Utc>::from),
        next_refresh_at: None,
        enabled: true,
        credential_state: gateway_admin::model::accounts::CredentialState::Ready,
        credential_observed_at: Utc::now(),
    }
}

struct RotationGuard {
    _guards: Vec<Box<dyn ProviderLeaseGuard>>,
}
impl CredentialCommitGuard for RotationGuard {
    fn finish(self: Box<Self>) {}
}

fn rotation(rotation: Rotation) -> Result<PreparedCredentialRotation, ProviderAdminError> {
    let account = &rotation.loaded.account;
    Ok(PreparedCredentialRotation::new(
        PreparedCredentialRotationFacts {
            account_id: account.id().clone(),
            provider_kind: account.provider().clone(),
            expected_credential_revision: Revision::new(account.revision().get())
                .map_err(|_| invalid())?,
            replacement_identity: None,
            name: account.name().to_owned(),
            email: account.email().map(str::to_owned),
            plan_type: account.plan_type().map(str::to_owned),
            preserve_profile: true,
            preserve_credential_state: false,
            provider_material: ProviderDocument::new(rotation.tokens.material()),
            has_refresh_token: rotation.tokens.refresh_token.is_some(),
            access_token_expires_at: rotation.tokens.expiry().map(Into::into),
            next_refresh_at: None,
        },
        Box::new(RotationGuard {
            _guards: rotation.guards,
        }),
    ))
}

fn owner_digest(owner: &AuthorizationOwner) -> String {
    let identity = match owner {
        AuthorizationOwner::AdminSession { admin_user_id } => format!("session:{admin_user_id}"),
        AuthorizationOwner::AdminApiKey => "admin_api_key".into(),
        AuthorizationOwner::System => "system".into(),
    };
    hex::encode(Sha256::digest(identity.as_bytes()))
}
fn random() -> Result<String, ProviderAdminError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| unavailable())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn parse_callback(value: &str) -> Result<(String, String), ProviderAdminError> {
    if let Ok(url) = url::Url::parse(value.trim()) {
        let expected = url::Url::parse(REDIRECT_URI).expect("redirect URI");
        if url.origin() != expected.origin()
            || url.path() != expected.path()
            || url.fragment().is_some()
        {
            return Err(invalid());
        }
        let pairs: Vec<_> = url.query_pairs().collect();
        if pairs.iter().filter(|(key, _)| key == "code").count() != 1
            || pairs.iter().filter(|(key, _)| key == "state").count() != 1
        {
            return Err(invalid());
        }
        if pairs
            .iter()
            .any(|(key, value)| matches!(key.as_ref(), "code" | "state") && value.is_empty())
        {
            return Err(invalid());
        }
        return Ok((
            pairs
                .iter()
                .find(|(key, _)| key == "code")
                .expect("code")
                .1
                .to_string(),
            pairs
                .iter()
                .find(|(key, _)| key == "state")
                .expect("state")
                .1
                .to_string(),
        ));
    }
    let (code, state) = value.trim().split_once('#').ok_or_else(invalid)?;
    if code.is_empty() || state.is_empty() || state.contains('#') {
        return Err(invalid());
    }
    Ok((code.to_owned(), state.to_owned()))
}
fn invalid() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Invalid)
}
fn unavailable() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Unavailable)
}
fn admin_error(error: CredentialError) -> ProviderAdminError {
    ProviderAdminError::new(match error {
        CredentialError::Invalid
        | CredentialError::Revoked
        | CredentialError::AccessUnauthorized => ProviderAdminErrorKind::Invalid,
        CredentialError::Ambiguous => ProviderAdminErrorKind::Ambiguous,
        CredentialError::Unavailable | CredentialError::Rejected => {
            ProviderAdminErrorKind::Unavailable
        }
        CredentialError::Busy => ProviderAdminErrorKind::Conflict,
    })
}
