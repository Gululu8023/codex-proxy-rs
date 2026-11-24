//! 缓存桶、未知价格与中断后的已观测用量不能在计费时丢失。

use super::*;
use gateway_core::upstream::UpstreamTransport;

#[test]
fn recorded_projection_uses_historical_rates_and_keeps_cache_write_premium() {
    use gateway_admin::model::observability::{CurrencyCost, ProviderBillingInput};

    let input = ProviderBillingInput {
        upstream_model_id: "retired-model".into(),
        service_tier: None,
        input_tokens: Some(600),
        output_tokens: Some(10),
        cached_input_tokens: Some(200),
        cache_write_input_tokens: Some(300),
        total: CurrencyCost {
            currency: "USD".into(),
            amount: "0.0019725".parse().unwrap(),
        },
    };
    let mut recorded = json!({
        "kind":"api_equivalent_estimate",
        "costBreakdown":{"currency":"USD","input":"0.0003","output":"0.00015",
            "cacheRead":"0.00006","cacheWrite":"0.0014625","cacheWrite5m":"0.0005625",
            "cacheWrite1h":"0.0009","standard":"0.0019725","total":"0.0019725"},
        "pricing":{"model":"retired-model","inputTicksPerToken":30000,
            "outputTicksPerToken":150000,"multiplierBps":10000}
    });
    let projected = recorded_billing_projection(&input, &recorded).unwrap();
    assert_eq!(projected.no_cache_amount.unwrap().as_str(), "0.00195");
    assert_eq!(projected.standard_amount.unwrap(), input.total.amount);
    recorded["pricing"]["inputTicksPerToken"] = json!(60000);
    assert!(recorded_billing_projection(&input, &recorded).is_none());
    recorded["pricing"]["inputTicksPerToken"] = json!(30000);
    recorded["costBreakdown"]["cacheWrite"] = json!("0");
    assert!(recorded_billing_projection(&input, &recorded).is_none());
    assert!(recorded_billing_projection(&input, &json!({})).is_none());
}

fn facts(events: &[ProviderEvent]) -> Vec<&GatewayEvent> {
    events
        .iter()
        .flat_map(ProviderEvent::canonical_facts)
        .collect()
}

#[test]
fn usage_updates_keep_quota_headers_status_and_stream_timings() {
    use gateway_core::event::{ProviderResponseHeader, ProviderResponseTimings};
    let quota = json!({"claudeQuota":{"planType":"max","windows":{
        "seven_day":{"utilization":25,"resets_at":"2026-11-20T00:00:00Z"}
    }}});
    let observation = ProviderResponseObservation::new(UpstreamTransport::new("http_sse").unwrap())
        .with_status_code(200)
        .with_client_headers(vec![ProviderResponseHeader::new(
            "request-id",
            bytes::Bytes::from_static(b"redacted"),
        )])
        .with_timings(ProviderResponseTimings {
            headers_ms: Some(10),
            first_token_ms: Some(30),
            ..Default::default()
        })
        .with_provider_metadata(ProviderResponseMetadata::new(quota.to_string()).unwrap());
    let mut capture = UsageCapture::default();
    capture.observe(&json!({"input_tokens":10,"output_tokens":2}));
    for finalized in [false, true] {
        let events = capture.finish(
            "claude-sonnet-4-5",
            &Default::default(),
            &json!({}),
            finalized,
            &observation,
        );
        let snapshot = events
            .iter()
            .find_map(ProviderEvent::response_observation)
            .unwrap();
        assert_eq!(snapshot.status_code(), observation.status_code());
        assert_eq!(snapshot.client_headers(), observation.client_headers());
        assert_eq!(snapshot.timings(), observation.timings());
        let metadata: Value =
            serde_json::from_str(snapshot.provider_metadata().unwrap().as_json()).unwrap();
        assert_eq!(metadata["claudeQuota"], quota["claudeQuota"]);
        assert_eq!(metadata["rawUsage"]["output_tokens"], 2);
        assert_eq!(metadata["usageComplete"], finalized);
    }
}

#[test]
fn later_unpriced_usage_discards_previous_breakdown_but_keeps_quota() {
    let observation = ProviderResponseObservation::new(UpstreamTransport::new("http_sse").unwrap())
        .with_provider_metadata(
            ProviderResponseMetadata::new(json!({"claudeQuota":{"planType":"max"}}).to_string())
                .unwrap(),
        );
    let mut capture = UsageCapture::default();
    capture.observe(&json!({"input_tokens":100,"output_tokens":0}));
    let first = capture.finish(
        "claude-sonnet-4-5",
        &Default::default(),
        &json!({}),
        false,
        &observation,
    );
    let previous = first
        .iter()
        .find_map(ProviderEvent::response_observation)
        .unwrap();
    let metadata: Value =
        serde_json::from_str(previous.provider_metadata().unwrap().as_json()).unwrap();
    assert!(metadata["costBreakdown"].is_object());

    capture.observe(&json!({"output_tokens":10,"cache_creation_input_tokens":20}));
    let last = capture.finish(
        "claude-sonnet-4-5",
        &Default::default(),
        &json!({}),
        true,
        previous,
    );
    assert!(
        !facts(&last)
            .iter()
            .any(|event| matches!(event, GatewayEvent::CalculatedCost(_)))
    );
    let snapshot = last
        .iter()
        .find_map(ProviderEvent::response_observation)
        .unwrap();
    let metadata: Value =
        serde_json::from_str(snapshot.provider_metadata().unwrap().as_json()).unwrap();
    assert!(metadata.get("costBreakdown").is_none());
    assert!(metadata.get("pricing").is_none());
    assert_eq!(metadata["claudeQuota"]["planType"], "max");
    assert_eq!(metadata["rawUsage"]["output_tokens"], 10);
    assert_eq!(metadata["usageComplete"], true);
}

