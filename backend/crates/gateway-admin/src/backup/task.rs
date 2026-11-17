//! Backup Worker 贡献：单个可取消 `DaemonTask` 承担调度、执行、删除收敛与保留清理
//!
//! 当前部署边界是单副本，因此不需要 Redis lease、fencing token 或 heartbeat
//! 长时间
//! `pg_dump` 与上传由本 Daemon 自身持有；Host 只负责 panic 后重启、健康与关闭

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use gateway_core::{
    lifecycle::CancellationToken,
    task::{
        DaemonRestartPolicy, DaemonTask, WorkerContribution, WorkerDaemonContext, WorkerId,
        WorkerKind, WorkerRegistration, WorkerRunnable, WorkerTaskError,
    },
};
use tracing::{info, warn};

use crate::{
    model::AdminError,
    model::backup::{
        BackupError, BackupObject, BackupObjectMetadata, BackupRecord, BackupStatus,
        BackupStatusTransition, BackupStorageConfig, BackupTriggerKind, build_backup_seed,
        build_privacy_store_object_key, code,
    },
    ports::backup::{
        BackupObjectStorePort, BackupRepository, DatabaseDumpPort, DumpRequest, NoPrivacyStore,
        PrivacyStoreSnapshotPort, StatusTransitionUpdate, UploadObjectRequest,
    },
};

impl From<BackupError> for WorkerTaskError {
    fn from(error: BackupError) -> Self {
        WorkerTaskError::safe(error.message()).with_source(error)
    }
}

use super::policy::{BackupSchedule, decide_retention};

const BACKUP_WORKER_OWNER: &str = "backup";

/// 循环间隔：管理员修改 Cron 后该时长内生效；无工作时可取消等待
const BACKUP_LOOP_INTERVAL: Duration = Duration::from_secs(30);
/// 每个循环最多执行的保留删除数量
const RETENTION_BATCH_SIZE: usize = 10;
/// 保留扫描每页读取的 completed 计划备份数量
const RETENTION_PAGE_SIZE: u32 = 1000;
/// 每个循环最多完成的待删除记录数量
const PENDING_DELETION_BATCH: u32 = 20;

/// 备份 Daemon 任务
pub struct BackupTask {
    timezone: gateway_core::time::DeploymentTimeZone,
    repository: Arc<dyn BackupRepository>,
    dump: Arc<dyn DatabaseDumpPort>,
    privacy_store: Arc<dyn PrivacyStoreSnapshotPort>,
    object_store: Arc<dyn BackupObjectStorePort>,
}

/// 已暂存、待上传到指定对象 key 的归档
struct StagedObject {
    object_key: String,
    path: std::path::PathBuf,
    size_bytes: u64,
    sha256: String,
}

impl StagedObject {
    fn new(object_key: String, path: std::path::PathBuf, size_bytes: u64, sha256: String) -> Self {
        Self {
            object_key,
            path,
            size_bytes,
            sha256,
        }
    }
}

impl BackupTask {
    /// 组合仓储、导出器与对象存储适配器
    #[must_use]
    pub fn new(
        repository: Arc<dyn BackupRepository>,
        dump: Arc<dyn DatabaseDumpPort>,
        object_store: Arc<dyn BackupObjectStorePort>,
    ) -> Self {
        Self {
            timezone: Default::default(),
            repository,
            dump,
            privacy_store: Arc::new(NoPrivacyStore),
            object_store,
        }
    }

    /// 接入隐私存储快照；未接入时视为部署没有映射卷
    #[must_use]
    pub fn with_privacy_store(mut self, privacy_store: Arc<dyn PrivacyStoreSnapshotPort>) -> Self {
        self.privacy_store = privacy_store;
        self
    }

    #[must_use]
    pub fn with_timezone(mut self, timezone: gateway_core::time::DeploymentTimeZone) -> Self {
        self.timezone = timezone;
        self
    }

