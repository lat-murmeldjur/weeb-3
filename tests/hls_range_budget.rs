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
fn hls_fragments_use_one_small_windowed_retrieval_path() {
    let retrieve = section(HLS_RUNTIME, "async fn hls_range(", "fn next_feed_id(");
    assert_eq!(
        retrieve.matches("retrieve_data_range_from_root(").count(),
        1
    );
    assert!(!retrieve.contains("retrieve_data_payload("));
    assert!(!retrieve.contains("RawFetchLifecycle"));
    assert!(!retrieve.contains("cancel_generations"));
}

#[test]
fn hls_range_cache_is_bounded_without_a_policy_engine() {
    assert!(HLS_RUNTIME.contains("const RANGE_CACHE_HARD_MAX_BYTES: u64 = 96 * 1024 * 1024;"));
    let cache = section(HLS_RUNTIME, "impl RangeCache {", "struct FeedSession");
    assert!(cache.contains("saturating_sub(completed_media_range_bytes())"));
    assert!(cache.contains("set_auxiliary_media_cache_bytes(self.bytes)"));
    assert!(cache.contains("self.order.pop_front()"));
    assert!(!cache.contains("self.order.retain"));
}

#[test]
fn hls_feed_catchup_warms_only_new_authenticated_live_fragments() {
    assert!(!HLS_RUNTIME.contains("fn prefetch("));
    let follower = section(
        HLS_RUNTIME,
        "fn spawn_follower(",
        "async fn fetch_hls_body_response(",
    );
    assert!(follower.contains("head.checked_add(1)"));
    assert!(follower.contains("warm_appended(id, appended, start)"));
    let warm = section(HLS_RUNTIME, "fn warm_appended(", "fn spawn_follower(");
    assert!(warm.contains("start != HlsStart::Live"));
    let first_new = warm.find("saturating_sub(appended)").unwrap();
    let oldest_first = warm.find(".skip(first_new)").unwrap();
    let skip_gaps = warm.find(".filter(|segment| !segment.gap)").unwrap();
    let runway = warm.find(".take(HLS_LIVE_SYNC_SEGMENTS)").unwrap();
    assert!(first_new < oldest_first && oldest_first < skip_gaps && skip_gaps < runway);
    assert!(warm.contains("warm_startup_reference(&reference, start)"));
    assert!(!follower.contains("discover_latest_once("));
    assert!(!follower.contains("last_progress"));
    assert!(!follower.contains("offset"));
    assert!(!follower.contains("FuturesUnordered"));
}

#[test]
fn beginning_attaches_the_warmed_prefix_while_history_grows_in_background() {
    const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
    let attach = section(
        HLS_RUNTIME,
        "pub(crate) async fn attach_hls_feed_player(",
        "pub(crate) async fn open_hls_feed_view(",
    );
    let beginning = section(attach, "HlsStart::Beginning => {", "HlsStart::Live => {");
    let early = beginning.find("let early =").unwrap();
    let history = beginning.find("spawn_beginning_history(").unwrap();
    assert!(beginning.contains("discover_beginning(client.clone(), owner.clone(), topic.clone())"));
    let overlap = beginning
        .find("join(loader, join(worker, early)).await")
        .unwrap();
    assert!(early < history && history < overlap);
    assert!(!beginning.contains("warm_live_runway"));
    assert!(beginning.contains("(worker, loader, prefix)"));
    assert!(!beginning.contains("join(loader, full)"));

    let prefix_parse = attach
        .find("let playlist = match payload.history.take()")
        .unwrap();
    let install = attach.find("install_snapshot(").unwrap();
    let playback = attach.find("player::play_hls(").unwrap();
    assert!(prefix_parse < install && install < playback);
    assert!(!attach.contains("spawn_follower(id)"));
    assert!(PLAYER.contains("duration + BUFFER_EPSILON_SECONDS >= plan.duration"));

    let background = section(
        HLS_RUNTIME,
        "fn spawn_beginning_history(",
        "fn spawn_follower(",
    );
    assert!(background.contains("discover_for_view("));
    assert!(background.contains("!BEGINNING_MEDIA_READY.with(Cell::get)"));
    assert!(background.contains("payload.history.take()"));
    assert!(background.contains("apply_full_update(id, payload.index, history)"));
    assert!(background.contains("result_view_request_is_current(view_generation)"));
    let history_gate = section(
        PLAYER,
        "fn start_beginning_history_when_safe(",
        "fn begin_playback(",
    );
    assert!(history_gate.contains("!live"));
    assert!(history_gate.contains("buffered_covers(media, plan.play_position, plan.runway_end)"));
    assert!(history_gate.contains("super::runtime::start_beginning_history()"));
    assert!(!background.contains("warm_startup_batch"));
    assert!(!HLS_RUNTIME.contains("BEGINNING_WARM_SEGMENTS"));
    assert!(!PLAYER.contains("warm_startup_batch"));
}

