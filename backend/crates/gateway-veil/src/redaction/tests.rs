use super::*;
use crate::{Config, Runtime, Scope};
use serde_json::json;

async fn runtime() -> (tempfile::TempDir, Arc<Runtime>) {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::initialize(Config {
        data_directory: directory.path().join("veil"),
        ..Default::default()
    })
    .await;
    runtime.ensure_ready().unwrap();
    (directory, runtime)
}

#[tokio::test]
async fn cached_redaction_matches_uncached_protocol_and_isolates_keys() {
    let (_directory, runtime) = runtime().await;
    let session = runtime
        .session(Scope::ClientKey("one"))
        .unwrap()
        .reserve()
        .unwrap();
    let body = json!({
        "model":"claude-sonnet-4-5", "thinking":{"type":"adaptive"},
        "messages":[{"role":"assistant","content":[
            {"type":"thinking","thinking":"8.8.8.8","signature":"literal {{IP_00000000000000000000000000}}"},
            {"type":"tool_use","id":"tool_8.8.8.8","name":"tool","input":{"thinking":"8.8.8.8"}},
            {"type":"text","text":"address 8.8.8.8"},
            {"type":"text","text":"again 8.8.8.8"},
            {"type":"text","text":"literal {{IP_00000000000000000000000000}}"}
        ]}],
        "tools":[{"name":"tool","input_schema":{"type":"object","properties":{"8.8.8.8":{"type":"string","description":"8.8.8.8"}}}}]
    });
    let state = runtime.state.as_ref().unwrap();
    let expected = veil_engine::anthropic::desensitize_json(
        &body,
        &state.rules,
        &state.store,
        &session.session.creator,
    )
    .unwrap();
    let cold = session.redact_observed(body.clone()).await.unwrap();
    assert_eq!(cold.value, expected);
    assert!(cold.statistics.mapping_reuses > 0);
    let warm = session.redact_observed(body.clone()).await.unwrap();
    assert_eq!(warm.value, expected);
    assert_eq!(warm.statistics.scanned_bytes, 0);
    assert_eq!(warm.statistics.mapping_operations, 0);
    assert!(warm.statistics.cache_hits > 0);
    assert_eq!(session.restore(warm.value).await.unwrap(), body);
    let other = runtime
        .session(Scope::ClientKey("two"))
        .unwrap()
        .reserve()
        .unwrap();
    let other_result = other.redact_observed(body.clone()).await.unwrap();
    assert!(other_result.statistics.scanned_bytes > 0);
    assert_ne!(
        other_result.value["messages"][0]["content"][2],
        expected["messages"][0]["content"][2]
    );
    assert_eq!(other.restore(other_result.value).await.unwrap(), body);
    let mut edited = body;
    edited["messages"][0]["content"][2]["text"] = json!("changed address 1.1.1.1");
    let changed = session.redact_observed(edited).await.unwrap();
    assert_eq!(
        changed.statistics.scanned_bytes,
        "changed address 1.1.1.1".len()
    );
}

#[tokio::test]
async fn warm_cache_never_hides_missing_or_corrupt_persistent_mappings() {
    for corrupt in [false, true] {
        let (directory, runtime) = runtime().await;
        let session = runtime
            .session(Scope::ClientKey("one"))
            .unwrap()
            .reserve()
            .unwrap();
        let body = json!({"messages":[{"role":"user","content":"8.8.8.8"}]});
        session.redact(body.clone()).await.unwrap();
        let connection =
            rusqlite::Connection::open(directory.path().join("veil/mappings.db")).unwrap();
        connection
            .execute(
                if corrupt {
                    "UPDATE mappings SET plaintext_ct = X'00'"
                } else {
                    "DELETE FROM mappings"
                },
                [],
            )
            .unwrap();
        let failure = session
            .redact(body)
            .await
            .expect_err("cache must revalidate references");
        assert!(if corrupt {
            matches!(failure, Error::StoreUnavailable)
        } else {
            matches!(failure, Error::MappingMissing)
        });
        if corrupt {
            assert!(runtime.ensure_ready().is_err());
        } else {
            assert_eq!(
                runtime
                    .state
                    .as_ref()
                    .unwrap()
                    .store
                    .mapping_count()
                    .unwrap(),
                0
            );
        }
    }
}

