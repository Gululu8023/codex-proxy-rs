//! 各协议共用的身份快照与流式响应取消边界。

use crate::middleware::headers::decode_headers;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures::future::BoxFuture;
use gateway_core::{
    engine::{
        execution::PreparedRootExecution,
        middleware::{
            MiddlewareBody, MiddlewareError, MiddlewareFrame, MiddlewareRequest,
            MiddlewareResponse, RequestHeader,
        },
    },
    lifecycle::CancellationToken,
};

pub(crate) struct RequestInput {
    pub endpoint: String,
    pub protocol: String,
    pub headers: Vec<RequestHeader>,
    pub body: Bytes,
}

pub(crate) type RequestTerminal = Box<
    dyn FnOnce(
            PreparedRootExecution,
            MiddlewareRequest,
        ) -> BoxFuture<'static, Result<MiddlewareResponse, MiddlewareError>>
        + Send,
>;

pub(crate) async fn invoke_request(
    mut prepared: PreparedRootExecution,
    input: RequestInput,
    terminal: RequestTerminal,
) -> Result<MiddlewareResponse, MiddlewareError> {
    let origin = crate::middleware::current();
    if let Some(origin) = &origin {
        prepared = prepared.with_request_scope(origin.extensions.clone());
    }
    let lifetime = RequestLifetime {
        cancellation: prepared.cancellation(),
    };
    let request = MiddlewareRequest::new(input.protocol, input.headers, input.body);
    let response = terminal(prepared, request).await?;
    let (protocol, status, headers, body) = response.into_parts();
    let response = MiddlewareResponse::new(
        protocol,
        status,
        headers,
        Box::new(RequestBody { body, lifetime }),
    );
    Ok(response)
}

struct RequestLifetime {
    cancellation: CancellationToken,
}

impl Drop for RequestLifetime {
    fn drop(&mut self) {
        // 包括终端仍在等待、客户端断开和提前关流；不启动第二份账本终结逻辑。
        self.cancellation.cancel();
    }
}

struct RequestBody {
    body: Box<dyn MiddlewareBody>,
    lifetime: RequestLifetime,
}

impl MiddlewareBody for RequestBody {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<MiddlewareFrame>, MiddlewareError>> {
        self.body.next_frame()
    }

    fn commit_downstream(
        &mut self,
        status: Option<u16>,
    ) -> BoxFuture<'_, Result<(), MiddlewareError>> {
        self.body.commit_downstream(status)
    }

    fn record_client_status(&mut self, status: u16) -> BoxFuture<'_, Result<(), MiddlewareError>> {
        self.body.record_client_status(status)
    }

    fn is_finalized(&self) -> bool {
        self.body.is_finalized()
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let Self { body, lifetime } = *self;
            body.close().await;
            drop(lifetime);
        })
    }
}

/// 解码入口请求，冻结认证身份不从这些请求头再次取得。
pub(crate) fn request_parts(
    request: MiddlewareRequest,
) -> Result<(String, HeaderMap, Bytes), MiddlewareError> {
    let (protocol, headers, body) = request.into_parts();
    Ok((protocol, decode_headers(headers)?, body))
}
