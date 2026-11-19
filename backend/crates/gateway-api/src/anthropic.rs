//! Anthropic 原生入口、网关鉴权与保留提交屏障的 JSON／SSE 交付

use std::{borrow::Cow, collections::BTreeMap};

use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use gateway_core::{
    engine::middleware::{
        MiddlewareBody, MiddlewareError, MiddlewareFrame, MiddlewareFraming, MiddlewareResponse,
    },
    engine::{
        EngineError,
        authentication::ClientAuthenticationRequest,
        execution::{
            AuthenticatedClient, ClientTransport, ExecutionRequestMetadata, PreparedRootExecution,
            StartedExecution,
        },
    },
    error::{GatewayError, GatewayErrorKind},
    event::{GatewayEvent, ProviderEvent, ProviderResponseHeader},
    operation::{
        CapabilityRequirements, Feature, GenerateRequest, Operation, OperationKind,
        ProtocolPayload, RawJsonPayload, TokenCountRequest,
    },
    policy::ClaudeCodeVersion,
    routing::{ProviderKind, PublicModelDescriptor, UpstreamModelId},
};
use serde_json::{Map, Value, json};

use crate::{
    ApiState,
    execution::{PendingExecution, pending_execution_response},
    middleware::{
        delivery::{ExpectedBody, buffered_response, into_http_response},
        headers::encode_headers,
        request::{RequestInput, invoke_request, request_parts},
    },
};
use futures::future::BoxFuture;

const PROTOCOL: &str = "anthropic";
// 响应缓冲预算与可配置的入口请求体上限独立。
const MAX_RESPONSE_BODY_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn router() -> Router<ApiState> {
    Router::new().nest(
        "/claude",
        Router::new()
            .route("/v1/messages", post(messages))
            .route("/v1/messages/count_tokens", post(count_tokens))
            .route("/v1/models", get(models))
            .route("/v1/models/{id}", get(model))
            .fallback(|| async { error(404, "not_found_error", "Unknown Anthropic endpoint") })
            .method_not_allowed_fallback(|| async {
                error(405, "invalid_request_error", "Method not allowed")
            }),
    )
}

// 拒绝重复或冲突的凭据，映射空间只使用认证后的内部 Key ID。
fn client_key(headers: &HeaderMap) -> Result<String, Box<Response>> {
    let single = |name: &str| -> Result<Option<&str>, Box<Response>> {
        let mut values = headers.get_all(name).iter();
        let first = values.next();
        if values.next().is_some() {
            return Err(Box::new(error(
                401,
                "authentication_error",
                "Duplicate credentials",
            )));
        }
        first
            .map(|value| {
                value.to_str().map_err(|_| {
                    Box::new(error(401, "authentication_error", "Invalid credentials"))
                })
            })
            .transpose()
    };
    let bearer = single("authorization")?
        .map(|value| {
            let (scheme, key) = value.split_once(' ').ok_or_else(|| {
                Box::new(error(401, "authentication_error", "Invalid bearer token"))
            })?;
            if !scheme.eq_ignore_ascii_case("bearer") {
                return Err(Box::new(error(
                    401,
                    "authentication_error",
                    "Invalid bearer token",
                )));
            }
            Ok(key)
        })
        .transpose()?;
    let api_key = single("x-api-key")?;
    if bearer.zip(api_key).is_some_and(|(a, b)| a != b) {
        return Err(Box::new(error(
            401,
            "authentication_error",
            "Conflicting credentials",
        )));
    }
    let key = bearer
        .or(api_key)
        .ok_or_else(|| Box::new(error(401, "authentication_error", "Missing client key")))?;
    if key.is_empty()
        || key
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b',')
    {
        return Err(Box::new(error(
            401,
            "authentication_error",
            "Invalid client key",
        )));
    }
    Ok(key.to_owned())
}