#[test]
fn cache_limits_evict_acceleration_data_only() {
    let mut cache = Cache::default();
    let size = 64 * 1024;
    for owner in 0..8 {
        for index in 0..100 {
            let creator = Creator([owner; 32]);
            let key = Key {
                creator,
                digest: Sha256::digest([index]).into(),
            };
            cache.insert(
                key,
                CachedText {
                    text: Arc::from("fixture"),
                    references: Arc::from([]),
                    size,
                },
            );
            assert!(cache.bytes <= MAX_BYTES);
            let bytes: usize = cache
                .entries
                .iter()
                .filter(|(key, _)| key.creator == creator)
                .map(|(_, entry)| entry.value.size)
                .sum();
            assert!(bytes <= MAX_CREATOR_BYTES);
        }
    }
    let previous = cache.entries.len();
    cache.insert(
        Key {
            creator: Creator([9; 32]),
            digest: [0; 32],
        },
        CachedText {
            text: Arc::from("oversized"),
            references: Arc::from([]),
            size: MAX_ENTRY_BYTES + 1,
        },
    );
    assert_eq!(cache.entries.len(), previous);
}

#[tokio::test]
async fn strict_stream_restores_split_text_and_tool_json_before_block_stop() {
    let (_directory, runtime) = runtime().await;
    let privacy = runtime
        .session(Scope::ClientKey("fixture"))
        .unwrap()
        .reserve()
        .unwrap();
    let redacted = privacy
        .redact(json!({"messages":[{"role":"user","content":"8.8.8.8"}]}))
        .await
        .unwrap();
    let token = redacted["messages"][0]["content"].as_str().unwrap();
    let tool = json!({"host":token}).to_string();
    for newline in ["\n", "\r\n"] {
        let frames = [
            json!({"type":"message_start","message":{"id":"msg_fixture"}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":token}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":&token[..token.len()/2]}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":&token[token.len()/2..]}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tool_fixture","name":"tool","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":&tool[..tool.len()/2]}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":&tool[tool.len()/2..]}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_stop"}),
        ];
        let mut stream = privacy.stream();
        let mut output = String::new();
        for value in frames {
            output.push_str(
                &stream
                    .push(format!(
                        "event: {}{newline}data: {value}{newline}{newline}",
                        value["type"].as_str().unwrap()
                    ))
                    .await
                    .unwrap(),
            );
        }
        assert!(stream.finish().await.unwrap().is_empty());
        let events: Vec<serde_json::Value> = output
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|data| serde_json::from_str(data.trim()).unwrap())
            .collect();
        let text: String = events
            .iter()
            .filter_map(|event| event["delta"]["text"].as_str())
            .collect();
        assert_eq!(text, "8.8.8.8");
        let params: String = events
            .iter()
            .filter_map(|event| event["delta"]["partial_json"].as_str())
            .collect();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&params).unwrap(),
            json!({"host":"8.8.8.8"})
        );
        assert!(
            events
                .iter()
                .any(|event| event["delta"]["signature"] == token)
        );
        for index in [1, 2] {
            let stop = events
                .iter()
                .position(|event| event["type"] == "content_block_stop" && event["index"] == index)
                .unwrap();
            assert_eq!(events[stop - 1]["type"], "content_block_delta");
            assert_eq!(events[stop - 1]["index"], index);
        }
    }
}

#[tokio::test]
#[ignore = "manual bounded cold/warm performance sample"]
async fn redaction_performance_sample() {
    let (_directory, runtime) = runtime().await;
    for count in [50, 200] {
        let privacy = runtime
            .session(Scope::ClientKey(&format!("sample-{count}")))
            .unwrap()
            .reserve()
            .unwrap();
        let messages: Vec<_> = (0..count).map(|index| json!({"role":"user", "content":format!("{} 8.8.8.8 segment {index}", "routine text ".repeat(320))})).collect();
        let body = json!({"messages":messages});
        let start = Instant::now();
        let reference = privacy.session.redact(body.clone()).await.unwrap();
        let reference_us = micros(start);
        let cold = privacy.redact_observed(body.clone()).await.unwrap();
        let warm = privacy.redact_observed(body).await.unwrap();
        assert_eq!(reference, cold.value);
        assert_eq!(reference, warm.value);
        assert_eq!(warm.statistics.scanned_bytes, 0);
        assert_eq!(warm.statistics.mapping_operations, 0);
        println!(
            "{}",
            json!({"textBytes":cold.statistics.text_bytes,"uncachedUs":reference_us,"cold":cold.statistics,"warm":warm.statistics})
        );
    }
}
