//! OpenAI HTTP 交付与本协议错误格式

pub(crate) use crate::middleware::delivery::{
    ExpectedBody, SSE_KEEPALIVE_FRAME, SSE_KEEPALIVE_INTERVAL, buffered_response,
};
use crate::openai::error::{
    gateway_error_from_engine, gateway_error_response, openai_error_response,
};
use axum::{
    http::StatusCode,
    response::{IntoResponse as _, Response},
};
use gateway_core::{
    engine::middleware::{MiddlewareError, MiddlewareResponse},
    error::GatewayError,
};

pub(crate) async fn into_http_response(
    response: MiddlewareResponse,
    expected: ExpectedBody,
) -> Response {
    crate::middleware::delivery::into_http_response(response, expected, "openai", error_response)
        .await
}

pub(crate) fn error_response(error: MiddlewareError) -> Response {
    match error {
        MiddlewareError::Gateway(error) => gateway_error_response(&error),
        MiddlewareError::Engine(error) => {
            gateway_error_response(&gateway_error_from_engine(&error))
        }
        MiddlewareError::Provider(error) => {
            gateway_error_response(&GatewayError::from_provider(&error))
        }
        MiddlewareError::Rejected => openai_error_response(
            StatusCode::FORBIDDEN,
            "Request rejected by middleware.",
            "invalid_request_error",
            "middleware_rejected",
        )
        .into_response(),
        MiddlewareError::Fault | MiddlewareError::InvalidState => openai_error_response(
            StatusCode::BAD_GATEWAY,
            "Request middleware failed.",
            "server_error",
            "middleware_failed",
        )
        .into_response(),
    }
}
