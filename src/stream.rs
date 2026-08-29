use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, VecDeque},
    rc::Rc,
    time::Duration,
};

use async_std::sync::Arc;
use bytes::Bytes;
use js_sys::{Array, Object, Reflect};
use libp2p::futures::future::join_all;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlMediaElement};

use crate::{
    Weeb3,
    bzz_stream::{BzzMetadata, bzz_reference_hex, normalize_bzz_path},
    interface::service_worker_controls_bzz_requests,
    mpsc,
    network_profile::{NetworkMode, active_profile},
    retrieval_conventions::{
        PendingGenerationRelation, next_nonzero_generation, pending_generation_relation,
    },
    shared_runtime::SharedNodeClient,
    stream_conventions::{
        MEDIA_PREFETCH_BATCH_YIELD_MS, MEDIA_PREFETCH_MAX_PARALLEL, MEDIA_STARTUP_RESPONSE_BYTES,
        MEDIA_STORAGE_WINDOW_BYTES, MIB_BYTES, decode_component, if_none_match_matches,
        if_range_allows_range, immutable_metadata_identity, media_cache_budget_bytes,
        media_prefetch_ahead_limit_bytes, media_prefetch_stage_targets, parse_single_range,
        streaming_route_path, window_key,
    },
    worker_protocol::{bytes_to_js, set as set_js, string_property},
};

const STREAM_RESPONSE_BUFFER_BYTES: u64 = MEDIA_STARTUP_RESPONSE_BYTES;
const STREAM_ACTIVE_RESPONSE_BUFFER_BYTES: u64 = 2 * MIB_BYTES;
const STREAM_SEEK_KEEP_AHEAD_BYTES: u64 = 16 * MIB_BYTES;
const STREAM_SEEK_RESET_GAP_BYTES: u64 = STREAM_SEEK_KEEP_AHEAD_BYTES;
const STREAM_SEEK_REQUEST_GAP_BYTES: u64 = 6 * MIB_BYTES;
const METADATA_CACHE_MAX_ENTRIES: usize = 1024;
const MEDIA_STREAM_STATE_MAX_ENTRIES: usize = 64;
const RANGE_SINGLEFLIGHT_MAX_LOADS: usize = 256;
const RANGE_SINGLEFLIGHT_MAX_WAITERS: usize = 64;
const RANGE_RETRY_DELAY_MS: u64 = 700;
const RANGE_REQUEST_TIMEOUT_MS: u64 = 210_000;
const STREAM_RANGE_RETRY_COUNT: usize = 1;
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
    metadata_order: VecDeque<String>,
    metadata: HashMap<String, BzzMetadata>,
    range_order: VecDeque<String>,
    ranges: HashMap<String, Bytes>,
    pending_ranges: HashMap<String, PendingRange>,
    next_range_load_id: u64,
    range_bytes: u64,
    media_states: HashMap<String, MediaState>,
}

impl FetchCache {
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

    fn range(&mut self, key: &str) -> Option<Bytes> {
        let body = self.ranges.get(key)?.clone();
        self.range_order.retain(|cached_key| cached_key != key);
        self.range_order.push_back(key.to_string());
        Some(body)
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
        if let Some(old) = self.ranges.remove(&key) {
            self.range_bytes = self.range_bytes.saturating_sub(old.len() as u64);
        }
        self.range_order.retain(|cached_key| cached_key != &key);
        self.range_order.push_back(key.clone());
        self.ranges.insert(key, body);
        self.range_bytes = self.range_bytes.saturating_add(body_len);
        self.trim_ranges();
    }

    fn clear_completed_ranges(&mut self) {
        for state in self.media_states.values_mut() {
            state.reset();
        }
        self.range_order.clear();
        self.ranges.clear();
        self.range_bytes = 0;
    }

