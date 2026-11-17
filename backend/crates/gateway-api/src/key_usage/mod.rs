//! 当前 Key 的单页用量 API；只暴露页面需要的只读字段

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{any, get},
};

use crate::{
    admin::{AdminEnvelope, AdminError, AdminQuery, AdminResponse, wire::map_admin_service_error},
    auth::SessionState,
    session_cookie,
};

mod presenter;
mod query;

pub(crate) fn claude_models_router() -> Router<crate::ApiState> {
    Router::new()
        .route("/api/key-usage/claude-models", get(claude_models))
        .layer(middleware::map_response(no_store))
}

async fn claude_models(
    State(state): State<crate::ApiState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AdminError> {
    use gateway_admin::model::auth::SessionSubject;
    let session = state
        .admin
        .auth()
        .session(session_cookie::value(&headers).as_deref())
        .await
        .map_err(map_admin_service_error)?
        .ok_or_else(AdminError::session_required)?;
    // 只从服务端会话取得 Key ID；自助入口不接受调用方指定其他 Key。
    let SessionSubject::Key { client_key_id } = session.subject else {
        return Err(AdminError::forbidden());
    };
    state
        .claude_privacy
        .get()
        .ok_or_else(AdminError::internal)?
        .ensure_ready()
        .map_err(|_| AdminError::service_unavailable())?;
    let models = state
        .execution
        .inspect_client_model_catalog(&client_key_id, "anthropic")
        .await
        .map_err(|error| match error.kind() {
            gateway_core::error::GatewayErrorKind::Unauthorized
            | gateway_core::error::GatewayErrorKind::PolicyDenied => AdminError::forbidden(),
            _ => AdminError::service_unavailable(),
        })?;
    let models = models
        .into_iter()
        .filter_map(|model| match model {
            gateway_core::routing::PublicModelDescriptor::Native { payload, .. }
                if payload.protocol() == "anthropic" =>
            {
                Some(
                    serde_json::from_slice::<serde_json::Value>(payload.body())
                        .map_err(|_| AdminError::internal()),
                )
            }
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(serde_json::json!({"models":models})),
    ))
}

pub(crate) fn router<S>() -> Router<S>
where
    S: SessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/key-usage/version", get(version::<S>))
        .route("/api/key-usage/config", get(config::<S>))
        .route("/api/key-usage/overview", get(overview::<S>))
        .route("/api/key-usage/records", get(records::<S>))
        .route("/api/key-usage", any(not_found))
        .route("/api/key-usage/{*path}", any(not_found))
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::map_response(no_store))
}

async fn version<S>(
    State(state): State<S>,
    headers: HeaderMap,
    AdminQuery(_): AdminQuery<query::EmptyQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let version = state
        .admin_services()
        .key_usage()
        .version(session_cookie::value(&headers).as_deref())
        .await
        .map_err(map_admin_service_error)?
        .ok_or_else(AdminError::session_required)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(presenter::version(version)),
    ))
}

async fn config<S>(
    State(state): State<S>,
    headers: HeaderMap,
    AdminQuery(_): AdminQuery<query::EmptyQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let secret = state
        .admin_services()
        .key_usage()
        .config(session_cookie::value(&headers).as_deref())
        .await
        .map_err(map_admin_service_error)?
        .ok_or_else(AdminError::session_required)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(presenter::config(secret)),
    ))
}

async fn overview<S>(
    State(state): State<S>,
    headers: HeaderMap,
    AdminQuery(query): AdminQuery<query::OverviewQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let time = crate::time::TimePresenter::new(state.admin_services().timezone());
    let overview = state
        .admin_services()
        .key_usage()
        .overview(
            session_cookie::value(&headers).as_deref(),
            query.into_domain(state.admin_services().timezone())?,
        )
        .await
        .map_err(map_admin_service_error)?
        .ok_or_else(AdminError::session_required)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(presenter::overview(overview, time)),
    ))
}

async fn records<S>(
    State(state): State<S>,
    headers: HeaderMap,
    AdminQuery(query): AdminQuery<query::RecordsQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let time = crate::time::TimePresenter::new(state.admin_services().timezone());
    let records = state
        .admin_services()
        .key_usage()
        .records(
            session_cookie::value(&headers).as_deref(),
            query.into_domain(state.admin_services().timezone())?,
        )
        .await
        .map_err(map_admin_service_error)?
        .ok_or_else(AdminError::session_required)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(presenter::records(records, time)),
    ))
}

async fn not_found() -> AdminError {
    AdminError::not_found("密钥用量接口不存在")
}
async fn method_not_allowed() -> AdminError {
    AdminError::method_not_allowed()
}
async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
