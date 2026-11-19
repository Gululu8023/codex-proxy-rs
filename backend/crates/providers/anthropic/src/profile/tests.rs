//! 身份跟随与预设独立：跟随必须完整校验，预设不能被入站 UA 覆盖。

use super::*;
use reqwest::header::{HeaderMap, HeaderValue};

fn preset() -> Effective {
    Effective {
        selection: Selection::default(),
        selection_fingerprint: "fixture".into(),
        user_agent: "claude-cli/2.1.321 (external, cli)".into(),
        version: "2.1.321".into(),
        entry: "cli".into(),
        os: "MacOS".into(),
        arch: "arm64".into(),
        sdk_version: SDK_VERSION.into(),
        runtime_version: RUNTIME_VERSION.into(),
        template: TEMPLATE.into(),
        version_source: "fixture".into(),
        verified_at: None,
    }
}

fn identity(entry: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("x-app", "cli"),
        ("x-stainless-lang", "js"),
        ("x-stainless-runtime", "node"),
        ("x-stainless-package-version", "0.128.0"),
        ("x-stainless-runtime-version", "v26.12.1"),
        ("x-stainless-os", "Linux"),
        ("x-stainless-arch", "x64"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.insert(
        "user-agent",
        format!("claude-cli/2.1.322 (external, {entry})")
            .parse()
            .unwrap(),
    );
    headers
}

#[test]
fn follow_requires_a_complete_identity_and_preserves_official_entries() {
    for entry in ["cli", "cli-bg", "vscode", "native"] {
        let mut profile = preset();
        profile.selection.mode = "follow".into();
        let headers = identity(entry);
        profile.follow(&headers).unwrap();
        let outbound = profile.headers(HeaderMap::new(), None, false, 0);
        assert_eq!(outbound["user-agent"], headers["user-agent"]);
        assert_eq!(outbound["x-stainless-os"], "Linux");
        assert_eq!(profile.entry, entry);
        let mut incomplete = headers;
        incomplete.remove("x-stainless-runtime-version");
        assert!(profile.follow(&incomplete).is_err());
    }
    for agent in [
        "claude-cli/2.1 (external, cli)",
        "claude-cli/2.1.322 (external, unknown)",
        "untrusted browser",
    ] {
        let mut profile = preset();
        profile.selection.mode = "follow".into();
        let mut headers = identity("cli");
        headers.insert("user-agent", agent.parse().unwrap());
        assert!(profile.follow(&headers).is_err());
    }
}

#[test]
fn preset_replaces_inbound_identity_without_requiring_follow_headers() {
    let mut profile = preset();
    profile.follow(&HeaderMap::new()).unwrap();
    let incoming = identity("vscode");
    profile.follow(&incoming).unwrap();
    let outgoing = profile.headers(incoming, None, false, 0);
    assert_eq!(outgoing["user-agent"], "claude-cli/2.1.321 (external, cli)");
    assert_eq!(outgoing["x-stainless-os"], "MacOS");
    assert_eq!(outgoing["x-stainless-arch"], "arm64");
}
