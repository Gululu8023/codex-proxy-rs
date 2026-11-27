-- 既有分组保持原有能力；模型规则复用账号的完整 ID 黑白名单合同。
alter table account_groups
  add column allow_subagents boolean not null default true,
  add column model_access_json jsonb not null default '{"mode":"all","models":[]}'::jsonb,
  add constraint account_groups_model_access_ck check (
    jsonb_typeof(model_access_json) = 'object'
    and model_access_json ?& array['mode', 'models']
    and model_access_json - 'mode' - 'models' = '{}'::jsonb
    and octet_length(model_access_json::text) <= 131072
    and case when jsonb_typeof(model_access_json -> 'models') = 'array' then
      case model_access_json ->> 'mode'
        when 'all' then jsonb_array_length(model_access_json -> 'models') = 0
        when 'allowlist' then jsonb_array_length(model_access_json -> 'models') between 1 and 256
        when 'denylist' then jsonb_array_length(model_access_json -> 'models') between 1 and 256
        else false
      end
      and not jsonb_path_exists(model_access_json, '$.models[*] ? (@.type() != "string")')
    else false end
  );
