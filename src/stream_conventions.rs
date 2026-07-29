use std::cell::RefCell;

const DEFAULT_ROUTE_BASE: &str = "/weeb-3";
const DEFAULT_SERVICE_WORKER_URL: &str = "/weeb-3/service.js";

#[derive(Clone, Debug, Eq, PartialEq)]
struct StreamingRouteConfig {
    route_base: String,
    service_worker_url: String,
}

impl Default for StreamingRouteConfig {
    fn default() -> Self {
        Self {
            route_base: DEFAULT_ROUTE_BASE.to_string(),
            service_worker_url: DEFAULT_SERVICE_WORKER_URL.to_string(),
        }
    }
}

thread_local! {
    static STREAMING_ROUTE_CONFIG: RefCell<StreamingRouteConfig> =
        RefCell::new(StreamingRouteConfig::default());
}

pub(crate) fn configure_streaming_routes(
    service_worker_url: &str,
    route_base: &str,
) -> Result<(), String> {
    let service_worker_url = service_worker_url.trim();
    if service_worker_url.is_empty()
        || service_worker_url
            .chars()
            .any(|character| character.is_control())
    {
        return Err(
            "service worker URL must be non-empty and contain no control characters".into(),
        );
    }
    let route_base = normalize_route_base(route_base)?;

    STREAMING_ROUTE_CONFIG.with(|config| {
        *config.borrow_mut() = StreamingRouteConfig {
            route_base,
            service_worker_url: service_worker_url.to_string(),
        };
    });
    Ok(())
}

pub(crate) fn streaming_route_base() -> String {
    STREAMING_ROUTE_CONFIG.with(|config| config.borrow().route_base.clone())
}

pub(crate) fn streaming_service_worker_url() -> String {
    STREAMING_ROUTE_CONFIG.with(|config| config.borrow().service_worker_url.clone())
}

pub(crate) fn streaming_service_worker_scope() -> String {
    let base = streaming_route_base();
    if base.is_empty() {
        "/".to_string()
    } else {
        format!("{base}/")
    }
}