async fn authenticate(
    state: &ApiState,
    headers: &HeaderMap,
    readonly: bool,
) -> Result<AuthenticatedClient, Box<Response>> {
    let key = client_key(headers)?;
    let request = ClientAuthenticationRequest::bearer(&key)
        .map_err(|_| Box::new(error(401, "authentication_error", "Invalid client key")))?;
    let result = if readonly {
        state.execution.verify_request(request).await
    } else {
        state.execution.authenticate_request(request).await
    };
    let client = result.map_err(|failure| {
        use gateway_core::engine::execution::ClientAuthenticationError;
        Box::new(match failure {
            ClientAuthenticationError::InvalidKey => {
                error(401, "authentication_error", "Invalid client key")
            }
            _ => error(503, "api_error", "Authentication service unavailable"),
        })
    })?;
    if client.policy().authorize().is_err()
        || !client
            .policy()
            .account_scope()
            .provider_kinds()
            .iter()
            .any(|kind| kind.as_str() == PROTOCOL)
    {
        return Err(Box::new(error(
            403,
            "permission_error",
            "Client key cannot access Anthropic",
        )));
    }
    // 只使用下游原始 UA；出站身份伪装和版本自动同步不能改变客户端准入结果。
    if let Some(minimum) = client.snapshot().min_claude_code_version() {
        enforce_claude_code_version(headers, minimum)?;
    }
    // 模型目录可能完全来自 Core 快照，也必须通过隐私边界，不能形成缓存旁路。
    if state
        .claude_privacy
        .get()
        .is_none_or(|runtime| runtime.ensure_ready().is_err())
    {
        return Err(Box::new(error(
            503,
            "api_error",
            "Claude privacy runtime is unavailable",
        )));
    }
    Ok(client)
}

fn enforce_claude_code_version(
    headers: &HeaderMap,
    minimum: &ClaudeCodeVersion,
) -> Result<(), Box<Response>> {
    let values = headers.get_all(header::USER_AGENT);
    if !values
        .iter()
        .any(|value| value.as_bytes().starts_with(b"claude-cli/"))
    {
        return Ok(());
    }
    let mut values = values.iter();
    let current = values
        .next()
        .filter(|value| value.as_bytes().len() <= 4096)
        .filter(|_| values.next().is_none())
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("claude-cli/"))
        .and_then(|value| value.split_ascii_whitespace().next())
        .and_then(ClaudeCodeVersion::parse);
    let (message, code) = match current {
        Some(current) if current.meets_minimum(minimum) => return Ok(()),
        Some(current) => (
            format!("Claude Code {current} is outdated. Upgrade to {minimum} or later."),
            "client_version_too_old",
        ),
        None => (
            format!(
                "Claude Code must provide a valid client version. Version {minimum} or later is required."
            ),
            "client_version_unavailable",
        ),
    };
    let mut response = error(400, "invalid_request_error", &message);
    response
        .extensions_mut()
        .insert(crate::client_error::AccessRejection(code));
    Err(Box::new(response))
}

fn decode_body(
    bytes: Bytes,
    mut headers: HeaderMap,
    maximum_body_bytes: usize,
) -> Result<(HeaderMap, Bytes), Box<Response>> {
    let decoded =
        crate::request_body::decompress_request_body(&bytes, &headers, maximum_body_bytes)
            .map_err(|failure| Box::new(request_body_error(failure)))?;
    let bytes = match decoded {
        Cow::Borrowed(_) => bytes,
        Cow::Owned(body) => Bytes::from(body),
    };
    // 执行层收到的是解压后的正文，原始传输长度和编码不能继续描述它。
    headers.remove(header::CONTENT_ENCODING);
    headers.remove(header::CONTENT_LENGTH);
    Ok((headers, bytes))
}

fn request_body_error(failure: crate::request_body::RequestBodyDecodeError) -> Response {
    use crate::request_body::RequestBodyDecodeError;
    match failure {
        RequestBodyDecodeError::TooLarge { limit_bytes } => error(
            413,
            "request_too_large",
            &format!("Decompressed request body exceeds the allowed size ({limit_bytes} bytes)."),
        ),
        RequestBodyDecodeError::UnsupportedEncoding { .. } => error(
            400,
            "invalid_request_error",
            "Unsupported request Content-Encoding",
        ),
        RequestBodyDecodeError::MalformedBody => error(
            400,
            "invalid_request_error",
            "Invalid compressed request body or Content-Encoding",
        ),
    }
}

async fn messages(State(state): State<ApiState>, request: Request<Body>) -> Response {
    execute(state, request, false).await
}
async fn count_tokens(State(state): State<ApiState>, request: Request<Body>) -> Response {
    execute(state, request, true).await
}