    fn range_load_role(
        &mut self,
        cache_key: &str,
        pending_key: &str,
        generation: u64,
    ) -> RangeLoadRole {
        if let Some(body) = self.range(cache_key) {
            return RangeLoadRole::Cached(body);
        }

        if let Some(pending) = self.pending_ranges.get_mut(pending_key) {
            match pending_generation_relation(pending.generation, generation) {
                PendingGenerationRelation::Join => {
                    pending.waiters.retain(|waiter| !waiter.is_closed());
                    if pending.waiters.len() >= RANGE_SINGLEFLIGHT_MAX_WAITERS {
                        return RangeLoadRole::Reject(
                            "range already has too many waiting requests".to_string(),
                        );
                    }
                    let (sender, receiver) = mpsc::bounded(1);
                    pending.waiters.push(sender);
                    return RangeLoadRole::Wait(receiver);
                }
                PendingGenerationRelation::RejectStale => {
                    return RangeLoadRole::Reject("stale range generation".to_string());
                }
                PendingGenerationRelation::Replace => {}
            }
        }
        if let Some(stale) = self.pending_ranges.remove(pending_key) {
            stale.finish(Err("stale range generation replaced".to_string()));
        }
        if self.pending_ranges.len() >= RANGE_SINGLEFLIGHT_MAX_LOADS {
            return RangeLoadRole::Reject("too many range loads are already pending".to_string());
        }

        let (sender, receiver) = mpsc::bounded(1);
        self.next_range_load_id = next_nonzero_generation(self.next_range_load_id);
        let load_id = self.next_range_load_id;
        self.pending_ranges.insert(
            pending_key.to_string(),
            PendingRange {
                generation,
                load_id,
                waiters: vec![sender],
            },
        );
        RangeLoadRole::Lead(receiver, load_id)
    }

    fn finish_pending_range(
        &mut self,
        key: &str,
        generation: u64,
        load_id: u64,
        result: Result<Bytes, String>,
    ) {
        if !self
            .pending_ranges
            .get(key)
            .is_some_and(|pending| pending.generation == generation && pending.load_id == load_id)
        {
            return;
        }
        if let Some(pending) = self.pending_ranges.remove(key) {
            pending.finish(result);
        }
    }

    fn trim_ranges(&mut self) {
        let max_bytes = range_cache_capacity_bytes();
        while self.range_bytes > max_bytes {
            let Some(oldest) = self.range_order.pop_front() else {
                break;
            };
            if let Some(range) = self.ranges.remove(&oldest) {
                self.range_bytes = self.range_bytes.saturating_sub(range.len() as u64);
            }
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
                .filter(|(key, state)| key.as_str() != active_key && !state.prefetch_running)
                .min_by(|left, right| left.1.last_touch.total_cmp(&right.1.last_touch))
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.media_states.remove(&oldest);
        }
    }
}

struct PendingRange {
    generation: u64,
    load_id: u64,
    waiters: Vec<mpsc::Sender<Result<Bytes, String>>>,
}

impl PendingRange {
    fn finish(self, result: Result<Bytes, String>) {
        let mut waiters = self
            .waiters
            .into_iter()
            .filter(|waiter| !waiter.is_closed())
            .peekable();
        while let Some(waiter) = waiters.next() {
            if waiters.peek().is_none() {
                let _ = waiter.try_send(result);
                return;
            }
            let _ = waiter.try_send(result.clone());
        }
    }
}

enum RangeLoadRole {
    Cached(Bytes),
    Wait(mpsc::Receiver<Result<Bytes, String>>),
    Lead(mpsc::Receiver<Result<Bytes, String>>, u64),
    Reject(String),
}

#[derive(Debug)]
struct RangeReadError {
    message: String,
    waiter_timed_out: bool,
}

impl RangeReadError {
    fn terminal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            waiter_timed_out: false,
        }
    }

    fn waiter_timeout(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            waiter_timed_out: true,
        }
    }
}

impl From<String> for RangeReadError {
    fn from(message: String) -> Self {
        Self::terminal(message)
    }
}

