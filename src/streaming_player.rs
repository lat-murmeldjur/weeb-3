use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, VecDeque},
    time::Duration,
};

use async_std::sync::Arc;
use js_sys::{Array, Function, Object, Reflect};
use libp2p::futures::future::join_all;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlElement, MessageEvent};

use crate::{
    Weeb3,
    bzz_stream::{BzzMetadata, bzz_reference_hex, normalize_bzz_path},
    interface::service_worker_controls_bzz_requests,
    mpsc,
    network_profile::{NetworkMode, active_profile},
};

const BZZ_MARKERS: [&str; 3] = [
    "/weeb-3/bzz/",
    "/weeb-3/mainnet/bzz/",
    "/weeb-3/testnet/bzz/",
];
const RAW_ROUTE_MARKERS: [(&str, &str); 9] = [
    ("/weeb-3/bytes/", "bytes"),
    ("/weeb-3/chunks/", "chunk"),
    ("/weeb-3/chunk/", "chunk"),
    ("/weeb-3/mainnet/bytes/", "bytes"),
    ("/weeb-3/mainnet/chunks/", "chunk"),
    ("/weeb-3/mainnet/chunk/", "chunk"),
    ("/weeb-3/testnet/bytes/", "bytes"),
    ("/weeb-3/testnet/chunks/", "chunk"),
    ("/weeb-3/testnet/chunk/", "chunk"),
];
const MIB_BYTES: u64 = 1024 * 1024;
const STREAM_STORAGE_WINDOW_BYTES: u64 = MIB_BYTES / 2;
const STREAM_RESPONSE_BUFFER_BYTES: u64 = 8 * MIB_BYTES;
const STREAM_ACTIVE_RESPONSE_BUFFER_BYTES: u64 = 2 * MIB_BYTES;
const STREAM_PREFETCH_AHEAD_LIMIT_BYTES: u64 = 64 * MIB_BYTES;
const STREAM_SEEK_KEEP_AHEAD_BYTES: u64 = 16 * MIB_BYTES;
const STREAM_SEEK_RESET_GAP_BYTES: u64 = STREAM_SEEK_KEEP_AHEAD_BYTES;
const STREAM_SEEK_REQUEST_GAP_BYTES: u64 = 6 * MIB_BYTES;
const STREAM_PREFETCH_MAX_WINDOWS: usize = 8;
const RANGE_CACHE_MEMORY_RATIO: f64 = 0.75;
const RANGE_CACHE_DEVICE_MEMORY_RATIO: f64 = 0.45;
const RANGE_CACHE_FALLBACK_MAX_BYTES: u64 = 3 * 1024 * MIB_BYTES;
const RANGE_CACHE_MIN_MAX_BYTES: u64 = 512 * MIB_BYTES;
const METADATA_CACHE_MAX_ENTRIES: usize = 1024;
const MEDIA_STREAM_STATE_MAX_ENTRIES: usize = 64;
const RANGE_CACHE_STALE_MS: f64 = 270_000.0;
const RANGE_RETRY_COUNT: usize = 4;
const RANGE_RETRY_DELAY_MS: u64 = 700;
const RANGE_REQUEST_TIMEOUT_MS: u64 = 210_000;
const STREAM_RANGE_RETRY_COUNT: usize = 1;
const STREAM_RANGE_REQUEST_TIMEOUT_MS: u64 = 15_000;
const STREAM_PREFETCH_BATCH_YIELD_MS: u64 = 25;
const MEDIA_RETRY_DELAYS_MS: [u64; 6] = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
const STREAM_PREFETCH_STAGE_BYTES: [u64; 9] = [
    4 * MIB_BYTES,
    4 * MIB_BYTES,
    4 * MIB_BYTES,
    4 * MIB_BYTES,
    8 * MIB_BYTES,
    8 * MIB_BYTES,
    8 * MIB_BYTES,
    8 * MIB_BYTES,
    16 * MIB_BYTES,
];

thread_local! {
    static FETCH_CACHE: RefCell<FetchCache> = RefCell::new(FetchCache::new());
}

fn reset_bzz_fetch_resource_activity(resource: &str, reason: &str) -> bool {
    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let mut reset = false;

        if let Some(metadata) = cache.metadata.get(resource).cloned() {
            cache.fail_pending_ranges_with_prefix(&range_cache_prefix(resource, &metadata), reason);
            cache.reset_media_state(&media_state_key(resource, &metadata));
            reset = true;
        }

        let suffix = format!("|{}", resource);
        let media_keys: Vec<String> = cache
            .media_states
            .keys()
            .filter(|key| key.ends_with(&suffix))
            .cloned()
            .collect();
        for key in media_keys {
            cache.reset_media_state(&key);
            reset = true;
        }

        reset
    })
}

fn reset_bzz_fetch_url_activity(url: &str, reason: &str) -> bool {
    canonical_bzz_resource(url)
        .map(|resource| reset_bzz_fetch_resource_activity(&resource, reason))
        .unwrap_or(false)
}

struct FetchCache {
    metadata_order: VecDeque<String>,
    metadata: HashMap<String, BzzMetadata>,
    range_order: VecDeque<String>,
    ranges: HashMap<String, CachedRange>,
    pending_ranges: HashMap<String, PendingRange>,
    range_bytes: u64,
    media_states: HashMap<String, MediaState>,
}

impl FetchCache {
    fn new() -> Self {
        Self {
            metadata_order: VecDeque::new(),
            metadata: HashMap::new(),
            range_order: VecDeque::new(),
            ranges: HashMap::new(),
            pending_ranges: HashMap::new(),
            range_bytes: 0,
            media_states: HashMap::new(),
        }
    }

    fn metadata(&mut self, resource: &str) -> Option<BzzMetadata> {
        let metadata = self.metadata.get(resource).cloned()?;
        self.metadata_order.retain(|key| key != resource);
        self.metadata_order.push_back(resource.to_string());
        Some(metadata)
    }

    fn remember_metadata(&mut self, resource: String, metadata: BzzMetadata) {
        self.metadata_order.retain(|key| key != &resource);
        self.metadata_order.push_back(resource.clone());
        self.metadata.insert(resource, metadata);
        while self.metadata.len() > METADATA_CACHE_MAX_ENTRIES {
            let Some(oldest) = self.metadata_order.pop_front() else {
                break;
            };
            self.metadata.remove(&oldest);
        }
    }

    fn range(&mut self, key: &str, generation: u64) -> Option<Vec<u8>> {
        let range = self.ranges.get(key)?;
        if range.generation != 0 && generation != 0 && range.generation != generation {
            return None;
        }
        let body = range.body.clone();
        self.range_order.retain(|cached_key| cached_key != key);
        self.range_order.push_back(key.to_string());
        Some(body)
    }

    fn remember_range(&mut self, key: String, body: Vec<u8>, generation: u64) {
        let body_len = body.len() as u64;
        if let Some(old) = self.ranges.remove(&key) {
            self.range_bytes = self.range_bytes.saturating_sub(old.body.len() as u64);
        }
        self.range_order.retain(|cached_key| cached_key != &key);
        self.range_order.push_back(key.clone());
        self.ranges.insert(key, CachedRange { body, generation });
        self.range_bytes = self.range_bytes.saturating_add(body_len);
        self.trim_ranges();
    }

