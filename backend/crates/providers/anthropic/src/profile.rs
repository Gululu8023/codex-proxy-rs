//! Claude Code 身份模板与官方发布缓存；CLI 发布版本不代表 SDK 模板已经重新核验。

use gateway_core::{
    account::OpaqueProviderData,
    provider_ports::{ProviderArtifactProfile, ProviderArtifactProfileCachePort},
    routing::ProviderKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

pub(crate) const TEMPLATE: &str = "cpa-1aaf56bc-claude-2.1.321-sdk-0.128.0-v1";
// 配套请求头属于协议模板，不接受用户版本覆盖，也不随 CLI 发布号独立升级。
const SDK_VERSION: &str = "0.128.0";
const RUNTIME_VERSION: &str = "v26.12.1";
const CHANNELS: [&str; 2] = ["latest", "stable"];

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Selection {
    pub mode: String,
    pub client: String,
    pub platform: String,
    pub arch: String,
    pub version_mode: String,
    pub version: Option<String>,
    pub user_agent: Option<String>,
    pub installation_seed: Option<String>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            mode: "preset".into(),
            client: "cli".into(),
            platform: "macos".into(),
            arch: "arm64".into(),
            version_mode: "latest".into(),
            version: None,
            user_agent: None,
            installation_seed: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Effective {
    pub selection: Selection,
    pub selection_fingerprint: String,
    pub user_agent: String,
    pub version: String,
    pub entry: String,
    pub os: String,
    pub arch: String,
    pub sdk_version: String,
    pub runtime_version: String,
    pub template: String,
    pub version_source: String,
    pub verified_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(crate) fn object(value: &impl Serialize) -> OpaqueProviderData {
    OpaqueProviderData::new(
        serde_json::to_value(value)
            .expect("profile serialization")
            .as_object()
            .expect("profile object")
            .clone(),
    )
}

pub(crate) fn version_sequence(version: &str) -> Option<u64> {
    if version
        .split('.')
        .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let parts: Vec<_> = version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .ok()?;
    if parts.len() != 3 || parts.iter().any(|part| *part >= 1_000_000) {
        return None;
    }
    Some(parts[0] * 1_000_000_000_000 + parts[1] * 1_000_000 + parts[2])
}

pub(crate) fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

pub(crate) fn client_agent(value: &str) -> Option<(&str, &str)> {
    let (version, suffix) = value
        .strip_prefix("claude-cli/")?
        .split_once(" (external, ")?;
    let entry = suffix.strip_suffix(')')?;
    (version_sequence(version).is_some() && matches!(entry, "cli" | "cli-bg" | "vscode" | "native"))
        .then_some((version, entry))
}

impl Selection {
    pub fn parse(configuration: &OpaqueProviderData) -> Result<Self, &'static str> {
        let selection: Self =
            serde_json::from_value(Value::Object(configuration.expose_to_provider().clone()))
                .map_err(|_| "Invalid Claude client profile")?;
        if !matches!(selection.mode.as_str(), "preset" | "custom" | "follow")
            || !matches!(selection.client.as_str(), "cli" | "vscode")
            || !matches!(selection.platform.as_str(), "macos" | "linux" | "windows")
            || !matches!(selection.arch.as_str(), "arm64" | "x64")
            || !matches!(
                selection.version_mode.as_str(),
                "latest" | "stable" | "fixed"
            )
            || selection
                .installation_seed
                .as_deref()
                .is_some_and(|seed| !uuid(seed))
        {
            return Err("Invalid Claude client profile");
        }
        if selection.version_mode == "fixed"
            && selection
                .version
                .as_deref()
                .and_then(version_sequence)
                .is_none()
        {
            return Err("A fixed Claude version is required");
        }
        if selection.mode == "custom" {
            let (version, entry) = selection
                .user_agent
                .as_deref()
                .and_then(client_agent)
                .ok_or("Invalid Claude User-Agent")?;
            if selection.version.as_deref() != Some(version) || selection.client != entry {
                return Err("Claude User-Agent, version and entry must agree");
            }
        } else if selection.user_agent.is_some() {
            return Err("User-Agent is only configurable in custom mode");
        }
        Ok(selection)
    }

    pub fn fingerprint(&self) -> String {
        // 自动发布结果不在选择指纹中；管理员显式配置、安装种子及模板变化仍会失效旧 Thread。
        hex::encode(Sha256::digest(
            serde_json::to_vec(self).expect("selection serialization"),
        ))
    }
}

#[derive(Clone)]
struct Release {
    version: String,
    verified_at: Option<SystemTime>,
    checked_at: Option<SystemTime>,
    error: Option<&'static str>,
}

pub(crate) struct Profiles {
    cache: Arc<dyn ProviderArtifactProfileCachePort>,
    releases: RwLock<BTreeMap<String, Release>>,
}

impl Profiles {
    pub fn new(cache: Arc<dyn ProviderArtifactProfileCachePort>) -> Self {
        // 2026-11-12 官方发布基线；离线使用时明确标为 baseline，不能冒充已同步。
        let releases = [("latest", "2.1.323"), ("stable", "2.1.311")]
            .into_iter()
            .map(|(channel, version)| {
                (
                    channel.into(),
                    Release {
                        version: version.into(),
                        verified_at: None,
                        checked_at: None,
                        error: None,
                    },
                )
            })
            .collect();
        Self {
            cache,
            releases: RwLock::new(releases),
        }
    }

    fn kind() -> ProviderKind {
        ProviderKind::new(crate::NAME).expect("provider")
    }

    pub async fn restore(&self) {
        for channel in CHANNELS {
            if let Err(error) = self.restore_channel(channel).await {
                self.releases
                    .write()
                    .expect("release state")
                    .get_mut(channel)
                    .expect("known channel")
                    .error = Some(error);
            }
        }
    }

    async fn restore_channel(&self, channel: &str) -> Result<(), &'static str> {
        let kind = Self::kind();
        let key = format!("claude-code-{channel}");
        if let Some(cached) = self
            .cache
            .read(&kind, &key)
            .await
            .map_err(|_| "Claude release cache unavailable")?
        {
            let version = cached
                .profile()
                .expose_to_provider()
                .get("version")
                .and_then(Value::as_str)
                .filter(|version| version_sequence(version) == Some(cached.artifact_sequence()))
                .ok_or("Invalid Claude release cache")?;
            let mut releases = self.releases.write().expect("release state");
            let release = releases.get_mut(channel).expect("known release channel");
            if version_sequence(version) >= version_sequence(&release.version)
                || release.verified_at.is_none()
            {
                release.version = version.into();
                release.verified_at = Some(cached.verified_at());
                if release
                    .checked_at
                    .is_none_or(|checked| checked < cached.verified_at())
                {
                    release.checked_at = Some(cached.verified_at());
                    release.error = None;
                }
            }
        }
        Ok(())
    }

    pub async fn refresh(&self) -> Result<(), &'static str> {
        // 官方版本请求沿用部署环境代理；独立客户端不携带账号凭据且禁止重定向。
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "Claude release client unavailable")?;
        let mut failed = false;
        for channel in CHANNELS {
            let result = async {
                let mut response = client
                    .get(format!(
                        "https://downloads.claude.ai/claude-code-releases/{channel}"
                    ))
                    .send()
                    .await
                    .map_err(|_| "Claude release fetch failed")?
                    .error_for_status()
                    .map_err(|_| "Claude release fetch rejected")?;
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| "Claude release body failed")?
                {
                    if bytes.len() + chunk.len() > 64 {
                        return Err("Invalid Claude release version");
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let version = std::str::from_utf8(&bytes)
                    .map_err(|_| "Invalid Claude release version")?
                    .trim();
                let sequence = version_sequence(version).ok_or("Invalid Claude release version")?;
                let profile = ProviderArtifactProfile::new(
                    Self::kind(),
                    format!("claude-code-{channel}"),
                    sequence,
                    SystemTime::now(),
                    object(&json!({"version":version})),
                );
                // 先提交共享缓存，再发布本进程；较旧或并发冲突时采用已提交的值。
                match self
                    .cache
                    .replace_if_newer(profile, Duration::from_secs(90 * 86400))
                    .await
                {
                    Ok(_) => {}
                    Err(error)
                        if error.kind()
                            == gateway_core::provider_ports::ProviderStoreErrorKind::Conflict => {}
                    Err(_) => return Err("Claude release cache write failed"),
                }
                self.restore_channel(channel).await
            }
            .await;
            let mut releases = self.releases.write().expect("release state");
            let release = releases.get_mut(channel).expect("known release channel");
            release.checked_at = Some(SystemTime::now());
            release.error = result.err();
            failed |= release.error.is_some();
        }
        if failed {
            Err("Claude release synchronization failed")
        } else {
            Ok(())
        }
    }

    pub fn resolve(&self, selection: Selection) -> Effective {
        let release = self.releases.read().expect("release state");
        let release = release
            .get(if selection.version_mode == "stable" {
                "stable"
            } else {
                "latest"
            })
            .expect("known channel");
        let explicit = selection.mode == "custom" || selection.version_mode == "fixed";
        let version = if explicit {
            selection.version.clone().expect("validated fixed version")
        } else {
            release.version.clone()
        };
        let entry = selection.client.clone();
        Effective {
            selection_fingerprint: selection.fingerprint(),
            user_agent: selection
                .user_agent
                .clone()
                .unwrap_or_else(|| format!("claude-cli/{version} (external, {entry})")),
            version,
            entry,
            os: match selection.platform.as_str() {
                "macos" => "MacOS",
                "windows" => "Windows",
                _ => "Linux",
            }
            .into(),
            arch: selection.arch.clone(),
            sdk_version: SDK_VERSION.into(),
            runtime_version: RUNTIME_VERSION.into(),
            template: TEMPLATE.into(),
            version_source: if explicit {
                "custom"
            } else if release.verified_at.is_some() {
                "official"
            } else {
                "baseline-2026-11-12"
            }
            .into(),
            verified_at: (!explicit)
                .then_some(release.verified_at)
                .flatten()
                .map(Into::into),
            selection,
        }
    }

    pub fn options(&self) -> OpaqueProviderData {
        let presets: Vec<_> = [("macos", "arm64"), ("linux", "x64"), ("windows", "x64")]
            .into_iter()
            .flat_map(|(platform, arch)| {
                ["cli", "vscode"].into_iter().map(move |client| Selection {
                    platform: platform.into(),
                    arch: arch.into(),
                    client: client.into(),
                    ..Selection::default()
                })
            })
            .collect();
        object(
            &json!({"globalConfiguration":Selection::default(), "presets":presets, "templateRevision":TEMPLATE,
            "clients":["cli","vscode"], "platforms":["macos","linux","windows"], "channels":CHANNELS}),
        )
    }

    pub fn preview(&self, selection: Selection) -> OpaqueProviderData {
        let effective = self.resolve(selection);
        let releases = self.releases.read().expect("release state");
        let release = &releases[if effective.selection.version_mode == "stable" {
            "stable"
        } else {
            "latest"
        }];
        object(
            &json!({"configuration":effective.selection, "effective":effective,
            "checkedAt":release.checked_at.map(chrono::DateTime::<chrono::Utc>::from), "error":release.error,
            "requiresClientIdentity": effective.selection.mode == "follow"}),
        )
    }

    pub fn dashboard(
        &self,
        selection: Selection,
    ) -> gateway_admin::model::observability::DashboardWireProfile {
        use gateway_admin::model::observability::{
            DashboardDesktopRelease, DashboardWireAttribute, DashboardWireProfile,
            DashboardWireTarget, DesktopReleaseStatus,
        };

        let effective = self.resolve(selection);
        let follows_client = effective.selection.mode == "follow";
        let fixed =
            effective.selection.mode == "custom" || effective.selection.version_mode == "fixed";
        let releases = self.releases.read().expect("release state");
        let release = &releases[if effective.selection.version_mode == "stable" {
            "stable"
        } else {
            "latest"
        }];
        // 概览展示全局选择；跟随模式的真实身份只能从单次请求获取，不能冒充预设身份。
        let visible = |value: String| {
            if follows_client {
                "—".to_owned()
            } else {
                value
            }
        };
        DashboardWireProfile {
            provider: crate::NAME.to_owned(),
            product: "Claude Code".to_owned(),
            version: visible(effective.version.clone()),
            build: None,
            target: DashboardWireTarget {
                os_type: visible(effective.os),
                os_version: "—".to_owned(),
                arch: visible(effective.arch),
                terminal: "—".to_owned(),
            },
            user_agent: visible(effective.user_agent),
            attributes: vec![
                DashboardWireAttribute {
                    key: "client_identifier".to_owned(),
                    value: visible(effective.entry),
                },
                DashboardWireAttribute {
                    key: "client_mode".to_owned(),
                    value: effective.selection.mode,
                },
                DashboardWireAttribute {
                    key: "version_policy".to_owned(),
                    value: if fixed {
                        "fixed".to_owned()
                    } else {
                        effective.selection.version_mode
                    },
                },
            ],
            verified_at: if follows_client {
                None
            } else {
                effective.verified_at
            },
            release: (!follows_client && !fixed).then(|| DashboardDesktopRelease {
                status: if release.error.is_some() {
                    DesktopReleaseStatus::Failed
                } else if release.verified_at.is_none() {
                    DesktopReleaseStatus::Unchecked
                } else if release.version == effective.version {
                    DesktopReleaseStatus::Current
                } else {
                    DesktopReleaseStatus::UpdateAvailable
                },
                checked_at: release.checked_at.map(Into::into),
                latest_version: release.verified_at.map(|_| release.version.clone()),
                latest_build: None,
                published_at: None,
                minimum_system_version: None,
                hardware_requirements: None,
                download_url: None,
                download_size: None,
                signature_present: None,
                error: release.error.map(str::to_owned),
            }),
        }
    }
}

