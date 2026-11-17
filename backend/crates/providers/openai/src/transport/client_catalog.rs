//! Codex 与 Responses API 模型目录的 HTTP 读取与客户端版本协商

use gateway_protocol::openai::events::retry_after_seconds_from_body;

use super::{
    catalog::{CodexModelCatalogSnapshot, catalog_etag, parse_codex_model_catalog},
    client::{
        CodexBackendClient, CodexBackendTransport, CodexClientError, CodexClientResult,
        CodexRequestContext, OpenAiUpstreamProtocol, retry_after_seconds,
    },
    diagnostics::CodexUpstreamSendPhase,
    endpoints::endpoint_url,
    profile::CodexWireProfile,
    response_meta,
};

impl CodexBackendClient {
    /// 目录优先使用显式协商版本，否则沿用调用方冻结的画像版本
    pub async fn fetch_models_with_context(
        &self,
        context: CodexRequestContext<'_>,
        client_version: Option<&str>,
    ) -> CodexClientResult<CodexModelCatalogSnapshot> {
        let profile = self.profile.snapshot();
        let query = (
            "client_version",
            client_version.unwrap_or(&profile.codex_version),
        );
        let (body, etag) = match self.fetch_models_page(context, &profile, &[query]).await {
            Err(CodexClientError::Upstream {
                status, ref body, ..
            }) if self.protocol == OpenAiUpstreamProtocol::ResponsesApi
                && rejects_catalog_version(status, body) =>
            {
                self.fetch_models_page(context, &profile, &[]).await?
            }
            result => result?,
        };
        match self.protocol {
            OpenAiUpstreamProtocol::Codex => Ok(parse_codex_model_catalog(&body, etag.as_deref())?),
            OpenAiUpstreamProtocol::ResponsesApi => Ok(super::catalog::parse_api_model_catalog(
                &body,
                etag.as_deref(),
            )?),
        }
    }

    async fn fetch_models_page(
        &self,
        context: CodexRequestContext<'_>,
        profile: &CodexWireProfile,
        query: &[(&str, &str)],
    ) -> CodexClientResult<(bytes::Bytes, Option<String>)> {
        let path = match self.protocol {
            OpenAiUpstreamProtocol::Codex => "codex/models",
            OpenAiUpstreamProtocol::ResponsesApi => "models",
        };
        let headers = self.model_request_headers(profile, context)?;
        let mut request = self
            .client
            .get(endpoint_url(&self.base_url, path))
            .headers(headers)
            .query(query)
            .build()?;
        if let Some(route) = self.route_to(request.url()) {
            request
                .headers_mut()
                .insert(gateway_protocol::peer::ROUTE_HEADER, route.clone());
        }
        let response = self.client.execute(request).await?;
        let status = response.status();
        let diagnostics = response_meta::diagnostics(Some(status.as_u16()), response.headers())
            .with_response_origin(Some(response.url().clone()));
        let set_cookie_headers = response_meta::set_cookie_headers(response.headers());
        let retry_after_seconds = retry_after_seconds(response.headers(), None);
        let etag = status
            .is_success()
            .then(|| catalog_etag(response.headers()))
            .transpose()?
            .flatten();
        let body = response.bytes().await?;
        if !status.is_success() {
            let body = String::from_utf8_lossy(&body).into_owned();
            return Err(CodexClientError::Upstream {
                status,
                retry_after_seconds: retry_after_seconds
                    .or_else(|| retry_after_seconds_from_body(&body)),
                body,
                client_response: None,
                diagnostics: Box::new(diagnostics),
                set_cookie_headers,
                rate_limit_headers: Vec::new(),
                transport: CodexBackendTransport::HttpSse,
                transport_metrics: Box::default(),
                send_phase: CodexUpstreamSendPhase::AfterPayload,
            });
        }
        Ok((body, etag))
    }
}

/// 仅在上游明确不接受版本参数时读取普通目录，其他失败保留原错误
fn rejects_catalog_version(status: reqwest::StatusCode, body: &str) -> bool {
    if !matches!(status.as_u16(), 400 | 422) {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    let error = &value["error"];
    error["param"].as_str() == Some("client_version")
        && matches!(
            error["code"].as_str(),
            Some("unknown_parameter" | "unsupported_parameter")
        )
}
