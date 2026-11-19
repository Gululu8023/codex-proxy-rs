//! Claude 文本流序列、请求观测和鉴权失败归因的回归覆盖。

use super::*;
use gateway_core::upstream::UpstreamFailureFields;

#[test]
fn stream_timings_separate_first_payload_thinking_and_text() {
    for newline in ["\n", "\r\n"] {
        let mut timings = ProviderResponseTimings {
            first_event_ms: Some(10),
            ..Default::default()
        };
        for frame in [
            format!(": keep-alive{newline}{newline}"),
            format!("data:{newline}{newline}"),
        ] {
            assert!(event_json(frame.as_bytes()).unwrap().is_none());
        }
        assert_eq!(timings.first_token_ms, None);
        // message_start 早于思考和正文，空增量不能提前记录内容到达。
        for (elapsed, payload) in [
            (
                20,
                serde_json::json!({"type":"message_start","message":{"id":"msg_timing","usage":{"input_tokens":10,"output_tokens":0}}}),
            ),
            (
                30,
                serde_json::json!({"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":""}}),
            ),
            (
                50,
                serde_json::json!({"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"consider"}}),
            ),
            (
                80,
                serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}),
            ),
            (
                90,
                serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":" again"}}),
            ),
        ] {
            let raw = format!(
                "event: {}{newline}data: {payload}{newline}{newline}",
                payload["type"].as_str().unwrap()
            );
            observe_event_timings(
                &mut timings,
                &event_json(raw.as_bytes()).unwrap().unwrap(),
                elapsed,
            );
        }
        assert_eq!(timings.first_event_ms, Some(10));
        assert_eq!(timings.first_token_ms, Some(20));
        assert_eq!(timings.first_reasoning_ms, Some(50));
        assert_eq!(timings.first_text_ms, Some(80));
    }
}

