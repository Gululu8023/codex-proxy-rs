//! 冻结入口配置，并按 Key 默认值解析原生身份画像和限额

use std::sync::Arc;

use crate::policy::ClientPolicy;
use crate::routing::RuntimeSnapshot;
use crate::settings::SettingsValues;

/// 单次请求共享不可变配置；长连接的新一轮请求须重新获取当前快照。
#[derive(Clone, Debug)]
pub struct RequestSettings {
    snapshot: Arc<RuntimeSnapshot>,
}

impl PartialEq for RequestSettings {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.snapshot, &other.snapshot)
    }
}
impl Eq for RequestSettings {}

impl RequestSettings {
    #[must_use]
    pub fn new(snapshot: Arc<RuntimeSnapshot>) -> Self {
        Self { snapshot }
    }

    #[must_use]
    pub fn snapshot(&self) -> Arc<RuntimeSnapshot> {
        self.snapshot.clone()
    }

    #[must_use]
    pub fn values(&self) -> &SettingsValues {
        self.snapshot.settings()
    }

    /// Key 画像覆盖宿主默认；继承字段按当前宿主画像解析，不回写 Key 默认值。
    #[must_use]
    pub fn apply_policy(&self, policy: ClientPolicy) -> ClientPolicy {
        let defaults = policy.defaults();
        let mut profiles = self.values().request_profiles().clone();
        for (provider, key_profile) in &defaults.request_profiles {
            let profile = crate::settings::request_profiles::resolve_key_profile(
                key_profile,
                profiles.get(provider),
            );
            profiles.insert(provider.clone(), profile.into_owned());
        }
        let fast_mode = defaults.fast_mode;
        let limits = defaults.limits;
        policy.with_settings(&profiles, fast_mode, limits)
    }
}
