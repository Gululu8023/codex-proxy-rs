//! Key 使用完整画像，或显式继承全局画像并附加 Provider 所有的字段

use std::borrow::Cow;

use crate::account::OpaqueProviderData;
use serde_json::{Map, Value};

/// `inherit` 是 Key 配置的外层协议；内部字段名和值只由对应 Provider 校验
#[must_use]
pub fn inherited_fields(key: &OpaqueProviderData) -> Option<&Map<String, Value>> {
    let document = key.expose_to_provider();
    (document.len() == 1)
        .then(|| document.get("inherit")?.as_object())
        .flatten()
}

#[derive(Debug, thiserror::Error)]
#[error("batch client profile must use the separate installation seed field")]
pub struct InvalidBatchKeyProfile;

/// 批量切换画像保留每把 Key、每个 Provider 的种子；显式更换种子时才覆盖。
pub fn merge_batch_key_profile(
    current: Option<&OpaqueProviderData>,
    update: Option<&Option<OpaqueProviderData>>,
    installation_seed: Option<uuid::Uuid>,
) -> Result<Option<OpaqueProviderData>, InvalidBatchKeyProfile> {
    if let Some(Some(profile)) = update
        && inherited_fields(profile)
            .unwrap_or_else(|| profile.expose_to_provider())
            .contains_key("installationSeed")
    {
        return Err(InvalidBatchKeyProfile);
    }
    let selection = update.map_or(current, Option::as_ref);
    let seed = installation_seed.map(|seed| seed.to_string()).or_else(|| {
        current.and_then(|profile| {
            inherited_fields(profile)
                .unwrap_or_else(|| profile.expose_to_provider())
                .get("installationSeed")
                .and_then(Value::as_str)
                .filter(|seed| !seed.is_empty())
                .map(str::to_owned)
        })
    });
    let Some(seed) = seed else {
        return Ok(selection.cloned());
    };
    if let Some(selection) = selection
        && inherited_fields(selection).is_none()
    {
        let mut fields = selection.expose_to_provider().clone();
        fields.insert("installationSeed".into(), Value::String(seed));
        return Ok(Some(OpaqueProviderData::new(fields)));
    }
    Ok(Some(OpaqueProviderData::new(
        serde_json::json!({
            "inherit": {"installationSeed": seed},
        })
        .as_object()
        .expect("fixed profile object")
        .clone(),
    )))
}

/// 在请求快照上应用继承，不把全局字段复制回 Key；没有宿主配置时由 Provider 解析默认值
#[must_use]
pub fn resolve_key_profile<'a>(
    key: &'a OpaqueProviderData,
    global: Option<&OpaqueProviderData>,
) -> Cow<'a, OpaqueProviderData> {
    if let Some(fields) = inherited_fields(key)
        && let Some(global) = global
    {
        let mut resolved = global.clone().into_inner();
        resolved.extend(fields.clone());
        return Cow::Owned(OpaqueProviderData::new(resolved));
    }
    Cow::Borrowed(key)
}