#[test]
fn thinking_usage_is_explicit_partial_and_never_double_billed() {
    let mut capture = UsageCapture::default();
    capture.observe(&json!({"input_tokens":100,"output_tokens":0}));
    capture.observe(&json!({"output_tokens":20,"output_tokens_details":{"thinking_tokens":12,"private_text":"never persist"}}));
    capture.observe(&json!({"output_tokens":30}));
    assert_eq!(
        capture.fields["output_tokens_details"],
        json!({"thinking_tokens":12})
    );
    let events = capture.finish(
        "claude-sonnet-4-5",
        &Default::default(),
        &json!({}),
        true,
        &ProviderResponseObservation::new(UpstreamTransport::new("http_sse").unwrap()),
    );
    let observed = facts(&events);
    let usage = observed
        .iter()
        .find_map(|event| match event {
            GatewayEvent::Usage(usage) => Some(usage),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (
            usage.output_tokens,
            usage.reasoning_output_tokens,
            usage.total_tokens
        ),
        (Some(30), Some(12), Some(130))
    );
    let cost = observed
        .iter()
        .find_map(|event| match event {
            GatewayEvent::CalculatedCost(cost) => Some(cost.total().amount().canonical()),
            _ => None,
        })
        .unwrap();
    assert_eq!(cost, "0.00075");
    capture.observe(&json!({"output_tokens_details":{"thinking_tokens":31}}));
    let events = capture.finish(
        "claude-sonnet-4-5",
        &Default::default(),
        &json!({}),
        true,
        &ProviderResponseObservation::new(UpstreamTransport::new("http_sse").unwrap()),
    );
    assert!(facts(&events).iter().any(|event| matches!(event, GatewayEvent::Usage(usage) if usage.reasoning_output_tokens.is_none())));
}

#[test]
fn cache_buckets_merge_across_frames_and_are_priced_separately() {
    let mut capture = UsageCapture::default();
    capture.observe(&json!({"input_tokens":100,"cache_read_input_tokens":30,
        "cache_creation_input_tokens":30,"output_tokens":0,
        "cache_creation":{"ephemeral_5m_input_tokens":10},"private_text":"never persist"}));
    capture.observe(&json!({"output_tokens":20,
        "cache_creation":{"ephemeral_1h_input_tokens":20}}));
    assert_eq!(
        capture.fields["cache_creation"],
        json!({
        "ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":20})
    );
    assert!(!capture.fields.contains_key("private_text"));
    for finalized in [false, true] {
        let events = capture.finish(
            "claude-sonnet-4-5",
            &Default::default(),
            &json!({}),
            finalized,
            &ProviderResponseObservation::new(UpstreamTransport::new("http_sse").unwrap()),
        );
        let facts = facts(&events);
        let usage = facts
            .iter()
            .find_map(|fact| match fact {
                GatewayEvent::Usage(value) => Some(value),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            (usage.input_tokens, usage.output_tokens, usage.total_tokens),
            (Some(160), Some(20), Some(180))
        );
        let amount = facts
            .iter()
            .find_map(|fact| match fact {
                GatewayEvent::CalculatedCost(value) => Some(value.total().amount().canonical()),
                _ => None,
            })
            .unwrap();
        // $3/M 输入、$15/M 输出、5m 写入 1.25 倍、1h 写入 2 倍、读取 $0.3/M。
        assert_eq!(amount, "0.0007665");
    }
}

#[test]
fn unknown_or_incomplete_pricing_does_not_invent_zero_cost() {
    for (model, usage, request) in [
        (
            "claude-unpriced-future",
            json!({"input_tokens":1,"output_tokens":1}),
            json!({}),
        ),
        (
            "claude-sonnet-4-5",
            json!({"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":10}),
            json!({}),
        ),
        (
            "claude-sonnet-4-5",
            json!({"input_tokens":1,"output_tokens":1}),
            json!({"speed":"fast"}),
        ),
        (
            "claude-sonnet-4-5",
            json!({"input_tokens":1,"output_tokens":1,"server_tool_use":{"web_search_requests":1}}),
            json!({}),
        ),
    ] {
        let mut capture = UsageCapture::default();
        capture.observe(&usage);
        let events = capture.finish(
            model,
            &Default::default(),
            &request,
            true,
            &ProviderResponseObservation::new(UpstreamTransport::new("http_sse").unwrap()),
        );
        let facts = facts(&events);
        assert!(
            facts
                .iter()
                .any(|fact| matches!(fact, GatewayEvent::Usage(_)))
        );
        assert!(
            !facts
                .iter()
                .any(|fact| matches!(fact, GatewayEvent::CalculatedCost(_))),
            "{model} {usage}"
        );
    }
}
