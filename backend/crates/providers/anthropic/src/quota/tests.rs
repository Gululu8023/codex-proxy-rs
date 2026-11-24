//! 通用窗口解析、历史配对和本地暂停证据的纯逻辑回归，不模拟上游授权或消费成功。

use super::*;
use chrono::{Duration as ChronoDuration, TimeZone as _};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 19, 0, 0, 0).unwrap()
}

fn limits() -> AccountQuotaLimits {
    serde_json::from_value(json!({"five_hour":80,"seven_day":90})).unwrap()
}

fn window(key: &str, seconds: u64) -> ProviderQuotaWindow {
    ProviderQuotaWindow {
        upstream: None,
        key: key.into(),
        group: "claude_subscription".into(),
        label: key.into(),
        limit_id: Some("account".into()),
        limit_name: None,
        role: None,
        local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
        window_seconds: Some(seconds),
        used_percent: None,
        reset_at: None,
        limit_reached: false,
        local_usage: None,
        provider_data: None,
    }
}

#[test]
fn only_complete_generic_headers_become_forecast_observations() {
    let reset = (now() + ChronoDuration::hours(1)).timestamp().to_string();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "anthropic-ratelimit-unified-5h-utilization",
        "0.25".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-unified-5h-reset",
        reset.parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-unified-7d-utilization",
        "0.5".parse().unwrap(),
    );
    // 缺 reset 的通用窗口和模型专属窗口都不能补造配对样本。
    headers.insert(
        "anthropic-ratelimit-unified-7d_sonnet-utilization",
        "0.9".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-unified-7d_sonnet-reset",
        reset.parse().unwrap(),
    );
    let windows = header_windows(&headers, now());
    assert_eq!(windows.len(), 1);
    assert_eq!(windows["five_hour"]["utilization"], 25.0);
    let metadata = document(json!({"claudeQuota":{"planType":"max","windows":windows}}));
    let observed = forecast_observation(&metadata, &window("five_hour", 18_000)).unwrap();
    assert_eq!(observed.used_percent, 25.0);
    assert_eq!(observed.reset_at, now() + ChronoDuration::hours(1));
    assert_eq!(observed.plan_type.as_deref(), Some("max"));
    assert!(forecast_observation(&metadata, &window("five_hour", 604_800)).is_none());
    assert!(forecast_observation(&metadata, &window("seven_day_sonnet", 604_800)).is_none());
    let mut scoped = window("five_hour", 18_000);
    scoped.local_usage_attribution = QuotaLocalUsageAttribution::Unavailable;
    assert!(forecast_observation(&metadata, &scoped).is_none());
}

#[test]
fn duplicate_invalid_and_expired_headers_do_not_produce_samples() {
    let reset = (now() + ChronoDuration::hours(1)).timestamp().to_string();
    for invalid in ["NaN", "inf", "-0.01", "unknown"] {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            invalid.parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-5h-reset",
            reset.parse().unwrap(),
        );
        assert!(header_windows(&headers, now()).is_empty());
    }
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "anthropic-ratelimit-unified-5h-utilization",
        "0.5".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-unified-5h-reset",
        reset.parse().unwrap(),
    );
    headers.append(
        "anthropic-ratelimit-unified-5h-utilization",
        "0.6".parse().unwrap(),
    );
    assert!(header_windows(&headers, now()).is_empty());
    headers.remove("anthropic-ratelimit-unified-5h-utilization");
    headers.insert(
        "anthropic-ratelimit-unified-5h-utilization",
        "0.5".parse().unwrap(),
    );
    assert!(header_windows(&headers, now() + ChronoDuration::hours(2)).is_empty());
}

#[test]
fn pauses_use_inclusive_account_limits_and_ignore_model_limits() {
    let value = json!({"windows":{
        "five_hour":{"utilization":80},
        "seven_day":{"utilization":89.9},
        "seven_day_sonnet":{"utilization":100}
    }});
    assert_eq!(paused_windows(&value, &limits()), ["five_hour"]);
    assert!(paused_windows(&value, &AccountQuotaLimits::disabled()).is_empty());
    let higher = serde_json::from_value(json!({"five_hour":81,"seven_day":90})).unwrap();
    assert!(paused_windows(&value, &higher).is_empty());
}

#[test]
fn missing_or_incomplete_recovery_retains_pause_until_valid_lower_observation() {
    let reset = now() + ChronoDuration::hours(1);
    let previous = json!({"windows":{"five_hour":{"utilization":85,"resets_at":reset}}});
    let reported = json!({"seven_day":{"utilization":20,"resets_at":reset}});
    let mut value = json!({"windows":reported});
    retain_limit_evidence(
        &previous,
        &mut value,
        reported.as_object().unwrap(),
        &limits(),
        now(),
    );
    assert_eq!(paused_windows(&value, &limits()), ["five_hour"]);
    for incomplete in [
        json!({"five_hour":{"utilization":10}}),
        json!({"five_hour":{"utilization":10,"resets_at":now()}}),
    ] {
        let previous = value.clone();
        value["windows"] = incomplete.clone();
        retain_limit_evidence(
            &previous,
            &mut value,
            incomplete.as_object().unwrap(),
            &limits(),
            now(),
        );
        assert_eq!(paused_windows(&value, &limits()), ["five_hour"]);
    }
    let previous = value.clone();
    let recovered = json!({"five_hour":{"utilization":10,"resets_at":reset}});
    value["windows"] = recovered.clone();
    retain_limit_evidence(
        &previous,
        &mut value,
        recovered.as_object().unwrap(),
        &limits(),
        now(),
    );
    assert!(paused_windows(&value, &limits()).is_empty());
    assert!(value.get("unconfirmedLimits").is_none());
}

