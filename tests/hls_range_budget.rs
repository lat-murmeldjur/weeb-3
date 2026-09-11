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
fn complete_hls_bodies_use_shared_ranges_and_respect_cache_epoch_and_budget() {
    assert!(HLS_RUNTIME.contains("const HLS_BODY_CACHE_MAX_BYTES: u64 = 32 * 1024 * 1024;"));
    assert!(HLS_CORE.contains("const HLS_BODY_MAX_BYTES: u64 = 96 * 1024 * 1024;"));
    assert!(STREAM.contains("pending_ranges: SingleflightRegistry<"));

    let load = section(HLS_RUNTIME, "async fn hls_body(", "async fn foreground_hls_body(");
    assert!(load.contains("root.span == 0"));
    assert!(load.contains("root.span > HLS_BODY_MAX_BYTES"));
    assert!(load.contains("hls_range(&client, &reference, root.span, 0, end, generation, &|| {"));
    assert!(load.contains("generation.is_none_or(|id| body_is_current(id, &reference))"));
    assert!(HLS_RUNTIME.contains("feed_is_current(id) && result_view_request_is_current(view_generation)"));
    assert!(load.find("hls_range(").unwrap() < load.find("finish_body(reference, epoch, body)").unwrap());

    let cache = section(HLS_RUNTIME, "struct BodyCache {", "struct FeedSession");
    let settlement = section(cache, "fn finish_body(", "fn trim(");
    let insert = settlement.find("self.bodies.insert(").unwrap();
    assert!(settlement.find("if epoch != self.epoch").unwrap() < insert);
    assert!(settlement.find("if let Some(body) = &body").unwrap() < insert);
    assert!(settlement.contains("body.len() as u64 <= media_cache_max_bytes().min(HLS_BODY_CACHE_MAX_BYTES)"));
    assert!(settlement.contains("!self.bodies.contains_key(&reference)"));
    assert!(settlement.contains("self.trim()"));
    assert!(settlement.find("forget_completed_reference_ranges(&reference)").unwrap() < insert);

    let trim = section(cache, "fn trim(", "fn clear(");
    assert!(trim.contains(".min(HLS_BODY_CACHE_MAX_BYTES)"));
    assert!(trim.contains("self.body_order.pop_front()"));
    assert!(trim.contains("set_auxiliary_media_cache_bytes(self.bytes)"));
    assert!(cache.contains("self.epoch = self.epoch.wrapping_add(1)"));
    assert!(cache.contains("body.slice(start..end)"));

    let window = section(STREAM, "async fn read_range_window(", "pub(crate) async fn read_cached_hls_range(");
    let validated = window.find("if body.len() == expected_len").unwrap();
    let remembered = window.find("cache.remember_range(").unwrap();
    assert!(window.find("if registration.leader").unwrap() < window.find("spawn_local(async move").unwrap());
    assert!(validated < remembered);
    assert!(window.find("if cache.epoch == epoch").unwrap() < remembered);
    assert!(window.contains("registration.shared.admission"));
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
    let runway = section(HLS_RUNTIME, "fn body_runway_targets(", "fn prefetch_from_reference(");
    assert!(runway.contains("seconds >= HLS_LIVE_STARTUP_BUFFER_SECONDS"));
    assert!(runway.contains("!live_segment_is_playable(active, position + offset)"));
    assert!(runway.contains("hls_body(client.clone(), reference.clone(), Some(id))"));
    assert!(runway.contains("!active.body_runway_running"));
    let cursor = section(HLS_RUNTIME, "fn prefetch_from_reference(", "fn next_feed_id(");
    assert!(cursor.contains("hls_progressive_foreground_transition"));
    assert!(cursor.find("if follow {").unwrap() < cursor.find("spawn_body_runway(id)").unwrap());
}

#[test]
fn follower_applies_commit337_successors_in_order_and_tolerates_one_gap() {
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
        .find("stream::iter(1..=FEED_FOLLOW_AHEAD)")
        .unwrap();
    let index = follower.find("head.checked_add(offset)").unwrap();
    let dispatched = follower[index..].find("probe_feed_payload(").unwrap() + index;
    let settled = follower[dispatched..].find(".await").unwrap() + dispatched;
    let candidate = follower.find("while let Some(candidate) = probes.next().await").unwrap();
    assert!(follower.contains(".buffered(2)"));
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
            && index < dispatched
            && dispatched < settled
            && settled < candidate
            && candidate < applied
    );
    assert!(applied < missing && missing < skip_once && skip_once < remember_gap);
    assert!(remember_gap < failed && failed < progressed && progressed < idle_sleep);
    assert!(follower.find("drop(probes)").unwrap() < idle_sleep);
    assert!(!follower.contains("pace_next"));
    assert!(!follower.contains("Duration::try_from_secs_f64"));
    assert!(follower[dispatched..settled].contains("FEED_TAIL_PROBE_BYTES"));
    assert!(follower[dispatched..settled].contains("None"));
    assert!(!follower.contains("payload_probe_wave("));
    assert!(!follower.contains("settled_payload_wave("));
    assert!(follower.contains("skipped_missing_index"));
    assert!(!follower[progressed..idle_sleep].contains("recover_feed_frontier"));
    assert!(!follower.contains("Vec<Option<(u64, FeedPayloadProbe)>>"));
    assert!(follower.contains("now - last_frontier_check >= FEED_FRONTIER_REFRESH_INTERVAL"));
    assert!(follower.contains("discover_latest_once(client, owner, topic, None).await"));
    assert!(follower.contains("if index < head"));
    assert!(follower.contains("hls_history("));
    assert!(
        follower.find("HlsPlaylist::parse(&payload.bytes)").unwrap()
            < follower.find("let Some(history) = hls_history(").unwrap()
    );

    let publish = section(HLS_RUNTIME, "fn apply_update(", "fn apply_full_update(");
    assert!(publish.contains("Some(appended)"));
    assert!(publish.find("if appended != 0 {").unwrap() < publish.find("spawn_body_runway(id)").unwrap());

    let runway = section(
        HLS_RUNTIME,
        "fn live_segment_is_playable(",
        "fn spawn_body_runway(",
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
        "fn live_segment_is_playable(",
    );
    assert!(foreground.contains("for attempt in 0..HLS_BODY_ATTEMPTS"));
    assert!(foreground.contains("hls_body(client.clone(), reference.clone(), None).await"));
    assert!(foreground.contains("HLS_BODY_RETRY_DELAY_MS * (attempt + 1) as u64"));

    let response = section(
        HLS_RUNTIME,
        "async fn fetch_hls_body_response(",
        "fn parse_hls_range(",
    );
    let seek = response.find("if complete_body && root.as_ref()").unwrap();
    let joined = response[seek..].find("foreground_hls_body(").unwrap() + seek;
    let shared = response[joined..].find("hls_body_response(body,").unwrap() + joined;
    let root = response.find("retrieve_decoded_data_root(").unwrap();
    assert!(root < seek && seek < joined && joined < shared);
    assert!(!response.contains("live && whole_media_get"));
    assert!(response.contains("FetchResponse::stream(200, headers)"));
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
        .find("hls_range(&client, &reference, span, start, end, None, &|| true).await")
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
