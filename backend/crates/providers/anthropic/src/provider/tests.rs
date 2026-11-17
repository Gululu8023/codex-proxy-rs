//! 通用鉴权失败不能误停号，模型拒绝和隐私故障不能污染账号评分。

use super::*;
use gateway_core::upstream::UpstreamFailureFields;

#[test]
fn only_explicit_revocation_or_ban_changes_credential_state() {
    for (status, kind, code, message, expected) in [
        (
            401,
            "authentication_error",
            Some("token_revoked"),
            "revoked",
            Some(CredentialState::Expired),
        ),
        (
            401,
            "authentication_error",
            None,
            "Invalid bearer token. Please log in.",
            Some(CredentialState::Expired),
        ),
        (
            403,
            "permission_error",
            Some("account_disabled"),
            "disabled",
            Some(CredentialState::Banned),
        ),
        (
            403,
            "permission_error",
            None,
            "Your account has been banned.",
            Some(CredentialState::Banned),
        ),
        (
            401,
            "authentication_error",
            None,
            "OAuth token has expired",
            None,
        ),
        (
            401,
            "authentication_error",
            None,
            "Authentication failed",
            None,
        ),
        (
            403,
            "permission_error",
            None,
            "This model requires a different subscription",
            None,
        ),
        (
            403,
            "permission_error",
            None,
            "Model denied. Your account has been banned.",
            None,
        ),
        (
            429,
            "rate_limit_error",
            Some("account_disabled"),
            "limited",
            None,
        ),
    ] {
        let fields = UpstreamFailureFields {
            status: Some(status),
            code: code.map(str::to_owned),
            error_type: Some(kind.into()),
            message: Some(message.into()),
        };
        assert_eq!(
            rejected_credential_state(&fields),
            expected,
            "{status} {message}"
        );
    }
}

#[test]
fn local_and_model_failures_do_not_penalize_the_account() {
    for (kind, state, scope, expected) in [
        (
            ProviderErrorKind::InvalidRequest,
            UpstreamSendState::Sent,
            "model",
            false,
        ),
        (
            ProviderErrorKind::PermissionDenied,
            UpstreamSendState::Sent,
            "model",
            false,
        ),
        (
            ProviderErrorKind::RateLimited,
            UpstreamSendState::Sent,
            "account",
            true,
        ),
        (
            ProviderErrorKind::Transport,
            UpstreamSendState::NotSent,
            "account",
            false,
        ),
    ] {
        let error = ProviderError::new(kind, state).with_diagnostic(
            gateway_core::error::ProviderDiagnostic::new("fixture")
                .with_classification("anthropic_upstream", scope),
        );
        assert_eq!(score_account_failure(&error), expected);
    }
    assert!(!score_account_failure(&veil_error(
        gateway_veil::Error::NotConfigured,
        UpstreamSendState::NotSent
    )));
}
