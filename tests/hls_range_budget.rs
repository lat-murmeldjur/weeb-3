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
    assert!(HLS_RUNTIME.contains("const RANGE_CACHE_HARD_MAX_BYTES: u64 = 96 * 1024 * 1024;"));

    let cache = section(HLS_RUNTIME, "struct RangeCache {", "struct FeedSession");
    assert!(cache.contains("bodies: HashMap<String, Bytes>"));
    assert!(cache.contains("body_order: VecDeque<String>"));
    assert!(cache.contains("pending_bodies: HashMap<String, PendingBody>"));
    assert!(cache.contains("generation: Option<u64>"));
    assert!(cache.contains("waiters: Vec<mpsc::Sender<Option<Bytes>>>"));
    assert!(cache.contains("enum BodyLoad"));

    let admission = section(cache, "fn body_load(", "fn pending_body(");
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
    assert!(trim.contains("saturating_sub(completed_media_range_bytes())"));
    assert!(trim.contains(".min(RANGE_CACHE_HARD_MAX_BYTES)"));
    assert!(trim.contains("self.order.pop_front()"));
    assert!(trim.contains("self.body_order.pop_front()"));
    assert!(trim.contains("set_auxiliary_media_cache_bytes(self.bytes)"));

    let load = section(HLS_RUNTIME, "async fn hls_body(", "fn prefetch_bodies(");
    assert!(load.contains("BodyLoad::Cached(body)"));
    assert!(load.contains("then(|| Bytes::from(body))"));
    assert!(!load.contains("Arc::from(body)"));

    let range = section(HLS_RUNTIME, "async fn hls_range(", "async fn hls_body(");
    assert!(range.contains("let bytes = Bytes::from(bytes);"));
    assert!(range.contains("body.slice(start..end)"));
    assert!(!range.contains("Arc::from"));
    assert!(load.contains("BodyLoad::Wait(waiter)"));
    assert!(load.contains("BodyLoad::Lead"));
    assert!(load.contains("root.span > RANGE_CACHE_HARD_MAX_BYTES"));
    assert!(load.contains("finish_body(reference, epoch, body)"));
}

#[test]
fn live_exact_ranges_bypass_pending_bodies_while_other_ranges_can_join_them() {
    let range = section(HLS_RUNTIME, "async fn hls_range(", "async fn hls_body(");
    let cached = range
        .find("cache.borrow().get(&reference, start, end)")
        .unwrap();
    let conditional = range.find("if join_pending_body").unwrap();
    let pending = range.find("pending_body(&reference)").unwrap();
    let joined = range.find("waiter.recv().await").unwrap();
    let fallback = range.find("retrieve_data_range_from_root(").unwrap();
    let exact = range
        .find("bytes.len() as u64 != end.checked_sub(start)?.checked_add(1)?")
        .unwrap();
    let stored = range
        .find("insert(epoch, reference, start, end, bytes.clone())")
        .unwrap();

    assert!(cached < conditional && conditional < pending && pending < joined && joined < fallback);
    assert!(fallback < exact && exact < stored);
    assert!(range.contains("body.get(start..end)?;"));
    assert!(range.contains("body.slice(start..end)"));
    assert!(!range.contains("Arc::from"));
    assert_eq!(range.matches("retrieve_data_range_from_root(").count(), 1);

    let response = section(
        HLS_RUNTIME,
        "async fn fetch_hls_body_response(",
        "fn parse_hls_range(",
    );
    assert!(
        response.contains("hls_range(client, reference, root, encrypted, start, end, !live).await")
    );
    assert!(
        response.contains("hls_range(client, reference, root, encrypted, 0, prefix_end, !live)")
    );
}

