//! Claude 动态订阅窗口与响应头观测，不以 API 等价费用推算订阅余额

use crate::{
    API, OAUTH_BETA,
    credential::{CredentialError, Credentials},
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use gateway_admin::model::provider_credentials::*;
use gateway_core::{
    account::{OpaqueProviderData, QuotaEvidence, QuotaObservation, QuotaState, QuotaWriteOutcome},
    provider_ports::{ProviderCatalogCacheKey, ProviderCatalogScope},
};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime};

fn cache_key(
    credentials: &Credentials,
    account: &gateway_core::account::ProviderAccount,
    scope: &str,
) -> Result<ProviderCatalogCacheKey, CredentialError> {
    Ok(ProviderCatalogCacheKey::new(
        credentials.kind(),
        ProviderCatalogScope::new(format!("{scope}:{}", account.id()))
            .map_err(|_| CredentialError::Invalid)?,
    ))
}
fn document(value: Value) -> ProviderDocument {
    ProviderDocument::new(OpaqueProviderData::new(
        value.as_object().expect("quota object").clone(),
    ))
}

pub(crate) async fn read(
    credentials: &Credentials,
    request: ProviderQuotaRequest,
) -> Result<ProviderQuota, CredentialError> {
    credentials
        .veil
        .control(gateway_veil::ControlPurpose::Quota)
        .map_err(|_| CredentialError::Unavailable)?;
    let (loaded, tokens) = credentials.load(&request.account_id).await?;
    let mut plan_type = loaded.account.plan_type().map(str::to_owned);
    if request.refresh
        && (loaded.account.credential_state() != gateway_core::account::CredentialState::Ready
            || tokens
                .expiry()
                .is_some_and(|expiry| expiry <= SystemTime::now()))
    {
        return Err(CredentialError::Invalid);
    }
    let accounts = credentials.ports.accounts();
    let mut value = accounts
        .get_quotas(std::slice::from_ref(loaded.account.id()))
        .await
        .map_err(|_| CredentialError::Unavailable)?
        .into_iter()
        .next()
        .map(|observation| Value::Object(observation.quota.into_inner()))
        .unwrap_or_else(|| json!({"windows":{}}));
    let cache = credentials.ports.catalog_cache();
    if request.refresh {
        let attempted_at = Utc::now();
        let result = async {
            let request = credentials
                .transport
                .client(loaded.account.outbound_proxy())?
                .get(format!("{API}/api/oauth/usage"))
                .headers(credentials.control_headers())
                .bearer_auth(&tokens.access_token)
                .header("anthropic-beta", OAUTH_BETA)
                .timeout(Duration::from_secs(5));
            let response = credentials
                .transport
                .send(
                    request,
                    crate::transport::Purpose::Control(gateway_veil::ControlPurpose::Quota),
                )
                .await
                .map_err(|_| CredentialError::Unavailable)?;
            let raw = crate::credential::read_json(response).await?;
            if !raw.is_object() {
                return Err(CredentialError::Unavailable);
            }
            Ok(raw)
        }
        .await;
        match result {
            Ok(raw) => {
                let mut windows = serde_json::Map::new();
                // 只接受上游真正报告的窗口，不把固定四项误作完整订阅合同
                for (name, window) in raw.as_object().expect("validated quota object") {
                    if name.len() > 128 {
                        continue;
                    }
                    if let Some(percent) = window["utilization"]
                        .as_f64()
                        .filter(|n| n.is_finite() && *n >= 0.0)
                    {
                        windows.insert(name.clone(),json!({"utilization":percent,"resets_at":window["resets_at"].as_str(),"source":"oauth_usage","observedAt":attempted_at}));
                    }
                }
                value["windows"] = Value::Object(windows);
                value["extraUsageEnabled"] = json!(raw["extra_usage"]["is_enabled"].as_bool());
                value["observedAt"] = json!(attempted_at);
                value["queryFailed"] = json!(false);
            }
            Err(_) => {
                value["queryFailed"] = json!(true);
                value["queryFailedAt"] = json!(attempted_at);
            }
        }
        // 旧账号缺失套餐时随额度刷新补全；只接受同一身份，资料查询最多额外等待 5 秒。
        let profile = if plan_type.is_none() && value["queryFailed"] == false {
            Some(
                tokio::time::timeout(
                    Duration::from_secs(5),
                    credentials.profile(&tokens, loaded.account.outbound_proxy()),
                )
                .await
                .unwrap_or(Err(CredentialError::Unavailable)),
            )
        } else {
            None
        };
        if let Some(Ok(profile)) = &profile {
            if profile.id() != *loaded.account.id() {
                return Err(CredentialError::Invalid);
            }
            plan_type.clone_from(&profile.plan);
        }
        // 原始观测与额度访问状态共用既有存储；查询失败不解除旧耗尽，迟到结果受 revision 与时间双重围栏保护
        let state = if value["queryFailed"] == true {
            loaded.account.quota()
        } else {
            access_state(&value, loaded.account.quota(), attempted_at.into())
        };
        if accounts
            .compare_and_swap_quota(QuotaObservation {
                quota_pause: None,
                expected_connection: None,
                account_id: loaded.account.id().clone(),
                expected_revision: loaded.account.revision(),
                quota: OpaqueProviderData::new(value.as_object().expect("quota object").clone()),
                plan_type: plan_type.clone(),
                observed_at: attempted_at.into(),
                state,
            })
            .await
            .map_err(|_| CredentialError::Unavailable)?
            == QuotaWriteOutcome::Conflict
        {
            return Err(CredentialError::Busy);
        }
        // 资料失败仍保存已取得的额度，但向调用方报告失败，后台下轮继续补全套餐。
        if let Some(result) = profile {
            result?;
        }
    }
    let headers_key = cache_key(credentials, &loaded.account, "quota_headers")?;
    let header_data = cache
        .read(&headers_key)
        .await
        .map_err(|_| CredentialError::Unavailable)?;
    let observed_at = timestamp(&value["observedAt"]);
    let stale =
        observed_at.is_some_and(|time| Utc::now().signed_duration_since(time).num_seconds() > 300);
    let mut windows = Vec::new();
    if let Some(items) = value["windows"].as_object() {
        for (name, item) in items {
            let Some(percent) = item["utilization"].as_f64() else {
                continue;
            };
            let seconds = if name == "five_hour" {
                Some(18_000)
            } else if name.starts_with("seven_day") {
                Some(604_800)
            } else {
                None
            };
            let role = match name.as_str() {
                "five_hour" => Some(ProviderQuotaWindowRole::Primary),
                "seven_day" => Some(ProviderQuotaWindowRole::Secondary),
                _ => None,
            };
            let scope = if matches!(name.as_str(), "five_hour" | "seven_day") {
                "account"
            } else {
                name
            };
            windows.push(ProviderQuotaWindow {
                upstream: None,
                key: name.clone(),
                group: "claude_subscription".into(),
                label: name.replace('_', " "),
                limit_id: Some(scope.into()),
                limit_name: Some(scope.into()),
                role,
                local_usage_attribution: QuotaLocalUsageAttribution::Unavailable,
                window_seconds: seconds,
                used_percent: Some(percent),
                reset_at: timestamp(&item["resets_at"]),
                limit_reached: percent >= 100.0,
                local_usage: None,
                provider_data: Some(document(
                    json!({"source":item["source"],"observedAt":item["observedAt"],"scope":scope}),
                )),
            });
        }
    }
    let reached = windows
        .iter()
        .any(|window| window.limit_reached && window.limit_id.as_deref() == Some("account"));
    let state = if value["queryFailed"] == true {
        "query_failed"
    } else if observed_at.is_none() || windows.is_empty() {
        "unsampled"
    } else if stale {
        "stale"
    } else if reached {
        "exhausted"
    } else {
        "sampled"
    };
    Ok(ProviderQuota {
        paused_windows: Vec::new(),
        upstream: None,
        plan_type,
        observed_at,
        refresh_token_expires_at: None,
        limit_reached: reached,
        windows,
        credits: None,
        provider_data: Some(document(
            json!({"schemaVersion":1,"state":state,"source":"oauth_usage","scope":"account","responseHeaders":header_data.map(|data|Value::Object(data.into_inner())),"queryFailedAt":value["queryFailedAt"]}),
        )),
    })
}
fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    value
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}