impl Effective {
    pub fn follow(&mut self, headers: &reqwest::header::HeaderMap) -> Result<(), &'static str> {
        if self.selection.mode != "follow" {
            return Ok(());
        }
        let field = |name| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .ok_or("Complete Claude client identity required")
        };
        let agent = field("user-agent")?;
        let (version, entry) = client_agent(agent).ok_or("Invalid Claude client identity")?;
        if field("x-app")? != "cli"
            || field("x-stainless-lang")? != "js"
            || field("x-stainless-runtime")? != "node"
        {
            return Err("Invalid Claude client identity");
        }
        let sdk = field("x-stainless-package-version")?;
        let runtime = field("x-stainless-runtime-version")?;
        let os = field("x-stainless-os")?;
        let arch = field("x-stainless-arch")?;
        if version_sequence(sdk).is_none()
            || runtime
                .strip_prefix('v')
                .and_then(version_sequence)
                .is_none()
            || !matches!(os, "MacOS" | "Linux" | "Windows")
            || !matches!(arch, "arm64" | "x64")
        {
            return Err("Invalid Claude environment identity");
        }
        self.version = version.into();
        self.entry = entry.into();
        self.user_agent = agent.into();
        self.os = os.into();
        self.arch = arch.into();
        self.sdk_version = sdk.into();
        self.runtime_version = runtime.into();
        self.version_source = "client".into();
        self.verified_at = None;
        Ok(())
    }

    pub fn headers(
        &self,
        mut headers: reqwest::header::HeaderMap,
        timeout: Option<Duration>,
        count: bool,
        retry: u32,
    ) -> reqwest::header::HeaderMap {
        // 身份族由同一有效画像覆盖；重试次数和 timeout 来自真实执行预算。
        let asynchronous = headers
            .get("x-stainless-async")
            .is_some_and(|value| value == "async");
        let stale: Vec<_> = headers
            .keys()
            .filter(|name| name.as_str().starts_with("x-stainless-"))
            .cloned()
            .collect();
        for name in stale {
            headers.remove(name);
        }
        for (name, value) in [
            ("user-agent", self.user_agent.as_str()),
            ("x-app", "cli"),
            ("x-stainless-lang", "js"),
            ("x-stainless-runtime", "node"),
            ("x-stainless-runtime-version", &self.runtime_version),
            ("x-stainless-package-version", &self.sdk_version),
            ("x-stainless-os", &self.os),
            ("x-stainless-arch", &self.arch),
        ] {
            headers.insert(
                reqwest::header::HeaderName::from_static(name),
                reqwest::header::HeaderValue::from_str(value).expect("validated identity header"),
            );
        }
        headers.insert(
            "x-stainless-retry-count",
            reqwest::header::HeaderValue::from_str(&retry.to_string()).expect("retry header"),
        );
        headers.insert(
            "anthropic-dangerous-direct-browser-access",
            reqwest::header::HeaderValue::from_static("true"),
        );
        if asynchronous {
            headers.insert(
                "x-stainless-async",
                reqwest::header::HeaderValue::from_static("async"),
            );
        }
        if !count && let Some(timeout) = timeout {
            headers.insert(
                "x-stainless-timeout",
                reqwest::header::HeaderValue::from_str(&timeout.as_secs_f64().to_string())
                    .expect("timeout header"),
            );
        }
        headers
    }
}
