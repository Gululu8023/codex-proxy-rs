//! Native Messages Threads carry increments, so a lost or changed checkpoint must fail closed.

use crate::profile::{Effective, uuid};
use gateway_core::{
    account::ProviderAccount,
    error::{ProviderDiagnostic, ProviderError, ProviderErrorKind},
    operation::ProviderSessionState,
    upstream::UpstreamSendState,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) const BETA: &str = "message-threads-2026-08-12";

pub(crate) fn message_id(id: &str) -> bool {
    id.strip_prefix("msg_").is_some_and(|id| {
        !id.is_empty()
            && id.len() <= 252
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

pub(crate) fn request(body: &Value) -> Result<Option<Option<String>>, ProviderError> {
    let Some(thread) = body.get("thread") else {
        return Ok(None);
    };
    if !thread.is_object() {
        return Err(error("cprs_thread_invalid"));
    }
    match thread["type"].as_str() {
        Some("create") if thread.get("previous_message_id").is_none() => Ok(Some(None)),
        Some("continue") => thread["previous_message_id"]
            .as_str()
            .filter(|id| message_id(id))
            .map(|id| Some(Some(id.into())))
            .ok_or_else(|| error("cprs_thread_invalid")),
        _ => Err(error("cprs_thread_invalid")),
    }
}

/// 只核对已知的显式功能；续接线程不能静默扩充已冻结的 beta 集合。
pub(crate) fn validate_features(body: &Value, betas: &[String]) -> Result<(), ProviderError> {
    let tools = body["tools"].as_array().into_iter().flatten();
    let cache_hour = |value: &Value| value["cache_control"]["ttl"] == "1h";
    let extended_cache = cache_hour(body)
        || tools.clone().any(cache_hour)
        || body["system"]
            .as_array()
            .into_iter()
            .flatten()
            .any(cache_hour)
        || body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|message| message["content"].as_array().into_iter().flatten())
            .any(cache_hour);
    for (needed, beta) in [
        (body["speed"] == "fast", "fast-mode-2026-02-01"),
        (
            tools.clone().any(|tool| {
                tool["type"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("advisor_"))
            }),
            "advisor-tool-2026-03-01",
        ),
        (
            body["thinking"].get("block_binding").is_some(),
            "thinking-binding-controls-2026-08-01",
        ),
        (extended_cache, "extended-cache-ttl-2025-04-11"),
    ] {
        if needed && !betas.iter().any(|declared| declared == beta) {
            return Err(error("cprs_thread_feature_beta_missing"));
        }
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Checkpoint {
    pub message_id: String,
    #[serde(default)]
    model: String,
    account: String,
    account_uuid: String,
    profile: Effective,
    version: String,
    betas: String,
    veil: String,
    session: String,
    device: String,
}

pub(crate) struct Identity {
    pub session: String,
    pub device: String,
}

impl Checkpoint {
    pub fn restore(
        state: Option<&ProviderSessionState>,
        previous: &str,
        account: &ProviderAccount,
        effective: &mut Effective,
        headers: &mut reqwest::header::HeaderMap,
        veil: &str,
    ) -> Result<Identity, ProviderError> {
        let mut saved: Self = state
            .and_then(|state| serde_json::from_value(Value::Object(state.payload().clone())).ok())
            .ok_or_else(|| error("cprs_thread_state_unavailable"))?;
        // 按现有配置字段重算指纹，指纹算法调整不打断旧 Thread；实际身份仍恢复冻结值。
        saved.profile.selection_fingerprint = saved.profile.selection.fingerprint();
        if saved.message_id != previous || saved.account != account.id().as_str()
            // Credential refresh may change its revision, but must never change the Thread's OAuth user.
            || account.upstream_user_id() != Some(saved.account_uuid.as_str())
            || saved.profile.selection_fingerprint != effective.selection_fingerprint
            || saved.profile.template != crate::profile::TEMPLATE || saved.veil != veil
            || (effective.selection.mode == "follow" && (saved.profile.user_agent != effective.user_agent
                || saved.profile.os != effective.os || saved.profile.arch != effective.arch
                || saved.profile.sdk_version != effective.sdk_version || saved.profile.runtime_version != effective.runtime_version))
        {
            return Err(error("cprs_thread_configuration_changed"));
        }
        // An explicitly supplied protocol version/beta must agree; omission restores the frozen contract.
        for (name, value) in [
            ("anthropic-version", saved.version.as_str()),
            ("anthropic-beta", saved.betas.as_str()),
        ] {
            let mismatch = headers.get(name).is_some_and(|header| {
                if name != "anthropic-beta" {
                    return header.as_bytes() != value.as_bytes();
                }
                let incoming = header.to_str().unwrap_or("");
                let mut incoming: std::collections::BTreeSet<_> = incoming
                    .split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .collect();
                incoming.extend([crate::OAUTH_BETA, BETA]);
                let saved: std::collections::BTreeSet<_> = value
                    .split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .collect();
                incoming != saved
            });
            if mismatch {
                return Err(error("cprs_thread_protocol_changed"));
            }
            headers.insert(
                reqwest::header::HeaderName::from_static(name),
                reqwest::header::HeaderValue::from_str(value)
                    .map_err(|_| error("cprs_thread_state_unavailable"))?,
            );
        }
        *effective = saved.profile;
        Ok(Identity {
            session: saved.session,
            device: saved.device,
        })
    }

    pub fn new(
        account: &ProviderAccount,
        profile: Effective,
        headers: &reqwest::header::HeaderMap,
        veil: &str,
        identity: &Identity,
    ) -> Self {
        Self {
            message_id: String::new(),
            model: String::new(),
            account: account.id().as_str().into(),
            account_uuid: account
                .upstream_user_id()
                .expect("validated OAuth user identity")
                .into(),
            profile,
            version: headers["anthropic-version"]
                .to_str()
                .expect("validated header")
                .into(),
            betas: headers["anthropic-beta"]
                .to_str()
                .expect("validated header")
                .into(),
            veil: veil.into(),
            session: identity.session.clone(),
            device: identity.device.clone(),
        }
    }
    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_owned();
        self
    }
    pub fn event(&self, id: &str) -> ProviderSessionState {
        let mut checkpoint = self.clone();
        checkpoint.message_id = id.into();
        ProviderSessionState::new(
            crate::NAME,
            serde_json::to_value(checkpoint)
                .expect("checkpoint serialization")
                .as_object()
                .expect("object")
                .clone(),
        )
        .expect("provider state")
        .with_required_persistence()
    }
}

pub(crate) fn identity(
    body: &mut Value,
    profile: &Effective,
    key: &str,
    account: &ProviderAccount,
    explicit: Option<&str>,
    restored: Option<Identity>,
) -> Result<Identity, ProviderError> {
    let identity = if let Some(restored) = restored {
        if explicit.is_some_and(|session| session != restored.session) {
            return Err(error("cprs_thread_identity_changed"));
        }
        restored
    } else {
        let session = if let Some(session) = explicit {
            if !uuid(session) {
                return Err(error("cprs_identity_invalid"));
            }
            session.into()
        } else {
            random_uuid()?
        };
        let device = hex::encode(Sha256::digest(format!(
            "cprs:claude-device:v1\0{key}\0{}\0{}",
            account.id(),
            profile.selection.installation_seed.as_deref().unwrap_or("")
        )));
        Identity { session, device }
    };
    let account_uuid = account
        .upstream_user_id()
        .filter(|id| uuid(id))
        .ok_or_else(|| error("cprs_account_identity_unavailable"))?;
    if body.get("metadata").is_some_and(|value| !value.is_object()) {
        return Err(error("cprs_identity_invalid"));
    }
    if body.get("metadata").is_none() {
        body["metadata"] = json!({});
    }
    let metadata = body["metadata"]
        .as_object_mut()
        .expect("validated metadata");
    let mut user = match metadata.get("user_id") {
        None => serde_json::Map::new(),
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(object)) => object,
            // Retain non-JSON client metadata as textual extras so veil can process it.
            _ => json!({"client_user_id":text})
                .as_object()
                .expect("object")
                .clone(),
        },
        Some(_) => return Err(error("cprs_identity_invalid")),
    };
    user.insert("device_id".into(), json!(identity.device));
    user.insert("account_uuid".into(), json!(account_uuid));
    user.insert("session_id".into(), json!(identity.session));
    metadata.insert("user_id".into(), json!(Value::Object(user).to_string()));
    Ok(identity)
}

fn random_uuid() -> Result<String, ProviderError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| error("cprs_identity_unavailable"))?;
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex = hex::encode(bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

pub(crate) fn eligible(message: &Value) -> bool {
    message["stop_reason"]
        .as_str()
        .is_some_and(|reason| reason != "refusal")
        && !message["usage"]["iterations"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["type"] == "fallback_message"))
}

pub(crate) fn error(code: &'static str) -> ProviderError {
    ProviderError::new(ProviderErrorKind::InvalidRequest, UpstreamSendState::NotSent).with_retry_prohibited()
        .with_diagnostic(ProviderDiagnostic::new("Claude request state is unavailable or has changed; start a new thread with full history.")
            .with_classification("anthropic_local", code))
}