#[test]
fn exhausted_access_requires_complete_recovery_of_both_account_windows() {
    let state = QuotaState::exhausted(QuotaEvidence::AccountLimitReached, now().into(), None);
    let partial = json!({"windows":{"five_hour":{"utilization":0},"seven_day":{"utilization":0}}});
    assert_eq!(access_state(&partial, state, now().into()), state);
    let reset = now() + ChronoDuration::hours(1);
    let complete = json!({"windows":{
        "five_hour":{"utilization":0,"resets_at":reset},
        "seven_day":{"utilization":10,"resets_at":reset}
    }});
    assert!(!access_state(&complete, state, now().into()).is_exhausted());
}

#[test]
fn incomplete_oauth_updates_keep_known_resets_when_reaching_both_window_limits() {
    let reset_5h = now() + ChronoDuration::hours(1);
    let reset_7d = now() + ChronoDuration::days(3);
    let previous = json!({"windows":{
        "five_hour":{"utilization":95,"resets_at":reset_5h},
        "seven_day":{"utilization":98,"resets_at":reset_7d}
    }});
    for reset in [None, Some(Value::Null), Some(json!("invalid"))] {
        let mut reported = json!({
            "five_hour":{"utilization":100},
            "seven_day":{"utilization":100}
        })
        .as_object()
        .unwrap()
        .clone();
        if let Some(reset) = reset {
            for window in reported.values_mut() {
                window["resets_at"] = reset.clone();
            }
        }
        retain_window_resets(&previous, &mut reported, now());
        assert_eq!(
            timestamp(&reported["five_hour"]["resets_at"]),
            Some(reset_5h)
        );
        assert_eq!(
            timestamp(&reported["seven_day"]["resets_at"]),
            Some(reset_7d)
        );
        let value = json!({"windows":reported,"extraUsageEnabled":false});
        let state = access_state(&value, QuotaState::allowed(now().into()), now().into());
        assert!(state.is_exhausted());
        assert_eq!(state.reset_at(), Some(reset_7d.into()));
    }
}

#[test]
fn oauth_reset_retention_does_not_invent_the_next_cycle() {
    let reset = now() + ChronoDuration::hours(1);
    let previous = json!({"windows":{"five_hour":{"utilization":100,"resets_at":reset}}});
    for (incoming, observed) in [
        (json!({"utilization":0,"resets_at":null}), now()),
        (json!({"utilization":10}), now()),
        (json!({"utilization":100}), reset),
        (
            json!({"utilization":10,"resets_at":reset + ChronoDuration::hours(5)}),
            now(),
        ),
    ] {
        let mut reported = json!({"five_hour":incoming}).as_object().unwrap().clone();
        let original = reported.clone();
        retain_window_resets(&previous, &mut reported, observed);
        assert_eq!(reported, original);
    }
}

#[test]
fn omitted_oauth_window_keeps_its_valid_reset_and_original_observation_time() {
    let reset = now() + ChronoDuration::hours(1);
    let previous = json!({"windows":{"five_hour":{
        "utilization":100,"resets_at":reset,"observedAt":now()-ChronoDuration::minutes(1)
    }}});
    let mut reported = serde_json::Map::new();
    retain_window_resets(&previous, &mut reported, now());
    assert_eq!(reported["five_hour"], previous["windows"]["five_hour"]);
    let mut expired = serde_json::Map::new();
    retain_window_resets(&previous, &mut expired, reset);
    assert!(expired.is_empty());
}

#[test]
fn fresh_inactive_oauth_window_recovers_exhaustion_and_threshold_pause() {
    let reset = now() + ChronoDuration::hours(1);
    let previous = json!({"windows":{"five_hour":{"utilization":100,"resets_at":now()}}});
    let windows = json!({
        "five_hour":{"utilization":0,"resets_at":null,"source":"oauth_usage","observedAt":now()},
        "seven_day":{"utilization":28,"resets_at":reset}
    });
    let state = QuotaState::exhausted(QuotaEvidence::AccountLimitReached, now().into(), None);
    let mut value = json!({"windows":windows});
    retain_limit_evidence(
        &previous,
        &mut value,
        windows.as_object().unwrap(),
        &limits(),
        now(),
    );
    assert!(paused_windows(&value, &limits()).is_empty());
    assert!(!access_state(&value, state, now().into()).is_exhausted());
    for incomplete in [
        json!({"utilization":0,"source":"oauth_usage","observedAt":now()}),
        json!({"utilization":7,"resets_at":null,"source":"oauth_usage","observedAt":now()}),
        json!({"utilization":0,"resets_at":null,"source":"response_headers","observedAt":now()}),
        json!({"utilization":0,"resets_at":null,"source":"oauth_usage","observedAt":now()-ChronoDuration::hours(1)}),
    ] {
        value["windows"]["five_hour"] = incomplete;
        assert_eq!(access_state(&value, state, now().into()), state);
    }
}

