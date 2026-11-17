//! Prompt 审查的候选、会话锁；请求评分仍使用现有根请求账本。

use async_trait::async_trait;
use gateway_admin::{
    model::{AdminError, MutationContext, audit::MutationAuditOperation},
    ports::prompt_guard::*,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row, types::Json};

#[derive(Clone)]
pub struct PgPromptGuardStore {
    pool: PgPool,
}
impl PgPromptGuardStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &MutationContext,
    operation: MutationAuditOperation,
    id: i64,
    fields: Vec<String>,
) -> Result<(), AdminError> {
    let event =
        super::admin_security_audit::mutation_audit(context, operation, &id.to_string(), fields);
    super::append_admin_audit_event_in_transaction(tx, event, None)
        .await
        .map_err(|error| AdminError::unavailable("审查操作审计写入失败").with_source(error))
}

fn unavailable(error: sqlx::Error) -> AdminError {
    if error
        .as_database_error()
        .is_some_and(|e| e.is_unique_violation())
    {
        return AdminError::conflict("规则标识已存在，请刷新后重试");
    }
    AdminError::unavailable("Prompt 审查存储不可用").with_source(error)
}

#[derive(sqlx::FromRow, Deserialize)]
struct CandidateRow {
    id: i64,
    revision: i64,
    status: String,
    source: String,
    protocol: String,
    evidence_count: i64,
    evidence: Option<String>,
    evidence_quality: String,
    rule_json: Option<Value>,
    rationale: String,
    last_seen_at: chrono::DateTime<chrono::Utc>,
}
impl CandidateRow {
    fn into_model(self) -> Result<RuleCandidate, AdminError> {
        Ok(RuleCandidate {
            id: self.id,
            revision: self.revision,
            status: self.status,
            source: self.source,
            protocol: self.protocol,
            evidence_count: self.evidence_count,
            evidence: self.evidence,
            evidence_quality: self.evidence_quality,
            rule: self
                .rule_json
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| AdminError::internal("候选规则格式无效"))?,
            rationale: self.rationale,
            last_seen_at: self.last_seen_at,
        })
    }
}

#[async_trait]
impl PromptGuardStore for PgPromptGuardStore {
    async fn rules(&self) -> Result<RuleSnapshot, AdminError> {
        let (revision, Json(rules)) = sqlx::query_as::<_, (i64, Json<Vec<GuardRule>>)>(
            "select revision, coalesce((select jsonb_agg(rule_json order by id) from prompt_guard_candidates where status = 'published'), '[]'::jsonb) from prompt_guard_rule_revision where id = 1")
            .fetch_one(&self.pool).await.map_err(unavailable)?;
        Ok(RuleSnapshot { revision, rules })
    }

    async fn is_locked(
        &self,
        key: &str,
        protocol: &str,
        scopes: &[String],
    ) -> Result<bool, AdminError> {
        sqlx::query_scalar("select exists(select 1 from prompt_guard_locks where client_key_id = $1 and protocol = $2 and scope = any($3) and expires_at > now())")
            .bind(key).bind(protocol).bind(scopes).fetch_one(&self.pool).await.map_err(unavailable)
    }

