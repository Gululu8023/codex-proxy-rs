//! OAuth 重置权益库存。仅实现已有脱敏响应证据的读取，不推测消费端点或幂等合同。

use chrono::{DateTime, Utc};
use gateway_admin::{
    model::provider_credentials::{ProviderResetCredit, ProviderResetCredits},
    ports::provider::{ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::account::{CredentialState, ProviderAccountId};
use serde_json::Value;
use std::time::{Duration, SystemTime};

use crate::{API, OAUTH_BETA, credential::Credentials};

fn unavailable() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Unavailable)
}

pub(crate) async fn read(
    credentials: &Credentials,
    account_id: &ProviderAccountId,
) -> Result<ProviderResetCredits, ProviderAdminError> {
    credentials
        .veil
        .control(gateway_veil::ControlPurpose::Quota)
        .map_err(|_| unavailable())?;
    let (loaded, tokens) = credentials
        .load(account_id)
        .await
        .map_err(super::admin::admin_error)?;
    if loaded.account.credential_state() != CredentialState::Ready {
        return Err(ProviderAdminError::new(ProviderAdminErrorKind::Invalid));
    }
    if tokens
        .expiry()
        .is_some_and(|expiry| expiry <= SystemTime::now())
    {
        return Err(ProviderAdminError::new(
            ProviderAdminErrorKind::CredentialRefreshRequired,
        ));
    }
    // ?cedar_ember=1 保留普通 usage 的 spend 数据；不添加会清空 extra_usage 的 skip_spend。
    // 使用现有控制面客户端身份，不通过替换 UA 或更换凭据来伪造权益资格。
    let request = credentials
        .transport
        .client(loaded.account.outbound_proxy())
        .map_err(super::admin::admin_error)?
        .get(format!("{API}/api/oauth/usage?cedar_ember=1"))
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
        .map_err(|_| unavailable())?;
    if response.status().as_u16() == 401 {
        return Err(ProviderAdminError::new(
            ProviderAdminErrorKind::CredentialRefreshRequired,
        ));
    }
    let raw = crate::credential::read_json(response)
        .await
        .map_err(super::admin::admin_error)?;
    let credits = parse_inventory(&raw, Utc::now())?;
    // 查询期间的重新授权或换绑不能把旧身份的库存返回给新凭据。
    let (current, _) = credentials
        .load(account_id)
        .await
        .map_err(super::admin::admin_error)?;
    if current.account.revision() != loaded.account.revision() {
        return Err(ProviderAdminError::new(ProviderAdminErrorKind::Conflict));
    }
    Ok(credits)
}

fn date(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

fn parse_inventory(
    raw: &Value,
    now: DateTime<Utc>,
) -> Result<ProviderResetCredits, ProviderAdminError> {
    let inventory = &raw["cedar_ember"];
    match inventory["eligible"].as_bool() {
        Some(true) => {}
        Some(false) => {
            return Err(unavailable().with_public_message("上游未开放该账号或客户端的重置权益查询"));
        }
        None => {
            return Err(
                unavailable().with_public_message("上游未返回重置权益库存，无法确认可用数量")
            );
        }
    }
    let grants = inventory["grants"].as_array().ok_or_else(unavailable)?;
    let mut available_count = 0_u64;
    let mut credits = Vec::with_capacity(grants.len());
    for grant in grants {
        let id = grant["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(unavailable)?;
        let remaining = grant["resets_left"].as_u64().ok_or_else(unavailable)?;
        let paused = grant["paused"].as_bool().ok_or_else(unavailable)?;
        let starts_at = date(&grant["starts_at"]).ok_or_else(unavailable)?;
        let expires_at = date(&grant["ends_at"]).ok_or_else(unavailable)?;
        let scopes = grant["clears"]
            .as_array()
            .ok_or_else(unavailable)?
            .iter()
            .map(|scope| scope.as_str().ok_or_else(unavailable))
            .collect::<Result<Vec<_>, _>>()?;
        let status = if expires_at <= now {
            "expired"
        } else if remaining == 0 {
            "depleted"
        } else if starts_at > now {
            "scheduled"
        } else if paused {
            "paused"
        } else {
            // 库存数量和此刻是否允许消费是不同概念；usable_now 不能抹掉仍持有的权益。
            available_count = available_count
                .checked_add(remaining)
                .ok_or_else(unavailable)?;
            "available"
        };
        credits.push(ProviderResetCredit {
            id: id.to_owned(),
            status: Some(status.to_owned()),
            title: grant["label"].as_str().map(str::to_owned),
            expires_at: Some(expires_at),
            reset_type: Some(scopes.join(", ")),
        });
    }
    Ok(ProviderResetCredits {
        available_count,
        credits,
    })
}

#[cfg(test)]
mod tests;
