//! 协议判定后的有界文本缓存；持久映射仍是唯一恢复依据。

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    sync::Arc,
    time::Instant,
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use veil_engine::{
    creator::Creator,
    mapping::{Lookup, MappingStore, StoreError},
    placeholder::{PLACEHOLDER_REGEX, Placeholder, parse_placeholder},
};

use super::{Error, State};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_CREATOR_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 256 * 1024;
const MAX_ENTRIES: usize = 2048;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionStatistics {
    pub worker_wait_us: u64,
    pub processing_us: u64,
    pub text_processing_us: u64,
    pub mapping_us: u64,
    pub verification_us: u64,
    pub text_bytes: usize,
    pub scanned_bytes: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub mapping_operations: usize,
    pub mapping_reuses: usize,
}

pub struct RedactionResult {
    pub value: serde_json::Value,
    pub statistics: RedactionStatistics,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    creator: Creator,
    digest: [u8; 32],
}

#[derive(Clone)]
struct CachedText {
    text: Arc<str>,
    references: Arc<[Placeholder]>,
    size: usize,
}

struct Entry {
    value: CachedText,
    used_at: u64,
}

/// 每个不可变 State 拥有独立缓存，规则、密钥或映射卷代次变化不会继承缓存。
#[derive(Default)]
pub(super) struct Cache {
    entries: HashMap<Key, Entry>,
    bytes: usize,
    clock: u64,
}

impl Cache {
    fn get(&mut self, key: &Key) -> Option<CachedText> {
        let entry = self.entries.get_mut(key)?;
        self.clock += 1;
        entry.used_at = self.clock;
        Some(entry.value.clone())
    }

    fn insert(&mut self, key: Key, value: CachedText) {
        if value.size > MAX_ENTRY_BYTES || self.entries.contains_key(&key) {
            return;
        }
        let mut creator_bytes: usize = self
            .entries
            .iter()
            .filter(|(entry, _)| entry.creator == key.creator)
            .map(|(_, entry)| entry.value.size)
            .sum();
        while self.bytes + value.size > MAX_BYTES
            || creator_bytes + value.size > MAX_CREATOR_BYTES
            || self.entries.len() >= MAX_ENTRIES
        {
            let creator_full = creator_bytes + value.size > MAX_CREATOR_BYTES;
            let oldest = self
                .entries
                .iter()
                .filter(|(entry, _)| !creator_full || entry.creator == key.creator)
                .min_by_key(|(_, entry)| entry.used_at)
                .map(|(entry, _)| *entry)
                .expect("an over-budget cache has an eviction candidate");
            let removed = self.entries.remove(&oldest).expect("selected cache entry");
            self.bytes -= removed.value.size;
            if oldest.creator == key.creator {
                creator_bytes -= removed.value.size;
            }
        }
        self.clock += 1;
        self.bytes += value.size;
        self.entries.insert(
            key,
            Entry {
                value,
                used_at: self.clock,
            },
        );
    }
}

struct RequestMappings<'a> {
    store: &'a veil_store::SqliteStore,
    memo: RefCell<HashMap<Key, Placeholder>>,
    operations: Cell<usize>,
    reuses: Cell<usize>,
    elapsed_us: Cell<u64>,
}

impl MappingStore for RequestMappings<'_> {
    fn get_or_insert(
        &self,
        creator: &Creator,
        prefix: &str,
        text: &str,
    ) -> Result<Placeholder, StoreError> {
        // 映射库按 creator+原文唯一化，重复命中必须保留首次命中的类型前缀。
        let key = Key {
            creator: *creator,
            digest: Sha256::digest(text.as_bytes()).into(),
        };
        if let Some(value) = self.memo.borrow().get(&key).cloned() {
            self.reuses.set(self.reuses.get() + 1);
            return Ok(value);
        }
        let started = Instant::now();
        let value = self.store.get_or_insert(creator, prefix, text);
        self.elapsed_us.set(self.elapsed_us.get() + micros(started));
        self.operations.set(self.operations.get() + 1);
        if let Ok(value) = &value
            && self.memo.borrow().len() < MAX_ENTRIES
        {
            self.memo.borrow_mut().insert(key, value.clone());
        }
        value
    }

    fn lookup(&self, creator: &Creator, placeholder: &Placeholder) -> Result<Lookup, StoreError> {
        self.store.lookup(creator, placeholder)
    }
}

pub(super) fn redact(
    state: &State,
    creator: Creator,
    value: &serde_json::Value,
) -> Result<RedactionResult, Error> {
    let started = Instant::now();
    let mappings = RequestMappings {
        store: &state.store,
        memo: RefCell::default(),
        operations: Cell::new(0),
        reuses: Cell::new(0),
        elapsed_us: Cell::new(0),
    };
    let mut statistics = RedactionStatistics::default();
    let mut references = HashMap::new();
    let value = veil_engine::anthropic::transform_text(value, &mut |text| {
        statistics.text_bytes += text.len();
        let key = Key {
            creator,
            digest: Sha256::digest(text.as_bytes()).into(),
        };
        let cached = state
            .redaction_cache
            .lock()
            .expect("redaction cache")
            .get(&key);
        if let Some(cached) = cached {
            statistics.cache_hits += 1;
            for reference in cached.references.iter() {
                references
                    .entry(reference.format())
                    .or_insert_with(|| reference.clone());
            }
            return Ok(cached.text.to_string());
        }
        statistics.cache_misses += 1;
        statistics.scanned_bytes += text.len();
        let text_started = Instant::now();
        let prior_mapping_us = mappings.elapsed_us.get();
        let output = veil_engine::walk::desensitize_literal_safe_string(
            text,
            &state.rules,
            &mappings,
            &creator,
        )?;
        statistics.text_processing_us +=
            micros(text_started).saturating_sub(mappings.elapsed_us.get() - prior_mapping_us);
        if text.len() <= MAX_ENTRY_BYTES && output.len() <= MAX_ENTRY_BYTES {
            let references: Vec<_> = PLACEHOLDER_REGEX
                .find_iter(&output)
                .filter_map(|matched| parse_placeholder(matched.as_str()))
                .collect();
            let size = output.len()
                + std::mem::size_of::<Entry>()
                + std::mem::size_of::<Key>()
                + references
                    .iter()
                    .map(|item| std::mem::size_of::<Placeholder>() + item.type_prefix.len())
                    .sum::<usize>();
            state
                .redaction_cache
                .lock()
                .expect("redaction cache")
                .insert(
                    key,
                    CachedText {
                        text: Arc::from(output.as_str()),
                        references: Arc::from(references),
                        size,
                    },
                );
        }
        Ok(output)
    })
    .map_err(Error::from)?;
    let verification_started = Instant::now();
    // 暖缓存不能掩盖映射丢失、损坏或作用域不符；只读批量校验避免逐条更新时间。
    if !state
        .store
        .verify_placeholders(&creator, &references.into_values().collect::<Vec<_>>())
        .map_err(|_| Error::StoreUnavailable)?
    {
        return Err(Error::MappingMissing);
    }
    statistics.verification_us = micros(verification_started);
    statistics.mapping_us = mappings.elapsed_us.get();
    statistics.mapping_operations = mappings.operations.get();
    statistics.mapping_reuses = mappings.reuses.get();
    statistics.processing_us = micros(started);
    Ok(RedactionResult { value, statistics })
}

fn micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