async fn execute(state: ApiState, request: Request<Body>, count: bool) -> Response {
    let (parts, body) = request.into_parts();
    let client = match authenticate(&state, &parts.headers, false).await {
        Ok(client) => client,
        Err(response) => return *response,
    };
    let maximum_body_bytes = client.snapshot().responses_max_decompressed_body_bytes();
    let bytes = match to_bytes(body, maximum_body_bytes).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error(
                413,
                "request_too_large",
                "Request body exceeds the configured limit or cannot be read",
            );
        }
    };
    let (headers, bytes) = match decode_body(bytes, parts.headers, maximum_body_bytes) {
        Ok(decoded) => decoded,
        Err(response) => return *response,
    };
    let prepared = match state.execution.prepare_execution(client).await {
        Ok(prepared) => prepared,
        Err(failure) => return gateway_error(&failure),
    };
    let uri = parts.uri;
    let (_, _, stream) = match parse_operation(bytes.clone(), &headers, count, uri.query()) {
        Ok(parsed) => parsed,
        Err(response) => return *response,
    };
    let input = RequestInput {
        endpoint: uri.path().to_owned(),
        protocol: PROTOCOL.into(),
        headers: encode_headers(&headers),
        body: bytes,
    };
    let result = invoke_request(
        prepared,
        input,
        Box::new(move |prepared, request| {
            Box::pin(async move {
                let (protocol, headers, bytes) = request_parts(request)?;
                if protocol != PROTOCOL {
                    return Err(MiddlewareError::Rejected);
                }
                let maximum_body_bytes = prepared
                    .client()
                    .snapshot()
                    .responses_max_decompressed_body_bytes();
                let (headers, bytes) = match decode_body(bytes, headers, maximum_body_bytes) {
                    Ok(decoded) => decoded,
                    Err(response) => return buffered_response(PROTOCOL, *response).await,
                };
                execute_native(state, prepared, headers, bytes, count, stream, uri).await
            })
        }),
    )
    .await;
    match result {
        Ok(response) => {
            into_http_response(
                response,
                if stream {
                    ExpectedBody::Sse
                } else {
                    ExpectedBody::SingleJson
                },
                PROTOCOL,
                middleware_error,
            )
            .await
        }
        Err(failure) => middleware_error(failure),
    }
}

