use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, hash_map::RandomState},
    rc::Rc,
    time::Duration,
};

use async_std::sync::Arc;
use bytes::Bytes;
use hashlink::LinkedHashMap;
use js_sys::{Array, Object, Reflect};
use libp2p::futures::{StreamExt, future::join_all, stream};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlMediaElement};

use crate::{
    Weeb3,
    bzz_stream::{BzzMetadata, canonical_bzz_url},
    interface::service_worker_controls_bzz_requests,
    oneshot,
    retrieval_conventions::{
        PendingGenerationRelation, RetrieveAdmission, SingleflightRegistration,
        SingleflightRegistry, next_nonzero_generation, pending_generation_relation,
    },
    shared_runtime::SharedNodeClient,
    stream_conventions::{
        MEDIA_PREFETCH_BATCH_YIELD_MS, MEDIA_PREFETCH_MAX_PARALLEL, MEDIA_RANGE_READ_MAX_PARALLEL,
        MEDIA_STARTUP_RESPONSE_BYTES,
        MEDIA_STORAGE_WINDOW_BYTES, MIB_BYTES, decode_component, if_none_match_matches,
        if_range_allows_range, immutable_metadata_identity, is_swarm_reference_hex,
        media_cache_budget_bytes, media_prefetch_ahead_limit_bytes, media_prefetch_stage_targets,
        parse_single_range, route_resource, window_key,
    },
    stream_hls::HLS_BODY_MAX_BYTES,
    worker_protocol::{bytes_to_js, set as set_js, string_property},
};

const STREAM_ACTIVE_RESPONSE_BUFFER_BYTES: u64 = 2 * MIB_BYTES;
const STREAM_SEEK_KEEP_AHEAD_BYTES: u64 = 16 * MIB_BYTES;
const STREAM_SEEK_REQUEST_GAP_BYTES: u64 = 6 * MIB_BYTES;
const METADATA_CACHE_MAX_ENTRIES: usize = 1024;
const MEDIA_STREAM_STATE_MAX_ENTRIES: usize = 64;
const RANGE_SINGLEFLIGHT_MAX_LOADS: usize = 256;
const RANGE_SINGLEFLIGHT_MAX_WAITERS: usize = 64;
const RANGE_RETRY_DELAY_MS: u64 = 700;
const RANGE_REQUEST_TIMEOUT_MS: u64 = 210_000;
const STREAM_RANGE_REQUEST_TIMEOUT_MS: u64 = 15_000;
const MEDIA_RETRY_DELAYS_MS: [u64; 6] = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
thread_local! {
    static MEDIA_GENERATION_SEQUENCE: Cell<u64> = const { Cell::new(0) };
    static RESULT_VIEW_GENERATION: Cell<u64> = const { Cell::new(0) };
    static FETCH_CACHE: RefCell<FetchCache> = RefCell::new(FetchCache::default());
    static AUXILIARY_MEDIA_CACHE_BYTES: Cell<u64> = const { Cell::new(0) };
    static MEDIA_CACHE_BUDGET_BYTES: u64 = detect_media_cache_max_bytes();
    static MEDIA_ELEMENT_CALLBACKS: RefCell<Vec<MediaElementCallback>> =
        const { RefCell::new(Vec::new()) };
}

/// Reserve the result view without cancelling already-dispatched work.
pub(crate) fn begin_result_view_request() -> u64 {
    RESULT_VIEW_GENERATION.with(|generation| {
        let next = next_nonzero_generation(generation.get());
        generation.set(next);
        next
    })
}

pub(crate) fn result_view_request_is_current(expected: u64) -> bool {
    RESULT_VIEW_GENERATION.with(|generation| generation.get() == expected)
}

struct MediaElementCallback {
    target: Element,
    event_names: &'static [&'static str],
    callback: Closure<dyn FnMut()>,
}

#[derive(Default)]
struct MediaRetryState {
    errored: bool,
    retrying: bool,
    scheduled: bool,
    attempt: usize,
    playback_time: Option<f64>,
}

impl Drop for MediaElementCallback {
    fn drop(&mut self) {
        for event_name in self.event_names {
            let _ = self.target.remove_event_listener_with_callback(
                event_name,
                self.callback.as_ref().unchecked_ref(),
            );
        }
    }
}

pub(crate) fn next_media_generation() -> u64 {
    MEDIA_GENERATION_SEQUENCE.with(|sequence| {
        let next = next_nonzero_generation(sequence.get());
        sequence.set(next);
        next
    })
}

#[derive(Default)]
struct FetchCache {
    epoch: u64,
    metadata: LinkedHashMap<String, BzzMetadata, RandomState>,
    ranges: LinkedHashMap<String, Bytes, RandomState>,
    pending_ranges: SingleflightRegistry<String, oneshot::Sender<Result<Bytes, String>>, RangeFlight>,
    range_bytes: u64,
    media_states: HashMap<String, MediaState>,
}

impl FetchCache {
    fn metadata(&mut self, resource: &str) -> Option<BzzMetadata> {
        self.metadata.to_back(resource).cloned()
    }

    fn remember_metadata(&mut self, resource: String, metadata: BzzMetadata) {
        self.metadata.insert(resource, metadata);
        while self.metadata.len() > METADATA_CACHE_MAX_ENTRIES {
            self.metadata.pop_front();
        }
    }

    fn range(&mut self, key: &str) -> Option<Bytes> {
        self.ranges.to_back(key).cloned()
    }

    fn remember_range(&mut self, key: String, body: Bytes, media_state_key: &str, generation: u64) {
        if generation > 0
            && !self
                .media_states
                .get(media_state_key)
                .is_some_and(|state| state.generation == generation)
        {
            return;
        }
        let body_len = body.len() as u64;
        if body_len > range_cache_capacity_bytes() {
            return;
        }
        if let Some(old) = self.ranges.insert(key, body) {
            self.range_bytes = self.range_bytes.saturating_sub(old.len() as u64);
        }
        self.range_bytes = self.range_bytes.saturating_add(body_len);
        self.trim_ranges();
    }

    fn clear_completed_ranges(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        for state in self.media_states.values_mut() {
            state.reset();
        }
        self.ranges.clear();
        self.range_bytes = 0;
    }

    fn forget_reference_ranges(&mut self, reference: &str) {
        let prefix = format!("{reference}|");
        self.ranges.retain(|key, body| {
            if key.starts_with(&prefix) {
                self.range_bytes = self.range_bytes.saturating_sub(body.len() as u64);
                false
            } else {
                true
            }
        });
    }

