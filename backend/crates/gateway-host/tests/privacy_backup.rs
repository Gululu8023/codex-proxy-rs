//! veil 映射卷快照：真实 SQLite 映射经归档、解包到空目录后仍能用原主密钥恢复

use std::{path::Path, sync::Arc};

use gateway_admin::{
    model::backup::code,
    ports::backup::{DumpRequest, PrivacyStoreSnapshotPort},
};
use gateway_core::lifecycle::CancellationToken;
use gateway_host::privacy_backup::VeilBackupSnapshot;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const MASTER_KEY: &str = "4f1c2b7a9e8d6c5b4a39281706f5e4d3c2b1a09f8e7d6c5b4a3928170615e4d3";
const OTHER_KEY: &str = "0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9";
const SECRET: &str = "AKIAIOSFODNN7EXAMPLE";
const BACKUP_ID: &str = "backup_0123456789abcdef0123456789abcdef";

fn private_directory(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn write_key(directory: &Path, key: &str) {
    private_directory(directory);
    std::fs::write(directory.join("master.key"), key).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            directory.join("master.key"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
}

async fn runtime(directory: &Path, initialize_store: bool) -> Arc<gateway_veil::Runtime> {
    gateway_veil::Runtime::initialize(gateway_veil::Config {
        data_directory: directory.to_path_buf(),
        initialize_store,
        max_parallel_operations: 2,
        ..Default::default()
    })
    .await
}

fn request() -> DumpRequest {
    DumpRequest {
        backup_id: BACKUP_ID.to_owned(),
        cancellation: CancellationToken::new(),
    }
}

fn body() -> Value {
    json!({"messages": [{"role": "user", "content": format!("token {SECRET} please")}]})
}

fn sha256_file(path: &Path) -> String {
    hex::encode(Sha256::digest(std::fs::read(path).unwrap()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archived_snapshot_restores_mappings_into_an_empty_volume() {
    let root = tempfile::tempdir().unwrap();
    let volume = root.path().join("veil");
    let staging = root.path().join("backup-staging");
    private_directory(&staging);
    write_key(&volume, MASTER_KEY);
    let veil = runtime(&volume, true).await;
    veil.ensure_ready().expect("fresh volume initializes");

    let session = veil
        .session(gateway_veil::Scope::ClientKey("key-1"))
        .unwrap();
    let redacted = session.redact(body()).await.unwrap();
    assert!(
        !redacted.to_string().contains(SECRET),
        "secret must be replaced"
    );
    let health = veil.health().await;
    assert!(health.mapping_count.unwrap() > 0);

    let adapter = VeilBackupSnapshot::new(veil.clone(), staging.clone());
    let artifact = adapter
        .snapshot(request())
        .await
        .unwrap()
        .expect("initialized volume produces a snapshot");
    assert_eq!(
        artifact.path,
        staging.join(format!("{BACKUP_ID}.veil.tar.gz"))
    );
    assert_eq!(artifact.sha256, sha256_file(&artifact.path));
    assert_eq!(
        artifact.size_bytes,
        std::fs::metadata(&artifact.path).unwrap().len()
    );
    // 中间目录与 partial 文件不留在暂存区
    let mut staged: Vec<_> = std::fs::read_dir(&staging)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    staged.sort();
    assert_eq!(staged, [format!("{BACKUP_ID}.veil.tar.gz")]);
    let inspected = adapter.inspect_staging(BACKUP_ID).await.unwrap().unwrap();
    assert_eq!(
        (inspected.size_bytes, inspected.sha256.as_str()),
        (artifact.size_bytes, artifact.sha256.as_str())
    );

    // 解包到全新的空目录，只补回原主密钥
    let restored = root.path().join("restored");
    private_directory(&restored);
    let archive = std::fs::File::open(&artifact.path).unwrap();
    tar::Archive::new(flate2::read::GzDecoder::new(archive))
        .unpack(&restored)
        .unwrap();
    let mut entries: Vec<_> = std::fs::read_dir(&restored)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        [
            "initialized",
            "manifest.json",
            "mappings.db",
            "rules-revision"
        ]
    );
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(restored.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["format"], gateway_veil::SNAPSHOT_FORMAT);
    assert_eq!(
        manifest["keyFingerprint"].as_str(),
        health.key_fingerprint.as_deref()
    );
    assert_eq!(manifest["mappingCount"].as_u64(), health.mapping_count);
    assert!(manifest["rulesFile"].is_null());
    assert!(
        !restored.join("master.key").exists(),
        "master key must never be archived"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(restored.join("mappings.db"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    write_key(&restored, MASTER_KEY);
    let recovered = runtime(&restored, false).await;
    recovered
        .ensure_ready()
        .expect("restored volume opens without initialization");
    assert_eq!(recovered.state_fingerprint(), veil.state_fingerprint());
    let restored_body = recovered
        .session(gateway_veil::Scope::ClientKey("key-1"))
        .unwrap()
        .restore(redacted.clone())
        .await
        .unwrap();
    assert_eq!(restored_body, body());

    // 主密钥不匹配时拒绝打开恢复的映射库
    let wrong = root.path().join("wrong-key");
    private_directory(&wrong);
    let archive = std::fs::File::open(&artifact.path).unwrap();
    tar::Archive::new(flate2::read::GzDecoder::new(archive))
        .unpack(&wrong)
        .unwrap();
    write_key(&wrong, OTHER_KEY);
    assert!(runtime(&wrong, false).await.ensure_ready().is_err());

    adapter.cleanup_staging(BACKUP_ID).await.unwrap();
    assert!(adapter.inspect_staging(BACKUP_ID).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uninitialized_volume_has_nothing_to_back_up() {
    let root = tempfile::tempdir().unwrap();
    let volume = root.path().join("veil");
    let staging = root.path().join("backup-staging");
    private_directory(&staging);
    write_key(&volume, MASTER_KEY);
    let veil = runtime(&volume, false).await;
    assert!(
        veil.ensure_ready().is_err(),
        "uninitialized volume is not ready"
    );

    let adapter = VeilBackupSnapshot::new(veil, staging.clone());
    assert!(adapter.snapshot(request()).await.unwrap().is_none());
    assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_initialized_volume_fails_the_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let volume = root.path().join("veil");
    let staging = root.path().join("backup-staging");
    private_directory(&staging);
    write_key(&volume, MASTER_KEY);
    let initial = runtime(&volume, true).await;
    // veil 以已有密文校验主密钥，先写入一条映射
    initial
        .session(gateway_veil::Scope::ClientKey("key-1"))
        .unwrap()
        .redact(body())
        .await
        .unwrap();
    drop(initial);
    // 已初始化的卷换成错误主密钥：运行时不可用，但映射卷存在，不能当作“无需备份”
    write_key(&volume, OTHER_KEY);
    let veil = runtime(&volume, false).await;
    assert!(veil.ensure_ready().is_err());

    let error = VeilBackupSnapshot::new(veil, staging.clone())
        .snapshot(request())
        .await
        .unwrap_err();
    assert_eq!(error.code(), code::PRIVACY_STORE_FAILED);
    assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);
}