#[test]
fn beginning_stays_progressive_while_live_keeps_three_successors_ahead() {
    assert!(HLS_CORE.contains("const HLS_LIVE_BODY_RUNWAY_SEGMENTS: usize = 4;"));
    assert!(
        HLS_RUNTIME.contains("const BODY_PREFETCH_HORIZON: usize = HLS_LIVE_BODY_RUNWAY_SEGMENTS;")
    );
    assert!(HLS_RUNTIME.contains("const HLS_BODY_PREFETCH_MAX_PARALLEL: usize = 3;"));
    assert!(
        HLS_RUNTIME.contains("const HLS_NEXT_RESERVE_STAGGER: Duration = Duration::from_secs(1);")
    );

    let parallel = section(
        HLS_RUNTIME,
        "fn prefetch_bodies(",
        "fn prefetch_priority_runway(",
    );
    assert!(parallel.contains(".take(BODY_PREFETCH_HORIZON)"));
    assert!(parallel.contains("spawn_local(async move"));
    assert!(parallel.contains("hls_body(client, reference, generation).await"));

    let priority = section(
        HLS_RUNTIME,
        "fn prefetch_priority_runway(",
        "fn prefetch_playlist_runway(",
    );
    let bounded = priority
        .find("references.truncate(BODY_PREFETCH_HORIZON)")
        .unwrap();
    let live_only = priority.find("if start == HlsStart::Live").unwrap();
    let live_runway = priority
        .find("prefetch_bodies(client, references, generation)")
        .unwrap();
    let live_return = priority[live_runway..].find("return;").unwrap() + live_runway;
    let progressive_current = priority.find("references.into_iter().skip(1)").unwrap();
    let stagger = priority
        .find("offset as u64 + u64::from(!head_ready)")
        .unwrap();
    let successor = priority
        .find("hls_body(client, reference, None).await")
        .unwrap();
    assert!(
        bounded < live_only
            && live_only < live_runway
            && live_runway < live_return
            && live_return < progressive_current
            && progressive_current < stagger
            && stagger < successor
    );

    let targets = section(
        HLS_RUNTIME,
        "fn live_runway_targets(",
        "fn live_runway_context(",
    );
    assert!(targets.contains("active.live_foreground.as_deref()"));
    assert!(targets.contains("latest_live_foreground(active)"));
    assert!(targets.contains(".take(HLS_LIVE_BODY_RUNWAY_SEGMENTS)"));

    let persistent = section(
        HLS_RUNTIME,
        "fn spawn_live_runway(",
        "fn prefetch_from_reference(",
    );
    let outer = persistent.find("spawn_local(async move").unwrap();
    let retry = persistent.find("loop {").unwrap();
    let cached = persistent
        .find("cache.borrow().body_cached(reference)")
        .unwrap();
    let current = persistent
        .find("live_runway_context(id).is_some_and(|(_, current)|")
        .unwrap();
    let each = persistent.find("for reference in references").unwrap();
    let owned = persistent
        .find("body_ready_or_pending(&reference)")
        .unwrap();
    let stagger = persistent.find("if stagger").unwrap();
    let scoped = persistent.find("cache.pending_body_count(id)").unwrap();
    let bounded = persistent.find("< HLS_BODY_PREFETCH_MAX_PARALLEL").unwrap();
    let detached = persistent[owned..].find("spawn_local(async move").unwrap() + owned;
    let load = persistent
        .find("hls_body(client, reference, Some(id)).await")
        .unwrap();
    let poll = persistent
        .find("async_std::task::sleep(Duration::from_millis(25)).await")
        .unwrap();
    assert!(outer < retry && retry < cached && cached < current);
    assert!(current < each && each < owned && owned < stagger && stagger < scoped);
    assert!(scoped < bounded && bounded < detached && detached < load && load < poll);
    assert_eq!(
        persistent
            .matches("live_runway_context(id).is_some_and(|(_, current)|")
            .count(),
        2
    );
    assert!(persistent.contains("active.live_runway_running = true"));
    assert!(persistent.contains("active.live_runway_running = false"));

    let cursor = section(
        HLS_RUNTIME,
        "fn prefetch_from_reference(",
        "fn next_feed_id(",
    );
    assert!(cursor.contains(".position(matches)"));
    assert!(cursor.contains(".rfind(|(position, segment)|"));
    assert!(cursor.contains("live_segment_is_playable(active, *position)"));
    assert!(cursor.contains(".take(BODY_PREFETCH_HORIZON)"));
    assert!(cursor.contains("active.live_foreground = Some(reference.to_string())"));
    assert!(cursor.contains(
        "prefetch_priority_runway(client, references, HlsStart::Beginning, None, cached)"
    ));
    assert!(cursor.contains("hls_progressive_foreground_transition"));
    assert!(cursor.contains("let playable = playlist.segments.iter().filter"));
    assert!(cursor.contains("let successor = transition"));
    assert!(cursor.contains("spawn_live_runway(id)"));

    let discovery = section(
        HLS_RUNTIME,
        "async fn discover_beginning(",
        "async fn edge_probe_wave(",
    );
    assert!(!discovery.contains("prefetch_playlist_runway("));
}