fn parse_operation(
    bytes: Bytes,
    headers: &HeaderMap,
    count: bool,
    raw_query: Option<&str>,
) -> Result<(Operation, UpstreamModelId, bool), Box<Response>> {
    let object: Map<String, Value> = match serde_json::from_slice(&bytes) {
        Ok(object) => object,
        Err(_) => {
            return Err(Box::new(error(
                400,
                "invalid_request_error",
                "Request must be a JSON object",
            )));
        }
    };
    let Some(model) = object.get("model").and_then(Value::as_str) else {
        return Err(Box::new(error(
            400,
            "invalid_request_error",
            "model is required",
        )));
    };
    let upstream_model = match UpstreamModelId::new(model) {
        Ok(model) => model,
        Err(_) => {
            return Err(Box::new(error(
                400,
                "invalid_request_error",
                "Invalid model",
            )));
        }
    };
    let stream = !count && object.get("stream").and_then(Value::as_bool) == Some(true);
    if !count
        && object
            .get("max_tokens")
            .and_then(Value::as_u64)
            .is_none_or(|value| value == 0)
    {
        return Err(Box::new(error(
            400,
            "invalid_request_error",
            "max_tokens must be a positive integer",
        )));
    }
    let mut context = Map::new();
    if let Some(query) = raw_query {
        context.insert("raw_query".into(), json!(query));
    }
    if let Some(thread) = object.get("thread") {
        let valid = !count
            && thread.is_object()
            && match thread["type"].as_str() {
                Some("create") => thread.get("previous_message_id").is_none(),
                Some("continue") => thread["previous_message_id"]
                    .as_str()
                    .is_some_and(valid_message_id),
                _ => false,
            };
        if !valid {
            return Err(Box::new(error(
                400,
                "invalid_request_error",
                "Invalid Messages Thread",
            )));
        }
    }
    for name in headers.keys().map(HeaderName::as_str).filter(|name| {
        name.starts_with("anthropic-")
            || name.starts_with("x-claude-code-")
            || name.starts_with("x-stainless-")
    }) {
        let values = headers
            .get_all(name)
            .iter()
            .map(|value| value.to_str())
            .collect::<Result<Vec<_>, _>>();
        let values = match values {
            Ok(values) => values,
            Err(_) => {
                return Err(Box::new(error(
                    400,
                    "invalid_request_error",
                    "Invalid protocol header",
                )));
            }
        };
        if name != "anthropic-beta" && values.len() > 1 {
            return Err(Box::new(error(
                400,
                "invalid_request_error",
                "Duplicate protocol header",
            )));
        }
        if !values.is_empty() {
            context.insert(name.to_owned(), json!(values.join(",")));
        }
    }
    if let Some(session) = context.get("x-claude-code-session-id").cloned() {
        context.insert("session_id".into(), session);
    }
    // 新客户端的 JSON user_id 和旧版会话后缀只提供显式 session 事实，不作为租户身份。
    if !context.contains_key("session_id")
        && let Some(user) = object
            .get("metadata")
            .and_then(|metadata| metadata.get("user_id"))
            .and_then(Value::as_str)
        && let Ok(user) = serde_json::from_str::<Value>(user)
        && let Some(session) = user["session_id"]
            .as_str()
            .filter(|session| !session.is_empty() && session.len() <= 256)
    {
        context.insert("session_id".into(), json!(session));
    }
    if !context.contains_key("session_id")
        && let Some(user) = object
            .get("metadata")
            .and_then(|metadata| metadata.get("user_id"))
            .and_then(Value::as_str)
        && let Some((_, session)) = user.rsplit_once("_session_")
        && session.len() == 36
        && session.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        context.insert("session_id".into(), json!(session));
    }
    // 身份字段仅供 Provider 的画像策略验证和选择，最终发送身份由同一请求快照组装。
    for name in ["x-app", "user-agent"] {
        let values: Vec<_> = headers.get_all(name).iter().collect();
        if values.len() > 1 {
            return Err(Box::new(error(
                400,
                "invalid_request_error",
                "Duplicate client header",
            )));
        }
        if let Some(value) = values.first().and_then(|value| value.to_str().ok()) {
            let valid = if name == "x-app" {
                value == "cli"
            } else {
                official_user_agent(value)
            };
            if valid {
                context.insert(name.into(), json!(value));
            }
        }
    }
    let operation = if count {
        Operation::CountTokens(TokenCountRequest::from_raw_json(
            RawJsonPayload::new(PROTOCOL, bytes)
                .expect("protocol")
                .with_context(context),
        ))
    } else {
        let requirements = requirements(&object);
        Operation::Generate(
            GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object(PROTOCOL, object)
                    .expect("protocol")
                    .with_context(context),
            )
            .with_native_requirements(requirements),
        )
    };
    Ok((operation, upstream_model, stream))
}

fn official_user_agent(value: &str) -> bool {
    let Some((version, entry)) = value
        .strip_prefix("claude-cli/")
        .and_then(|value| value.split_once(" (external, "))
    else {
        return false;
    };
    !version.is_empty()
        && version.len() <= 32
        && version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && entry
            .strip_suffix(')')
            .is_some_and(|entry| matches!(entry, "cli" | "cli-bg" | "vscode" | "native"))
}

fn valid_message_id(id: &str) -> bool {
    id.strip_prefix("msg_").is_some_and(|id| {
        !id.is_empty()
            && id.len() <= 252
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    })
}

async fn execute_native(
    state: ApiState,
    prepared: PreparedRootExecution,
    headers: HeaderMap,
    bytes: Bytes,
    count: bool,
    expected_stream: bool,
    uri: Uri,
) -> Result<MiddlewareResponse, MiddlewareError> {
    let (operation, upstream_model, stream) =
        match parse_operation(bytes, &headers, count, uri.query()) {
            Ok(parsed) => parsed,
            Err(response) => return buffered_response(PROTOCOL, *response).await,
        };
    // 协议解码结果须遵循入口选定的 HTTP 传输合同。
    if stream != expected_stream {
        return Err(MiddlewareError::Rejected);
    }
    let previous_response_id = match &operation {
        Operation::Generate(generate) => generate
            .protocol_payload()
            .body()
            .get("thread")
            .filter(|thread| thread["type"] == "continue")
            .and_then(|thread| thread["previous_message_id"].as_str())
            .map(|id| gateway_core::engine::continuation::PreviousResponseId::new(id.to_owned())),
        _ => None,
    };
    let metadata = ExecutionRequestMetadata {
        protocol: PROTOCOL.into(),
        endpoint: uri.path().to_owned(),
        transport: if stream {
            ClientTransport::HttpSse
        } else {
            ClientTransport::HttpJson
        },
        stream,
        client_ip: None,
        user_agent: None,
        previous_response_id,
    };
    let started = state
        .execution
        .start_prepared_provider_endpoint(
            prepared,
            ProviderKind::new(PROTOCOL).expect("provider"),
            Some(upstream_model),
            operation,
            metadata,
        )
        .await;
    match started {
        Ok(started) => deliver(started, stream).await,
        Err(failure) => buffered_response(PROTOCOL, gateway_error(&failure)).await,
    }
}

