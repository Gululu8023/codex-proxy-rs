//! Anthropic 原始缓存计数、统一用量与 API 等价费用估算

#[cfg(test)]
mod tests;

use gateway_core::{
    event::{GatewayEvent, ProviderEvent, ProviderResponseMetadata, ProviderResponseObservation},
    metering::{CalculatedCost, PricingOverrides, Usage},
    upstream::UpstreamTransport,
};
use serde_json::{Map, Value, json};

#[derive(Default)]
pub(crate) struct UsageCapture {
    fields: Map<String, Value>,
}

impl UsageCapture {
    pub fn observe(&mut self, value: &Value) {
        // 只持久化数字计数，不能把未知 usage 扩展中的文本写入观测
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
            .is_some_and(|input| input > long_threshold)
            && (defaults.is_some_and(|price| price.bands.contains_key("long_standard"))
                || override_model.is_some_and(|price| price.bands.contains_key("long_standard")));
        let mut events = vec![ProviderEvent::canonical(GatewayEvent::Usage(usage))];
        let mut metadata = json!({"schemaVersion":1,"rawUsage":self.fields,"billingKind":"api_equivalent_estimate","usageComplete":finalized});
        // 与管理目录共用固定价目；未识别模型保持费用未知，不按名称猜价格族。
        let band = if long_context {
            "long_standard"
        } else {
            "standard"
        };
        let override_band = override_model.and_then(|model| model.bands.get(band));
        let price = override_band
            .or_else(|| defaults.and_then(|price| price.bands.get(band)))
            .map(|rate| {
                (
                    rate.input.ticks_per_token(),
                    rate.output.ticks_per_token(),
                    rate.cache_read.ticks_per_token(),
                )
            });
        if let (Some((input_rate, output_rate, cache_read_rate)), Some(input), Some(output)) =
            (price, ordinary, output)
        {
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
                (Some(five), Some(hour)) if five.checked_add(hour) == Some(write) => {
                    Some((five, hour))
                }
                (None, None) if write == 0 => Some((0, 0)),
                _ => None,
            };
            if let Some((five, hour)) = split.or_else(|| override_band.map(|_| (0, 0))) {
                let multiplier = override_model.map_or(10_000, |price| price.multiplier_bps);
                let rates = override_band.map(|rate| {
                    (
                        rate.input.ticks_per_token(),
                        rate.output.ticks_per_token(),
                        rate.cache_read.ticks_per_token(),
                        rate.cache_write.ticks_per_token(),
                    )
                });
                // 缓存读取直接取价目；新模型的折扣可能为 2.5% 或 5%，不能统一按输入价的 10% 推导。
                let (in_rate, out_rate, read_rate, write_rate) =
                    rates.unwrap_or((input_rate, output_rate, cache_read_rate, 0));
                let scaled = |tokens: u64, rate: u128| {
                    u128::from(tokens)
                        .checked_mul(rate)?
                        .checked_mul(u128::from(multiplier))
                        .map(|ticks| ticks / 10_000)
                };
                let components = (|| {
                    let input = scaled(input, in_rate)?;
                    let output = scaled(output, out_rate)?;
                    let read = scaled(read.unwrap_or(0), read_rate)?;
                    let five = scaled(five, input_rate * 5 / 4)?;
                    let hour = scaled(hour, input_rate * 2)?;
                    let written = if override_band.is_some() {
                        scaled(write, write_rate)?
                    } else {
                        five.checked_add(hour)?
                    };
                    let total = input
                        .checked_add(output)?
                        .checked_add(read)?
                        .checked_add(written)?;
                    Some((input, output, read, written, five, hour, total))
                })();
                // 费用项分别定点计价再求和，显示明细与账本总额使用同一精度
                // 中断不抹去已观察到的可计价用量；usageComplete 单独表达是否正常结束。
                if request.get("speed").is_none_or(|speed| speed == "standard")
                    && !self.fields.contains_key("server_tool_use")
                    && let Some((input, output, read, written, five, hour, ticks)) = components
                    && let Ok(cost) = CalculatedCost::from_usd_ticks(ticks)
                {
                    let amount = |ticks| {
                        gateway_core::metering::Decimal::from_scaled(ticks)
                            .expect("component bounded by total")
                            .canonical()
                    };
                    metadata["costBreakdown"] = json!({"currency":"USD","input":amount(input),"output":amount(output),"cacheRead":amount(read),
                        "cacheWrite":amount(written),"cacheWrite5m":if override_band.is_none() {Some(amount(five))} else {None},
                        "cacheWrite1h":if override_band.is_none() {Some(amount(hour))} else {None},"total":amount(ticks)});
                    events.push(ProviderEvent::canonical(GatewayEvent::CalculatedCost(cost)));
                }
                metadata["pricing"] = json!({"model":model,"inputTicksPerToken":input_rate,"outputTicksPerToken":output_rate,
                    "band":band,"multiplierBps":multiplier,"override":override_band,
                    "cacheReadTicksPerToken":read_rate,"cacheWrite5mMultiplier":"1.25","cacheWrite1hMultiplier":"2"});
            }
        }
        if let Some(metadata) = ProviderResponseMetadata::new(metadata.to_string()) {
            events.push(ProviderEvent::observation(
                ProviderResponseObservation::new(
                    UpstreamTransport::new("http_sse").expect("transport"),
                )
                .with_provider_metadata(metadata),
            ));
        }
        events
    }
}

/// 目录按五分钟缓存写入展示；一小时及混合写入的实际计价依据保存在请求观测中
pub(crate) fn pricing_catalog() -> gateway_admin::model::pricing::ProviderPricingCatalog {
    default_prices().clone()
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