    fn range_load_role(&mut self, key: &str, generation: u64) -> RangeLoadRole {
        if let Some(body) = self.range(key, generation) {
            return RangeLoadRole::Cached(body);
        }

        if let Some(pending) = self.pending_ranges.get_mut(key) {
            if js_sys::Date::now() - pending.created_at > RANGE_CACHE_STALE_MS {
                if let Some(stale) = self.pending_ranges.remove(key) {
                    stale.finish(Err("stale range request expired".to_string()));
                }
            } else if pending.generation == generation || generation == 0 {
                let (sender, receiver) = mpsc::bounded(1);
                pending.waiters.push(sender);
                return RangeLoadRole::Wait(receiver);
            } else if let Some(stale) = self.pending_ranges.remove(key) {
                stale.finish(Err("stale range generation replaced".to_string()));
            }
        }

        self.pending_ranges.insert(
            key.to_string(),
            PendingRange {
                generation,
                created_at: js_sys::Date::now(),
                waiters: Vec::new(),
            },
        );
        RangeLoadRole::Lead
    }

    fn finish_pending_range(&mut self, key: &str, result: Result<Vec<u8>, String>) {
        if let Some(pending) = self.pending_ranges.remove(key) {
            pending.finish(result);
        }
    }

    fn fail_pending_ranges_with_prefix(&mut self, prefix: &str, reason: &str) {
        let keys: Vec<String> = self
            .pending_ranges
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(pending) = self.pending_ranges.remove(&key) {
                pending.finish(Err(reason.to_string()));
            }
        }
    }

    fn reset_media_state(&mut self, key: &str) {
        if let Some(state) = self.media_states.get_mut(key) {
            state.generation = state.generation.saturating_add(1);
            state.anchor_start = None;
            state.high_water_end = -1;
            state.scheduled_high_water_end = -1;
            state.completed_ranges.clear();
            state.consecutive_failures = 0;
            state.last_range_was_seek = false;
            state.last_range_was_startup = false;
            state.prefetch_running = false;
            state.prefetch_generation = 0;
            state.last_touch = js_sys::Date::now();
        }
    }

    fn trim_ranges(&mut self) {
        let max_bytes = range_cache_max_bytes();
        while self.range_bytes > max_bytes {
            let Some(oldest) = self.range_order.pop_front() else {
                break;
            };
            if let Some(range) = self.ranges.remove(&oldest) {
                self.range_bytes = self.range_bytes.saturating_sub(range.body.len() as u64);
            }
        }
    }

    fn media_state_mut(&mut self, key: &str) -> &mut MediaState {
        if !self.media_states.contains_key(key) {
            self.media_states.insert(key.to_string(), MediaState::new());
        }
        self.trim_media_states(key);
        self.media_states
            .get_mut(key)
            .expect("media state inserted above")
    }

    fn trim_media_states(&mut self, active_key: &str) {
        if self.media_states.len() <= MEDIA_STREAM_STATE_MAX_ENTRIES {
            return;
        }

        let mut candidates: Vec<(String, f64)> = self
            .media_states
            .iter()
            .filter(|(key, state)| key.as_str() != active_key && !state.prefetch_running)
            .map(|(key, state)| (key.clone(), state.last_touch))
            .collect();
        candidates.sort_by(|left, right| {
            left.1
                .partial_cmp(&right.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for (key, _) in candidates {
            if self.media_states.len() <= MEDIA_STREAM_STATE_MAX_ENTRIES {
                break;
            }
            self.media_states.remove(&key);
        }
    }
}

struct CachedRange {
    body: Vec<u8>,
    generation: u64,
}

struct PendingRange {
    generation: u64,
    created_at: f64,
    waiters: Vec<mpsc::Sender<Result<Vec<u8>, String>>>,
}

impl PendingRange {
    fn finish(self, result: Result<Vec<u8>, String>) {
        for waiter in self.waiters {
            let _ = waiter.try_send(result.clone());
        }
    }
}

enum RangeLoadRole {
    Cached(Vec<u8>),
    Wait(mpsc::Receiver<Result<Vec<u8>, String>>),
    Lead,
}

fn range_cache_max_bytes() -> u64 {
    let global = js_sys::global();
    if let Ok(performance) = Reflect::get(&global, &"performance".into()) {
        if let Ok(memory) = Reflect::get(&performance, &"memory".into()) {
            if let Ok(limit) = Reflect::get(&memory, &"jsHeapSizeLimit".into()) {
                if let Some(limit) = limit
                    .as_f64()
                    .filter(|limit| limit.is_finite() && *limit > 0.0)
                {
                    return RANGE_CACHE_MIN_MAX_BYTES
                        .max((limit * RANGE_CACHE_MEMORY_RATIO) as u64);
                }
            }
        }
    }

    if let Some(window) = web_sys::window() {
        let navigator = window.navigator();
        if let Ok(device_memory) = Reflect::get(navigator.as_ref(), &"deviceMemory".into())
            .and_then(|value| {
                value
                    .as_f64()
                    .filter(|memory| memory.is_finite() && *memory > 0.0)
                    .ok_or(JsValue::NULL)
            })
        {
            return RANGE_CACHE_MIN_MAX_BYTES.max(
                (device_memory * 1024.0 * MIB_BYTES as f64 * RANGE_CACHE_DEVICE_MEMORY_RATIO)
                    as u64,
            );
        }
    }

    RANGE_CACHE_FALLBACK_MAX_BYTES
}

#[derive(Clone)]
struct MediaRangeState {
    generation: u64,
    last_range_was_startup: bool,
}

struct MediaState {
    generation: u64,
    anchor_start: Option<u64>,
    high_water_end: i64,
    scheduled_high_water_end: i64,
    completed_ranges: BTreeMap<u64, u64>,
    consecutive_failures: u32,
    last_request_start: u64,
    last_range_was_seek: bool,
    last_range_was_startup: bool,
    prefetch_running: bool,
    prefetch_generation: u64,
    last_touch: f64,
}

impl MediaState {
    fn new() -> Self {
        Self {
            generation: 1,
            anchor_start: None,
            high_water_end: -1,
            scheduled_high_water_end: -1,
            completed_ranges: BTreeMap::new(),
            consecutive_failures: 0,
            last_request_start: 0,
            last_range_was_seek: false,
            last_range_was_startup: false,
            prefetch_running: false,
            prefetch_generation: 0,
            last_touch: js_sys::Date::now(),
        }
    }

    fn effective_high_water_end(&self) -> i64 {
        self.high_water_end.max(self.scheduled_high_water_end)
    }

    fn mark_scheduled(&mut self, end: u64) {
        self.scheduled_high_water_end = self.scheduled_high_water_end.max(end as i64);
        self.last_touch = js_sys::Date::now();
    }

    fn mark_complete(&mut self, start: u64, end: u64) {
        self.completed_ranges.insert(start, end);
        while let Some((&range_start, &range_end)) = self.completed_ranges.iter().next() {
            if range_start <= (self.high_water_end + 1).max(0) as u64 {
                self.high_water_end = self.high_water_end.max(range_end as i64);
                self.completed_ranges.remove(&range_start);
            } else {
                break;
            }
        }
        self.consecutive_failures = 0;
        self.last_touch = js_sys::Date::now();
    }

    fn mark_failure(&mut self, start: u64) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let failure_end = if start == 0 { -1 } else { start as i64 - 1 };
        self.scheduled_high_water_end = self.scheduled_high_water_end.min(failure_end);
        self.scheduled_high_water_end = self.scheduled_high_water_end.max(self.high_water_end);
        self.last_touch = js_sys::Date::now();
    }
}

pub fn handle_service_worker_message(
    obj: &js_sys::Object,
    event: &MessageEvent,
    weeb3: Arc<Weeb3>,
) -> bool {
    let ty = Reflect::get(obj, &JsValue::from_str("type")).unwrap_or(JsValue::NULL);

    if ty == JsValue::from_str("WEEB3_FETCH_REQUEST") {
        handle_fetch_request_message(obj, event, weeb3);
        return true;
    }

    if ty == JsValue::from_str("RESOLVE_BZZ_REQUEST") {
        handle_resolve_bzz_message(obj, event, weeb3);
        return true;
    }

    if ty == JsValue::from_str("RETRIEVE_RANGE_REQUEST") {
        handle_retrieve_range_message(obj, event, weeb3);
        return true;
    }

    if ty == JsValue::from_str("PREPARE_BZZ_STREAM_REQUEST") {
        handle_prepare_bzz_stream_message(obj, event, weeb3);
        return true;
    }

    false
}

struct FetchResponse {
    ok: bool,
    status: u16,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    error: String,
    stream: bool,
}

impl FetchResponse {
    fn ok(status: u16, headers: Vec<(String, String)>, body: Option<Vec<u8>>) -> Self {
        Self {
            ok: true,
            status,
            headers,
            body,
            error: String::new(),
            stream: false,
        }
    }

    fn stream(status: u16, headers: Vec<(String, String)>) -> Self {
        Self {
            ok: true,
            status,
            headers,
            body: None,
            error: String::new(),
            stream: true,
        }
    }

    fn error(status: u16, error: impl Into<String>) -> Self {
        Self {
            ok: false,
            status,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: None,
            error: error.into(),
            stream: false,
        }
    }

    fn into_js(self) -> Object {
        let resp = Object::new();
        set_js(&resp, "ok", JsValue::from_bool(self.ok));
        set_js(&resp, "status", JsValue::from_f64(self.status as f64));
        set_js(&resp, "error", JsValue::from_str(&self.error));
        set_js(&resp, "stream", JsValue::from_bool(self.stream));

        let headers = Array::new();
        for (name, value) in self.headers {
            let pair = Array::new();
            pair.push(&name.into());
            pair.push(&value.into());
            headers.push(&pair);
        }
        set_js(&resp, "headers", headers.into());

        if let Some(body) = self.body {
            let u8arr = js_sys::Uint8Array::new_with_length(body.len() as u32);
            u8arr.copy_from(&body);
            set_js(&resp, "body", u8arr.into());
        }

        resp
    }
}

fn set_js(target: &Object, name: &str, value: JsValue) {
    let _ = Reflect::set(target, &JsValue::from_str(name), &value);
}

fn handle_fetch_request_message(obj: &js_sys::Object, event: &MessageEvent, weeb3: Arc<Weeb3>) {
    let url = js_string_property(obj, "url").unwrap_or_default();
    let method = js_string_property(obj, "method")
        .unwrap_or_else(|| "GET".into())
        .to_uppercase();
    let range = js_string_property(obj, "range").filter(|range| !range.is_empty());
    let port = message_port(event);

    spawn_local(async move {
        let subject = fetch_log_subject(&url);
        weeb3.interface_log(format!("service fetch {} {}", method, subject));
        let resp = fetch_request_response(weeb3.clone(), url, method, range).await;
        let status = resp.status;
        let ok = resp.ok;
        let error = resp.error.clone();
        weeb3.interface_log(if ok {
            format!("service fetch complete {} {}", status, subject)
        } else {
            format!("service fetch failed {} {} {}", status, subject, error)
        });
        let resp = resp.into_js();

        if let Some(port) = port {
            let _ = port.post_message(&resp);
        }
    });
}

fn fetch_log_subject(url: &str) -> String {
    let pathname = match web_sys::Url::new(url) {
        Ok(url) => url.pathname(),
        Err(_) => url.to_string(),
    };

    if let Some(resource) = canonical_bzz_resource(&pathname) {
        return format!("bzz/{}", resource);
    }

    if let Some((raw_type, reference)) = canonical_raw_resource(&pathname) {
        return format!("{}/{}", raw_type, reference);
    }

    pathname
}

async fn fetch_request_response(
    weeb3: Arc<Weeb3>,
    url: String,
    method: String,
    range: Option<String>,
) -> FetchResponse {
    if method != "GET" && method != "HEAD" {
        return FetchResponse::error(405, "method not allowed");
    }

    let pathname = match web_sys::Url::new(&url) {
        Ok(url) => url.pathname(),
        Err(_) => url,
    };

    if let Some(resource) = canonical_bzz_resource(&pathname) {
        return fetch_bzz_response(weeb3, resource, method, range).await;
    }

    if let Some((raw_type, reference)) = canonical_raw_resource(&pathname) {
        return fetch_raw_response(weeb3, raw_type, reference, method).await;
    }

    FetchResponse::error(404, "weeb-3 route not found")
}

async fn fetch_raw_response(
    weeb3: Arc<Weeb3>,
    raw_type: String,
    reference: String,
    method: String,
) -> FetchResponse {
    let parts: Vec<&str> = reference.split('/').collect();
    let reference = parts.first().copied().unwrap_or_default().to_string();
    if parts.iter().skip(1).any(|part| !part.is_empty()) {
        return FetchResponse::error(400, "raw route accepts one swarm reference");
    }
    if !is_swarm_reference(&reference) {
        return FetchResponse::error(400, "invalid swarm reference");
    }

    let mut headers = vec![
        (
            "Content-Type".to_string(),
            "application/octet-stream".to_string(),
        ),
        (
            "Content-Disposition".to_string(),
            format!("attachment; filename=\"{}\"", reference),
        ),
        ("Cache-Control".to_string(), "no-store".to_string()),
    ];

    if method == "HEAD" {
        return FetchResponse::ok(200, headers, None);
    }

    let bytes = if raw_type == "chunk" {
        weeb3.retrieve_chunk_bytes(reference).await
    } else {
        weeb3.retrieve_bytes(reference).await
    };

    if bytes.is_empty() {
        return FetchResponse::error(404, "weeb-3 did not retrieve resource");
    }

    headers.push(("Content-Length".to_string(), bytes.len().to_string()));
    FetchResponse::ok(200, headers, Some(bytes))
}

async fn fetch_bzz_response(
    weeb3: Arc<Weeb3>,
    resource: String,
    method: String,
    range: Option<String>,
) -> FetchResponse {
    let Some(metadata) = resolve_bzz_cached(weeb3.clone(), resource.clone()).await else {
        return FetchResponse::error(404, "weeb-3 did not resolve resource");
    };

    if method == "HEAD" {
        return FetchResponse::ok(200, metadata_headers(&metadata, metadata.size), None);
    }

    if metadata.size == 0 {
        return FetchResponse::ok(200, metadata_headers(&metadata, 0), Some(vec![]));
    }

    let streamable = is_streamable_mime(&metadata.mime) && metadata.size > 0;
    let parsed_range = parse_single_range(range.as_deref(), metadata.size);
    if !streamable && parsed_range.is_none() {
        if should_inline_non_streamable_response(&metadata) {
            return full_bzz_response(weeb3, resource, metadata).await;
        }
        return FetchResponse::stream(200, metadata_headers(&metadata, metadata.size));
    }

    let (start, end, partial, media_state) = match parsed_range {
        Some(Err(_)) => {
            return FetchResponse::ok(
                416,
                vec![(
                    "Content-Range".to_string(),
                    format!("bytes */{}", metadata.size),
                )],
                None,
            );
        }
        Some(Ok((requested_start, requested_end))) => {
            let media_state = if streamable {
                Some(begin_media_range(&resource, &metadata, requested_start))
            } else {
                None
            };
            let (start, end) = response_range_for_request(
                requested_start,
                requested_end,
                &metadata,
                streamable,
                &media_state,
            );
            (start, end, true, media_state)
        }
        None if streamable => {
            let media_state = begin_media_range(&resource, &metadata, 0);
            let end = STREAM_RESPONSE_BUFFER_BYTES
                .saturating_sub(1)
                .min(metadata.size - 1);
            (0, end, true, Some(media_state))
        }
        None => (0, metadata.size - 1, false, None),
    };

    mark_range_windows_scheduled(&resource, &metadata, start, end, &media_state);
    let generation = media_state
        .as_ref()
        .map(|state| state.generation)
        .unwrap_or(0);

    let bytes = match read_cached_range_with_retry(
        weeb3.clone(),
        resource.clone(),
        metadata.clone(),
        start,
        end,
        generation,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(error) => {
            note_media_range_failure(&resource, &metadata, start, &media_state);
            return FetchResponse::error(503, error);
        }
    };

    if bytes.len() != (end - start + 1) as usize {
        note_media_range_failure(&resource, &metadata, start, &media_state);
        return FetchResponse::error(502, "weeb-3 returned a short range");
    }

    if let Some(media_state) = &media_state {
        mark_media_range_complete(&resource, &metadata, start, end, media_state);
        spawn_prefetch_media_stages(
            weeb3.clone(),
            resource.clone(),
            metadata.clone(),
            start,
            end,
            metadata.size - 1,
            media_state.generation,
        );
    }

    let mut headers = metadata_headers(&metadata, bytes.len() as u64);
    if partial {
        headers.push((
            "Content-Range".to_string(),
            format!("bytes {}-{}/{}", start, end, metadata.size),
        ));
        FetchResponse::ok(206, headers, Some(bytes))
    } else {
        FetchResponse::ok(200, headers, Some(bytes))
    }
}

async fn resolve_bzz_cached(weeb3: Arc<Weeb3>, resource: String) -> Option<BzzMetadata> {
    if let Some(metadata) = FETCH_CACHE.with(|cache| cache.borrow_mut().metadata(&resource)) {
        return Some(metadata);
    }

    let metadata = weeb3.resolve_bzz(resource.clone()).await?;
    FETCH_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .remember_metadata(resource, metadata.clone());
    });
    Some(metadata)
}

