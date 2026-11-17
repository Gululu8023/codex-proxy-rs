//! 交给宿主监督的 Anthropic OAuth 续期、额度复核与目录任务

use std::{
    num::NonZeroU32,
    sync::Arc,
    time::{Duration, SystemTime},
};

use futures::future::BoxFuture;
use gateway_core::{
    account::ProviderRefreshQuery,
    task::{
        ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerDefinitionError, WorkerId,
        WorkerKind, WorkerRegistration, WorkerRunnable, WorkerSchedule, WorkerTaskError,
    },
};

use crate::{
    credential::{CredentialError, Credentials},
    provider::AnthropicProvider,
};

pub(crate) fn contributions(
    credentials: Arc<Credentials>,
    provider: Arc<AnthropicProvider>,
) -> Result<Vec<WorkerContribution>, WorkerDefinitionError> {
    let schedule = WorkerSchedule::try_new(
        Duration::from_secs(30),
        Duration::from_secs(5),
        Duration::from_secs(300),
        Duration::from_secs(120),
        Duration::from_secs(30),
    )?;
    let quota_credentials = credentials.clone();
    let releases = credentials.profiles.clone();
    let release_id = WorkerId::try_new(WorkerKind::QuotaCatalogHealth, "anthropic-client-release")?;
    Ok(vec![
        WorkerContribution::Registration(WorkerRegistration::try_new(
            WorkerId::try_new(WorkerKind::OAuthRefresh, "anthropic")?,
            WorkerRunnable::Scheduled {
                schedule,
                lease: None,
                task: Box::new(RefreshTask { credentials }),
            },
        )?),
        WorkerContribution::Registration(WorkerRegistration::try_new(
            WorkerId::try_new(WorkerKind::QuotaCatalogHealth, "anthropic")?,
            WorkerRunnable::Scheduled {
                schedule: WorkerSchedule::try_new(
                    Duration::from_secs(60),
                    Duration::from_secs(5),
                    Duration::from_secs(300),
                    Duration::from_secs(120),
                    Duration::from_secs(30),
                )?,
                lease: None,
                task: Box::new(CatalogTask {
                    provider,
                    credentials: quota_credentials,
                }),
            },
        )?),
        WorkerContribution::Registration(WorkerRegistration::try_new(
            release_id.clone(),
            WorkerRunnable::Scheduled {
                schedule: WorkerSchedule::try_new(
                    Duration::from_secs(3600),
                    Duration::from_secs(60),
                    Duration::from_secs(3600),
                    Duration::from_secs(900),
                    Duration::from_secs(300),
                )?,
                lease: Some(gateway_core::task::WorkerLeaseRequest::try_new(
                    release_id,
                    Duration::from_secs(900),
                )?),
                task: Box::new(ReleaseTask { profiles: releases }),
            },
        )?),
    ])
}

struct RefreshTask {
    credentials: Arc<Credentials>,
}

impl ScheduledTask for RefreshTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let now = SystemTime::now();
            let policy = self
                .credentials
                .ports
                .runtime_policy()
                .load_refresh_policy()
                .await
                .map_err(|_| WorkerTaskError::safe("Anthropic refresh policy unavailable"))?;
            let candidates = self
                .credentials
                .ports
                .accounts()
                .list_refresh_candidates(ProviderRefreshQuery::new(
                    self.credentials.kind(),
                    now + policy.staggered_refresh_bound(),
                    now,
                    now,
                    vec![],
                    NonZeroU32::new(100).expect("nonzero"),
                ))
                .await
                .map_err(|_| WorkerTaskError::safe("Anthropic refresh candidate query failed"))?;
            let mut failed = false;
            for loaded in candidates {
                if loaded
                    .account
                    .access_token_expires_at()
                    .is_some_and(|expiry| {
                        !policy.is_refresh_due_staggered(loaded.account.id(), expiry, now)
                    })
                {
                    continue;
                }
                tokio::select! {
                    _ = context.cancellation().cancelled() => return Ok(()),
                    result = self.credentials.refresh(loaded.account.id()) => {
                        match result {
                            Ok(()) | Err(CredentialError::Busy) => {},
                            Err(CredentialError::Revoked) => {},
                            Err(_) => failed = true,
                        }
                    }
                }
            }
            if failed {
                Err(WorkerTaskError::safe("Anthropic credential refresh failed"))
            } else {
                Ok(())
            }
        })
    }
}

struct CatalogTask {
    provider: Arc<AnthropicProvider>,
    credentials: Arc<Credentials>,
}
impl ScheduledTask for CatalogTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            tokio::select! {
                _ = context.cancellation().cancelled() => Ok(()),
                result = async {
                    self.credentials.profiles.restore().await;
                    // 额度查询慢或失败不阻止目录采集；两者仍共用本周期的取消与期限。
                    let (quota, catalog) = tokio::join!(crate::quota::synchronize(&self.credentials), self.provider.refresh_catalog());
                    if quota.is_err() || catalog.is_err() {Err(WorkerTaskError::safe("Anthropic quota or catalog refresh failed"))} else {Ok(())}
                } => result,
            }
        })
    }
}

struct ReleaseTask {
    profiles: Arc<crate::profile::Profiles>,
}
impl ScheduledTask for ReleaseTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            tokio::select! {
                _ = context.cancellation().cancelled() => Ok(()),
                result = self.profiles.refresh() => result.map_err(WorkerTaskError::safe),
            }
        })
    }
}
