//! Anthropic 原始缓存计数、统一用量与 API 等价费用估算

#[cfg(test)]
mod tests;

use gateway_core::{
    event::{GatewayEvent, ProviderEvent, ProviderResponseMetadata, ProviderResponseObservation},
    metering::{CalculatedCost, PricingOverrides, Usage},
};
use serde_json::{Map, Value, json};

#[derive(Default)]
pub(crate) struct UsageCapture {
    fields: Map<String, Value>,
}

impl UsageCapture {
    pub fn observe(&mut self, value: &Value) {
        // 只持久化数字计数和已知计价枚举，不能保存未知 usage 扩展中的文本。
        for name in [
            "input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ] {
            if let Some(number) = value.get(name).and_then(Value::as_u64) {
                self.fields.insert(name.into(), json!(number));
            }
        }
        for (name, allowed) in [
            ("service_tier", &["standard", "priority", "batch"][..]),
            ("speed", &["standard", "fast"][..]),
        ] {
            if let Some(value) = value
                .get(name)
                .and_then(Value::as_str)
                .filter(|value| allowed.contains(value))
            {
                self.fields.insert(name.into(), json!(value));
            }
        }
        // 只使用上游明确给出的思考计数；它已包含在 output_tokens 中，不能再次累加费用。
        if let Some(thinking) = value
            .get("output_tokens_details")
            .and_then(|details| details.get("thinking_tokens"))
            .and_then(Value::as_u64)
        {
            self.fields.insert(
                "output_tokens_details".into(),
                json!({"thinking_tokens": thinking}),
            );
        }
        if value
            .get("server_tool_use")
            .and_then(Value::as_object)
            .is_some_and(|tools| {
                tools
                    .values()
                    .any(|value| value.as_u64().is_some_and(|count| count > 0))
            })
        {
            self.fields.insert("server_tool_use".into(), json!(true));
        }
        if let Some(cache) = value.get("cache_creation") {
            // message_delta 可能只带一个缓存桶，合并累计事实，不能清掉 message_start 的另一桶。
            let mut counts = self
                .fields
                .get("cache_creation")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for name in ["ephemeral_5m_input_tokens", "ephemeral_1h_input_tokens"] {
                if let Some(number) = cache.get(name).and_then(Value::as_u64) {
                    counts.insert(name.into(), json!(number));
                }
            }
            if !counts.is_empty() {
                self.fields
                    .insert("cache_creation".into(), Value::Object(counts));
            }
        }
    }

