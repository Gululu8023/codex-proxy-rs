//! Port of CLIProxyAPI 1aaf56bc claude_signing.go (MIT), limited to Claude Code billing/CCH.
//! Hashing uses the serialized wire bytes; the hash view deliberately need not be valid JSON.

use crate::profile::{Effective, version_sequence};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) fn billing(text: &str) -> Option<Vec<(&str, &str)>> {
    let tail = text.strip_prefix("x-anthropic-billing-header: ")?;
    let mut fields = Vec::new();
    for field in tail.strip_suffix(';')?.split(';') {
        let (key, value) = field.trim().split_once('=')?;
        if fields.iter().any(|(name, _)| *name == key) || value.is_empty() || value.len() > 256 {
            return None;
        }
        let valid = match key {
            "cc_version" => value.rsplit_once('.').is_some_and(|(version, hash)| {
                version_sequence(version).is_some()
                    && hash.len() == 3
                    && hash.bytes().all(|b| b.is_ascii_hexdigit())
            }),
            "cc_entrypoint" => matches!(value, "cli" | "cli-bg" | "vscode" | "native"),
            "cch" => {
                value.len() == 5
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }
            "cc_is_subagent" => value == "true",
            "cc_workload" | "cc_prev_req" | "cc_prompt_id" | "cc_turn_origin" => value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
            _ => false,
        };
        if !valid {
            return None;
        }
        fields.push((key, value));
    }
    (fields.iter().any(|(k, _)| *k == "cc_version")
        && fields.iter().any(|(k, _)| *k == "cc_entrypoint"))
    .then_some(fields)
}

fn fingerprint(body: &Value, version: &str) -> String {
    let first = body["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|message| message["role"] == "user"));
    let text = first
        .and_then(|message| {
            message["content"].as_str().or_else(|| {
                message["content"].as_array()?.iter().find_map(|block| {
                    (block["type"] == "text")
                        .then(|| block["text"].as_str())
                        .flatten()
                })
            })
        })
        .unwrap_or("");
    let units: Vec<_> = text.encode_utf16().take(21).collect();
    let sampled = [4, 7, 20].map(|i| units.get(i).copied().unwrap_or(u16::from(b'0')));
    // JavaScript indexing samples UTF-16, including lone surrogates encoded as U+FFFD by Node.
    let sample = String::from_utf16_lossy(&sampled);
    hex::encode(Sha256::digest(
        format!("59cf53e54c78{sample}{version}").as_bytes(),
    ))[..3]
        .to_owned()
}

/// All structural changes happen before Guard and veil. Helper/continue requests acquire no system.
pub(crate) fn prepare(body: &mut Value, profile: &Effective, count: bool) -> Result<bool, ()> {
    if count || body["thread"]["type"] == "continue" || body.get("system").is_none() {
        return Ok(false);
    }
    let system = body.get_mut("system").ok_or(())?;
    if let Some(text) = system.as_str() {
        *system = json!([{"type":"text", "text":text}]);
    }
    let blocks = system.as_array_mut().ok_or(())?;
    let existing = blocks
        .first()
        .filter(|block| block["type"] == "text")
        .and_then(|block| block["text"].as_str())
        .and_then(billing);
    let extras: Vec<_> = existing
        .as_ref()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !matches!(*key, "cc_version" | "cc_entrypoint" | "cch"))
        .map(|(key, value)| format!(" {key}={value};"))
        .collect();
    let text = format!(
        "x-anthropic-billing-header: cc_version={}.000; cc_entrypoint={}; cch=00000;{}",
        profile.version,
        profile.entry,
        extras.join("")
    );
    if existing.is_some() {
        blocks[0]["text"] = json!(text);
    } else {
        blocks.insert(0, json!({"type":"text", "text":text}));
    }
    Ok(true)
}

