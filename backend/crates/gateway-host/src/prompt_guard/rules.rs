//! 本地规则与已审核规则共用评分器；历史和工具输出只进入审计分。

use super::envelope::Envelope;
use gateway_admin::{model::prompt_guard::PromptGuardConfig, ports::prompt_guard::GuardRule};
use gateway_core::engine::prompt_guard::{CategoryScore, Coverage, PromptGuardAssessment, RuleHit};
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: &str = "codex2api-811ad45-cprs-3";
pub const POLICY_VERSION: &str = "cprs-guard-v2";
pub const CANDIDATE_THRESHOLD: u32 = 50;
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct RuleData {
    name: String,
    pattern: String,
    weight: u32,
    category: String,
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    signal_only: bool,
    #[serde(default)]
    all_patterns: Vec<String>,
    #[serde(default)]
    any_patterns: Vec<String>,
    #[serde(default)]
    exclude_patterns: Vec<String>,
    #[serde(default)]
    authorization_exclude_patterns: Vec<String>,
}
struct Rule {
    data: RuleData,
    source: &'static str,
    pattern: Regex,
    all: Vec<Regex>,
    alternatives: Vec<Regex>,
    exclude: Vec<Regex>,
    authorization: Vec<Regex>,
}
pub struct Rules {
    normalizer: super::normalize::Normalizer,
    rules: Vec<Rule>,
    defensive: Vec<Regex>,
    operational: Vec<Regex>,
    negation: Vec<Regex>,
    benign: Vec<Regex>,
}
pub struct Classification {
    pub assessment: PromptGuardAssessment,
    pub categories: BTreeSet<String>,
    pub required_roots: BTreeSet<usize>,
    pub terminal: bool,
}

