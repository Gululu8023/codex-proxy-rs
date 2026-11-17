//! Codex 与 Claude 共用的有界 HTTP 请求体解压，不包含协议字段解析。

use std::borrow::Cow;

use axum::http::{HeaderMap, header::CONTENT_ENCODING};

pub(crate) enum RequestBodyDecodeError {
    MalformedBody,
    TooLarge { limit_bytes: usize },
    UnsupportedEncoding { encoding: String },
}

/// zstd 回溯窗口独立于输出上限，调整设置不能放大解码器内部窗口分配。
const MAX_ZSTD_WINDOW_LOG: u32 = 26;

/// 只接受单一编码；压缩和未压缩正文均受同一个请求快照上限约束。
pub(crate) fn decompress_request_body<'a>(
    body: &'a [u8],
    headers: &HeaderMap,
    max_decompressed_bytes: usize,
) -> Result<Cow<'a, [u8]>, RequestBodyDecodeError> {
    let encoding = headers
        .get_all(CONTENT_ENCODING)
        .iter()
        .map(|value| {
            value
                .to_str()
                .map_err(|_| RequestBodyDecodeError::MalformedBody)
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(",");
    match encoding.trim().to_ascii_lowercase().as_str() {
        "" | "identity" => {
            if body.len() > max_decompressed_bytes {
                return Err(RequestBodyDecodeError::TooLarge {
                    limit_bytes: max_decompressed_bytes,
                });
            }
            Ok(Cow::Borrowed(body))
        }
        "gzip" => {
            let decoder = flate2::read::MultiGzDecoder::new(body);
            read_bounded(decoder, max_decompressed_bytes).map(Cow::Owned)
        }
        "deflate" => {
            let decoder = flate2::read::ZlibDecoder::new(body);
            read_bounded(decoder, max_decompressed_bytes).map(Cow::Owned)
        }
        "zstd" => {
            let mut decoder = zstd::stream::read::Decoder::with_buffer(body)
                .map_err(|_| RequestBodyDecodeError::MalformedBody)?;
            decoder
                .window_log_max(MAX_ZSTD_WINDOW_LOG)
                .map_err(|_| RequestBodyDecodeError::MalformedBody)?;
            read_bounded(decoder, max_decompressed_bytes).map(Cow::Owned)
        }
        other => Err(RequestBodyDecodeError::UnsupportedEncoding {
            encoding: other.to_owned(),
        }),
    }
}

fn read_bounded<R: std::io::Read>(
    mut reader: R,
    max_decompressed_bytes: usize,
) -> Result<Vec<u8>, RequestBodyDecodeError> {
    let mut decoded = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let remaining = max_decompressed_bytes - decoded.len();
        let read_limit = chunk.len().min(remaining.saturating_add(1));
        let read = reader
            .read(&mut chunk[..read_limit])
            .map_err(|_| RequestBodyDecodeError::MalformedBody)?;
        if read == 0 {
            return Ok(decoded);
        }
        if read > remaining {
            return Err(RequestBodyDecodeError::TooLarge {
                limit_bytes: max_decompressed_bytes,
            });
        }
        // 容量增长也受相同边界约束，越界探测只使用栈上分块缓冲区。
        let required = decoded.len() + read;
        if required > decoded.capacity() {
            let capacity = decoded
                .capacity()
                .saturating_mul(2)
                .max(required)
                .min(max_decompressed_bytes);
            decoded.reserve_exact(capacity - decoded.len());
        }
        decoded.extend_from_slice(&chunk[..read]);
    }
}
