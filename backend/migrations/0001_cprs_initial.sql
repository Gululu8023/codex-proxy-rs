-- CPRS 独立初始化基线：原 0001～0042 在 PostgreSQL 18 空库上的最终结构与初始数据。
-- 仅用于全新数据库，不支持源仓库或旧定制版迁移历史；SQLx 自行创建迁移记录表。
-- 保留调用方 search_path，兼容独立 schema；函数先于依赖表声明，仅在本次迁移事务中延迟函数体校验。
SET LOCAL check_function_bodies = false;

CREATE FUNCTION apply_quota_credit_policy() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
begin
  new.quota_access_state := 'exhausted';
  new.quota_evidence := 'account_limit_reached';
  return new;
end
$$;

CREATE FUNCTION check_model_request_observation() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
declare
  json_changed boolean := tg_op = 'INSERT' or old.request_observation_json is distinct from new.request_observation_json;
begin
  if json_changed then
    if model_request_observation_valid(new.request_observation_json) is not true then
      raise exception using errcode = '23514', constraint = 'model_requests_observation_shape_ck',
        message = 'request observation violates shape constraint', table = tg_table_name, schema = tg_table_schema;
    end if;
  end if;
  if json_changed then
    declare
      timings_upstream_response_ms bigint;
    begin
      timings_upstream_response_ms := (new.request_observation_json #>> '{timings,upstream,responseMs}')::bigint;
      if (timings_upstream_response_ms > 0) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_upstream_duration_ck',
          message = 'request observation violates model_requests_upstream_duration_ck', table = tg_table_name, schema = tg_table_schema;
      end if;
    end;
  end if;
  if json_changed then
    if ((new.request_observation_json #>> '{request,configRevision}')::bigint > 0) is false then
      raise exception using errcode = '23514', constraint = 'model_requests_revision_attempt_ck', message = 'request observation violates model_requests_revision_attempt_ck', table = tg_table_name, schema = tg_table_schema;
    end if;
  end if;
  if json_changed then
    if ((new.request_observation_json #>> '{error,retryAfterMs}')::bigint is null or (new.request_observation_json #>> '{error,retryAfterMs}')::bigint >= 0) is false then
      raise exception using errcode = '23514', constraint = 'model_requests_status_ck', message = 'request observation violates model_requests_status_ck', table = tg_table_name, schema = tg_table_schema;
    end if;
  end if;
  if json_changed then
    declare
      transport_websocket_pool text;
    begin
      transport_websocket_pool := (new.request_observation_json #>> '{transport,websocketPool}');
      if (transport_websocket_pool is null or transport_websocket_pool in ('new', 'reuse')) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_websocket_pool_ck',
          message = 'request observation violates model_requests_websocket_pool_ck', table = tg_table_name, schema = tg_table_schema;
      end if;
    end;
  end if;
  if json_changed then
    declare
      routing_group_names_snapshot jsonb;
      routing_scope text;
      routing_group_refs jsonb;
    begin
      routing_group_names_snapshot := (new.request_observation_json #> '{routing,groupNamesSnapshot}');
      routing_scope := (new.request_observation_json #>> '{routing,scope}');
      routing_group_refs := (new.request_observation_json #> '{routing,groupRefs}');
      if (jsonb_typeof(routing_group_names_snapshot) = 'array'
    and (
      (
        routing_scope in ('legacy_provider', 'all')
        and jsonb_array_length(routing_group_refs) = 0
        and jsonb_array_length(routing_group_names_snapshot) = 0
      )
      or (
        routing_scope = 'groups'
        and jsonb_array_length(routing_group_refs) > 0
        and not jsonb_path_exists(new.request_observation_json, '$.routing.groupRefs[*] ? (@ == null)')
        and jsonb_array_length(routing_group_names_snapshot)
          = jsonb_array_length(routing_group_refs)
      )
    )) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_routing_group_names_ck',
          message = 'request observation violates model_requests_routing_group_names_ck', table = tg_table_name, schema = tg_table_schema;
      end if;
    end;
  end if;
  if json_changed then
    declare
      routing_scope text;
    begin
      routing_scope := (new.request_observation_json #>> '{routing,scope}');
      if (routing_scope in ('legacy_provider', 'all', 'groups')) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_routing_scope_ck',
          message = 'request observation violates model_requests_routing_scope_ck', table = tg_table_name, schema = tg_table_schema;
      end if;
    end;
  end if;
  if json_changed then
    declare
      timings_local_transport_decision_wait_ms bigint;
      timings_local_connect_ms bigint;
      timings_local_headers_ms bigint;
      timings_local_first_event_ms bigint;
      timings_local_first_reasoning_ms bigint;
      timings_local_first_text_ms bigint;
      timings_local_first_token_ms bigint;
      timings_upstream_processing_ms bigint;
      timings_local_latency_ms bigint;
    begin
      timings_local_transport_decision_wait_ms := (new.request_observation_json #>> '{timings,local,transportDecisionWaitMs}')::bigint;
      timings_local_connect_ms := (new.request_observation_json #>> '{timings,local,connectMs}')::bigint;
      timings_local_headers_ms := (new.request_observation_json #>> '{timings,local,headersMs}')::bigint;
      timings_local_first_event_ms := (new.request_observation_json #>> '{timings,local,firstEventMs}')::bigint;
      timings_local_first_reasoning_ms := (new.request_observation_json #>> '{timings,local,firstReasoningMs}')::bigint;
      timings_local_first_text_ms := (new.request_observation_json #>> '{timings,local,firstTextMs}')::bigint;
      timings_local_first_token_ms := (new.request_observation_json #>> '{timings,local,firstTokenMs}')::bigint;
      timings_upstream_processing_ms := (new.request_observation_json #>> '{timings,upstream,processingMs}')::bigint;
      timings_local_latency_ms := (new.request_observation_json #>> '{timings,local,latencyMs}')::bigint;
      if ((timings_local_transport_decision_wait_ms is null or timings_local_transport_decision_wait_ms >= 0)
    and (timings_local_connect_ms is null or timings_local_connect_ms >= 0)
    and (timings_local_headers_ms is null or timings_local_headers_ms >= 0)
    and (timings_local_first_event_ms is null or timings_local_first_event_ms >= 0)
    and (timings_local_first_reasoning_ms is null or timings_local_first_reasoning_ms >= 0)
    and (timings_local_first_text_ms is null or timings_local_first_text_ms >= 0)
    and (timings_local_first_token_ms is null or timings_local_first_token_ms >= 0)
    and (timings_upstream_processing_ms is null or timings_upstream_processing_ms >= 0)
    and (timings_local_latency_ms is null or timings_local_latency_ms >= 0)
    and (
      timings_local_latency_ms is null
      or (
        (timings_local_transport_decision_wait_ms is null or timings_local_transport_decision_wait_ms <= timings_local_latency_ms)
        and (timings_local_connect_ms is null or timings_local_connect_ms <= timings_local_latency_ms)
        and (timings_local_headers_ms is null or timings_local_headers_ms <= timings_local_latency_ms)
        and (timings_local_first_event_ms is null or timings_local_first_event_ms <= timings_local_latency_ms)
        and (timings_local_first_reasoning_ms is null or timings_local_first_reasoning_ms <= timings_local_latency_ms)
        and (timings_local_first_text_ms is null or timings_local_first_text_ms <= timings_local_latency_ms)
        and (timings_local_first_token_ms is null or timings_local_first_token_ms <= timings_local_latency_ms)
      )
    )) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_latency_ck',
          message = 'request observation violates model_requests_latency_ck', table = tg_table_name, schema = tg_table_schema;
      end if;
    end;
  end if;
  if json_changed then
    declare
      scheduling_admission_decision_ms bigint;
      scheduling_account_selection_wait_ms bigint;
      scheduling_capacity_used_slots bigint;
      scheduling_capacity_total_slots bigint;
    begin
      scheduling_admission_decision_ms := (new.request_observation_json #>> '{scheduling,admissionDecisionMs}')::bigint;
      scheduling_account_selection_wait_ms := (new.request_observation_json #>> '{scheduling,accountSelectionWaitMs}')::bigint;
      scheduling_capacity_used_slots := (new.request_observation_json #>> '{scheduling,capacityUsedSlots}')::bigint;
      scheduling_capacity_total_slots := (new.request_observation_json #>> '{scheduling,capacityTotalSlots}')::bigint;
      if ((scheduling_admission_decision_ms is null or scheduling_admission_decision_ms >= 0)
    and (scheduling_account_selection_wait_ms is null or scheduling_account_selection_wait_ms >= 0)
    and (
      (scheduling_capacity_used_slots is null and scheduling_capacity_total_slots is null)
      or (
        scheduling_capacity_used_slots >= 0
        and scheduling_capacity_total_slots > 0
        and scheduling_capacity_used_slots <= scheduling_capacity_total_slots
      )
    )) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_runtime_pressure_ck',
          message = 'request observation violates model_requests_runtime_pressure_ck', table = tg_table_name, schema = tg_table_schema;
      end if;
    end;
  end if;
  if json_changed or old.continuation_requested is distinct from new.continuation_requested then
    if (((new.request_observation_json #>> '{continuation,previousResponseIdHash}') is null or (new.request_observation_json #>> '{continuation,previousResponseIdHash}') ~ '^[0-9a-f]{64}$') and (new.continuation_requested or (new.request_observation_json #>> '{continuation,previousResponseIdHash}') is null)) is false then
      raise exception using errcode = '23514', constraint = 'model_requests_continuation_hashes_ck', message = 'request observation violates model_requests_continuation_hashes_ck', table = tg_table_name, schema = tg_table_schema;
    end if;
  end if;
  if json_changed then
    declare
      continuation_unavailable_reason text;
    begin
      continuation_unavailable_reason := (new.request_observation_json #>> '{continuation,unavailableReason}');
      if (continuation_unavailable_reason is null
    or (
      octet_length(continuation_unavailable_reason) between 1 and 64
      and continuation_unavailable_reason ~ '^[a-z][a-z0-9_]*$'
    )) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_continuation_reason_ck',
          message = 'request observation violates model_requests_continuation_reason_ck', table = tg_table_name, schema = tg_table_schema;
      end if;
    end;
  end if;
  if json_changed then
    declare
      transport_connection_id text;
      transport_connection_exit_reason text;
      transport_connection_age_ms bigint;
      transport_connection_idle_ms bigint;
    begin
      transport_connection_id := (new.request_observation_json #>> '{transport,connection,id}');
      transport_connection_exit_reason := (new.request_observation_json #>> '{transport,connection,exitReason}');
      transport_connection_age_ms := (new.request_observation_json #>> '{transport,connection,ageMs}')::bigint;
      transport_connection_idle_ms := (new.request_observation_json #>> '{transport,connection,idleMs}')::bigint;
      if ((
      transport_connection_id is null
      and transport_connection_exit_reason is null
      and transport_connection_age_ms is null
      and transport_connection_idle_ms is null
    )
    or (
      octet_length(transport_connection_id) between 1 and 128
      and transport_connection_id !~ '[[:cntrl:]]'
      and octet_length(transport_connection_exit_reason) between 1 and 64
      and transport_connection_exit_reason ~ '^[a-z][a-z0-9_]*$'
      and transport_connection_age_ms >= 0
      and transport_connection_idle_ms between 0 and transport_connection_age_ms
    )) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_upstream_connection_ck',
          message = 'request observation violates model_requests_upstream_connection_ck', table = tg_table_name, schema = tg_table_schema;
      end if;
    end;
  end if;
  if json_changed or old.completed_at is distinct from new.completed_at or old.recovered_at is distinct from new.recovered_at or old.recovery_attempt_count is distinct from new.recovery_attempt_count or old.recovery_request_id is distinct from new.recovery_request_id then
    declare
      recovery_retry_delay_ms bigint;
      recovery_total_latency_ms bigint;
    begin
      recovery_retry_delay_ms := (new.request_observation_json #>> '{recovery,retryDelayMs}')::bigint;
      recovery_total_latency_ms := (new.request_observation_json #>> '{recovery,totalLatencyMs}')::bigint;
      if (new.recovery_attempt_count >= 0
    and (
      (
        new.recovered_at is null
        and new.recovery_request_id is null
        and recovery_retry_delay_ms is null
        and recovery_total_latency_ms is null
      )
      or (
        new.recovered_at is not null
        and new.recovered_at >= new.completed_at
        and new.recovery_request_id is not null
        and octet_length(new.recovery_request_id) between 1 and 128
        and new.recovery_attempt_count > 0
        and recovery_retry_delay_ms >= 0
        and recovery_total_latency_ms >= recovery_retry_delay_ms
      )
    )) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_recovery_ck',
          message = 'request observation violates model_requests_recovery_ck', table = tg_table_name, schema = tg_table_schema;
      end if;
    end;
  end if;
  if json_changed then
    if (num_nonnulls(new.request_observation_json #>> '{scheduling,capacityUsedSlots}', new.request_observation_json #>> '{scheduling,capacityTotalSlots}') in (0, 2)
        and num_nonnulls(new.request_observation_json #>> '{transport,connection,id}', new.request_observation_json #>> '{transport,connection,exitReason}',
          new.request_observation_json #>> '{transport,connection,ageMs}', new.request_observation_json #>> '{transport,connection,idleMs}') in (0, 4)) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_fact_completeness_ck',
          message = 'request observation violates model_requests_fact_completeness_ck', table = tg_table_name, schema = tg_table_schema;

    end if;
  end if;
  if json_changed or old.recovered_at is distinct from new.recovered_at or old.completed_at is distinct from new.completed_at then
    if (new.recovered_at is null or (new.completed_at is not null
        and new.request_observation_json #>> '{recovery,retryDelayMs}' is not null
        and new.request_observation_json #>> '{recovery,totalLatencyMs}' is not null)) is false then
        raise exception using errcode = '23514', constraint = 'model_requests_fact_completeness_ck',
          message = 'request observation violates model_requests_fact_completeness_ck', table = tg_table_name, schema = tg_table_schema;

    end if;
  end if;
  return null;
end;
$_$;

CREATE FUNCTION client_api_key_sha256(value text) RETURNS bytea
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    RETURN sha256(convert_to(value, 'UTF8'::name));

CREATE FUNCTION enqueue_request_analytics(request_ref text) RETURNS void
    LANGUAGE sql
    AS $$
    insert into request_analytics_pending_requests(request_id)
    select request_ref where request_ref is not null
    on conflict (request_id) do update
    set change_version = nextval('request_analytics_change_version')
$$;

CREATE FUNCTION finish_model_request_lease() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
declare
  expires_at timestamptz;
begin
  if old.outcome = 'running' and new.outcome <> 'running' then
    select lease_expires_at into expires_at from model_request_leases
      where request_id = new.id for update;
    new.recovery_lease_expires_at := coalesce(expires_at, old.recovery_lease_expires_at, old.started_at);
    delete from model_request_leases where request_id = new.id;
  end if;
  return new;
end;
$$;

CREATE FUNCTION model_request_observation_valid(document jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE
    AS $_$
declare
  spec record;
  item jsonb;
  number numeric;
begin
  if document is null or jsonb_typeof(document) <> 'object'
     or octet_length(document::text) > 1048576 then
    return false;
  end if;
  for spec in select * from (values
    ('{}'::text[], array['account','continuation','error','recovery','request','response','routing','scheduling','timings','transport']::text[]),
    ('{request}'::text[], array['clientApiKeyNameSnapshot','clientIp','compact','configRevision','endpoint','protocol','reasoningEffort','reasoningPreset','subagentKind','userAgent']::text[]),
    ('{account}'::text[], array['authenticationKind','email','name','planTypeSnapshot','notesSnapshot']::text[]),
    ('{routing}'::text[], array['groupNamesSnapshot','groupRefs','scope']::text[]),
    ('{error}'::text[], array['message','providerErrorCode','retryAfterMs']::text[]),
    ('{timings}'::text[], array['local','upstream']::text[]),
    ('{timings,local}'::text[], array['connectMs','firstEventMs','firstReasoningMs','firstTextMs','firstTokenMs','headersMs','latencyMs','transportDecisionWaitMs']::text[]),
    ('{timings,upstream}'::text[], array['processingMs','responseMs','apiOverheadMs','engineMs','engineIapiTtftMs','engineServiceTtftMs','engineIapiTbtMs','engineServiceTbtMs']::text[]),
    ('{scheduling}'::text[], array['accountSelectionWaitMs','admissionDecisionMs','capacityTotalSlots','capacityUsedSlots']::text[]),
    ('{transport}'::text[], array['connection','httpVersion','websocketPool']::text[]),
    ('{transport,connection}'::text[], array['ageMs','exitReason','id','idleMs']::text[]),
    ('{response}'::text[], array['responseModel']::text[]),
    ('{continuation}'::text[], array['previousResponseIdHash','unavailableReason']::text[]),
    ('{recovery}'::text[], array['retryDelayMs','totalLatencyMs']::text[])) as groups(path, allowed_keys) loop
    item := document #> spec.path;
    if item is not null and (jsonb_typeof(item) <> 'object' or item - spec.allowed_keys <> '{}'::jsonb) then
      return false;
    end if;
  end loop;
  for spec in select * from (values
    ('{request,configRevision}'::text[], 'number'),
    ('{request,protocol}'::text[], 'string'),
    ('{request,endpoint}'::text[], 'string'),
    ('{request,clientApiKeyNameSnapshot}'::text[], 'string'),
    ('{account,planTypeSnapshot}'::text[], 'string'),
    ('{account,notesSnapshot}'::text[], 'string'),
    ('{account,name}'::text[], 'string'),
    ('{account,email}'::text[], 'string'),
    ('{account,authenticationKind}'::text[], 'string'),
    ('{routing,scope}'::text[], 'string'),
    ('{routing,groupRefs}'::text[], 'array'),
    ('{routing,groupNamesSnapshot}'::text[], 'array'),
    ('{error,providerErrorCode}'::text[], 'string'),
    ('{error,message}'::text[], 'string'),
    ('{error,retryAfterMs}'::text[], 'number'),
    ('{request,clientIp}'::text[], 'string'),
    ('{request,userAgent}'::text[], 'string'),
    ('{request,reasoningEffort}'::text[], 'string'),
    ('{request,reasoningPreset}'::text[], 'string'),
    ('{request,subagentKind}'::text[], 'string'),
    ('{request,compact}'::text[], 'boolean'),
    ('{timings,local,transportDecisionWaitMs}'::text[], 'number'),
    ('{timings,local,connectMs}'::text[], 'number'),
    ('{timings,local,headersMs}'::text[], 'number'),
    ('{timings,local,firstEventMs}'::text[], 'number'),
    ('{timings,local,firstReasoningMs}'::text[], 'number'),
    ('{timings,local,firstTextMs}'::text[], 'number'),
    ('{timings,local,firstTokenMs}'::text[], 'number'),
    ('{timings,local,latencyMs}'::text[], 'number'),
    ('{timings,upstream,processingMs}'::text[], 'number'),
    ('{timings,upstream,apiOverheadMs}'::text[], 'milliseconds'),
    ('{timings,upstream,engineMs}'::text[], 'milliseconds'),
    ('{timings,upstream,engineIapiTtftMs}'::text[], 'milliseconds'),
    ('{timings,upstream,engineServiceTtftMs}'::text[], 'milliseconds'),
    ('{timings,upstream,engineIapiTbtMs}'::text[], 'milliseconds'),
    ('{timings,upstream,engineServiceTbtMs}'::text[], 'milliseconds'),
    ('{scheduling,admissionDecisionMs}'::text[], 'number'),
    ('{scheduling,accountSelectionWaitMs}'::text[], 'number'),
    ('{scheduling,capacityUsedSlots}'::text[], 'number'),
    ('{scheduling,capacityTotalSlots}'::text[], 'number'),
    ('{transport,httpVersion}'::text[], 'string'),
    ('{transport,websocketPool}'::text[], 'string'),
    ('{transport,connection,id}'::text[], 'string'),
    ('{transport,connection,exitReason}'::text[], 'string'),
    ('{transport,connection,ageMs}'::text[], 'number'),
    ('{transport,connection,idleMs}'::text[], 'number'),
    ('{response,responseModel}'::text[], 'string'),
    ('{continuation,previousResponseIdHash}'::text[], 'string'),
    ('{continuation,unavailableReason}'::text[], 'string'),
    ('{recovery,retryDelayMs}'::text[], 'number'),
    ('{recovery,totalLatencyMs}'::text[], 'number'),
    ('{timings,upstream,responseMs}'::text[], 'number')) as fields(path, kind) loop
    item := document #> spec.path;
    if item is null or item = 'null'::jsonb then continue; end if;
    if spec.kind = 'milliseconds' then
      if jsonb_typeof(item) <> 'number' then return false; end if;
      number := (item #>> '{}')::numeric;
      if number < 0 or number > 9223372036854775807 then return false; end if;
      continue;
    end if;
    if jsonb_typeof(item) <> spec.kind then return false; end if;
    if spec.kind = 'number' then
      number := (item #>> '{}')::numeric;
      if item #>> '{}' !~ '^[0-9]+$' or number > 9223372036854775807 then return false; end if;
    elsif spec.kind = 'array' then
      if exists (select 1 from jsonb_array_elements(item) member where jsonb_typeof(member) <> 'string') then return false; end if;
    end if;
  end loop;
  if (document #> '{request,configRevision}' is not null
          and document #> '{request,configRevision}' <> 'null'::jsonb
          and document #> '{request,compact}' in ('true'::jsonb, 'false'::jsonb)
          and coalesce(length(document #>> '{request,protocol}'), 0) > 0
          and coalesce(length(document #>> '{request,endpoint}'), 0) > 0
          and jsonb_typeof(document #> '{routing,scope}') = 'string'
          and jsonb_typeof(document #> '{routing,groupRefs}') = 'array'
          and jsonb_typeof(document #> '{routing,groupNamesSnapshot}') = 'array') is not true then
    return false;
  end if;
  if document #>> '{request,clientIp}' is not null then
    perform (document #>> '{request,clientIp}')::inet;
  end if;
  return true;
exception when invalid_text_representation or numeric_value_out_of_range then
  return false;
end;
$_$;

CREATE FUNCTION observe_ops_analytics_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
begin
    if TG_OP <> 'INSERT' then
        perform enqueue_request_analytics(OLD.model_request_id);
    end if;
    if TG_OP <> 'DELETE' then
        perform enqueue_request_analytics(NEW.model_request_id);
        return NEW;
    end if;
    return OLD;
end
$$;

CREATE FUNCTION observe_request_analytics_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
begin
    if TG_OP = 'DELETE' then
        perform enqueue_request_analytics(OLD.id);
        return OLD;
    end if;
    perform enqueue_request_analytics(NEW.id);
    return NEW;
end
$$;

CREATE FUNCTION sync_request_analytics_1d_counts() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
declare
    old_day timestamptz;
    new_day timestamptz;
    delta bigint;
begin
    if TG_OP <> 'INSERT' then
        old_day := date_bin(interval '1 day',OLD.bucket_start,timestamptz '2000-01-01 UTC');
    end if;
    if TG_OP <> 'DELETE' then
        new_day := date_bin(interval '1 day',NEW.bucket_start,timestamptz '2000-01-01 UTC');
    end if;
    if TG_OP = 'UPDATE' and
        (old_day,OLD.collection,OLD.dimensions[1],OLD.outcome,OLD.schema_version,OLD.histogram_version)
        is not distinct from
        (new_day,NEW.collection,NEW.dimensions[1],NEW.outcome,NEW.schema_version,NEW.histogram_version) then
        if NEW.collection in ('account','key') then
            delta := NEW.request_count - OLD.request_count;
            if delta <> 0 then
                update request_analytics_1d_counts set request_count=request_count+delta
                where collection=NEW.collection and bucket_start=new_day and dimension=NEW.dimensions[1]
                    and outcome=NEW.outcome and schema_version=NEW.schema_version and histogram_version=NEW.histogram_version;
            end if;
        end if;
        return null;
    end if;
    if TG_OP <> 'INSERT' and OLD.collection in ('account','key') then
        update request_analytics_1d_counts set request_count=request_count-OLD.request_count
        where collection=OLD.collection and bucket_start=old_day and dimension=OLD.dimensions[1]
            and outcome=OLD.outcome and schema_version=OLD.schema_version and histogram_version=OLD.histogram_version;
        delete from request_analytics_1d_counts
        where collection=OLD.collection and bucket_start=old_day and dimension=OLD.dimensions[1]
            and outcome=OLD.outcome and schema_version=OLD.schema_version and histogram_version=OLD.histogram_version
            and request_count=0;
    end if;
    if TG_OP <> 'DELETE' and NEW.collection in ('account','key') then
        insert into request_analytics_1d_counts values(new_day,NEW.collection,NEW.dimensions[1],NEW.outcome,NEW.schema_version,NEW.histogram_version,NEW.request_count)
        on conflict(collection,bucket_start,dimension,outcome,schema_version,histogram_version)
        do update set request_count=request_analytics_1d_counts.request_count+excluded.request_count;
    end if;
    return null;
end;
$$;

CREATE TABLE account_group_accounts (
    account_group_id text NOT NULL,
    provider_account_id text NOT NULL,
    created_at timestamp with time zone NOT NULL
);

CREATE TABLE account_groups (
    id text NOT NULL,
    name text NOT NULL,
    description text,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    color text NOT NULL,
    fast_mode text DEFAULT 'default'::text NOT NULL,
    allow_image_generation boolean DEFAULT true NOT NULL,
    CONSTRAINT account_groups_color_ck CHECK ((color ~ '^#[0-9A-F]{8}$'::text)),
    CONSTRAINT account_groups_description_ck CHECK (((description IS NULL) OR ((octet_length(description) <= 4096) AND (description !~ '[[:cntrl:]]'::text)))),
    CONSTRAINT account_groups_fast_mode_check CHECK ((fast_mode = ANY (ARRAY['default'::text, 'enabled'::text, 'disabled'::text]))),
    CONSTRAINT account_groups_id_ck CHECK ((id ~ '^grp_[0-9a-f]{32}$'::text)),
    CONSTRAINT account_groups_name_ck CHECK ((((char_length(btrim(name)) >= 1) AND (char_length(btrim(name)) <= 100)) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT account_groups_time_ck CHECK ((created_at <= updated_at))
);

CREATE TABLE admin_audit_events (
    id text NOT NULL,
    actor_kind text NOT NULL,
    actor_admin_user_id text,
    actor_ref text NOT NULL,
    admin_request_id text,
    action text NOT NULL,
    entity_kind text NOT NULL,
    entity_ref text NOT NULL,
    config_revision bigint,
    changed_fields text[] DEFAULT '{}'::text[] NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT admin_audit_events_actor_identity_ck CHECK (((actor_admin_user_id IS NULL) OR ((actor_kind = 'admin_session'::text) AND (actor_ref = ('admin:'::text || actor_admin_user_id))))),
    CONSTRAINT admin_audit_events_actor_kind_ck CHECK ((actor_kind = ANY (ARRAY['admin_session'::text, 'admin_api_key'::text, 'system'::text, 'anonymous'::text]))),
    CONSTRAINT admin_audit_events_changed_fields_ck CHECK (((cardinality(changed_fields) <= 64) AND (array_position(changed_fields, NULL::text) IS NULL))),
    CONSTRAINT admin_audit_events_config_revision_ck CHECK (((config_revision IS NULL) OR (config_revision > 0)))
);

CREATE TABLE admin_users (
    id text NOT NULL,
    password_hash text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT admin_users_time_ck CHECK ((created_at <= updated_at))
);

-- Claude 隐私映射卷快照与数据库归档共用一条记录；未初始化的映射卷不产生快照字段。
CREATE TABLE backup_records (
    id text NOT NULL,
    trigger_kind text NOT NULL,
    status text NOT NULL,
    scheduled_at timestamp with time zone,
    object_key text NOT NULL,
    size_bytes bigint,
    sha256 text,
    expires_at timestamp with time zone,
    attempt_count integer DEFAULT 0 NOT NULL,
    error_code text,
    error_message text,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    privacy_object_key text,
    privacy_size_bytes bigint,
    privacy_sha256 text,
    CONSTRAINT backup_records_artifact_ck CHECK ((((status <> ALL (ARRAY['uploading'::text, 'completed'::text])) OR ((size_bytes IS NOT NULL) AND (sha256 IS NOT NULL))) AND ((size_bytes IS NULL) OR (size_bytes >= 0)) AND ((sha256 IS NULL) OR (sha256 ~ '^[0-9a-f]{64}$'::text)))),
    CONSTRAINT backup_records_attempt_ck CHECK ((attempt_count >= 0)),
    CONSTRAINT backup_records_completion_order_ck CHECK (((completed_at IS NULL) OR (completed_at >= started_at))),
    CONSTRAINT backup_records_error_ck CHECK ((((error_code IS NULL) = (error_message IS NULL)) AND ((status <> 'failed'::text) OR ((error_code IS NOT NULL) AND (error_message IS NOT NULL))) AND ((status <> ALL (ARRAY['queued'::text, 'dumping'::text, 'uploading'::text, 'completed'::text])) OR (error_code IS NULL)))),
    CONSTRAINT backup_records_expiry_ck CHECK (((expires_at IS NULL) OR (expires_at >= created_at))),
    CONSTRAINT backup_records_id_ck CHECK ((id ~ '^backup_[0-9a-f]{32}$'::text)),
    CONSTRAINT backup_records_lifecycle_ck CHECK (((created_at <= updated_at) AND ((started_at IS NULL) OR (started_at >= created_at)) AND ((completed_at IS NULL) OR (completed_at >= created_at)) AND (((status = 'queued'::text) AND (started_at IS NULL) AND (completed_at IS NULL)) OR ((status = ANY (ARRAY['dumping'::text, 'uploading'::text])) AND (started_at IS NOT NULL) AND (completed_at IS NULL)) OR ((status = ANY (ARRAY['completed'::text, 'failed'::text, 'deleting'::text])) AND (started_at IS NOT NULL) AND (completed_at IS NOT NULL))))),
    CONSTRAINT backup_records_privacy_store_ck CHECK (((((privacy_object_key IS NULL) AND (privacy_size_bytes IS NULL) AND (privacy_sha256 IS NULL)) OR ((privacy_object_key IS NOT NULL) AND (privacy_size_bytes IS NOT NULL) AND (privacy_sha256 IS NOT NULL))) AND ((privacy_size_bytes IS NULL) OR (privacy_size_bytes >= 0)) AND ((privacy_sha256 IS NULL) OR (privacy_sha256 ~ '^[0-9a-f]{64}$'::text)) AND ((privacy_object_key IS NULL) OR (status <> ALL (ARRAY['queued'::text, 'dumping'::text]))))),
    CONSTRAINT backup_records_status_ck CHECK ((status = ANY (ARRAY['queued'::text, 'dumping'::text, 'uploading'::text, 'completed'::text, 'failed'::text, 'deleting'::text]))),
    CONSTRAINT backup_records_trigger_kind_ck CHECK (((trigger_kind = ANY (ARRAY['manual'::text, 'scheduled'::text])) AND (((trigger_kind = 'manual'::text) AND (scheduled_at IS NULL)) OR ((trigger_kind = 'scheduled'::text) AND (scheduled_at IS NOT NULL)))))
);

CREATE TABLE backup_settings (
    id bigint NOT NULL,
    storage_revision bigint DEFAULT 1 NOT NULL,
    endpoint text,
    region text,
    bucket text,
    access_key_id text,
    secret_access_key text,
    prefix text,
    force_path_style boolean DEFAULT false NOT NULL,
    schedule_enabled boolean DEFAULT false NOT NULL,
    cron_expression text,
    schedule_timezone text DEFAULT 'Asia/Shanghai'::text,
    retention_days bigint DEFAULT 0 NOT NULL,
    retention_count bigint DEFAULT 0 NOT NULL,
    next_run_at timestamp with time zone,
    last_verified_at timestamp with time zone,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT backup_settings_cursor_ck CHECK ((((NOT schedule_enabled) AND (next_run_at IS NULL)) OR (schedule_enabled AND (next_run_at IS NOT NULL)))),
    CONSTRAINT backup_settings_endpoint_ck CHECK (((endpoint IS NULL) OR ((endpoint ~ '^https?://'::text) AND (endpoint !~ '[[:space:][:cntrl:]]'::text) AND (endpoint !~ '://[^/]*@'::text)))),
    CONSTRAINT backup_settings_lengths_ck CHECK ((((bucket IS NULL) OR (((octet_length(bucket) >= 1) AND (octet_length(bucket) <= 255)) AND (bucket !~ '[[:cntrl:]]'::text))) AND ((region IS NULL) OR (((octet_length(region) >= 1) AND (octet_length(region) <= 64)) AND (region !~ '[[:cntrl:]]'::text))) AND ((prefix IS NULL) OR ((octet_length(prefix) <= 1024) AND (prefix !~ '[[:cntrl:]]'::text))) AND ((access_key_id IS NULL) OR (((octet_length(access_key_id) >= 1) AND (octet_length(access_key_id) <= 255)) AND (access_key_id !~ '[[:cntrl:]]'::text))) AND ((secret_access_key IS NULL) OR (((octet_length(secret_access_key) >= 1) AND (octet_length(secret_access_key) <= 512)) AND (secret_access_key !~ '[[:cntrl:]]'::text))))),
    CONSTRAINT backup_settings_revision_ck CHECK ((storage_revision > 0)),
    CONSTRAINT backup_settings_schedule_ck CHECK ((((cron_expression IS NULL) OR (cron_expression ~ '^[^[:space:]]+([[:space:]]+[^[:space:]]+){4}$'::text)) AND ((schedule_timezone IS NULL) OR (((octet_length(schedule_timezone) >= 1) AND (octet_length(schedule_timezone) <= 64)) AND (schedule_timezone !~ '[[:cntrl:]]'::text))) AND (retention_days >= 0) AND (retention_count >= 0) AND ((NOT schedule_enabled) OR ((endpoint IS NOT NULL) AND (bucket IS NOT NULL) AND (region IS NOT NULL) AND (access_key_id IS NOT NULL) AND (secret_access_key IS NOT NULL) AND (prefix IS NOT NULL) AND (cron_expression IS NOT NULL) AND (schedule_timezone IS NOT NULL) AND (last_verified_at IS NOT NULL))))),
    CONSTRAINT backup_settings_singleton_ck CHECK ((id = 1))
);

CREATE TABLE client_api_key_budget_windows (
    client_api_key_id text NOT NULL,
    daily_start timestamp with time zone NOT NULL,
    daily_end timestamp with time zone NOT NULL,
    weekly_start timestamp with time zone NOT NULL,
    weekly_end timestamp with time zone NOT NULL,
    daily_used_usd numeric(20,10) DEFAULT 0 NOT NULL,
    weekly_used_usd numeric(20,10) DEFAULT 0 NOT NULL,
    daily_explicit boolean DEFAULT false NOT NULL,
    weekly_explicit boolean DEFAULT false NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    last_operation_request jsonb,
    charge_history_since timestamp with time zone,
    CONSTRAINT client_api_key_budget_windows_daily_used_usd_check CHECK ((daily_used_usd >= (0)::numeric)),
    CONSTRAINT client_api_key_budget_windows_revision_check CHECK ((revision >= 0)),
    CONSTRAINT client_api_key_budget_windows_weekly_used_usd_check CHECK ((weekly_used_usd >= (0)::numeric)),
    CONSTRAINT daily_explicit_window_order CHECK (((NOT daily_explicit) OR (daily_start < daily_end))),
    CONSTRAINT weekly_explicit_window_order CHECK (((NOT weekly_explicit) OR (weekly_start < weekly_end)))
);

CREATE TABLE client_api_key_charge_events (
    request_id text NOT NULL,
    client_api_key_id text NOT NULL,
    completed_at timestamp with time zone NOT NULL,
    amount_usd numeric(20,10) NOT NULL,
    CONSTRAINT client_api_key_charge_events_amount_usd_check CHECK ((amount_usd >= (0)::numeric))
);

CREATE TABLE client_api_key_groups (
    client_api_key_id text NOT NULL,
    account_group_id text NOT NULL,
    created_at timestamp with time zone NOT NULL
);

CREATE TABLE client_api_keys (
    id text NOT NULL,
    name text NOT NULL,
    label text,
    key text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    max_concurrency bigint DEFAULT 0 NOT NULL,
    requests_per_minute bigint DEFAULT 0 NOT NULL,
    last_used_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    daily_limit_usd numeric(20,10) DEFAULT 0 NOT NULL,
    weekly_limit_usd numeric(20,10) DEFAULT 0 NOT NULL,
    provider_request_profiles_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    peer_sharing_json jsonb DEFAULT '{"enabled": false, "sharePool": false, "sharePlans": false, "shareQuota": false}'::jsonb NOT NULL,
    CONSTRAINT client_api_keys_daily_limit_usd_check CHECK ((daily_limit_usd >= (0)::numeric)),
    CONSTRAINT client_api_keys_key_ck CHECK (((key <> ''::text) AND ((key COLLATE "C") !~ '[^!-~]'::text))),
    CONSTRAINT client_api_keys_limits_ck CHECK (((max_concurrency >= 0) AND (requests_per_minute >= 0))),
    CONSTRAINT client_api_keys_request_profiles_object CHECK ((jsonb_typeof(provider_request_profiles_json) = 'object'::text)),
    CONSTRAINT client_api_keys_time_ck CHECK ((created_at <= updated_at)),
    CONSTRAINT client_api_keys_weekly_limit_usd_check CHECK ((weekly_limit_usd >= (0)::numeric))
);

CREATE TABLE model_request_leases (
    request_id text NOT NULL,
    lease_expires_at timestamp with time zone NOT NULL
);

CREATE TABLE model_requests (
    id text NOT NULL,
    client_api_key_ref text NOT NULL,
    operation text NOT NULL,
    client_transport text NOT NULL,
    requested_model_id text,
    provider_kind text,
    upstream_model_id text,
    service_tier text,
    provider_account_ref text,
    upstream_transport text,
    provider_observation_json jsonb,
    attempt_count integer DEFAULT 0 NOT NULL,
    upstream_send_state text DEFAULT 'not_sent'::text NOT NULL,
    downstream_committed_at timestamp with time zone,
    outcome text DEFAULT 'running'::text NOT NULL,
    client_status_code integer,
    upstream_status_code integer,
    client_response_id bytea,
    upstream_request_id text,
    upstream_response_id bytea,
    error_kind text,
    input_tokens bigint,
    output_tokens bigint,
    cached_input_tokens bigint,
    cache_write_input_tokens bigint,
    reasoning_output_tokens bigint,
    image_input_tokens bigint,
    image_output_tokens bigint,
    total_tokens bigint,
    image_generation_succeeded boolean,
    cost_source text DEFAULT 'unavailable'::text NOT NULL,
    cost_amount numeric(20,10),
    cost_currency text,
    request_kind text,
    image_generation_requested boolean DEFAULT false NOT NULL,
    started_at timestamp with time zone NOT NULL,
    recovery_lease_expires_at timestamp with time zone,
    completed_at timestamp with time zone,
    error_details text,
    continuation_affinity_hash text,
    continuation_requested boolean DEFAULT false NOT NULL,
    recovery_request_id text,
    recovered_at timestamp with time zone,
    recovery_attempt_count integer DEFAULT 0 NOT NULL,
    diagnostic_trace_json jsonb,
    billing_snapshot_json jsonb,
    request_observation_json jsonb NOT NULL,
    CONSTRAINT model_requests_account_ref_ck CHECK (((provider_account_ref IS NULL) OR ((octet_length(provider_account_ref) >= 1) AND (octet_length(provider_account_ref) <= 128)))),
    CONSTRAINT model_requests_billing_snapshot_json_check CHECK (((billing_snapshot_json IS NULL) OR (jsonb_typeof(billing_snapshot_json) = 'object'::text))),
    CONSTRAINT model_requests_client_ref_ck CHECK (((octet_length(client_api_key_ref) >= 1) AND (octet_length(client_api_key_ref) <= 128))),
    CONSTRAINT model_requests_continuation_hashes_ck CHECK (((continuation_affinity_hash IS NULL) OR (continuation_affinity_hash ~ '^[0-9a-f]{64}$'::text))),
    CONSTRAINT model_requests_cost_ck CHECK (((cost_source = ANY (ARRAY['provider_reported'::text, 'calculated'::text, 'unavailable'::text])) AND (((cost_source = 'unavailable'::text) AND (cost_amount IS NULL) AND (cost_currency IS NULL)) OR ((cost_source = ANY (ARRAY['provider_reported'::text, 'calculated'::text])) AND (cost_amount IS NOT NULL) AND (cost_amount >= (0)::numeric) AND (cost_currency ~ '^[A-Z]{3}$'::text))))),
    CONSTRAINT model_requests_diagnostic_trace_object CHECK (((diagnostic_trace_json IS NULL) OR (jsonb_typeof(diagnostic_trace_json) = 'object'::text))),
    CONSTRAINT model_requests_fact_completeness_ck CHECK (((cost_source = 'unavailable'::text) OR (cost_currency IS NOT NULL))),
    CONSTRAINT model_requests_image_generation_ck CHECK ((((NOT image_generation_requested) AND (image_generation_succeeded IS NULL)) OR (image_generation_requested AND (((outcome = 'running'::text) AND (image_generation_succeeded IS NULL)) OR ((outcome <> 'running'::text) AND (image_generation_succeeded IS NOT NULL)))))),
    CONSTRAINT model_requests_lifecycle_ck CHECK (((started_at <= recovery_lease_expires_at) AND (((outcome = 'running'::text) AND (completed_at IS NULL)) OR ((outcome <> 'running'::text) AND (completed_at IS NOT NULL) AND (completed_at >= started_at))))),
    CONSTRAINT model_requests_outcome_ck CHECK ((outcome = ANY (ARRAY['running'::text, 'succeeded'::text, 'failed'::text, 'cancelled'::text, 'incomplete'::text]))),
    CONSTRAINT model_requests_provider_observation_ck CHECK (((provider_observation_json IS NULL) OR (jsonb_typeof(provider_observation_json) = 'object'::text))),
    CONSTRAINT model_requests_recovery_ck CHECK ((recovery_attempt_count >= 0)),
    CONSTRAINT model_requests_revision_attempt_ck CHECK ((attempt_count >= 0)),
    CONSTRAINT model_requests_send_state_ck CHECK (((upstream_send_state = ANY (ARRAY['not_sent'::text, 'sent'::text, 'ambiguous'::text])) AND (((attempt_count = 0) AND (upstream_send_state = 'not_sent'::text)) OR ((attempt_count > 0) AND (provider_kind IS NOT NULL) AND (provider_account_ref IS NOT NULL) AND (upstream_transport IS NOT NULL))))),
    CONSTRAINT model_requests_service_tier_ck CHECK (((service_tier IS NULL) OR (((octet_length(service_tier) >= 1) AND (octet_length(service_tier) <= 64)) AND (service_tier !~ '[[:cntrl:]]'::text)))),
    CONSTRAINT model_requests_status_ck CHECK ((((client_status_code IS NULL) OR ((client_status_code >= 100) AND (client_status_code <= 599))) AND ((upstream_status_code IS NULL) OR ((upstream_status_code >= 100) AND (upstream_status_code <= 599))))),
    CONSTRAINT model_requests_tokens_ck CHECK ((((input_tokens IS NULL) OR (input_tokens >= 0)) AND ((output_tokens IS NULL) OR (output_tokens >= 0)) AND ((cached_input_tokens IS NULL) OR (cached_input_tokens >= 0)) AND ((cache_write_input_tokens IS NULL) OR (cache_write_input_tokens >= 0)) AND ((reasoning_output_tokens IS NULL) OR (reasoning_output_tokens >= 0)) AND ((image_input_tokens IS NULL) OR (image_input_tokens >= 0)) AND ((image_output_tokens IS NULL) OR (image_output_tokens >= 0)) AND ((total_tokens IS NULL) OR (total_tokens >= 0))))
);

CREATE VIEW model_request_observations AS
 SELECT mr.id,
    mr.client_api_key_ref,
    mr.operation,
    mr.client_transport,
    mr.requested_model_id,
    mr.provider_kind,
    mr.upstream_model_id,
    mr.service_tier,
    mr.provider_account_ref,
    mr.upstream_transport,
    mr.provider_observation_json,
    mr.attempt_count,
    mr.upstream_send_state,
    mr.downstream_committed_at,
    mr.outcome,
    mr.client_status_code,
    mr.upstream_status_code,
    mr.client_response_id,
    mr.upstream_request_id,
    mr.upstream_response_id,
    mr.error_kind,
    mr.input_tokens,
    mr.output_tokens,
    mr.cached_input_tokens,
    mr.cache_write_input_tokens,
    mr.reasoning_output_tokens,
    mr.image_input_tokens,
    mr.image_output_tokens,
    mr.total_tokens,
    mr.image_generation_succeeded,
    mr.cost_source,
    mr.cost_amount,
    mr.cost_currency,
    mr.request_kind,
    mr.image_generation_requested,
    mr.started_at,
    mr.recovery_lease_expires_at,
    mr.completed_at,
    mr.error_details,
    mr.continuation_affinity_hash,
    mr.continuation_requested,
    mr.recovery_request_id,
    mr.recovered_at,
    mr.recovery_attempt_count,
    mr.diagnostic_trace_json,
    mr.billing_snapshot_json,
    mr.request_observation_json,
    COALESCE(lease.lease_expires_at, mr.recovery_lease_expires_at) AS effective_recovery_lease_expires_at,
    ((mr.request_observation_json #>> '{request,configRevision}'::text[]))::bigint AS config_revision,
    (mr.request_observation_json #>> '{request,protocol}'::text[]) AS protocol,
    (mr.request_observation_json #>> '{request,endpoint}'::text[]) AS endpoint,
    (mr.request_observation_json #>> '{account,name}'::text[]) AS provider_account_name_snapshot,
    (mr.request_observation_json #>> '{account,email}'::text[]) AS provider_account_email_snapshot,
    (mr.request_observation_json #>> '{account,authenticationKind}'::text[]) AS provider_account_authentication_kind_snapshot,
    (mr.request_observation_json #>> '{routing,scope}'::text[]) AS routing_scope,
    ARRAY( SELECT jsonb_array_elements_text((mr.request_observation_json #> '{routing,groupRefs}'::text[])) AS jsonb_array_elements_text) AS routing_group_refs,
    (mr.request_observation_json #> '{routing,groupNamesSnapshot}'::text[]) AS routing_group_names_snapshot,
    (mr.request_observation_json #>> '{error,providerErrorCode}'::text[]) AS provider_error_code,
    (mr.request_observation_json #>> '{error,message}'::text[]) AS error_message,
    ((mr.request_observation_json #>> '{error,retryAfterMs}'::text[]))::bigint AS retry_after_ms,
    ((mr.request_observation_json #>> '{request,clientIp}'::text[]))::inet AS client_ip,
    (mr.request_observation_json #>> '{request,userAgent}'::text[]) AS user_agent,
    (mr.request_observation_json #>> '{request,reasoningEffort}'::text[]) AS reasoning_effort,
    (mr.request_observation_json #>> '{request,reasoningPreset}'::text[]) AS reasoning_preset,
    (mr.request_observation_json #>> '{request,subagentKind}'::text[]) AS subagent_kind,
    ((mr.request_observation_json #>> '{request,compact}'::text[]))::boolean AS compact,
    measured.transport_decision_wait_ms,
    measured.connect_ms,
    measured.headers_ms,
    measured.first_event_ms,
    measured.first_reasoning_ms,
    measured.first_text_ms,
    measured.first_token_ms,
    measured.latency_ms,
    measured.provider_processing_ms,
    measured.admission_decision_ms,
    measured.account_selection_wait_ms,
    measured.capacity_used_slots,
    measured.capacity_total_slots,
    (mr.request_observation_json #>> '{transport,httpVersion}'::text[]) AS http_version,
    (mr.request_observation_json #>> '{transport,websocketPool}'::text[]) AS websocket_pool,
    (mr.request_observation_json #>> '{transport,connection,id}'::text[]) AS upstream_connection_id,
    (mr.request_observation_json #>> '{transport,connection,exitReason}'::text[]) AS upstream_connection_exit_reason,
    ((mr.request_observation_json #>> '{transport,connection,ageMs}'::text[]))::bigint AS upstream_connection_age_ms,
    ((mr.request_observation_json #>> '{transport,connection,idleMs}'::text[]))::bigint AS upstream_connection_idle_ms,
    (mr.request_observation_json #>> '{response,responseModel}'::text[]) AS upstream_response_model,
    (mr.request_observation_json #>> '{continuation,previousResponseIdHash}'::text[]) AS continuation_previous_response_id_hash,
    (mr.request_observation_json #>> '{continuation,unavailableReason}'::text[]) AS continuation_unavailable_reason,
    ((mr.request_observation_json #>> '{recovery,retryDelayMs}'::text[]))::bigint AS recovery_retry_delay_ms,
    ((mr.request_observation_json #>> '{recovery,totalLatencyMs}'::text[]))::bigint AS recovery_total_latency_ms,
    ((mr.request_observation_json #>> '{timings,upstream,apiOverheadMs}'::text[]))::double precision AS upstream_api_overhead_ms,
    ((mr.request_observation_json #>> '{timings,upstream,engineMs}'::text[]))::double precision AS upstream_engine_ms,
    ((mr.request_observation_json #>> '{timings,upstream,engineIapiTtftMs}'::text[]))::double precision AS upstream_engine_iapi_ttft_ms,
    ((mr.request_observation_json #>> '{timings,upstream,engineServiceTtftMs}'::text[]))::double precision AS upstream_engine_service_ttft_ms,
    ((mr.request_observation_json #>> '{timings,upstream,engineIapiTbtMs}'::text[]))::double precision AS upstream_engine_iapi_tbt_ms,
    ((mr.request_observation_json #>> '{timings,upstream,engineServiceTbtMs}'::text[]))::double precision AS upstream_engine_service_tbt_ms,
    measured.upstream_response_ms,
    (mr.request_observation_json #>> '{request,clientApiKeyNameSnapshot}'::text[]) AS client_api_key_name_snapshot,
    (mr.request_observation_json #>> '{account,planTypeSnapshot}'::text[]) AS provider_account_plan_type_snapshot,
    (mr.request_observation_json #>> '{account,notesSnapshot}'::text[]) AS provider_account_notes_snapshot,
    (mr.request_observation_json -> 'prompt_guard'::text) AS prompt_guard_summary,
    (mr.request_observation_json #>> '{prompt_guard,action}'::text[]) AS prompt_guard_action,
    (mr.request_observation_json #>> '{prompt_guard,mode}'::text[]) AS prompt_guard_mode,
    ((mr.request_observation_json #>> '{prompt_guard,decisionScore}'::text[]))::bigint AS prompt_guard_decision_score,
    (mr.request_observation_json #>> '{prompt_guard,policyVersion}'::text[]) AS prompt_guard_policy_version,
    (mr.request_observation_json #>> '{prompt_guard,review,result}'::text[]) AS prompt_guard_review_result
   FROM ((model_requests mr
     LEFT JOIN model_request_leases lease ON ((lease.request_id = mr.id)))
     CROSS JOIN LATERAL ( SELECT ((mr.request_observation_json #>> '{timings,local,transportDecisionWaitMs}'::text[]))::bigint AS transport_decision_wait_ms,
            ((mr.request_observation_json #>> '{timings,local,connectMs}'::text[]))::bigint AS connect_ms,
            ((mr.request_observation_json #>> '{timings,local,headersMs}'::text[]))::bigint AS headers_ms,
            ((mr.request_observation_json #>> '{timings,local,firstEventMs}'::text[]))::bigint AS first_event_ms,
            ((mr.request_observation_json #>> '{timings,local,firstReasoningMs}'::text[]))::bigint AS first_reasoning_ms,
            ((mr.request_observation_json #>> '{timings,local,firstTextMs}'::text[]))::bigint AS first_text_ms,
            ((mr.request_observation_json #>> '{timings,local,firstTokenMs}'::text[]))::bigint AS first_token_ms,
            ((mr.request_observation_json #>> '{timings,local,latencyMs}'::text[]))::bigint AS latency_ms,
            ((mr.request_observation_json #>> '{timings,upstream,processingMs}'::text[]))::bigint AS provider_processing_ms,
            ((mr.request_observation_json #>> '{scheduling,admissionDecisionMs}'::text[]))::bigint AS admission_decision_ms,
            ((mr.request_observation_json #>> '{scheduling,accountSelectionWaitMs}'::text[]))::bigint AS account_selection_wait_ms,
            ((mr.request_observation_json #>> '{scheduling,capacityUsedSlots}'::text[]))::bigint AS capacity_used_slots,
            ((mr.request_observation_json #>> '{scheduling,capacityTotalSlots}'::text[]))::bigint AS capacity_total_slots,
            ((mr.request_observation_json #>> '{timings,upstream,responseMs}'::text[]))::bigint AS upstream_response_ms) measured);

CREATE TABLE ops_events (
    id text NOT NULL,
    model_request_id text,
    attempt_index integer,
    level text NOT NULL,
    component text NOT NULL,
    operation text NOT NULL,
    provider_kind text,
    provider_account_ref text,
    provider_account_name_snapshot text,
    provider_account_email_snapshot text,
    provider_account_authentication_kind_snapshot text,
    upstream_model_id text,
    failure_kind text NOT NULL,
    status_code integer,
    provider_error_code text,
    retry_after_ms bigint,
    upstream_request_id text,
    latency_ms bigint,
    message text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    upstream_send_state text,
    error_details text,
    provider_account_plan_type_snapshot text,
    CONSTRAINT ops_events_account_ref_ck CHECK (((provider_account_ref IS NULL) OR ((octet_length(provider_account_ref) >= 1) AND (octet_length(provider_account_ref) <= 128)))),
    CONSTRAINT ops_events_level_ck CHECK ((level = ANY (ARRAY['warning'::text, 'error'::text]))),
    CONSTRAINT ops_events_request_attempt_ck CHECK ((((model_request_id IS NULL) AND (attempt_index IS NULL)) OR ((model_request_id IS NOT NULL) AND (attempt_index IS NOT NULL) AND (attempt_index > 0)))),
    CONSTRAINT ops_events_upstream_send_state_ck CHECK (((upstream_send_state IS NULL) OR (upstream_send_state = ANY (ARRAY['not_sent'::text, 'sent'::text, 'ambiguous'::text])))),
    CONSTRAINT ops_events_values_ck CHECK ((((status_code IS NULL) OR ((status_code >= 100) AND (status_code <= 599))) AND ((retry_after_ms IS NULL) OR (retry_after_ms >= 0)) AND ((latency_ms IS NULL) OR (latency_ms >= 0))))
);

CREATE TABLE outbound_proxies (
    id text NOT NULL,
    name text NOT NULL,
    proxy_url text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    last_test_at timestamp with time zone,
    last_test_success boolean,
    last_test_latency_ms bigint,
    last_test_ip text,
    last_test_message text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    location_country text,
    location_region text,
    location_city text,
    location_timezone text,
    last_test_ipv4 text,
    last_test_ipv6 text,
    auto_location boolean DEFAULT false NOT NULL,
    detected_location_json jsonb,
    last_location_detection_json jsonb DEFAULT '{"status": "notRequested"}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    max_websocket_connections bigint DEFAULT 0 NOT NULL,
    CONSTRAINT outbound_proxies_location_complete CHECK (((num_nonnulls(location_country, location_region, location_city, location_timezone) = 0) OR ((num_nonnulls(location_country, location_region, location_city, location_timezone) = 4) AND (location_country ~ '^[A-Z]{2}$'::text) AND ((char_length(btrim(location_region)) >= 1) AND (char_length(btrim(location_region)) <= 128)) AND ((char_length(btrim(location_city)) >= 1) AND (char_length(btrim(location_city)) <= 128)) AND (char_length(location_timezone) > 0)))),
    CONSTRAINT outbound_proxies_max_websocket_connections_check CHECK (((max_websocket_connections >= 0) AND (max_websocket_connections <= '4294967295'::bigint))),
    CONSTRAINT outbound_proxies_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 100))),
    CONSTRAINT outbound_proxies_revision_check CHECK ((revision > 0))
);

CREATE TABLE plugin_account_group_resources (
    instance_id uuid NOT NULL,
    resource_key text NOT NULL,
    group_id text NOT NULL,
    CONSTRAINT plugin_account_group_resources_resource_key_check CHECK ((resource_key ~ '^[a-z0-9][a-z0-9_.-]{0,63}$'::text))
);

CREATE TABLE plugin_artifact_credentials (
    artifact_sha256 text NOT NULL,
    credential_id uuid NOT NULL
);

CREATE TABLE plugin_artifact_platforms (
    plugin_id text NOT NULL,
    version text NOT NULL,
    platform text NOT NULL,
    artifact_sha256 text NOT NULL
);

CREATE TABLE plugin_artifacts (
    sha256 text NOT NULL,
    plugin_id text NOT NULL,
    version text NOT NULL,
    metadata_json jsonb NOT NULL,
    source_json jsonb NOT NULL,
    outbound_proxy_id text,
    archive bytea NOT NULL,
    installed_at timestamp with time zone DEFAULT now() NOT NULL,
    accepted_at timestamp with time zone,
    CONSTRAINT plugin_artifacts_archive_check CHECK (((octet_length(archive) >= 1) AND (octet_length(archive) <= 33554432))),
    CONSTRAINT plugin_artifacts_metadata_json_check CHECK ((jsonb_typeof(metadata_json) = 'object'::text)),
    CONSTRAINT plugin_artifacts_sha256_check CHECK ((sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT plugin_artifacts_source_json_check CHECK ((jsonb_typeof(source_json) = 'object'::text)),
    CONSTRAINT plugin_artifacts_version_check CHECK (((length(version) >= 1) AND (length(version) <= 128)))
);

CREATE TABLE plugin_client_api_key_resources (
    instance_id uuid NOT NULL,
    resource_key text NOT NULL,
    key_id text NOT NULL,
    CONSTRAINT plugin_client_api_key_resources_resource_key_check CHECK ((resource_key ~ '^[a-z0-9][a-z0-9_.-]{0,63}$'::text))
);

CREATE TABLE plugin_instance_secrets (
    instance_id uuid NOT NULL,
    secrets_json jsonb NOT NULL,
    CONSTRAINT plugin_instance_secrets_secrets_json_check CHECK (((jsonb_typeof(secrets_json) = 'object'::text) AND (octet_length((secrets_json)::text) <= 65536)))
);

CREATE TABLE plugin_instances (
    id uuid NOT NULL,
    artifact_sha256 text NOT NULL,
    name text NOT NULL,
    enabled boolean NOT NULL,
    configuration_json jsonb NOT NULL,
    bindings_json jsonb NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT plugin_instances_bindings_json_check CHECK ((jsonb_typeof(bindings_json) = 'array'::text)),
    CONSTRAINT plugin_instances_configuration_json_check CHECK (((jsonb_typeof(configuration_json) = 'object'::text) AND (octet_length((configuration_json)::text) <= 65536))),
    CONSTRAINT plugin_instances_name_check CHECK (((length(name) >= 1) AND (length(name) <= 128))),
    CONSTRAINT plugin_instances_revision_check CHECK ((revision > 0))
);

CREATE TABLE plugin_source_credentials (
    id uuid NOT NULL,
    info_json jsonb NOT NULL,
    secret_json jsonb NOT NULL,
    CONSTRAINT plugin_source_credentials_info_json_check CHECK ((jsonb_typeof(info_json) = 'object'::text)),
    CONSTRAINT plugin_source_credentials_secret_json_check CHECK ((jsonb_typeof(secret_json) = 'object'::text))
);

CREATE TABLE plugin_state_generations (
    id uuid NOT NULL,
    transition_id uuid,
    instance_id uuid NOT NULL,
    namespace text NOT NULL,
    artifact_sha256 text NOT NULL,
    schema_version bigint NOT NULL,
    schema_sha256 text NOT NULL,
    maximum_records bigint NOT NULL,
    maximum_bytes bigint NOT NULL,
    maximum_value_bytes bigint NOT NULL,
    instance_revision bigint NOT NULL,
    fence uuid NOT NULL,
    status text NOT NULL,
    source_generation_id uuid,
    next_record_version bigint DEFAULT 1 NOT NULL,
    record_count bigint DEFAULT 0 NOT NULL,
    total_bytes bigint DEFAULT 0 NOT NULL,
    migration_cursor text,
    migration_complete boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    promoted_at timestamp with time zone,
    CONSTRAINT plugin_state_generations_check CHECK ((((maximum_value_bytes >= 1) AND (maximum_value_bytes <= 262144)) AND (maximum_value_bytes <= maximum_bytes))),
    CONSTRAINT plugin_state_generations_check1 CHECK (((record_count >= 0) AND (record_count <= maximum_records))),
    CONSTRAINT plugin_state_generations_check2 CHECK (((total_bytes >= 0) AND (total_bytes <= maximum_bytes))),
    CONSTRAINT plugin_state_generations_check3 CHECK (((source_generation_id IS NULL) OR (source_generation_id <> id))),
    CONSTRAINT plugin_state_generations_check4 CHECK ((((status = 'staging'::text) AND (transition_id IS NOT NULL)) OR ((status = 'active'::text) AND (transition_id IS NULL) AND (source_generation_id IS NULL) AND (migration_cursor IS NULL) AND migration_complete AND (promoted_at IS NOT NULL)))),
    CONSTRAINT plugin_state_generations_instance_revision_check CHECK ((instance_revision > 0)),
    CONSTRAINT plugin_state_generations_maximum_bytes_check CHECK (((maximum_bytes >= 1) AND (maximum_bytes <= 16777216))),
    CONSTRAINT plugin_state_generations_maximum_records_check CHECK (((maximum_records >= 1) AND (maximum_records <= 10000))),
    CONSTRAINT plugin_state_generations_namespace_check CHECK ((namespace ~ '^[a-z][a-z0-9_.-]{0,63}$'::text)),
    CONSTRAINT plugin_state_generations_next_record_version_check CHECK ((next_record_version > 0)),
    CONSTRAINT plugin_state_generations_schema_sha256_check CHECK ((schema_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT plugin_state_generations_schema_version_check CHECK (((schema_version >= 1) AND (schema_version <= '4294967295'::bigint))),
    CONSTRAINT plugin_state_generations_status_check CHECK ((status = ANY (ARRAY['staging'::text, 'active'::text])))
);

CREATE TABLE plugin_state_records (
    generation_id uuid NOT NULL,
    state_key text NOT NULL COLLATE pg_catalog."C",
    value_json jsonb NOT NULL,
    value_bytes bigint NOT NULL,
    record_version bigint NOT NULL,
    CONSTRAINT plugin_state_records_record_version_check CHECK ((record_version > 0)),
    CONSTRAINT plugin_state_records_state_key_check CHECK ((((length(state_key) >= 1) AND (length(state_key) <= 256)) AND (octet_length(state_key) <= 512) AND (state_key !~ '[[:cntrl:]]'::text))),
    CONSTRAINT plugin_state_records_value_bytes_check CHECK (((value_bytes >= 1) AND (value_bytes <= 262144)))
);

CREATE TABLE plugin_update_sources (
    plugin_id text NOT NULL,
    source_json jsonb NOT NULL,
    policy_json jsonb DEFAULT '{"kind": "manual"}'::jsonb NOT NULL,
    outbound_proxy_id text,
    CONSTRAINT plugin_update_sources_plugin_id_check CHECK (((octet_length(plugin_id) <= 64) AND (plugin_id ~ '^[a-z0-9]([a-z0-9-]*[a-z0-9])?[.][a-z0-9]([a-z0-9-]*[a-z0-9])?$'::text))),
    CONSTRAINT plugin_update_sources_policy_json_check CHECK ((jsonb_typeof(policy_json) = 'object'::text)),
    CONSTRAINT plugin_update_sources_source_json_check CHECK ((jsonb_typeof(source_json) = 'object'::text))
);

CREATE TABLE plugin_version_configurations (
    instance_id uuid NOT NULL,
    artifact_sha256 text NOT NULL,
    configuration_json jsonb NOT NULL,
    secrets_json jsonb NOT NULL,
    bindings_json jsonb NOT NULL,
    CONSTRAINT plugin_version_configurations_bindings_json_check CHECK ((jsonb_typeof(bindings_json) = 'array'::text)),
    CONSTRAINT plugin_version_configurations_configuration_json_check CHECK ((jsonb_typeof(configuration_json) = 'object'::text)),
    CONSTRAINT plugin_version_configurations_secrets_json_check CHECK ((jsonb_typeof(secrets_json) = 'object'::text))
);

-- CY 证据按逻辑请求去重；候选规则必须经人工发布才进入运行时。
CREATE TABLE prompt_guard_candidates (
    id bigint NOT NULL,
    fingerprint text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    source text NOT NULL,
    protocol text NOT NULL,
    evidence_count bigint DEFAULT 0 NOT NULL,
    evidence text,
    evidence_quality text NOT NULL,
    rule_json jsonb,
    rationale text DEFAULT ''::text NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT prompt_guard_candidates_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'published'::text, 'dismissed'::text, 'disabled'::text])))
);

ALTER TABLE prompt_guard_candidates ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME prompt_guard_candidates_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE prompt_guard_incidents (
    request_id text NOT NULL,
    candidate_id bigint,
    client_key_id text NOT NULL,
    protocol text NOT NULL,
    model text NOT NULL,
    code text NOT NULL,
    observed_at timestamp with time zone DEFAULT now() NOT NULL
);

-- 锁定范围始终含 Client Key 和协议；没有会话时仅锁同一请求指纹。
CREATE TABLE prompt_guard_locks (
    id bigint NOT NULL,
    client_key_id text NOT NULL,
    protocol text NOT NULL,
    scope text NOT NULL,
    request_id text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

ALTER TABLE prompt_guard_locks ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME prompt_guard_locks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE prompt_guard_rule_revision (
    id smallint NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT prompt_guard_rule_revision_id_check CHECK ((id = 1)),
    CONSTRAINT prompt_guard_rule_revision_revision_check CHECK ((revision > 0))
);

-- 目录绑定凭据版本，轮换前的目录不能成为新凭据的模型权限证据。
CREATE TABLE provider_account_model_catalogs (
    account_id text NOT NULL,
    credential_revision bigint NOT NULL,
    plan_type text,
    etag text,
    observed_at timestamp with time zone NOT NULL,
    document bytea NOT NULL,
    document_hash bytea GENERATED ALWAYS AS (sha256(document)) STORED,
    CONSTRAINT provider_account_model_catalogs_credential_revision_check CHECK ((credential_revision > 0))
);

CREATE TABLE provider_accounts (
    id text NOT NULL,
    provider_kind text NOT NULL,
    name text NOT NULL,
    email text,
    upstream_user_id text,
    upstream_account_id text,
    plan_type text,
    authentication_kind text NOT NULL,
    provider_credentials_json jsonb NOT NULL,
    credential_revision bigint DEFAULT 1 NOT NULL,
    has_refresh_token boolean NOT NULL,
    access_token_expires_at timestamp with time zone,
    next_refresh_at timestamp with time zone,
    enabled boolean DEFAULT true NOT NULL,
    credential_state text DEFAULT 'unknown'::text NOT NULL,
    provider_quota_json jsonb,
    last_error_message text,
    credential_observed_at timestamp with time zone NOT NULL,
    quota_observed_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    quota_access_state text DEFAULT 'unknown'::text NOT NULL,
    quota_evidence text,
    quota_access_observed_at timestamp with time zone,
    quota_reset_at timestamp with time zone,
    last_error_reason text,
    concurrency_limit bigint,
    weight smallint DEFAULT 1 NOT NULL,
    outbound_proxy_url text,
    outbound_proxy_id text,
    notes text,
    model_access_json jsonb DEFAULT '{"mode": "all", "models": []}'::jsonb NOT NULL,
    reset_credit_automation_json jsonb,
    reset_credit_next_run_at timestamp with time zone,
    auto_use_quota_credits boolean DEFAULT false NOT NULL,
    quota_limits jsonb DEFAULT '{}'::jsonb NOT NULL,
    quota_paused boolean DEFAULT false NOT NULL,
    CONSTRAINT provider_accounts_authentication_kind_ck CHECK ((authentication_kind ~ '^[a-z][a-z0-9_]{0,63}$'::text)),
    CONSTRAINT provider_accounts_concurrency_limit_ck CHECK (((concurrency_limit IS NULL) OR ((concurrency_limit >= 1) AND (concurrency_limit <= '4294967295'::bigint)))),
    CONSTRAINT provider_accounts_credential_state_ck CHECK ((credential_state = ANY (ARRAY['unknown'::text, 'ready'::text, 'expired'::text, 'banned'::text, 'invalid'::text]))),
    CONSTRAINT provider_accounts_credentials_ck CHECK (((jsonb_typeof(provider_credentials_json) = 'object'::text) AND (octet_length((provider_credentials_json)::text) <= 262144))),
    CONSTRAINT provider_accounts_error_reason_ck CHECK (((last_error_reason IS NULL) OR (last_error_reason = ANY (ARRAY['account_unverified'::text, 'access_token_expired'::text, 'credential_expired'::text, 'credential_invalid'::text, 'account_banned'::text])))),
    CONSTRAINT provider_accounts_model_access_ck CHECK (((jsonb_typeof(model_access_json) = 'object'::text) AND (model_access_json ?& ARRAY['mode'::text, 'models'::text]) AND (((model_access_json - 'mode'::text) - 'models'::text) = '{}'::jsonb) AND (octet_length((model_access_json)::text) <= 131072) AND
CASE
    WHEN (jsonb_typeof((model_access_json -> 'models'::text)) = 'array'::text) THEN (
    CASE (model_access_json ->> 'mode'::text)
        WHEN 'all'::text THEN (jsonb_array_length((model_access_json -> 'models'::text)) = 0)
        WHEN 'allowlist'::text THEN ((jsonb_array_length((model_access_json -> 'models'::text)) >= 1) AND (jsonb_array_length((model_access_json -> 'models'::text)) <= 256))
        WHEN 'denylist'::text THEN ((jsonb_array_length((model_access_json -> 'models'::text)) >= 1) AND (jsonb_array_length((model_access_json -> 'models'::text)) <= 256))
        ELSE false
    END AND (NOT jsonb_path_exists(model_access_json, '$."models"[*]?(@.type() != "string")'::jsonpath)))
    ELSE false
END)),
    CONSTRAINT provider_accounts_notes_check CHECK (((notes IS NULL) OR (char_length(notes) <= 500))),
    CONSTRAINT provider_accounts_quota_access_state_ck CHECK ((quota_access_state = ANY (ARRAY['unknown'::text, 'allowed'::text, 'exhausted'::text]))),
    CONSTRAINT provider_accounts_quota_ck CHECK (((provider_quota_json IS NULL) OR ((jsonb_typeof(provider_quota_json) = 'object'::text) AND (octet_length((provider_quota_json)::text) <= 8388608)))),
    CONSTRAINT provider_accounts_quota_evidence_ck CHECK ((quota_evidence = ANY (ARRAY['provider_denied'::text, 'account_limit_reached'::text, 'usage_limit_reached'::text, 'payment_required'::text, 'credits_available'::text]))),
    CONSTRAINT provider_accounts_quota_fact_ck CHECK ((((quota_access_state = 'unknown'::text) AND (quota_evidence IS NULL) AND (quota_reset_at IS NULL)) OR ((quota_access_state = 'allowed'::text) AND (quota_access_observed_at IS NOT NULL) AND (((quota_evidence IS NULL) AND (quota_reset_at IS NULL)) OR (NOT (quota_evidence IS DISTINCT FROM 'credits_available'::text)))) OR ((quota_access_state = 'exhausted'::text) AND (quota_evidence IS NOT NULL) AND (quota_evidence <> 'credits_available'::text) AND (quota_access_observed_at IS NOT NULL)))),
    CONSTRAINT provider_accounts_quota_observation_ck CHECK (((provider_quota_json IS NULL) = (quota_observed_at IS NULL))),
    CONSTRAINT provider_accounts_refresh_ck CHECK ((has_refresh_token OR (next_refresh_at IS NULL))),
    CONSTRAINT provider_accounts_reset_credit_automation_shape CHECK ((((reset_credit_automation_json IS NULL) AND (reset_credit_next_run_at IS NULL)) OR COALESCE(((reset_credit_automation_json IS NOT NULL) AND (reset_credit_next_run_at IS NOT NULL) AND (jsonb_typeof(reset_credit_automation_json) = 'object'::text) AND (jsonb_typeof((reset_credit_automation_json -> 'enabled'::text)) = 'boolean'::text) AND (jsonb_typeof((reset_credit_automation_json -> 'revision'::text)) = 'number'::text) AND (jsonb_typeof((reset_credit_automation_json -> 'state'::text)) = 'object'::text)), false))),
    CONSTRAINT provider_accounts_revision_ck CHECK ((credential_revision > 0)),
    CONSTRAINT provider_accounts_time_ck CHECK (((created_at <= updated_at) AND (credential_observed_at <= updated_at) AND ((quota_access_observed_at IS NULL) OR (quota_access_observed_at <= updated_at)) AND ((quota_observed_at IS NULL) OR (quota_observed_at <= updated_at)))),
    CONSTRAINT provider_accounts_weight_ck CHECK (((weight >= 1) AND (weight <= 100)))
);

CREATE TABLE provider_authorization_receipts (
    provider_kind text NOT NULL,
    flow_digest text NOT NULL,
    owner_digest text NOT NULL,
    config_revision bigint NOT NULL,
    accounts_json jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone DEFAULT (now() + '24:00:00'::interval) NOT NULL,
    CONSTRAINT provider_authorization_receipts_accounts_json_check CHECK (((jsonb_typeof(accounts_json) = 'array'::text) AND ((jsonb_array_length(accounts_json) >= 1) AND (jsonb_array_length(accounts_json) <= 200)) AND (octet_length((accounts_json)::text) <= 65536))),
    CONSTRAINT provider_authorization_receipts_check CHECK ((expires_at > created_at)),
    CONSTRAINT provider_authorization_receipts_config_revision_check CHECK ((config_revision > 0)),
    CONSTRAINT provider_authorization_receipts_flow_digest_check CHECK ((flow_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT provider_authorization_receipts_owner_digest_check CHECK ((owner_digest ~ '^[0-9a-f]{64}$'::text))
);

CREATE TABLE request_analytics_15m (
    bucket_start timestamp with time zone NOT NULL,
    collection text NOT NULL,
    dimensions text[] NOT NULL,
    outcome text NOT NULL,
    schema_version integer NOT NULL,
    histogram_version integer NOT NULL,
    counts jsonb NOT NULL,
    request_count bigint GENERATED ALWAYS AS (((counts ->> 'request_count'::text))::bigint) STORED,
    costs jsonb NOT NULL,
    distributions jsonb NOT NULL,
    last_used_at timestamp with time zone
);

CREATE TABLE request_analytics_1d_counts (
    bucket_start timestamp with time zone NOT NULL,
    collection text NOT NULL,
    dimension text NOT NULL,
    outcome text NOT NULL,
    schema_version integer NOT NULL,
    histogram_version integer NOT NULL,
    request_count bigint NOT NULL,
    CONSTRAINT request_analytics_1d_counts_collection_check CHECK ((collection = ANY (ARRAY['account'::text, 'key'::text]))),
    CONSTRAINT request_analytics_1d_counts_request_count_check CHECK ((request_count >= 0))
);

CREATE TABLE request_analytics_1h (
    bucket_start timestamp with time zone NOT NULL,
    collection text NOT NULL,
    dimensions text[] NOT NULL,
    outcome text NOT NULL,
    schema_version integer NOT NULL,
    histogram_version integer NOT NULL,
    counts jsonb NOT NULL,
    request_count bigint GENERATED ALWAYS AS (((counts ->> 'request_count'::text))::bigint) STORED,
    costs jsonb NOT NULL,
    distributions jsonb NOT NULL,
    last_used_at timestamp with time zone
);

CREATE TABLE request_analytics_build_state (
    singleton boolean DEFAULT true NOT NULL,
    schema_version integer DEFAULT 1 NOT NULL,
    histogram_version integer DEFAULT 1 NOT NULL,
    coverage_start timestamp with time zone,
    coverage_end timestamp with time zone,
    backfill_cursor_started_at timestamp with time zone,
    backfill_cursor_id text,
    backfill_complete boolean DEFAULT false NOT NULL,
    reset_phase smallint DEFAULT 0 NOT NULL,
    reset_cursor jsonb,
    generation bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT request_analytics_build_state_check CHECK (((reset_phase <> 0) OR (reset_cursor IS NULL))),
    CONSTRAINT request_analytics_build_state_reset_phase_check CHECK (((reset_phase >= 0) AND (reset_phase <= 4))),
    CONSTRAINT request_analytics_build_state_singleton_check CHECK (singleton)
);

CREATE SEQUENCE request_analytics_change_version
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE TABLE request_analytics_facts (
    id text NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    client_api_key_ref text,
    provider_account_ref text,
    routing_group_refs text[],
    operation text,
    provider_kind text,
    requested_model_id text,
    upstream_model_id text,
    outcome text,
    client_status_code integer,
    upstream_status_code integer,
    client_transport text,
    upstream_transport text,
    attempt_count integer,
    client_response_id bytea,
    upstream_response_id bytea,
    upstream_request_id text,
    recovered_at timestamp with time zone,
    downstream_committed_at timestamp with time zone,
    request_kind text,
    image_generation_requested boolean,
    image_generation_succeeded boolean,
    error_kind text,
    input_tokens bigint,
    output_tokens bigint,
    cached_input_tokens bigint,
    cache_write_input_tokens bigint,
    reasoning_output_tokens bigint,
    image_input_tokens bigint,
    image_output_tokens bigint,
    total_tokens bigint,
    cost_source text,
    cost_currency text,
    cost_amount numeric(20,10),
    first_token_ms bigint,
    latency_ms bigint,
    admission_decision_ms bigint,
    account_selection_wait_ms bigint,
    upstream_response_ms bigint,
    capacity_used_slots bigint,
    capacity_total_slots bigint,
    standard_cost_amount numeric(20,10),
    no_cache_cost_amount numeric(20,10),
    source_version bigint NOT NULL,
    schema_version integer DEFAULT 1 NOT NULL,
    ops_failure_count bigint DEFAULT 0 NOT NULL,
    ops_rate_limited_count bigint DEFAULT 0 NOT NULL,
    ops_auth_failure_count bigint DEFAULT 0 NOT NULL,
    ops_provider_5xx_count bigint DEFAULT 0 NOT NULL
);

CREATE TABLE request_analytics_pending_buckets (
    bucket_start timestamp with time zone NOT NULL,
    change_version bigint DEFAULT nextval('request_analytics_change_version'::regclass) NOT NULL,
    oldest_pending_at timestamp with time zone NOT NULL,
    build_version bigint,
    build_phase smallint DEFAULT 0 NOT NULL,
    build_started_at timestamp with time zone,
    build_id text,
    build_collection text,
    build_dimensions text[],
    build_outcome text,
    CONSTRAINT request_analytics_pending_buckets_build_phase_check CHECK (((build_phase >= 0) AND (build_phase <= 4)))
);

CREATE TABLE request_analytics_pending_requests (
    request_id text NOT NULL,
    change_version bigint DEFAULT nextval('request_analytics_change_version'::regclass) NOT NULL,
    oldest_pending_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL
);

CREATE TABLE runtime_settings (
    id bigint NOT NULL,
    config_revision bigint DEFAULT 1 NOT NULL,
    admin_api_key text,
    refresh_margin_seconds bigint DEFAULT 300 NOT NULL,
    refresh_concurrency bigint DEFAULT 2 NOT NULL,
    max_concurrent_per_account bigint DEFAULT 3 NOT NULL,
    rotation_strategy text DEFAULT 'smart'::text NOT NULL,
    model_mappings_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    usage_retention_days bigint DEFAULT 31 NOT NULL,
    ops_event_retention_days bigint DEFAULT 30 NOT NULL,
    audit_retention_days bigint DEFAULT 90 NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    min_codex_desktop_version text,
    min_codex_cli_version text,
    max_waiting_per_key bigint DEFAULT 10 NOT NULL,
    max_waiting_per_account bigint DEFAULT 10 NOT NULL,
    key_queue_timeout_seconds bigint DEFAULT 30 CONSTRAINT runtime_settings_concurrency_wait_timeout_seconds_not_null NOT NULL,
    request_location_enabled boolean DEFAULT false NOT NULL,
    request_location_json jsonb DEFAULT '{"city": "Piketon", "region": "Ohio", "country": "US", "timezone": "America/New_York"}'::jsonb NOT NULL,
    responses_max_decompressed_body_bytes bigint DEFAULT 67108864 NOT NULL,
    provider_request_profiles_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    pricing_overrides_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    pricing_synced_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    pricing_synced_at timestamp with time zone,
    account_warmup_enabled boolean DEFAULT false NOT NULL,
    account_warmup_model text,
    smart_scheduling_json jsonb DEFAULT '{"loadWeight": 1.0, "queueWeight": 0.0, "quotaWeight": 0.8, "resetWeight": 0.0, "healthWeight": 1.0, "latencyWeight": 0.5}'::jsonb NOT NULL,
    account_warmup_cursor timestamp with time zone,
    openai_guardian_reserved_concurrency bigint DEFAULT 5 NOT NULL,
    openai_account_affinity text DEFAULT 'strict'::text NOT NULL,
    max_account_rotations bigint DEFAULT 3 NOT NULL,
    openai_session_affinity_ttl_hours bigint DEFAULT 24 NOT NULL,
    codex_privacy_policy_json jsonb DEFAULT '{"rules": [], "enabled": false, "onError": "skip_rule"}'::jsonb NOT NULL,
    reset_credit_lead_minutes integer DEFAULT 10 NOT NULL,
    reset_credit_max_weekly_remaining_percent integer,
    max_retry_after_seconds bigint DEFAULT 240 NOT NULL,
    account_queue_timeout_seconds bigint DEFAULT 30 NOT NULL,
    switch_account_on_queue_failure boolean DEFAULT false NOT NULL,
    account_warmup_cron text DEFAULT ''::text NOT NULL,
    prompt_guard_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    claude_public_url text DEFAULT ''::text NOT NULL,
    anthropic_account_affinity text DEFAULT 'strict'::text NOT NULL,
    anthropic_session_affinity_ttl_hours bigint DEFAULT 720 NOT NULL,
    anthropic_model_mappings_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    min_claude_code_version text,
    reset_credit_on_low_weekly_quota boolean DEFAULT false NOT NULL,
    peer_routing_id uuid DEFAULT gen_random_uuid() NOT NULL,
    quota_reset_window_weights_json jsonb DEFAULT '{"weekly": 0, "monthly": 0, "fiveHour": 0}'::jsonb NOT NULL,
    CONSTRAINT account_warmup_requires_model CHECK (((NOT account_warmup_enabled) OR (account_warmup_model IS NOT NULL))),
    CONSTRAINT runtime_settings_account_queue_timeout_seconds_check CHECK (((account_queue_timeout_seconds >= 1) AND (account_queue_timeout_seconds <= 120))),
    CONSTRAINT runtime_settings_account_warmup_model_check CHECK (((account_warmup_model IS NULL) OR (((octet_length(account_warmup_model) >= 1) AND (octet_length(account_warmup_model) <= 128)) AND (account_warmup_model = btrim(account_warmup_model)) AND (account_warmup_model !~ '[[:cntrl:]]'::text)))),
    CONSTRAINT runtime_settings_anthropic_account_affinity_check CHECK ((anthropic_account_affinity = ANY (ARRAY['relaxed'::text, 'preferred'::text, 'strict'::text]))),
    CONSTRAINT runtime_settings_anthropic_mappings_ck CHECK ((jsonb_typeof(anthropic_model_mappings_json) = 'object'::text)),
    CONSTRAINT runtime_settings_anthropic_session_affinity_ttl_hours_check CHECK (((anthropic_session_affinity_ttl_hours >= 1) AND (anthropic_session_affinity_ttl_hours <= 720))),
    CONSTRAINT runtime_settings_check CHECK (((octet_length(account_warmup_cron) <= 255) AND (account_warmup_cron = btrim(account_warmup_cron)) AND (account_warmup_cron !~ '[[:cntrl:]]'::text) AND ((NOT account_warmup_enabled) OR (account_warmup_cron <> ''::text)))),
    CONSTRAINT runtime_settings_claude_code_version_ck CHECK (((min_claude_code_version IS NULL) OR (((octet_length(min_claude_code_version) >= 1) AND (octet_length(min_claude_code_version) <= 64)) AND (min_claude_code_version = btrim(min_claude_code_version)) AND (min_claude_code_version !~ '[[:cntrl:]]'::text)))),
    CONSTRAINT runtime_settings_client_versions_ck CHECK ((((min_codex_desktop_version IS NULL) OR (((octet_length(min_codex_desktop_version) >= 1) AND (octet_length(min_codex_desktop_version) <= 64)) AND (min_codex_desktop_version = btrim(min_codex_desktop_version)) AND (min_codex_desktop_version !~ '[[:cntrl:]]'::text))) AND ((min_codex_cli_version IS NULL) OR (((octet_length(min_codex_cli_version) >= 1) AND (octet_length(min_codex_cli_version) <= 64)) AND (min_codex_cli_version = btrim(min_codex_cli_version)) AND (min_codex_cli_version !~ '[[:cntrl:]]'::text))))),
    CONSTRAINT runtime_settings_codex_privacy_policy_json_check CHECK (((jsonb_typeof(codex_privacy_policy_json) = 'object'::text) AND (jsonb_typeof((codex_privacy_policy_json -> 'enabled'::text)) = 'boolean'::text) AND ((codex_privacy_policy_json ->> 'onError'::text) = ANY (ARRAY['skip_rule'::text, 'reject_request'::text])) AND (jsonb_typeof((codex_privacy_policy_json -> 'rules'::text)) = 'array'::text) AND (codex_privacy_policy_json ?& ARRAY['enabled'::text, 'onError'::text, 'rules'::text]))),
    CONSTRAINT runtime_settings_concurrency_wait_timeout_seconds_check CHECK (((key_queue_timeout_seconds >= 1) AND (key_queue_timeout_seconds <= 120))),
    CONSTRAINT runtime_settings_max_account_rotations_check CHECK (((max_account_rotations >= 0) AND (max_account_rotations <= 31))),
    CONSTRAINT runtime_settings_max_retry_after_seconds_check CHECK (((max_retry_after_seconds >= 0) AND (max_retry_after_seconds <= '4294967295'::bigint))),
    CONSTRAINT runtime_settings_max_waiting_per_account_check CHECK (((max_waiting_per_account >= 0) AND (max_waiting_per_account <= 1000))),
    CONSTRAINT runtime_settings_max_waiting_per_key_check CHECK (((max_waiting_per_key >= 0) AND (max_waiting_per_key <= 1000))),
    CONSTRAINT runtime_settings_model_mappings_ck CHECK (((jsonb_typeof(model_mappings_json) = 'object'::text) AND (octet_length((model_mappings_json)::text) <= 131072))),
    CONSTRAINT runtime_settings_openai_account_affinity_check CHECK ((openai_account_affinity = ANY (ARRAY['relaxed'::text, 'preferred'::text, 'strict'::text]))),
    CONSTRAINT runtime_settings_openai_guardian_reserved_concurrency_check CHECK (((openai_guardian_reserved_concurrency >= 0) AND (openai_guardian_reserved_concurrency <= '4294967295'::bigint))),
    CONSTRAINT runtime_settings_openai_session_affinity_ttl_hours_check CHECK (((openai_session_affinity_ttl_hours >= 1) AND (openai_session_affinity_ttl_hours <= 720))),
    CONSTRAINT runtime_settings_pricing_overrides_json_check CHECK ((jsonb_typeof(pricing_overrides_json) = 'object'::text)),
    CONSTRAINT runtime_settings_pricing_synced_json_check CHECK ((jsonb_typeof(pricing_synced_json) = 'object'::text)),
    CONSTRAINT runtime_settings_refresh_ck CHECK (((refresh_margin_seconds > 0) AND (refresh_concurrency > 0) AND (max_concurrent_per_account >= 0))),
    CONSTRAINT runtime_settings_request_location_valid CHECK (((jsonb_typeof(request_location_json) = 'object'::text) AND (request_location_json ?& ARRAY['country'::text, 'region'::text, 'city'::text, 'timezone'::text]) AND ((request_location_json - ARRAY['country'::text, 'region'::text, 'city'::text, 'timezone'::text]) = '{}'::jsonb) AND (jsonb_typeof((request_location_json -> 'country'::text)) = 'string'::text) AND ((request_location_json ->> 'country'::text) ~ '^[A-Z]{2}$'::text) AND (jsonb_typeof((request_location_json -> 'region'::text)) = 'string'::text) AND ((char_length(btrim((request_location_json ->> 'region'::text))) >= 1) AND (char_length(btrim((request_location_json ->> 'region'::text))) <= 128)) AND (jsonb_typeof((request_location_json -> 'city'::text)) = 'string'::text) AND ((char_length(btrim((request_location_json ->> 'city'::text))) >= 1) AND (char_length(btrim((request_location_json ->> 'city'::text))) <= 128)) AND (jsonb_typeof((request_location_json -> 'timezone'::text)) = 'string'::text) AND (char_length((request_location_json ->> 'timezone'::text)) > 0))),
    CONSTRAINT runtime_settings_request_profiles_object CHECK ((jsonb_typeof(provider_request_profiles_json) = 'object'::text)),
    CONSTRAINT runtime_settings_reset_credit_lead_minutes_check CHECK (((reset_credit_lead_minutes >= 1) AND (reset_credit_lead_minutes <= 1440))),
    CONSTRAINT runtime_settings_reset_credit_max_weekly_remaining_percen_check CHECK (((reset_credit_max_weekly_remaining_percent >= 0) AND (reset_credit_max_weekly_remaining_percent <= 100))),
    CONSTRAINT runtime_settings_responses_max_decompressed_body_bytes_check CHECK ((responses_max_decompressed_body_bytes > 0)),
    CONSTRAINT runtime_settings_retention_ck CHECK (((usage_retention_days > 0) AND (ops_event_retention_days > 0) AND (audit_retention_days > 0))),
    CONSTRAINT runtime_settings_revision_ck CHECK ((config_revision > 0)),
    CONSTRAINT runtime_settings_rotation_ck CHECK ((rotation_strategy = ANY (ARRAY['smart'::text, 'quota_reset_priority'::text, 'round_robin'::text, 'sticky'::text]))),
    CONSTRAINT runtime_settings_singleton_ck CHECK ((id = 1)),
    CONSTRAINT runtime_settings_smart_scheduling_json_check CHECK ((jsonb_typeof(smart_scheduling_json) = 'object'::text))
);

ALTER TABLE ONLY account_group_accounts
    ADD CONSTRAINT account_group_accounts_pkey PRIMARY KEY (account_group_id, provider_account_id);

ALTER TABLE ONLY account_groups
    ADD CONSTRAINT account_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY admin_audit_events
    ADD CONSTRAINT admin_audit_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY admin_users
    ADD CONSTRAINT admin_users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY backup_records
    ADD CONSTRAINT backup_records_pkey PRIMARY KEY (id);

ALTER TABLE ONLY backup_settings
    ADD CONSTRAINT backup_settings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY client_api_key_budget_windows
    ADD CONSTRAINT client_api_key_budget_windows_pkey PRIMARY KEY (client_api_key_id);

ALTER TABLE ONLY client_api_key_charge_events
    ADD CONSTRAINT client_api_key_charge_events_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY client_api_key_groups
    ADD CONSTRAINT client_api_key_groups_pkey PRIMARY KEY (client_api_key_id, account_group_id);

ALTER TABLE ONLY client_api_keys
    ADD CONSTRAINT client_api_keys_pkey PRIMARY KEY (id);

ALTER TABLE ONLY model_request_leases
    ADD CONSTRAINT model_request_leases_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY model_requests
    ADD CONSTRAINT model_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY ops_events
    ADD CONSTRAINT ops_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY outbound_proxies
    ADD CONSTRAINT outbound_proxies_pkey PRIMARY KEY (id);

ALTER TABLE ONLY plugin_account_group_resources
    ADD CONSTRAINT plugin_account_group_resources_group_id_key UNIQUE (group_id);

ALTER TABLE ONLY plugin_account_group_resources
    ADD CONSTRAINT plugin_account_group_resources_pkey PRIMARY KEY (instance_id, resource_key);

ALTER TABLE ONLY plugin_artifact_credentials
    ADD CONSTRAINT plugin_artifact_credentials_pkey PRIMARY KEY (artifact_sha256, credential_id);

ALTER TABLE ONLY plugin_artifact_platforms
    ADD CONSTRAINT plugin_artifact_platforms_pkey PRIMARY KEY (plugin_id, version, platform);

ALTER TABLE ONLY plugin_artifacts
    ADD CONSTRAINT plugin_artifacts_pkey PRIMARY KEY (sha256);

ALTER TABLE ONLY plugin_client_api_key_resources
    ADD CONSTRAINT plugin_client_api_key_resources_key_id_key UNIQUE (key_id);

ALTER TABLE ONLY plugin_client_api_key_resources
    ADD CONSTRAINT plugin_client_api_key_resources_pkey PRIMARY KEY (instance_id, resource_key);

ALTER TABLE ONLY plugin_instance_secrets
    ADD CONSTRAINT plugin_instance_secrets_pkey PRIMARY KEY (instance_id);

ALTER TABLE ONLY plugin_instances
    ADD CONSTRAINT plugin_instances_pkey PRIMARY KEY (id);

ALTER TABLE ONLY plugin_source_credentials
    ADD CONSTRAINT plugin_source_credentials_pkey PRIMARY KEY (id);

ALTER TABLE ONLY plugin_state_generations
    ADD CONSTRAINT plugin_state_generations_instance_id_namespace_status_key UNIQUE (instance_id, namespace, status);

ALTER TABLE ONLY plugin_state_generations
    ADD CONSTRAINT plugin_state_generations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY plugin_state_generations
    ADD CONSTRAINT plugin_state_generations_transition_id_namespace_key UNIQUE (transition_id, namespace);

ALTER TABLE ONLY plugin_state_records
    ADD CONSTRAINT plugin_state_records_pkey PRIMARY KEY (generation_id, state_key);

ALTER TABLE ONLY plugin_update_sources
    ADD CONSTRAINT plugin_update_sources_pkey PRIMARY KEY (plugin_id);

ALTER TABLE ONLY plugin_version_configurations
    ADD CONSTRAINT plugin_version_configurations_pkey PRIMARY KEY (instance_id, artifact_sha256);

ALTER TABLE ONLY prompt_guard_candidates
    ADD CONSTRAINT prompt_guard_candidates_fingerprint_key UNIQUE (fingerprint);

ALTER TABLE ONLY prompt_guard_candidates
    ADD CONSTRAINT prompt_guard_candidates_pkey PRIMARY KEY (id);

ALTER TABLE ONLY prompt_guard_incidents
    ADD CONSTRAINT prompt_guard_incidents_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY prompt_guard_locks
    ADD CONSTRAINT prompt_guard_locks_client_key_id_protocol_scope_key UNIQUE (client_key_id, protocol, scope);

ALTER TABLE ONLY prompt_guard_locks
    ADD CONSTRAINT prompt_guard_locks_pkey PRIMARY KEY (id);

ALTER TABLE ONLY prompt_guard_rule_revision
    ADD CONSTRAINT prompt_guard_rule_revision_pkey PRIMARY KEY (id);

ALTER TABLE ONLY provider_account_model_catalogs
    ADD CONSTRAINT provider_account_model_catalogs_pkey PRIMARY KEY (account_id);

ALTER TABLE ONLY provider_accounts
    ADD CONSTRAINT provider_accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY provider_authorization_receipts
    ADD CONSTRAINT provider_authorization_receipts_pkey PRIMARY KEY (provider_kind, flow_digest);

ALTER TABLE ONLY request_analytics_15m
    ADD CONSTRAINT request_analytics_15m_pkey PRIMARY KEY (bucket_start, collection, dimensions, outcome);

ALTER TABLE ONLY request_analytics_1d_counts
    ADD CONSTRAINT request_analytics_1d_counts_pkey PRIMARY KEY (collection, bucket_start, dimension, outcome, schema_version, histogram_version);

ALTER TABLE ONLY request_analytics_1h
    ADD CONSTRAINT request_analytics_1h_pkey PRIMARY KEY (bucket_start, collection, dimensions, outcome);

ALTER TABLE ONLY request_analytics_build_state
    ADD CONSTRAINT request_analytics_build_state_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY request_analytics_facts
    ADD CONSTRAINT request_analytics_facts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY request_analytics_pending_buckets
    ADD CONSTRAINT request_analytics_pending_buckets_pkey PRIMARY KEY (bucket_start);

ALTER TABLE ONLY request_analytics_pending_requests
    ADD CONSTRAINT request_analytics_pending_requests_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY runtime_settings
    ADD CONSTRAINT runtime_settings_pkey PRIMARY KEY (id);

CREATE INDEX account_group_accounts_account_idx ON account_group_accounts USING btree (provider_account_id, account_group_id);

CREATE UNIQUE INDEX account_groups_name_uq ON account_groups USING btree (lower(name));

CREATE INDEX admin_audit_events_actor_idx ON admin_audit_events USING btree (actor_admin_user_id) WHERE (actor_admin_user_id IS NOT NULL);

CREATE INDEX admin_audit_events_created_idx ON admin_audit_events USING btree (created_at DESC, id DESC);

CREATE UNIQUE INDEX backup_records_active_uq ON backup_records USING btree ((1)) WHERE (status = ANY (ARRAY['queued'::text, 'dumping'::text, 'uploading'::text]));

CREATE INDEX backup_records_created_idx ON backup_records USING btree (created_at DESC, id DESC);

CREATE UNIQUE INDEX backup_records_object_key_uq ON backup_records USING btree (object_key) WHERE (object_key IS NOT NULL);

CREATE UNIQUE INDEX backup_records_privacy_object_key_uq ON backup_records USING btree (privacy_object_key) WHERE (privacy_object_key IS NOT NULL);

CREATE INDEX backup_records_retention_idx ON backup_records USING btree (expires_at) WHERE ((expires_at IS NOT NULL) AND (status = ANY (ARRAY['completed'::text, 'failed'::text])));

CREATE INDEX backup_records_scheduled_completed_idx ON backup_records USING btree (completed_at DESC, id DESC) WHERE ((trigger_kind = 'scheduled'::text) AND (status = 'completed'::text));

CREATE UNIQUE INDEX backup_records_scheduled_uq ON backup_records USING btree (scheduled_at) WHERE ((trigger_kind = 'scheduled'::text) AND (scheduled_at IS NOT NULL));

CREATE INDEX backup_records_status_idx ON backup_records USING btree (status, created_at, id);

CREATE INDEX client_api_key_charge_events_client_api_key_id ON client_api_key_charge_events USING btree (client_api_key_id);

CREATE INDEX client_api_key_charge_events_completed ON client_api_key_charge_events USING btree (client_api_key_id, completed_at) INCLUDE (amount_usd);

CREATE INDEX client_api_key_groups_group_idx ON client_api_key_groups USING btree (account_group_id, client_api_key_id);

CREATE INDEX client_api_keys_created_idx ON client_api_keys USING btree (created_at DESC, id DESC);

CREATE UNIQUE INDEX client_api_keys_key_sha256_idx ON client_api_keys USING btree (client_api_key_sha256(key));

CREATE INDEX model_request_leases_expiry_idx ON model_request_leases USING btree (lease_expires_at, request_id);

CREATE INDEX model_requests_account_ref_idx ON model_requests USING btree (provider_account_ref, started_at DESC, id DESC) WHERE (provider_account_ref IS NOT NULL);

CREATE INDEX model_requests_client_ref_idx ON model_requests USING btree (client_api_key_ref, started_at DESC, id DESC);

CREATE INDEX model_requests_delivered_account ON model_requests USING btree (provider_account_ref, started_at, id) WHERE ((outcome = 'succeeded'::text) AND (downstream_committed_at IS NOT NULL) AND ((provider_kind IS DISTINCT FROM 'openai'::text) OR (request_kind IS DISTINCT FROM 'prewarm'::text)) AND (((client_transport = 'websocket'::text) AND (client_status_code IS NULL)) OR ((client_status_code >= 200) AND (client_status_code <= 399))) AND ((requested_model_id IS NOT NULL) OR (upstream_model_id IS NOT NULL) OR image_generation_requested OR (input_tokens IS NOT NULL) OR (output_tokens IS NOT NULL) OR (cached_input_tokens IS NOT NULL) OR (cache_write_input_tokens IS NOT NULL) OR (reasoning_output_tokens IS NOT NULL) OR (image_input_tokens IS NOT NULL) OR (image_output_tokens IS NOT NULL) OR (total_tokens IS NOT NULL) OR (cost_amount IS NOT NULL)));

CREATE INDEX model_requests_delivered_account_model ON model_requests USING btree (provider_account_ref, COALESCE(upstream_model_id, requested_model_id, ''::text), started_at DESC, id DESC) WHERE ((outcome = 'succeeded'::text) AND (downstream_committed_at IS NOT NULL) AND ((provider_kind IS DISTINCT FROM 'openai'::text) OR (request_kind IS DISTINCT FROM 'prewarm'::text)) AND (((client_transport = 'websocket'::text) AND (client_status_code IS NULL)) OR ((client_status_code >= 200) AND (client_status_code <= 399))) AND ((requested_model_id IS NOT NULL) OR (upstream_model_id IS NOT NULL) OR image_generation_requested OR (input_tokens IS NOT NULL) OR (output_tokens IS NOT NULL) OR (cached_input_tokens IS NOT NULL) OR (cache_write_input_tokens IS NOT NULL) OR (reasoning_output_tokens IS NOT NULL) OR (image_input_tokens IS NOT NULL) OR (image_output_tokens IS NOT NULL) OR (total_tokens IS NOT NULL) OR (cost_amount IS NOT NULL)));

CREATE INDEX model_requests_delivered_client ON model_requests USING btree (client_api_key_ref, started_at, id) WHERE ((outcome = 'succeeded'::text) AND (downstream_committed_at IS NOT NULL) AND ((provider_kind IS DISTINCT FROM 'openai'::text) OR (request_kind IS DISTINCT FROM 'prewarm'::text)) AND (((client_transport = 'websocket'::text) AND (client_status_code IS NULL)) OR ((client_status_code >= 200) AND (client_status_code <= 399))) AND ((requested_model_id IS NOT NULL) OR (upstream_model_id IS NOT NULL) OR image_generation_requested OR (input_tokens IS NOT NULL) OR (output_tokens IS NOT NULL) OR (cached_input_tokens IS NOT NULL) OR (cache_write_input_tokens IS NOT NULL) OR (reasoning_output_tokens IS NOT NULL) OR (image_input_tokens IS NOT NULL) OR (image_output_tokens IS NOT NULL) OR (total_tokens IS NOT NULL) OR (cost_amount IS NOT NULL)));

CREATE INDEX model_requests_delivered_started ON model_requests USING btree (started_at, id) WHERE ((outcome = 'succeeded'::text) AND (downstream_committed_at IS NOT NULL) AND ((provider_kind IS DISTINCT FROM 'openai'::text) OR (request_kind IS DISTINCT FROM 'prewarm'::text)) AND (((client_transport = 'websocket'::text) AND (client_status_code IS NULL)) OR ((client_status_code >= 200) AND (client_status_code <= 399))) AND ((requested_model_id IS NOT NULL) OR (upstream_model_id IS NOT NULL) OR image_generation_requested OR (input_tokens IS NOT NULL) OR (output_tokens IS NOT NULL) OR (cached_input_tokens IS NOT NULL) OR (cache_write_input_tokens IS NOT NULL) OR (reasoning_output_tokens IS NOT NULL) OR (image_input_tokens IS NOT NULL) OR (image_output_tokens IS NOT NULL) OR (total_tokens IS NOT NULL) OR (cost_amount IS NOT NULL)));

CREATE INDEX model_requests_forecast_documents ON model_requests USING btree (provider_account_ref, completed_at DESC, id DESC) INCLUDE (started_at) WHERE ((provider_observation_json IS NOT NULL) AND (outcome <> 'running'::text));

CREATE INDEX model_requests_ops_completed ON model_requests USING btree (completed_at, id) WHERE ((error_kind IS NOT NULL) AND (error_kind <> 'cancelled'::text));

CREATE INDEX model_requests_outcome_idx ON model_requests USING btree (outcome, started_at DESC, id DESC);

CREATE INDEX model_requests_pending_continuation_recovery_idx ON model_requests USING btree (client_api_key_ref, continuation_affinity_hash, completed_at DESC, id DESC) WHERE ((error_kind = 'continuation_recovery_required'::text) AND (recovered_at IS NULL) AND (continuation_affinity_hash IS NOT NULL));

CREATE INDEX model_requests_pending_ws_transport_recovery_idx ON model_requests USING btree (client_api_key_ref, continuation_affinity_hash, completed_at DESC, id DESC) WHERE ((provider_kind = 'openai'::text) AND (outcome = 'failed'::text) AND (error_kind = 'upstream_unavailable'::text) AND (upstream_transport = 'websocket'::text) AND (upstream_send_state = 'ambiguous'::text) AND (downstream_committed_at IS NULL) AND (recovered_at IS NULL) AND (continuation_affinity_hash IS NOT NULL));

CREATE INDEX model_requests_provider_idx ON model_requests USING btree (provider_kind, started_at DESC, id DESC) WHERE (provider_kind IS NOT NULL);

CREATE INDEX model_requests_recovery_request_idx ON model_requests USING btree (recovery_request_id) WHERE (recovery_request_id IS NOT NULL);

CREATE INDEX model_requests_requested_model_idx ON model_requests USING btree (requested_model_id, started_at DESC, id DESC);

CREATE INDEX model_requests_retention_idx ON model_requests USING btree (completed_at) WHERE (outcome <> 'running'::text);

CREATE INDEX model_requests_started_idx ON model_requests USING btree (started_at DESC, id DESC);

CREATE INDEX model_requests_upstream_model_idx ON model_requests USING btree (upstream_model_id, started_at DESC, id DESC) WHERE (upstream_model_id IS NOT NULL);

CREATE INDEX ops_events_account_ref_idx ON ops_events USING btree (provider_account_ref, created_at DESC, id DESC) WHERE (provider_account_ref IS NOT NULL);

CREATE INDEX ops_events_created_idx ON ops_events USING btree (created_at DESC, id DESC);

CREATE INDEX ops_events_request_idx ON ops_events USING btree (model_request_id, attempt_index, id);

CREATE INDEX ops_events_retention_idx ON ops_events USING btree (created_at) WHERE (model_request_id IS NULL);

CREATE INDEX plugin_artifact_credentials_credential_id_idx ON plugin_artifact_credentials USING btree (credential_id);

CREATE INDEX plugin_artifact_platforms_artifact_sha256_idx ON plugin_artifact_platforms USING btree (artifact_sha256);

CREATE INDEX plugin_artifacts_outbound_proxy_id_idx ON plugin_artifacts USING btree (outbound_proxy_id);

CREATE INDEX plugin_artifacts_plugin_id_idx ON plugin_artifacts USING btree (plugin_id);

CREATE INDEX plugin_instances_artifact_sha256_idx ON plugin_instances USING btree (artifact_sha256);

CREATE INDEX plugin_state_generations_artifact_sha256_idx ON plugin_state_generations USING btree (artifact_sha256);

CREATE INDEX plugin_state_generations_source_generation_id_idx ON plugin_state_generations USING btree (source_generation_id);

CREATE INDEX plugin_update_sources_outbound_proxy_id_idx ON plugin_update_sources USING btree (outbound_proxy_id);

CREATE INDEX prompt_guard_candidates_seen ON prompt_guard_candidates USING btree (last_seen_at);

CREATE INDEX prompt_guard_candidates_status ON prompt_guard_candidates USING btree (status, last_seen_at DESC);

CREATE INDEX prompt_guard_incidents_key_time ON prompt_guard_incidents USING btree (client_key_id, observed_at DESC);

CREATE INDEX prompt_guard_incidents_time ON prompt_guard_incidents USING btree (observed_at);

CREATE INDEX prompt_guard_locks_expiry ON prompt_guard_locks USING btree (expires_at);

CREATE UNIQUE INDEX prompt_guard_published_rule_id ON prompt_guard_candidates USING btree (((rule_json ->> 'id'::text))) WHERE (status = 'published'::text);

CREATE INDEX provider_accounts_access_expiry_idx ON provider_accounts USING btree (access_token_expires_at, id);

CREATE INDEX provider_accounts_credential_state_idx ON provider_accounts USING btree (credential_state, id);

CREATE INDEX provider_accounts_email_idx ON provider_accounts USING btree (provider_kind, lower(email)) WHERE (email IS NOT NULL);

CREATE INDEX provider_accounts_outbound_proxy_id_idx ON provider_accounts USING btree (outbound_proxy_id);

CREATE INDEX provider_accounts_reset_credit_due_idx ON provider_accounts USING btree (reset_credit_next_run_at, id) WHERE (enabled AND ((reset_credit_automation_json ->> 'enabled'::text) = 'true'::text));

CREATE INDEX provider_accounts_runtime_idx ON provider_accounts USING btree (provider_kind, enabled, id);

CREATE UNIQUE INDEX provider_accounts_upstream_identity_uq ON provider_accounts USING btree (provider_kind, upstream_user_id, COALESCE(upstream_account_id, ''::text));

CREATE INDEX provider_authorization_receipts_expiry_idx ON provider_authorization_receipts USING btree (expires_at);

CREATE INDEX request_analytics_15m_build_cursor ON request_analytics_15m USING btree (bucket_start, collection COLLATE "C", dimensions COLLATE "C", outcome COLLATE "C");

CREATE INDEX request_analytics_15m_collection ON request_analytics_15m USING btree (collection, bucket_start);

CREATE INDEX request_analytics_15m_dimension ON request_analytics_15m USING btree (collection, (dimensions[1]), bucket_start);

CREATE INDEX request_analytics_15m_rank ON request_analytics_15m USING btree (collection, bucket_start) INCLUDE (dimensions, outcome, schema_version, histogram_version, request_count) WHERE (collection = ANY (ARRAY['account'::text, 'key'::text]));

CREATE INDEX request_analytics_1h_collection ON request_analytics_1h USING btree (collection, bucket_start);

CREATE INDEX request_analytics_1h_dimension ON request_analytics_1h USING btree (collection, (dimensions[1]), bucket_start);

CREATE INDEX request_analytics_1h_rank ON request_analytics_1h USING btree (collection, bucket_start) INCLUDE (dimensions, outcome, schema_version, histogram_version, request_count) WHERE (collection = ANY (ARRAY['account'::text, 'key'::text]));

CREATE INDEX request_analytics_facts_account ON request_analytics_facts USING btree (provider_account_ref, started_at, id);

CREATE INDEX request_analytics_facts_account_completion ON request_analytics_facts USING btree (provider_account_ref, completed_at, started_at) WHERE ((completed_at IS NOT NULL) AND (outcome <> 'running'::text));

CREATE INDEX request_analytics_facts_build_cursor ON request_analytics_facts USING btree (started_at, id COLLATE "C");

CREATE INDEX request_analytics_facts_client_response ON request_analytics_facts USING hash (client_response_id) WHERE (client_response_id IS NOT NULL);

CREATE INDEX request_analytics_facts_delivered_account ON request_analytics_facts USING btree (provider_account_ref, started_at DESC, id DESC) WHERE ((outcome = 'succeeded'::text) AND (downstream_committed_at IS NOT NULL) AND ((provider_kind IS DISTINCT FROM 'openai'::text) OR (request_kind IS DISTINCT FROM 'prewarm'::text)) AND (((client_transport = 'websocket'::text) AND (client_status_code IS NULL)) OR ((client_status_code >= 200) AND (client_status_code <= 399))) AND ((requested_model_id IS NOT NULL) OR (upstream_model_id IS NOT NULL) OR image_generation_requested OR (input_tokens IS NOT NULL) OR (output_tokens IS NOT NULL) OR (cached_input_tokens IS NOT NULL) OR (cache_write_input_tokens IS NOT NULL) OR (reasoning_output_tokens IS NOT NULL) OR (image_input_tokens IS NOT NULL) OR (image_output_tokens IS NOT NULL) OR (total_tokens IS NOT NULL) OR (cost_amount IS NOT NULL)));

CREATE INDEX request_analytics_facts_groups ON request_analytics_facts USING gin (routing_group_refs);

CREATE INDEX request_analytics_facts_key ON request_analytics_facts USING btree (client_api_key_ref, started_at, id);

CREATE INDEX request_analytics_facts_started ON request_analytics_facts USING btree (started_at, id);

CREATE INDEX request_analytics_facts_upstream_request ON request_analytics_facts USING hash (upstream_request_id) WHERE (upstream_request_id IS NOT NULL);

CREATE INDEX request_analytics_facts_upstream_response ON request_analytics_facts USING hash (upstream_response_id) WHERE (upstream_response_id IS NOT NULL);

CREATE INDEX request_analytics_pending_buckets_age ON request_analytics_pending_buckets USING btree (oldest_pending_at, bucket_start);

CREATE INDEX request_analytics_pending_requests_age ON request_analytics_pending_requests USING btree (oldest_pending_at, request_id);

CREATE TRIGGER model_requests_finish_lease BEFORE UPDATE ON model_requests FOR EACH ROW WHEN (((old.outcome = 'running'::text) AND (new.outcome <> 'running'::text))) EXECUTE FUNCTION finish_model_request_lease();

CREATE TRIGGER model_requests_observation_insert AFTER INSERT ON model_requests FOR EACH ROW EXECUTE FUNCTION check_model_request_observation();

CREATE TRIGGER model_requests_observation_update AFTER UPDATE ON model_requests FOR EACH ROW WHEN (((old.request_observation_json IS DISTINCT FROM new.request_observation_json) OR (old.completed_at IS DISTINCT FROM new.completed_at) OR (old.continuation_requested IS DISTINCT FROM new.continuation_requested) OR (old.recovered_at IS DISTINCT FROM new.recovered_at) OR (old.recovery_attempt_count IS DISTINCT FROM new.recovery_attempt_count) OR (old.recovery_request_id IS DISTINCT FROM new.recovery_request_id))) EXECUTE FUNCTION check_model_request_observation();

CREATE TRIGGER provider_accounts_quota_credit_policy BEFORE INSERT OR UPDATE OF auto_use_quota_credits, quota_evidence, quota_access_state ON provider_accounts FOR EACH ROW WHEN (((NOT new.auto_use_quota_credits) AND (new.quota_evidence = 'credits_available'::text))) EXECUTE FUNCTION apply_quota_credit_policy();

CREATE TRIGGER request_analytics_1h_1d_counts AFTER INSERT OR DELETE OR UPDATE ON request_analytics_1h FOR EACH ROW EXECUTE FUNCTION sync_request_analytics_1d_counts();

CREATE TRIGGER request_analytics_delete AFTER DELETE ON model_requests FOR EACH ROW EXECUTE FUNCTION observe_request_analytics_change();

CREATE TRIGGER request_analytics_insert AFTER INSERT ON model_requests FOR EACH ROW EXECUTE FUNCTION observe_request_analytics_change();

CREATE TRIGGER request_analytics_ops_delete AFTER DELETE ON ops_events FOR EACH ROW EXECUTE FUNCTION observe_ops_analytics_change();

CREATE TRIGGER request_analytics_ops_insert AFTER INSERT ON ops_events FOR EACH ROW EXECUTE FUNCTION observe_ops_analytics_change();

CREATE TRIGGER request_analytics_ops_update AFTER UPDATE ON ops_events FOR EACH ROW WHEN (((((old.model_request_id IS DISTINCT FROM new.model_request_id) OR (old.failure_kind IS DISTINCT FROM new.failure_kind)) OR (old.status_code IS DISTINCT FROM new.status_code)) OR (old.attempt_index IS DISTINCT FROM new.attempt_index))) EXECUTE FUNCTION observe_ops_analytics_change();

CREATE TRIGGER request_analytics_update AFTER UPDATE ON model_requests FOR EACH ROW WHEN (((((((((((((((((((((((((((((((((((((old.id IS DISTINCT FROM new.id) OR (old.started_at IS DISTINCT FROM new.started_at)) OR (old.completed_at IS DISTINCT FROM new.completed_at)) OR (old.client_api_key_ref IS DISTINCT FROM new.client_api_key_ref)) OR (old.provider_account_ref IS DISTINCT FROM new.provider_account_ref)) OR (old.operation IS DISTINCT FROM new.operation)) OR (old.provider_kind IS DISTINCT FROM new.provider_kind)) OR (old.requested_model_id IS DISTINCT FROM new.requested_model_id)) OR (old.upstream_model_id IS DISTINCT FROM new.upstream_model_id)) OR (old.outcome IS DISTINCT FROM new.outcome)) OR (old.client_status_code IS DISTINCT FROM new.client_status_code)) OR (old.upstream_status_code IS DISTINCT FROM new.upstream_status_code)) OR (old.client_transport IS DISTINCT FROM new.client_transport)) OR (old.upstream_transport IS DISTINCT FROM new.upstream_transport)) OR (old.attempt_count IS DISTINCT FROM new.attempt_count)) OR (old.client_response_id IS DISTINCT FROM new.client_response_id)) OR (old.upstream_response_id IS DISTINCT FROM new.upstream_response_id)) OR (old.upstream_request_id IS DISTINCT FROM new.upstream_request_id)) OR (old.recovered_at IS DISTINCT FROM new.recovered_at)) OR (old.downstream_committed_at IS DISTINCT FROM new.downstream_committed_at)) OR (old.request_kind IS DISTINCT FROM new.request_kind)) OR (old.image_generation_requested IS DISTINCT FROM new.image_generation_requested)) OR (old.image_generation_succeeded IS DISTINCT FROM new.image_generation_succeeded)) OR (old.error_kind IS DISTINCT FROM new.error_kind)) OR (old.input_tokens IS DISTINCT FROM new.input_tokens)) OR (old.output_tokens IS DISTINCT FROM new.output_tokens)) OR (old.cached_input_tokens IS DISTINCT FROM new.cached_input_tokens)) OR (old.cache_write_input_tokens IS DISTINCT FROM new.cache_write_input_tokens)) OR (old.reasoning_output_tokens IS DISTINCT FROM new.reasoning_output_tokens)) OR (old.image_input_tokens IS DISTINCT FROM new.image_input_tokens)) OR (old.image_output_tokens IS DISTINCT FROM new.image_output_tokens)) OR (old.total_tokens IS DISTINCT FROM new.total_tokens)) OR (old.cost_source IS DISTINCT FROM new.cost_source)) OR (old.cost_currency IS DISTINCT FROM new.cost_currency)) OR (old.cost_amount IS DISTINCT FROM new.cost_amount)) OR (old.billing_snapshot_json IS DISTINCT FROM new.billing_snapshot_json) OR (old.service_tier IS DISTINCT FROM new.service_tier) OR ((old.request_observation_json -> 'timings'::text) IS DISTINCT FROM (new.request_observation_json -> 'timings'::text)) OR ((old.request_observation_json -> 'scheduling'::text) IS DISTINCT FROM (new.request_observation_json -> 'scheduling'::text)) OR ((old.request_observation_json -> 'routing'::text) IS DISTINCT FROM (new.request_observation_json -> 'routing'::text)))) EXECUTE FUNCTION observe_request_analytics_change();

ALTER TABLE ONLY account_group_accounts
    ADD CONSTRAINT account_group_accounts_account_fk FOREIGN KEY (provider_account_id) REFERENCES provider_accounts(id) ON UPDATE RESTRICT ON DELETE CASCADE;

ALTER TABLE ONLY account_group_accounts
    ADD CONSTRAINT account_group_accounts_group_fk FOREIGN KEY (account_group_id) REFERENCES account_groups(id) ON UPDATE RESTRICT ON DELETE CASCADE;

ALTER TABLE ONLY admin_audit_events
    ADD CONSTRAINT admin_audit_events_actor_fk FOREIGN KEY (actor_admin_user_id) REFERENCES admin_users(id) ON UPDATE RESTRICT ON DELETE SET NULL;

ALTER TABLE ONLY client_api_key_groups
    ADD CONSTRAINT client_api_key_groups_group_fk FOREIGN KEY (account_group_id) REFERENCES account_groups(id) ON UPDATE RESTRICT ON DELETE RESTRICT;

ALTER TABLE ONLY client_api_key_groups
    ADD CONSTRAINT client_api_key_groups_key_fk FOREIGN KEY (client_api_key_id) REFERENCES client_api_keys(id) ON UPDATE RESTRICT ON DELETE CASCADE;

ALTER TABLE ONLY model_request_leases
    ADD CONSTRAINT model_request_leases_request_id_fkey FOREIGN KEY (request_id) REFERENCES model_requests(id) ON DELETE CASCADE;

ALTER TABLE ONLY ops_events
    ADD CONSTRAINT ops_events_request_fk FOREIGN KEY (model_request_id) REFERENCES model_requests(id) ON UPDATE RESTRICT ON DELETE CASCADE;

ALTER TABLE ONLY plugin_account_group_resources
    ADD CONSTRAINT plugin_account_group_resources_group_id_fkey FOREIGN KEY (group_id) REFERENCES account_groups(id) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_account_group_resources
    ADD CONSTRAINT plugin_account_group_resources_instance_id_fkey FOREIGN KEY (instance_id) REFERENCES plugin_instances(id) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_artifact_credentials
    ADD CONSTRAINT plugin_artifact_credentials_artifact_sha256_fkey FOREIGN KEY (artifact_sha256) REFERENCES plugin_artifacts(sha256) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_artifact_credentials
    ADD CONSTRAINT plugin_artifact_credentials_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES plugin_source_credentials(id);

ALTER TABLE ONLY plugin_artifact_platforms
    ADD CONSTRAINT plugin_artifact_platforms_artifact_sha256_fkey FOREIGN KEY (artifact_sha256) REFERENCES plugin_artifacts(sha256) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_artifacts
    ADD CONSTRAINT plugin_artifacts_outbound_proxy_id_fkey FOREIGN KEY (outbound_proxy_id) REFERENCES outbound_proxies(id) ON DELETE RESTRICT;

ALTER TABLE ONLY plugin_artifacts
    ADD CONSTRAINT plugin_artifacts_plugin_id_fkey FOREIGN KEY (plugin_id) REFERENCES plugin_update_sources(plugin_id);

ALTER TABLE ONLY plugin_client_api_key_resources
    ADD CONSTRAINT plugin_client_api_key_resources_instance_id_fkey FOREIGN KEY (instance_id) REFERENCES plugin_instances(id) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_client_api_key_resources
    ADD CONSTRAINT plugin_client_api_key_resources_key_id_fkey FOREIGN KEY (key_id) REFERENCES client_api_keys(id) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_instance_secrets
    ADD CONSTRAINT plugin_instance_secrets_instance_id_fkey FOREIGN KEY (instance_id) REFERENCES plugin_instances(id) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_instances
    ADD CONSTRAINT plugin_instances_artifact_sha256_fkey FOREIGN KEY (artifact_sha256) REFERENCES plugin_artifacts(sha256);

ALTER TABLE ONLY plugin_state_generations
    ADD CONSTRAINT plugin_state_generations_artifact_sha256_fkey FOREIGN KEY (artifact_sha256) REFERENCES plugin_artifacts(sha256);

ALTER TABLE ONLY plugin_state_generations
    ADD CONSTRAINT plugin_state_generations_instance_id_fkey FOREIGN KEY (instance_id) REFERENCES plugin_instances(id) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_state_generations
    ADD CONSTRAINT plugin_state_generations_source_generation_id_fkey FOREIGN KEY (source_generation_id) REFERENCES plugin_state_generations(id);

ALTER TABLE ONLY plugin_state_records
    ADD CONSTRAINT plugin_state_records_generation_id_fkey FOREIGN KEY (generation_id) REFERENCES plugin_state_generations(id) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_update_sources
    ADD CONSTRAINT plugin_update_sources_outbound_proxy_id_fkey FOREIGN KEY (outbound_proxy_id) REFERENCES outbound_proxies(id) ON DELETE RESTRICT;

ALTER TABLE ONLY plugin_version_configurations
    ADD CONSTRAINT plugin_version_configurations_artifact_sha256_fkey FOREIGN KEY (artifact_sha256) REFERENCES plugin_artifacts(sha256) ON DELETE CASCADE;

ALTER TABLE ONLY plugin_version_configurations
    ADD CONSTRAINT plugin_version_configurations_instance_id_fkey FOREIGN KEY (instance_id) REFERENCES plugin_instances(id) ON DELETE CASCADE;

ALTER TABLE ONLY prompt_guard_incidents
    ADD CONSTRAINT prompt_guard_incidents_candidate_id_fkey FOREIGN KEY (candidate_id) REFERENCES prompt_guard_candidates(id);

ALTER TABLE ONLY provider_account_model_catalogs
    ADD CONSTRAINT provider_account_model_catalogs_account_id_fkey FOREIGN KEY (account_id) REFERENCES provider_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY provider_accounts
    ADD CONSTRAINT provider_accounts_outbound_proxy_id_fkey FOREIGN KEY (outbound_proxy_id) REFERENCES outbound_proxies(id) ON DELETE RESTRICT;

-- 保留历史迁移回填后的有效初值，不能简单改为逐列 DEFAULT。
-- 安装时间和节点 UUID 每次建库重新生成，避免复用基线导出时的身份。
INSERT INTO backup_settings (
    id,
    storage_revision,
    endpoint,
    region,
    bucket,
    access_key_id,
    secret_access_key,
    prefix,
    force_path_style,
    schedule_enabled,
    cron_expression,
    schedule_timezone,
    retention_days,
    retention_count,
    next_run_at,
    last_verified_at,
    updated_at
) VALUES (
    1,
    1,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    false,
    false,
    NULL,
    'Asia/Shanghai',
    0,
    0,
    NULL,
    NULL,
    now()
);

INSERT INTO prompt_guard_rule_revision (
    id,
    revision
) VALUES (
    1,
    1
);

INSERT INTO request_analytics_build_state (
    singleton,
    schema_version,
    histogram_version,
    coverage_start,
    coverage_end,
    backfill_cursor_started_at,
    backfill_cursor_id,
    backfill_complete,
    reset_phase,
    reset_cursor,
    generation,
    updated_at
) VALUES (
    true,
    1,
    1,
    NULL,
    NULL,
    NULL,
    NULL,
    false,
    0,
    NULL,
    0,
    clock_timestamp()
);

INSERT INTO runtime_settings (
    id,
    config_revision,
    admin_api_key,
    refresh_margin_seconds,
    refresh_concurrency,
    max_concurrent_per_account,
    rotation_strategy,
    model_mappings_json,
    usage_retention_days,
    ops_event_retention_days,
    audit_retention_days,
    updated_at,
    min_codex_desktop_version,
    min_codex_cli_version,
    max_waiting_per_key,
    max_waiting_per_account,
    key_queue_timeout_seconds,
    request_location_enabled,
    request_location_json,
    responses_max_decompressed_body_bytes,
    provider_request_profiles_json,
    pricing_overrides_json,
    pricing_synced_json,
    pricing_synced_at,
    account_warmup_enabled,
    account_warmup_model,
    smart_scheduling_json,
    account_warmup_cursor,
    openai_guardian_reserved_concurrency,
    openai_account_affinity,
    max_account_rotations,
    openai_session_affinity_ttl_hours,
    codex_privacy_policy_json,
    reset_credit_lead_minutes,
    reset_credit_max_weekly_remaining_percent,
    max_retry_after_seconds,
    account_queue_timeout_seconds,
    switch_account_on_queue_failure,
    account_warmup_cron,
    prompt_guard_json,
    claude_public_url,
    anthropic_account_affinity,
    anthropic_session_affinity_ttl_hours,
    anthropic_model_mappings_json,
    min_claude_code_version,
    reset_credit_on_low_weekly_quota,
    peer_routing_id,
    quota_reset_window_weights_json
) VALUES (
    1,
    2,
    NULL,
    300,
    2,
    3,
    'smart',
    '{}',
    31,
    30,
    90,
    now(),
    NULL,
    NULL,
    10,
    10,
    30,
    false,
    '{"city": "Piketon", "region": "Ohio", "country": "US", "timezone": "America/New_York"}',
    67108864,
    '{}',
    '{}',
    '{}',
    NULL,
    false,
    NULL,
    '{"loadWeight": 1.0, "queueWeight": 0.0, "quotaWeight": 0.8, "resetWeight": 0.0, "healthWeight": 1.0, "latencyWeight": 0.5}',
    NULL,
    5,
    'strict',
    3,
    24,
    '{"rules": [], "enabled": false, "onError": "skip_rule"}',
    10,
    NULL,
    240,
    30,
    false,
    '',
    '{"reviewEnabled": false}',
    '',
    'strict',
    720,
    '{}',
    NULL,
    false,
    gen_random_uuid(),
    '{"weekly": 0, "monthly": 0, "fiveHour": 0}'
);