#[test]
fn active_manifest_refresh_renders_without_cloning_the_full_timeline() {
    let render = section(
        HLS_RUNTIME,
        "fn render_active_feed(",
        "fn apply_full_update(",
    );
    assert!(render.contains("playlist.as_ref()?.render("));
    assert!(render.contains("(!feed.following).then_some(feed.id)"));
    assert!(render.contains("feed.following = true"));
    assert!(!render.contains("playlist.clone()"));

    let response = section(
        HLS_RUNTIME,
        "async fn fetch_feed_response(",
        "pub(crate) async fn try_fetch_response(",
    );
    let frozen = response.find("render_active_feed(").unwrap();
    let follower = response.find("spawn_follower(id)").unwrap();
    assert!(frozen < follower);
}

#[test]
fn edge_snapshot_reuses_bounded_boundary_and_revalidates_stale_setup() {
    let discovery = section(
        HLS_RUNTIME,
        "async fn discover_latest_once(",
        "async fn discover_for_view(",
    );
    let payload = discovery.find("retrieve_feed_payload(").unwrap();
    let confirmation = discovery.find("probe_feed_update(").unwrap();
    assert!(confirmation > payload);
    assert!(discovery.contains("confirmed_at: Some(prerequisite_timestamp())"));

    let catch_up = section(
        HLS_RUNTIME,
        "async fn catch_up_current_payload(",
        "async fn discover_for_view(",
    );
    assert!(catch_up.contains("FeedProbe::Transient => {}"));
    assert!(catch_up.contains("INITIAL_DISCOVERY_RETRY_DELAY"));

    let attach = section(
        HLS_RUNTIME,
        "pub(crate) async fn attach_hls_feed_player(",
        "pub(crate) async fn open_hls_feed_view(",
    );
    let joined = attach
        .find("join(worker, join(loader, discovery)).await")
        .unwrap();
    let freshness = attach.find("let confirmed_after_setup").unwrap();
    let revalidated = attach
        .match_indices("catch_up_current_payload(")
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    let parsed = attach
        .find("let playlist = match payload.history.take()")
        .unwrap();
    let installed = attach.find("install_snapshot(").unwrap();
    assert_eq!(revalidated.len(), 1);
    assert!(
        joined < freshness
            && freshness < revalidated[0]
            && revalidated[0] < parsed
            && parsed < installed
    );
    assert!(attach.contains("confirmed > worker_at && confirmed > loader_at"));
    assert!(attach.contains("if live && !confirmed_after_setup"));
    assert!(attach.contains("(result, prerequisite_timestamp())"));
    assert!(attach.contains("(ready, prerequisite_timestamp())"));
}

#[test]
fn hls_service_responses_stream_exact_windows() {
    let response = section(
        HLS_RUNTIME,
        "async fn fetch_hls_body_response(",
        "async fn fetch_feed_response(",
    );
    assert!(response.contains("FetchResponse::stream(200, headers)"));
    assert!(response.contains("Content-Range"));
    assert!(response.contains("parse_hls_range(range, span)"));

    let feed = section(
        HLS_RUNTIME,
        "async fn fetch_feed_response(",
        "pub(crate) async fn try_fetch_response(",
    );
    assert!(feed.contains("Cache-Control"));
    assert!(feed.contains("no-store"));
    assert!(!feed.contains("if_none_match_matches"));

    let routing = section(
        HLS_RUNTIME,
        "pub(crate) async fn try_fetch_response(",
        "fn canonical_hls_bytes_resource(",
    );
    assert!(routing.contains("range: Option<&str>"));
    assert!(routing.contains("fetch_hls_body_response("));
}

#[test]
fn removed_progressive_policy_engine_stays_absent() {
    let combined = format!("{HLS_CORE}{HLS_RUNTIME}");
    for removed in [
        "HLS_BACKGROUND_RANGE_MAX",
        "HlsProgressiveRunway",
        "HlsAlignedRangeState",
        "prefetch_hls_progressive_ranges",
        "sequence_zero_start_requested",
        "pending_ranges",
    ] {
        assert!(
            !combined.contains(removed),
            "retired progressive HLS policy {removed} returned"
        );
    }
}
