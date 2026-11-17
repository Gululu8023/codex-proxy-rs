//! 由宿主传入的可信写入主体，不接受请求载荷自行声明

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UsageActor {
    Admin { subject: String },
    System,
}

impl super::UsageRecordChange {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Put { id, .. } | Self::Update { id, .. } | Self::Delete { id } => id,
        }
    }
}