    pub fn finish(
        &self,
        model: &str,
        pricing: &PricingOverrides,
        request: &Value,
        finalized: bool,
        observation: &ProviderResponseObservation,
    ) -> Vec<ProviderEvent> {
        let ordinary = self.fields.get("input_tokens").and_then(Value::as_u64);
        let read = self
            .fields
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64);
        let write = self
            .fields
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64);
        let output = self.fields.get("output_tokens").and_then(Value::as_u64);
        let input = ordinary
            .and_then(|input| input.checked_add(read.unwrap_or(0)))
            .and_then(|input| input.checked_add(write.unwrap_or(0)));
        let usage = Usage {
            input_tokens: input,
            total_tokens: input
                .zip(output)
                .and_then(|(input, output)| input.checked_add(output)),
            output_tokens: output,
            reasoning_output_tokens: self
                .fields
                .get("output_tokens_details")
                .and_then(|details| details.get("thinking_tokens"))
                .and_then(Value::as_u64)
                .filter(|thinking| output.is_some_and(|total| *thinking <= total)),
            cached_input_tokens: read,
            cache_write_input_tokens: write,
            ..Usage::new()
        };
        let defaults = default_prices().get(model);
        let override_model = pricing
            .get(crate::NAME)
            .and_then(|models| models.get(model));
        // Haiku 5.5 按包含缓存的完整输入在 100K 分档；旧 Sonnet 长上下文价仍以 200K 为界。
        let long_threshold = if model == "claude-haiku-5-5" {
            100_000
        } else {
            200_000
        };
        let long_context = usage
            .input_tokens
            .is_some_and(|input| input > long_threshold);
        let select_band = |normal, long| {
            if long_context
                && (defaults.is_some_and(|price| price.bands.contains_key(long))
                    || override_model.is_some_and(|price| price.bands.contains_key(long)))
            {
                long
            } else {
                normal
            }
        };
        let mut events = vec![ProviderEvent::canonical(GatewayEvent::Usage(usage))];
        // Core 保存最后一份完整观测；计量更新必须保留响应头、计时及先前的额度事实。
        let mut metadata = observation.provider_metadata().map_or_else(
            || json!({}),
            |metadata| {
                serde_json::from_str::<Value>(metadata.as_json())
                    .expect("validated provider metadata")
            },
        );
        metadata["schemaVersion"] = json!(1);
        metadata["rawUsage"] = json!(self.fields);
        metadata["billingKind"] = json!("api_equivalent_estimate");
        metadata["usageComplete"] = json!(finalized);
        // 新增工具费用或不完整缓存桶可能使后续用量不可计价，不能沿用上一帧的费用明细。
        let saved = metadata.as_object_mut().expect("provider metadata object");
        saved.remove("costBreakdown");
        saved.remove("pricing");
        // 与管理目录共用固定价目；未识别模型保持费用未知，不按名称猜价格族。
        let service_tier = self.fields.get("service_tier").and_then(Value::as_str);
        let observed_speed = self.fields.get("speed").and_then(Value::as_str);
        // Priority 是容量档位，不是 Fast；速度以响应为准，不能按请求意图收取 Fast 费用。
        // 普通请求缺少 speed 时沿用标准价估算，Fast 请求未确认速度时保持费用未知。
        let speed = observed_speed.or_else(|| {
            request
                .get("speed")
                .is_none_or(|speed| speed == "standard")
                .then_some("standard")
        });
        let standard_band = select_band("standard", "long_standard");
        let band = if speed == Some("fast") {
            select_band("fast", "long_fast")
        } else {
            standard_band
        };
        let override_band = override_model.and_then(|model| model.bands.get(band));
        let one_hour = self
            .fields
            .get("cache_creation")
            .and_then(|cache| cache.get("ephemeral_1h_input_tokens"))
            .and_then(Value::as_u64);
        let five_minutes = self
            .fields
            .get("cache_creation")
            .and_then(|cache| cache.get("ephemeral_5m_input_tokens"))
            .and_then(Value::as_u64);
        let write = write.unwrap_or(0);
        let split = match (five_minutes, one_hour) {
            (Some(five), Some(hour)) if five.checked_add(hour) == Some(write) => Some((five, hour)),
            (None, None) if write == 0 => Some((0, 0)),
            _ => None,
        };
        let multiplier = override_model.map_or(10_000, |price| price.multiplier_bps);
        // 标准与实际模式复用同一组用量、缓存 TTL 和定点精度，只切换价格档位。
        let quote = |band: &str, multiplier: u32| {
            let custom = override_model.and_then(|model| model.bands.get(band));
            let rate = custom.or_else(|| defaults.and_then(|price| price.bands.get(band)))?;
            let (five, hour) = split.or_else(|| custom.map(|_| (0, 0)))?;
            let input_rate = rate.input.ticks_per_token();
            let scaled = |tokens: u64, rate: u128| {
                u128::from(tokens)
                    .checked_mul(rate)?
                    .checked_mul(u128::from(multiplier))
                    .map(|ticks| ticks / 10_000)
            };
            let input = scaled(ordinary?, input_rate)?;
            let output = scaled(output?, rate.output.ticks_per_token())?;
            // 缓存读取直接取价目，不能统一按输入价的 10% 推导。
            let read = scaled(read.unwrap_or(0), rate.cache_read.ticks_per_token())?;
            let five = scaled(five, input_rate * 5 / 4)?;
            let hour = scaled(hour, input_rate * 2)?;
            let written = if custom.is_some() {
                scaled(write, rate.cache_write.ticks_per_token())?
            } else {
                five.checked_add(hour)?
            };
            let total = input
                .checked_add(output)?
                .checked_add(read)?
                .checked_add(written)?;
            Some((input, output, read, written, five, hour, total))
        };
        if let Some((input, output, read, written, five, hour, ticks)) = quote(band, multiplier) {
            // 速度倍率独立于自定义倍率；自定义倍率为零时仍保留价差，超出 JSON 整数范围时保持未知。
            let speed_multiplier = if band == standard_band {
                Some(10_000)
            } else {
                quote(band, 10_000)
                    .zip(quote(standard_band, 10_000))
                    .and_then(|(selected, standard)| {
                        selected.6.checked_mul(10_000)?.checked_div(standard.6)
                    })
                    .and_then(|ratio| u64::try_from(ratio).ok())
            };
            let rate = override_band
                .or_else(|| defaults.and_then(|price| price.bands.get(band)))
                .expect("quoted price band");
            // 费用项分别定点计价再求和；中断不抹去已观察到的可计价用量。
            // Priority 合约费用不在此计算；这里始终是当前速度的公开 API 等价估算。
            if speed.is_some()
                && service_tier != Some("batch")
                && !self.fields.contains_key("server_tool_use")
                && let Ok(cost) = CalculatedCost::from_usd_ticks(ticks)
            {
                let amount = |ticks| {
                    gateway_core::metering::Decimal::from_scaled(ticks)
                        .expect("component bounded by total")
                        .canonical()
                };
                let standard = quote(standard_band, multiplier)
                    .and_then(|quote| gateway_core::metering::Decimal::from_scaled(quote.6).ok())
                    .map(|amount| amount.canonical());
                metadata["costBreakdown"] = json!({"currency":"USD","input":amount(input),"output":amount(output),"cacheRead":amount(read),
                    "cacheWrite":amount(written),"cacheWrite5m":if override_band.is_none() {Some(amount(five))} else {None},
                    "cacheWrite1h":if override_band.is_none() {Some(amount(hour))} else {None},"standard":standard,"total":amount(ticks)});
                events.push(ProviderEvent::canonical(GatewayEvent::CalculatedCost(cost)));
            }
            metadata["pricing"] = json!({"model":model,"inputTicksPerToken":rate.input.ticks_per_token(),"outputTicksPerToken":rate.output.ticks_per_token(),
                "band":band,"multiplierBps":multiplier,"override":override_band,
                "serviceTier":service_tier,"speed":observed_speed,"speedMultiplierBps":speed_multiplier,
                "cacheReadTicksPerToken":rate.cache_read.ticks_per_token(),"cacheWrite5mMultiplier":"1.25","cacheWrite1hMultiplier":"2"});
        }
        if let Some(metadata) = ProviderResponseMetadata::new(metadata.to_string()) {
            events.push(ProviderEvent::observation(
                observation.clone().with_provider_metadata(metadata),
            ));
        }
        events
    }
}

