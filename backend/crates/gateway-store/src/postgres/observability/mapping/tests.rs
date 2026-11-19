use super::*;
use admin_observability::{CurrencyCost, UsageBilling};
use serde_json::{Value, json};

fn total(source: &str) -> Option<UsageBilling> {
    Some(UsageBilling::Total {
        source: source.into(),
        total: CurrencyCost {
            currency: "USD".into(),
            amount: "0.10".parse().unwrap(),
        },
    })
}

fn recorded() -> Value {
    // 四个主分项与账本总额相等，TTL 子项已经包含在 cacheWrite 中。
    json!({"kind":"api_equivalent_estimate","costBreakdown":{
        "currency":"USD","input":"0.01","output":"0.05","cacheRead":"0.01",
        "cacheWrite":"0.03","cacheWrite5m":"0.01","cacheWrite1h":"0.02","total":"0.10"
    }})
}

#[test]
fn recorded_billing_restores_exact_amounts_without_repricing() {
    let saved = recorded();
    let Some(UsageBilling::Recorded {
        metadata, total, ..
    }) = restore_recorded_billing(total("calculated"), Some(saved.clone()))
    else {
        panic!("matching breakdown should be restored");
    };
    assert_eq!(metadata, saved);
    assert_eq!(total.amount, "0.10".parse().unwrap());
}

#[test]
fn recorded_billing_rejects_mismatched_or_incomplete_amounts() {
    for (field, value) in [
        ("total", json!("0.11")),
        ("currency", json!("EUR")),
        ("output", json!("0.04")),
        ("input", Value::Null),
        ("input", json!("-0.01")),
        ("cacheRead", json!(0.01)),
    ] {
        let mut saved = recorded();
        saved["costBreakdown"][field] = value;
        assert!(
            matches!(
                restore_recorded_billing(total("calculated"), Some(saved)),
                Some(UsageBilling::Total { .. })
            ),
            "invalid {field}"
        );
    }
}

#[test]
fn recorded_billing_does_not_invent_unknown_or_replace_provider_reported_costs() {
    assert!(restore_recorded_billing(None, Some(recorded())).is_none());
    assert!(matches!(
        restore_recorded_billing(total("provider_reported"), Some(recorded())),
        Some(UsageBilling::Total { .. })
    ));
    assert!(matches!(
        restore_recorded_billing(total("calculated"), None),
        Some(UsageBilling::Total { .. })
    ));
}