pub(crate) fn scheduling_signals(
    quota: &OpaqueProviderData,
    now: SystemTime,
) -> Option<gateway_core::account::AccountQuotaSignals> {
    use gateway_core::account::{AccountQuotaResetWindow, AccountQuotaSignals, QuotaResetPeriod};
    let fields = quota.expose_to_provider();
    let observed: SystemTime = timestamp(fields.get("observedAt")?)?.into();
    if now.duration_since(observed).ok()? > Duration::from_secs(300) {
        return None;
    }
    let windows = fields.get("windows")?.as_object()?;
    let mut resets = Vec::new();
    let mut remaining = None;
    // 只投影账号通用的真实窗口；模型专属周额度与未报告的月额度不能影响全部模型的选号。
    for (name, period) in [
        ("five_hour", QuotaResetPeriod::FiveHour),
        ("seven_day", QuotaResetPeriod::Weekly),
    ] {
        let Some(window) = windows.get(name) else {
            continue;
        };
        let Some(percent) = window["utilization"]
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
        else {
            continue;
        };
        let Some(reset_at) = timestamp(&window["resets_at"])
            .map(SystemTime::from)
            .filter(|reset| *reset > now)
        else {
            continue;
        };
        let rank = ((100.0 - percent.clamp(0.0, 100.0)) * 100.0).round() as u64;
        remaining = Some(remaining.map_or(rank, |current: u64| current.min(rank)));
        resets.push(AccountQuotaResetWindow { period, reset_at });
    }
    let reset_at = resets.iter().map(|window| window.reset_at).min();
    remaining.map(|remaining| {
        AccountQuotaSignals::new(reset_at, Some(remaining)).with_reset_windows(resets)
    })
}

