use super::*;
use crate::{Config, Error, ReservedSession, Runtime, Scope};
use serde_json::json;
use std::sync::Arc;

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

fn reserve(runtime: &Runtime, key: &str) -> ReservedSession {
    runtime
        .session(Scope::ClientKey(key))
        .unwrap()
        .reserve()
        .unwrap()
}

async fn redact_ip(privacy: &ReservedSession) -> Placeholder {
    let redacted = privacy
        .redact(json!({"messages":[{"role":"user","content":"8.8.8.8"}]}))
        .await
        .unwrap();
    parse_placeholder(redacted["messages"][0]["content"].as_str().unwrap()).unwrap()
}

fn changed_id(original: &Placeholder) -> Placeholder {
    let mut id = original.id.to_string();
    let last = if id.ends_with('0') { "1" } else { "0" };
    id.replace_range(25..26, last);
    parse_placeholder(&format!("{{{{{}_{id}}}}}", original.type_prefix)).unwrap()
}

fn response(token: &Placeholder) -> Value {
    json!({"type":"message","content":[{"type":"text","text":token.format()}]})
}

fn frame(value: Value) -> String {
    format!(
        "event: {}\ndata: {value}\n\n",
        value["type"].as_str().unwrap()
    )
}

#[tokio::test]
async fn json_passthrough_distinguishes_sent_mapping_loss_from_changed_response_id() {
    for deleted in [false, true] {
        let (directory, runtime) = runtime().await;
        let privacy = reserve(&runtime, "fixture-key");
        let original = redact_ip(&privacy).await;
        let missing = if deleted {
            let database = directory.path().join("veil/mappings.db");
            rusqlite::Connection::open(database)
                .unwrap()
                .execute(
                    "DELETE FROM mappings WHERE placeholder_id = ?1",
                    [original.id.to_string()],
                )
                .unwrap();
            original.clone()
        } else {
            changed_id(&original)
        };
        assert_eq!(
            privacy.restore(response(&missing)).await.unwrap(),
            response(&missing)
        );
        let evidence = privacy.take_missing_mapping().unwrap();
        assert!(privacy.take_missing_mapping().is_none());
        assert_eq!(evidence.seen_in_redacted_request, Some(deleted));
        assert_eq!(evidence.request_reference_count, 1);
        assert!(evidence.request_references_complete);
        assert_eq!(
            evidence.closest_request_id_distance,
            Some(usize::from(!deleted))
        );
        assert_eq!(
            evidence.closest_request_id_sha256,
            Some(fingerprint(original.id.to_string()))
        );
        assert_eq!(evidence.id_sha256, fingerprint(missing.id.to_string()));
        let encoded = serde_json::to_string(&evidence).unwrap();
        for private in [
            "8.8.8.8",
            "fixture-key",
            &missing.format(),
            &missing.id.to_string(),
        ] {
            assert!(!encoded.contains(private));
        }
        assert_eq!(
            runtime
                .state
                .as_ref()
                .unwrap()
                .store
                .mapping_count()
                .unwrap(),
            u64::from(!deleted)
        );
    }
}