async fn full_bzz_response(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
) -> FetchResponse {
    let size = metadata.size;
    let bytes = match read_cached_range_with_retry(
        weeb3,
        resource.clone(),
        metadata.clone(),
        0,
        size - 1,
        0,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(error) => return FetchResponse::error(503, error),
    };

    if bytes.len() != size as usize {
        return FetchResponse::error(502, "weeb-3 returned a short body");
    }

    FetchResponse::ok(200, metadata_headers(&metadata, size), Some(bytes))
}

fn should_inline_non_streamable_response(metadata: &BzzMetadata) -> bool {
    let mime = metadata.mime.split(';').next().unwrap_or("").trim();
    mime == "text/html"
        || mime == "application/xhtml+xml"
        || metadata.size <= STREAM_STORAGE_WINDOW_BYTES
}

pub(crate) fn warm_bzz_fetch_cache(resource: &str, metadata: BzzMetadata, body: Vec<u8>) {
    if metadata.size == 0 || body.len() != metadata.size as usize {
        return;
    }

    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.remember_metadata(resource.to_string(), metadata.clone());

        for (start, end) in range_storage_windows_for_span(0, metadata.size - 1, metadata.size) {
            let local_start = start as usize;
            let local_end = end as usize + 1;
            if local_end > body.len() {
                return;
            }

            let key = range_cache_key(resource, &metadata, start, end);
            cache.remember_range(key, body[local_start..local_end].to_vec(), 0);
        }
    });
}