pub(crate) struct Rejection {
    pub until: SystemTime,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorScope {
    Account,
    Model,
    Request,
}
impl ErrorScope {
    pub fn code(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Model => "model",
            Self::Request => "request",
        }
    }
}

pub(crate) fn error_scope(
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> ErrorScope {
    if status != 429 {
        return if status == 401 || status >= 500 {
            ErrorScope::Account
        } else {
            ErrorScope::Request
        };
    }
    if !model_only(headers) {
        return ErrorScope::Account;
    }
    let message = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned())
        .to_ascii_lowercase();
    if message.contains("fast request rejected")
        || (message.contains("fast")
            && (message.contains("usage credits") || message.contains("credits are required")))
    {
        return ErrorScope::Request;
    }
    ErrorScope::Model
}

fn field<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?.trim();
    if values.next().is_some() {
        None
    } else {
        Some(value)
    }
}

fn model_only(headers: &reqwest::header::HeaderMap) -> bool {
    let field = |name: &str| field(headers, name);
    let status = |window: &str| field(&format!("anthropic-ratelimit-unified{window}-status"));
    let rejected =
        |window: &str| status(window).is_some_and(|s| s.eq_ignore_ascii_case("rejected"));
    let healthy = |window: &str| match status(window) {
        Some(s) => s.eq_ignore_ascii_case("allowed") || s.eq_ignore_ascii_case("allowed_warning"),
        None => {
            !headers.contains_key(format!("anthropic-ratelimit-unified{window}-status"))
                && field(&format!("anthropic-ratelimit-unified{window}-utilization"))
                    .and_then(|s| s.parse::<f64>().ok())
                    .is_some_and(|n| (0.0..1.0).contains(&n))
        }
    };
    let shared_rejected = rejected("-5h") || rejected("-7d");
    let overage_claim = field("anthropic-ratelimit-unified-representative-claim")
        .is_some_and(|s| s.to_ascii_lowercase().contains("overage"));
    let overage_rejected = rejected("-7d_oi")
        || rejected("-overage")
        || overage_claim
        || field("anthropic-ratelimit-unified-overage-disabled-reason")
            .is_some_and(|s| !s.is_empty());
    // CPA 固定源码中的窄例外：明确拒绝 overage claim 时可能完全不评估共享窗口。
    // 仅此情形接受缺失利用率；歧义头、无效值或共享窗口耗尽都不能获得模型级豁免。
    let shared_unevaluated = rejected("")
        && overage_claim
        && ["-5h", "-7d"].into_iter().all(|window| {
            let status_name = format!("anthropic-ratelimit-unified{window}-status");
            let utilization_name = format!("anthropic-ratelimit-unified{window}-utilization");
            !headers.contains_key(status_name)
                && (!headers.contains_key(utilization_name.as_str()) || healthy(window))
        });
    let overage_model_only = !shared_rejected
        && overage_rejected
        && ((healthy("-5h") && healthy("-7d")) || shared_unevaluated);
    // Ordinary 429 belongs to the requested model; only explicit unified rejection cools the account.
    !shared_rejected && (!rejected("") || overage_model_only)
}

