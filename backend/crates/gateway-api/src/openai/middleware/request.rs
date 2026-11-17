//! OpenAI 只读查询的身份快照与响应交付适配

use super::http::{ExpectedBody, buffered_response, error_response, into_http_response};
pub(crate) use crate::middleware::request::{RequestInput, invoke_request, request_parts};
use crate::{middleware::headers::encode_headers, openai::error::gateway_error_response};
use axum::{http::HeaderMap, response::Response};
use bytes::Bytes;
use gateway_core::engine::{
    execution::{AuthenticatedClient, ExecutionService},
    middleware::MiddlewareError,
};
use std::{future::Future, sync::Arc};

impl RequestInput {
    pub(crate) fn query(endpoint: String, headers: &HeaderMap) -> Self {
        Self {
            endpoint,
            protocol: "openai".to_owned(),

            headers: encode_headers(headers),
            body: Bytes::new(),
        }
    }
}

/// 已鉴权只读查询共用入口；不重新鉴权、计量或创建 Provider attempt
pub(crate) async fn query_response<F, Fut>(
    execution: Arc<dyn ExecutionService>,
    client: AuthenticatedClient,
    input: RequestInput,
    terminal: F,
) -> Response
where
    F: FnOnce(AuthenticatedClient) -> Fut + Send + 'static,
    Fut: Future<Output = Response> + Send + 'static,
{
    let prepared = match execution.prepare_verified_execution(client) {
        Ok(prepared) => prepared,
        Err(error) => return gateway_error_response(&error),
    };
    let protocol = input.protocol.clone();
    let result = invoke_request(
        prepared,
        input,
        Box::new(move |prepared, request| {
            Box::pin(async move {
                if request.protocol() != protocol || !request.body().is_empty() {
                    return Err(MiddlewareError::Rejected);
                }
                buffered_response(&protocol, terminal(prepared.client().clone()).await).await
            })
        }),
    )
    .await;
    match result {
        Ok(response) => into_http_response(response, ExpectedBody::SingleJson).await,
        Err(error) => error_response(error),
    }
}
