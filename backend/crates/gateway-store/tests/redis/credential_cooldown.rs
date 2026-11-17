//! 验证账号冷却缓存的版本隔离、期限延长与到期清理

use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use gateway_core::{
    account::{CredentialRevision, ProviderAccountId},
    provider_ports::{
        ProviderCooldownKind, ProviderCooldownPort, ProviderCooldownScope, ProviderScopedCooldown,
    },
};
use gateway_store::{
    Revision,
    redis::{CredentialCooldown, CredentialCooldownRepository, RedisCredentialCooldownRepository},
};
use redis::aio::ConnectionManager;
use uuid::Uuid;

#[test]
fn credential_cooldown_is_revision_fenced() {
    let cooldown = CredentialCooldown {
        provider_account_id: "account-1".to_owned(),
        credential_revision: Revision::new(2).expect("positive revision"),
        cooldown_until: Utc::now() + Duration::seconds(30),
        kind: ProviderCooldownKind::RateLimit,
    };
    assert_eq!(cooldown.credential_revision.get(), 2);
}

#[tokio::test]
async fn credential_cooldown_round_trips_and_indexes_active_account_without_raw_id_in_keys() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let cooldown = cooldown("acct_cooldown_round_trip", 1, 30);

    assert!(
        repository
            .cache_credential_cooldown(&cooldown)
            .await
            .expect("cache cooldown")
    );
    assert_eq!(
        repository
            .read_credential_cooldown(&cooldown.provider_account_id)
            .await
            .expect("read cooldown"),
        Some(cooldown.clone())
    );
    let mut keys = namespace_keys(&mut connection, &namespace).await;
    keys.sort();
    assert_eq!(keys.len(), 2);
    assert!(
        keys.iter()
            .all(|key| !key.contains(&cooldown.provider_account_id))
    );

    let index_key = keys
        .iter()
        .find(|key| key.ends_with(":account:active-cooldowns"))
        .expect("active cooldown index key");
    let indexed_accounts: Vec<String> = redis::cmd("ZRANGE")
        .arg(index_key)
        .arg(0)
        .arg(-1)
        .query_async(&mut connection)
        .await
        .expect("read active cooldown index");
    assert_eq!(indexed_accounts, [cooldown.provider_account_id]);
}

#[tokio::test]
async fn credential_cooldown_rejects_older_revision_and_fences_invalidation() {
    let Some((repository, _connection, _namespace)) = repository().await else {
        return;
    };
    let current = cooldown("acct_cooldown_revision", 2, 30);
    let stale = cooldown("acct_cooldown_revision", 1, 60);
    repository
        .cache_credential_cooldown(&current)
        .await
        .expect("cache current cooldown");

    assert!(
        !repository
            .cache_credential_cooldown(&stale)
            .await
            .expect("reject stale cooldown")
    );
    assert!(
        !repository
            .invalidate_credential_cooldown(
                &current.provider_account_id,
                Revision::new(1).expect("positive revision"),
            )
            .await
            .expect("fence stale invalidation")
    );
    assert!(
        repository
            .invalidate_credential_cooldown(
                &current.provider_account_id,
                current.credential_revision,
            )
            .await
            .expect("invalidate current cooldown")
    );
    assert_eq!(
        repository
            .read_credential_cooldown(&current.provider_account_id)
            .await
            .expect("read invalidated cooldown"),
        None
    );
}

#[tokio::test]
async fn credential_cooldown_same_revision_only_extends_deadline() {
    let Some((repository, _connection, _namespace)) = repository().await else {
        return;
    };
    let initial = cooldown("acct_cooldown_extend", 3, 30);
    let shorter = CredentialCooldown {
        cooldown_until: initial.cooldown_until - Duration::seconds(5),
        ..initial.clone()
    };
    let longer = CredentialCooldown {
        cooldown_until: initial.cooldown_until + Duration::seconds(5),
        ..initial.clone()
    };
    repository
        .cache_credential_cooldown(&initial)
        .await
        .expect("cache initial cooldown");

    assert!(
        !repository
            .cache_credential_cooldown(&shorter)
            .await
            .expect("reject shorter cooldown")
    );
    assert!(
        repository
            .cache_credential_cooldown(&longer)
            .await
            .expect("extend cooldown")
    );
    assert_eq!(
        repository
            .read_credential_cooldown(&initial.provider_account_id)
            .await
            .expect("read extended cooldown"),
        Some(longer)
    );
}