fn metadata_identity(resource: &str, metadata: &BzzMetadata) -> String {
    if !metadata.etag.is_empty() {
        metadata.etag.clone()
    } else if !metadata.data_reference.is_empty() {
        hex::encode(&metadata.data_reference)
    } else {
        resource.to_string()
    }
}

fn media_state_key(resource: &str, metadata: &BzzMetadata) -> String {
    format!("{}|{}", metadata_identity(resource, metadata), resource)
}

fn range_cache_key(resource: &str, metadata: &BzzMetadata, start: u64, end: u64) -> String {
    format!(
        "{}|{}|{}-{}",
        metadata_identity(resource, metadata),
        metadata.size,
        start,
        end
    )
}

fn range_cache_prefix(resource: &str, metadata: &BzzMetadata) -> String {
    format!(
        "{}|{}|",
        metadata_identity(resource, metadata),
        metadata.size
    )
}

fn range_storage_window_for_start(start: u64, size: u64) -> (u64, u64) {
    let storage_start = (start / STREAM_STORAGE_WINDOW_BYTES) * STREAM_STORAGE_WINDOW_BYTES;
    (
        storage_start,
        storage_start
            .saturating_add(STREAM_STORAGE_WINDOW_BYTES)
            .saturating_sub(1)
            .min(size.saturating_sub(1)),
    )
}

fn range_storage_windows_for_span(start: u64, end: u64, size: u64) -> Vec<(u64, u64)> {
    let mut windows = Vec::new();
    let mut position = start;

    while position <= end {
        let window = range_storage_window_for_start(position, size);
        windows.push(window);
        if window.1 == u64::MAX {
            break;
        }
        position = window.1.saturating_add(1);
    }

    windows
}

fn begin_media_range(resource: &str, metadata: &BzzMetadata, start: u64) -> MediaRangeState {
    let key = media_state_key(resource, metadata);

    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let state = cache.media_state_mut(&key);
        let previous_anchor = state.anchor_start;
        let previous_high_water = state.effective_high_water_end();
        let previous_request_start = state.last_request_start;
        let is_startup = previous_anchor.is_none();
        let is_request_jump = previous_anchor.is_some()
            && (start.saturating_add(STREAM_SEEK_REQUEST_GAP_BYTES) < previous_request_start
                || start > previous_request_start.saturating_add(STREAM_SEEK_REQUEST_GAP_BYTES));
        let is_seek = previous_anchor.is_some()
            && (is_request_jump
                || start.saturating_add(STREAM_SEEK_RESET_GAP_BYTES)
                    < previous_anchor.unwrap_or(0)
                || start as i64 > previous_high_water + STREAM_SEEK_RESET_GAP_BYTES as i64);
        let is_prefetch_runaway = previous_anchor.is_some()
            && previous_high_water
                > start
                    .saturating_add(STREAM_RESPONSE_BUFFER_BYTES)
                    .saturating_add(STREAM_PREFETCH_AHEAD_LIMIT_BYTES) as i64;

        if is_seek || is_prefetch_runaway {
            state.generation = state.generation.saturating_add(1);
            state.anchor_start = Some(start);
            state.high_water_end = start as i64 - 1;
            state.scheduled_high_water_end = start as i64 - 1;
            state.completed_ranges.clear();
            state.consecutive_failures = 0;
        } else if is_startup {
            state.anchor_start = Some(start);
        }

        state.last_request_start = start;
        state.last_range_was_seek = is_seek || is_prefetch_runaway;
        state.last_range_was_startup = is_startup;
        state.last_touch = js_sys::Date::now();

        MediaRangeState {
            generation: state.generation,
            last_range_was_startup: state.last_range_was_startup || state.last_range_was_seek,
        }
    })
}

