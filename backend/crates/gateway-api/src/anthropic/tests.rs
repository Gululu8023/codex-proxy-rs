//! Claude 原生入口拒绝歧义凭据、无效线程及超限压缩正文。

use super::*;

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