#[test]
fn overlapping_response_cannot_clear_newer_limit_but_can_report_higher_usage() {
    let previous =
        json!({"windows":{"five_hour":{"utilization":85},"seven_day":{"utilization":20}}});
    let mut incoming = json!({"five_hour":{"utilization":10},"seven_day":{"utilization":95}})
        .as_object()
        .unwrap()
        .clone();
    retain_overlapping_increases(&previous, &mut incoming, None);
    assert!(!incoming.contains_key("five_hour"));
    assert_eq!(incoming["seven_day"]["utilization"], 95);
}

#[test]
fn overlapping_response_from_old_plan_cannot_replace_refreshed_plan_observation() {
    let previous = json!({"planType":"max","windows":{"seven_day":{"utilization":20}}});
    let reported = json!({"seven_day":{"utilization":95}})
        .as_object()
        .unwrap()
        .clone();
    let mut incoming = reported.clone();
    retain_overlapping_increases(&previous, &mut incoming, Some("pro"));
    assert!(incoming.is_empty());
    let mut incoming = reported;
    retain_overlapping_increases(&previous, &mut incoming, Some("MAX"));
    assert_eq!(incoming["seven_day"]["utilization"], 95);
}

#[test]
fn overlapping_old_cycle_cannot_replace_new_cycle_even_with_higher_usage() {
    let reset = now() + ChronoDuration::days(7);
    let previous = json!({"windows":{"seven_day":{"utilization":5,"resets_at":reset}}});
    for (delta, accepted) in [
        (-604_800, false),
        (-2, true),
        (0, true),
        (2, true),
        (3, false),
    ] {
        let mut incoming = json!({"seven_day":{"utilization":95,"resets_at":reset + ChronoDuration::seconds(delta)}})
            .as_object().unwrap().clone();
        retain_overlapping_increases(&previous, &mut incoming, None);
        assert_eq!(
            incoming.contains_key("seven_day"),
            accepted,
            "reset delta {delta}"
        );
    }
}

#[test]
fn request_metadata_uses_the_header_arrival_time() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "anthropic-ratelimit-unified-7d-utilization",
        "0.25".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-unified-7d-reset",
        (now() + ChronoDuration::days(1))
            .timestamp()
            .to_string()
            .parse()
            .unwrap(),
    );
    let metadata = response_metadata(&headers, Some("max"), now()).unwrap();
    let value: Value = serde_json::from_str(metadata.as_json()).unwrap();
    assert_eq!(
        timestamp(&value["claudeQuota"]["windows"]["seven_day"]["observedAt"]),
        Some(now())
    );
}

#[test]
fn partial_header_updates_do_not_refresh_old_weekly_samples_or_scheduling_signals() {
    let old = now() - ChronoDuration::minutes(10);
    let reset = now() + ChronoDuration::hours(1);
    let value = json!({"observedAt":now(),"windows":{
        "five_hour":{"utilization":10,"resets_at":reset,"observedAt":now()},
        "seven_day":{"utilization":99,"resets_at":reset,"observedAt":old}
    }});
    assert_eq!(account_observed_at(&value), Some(old));
    let quota = OpaqueProviderData::new(value.as_object().unwrap().clone());
    let signals = scheduling_signals(&quota, now().into()).unwrap();
    let expected = json!({"observedAt":now(),"windows":{
        "five_hour":{"utilization":10,"resets_at":reset,"observedAt":now()}
    }});
    let expected = OpaqueProviderData::new(expected.as_object().unwrap().clone());
    assert_eq!(
        signals,
        scheduling_signals(&expected, now().into()).unwrap()
    );
}

#[test]
fn changed_plan_invalidates_old_percentages_without_discarding_a_confirmed_pause() {
    let mut value = json!({"planType":"pro","observedAt":now(),"windows":{
        "five_hour":{"utilization":85,"resets_at":now() + ChronoDuration::hours(1)},
        "seven_day":{"utilization":50,"resets_at":now() + ChronoDuration::days(1)}
    }});
    let original = value.clone();
    invalidate_changed_plan(&mut value, Some("PRO"), &limits(), now());
    assert_eq!(value, original);
    invalidate_changed_plan(&mut value, Some("max"), &limits(), now());
    assert!(value["windows"].as_object().unwrap().is_empty());
    assert!(account_observed_at(&value).is_none());
    assert_eq!(paused_windows(&value, &limits()), ["five_hour"]);
}