    fn range_load_role(
        &mut self,
        cache_key: &str,
        generation: u64,
        cancel_when_unused: bool,
    ) -> RangeLoadRole {
        if let Some(body) = self.range(cache_key) {
            return RangeLoadRole::Cached(body);
        }
        let pending_key = pending_range_key(cache_key, generation);
        let mut joining = false;
        if let Some((flight_id, shared, waiters)) = self
            .pending_ranges
            .inspect_waiters(&pending_key, |waiter| !waiter.is_canceled())
        {
            match pending_generation_relation(shared.generation, generation) {
                PendingGenerationRelation::RejectStale => {
                    return RangeLoadRole::Reject("stale range generation".to_string());
                }
                PendingGenerationRelation::Join
                    if shared
                        .admission
                        .as_ref()
                        .is_none_or(RetrieveAdmission::is_open) =>
                {
                    if waiters >= RANGE_SINGLEFLIGHT_MAX_WAITERS {
                        return RangeLoadRole::Reject(
                            "range already has too many waiting requests".to_string(),
                        );
                    }
                    joining = true;
                }
                _ => {
                    if let Some(stale) = self.pending_ranges.take(&pending_key, flight_id) {
                        if let Some(admission) = stale.shared.admission {
                            admission.close();
                        }
                        finish_range_waiters(
                            stale.waiters,
                            Err("stale range generation replaced".to_string()),
                        );
                    }
                }
            }
        }
        if !joining && self.pending_ranges.len() >= RANGE_SINGLEFLIGHT_MAX_LOADS {
            return RangeLoadRole::Reject("too many range loads are already pending".to_string());
        }
        let (sender, receiver) = oneshot::channel();
        let registration = self.pending_ranges.register(
            pending_key,
            sender,
            || RangeFlight { generation, admission: cancel_when_unused.then(RetrieveAdmission::new) },
            |shared| {
                // Stable readers retain completion/cache ownership even if they leave.
                if !cancel_when_unused { shared.admission = None; }
                shared.clone()
            },
        );
        RangeLoadRole::Read(receiver, registration)
    }

    fn trim_ranges(&mut self) {
        let max_bytes = range_cache_capacity_bytes();
        while self.range_bytes > max_bytes {
            let Some((_, range)) = self.ranges.pop_front() else {
                break;
            };
            self.range_bytes = self.range_bytes.saturating_sub(range.len() as u64);
        }
    }

    fn media_state_mut(&mut self, key: &str) -> &mut MediaState {
        if !self.media_states.contains_key(key) {
            self.media_states
                .insert(key.to_string(), MediaState::new(next_media_generation()));
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

        while self.media_states.len() > MEDIA_STREAM_STATE_MAX_ENTRIES {
            let Some(oldest) = self
                .media_states
                .iter()
                .filter(|(key, state)| key.as_str() != active_key && state.prefetch_generation == 0)
                .min_by(|left, right| left.1.last_touch.total_cmp(&right.1.last_touch))
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.media_states.remove(&oldest);
        }
    }
}

#[derive(Clone)]
struct RangeFlight {
    generation: u64,
    admission: Option<RetrieveAdmission>,
}

struct RangeWaiterGuard {
    key: String,
    flight_id: u64,
    waiter_id: u64,
}

impl RangeWaiterGuard {
    fn retain_owner(&self) {
        FETCH_CACHE.with(|cache| {
            if let Some(shared) = cache
                .borrow_mut()
                .pending_ranges
                .shared_mut(&self.key, self.flight_id)
            {
                shared.admission = None;
            }
        });
    }
}

impl Drop for RangeWaiterGuard {
    fn drop(&mut self) {
        let admission = FETCH_CACHE.with(|cache| {
            cache.borrow_mut().pending_ranges.remove_waiter(
                &self.key,
                self.flight_id,
                self.waiter_id,
            ).and_then(|shared| shared.admission.clone())
        });
        if let Some(admission) = admission {
            admission.close();
        }
    }
}

fn finish_range_waiters(
    waiters: Vec<(u64, oneshot::Sender<Result<Bytes, String>>)>,
    result: Result<Bytes, String>,
) {
    for (_, waiter) in waiters {
        let _ = waiter.send(result.clone());
    }
}

enum RangeLoadRole {
    Cached(Bytes),
    Read(
        oneshot::Receiver<Result<Bytes, String>>,
        SingleflightRegistration<String, RangeFlight>,
    ),
    Reject(String),
}

#[derive(Debug)]
struct RangeReadError {
    message: String,
    waiter_timed_out: bool,
}

impl RangeReadError {
    fn waiter_timeout(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            waiter_timed_out: true,
        }
    }
}

impl<T: Into<String>> From<T> for RangeReadError {
    fn from(message: T) -> Self {
        Self {
            message: message.into(),
            waiter_timed_out: false,
        }
    }
}

fn detect_media_cache_max_bytes() -> u64 {
    let mut js_heap_size_limit = None;
    let global = js_sys::global();
    if let Ok(performance) = Reflect::get(&global, &"performance".into())
        && let Ok(memory) = Reflect::get(&performance, &"memory".into())
        && let Ok(limit) = Reflect::get(&memory, &"jsHeapSizeLimit".into())
    {
        js_heap_size_limit = limit.as_f64();
    }

    let mut device_memory_gib = None;
    if let Some(window) = web_sys::window() {
        let navigator = window.navigator();
        if let Ok(device_memory) = Reflect::get(navigator.as_ref(), &"deviceMemory".into()) {
            device_memory_gib = device_memory.as_f64();
        }
    }

    media_cache_budget_bytes(js_heap_size_limit, device_memory_gib)
}

pub(crate) fn media_cache_max_bytes() -> u64 {
    MEDIA_CACHE_BUDGET_BYTES.with(|bytes| *bytes)
}

fn range_cache_capacity_bytes() -> u64 {
    AUXILIARY_MEDIA_CACHE_BYTES.with(|bytes| media_cache_max_bytes().saturating_sub(bytes.get()))
}

fn stream_prefetch_ahead_limit_bytes() -> u64 {
    media_prefetch_ahead_limit_bytes(media_cache_max_bytes())
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
    last_request_start: u64,
    prefetch_generation: u64,
    last_touch: f64,
}

