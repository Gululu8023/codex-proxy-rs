//! 原生入口请求快照、节点转发和 HTTP 响应生命周期。
pub(crate) mod delivery;
pub(crate) mod headers;
mod peer_route;
pub(crate) mod request;

use axum::{
    Router,
    body::Body,
    http::{HeaderName, HeaderValue, Request, StatusCode},
    response::{IntoResponse as _, Response},
};
use gateway_core::{engine::request_context::RequestScope, lifecycle::CancellationToken};
use http_body::{Body as HttpBody, Frame, SizeHint};
use std::{
    convert::Infallible,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::time::Instant;
use tower::ServiceExt as _;

#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) request_id: String,
    pub(crate) extensions: RequestScope,
    pub(crate) cancellation: CancellationToken,
}

#[derive(Clone)]
struct Settings {
    timeout: Option<Duration>,
    runtime: Option<gateway_core::routing::request_settings::RequestSettings>,
}

tokio::task_local! { static CURRENT: Context; }
tokio::task_local! { static SETTINGS: Option<gateway_core::routing::request_settings::RequestSettings>; }

pub(crate) fn current_settings() -> Option<gateway_core::routing::request_settings::RequestSettings>
{
    SETTINGS.try_with(Clone::clone).ok().flatten()
}
pub(crate) fn current() -> Option<Context> {
    CURRENT.try_with(Clone::clone).ok()
}
pub(crate) async fn scope<T>(
    context: Option<Context>,
    future: impl std::future::Future<Output = T>,
) -> T {
    match context {
        Some(context) => CURRENT.scope(context, future).await,
        None => future.await,
    }
}
pub(crate) type SettingsSource =
    Arc<dyn Fn() -> Option<gateway_core::routing::request_settings::RequestSettings> + Send + Sync>;

pub(crate) fn wrap(
    router: Router,
    timeout: Option<Duration>,
    request_id_header: HeaderName,
    settings: SettingsSource,
    peer_node: HeaderValue,
) -> Router {
    Router::new().fallback_service(tower::service_fn(move |mut request: Request<Body>| {
        let router = router.clone();
        let request_id_header = request_id_header.clone();
        let peer_node = peer_node.clone();
        // 身份认证、请求覆盖和执行共用同一份入口配置快照。
        request.extensions_mut().insert(Settings { timeout, runtime: settings() });
        async move { Ok::<_, Infallible>(handle(router, request, request_id_header, peer_node).await) }
    }))
}

async fn handle(
    router: Router,
    mut request: Request<Body>,
    request_id_header: HeaderName,
    peer_node: HeaderValue,
) -> Response {
    let started = Instant::now();
    let path = match peer_route::enter(request.headers_mut(), peer_node) {
        Ok(path) => path,
        Err(error) => return error.response(request.uri().path()),
    };
    let request_id = request
        .headers()
        .get(&request_id_header)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let context = Context {
        request_id,
        extensions: RequestScope::default()
            .with_request(crate::request_context::capture(&request))
            .with_peer_route(path),
        cancellation: CancellationToken::new(),
    };
    if let Ok(value) = HeaderValue::from_str(&context.request_id) {
        request.headers_mut().insert(request_id_header, value);
    }
    let guard = Arc::new(CallLifetime {
        cancellation: context.cancellation.clone(),
    });
    request.extensions_mut().insert(guard.clone());
    request.extensions_mut().insert(context.clone());
    let settings = request
        .extensions()
        .get::<Settings>()
        .and_then(|settings| settings.runtime.clone());
    let response = CURRENT
        .scope(
            context,
            SETTINGS.scope(settings, dispatch(router, request, started)),
        )
        .await;
    response.map(|body| Body::new(ResponseBody::new(body, guard)))
}

async fn dispatch(router: Router, request: Request<Body>, started: Instant) -> Response {
    let timeout = request
        .extensions()
        .get::<Settings>()
        .and_then(|settings| settings.timeout);
    let dispatch = router.oneshot(request);
    let result = match timeout {
        Some(timeout) => {
            let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
                return StatusCode::REQUEST_TIMEOUT.into_response();
            };
            match tokio::time::timeout(remaining, dispatch).await {
                Ok(result) => result,
                Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
            }
        }
        None => dispatch.await,
    };
    match result {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

pub(crate) struct CallLifetime {
    cancellation: CancellationToken,
}

impl Drop for CallLifetime {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

struct ResponseBody {
    body: Body,
    _lifetime: Arc<CallLifetime>,
    cancellation: futures::future::BoxFuture<'static, ()>,
    finished: bool,
}

impl ResponseBody {
    fn new(body: Body, lifetime: Arc<CallLifetime>) -> Self {
        let cancellation = lifetime.cancellation.clone();
        Self {
            body,
            _lifetime: lifetime,
            cancellation: Box::pin(async move { cancellation.cancelled().await }),
            finished: false,
        }
    }
}

impl HttpBody for ResponseBody {
    type Data = bytes::Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.finished {
            return Poll::Ready(None);
        }
        if self.cancellation.as_mut().poll(context).is_ready() {
            self.finished = true;
            self.body = Body::empty();
            return Poll::Ready(Some(Err(Box::new(gateway_core::error::GatewayError::new(
                gateway_core::error::GatewayErrorKind::Cancelled,
                "HTTP body cancelled",
            )))));
        }
        let frame = Pin::new(&mut self.body).poll_frame(context);
        if matches!(frame, Poll::Ready(None)) {
            self.finished = true;
        }
        frame.map(|frame| frame.map(|result| result.map_err(|error| Box::new(error) as _)))
    }

    fn is_end_stream(&self) -> bool {
        self.finished || self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
