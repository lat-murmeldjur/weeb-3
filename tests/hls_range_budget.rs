const STREAM: &str = include_str!("../src/stream.rs");
const HLS_CORE: &str = include_str!("../src/stream_hls.rs");
const HLS_RUNTIME: &str = include_str!("../src/stream_hls/runtime.rs");

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .and_then(|(_, tail)| tail.split_once(end))
        .map(|(body, _)| body)
        .unwrap_or_else(|| panic!("missing source section between {start:?} and {end:?}"))
}

#[test]
fn ordinary_media_keeps_its_existing_range_retry_policy() {
    let ordinary = section(
        STREAM,
        "async fn read_cached_range_with_retry(",
        "async fn read_cached_range(",
    );
    assert_eq!(ordinary.matches("read_cached_range(").count(), 1);
    assert!(ordinary.contains("STREAM_RANGE_RETRY_COUNT"));
    assert!(ordinary.contains("RANGE_RETRY_DELAY_MS"));
}

#[test]
fn whole_hls_bodies_are_singleflight_and_share_the_bounded_range_budget() {
    assert!(HLS_RUNTIME.contains("const HLS_BODY_CACHE_MAX_BYTES: u64 = 32 * 1024 * 1024;"));
    assert!(HLS_RUNTIME.contains("const HLS_BODY_MAX_BYTES: u64 = 96 * 1024 * 1024;"));

    let cache = section(HLS_RUNTIME, "struct BodyCache {", "struct FeedSession");
    assert!(cache.contains("bodies: HashMap<String, Bytes>"));
    assert!(cache.contains("body_order: VecDeque<String>"));
    assert!(cache.contains("pending_bodies: HashMap<String, PendingBody>"));
    assert!(!cache.contains("generation: Option<u64>"));
    assert!(cache.contains("waiters: Vec<mpsc::Sender<Option<Bytes>>>"));
    assert!(cache.contains("enum BodyLoad"));

    let admission = section(cache, "fn body_load(", "fn body_cached(");
    assert!(admission.contains("BodyLoad::Cached(body.clone())"));
    assert!(admission.contains("self.pending_bodies.get_mut(reference)"));
    assert!(admission.contains("pending.waiters.push(sender)"));
    assert!(admission.contains("mpsc::bounded(1)"));
    assert!(admission.contains("BodyLoad::Wait(receiver)"));
    assert!(admission.contains("BodyLoad::Lead"));

    let settlement = section(cache, "fn finish_body(", "fn trim(");
    assert!(settlement.contains(".pending_bodies"));
    assert!(settlement.contains(".remove(&reference)"));
    assert!(settlement.contains("self.bodies.insert(reference.clone(), body.clone())"));
    assert!(settlement.contains("self.trim()"));
    assert!(settlement.contains("waiter.try_send(delivered.clone())"));

    let trim = section(cache, "fn trim(", "fn clear(");
    assert!(trim.contains(".min(HLS_BODY_CACHE_MAX_BYTES)"));
    assert!(trim.contains("self.body_order.pop_front()"));
    assert!(trim.contains("set_auxiliary_media_cache_bytes(self.bytes)"));

    let load = section(HLS_RUNTIME, "async fn hls_body(", "fn prefetch_bodies(");
    assert!(load.contains("BodyLoad::Cached(body)"));
    assert!(load.contains("hls_range(&client, &reference, root.span, 0, end, generation).await"));
    assert!(!load.contains("Arc::from(body)"));

    let range = section(HLS_RUNTIME, "async fn hls_range(", "async fn hls_body(");
    assert!(
        range.contains(
            "read_cached_hls_range(client, reference, span, start, end, &current)"
        )
    );
    assert!(cache.contains("body.slice(start..end)"));
    assert!(!range.contains("Arc::from"));
    assert!(load.contains("BodyLoad::Wait(waiter)"));
    assert!(load.contains("BodyLoad::Lead"));
    assert!(load.contains("root.span > HLS_BODY_MAX_BYTES"));
    assert!(load.contains("finish_body(reference, epoch, body)"));
}

#[test]
fn foreground_windows_and_whole_body_assembly_share_the_aligned_cache() {
    let range = section(HLS_RUNTIME, "async fn hls_range(", "async fn hls_body(");
    let cached = range
        .find("cache.borrow().get(reference, start, end)")
        .unwrap();
    let retrieve = range.find("read_cached_hls_range(").unwrap();
    assert!(cached < retrieve);
    assert!(!range.contains("waiter.recv()"));
    assert_eq!(range.matches("read_cached_hls_range(").count(), 1);
}
#[test]
fn live_duration_window_keeps_owned_bodies_and_beginning_seek_delivery() {
    let runway = section(HLS_RUNTIME, "fn live_runway_targets(", "fn prefetch_from_reference(");
    assert!(runway.contains("seconds >= HLS_LIVE_STARTUP_BUFFER_SECONDS"));
    assert!(runway.contains("!live_segment_is_playable(active, position + offset)"));
    assert!(runway.contains("!*complete || references.contains(reference)"));
    assert!(runway.contains("hls_body(client.clone(), reference.clone(), Some(id))"));
    assert!(runway.find("loads.next().await").unwrap() < runway.find("live_runway_running = false").unwrap());
    let cursor = section(HLS_RUNTIME, "fn prefetch_from_reference(", "fn next_feed_id(");
    assert!(cursor.contains("hls_progressive_foreground_transition"));
    assert!(cursor.contains("let successor = transition"));
    assert!(cursor.contains("if !active.beginning_history_started"));
    assert!(cursor.contains("spawn_live_runway(id)"));
}

