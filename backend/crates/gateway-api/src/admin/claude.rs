//! Claude 专项运行状态，只提供脱敏健康事实，不提供映射内容或密钥读取。

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminQuery, AdminResponse,
    wire::map_admin_service_error,
};
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;

pub(super) fn router() -> Router<crate::ApiState> {
    Router::new()
        .route("/api/admin/claude/privacy", get(privacy))
        .route(
            "/api/admin/claude/settings",
            get(settings).post(update_settings),
        )
        .route("/api/admin/claude/models", get(client_models))
        .route("/api/admin/prompt-guard/settings", get(guard_settings))
        .route(
            "/api/admin/prompt-guard/settings/update",
            post(update_guard_settings),
        )
        .route("/api/admin/prompt-guard/records", get(guard_records))
        .route("/api/admin/prompt-guard/policy", get(guard_policy))
        .route("/api/admin/prompt-guard/evaluate", post(guard_evaluate))
        .route("/api/admin/prompt-guard/candidates", get(guard_candidates))
        .route(
            "/api/admin/prompt-guard/candidates/save",
            post(guard_candidate_save),
        )
        .route(
            "/api/admin/prompt-guard/candidates/action",
            post(guard_candidate_action),
        )
        .route("/api/admin/prompt-guard/profiles", get(guard_profiles))
        .route("/api/admin/prompt-guard/locks", get(guard_locks))
        .route("/api/admin/prompt-guard/locks/release", post(guard_release))
}

async fn settings(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
) -> Result<impl IntoResponse, AdminError> {
    let current = state
        .admin
        .settings()
        .load()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(
            json!({"configRevision": current.config_revision.get(), "publicUrl": current.values.claude_public_url}),
        ),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClaudeSettingsUpdate {
    config_revision: u64,
    public_url: String,
}

async fn update_settings(
    auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminJson(input): AdminJson<ClaudeSettingsUpdate>,
) -> Result<impl IntoResponse, AdminError> {
    let revision = gateway_admin::model::Revision::new(input.config_revision)
        .map_err(|_| AdminError::bad_request("configRevision 无效"))?;
    let current = state
        .admin
        .settings()
        .update_domain(
            &auth.context().mutation_context(),
            revision,
            gateway_admin::model::settings::DomainSettingsUpdate::ClaudePublicUrl(input.public_url),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(
            json!({"configRevision": current.config_revision.get(), "publicUrl": current.values.claude_public_url}),
        ),
    ))
}

async fn guard_settings(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
) -> Result<impl IntoResponse, AdminError> {
    let current = state
        .admin
        .settings()
        .load()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(json!({"configRevision": current.config_revision.get(),
        "config": super::prompt_guard::PromptGuardWire(current.values.prompt_guard), "retentionDays": current.values.usage_retention_days})),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GuardSettingsUpdate {
    config_revision: u64,
    config: super::prompt_guard::PromptGuardWire,
}

async fn update_guard_settings(
    auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminJson(input): AdminJson<GuardSettingsUpdate>,
) -> Result<impl IntoResponse, AdminError> {
    let revision = gateway_admin::model::Revision::new(input.config_revision)
        .map_err(|_| AdminError::bad_request("configRevision 无效"))?;
    let mut validation = input.config.0.clone();
    if validation.review_api_key.is_none() {
        validation.review_api_key = state
            .admin
            .settings()
            .load()
            .await
            .map_err(map_admin_service_error)?
            .values
            .prompt_guard
            .review_api_key;
    }
    guard_control(&state)?
        .validate_config(&validation)
        .await
        .map_err(map_admin_service_error)?;
    let current = state
        .admin
        .settings()
        .update_domain(
            &auth.context().mutation_context(),
            revision,
            gateway_admin::model::settings::DomainSettingsUpdate::PromptGuard(Box::new(
                input.config.0,
            )),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(json!({"configRevision": current.config_revision.get(),
        "config": super::prompt_guard::PromptGuardWire(current.values.prompt_guard), "retentionDays": current.values.usage_retention_days})),
    ))
}

async fn privacy(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
) -> Result<impl IntoResponse, AdminError> {
    let runtime = state
        .claude_privacy
        .get()
        .ok_or_else(AdminError::internal)?;
    let health = runtime.health().await;
    let time = crate::time::TimePresenter::new(state.admin.timezone());
    let last_error_at = health
        .last_error_at
        .map(|at| time.datetime(&chrono::DateTime::<chrono::Utc>::from(at)));
    let mut body = serde_json::to_value(health).map_err(|_| AdminError::internal())?;
    body["lastErrorAt"] = json!(last_error_at);
    body["observedAt"] = json!(time.datetime(&time.now()));
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(body)))
}