impl MediaState {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            anchor_start: None,
            high_water_end: -1,
            scheduled_high_water_end: -1,
            completed_ranges: BTreeMap::new(),
            last_request_start: 0,
            prefetch_generation: 0,
            last_touch: js_sys::Date::now(),
        }
    }

    fn effective_high_water_end(&self) -> i64 {
        self.high_water_end.max(self.scheduled_high_water_end)
    }

    fn reset(&mut self) {
        let last_request_start = self.last_request_start;
        *self = Self::new(next_media_generation());
        self.last_request_start = last_request_start;
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
        self.last_touch = js_sys::Date::now();
    }

    fn mark_failure(&mut self, start: u64) {
        let failure_end = if start == 0 { -1 } else { start as i64 - 1 };
        self.scheduled_high_water_end = self.scheduled_high_water_end.min(failure_end);
        self.scheduled_high_water_end = self.scheduled_high_water_end.max(self.high_water_end);
        self.last_touch = js_sys::Date::now();
    }
}

pub(crate) struct FetchResponse {
    ok: bool,
    status: u16,
    headers: Vec<(String, String)>,
    body: Option<Bytes>,
    error: String,
    stream: bool,
}

impl FetchResponse {
    pub(crate) fn ok(status: u16, headers: Vec<(String, String)>, body: Option<Vec<u8>>) -> Self {
        Self {
            ok: true,
            status,
            headers,
            body: body.map(Bytes::from),
            error: String::new(),
            stream: false,
        }
    }

    pub(crate) fn ok_shared(status: u16, headers: Vec<(String, String)>, body: Bytes) -> Self {
        Self {
            body: Some(body),
            ..Self::ok(status, headers, None)
        }
    }

    pub(crate) fn stream(status: u16, headers: Vec<(String, String)>) -> Self {
        Self {
            stream: true,
            ..Self::ok(status, headers, None)
        }
    }

    pub(crate) fn error(status: u16, error: impl Into<String>) -> Self {
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
            set_js(&resp, "body", bytes_to_js(&body).into());
        }

        resp
    }
}

pub(crate) async fn service_worker_message_response(
    obj: &js_sys::Object,
    weeb3: Arc<Weeb3>,
) -> Object {
    let url = string_property(obj.as_ref(), "url").unwrap_or_default();
    let mut method = string_property(obj.as_ref(), "method").unwrap_or_else(|| "GET".into());
    method.make_ascii_uppercase();
    let range = string_property(obj.as_ref(), "range").filter(|value| !value.is_empty());
    let if_none_match =
        string_property(obj.as_ref(), "ifNoneMatch").filter(|value| !value.trim().is_empty());
    let if_range =
        string_property(obj.as_ref(), "ifRange").filter(|value| !value.trim().is_empty());
    fetch_request_response(
        weeb3,
        url,
        method,
        range,
        if_none_match,
        if_range,
    )
    .await
    .into_js()
}

async fn fetch_request_response(
    weeb3: Arc<Weeb3>,
    url: String,
    method: String,
    range: Option<String>,
    if_none_match: Option<String>,
    if_range: Option<String>,
) -> FetchResponse {
    if method != "GET" && method != "HEAD" {
        return FetchResponse::error(405, "method not allowed");
    }

    let parsed_url = web_sys::Url::new(&url).ok();
    let pathname = match &parsed_url {
        Some(url) => url.pathname(),
        None => url.clone(),
    };

    if let Some(resource) = canonical_bzz_resource(&pathname) {
        return fetch_bzz_response(weeb3, resource, method, range, if_none_match, if_range).await;
    }

    if let Some(response) = crate::stream_hls::try_fetch_response(
        weeb3.clone(),
        &url,
        &pathname,
        &method,
        range.as_deref(),
        if_none_match.as_deref(),
    )
    .await
    {
        return response;
    }

    if let Some((raw_type, reference)) = canonical_raw_resource(&pathname) {
        return fetch_raw_response(weeb3, raw_type, reference, method).await;
    }

    FetchResponse::error(404, "weeb-3 route not found")
}

async fn fetch_raw_response(
    weeb3: Arc<Weeb3>,
    raw_type: &'static str,
    reference: String,
    method: String,
) -> FetchResponse {
    let mut parts = reference.split('/');
    let reference = parts.next().unwrap_or_default();
    if parts.any(|part| !part.is_empty()) {
        return FetchResponse::error(400, "raw route accepts one swarm reference");
    }
    if !is_swarm_reference_hex(reference) {
        return FetchResponse::error(400, "invalid swarm reference");
    }
    let reference = reference.to_string();

    let mut headers = vec![
        (
            "Content-Type".to_string(),
            "application/octet-stream".to_string(),
        ),
        ("Cache-Control".to_string(), "no-store".to_string()),
        (
            "Content-Disposition".to_string(),
            format!("attachment; filename=\"{}\"", reference),
        ),
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
    if_none_match: Option<String>,
    if_range: Option<String>,
) -> FetchResponse {
    let Some(metadata) = resolve_bzz_cached(weeb3.clone(), resource.clone()).await else {
        return FetchResponse::error(404, "weeb-3 did not resolve resource");
    };

    if if_none_match_matches(if_none_match.as_deref(), &metadata.etag) {
        let headers = metadata_headers(&metadata, metadata.size)
            .into_iter()
            .filter(|(name, _)| !name.eq_ignore_ascii_case("Content-Length"))
            .collect();
        return FetchResponse::ok(304, headers, None);
    }

    if method == "HEAD" {
        return FetchResponse::ok(200, metadata_headers(&metadata, metadata.size), None);
    }

    if metadata.size == 0 {
        return FetchResponse::ok(200, metadata_headers(&metadata, 0), Some(vec![]));
    }

    let streamable = is_streamable_mime(&metadata.mime);
    let parsed_range = parse_single_range(range.as_deref(), metadata.size);
    if (range.is_some() && !if_range_allows_range(if_range.as_deref(), &metadata.etag))
        || (!streamable && parsed_range.is_none())
    {
        // Failed If-Range and un-ranged non-media requests return the whole resource.
        if should_inline_non_streamable_response(&metadata) {
            return full_bzz_response(weeb3, resource, metadata).await;
        }
        return FetchResponse::stream(200, metadata_headers(&metadata, metadata.size));
    }

    let (start, end, media_state) = match parsed_range {
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
            (start, end, media_state)
        }
        None => {
            let media_state = begin_media_range(&resource, &metadata, 0);
            let end = MEDIA_STARTUP_RESPONSE_BYTES
                .saturating_sub(1)
                .min(metadata.size - 1);
            (0, end, Some(media_state))
        }
    };

    mark_range_windows_scheduled(&resource, &metadata, start, end, &media_state);
    let generation = media_state
        .as_ref()
        .map(|state| state.generation)
        .unwrap_or(0);

    let bytes =
        match read_cached_range_with_retry(&weeb3, &resource, &metadata, start, end, generation)
            .await
        {
            Ok(bytes) => bytes,
            Err(error) => {
                // A waiter timeout is not a terminal transport result. Keep the
                // shared pending load and its media generation intact so a browser
                // retry joins the accounting-safe drain instead of redispatching.
                if !error.waiter_timed_out {
                    note_media_range_failure(&resource, &metadata, start, &media_state);
                }
                return FetchResponse::error(503, error.message);
            }
        };

    if bytes.len() != (end - start + 1) as usize {
        note_media_range_failure(&resource, &metadata, start, &media_state);
        return FetchResponse::error(502, "weeb-3 returned a short range");
    }

    let mut headers = metadata_headers(&metadata, bytes.len() as u64);
    headers.push((
        "Content-Range".to_string(),
        format!("bytes {}-{}/{}", start, end, metadata.size),
    ));
    if let Some(media_state) = &media_state {
        mark_media_range_complete(&resource, &metadata, start, end, media_state);
        spawn_prefetch_media_stages(
            weeb3,
            resource,
            metadata,
            end,
            media_state.generation,
        );
    }

    FetchResponse::ok_shared(206, headers, bytes)
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
    let bytes =
        match read_cached_range_with_retry(&weeb3, &resource, &metadata, 0, size - 1, 0).await {
            Ok(bytes) => bytes,
            Err(error) => return FetchResponse::error(503, error.message),
        };

    if bytes.len() != size as usize {
        return FetchResponse::error(502, "weeb-3 returned a short body");
    }

    FetchResponse::ok_shared(200, metadata_headers(&metadata, size), bytes)
}