impl From<&str> for RangeReadError {
    fn from(message: &str) -> Self {
        Self::terminal(message)
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
    prefetch_running: bool,
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
            prefetch_running: false,
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
    body: Option<FetchBody>,
    error: String,
    stream: bool,
}

enum FetchBody {
    Owned(Vec<u8>),
    Shared(Bytes),
}

impl FetchResponse {
    pub(crate) fn ok(status: u16, headers: Vec<(String, String)>, body: Option<Vec<u8>>) -> Self {
        Self {
            ok: true,
            status,
            headers,
            body: body.map(FetchBody::Owned),
            error: String::new(),
            stream: false,
        }
    }

    pub(crate) fn ok_shared(status: u16, headers: Vec<(String, String)>, body: Bytes) -> Self {
        Self {
            ok: true,
            status,
            headers,
            body: Some(FetchBody::Shared(body)),
            error: String::new(),
            stream: false,
        }
    }

    pub(crate) fn ok_shared_slice(
        status: u16,
        headers: Vec<(String, String)>,
        body: Bytes,
        start: usize,
        end: usize,
    ) -> Option<Self> {
        body.get(start..end)?;
        Some(Self {
            ok: true,
            status,
            headers,
            body: Some(FetchBody::Shared(body.slice(start..end))),
            error: String::new(),
            stream: false,
        })
    }

