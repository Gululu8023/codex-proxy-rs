//! 原生出站请求构造与节点转发路径。
use super::{
    client::{CodexBackendClient, CodexClientError, CodexClientResult},
    websocket::CodexWebSocketRequest,
};
use bytes::Bytes;
use reqwest::{Url, header::HeaderMap};
use tungstenite::http;
impl CodexBackendClient {
    // 已识别的互联账号才携带路径，官方端点不参与节点转发
    pub(super) fn route_to(&self, target: &Url) -> Option<&http::HeaderValue> {
        self.peer_route.as_ref().filter(|_| {
            self.protocol != super::client::OpenAiUpstreamProtocol::Codex
                && !matches!(
                    target.host_str(),
                    Some("api.openai.com" | "api.anthropic.com")
                )
        })
    }

    pub(super) async fn prepare_http_request(
        &self,
        endpoint: String,
        mut headers: HeaderMap,
        body: Bytes,
    ) -> CodexClientResult<reqwest::Request> {
        let target = Url::parse(&endpoint).map_err(|_| invalid_request())?;
        headers.remove(gateway_protocol::peer::ROUTE_HEADER);
        if let Some(path) = self.route_to(&target) {
            headers.insert(gateway_protocol::peer::ROUTE_HEADER, path.clone());
        }
        self.client
            .post(endpoint)
            .headers(headers)
            .body(body)
            .build()
            .map_err(CodexClientError::Http)
    }

    pub(super) async fn prepare_websocket_request(
        &self,
        mut request: CodexWebSocketRequest,
    ) -> CodexClientResult<CodexWebSocketRequest> {
        let path = if self.peer_route.is_some()
            && self.protocol != super::client::OpenAiUpstreamProtocol::Codex
        {
            let target =
                Url::parse(request.connection().endpoint()).map_err(|_| invalid_request())?;
            self.route_to(&target)
                .map(http::HeaderValue::to_str)
                .transpose()
                .map_err(|_| invalid_request())?
        } else {
            None
        };
        request.connection.set_peer_route(path);
        Ok(request)
    }
}

fn invalid_request() -> CodexClientError {
    CodexClientError::Outbound(Box::new(gateway_core::error::ProviderError::new(
        gateway_core::error::ProviderErrorKind::InvalidRequest,
        gateway_core::upstream::UpstreamSendState::NotSent,
    )))
}
