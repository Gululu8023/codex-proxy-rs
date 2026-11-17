//! HTTP HeaderMap 与请求头值的无损转换

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use bytes::Bytes;
use gateway_core::engine::middleware::{MiddlewareError, RequestHeader};

/// 完整保留多值与非 UTF-8 header，Runtime 只转换 wire 编码
pub(crate) fn encode_headers(headers: &HeaderMap) -> Vec<RequestHeader> {
    headers
        .iter()
        .map(|(name, value)| {
            RequestHeader::new(name.as_str(), Bytes::copy_from_slice(value.as_bytes()))
        })
        .collect()
}

pub(crate) fn decode_headers(headers: Vec<RequestHeader>) -> Result<HeaderMap, MiddlewareError> {
    let mut result = HeaderMap::new();
    for header in headers {
        let name = HeaderName::from_bytes(header.name().as_bytes())
            .map_err(|_| MiddlewareError::InvalidState)?;
        let value =
            HeaderValue::from_bytes(header.value()).map_err(|_| MiddlewareError::InvalidState)?;
        result.append(name, value);
    }
    Ok(result)
}
