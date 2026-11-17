//! OAuth/刷新的一次性交换检查点；Redis 领取权与密文共同绑定 flow 和 owner。

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use gateway_admin::model::{AdminError, provider_credentials::AuthorizationCommitGuard};
use gateway_core::{
    account::OpaqueProviderData,
    provider_ports::{
        NewOAuthPendingFlow, OAuthPendingBinding, OAuthPendingClaimOutcome,
        OAuthPendingConsumeOutcome, OAuthPendingPutOutcome, OAuthPendingReleaseOutcome,
        ProviderStorePorts,
    },
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

use crate::credential::{CredentialError, Credentials};

fn kind() -> gateway_core::routing::ProviderKind {
    gateway_core::routing::ProviderKind::new(crate::NAME).expect("provider")
}

pub(crate) struct Claim {
    ports: ProviderStorePorts,
    veil: Arc<gateway_veil::Runtime>,
    flow: OAuthPendingBinding,
    owner: OAuthPendingBinding,
    claim: OAuthPendingBinding,
    active: bool,
}

fn binding(value: String) -> Result<OAuthPendingBinding, CredentialError> {
    OAuthPendingBinding::try_new(value).map_err(|_| CredentialError::Invalid)
}
fn aad(flow: &OAuthPendingBinding, owner: &OAuthPendingBinding) -> Vec<u8> {
    serde_json::to_vec(&[crate::NAME, flow.expose_to_store(), owner.expose_to_store()])
        .expect("string bindings")
}
fn seal(
    veil: &gateway_veil::Runtime,
    flow: &OAuthPendingBinding,
    owner: &OAuthPendingBinding,
    value: &Value,
) -> Result<OpaqueProviderData, CredentialError> {
    let plaintext = serde_json::to_vec(value).map_err(|_| CredentialError::Invalid)?;
    if plaintext.len() > 512 * 1024 {
        return Err(CredentialError::Invalid);
    }
    let ciphertext = veil
        .seal_checkpoint(&aad(flow, owner), &plaintext)
        .map_err(|_| CredentialError::Unavailable)?;
    Ok(OpaqueProviderData::new(
        json!({"checkpointV1": URL_SAFE_NO_PAD.encode(ciphertext)})
            .as_object()
            .expect("object")
            .clone(),
    ))
}

impl Claim {
    pub async fn put(
        credentials: &Credentials,
        flow: String,
        owner: String,
        value: &Value,
        ttl: Duration,
    ) -> Result<OAuthPendingPutOutcome, CredentialError> {
        let flow = binding(flow)?;
        let owner = binding(owner)?;
        let payload = seal(&credentials.veil, &flow, &owner, value)?;
        let pending = NewOAuthPendingFlow::try_new(credentials.kind(), flow, owner, ttl, payload)
            .map_err(|_| CredentialError::Invalid)?;
        credentials
            .ports
            .oauth_pending()
            .put_if_absent(pending)
            .await
            .map_err(|_| CredentialError::Unavailable)
    }

    pub async fn acquire(
        credentials: &Credentials,
        flow: String,
        owner: String,
    ) -> Result<(Self, Value), CredentialError> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| CredentialError::Unavailable)?;
        let mut guard = Self {
            ports: credentials.ports.clone(),
            veil: credentials.veil.clone(),
            flow: binding(flow)?,
            owner: binding(owner)?,
            claim: binding(URL_SAFE_NO_PAD.encode(random))?,
            active: false,
        };
        let payload = match guard
            .ports
            .oauth_pending()
            .claim_if_owner(
                &credentials.kind(),
                &guard.flow,
                &guard.owner,
                &guard.claim,
                Duration::from_secs(120),
            )
            .await
            .map_err(|_| CredentialError::Unavailable)?
        {
            OAuthPendingClaimOutcome::Claimed(payload) => payload,
            OAuthPendingClaimOutcome::InProgress => return Err(CredentialError::Busy),
            _ => return Err(CredentialError::Invalid),
        };
        guard.active = true;
        let ciphertext = payload
            .expose_to_provider()
            .get("checkpointV1")
            .and_then(Value::as_str)
            .ok_or(CredentialError::Invalid)?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(ciphertext)
            .map_err(|_| CredentialError::Invalid)?;
        let plaintext = guard
            .veil
            .open_checkpoint(&aad(&guard.flow, &guard.owner), &ciphertext)
            .map_err(|_| CredentialError::Unavailable)?;
        let value = serde_json::from_slice(&plaintext).map_err(|_| CredentialError::Invalid)?;
        Ok((guard, value))
    }

    pub async fn save(&self, value: &Value) -> Result<(), CredentialError> {
        let payload = seal(&self.veil, &self.flow, &self.owner, value)?;
        let updated = self
            .ports
            .oauth_pending()
            .checkpoint_claim(&kind(), &self.flow, &self.owner, &self.claim, &payload)
            .await
            .map_err(|_| CredentialError::Unavailable)?;
        if !updated {
            return Err(CredentialError::Busy);
        }
        Ok(())
    }

    pub async fn release(mut self) -> Result<(), CredentialError> {
        let result = self
            .ports
            .oauth_pending()
            .release_claim(&kind(), &self.flow, &self.owner, &self.claim)
            .await;
        self.active = false;
        match result {
            Ok(OAuthPendingReleaseOutcome::Released | OAuthPendingReleaseOutcome::NotFound) => {
                Ok(())
            }
            Ok(_) => Err(CredentialError::Busy),
            Err(_) => Err(CredentialError::Unavailable),
        }
    }
}

#[async_trait]
impl AuthorizationCommitGuard for Claim {
    async fn commit(mut self: Box<Self>) -> Result<(), AdminError> {
        let result = self
            .ports
            .oauth_pending()
            .consume_claim(&kind(), &self.flow, &self.owner, &self.claim)
            .await;
        self.active = false;
        match result {
            Ok(OAuthPendingConsumeOutcome::Consumed) => Ok(()),
            _ => Err(AdminError::unavailable("授权回执消费失败")),
        }
    }
    async fn abort(self: Box<Self>) -> Result<(), AdminError> {
        (*self)
            .release()
            .await
            .map_err(|_| AdminError::unavailable("授权流程释放失败"))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if self.active {
            // 取消只释放领取权，绝不清除已开始交换或已取得 token 的持久阶段。
            let (port, flow, owner, claim) = (
                self.ports.oauth_pending(),
                self.flow.clone(),
                self.owner.clone(),
                self.claim.clone(),
            );
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                drop(runtime.spawn(async move {
                    let _ = port.release_claim(&kind(), &flow, &owner, &claim).await;
                }));
            }
        }
    }
}