    pub(crate) fn into_worker(self) -> Result<WorkerContribution, AdminError> {
        let id = WorkerId::try_new(WorkerKind::Backup, BACKUP_WORKER_OWNER)
            .map_err(|_| AdminError::internal("备份 Worker ID 不合法"))?;
        let restart = DaemonRestartPolicy::try_new(Duration::from_secs(1), Duration::from_secs(60))
            .map_err(|_| AdminError::internal("备份 Worker 重启策略不合法"))?;
        let registration = WorkerRegistration::try_new(
            id,
            WorkerRunnable::Daemon {
                restart,
                task: Box::new(self),
            },
        )
        .map_err(|_| AdminError::internal("备份 Worker 注册信息不合法"))?;
        Ok(WorkerContribution::Registration(registration))
    }
}

impl DaemonTask for BackupTask {
    fn run(&self, context: WorkerDaemonContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move { self.run_daemon(context).await })
    }
}

impl BackupTask {
    async fn run_daemon(&self, context: WorkerDaemonContext) -> Result<(), WorkerTaskError> {
        let cancellation = context.cancellation();
        loop {
            if cancellation.is_cancelled() {
                return Ok(());
            }
            self.run_cycle(cancellation).await?;
            context.succeeded();

            tokio::select! {
                _ = cancellation.cancelled() => return Ok(()),
                _ = tokio::time::sleep(BACKUP_LOOP_INTERVAL) => {}
            }
        }
    }

    /// 执行一个周期：推进计划、恢复中间态、删除收敛、领取执行一个任务、保留清理
    ///
    /// 对外暴露以便集成测试逐周期驱动；daemon 循环内部复用
    ///
    /// # Errors
    ///
    /// 仓储或基础设施不可用、任务执行失败时返回 [`WorkerTaskError`]
    pub async fn run_cycle(&self, cancellation: &CancellationToken) -> Result<(), WorkerTaskError> {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        self.advance_schedule(Utc::now()).await?;
        self.recover_intermediate(cancellation).await?;
        self.finish_pending_deletions().await?;
        self.process_next_task(cancellation).await?;
        self.run_retention_batch(Utc::now(), cancellation).await?;
        Ok(())
    }

    /// 推进到期计划并幂等插入 scheduled 任务
    async fn advance_schedule(&self, now: DateTime<Utc>) -> Result<(), WorkerTaskError> {
        let settings = self.repository.load_settings().await.map_err(repo_error)?;
        if !settings.schedule_enabled {
            return Ok(());
        }
        let cron = settings.cron_expression.as_deref().unwrap_or_default();
        let timezone = self.timezone.name();
        if cron.is_empty() {
            return Ok(());
        }
        let schedule = BackupSchedule::parse(cron, self.timezone).map_err(|source| {
            WorkerTaskError::safe("backup schedule is invalid").with_source(source)
        })?;

        if settings.schedule_timezone.as_deref() != Some(timezone) {
            // 时区切换只初始化未来游标，不把旧时区计划当作应补跑任务
            if let Some(next_run_at) = schedule.next_after(now) {
                self.repository
                    .advance_schedule_cursor(
                        next_run_at,
                        cron,
                        settings.schedule_timezone.as_deref(),
                        settings.next_run_at,
                        timezone,
                    )
                    .await
                    .map_err(repo_error)?;
            }
            return Ok(());
        }
        let due = match settings.next_run_at {
            Some(next_run_at) if next_run_at <= now => true,
            Some(_) => false,
            None => {
                // 刚启用计划但尚无游标：直接初始化
                if let Some(next_run_at) = schedule.next_after(now) {
                    let _ = self
                        .repository
                        .advance_schedule_cursor(
                            next_run_at,
                            cron,
                            Some(timezone),
                            settings.next_run_at,
                            timezone,
                        )
                        .await
                        .map_err(repo_error)?;
                }
                return Ok(());
            }
        };
        if !due {
            return Ok(());
        }

        let scheduled_at = match schedule.last_firing_at_or_before(now) {
            Some(scheduled_at) => scheduled_at,
            None => {
                // 无法从计划推导最近到期时间：推进游标避免反复触发
                if let Some(next_run_at) = schedule.next_after(now) {
                    let _ = self
                        .repository
                        .advance_schedule_cursor(
                            next_run_at,
                            cron,
                            Some(timezone),
                            settings.next_run_at,
                            timezone,
                        )
                        .await
                        .map_err(repo_error)?;
                }
                return Ok(());
            }
        };
        let prefix = settings.prefix.as_deref().unwrap_or_default();
        let expires_at = if settings.retention_days > 0 {
            Some(now + chrono::Duration::days(i64::from(settings.retention_days)))
        } else {
            None
        };
        let seed = build_backup_seed(
            BackupTriggerKind::Scheduled,
            Some(scheduled_at),
            prefix,
            now,
            expires_at,
        )
        .map_err(infra_error)?;
        let inserted = self
            .repository
            .insert_scheduled_record(
                seed,
                schedule.next_after(now),
                cron,
                timezone,
                settings.next_run_at,
            )
            .await
            .map_err(repo_error)?;
        if inserted {
            info!(scheduled_at = %scheduled_at, "计划备份任务已创建");
        } else {
            warn!(scheduled_at = %scheduled_at, "计划已变化或存在冲突，本轮未创建任务");
        }
        Ok(())
    }