/// 仅用于明确的 429。reset 只有所属窗口拒绝时才生效，额外用量账期不能变成账号冷却。
pub(crate) fn rejection(headers: &reqwest::header::HeaderMap) -> Option<Rejection> {
    let field = |name: &str| field(headers, name);
    let rejected = |window: &str| {
        field(&format!("anthropic-ratelimit-unified{window}-status"))
            .is_some_and(|s| s.eq_ignore_ascii_case("rejected"))
    };
    let model_only = model_only(headers);
    let overage_claim = field("anthropic-ratelimit-unified-representative-claim")
        .is_some_and(|s| s.to_ascii_lowercase().contains("overage"));
    let now = SystemTime::now();
    let parse_date = |value: &str| {
        DateTime::parse_from_rfc3339(value)
            .or_else(|_| DateTime::parse_from_rfc2822(value))
            .ok()
            .map(SystemTime::from)
    };
    let parse_reset = |value: &str| {
        value
            .parse::<f64>()
            .ok()
            .and_then(|s| Duration::try_from_secs_f64(s).ok())
            .and_then(|d| SystemTime::UNIX_EPOCH.checked_add(d))
            .or_else(|| parse_date(value))
    };
    let mut deadlines = Vec::new();
    if let Some(value) = field("retry-after") {
        let until = value
            .parse::<f64>()
            .ok()
            .and_then(|s| Duration::try_from_secs_f64(s).ok())
            .and_then(|d| now.checked_add(d))
            .or_else(|| parse_date(value));
        deadlines.extend(until);
    }
    for window in ["-5h", "-7d", "-7d_oi"] {
        if rejected(window) && (window != "-7d_oi" || model_only) {
            deadlines.extend(
                field(&format!("anthropic-ratelimit-unified{window}-reset")).and_then(parse_reset),
            );
        }
    }
    if rejected("") && !model_only {
        let reset = field("anthropic-ratelimit-unified-reset").and_then(parse_reset);
        let overage_reset =
            field("anthropic-ratelimit-unified-overage-reset").and_then(parse_reset);
        if !overage_claim || reset != overage_reset {
            deadlines.extend(reset);
        }
    }
    let until = deadlines
        .into_iter()
        .filter(|until| {
            until
                .duration_since(now)
                .is_ok_and(|d| !d.is_zero() && d <= Duration::from_secs(7 * 86400 + 3600))
        })
        .max()?;
    Some(Rejection { until })
}