    async fn record_incident(
        &self,
        incident: GuardIncident,
        lock_hours: Option<u32>,
    ) -> Result<(), AdminError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query(
            "select pg_advisory_xact_lock(hashtextextended('prompt-guard:' || $1 || ':' || $2, 0))",
        )
        .bind(&incident.client_key_id)
        .bind(&incident.protocol)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let inserted = sqlx::query("insert into prompt_guard_incidents(request_id, client_key_id, protocol, model, code) values($1,$2,$3,$4,$5) on conflict(request_id) do nothing")
            .bind(&incident.request_id).bind(&incident.client_key_id).bind(&incident.protocol).bind(&incident.model).bind(&incident.code)
            .execute(&mut *tx).await.map_err(unavailable)?.rows_affected() != 0;
        // 同一根请求的重复终态不能增加证据数，也不能延长已经人工解除的锁。
        if inserted {
            if incident.learning_enabled {
                let id: i64 = sqlx::query_scalar("insert into prompt_guard_candidates(fingerprint, source, protocol, evidence_count, evidence, evidence_quality) values($1,$5,$2,1,$3,$4)
                    on conflict(fingerprint) do update set evidence_count = prompt_guard_candidates.evidence_count + 1, last_seen_at = now(),
                        evidence = coalesce(prompt_guard_candidates.evidence, excluded.evidence),
                        evidence_quality = case when prompt_guard_candidates.evidence is null then excluded.evidence_quality else prompt_guard_candidates.evidence_quality end,
                        revision = prompt_guard_candidates.revision + case when prompt_guard_candidates.evidence is null and excluded.evidence is not null then 1 else 0 end returning id")
                    .bind(&incident.fingerprint).bind(&incident.protocol).bind(&incident.evidence).bind(&incident.evidence_quality)
                    .bind(format!("upstream_{}", incident.code))
                    .fetch_one(&mut *tx).await.map_err(unavailable)?;
                sqlx::query(
                    "update prompt_guard_incidents set candidate_id = $1 where request_id = $2",
                )
                .bind(id)
                .bind(&incident.request_id)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
            }
            if let Some(hours) = lock_hours {
                for scope in &incident.lock_scopes {
                    sqlx::query("insert into prompt_guard_locks(client_key_id, protocol, scope, request_id, expires_at) values($1,$2,$3,$4,now() + $5 * interval '1 hour')
                        on conflict(client_key_id, protocol, scope) do update set request_id = excluded.request_id, revision = prompt_guard_locks.revision + 1, expires_at = greatest(prompt_guard_locks.expires_at, excluded.expires_at)")
                        .bind(&incident.client_key_id).bind(&incident.protocol).bind(scope).bind(&incident.request_id).bind(i64::from(hours))
                        .execute(&mut *tx).await.map_err(unavailable)?;
                }
            }
        }
        tx.commit().await.map_err(unavailable)
    }

    async fn candidates(&self, query: GuardListQuery) -> Result<CandidatePage, AdminError> {
        query.validate()?;
        let row = sqlx::query("with filtered as materialized (select * from prompt_guard_candidates where ($1::text is null or protocol = $1) and ($2::text is null or status = $2)),
            page as (select * from filtered order by last_seen_at desc, id desc limit $3 offset $4)
            select (select count(*) from filtered) as total, coalesce((select jsonb_agg(to_jsonb(page) order by last_seen_at desc, id desc) from page), '[]'::jsonb) as items")
            .bind(&query.protocol).bind(&query.status).bind(i64::from(query.page_size)).bind(i64::from(query.page - 1) * i64::from(query.page_size))
            .fetch_one(&self.pool).await.map_err(unavailable)?;
        let rows: Vec<CandidateRow> =
            serde_json::from_value(row.try_get("items").map_err(unavailable)?)
                .map_err(|_| AdminError::internal("候选记录格式无效"))?;
        Ok(CandidatePage {
            items: rows
                .into_iter()
                .map(CandidateRow::into_model)
                .collect::<Result<_, _>>()?,
            total: row.try_get("total").map_err(unavailable)?,
            page: query.page,
            page_size: query.page_size,
        })
    }

    async fn candidate(&self, id: i64) -> Result<RuleCandidate, AdminError> {
        sqlx::query_as::<_, CandidateRow>("select * from prompt_guard_candidates where id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| AdminError::not_found("候选不存在"))?
            .into_model()
    }

    async fn save_candidate(
        &self,
        context: &MutationContext,
        update: CandidateUpdate,
    ) -> Result<RuleCandidate, AdminError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let rule =
            Json(serde_json::to_value(&update.rule).map_err(|_| AdminError::invalid("规则无效"))?);
        let row = if update.id == 0 {
            // 手工规则和学习规则都先进入待审核区，不直接改运行规则。
            sqlx::query_as::<_, CandidateRow>("insert into prompt_guard_candidates(fingerprint, source, protocol, evidence_quality, rule_json, rationale) values('manual:' || $1,'manual','', 'manual',$2,$3) returning *")
                .bind(&update.rule.id).bind(rule).bind(&update.rationale).fetch_one(&mut *tx).await.map_err(unavailable)?
        } else {
            sqlx::query_as::<_, CandidateRow>("update prompt_guard_candidates set rule_json=$1, rationale=$2, revision=revision+1 where id=$3 and revision=$4 and status in ('pending','disabled') returning *")
                .bind(rule).bind(&update.rationale).bind(update.id).bind(update.revision).fetch_optional(&mut *tx).await.map_err(unavailable)?
                .ok_or_else(|| AdminError::conflict("候选已更新或正在生效，请刷新后重试"))?
        };
        audit(
            &mut tx,
            context,
            MutationAuditOperation::PromptGuardRuleSave,
            row.id,
            vec!["rule".into(), "rationale".into()],
        )
        .await?;
        tx.commit().await.map_err(unavailable)?;
        row.into_model()
    }

    async fn transition(
        &self,
        context: &MutationContext,
        action: CandidateAction,
    ) -> Result<RuleCandidate, AdminError> {
        let (target, allowed): (&str, &[&str]) = match action.action.as_str() {
            "publish" => ("published", &["pending", "disabled"]),
            "dismiss" => ("dismissed", &["pending"]),
            "disable" => ("disabled", &["published"]),
            "reopen" => ("pending", &["dismissed"]),
            _ => return Err(AdminError::invalid("候选操作无效")),
        };
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        // 所有发布/停用先锁规则代次，跨进程保持规则内容与代次一致。
        sqlx::query("select revision from prompt_guard_rule_revision where id=1 for update")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        if target == "published" {
            let count: i64 = sqlx::query_scalar(
                "select count(*) from prompt_guard_candidates where status='published'",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
            if count >= 256 {
                return Err(AdminError::invalid("生效自定义规则最多 256 条"));
            }
        }
        let row = sqlx::query_as::<_, CandidateRow>("update prompt_guard_candidates set status=$1, revision=revision+1 where id=$2 and revision=$3 and status=any($4) and ($1 <> 'published' or rule_json is not null) returning *")
            .bind(target).bind(action.id).bind(action.revision).bind(allowed).fetch_optional(&mut *tx).await.map_err(unavailable)?
            .ok_or_else(|| AdminError::conflict("候选状态或版本已变化，证据记录须先形成规则草稿"))?;
        if matches!(target, "published" | "disabled") {
            sqlx::query("update prompt_guard_rule_revision set revision=revision+1 where id=1")
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
        }
        audit(
            &mut tx,
            context,
            MutationAuditOperation::PromptGuardRuleTransition,
            row.id,
            vec![format!("status:{target}")],
        )
        .await?;
        tx.commit().await.map_err(unavailable)?;
        row.into_model()
    }

    async fn profiles(&self, query: GuardListQuery) -> Result<Value, AdminError> {
        query.validate()?;
        // 画像只汇总根请求事实，不因历史/工具文本命中给 Key 记违规，也不自动封禁整个 Key。
        let row = sqlx::query("with profiles as materialized (
            select client_api_key_ref as key, protocol, count(*)::bigint as requests,
                count(*) filter(where prompt_guard_action='block')::bigint as blocked,
                count(*) filter(where prompt_guard_action='warn')::bigint as warned,
                count(*) filter(where prompt_guard_summary->'upstreamPolicy' is not null and prompt_guard_summary->'upstreamPolicy' <> 'null'::jsonb)::bigint as upstream_rejections,
                max(prompt_guard_decision_score)::bigint as maximum_score, max(started_at) as last_seen_at
            from model_request_observations where started_at >= now() - interval '30 days' and outcome <> 'running' and prompt_guard_action <> 'disabled'
                and ($1::text is null or protocol=$1) group by client_api_key_ref, protocol),
            page as (select * from profiles order by upstream_rejections desc, blocked desc, last_seen_at desc, key limit $2 offset $3)
            select (select count(*) from profiles) as total, coalesce((select jsonb_agg(to_jsonb(page) order by upstream_rejections desc, blocked desc, last_seen_at desc, key) from page),'[]'::jsonb) as items")
            .bind(&query.protocol).bind(i64::from(query.page_size)).bind(i64::from(query.page - 1) * i64::from(query.page_size)).fetch_one(&self.pool).await.map_err(unavailable)?;
        Ok(
            json!({"items":row.try_get::<Value,_>("items").map_err(unavailable)?, "total":row.try_get::<i64,_>("total").map_err(unavailable)?,
            "page":query.page,"pageSize":query.page_size,"windowDays":30}),
        )
    }

    async fn locks(&self, query: GuardListQuery) -> Result<Value, AdminError> {
        query.validate()?;
        // 一次拒绝的会话锁和指纹锁合并为一行；总数和分页使用同一快照，解除仍覆盖整次事件。
        let row = sqlx::query("with incidents as materialized (
                select distinct on (client_key_id, protocol, request_id) id, revision, client_key_id, protocol, request_id, expires_at
                from prompt_guard_locks where expires_at > now() and ($1::text is null or protocol=$1)
                order by client_key_id, protocol, request_id, expires_at desc, id desc),
            page as (select * from incidents order by expires_at desc, id desc limit $2 offset $3)
            select (select count(*) from incidents) as total,
                coalesce((select jsonb_agg(to_jsonb(page) order by expires_at desc, id desc) from page),'[]'::jsonb) as items")
            .bind(&query.protocol).bind(i64::from(query.page_size)).bind(i64::from(query.page - 1) * i64::from(query.page_size))
            .fetch_one(&self.pool).await.map_err(unavailable)?;
        Ok(
            json!({"items":row.try_get::<Value,_>("items").map_err(unavailable)?, "total":row.try_get::<i64,_>("total").map_err(unavailable)?,
            "page":query.page,"pageSize":query.page_size}),
        )
    }

    async fn release_lock(
        &self,
        context: &MutationContext,
        id: i64,
        revision: i64,
    ) -> Result<(), AdminError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let (key, protocol): (String, String) =
            sqlx::query_as("select client_key_id, protocol from prompt_guard_locks where id=$1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?
                .ok_or_else(|| AdminError::conflict("会话锁已过期或不存在"))?;
        // 与上游反馈共用 Key/协议事务锁，避免同时更新会话锁和指纹锁时交叉持锁。
        sqlx::query(
            "select pg_advisory_xact_lock(hashtextextended('prompt-guard:' || $1 || ':' || $2, 0))",
        )
        .bind(key)
        .bind(protocol)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        // 一次拒绝同时产生会话锁和指纹锁；解除必须覆盖该事件的全部锁。
        let changed = sqlx::query("with selected as (select client_key_id, protocol, request_id from prompt_guard_locks where id=$1 and revision=$2 and expires_at>now() for update)
            update prompt_guard_locks l set expires_at=now(), revision=l.revision+1 from selected s where l.client_key_id=s.client_key_id and l.protocol=s.protocol and l.request_id=s.request_id")
            .bind(id).bind(revision).execute(&mut *tx).await.map_err(unavailable)?.rows_affected();
        if changed == 0 {
            return Err(AdminError::conflict("会话锁已变化或已解除，请刷新"));
        }
        audit(
            &mut tx,
            context,
            MutationAuditOperation::PromptGuardLockRelease,
            id,
            vec!["expires_at".into()],
        )
        .await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(())
    }
}