fn response_range_for_request(
    requested_start: u64,
    requested_end: u64,
    metadata: &BzzMetadata,
    streamable: bool,
    media_state: &Option<MediaRangeState>,
) -> (u64, u64) {
    let mut response_bytes = STREAM_STORAGE_WINDOW_BYTES;
    if streamable {
        let startup_like = media_state
            .as_ref()
            .map(|state| state.last_range_was_startup)
            .unwrap_or(false);
        response_bytes = if startup_like {
            STREAM_RESPONSE_BUFFER_BYTES
        } else {
            STREAM_ACTIVE_RESPONSE_BUFFER_BYTES
        };
    }

    (
        requested_start,
        requested_start
            .saturating_add(response_bytes)
            .saturating_sub(1)
            .min(requested_end)
            .min(metadata.size.saturating_sub(1)),
    )
}

fn mark_range_windows_scheduled(
    resource: &str,
    metadata: &BzzMetadata,
    start: u64,
    end: u64,
    media_state: &Option<MediaRangeState>,
) {
    let Some(media_state) = media_state else {
        return;
    };

    let key = media_state_key(resource, metadata);
    let windows = range_storage_windows_for_span(start, end, metadata.size);
    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let state = cache.media_state_mut(&key);
        if state.generation != media_state.generation {
            return;
        }
        for (_, window_end) in windows {
            state.mark_scheduled(window_end);
        }
    });
}

fn mark_media_range_complete(
    resource: &str,
    metadata: &BzzMetadata,
    start: u64,
    end: u64,
    media_state: &MediaRangeState,
) {
    let key = media_state_key(resource, metadata);
    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let state = cache.media_state_mut(&key);
        if state.generation == media_state.generation {
            state.mark_complete(start, end);
        }
    });
}

fn note_media_range_failure(
    resource: &str,
    metadata: &BzzMetadata,
    start: u64,
    media_state: &Option<MediaRangeState>,
) {
    let Some(media_state) = media_state else {
        return;
    };
    let key = media_state_key(resource, metadata);
    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let should_reset = {
            let state = cache.media_state_mut(&key);
            if state.generation == media_state.generation {
                state.mark_failure(start);
                true
            } else {
                false
            }
        };
        if should_reset {
            cache.fail_pending_ranges_with_prefix(
                &range_cache_prefix(resource, metadata),
                "media range failed",
            );
            cache.reset_media_state(&key);
        }
    });
}

async fn read_cached_range_with_retry(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
    start: u64,
    end: u64,
    generation: u64,
) -> Result<Vec<u8>, String> {
    let mut last_error = "range retry did not run".to_string();
    let retry_count = if generation > 0 {
        STREAM_RANGE_RETRY_COUNT
    } else {
        RANGE_RETRY_COUNT
    };

    for attempt in 0..=retry_count {
        match read_cached_range(
            weeb3.clone(),
            resource.clone(),
            metadata.clone(),
            start,
            end,
            generation,
        )
        .await
        {
            Ok(bytes) if bytes.len() == (end - start + 1) as usize => return Ok(bytes),
            Ok(bytes) => {
                last_error = format!(
                    "weeb-3 returned {} bytes for {} byte range",
                    bytes.len(),
                    end - start + 1
                );
            }
            Err(error) => last_error = error,
        }

        if attempt < retry_count {
            async_std::task::sleep(Duration::from_millis(
                RANGE_RETRY_DELAY_MS * (attempt as u64 + 1),
            ))
            .await;
        }
    }

    Err(last_error)
}

async fn read_cached_range(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
    start: u64,
    end: u64,
    generation: u64,
) -> Result<Vec<u8>, String> {
    let windows = range_storage_windows_for_span(start, end, metadata.size);
    let loads = windows.iter().map(|(window_start, window_end)| {
        read_range_window(
            weeb3.clone(),
            resource.clone(),
            metadata.clone(),
            *window_start,
            *window_end,
            generation,
        )
    });
    let responses = join_all(loads).await;
    let mut body = vec![0; (end - start + 1) as usize];
    let mut offset = 0usize;

    for (index, response) in responses.into_iter().enumerate() {
        let (window_start, window_end) = windows[index];
        let storage_body = response?;
        let expected_len = (window_end - window_start + 1) as usize;
        if storage_body.len() != expected_len {
            return Err(format!(
                "weeb-3 returned {} bytes for {} byte storage window",
                storage_body.len(),
                expected_len
            ));
        }

        let overlap_start = start.max(window_start);
        let overlap_end = end.min(window_end);
        let local_start = (overlap_start - window_start) as usize;
        let local_end = (overlap_end - window_start) as usize;
        let slice = &storage_body[local_start..=local_end];
        body[offset..offset + slice.len()].copy_from_slice(slice);
        offset += slice.len();
    }

    Ok(body)
}

async fn read_range_window(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
    start: u64,
    end: u64,
    generation: u64,
) -> Result<Vec<u8>, String> {
    let key = range_cache_key(&resource, &metadata, start, end);
    match FETCH_CACHE.with(|cache| cache.borrow_mut().range_load_role(&key, generation)) {
        RangeLoadRole::Cached(body) => return Ok(body),
        RangeLoadRole::Wait(receiver) => {
            return receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("range load was canceled".to_string()));
        }
        RangeLoadRole::Lead => {}
    }

    let timeout_ms = if generation > 0 {
        STREAM_RANGE_REQUEST_TIMEOUT_MS
    } else {
        RANGE_REQUEST_TIMEOUT_MS
    };
    let result = async_std::future::timeout(Duration::from_millis(timeout_ms), {
        let weeb3 = weeb3.clone();
        let metadata = metadata.clone();
        let stream_key = media_state_key(&resource, &metadata);
        async move {
            if generation > 0 {
                weeb3
                    .acquire_resolved_stream_range(metadata, start, end, stream_key, generation)
                    .await
            } else {
                weeb3.acquire_resolved_range(metadata, start, end).await
            }
        }
    })
    .await;

    let load_result = match result {
        Ok(Some((body, _metadata))) if body.len() == (end - start + 1) as usize => Ok(body),
        Ok(Some((body, _metadata))) => Err(format!(
            "weeb-3 returned {} bytes for {} byte range",
            body.len(),
            end - start + 1
        )),
        Ok(None) => Err(format!("weeb-3 did not retrieve range {}-{}", start, end)),
        Err(_) => Err(format!("timed out retrieving range {}-{}", start, end)),
    };

    if let Ok(body) = &load_result {
        FETCH_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .remember_range(key.clone(), body.clone(), generation);
        });
    }

    FETCH_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .finish_pending_range(&key, load_result.clone());
    });
    load_result
}

fn spawn_prefetch_media_stages(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
    response_start: u64,
    response_end: u64,
    requested_end: u64,
    generation: u64,
) {
    let key = media_state_key(&resource, &metadata);
    let should_spawn = FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let state = cache.media_state_mut(&key);
        if state.prefetch_running && state.prefetch_generation == generation {
            return false;
        }
        state.prefetch_running = true;
        state.prefetch_generation = generation;
        true
    });

    if !should_spawn {
        return;
    }

    spawn_local(async move {
        prefetch_media_stages(
            weeb3,
            resource.clone(),
            metadata.clone(),
            response_start,
            response_end,
            requested_end,
            generation,
        )
        .await;

        let key = media_state_key(&resource, &metadata);
        FETCH_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let state = cache.media_state_mut(&key);
            if state.prefetch_generation == generation {
                state.prefetch_running = false;
            }
        });
    });
}