#[test]
fn follower_settles_commit337_successors_sequentially_and_tolerates_one_gap() {
    assert!(HLS_RUNTIME.contains("const FEED_FOLLOW_AHEAD: u64 = 4;"));
    assert!(HLS_RUNTIME.contains("const FEED_FRONTIER_REFRESH_INTERVAL: f64 = 15_000.0;"));
    let initial = section(HLS_RUNTIME, "fn initialize_live_head(", "async fn prepare_live_plan(");
    assert!(initial.contains("spawn_follower(id)"));
    assert!(!HLS_RUNTIME.contains("async fn catch_up_history("));

    let follower = section(
        HLS_RUNTIME,
        "fn spawn_follower(",
        "async fn fetch_hls_body_response(",
    );
    let indices = follower
        .find("for offset in 1..=FEED_FOLLOW_AHEAD")
        .unwrap();
    let index = follower.find("head.checked_add(offset)").unwrap();
    let candidate = follower.find("let candidate =").unwrap();
    let dispatched = follower[candidate..].find("probe_feed_payload(").unwrap() + candidate;
    let settled = follower[dispatched..].find(".await;").unwrap() + dispatched;
    let applied = follower
        .find("apply_full_update(id, payload.index, playlist)")
        .unwrap();
    let missing = follower
        .find("FeedPayloadProbe::Missing | FeedPayloadProbe::Transient => {")
        .unwrap();
    let skip_once = follower[missing..]
        .find("if skipped_missing_index")
        .unwrap()
        + missing;
    let remember_gap = follower[skip_once..]
        .find("skipped_missing_index = true")
        .unwrap()
        + skip_once;
    let failed = follower.find("let Some(appended) = appended else").unwrap();
    let progressed = follower.find("if progressed {").unwrap();
    let idle_sleep = follower[progressed..]
        .find("async_std::task::sleep(FEED_POLL_INTERVAL).await")
        .unwrap()
        + progressed;
    assert!(
        indices < index
            && index < candidate
            && candidate < dispatched
            && dispatched < settled
            && settled < applied
    );
    assert!(applied < missing && missing < skip_once && skip_once < remember_gap);
    assert!(remember_gap < failed && failed < progressed && progressed < idle_sleep);
    assert!(!follower.contains("pace_next"));
    assert!(!follower.contains("Duration::try_from_secs_f64"));
    assert!(follower.contains("FEED_TAIL_PROBE_BYTES, None)"));
    assert!(!follower.contains("payload_probe_wave("));
    assert!(!follower.contains("settled_payload_wave("));
    assert!(follower.contains("skipped_missing_index"));
    assert!(!follower[progressed..idle_sleep].contains("recover_feed_frontier"));
    assert!(!follower.contains("Vec<Option<(u64, FeedPayloadProbe)>>"));
    assert!(follower.contains("now - last_frontier_check >= FEED_FRONTIER_REFRESH_INTERVAL"));
    assert!(follower.contains("discover_latest_once(client, owner, topic, None).await"));
    assert!(follower.contains("if index == head"));
    assert!(follower.contains("if index < head"));
    assert!(follower.contains("hls_history("));
    assert!(
        follower.find("HlsPlaylist::parse(&payload.bytes)").unwrap()
            < follower.find("let Some(history) = hls_history(").unwrap()
    );

    let publish = section(HLS_RUNTIME, "fn apply_update(", "fn apply_full_update(");
    assert!(publish.contains("Some((appended, active.start == HlsStart::Live))"));
    assert!(publish.contains("if updated.0 != 0 && updated.1"));
    assert!(publish.contains("spawn_live_runway(id)"));

    let runway = section(
        HLS_RUNTIME,
        "fn live_segment_is_playable(",
        "fn spawn_live_runway(",
    );
    assert!(runway.contains("presentation_gaps"));
    assert!(runway.contains("live_segment_is_playable(active"));
}