    pub(crate) fn stream(status: u16, headers: Vec<(String, String)>) -> Self {
        Self {
            ok: true,
            status,
            headers,
            body: None,
            error: String::new(),
            stream: true,
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
            let bytes: &[u8] = match &body {
                FetchBody::Owned(body) => body,
                FetchBody::Shared(body) => body,
            };
            set_js(&resp, "body", bytes_to_js(bytes).into());
        }

        resp
    }
}

/// SharedWorker equivalent of the page message bridge.
pub(crate) async fn service_worker_message_response(
    obj: &js_sys::Object,
    weeb3: Arc<Weeb3>,
) -> Option<Object> {
    if string_property(obj.as_ref(), "type").as_deref() != Some("WEEB3_FETCH_REQUEST") {
        return None;
    }
    let url = string_property(obj.as_ref(), "url").unwrap_or_default();
    let mut method = string_property(obj.as_ref(), "method").unwrap_or_else(|| "GET".into());
    method.make_ascii_uppercase();
    let range = string_property(obj.as_ref(), "range").filter(|value| !value.is_empty());
    let if_none_match =
        string_property(obj.as_ref(), "ifNoneMatch").filter(|value| !value.trim().is_empty());
    let if_range =
        string_property(obj.as_ref(), "ifRange").filter(|value| !value.trim().is_empty());
    let stream_token =
        string_property(obj.as_ref(), "streamToken").filter(|value| !value.trim().is_empty());
    Some(
        fetch_request_response(
            weeb3,
            url,
            method,
            range,
            if_none_match,
            if_range,
            stream_token,
        )
        .await
        .into_js(),
    )
}

async fn fetch_request_response(
    weeb3: Arc<Weeb3>,
    url: String,
    method: String,
    range: Option<String>,
    if_none_match: Option<String>,
    if_range: Option<String>,
    stream_token: Option<String>,
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
        if_range.as_deref(),
        stream_token.as_deref(),
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
    if !is_swarm_reference(reference) {
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

    let streamable = is_streamable_mime(&metadata.mime) && metadata.size > 0;
    let range_validator_mismatch =
        range.is_some() && !if_range_allows_range(if_range.as_deref(), &metadata.etag);
    if range_validator_mismatch {
        // A failed If-Range condition changes the request into a complete 200
        // representation. Large bodies retain the ordered range stream, while
        // already-dispatched retrieval/accounting work keeps its lifecycle.
        if should_inline_non_streamable_response(&metadata) {
            return full_bzz_response(weeb3, resource, metadata).await;
        }
        return FetchResponse::stream(200, metadata_headers(&metadata, metadata.size));
    }

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

    if let Some(media_state) = &media_state {
        mark_media_range_complete(&resource, &metadata, start, end, media_state);
        spawn_prefetch_media_stages(
            weeb3.clone(),
            resource.clone(),
            metadata.clone(),
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
        FetchResponse::ok_shared(206, headers, bytes)
    } else {
        FetchResponse::ok_shared(200, headers, bytes)
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
    FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(state) = cache
            .media_states
            .get_mut(key)
            .filter(|state| state.generation == generation)
        {
            update(state);
        }
    });
}

fn range_cache_key(resource: &str, metadata: &BzzMetadata, start: u64, end: u64) -> String {
    window_key(
        &metadata_identity(resource, metadata),
        metadata.size,
        start,
        end,
    )
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

fn range_storage_windows_for_span(start: u64, end: u64, size: u64) -> Vec<(u64, u64)> {
    if size == 0 || start > end || start >= size || end >= size {
        return Vec::new();
    }

    let mut windows = Vec::new();
    let mut position = start;

    while position <= end {
        let window = range_storage_window_for_start(position, size);
        if window.0 > window.1 || window.1 < position {
            return Vec::new();
        }
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
            && start.saturating_add(STREAM_SEEK_REQUEST_GAP_BYTES) < previous_request_start;
        let is_seek = previous_anchor.is_some()
            && (is_request_jump
                || start.saturating_add(STREAM_SEEK_RESET_GAP_BYTES)
                    < previous_anchor.unwrap_or(0)
                || start as i64 > previous_high_water + STREAM_SEEK_RESET_GAP_BYTES as i64);
        let is_prefetch_runaway = previous_anchor.is_some()
            && previous_high_water
                > start
                    .saturating_add(STREAM_RESPONSE_BUFFER_BYTES)
                    .saturating_add(stream_prefetch_ahead_limit_bytes()) as i64;

        if is_seek || is_prefetch_runaway {
            state.generation = next_media_generation();
            state.anchor_start = Some(start);
            state.high_water_end = start as i64 - 1;
            state.scheduled_high_water_end = start as i64 - 1;
            state.completed_ranges.clear();
        } else if is_startup {
            state.anchor_start = Some(start);
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
        STREAM_RESPONSE_BUFFER_BYTES
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

    let windows = range_storage_windows_for_span(start, end, metadata.size);
    let key = media_state_key(resource, metadata);
    with_current_media_state(&key, media_state.generation, |state| {
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
    let expected_len =
        inclusive_range_len(start, end).ok_or_else(|| "invalid or oversized range".to_string())?;
    if metadata.size == 0 || start >= metadata.size || end >= metadata.size {
        return Err("range lies outside the resolved resource".into());
    }

    let mut last_error = RangeReadError::terminal("range retry did not run");
    // Generation-zero reads already retry inside the Bee range retriever. A second
    // outer retry would start after the 210s timeout and outlive the service
    // worker's request budget, while its detached first attempt still drains.
    let retry_count = if generation == 0 {
        0
    } else {
        STREAM_RANGE_RETRY_COUNT
    };

    for attempt in 0..=retry_count {
        match read_cached_range(weeb3, resource, metadata, start, end, generation).await {
            Ok(bytes) if bytes.len() == expected_len => return Ok(bytes),
            Ok(bytes) => {
                last_error = RangeReadError::terminal(format!(
                    "weeb-3 returned {} bytes for {} byte range",
                    bytes.len(),
                    expected_len
                ));
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
    weeb3: &Arc<Weeb3>,
    resource: &str,
    metadata: &BzzMetadata,
    start: u64,
    end: u64,
    generation: u64,
) -> Result<Bytes, RangeReadError> {
    if metadata.size == 0 || start > end || start >= metadata.size || end >= metadata.size {
        return Err("range lies outside the resolved resource".into());
    }
    let windows = range_storage_windows_for_span(start, end, metadata.size);
    if windows.is_empty() {
        return Err("range did not produce storage windows".into());
    }
    if windows.len() == 1 && windows[0] == (start, end) {
        return read_range_window(weeb3, resource, metadata, start, end, generation).await;
    }

    let body_len = inclusive_range_len(start, end)
        .ok_or_else(|| "requested range is too large".to_string())?;
    let mut body = vec![0; body_len];

    for batch in windows.chunks(MEDIA_PREFETCH_MAX_PARALLEL) {
        let loads = batch.iter().map(|(window_start, window_end)| {
            read_range_window(
                weeb3,
                resource,
                metadata,
                *window_start,
                *window_end,
                generation,
            )
        });
        let responses = join_all(loads).await;

        for (index, response) in responses.into_iter().enumerate() {
            let (window_start, window_end) = batch[index];
            let storage_body = response?;
            let expected_len = inclusive_range_len(window_start, window_end)
                .ok_or_else(|| "storage window is too large".to_string())?;
            if storage_body.len() != expected_len {
                return Err(RangeReadError::terminal(format!(
                    "weeb-3 returned {} bytes for {} byte storage window",
                    storage_body.len(),
                    expected_len
                )));
            }

            let overlap_start = start.max(window_start);
            let overlap_end = end.min(window_end);
            let local_start = usize::try_from(overlap_start - window_start)
                .map_err(|_| "storage window offset overflow".to_string())?;
            let local_end = usize::try_from(overlap_end - window_start)
                .map_err(|_| "storage window offset overflow".to_string())?;
            let destination_start = usize::try_from(overlap_start - start)
                .map_err(|_| "range destination offset overflow".to_string())?;
            let slice = &storage_body[local_start..=local_end];
            let destination_end = destination_start + slice.len();
            body[destination_start..destination_end].copy_from_slice(slice);
        }
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
) -> Result<Bytes, RangeReadError> {
    if metadata.size == 0 || start > end || start >= metadata.size || end >= metadata.size {
        return Err("range window lies outside the resolved resource".into());
    }
    let cache_key = range_cache_key(resource, metadata, start, end);
    let pending_key = pending_range_key(&cache_key, generation);
    let (receiver, leader_load_id) = match FETCH_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .range_load_role(&cache_key, &pending_key, generation)
    }) {
        RangeLoadRole::Cached(body) => return Ok(body),
        RangeLoadRole::Wait(receiver) => (receiver, None),
        RangeLoadRole::Lead(receiver, load_id) => (receiver, Some(load_id)),
        RangeLoadRole::Reject(error) => return Err(error.into()),
    };

    let timeout_ms = if generation > 0 {
        STREAM_RANGE_REQUEST_TIMEOUT_MS
    } else {
        RANGE_REQUEST_TIMEOUT_MS
    };
    if let Some(load_id) = leader_load_id {
        let weeb3 = weeb3.clone();
        let metadata = metadata.clone();
        let media_key = media_state_key(resource, &metadata);
        let leader_cache_key = cache_key;
        let leader_pending_key = pending_key;
        spawn_local(async move {
            let result = if generation > 0 {
                weeb3
                    .acquire_resolved_stream_range(
                        metadata,
                        start,
                        end,
                        media_key.clone(),
                        generation,
                    )
                    .await
            } else {
                weeb3.acquire_resolved_range(metadata, start, end).await
            };
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
                FETCH_CACHE.with(|cache| {
                    cache.borrow_mut().remember_range(
                        leader_cache_key,
                        body.clone(),
                        &media_key,
                        generation,
                    );
                });
            }

            FETCH_CACHE.with(|cache| {
                cache.borrow_mut().finish_pending_range(
                    &leader_pending_key,
                    generation,
                    load_id,
                    load_result,
                );
            });
        });
    }

    match async_std::future::timeout(Duration::from_millis(timeout_ms), receiver.recv()).await {
        Ok(Ok(result)) => result.map_err(RangeReadError::terminal),
        Ok(Err(_)) => Err(RangeReadError::terminal("range load was canceled")),
        Err(_) => {
            let error = format!("timed out retrieving range {}-{}", start, end);
            // Keep the shared slot while its detached transport drains. This waiter
            // closes when we return, and a retry joins the same load instead of
            // launching duplicate chunk/accounting work. Completion still removes
            // only the exact generation/load id, so a seek may replace it safely.
            Err(RangeReadError::waiter_timeout(error))
        }
    }
}

fn spawn_prefetch_media_stages(
    weeb3: Arc<Weeb3>,
    resource: String,
    metadata: BzzMetadata,
    response_end: u64,
    requested_end: u64,
    generation: u64,
) {
    let key = media_state_key(&resource, &metadata);
    let should_spawn = FETCH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let Some(state) = cache.media_states.get_mut(&key) else {
            return false;
        };
        if state.generation != generation {
            return false;
        }
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
            &weeb3,
            &resource,
            &metadata,
            &key,
            response_end,
            requested_end,
            generation,
        )
        .await;

        FETCH_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some(state) = cache.media_states.get_mut(&key)
                && state.generation == generation
                && state.prefetch_generation == generation
            {
                state.prefetch_running = false;
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
    requested_end: u64,
    generation: u64,
) {
    let ahead_limit_bytes = stream_prefetch_ahead_limit_bytes();
    let prefetch_limit_end = requested_end
        .min(response_end.saturating_add(ahead_limit_bytes))
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
            mark_media_window_scheduled(media_key, window.1, generation);
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
                    mark_media_window_complete(media_key, start, end, generation);
                }
                _ => {
                    mark_media_window_failure(media_key, start, generation);
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
    FETCH_CACHE.with(|cache| {
        let cache = cache.borrow();
        let state = cache.media_states.get(key)?;
        if state.generation != generation || state.high_water_end < 0 {
            return None;
        }
        Some(state.high_water_end as u64)
    })
}

fn mark_media_window_scheduled(key: &str, end: u64, generation: u64) {
    with_current_media_state(key, generation, |state| {
        state.mark_scheduled(end);
    });
}

fn mark_media_window_complete(key: &str, start: u64, end: u64, generation: u64) {
    with_current_media_state(key, generation, |state| {
        state.mark_complete(start, end);
    });
}

fn mark_media_window_failure(key: &str, start: u64, generation: u64) {
    with_current_media_state(key, generation, |state| {
        state.mark_failure(start);
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

fn canonical_bzz_resource(pathname: &str) -> Option<String> {
    let resource = route_resource(pathname, "bzz/")?.trim();
    let reference = resource.split('/').next().unwrap_or_default();
    if resource.is_empty() || !is_swarm_reference(reference) {
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

fn route_resource<'a>(pathname: &'a str, route: &str) -> Option<&'a str> {
    let path = pathname.strip_prefix("/weeb-3/")?;
    let path = path
        .strip_prefix("mainnet/")
        .or_else(|| path.strip_prefix("testnet/"))
        .unwrap_or(path);
    path.strip_prefix(route)
}

fn is_swarm_reference(reference: &str) -> bool {
    (reference.len() == 64 || reference.len() == 128)
        && reference.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
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

    let Some(src) = canonical_bzz_url(&resource, &metadata) else {
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
        NetworkMode::Mainnet => streaming_route_path("bzz"),
        NetworkMode::Testnet => streaming_route_path("testnet/bzz"),
    };

    if path.is_empty() || path.starts_with("unknown") || path == "not found" {
        Some(format!("{}/{}", prefix, reference))
    } else {
        Some(format!("{}/{}/{}", prefix, reference, path))
    }
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
    replace_result_view_dom(new_element);
    true
}

pub(crate) fn replace_result_view_contents(new_element: &Element) {
    release_current_stream_view();
    replace_result_view_dom(new_element);
}

fn replace_result_view_dom(new_element: &Element) {
    crate::interface::replace_result_view(new_element);
}

pub(crate) fn release_current_stream_view() {
    crate::stream_hls::release_hls_view();
    release_bzz_view();
}

pub(crate) fn completed_media_range_bytes() -> u64 {
    FETCH_CACHE.with(|cache| cache.borrow().range_bytes)
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
