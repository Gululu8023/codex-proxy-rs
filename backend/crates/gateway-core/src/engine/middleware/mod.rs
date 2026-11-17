//! 协议入口使用的流式正文封套与响应结算边界。

mod contract;
pub use crate::engine::request_context::RequestHeader;
pub use contract::{
    MiddlewareBody, MiddlewareError, MiddlewareFrame, MiddlewareFraming, MiddlewareRequest,
    MiddlewareResponse,
};