fn requirements(body: &Map<String, Value>) -> CapabilityRequirements {
    fn has_type(value: &Value, expected: &str) -> bool {
        match value {
            Value::Object(object) => {
                object.get("type").and_then(Value::as_str) == Some(expected)
                    || object.values().any(|value| has_type(value, expected))
            }
            Value::Array(values) => values.iter().any(|value| has_type(value, expected)),
            _ => false,
        }
    }
    let mut result = CapabilityRequirements::new(OperationKind::Generate)
        .with_requested_output_tokens(body.get("max_tokens").and_then(Value::as_u64));
    if body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
    {
        result = result.require(Feature::Tools);
    }
    if body
        .get("messages")
        .is_some_and(|value| has_type(value, "image"))
    {
        result = result.require(Feature::Vision);
    }
    if body
        .get("thinking")
        .is_some_and(|value| value["type"] != "disabled")
        || body.get("messages").is_some_and(|value| {
            has_type(value, "thinking") || has_type(value, "redacted_thinking")
        })
    {
        result = result.require(Feature::Reasoning);
    }
    if body.contains_key("output_format")
        || body
            .get("output_config")
            .is_some_and(|value| value.get("format").is_some())
    {
        result = result.require(Feature::JsonSchema);
    }
    result
}

async fn catalog(state: &ApiState, headers: &HeaderMap) -> Result<Vec<Value>, Box<Response>> {
    let client = authenticate(state, headers, true).await?;
    let context = crate::middleware::current();
    let models = state
        .execution
        .client_model_catalog(
            &client,
            PROTOCOL,
            "",
            context.as_ref().and_then(|ctx| ctx.extensions.peer_route()),
        )
        .await
        .map_err(|_| Box::new(error(503, "api_error", "Model catalog unavailable")))?;
    models
        .into_iter()
        .filter_map(|model| match model {
            PublicModelDescriptor::Native { payload, .. } if payload.protocol() == PROTOCOL => {
                Some(
                    serde_json::from_slice(payload.body())
                        .map_err(|_| Box::new(error(502, "api_error", "Invalid model catalog"))),
                )
            }
            _ => None,
        })
        .collect()
}

async fn models(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    let mut models = match catalog(&state, &headers).await {
        Ok(models) => models,
        Err(response) => return *response,
    };
    let limit = match query.get("limit").map(|value| value.parse::<usize>()) {
        None => 20,
        Some(Ok(value)) if (1..=1000).contains(&value) => value,
        _ => {
            return error(
                400,
                "invalid_request_error",
                "limit must be between 1 and 1000",
            );
        }
    };
    if query.contains_key("before_id") && query.contains_key("after_id") {
        return error(
            400,
            "invalid_request_error",
            "Use only one pagination cursor",
        );
    }
    for (name, before) in [("before_id", true), ("after_id", false)] {
        if let Some(cursor) = query.get(name) {
            let Some(index) = models
                .iter()
                .position(|model| model["id"].as_str() == Some(cursor))
            else {
                return error(400, "invalid_request_error", "Unknown pagination cursor");
            };
            models = if before {
                models[..index].to_vec()
            } else {
                models[index + 1..].to_vec()
            };
        }
    }
    let has_more = models.len() > limit;
    // 反向分页取紧邻游标的上一页；has_more 表示该方向仍有更早的记录。
    if query.contains_key("before_id") && has_more {
        models.drain(..models.len() - limit);
    } else {
        models.truncate(limit);
    }
    Json(json!({"first_id":models.first().map(|model| &model["id"]),"last_id":models.last().map(|model| &model["id"]),"has_more":has_more,"data":models})).into_response()
}