#[test]
fn completed_hls_bodies_are_served_before_root_or_range_retrieval() {
    let cached = section(
        HLS_RUNTIME,
        "fn hls_body_response(",
        "async fn fetch_hls_body_response(",
    );
    assert!(cached.contains("parse_hls_range(range, span)?"));
    assert!(
        cached
            .contains("FetchResponse::ok_shared_slice(status, headers, body, start, end)")
    );
    assert!(cached.contains("FetchResponse::ok_shared(status, headers, body)"));
    assert!(!cached.contains("Arc::from(body.get"));

    let transfer = section(
        STREAM,
        "pub(crate) struct FetchResponse {",
        "pub(crate) async fn service_worker_message_response(",
    );
    assert!(transfer.contains("body: Option<Bytes>"));
    assert!(transfer.contains("body: body.map(Bytes::from)"));
    assert!(transfer.contains("body.get(start..end)?"));
    assert!(transfer.contains("body.slice(start..end)"));
    assert_eq!(transfer.matches("bytes_to_js(&body)").count(), 1);

    let response = section(
        HLS_RUNTIME,
        "async fn fetch_hls_body_response(",
        "fn parse_hls_range(",
    );
    let fast_path = response.find("cache.borrow().body(&reference)").unwrap();
    let fast_return = response[fast_path..].find("return response;").unwrap() + fast_path;
    let decode = response.find("hex::decode(&reference)").unwrap();
    let root = response.find("retrieve_decoded_data_root(").unwrap();
    assert!(fast_path < fast_return && fast_return < decode && decode < root);
}

#[test]
fn playback_does_not_add_a_predecessor_eviction_policy() {
    assert!(!HLS_CORE.contains("cached_predecessors"));
    assert!(!HLS_RUNTIME.contains("evict_cached_predecessors"));
    assert!(!HLS_RUNTIME.contains("fn evict_references("));
}

#[test]
fn media_delivery_shares_windows_and_seeks_retain_whole_body_retries() {
    assert!(HLS_RUNTIME.contains("const HLS_BODY_ATTEMPTS: usize = 6;"));
    assert!(HLS_RUNTIME.contains("const HLS_BODY_RETRY_DELAY_MS: u64 = 75;"));

    let foreground = section(
        HLS_RUNTIME,
        "async fn foreground_hls_body(",
        "fn prefetch_bodies(",
    );
    assert!(foreground.contains("for attempt in 0..HLS_BODY_ATTEMPTS"));
    assert!(foreground.contains("hls_body(client.clone(), reference.clone(), generation).await"));
    assert!(foreground.contains("HLS_BODY_RETRY_DELAY_MS * (attempt + 1) as u64"));

    let response = section(
        HLS_RUNTIME,
        "async fn fetch_hls_body_response(",
        "fn parse_hls_range(",
    );
    let seek = response.find("if seek_successor.is_some()").unwrap();
    let joined = response[seek..].find("foreground_hls_body(").unwrap() + seek;
    let shared = response[joined..].find("hls_body_response(body,").unwrap() + joined;
    let root = response.find("retrieve_decoded_data_root(").unwrap();
    assert!(seek < joined && joined < shared && shared < root);
    assert!(!response.contains("live && whole_media_get"));
    assert!(response.contains("FetchResponse::stream(200, headers)"));
}

#[test]
fn commit337_seek_waits_for_the_current_body_and_successor_only_on_a_discontinuity() {
    let response = section(
        HLS_RUNTIME,
        "async fn fetch_hls_body_response(",
        "fn parse_hls_range(",
    );
    let seek = response.find("if seek_successor.is_some()").unwrap();
    let current = response[seek..].find("foreground_hls_body(").unwrap() + seek;
    let successor = response[current..]
        .find("hls_body(client.clone(), successor, None).await")
        .unwrap()
        + current;
    let release = response[successor..]
        .find("hls_body_response(body,")
        .unwrap()
        + successor;
    let ordinary_fast_path = response.find("if seek_successor.is_none()").unwrap();
    let progressive = response
        .find("FetchResponse::stream(200, headers)")
        .unwrap();
    assert!(seek < current && current < successor && successor < release);
    assert!(ordinary_fast_path < seek && release < progressive);
    assert!(response[seek..current].contains("let Some(body) ="));
}

#[test]
fn hls_service_streams_whole_bodies_through_exact_inclusive_ranges() {
    let response = section(
        HLS_RUNTIME,
        "async fn fetch_hls_body_response(",
        "fn parse_hls_range(",
    );
    let parsed = response.find("parse_hls_range(range, span)").unwrap();
    let retrieved = response
        .find("hls_range(&client, &reference, span, start, end, None).await")
        .unwrap();
    let content_range = response
        .find("let headers = hls_body_headers(")
        .unwrap();
    let shared = response
        .find("FetchResponse::ok_shared(206, headers, bytes)")
        .unwrap();
    assert!(parsed < retrieved && retrieved < content_range && content_range < shared);
    assert!(response.contains("FetchResponse::stream(200, headers)"));
    let headers = section(HLS_RUNTIME, "fn hls_body_headers(", "fn hls_body_response(");
    assert!(headers.contains("Content-Length"));
    assert!(headers.contains("Accept-Ranges"));
    assert!(response.contains("let mime = if codec_bootstrap"));
    assert!(response.contains("hls_payload_mime(&prefix)"));
    assert!(response.contains("else {\n        \"application/octet-stream\""));

    let parser = section(
        HLS_RUNTIME,
        "fn parse_hls_range(",
        "async fn fetch_feed_response(",
    );
    assert!(parser.contains("strip_prefix(\"bytes=\")"));
    assert!(parser.contains("split_once('-')"));
    assert!(parser.contains("start.is_empty() || end.is_empty() || end.contains(',')"));
    assert!(parser.contains("start <= end && end < size"));
}