#[tokio::test]
async fn restored_text_stream_reaches_completed_after_usage() {
    use serde_json::json;

    // 独立 Usage 会启用 Core 的完整序列校验，必须保留线上出错的这一边界。
    for newline in ["\n", "\r\n"] {
        let frames = [
            json!({"type":"message_start","message":{"id":"msg_fixture","usage":{"input_tokens":10,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"consider"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"hello"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":" world"}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}),
            json!({"type":"message_stop"}),
        ];
        let meta = ResponseMeta::new("msg_fixture", "claude-sonnet-4-5");
        let mut usage = UsageCapture::default();
        let mut events = Vec::new();
        let mut expected_wire = Vec::new();
        for value in frames {
            let raw = Bytes::from(format!(
                "event: {}{newline}data: {value}{newline}{newline}",
                value["type"].as_str().unwrap()
            ));
            expected_wire.extend_from_slice(&raw);
            usage.observe(&value["message"]["usage"]);
            usage.observe(&value["usage"]);
            let mut canonical = restored_text_events(&raw).unwrap();
            if value["type"] == "message_start" {
                canonical.insert(0, GatewayEvent::Started(meta.clone()));
            } else if value["type"] == "message_delta" || value["type"] == "message_stop" {
                events.extend(usage.finish(
                    "claude-sonnet-4-5",
                    &Default::default(),
                    &json!({}),
                    value["type"] == "message_stop",
                    &ProviderResponseObservation::new(transport()),
                ));
            }
            if value["type"] == "message_stop" {
                canonical.push(GatewayEvent::Completed(meta.clone()));
            }
            events.push(ProviderEvent::canonical_with_wire(
                canonical,
                ProtocolWireEvent::raw_sse(NAME, raw).unwrap(),
            ));
        }
        let metadata = ProviderCallMetadata::new(
            gateway_core::routing::ProviderKind::new(NAME).unwrap(),
            UpstreamModelId::new("claude-sonnet-4-5").unwrap(),
            ProviderAccountId::new("acct_fixture").unwrap(),
            transport(),
        );
        let mut stream = ProviderStream::new(
            metadata,
            futures::stream::iter(events.into_iter().map(Ok)),
            (),
        );
        let mut delivered = Vec::new();
        while let Some(event) = stream.next().await {
            delivered.push(event.expect("text stream must pass canonical validation"));
        }
        let facts: Vec<_> = delivered
            .iter()
            .flat_map(ProviderEvent::canonical_facts)
            .collect();
        assert!(matches!(facts.last(), Some(GatewayEvent::Completed(_))));
        let indexes: Vec<_> = facts
            .iter()
            .filter_map(|fact| match fact {
                GatewayEvent::ContentAdded(item) => Some(item.index()),
                _ => None,
            })
            .collect();
        assert_eq!(indexes, [1, 2]);
        assert!(facts.iter().any(
            |fact| matches!(fact, GatewayEvent::Usage(usage) if usage.output_tokens == Some(12))
        ));
        let actual_wire: Vec<_> = delivered
            .iter()
            .filter_map(ProviderEvent::wire_event)
            .flat_map(|wire| wire.raw_sse_frame().unwrap().iter().copied())
            .collect();
        assert_eq!(actual_wire, expected_wire);
    }
}

fn observed_request(protocol: &str, body: Value) -> Operation {
    use gateway_core::operation::{GenerateRequest, ProtocolPayload};

    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object(protocol, body.as_object().unwrap().clone()).unwrap(),
    ))
}

#[tokio::test]
async fn restored_json_text_blocks_pass_canonical_validation_with_usage() {
    let value = serde_json::json!({
        "id":"msg_fixture", "type":"message",
        "content":[
            {"type":"thinking","thinking":"opaque","signature":"unchanged"},
            {"type":"text","text":"你好"},
            {"type":"text","text":""},
            {"type":"tool_use","id":"tool_fixture","name":"tool","input":{}}
        ],
        "usage":{"input_tokens":10,"output_tokens":12}
    });
    let meta = ResponseMeta::new("msg_fixture", "claude-sonnet-4-5");
    let mut usage = UsageCapture::default();
    usage.observe(&value["usage"]);
    let mut facts = vec![GatewayEvent::Started(meta.clone())];
    for event in usage.finish(
        "claude-sonnet-4-5",
        &Default::default(),
        &value,
        true,
        &ProviderResponseObservation::new(transport()),
    ) {
        facts.extend_from_slice(event.canonical_facts());
    }
    facts.extend(restored_json_text_events(&value));
    facts.push(GatewayEvent::Completed(meta));
    let bytes = Bytes::from(value.to_string());
    let event = ProviderEvent::canonical_with_wire(
        facts,
        ProtocolWireEvent::raw_json(NAME, bytes.clone()).unwrap(),
    );
    let metadata = ProviderCallMetadata::new(
        gateway_core::routing::ProviderKind::new(NAME).unwrap(),
        UpstreamModelId::new("claude-sonnet-4-5").unwrap(),
        ProviderAccountId::new("acct_fixture").unwrap(),
        transport(),
    );
    let mut stream = ProviderStream::new(metadata, futures::stream::iter([Ok(event)]), ());
    let delivered = stream.next().await.unwrap().unwrap();
    assert_eq!(
        delivered.wire_event().unwrap().raw_json_body(),
        Some(&bytes)
    );
    let indexes: Vec<_> = delivered
        .canonical_facts()
        .iter()
        .filter_map(|fact| match fact {
            GatewayEvent::ContentAdded(content) => Some(content.index()),
            _ => None,
        })
        .collect();
    assert_eq!(indexes, [1, 2]);
    assert!(stream.next().await.is_none());
}

#[test]
fn sse_framing_waits_for_complete_utf8_and_line_delimiters() {
    for newline in ["\n", "\r\n"] {
        let value = serde_json::json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"你好🌍"}});
        let wire = format!("event: content_block_delta{newline}data: {value}{newline}{newline}");
        for split in 0..=wire.len() {
            let mut buffer = BytesMut::new();
            let mut decoded = Vec::new();
            for chunk in [&wire.as_bytes()[..split], &wire.as_bytes()[split..]] {
                buffer.extend_from_slice(chunk);
                while let Some(end) = event_end(&buffer) {
                    let frame = buffer.split_to(end);
                    decoded.push(event_json(&frame).unwrap().unwrap());
                }
            }
            assert!(buffer.is_empty());
            assert_eq!(decoded.as_slice(), std::slice::from_ref(&value));
        }
    }
}

