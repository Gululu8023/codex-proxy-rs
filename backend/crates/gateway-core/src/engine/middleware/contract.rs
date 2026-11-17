//! 协议解码后的请求和流式响应封套。

use crate::engine::request_context::RequestHeader;
use std::fmt;

use bytes::Bytes;
use futures::future::BoxFuture;

use crate::engine::EngineError;
use crate::error::{GatewayError, ProviderError};

/// 协议编码后的正文边界
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewareFraming {
    /// 一个完整 JSON document
    JsonDocument,
    /// 一个完整 SSE event，包含终止该事件的空行
    SseEvent,
    /// 不承诺 UTF-8 或消息边界的原始字节
    RawBytes,
}

/// 完成入口解码的协议请求。
#[derive(Clone, PartialEq, Eq)]
pub struct MiddlewareRequest {
    protocol: String,
    headers: Vec<RequestHeader>,
    body: Bytes,
}

impl MiddlewareRequest {
    #[must_use]
    pub fn new(protocol: impl Into<String>, headers: Vec<RequestHeader>, body: Bytes) -> Self {
        let protocol = protocol.into();
        Self {
            protocol,
            headers,
            body,
        }
    }

    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    #[must_use]
    pub fn headers(&self) -> &[RequestHeader] {
        &self.headers
    }

    #[must_use]
    pub const fn body(&self) -> &Bytes {
        &self.body
    }

    #[must_use]
    pub fn into_parts(self) -> (String, Vec<RequestHeader>, Bytes) {
        (self.protocol, self.headers, self.body)
    }
}

impl fmt::Debug for MiddlewareRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MiddlewareRequest")
            .field("protocol", &self.protocol)
            .field("headers", &self.headers)
            .field("body", &"<not included in Debug>")
            .finish()
    }
}

/// 逐次拉取的完整协议正文块
pub struct MiddlewareFrame {
    bytes: Bytes,
    framing: MiddlewareFraming,
    terminal: bool,
}

impl MiddlewareFrame {
    #[must_use]
    pub const fn new(bytes: Bytes, framing: MiddlewareFraming, terminal: bool) -> Self {
        Self {
            bytes,
            framing,
            terminal,
        }
    }

    #[must_use]
    pub const fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    #[must_use]
    pub const fn framing(&self) -> MiddlewareFraming {
        self.framing
    }

    #[must_use]
    pub const fn terminal(&self) -> bool {
        self.terminal
    }

    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }

    /// 拆出协议编码后的正文及边界事实。
    #[must_use]
    pub fn into_parts(self) -> (Bytes, MiddlewareFraming, bool) {
        (self.bytes, self.framing, self.terminal)
    }
}

impl fmt::Debug for MiddlewareFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MiddlewareFrame")
            .field("bytes", &"<not included in Debug>")
            .field("framing", &self.framing)
            .field("terminal", &self.terminal)
            .finish()
    }
}

/// 协议交付层的惰性正文
///
/// 实现必须让 `Drop` 与 [`Self::close`] 都回收底层流
/// 请求 finalization 继续委托
/// 原有 [`crate::engine::execution::ExecutionSession`]，本端口不建立第二套清理器
pub trait MiddlewareBody: Send {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<MiddlewareFrame>, MiddlewareError>>;

    /// 在最外层输出准备完成后提交原有 Core delivery barrier
    /// 包装器必须向内委托，避免重复结算。
    fn commit_downstream(
        &mut self,
        _client_status_code: Option<u16>,
    ) -> BoxFuture<'_, Result<(), MiddlewareError>> {
        Box::pin(async { Ok(()) })
    }

    /// 首字节前失败时记录最终返回给客户端的状态
    /// 包装器必须向内委托，避免重复结算。
    fn record_client_status(
        &mut self,
        _client_status_code: u16,
    ) -> BoxFuture<'_, Result<(), MiddlewareError>> {
        Box::pin(async { Ok(()) })
    }

    /// 静态错误正文无需 Core 结算；执行会话的包装器必须委托原会话。
    fn is_finalized(&self) -> bool {
        true
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, ()>;
}

/// 协议响应头与惰性正文
pub struct MiddlewareResponse {
    protocol: String,
    status_code: u16,
    headers: Vec<RequestHeader>,
    body: Box<dyn MiddlewareBody>,
}

impl MiddlewareResponse {
    #[must_use]
    pub fn new(
        protocol: String,
        status_code: u16,
        headers: Vec<RequestHeader>,
        body: Box<dyn MiddlewareBody>,
    ) -> Self {
        Self {
            protocol,
            status_code,
            headers,
            body,
        }
    }

    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    #[must_use]
    pub const fn status_code(&self) -> u16 {
        self.status_code
    }

    #[must_use]
    pub fn headers(&self) -> &[RequestHeader] {
        &self.headers
    }

    #[must_use]
    pub fn into_parts(self) -> (String, u16, Vec<RequestHeader>, Box<dyn MiddlewareBody>) {
        (self.protocol, self.status_code, self.headers, self.body)
    }
}

impl fmt::Debug for MiddlewareResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MiddlewareResponse")
            .field("protocol", &self.protocol)
            .field("status_code", &self.status_code)
            .field("headers", &self.headers)
            .field("body", &"<stream>")
            .finish()
    }
}

/// 原有领域错误保持原类型，交付层不能改写上游发送事实。
#[derive(Debug)]
pub enum MiddlewareError {
    Rejected,
    Fault,
    InvalidState,
    Gateway(GatewayError),
    Engine(EngineError),
    Provider(ProviderError),
}

impl fmt::Display for MiddlewareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Rejected => "middleware rejected the request",
            Self::Fault => "middleware call failed",
            Self::InvalidState => "middleware state is invalid",
            Self::Gateway(_) => "gateway middleware terminal failed",
            Self::Engine(_) => "execution middleware terminal failed",
            Self::Provider(_) => "provider middleware terminal failed",
        })
    }
}

impl std::error::Error for MiddlewareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Gateway(error) => Some(error),
            Self::Engine(error) => Some(error),
            Self::Provider(error) => Some(error),
            _ => None,
        }
    }
}

impl From<GatewayError> for MiddlewareError {
    fn from(error: GatewayError) -> Self {
        Self::Gateway(error)
    }
}

impl From<EngineError> for MiddlewareError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<ProviderError> for MiddlewareError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}