pub(crate) fn streaming_route_path(suffix: &str) -> String {
    let base = streaming_route_base();
    let suffix = suffix.trim_start_matches('/');
    if suffix.is_empty() {
        if base.is_empty() {
            "/".to_string()
        } else {
            base
        }
    } else {
        format!("{base}/{suffix}")
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

pub(crate) fn route_base_controls_path(route_base: &str, pathname: &str) -> bool {
    let Ok(base) = normalize_route_base(route_base) else {
        return false;
    };
    base.is_empty() || pathname.starts_with(&format!("{base}/"))
}

pub(crate) fn normalize_route_base(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value == "/" {
        return Ok(String::new());
    }
    if !value.starts_with('/') {
        return Err("streaming route base must be an absolute same-origin path".into());
    }
    if value.contains(['?', '#', '\\']) || value.chars().any(|character| character.is_control()) {
        return Err("streaming route base contains an invalid path character".into());
    }

    let normalized = value.trim_end_matches('/');
    if normalized.split('/').skip(1).any(|component| {
        component.is_empty()
            || component == "."
            || component == ".."
            || component.eq_ignore_ascii_case("%2e")
            || component.eq_ignore_ascii_case("%2e%2e")
            || component.eq_ignore_ascii_case(".%2e")
            || component.eq_ignore_ascii_case("%2e.")
    }) {
        return Err("streaming route base must not contain empty or dot path segments".into());
    }
    Ok(normalized.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn is_direct_share_route(path: &str) -> bool {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let Some(route) = path.strip_prefix("/weeb-3/") else {
        return false;
    };
    if route.ends_with('/') {
        return false;
    }
    let mut parts = route.split('/');

    match parts.next() {
        Some("stream") => {}
        Some("testnet") if parts.next() == Some("stream") => {}
        _ => return false,
    }

    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(topic) = parts.next() else {
        return false;
    };
    let index = parts.next();
    parts.next().is_none()
        && is_feed_owner(owner)
        && is_stream_topic(topic)
        && index.is_none_or(|index| index.parse::<u64>().is_ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn is_stream_topic(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            let Some(high) = bytes
                .get(cursor + 1)
                .and_then(|byte| navigation_hex_value(*byte))
            else {
                return false;
            };
            let Some(low) = bytes
                .get(cursor + 2)
                .and_then(|byte| navigation_hex_value(*byte))
            else {
                return false;
            };
            decoded.push((high << 4) | low);
            cursor += 3;
        } else {
            decoded.push(bytes[cursor]);
            cursor += 1;
        }
    }
    let Ok(topic) = std::str::from_utf8(&decoded) else {
        return false;
    };
    !topic.is_empty()
        && topic.len() <= 256
        && !topic.chars().any(char::is_control)
        && !matches!(topic, "." | "..")
}

#[cfg(not(target_arch = "wasm32"))]
fn navigation_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn is_feed_owner(value: &str) -> bool {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn is_document_navigation(
    method: &str,
    fetch_mode: Option<&str>,
    fetch_destination: Option<&str>,
    accept: Option<&str>,
) -> bool {
    if !method.eq_ignore_ascii_case("GET") {
        return false;
    }

    if fetch_destination.is_some_and(|destination| {
        destination.eq_ignore_ascii_case("frame") || destination.eq_ignore_ascii_case("iframe")
    }) {
        return false;
    }

    if let Some(mode) = fetch_mode {
        return mode.eq_ignore_ascii_case("navigate");
    }
    if let Some(destination) = fetch_destination {
        return destination.eq_ignore_ascii_case("document");
    }

    accept.is_some_and(accepts_html)
}

#[cfg(not(target_arch = "wasm32"))]
fn accepts_html(value: &str) -> bool {
    value.split(',').any(|entry| {
        matches!(
            entry
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "text/html" | "application/xhtml+xml"
        )
    })
}

/// Parse one HTTP byte range against a known representation size.
///
/// `None` means that no Range header was supplied. A supplied but malformed,
/// multipart, empty, or unsatisfiable range is always `Some(Err(()))`; callers
/// must not silently turn it into a full-body response.
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

/// Apply the weak entity-tag comparison required by `If-None-Match` on GET
/// and HEAD requests.
///
/// The representations emitted by weeb-3 use simple quoted entity tags, so a
/// comma-separated validator list is sufficient here. An opaque tag containing
/// a comma is not produced by this server.
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

/// Return whether an optional `If-Range` validator permits a partial response.
///
/// Entity tags use strong comparison. HTTP-date validators conservatively
/// fall back to a full representation because Swarm metadata does not expose a
/// trustworthy last-modified time.
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

#[cfg(test)]
mod range_cache_identity_tests {
    use super::{immutable_metadata_identity, window_key, window_prefix};

    #[test]
    fn content_reference_is_authoritative_over_etag() {
        let first = immutable_metadata_identity("resource", &[0x11; 32], "same-etag");
        let second = immutable_metadata_identity("resource", &[0x22; 32], "same-etag");
        assert_ne!(first, second);
        assert_eq!(first, "11".repeat(32));
    }

    #[test]
    fn invalid_reference_uses_safe_fallbacks() {
        assert_eq!(
            immutable_metadata_identity("resource", &[0x11; 31], "etag"),
            "etag"
        );
        assert_eq!(immutable_metadata_identity("resource", &[], ""), "resource");
    }

    #[test]
    fn window_keys_separate_size_and_bounds() {
        let identity = immutable_metadata_identity("resource", &[0x33; 64], "etag");
        assert_ne!(
            window_key(&identity, 10, 0, 4),
            window_key(&identity, 11, 0, 4)
        );
        assert_ne!(
            window_key(&identity, 10, 0, 4),
            window_key(&identity, 10, 5, 9)
        );
        assert!(window_key(&identity, 10, 0, 4).starts_with(&window_prefix(&identity, 10)));
    }
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

/// Return the byte distance speculative media may advance beyond the response
/// that triggered it.
///
/// The foreground startup response and three storage-window edge allowances
/// remain resident. Lookahead is then capped at 96 MiB even on large heaps.
pub(crate) fn media_prefetch_ahead_limit_bytes(cache_budget_bytes: u64) -> u64 {
    cache_budget_bytes
        .saturating_sub(MEDIA_PREFETCH_ACTIVE_HEADROOM_BYTES)
        .min(MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES)
}

/// Return cumulative byte targets for the shared staged prefetch policy.
///
/// Targets are relative to the end of the foreground response. The caller can
/// pass a smaller limit for end-of-file/end-of-playlist clipping. A clipped
/// final target appears only once.
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

/// One bounded batch of indivisible prefetch units.
///
/// A unit is a fixed byte-storage window or another indivisible media object.
/// `already_planned_bytes`, `stage_target_bytes`, and
/// `hard_limit_bytes` are all measured from the end of the triggering
/// foreground response. The stage target may be exceeded by the final
/// indivisible unit, but the hard limit is never exceeded. Units are kept in
/// order; an invalid or over-budget unit stops the batch rather than skipping
/// ahead in the media.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MediaPrefetchBatch {
    pub(crate) unit_count: usize,
    pub(crate) additional_bytes: u64,
    pub(crate) planned_end_bytes: u64,
}

pub(crate) fn plan_media_prefetch_batch(
    already_planned_bytes: u64,
    stage_target_bytes: u64,
    hard_limit_bytes: u64,
    ordered_unit_sizes: &[u64],
) -> MediaPrefetchBatch {
    let hard_limit = hard_limit_bytes.min(MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES);
    let stage_target = stage_target_bytes.min(hard_limit);
    if already_planned_bytes >= stage_target || already_planned_bytes >= hard_limit {
        return MediaPrefetchBatch {
            planned_end_bytes: already_planned_bytes,
            ..MediaPrefetchBatch::default()
        };
    }

    let mut planned_end = already_planned_bytes;
    let mut unit_count = 0;

    for &unit_size in ordered_unit_sizes.iter().take(MEDIA_PREFETCH_MAX_PARALLEL) {
        if unit_size == 0 {
            break;
        }
        let Some(next_end) = planned_end.checked_add(unit_size) else {
            break;
        };
        if next_end > hard_limit {
            break;
        }

        planned_end = next_end;
        unit_count += 1;
        if planned_end >= stage_target {
            break;
        }
    }

    MediaPrefetchBatch {
        unit_count,
        additional_bytes: planned_end.saturating_sub(already_planned_bytes),
        planned_end_bytes: planned_end,
    }
}

fn positive_finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

fn clamp_cache_budget(candidate_bytes: f64) -> u64 {
    candidate_bytes
        .max(MEDIA_CACHE_MIN_BYTES as f64)
        .min(MEDIA_CACHE_HARD_MAX_BYTES as f64) as u64
}