    /// 恢复中间状态（dumping/uploading/deleting）
    async fn recover_intermediate(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerTaskError> {
        let records = self
            .repository
            .list_intermediate_records()
            .await
            .map_err(repo_error)?;
        for record in records {
            match record.status {
                BackupStatus::Dumping => self.recover_dumping(&record, cancellation).await?,
                BackupStatus::Uploading => self.recover_uploading(&record, cancellation).await?,
                _ => {}
            }
        }
        Ok(())
    }

    /// dumping：有完整数据库暂存则补齐隐私存储快照并继续上传，否则清理暂存并标记失败
    ///
    /// 映射只增不删，重启后重新生成的快照仍覆盖先前数据库快照引用的映射
    async fn recover_dumping(
        &self,
        record: &BackupRecord,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerTaskError> {
        let Some(artifact) = self.dump.inspect_staging(&record.id).await? else {
            return self
                .fail_task(record, code::PG_DUMP_FAILED, "导出进程中断，暂存归档不完整")
                .await;
        };
        let privacy = match self.privacy_store.inspect_staging(&record.id).await? {
            Some(staged) => Some(StagedObject::new(
                String::new(),
                staged.path,
                staged.size_bytes,
                staged.sha256,
            )),
            None => match self.snapshot_privacy_store(record, cancellation).await {
                Ok(privacy) => privacy,
                Err((error_code, message)) => {
                    return self.fail_task(record, error_code, message).await;
                }
            },
        };
        let database = StagedObject::new(
            record.object_key.clone(),
            artifact.path,
            artifact.size_bytes,
            artifact.sha256,
        );
        self.start_upload(record, database, privacy, cancellation)
            .await
    }

    /// uploading：远端全部匹配则补记完成，有完整暂存则重试上传，否则失败
    async fn recover_uploading(
        &self,
        record: &BackupRecord,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerTaskError> {
        let settings = self.repository.load_settings().await.map_err(repo_error)?;
        let Some(config) = BackupStorageConfig::from_settings(&settings) else {
            return Ok(());
        };
        if self.remote_objects_match(&config, record).await? {
            let now = Utc::now();
            if self
                .repository
                .transition_status(
                    &record.id,
                    status_transition(BackupStatus::Uploading, BackupStatus::Completed)?,
                    StatusTransitionUpdate::default(),
                    now,
                )
                .await
                .map_err(repo_error)?
                .is_some()
            {
                self.cleanup_staging(&record.id).await;
            }
            return Ok(());
        }
        let database = self.dump.inspect_staging(&record.id).await?;
        let privacy = match &record.privacy_store {
            Some(expected) => match self.privacy_store.inspect_staging(&record.id).await? {
                Some(staged) => Some(StagedObject::new(
                    expected.object_key.clone(),
                    staged.path,
                    staged.size_bytes,
                    staged.sha256,
                )),
                None => {
                    return self
                        .fail_task(
                            record,
                            code::S3_UPLOAD_FAILED,
                            "远端对象缺失且本地无完整暂存",
                        )
                        .await;
                }
            },
            None => None,
        };
        let Some(database) = database else {
            return self
                .fail_task(
                    record,
                    code::S3_UPLOAD_FAILED,
                    "远端对象缺失且本地无完整暂存",
                )
                .await;
        };
        let mut objects = vec![StagedObject::new(
            record.object_key.clone(),
            database.path,
            database.size_bytes,
            database.sha256,
        )];
        objects.extend(privacy);
        self.upload_and_verify(record, objects, Utc::now(), cancellation)
            .await
    }

    /// 已记录的全部远端对象存在且大小与 sha256 一致
    async fn remote_objects_match(
        &self,
        config: &BackupStorageConfig,
        record: &BackupRecord,
    ) -> Result<bool, WorkerTaskError> {
        let (Some(size_bytes), Some(sha256)) = (record.size_bytes, record.sha256.as_deref()) else {
            return Ok(false);
        };
        let mut expected = vec![(record.object_key.as_str(), size_bytes, sha256)];
        if let Some(privacy) = &record.privacy_store {
            expected.push((
                privacy.object_key.as_str(),
                privacy.size_bytes,
                &privacy.sha256,
            ));
        }
        for (object_key, size_bytes, sha256) in expected {
            match self.object_store.head_object(config, object_key).await? {
                Some(remote) if remote.size_bytes == size_bytes && remote.sha256 == sha256 => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    /// 完成待删除记录（deleting → DeleteObject → 硬删除）
    async fn finish_pending_deletions(&self) -> Result<(), WorkerTaskError> {
        let settings = self.repository.load_settings().await.map_err(repo_error)?;
        let Some(config) = BackupStorageConfig::from_settings(&settings) else {
            return Ok(());
        };
        let records = self
            .repository
            .list_pending_deletions(PENDING_DELETION_BATCH)
            .await
            .map_err(repo_error)?;
        for record in records {
            if self.delete_objects(&config, &record).await.is_ok() {
                self.repository
                    .delete_record(&record.id)
                    .await
                    .map_err(repo_error)?;
            }
        }
        Ok(())
    }

    /// 删除记录的全部远端对象；隐私存储先删，数据库归档最后删除，
    /// 任一失败时保留记录以便下一轮重试
    async fn delete_objects(
        &self,
        config: &BackupStorageConfig,
        record: &BackupRecord,
    ) -> Result<(), BackupError> {
        if let Some(privacy) = &record.privacy_store {
            self.object_store
                .delete_object(config, &privacy.object_key)
                .await?;
        }
        self.object_store
            .delete_object(config, &record.object_key)
            .await
    }

    /// 领取一个 queued 任务并执行导出、上传与远端校验
    async fn process_next_task(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerTaskError> {
        let now = Utc::now();
        let Some(record) = self
            .repository
            .claim_next_queued(now)
            .await
            .map_err(repo_error)?
        else {
            return Ok(());
        };
        info!(
            backup_id = %record.id,
            attempt = record.attempt_count,
            "领取备份任务"
        );
        if let Err(error) = self.execute_task(&record, cancellation).await {
            warn!(backup_id = %record.id, error = %error, "备份任务执行失败");
        }
        Ok(())
    }

    /// 执行单个任务；内部处理全部状态迁移
    async fn execute_task(
        &self,
        record: &BackupRecord,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerTaskError> {
        let settings = self.repository.load_settings().await.map_err(repo_error)?;
        if BackupStorageConfig::from_settings(&settings).is_none() {
            return self
                .fail_task(record, code::INVALID_CONFIG, "备份存储配置不完整")
                .await;
        }
        if !settings.storage_verified() {
            return self
                .fail_task(record, code::INVALID_CONFIG, "备份存储配置尚未通过连接测试")
                .await;
        }

        let artifact = match self
            .dump
            .dump(DumpRequest {
                backup_id: record.id.clone(),
                cancellation: cancellation.clone(),
            })
            .await
        {
            Ok(artifact) => artifact,
            Err(error) => {
                let (error_code, message) = classify_dump_error(error);
                return self.fail_task(record, error_code, message).await;
            }
        };
        // 隐私存储快照必须晚于数据库快照，才能覆盖数据库引用的全部映射
        let privacy = match self.snapshot_privacy_store(record, cancellation).await {
            Ok(privacy) => privacy,
            Err((error_code, message)) => return self.fail_task(record, error_code, message).await,
        };
        let database = StagedObject::new(
            record.object_key.clone(),
            artifact.path,
            artifact.size_bytes,
            artifact.sha256,
        );
        self.start_upload(record, database, privacy, cancellation)
            .await
    }

    /// 生成隐私存储快照；映射卷从未初始化时无需附属对象
    async fn snapshot_privacy_store(
        &self,
        record: &BackupRecord,
        cancellation: &CancellationToken,
    ) -> Result<Option<StagedObject>, (&'static str, &'static str)> {
        match self
            .privacy_store
            .snapshot(DumpRequest {
                backup_id: record.id.clone(),
                cancellation: cancellation.clone(),
            })
            .await
        {
            Ok(artifact) => Ok(artifact.map(|artifact| {
                StagedObject::new(
                    String::new(),
                    artifact.path,
                    artifact.size_bytes,
                    artifact.sha256,
                )
            })),
            Err(error) => {
                warn!(backup_id = %record.id, error_code = error.code(), "Claude 隐私存储快照失败");
                Err(match error.code() {
                    code::CANCELLED => (code::CANCELLED, "备份已取消"),
                    code::STAGING_SPACE_EXHAUSTED => {
                        (code::STAGING_SPACE_EXHAUSTED, "暂存磁盘空间不足")
                    }
                    _ => (
                        code::PRIVACY_STORE_FAILED,
                        "Claude 隐私存储快照失败，请检查 veil 状态后重试",
                    ),
                })
            }
        }
    }

    /// dumping → uploading：持久化全部对象的大小与 sha256 后上传
    async fn start_upload(
        &self,
        record: &BackupRecord,
        database: StagedObject,
        privacy: Option<StagedObject>,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerTaskError> {
        let now = Utc::now();
        let privacy = privacy.map(|object| StagedObject {
            object_key: build_privacy_store_object_key(&record.object_key),
            ..object
        });
        let update = StatusTransitionUpdate {
            size_bytes: Some(database.size_bytes),
            sha256: Some(database.sha256.clone()),
            privacy_store: privacy.as_ref().map(|object| BackupObject {
                object_key: object.object_key.clone(),
                size_bytes: object.size_bytes,
                sha256: object.sha256.clone(),
            }),
            ..Default::default()
        };
        let Some(uploading_record) = self
            .repository
            .transition_status(
                &record.id,
                status_transition(BackupStatus::Dumping, BackupStatus::Uploading)?,
                update,
                now,
            )
            .await
            .map_err(repo_error)?
        else {
            return Ok(());
        };
        let mut objects = vec![database];
        objects.extend(privacy);
        self.upload_and_verify(&uploading_record, objects, now, cancellation)
            .await
    }

    /// 上传 + 远端校验全部对象，内部完成终态迁移
    async fn upload_and_verify(
        &self,
        record: &BackupRecord,
        objects: Vec<StagedObject>,
        now: DateTime<Utc>,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerTaskError> {
        let settings = self.repository.load_settings().await.map_err(repo_error)?;
        let Some(config) = BackupStorageConfig::from_settings(&settings) else {
            return Ok(());
        };
        let store = self.object_store.clone();
        for object in &objects {
            let metadata = BackupObjectMetadata::new(
                record.id.clone(),
                object.sha256.clone(),
                now,
                object.size_bytes,
            );
            if let Err(error) = store
                .upload_file(
                    &config,
                    UploadObjectRequest {
                        object_key: object.object_key.clone(),
                        source: object.path.clone(),
                        metadata,
                        cancellation: cancellation.clone(),
                    },
                )
                .await
            {
                return self
                    .fail_uploaded_task(
                        record,
                        &config,
                        BackupError::new(
                            error.code(),
                            format!("上传到对象存储失败：{}", error.message()),
                        ),
                    )
                    .await;
            }
        }

        for object in &objects {
            let size_bytes = object.size_bytes;
            match store.head_object(&config, &object.object_key).await {
                Ok(Some(remote))
                    if remote.size_bytes == size_bytes && remote.sha256 == object.sha256 => {}
                Ok(Some(remote)) => {
                    let size_matches = remote.size_bytes == size_bytes;
                    let sha256_matches = remote.sha256 == object.sha256;
                    warn!(
                        backup_id = %record.id,
                        expected_size_bytes = size_bytes,
                        remote_size_bytes = remote.size_bytes,
                        sha256_matches,
                        "备份远端对象校验不一致"
                    );
                    return self
                        .fail_uploaded_task(
                            record,
                            &config,
                            BackupError::new(
                                code::REMOTE_VERIFICATION_FAILED,
                                match (size_matches, sha256_matches) {
                                    (false, true) => "远端对象大小不一致",
                                    (true, false) => "远端对象 sha256 校验值不一致",
                                    (false, false) => "远端对象大小和 sha256 校验值均不一致",
                                    (true, true) => "远端对象校验状态异常",
                                }
                                .to_owned(),
                            ),
                        )
                        .await;
                }
                Ok(None) => {
                    warn!(backup_id = %record.id, "上传完成后未找到远端对象");
                    // 其他对象可能已经上传，统一按已上传任务清理
                    return self
                        .fail_uploaded_task(
                            record,
                            &config,
                            BackupError::new(
                                code::REMOTE_VERIFICATION_FAILED,
                                "上传完成后未找到远端对象".to_owned(),
                            ),
                        )
                        .await;
                }
                Err(error) => {
                    return self
                        .fail_uploaded_task(
                            record,
                            &config,
                            BackupError::new(
                                error.code(),
                                format!("远端对象校验失败：{}", error.message()),
                            ),
                        )
                        .await;
                }
            }
        }
        if self
            .repository
            .transition_status(
                &record.id,
                status_transition(BackupStatus::Uploading, BackupStatus::Completed)?,
                StatusTransitionUpdate::default(),
                Utc::now(),
            )
            .await
            .map_err(repo_error)?
            .is_some()
        {
            info!(
                backup_id = %record.id,
                size_bytes = objects.iter().map(|object| object.size_bytes).sum::<u64>(),
                privacy_store = record.privacy_store.is_some(),
                "备份完成"
            );
            self.cleanup_staging(&record.id).await;
        }
        Ok(())
    }

    /// 清理失败并入原终态，后续清理沿用备份记录的到期策略
    async fn fail_uploaded_task(
        &self,
        record: &BackupRecord,
        config: &BackupStorageConfig,
        mut error: BackupError,
    ) -> Result<(), WorkerTaskError> {
        if let Err(cleanup) = self.delete_objects(config, record).await {
            error =
                error.with_cleanup_failure("DeleteObject", &cleanup, record.expires_at.is_some());
        }
        self.fail_task(record, error.code(), error.message()).await
    }

    /// 把活跃任务写入失败终态并清理暂存
    async fn fail_task(
        &self,
        record: &BackupRecord,
        error_code: &'static str,
        message: impl Into<String>,
    ) -> Result<(), WorkerTaskError> {
        let message = message.into();
        warn!(
            backup_id = %record.id,
            error_code,
            error_message = %message,
            "备份任务失败"
        );
        let now = Utc::now();
        let update = StatusTransitionUpdate {
            error_code: Some(error_code.to_string()),
            error_message: Some(message),
            completed_at: Some(now),
            ..Default::default()
        };
        let _ = self
            .repository
            .transition_status(
                &record.id,
                status_transition(record.status, BackupStatus::Failed)?,
                update,
                now,
            )
            .await
            .map_err(repo_error)?;
        self.cleanup_staging(&record.id).await;
        Ok(())
    }

    async fn cleanup_staging(&self, backup_id: &str) {
        if let Err(error) = self.dump.cleanup_staging(backup_id).await {
            warn!(backup_id, error = %error, "清理备份暂存失败");
        }
        if let Err(error) = self.privacy_store.cleanup_staging(backup_id).await {
            warn!(backup_id, error = %error, "清理隐私存储暂存失败");
        }
    }

    /// 执行一小批到期保留清理：先处理 `expires_at` 已到期的记录，再按天数/份数清理计划备份
    async fn run_retention_batch(
        &self,
        now: DateTime<Utc>,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerTaskError> {
        let settings = self.repository.load_settings().await.map_err(repo_error)?;

        // 1. expires_at 已到期的记录（手动或计划）无条件进入删除
        let expired = self
            .repository
            .list_expired_records(RETENTION_BATCH_SIZE as u32)
            .await
            .map_err(repo_error)?;
        for record in expired {
            let Some(record) = self
                .repository
                .transition_to_deleting(&record.id, now, None)
                .await
                .map_err(repo_error)?
            else {
                continue;
            };
            self.finalize_deletion(&record).await?;
        }

        // 2. 计划备份按 retentionDays / retentionCount 清理
        if settings.retention_days == 0 && settings.retention_count == 0 {
            return Ok(());
        }
        let mut offset = 0;
        let mut decisions = Vec::new();
        loop {
            if cancellation.is_cancelled() {
                return Ok(());
            }
            let records = self
                .repository
                .list_scheduled_completed_desc(offset, RETENTION_PAGE_SIZE)
                .await
                .map_err(repo_error)?;
            let remaining = RETENTION_BATCH_SIZE - decisions.len();
            decisions.extend(
                decide_retention(
                    settings.retention_days,
                    settings.retention_count,
                    now,
                    offset,
                    &records,
                )
                .into_iter()
                .take(remaining),
            );
            if decisions.len() == RETENTION_BATCH_SIZE
                || records.len() < RETENTION_PAGE_SIZE as usize
            {
                break;
            }
            offset += records.len() as u64;
        }
        // 收齐本批候选后再删除，避免删除前页记录导致后续分页跳过数据
        for decision in decisions {
            let Some(record) = self
                .repository
                .transition_to_deleting(&decision.record_id, now, None)
                .await
                .map_err(repo_error)?
            else {
                continue;
            };
            self.finalize_deletion(&record).await?;
        }
        Ok(())
    }

    /// 对已进入 deleting 的记录执行对象删除并硬删除记录行
    async fn finalize_deletion(&self, record: &BackupRecord) -> Result<(), WorkerTaskError> {
        let settings = self.repository.load_settings().await.map_err(repo_error)?;
        let Some(config) = BackupStorageConfig::from_settings(&settings) else {
            return Ok(());
        };
        if self.delete_objects(&config, record).await.is_ok() {
            self.repository
                .delete_record(&record.id)
                .await
                .map_err(repo_error)?;
        }
        Ok(())
    }
}

fn classify_dump_error(error: BackupError) -> (&'static str, &'static str) {
    match error.code() {
        code::CANCELLED => (code::CANCELLED, "备份已取消"),
        code::ANALYTICS_NOT_READY => (
            code::ANALYTICS_NOT_READY,
            "用量统计正在重建，请在统计就绪后重试备份",
        ),
        code::STAGING_SPACE_EXHAUSTED => (code::STAGING_SPACE_EXHAUSTED, "暂存磁盘空间不足"),
        _ => (code::PG_DUMP_FAILED, "数据库导出失败"),
    }
}

fn status_transition(
    from: BackupStatus,
    to: BackupStatus,
) -> Result<BackupStatusTransition, WorkerTaskError> {
    BackupStatusTransition::try_new(from, to)
        .ok_or_else(|| WorkerTaskError::safe("backup status transition is invalid"))
}

fn repo_error(error: crate::ports::store::AdminStoreError) -> WorkerTaskError {
    WorkerTaskError::safe("backup store failed").with_source(error)
}

fn infra_error(error: BackupError) -> WorkerTaskError {
    WorkerTaskError::safe(error.message()).with_source(error)
}
