use super::*;
use chrono::TimeDelta;
use serde_json::json;

#[test]
fn forecast_time_uses_headers_for_claude_and_completion_for_legacy_providers() {
    let start = DateTime::parse_from_rfc3339("2026-11-19T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let headers = start + TimeDelta::seconds(2);
    let end = start + TimeDelta::minutes(2);
    let path = ["claudeQuota", "windows", "seven_day", "observedAt"].map(String::from);
    let document = json!({"claudeQuota":{"windows":{"seven_day":{"observedAt":headers}}}});
    assert_eq!(
        observation_time(&document, Some(&path), start, end),
        Some(headers)
    );
    assert_eq!(observation_time(&document, None, start, end), Some(end));
}

#[test]
fn missing_invalid_or_outside_request_times_are_not_replaced_with_completion() {
    let start = DateTime::parse_from_rfc3339("2026-11-19T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let end = start + TimeDelta::minutes(2);
    let path = ["observedAt".to_owned()];
    for document in [
        json!({}),
        json!({"observedAt":"invalid"}),
        json!({"observedAt":0}),
        json!({"observedAt":start - TimeDelta::microseconds(1)}),
        json!({"observedAt":end + TimeDelta::microseconds(1)}),
    ] {
        assert_eq!(observation_time(&document, Some(&path), start, end), None);
    }
    for time in [start, end] {
        assert_eq!(
            observation_time(&json!({"observedAt":time}), Some(&path), start, end),
            Some(time)
        );
    }
}
