//! 启动模式共享的基础设施与 Provider 组装，不启动 HTTP 或 Worker

use std::sync::Arc;

use gateway_admin::ports::provider::ProviderAdminRegistry;
use gateway_core::{engine::provider::ProviderRegistry, time::DeploymentTimeZone};
use gateway_host::{HostBundle, HostConfig};
use gateway_store::{StoreBundle, StoreConfig};

use super::BootstrapError;

pub(super) struct Environment {
    pub(super) host: HostBundle,
    pub(super) store: StoreBundle,
    pub(super) timezone: DeploymentTimeZone,
}

impl Environment {
    pub(super) async fn initialize(
        host: HostConfig,
        store: StoreConfig,
    ) -> Result<Self, BootstrapError> {
        let timezone = host.timezone;
        let store = store.with_timezone(timezone);
        let host = gateway_host::initialize(host).await?;
        host.report_startup_ready("Host");
        let store = gateway_store::initialize(store).await?;
        host.report_startup_ready("Store");
        Ok(Self {
            host,
            store,
            timezone,
        })
    }
}

pub(super) struct Providers {
    pub(super) veil: Arc<gateway_veil::Runtime>,
    pub(super) openai: provider_openai::ProviderBundle,
    pub(super) anthropic: provider_anthropic::ProviderBundle,
    pub(super) core: ProviderRegistry,
    pub(super) admin: ProviderAdminRegistry,
    pub(super) peer: gateway_admin::PeerUpstreamBundle,
}

impl Providers {
    pub(super) async fn initialize(
        config: provider_openai::OpenAiConfig,
        veil_config: gateway_veil::Config,
        environment: &Environment,
    ) -> Result<Self, BootstrapError> {
        let ports = environment.store.provider_ports();
        let upstream_reader = Arc::new(gateway_core::account::UpstreamInfoReader::default());
        let openai = provider_openai::initialize(
            config.with_timezone(environment.timezone),
            ports.clone(),
            upstream_reader.clone(),
        )
        .await?;
        environment.host.report_startup_ready("OpenAI Provider");
        let veil = gateway_veil::Runtime::initialize(veil_config).await;
        let anthropic = provider_anthropic::initialize(ports, veil.clone()).await?;
        if veil.ensure_ready().is_ok() {
            environment.host.report_startup_ready("Anthropic Provider");
        }
        let core = ProviderRegistry::new([openai.core_provider(), anthropic.core_provider()])?;
        let admin =
            ProviderAdminRegistry::new([openai.admin_provider(), anthropic.admin_provider()])
                .map_err(|_| gateway_admin::model::AdminError::invalid("Provider 管理身份冲突"))?;
        let peer = gateway_admin::initialize_peer_upstream(
            environment.store.provider_ports().accounts(),
            &admin,
            Arc::new(gateway_host::peer::HttpPeerTransport::with_client_builder(
                provider_openai::build_reqwest_client_with_custom_ca,
            )),
        )
        .map_err(|_| gateway_admin::model::AdminError::invalid("互联后台任务配置无效"))?;
        upstream_reader
            .bind(&peer.scheduling_source())
            .map_err(|_| gateway_admin::model::AdminError::invalid("互联观测端口重复绑定"))?;
        Ok(Self {
            peer,
            veil,
            openai,
            anthropic,
            core,
            admin,
        })
    }
}
