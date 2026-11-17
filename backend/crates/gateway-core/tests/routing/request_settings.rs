//! 验证运行设置编译以及 Key 默认值与宿主画像的解析和继承

use std::{collections::BTreeMap, sync::Arc};

use gateway_core::{
    account::FastMode,
    account::OpaqueProviderData,
    identity::ProviderKind,
    policy::{ClientApiKeyId, ClientPolicy, PlaintextClientApiKey, RateLimits},
    routing::request_settings::RequestSettings,
    routing::{ClientRoutingScope, ConfigRevision, FrozenAccountScope, RuntimeSnapshot},
    settings::SettingsValues,
};
use serde_json::json;

fn profiles(entries: &[(&str, &str)]) -> BTreeMap<ProviderKind, OpaqueProviderData> {
    entries
        .iter()
        .map(|(provider, name)| {
            (
                ProviderKind::new(*provider).unwrap(),
                OpaqueProviderData::new(json!({"identity":name}).as_object().unwrap().clone()),
            )
        })
        .collect()
}

fn snapshot(revision: u64, name: &str) -> Arc<RuntimeSnapshot> {
    Arc::new(
        RuntimeSnapshot::new(
            ConfigRevision::new(revision).unwrap(),
            SettingsValues::new(3, "smart", BTreeMap::new(), None, None)
                .with_request_profiles(profiles(&[("openai", name), ("xai", name)])),
            vec![],
            vec![],
            vec![],
        )
        .unwrap(),
    )
}

fn policy(id: &str) -> ClientPolicy {
    policy_with_profiles(id, profiles(&[("openai", id)]))
}

fn policy_with_profiles(
    id: &str,
    overrides: BTreeMap<ProviderKind, OpaqueProviderData>,
) -> ClientPolicy {
    ClientPolicy::new(
        ClientApiKeyId::new(id).unwrap(),
        PlaintextClientApiKey::new(format!("fixture-{id}")).unwrap(),
        Arc::new(
            FrozenAccountScope::new(Arc::default(), ClientRoutingScope::all_accounts())
                .with_request_profiles(overrides)
                .with_fast_mode(FastMode::Disabled),
        ),
        true,
        RateLimits {
            max_concurrency: 3,
            requests_per_minute: 10,
        },
    )
}

#[test]
fn settings_facts_are_shared_and_invalid_compilation_does_not_change_them() {
    let original = snapshot(1, "host");
    let frozen = original.as_ref().clone();
    assert!(std::ptr::eq(original.settings(), frozen.settings()));
    let facts = serde_json::to_value(original.settings()).unwrap();
    let changed = original
        .with_settings(
            &original
                .settings()
                .clone()
                .with_model_mappings(BTreeMap::from([("alias".into(), "model".into())])),
        )
        .unwrap();
    assert_eq!(
        changed.mapped_model(&ProviderKind::new("openai").unwrap(), "alias"),
        "model"
    );
    assert_eq!(
        original.mapped_model(&ProviderKind::new("openai").unwrap(), "alias"),
        "alias"
    );
    assert_eq!(serde_json::to_value(original.settings()).unwrap(), facts);
    assert!(
        original
            .with_settings(
                &original
                    .settings()
                    .clone()
                    .with_responses_max_decompressed_body_bytes(0)
            )
            .is_err()
    );
    assert!(std::ptr::eq(original.settings(), frozen.settings()));
}

#[test]
fn inherited_key_fields_follow_the_current_global_profile_without_changing_key_defaults() {
    let provider = ProviderKind::new("openai").unwrap();
    let key_profile = OpaqueProviderData::new(
        json!({"inherit":{"device":"test-device"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    let mut key = policy_with_profiles(
        "device-key",
        BTreeMap::from([(provider.clone(), key_profile.clone())]),
    );
    for (revision, name) in [(1, "first"), (2, "second")] {
        key = RequestSettings::new(snapshot(revision, name)).apply_policy(key);
        assert_eq!(
            key.account_scope()
                .request_profile(&provider)
                .unwrap()
                .expose_to_provider(),
            json!({"identity":name,"device":"test-device"})
                .as_object()
                .unwrap()
        );
        assert_eq!(key.defaults().request_profiles[&provider], key_profile);
    }
}

#[test]
fn resolving_an_existing_policy_uses_key_defaults_and_the_current_host() {
    let key = policy("key");
    let first = RequestSettings::new(snapshot(1, "first")).apply_policy(key.clone());
    assert_eq!(
        first.account_scope().request_profiles(),
        &profiles(&[("openai", "key"), ("xai", "first")])
    );
    assert_eq!(
        first.defaults().request_profiles,
        key.defaults().request_profiles
    );
    let second = RequestSettings::new(snapshot(2, "second")).apply_policy(first);
    assert_eq!(
        second.account_scope().request_profiles(),
        &profiles(&[("openai", "key"), ("xai", "second")])
    );
    assert_eq!(
        second.defaults().request_profiles,
        profiles(&[("openai", "key")])
    );
}

#[test]
fn affinity_and_rotation_settings_are_validated_without_mutating_the_snapshot() {
    use gateway_core::account::AccountAffinity;
    let original = snapshot(1, "host");
    let facts = serde_json::to_value(original.settings()).unwrap();
    assert_eq!(facts["openai_account_affinity"], "strict");
    assert_eq!(facts["max_account_rotations"], 3);
    for (mode, rotations) in [
        (AccountAffinity::Relaxed, 0),
        (AccountAffinity::Preferred, 3),
        (AccountAffinity::Strict, 31),
    ] {
        let changed = original
            .with_settings(
                &original
                    .settings()
                    .clone()
                    .with_openai_account_affinity(mode)
                    .with_max_account_rotations(rotations),
            )
            .unwrap();
        let changed = serde_json::to_value(changed.settings()).unwrap();
        assert_eq!(changed["max_account_rotations"], rotations);
        assert_eq!(changed["openai_account_affinity"], mode.as_str());
    }
    assert!(
        original
            .with_settings(&original.settings().clone().with_max_account_rotations(32))
            .is_err()
    );
    assert_eq!(serde_json::to_value(original.settings()).unwrap(), facts);
}

#[test]
fn session_affinity_ttl_rejects_unbounded_or_empty_retention() {
    let original = snapshot(1, "host");
    assert_eq!(
        serde_json::to_value(original.settings()).unwrap()["openai_session_affinity_ttl_hours"],
        24
    );
    for hours in [1, 168, 720] {
        assert!(
            original
                .with_settings(
                    &original
                        .settings()
                        .clone()
                        .with_openai_session_affinity_ttl_hours(hours)
                )
                .is_ok()
        );
    }
    for hours in [0, 721, u32::MAX] {
        assert!(
            original
                .with_settings(
                    &original
                        .settings()
                        .clone()
                        .with_openai_session_affinity_ttl_hours(hours)
                )
                .is_err()
        );
    }
}