async fn prefetch_media_stages(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
    response_start: u64,
    response_end: u64,
    requested_end: u64,
    generation: u64,
) {
    let prefetch_limit_end = requested_end
        .min(response_end.saturating_add(STREAM_PREFETCH_AHEAD_LIMIT_BYTES))
        .min(metadata.size.saturating_sub(1));

    for stage_bytes in STREAM_PREFETCH_STAGE_BYTES {
        if !media_generation_current(&resource, &metadata, generation) {
            return;
        }

        let current_end = media_high_water_end(&resource, &metadata, generation)
            .unwrap_or(response_end)
            .max(response_end);
        if current_end >= prefetch_limit_end || current_end >= metadata.size.saturating_sub(1) {
            return;
        }

        let target_end = current_end
            .saturating_add(stage_bytes)
            .min(prefetch_limit_end)
            .min(metadata.size.saturating_sub(1));
        prefetch_media_windows(
            weeb3.clone(),
            resource.clone(),
            metadata.clone(),
            response_start,
            response_end,
            target_end,
            generation,
        )
        .await;
    }
}

async fn prefetch_media_windows(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
    response_start: u64,
    response_end: u64,
    target_end: u64,
    generation: u64,
) {
    mark_media_window_complete(
        &resource,
        &metadata,
        response_start,
        response_end,
        generation,
    );
    mark_media_window_scheduled(&resource, &metadata, response_end, generation);

    loop {
        if !media_generation_current(&resource, &metadata, generation) {
            return;
        }

        let position = media_high_water_end(&resource, &metadata, generation)
            .map(|end| end.saturating_add(1))
            .unwrap_or(response_end.saturating_add(1));
        if position > target_end {
            return;
        }

        let mut windows = Vec::new();
        let mut next = position;
        while next <= target_end && windows.len() < STREAM_PREFETCH_MAX_WINDOWS {
            let window = range_storage_window_for_start(next, metadata.size);
            windows.push(window);
            mark_media_window_scheduled(&resource, &metadata, window.1, generation);
            next = window.1.saturating_add(1);
        }

        let loads = windows.iter().map(|(start, end)| {
            read_cached_range_with_retry(
                weeb3.clone(),
                resource.clone(),
                metadata.clone(),
                *start,
                *end,
                generation,
            )
        });
        let results = join_all(loads).await;

        for (index, result) in results.into_iter().enumerate() {
            if !media_generation_current(&resource, &metadata, generation) {
                return;
            }

            let (start, end) = windows[index];
            match result {
                Ok(bytes) if bytes.len() == (end - start + 1) as usize => {
                    mark_media_window_complete(&resource, &metadata, start, end, generation);
                }
                _ => {
                    mark_media_window_failure(&resource, &metadata, start, generation);
                    return;
                }
            }
        }

        async_std::task::sleep(Duration::from_millis(STREAM_PREFETCH_BATCH_YIELD_MS)).await;
    }
}

fn media_generation_current(resource: &str, metadata: &BzzMetadata, generation: u64) -> bool {
    let key = media_state_key(resource, metadata);
    FETCH_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .media_states
            .get(&key)
            .map(|state| state.generation == generation)
            .unwrap_or(false)
    })
}

fn media_high_water_end(resource: &str, metadata: &BzzMetadata, generation: u64) -> Option<u64> {
    let key = media_state_key(resource, metadata);
    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let state = cache.media_states.get_mut(&key)?;
        if state.generation != generation || state.high_water_end < 0 {
            return None;
        }
        Some(state.high_water_end as u64)
    })
}

fn mark_media_window_scheduled(resource: &str, metadata: &BzzMetadata, end: u64, generation: u64) {
    let key = media_state_key(resource, metadata);
    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let state = cache.media_state_mut(&key);
        if state.generation == generation {
            state.mark_scheduled(end);
        }
    });
}

fn mark_media_window_complete(
    resource: &str,
    metadata: &BzzMetadata,
    start: u64,
    end: u64,
    generation: u64,
) {
    let key = media_state_key(resource, metadata);
    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let state = cache.media_state_mut(&key);
        if state.generation == generation {
            state.mark_complete(start, end);
        }
    });
}

fn mark_media_window_failure(resource: &str, metadata: &BzzMetadata, start: u64, generation: u64) {
    let key = media_state_key(resource, metadata);
    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let state = cache.media_state_mut(&key);
        if state.generation == generation {
            state.mark_failure(start);
        }
    });
}

fn metadata_headers(metadata: &BzzMetadata, length: u64) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Accept-Ranges".to_string(), "bytes".to_string()),
        ("Content-Length".to_string(), length.to_string()),
        (
            "Content-Type".to_string(),
            if metadata.mime.is_empty() {
                "application/octet-stream".to_string()
            } else {
                metadata.mime.clone()
            },
        ),
    ];

    if !metadata.etag.is_empty() {
        headers.push(("ETag".to_string(), metadata.etag.clone()));
    }

    headers
}

fn parse_single_range(range: Option<&str>, size: u64) -> Option<Result<(u64, u64), ()>> {
    let range = range?.trim();
    let Some(spec) = range.strip_prefix("bytes=") else {
        return Some(Err(()));
    };
    if spec.contains(',') || size == 0 {
        return Some(Err(()));
    }

    let (start, end) = spec.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?;
        if suffix == 0 {
            return Some(Err(()));
        }
        let start = size.saturating_sub(suffix);
        return Some(Ok((start, size - 1)));
    }

    let start = start.parse::<u64>().ok()?;
    if start >= size {
        return Some(Err(()));
    }

    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>().ok()?.min(size - 1)
    };

    if end < start {
        return Some(Err(()));
    }

    Some(Ok((start, end)))
}

fn canonical_bzz_resource(pathname: &str) -> Option<String> {
    for marker in BZZ_MARKERS {
        let Some(idx) = pathname.find(marker) else {
            continue;
        };
        let resource = pathname[idx + marker.len()..].trim();
        if resource.is_empty() {
            return None;
        }

        let reference = resource.split('/').next().unwrap_or_default();
        if !is_swarm_reference(reference) {
            return None;
        }

        return Some(decode_component(resource));
    }

    None
}

fn canonical_raw_resource(pathname: &str) -> Option<(String, String)> {
    for (marker, raw_type) in RAW_ROUTE_MARKERS {
        if let Some(idx) = pathname.find(marker) {
            let resource = pathname[idx + marker.len()..].trim();
            if resource.is_empty() {
                return None;
            }
            return Some((raw_type.to_string(), decode_component(resource)));
        }
    }

    None
}

fn decode_component(value: &str) -> String {
    js_sys::decode_uri_component(value)
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| value.to_string())
}

fn is_swarm_reference(reference: &str) -> bool {
    (reference.len() == 64 || reference.len() == 128)
        && reference.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
}

pub async fn try_render_streaming_player(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
) -> bool {
    if !is_streamable_mime(&metadata.mime) {
        return false;
    }

    let Some(src) = canonical_bzz_url(&resource, &metadata) else {
        return false;
    };

    if !service_worker_controls_bzz_requests(&weeb3, "stream requests").await {
        navigate_to_bzz_url(&src);
        return true;
    }

    let player = create_streaming_player(&metadata.mime, &src);
    replace_result_view(&player);
    install_playback_notifications(&player, &src);
    install_play_retries(&player, &src);
    start_streaming_player(&player);
    true
}

