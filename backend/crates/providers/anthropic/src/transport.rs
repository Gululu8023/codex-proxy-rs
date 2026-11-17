//! Claude 唯一出站边界；复用连接池，发送前重新核验同一 veil 运行时。

use crate::credential::CredentialError;
use gateway_core::account::OutboundProxy;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

pub(crate) enum Purpose {
    Inference,
    Control(gateway_veil::ControlPurpose),
}
pub(crate) enum SendError {
    Privacy(gateway_veil::Error),
    Http(reqwest::Error),
}
impl SendError {
    pub fn ambiguous(&self) -> bool {
        matches!(self, Self::Http(error) if !error.is_connect() && !error.is_builder())
    }
}

pub(crate) struct Transport {
    veil: Arc<gateway_veil::Runtime>,
    clients: Mutex<BTreeMap<[u8; 32], reqwest::Client>>,
}
impl Transport {
    pub fn new(veil: Arc<gateway_veil::Runtime>) -> Self {
        Self {
            veil,
            clients: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn client(
        &self,
        proxy: Option<&OutboundProxy>,
    ) -> Result<reqwest::Client, CredentialError> {
        self.veil
            .ensure_ready()
            .map_err(|_| CredentialError::Unavailable)?;
        let key: [u8; 32] =
            Sha256::digest(proxy.map_or("", |proxy| proxy.expose_url()).as_bytes()).into();
        let mut clients = self
            .clients
            .lock()
            .map_err(|_| CredentialError::Unavailable)?;
        if let Some(client) = clients.get(&key) {
            return Ok(client.clone());
        }
        // 与 Codex 共用账号出口规则：未绑定代理时直连，不继承环境代理。
        let builder = gateway_egress::account_http_client(proxy)
            .map_err(|_| CredentialError::Invalid)?
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15));
        let client = builder.build().map_err(|_| CredentialError::Unavailable)?;
        // 代理设置变更不无限积累池；在途请求持有 Client 克隆，不受缓存移除影响。
        if clients.len() >= 64 {
            clients.pop_first();
        }
        clients.insert(key, client.clone());
        Ok(client)
    }

    pub async fn send(
        &self,
        request: reqwest::RequestBuilder,
        purpose: Purpose,
    ) -> Result<reqwest::Response, SendError> {
        match purpose {
            Purpose::Inference => self.veil.ensure_ready(),
            Purpose::Control(purpose) => self.veil.control(purpose),
        }
        .map_err(SendError::Privacy)?;
        // OAuth/profile 等控制字段为协议数据，不应用自由文本替换，但不允许绕过故障门禁。
        request.send().await.map_err(SendError::Http)
    }
}

#[cfg(test)]
mod tests;