/// 目录按五分钟缓存写入展示；一小时及混合写入的实际计价依据保存在请求观测中
pub(crate) fn pricing_catalog() -> gateway_admin::model::pricing::ProviderPricingCatalog {
    default_prices().clone()
}

/// 历史写入可能混合 5m/1h TTL；采用请求时分项，不能伪造统一写入单价。
pub(crate) fn recorded_billing_projection(
    input: &gateway_admin::model::observability::ProviderBillingInput,
    recorded: &Value,
) -> Option<gateway_admin::model::observability::UsageBillingProjection> {
    use gateway_admin::model::observability::{DecimalAmount, UsageBillingProjection};
    use gateway_core::metering::Decimal;

    let breakdown = &recorded["costBreakdown"];
    let pricing = &recorded["pricing"];
    if recorded["kind"] != "api_equivalent_estimate"
        || input.total.currency != "USD"
        || breakdown["currency"] != "USD"
        || pricing["model"].as_str() != Some(input.upstream_model_id.as_str())
    {
        return None;
    }
    let amount = |key: &str| breakdown[key].as_str()?.parse::<Decimal>().ok();
    let total = amount("total")?;
    if total.canonical() != input.total.amount.as_str() {
        return None;
    }
    let ordinary = amount("input")?;
    let output = amount("output")?;
    let read = amount("cacheRead")?;
    let write = amount("cacheWrite")?;
    if ordinary
        .checked_add(output)?
        .checked_add(read)?
        .checked_add(write)?
        != total
    {
        return None;
    }
    let input_tokens = input.input_tokens?;
    let read_tokens = input.cached_input_tokens.unwrap_or_default();
    let write_tokens = input.cache_write_input_tokens.unwrap_or_default();
    let ordinary_tokens = input_tokens
        .checked_sub(read_tokens)?
        .checked_sub(write_tokens)?;
    let input_rate = pricing["inputTicksPerToken"].as_u64()?;
    let output_rate = pricing["outputTicksPerToken"].as_u64()?;
    let multiplier = pricing["multiplierBps"].as_u64()?;
    let quote = |tokens: u64, rate: u64| {
        u128::from(tokens)
            .checked_mul(u128::from(rate))?
            .checked_mul(u128::from(multiplier))
            .map(|ticks| ticks / 10_000)
    };
    // 核对持久用量与历史单价，避免把不匹配的观测当作可比样本。
    if quote(ordinary_tokens, input_rate)? != ordinary.scaled()
        || quote(input.output_tokens?, output_rate)? != output.scaled()
    {
        return None;
    }
    let no_cache = quote(input_tokens, input_rate)?.checked_add(output.scaled())?;
    Some(UsageBillingProjection {
        standard_amount: breakdown["standard"]
            .as_str()
            .and_then(|value| value.parse::<DecimalAmount>().ok()),
        no_cache_amount: Decimal::from_scaled(no_cache)
            .ok()?
            .canonical()
            .parse()
            .ok(),
    })
}

