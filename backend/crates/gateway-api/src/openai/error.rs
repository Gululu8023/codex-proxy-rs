//! OpenAI 客户端协议的稳定错误响应

use axum::{
    Json,
    body::Body,
    http::{HeaderName, HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use gateway_core::{
    engine::EngineError,
    error::ProviderErrorKind,
    policy::{ClientVersionRejection, CodexClientKind},
};
use serde_json::{Value, json};

pub use gateway_core::engine::execution::gateway_error_from_engine;

use crate::client_error::{
    ProtocolErrorBody, capacity_error_for_client, is_retry_after_header, remove_retry_after_headers,
};

pub use crate::client_error::{
    gateway_error_contract, gateway_error_response, openai_error_response,
};

/// 严格协议 decoder/encoder 返回的安全错误 body
pub fn protocol_error_response(status: StatusCode, body: ProtocolErrorBody) -> Response {
    let mut response = (status, Json(body.clone().into_value())).into_response();
    response.extensions_mut().insert(body);
    response
}

/// 下游 Client API Key 无效
pub fn missing_client_api_key_response() -> (StatusCode, Json<Value>) {
    openai_error_response(
        StatusCode::UNAUTHORIZED,
        "Missing or invalid API key.",
        "invalid_request_error",
        "invalid_api_key",
    )
}

/// RuntimeSnapshot 当前不允许接收新的配置依赖请求
pub fn runtime_unavailable_response() -> (StatusCode, Json<Value>) {
    openai_error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "Gateway configuration is temporarily unavailable.",
        "server_error",
        "runtime_configuration_unavailable",
    )
}

/// 已识别 Codex 客户端不满足最低版本要求
pub fn client_version_rejection_response(
    rejection: &ClientVersionRejection,
) -> (StatusCode, Json<Value>) {
    let client_label = match rejection.kind() {
        CodexClientKind::Desktop => "Codex Desktop",
        CodexClientKind::Cli => "Codex CLI",
    };
    let (message, code, current_version) = match rejection.current() {
        Some(current) => (
            format!(
                "{client_label} {current} is outdated. Upgrade to {} or later.",
                rejection.min()
            ),
            "client_version_too_old",
            Some(current.to_string()),
        ),
        None => (
            format!(
                "Cannot read the {client_label} version. Check the User-Agent and version headers."
            ),
            "client_version_unavailable",
            None,
        ),
    };
    // Codex 将 400 视为不可重试的请求错误，426 则会触发 WS 回退和 HTTP 重试
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": code,
                "client": rejection.kind().as_str(),
                "current_version": current_version,
                "min_version": rejection.min().to_string()
            }
        })),
    )
}

/// 对外模型不存在
pub fn model_not_found_response() -> (StatusCode, Json<Value>) {
    openai_error_response(
        StatusCode::NOT_FOUND,
        "The requested model does not exist or is not available.",
        "invalid_request_error",
        "model_not_found",
    )
}

/// 容量恢复和完整历史重放不能继承旧账号的等待窗口，普通限流仍保留原提示
pub(crate) fn suppress_recovery_retry_after(error: &EngineError) -> bool {
    matches!(error, EngineError::Provider(provider)
        if provider.kind() == ProviderErrorKind::UpstreamCapacityUnavailable
            || provider.client_visible_upstream_error().and_then(|detail| detail.code())
                == Some("previous_response_not_found"))
}

/// HTTP 头已经提交时，把当前失败的关联 ID 与等待提示放入 SSE 错误对象
pub(crate) fn attach_sse_error_headers(data: &mut Value, response_headers: &axum::http::HeaderMap) {
    let Some(error) = data
        .pointer_mut("/response/error")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for name in [
        "x-request-id",
        "x-oai-request-id",
        "retry-after",
        "retry-after-ms",
    ] {
        let Some(value) = response_headers
            .get(name)
            .and_then(|value| value.to_str().ok())
        else {
            continue;
        };
        if let Some(headers) = error
            .entry("headers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
        {
            // 已投影的错误头优先，不能被先前 opening 的快照覆盖
            headers
                .entry(name)
                .or_insert_with(|| Value::String(value.to_owned()));
        }
    }
}

/// 在下游尚未提交时交付 Provider 的 HTTP 失败响应，并应用客户端容量恢复合同
pub fn engine_error_response(error: &EngineError) -> Response {
    let capacity_unavailable = matches!(error, EngineError::Provider(error)
        if error.kind() == ProviderErrorKind::UpstreamCapacityUnavailable
            && error.client_visible_upstream_error().and_then(|detail| detail.code())
                != Some("previous_response_not_found"));
    let suppress_retry_after = suppress_recovery_retry_after(error);
    if let EngineError::Provider(error) = error
        && let Some(upstream) = error.client_visible_upstream_response()
        && let Ok(status) = StatusCode::from_u16(upstream.status())
    {
        let projected = serde_json::from_slice::<Value>(upstream.body())
            .ok()
            .and_then(|mut body| match capacity_error_for_client(&body) {
                Some(body) => Some((StatusCode::SERVICE_UNAVAILABLE, body)),
                None if suppress_retry_after => {
                    remove_retry_after_headers(&mut body);
                    Some((status, body))
                }
                None => None,
            });
        let suppress_retry_after = suppress_retry_after || projected.is_some();
        let mut response = match projected {
            Some((status, body)) => {
                let mut response = Response::new(Body::from(body.to_string()));
                *response.status_mut() = if capacity_unavailable {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    status
                };
                response
            }
            None => {
                let mut response = Response::new(Body::from(upstream.body().clone()));
                *response.status_mut() = if capacity_unavailable {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    status
                };
                response
            }
        };
        if let Some(content_type) = upstream.content_type()
            && let Ok(content_type) = HeaderValue::from_bytes(content_type)
        {
            response.headers_mut().insert(CONTENT_TYPE, content_type);
        }
        for header in upstream.headers() {
            if suppress_retry_after && is_retry_after_header(header.name()) {
                continue;
            }
            let Ok(name) = HeaderName::from_bytes(header.name().as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_bytes(header.value()) else {
                continue;
            };
            response.headers_mut().append(name, value);
        }
        if !response.headers().contains_key("x-request-id")
            && !response.headers().contains_key("x-oai-request-id")
            && let Some(request_id) = error.upstream_request_id()
            && let Ok(value) = HeaderValue::from_str(request_id.as_str())
        {
            response.headers_mut().insert("x-request-id", value);
        }
        return response;
    }
    let mut response = gateway_error_response(&gateway_error_from_engine(error));
    if capacity_unavailable {
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    }
    // 流内失败可能没有 HTTP 错误响应；普通限流仍保留 Provider 已确认的等待建议
    if !suppress_retry_after
        && let EngineError::Provider(provider) = error
        && let Some(delay) = provider.retry_after()
        && let Ok(value) = HeaderValue::from_str(
            &delay
                .as_secs()
                .saturating_add(u64::from(delay.subsec_nanos() > 0))
                .to_string(),
        )
    {
        response.headers_mut().insert("retry-after", value);
    }
    // 正文未完整取得时不能透传残缺响应，但已确认的上游关联 ID 仍应交付
    if let EngineError::Provider(error) = error
        && let Some(request_id) = error.upstream_request_id()
        && let Ok(value) = HeaderValue::from_str(request_id.as_str())
    {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}
