const RETRIEVAL: &str = include_str!("../src/retrieval.rs");
const HLS_CORE: &str = include_str!("../src/stream_hls.rs");
const HLS_RUNTIME: &str = include_str!("../src/stream_hls/runtime.rs");

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .and_then(|(_, tail)| tail.split_once(end))
        .map(|(body, _)| body)
        .unwrap_or_else(|| panic!("missing source section between {start:?} and {end:?}"))
}

fn edge_anchors() -> Vec<u64> {
    section(
        HLS_RUNTIME,
        "const EDGE_ANCHORS: [u64; EDGE_PROBE_WIDTH] = [",
        "];",
    )
    .split(',')
    .filter_map(|value| {
        let value = value.trim();
        if value.is_empty() {
            None
        } else if value == "u64::MAX" {
            Some(u64::MAX)
        } else {
            Some(
                value
                    .replace('_', "")
                    .parse()
                    .expect("numeric HLS edge anchor"),
            )
        }
    })
    .collect()
}

fn refinement_indices(lower: u64, upper: u64, width: usize) -> Vec<u64> {
    let interior = upper - lower - 1;
    let count = interior.min((width - 1) as u64) as usize;
    let mut indices = if count as u64 == interior {
        (lower + 1..upper).collect::<Vec<_>>()
    } else {
        let first = lower + 1;
        let remaining = count - 1;
        let divisor = (remaining + 1) as u128;
        let span = u128::from(upper - first);
        std::iter::once(first)
            .chain(
                (1..=remaining)
                    .map(|position| first + (span * position as u128).div_ceil(divisor) as u64),
            )
            .collect()
    };
    indices.push(upper);
    indices
}

fn contiguous_edge_waves(head: u64) -> (usize, usize) {
    let anchors = edge_anchors();
    let mut probes = anchors.len();
    let mut waves = 1;
    let mut lower = *anchors
        .iter()
        .filter(|index| **index <= head)
        .max()
        .expect("edge zero anchor");
    let mut upper = *anchors
        .iter()
        .filter(|index| **index > head)
        .min()
        .expect("edge missing anchor");
    while lower + 1 != upper {
        let indices = refinement_indices(lower, upper, 32);
        assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(indices.len() <= 32);
        probes += indices.len();
        waves += 1;
        lower = indices
            .iter()
            .copied()
            .filter(|index| *index <= head)
            .max()
            .unwrap_or(lower);
        upper = indices
            .iter()
            .copied()
            .filter(|index| *index > head)
            .min()
            .expect("every refinement rechecks a missing upper bound");
    }
    assert_eq!(lower, head);
    (waves, probes)
}

#[test]
fn hls_retrieval_ownership_boundary_stays_strict() {
    assert!(!RETRIEVAL.to_ascii_lowercase().contains("hls"));
    for forbidden in [
        "StartupRawScout",
        "RawFetchLeaderCompletion",
        "crate::stream_hls",
    ] {
        assert!(
            !RETRIEVAL.contains(forbidden),
            "generic retrieval contains HLS-owned symbol {forbidden}"
        );
    }

    assert!(HLS_RUNTIME.contains("retrieve_data_range_from_root"));
    assert!(HLS_RUNTIME.contains("retrieve_decoded_data_root"));
    assert!(!HLS_RUNTIME.contains("retrieve_data_payload("));
    assert!(!HLS_RUNTIME.contains("retrieve_data_payload_cancellable"));
    assert!(!HLS_RUNTIME.contains("register_retrieve_cancel_token"));
}

