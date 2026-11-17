//! 原生入口请求事实与已校验的节点转发路径。

use bytes::Bytes;
use std::{fmt, sync::Arc};

/// 同一次执行携带入口事实，保持客户端身份与节点转发链一致。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestScope {
    request: Option<Arc<RequestContext>>,
    peer_route: Option<http::HeaderValue>,
}

impl RequestScope {
    /// 随本次执行保留已校验的转发路径，防止互联转发绕过节点环路检查。
    #[must_use]
    pub fn with_peer_route(mut self, path: http::HeaderValue) -> Self {
        self.peer_route = Some(path);
        self
    }

    #[must_use]
    pub fn peer_route(&self) -> Option<&http::HeaderValue> {
        self.peer_route.as_ref()
    }

    #[must_use]
    pub fn with_request(mut self, request: Arc<RequestContext>) -> Self {
        self.request = Some(request);
        self
    }

    #[must_use]
    pub fn request(&self) -> Option<&RequestContext> {
        self.request.as_deref()
    }

    #[must_use]
    pub fn shared_request(&self) -> Option<Arc<RequestContext>> {
        self.request.clone()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RequestHeader {
    name: String,
    value: Bytes,
}

impl RequestHeader {
    #[must_use]
    pub fn new(name: impl Into<String>, value: Bytes) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn value(&self) -> &Bytes {
        &self.value
    }

    #[must_use]
    pub fn into_parts(self) -> (String, Bytes) {
        (self.name, self.value)
    }
}

impl fmt::Debug for RequestHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestHeader")
            .field("name", &self.name)
            .field("value", &"<not included in Debug>")
            .finish()
    }
}

/// 不可变入口快照；请求头和 URI 不进入诊断日志。
#[derive(Clone, PartialEq, Eq)]
pub struct RequestContext {
    pub method: String,
    pub uri: String,
    pub http_version: String,
    pub headers: Vec<RequestHeader>,
    pub peer_addr: Option<std::net::SocketAddr>,
    pub client_ip: Option<std::net::IpAddr>,
}

impl std::fmt::Debug for RequestContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // URI 查询与请求头可能包含凭据，不进入诊断日志
        formatter
            .debug_struct("RequestContext")
            .field("method", &self.method)
            .field("http_version", &self.http_version)
            .finish_non_exhaustive()
    }
}
