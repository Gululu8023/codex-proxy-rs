//! 新卷初始化、重启复用和缺失旧卷文件必须遵守同一密钥边界。

use super::*;

#[tokio::test]
async fn restart_reuses_key_and_checkpoint_binding_cannot_be_changed() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        data_directory: directory.path().join("veil"),
        ..Default::default()
    };
    let first = Runtime::initialize(config.clone()).await;
    first.ensure_ready().unwrap();
    let fingerprint = first.state_fingerprint().unwrap().to_owned();
    let ciphertext = first
        .seal_checkpoint(b"key-a/session-a", b"private history")
        .unwrap();
    assert!(
        !ciphertext
            .windows(b"private history".len())
            .any(|v| v == b"private history")
    );
    drop(first);
    let restarted = Runtime::initialize(config).await;
    assert_eq!(restarted.state_fingerprint(), Some(fingerprint.as_str()));
    assert_eq!(
        restarted
            .open_checkpoint(b"key-a/session-a", &ciphertext)
            .unwrap(),
        b"private history"
    );
    assert!(
        restarted
            .open_checkpoint(b"key-b/session-a", &ciphertext)
            .is_err()
    );
}

#[tokio::test]
async fn partial_existing_volume_is_never_silently_reinitialized() {
    for missing in ["mappings.db", "initialized", "rules-revision"] {
        let directory = tempfile::tempdir().unwrap();
        let config = Config {
            data_directory: directory.path().join("veil"),
            ..Default::default()
        };
        let runtime = Runtime::initialize(config.clone()).await;
        runtime.ensure_ready().unwrap();
        let fingerprint = runtime.state_fingerprint().unwrap().to_owned();
        drop(runtime);
        std::fs::remove_file(config.data_directory.join(missing)).unwrap();
        let restarted = Runtime::initialize(config.clone()).await;
        assert!(restarted.ensure_ready().is_err(), "{missing} {fingerprint}");
        assert!(!config.data_directory.join(missing).exists());
        for purpose in [
            ControlPurpose::OAuth,
            ControlPurpose::Profile,
            ControlPurpose::Quota,
            ControlPurpose::Models,
        ] {
            assert!(restarted.control(purpose).is_err());
        }
    }
}

#[tokio::test]
async fn automatic_initialization_can_be_disabled() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        data_directory: directory.path().join("veil"),
        initialize_store: false,
        ..Default::default()
    };
    let runtime = Runtime::initialize(config).await;
    assert!(runtime.ensure_ready().is_err());
    assert!(runtime.session(Scope::ClientKey("fixture-key")).is_err());
}