#[tokio::test]
async fn credential_cooldown_read_removes_expired_grace_key() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let (seconds, microseconds): (i64, i64) = redis::cmd("TIME")
        .query_async(&mut connection)
        .await
        .expect("read Redis time");
    let now_ms = seconds * 1_000 + microseconds / 1_000;
    let cooldown = CredentialCooldown {
        provider_account_id: "acct_cooldown_expiry".to_owned(),
        credential_revision: Revision::new(1).expect("positive revision"),
        cooldown_until: DateTime::from_timestamp_millis(now_ms + 30_000).unwrap(),
        kind: ProviderCooldownKind::RateLimit,
    };
    assert!(
        repository
            .cache_credential_cooldown(&cooldown)
            .await
            .expect("cache cooldown")
    );
    let keys = namespace_keys(&mut connection, &namespace).await;
    let cooldown_key = keys
        .iter()
        .find(|key| key.ends_with(":cooldown"))
        .expect("cooldown grace key");
    let index_key = keys
        .iter()
        .find(|key| key.ends_with(":account:active-cooldowns"))
        .expect("active cooldown index");
    // 把逻辑期限设为已过期并保留实体 TTL，确保由读取路径清理 grace key
    let (new_fields, new_members, ttl): (i64, i64, i64) = redis::pipe()
        .atomic()
        .cmd("HSET")
        .arg(cooldown_key)
        .arg("until_ms")
        .arg(now_ms - 1)
        .cmd("ZADD")
        .arg(index_key)
        .arg(now_ms - 1)
        .arg(&cooldown.provider_account_id)
        .cmd("PTTL")
        .arg(cooldown_key)
        .query_async(&mut connection)
        .await
        .expect("expire existing cooldown within its grace TTL");
    assert_eq!(new_fields, 0, "expiry field already exists");
    assert_eq!(new_members, 0, "account is already indexed");
    assert!(ttl > 0, "expired cooldown still has a grace key");

    assert_eq!(
        repository
            .read_credential_cooldown(&cooldown.provider_account_id)
            .await
            .expect("read expired cooldown"),
        None
    );
    assert!(namespace_keys(&mut connection, &namespace).await.is_empty());
}

#[tokio::test]
async fn scoped_cooldown_isolated_by_model_and_revision_fenced() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let account_id = ProviderAccountId::new("acct_scoped_cooldown").expect("valid account ID");
    let revision = CredentialRevision::new(2).expect("positive revision");
    let model_a = ProviderCooldownScope::upstream_model(
        gateway_core::routing::UpstreamModelId::new("grok-4.5").expect("model"),
    );
    let model_b = ProviderCooldownScope::upstream_model(
        gateway_core::routing::UpstreamModelId::new("grok-4.6").expect("model"),
    );
    let until_a: SystemTime = millisecond_precision(Utc::now() + Duration::seconds(30)).into();
    let until_b: SystemTime = millisecond_precision(Utc::now() + Duration::seconds(60)).into();

    assert!(
        repository
            .put_scoped_if_later(ProviderScopedCooldown::new(
                account_id.clone(),
                revision,
                model_a.clone(),
                until_a,
            ))
            .await
            .expect("cache model A cooldown")
    );
    assert!(
        !repository
            .put_scoped_if_later(ProviderScopedCooldown::new(
                account_id.clone(),
                CredentialRevision::new(1).expect("stale revision"),
                model_a.clone(),
                until_b,
            ))
            .await
            .expect("reject stale model A cooldown")
    );
    assert!(
        repository
            .put_scoped_if_later(ProviderScopedCooldown::new(
                account_id.clone(),
                revision,
                model_b.clone(),
                until_b,
            ))
            .await
            .expect("cache model B cooldown")
    );
    assert_eq!(
        repository
            .read_scoped(&account_id, &model_a)
            .await
            .expect("read model A cooldown")
            .expect("model A cooldown")
            .until(),
        until_a
    );
    assert_eq!(
        repository
            .read_scoped(&account_id, &model_b)
            .await
            .expect("read model B cooldown")
            .expect("model B cooldown")
            .until(),
        until_b
    );
    assert!(
        !repository
            .clear_scoped(
                &account_id,
                &model_a,
                CredentialRevision::new(1).expect("stale revision"),
            )
            .await
            .expect("fence stale model A invalidation")
    );
    assert!(
        repository
            .clear_scoped(&account_id, &model_a, revision)
            .await
            .expect("clear model A cooldown")
    );
    assert!(
        repository
            .read_scoped(&account_id, &model_a)
            .await
            .expect("read cleared model A cooldown")
            .is_none()
    );
    assert!(
        repository
            .read_scoped(&account_id, &model_b)
            .await
            .expect("read retained model B cooldown")
            .is_some()
    );
    let keys = namespace_keys(&mut connection, &namespace).await;
    assert_eq!(keys.len(), 1);
    assert!(!keys[0].contains(account_id.as_str()));
    assert!(!keys[0].contains(model_b.value()));
}

fn cooldown(account_id: &str, revision: u64, seconds: i64) -> CredentialCooldown {
    CredentialCooldown {
        provider_account_id: account_id.to_owned(),
        credential_revision: Revision::new(revision).expect("positive revision"),
        cooldown_until: millisecond_precision(Utc::now() + Duration::seconds(seconds)),
        kind: ProviderCooldownKind::RateLimit,
    }
}