fn should_inline_non_streamable_response(metadata: &BzzMetadata) -> bool {
    metadata.size <= MEDIA_STORAGE_WINDOW_BYTES
}

fn metadata_identity(resource: &str, metadata: &BzzMetadata) -> String {
    immutable_metadata_identity(resource, &metadata.data_reference, &metadata.etag)
}

fn media_state_key(resource: &str, metadata: &BzzMetadata) -> String {
    format!("{}|{}", metadata_identity(resource, metadata), resource)
}

fn with_current_media_state(key: &str, generation: u64, update: impl FnOnce(&mut MediaState)) {
    FETCH_CACHE.with_borrow_mut(|cache| {
        if let Some(state) = cache
            .media_states
            .get_mut(key)
            .filter(|state| state.generation == generation)
        {
            update(state);
        }
    });
}

fn pending_range_key(cache_key: &str, generation: u64) -> String {
    // Stable document/download streams must not share a cancellable pending
    // slot with seekable media. Both scopes still converge on the same
    // immutable completed-range cache after either leader finishes.
    if generation == 0 {
        format!("{cache_key}|pending:stable")
    } else {
        format!("{cache_key}|pending:media")
    }
}

fn range_storage_window_for_start(start: u64, size: u64) -> (u64, u64) {
    let storage_start = (start / MEDIA_STORAGE_WINDOW_BYTES) * MEDIA_STORAGE_WINDOW_BYTES;
    (
        storage_start,
        storage_start
            .saturating_add(MEDIA_STORAGE_WINDOW_BYTES)
            .saturating_sub(1)
            .min(size.saturating_sub(1)),
    )
}

fn inclusive_range_len(start: u64, end: u64) -> Option<usize> {
    end.checked_sub(start)?.checked_add(1)?.try_into().ok()
}