async fn model(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    match catalog(&state, &headers).await {
        Ok(models) => models
            .into_iter()
            .find(|model| model["id"].as_str() == Some(&id))
            .map(|model| Json(model).into_response())
            .unwrap_or_else(|| error(404, "not_found_error", "Model not found")),
        Err(response) => *response,
    }
}

fn wire(events: Vec<ProviderEvent>, stream: bool) -> Result<Option<Bytes>, ()> {
    let mut bytes = Vec::new();
    for event in events {
        let (_, payload) = event.into_parts();
        if let Some(payload) = payload {
            if payload.protocol() != PROTOCOL {
                return Err(());
            }
            let chunk = if stream {
                payload.raw_sse_frame().cloned()
            } else {
                payload.into_raw_json_body()
            }
            .ok_or(())?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BODY_BYTES {
                return Err(());
            }
            bytes.extend_from_slice(&chunk);
        }
    }
    Ok((!bytes.is_empty()).then(|| Bytes::from(bytes)))
}

async fn deliver(
    started: StartedExecution,
    stream: bool,
) -> Result<MiddlewareResponse, MiddlewareError> {
    let mut execution = PendingExecution::new(started.session);
    if !stream {
        let events = match execution
            .session_mut()
            .expect("session")
            .collect_uncommitted()
            .await
        {
            Ok(events) => events,
            Err(failure) => {
                return buffered_response(
                    PROTOCOL,
                    execution
                        .record_response_status(engine_error(&failure))
                        .await,
                )
                .await;
            }
        };
        let bytes = wire(events, false)
            .map_err(|_| MiddlewareError::InvalidState)?
            .ok_or(MiddlewareError::InvalidState)?;
        let session = execution.session_mut().expect("session");
        let status = session.response_status_code().unwrap_or(200);
        let mut headers = encode_headers(
            native_response(Bytes::new(), session.response_headers(), false).headers(),
        );
        headers.push(gateway_core::engine::middleware::RequestHeader::new(
            "x-gateway-request-id",
            Bytes::copy_from_slice(started.request_id.as_str().as_bytes()),
        ));
        return Ok(pending_execution_response(
            PROTOCOL.into(),
            status,
            headers,
            MiddlewareFrame::new(bytes, MiddlewareFraming::JsonDocument, true),
            execution,
        ));
    }
    // 首个 wire 以前可以保留上游原始 HTTP 错误；返回包装全部完成以后才提交
    let first = loop {
        match execution.session_mut().expect("session").next_event().await {
            Ok(Some(event)) => {
                let must_deliver = event.commit_requirement()
                    == gateway_core::engine::CommitRequirement::CommitBeforeDelivery;
                match wire(event.into_provider_events(), true)
                    .map_err(|_| MiddlewareError::InvalidState)?
                {
                    Some(bytes) => break bytes,
                    None if !must_deliver => {}
                    None => return Err(MiddlewareError::InvalidState),
                }
            }
            Ok(None) => return Err(MiddlewareError::InvalidState),
            Err(mut failure) => {
                if let Some(bytes) = atomic_wire(&mut failure) {
                    let headers = encode_headers(
                        native_response(
                            Bytes::new(),
                            execution.session_mut().expect("session").response_headers(),
                            true,
                        )
                        .headers(),
                    );
                    return Ok(MiddlewareResponse::new(
                        PROTOCOL.into(),
                        200,
                        headers,
                        Box::new(NativeStreamBody {
                            execution,
                            first: Some(bytes),
                            pending_terminal: None,
                            ended: true,
                            status: 200,
                        }),
                    ));
                }
                return buffered_response(
                    PROTOCOL,
                    execution
                        .record_response_status(engine_error(&failure))
                        .await,
                )
                .await;
            }
        }
    };
    let session = execution.session_mut().expect("session");
    let status = session.response_status_code().unwrap_or(200);
    let mut headers =
        encode_headers(native_response(Bytes::new(), session.response_headers(), true).headers());
    headers.push(gateway_core::engine::middleware::RequestHeader::new(
        "x-gateway-request-id",
        Bytes::copy_from_slice(started.request_id.as_str().as_bytes()),
    ));
    Ok(MiddlewareResponse::new(
        PROTOCOL.into(),
        status,
        headers,
        Box::new(NativeStreamBody {
            execution,
            first: Some(first),
            pending_terminal: None,
            ended: false,
            status,
        }),
    ))
}