fn default_prices() -> &'static gateway_admin::model::pricing::ProviderPricingCatalog {
    static PRICES: std::sync::OnceLock<gateway_admin::model::pricing::ProviderPricingCatalog> =
        std::sync::OnceLock::new();
    PRICES.get_or_init(|| {
        use gateway_core::metering::{ModelPriceOverride, TokenPriceOverride};
        let rates = |input: &str, output: &str, read: &str, write: &str| TokenPriceOverride {
            input: input.to_owned().try_into().expect("fixed price"),
            output: output.to_owned().try_into().expect("fixed price"),
            cache_read: read.to_owned().try_into().expect("fixed price"),
            cache_write: write.to_owned().try_into().expect("fixed price"),
        };
        // 2026-11-12 核对官方价目 https://platform.claude.com/docs/en/about-claude/pricing 。
        // 这是费用估算目录；账号可用模型仍由原生 /v1/models 与 Key 权限决定。
        [
            ("claude-fable-5-1", None, rates("10", "50", "0.25", "12.5")),
            ("claude-mythos-5-1", None, rates("10", "50", "0.25", "12.5")),
            ("claude-fable-5", None, rates("10", "50", "1", "12.5")),
            ("claude-mythos-5", None, rates("10", "50", "1", "12.5")),
            ("claude-opus-5-5", None, rates("4", "20", "0.2", "5")),
            ("claude-opus-5", None, rates("5", "25", "0.5", "6.25")),
            ("claude-opus-4-8", None, rates("5", "25", "0.5", "6.25")),
            ("claude-opus-4-7", None, rates("5", "25", "0.5", "6.25")),
            ("claude-opus-4-6", None, rates("5", "25", "0.5", "6.25")),
            ("claude-opus-4-5", None, rates("5", "25", "0.5", "6.25")),
            (
                "claude-opus-4-5-20251101",
                None,
                rates("5", "25", "0.5", "6.25"),
            ),
            ("claude-sonnet-5-5", None, rates("2", "10", "0.1", "2.5")),
            ("claude-sonnet-5", None, rates("2", "10", "0.2", "2.5")),
            ("claude-sonnet-4-6", None, rates("3", "15", "0.3", "3.75")),
            (
                "claude-sonnet-4-5",
                Some(rates("6", "22.5", "0.6", "7.5")),
                rates("3", "15", "0.3", "3.75"),
            ),
            (
                "claude-sonnet-4-5-20250929",
                Some(rates("6", "22.5", "0.6", "7.5")),
                rates("3", "15", "0.3", "3.75"),
            ),
            (
                "claude-sonnet-4-20250514",
                Some(rates("6", "22.5", "0.6", "7.5")),
                rates("3", "15", "0.3", "3.75"),
            ),
            (
                "claude-haiku-5-5",
                Some(rates("0.5", "2.5", "0.05", "0.625")),
                rates("0.1", "0.5", "0.01", "0.125"),
            ),
            ("claude-haiku-4-5", None, rates("1", "5", "0.1", "1.25")),
            (
                "claude-haiku-4-5-20251001",
                None,
                rates("1", "5", "0.1", "1.25"),
            ),
        ]
        .into_iter()
        .map(|(model, long, standard)| {
            let mut bands = std::collections::BTreeMap::from([("standard".into(), standard)]);
            if let Some(long) = long {
                bands.insert("long_standard".into(), long);
            }
            // 2026-11-19 核对官方 Fast 价；全上下文同价，不能把 Priority 当作 Fast。
            let fast = match model {
                "claude-opus-5-5" => Some(rates("8", "40", "0.4", "10")),
                "claude-opus-5" | "claude-opus-4-8" => Some(rates("10", "50", "1", "12.5")),
                _ => None,
            };
            if let Some(fast) = fast {
                bands.insert("fast".into(), fast);
            }
            (
                model.into(),
                ModelPriceOverride {
                    multiplier_bps: 10_000,
                    bands,
                },
            )
        })
        .collect()
    })
}
