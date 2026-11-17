//! 审查摘要与请求记录共用生命周期；未记录不等于允许或关闭审查。

use super::{AdminError, observability::TimeRange};
use chrono::{DateTime, Utc};
use gateway_core::engine::prompt_guard::PromptGuardAssessment;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptGuardQuery {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub page: u32,
    pub page_size: u16,
    pub request_id: Option<String>,
    pub client_key_id: Option<String>,
    pub protocol: Option<String>,
    pub model: Option<String>,
    pub action: Option<String>,
    pub mode: Option<String>,
    pub category: Option<String>,
    pub policy_version: Option<String>,
    pub review_result: Option<String>,
    pub origin: Option<String>,
    pub min_score: Option<u32>,
    pub max_score: Option<u32>,
}

impl PromptGuardQuery {
    pub fn validate(&self) -> Result<(), AdminError> {
        TimeRange::new(self.start, self.end).map_err(|_| AdminError::invalid("时间范围无效"))?;
        if self.page == 0
            || self.page_size == 0
            || self.page_size > 100
            || u64::from(self.page - 1) * u64::from(self.page_size) > 100_000
        {
            return Err(AdminError::invalid("分页范围无效"));
        }
        if self
            .min_score
            .zip(self.max_score)
            .is_some_and(|(min, max)| min > max)
        {
            return Err(AdminError::invalid("评分范围无效"));
        }
        for value in [
            &self.request_id,
            &self.client_key_id,
            &self.protocol,
            &self.model,
            &self.action,
            &self.mode,
            &self.category,
            &self.policy_version,
            &self.review_result,
            &self.origin,
        ]
        .into_iter()
        .flatten()
        {
            if value.is_empty() || value.len() > 256 {
                return Err(AdminError::invalid("筛选条件无效"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptGuardRecord {
    pub request_id: String,
    pub started_at: DateTime<Utc>,
    pub client_key_id: String,
    pub protocol: Option<String>,
    pub model: String,
    pub outcome: String,
    pub assessment: Option<PromptGuardAssessment>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptGuardSummary {
    pub total: u64,
    pub recorded: u64,
    pub missing: u64,
    pub allowed: u64,
    pub blocked: u64,
    pub unavailable: u64,
    pub coverage_error: u64,
    pub observed: u64,
    pub observed_hits: u64,
    pub average_review_ms: Option<f64>,
    pub cancelled: u64,
    pub disabled: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptGuardPage {
    pub items: Vec<PromptGuardRecord>,
    pub summary: PromptGuardSummary,
    pub page: u32,
    pub page_size: u16,
}
