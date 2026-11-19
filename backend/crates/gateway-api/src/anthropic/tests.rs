//! Claude 原生入口校验及 SSE 终态交付、取消清理的回归覆盖。

use super::*;
use gateway_core::{
    engine::{CommitRequirement, CoordinatedEvent, ModelRequestId, execution::ExecutionSession},
    event::{GatewayEvent, ProtocolWireEvent, ResponseMeta},
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::SystemTime,
};

#[derive(Default)]
struct StreamState {
    commits: AtomicUsize,
    cancellations: AtomicUsize,
    finalizations: AtomicUsize,
}

struct StreamSession {
    events: VecDeque<Result<CoordinatedEvent, EngineError>>,
    state: Arc<StreamState>,
    finalized: bool,
    pending_commit: bool,
    eof_gate: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl ExecutionSession for StreamSession {
    fn next_event(&mut self) -> BoxFuture<'_, Result<Option<CoordinatedEvent>, EngineError>> {
        Box::pin(async move {
            assert!(
                !self.pending_commit,
                "first event must be committed before advancing"
            );
            if let Some(event) = self.events.pop_front() {
                match event {
                    Ok(event) => {
                        self.pending_commit =
                            event.commit_requirement() == CommitRequirement::CommitBeforeDelivery;
                        return Ok(Some(event));
                    }
                    Err(error) => {
                        self.finalized = true;
                        self.state.finalizations.fetch_add(1, Ordering::SeqCst);
                        return Err(error);
                    }
                }
            }
            if let Some(gate) = self.eof_gate.as_mut() {
                let _ = gate.await;
                self.eof_gate = None;
            }
            // 与真实 Core 一致：Completed 交付后，继续读取 EOF 才完成结算与租约释放。
            self.finalized = true;
            self.state.finalizations.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        })
    }

    fn collect_uncommitted(&mut self) -> BoxFuture<'_, Result<Vec<ProviderEvent>, EngineError>> {
        Box::pin(async { panic!("streaming session") })
    }

    fn response_headers(&self) -> &[ProviderResponseHeader] {
        &[]
    }

    fn commit_downstream(&mut self, _: Option<u16>) -> BoxFuture<'_, Result<(), EngineError>> {
        Box::pin(async move {
            assert!(self.pending_commit);
            self.pending_commit = false;
            self.state.commits.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn record_client_status(&mut self, _: u16) -> BoxFuture<'_, Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    fn is_finalized(&self) -> bool {
        self.finalized
    }

    fn cancel(&self) {
        self.state.cancellations.fetch_add(1, Ordering::SeqCst);
    }

    fn detach_finalize(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            self.state.finalizations.fetch_add(1, Ordering::SeqCst);
        })
    }
}

fn stream_event(kind: &str) -> CoordinatedEvent {
    let meta = ResponseMeta::new("msg_fixture", "claude-sonnet-4-5");
    let (facts, commit) = match kind {
        "message_start" => (
            vec![GatewayEvent::Started(meta)],
            CommitRequirement::CommitBeforeDelivery,
        ),
        "message_stop" => (
            vec![GatewayEvent::Completed(meta)],
            CommitRequirement::AlreadyCommitted,
        ),
        _ => (vec![], CommitRequirement::AlreadyCommitted),
    };
    CoordinatedEvent::single(
        ProviderEvent::canonical_with_wire(
            facts,
            ProtocolWireEvent::raw_sse(
                PROTOCOL,
                Bytes::from(format!("event: {kind}\ndata: {{\"type\":\"{kind}\"}}\n\n")),
            )
            .unwrap(),
        ),
        commit,
    )
}

fn stream_execution(
    events: Vec<Result<CoordinatedEvent, EngineError>>,
    eof_gate: Option<tokio::sync::oneshot::Receiver<()>>,
) -> (StartedExecution, Arc<StreamState>) {
    let state = Arc::new(StreamState::default());
    let started = StartedExecution {
        request_id: ModelRequestId::new("req_anthropic_stream").unwrap(),
        created_at: SystemTime::now(),
        stream: true,
        session: Box::new(StreamSession {
            events: events.into(),
            state: state.clone(),
            finalized: false,
            pending_commit: false,
            eof_gate,
        }),
    };
    (started, state)
}

