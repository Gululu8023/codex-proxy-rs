//! Claude 模型目录必须经过鉴权及 veil 门禁，反向分页返回紧邻游标的记录。

use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use gateway_core::engine::execution::{
    ClientAuthenticationError, StartExecution, StartProviderExecution, StartedExecution,
};
use gateway_core::error::GatewayError;
use gateway_core::operation::RawJsonPayload;
use gateway_core::routing::{ProviderCatalogUnavailable, PublicModelDescriptor, PublicModelId};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

struct CatalogExecution {
    client: AuthenticatedClient,
    calls: AtomicUsize,
}

impl ExecutionService for CatalogExecution {
    fn authenticate(&self, key: &str) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        if key == "sk_claude_test" {
            Ok(self.client.clone())
        } else {
            Err(ClientAuthenticationError::InvalidKey)
        }
    }
    fn verify_request(
        &self,
        request: gateway_core::engine::authentication::ClientAuthenticationRequest,
    ) -> BoxFuture<'_, Result<AuthenticatedClient, ClientAuthenticationError>> {
        // 目录使用只读认证入口，默认 trait 实现会拒绝所有 Key。
        self.authenticate_request(request)
    }
    fn client_model_catalog<'a>(
        &'a self,
        _: &'a AuthenticatedClient,
        protocol: &'a str,
        _: &'a str,
        _: Option<&'a http::HeaderValue>,
    ) -> BoxFuture<'a, Result<Vec<PublicModelDescriptor>, ProviderCatalogUnavailable>> {
        Box::pin(async move {
            assert_eq!(protocol, "anthropic");
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok((1..=6)
                .map(|i| {
                    let id = format!("claude-{i}");
                    PublicModelDescriptor::Native {
                        model: PublicModelId::new(&id).unwrap(),
                        payload: RawJsonPayload::new(
                            "anthropic",
                            serde_json::to_vec(&json!({"id":id,"type":"model","display_name":id}))
                                .unwrap()
                                .into(),
                        )
                        .unwrap(),
                    }
                })
                .collect())
        })
    }
    fn public_models(&self, _: &AuthenticatedClient) -> Vec<PublicModelId> {
        unreachable!("native catalog only")
    }
    fn contains_public_model(&self, _: &AuthenticatedClient, _: &PublicModelId) -> bool {
        unreachable!("native catalog only")
    }
    fn start(&self, _: StartExecution) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        panic!("catalog must not generate")
    }
    fn start_provider_endpoint(
        &self,
        _: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        panic!("rejected requests must not generate")
    }
}

async fn fixture(
    provider: &str,
    ready: bool,
) -> (axum::Router, Arc<CatalogExecution>, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let privacy = gateway_veil::Runtime::initialize(gateway_veil::Config {
        data_directory: directory.path().join("veil"),
        initialize_store: ready,
        ..Default::default()
    })
    .await;
    assert_eq!(privacy.ensure_ready().is_ok(), ready);
    let execution = Arc::new(CatalogExecution {
        client: authenticated_client_for_provider("sk_claude_test", provider),
        calls: AtomicUsize::new(0),
    });
    let admin = crate::admin::AdminTestFixture::new().await;
    let app = gateway_api::initialize(
        uuid::Uuid::new_v4(),
        gateway_api::ApiConfig {
            asset_directory: directory.path().to_owned(),
            cors_allowed_origins: Vec::new(),
            request_timeout_seconds: None,
            request_id_header: "x-request-id".into(),
        },
        execution.clone(),
        admin.services,
        Vec::new(),
        Arc::new(EmptyWorkerHealth),
        Arc::new(TestLifecycle::default()),
        Arc::new(crate::support::RecordingDiagnostics::default()),
    )
    .unwrap()
    .with_claude_privacy(privacy)
    .router();
    (app, execution, directory)
}

fn request(path: &str, post: bool, key: &str) -> Request<Body> {
    Request::builder()
        .method(if post { "POST" } else { "GET" })
        .uri(path)
        .header("x-api-key", key)
        .header("content-type", "application/json")
        .body(if post {
            Body::from(r#"{"model":"claude-1","max_tokens":32,"messages":[]}"#)
        } else {
            Body::empty()
        })
        .unwrap()
}

#[tokio::test]
async fn every_claude_endpoint_requires_provider_permission_and_ready_privacy() {
    for (provider, ready, key, status) in [
        ("openai", true, "sk_claude_test", StatusCode::FORBIDDEN),
        (
            "anthropic",
            false,
            "sk_claude_test",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        ("anthropic", true, "wrong-key", StatusCode::UNAUTHORIZED),
    ] {
        let (app, execution, _directory) = fixture(provider, ready).await;
        for (path, post) in [
            ("/claude/v1/models", false),
            ("/claude/v1/models/claude-1", false),
            ("/claude/v1/messages", true),
            ("/claude/v1/messages/count_tokens", true),
        ] {
            assert_eq!(
                app.clone()
                    .oneshot(request(path, post, key))
                    .await
                    .unwrap()
                    .status(),
                status,
                "{provider} {ready} {path}"
            );
        }
        assert_eq!(
            execution.calls.load(Ordering::SeqCst),
            0,
            "不能从缓存目录绕过隐私和权限检查"
        );
    }
}

#[tokio::test]
async fn reverse_pagination_returns_the_adjacent_page_and_preserves_direction() {
    let (app, execution, _directory) = fixture("anthropic", true).await;
    for (query, ids, more) in [
        (
            "before_id=claude-6&limit=2",
            vec!["claude-4", "claude-5"],
            true,
        ),
        (
            "before_id=claude-3&limit=2",
            vec!["claude-1", "claude-2"],
            false,
        ),
        ("before_id=claude-1&limit=2", vec![], false),
        (
            "after_id=claude-3&limit=2",
            vec!["claude-4", "claude-5"],
            true,
        ),
        ("after_id=claude-6&limit=2", vec![], false),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                &format!("/claude/v1/models?{query}"),
                false,
                "sk_claude_test",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(
            body["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ids
        );
        assert_eq!(body["has_more"], more);
        assert_eq!(body["first_id"], json!(ids.first()));
        assert_eq!(body["last_id"], json!(ids.last()));
    }
    for query in [
        "before_id=missing",
        "before_id=claude-2&after_id=claude-3",
        "limit=0",
        "limit=1001",
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    &format!("/claude/v1/models?{query}"),
                    false,
                    "sk_claude_test"
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(execution.calls.load(Ordering::SeqCst), 9);
}