struct NativeStreamBody {
    execution: PendingExecution,
    first: Option<Bytes>,
    pending_terminal: Option<Bytes>,
    ended: bool,
    status: u16,
}
impl MiddlewareBody for NativeStreamBody {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<MiddlewareFrame>, MiddlewareError>> {
        Box::pin(async move {
            if let Some(first) = self.first.take() {
                return Ok(Some(MiddlewareFrame::new(
                    first,
                    MiddlewareFraming::SseEvent,
                    self.ended,
                )));
            }
            if self.ended {
                return Ok(None);
            }
            loop {
                match self
                    .execution
                    .session_mut()
                    .expect("session")
                    .next_event()
                    .await
                {
                    Ok(Some(event)) => {
                        if self.pending_terminal.is_some() {
                            return Err(MiddlewareError::InvalidState);
                        }
                        if event.commit_requirement()
                            == gateway_core::engine::CommitRequirement::CommitBeforeDelivery
                        {
                            self.execution
                                .session_mut()
                                .expect("session")
                                .commit_downstream(Some(self.status))
                                .await?;
                        }
                        let events = event.into_provider_events();
                        let completed = events
                            .iter()
                            .flat_map(ProviderEvent::canonical_facts)
                            .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
                        let bytes =
                            wire(events, true).map_err(|_| MiddlewareError::InvalidState)?;
                        if let Some(bytes) = bytes {
                            if completed {
                                // message_stop 必须等 Core 确认 EOF、结算和释放后再作为终帧交付。
                                // 保存在正文状态内，取消一次读取等待也不能丢失终帧。
                                self.pending_terminal = Some(bytes);
                                continue;
                            }
                            let terminal = self.execution.is_finalized();
                            if terminal {
                                self.ended = true;
                                self.execution.disarm();
                            }
                            return Ok(Some(MiddlewareFrame::new(
                                bytes,
                                MiddlewareFraming::SseEvent,
                                terminal,
                            )));
                        }
                    }
                    Ok(None) => {
                        return if self.execution.is_finalized() {
                            let bytes = self
                                .pending_terminal
                                .take()
                                .ok_or(MiddlewareError::InvalidState)?;
                            self.ended = true;
                            self.execution.disarm();
                            Ok(Some(MiddlewareFrame::new(
                                bytes,
                                MiddlewareFraming::SseEvent,
                                true,
                            )))
                        } else {
                            Err(MiddlewareError::InvalidState)
                        };
                    }
                    Err(mut failure) => {
                        let raw = atomic_wire(&mut failure);
                        // 终结确认失败时只交付失败，不能把尚未发出的成功终帧混入响应。
                        self.pending_terminal = None;
                        self.ended = true;
                        let bytes = raw.unwrap_or_else(|| {
                            let gateway =
                                gateway_core::engine::execution::gateway_error_from_engine(
                                    &failure,
                                );
                            let (_, document) = gateway_error_document(&gateway);
                            Bytes::from(format!("event: error\ndata: {document}\n\n"))
                        });
                        return Ok(Some(MiddlewareFrame::new(
                            bytes,
                            MiddlewareFraming::SseEvent,
                            true,
                        )));
                    }
                }
            }
        })
    }
    fn commit_downstream(
        &mut self,
        status: Option<u16>,
    ) -> BoxFuture<'_, Result<(), MiddlewareError>> {
        Box::pin(async move {
            if self.ended {
                // 首事件即原生 SSE 错误时，记录交付状态并终结失败，不能把 HTTP 200 当作推理成功
                if let (Some(session), Some(status)) = (self.execution.session_mut(), status) {
                    session.record_client_status(status).await?;
                }
                self.execution.cancel_and_finalize().await;
            } else if let Some(session) = self.execution.session_mut() {
                session.commit_downstream(status).await?;
            }
            Ok(())
        })
    }
    fn record_client_status(&mut self, status: u16) -> BoxFuture<'_, Result<(), MiddlewareError>> {
        Box::pin(async move {
            if let Some(session) = self.execution.session_mut() {
                session.record_client_status(status).await?;
            }
            Ok(())
        })
    }
    fn is_finalized(&self) -> bool {
        self.execution.is_finalized()
    }
    fn close(mut self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            self.execution.cancel_and_finalize().await;
        })
    }
}