#[test]
fn follower_settles_commit337_successors_sequentially_and_tolerates_one_gap() {
    assert!(HLS_RUNTIME.contains("const FEED_FOLLOW_AHEAD: u64 = 4;"));
    assert!(HLS_RUNTIME.contains("const FEED_FRONTIER_REFRESH_INTERVAL: f64 = 15_000.0;"));
    let dispatch = section(
        HLS_RUNTIME,
        "fn payload_probe_wave(",
        "async fn settled_payload_wave(",
    );
    let spawn = dispatch.find("spawn_local(async move").unwrap();
    let await_probe = dispatch.find("probe_feed_payload(").unwrap();
    let send = dispatch
        .find("results.try_send((slot, index, result))")
        .unwrap();
    let close_dispatch = dispatch.find("drop(results)").unwrap();
    assert!(dispatch.contains("indices.iter().copied().enumerate()"));
    assert!(spawn < await_probe && await_probe < send && send < close_dispatch);
    assert!(dispatch.contains("attempt_limit: Option<usize>"));
    assert!(dispatch.contains("attempt_limit,"));

    let settled = section(
        HLS_RUNTIME,
        "async fn settled_payload_wave(",
        "async fn merge_history_probe(",
    );
    let wave = settled
        .find("payload_probe_wave(client, owner, topic, indices, Some(FEED_PROBE_ATTEMPTS))")
        .unwrap();
    let drain = settled
        .find("while let Ok((_, index, result)) = input.recv().await")
        .unwrap();
    let ordered = settled.find("settled.sort_by_key").unwrap();
    assert!(wave < drain && drain < ordered);

    let catch_up = section(
        HLS_RUNTIME,
        "async fn catch_up_history(",
        "async fn warm_codec_bootstrap(",
    );
    assert!(catch_up.contains("(1..=FEED_FOLLOW_AHEAD)"));
    assert!(catch_up.contains("settled_payload_wave(client, owner, topic, &indices).await"));
    assert!(catch_up.contains("for (_, probe) in wave"));

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
    assert!(follower.contains("discover_latest_once(client, owner, topic).await"));
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
        "fn live_runway_context(",
    );
    assert!(runway.contains("presentation_gaps"));
    assert!(runway.contains("live_segment_is_playable(active"));
}

#[test]
fn completed_hls_bodies_are_served_before_root_or_range_retrieval() {
    let cached = section(
        HLS_RUNTIME,
        "fn cached_hls_body_response(",
        "async fn fetch_hls_body_response(",
    );
    assert!(cached.contains("cache.borrow().body(reference)"));
    assert!(cached.contains("body.get(slice_start..slice_end)?"));
    assert!(
        cached
            .contains("FetchResponse::ok_shared_slice(206, headers, body, slice_start, slice_end)")
    );
    assert!(cached.contains("FetchResponse::ok_shared(200, headers, body)"));
    assert!(!cached.contains("Arc::from(body.get"));

    let transfer = section(
        STREAM,
        "enum FetchBody {",
        "pub(crate) async fn service_worker_message_response(",
    );
    assert!(transfer.contains("Shared(Bytes)"));
    assert!(transfer.contains("body.get(start..end)?"));
    assert!(transfer.contains("body.slice(start..end)"));
    assert!(transfer.contains("FetchBody::Shared(body) => body"));
    assert_eq!(transfer.matches("bytes_to_js(bytes)").count(), 1);

    let response = section(
        HLS_RUNTIME,
        "async fn fetch_hls_body_response(",
        "fn parse_hls_range(",
    );
    let fast_path = response.find("cached_hls_body_response(").unwrap();
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
fn live_whole_get_joins_the_runway_body_singleflight() {
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
    let live = response
        .find("if live && method == \"GET\" && range.is_none() && !codec_bootstrap")
        .unwrap();
    let joined = response[live..].find("foreground_hls_body(").unwrap() + live;
    let shared = response[joined..]
        .find("cached_hls_body_response(")
        .unwrap()
        + joined;
    let root = response.find("retrieve_decoded_data_root(").unwrap();
    assert!(live < joined && joined < shared && shared < root);
}

#[test]
fn commit337_seek_waits_for_the_current_body_and_successor_only_on_a_discontinuity() {
    let response = section(
        HLS_RUNTIME,
        "async fn fetch_hls_body_response(",
        "fn parse_hls_range(",
    );
    let seek = response
        .find("if let Some(successor) = seek_successor")
        .unwrap();
    let current = response[seek..].find("foreground_hls_body(").unwrap() + seek;
    let successor = response[current..]
        .find("hls_body(client.clone(), successor, None).await")
        .unwrap()
        + current;
    let release = response[successor..]
        .find("cached_hls_body_response(")
        .unwrap()
        + successor;
    let ordinary_fast_path = response[release..].find("if let Some(response)").unwrap() + release;
    let progressive = response
        .find("FetchResponse::stream(200, headers)")
        .unwrap();
    assert!(seek < current && current < successor && successor < release);
    assert!(release < ordinary_fast_path && ordinary_fast_path < progressive);
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
        .find("hls_range(client, reference, root, encrypted, start, end, !live).await")
        .unwrap();
    let content_range = response
        .find("format!(\"bytes {start}-{end}/{span}\")")
        .unwrap();
    let shared = response
        .find("FetchResponse::ok_shared(206, headers, bytes)")
        .unwrap();
    assert!(parsed < retrieved && retrieved < content_range && content_range < shared);
    assert!(response.contains("FetchResponse::stream(200, headers)"));
    assert!(response.contains("Content-Length"));
    assert!(response.contains("Accept-Ranges"));
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
