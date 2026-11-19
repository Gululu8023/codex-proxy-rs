//! 新卷初始化、重启复用和缺失旧卷文件必须遵守同一密钥边界。
//! 规则视图使用真实配置和映射库验证，并且不得暴露词典原文。

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

    let health = serde_json::to_value(runtime.health().await).unwrap();
    assert_eq!(health["ready"], false);
    assert_eq!(health["lastError"], "store_unavailable");
    assert_eq!(health["ruleCount"], 0);
    assert_eq!(health["categories"], serde_json::json!({}));
    for field in ["rules", "packs", "rulesRevision", "mappingCount"] {
        assert_eq!(health.get(field), Some(&Value::Null), "{field}");
    }
}

#[tokio::test]
async fn health_reports_builtin_rules_including_disabled_entries() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::initialize(Config {
        data_directory: directory.path().join("veil"),
        ..Default::default()
    })
    .await;
    runtime.ensure_ready().unwrap();

    let health = serde_json::to_value(runtime.health().await).unwrap();
    assert_eq!(health["ready"], true);
    assert_eq!(health["mappingCount"], 0);
    assert_eq!(health["ruleCount"], 10);
    assert_eq!(
        health["packs"],
        serde_json::json!({"secrets": true, "region": true})
    );
    assert_eq!(health["rules"]["source"], "builtin");
    assert_eq!(
        health["rules"]["allowlist"],
        serde_json::json!(["127.0.0.1", "localhost", "0.0.0.0", "::1"])
    );
    assert_eq!(
        health["categories"],
        serde_json::json!({
            "PEM": 1, "APIKEY": 1, "TOKEN": 1, "CONNSTR": 1, "PASSWORD": 1,
            "IDCARD": 1, "PHONE": 1, "IP": 1, "TZ": 1, "LOCALE": 1
        })
    );
    let items = health["rules"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 11);
    let email = items.iter().find(|item| item["id"] == "email").unwrap();
    assert_eq!(
        email,
        &serde_json::json!({
            "id": "email", "typePrefix": "EMAIL", "enabled": false, "priority": 80,
            "matcher": "regex", "pattern": r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}",
            "wordCount": null
        })
    );
}

#[tokio::test]
async fn health_treats_an_empty_rules_file_as_builtin_and_honors_packs() {
    let directory = tempfile::tempdir().unwrap();
    let rules_file = directory.path().join("rules.toml");
    std::fs::write(
        &rules_file,
        r#"
rules = []
allowlist = ["Asia/Shanghai"]

[packs]
secrets = false
region = true
"#,
    )
    .unwrap();
    let runtime = Runtime::initialize(Config {
        data_directory: directory.path().join("veil"),
        rules_file: Some(rules_file),
        ..Default::default()
    })
    .await;
    runtime.ensure_ready().unwrap();

    let health = serde_json::to_value(runtime.health().await).unwrap();
    assert_eq!(health["ready"], true);
    assert_eq!(health["ruleCount"], 2);
    assert_eq!(
        health["packs"],
        serde_json::json!({"secrets": false, "region": true})
    );
    assert_eq!(health["rules"]["source"], "builtin");
    assert_eq!(
        health["rules"]["allowlist"],
        serde_json::json!(["Asia/Shanghai"])
    );
    assert_eq!(
        health["categories"],
        serde_json::json!({"TZ": 1, "LOCALE": 1})
    );
    let items = health["rules"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0],
        serde_json::json!({
            "id": "timezone", "typePrefix": "TZ", "enabled": true, "priority": 115,
            "matcher": "dictionary", "pattern": null, "wordCount": 5
        })
    );
    assert_eq!(items[1]["id"], "locale");

    let privacy = runtime
        .session(Scope::ClientKey("rules-view"))
        .unwrap()
        .reserve()
        .unwrap();
    let original = serde_json::json!({
        "messages": [{"role": "user", "content": "Asia/Shanghai Asia/Hong_Kong zh-CN 8.8.8.8"}]
    });
    let redacted = privacy.redact(original.clone()).await.unwrap();
    let content = redacted["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("Asia/Shanghai"));
    assert!(!content.contains("Asia/Hong_Kong"));
    assert!(!content.contains("zh-CN"));
    assert!(content.contains("8.8.8.8"));
    assert_eq!(privacy.restore(redacted).await.unwrap(), original);
    assert_eq!(runtime.health().await.mapping_count, Some(2));
}