fn begin_media_range(resource: &str, metadata: &BzzMetadata, start: u64) -> MediaRangeState {
    let key = media_state_key(resource, metadata);

    FETCH_CACHE.with_borrow_mut(|cache| {
        let state = cache.media_state_mut(&key);
        let previous_anchor = state.anchor_start;
        let previous_high_water = state.effective_high_water_end();
        let previous_request_start = state.last_request_start;
        let is_startup = previous_anchor.is_none();
        let is_request_jump = previous_anchor.is_some()
            && start.saturating_add(STREAM_SEEK_REQUEST_GAP_BYTES) < previous_request_start;
        let is_seek = previous_anchor.is_some()
            && (is_request_jump
                || start.saturating_add(STREAM_SEEK_KEEP_AHEAD_BYTES)
                    < previous_anchor.unwrap_or(0)
                || start as i64 > previous_high_water + STREAM_SEEK_KEEP_AHEAD_BYTES as i64);
        let is_prefetch_runaway = previous_anchor.is_some()
            && previous_high_water
                > start
                    .saturating_add(MEDIA_STARTUP_RESPONSE_BYTES)
                    .saturating_add(stream_prefetch_ahead_limit_bytes()) as i64;

        if is_seek || is_prefetch_runaway {
            state.generation = next_media_generation();
            state.completed_ranges.clear();
        }
        if is_startup || is_seek || is_prefetch_runaway {
            state.anchor_start = Some(start);
            state.high_water_end = start as i64 - 1;
            state.scheduled_high_water_end = start as i64 - 1;
        }

        state.last_request_start = start;
        state.last_touch = js_sys::Date::now();

        MediaRangeState {
            generation: state.generation,
            last_range_was_startup: is_startup || is_seek || is_prefetch_runaway,
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
    let response_bytes = if !streamable {
        MEDIA_STORAGE_WINDOW_BYTES
    } else if media_state
        .as_ref()
        .is_some_and(|state| state.last_range_was_startup)
    {
        MEDIA_STARTUP_RESPONSE_BYTES
    } else {
        STREAM_ACTIVE_RESPONSE_BUFFER_BYTES
    };

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

    if metadata.size == 0 || start > end || start >= metadata.size || end >= metadata.size {
        return;
    }
    let (_, window_end) = range_storage_window_for_start(end, metadata.size);
    let key = media_state_key(resource, metadata);
    with_current_media_state(&key, media_state.generation, |state| {
        state.mark_scheduled(window_end);
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
    with_current_media_state(&key, media_state.generation, |state| {
        state.mark_complete(start, end);
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
    with_current_media_state(&key, media_state.generation, |state| {
        state.mark_failure(start);
    });
}

async fn read_cached_range_with_retry(
    weeb3: &Arc<Weeb3>,
    resource: &str,
    metadata: &BzzMetadata,
    start: u64,
    end: u64,
    generation: u64,
) -> Result<Bytes, RangeReadError> {
    inclusive_range_len(start, end).ok_or_else(|| "invalid or oversized range".to_string())?;
    if metadata.size == 0 || start >= metadata.size || end >= metadata.size {
        return Err("range lies outside the resolved resource".into());
    }

    // Generation-zero reads already retry inside the Bee range retriever. A second
    // outer retry would start after the 210s timeout and outlive the service
    // worker's request budget, while its detached first attempt still drains.
    let mut result =
        read_cached_range(weeb3, resource, metadata, start, end, generation, None).await;
    if generation != 0 && result.is_err() {
        async_std::task::sleep(Duration::from_millis(RANGE_RETRY_DELAY_MS)).await;
        result = read_cached_range(weeb3, resource, metadata, start, end, generation, None).await;
    }
    result
}

async fn read_cached_range(
    weeb3: &Arc<Weeb3>,
    resource: &str,
    metadata: &BzzMetadata,
    start: u64,
    end: u64,
    generation: u64,
    current: Option<&dyn Fn() -> bool>,
) -> Result<Bytes, RangeReadError> {
    if current.is_some_and(|current| !current()) {
        return Err("range admission was retired".into());
    }
    if metadata.size == 0 || start > end || start >= metadata.size || end >= metadata.size {
        return Err("range lies outside the resolved resource".into());
    }
    let (window_start, window_end) = if current.is_some()
        && start == 0
        && end == metadata.size - 1
        && metadata.size <= HLS_BODY_MAX_BYTES
    {
        (start, end)
    } else {
        range_storage_window_for_start(start, metadata.size)
    };
    if end <= window_end {
        let body = read_range_window(
            weeb3,
            resource,
            metadata,
            window_start,
            window_end,
            generation,
            current.is_some(),
        )
        .await?;
        if current.is_some_and(|current| !current()) {
            return Err("range admission was retired".into());
        }
        let slice_start = usize::try_from(start - window_start)
            .map_err(|_| "storage window offset overflow".to_string())?;
        let slice_end = usize::try_from(end - window_start)
            .ok()
            .and_then(|end| end.checked_add(1))
            .ok_or_else(|| "storage window offset overflow".to_string())?;
        if body.get(slice_start..slice_end).is_none() {
            return Err("storage window did not contain the requested range".into());
        }
        return Ok(body.slice(slice_start..slice_end));
    }

    let body_len = inclusive_range_len(start, end)
        .ok_or_else(|| "requested range is too large".to_string())?;
    let mut body = vec![0; body_len];

    let windows =
        (start / MEDIA_STORAGE_WINDOW_BYTES..=end / MEDIA_STORAGE_WINDOW_BYTES).map(|index| {
            range_storage_window_for_start(index * MEDIA_STORAGE_WINDOW_BYTES, metadata.size)
        });
    let admitting = Cell::new(true);
    let responses = stream::iter(windows)
        .map(|(window_start, window_end)| {
            let admitting = &admitting;
            async move {
                let response = if !admitting.get() || current.is_some_and(|current| !current()) {
                    Err("range admission was retired".into())
                } else {
                    read_range_window(
                        weeb3,
                        resource,
                        metadata,
                        window_start,
                        window_end,
                        generation,
                        current.is_some(),
                    )
                    .await
                };
                if response.is_err() {
                    admitting.set(false);
                }
                (window_start, window_end, response)
            }
        })
        .buffer_unordered(MEDIA_RANGE_READ_MAX_PARALLEL);
    futures::pin_mut!(responses);
    while let Some((window_start, window_end, response)) = responses.next().await {
        if current.is_some_and(|current| !current()) {
            return Err("range admission was retired".into());
        }
        let storage_body = response?;
        let overlap_start = start.max(window_start);
        let overlap_end = end.min(window_end);
        let local_start = usize::try_from(overlap_start - window_start)
            .map_err(|_| "storage window offset overflow".to_string())?;
        let local_end = usize::try_from(overlap_end - window_start)
            .map_err(|_| "storage window offset overflow".to_string())?;
        let offset = usize::try_from(overlap_start - start)
            .map_err(|_| "storage window offset overflow".to_string())?;
        let bytes = &storage_body[local_start..=local_end];
        body[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    Ok(Bytes::from(body))
}

async fn read_range_window(
    weeb3: &Arc<Weeb3>,
    resource: &str,
    metadata: &BzzMetadata,
    start: u64,
    end: u64,
    generation: u64,
    cancel_when_unused: bool,
) -> Result<Bytes, RangeReadError> {
    if metadata.size == 0 || start > end || start >= metadata.size || end >= metadata.size {
        return Err("range window lies outside the resolved resource".into());
    }
    let identity = metadata_identity(resource, metadata);
    let cache_key = window_key(&identity, metadata.size, start, end);
    let (epoch, role) = FETCH_CACHE.with_borrow_mut(|cache| {
        (
            cache.epoch,
            cache.range_load_role(&cache_key, generation, cancel_when_unused),
        )
    });
    let (receiver, registration) = match role {
        RangeLoadRole::Cached(body) => return Ok(body),
        RangeLoadRole::Read(receiver, registration) => (receiver, registration),
        RangeLoadRole::Reject(error) => return Err(error.into()),
    };
    let waiter = RangeWaiterGuard {
        key: registration.key.clone(),
        flight_id: registration.flight_id,
        waiter_id: registration.waiter_id,
    };

    let timeout_ms = if generation > 0 {
        STREAM_RANGE_REQUEST_TIMEOUT_MS
    } else {
        RANGE_REQUEST_TIMEOUT_MS
    };
    if registration.leader {
        let load_id = registration.flight_id;
        let weeb3 = weeb3.clone();
        let metadata = metadata.clone();
        let media_key = format!("{identity}|{resource}");
        let leader_cache_key = cache_key;
        let leader_pending_key = registration.key;
        spawn_local(async move {
            let result = weeb3
                .request_resolved_range(
                    metadata,
                    start,
                    end,
                    (generation > 0).then(|| (media_key.clone(), generation)),
                    registration.shared.admission,
                )
                .await;
            let expected_len = inclusive_range_len(start, end);
            let load_result = match (result, expected_len) {
                (Some((body, _metadata)), Some(expected_len)) if body.len() == expected_len => {
                    Ok(Bytes::from(body))
                }
                (Some((body, _metadata)), Some(expected_len)) => Err(format!(
                    "weeb-3 returned {} bytes for {} byte range",
                    body.len(),
                    expected_len
                )),
                (Some(_), None) => Err("requested range is too large".to_string()),
                (None, _) => Err(format!("weeb-3 did not retrieve range {}-{}", start, end)),
            };

            if let Ok(body) = &load_result {
                FETCH_CACHE.with_borrow_mut(|cache| {
                    if cache.epoch == epoch {
                        cache.remember_range(
                            leader_cache_key,
                            body.clone(),
                            &media_key,
                            generation,
                        );
                    }
                });
            }

            FETCH_CACHE.with(|cache| {
                if let Some(pending) = cache
                    .borrow_mut()
                    .pending_ranges
                    .take(&leader_pending_key, load_id)
                {
                    finish_range_waiters(pending.waiters, load_result);
                }
            });
        });
    }

    drop(identity);
    match async_std::future::timeout(Duration::from_millis(timeout_ms), receiver).await {
        Ok(Ok(result)) => result.map_err(RangeReadError::from),
        Ok(Err(_)) => Err("range load was canceled".into()),
        Err(_) => {
            waiter.retain_owner();
            let error = format!("timed out retrieving range {}-{}", start, end);
            // A timeout retains the owner for retry; explicit retirement only closes
            // unused admission. Completion still requires the exact generation/load id.
            Err(RangeReadError::waiter_timeout(error))
        }
    }
}

pub(crate) async fn read_cached_hls_range(
    weeb3: &Arc<Weeb3>,
    reference: &str,
    size: u64,
    start: u64,
    end: u64,
    current: &dyn Fn() -> bool,
) -> Option<Bytes> {
    let metadata = BzzMetadata {
        data_reference: hex::decode(reference).ok()?,
        mime: "application/octet-stream".into(),
        size,
        etag: format!("\"{reference}\""),
        path: reference.into(),
        target_count: 1,
    };
    read_cached_range(weeb3, reference, &metadata, start, end, 0, Some(current))
        .await
        .ok()
}

pub(crate) fn forget_completed_reference_ranges(reference: &str) {
    FETCH_CACHE.with(|cache| cache.borrow_mut().forget_reference_ranges(reference));
}

fn spawn_prefetch_media_stages(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
    response_end: u64,
    generation: u64,
) {
    let key = media_state_key(&resource, &metadata);
    let should_spawn = FETCH_CACHE.with_borrow_mut(|cache| {
        let Some(state) = cache.media_states.get_mut(&key) else {
            return false;
        };
        if state.generation != generation {
            return false;
        }
        if state.prefetch_generation == generation {
            return false;
        }
        state.prefetch_generation = generation;
        true
    });

    if !should_spawn {
        return;
    }

    spawn_local(async move {
        prefetch_media_stages(
            &weeb3,
            &resource,
            &metadata,
            &key,
            response_end,
            generation,
        )
        .await;

        FETCH_CACHE.with_borrow_mut(|cache| {
            if let Some(state) = cache.media_states.get_mut(&key)
                && state.generation == generation
                && state.prefetch_generation == generation
            {
                state.prefetch_generation = 0;
            }
        });
    });
}

async fn prefetch_media_stages(
    weeb3: &Arc<Weeb3>,
    resource: &str,
    metadata: &BzzMetadata,
    media_key: &str,
    response_end: u64,
    generation: u64,
) {
    let ahead_limit_bytes = stream_prefetch_ahead_limit_bytes();
    let prefetch_limit_end = response_end
        .saturating_add(ahead_limit_bytes)
        .min(metadata.size.saturating_sub(1));

    for stage_target_bytes in media_prefetch_stage_targets(ahead_limit_bytes) {
        if !media_generation_current(media_key, generation) {
            return;
        }

        let current_end = media_high_water_end(media_key, generation)
            .unwrap_or(response_end)
            .max(response_end);
        if current_end >= prefetch_limit_end || current_end >= metadata.size.saturating_sub(1) {
            return;
        }

        let target_end = response_end
            .saturating_add(stage_target_bytes)
            .min(prefetch_limit_end)
            .min(metadata.size.saturating_sub(1));
        prefetch_media_windows(
            weeb3,
            resource,
            metadata,
            media_key,
            response_end,
            target_end,
            generation,
        )
        .await;
    }
}

async fn prefetch_media_windows(
    weeb3: &Arc<Weeb3>,
    resource: &str,
    metadata: &BzzMetadata,
    media_key: &str,
    response_end: u64,
    target_end: u64,
    generation: u64,
) {
    loop {
        if !media_generation_current(media_key, generation) {
            return;
        }

        let position = media_high_water_end(media_key, generation)
            .map(|end| end.saturating_add(1))
            .unwrap_or(response_end.saturating_add(1));
        if position > target_end {
            return;
        }

        let mut windows = Vec::new();
        let mut next = position;
        while next <= target_end && windows.len() < MEDIA_PREFETCH_MAX_PARALLEL {
            let window = range_storage_window_for_start(next, metadata.size);
            windows.push(window);
            with_current_media_state(media_key, generation, |state| {
                state.mark_scheduled(window.1);
            });
            next = window.1.saturating_add(1);
        }

        let loads = windows.iter().map(|(start, end)| {
            read_cached_range_with_retry(weeb3, resource, metadata, *start, *end, generation)
        });
        let results = join_all(loads).await;

        for (index, result) in results.into_iter().enumerate() {
            if !media_generation_current(media_key, generation) {
                return;
            }

            let (start, end) = windows[index];
            match result {
                Ok(bytes) if bytes.len() == (end - start + 1) as usize => {
                    with_current_media_state(media_key, generation, |state| {
                        state.mark_complete(start, end);
                    });
                }
                _ => {
                    with_current_media_state(media_key, generation, |state| {
                        state.mark_failure(start);
                    });
                    return;
                }
            }
        }

        async_std::task::sleep(Duration::from_millis(MEDIA_PREFETCH_BATCH_YIELD_MS)).await;
    }
}

fn media_generation_current(key: &str, generation: u64) -> bool {
    FETCH_CACHE.with(|cache| {
        cache
            .borrow()
            .media_states
            .get(key)
            .is_some_and(|state| state.generation == generation)
    })
}

fn media_high_water_end(key: &str, generation: u64) -> Option<u64> {
    FETCH_CACHE.with_borrow(|cache| {
        let state = cache.media_states.get(key)?;
        if state.generation != generation || state.high_water_end < 0 {
            return None;
        }
        Some(state.high_water_end as u64)
    })
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

fn canonical_bzz_resource(pathname: &str) -> Option<String> {
    let resource = route_resource(pathname, "bzz/")?.trim();
    let reference = resource.split('/').next().unwrap_or_default();
    if resource.is_empty() || !is_swarm_reference_hex(reference) {
        return None;
    }
    Some(decode_component(resource))
}

fn canonical_raw_resource(pathname: &str) -> Option<(&'static str, String)> {
    for (route, raw_type) in [("bytes/", "bytes"), ("chunks/", "chunk")] {
        if let Some(resource) = route_resource(pathname, route) {
            let resource = resource.trim();
            if resource.is_empty() {
                return None;
            }
            return Some((raw_type, decode_component(resource)));
        }
    }

    None
}

pub async fn try_render_streaming_player(
    weeb3: Rc<SharedNodeClient>,
    resource: String,
    metadata: BzzMetadata,
    view_generation: u64,
) -> bool {
    if !is_streamable_mime(&metadata.mime) {
        return false;
    }
    if !result_view_request_is_current(view_generation) {
        return true;
    }

    let Some(src) = canonical_bzz_url(&resource, &metadata.path, None) else {
        return false;
    };

    if !service_worker_controls_bzz_requests(&weeb3, "stream requests", || {
        result_view_request_is_current(view_generation)
    })
    .await
    {
        if !result_view_request_is_current(view_generation) {
            return true;
        }
        navigate_to_bzz_url(&src);
        return true;
    }
    if !result_view_request_is_current(view_generation) {
        return true;
    }

    let player = create_streaming_player(&metadata.mime, &src);
    if !replace_bzz_result_view(&weeb3, &player, view_generation) {
        return true;
    }
    let retry_state = Rc::new(RefCell::new(MediaRetryState::default()));
    install_playback_state_reset(&player, retry_state.clone());
    install_play_retries(&player, retry_state);
    start_streaming_player(&player);
    true
}

fn is_streamable_mime(mime: &str) -> bool {
    mime.starts_with("video/") || mime.starts_with("audio/")
}

pub(crate) fn replace_stream_result_view(new_element: &Element, view_generation: u64) -> bool {
    if !result_view_request_is_current(view_generation) {
        return false;
    }
    replace_result_view_contents(new_element);
    true
}

fn replace_bzz_result_view(
    weeb3: &SharedNodeClient,
    new_element: &Element,
    view_generation: u64,
) -> bool {
    if !result_view_request_is_current(view_generation) {
        return false;
    }
    crate::stream_hls::release_hls_for_bzz_view(weeb3);
    release_bzz_view();
    crate::interface::replace_result_view(new_element);
    true
}

pub(crate) fn replace_result_view_contents(new_element: &Element) {
    release_current_stream_view();
    crate::interface::replace_result_view(new_element);
}

pub(crate) fn release_current_stream_view() {
    crate::stream_hls::release_hls_view();
    release_bzz_view();
}

pub(crate) fn set_auxiliary_media_cache_bytes(bytes: u64) {
    AUXILIARY_MEDIA_CACHE_BYTES.with(|current| current.set(bytes));
    FETCH_CACHE.with(|cache| cache.borrow_mut().trim_ranges());
}

pub(crate) fn clear_completed_media_ranges() {
    FETCH_CACHE.with(|cache| cache.borrow_mut().clear_completed_ranges());
}

fn release_bzz_view() {
    MEDIA_ELEMENT_CALLBACKS.with(|callbacks| callbacks.borrow_mut().clear());
}

fn create_streaming_player(mime: &str, src: &str) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();
    let is_video = mime.starts_with("video/");
    let tag = if is_video { "video" } else { "audio" };
    let player = document
        .create_element(tag)
        .unwrap()
        .dyn_into::<HtmlMediaElement>()
        .unwrap();

    let _ = player.set_attribute("controls", "");
    let _ = player.set_attribute("preload", "metadata");
    if is_video {
        let _ = player.set_attribute("playsinline", "");
    }
    player.set_muted(false);
    player.set_default_muted(false);
    player.set_volume(1.0);
    player.set_autoplay(true);
    player.set_src(src);
    let _ = player.set_attribute("style", "width:90%;max-height:75vh;");

    player.into()
}

fn start_streaming_player(player: &Element) {
    if let Some(player) = player.dyn_ref::<HtmlMediaElement>() {
        let _ = player.play();
    }
}

fn retain_media_element_callback(
    target: &Element,
    event_names: &'static [&'static str],
    callback: Closure<dyn FnMut()>,
) {
    for event_name in event_names {
        let _ =
            target.add_event_listener_with_callback(event_name, callback.as_ref().unchecked_ref());
    }
    MEDIA_ELEMENT_CALLBACKS.with(|callbacks| {
        callbacks.borrow_mut().push(MediaElementCallback {
            target: target.clone(),
            event_names,
            callback,
        });
    });
}

fn install_playback_state_reset(player: &Element, retry_state: Rc<RefCell<MediaRetryState>>) {
    let callback = Closure::<dyn FnMut()>::new(move || {
        *retry_state.borrow_mut() = MediaRetryState::default();
    });

    retain_media_element_callback(player, &["playing"], callback);
}

fn install_play_retries(player: &Element, retry_state: Rc<RefCell<MediaRetryState>>) {
    let player_for_callback = player.clone();
    let ready_retry_state = retry_state.clone();
    let callback = Closure::<dyn FnMut()>::new(move || {
        if !ready_retry_state.borrow().retrying {
            return;
        }
        apply_media_retry_time(&player_for_callback, &ready_retry_state);
        start_streaming_player(&player_for_callback);
    });
    retain_media_element_callback(
        player,
        &["loadedmetadata", "loadeddata", "canplay"],
        callback,
    );

    {
        let player_for_callback = player.clone();
        let retry_state = retry_state.clone();
        let callback = Closure::<dyn FnMut()>::new(move || {
            {
                let mut state = retry_state.borrow_mut();
                state.errored = true;
                state.retrying = false;
            }
            schedule_media_retry(player_for_callback.clone(), retry_state.clone());
        });
        retain_media_element_callback(player, &["error"], callback);
    }

    let player_for_callback = player.clone();
    let callback = Closure::<dyn FnMut()>::new(move || {
        if !retry_state.borrow().errored {
            return;
        }

        remember_media_retry_time(&player_for_callback, &retry_state);
        if retry_state.borrow().retrying {
            return;
        }

        start_media_retry(&player_for_callback, false, &retry_state);
    });
    retain_media_element_callback(
        player,
        &[
            "play",
            "seeking",
            "seeked",
            "click",
            "pointerdown",
            "mousedown",
            "touchstart",
            "keydown",
        ],
        callback,
    );
}

fn schedule_media_retry(player: Element, retry_state: Rc<RefCell<MediaRetryState>>) {
    if !player.is_connected() {
        return;
    }
    let delay_ms = {
        let mut state = retry_state.borrow_mut();
        if !state.errored || state.scheduled {
            return;
        }
        let Some(delay_ms) = MEDIA_RETRY_DELAYS_MS.get(state.attempt).copied() else {
            return;
        };
        state.scheduled = true;
        delay_ms
    };

    spawn_local(async move {
        async_std::task::sleep(Duration::from_millis(delay_ms)).await;
        if !player.is_connected() {
            return;
        }
        retry_state.borrow_mut().scheduled = false;
        start_media_retry(&player, true, &retry_state);
    });
}

fn start_media_retry(
    player: &Element,
    advance_attempt: bool,
    retry_state: &Rc<RefCell<MediaRetryState>>,
) {
    if !player.is_connected() {
        return;
    }
    remember_media_retry_time(player, retry_state);
    {
        let mut state = retry_state.borrow_mut();
        if !state.errored || state.retrying {
            return;
        }
        state.attempt = if advance_attempt {
            state.attempt.saturating_add(1)
        } else {
            0
        };
        state.retrying = true;
        state.scheduled = false;
    }
    if let Some(player) = player.dyn_ref::<HtmlMediaElement>() {
        player.load();
    }
    apply_media_retry_time(player, retry_state);
    start_streaming_player(player);
}

fn remember_media_retry_time(player: &Element, retry_state: &RefCell<MediaRetryState>) {
    let Some(time) = media_current_time(player) else {
        return;
    };
    if time <= 0.0 {
        return;
    }
    retry_state.borrow_mut().playback_time = Some(time);
}

fn apply_media_retry_time(player: &Element, retry_state: &RefCell<MediaRetryState>) {
    let Some(time) = retry_state.borrow().playback_time else {
        return;
    };
    if let Some(player) = player.dyn_ref::<HtmlMediaElement>() {
        player.set_current_time(time);
    }
}

fn media_current_time(player: &Element) -> Option<f64> {
    player
        .dyn_ref::<HtmlMediaElement>()
        .map(HtmlMediaElement::current_time)
        .filter(|time| time.is_finite())
}

fn navigate_to_bzz_url(src: &str) {
    if let Some(location) = web_sys::window().map(|window| window.location()) {
        let _ = location.assign(src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn metadata_cache_hits_and_replacement_preserve_the_entry_limit_and_recency() {
        let metadata = BzzMetadata {
            data_reference: vec![0; 32],
            mime: "video/mp4".into(),
            size: 10,
            etag: "metadata-cache".into(),
            path: String::new(),
            target_count: 1,
        };
        let mut cache = FetchCache::default();
        for index in 0..METADATA_CACHE_MAX_ENTRIES {
            cache.remember_metadata(index.to_string(), metadata.clone());
        }
        assert!(cache.metadata("0").is_some());
        let mut replacement = metadata.clone();
        replacement.size = 20;
        cache.remember_metadata("1".into(), replacement);
        cache.remember_metadata("next".into(), metadata);
        assert_eq!(cache.metadata.len(), METADATA_CACHE_MAX_ENTRIES);
        assert!(cache.metadata("0").is_some());
        assert_eq!(cache.metadata("1").unwrap().size, 20);
        assert!(cache.metadata("2").is_none());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn range_cache_preserves_recency_byte_accounting_and_invalidation() {
        let previous = AUXILIARY_MEDIA_CACHE_BYTES
            .with(|bytes| bytes.replace(media_cache_max_bytes().saturating_sub(6)));
        let mut cache = FetchCache::default();
        let first = Bytes::from_static(b"aa");
        cache.remember_range("one|0".into(), first.clone(), "", 0);
        cache.remember_range("two|0".into(), Bytes::from_static(b"bbb"), "", 0);
        assert_eq!(cache.range("one|0").unwrap().as_ptr(), first.as_ptr());
        cache.remember_range("three|0".into(), Bytes::from_static(b"cc"), "", 0);
        assert_eq!(cache.range_bytes, 4);
        assert!(cache.range("two|0").is_none());

        cache.remember_range("one|0".into(), Bytes::from_static(b"new"), "", 0);
        assert_eq!(cache.range_bytes, 5);
        cache
            .media_states
            .insert("media".into(), MediaState::new(3));
        cache.remember_range("one|0".into(), Bytes::from_static(b"x"), "media", 2);
        assert_eq!(cache.range("one|0").unwrap().as_ref(), b"new");
        cache.remember_range("one|0".into(), first, "media", 3);
        assert_eq!(cache.range_bytes, 4);
        cache.forget_reference_ranges("one");
        assert!(cache.range("one|0").is_none());
        assert_eq!(cache.range_bytes, 2);
        cache.clear_completed_ranges();
        assert_eq!(
            (cache.range_bytes, cache.ranges.len(), cache.epoch),
            (0, 0, 1)
        );
        AUXILIARY_MEDIA_CACHE_BYTES.with(|bytes| bytes.set(previous));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn first_unaligned_nonzero_range_advances_prefetch() {
        let resource = "nonzero-prefetch-start";
        let metadata = BzzMetadata {
            data_reference: vec![0; 32],
            mime: "video/mp4".into(),
            size: 32 * MIB_BYTES,
            etag: resource.into(),
            path: resource.into(),
            target_count: 1,
        };
        let start = 10 * MIB_BYTES + 1;
        let end = start + MEDIA_STARTUP_RESPONSE_BYTES - 1;
        let range = begin_media_range(resource, &metadata, start);
        let key = media_state_key(resource, &metadata);
        for (start, end) in [
            (start, end),
            range_storage_window_for_start(end + 1, metadata.size),
        ] {
            with_current_media_state(&key, range.generation, |state| {
                state.mark_complete(start, end);
            });
            assert_eq!(media_high_water_end(&key, range.generation), Some(end));
        }
    }
}
