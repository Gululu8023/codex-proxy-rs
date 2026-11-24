-- 旧汇总没有同样本费用对。标记只随重新投影写入，撤销旧贡献时不得减掉不存在的新指标。
ALTER TABLE request_analytics_facts
    ADD COLUMN billing_comparison_ready boolean NOT NULL DEFAULT false;

-- 保留既有事实与汇总，通过原队列分批补算仍留存的请求；不重置全局 analytics schema。
ALTER TABLE request_analytics_build_state
    ADD COLUMN billing_backfill_complete boolean NOT NULL DEFAULT false,
    ADD COLUMN billing_backfill_started_at timestamp with time zone,
    ADD COLUMN billing_backfill_id text;