fn handle_resolve_bzz_message(obj: &js_sys::Object, event: &MessageEvent, weeb3: Arc<Weeb3>) {
    let url = Reflect::get(obj, &JsValue::from_str("url")).unwrap_or(JsValue::NULL);
    let reference = url.as_string().unwrap_or_default();
    let port = message_port(event);

    spawn_local(async move {
        let resp = js_sys::Object::new();

        if let Some(metadata) = weeb3.resolve_bzz(reference).await {
            set_js(&resp, "ok", JsValue::TRUE);
            set_js(&resp, "type", JsValue::from_str("RESOLVE_BZZ_RESPONSE"));
            set_js(
                &resp,
                "data_reference",
                JsValue::from_str(&hex::encode(metadata.data_reference)),
            );
            set_js(&resp, "mime", JsValue::from_str(&metadata.mime));
            set_js(&resp, "size", JsValue::from_f64(metadata.size as f64));
            set_js(&resp, "etag", JsValue::from_str(&metadata.etag));
            set_js(&resp, "path", JsValue::from_str(&metadata.path));
        } else {
            set_js(&resp, "ok", JsValue::FALSE);
        }

        if let Some(port) = port {
            let _ = port.post_message(&resp);
        }
    });
}

fn handle_retrieve_range_message(obj: &js_sys::Object, event: &MessageEvent, weeb3: Arc<Weeb3>) {
    let url = Reflect::get(obj, &JsValue::from_str("url")).unwrap_or(JsValue::NULL);
    let reference = url.as_string().unwrap_or_default();
    let start = Reflect::get(obj, &JsValue::from_str("start"))
        .unwrap_or(JsValue::from_f64(0.0))
        .as_f64()
        .unwrap_or(0.0)
        .max(0.0) as u64;
    let end_inclusive = Reflect::get(obj, &JsValue::from_str("end"))
        .unwrap_or(JsValue::from_f64(0.0))
        .as_f64()
        .unwrap_or(0.0)
        .max(0.0) as u64;
    let stream_key = js_string_property(obj, "stream_key").unwrap_or_default();
    let stream_generation = Reflect::get(obj, &JsValue::from_str("stream_generation"))
        .unwrap_or(JsValue::from_f64(0.0))
        .as_f64()
        .unwrap_or(0.0)
        .max(0.0) as u64;
    let resolved_metadata = metadata_from_range_message(obj);
    let port = message_port(event);

    spawn_local(async move {
        let resp = js_sys::Object::new();

        let range_result = if let Some(metadata) = resolved_metadata {
            if !stream_key.is_empty() && stream_generation > 0 {
                weeb3
                    .acquire_resolved_stream_range(
                        metadata,
                        start,
                        end_inclusive,
                        stream_key,
                        stream_generation,
                    )
                    .await
            } else {
                weeb3
                    .acquire_resolved_range(metadata, start, end_inclusive)
                    .await
            }
        } else {
            weeb3.acquire_range(reference, start, end_inclusive).await
        };

        if let Some((bytes, metadata)) = range_result {
            set_js(&resp, "ok", JsValue::TRUE);
            set_js(&resp, "type", JsValue::from_str("RETRIEVE_RANGE_RESPONSE"));

            let u8arr = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
            u8arr.copy_from(&bytes);
            set_js(&resp, "body", u8arr.into());
            set_js(&resp, "mime", JsValue::from_str(&metadata.mime));
            set_js(&resp, "size", JsValue::from_f64(metadata.size as f64));
            set_js(&resp, "etag", JsValue::from_str(&metadata.etag));
            set_js(&resp, "path", JsValue::from_str(&metadata.path));
        } else {
            set_js(&resp, "ok", JsValue::FALSE);
            set_js(
                &resp,
                "error",
                JsValue::from_str(&format!(
                    "failed to retrieve range {}-{}",
                    start, end_inclusive
                )),
            );
        }

        if let Some(port) = port {
            let _ = port.post_message(&resp);
        }
    });
}

fn handle_prepare_bzz_stream_message(
    obj: &js_sys::Object,
    event: &MessageEvent,
    weeb3: Arc<Weeb3>,
) {
    let metadata = metadata_from_range_message(obj);
    let port = message_port(event);

    spawn_local(async move {
        let resp = js_sys::Object::new();
        let prepared = if let Some(metadata) = metadata {
            weeb3.prepare_bzz_stream(metadata).await
        } else {
            false
        };

        set_js(&resp, "ok", JsValue::from_bool(prepared));
        set_js(
            &resp,
            "type",
            JsValue::from_str("PREPARE_BZZ_STREAM_RESPONSE"),
        );

        if let Some(port) = port {
            let _ = port.post_message(&resp);
        }
    });
}

fn metadata_from_range_message(obj: &js_sys::Object) -> Option<BzzMetadata> {
    let data_reference = js_string_property(obj, "data_reference")
        .and_then(|reference| hex::decode(reference).ok())?;
    if data_reference.len() != 32 && data_reference.len() != 64 {
        return None;
    }

    let size = Reflect::get(obj, &JsValue::from_str("size"))
        .ok()
        .and_then(|size| size.as_f64())
        .filter(|size| *size >= 0.0)? as u64;

    Some(BzzMetadata {
        data_reference,
        mime: js_string_property(obj, "mime").unwrap_or_else(|| "application/octet-stream".into()),
        size,
        etag: js_string_property(obj, "etag").unwrap_or_default(),
        path: js_string_property(obj, "path").unwrap_or_default(),
        target_count: 1,
    })
}

fn js_string_property(obj: &js_sys::Object, name: &str) -> Option<String> {
    Reflect::get(obj, &JsValue::from_str(name))
        .ok()
        .and_then(|value| value.as_string())
}

fn message_port(event: &MessageEvent) -> Option<web_sys::MessagePort> {
    let ports: Array = event.ports().into();
    ports.get(0).dyn_into::<web_sys::MessagePort>().ok()
}

fn is_streamable_mime(mime: &str) -> bool {
    mime.starts_with("video/") || mime.starts_with("audio/")
}

fn canonical_bzz_url(resource: &str, metadata: &BzzMetadata) -> Option<String> {
    let reference = bzz_reference_hex(resource)?;
    let requested_path = resource
        .split_once(&reference)
        .map(|(_, tail)| normalize_bzz_path(tail))
        .unwrap_or_default();
    let resolved_path = normalize_bzz_path(&metadata.path);
    let path = if !requested_path.is_empty()
        && (resolved_path.is_empty() || requested_path == resolved_path)
    {
        requested_path
    } else {
        resolved_path
    };

    let prefix = match active_profile().mode {
        NetworkMode::Mainnet => "/weeb-3/bzz",
        NetworkMode::Testnet => "/weeb-3/testnet/bzz",
    };

    if path.is_empty() || path.starts_with("unknown") || path == "not found" {
        Some(format!("{}/{}", prefix, reference))
    } else {
        Some(format!("{}/{}/{}", prefix, reference, path))
    }
}

fn replace_result_view(new_element: &Element) {
    let document = web_sys::window().unwrap().document().unwrap();
    let result = document
        .get_element_by_id("resultField")
        .expect("#resultField should exist")
        .dyn_into::<HtmlElement>()
        .expect("#resultField should be a HtmlElement");

    result.set_inner_html("");
    let _ = result.append_child(new_element);
}

fn create_streaming_player(mime: &str, src: &str) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();
    let is_video = mime.starts_with("video/");
    let tag = if is_video { "video" } else { "audio" };
    let player = document.create_element(tag).unwrap();

    let _ = player.set_attribute("controls", "");
    let _ = player.set_attribute("autoplay", "");
    let _ = player.set_attribute("preload", "metadata");
    if is_video {
        let _ = player.set_attribute("playsinline", "");
    }
    let _ = Reflect::set(
        player.as_ref(),
        &JsValue::from_str("muted"),
        &JsValue::FALSE,
    );
    let _ = Reflect::set(
        player.as_ref(),
        &JsValue::from_str("defaultMuted"),
        &JsValue::FALSE,
    );
    let _ = Reflect::set(
        player.as_ref(),
        &JsValue::from_str("volume"),
        &JsValue::from_f64(1.0),
    );
    let _ = Reflect::set(
        player.as_ref(),
        &JsValue::from_str("autoplay"),
        &JsValue::TRUE,
    );
    let _ = player.set_attribute("src", src);
    let _ = player.set_attribute("style", "width:90%;max-height:75vh;");

    player
}

