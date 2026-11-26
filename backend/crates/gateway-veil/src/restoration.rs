//! 未还原引用的有界诊断；只输出指纹，不改变映射查找或原样保留的结果。

use std::{collections::BTreeSet, sync::Mutex};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use veil_engine::{
    creator::Creator,
    mapping::{Lookup, MappingStore, StoreError},
    placeholder::{PLACEHOLDER_REGEX, Placeholder, parse_placeholder},
};

const MAX_REFERENCES: usize = 4096;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingMapping {
    placeholder_sha256: String,
    id_sha256: String,
    creator_sha256: String,
    request_reference_count: usize,
    request_references_complete: bool,
    seen_in_redacted_request: Option<bool>,
    closest_request_id_distance: Option<usize>,
    closest_request_id_sha256: Option<String>,
}

#[derive(Default)]
pub(super) struct Observations {
    request_ids: BTreeSet<String>,
    complete: bool,
    missing: Option<MissingMapping>,
}

impl Observations {
    pub(super) fn request(&mut self, value: &Value) {
        self.request_ids.clear();
        self.complete = true;
        self.missing = None;
        self.collect(value);
    }

    fn collect(&mut self, value: &Value) {
        match value {
            Value::String(text) => self.collect_text(text),
            Value::Array(items) => {
                for item in items {
                    self.collect(item);
                }
            }
            Value::Object(object) => {
                for (key, value) in object {
                    self.collect_text(key);
                    self.collect(value);
                }
            }
            _ => {}
        }
    }

    fn collect_text(&mut self, text: &str) {
        for matched in PLACEHOLDER_REGEX.find_iter(text) {
            let Some(placeholder) = parse_placeholder(matched.as_str()) else {
                continue;
            };
            let id = placeholder.id.to_string();
            if self.request_ids.len() < MAX_REFERENCES || self.request_ids.contains(&id) {
                self.request_ids.insert(id);
            } else {
                self.complete = false;
            }
        }
    }

    pub(super) fn take_missing(&mut self) -> Option<MissingMapping> {
        self.missing.take()
    }

    fn record(&mut self, creator: &Creator, placeholder: &Placeholder) {
        let id = placeholder.id.to_string();
        let seen = self.request_ids.contains(&id);
        // SQLite 按 ULID 查找，前缀不参与 lookup；距离只描述字符变化，绝不用来猜测恢复。
        let mut closest: Option<(usize, &str)> = None;
        let mut ties = false;
        for candidate in &self.request_ids {
            let distance = id
                .bytes()
                .zip(candidate.bytes())
                .filter(|(a, b)| a != b)
                .count();
            match closest {
                Some((best, _)) if distance > best => {}
                Some((best, _)) if distance == best => ties = true,
                _ => {
                    closest = Some((distance, candidate));
                    ties = false;
                }
            }
        }
        self.missing = Some(MissingMapping {
            placeholder_sha256: fingerprint(placeholder.format()),
            id_sha256: fingerprint(&id),
            creator_sha256: fingerprint(creator.hex()),
            request_reference_count: self.request_ids.len(),
            request_references_complete: self.complete,
            // 超出诊断预算时，未命中只能表示未知，不能宣称它未出现在请求中。
            seen_in_redacted_request: if seen {
                Some(true)
            } else {
                self.complete.then_some(false)
            },
            closest_request_id_distance: closest.map(|(distance, _)| distance),
            closest_request_id_sha256: closest.filter(|_| !ties).map(|(_, id)| fingerprint(id)),
        });
    }
}

fn fingerprint(value: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(value.as_ref()))
}

pub(super) struct Store<'a> {
    pub(super) inner: &'a veil_store::SqliteStore,
    pub(super) observations: &'a Mutex<Observations>,
}

impl MappingStore for Store<'_> {
    fn get_or_insert(
        &self,
        creator: &Creator,
        prefix: &str,
        text: &str,
    ) -> Result<Placeholder, StoreError> {
        self.inner.get_or_insert(creator, prefix, text)
    }

    fn lookup(&self, creator: &Creator, placeholder: &Placeholder) -> Result<Lookup, StoreError> {
        let result = self.inner.lookup(creator, placeholder);
        if matches!(&result, Ok(Lookup::Miss)) {
            self.observations
                .lock()
                .expect("restoration observations")
                .record(creator, placeholder);
        }
        result
    }
}

#[cfg(test)]
mod tests;