#[tokio::test]
async fn split_sse_passthrough_records_the_same_evidence_as_json() {
    let (_directory, runtime) = runtime().await;
    let json_privacy = reserve(&runtime, "fixture-key");
    let original = redact_ip(&json_privacy).await;
    let missing = changed_id(&original);
    assert_eq!(
        json_privacy.restore(response(&missing)).await.unwrap(),
        response(&missing)
    );
    let expected = serde_json::to_value(json_privacy.take_missing_mapping().unwrap()).unwrap();

    let stream_privacy = reserve(&runtime, "fixture-key");
    assert_eq!(redact_ip(&stream_privacy).await, original);
    let token = missing.format();
    let midpoint = token.len() / 2;
    let mut stream = stream_privacy.stream();
    for value in [
        json!({"type":"message_start","message":{"id":"msg_fixture"}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":&token[..midpoint]}}),
    ] {
        let output = stream.push(frame(value)).await.unwrap();
        assert!(!output.contains("{{"));
        assert!(stream_privacy.take_missing_mapping().is_none());
    }
    let final_fragment = frame(json!({"type":"content_block_delta","index":0,
        "delta":{"type":"text_delta","text":&token[midpoint..]}}));
    let output = stream.push(final_fragment).await.unwrap();
    assert!(output.contains(&token));
    assert_eq!(
        serde_json::to_value(stream_privacy.take_missing_mapping().unwrap()).unwrap(),
        expected
    );
    stream
        .push(frame(json!({"type":"content_block_stop","index":0})))
        .await
        .unwrap();
    assert!(
        stream
            .push(frame(json!({"type":"message_stop"})))
            .await
            .unwrap()
            .contains("message_stop")
    );
    stream.finish().await.unwrap();
    assert!(stream_privacy.take_missing_mapping().is_none());
}

#[tokio::test]
async fn foreign_mapping_stays_private_and_client_literal_roundtrips() {
    let (_directory, runtime) = runtime().await;
    let owner = reserve(&runtime, "owner");
    let original = redact_ip(&owner).await;
    let other = reserve(&runtime, "other");
    other
        .redact(json!({"messages":[{"role":"user","content":"ordinary text"}]}))
        .await
        .unwrap();
    assert_eq!(
        other.restore(response(&original)).await.unwrap(),
        response(&original)
    );
    let evidence = other.take_missing_mapping().unwrap();
    assert_eq!(evidence.seen_in_redacted_request, Some(false));
    assert_eq!(evidence.request_reference_count, 0);
    assert!(evidence.closest_request_id_distance.is_none());
    assert_ne!(
        evidence.creator_sha256,
        fingerprint(owner.session.creator.hex())
    );

    let literal = json!({"messages":[{"role":"user","content":original.format()}]});
    let protected = other.redact(literal.clone()).await.unwrap();
    assert!(other.take_missing_mapping().is_none());
    assert_ne!(protected, literal);
    assert_eq!(other.restore(protected).await.unwrap(), literal);
    assert!(other.take_missing_mapping().is_none());
}

#[tokio::test]
async fn cloned_sessions_reserve_separate_unresolved_evidence() {
    let (_directory, runtime) = runtime().await;
    let session = runtime.session(Scope::ClientKey("shared-key")).unwrap();
    let first = session.clone().reserve().unwrap();
    let second = session.reserve().unwrap();
    let original = redact_ip(&first).await;
    second
        .redact(json!({"messages":[{"role":"user","content":"ordinary text"}]}))
        .await
        .unwrap();
    let unknown = response(&changed_id(&original));
    assert_eq!(first.restore(unknown.clone()).await.unwrap(), unknown);
    assert!(second.take_missing_mapping().is_none());
    // 诊断引用集不是访问控制；同一 Key 的历史映射仍可正常恢复。
    let restored = second.restore(response(&original)).await.unwrap();
    assert_eq!(restored["content"][0]["text"], "8.8.8.8");
    assert!(second.take_missing_mapping().is_none());
    assert!(first.take_missing_mapping().is_some());
}

#[test]
fn reference_budget_preserves_unknown_and_does_not_select_tied_candidates() {
    let creator = Creator::from_secret("fixture-key");
    let tokens: Vec<_> = (0..=MAX_REFERENCES)
        .map(|index| format!("{{{{IP_{index:026}}}}}"))
        .collect();
    let mut observations = Observations::default();
    observations.request(&json!(tokens));
    observations.record(
        &creator,
        &parse_placeholder(tokens.last().unwrap()).unwrap(),
    );
    let evidence = observations.take_missing().unwrap();
    assert_eq!(evidence.request_reference_count, MAX_REFERENCES);
    assert!(!evidence.request_references_complete);
    assert_eq!(evidence.seen_in_redacted_request, None);

    let mut object = serde_json::Map::new();
    object.insert(tokens[0].clone(), json!(tokens[2]));
    observations.request(&Value::Object(object));
    observations.record(&creator, &parse_placeholder(&tokens[1]).unwrap());
    let evidence = observations.take_missing().unwrap();
    assert_eq!(evidence.request_reference_count, 2);
    assert!(evidence.request_references_complete);
    assert_eq!(evidence.seen_in_redacted_request, Some(false));
    assert_eq!(evidence.closest_request_id_distance, Some(1));
    assert!(evidence.closest_request_id_sha256.is_none());
}

#[tokio::test]
async fn json_preserves_unknown_text_and_tool_values_while_restoring_known_mappings() {
    let (_directory, runtime) = runtime().await;
    let privacy = reserve(&runtime, "fixture-key");
    let known = redact_ip(&privacy).await;
    let unknown = changed_id(&known).format();
    let body = json!({"type":"message","content":[
        {"type":"text","text":format!("{} {unknown}", known.format())},
        {"type":"tool_use","id":"tool_fixture","name":"example","input":{
            "known":known.format(),"unknown":unknown,"nested":[unknown]
        }},
        {"type":"thinking","thinking":known.format(),"signature":"opaque-signature"}
    ]});
    let mut expected = body.clone();
    expected["content"][0]["text"] = json!(format!("8.8.8.8 {unknown}"));
    expected["content"][1]["input"]["known"] = json!("8.8.8.8");
    assert_eq!(privacy.restore(body).await.unwrap(), expected);
    assert!(privacy.take_missing_mapping().is_some());
    assert!(privacy.take_missing_mapping().is_none());
}

#[tokio::test]
async fn split_tool_json_preserves_unknown_values_and_finishes_normally() {
    let (_directory, runtime) = runtime().await;
    let privacy = reserve(&runtime, "fixture-key");
    let known = redact_ip(&privacy).await;
    let unknown = changed_id(&known).format();
    let input = json!({"known":known.format(),"unknown":unknown});
    let encoded = input.to_string();
    let midpoint = encoded.find(&unknown).unwrap() + unknown.len() / 2;
    let mut stream = privacy.stream();
    let mut output = String::new();
    for value in [
        json!({"type":"message_start","message":{"id":"msg_fixture"}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_fixture","name":"example","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":&encoded[..midpoint]}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":&encoded[midpoint..]}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_stop"}),
    ] {
        output.push_str(&stream.push(frame(value)).await.unwrap());
    }
    output.push_str(&stream.finish().await.unwrap());
    let events: Vec<Value> = output
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let restored_input: String = events
        .iter()
        .filter_map(|event| event["delta"]["partial_json"].as_str())
        .collect();
    assert_eq!(
        serde_json::from_str::<Value>(&restored_input).unwrap(),
        json!({"known":"8.8.8.8","unknown":unknown})
    );
    assert_eq!(events.last().unwrap()["type"], "message_stop");
    assert!(privacy.take_missing_mapping().is_some());
    assert!(privacy.take_missing_mapping().is_none());
}

#[tokio::test]
async fn passthrough_does_not_hide_corrupt_mapping_errors() {
    let (directory, runtime) = runtime().await;
    let privacy = reserve(&runtime, "fixture-key");
    let known = redact_ip(&privacy).await;
    rusqlite::Connection::open(directory.path().join("veil/mappings.db"))
        .unwrap()
        .execute("UPDATE mappings SET plaintext_ct = X'00'", [])
        .unwrap();
    assert!(matches!(
        privacy.restore(response(&known)).await,
        Err(Error::StoreUnavailable)
    ));
    let mut stream = privacy.stream();
    for value in [
        json!({"type":"message_start","message":{"id":"msg_fixture"}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
    ] {
        stream.push(frame(value)).await.unwrap();
    }
    assert!(matches!(
        stream
            .push(frame(json!({"type":"content_block_delta","index":0,
                "delta":{"type":"text_delta","text":known.format()}})))
            .await,
        Err(Error::StoreUnavailable)
    ));
}

#[tokio::test]
async fn passthrough_still_rejects_invalid_tool_json() {
    let (_directory, runtime) = runtime().await;
    let privacy = reserve(&runtime, "fixture-key");
    let mut stream = privacy.stream();
    for value in [
        json!({"type":"message_start","message":{"id":"msg_fixture"}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_fixture","name":"example","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}}),
    ] {
        stream.push(frame(value)).await.unwrap();
    }
    assert!(matches!(
        stream
            .push(frame(json!({"type":"content_block_stop","index":0})))
            .await,
        Err(Error::InvalidStream)
    ));
}
