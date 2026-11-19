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

#[tokio::test]
async fn reserved_request_reuses_one_slot_for_redaction_restore_and_stream() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::initialize(Config {
        data_directory: directory.path().join("veil"),
        max_parallel_operations: 1,
        ..Default::default()
    })
    .await;
    let privacy = runtime
        .session(Scope::ClientKey("fixture"))
        .unwrap()
        .reserve()
        .unwrap();
    assert_eq!(runtime.slots.available_permits(), 0);
    assert!(matches!(
        runtime
            .session(Scope::ClientKey("second"))
            .unwrap()
            .reserve(),
        Err(Error::Busy)
    ));
    let original = serde_json::json!({"messages":[{"role":"user","content":"server 8.8.8.8"}]});
    let redacted = privacy.redact(original.clone()).await.unwrap();
    assert_ne!(redacted, original);
    assert_eq!(privacy.restore(redacted).await.unwrap(), original);
    assert_eq!(
        privacy
            .restore(serde_json::json!({"error":{"message":"failure"}}))
            .await
            .unwrap()["error"]["message"],
        "failure"
    );
    assert_eq!(
        privacy
            .restore(Value::String("failure".into()))
            .await
            .unwrap(),
        "failure"
    );
    let mut stream = privacy.stream();
    drop(privacy);
    assert_eq!(runtime.slots.available_permits(), 0);
    stream
        .push("event: message_start\ndata: {\"type\":\"message_start\"}\n\n".into())
        .await
        .unwrap();
    stream
        .push("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".into())
        .await
        .unwrap();
    assert!(stream.finish().await.unwrap().is_empty());
    drop(stream);
    let permit = tokio::time::timeout(std::time::Duration::from_secs(2), runtime.slots.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    assert_eq!(runtime.slots.available_permits(), 1);
}

#[test]
fn cancelled_wait_does_not_release_a_queued_blocking_operation() {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    executor.block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::initialize(Config {
            data_directory: directory.path().join("veil"),
            max_parallel_operations: 1,
            ..Default::default()
        })
        .await;
        let privacy = runtime
            .session(Scope::ClientKey("fixture"))
            .unwrap()
            .reserve()
            .unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::sync_channel::<()>(1);
        let blocker = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            blocked.recv().unwrap();
        });
        ready.await.unwrap();
        let mut work = Box::pin(privacy.redact(serde_json::json!({"messages":[]})));
        std::future::poll_fn(|context| {
            assert!(std::future::Future::poll(work.as_mut(), context).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(work);
        drop(privacy);
        assert_eq!(runtime.slots.available_permits(), 0);
        release.send(()).unwrap();
        blocker.await.unwrap();
        let permit =
            tokio::time::timeout(std::time::Duration::from_secs(2), runtime.slots.acquire())
                .await
                .unwrap()
                .unwrap();
        drop(permit);
        assert_eq!(runtime.slots.available_permits(), 1);
    });
}
