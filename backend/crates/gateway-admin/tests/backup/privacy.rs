//! 隐私存储快照随数据库归档进入同一条备份记录

use std::sync::Arc;

use chrono::Utc;
use gateway_admin::{
    backup::task::BackupTask,
    model::backup::{
        BackupRecordSeed, BackupStatus, BackupTriggerKind, build_privacy_store_object_key, code,
    },
    ports::backup::{BackupRepository, DatabaseDumpPort, DumpRequest, PrivacyStoreSnapshotPort},
};
use gateway_core::lifecycle::CancellationToken;

use super::support::{
    DUMP_CONTENT, FakeBackupRepository, FakeDumpPort, FakeObjectStore, FakePrivacyStore,
    PRIVACY_CONTENT, backup_id, configured_settings, sha256_hex, system_context,
};

struct Harness {
    repository: Arc<FakeBackupRepository>,
    dump: Arc<FakeDumpPort>,
    privacy: Arc<FakePrivacyStore>,
    object_store: Arc<FakeObjectStore>,
    seed: BackupRecordSeed,
}

impl Harness {
    async fn new(privacy: FakePrivacyStore) -> Self {
        let repository = Arc::new(FakeBackupRepository::new(configured_settings()));
        let seed = BackupRecordSeed {
            id: backup_id("privacy"),
            trigger_kind: BackupTriggerKind::Manual,
            scheduled_at: None,
            object_key: "codex/2026/11/16/codex-proxy-rs_20261116_000000_abcd1234.sql.gz"
                .to_owned(),
            expires_at: None,
        };
        repository
            .insert_backup_record(seed.clone(), &system_context())
            .await
            .expect("insert queued");
        Self {
            repository,
            dump: Arc::new(FakeDumpPort::new()),
            privacy: Arc::new(privacy),
            object_store: Arc::new(FakeObjectStore::new()),
            seed,
        }
    }

    fn task(&self) -> BackupTask {
        BackupTask::new(
            self.repository.clone(),
            self.dump.clone(),
            self.object_store.clone(),
        )
        .with_privacy_store(self.privacy.clone())
    }

    async fn run_cycle(&self) {
        self.task()
            .run_cycle(&CancellationToken::new())
            .await
            .expect("run cycle");
    }

    fn privacy_key(&self) -> String {
        build_privacy_store_object_key(&self.seed.object_key)
    }
}

#[test]
fn privacy_store_object_shares_the_database_archive_name() {
    assert_eq!(
        build_privacy_store_object_key("codex/2026/11/16/codex-proxy-rs_x_abcd1234.sql.gz"),
        "codex/2026/11/16/codex-proxy-rs_x_abcd1234.veil.tar.gz"
    );
}

#[tokio::test]
async fn backup_uploads_privacy_store_with_the_database_archive() {
    let harness = Harness::new(FakePrivacyStore::new()).await;
    harness.run_cycle().await;

    let record = &harness.repository.all_records()[0];
    assert_eq!(record.status, BackupStatus::Completed);
    assert_eq!(
        record.sha256.as_deref(),
        Some(sha256_hex(DUMP_CONTENT).as_str())
    );
    let privacy = record
        .privacy_store
        .as_ref()
        .expect("privacy store recorded");
    assert_eq!(privacy.object_key, harness.privacy_key());
    assert_eq!(privacy.size_bytes, PRIVACY_CONTENT.len() as u64);
    assert_eq!(privacy.sha256, sha256_hex(PRIVACY_CONTENT));
    assert_eq!(
        harness
            .object_store
            .object(&harness.seed.object_key)
            .as_deref(),
        Some(DUMP_CONTENT)
    );
    assert_eq!(
        harness
            .object_store
            .object(&harness.privacy_key())
            .as_deref(),
        Some(PRIVACY_CONTENT)
    );
    assert!(
        harness
            .dump
            .inspect_staging(&harness.seed.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        harness
            .privacy
            .inspect_staging(&harness.seed.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn uninitialized_privacy_store_backs_up_only_the_database() {
    let harness = Harness::new(FakePrivacyStore::new().uninitialized()).await;
    harness.run_cycle().await;

    let record = &harness.repository.all_records()[0];
    assert_eq!(record.status, BackupStatus::Completed);
    assert!(record.privacy_store.is_none());
    assert!(
        harness
            .object_store
            .object(&harness.privacy_key())
            .is_none()
    );
    assert_eq!(harness.privacy.snapshots.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn privacy_store_failure_fails_the_whole_backup_before_upload() {
    let harness = Harness::new(FakePrivacyStore::new().failing()).await;
    harness.run_cycle().await;

    let record = &harness.repository.all_records()[0];
    assert_eq!(record.status, BackupStatus::Failed);
    assert_eq!(
        record.error_code.as_deref(),
        Some(code::PRIVACY_STORE_FAILED)
    );
    assert!(record.privacy_store.is_none());
    assert!(harness.object_store.objects.lock().unwrap().is_empty());
    assert!(
        harness
            .dump
            .inspect_staging(&harness.seed.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn deletion_removes_both_objects_before_the_record() {
    let harness = Harness::new(FakePrivacyStore::new()).await;
    harness.run_cycle().await;
    harness
        .repository
        .transition_to_deleting(&harness.seed.id, Utc::now(), None)
        .await
        .unwrap()
        .expect("completed record enters deleting");
    harness.run_cycle().await;

    assert!(harness.repository.all_records().is_empty());
    assert!(harness.object_store.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn dumping_recovery_takes_a_fresh_privacy_snapshot_after_the_staged_dump() {
    let harness = Harness::new(FakePrivacyStore::new()).await;
    harness
        .dump
        .dump(DumpRequest {
            backup_id: harness.seed.id.clone(),
            cancellation: CancellationToken::new(),
        })
        .await
        .unwrap();
    harness
        .repository
        .force_status(&harness.seed.id, BackupStatus::Dumping);
    harness.run_cycle().await;

    let record = &harness.repository.all_records()[0];
    assert_eq!(record.status, BackupStatus::Completed);
    assert!(record.privacy_store.is_some());
    assert_eq!(
        harness.privacy.snapshots.lock().unwrap().as_slice(),
        std::slice::from_ref(&harness.seed.id)
    );
    assert_eq!(
        harness
            .object_store
            .object(&harness.privacy_key())
            .as_deref(),
        Some(PRIVACY_CONTENT)
    );
}

#[tokio::test]
async fn uploading_recovery_requires_every_recorded_object() {
    let harness = Harness::new(FakePrivacyStore::new()).await;
    harness.run_cycle().await;
    // 远端隐私对象丢失且本地已无暂存：不能把只有数据库的备份记为完成
    harness
        .object_store
        .objects
        .lock()
        .unwrap()
        .remove(&harness.privacy_key());
    harness
        .repository
        .force_status(&harness.seed.id, BackupStatus::Uploading);
    harness.run_cycle().await;

    let record = &harness.repository.all_records()[0];
    assert_eq!(record.status, BackupStatus::Failed);
    assert_eq!(record.error_code.as_deref(), Some(code::S3_UPLOAD_FAILED));
}
