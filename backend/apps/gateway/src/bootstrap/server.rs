//! 服务模式完成原生 Provider、Core、Admin 和 API 组装后交给 Host 监听

use gateway_host::ConfigError;

use super::{
    BootstrapError, GatewayConfig,
    startup::{Environment, Providers},
};

/// 首次快照和准入恢复成功后才开放监听。
pub async fn run() -> Result<(), BootstrapError> {
    let config = gateway_host::load_config::<GatewayConfig>()?;
    let backup_staging = config.store.backup_staging_dir().to_path_buf();
    let environment = Environment::initialize(config.host, config.store).await?;
    let mut providers = Providers::initialize(config.openai, config.veil, &environment).await?;
    let Environment {
        host,
        mut store,
        timezone,
    } = environment;
    let prompt_guard = std::sync::Arc::new(
        gateway_host::prompt_guard::HostPromptGuard::new(
            store.admin_ports().settings(),
            providers.veil.clone(),
        )
        .map(|guard| guard.with_store(store.prompt_guard()))
        .map_err(ConfigError::InvalidField)?,
    );
    let mut core = gateway_core::prepare(store.core_ports(), providers.core)
        .with_prompt_guard(prompt_guard.clone())
        .activate()
        .await?;
    host.report_startup_ready("Core");
    let analytics_worker = store.request_analytics_worker(std::sync::Arc::new(
        gateway_admin::ProviderUsageBillingProjector::new(providers.admin.clone()),
    ))?;
    let mut admin = gateway_admin::initialize(
        config.admin,
        config.client,
        // veil 映射卷与数据库归档在同一备份记录中导出。
        store
            .admin_ports()
            .with_backup_privacy_store(std::sync::Arc::new(
                gateway_host::privacy_backup::VeilBackupSnapshot::new(
                    providers.veil.clone(),
                    backup_staging,
                ),
            )),
        gateway_admin::AdminRuntimePorts {
            published_snapshot: core.snapshots(),
            upstream_info: Some(providers.peer.service()),
            timezone,
            providers: providers.admin,
            pricing_source: std::sync::Arc::new(gateway_host::pricing::ModelsDevPricing),
            snapshot: core.snapshot_control(),
            account_probe: core.account_probe(),
            proxy_probe: host.proxy_probe(provider_openai::build_reqwest_client_with_custom_ca),
            client_distribution: host.client_distribution_resolver(),
            system: host.system_operations(),
            client_key_verifier: core.client_key_verifier(),
        },
    )
    .await?;
    host.report_startup_ready("Admin");

    let mut probes = store.health_probes();
    probes.extend(core.health_probes());
    probes.push(host.logging_health_probe());
    let api = gateway_api::initialize(
        store.peer_routing_id().await?,
        config.api,
        core.execution_service(),
        admin.services(),
        probes,
        host.worker_health(),
        host.connection_lifecycle(),
        store.diagnostics(),
    )?
    .with_claude_privacy(providers.veil.clone())
    .with_prompt_guard_control(prompt_guard);
    host.report_startup_ready("API");

    let mut plan = store.take_worker_contributions();
    plan.push(analytics_worker);
    plan.push(gateway_host::retention::worker(store.retention())?);
    plan.extend(core.take_worker_contributions());
    plan.extend(providers.openai.take_worker_contributions());
    plan.extend(providers.anthropic.take_worker_contributions());
    plan.extend(admin.take_worker_contributions());
    plan.extend(providers.peer.take_worker_contributions());
    host.start_workers(plan, store.worker_leader_lease(), store.diagnostics())?;
    host.report_startup_ready("Workers");
    Ok(host.serve(api.router()).await?)
}