#[test]
fn cold_discovery_is_bounded_and_edge_search_is_hls_owned() {
    let beginning = section(
        HLS_RUNTIME,
        "async fn discover_beginning(",
        "async fn edge_probe_wave(",
    );
    assert!(beginning.contains("for index in 0..BEGINNING_DISCOVERY_WIDTH"));
    assert!(beginning.contains("probe_feed_payload("));
    assert!(beginning.contains("async_std::future::timeout(FEED_DISCOVERY_TIMEOUT"));
    assert!(beginning.contains("async_std::future::timeout(BEGINNING_WAVE_TIMEOUT"));

    let edge = section(
        HLS_RUNTIME,
        "async fn discover_edge_update(",
        "async fn discover_latest_once(",
    );
    assert!(HLS_RUNTIME.contains("const EDGE_ANCHORS: [u64; EDGE_PROBE_WIDTH]"));
    assert!(HLS_RUNTIME.contains("const EDGE_PROBE_WIDTH: usize = 32;"));
    assert!(edge.contains("edge_probe_wave(client, owner, topic, &EDGE_ANCHORS, false)"));
    assert!(edge.contains("interior.min((EDGE_PROBE_WIDTH - 1) as u64)"));
    assert!(edge.contains("let first = latest.0 + 1;"));
    assert!(edge.contains("indices.push(upper);"));
    assert!(edge.contains("edge_probe_wave(client, owner, topic, &indices, true)"));
    assert!(edge.contains("if (latest.0, upper) == previous"));
    assert!(edge.contains("return None;"));

    let wave = section(
        HLS_RUNTIME,
        "async fn edge_probe_wave(",
        "async fn discover_edge_update(",
    );
    assert!(wave.contains("let mut completed = vec![false; indices.len()];"));
    assert!(wave.contains("completed[first_unsettled..=upper]"));
    assert!(wave.contains("break;"));

    let latest = section(
        HLS_RUNTIME,
        "async fn discover_latest_once(",
        "async fn discover_for_view(",
    );
    assert!(latest.contains("retrieve_feed_payload("));
    assert!(latest.contains("probe_feed_update(client, owner, topic, next)"));
    assert!(latest.contains("FeedProbe::Found(next_update)"));
    assert!(latest.contains("FeedProbe::Missing => {"));
    assert!(latest.contains("confirmed_at: Some(prerequisite_timestamp())"));
    assert!(!latest.contains("async_std::future::timeout"));
}

#[test]
fn captured_hls_heads_resolve_in_at_most_three_probe_waves() {
    assert_eq!(edge_anchors().len(), 32);
    for head in [217, 511, 1_687, 2_047, 3_798, 6_179, 6_256] {
        let (waves, probes) = contiguous_edge_waves(head);
        assert!(waves <= 3, "head {head} took {waves} waves");
        assert!(probes <= 96, "head {head} took {probes} probes");
    }
    assert_eq!(contiguous_edge_waves(4_481), (3, 82));
}

#[test]
fn live_follower_probes_only_the_contiguous_next_update_without_media_prefetch() {
    assert!(HLS_RUNTIME.contains("const FEED_TAIL_PROBE_BYTES: usize = 4 * 1024;"));
    assert!(
        HLS_RUNTIME.contains("const FEED_POLL_INTERVAL: Duration = Duration::from_millis(400);")
    );

    let follower = section(
        HLS_RUNTIME,
        "fn spawn_follower(id: u64)",
        "async fn fetch_hls_body_response(",
    );
    assert!(follower.contains("let Some(index) = head.checked_add(1)"));
    assert!(follower.contains("probe_feed_payload("));
    assert!(follower.contains("FeedPayloadProbe::Found(payload)"));
    assert!(follower.contains("FeedPayloadProbe::Deferred(root)"));
    assert!(follower.contains("retrieve_feed_payload_tail_conservative("));
    assert!(follower.contains("apply_full_update(id, payload.index, playlist)"));
    assert!(follower.contains("playlist.merge_tail(&tail)"));
    assert!(follower.contains("async_std::task::sleep(FEED_POLL_INTERVAL)"));
    assert!(!follower.contains("FuturesUnordered"));
    assert!(!follower.contains("prefetch"));
}

#[test]
fn authenticated_tail_growth_requires_a_reference_overlap() {
    let merge = section(
        HLS_CORE,
        "pub(crate) fn merge_tail",
        "pub(crate) fn merge_playlist",
    );
    assert!(merge.contains("parse_segment_lines(text)"));
    assert!(merge.contains(".or_else(||"));
    assert!(merge.contains("self.merge_segments(candidates"));

    let segments = section(HLS_CORE, "fn merge_segments(", "pub(crate) fn render(");
    assert!(segments.contains("rposition(|candidate| &candidate.reference == current_tail)"));
    assert!(segments.contains("candidates.into_iter().skip(overlap + 1)"));
}

#[test]
fn retired_multi_horizon_raw_scout_does_not_return() {
    let combined = format!("{HLS_CORE}{HLS_RUNTIME}");
    for removed in [
        "StartupRawScout",
        "startup_scout_next_admission_horizon",
        "scout_data_ranges_cache_only_cancellable",
        "HlsBeginningPrefixPhase",
        "HLS_BEGINNING_PREFIX_MAX_WINDOWS",
    ] {
        assert!(
            !combined.contains(removed),
            "retired startup scout symbol {removed} returned"
        );
    }
}
