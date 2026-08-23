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
fn hls_media_is_demand_driven_and_feed_catchup_does_not_prefetch_fragments() {
    assert!(!HLS_RUNTIME.contains("fn prefetch("));
    let follower = section(
        HLS_RUNTIME,
        "fn spawn_follower(",
        "async fn fetch_hls_body_response(",
    );
    assert!(follower.contains("head.checked_add(1)"));
    assert!(!follower.contains("FuturesUnordered"));
}

#[test]
fn beginning_warms_the_early_runway_but_attaches_only_the_full_edge() {
    const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
    let attach = section(
        HLS_RUNTIME,
        "pub(crate) async fn attach_hls_feed_player(",
        "pub(crate) async fn open_hls_feed_view(",
    );
    let beginning = section(attach, "HlsStart::Beginning => {", "HlsStart::Live => {");
    let early = beginning.find("let early = discover_for_view(").unwrap();
    let edge = beginning.find("let edge = discover_for_view(").unwrap();
    let worker_ready = beginning.find("join(worker, early).await").unwrap();
    let warmup = beginning
        .find("player::warm_startup_runway(&plan.references, HlsStart::Beginning)")
        .unwrap();
    let overlap = beginning
        .find("join(warmup, join(loader, edge)).await")
        .unwrap();
    assert!(early < worker_ready && worker_ready < warmup && warmup < overlap);
    assert!(edge < overlap);
    assert!(beginning.contains("(worker, loader, edge)"));

    let full_edge_ready = attach
        .find("join(warmup, join(loader, edge)).await")
        .unwrap();
    let full_parse = attach
        .find("let playlist = HlsPlaylist::parse(&payload.bytes)")
        .unwrap();
    let install = attach
        .find("install_snapshot(id, index, playlist)")
        .unwrap();
    let playback = attach.find("player::play_hls(").unwrap();
    assert!(full_edge_ready < full_parse && full_parse < install && install < playback);
    assert!(!attach.contains("spawn_edge_upgrade"));
    assert!(PLAYER.contains("duration + BUFFER_EPSILON_SECONDS >= plan.duration"));
}

#[test]
fn active_manifest_refresh_renders_without_cloning_the_full_timeline() {
    let render = section(
        HLS_RUNTIME,
        "fn render_active_feed(",
        "fn apply_full_update(",
    );
    assert!(render.contains("playlist.as_ref()?.render("));
    assert!(!render.contains("playlist.clone()"));
}

#[test]
fn edge_snapshot_revalidates_only_when_confirmation_predates_cold_setup() {
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
        .find("join3(worker, loader, discovery).await")
        .unwrap();
    let freshness = attach.find("let confirmed_after_setup").unwrap();
    let revalidated = attach.find("catch_up_current_payload(").unwrap();
    let parsed = attach
        .find("let playlist = HlsPlaylist::parse(&payload.bytes)")
        .unwrap();
    assert!(joined < freshness && freshness < revalidated && revalidated < parsed);
    assert!(attach.contains("confirmed > worker_at && confirmed > loader_at"));
    assert!(attach.contains("if !confirmed_after_setup"));
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