fn atomic_wire(failure: &mut EngineError) -> Option<Bytes> {
    if let EngineError::Provider(provider) = failure {
        wire(provider.take_atomic_client_events(), true)
            .ok()
            .flatten()
    } else {
        None
    }
}

fn middleware_error(failure: MiddlewareError) -> Response {
    match failure {
        MiddlewareError::Gateway(failure) => gateway_error(&failure),
        MiddlewareError::Engine(failure) => engine_error(&failure),
        MiddlewareError::Provider(failure) => engine_error(&EngineError::Provider(failure)),
        MiddlewareError::Rejected => error(403, "permission_error", "Request protocol is invalid"),
        _ => error(502, "api_error", "Request middleware failed"),
    }
}

fn native_response(bytes: Bytes, headers: &[ProviderResponseHeader], stream: bool) -> Response {
    let mut response = Response::new(Body::from(bytes));
    for header in headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(header.name().as_bytes()),
            HeaderValue::from_bytes(header.value()),
        ) {
            response.headers_mut().append(name, value);
        }
    }
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(if stream {
            "text/event-stream"
        } else {
            "application/json"
        }),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn engine_error(failure: &EngineError) -> Response {
    if let EngineError::Provider(provider) = failure
        && let Some(upstream) = provider.client_visible_upstream_response()
    {
        let mut response = native_response(upstream.body().clone(), upstream.headers(), false);
        *response.status_mut() = StatusCode::from_u16(upstream.status()).expect("upstream status");
        if let Some(value) = upstream
            .content_type()
            .and_then(|value| HeaderValue::from_bytes(value).ok())
        {
            response.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        return response;
    }
    gateway_error(&gateway_core::engine::execution::gateway_error_from_engine(
        failure,
    ))
}

fn gateway_error(failure: &GatewayError) -> Response {
    let (status, document) = gateway_error_document(failure);
    (
        StatusCode::from_u16(status).expect("HTTP status"),
        Json(document),
    )
        .into_response()
}

fn gateway_error_document(failure: &GatewayError) -> (u16, Value) {
    let (status, kind) = match failure.kind() {
        GatewayErrorKind::Unauthorized => (401, "authentication_error"),
        GatewayErrorKind::PolicyDenied => (403, "permission_error"),
        GatewayErrorKind::ModelNotFound => (404, "not_found_error"),
        GatewayErrorKind::InvalidRequest | GatewayErrorKind::Unsupported => {
            (400, "invalid_request_error")
        }
        GatewayErrorKind::MessageTooBig => (413, "request_too_large"),
        GatewayErrorKind::RateLimited => (429, "rate_limit_error"),
        GatewayErrorKind::Timeout => (504, "api_error"),
        _ => (503, "api_error"),
    };
    let local = failure.client_error_code().or_else(|| {
        failure
            .diagnostic()
            .filter(|diagnostic| {
                matches!(diagnostic.stage(), Some("anthropic_local" | "continuation"))
            })
            .and_then(|diagnostic| diagnostic.code())
    });
    let status = match local {
        Some(
            "cprs_thread_configuration_changed"
            | "cprs_thread_protocol_changed"
            | "cprs_thread_identity_changed"
            | "cprs_thread_state_unavailable",
        ) => 409,
        Some(
            "cprs_thread_store_unavailable"
            | "cprs_identity_unavailable"
            | "cprs_account_identity_unavailable",
        ) => 503,
        Some("thread_not_found") => 404,
        _ => status,
    };
    let kind = match status {
        409 => "invalid_request_error",
        503 => "api_error",
        _ => kind,
    };
    let mut document =
        json!({"type":"error","error":{"type":kind,"message":failure.safe_message()}});
    if let Some(code) = local {
        document["error"]["details"] = json!({"error_code":code});
    }
    (status, document)
}

fn error(status: u16, kind: &str, message: &str) -> Response {
    (
        StatusCode::from_u16(status).expect("HTTP status"),
        Json(json!({"type":"error","error":{"type":kind,"message":message}})),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