#[tokio::test]
async fn cooldown_batch_preserves_order_partial_errors_and_expired_entries() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let mut accounts = (0..131)
        .map(|index| ProviderAccountId::new(format!("acct_batch_{index}")).unwrap())
        .collect::<Vec<_>>();
    accounts.push(accounts[130].clone());
    repository
        .cache_credential_cooldown(&cooldown(accounts[1].as_str(), 1, 60))
        .await
        .unwrap();
    let bad_type_key = namespace_keys(&mut connection, &namespace)
        .await
        .into_iter()
        .find(|key| key.ends_with(":cooldown"))
        .unwrap();
    redis::cmd("SET")
        .arg(&bad_type_key)
        .arg("invalid")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    repository
        .cache_credential_cooldown(&cooldown(accounts[2].as_str(), 2, 60))
        .await
        .unwrap();
    let bad_value_key = namespace_keys(&mut connection, &namespace)
        .await
        .into_iter()
        .find(|key| key.ends_with(":cooldown") && key != &bad_type_key)
        .unwrap();
    redis::cmd("HSET")
        .arg(bad_value_key)
        .arg("revision")
        .arg("invalid")
        .query_async::<i64>(&mut connection)
        .await
        .unwrap();
    repository
        .cache_credential_cooldown(&CredentialCooldown {
            kind: ProviderCooldownKind::RateLimit,
            ..cooldown(accounts[3].as_str(), 3, -60)
        })
        .await
        .unwrap();
    repository
        .cache_credential_cooldown(&cooldown(accounts[130].as_str(), 4, 60))
        .await
        .unwrap();

    assert!(repository.read_many(&[]).await.is_empty());
    let results = repository.read_many(&accounts).await;
    assert_eq!(results.len(), accounts.len());
    assert!(results[3].as_ref().unwrap().is_none());
    assert!(results[130].as_ref().unwrap().is_some());
    for (index, (account, result)) in accounts.iter().zip(results).enumerate() {
        if matches!(index, 1 | 2) {
            assert!(result.is_err(), "{index}");
        } else {
            assert_eq!(
                result.unwrap(),
                repository.read(account).await.unwrap(),
                "{index}"
            );
        }
    }
}

#[tokio::test]
async fn scoped_cooldown_batch_preserves_model_revision_and_partial_errors() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let mut accounts = (0..130)
        .map(|index| ProviderAccountId::new(format!("acct_scoped_batch_{index}")).unwrap())
        .collect::<Vec<_>>();
    accounts.push(accounts[129].clone());
    let scope = ProviderCooldownScope::upstream_model(
        gateway_core::routing::UpstreamModelId::new("gpt-test").unwrap(),
    );
    let other_scope = ProviderCooldownScope::upstream_model(
        gateway_core::routing::UpstreamModelId::new("gpt-other").unwrap(),
    );
    let revision = CredentialRevision::new(3).unwrap();
    let until = millisecond_precision(Utc::now() + Duration::seconds(60)).into();
    repository
        .put_scoped_if_later(ProviderScopedCooldown::new(
            accounts[1].clone(),
            revision,
            scope.clone(),
            until,
        ))
        .await
        .unwrap();
    let bad_key = namespace_keys(&mut connection, &namespace)
        .await
        .pop()
        .unwrap();
    redis::cmd("SET")
        .arg(bad_key)
        .arg("invalid")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    for (account, model) in [(&accounts[2], &other_scope), (&accounts[129], &scope)] {
        repository
            .put_scoped_if_later(ProviderScopedCooldown::new(
                account.clone(),
                revision,
                model.clone(),
                until,
            ))
            .await
            .unwrap();
    }

    assert!(repository.read_scoped_many(&[], &scope).await.is_empty());
    let results = repository.read_scoped_many(&accounts, &scope).await;
    assert_eq!(results.len(), accounts.len());
    assert!(results[2].as_ref().unwrap().is_none());
    assert_eq!(
        results[129]
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .credential_revision(),
        revision
    );
    for (index, (account, result)) in accounts.iter().zip(results).enumerate() {
        if index == 1 {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.unwrap(),
                repository.read_scoped(account, &scope).await.unwrap(),
                "{index}"
            );
        }
    }
}

fn millisecond_precision(value: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(value.timestamp_millis()).expect("valid timestamp")
}

async fn repository() -> Option<(RedisCredentialCooldownRepository, ConnectionManager, String)> {
    let redis_url = crate::support::test_env("CPR_TEST_REDIS_URL")?;
    let client = redis::Client::open(redis_url).expect("valid CPR_TEST_REDIS_URL");
    let connection = client
        .get_connection_manager()
        .await
        .expect("connect test Redis");
    let namespace = format!("gateway-store-cooldown-test-{}", Uuid::new_v4());
    let repository = RedisCredentialCooldownRepository::new(connection.clone(), &namespace)
        .expect("valid cooldown namespace");
    Some((repository, connection, namespace))
}

async fn namespace_keys(connection: &mut ConnectionManager, namespace: &str) -> Vec<String> {
    redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(connection)
        .await
        .expect("list isolated cooldown keys")
}