async fn guard_records(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminQuery(query): AdminQuery<gateway_admin::model::prompt_guard_records::PromptGuardQuery>,
) -> Result<impl IntoResponse, AdminError> {
    let page = state
        .admin
        .observability()
        .prompt_guard_records(query)
        .await
        .map_err(map_admin_service_error)?;
    let mut page = serde_json::to_value(page).map_err(|_| AdminError::internal())?;
    let time = crate::time::TimePresenter::new(state.admin.timezone());
    if let Some(items) = page["items"].as_array_mut() {
        for item in items {
            item["startedAtDisplay"] = json!(time.rfc_display(item["startedAt"].as_str()));
        }
    }
    page["recordingHealth"] = json!(state.health().observation_buffer_health());
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(page)))
}

async fn guard_policy(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
) -> Result<impl IntoResponse, AdminError> {
    let policy = guard_control(&state)?
        .policy()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(policy),
    ))
}

fn guard_control(
    state: &crate::ApiState,
) -> Result<&std::sync::Arc<dyn gateway_admin::ports::prompt_guard::PromptGuardControl>, AdminError>
{
    state
        .prompt_guard_control
        .get()
        .ok_or_else(AdminError::service_unavailable)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GuardEvaluation {
    text: String,
    origin: String,
}
async fn guard_evaluate(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminJson(input): AdminJson<GuardEvaluation>,
) -> Result<impl IntoResponse, AdminError> {
    let result = guard_control(&state)?
        .evaluate(input.text, input.origin)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}
async fn guard_candidates(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminQuery(query): AdminQuery<gateway_admin::ports::prompt_guard::GuardListQuery>,
) -> Result<impl IntoResponse, AdminError> {
    let result = guard_control(&state)?
        .candidates(query)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}
async fn guard_candidate_save(
    auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminJson(input): AdminJson<gateway_admin::ports::prompt_guard::CandidateUpdate>,
) -> Result<impl IntoResponse, AdminError> {
    let result = guard_control(&state)?
        .save_candidate(&auth.context().mutation_context(), input)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}
async fn guard_candidate_action(
    auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminJson(input): AdminJson<gateway_admin::ports::prompt_guard::CandidateAction>,
) -> Result<impl IntoResponse, AdminError> {
    let result = guard_control(&state)?
        .candidate_action(&auth.context().mutation_context(), input)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}
async fn guard_profiles(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminQuery(query): AdminQuery<gateway_admin::ports::prompt_guard::GuardListQuery>,
) -> Result<impl IntoResponse, AdminError> {
    let result = guard_control(&state)?
        .profiles(query)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}
async fn guard_locks(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminQuery(query): AdminQuery<gateway_admin::ports::prompt_guard::GuardListQuery>,
) -> Result<impl IntoResponse, AdminError> {
    let result = guard_control(&state)?
        .locks(query)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GuardRelease {
    id: i64,
    revision: i64,
}
async fn guard_release(
    auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminJson(input): AdminJson<GuardRelease>,
) -> Result<impl IntoResponse, AdminError> {
    guard_control(&state)?
        .release_lock(&auth.context().mutation_context(), input.id, input.revision)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(json!({"released":true})),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClientModelsQuery {
    key_id: String,
}

async fn client_models(
    _auth: AdminAuth,
    State(state): State<crate::ApiState>,
    AdminQuery(query): AdminQuery<ClientModelsQuery>,
) -> Result<impl IntoResponse, AdminError> {
    state
        .claude_privacy
        .get()
        .ok_or_else(AdminError::internal)?
        .ensure_ready()
        .map_err(|_| AdminError::service_unavailable())?;
    let key = gateway_core::policy::ClientApiKeyId::new(query.key_id)
        .map_err(|_| AdminError::bad_request("keyId 无效"))?;
    let models = state
        .execution
        .inspect_client_model_catalog(&key, "anthropic")
        .await
        .map_err(|error| match error.kind() {
            gateway_core::error::GatewayErrorKind::Unauthorized => {
                AdminError::not_found("Key 不存在")
            }
            gateway_core::error::GatewayErrorKind::PolicyDenied => AdminError::forbidden(),
            _ => AdminError::service_unavailable(),
        })?;
    let data = models
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
        AdminEnvelope::ok(json!({"models": data, "scope": "client_key", "keyId": key.as_str()})),
    ))
}