#[tokio::test]
async fn anthropic_stream_finishes_http_body_after_message_stop() {
    let (started, state) = stream_execution(
        vec![
            Ok(stream_event("message_start")),
            Ok(stream_event("content_block_delta")),
            Ok(stream_event("message_stop")),
        ],
        None,
    );
    let response = into_http_response(
        deliver(started, true).await.unwrap(),
        ExpectedBody::Sse,
        PROTOCOL,
        middleware_error,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 8192)
        .await
        .expect("normal SSE must reach HTTP EOF");
    let text = std::str::from_utf8(&bytes).unwrap();
    assert_eq!(text.matches("event: message_stop").count(), 1);
    assert!(!text.contains("event: error"));
    assert!(!text.contains("[DONE]"));
    assert_eq!(state.commits.load(Ordering::SeqCst), 1);
    assert_eq!(state.finalizations.load(Ordering::SeqCst), 1);
    assert_eq!(state.cancellations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn anthropic_stream_delivers_failure_without_a_success_terminal() {
    use gateway_core::{
        error::{ProviderError, ProviderErrorKind},
        upstream::UpstreamSendState,
    };

    let raw = Bytes::from_static(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"fixture\"}}\n\n");
    for after_stop in [false, true] {
        let mut events = vec![Ok(stream_event("message_start"))];
        if after_stop {
            events.push(Ok(stream_event("message_stop")));
        }
        events.push(Err(EngineError::Provider(
            ProviderError::new(ProviderErrorKind::Unavailable, UpstreamSendState::Sent)
                .with_atomic_client_events(vec![ProviderEvent::wire(
                    ProtocolWireEvent::raw_sse(PROTOCOL, raw.clone()).unwrap(),
                )]),
        )));
        let (started, state) = stream_execution(events, None);
        let response = into_http_response(
            deliver(started, true).await.unwrap(),
            ExpectedBody::Sse,
            PROTOCOL,
            middleware_error,
        )
        .await;
        let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(bytes.ends_with(&raw));
        assert_eq!(text.matches("event: error").count(), 1);
        assert!(!text.contains("event: message_stop"));
        assert_eq!(state.finalizations.load(Ordering::SeqCst), 1);
        assert_eq!(state.cancellations.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn anthropic_stream_terminal_wait_preserves_frame_and_cleanup() {
    use futures::FutureExt;

    for resume in [false, true] {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (started, state) = stream_execution(
            vec![
                Ok(stream_event("message_start")),
                Ok(stream_event("message_stop")),
            ],
            Some(rx),
        );
        let (_, _, _, mut body) = deliver(started, true).await.unwrap().into_parts();
        assert!(!body.next_frame().await.unwrap().unwrap().terminal());
        body.commit_downstream(Some(200)).await.unwrap();
        // 取消一次等待不能丢掉已取出的 message_stop，也不能提前解除清理责任。
        assert!(body.next_frame().now_or_never().is_none());
        assert!(!body.is_finalized());
        assert_eq!(state.finalizations.load(Ordering::SeqCst), 0);
        if resume {
            tx.send(()).unwrap();
            let frame = body.next_frame().await.unwrap().unwrap();
            assert!(frame.terminal());
            assert_eq!(
                frame.bytes(),
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".as_slice()
            );
            assert!(body.is_finalized());
            assert!(body.next_frame().await.unwrap().is_none());
        }
        body.close().await;
        assert_eq!(state.commits.load(Ordering::SeqCst), 1);
        assert_eq!(state.finalizations.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.cancellations.load(Ordering::SeqCst),
            usize::from(!resume)
        );
    }
}

#[tokio::test]
async fn anthropic_stream_missing_message_stop_is_not_success() {
    let (started, _) = stream_execution(vec![Ok(stream_event("message_start"))], None);
    let response = into_http_response(
        deliver(started, true).await.unwrap(),
        ExpectedBody::Sse,
        PROTOCOL,
        middleware_error,
    )
    .await;
    assert!(to_bytes(response.into_body(), 8192).await.is_err());
}

#[test]
fn key_headers_must_be_unambiguous() {
    let mut headers = HeaderMap::new();
    assert_eq!(client_key(&headers).unwrap_err().status(), 401);
    headers.insert("x-api-key", "sk_fixture".parse().unwrap());
    assert_eq!(client_key(&headers).unwrap(), "sk_fixture");
    headers.append("x-api-key", "sk_other".parse().unwrap());
    assert_eq!(client_key(&headers).unwrap_err().status(), 401);
    headers.remove("x-api-key");
    headers.insert(header::AUTHORIZATION, "Bearer sk_fixture".parse().unwrap());
    assert_eq!(client_key(&headers).unwrap(), "sk_fixture");
    headers.insert("x-api-key", "sk_other".parse().unwrap());
    assert_eq!(client_key(&headers).unwrap_err().status(), 401);
}

#[test]
fn minimum_version_uses_original_claude_cli_user_agent() {
    let minimum = ClaudeCodeVersion::parse("2.1.0").unwrap();
    for (ua, permitted) in [
        ("claude-cli/2.1.0", true),
        ("claude-cli/2.0.9", false),
        ("claude-cli/invalid", false),
        ("custom-sdk/1.0", true),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(header::USER_AGENT, ua.parse().unwrap());
        assert_eq!(
            enforce_claude_code_version(&headers, &minimum).is_ok(),
            permitted,
            "{ua}"
        );
    }
    let mut headers = HeaderMap::new();
    headers.append(header::USER_AGENT, "claude-cli/2.1.0".parse().unwrap());
    headers.append(header::USER_AGENT, "claude-cli/2.0.0".parse().unwrap());
    assert!(enforce_claude_code_version(&headers, &minimum).is_err());
}

#[test]
fn decompression_limit_applies_to_expanded_body_and_removes_wire_headers() {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&vec![b'a'; 4096]).unwrap();
    let compressed = Bytes::from(encoder.finish().unwrap());
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
    headers.insert(
        header::CONTENT_LENGTH,
        compressed.len().to_string().parse().unwrap(),
    );
    assert_eq!(
        decode_body(compressed.clone(), headers.clone(), 100)
            .unwrap_err()
            .status(),
        413
    );
    let (headers, body) = decode_body(compressed, headers, 4096).unwrap();
    assert_eq!(body.len(), 4096);
    assert!(!headers.contains_key(header::CONTENT_ENCODING));
    assert!(!headers.contains_key(header::CONTENT_LENGTH));
}

#[test]
fn invalid_thread_contracts_fail_before_execution() {
    for (thread, count) in [
        (json!({"type":"continue"}), false),
        (
            json!({"type":"continue","previous_message_id":"wrong-id"}),
            false,
        ),
        (
            json!({"type":"create","previous_message_id":"msg_previous"}),
            false,
        ),
        (json!({"type":"create"}), true),
        (json!("create"), false),
    ] {
        let body =
            json!({"model":"claude-sonnet-4-5","max_tokens":32,"messages":[],"thread":thread});
        assert_eq!(
            parse_operation(
                Bytes::from(body.to_string()),
                &HeaderMap::new(),
                count,
                None
            )
            .unwrap_err()
            .status(),
            400
        );
    }
}