fn start_streaming_player(player: &Element) {
    call_media_method(player, "play");
}

fn call_media_method(player: &Element, method: &str) {
    let Ok(function) = Reflect::get(player.as_ref(), &JsValue::from_str(method)) else {
        return;
    };
    let Some(function) = function.dyn_ref::<Function>() else {
        return;
    };
    let _ = function.call0(player.as_ref());
}

fn install_playback_notifications(player: &Element, src: &str) {
    let player_for_callback = player.clone();
    let src = src.to_string();
    let callback = Closure::<dyn FnMut()>::new(move || {
        let _ = player_for_callback.remove_attribute("data-weeb3-media-error");
        let _ = player_for_callback.remove_attribute("data-weeb3-media-retrying");
        let _ = player_for_callback.remove_attribute("data-weeb3-media-retry-scheduled");
        let _ = player_for_callback.remove_attribute("data-weeb3-media-retry-attempt");
        let _ = player_for_callback.remove_attribute("data-weeb3-retry-time");
        notify_media_playing(&src);
    });

    let _ = player.add_event_listener_with_callback("playing", callback.as_ref().unchecked_ref());
    callback.forget();
}

fn install_play_retries(player: &Element, src: &str) {
    for event_name in ["loadedmetadata", "loadeddata", "canplay"] {
        let player_for_callback = player.clone();
        let event_target = player.clone();
        let callback = Closure::<dyn FnMut()>::new(move || {
            let retrying = player_for_callback
                .get_attribute("data-weeb3-media-retrying")
                .as_deref()
                == Some("1");
            if !retrying {
                return;
            }
            apply_media_retry_time(&player_for_callback);
            start_streaming_player(&player_for_callback);
        });

        let _ = event_target
            .add_event_listener_with_callback(event_name, callback.as_ref().unchecked_ref());
        callback.forget();
    }

    {
        let player_for_callback = player.clone();
        let src = src.to_string();
        let callback = Closure::<dyn FnMut()>::new(move || {
            let _ = player_for_callback.set_attribute("data-weeb3-media-error", "1");
            let _ = player_for_callback.remove_attribute("data-weeb3-media-retrying");
            let _ = reset_bzz_fetch_url_activity(&src, "media error");
            schedule_media_retry(player_for_callback.clone(), src.clone());
        });
        let _ = player.add_event_listener_with_callback("error", callback.as_ref().unchecked_ref());
        callback.forget();
    }

    for event_name in [
        "play",
        "seeking",
        "seeked",
        "click",
        "pointerdown",
        "mousedown",
        "touchstart",
        "keydown",
    ] {
        let player_for_callback = player.clone();
        let event_target = player.clone();
        let src = src.to_string();
        let callback = Closure::<dyn FnMut()>::new(move || {
            let errored = player_for_callback
                .get_attribute("data-weeb3-media-error")
                .as_deref()
                == Some("1");
            if !errored {
                return;
            }

            remember_media_retry_time(&player_for_callback);
            let retrying = player_for_callback
                .get_attribute("data-weeb3-media-retrying")
                .as_deref()
                == Some("1");
            if retrying {
                return;
            }

            start_media_retry(&player_for_callback, &src, false);
        });

        let _ = event_target
            .add_event_listener_with_callback(event_name, callback.as_ref().unchecked_ref());
        callback.forget();
    }
}

fn schedule_media_retry(player: Element, src: String) {
    let errored = player.get_attribute("data-weeb3-media-error").as_deref() == Some("1");
    if !errored {
        return;
    }
    let scheduled = player
        .get_attribute("data-weeb3-media-retry-scheduled")
        .as_deref()
        == Some("1");
    if scheduled {
        return;
    }

    let attempt = player
        .get_attribute("data-weeb3-media-retry-attempt")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let Some(delay_ms) = MEDIA_RETRY_DELAYS_MS.get(attempt).copied() else {
        return;
    };

    let _ = player.set_attribute("data-weeb3-media-retry-scheduled", "1");
    spawn_local(async move {
        async_std::task::sleep(Duration::from_millis(delay_ms)).await;
        let _ = player.remove_attribute("data-weeb3-media-retry-scheduled");
        start_media_retry(&player, &src, true);
    });
}

fn start_media_retry(player: &Element, src: &str, advance_attempt: bool) {
    let errored = player.get_attribute("data-weeb3-media-error").as_deref() == Some("1");
    if !errored {
        return;
    }
    let retrying = player.get_attribute("data-weeb3-media-retrying").as_deref() == Some("1");
    if retrying {
        return;
    }

    remember_media_retry_time(player);
    let attempt = player
        .get_attribute("data-weeb3-media-retry-attempt")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let next_attempt = if advance_attempt {
        attempt.saturating_add(1)
    } else {
        0
    };
    let _ = player.set_attribute("data-weeb3-media-retry-attempt", &next_attempt.to_string());
    let _ = player.set_attribute("data-weeb3-media-retrying", "1");
    let _ = player.remove_attribute("data-weeb3-media-retry-scheduled");
    let _ = reset_bzz_fetch_url_activity(src, "media retry");
    call_media_method(player, "load");
    apply_media_retry_time(player);
    start_streaming_player(player);
}

fn remember_media_retry_time(player: &Element) {
    let Some(time) = media_current_time(player) else {
        return;
    };
    if time <= 0.0 {
        return;
    }
    let _ = player.set_attribute("data-weeb3-retry-time", &time.to_string());
}

fn apply_media_retry_time(player: &Element) {
    let Some(time) = player
        .get_attribute("data-weeb3-retry-time")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|time| time.is_finite() && *time > 0.0)
    else {
        return;
    };
    let _ = Reflect::set(
        player.as_ref(),
        &JsValue::from_str("currentTime"),
        &JsValue::from_f64(time),
    );
}

fn media_current_time(player: &Element) -> Option<f64> {
    Reflect::get(player.as_ref(), &JsValue::from_str("currentTime"))
        .ok()
        .and_then(|value| value.as_f64())
        .filter(|time| time.is_finite())
}

fn notify_media_playing(src: &str) {
    let service0 = web_sys::window().unwrap().navigator().service_worker();
    let Ok(controller) = Reflect::get(service0.as_ref(), &JsValue::from_str("controller")) else {
        return;
    };
    if controller.is_null() || controller.is_undefined() {
        return;
    }

    let message = Object::new();
    let _ = Reflect::set(
        &message,
        &JsValue::from_str("type"),
        &JsValue::from_str("BZZ_MEDIA_PLAYING"),
    );
    let _ = Reflect::set(&message, &JsValue::from_str("url"), &JsValue::from_str(src));

    let Ok(post_message) = Reflect::get(&controller, &JsValue::from_str("postMessage")) else {
        return;
    };
    let Some(post_message) = post_message.dyn_ref::<Function>() else {
        return;
    };

    let _ = post_message.call1(&controller, message.as_ref());
}

fn navigate_to_bzz_url(src: &str) {
    if let Some(location) = web_sys::window().map(|window| window.location()) {
        let _ = location.assign(src);
    }
}
