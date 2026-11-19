//! Claude 订阅 OAuth、原生 Messages 与管理能力的组合边界

mod admin;
mod checkpoint;
mod credential;
mod profile;
mod provider;
mod quota;
mod reset_credits;
mod signing;
mod thread;
mod transport;
mod usage;
mod workers;

use std::sync::Arc;

use gateway_admin::ports::provider::ProviderAdmin;
use gateway_core::{
    engine::provider::Provider, provider_ports::ProviderStorePorts, task::WorkerContribution,
};

pub(crate) const NAME: &str = "anthropic";
pub(crate) const API: &str = "https://api.anthropic.com";
pub(crate) const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub(crate) const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub(crate) const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
pub(crate) const SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers";
pub(crate) const OAUTH_BETA: &str = "oauth-2025-04-20";
pub(crate) const MAX_BODY: usize = 32 * 1024 * 1024;

pub struct ProviderBundle {
    provider: Arc<provider::AnthropicProvider>,
    admin: Arc<admin::AnthropicAdmin>,
    workers: Vec<WorkerContribution>,
}

#[derive(Debug, thiserror::Error)]
pub enum AnthropicInitializeError {
    #[error(transparent)]
    Worker(#[from] gateway_core::task::WorkerDefinitionError),
}

pub async fn initialize(
    ports: ProviderStorePorts,
    veil: Arc<gateway_veil::Runtime>,
) -> Result<ProviderBundle, AnthropicInitializeError> {
    let credentials = Arc::new(credential::Credentials::new(ports.clone(), veil));
    credentials.profiles.restore().await;
    let provider = Arc::new(provider::AnthropicProvider::new(credentials.clone(), ports));
    let admin = Arc::new(admin::AnthropicAdmin::new(credentials.clone()));
    let workers = workers::contributions(credentials, provider.clone())?;
    Ok(ProviderBundle {
        provider,
        admin,
        workers,
    })
}

impl ProviderBundle {
    pub fn core_provider(&self) -> Arc<dyn Provider> {
        self.provider.clone()
    }
    pub fn admin_provider(&self) -> Arc<dyn ProviderAdmin> {
        self.admin.clone()
    }
    pub fn take_worker_contributions(&mut self) -> Vec<WorkerContribution> {
        std::mem::take(&mut self.workers)
    }
}
