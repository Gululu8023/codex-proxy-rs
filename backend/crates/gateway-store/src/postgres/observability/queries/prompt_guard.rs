//! 原始请求摘要查询，不创建第二套评分存储或异步投影队列。

use super::super::*;
use gateway_admin::model::prompt_guard_records::{
    PromptGuardPage, PromptGuardQuery, PromptGuardRecord, PromptGuardSummary,
};

fn filter(query: &mut QueryBuilder<Postgres>, input: &PromptGuardQuery) {
    query.push(" from model_request_observations where outcome <> 'running'");
    // 从用量详情跳转按精确 ID 读取，不受页面默认时间窗口影响，删除仍由原有保留策略负责。
    if input.request_id.is_none() {
        query
            .push(" and started_at >= ")
            .push_bind(input.start)
            .push(" and started_at < ")
            .push_bind(input.end);
    }
    for (column, value) in [
        ("id", &input.request_id),
        ("client_api_key_ref", &input.client_key_id),
        ("protocol", &input.protocol),
        ("requested_model_id", &input.model),
        ("prompt_guard_action", &input.action),
        ("prompt_guard_mode", &input.mode),
        ("prompt_guard_policy_version", &input.policy_version),
        ("prompt_guard_review_result", &input.review_result),
    ] {
        if let Some(value) = value {
            if column == "prompt_guard_action" && value == "missing" {
                query.push(" and prompt_guard_summary is null");
            } else {
                query
                    .push(" and ")
                    .push(column)
                    .push(" = ")
                    .push_bind(value.clone());
            }
        }
    }
    if let Some(value) = &input.category {
        query.push(" and exists (select 1 from jsonb_array_elements(prompt_guard_summary->'categories') category where category->>'category' = ")
            .push_bind(value.clone()).push(")");
    }
    if let Some(value) = &input.origin {
        query.push(" and exists (select 1 from jsonb_array_elements(prompt_guard_summary->'hits') hit where hit->>'origin' = ")
            .push_bind(value.clone()).push(")");
    }
    if let Some(min) = input.min_score {
        query
            .push(" and prompt_guard_decision_score >= ")
            .push_bind(i64::from(min));
    }
    if let Some(max) = input.max_score {
        query
            .push(" and prompt_guard_decision_score <= ")
            .push_bind(i64::from(max));
    }
}

pub(crate) async fn prompt_guard_records(
    connection: &mut PgConnection,
    input: PromptGuardQuery,
) -> StoreResult<PromptGuardPage> {
    // 一次数据库语句保证列表和计数来自同一快照；按根请求 ID 计数，重试不会重复。
    // 计数中间集仅保留标量；完整命中摘要只为本页读取，避免宽时间查询物化全部 JSON。
    let mut query = QueryBuilder::<Postgres>::new(
        "with filtered as materialized (select id, started_at, (prompt_guard_summary is not null) as recorded, prompt_guard_action, prompt_guard_mode, prompt_guard_summary->>'wouldAction' as would_action, case when prompt_guard_review_result is not null then greatest(0, (prompt_guard_summary->>'totalMs')::bigint - (prompt_guard_summary->>'localMs')::bigint) end as review_ms",
    );
    filter(&mut query, &input);
    query.push("), page_ids as (select id from filtered order by started_at desc, id desc limit ")
        .push_bind(i64::from(input.page_size)).push(" offset ")
        .push_bind(i64::from(input.page - 1) * i64::from(input.page_size))
        .push("), page as (select o.id, o.started_at, o.client_api_key_ref, o.protocol, o.requested_model_id, o.outcome, o.prompt_guard_summary from model_request_observations o join page_ids using(id)) select count(*)::bigint as total,
            count(*) filter(where recorded)::bigint as recorded,
            count(*) filter(where not recorded)::bigint as missing,
            count(*) filter(where prompt_guard_action = 'allow')::bigint as allowed,
            count(*) filter(where prompt_guard_action = 'block')::bigint as blocked,
            count(*) filter(where prompt_guard_action = 'unavailable')::bigint as unavailable,
            count(*) filter(where prompt_guard_action = 'coverage_error')::bigint as coverage_error,
            count(*) filter(where prompt_guard_mode = 'observe')::bigint as observed,
            count(*) filter(where prompt_guard_mode = 'observe' and would_action = 'block')::bigint as observed_hits,
            avg(review_ms)::double precision as average_review_ms,
            count(*) filter(where prompt_guard_action = 'cancelled')::bigint as cancelled,
            count(*) filter(where prompt_guard_action = 'disabled')::bigint as disabled,
            coalesce((select jsonb_agg(to_jsonb(page) order by started_at desc, id desc) from page), '[]'::jsonb) as items from filtered");
    let row = query
        .build()
        .fetch_one(connection)
        .await
        .map_err(|source| postgres_unavailable("list prompt guard records", source))?;
    let count = |name| -> StoreResult<u64> { to_u64(get(&row, name)?) };
    let summary = PromptGuardSummary {
        total: count("total")?,
        recorded: count("recorded")?,
        missing: count("missing")?,
        allowed: count("allowed")?,
        blocked: count("blocked")?,
        unavailable: count("unavailable")?,
        coverage_error: count("coverage_error")?,
        observed: count("observed")?,
        observed_hits: count("observed_hits")?,
        average_review_ms: get(&row, "average_review_ms")?,
        cancelled: count("cancelled")?,
        disabled: count("disabled")?,
    };
    #[derive(serde::Deserialize)]
    struct Record {
        id: String,
        started_at: DateTime<Utc>,
        client_api_key_ref: String,
        protocol: Option<String>,
        requested_model_id: String,
        outcome: String,
        prompt_guard_summary: Option<gateway_core::engine::prompt_guard::PromptGuardAssessment>,
    }
    let items: Vec<Record> = serde_json::from_value(get(&row, "items")?)
        .map_err(|_| invalid("invalid prompt guard summary"))?;
    Ok(PromptGuardPage {
        items: items
            .into_iter()
            .map(|r| PromptGuardRecord {
                request_id: r.id,
                started_at: r.started_at,
                client_key_id: r.client_api_key_ref,
                protocol: r.protocol,
                model: r.requested_model_id,
                outcome: r.outcome,
                assessment: r.prompt_guard_summary,
            })
            .collect(),
        summary,
        page: input.page,
        page_size: input.page_size,
    })
}