pub(crate) async fn observe_headers(
    credentials: &Credentials,
    account: &gateway_core::account::ProviderAccount,
    headers: &reqwest::header::HeaderMap,
) -> Result<(), CredentialError> {
    let mut facts = serde_json::Map::new();
    for (name, raw) in headers {
        if name.as_str().starts_with("anthropic-ratelimit-")
            && facts.len() < 32
            && let Ok(text) = raw.to_str()
            && text.len() <= 256
        {
            facts.insert(name.to_string(), json!(text));
        }
    }
    if facts.is_empty() {
        return Ok(());
    }
    let key = cache_key(credentials, account, "quota_headers")?;
    // 独立缓存避免并发响应头覆盖主动查询的订阅快照
    let value = json!({"source":"response_headers","observedAt":Utc::now(),"scope":"upstream_reported","facts":facts});
    credentials
        .ports
        .catalog_cache()
        .replace(
            &key,
            &OpaqueProviderData::new(value.as_object().expect("quota object").clone()),
            Duration::from_secs(90 * 86400),
        )
        .await
        .map_err(|_| CredentialError::Unavailable)
}

// 仅账号级窗口能够限制整个账号；额外付费用量可用性未知时不把订阅触顶扩大成账号耗尽
fn access_state(value: &Value, previous: QuotaState, observed_at: SystemTime) -> QuotaState {
    let windows: Vec<_> = ["five_hour", "seven_day"]
        .into_iter()
        .filter_map(|name| {
            let window = &value["windows"][name];
            window["utilization"]
                .as_f64()
                .map(|percent| (percent, timestamp(&window["resets_at"])))
        })
        .collect();
    let exhausted: Vec<_> = windows
        .iter()
        .filter(|(percent, _)| *percent >= 100.0)
        .collect();
    if !exhausted.is_empty() && value["extraUsageEnabled"] == false {
        let reset = if exhausted.iter().all(|(_, reset)| reset.is_some()) {
            exhausted
                .iter()
                .filter_map(|(_, reset)| *reset)
                .max()
                .map(Into::into)
        } else {
            None
        };
        QuotaState::exhausted(QuotaEvidence::AccountLimitReached, observed_at, reset)
    } else if windows.len() == 2 && exhausted.is_empty() {
        QuotaState::allowed(observed_at)
    } else {
        previous
    }
}

/// 后台按最后一次查询时间复核；到达 reset 只触发求证，不直接解除耗尽
pub(crate) async fn synchronize(credentials: &Credentials) -> Result<(), CredentialError> {
    let accounts = credentials.ports.accounts();
    let candidates = accounts
        .list_for_provider(&credentials.kind())
        .await
        .map_err(|_| CredentialError::Unavailable)?;
    let ids: Vec<_> = candidates
        .iter()
        .map(|account| account.id().clone())
        .collect();
    let observations = accounts
        .get_quotas(&ids)
        .await
        .map_err(|_| CredentialError::Unavailable)?;
    let now = SystemTime::now();
    let requests = candidates
        .into_iter()
        .filter(|account| {
            account.enabled()
                && account.credential_state() == gateway_core::account::CredentialState::Ready
                && !(account.quota().is_exhausted()
                    && account.quota().reset_at().is_some_and(|reset| reset > now))
                && !observations
                    .iter()
                    .find(|observation| observation.account_id == *account.id())
                    .is_some_and(|observation| {
                        now.duration_since(observation.observed_at)
                            .unwrap_or_default()
                            < Duration::from_secs(300)
                    })
        })
        .map(|account| async move {
            read(
                credentials,
                ProviderQuotaRequest {
                    account_id: account.id().clone(),
                    refresh: true,
                    rolling_usage: None,
                },
            )
            .await
        });
    // 与目录相同的有界并发，慢账号不阻塞整个订阅池的观测。
    let results = futures::stream::iter(requests)
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
    let failed = results.iter().any(Result::is_err);
    if failed {
        Err(CredentialError::Unavailable)
    } else {
        Ok(())
    }
}
