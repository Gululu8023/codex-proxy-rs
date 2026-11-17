//! 对可见文本构造有界解码视图，无法完整识别的主动编码载荷拒绝放行

use super::envelope::Envelope;
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use regex::Regex;

pub const VERSION: &str = "cprs-normalize-v2";

pub struct Normalizer {
    encoded: Regex,
    escaped: Regex,
    cue: Regex,
}
impl Normalizer {
    pub fn new() -> Result<Self, &'static str> {
        let compile = |text| Regex::new(text).map_err(|_| "prompt guard normalization regex");
        Ok(Self {
            encoded: compile(r"[A-Za-z0-9+/_-]{32,}={0,2}")?,
            escaped: compile(r"(?:\\u[0-9a-fA-F]{4}|\\x[0-9a-fA-F]{2})+")?,
            cue: compile(
                r"(?i)frombase64string|atob\s*\(|encodedcommand|base64\s+(?:--decode|-d)|(?:decode|解码).{0,24}(?:payload|载荷|执行)",
            )?,
        })
    }
    pub fn expand(&self, envelope: &mut Envelope) {
        let mut budget = 1024 * 1024usize;
        let originals = envelope.segments.len();
        for index in 0..originals {
            let mut segment = envelope.segments[index].clone();
            for round in 0..4 {
                let (text, incomplete) = self.decode(&segment.text);
                envelope.unclassified |= incomplete;
                if text == segment.text {
                    break;
                }
                if round == 3 || text.len() > budget {
                    envelope.unclassified = true;
                    break;
                }
                budget -= text.len();
                // 保存完整块及否定/授权上下文，复审只选最后一个展开视图，不附带原编码副本。
                segment.text = text;
                segment.representation = "decoded";
                envelope.segments.push(segment.clone());
            }
        }
    }

    fn decode(&self, original: &str) -> (String, bool) {
        let active = self.cue.is_match(original);
        let mut incomplete = false;
        let text = match percent_encoding::percent_decode_str(original).decode_utf8() {
            Ok(text) => text.into_owned(),
            Err(_) => {
                incomplete |= active;
                original.to_owned()
            }
        };
        let text = self
            .escaped
            .replace_all(&text, |captures: &regex::Captures<'_>| {
                let value = &captures[0];
                let decoded = if value.contains(r"\x") {
                    value
                        .split(r"\x")
                        .skip(1)
                        .map(|byte| u8::from_str_radix(byte, 16))
                        .collect::<Result<Vec<_>, _>>()
                        .ok()
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                } else {
                    serde_json::from_str::<String>(&format!("\"{value}\"")).ok()
                };
                decoded.unwrap_or_else(|| {
                    incomplete = true;
                    value.to_owned()
                })
            })
            .into_owned();
        let text = self
            .encoded
            .replace_all(&text, |captures: &regex::Captures<'_>| {
                let value = &captures[0];
                let bytes = if value.len().is_multiple_of(2)
                    && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    hex::decode(value).ok()
                } else {
                    STANDARD
                        .decode(value)
                        .or_else(|_| STANDARD_NO_PAD.decode(value))
                        .or_else(|_| URL_SAFE.decode(value))
                        .or_else(|_| URL_SAFE_NO_PAD.decode(value))
                        .ok()
                };
                match bytes.and_then(|bytes| String::from_utf8(bytes).ok()) {
                    Some(decoded)
                        if !decoded
                            .chars()
                            .any(|ch| ch.is_control() && !ch.is_whitespace()) =>
                    {
                        decoded
                    }
                    _ => {
                        incomplete |= active;
                        value.to_owned()
                    }
                }
            })
            .into_owned();
        (text, incomplete)
    }
}