pub(crate) fn encode(body: &mut Value, profile: &Effective, signed: bool) -> Result<Vec<u8>, ()> {
    if signed {
        let hash = fingerprint(body, &profile.version);
        let marker = body["system"][0]["text"].as_str().ok_or(())?;
        if billing(marker).is_none() {
            return Err(());
        }
        let needle = format!("cc_version={}.000;", profile.version);
        body["system"][0]["text"] = json!(marker.replacen(
            &needle,
            &format!("cc_version={}.{};", profile.version, hash),
            1
        ));
    }
    let mut bytes = serde_json::to_vec(&body).map_err(|_| ())?;
    if !signed {
        return Ok(bytes);
    }
    let mut scanner = Scanner {
        bytes: &bytes,
        pos: 0,
        edits: Vec::new(),
        marker: None,
    };
    scanner.value(true, "")?;
    let marker = scanner.marker.ok_or(())?;
    let offset = bytes[marker.0..marker.1]
        .windows(10)
        .position(|part| part == b"cch=00000;")
        .ok_or(())?
        + marker.0
        + 4;
    let mut edits = scanner.edits;
    edits.sort_unstable();
    let mut hash_view = Vec::with_capacity(bytes.len());
    let mut last = 0;
    for (start, end) in edits {
        if start < last {
            return Err(());
        }
        hash_view.extend_from_slice(&bytes[last..start]);
        last = end;
    }
    hash_view.extend_from_slice(&bytes[last..]);
    let checksum = format!(
        "{:05x}",
        xxhash_rust::xxh64::xxh64(&hash_view, 0x4D659218E32A3268) & 0xfffff
    );
    bytes[offset..offset + 5].copy_from_slice(checksum.as_bytes());
    // This vector is sent directly. Reserialization would invalidate CCH.
    Ok(bytes)
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
    edits: Vec<(usize, usize)>,
    marker: Option<(usize, usize)>,
}
impl Scanner<'_> {
    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }
    fn take(&mut self, byte: u8) -> bool {
        if self.bytes.get(self.pos) == Some(&byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn string(&mut self) -> Result<(usize, usize), ()> {
        let start = self.pos;
        if !self.take(b'"') {
            return Err(());
        }
        while let Some(byte) = self.bytes.get(self.pos) {
            match byte {
                b'\\' => self.pos += 2,
                b'"' => {
                    self.pos += 1;
                    return Ok((start, self.pos));
                }
                _ => self.pos += 1,
            }
        }
        Err(())
    }
    fn value(&mut self, collect: bool, path: &str) -> Result<(), ()> {
        self.whitespace();
        match self.bytes.get(self.pos) {
            Some(b'{') => {
                self.pos += 1;
                self.whitespace();
                if self.take(b'}') {
                    return Ok(());
                }
                let mut members = Vec::new();
                let mut before = None;
                loop {
                    self.whitespace();
                    let start = self.pos;
                    let (key_start, key_end) = self.string()?;
                    let key = &self.bytes[key_start..key_end];
                    let excluded = collect
                        && matches!(
                            key,
                            b"\"max_tokens\"" | b"\"fallbacks\"" | b"\"fallback_credit_token\""
                        );
                    self.whitespace();
                    if !self.take(b':') {
                        return Err(());
                    }
                    self.whitespace();
                    if collect && key == b"\"model\"" && self.bytes.get(self.pos) == Some(&b'"') {
                        let (a, b) = self.string()?;
                        self.edits.push((a + 1, b - 1));
                    } else {
                        let key: String = serde_json::from_slice(key).map_err(|_| ())?;
                        self.value(collect && !excluded, &format!("{path}/{key}"))?;
                    }
                    let end = self.pos;
                    self.whitespace();
                    let after = self.take(b',').then_some(self.pos - 1);
                    members.push((start, end, before, after, excluded));
                    if after.is_some() {
                        before = after;
                        continue;
                    }
                    if !self.take(b'}') {
                        return Err(());
                    }
                    break;
                }
                if collect {
                    let mut start = 0;
                    while start < members.len() {
                        if !members[start].4 {
                            start += 1;
                            continue;
                        }
                        let mut end = start;
                        while end + 1 < members.len() && members[end + 1].4 {
                            end += 1;
                        }
                        let range = if end + 1 < members.len() {
                            (members[start].0, members[end].3.ok_or(())? + 1)
                        }
                        // Native hash retains the preceding comma for multiple trailing dispatch members.
                        else if start > 0 && end == start {
                            (members[start].2.ok_or(())?, members[end].1)
                        } else {
                            (members[start].0, members[end].1)
                        };
                        self.edits.push(range);
                        start = end + 1;
                    }
                }
            }
            Some(b'[') => {
                self.pos += 1;
                self.whitespace();
                if self.take(b']') {
                    return Ok(());
                }
                let mut index = 0;
                loop {
                    self.value(collect, &format!("{path}/{index}"))?;
                    index += 1;
                    self.whitespace();
                    if self.take(b',') {
                        continue;
                    }
                    if !self.take(b']') {
                        return Err(());
                    }
                    break;
                }
            }
            Some(b'"') => {
                let span = self.string()?;
                if path == "/system/0/text" {
                    self.marker = Some(span);
                }
            }
            Some(_) => {
                while self
                    .bytes
                    .get(self.pos)
                    .is_some_and(|byte| !b",}] \t\r\n".contains(byte))
                {
                    self.pos += 1;
                }
            }
            None => return Err(()),
        }
        Ok(())
    }
}
