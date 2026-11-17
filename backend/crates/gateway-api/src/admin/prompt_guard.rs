//! Prompt 审查设置的 HTTP 投影，密钥只写并显式返回节点配置状态

use gateway_admin::model::prompt_guard::PromptGuardConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptGuardWire(pub PromptGuardConfig);
impl Serialize for PromptGuardWire {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut value = serde_json::to_value(&self.0).map_err(serde::ser::Error::custom)?;
        let fields = value.as_object_mut().expect("config object");
        fields.remove("reviewApiKey");
        fields.insert(
            "hasApiKey".into(),
            serde_json::json!(!self.0.api_keys().is_empty()),
        );
        fields.insert(
            "apiKeyCount".into(),
            serde_json::json!(self.0.api_keys().len()),
        );
        fields.insert(
            "state".into(),
            serde_json::json!(if !self.0.enabled {
                "disabled"
            } else if !self.0.review_enabled || self.0.ready() {
                "ready"
            } else {
                "candidate_review_unavailable"
            }),
        );
        value.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for PromptGuardWire {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        if let Some(fields) = value.as_object_mut() {
            fields.remove("hasApiKey");
            fields.remove("apiKeyCount");
            fields.remove("state");
        }
        serde_json::from_value(value)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}
