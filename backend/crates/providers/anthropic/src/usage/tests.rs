//! 缓存桶、未知价格与中断后的已观测用量不能在计费时丢失。

use super::*;

fn facts(events: &[ProviderEvent]) -> Vec<&GatewayEvent> {
    events
        .iter()
        .flat_map(ProviderEvent::canonical_facts)
        .collect()
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
    let events = capture.finish("claude-sonnet-4-5", &Default::default(), &json!({}), true);
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
    let events = capture.finish("claude-sonnet-4-5", &Default::default(), &json!({}), true);
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
        let events = capture.finish(model, &Default::default(), &request, true);
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