#[test]
fn request_observation_records_explicit_claude_effort_without_changing_payload() {
    for effort in ["low", "medium", "high", "xhigh", "max"] {
        let operation = observed_request(
            NAME,
            serde_json::json!({
                "model":"claude-opus-5-5", "thinking":{"type":"adaptive"},
                "output_config":{"effort":effort}, "messages":[]
            }),
        );
        let original = operation.wire_body().unwrap();
        let observation = request_observation(&operation);
        assert_eq!(observation.reasoning_effort.as_deref(), Some(effort));
        assert_eq!(
            observation
                .requested_model
                .as_ref()
                .map(|model| model.as_str()),
            Some("claude-opus-5-5")
        );
        assert_eq!(observation.thinking_mode.as_deref(), Some("adaptive"));
        assert_eq!(observation.reasoning_observed, Some(true));
        assert!(observation.reasoning_preset.is_none());
        assert_eq!(operation.wire_body().unwrap(), original);
    }
}

#[test]
fn request_observation_does_not_guess_effort_from_thinking_or_other_protocols() {
    for body in [
        serde_json::json!({}),
        serde_json::json!({"thinking":{"type":"adaptive"}}),
        serde_json::json!({"thinking":{"type":"enabled","budget_tokens":32000}}),
        serde_json::json!({"thinking":{"type":"disabled"}}),
        serde_json::json!({"output_config":{"effort":null}}),
        serde_json::json!({"output_config":{"effort":123}}),
        serde_json::json!({"output_config":{"effort":"private arbitrary text"}}),
    ] {
        assert!(
            request_observation(&observed_request(NAME, body))
                .reasoning_effort
                .is_none()
        );
    }
    let body = serde_json::json!({"output_config":{"effort":"high"}});
    assert!(
        request_observation(&observed_request("openai", body.clone()))
            .reasoning_effort
            .is_none()
    );
    let count = Operation::CountTokens(gateway_core::operation::TokenCountRequest::from_raw_json(
        RawJsonPayload::new(NAME, Bytes::from(body.to_string())).unwrap(),
    ));
    assert!(request_observation(&count).reasoning_effort.is_none());
}

#[test]
fn thinking_modes_budgets_and_count_models_are_independent_observations() {
    for mode in ["adaptive", "enabled", "disabled", "between_tools"] {
        let body = serde_json::json!({"model":"claude-sonnet-4-5","thinking":{"type":mode,"budget_tokens":32000}});
        let observation = request_observation(&observed_request(NAME, body.clone()));
        assert_eq!(observation.thinking_mode.as_deref(), Some(mode));
        assert_eq!(observation.thinking_budget_tokens, Some(32000));
        assert!(observation.reasoning_effort.is_none());
        assert!(observation.reasoning_preset.is_none());
        let count =
            Operation::CountTokens(gateway_core::operation::TokenCountRequest::from_raw_json(
                RawJsonPayload::new(NAME, Bytes::from(body.to_string())).unwrap(),
            ));
        let counted = request_observation(&count);
        assert_eq!(
            counted.requested_model.as_ref().map(|model| model.as_str()),
            Some("claude-sonnet-4-5")
        );
        assert!(counted.thinking_mode.is_none());
        assert!(counted.reasoning_observed.is_none());
    }
    for budget in [
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!(u64::MAX),
    ] {
        let observation = request_observation(&observed_request(
            NAME,
            serde_json::json!({"thinking":{"type":"enabled","budget_tokens":budget}}),
        ));
        assert!(observation.thinking_budget_tokens.is_none());
    }
}