const EXECUTING: &[&str] = &[
    "malicious",
    "malware",
    "credential_attack",
    "data_theft",
    "unauthorized_access",
    "remote_access",
    "exploit",
    "post_exploitation",
    "evasion",
    "network_attack",
    "web_attack",
    "api_security",
    "cloud_security",
    "container_security",
    "iot_security",
    "wireless_attack",
    "blockchain_security",
    "crypto_attack",
    "resource_abuse",
    "supply_chain",
    "physical_attack",
    "scanning",
    "reverse_engineering",
    "tooling",
    "vulnerability",
    "prompt_injection",
    "fraud",
    "social_engineering",
    "harassment",
    "sexual_violence",
    "minor_safety",
    "self_harm",
    "violent_extremism",
    "weapons",
];
const OBSERVING: &[&str] = &[
    "license_cracking",
    "illicit",
    "political_manipulation",
    "high_impact_decision",
    "academic_dishonesty",
    "regulated_advice",
    "unsolicited_testing",
    "gambling",
];
fn executable(rule: &RuleData) -> bool {
    EXECUTING.contains(&rule.category.as_str()) || rule.name == "privacy_doxxing"
}
fn compile(value: &str) -> Result<Regex, &'static str> {
    // Go 的单词边界采用 ASCII，直接使用 Rust Unicode 边界会改变中英相邻文本的匹配
    Regex::new(&value.replace(r"\b", r"(?-u:\b)")).map_err(|_| "prompt guard regex")
}
fn patterns(values: &[String]) -> Result<Vec<Regex>, &'static str> {
    values.iter().map(|v| compile(v)).collect()
}
fn any(patterns: &[Regex], text: &str) -> bool {
    patterns.iter().any(|r| r.is_match(text))
}
pub(super) fn matches_custom(rule: &GuardRule, text: &str) -> Result<bool, &'static str> {
    let text = normalize(text);
    Ok(compile(&rule.pattern)?.is_match(&text)
        && patterns(&rule.all_patterns)?
            .iter()
            .all(|r| r.is_match(&text))
        && (rule.any_patterns.is_empty() || any(&patterns(&rule.any_patterns)?, &text))
        && !any(&patterns(&rule.exclude_patterns)?, &text))
}
impl Rules {
    pub fn catalog(&self) -> serde_json::Value {
        serde_json::json!({ "policyVersion": POLICY_VERSION, "rulesVersion": VERSION,
            "normalizationVersion": super::normalize::VERSION, "candidateThreshold": CANDIDATE_THRESHOLD,
            "localTerminationEnabled": true, "executingCategories": Self::execution_categories(), "observingCategories": OBSERVING,
            "rules": self.rules.iter().map(|rule| serde_json::json!({"id": rule.data.name, "category": rule.data.category,
                "weight": rule.data.weight, "signalOnly": rule.data.signal_only, "observationOnly": !executable(&rule.data),
                "localTermination": rule.data.strict, "pattern": rule.data.pattern, "sources": ["current_user"],
                "source": rule.source, "allPatterns": rule.data.all_patterns, "anyPatterns": rule.data.any_patterns,
                "excludePatterns": rule.data.exclude_patterns, "authorizationExcludePatterns": rule.data.authorization_exclude_patterns,
                "intentRequired": matches!(rule.data.category.as_str(), "reverse_engineering" | "tooling" | "vulnerability") })).collect::<Vec<_>>() })
    }
    pub fn new() -> Result<Self, &'static str> {
        let data: Vec<RuleData> =
            serde_json::from_str(include_str!("rules.json")).map_err(|_| "prompt guard rules")?;
        let mut rules = Vec::new();
        let mut ids = BTreeSet::new();
        for data in data {
            if !ids.insert(data.name.clone())
                || (!EXECUTING.contains(&data.category.as_str())
                    && !OBSERVING.contains(&data.category.as_str())
                    && data.category != "privacy")
            {
                return Err("prompt guard rule category");
            }
            rules.push(Rule {
                source: "builtin",
                pattern: compile(&data.pattern)?,
                all: patterns(&data.all_patterns)?,
                alternatives: patterns(&data.any_patterns)?,
                exclude: patterns(&data.exclude_patterns)?,
                authorization: patterns(&data.authorization_exclude_patterns)?,
                data,
            });
        }
        let context: std::collections::BTreeMap<String, Vec<String>> =
            serde_json::from_str(include_str!("context_patterns.json"))
                .map_err(|_| "prompt guard context")?;
        let get = |key: &str| {
            context
                .get(key)
                .ok_or("prompt guard context")
                .and_then(|values| patterns(values))
        };
        Ok(Self {
            normalizer: super::normalize::Normalizer::new()?,
            rules,
            defensive: get("defensiveContextPatterns")?,
            operational: get("operationalRequestPatterns")?,
            negation: get("operationalNegationPatterns")?,
            benign: get("benignOperationalArtifactPatterns")?,
        })
    }
    pub fn with_custom(custom: &[GuardRule]) -> Result<Self, &'static str> {
        let mut result = Self::new()?;
        let mut ids: BTreeSet<String> = result
            .rules
            .iter()
            .map(|rule| rule.data.name.clone())
            .collect();
        for rule in custom {
            Self::validate_custom(rule)?;
            if !ids.insert(rule.id.clone()) {
                return Err("重复的规则标识");
            }
            let data = RuleData {
                name: rule.id.clone(),
                pattern: rule.pattern.clone(),
                category: rule.category.clone(),
                weight: rule.weight,
                strict: rule.strict,
                signal_only: rule.signal_only,
                all_patterns: rule.all_patterns.clone(),
                any_patterns: rule.any_patterns.clone(),
                exclude_patterns: rule.exclude_patterns.clone(),
                authorization_exclude_patterns: Vec::new(),
            };
            // 来源由编译器显式保留，管理页面不能按规则标识前缀猜测。
            result.rules.push(Rule {
                source: "custom",
                pattern: compile(&data.pattern)?,
                all: patterns(&data.all_patterns)?,
                alternatives: patterns(&data.any_patterns)?,
                exclude: patterns(&data.exclude_patterns)?,
                authorization: Vec::new(),
                data,
            });
        }
        Ok(result)
    }
    pub fn validate_custom(rule: &GuardRule) -> Result<(), &'static str> {
        if rule.strict && rule.signal_only {
            return Err("严格规则不能同时仅记录信号");
        }
        if rule.id.is_empty()
            || rule.id.len() > 128
            || !rule
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || rule.pattern.is_empty()
            || rule.pattern.len() > 4096
            || !(1..=1000).contains(&rule.weight)
            || !EXECUTING.contains(&rule.category.as_str())
                && !OBSERVING.contains(&rule.category.as_str())
            || [
                &rule.all_patterns,
                &rule.any_patterns,
                &rule.exclude_patterns,
            ]
            .iter()
            .any(|p| p.len() > 8 || p.iter().any(|v| v.len() > 4096))
        {
            return Err("规则标识、类别、权重或长度无效");
        }
        // 会匹配空串的规则会把无关请求全部命中，不能发布。
        if compile(&rule.pattern)?.is_match("") {
            return Err("规则不能匹配空文本");
        }
        patterns(&rule.all_patterns)?;
        patterns(&rule.any_patterns)?;
        patterns(&rule.exclude_patterns)?;
        Ok(())
    }
    pub fn has_rule(&self, id: &str) -> bool {
        self.rules.iter().any(|rule| rule.data.name == id)
    }
    pub fn execution_categories() -> BTreeSet<String> {
        // 全量复审也必须包含按规则名启用的隐私类别，不能在扩大范围时漏掉 doxxing。
        EXECUTING
            .iter()
            .copied()
            .chain(std::iter::once("privacy"))
            .map(str::to_owned)
            .collect()
    }

    pub fn classify(&self, envelope: &mut Envelope, config: &PromptGuardConfig) -> Classification {
        self.normalizer.expand(envelope);
        let mut assessment = PromptGuardAssessment::pending();
        assessment.policy_version = POLICY_VERSION.into();
        assessment.rules_version = VERSION.into();
        assessment.normalization_version = super::normalize::VERSION.into();
        assessment.coverage = if envelope.unclassified {
            Coverage::Incomplete
        } else if envelope.text_only {
            Coverage::TextOnly
        } else {
            Coverage::Complete
        };
        let mut scores = BTreeMap::<String, CategoryScore>::new();
        let mut dedup = BTreeSet::new();
        let mut effective = BTreeSet::new();
        let mut categories = BTreeSet::new();
        let mut required_roots = BTreeSet::new();
        let mut strict_scores = BTreeMap::<String, u32>::new();
        for segment in &envelope.segments {
            let text = normalize(&segment.text);
            let operational = any(&self.operational, &text) && !any(&self.negation, &text);
            let defensive =
                (any(&self.benign, &text) || any(&self.defensive, &text)) && !operational;
            for rule in &self.rules {
                if config.disabled_rules.contains(&rule.data.name)
                    || !rule.pattern.is_match(&text)
                    || !rule.all.iter().all(|r| r.is_match(&text))
                    || !rule.alternatives.is_empty() && !any(&rule.alternatives, &text)
                    || any(&rule.exclude, &text)
                {
                    continue;
                }
                let executing = executable(&rule.data);
                let intent_required = matches!(
                    rule.data.category.as_str(),
                    "reverse_engineering" | "tooling" | "vulnerability"
                );
                let substantive =
                    executing && !rule.data.signal_only && (!intent_required || operational);
                // 系统指令、仓库内容、历史和工具结果不能变成当前用户的违规事实。
                let current_user = segment.origin == super::envelope::Origin::CurrentUser;
                let contributes = substantive && current_user && !defensive;
                let identity = (rule.data.name.clone(), segment.origin);
                let score =
                    scores
                        .entry(rule.data.category.clone())
                        .or_insert_with(|| CategoryScore {
                            category: rule.data.category.clone(),
                            raw_score: 0,
                            decision_score: 0,
                            observation_only: !executing,
                        });
                if dedup.insert(identity.clone()) {
                    assessment.raw_score = assessment.raw_score.saturating_add(rule.data.weight);
                    score.raw_score = score.raw_score.saturating_add(rule.data.weight);
                    if rule.data.signal_only {
                        assessment.signal_score =
                            assessment.signal_score.saturating_add(rule.data.weight);
                    }
                    if !current_user || !executing {
                        assessment.audit_score =
                            assessment.audit_score.saturating_add(rule.data.weight);
                    }
                }
                let effective_hit = contributes && effective.insert(identity);
                if effective_hit {
                    score.decision_score = score.decision_score.saturating_add(rule.data.weight);
                }
                if effective_hit && rule.data.strict {
                    let strict_score = strict_scores.entry(rule.data.category.clone()).or_default();
                    *strict_score = strict_score.saturating_add(rule.data.weight);
                }
                if substantive && current_user {
                    required_roots.insert(segment.root);
                    // 防御/授权冲突和历史操作证据交给复审，不伪装成高风险分或直接豁免。
                    if defensive || any(&rule.authorization, &text) {
                        categories.insert(rule.data.category.clone());
                    }
                }
                if assessment.hits.len() < 1024 {
                    assessment.hits.push(RuleHit {
                        rule_id: rule.data.name.clone(),
                        category: rule.data.category.clone(),
                        origin: segment.origin.label().into(),
                        source: segment.source.clone(),
                        representation: segment.representation.into(),
                        weight: rule.data.weight,
                        effective_weight: if effective_hit { rule.data.weight } else { 0 },
                        signal_only: rule.data.signal_only,
                        observation_only: !executing,
                    });
                } else {
                    assessment.omitted_hits = assessment.omitted_hits.saturating_add(1);
                }
            }
        }
        assessment.categories = scores.into_values().collect();
        for category in &assessment.categories {
            assessment.decision_score = assessment.decision_score.max(category.decision_score);
            if category.decision_score >= config.threshold {
                categories.insert(category.category.clone());
            }
        }
        // 普通规则不能把低权重的严格标记累加成不可复审的终止结论。
        let terminal = config.strict_terminal_enabled
            && strict_scores
                .values()
                .any(|score| *score >= config.strict_threshold);
        Classification {
            assessment,
            categories,
            required_roots,
            terminal,
        }
    }
}
fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|c| !matches!(c, '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}'))
        .map(|c| {
            if ('\u{ff01}'..='\u{ff5e}').contains(&c) {
                char::from_u32(c as u32 - 0xfee0).expect("fullwidth ASCII")
            } else {
                c
            }
        })
        .collect()
}
