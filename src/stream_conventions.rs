pub(crate) const STREAMING_ROUTE_BASE: &str = "/weeb-3";
pub(crate) const STREAMING_SERVICE_WORKER_URL: &str = "/weeb-3/service.js";
pub(crate) const STREAMING_SERVICE_WORKER_SCOPE: &str = "/weeb-3/";

const MAX_STREAM_TOPIC_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsStart {
    Beginning,
    Live,
}

pub(crate) fn streaming_route_path(suffix: &str) -> String {
    let suffix = suffix.trim_start_matches('/');
    if suffix.is_empty() {
        STREAMING_ROUTE_BASE.to_string()
    } else {
        format!("{STREAMING_ROUTE_BASE}/{suffix}")
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn route_markers(kind: &str) -> Vec<String> {
    ["", "mainnet/", "testnet/"]
        .into_iter()
        .map(|network| streaming_route_path(&format!("{network}{kind}/")))
        .collect()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn decode_component(value: &str) -> String {
    js_sys::decode_uri_component(value)
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| value.to_string())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StreamShareRoute {
    pub owner: String,
    pub topic: String,
    pub start: HlsStart,
}

impl StreamShareRoute {
    pub(crate) fn new(owner: impl Into<String>, topic: impl Into<String>) -> Result<Self, String> {
        let owner = owner.into();
        let topic = topic.into();
        if owner.len() != 40 || !owner.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("stream owner must be a 20-byte hexadecimal address".into());
        }
        if topic.is_empty()
            || topic.len() > MAX_STREAM_TOPIC_BYTES
            || topic.chars().any(char::is_control)
            || matches!(topic.as_str(), "." | "..")
        {
            return Err("stream topic is invalid".into());
        }
        Ok(Self {
            owner: owner.to_ascii_lowercase(),
            topic,
            start: HlsStart::Beginning,
        })
    }
}

pub(crate) fn parse_stream_share_link(input: &str) -> Result<StreamShareRoute, String> {
    let input = input.trim();
    if input.is_empty() || input.contains(['?', '#', '\\']) || input.chars().any(char::is_control) {
        return Err("stream link must be a clean path".into());
    }
    let route = input
        .strip_prefix(STREAMING_ROUTE_BASE)
        .and_then(|tail| tail.strip_prefix('/'))
        .unwrap_or_else(|| input.trim_start_matches('/'));
    let mut parts = route.split('/');
    let start = match parts.next() {
        Some("stream") => HlsStart::Beginning,
        Some("live") if parts.next() == Some("stream") => HlsStart::Live,
        _ => return Err("stream link has an invalid path".into()),
    };
    let owner = parts
        .next()
        .ok_or_else(|| "stream link is missing its owner".to_string())?;
    let topic = parts
        .next()
        .ok_or_else(|| "stream link is missing its topic".to_string())?;
    if parts.next().is_some() {
        return Err("stream link has an invalid path".into());
    }
    Ok(StreamShareRoute {
        start,
        ..StreamShareRoute::new(owner, decode_path_segment(topic)?)?
    })
}

fn decode_path_segment(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'%' {
            decoded.push(bytes[cursor]);
            cursor += 1;
            continue;
        }
        let high = bytes
            .get(cursor + 1)
            .and_then(|byte| hex_value(*byte))
            .ok_or_else(|| "stream topic has an invalid percent escape".to_string())?;
        let low = bytes
            .get(cursor + 2)
            .and_then(|byte| hex_value(*byte))
            .ok_or_else(|| "stream topic has an invalid percent escape".to_string())?;
        decoded.push((high << 4) | low);
        cursor += 3;
    }
    String::from_utf8(decoded).map_err(|_| "stream topic is not valid UTF-8".to_string())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn parse_single_range(range: Option<&str>, size: u64) -> Option<Result<(u64, u64), ()>> {
    let range = range?.trim();
    let Some((unit, spec)) = range.split_once('=') else {
        return Some(Err(()));
    };
    if !unit.eq_ignore_ascii_case("bytes") || spec.is_empty() || spec.contains(',') || size == 0 {
        return Some(Err(()));
    }

    let Some((start, end)) = spec.split_once('-') else {
        return Some(Err(()));
    };
    if start.is_empty() {
        let Some(suffix) = parse_decimal(end) else {
            return Some(Err(()));
        };
        if suffix == 0 {
            return Some(Err(()));
        }
        return Some(Ok((size.saturating_sub(suffix), size - 1)));
    }

    let Some(start) = parse_decimal(start) else {
        return Some(Err(()));
    };
    if start >= size {
        return Some(Err(()));
    }

    let end = if end.is_empty() {
        size - 1
    } else {
        let Some(end) = parse_decimal(end) else {
            return Some(Err(()));
        };
        end.min(size - 1)
    };

    if end < start {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

fn parse_decimal(value: &str) -> Option<u64> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse::<u64>().ok())
        .flatten()
}

pub(crate) fn if_none_match_matches(value: Option<&str>, current_etag: &str) -> bool {
    let Some(value) = value else {
        return false;
    };
    if value.split(',').any(|candidate| candidate.trim() == "*") {
        return true;
    }
    if current_etag.is_empty() {
        return false;
    }
    let current = strip_weak_validator(current_etag.trim());
    value.split(',').any(|candidate| {
        let candidate = candidate.trim();
        strip_weak_validator(candidate) == current
    })
}

pub(crate) fn if_range_allows_range(value: Option<&str>, current_etag: &str) -> bool {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    !current_etag.is_empty()
        && !is_weak_validator(value)
        && !is_weak_validator(current_etag.trim())
        && value == current_etag.trim()
}

fn strip_weak_validator(value: &str) -> &str {
    if is_weak_validator(value) {
        value.get(2..).unwrap_or_default().trim_start()
    } else {
        value
    }
}

fn is_weak_validator(value: &str) -> bool {
    value
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("W/"))
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

pub(crate) fn immutable_metadata_identity(
    resource: &str,
    data_reference: &[u8],
    etag: &str,
) -> String {
    if data_reference.len() == 32 || data_reference.len() == 64 {
        encode_hex(data_reference)
    } else if !etag.is_empty() {
        etag.to_string()
    } else {
        resource.to_string()
    }
}

pub(crate) fn window_key(identity: &str, size: u64, start: u64, end: u64) -> String {
    format!("{}|{}|{}-{}", identity, size, start, end)
}

pub(crate) fn window_prefix(identity: &str, size: u64) -> String {
    format!("{}|{}|", identity, size)
}

pub(crate) const MIB_BYTES: u64 = 1024 * 1024;

pub(crate) const MEDIA_STORAGE_WINDOW_BYTES: u64 = MIB_BYTES / 2;
pub(crate) const MEDIA_STARTUP_RESPONSE_BYTES: u64 = 8 * MIB_BYTES;
pub(crate) const MEDIA_PREFETCH_ACTIVE_HEADROOM_BYTES: u64 =
    MEDIA_STARTUP_RESPONSE_BYTES + 3 * MEDIA_STORAGE_WINDOW_BYTES;
pub(crate) const MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES: u64 = 96 * MIB_BYTES;
pub(crate) const MEDIA_PREFETCH_MAX_PARALLEL: usize = 4;
pub(crate) const MEDIA_PREFETCH_BATCH_YIELD_MS: u64 = 25;
pub(crate) const MEDIA_PREFETCH_STAGE_BYTES: [u64; 10] = [
    4 * MIB_BYTES,
    4 * MIB_BYTES,
    4 * MIB_BYTES,
    4 * MIB_BYTES,
    8 * MIB_BYTES,
    8 * MIB_BYTES,
    8 * MIB_BYTES,
    8 * MIB_BYTES,
    16 * MIB_BYTES,
    32 * MIB_BYTES,
];

pub(crate) const MEDIA_CACHE_HEAP_RATIO: f64 = 0.20;
pub(crate) const MEDIA_CACHE_DEVICE_MEMORY_RATIO: f64 = 0.10;
pub(crate) const MEDIA_CACHE_FALLBACK_BYTES: u64 = 96 * MIB_BYTES;
pub(crate) const MEDIA_CACHE_MIN_BYTES: u64 = 32 * MIB_BYTES;
pub(crate) const MEDIA_CACHE_HARD_MAX_BYTES: u64 = 128 * MIB_BYTES;

/// Derive the shared completed-media cache budget from browser memory signals.
///
/// `js_heap_size_limit_bytes` has priority because it describes the actual
/// JavaScript heap ceiling. `device_memory_gib` is the coarse Navigator value
/// and is used only when the heap signal is absent or invalid. Invalid signals
/// are ignored. The 96 MiB fallback is paired with the player's shorter
/// back-buffer so unknown-memory browsers can still sustain a 90-second lead
/// without increasing the combined media-cache target materially.
pub(crate) fn media_cache_budget_bytes(
    js_heap_size_limit_bytes: Option<f64>,
    device_memory_gib: Option<f64>,
) -> u64 {
    if let Some(heap_limit) = positive_finite(js_heap_size_limit_bytes) {
        return clamp_cache_budget(heap_limit * MEDIA_CACHE_HEAP_RATIO);
    }

    if let Some(device_memory) = positive_finite(device_memory_gib) {
        let device_bytes = device_memory * 1024.0 * MIB_BYTES as f64;
        return clamp_cache_budget(device_bytes * MEDIA_CACHE_DEVICE_MEMORY_RATIO);
    }

    MEDIA_CACHE_FALLBACK_BYTES
}

pub(crate) fn media_prefetch_ahead_limit_bytes(cache_budget_bytes: u64) -> u64 {
    cache_budget_bytes
        .saturating_sub(MEDIA_PREFETCH_ACTIVE_HEADROOM_BYTES)
        .min(MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES)
}

pub(crate) fn media_prefetch_stage_targets(ahead_limit_bytes: u64) -> Vec<u64> {
    let limit = ahead_limit_bytes.min(MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES);
    let mut targets = Vec::with_capacity(MEDIA_PREFETCH_STAGE_BYTES.len());
    let mut target = 0_u64;

    for stage_bytes in MEDIA_PREFETCH_STAGE_BYTES {
        if target >= limit {
            break;
        }
        target = target.saturating_add(stage_bytes).min(limit);
        targets.push(target);
    }

    targets
}

fn positive_finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

fn clamp_cache_budget(candidate_bytes: f64) -> u64 {
    candidate_bytes
        .max(MEDIA_CACHE_MIN_BYTES as f64)
        .min(MEDIA_CACHE_HARD_MAX_BYTES as f64) as u64
}