#[test]
fn only_explicit_revocation_or_ban_changes_credential_state() {
    for (status, kind, code, message, expected) in [
        (
            401,
            "authentication_error",
            Some("token_revoked"),
            "revoked",
            Some(CredentialState::Expired),
        ),
        (
            401,
            "authentication_error",
            None,
            "Invalid bearer token. Please log in.",
            Some(CredentialState::Expired),
        ),
        (
            403,
            "permission_error",
            Some("account_disabled"),
            "disabled",
            Some(CredentialState::Banned),
        ),
        (
            403,
            "permission_error",
            None,
            "Your account has been banned.",
            Some(CredentialState::Banned),
        ),
        (
            401,
            "authentication_error",
            None,
            "OAuth token has expired",
            None,
        ),
        (
            401,
            "authentication_error",
            None,
            "Authentication failed",
            None,
        ),
        (
            403,
            "permission_error",
            None,
            "This model requires a different subscription",
            None,
        ),
        (
            403,
            "permission_error",
            None,
            "Model denied. Your account has been banned.",
            None,
        ),
        (
            429,
            "rate_limit_error",
            Some("account_disabled"),
            "limited",
            None,
        ),
    ] {
        let fields = UpstreamFailureFields {
            status: Some(status),
            code: code.map(str::to_owned),
            error_type: Some(kind.into()),
            message: Some(message.into()),
        };
        assert_eq!(
            rejected_credential_state(&fields),
            expected,
            "{status} {message}"
        );
    }
}

#[test]
fn local_and_model_failures_do_not_penalize_the_account() {
    for (kind, state, scope, expected) in [
        (
            ProviderErrorKind::InvalidRequest,
            UpstreamSendState::Sent,
            "model",
            false,
        ),
        (
            ProviderErrorKind::PermissionDenied,
            UpstreamSendState::Sent,
            "model",
            false,
        ),
        (
            ProviderErrorKind::RateLimited,
            UpstreamSendState::Sent,
            "account",
            true,
        ),
        (
            ProviderErrorKind::Transport,
            UpstreamSendState::NotSent,
            "account",
            false,
        ),
    ] {
        let error = ProviderError::new(kind, state).with_diagnostic(
            gateway_core::error::ProviderDiagnostic::new("fixture")
                .with_classification("anthropic_upstream", scope),
        );
        assert_eq!(score_account_failure(&error), expected);
    }
    assert!(!score_account_failure(&veil_error(
        gateway_veil::Error::NotConfigured,
        UpstreamSendState::NotSent
    )));
}

#[test]
fn local_protocol_and_service_failures_never_penalize_an_account() {
    for kind in [
        ProviderErrorKind::Protocol,
        ProviderErrorKind::ProviderInfrastructureUnavailable,
    ] {
        assert!(!score_account_failure(&sent(kind)));
    }
    assert!(score_account_failure(&sent(ProviderErrorKind::Transport)));
    for status in [400, 403, 500, 502, 503, 529] {
        let scope = crate::quota::error_scope(status, &Default::default(), b"{}");
        let error = sent(ProviderErrorKind::Unavailable)
            .with_status(status)
            .with_diagnostic(
                gateway_core::error::ProviderDiagnostic::new("fixture")
                    .with_classification("anthropic_upstream", scope.code()),
            );
        assert!(!score_account_failure(&error), "{status}");
    }
    for kind in [
        Some("overloaded_error"),
        Some("api_error"),
        Some("future_error"),
        None,
    ] {
        let (kind, scope) = stream_error_classification(kind);
        let error = sent(kind).with_diagnostic(
            gateway_core::error::ProviderDiagnostic::new("fixture")
                .with_classification("anthropic_upstream", scope),
        );
        assert!(!score_account_failure(&error));
    }
    assert_eq!(
        stream_error_classification(Some("authentication_error")).1,
        "account"
    );
    assert_eq!(
        stream_error_classification(Some("rate_limit_error")).1,
        "model"
    );
    for (status, expected) in [(401, "account"), (429, "model")] {
        assert_eq!(
            crate::quota::error_scope(status, &Default::default(), b"{}").code(),
            expected
        );
    }
}
