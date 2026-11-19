use super::*;
use serde_json::json;

fn inventory() -> Value {
    // 公开脱敏响应原样字段：https://github.com/steipete/CodexBar/issues/3895#issuecomment-5808949886
    // 此样本只证明库存解析，不证明 CPRS 已取得真实账号权限或消费成功。
    json!({"cedar_ember":{
        "eligible":true,"grants":[{
            "id":"<redacted>","label":"<redacted>","resets_total":1,"resets_left":1,
            "starts_at":"2026-09-22T16:00:00+00:00","ends_at":"2026-10-22T16:00:00+00:00",
            "clears":["five_hour","seven_day","seven_day_overage_included"],
            "paused":false,"usable_now":true
        }]
    }})
}

#[test]
fn published_redacted_inventory_preserves_scope_and_expiry() {
    let sample = inventory();
    let now = date(&sample["cedar_ember"]["grants"][0]["starts_at"]).unwrap();
    let result = parse_inventory(&sample, now).unwrap();
    assert_eq!(result.available_count, 1);
    assert_eq!(result.credits[0].status.as_deref(), Some("available"));
    assert_eq!(
        result.credits[0].reset_type.as_deref(),
        Some("five_hour, seven_day, seven_day_overage_included")
    );
    assert_eq!(
        result.credits[0].expires_at,
        date(&sample["cedar_ember"]["grants"][0]["ends_at"])
    );
}

#[test]
fn absent_ineligible_or_malformed_inventory_is_unknown_not_zero() {
    for sample in [
        json!({}),
        json!({"cedar_ember":null}),
        json!({"cedar_ember":{"eligible":false,"ineligible_reason":"surface"}}),
        json!({"cedar_ember":{"eligible":true}}),
    ] {
        assert!(parse_inventory(&sample, Utc::now()).is_err());
    }
    let mut malformed = inventory();
    malformed["cedar_ember"]["grants"][0]["resets_left"] = Value::Null;
    assert!(parse_inventory(&malformed, Utc::now()).is_err());
}

#[test]
fn expiry_boundary_excludes_saved_grant_and_only_explicit_empty_inventory_is_zero() {
    let sample = inventory();
    let expires_at = date(&sample["cedar_ember"]["grants"][0]["ends_at"]).unwrap();
    let result = parse_inventory(&sample, expires_at).unwrap();
    assert_eq!(result.available_count, 0);
    assert_eq!(result.credits[0].status.as_deref(), Some("expired"));
    assert_eq!(
        parse_inventory(
            &json!({"cedar_ember":{"eligible":true,"grants":[]}}),
            expires_at
        )
        .unwrap()
        .available_count,
        0
    );
}