#[tokio::test]
async fn health_reports_effective_custom_rules_without_dictionary_contents() {
    let directory = tempfile::tempdir().unwrap();
    let rules_file = directory.path().join("rules.toml");
    // 同时配置词条与正则，确认视图跟随实际编译的词典匹配器，而不回显原始配置。
    std::fs::write(
        &rules_file,
        r#"
allowlist = ["9.9.9.9"]

[packs]
secrets = true
region = true

[[rules]]
id = "disabled-code"
type_prefix = "SKIP"
enabled = false
priority = 5
pattern = '\bSKIP-[0-9]{3}\b'

[[rules]]
id = "ip"
type_prefix = "IP"
priority = 10

[[rules]]
id = "dictionary"
type_prefix = "PROJECT"
priority = 30
words = ["veil-private-alpha", "veil-private-beta"]
pattern = '\bUNUSED-[0-9]{3}\b'

[[rules]]
id = "code"
type_prefix = "CODE"
priority = 20
pattern = '\bTEST-[0-9]{3}\b'
"#,
    )
    .unwrap();
    let runtime = Runtime::initialize(Config {
        data_directory: directory.path().join("veil"),
        rules_file: Some(rules_file),
        ..Default::default()
    })
    .await;
    runtime.ensure_ready().unwrap();

    let health = serde_json::to_value(runtime.health().await).unwrap();
    assert_eq!(health["ready"], true);
    assert_eq!(health.get("packs"), Some(&Value::Null));
    assert_eq!(health["ruleCount"], 3);
    assert_eq!(
        health["categories"],
        serde_json::json!({"PROJECT": 1, "CODE": 1, "IP": 1})
    );
    assert_eq!(
        health["rules"],
        serde_json::json!({
            "source": "custom",
            "allowlist": ["9.9.9.9"],
            "items": [
                {
                    "id": "dictionary", "typePrefix": "PROJECT", "enabled": true,
                    "priority": 30, "matcher": "dictionary", "pattern": null, "wordCount": 2
                },
                {
                    "id": "code", "typePrefix": "CODE", "enabled": true,
                    "priority": 20, "matcher": "regex", "pattern": r"\bTEST-[0-9]{3}\b", "wordCount": null
                },
                {
                    "id": "ip", "typePrefix": "IP", "enabled": true,
                    "priority": 10, "matcher": "ip", "pattern": null, "wordCount": null
                },
                {
                    "id": "disabled-code", "typePrefix": "SKIP", "enabled": false,
                    "priority": 5, "matcher": "regex", "pattern": r"\bSKIP-[0-9]{3}\b", "wordCount": null
                }
            ]
        })
    );
    let serialized = serde_json::to_string(&health).unwrap();
    for word in ["veil-private-alpha", "veil-private-beta", "UNUSED"] {
        assert!(!serialized.contains(word), "{word}");
    }

    let privacy = runtime
        .session(Scope::ClientKey("rules-view"))
        .unwrap()
        .reserve()
        .unwrap();
    let original = serde_json::json!({
        "messages": [{
            "role": "user",
            "content": "TEST-123 veil-private-alpha veil-private-beta 8.8.8.8 SKIP-456 9.9.9.9 UNUSED-789 Asia/Shanghai zh-CN"
        }]
    });
    let redacted = privacy.redact(original.clone()).await.unwrap();
    let content = redacted["messages"][0]["content"].as_str().unwrap();
    for protected in [
        "TEST-123",
        "veil-private-alpha",
        "veil-private-beta",
        "8.8.8.8",
    ] {
        assert!(!content.contains(protected), "{protected}");
    }
    for unchanged in [
        "SKIP-456",
        "9.9.9.9",
        "UNUSED-789",
        "Asia/Shanghai",
        "zh-CN",
    ] {
        assert!(content.contains(unchanged), "{unchanged}");
    }
    assert_eq!(privacy.restore(redacted).await.unwrap(), original);
    assert_eq!(runtime.health().await.mapping_count, Some(4));
}

#[tokio::test]
async fn health_does_not_report_rules_when_rule_compilation_fails() {
    let directory = tempfile::tempdir().unwrap();
    let rules_file = directory.path().join("rules.toml");
    std::fs::write(
        &rules_file,
        r#"
[[rules]]
id = "invalid-regex"
type_prefix = "INVALID"
pattern = '('
"#,
    )
    .unwrap();
    let runtime = Runtime::initialize(Config {
        data_directory: directory.path().join("veil"),
        rules_file: Some(rules_file),
        ..Default::default()
    })
    .await;
    assert!(matches!(runtime.ensure_ready(), Err(Error::InvalidRules)));

    let health = serde_json::to_value(runtime.health().await).unwrap();
    assert_eq!(health["ready"], false);
    assert_eq!(health["lastError"], "invalid_rules");
    assert_eq!(health["ruleCount"], 0);
    assert_eq!(health["categories"], serde_json::json!({}));
    for field in ["rules", "packs", "rulesRevision", "mappingCount"] {
        assert_eq!(health.get(field), Some(&Value::Null), "{field}");
    }
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
