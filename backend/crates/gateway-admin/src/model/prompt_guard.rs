//! Prompt 审查的持久配置与只写密钥，不能出现在公开的运行设置投影中

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReviewApiKey(String);
impl ReviewApiKey {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for ReviewApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReviewApiKey([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptGuardConfig {
    pub enabled: bool,
    pub mode: String,
    pub local_action: String,
    pub threshold: u32,
    pub strict_threshold: u32,
    pub strict_terminal_enabled: bool,
    pub review_enabled: bool,
    pub review_fail_closed: bool,
    pub review_max_concurrent: u32,
    pub review_request_mode: String,
    pub moderation_thresholds: std::collections::BTreeMap<String, u32>,
    pub scope: String,
    pub review_url: String,
    pub review_model: String,
    // 公开服务会序列化管理模型，存储层必须显式保存密钥，不能随读取结果返回
    #[serde(skip_serializing)]
    pub review_api_key: Option<ReviewApiKey>,
    pub timeout_seconds: u32,
    pub confidence_threshold_bps: u32,
    pub max_text_chars: u32,
    pub disabled_rules: Vec<String>,
    pub conversation_lock_enabled: bool,
    pub conversation_lock_ttl_hours: u32,
    pub learning_enabled: bool,
    pub learning_model: String,
}
impl Default for PromptGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: "enforce".into(),
            local_action: "block".into(),
            threshold: 50,
            strict_threshold: 90,
            strict_terminal_enabled: true,
            review_enabled: false,
            review_fail_closed: true,
            review_max_concurrent: 8,
            review_request_mode: "chat_completions".into(),
            moderation_thresholds: default_moderation_thresholds(),
            scope: "local_candidates".into(),
            review_url: String::new(),
            review_model: String::new(),
            review_api_key: None,
            timeout_seconds: 10,
            confidence_threshold_bps: 7000,
            max_text_chars: 32768,
            disabled_rules: Vec::new(),
            conversation_lock_enabled: true,
            conversation_lock_ttl_hours: 168,
            learning_enabled: true,
            learning_model: String::new(),
        }
    }
}
impl PromptGuardConfig {
    /// 仅供持久化使用，公开服务序列化必须继续隐去密钥
    pub fn to_storage(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("prompt guard config JSON");
        if let Some(key) = &self.review_api_key {
            value["reviewApiKey"] = serde_json::Value::String(key.expose().to_owned());
        }
        value
    }
    pub fn ready(&self) -> bool {
        !self.review_url.trim().is_empty()
            && !self.review_model.trim().is_empty()
            && !self.api_keys().is_empty()
    }
    /// 与复审 Key 池共用解析规则；读取接口只返回数量，绝不回显 Key。
    pub fn api_keys(&self) -> Vec<&str> {
        let mut keys = Vec::new();
        if let Some(value) = &self.review_api_key {
            for key in value
                .expose()
                .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                .filter(|v| !v.is_empty())
            {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        keys
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.mode.as_str(), "observe" | "enforce")
            || !matches!(self.local_action.as_str(), "warn" | "block")
            || !matches!(self.scope.as_str(), "local_candidates" | "all")
            || !(1..=60).contains(&self.timeout_seconds)
            || !(1..=1000).contains(&self.threshold)
            || !(self.threshold..=1000).contains(&self.strict_threshold)
            || !(1..=64).contains(&self.review_max_concurrent)
            || !(1..=720).contains(&self.conversation_lock_ttl_hours)
            || !matches!(
                self.review_request_mode.as_str(),
                "chat_completions" | "moderations"
            )
            || self
                .moderation_thresholds
                .keys()
                .ne(default_moderation_thresholds().keys())
            || self
                .moderation_thresholds
                .values()
                .any(|v| !(1..=10000).contains(v))
            || !(1..=10000).contains(&self.confidence_threshold_bps)
            || !(1..=262144).contains(&self.max_text_chars)
            || self.review_model.len() > 256
            || self.learning_model.len() > 256
            || self
                .review_api_key
                .as_ref()
                .is_some_and(|key| key.expose().len() > 65536)
            || self.api_keys().len() > 64
            || self.api_keys().iter().any(|key| {
                key.len() > 8192 || !key.is_ascii() || key.bytes().any(|b| b.is_ascii_control())
            })
            || self.disabled_rules.len() > 1024
            || self
                .disabled_rules
                .iter()
                .any(|id| id.is_empty() || id.len() > 128)
            || self.review_enabled && !self.ready()
        {
            return Err("promptGuard");
        }
        if !self.review_url.is_empty() {
            let url = url::Url::parse(&self.review_url).map_err(|_| "promptGuard.reviewUrl")?;
            let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
            if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || !url
                    .path()
                    .ends_with(if self.review_request_mode == "moderations" {
                        "/moderations"
                    } else {
                        "/chat/completions"
                    })
            {
                return Err("promptGuard.reviewUrl");
            }
        }
        Ok(())
    }
}

/// Moderations 使用分类分数而非单个 flagged；阈值单位与复审置信度一致。
pub fn default_moderation_thresholds() -> std::collections::BTreeMap<String, u32> {
    [
        ("harassment", 9800),
        ("harassment/threatening", 9000),
        ("hate", 6500),
        ("hate/threatening", 6500),
        ("illicit", 9500),
        ("illicit/violent", 9500),
        ("self-harm", 6500),
        ("self-harm/intent", 8500),
        ("self-harm/instructions", 6500),
        ("sexual", 6500),
        ("sexual/minors", 6500),
        ("violence", 9500),
        ("violence/graphic", 9500),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect()
}
