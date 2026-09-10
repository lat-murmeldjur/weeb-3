#[path = "support/source.rs"]
pub mod source;

use source::between as section;

const RETRIEVAL: &str = include_str!("../src/retrieval.rs");
const HLS_CORE: &str = include_str!("../src/stream_hls.rs");
const HLS_RUNTIME: &str = include_str!("../src/stream_hls/runtime.rs");

fn edge_anchors() -> Vec<u64> {
    section(HLS_RUNTIME, "const EDGE_ANCHORS: [u64; 16] = [", "];")
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
    let first = lower + 1;
    let divisor = (width - 1) as u128;
    let span = u128::from(upper - first);
    std::iter::once(first)
        .chain(
            (1..width - 1)
                .map(|position| first + (span * position as u128).div_ceil(divisor) as u64),
        )
        .chain(std::iter::once(upper))
        .collect()
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
    while upper - lower > 20 {
        let indices = refinement_indices(lower, upper, 16);
        assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(indices.len() <= 16);
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
    // Coarse geometry only selects the starting positive. The existing dense
    // confirmation advances through positives, then requires all twenty guards.
    loop {
        let guards = (1..=20).map(|offset| lower + offset).collect::<Vec<_>>();
        let Some(latest) = guards.into_iter().filter(|index| *index <= head).max() else {
            break;
        };
        lower = latest;
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
        "HLS_LIVE_BODY_RUNWAY_SEGMENTS",
        "body_runway_targets",
        "payload_probe_wave",
    ] {
        assert!(
            !RETRIEVAL.contains(forbidden),
            "generic retrieval contains HLS-owned symbol {forbidden}"
        );
    }

    assert!(HLS_RUNTIME.contains("read_cached_hls_range"));
    assert!(HLS_RUNTIME.contains("retrieve_decoded_data_root"));
    assert!(!HLS_RUNTIME.contains("retrieve_data_payload("));
    assert!(!HLS_RUNTIME.contains("retrieve_data_payload_cancellable"));
    assert!(!HLS_RUNTIME.contains("register_retrieve_cancel_token"));
}

#[test]
fn live_preparation_and_following_share_one_duration_based_owner() {
    assert!(HLS_CORE.contains("HLS_LIVE_STARTUP_BUFFER_SECONDS: f64 = 8.0"));
    let runway = section(HLS_RUNTIME, "fn body_runway_targets(", "fn prefetch_from_reference(");
    assert!(runway.contains("seconds >= HLS_LIVE_STARTUP_BUFFER_SECONDS"));
    assert!(runway.contains("-> &[super::HlsSegment]"));
    assert!(runway.contains("&playlist.segments[position..position + length]"));
    assert!(runway.contains("active.body_runway_running = true"));
    assert!(runway.contains("active.body_runway_running = false"));
    assert!(runway.contains("hls_body(client.clone(), reference.clone(), Some(id))"));
    let prepare = section(HLS_RUNTIME, "async fn discover_live_history(", "async fn discover_for_view(");
    assert!(prepare.find("initialize_live_head(").unwrap() < prepare.find("future::join(").unwrap());
    let update = section(HLS_RUNTIME, "fn apply_update(", "fn apply_full_update(");
    assert!(update.contains("spawn_body_runway(id)"));
    assert!(!update.contains("active.foreground ="));
}

#[test]
fn cold_discovery_is_bounded_and_edge_search_is_hls_owned() {
    let beginning = section(
        HLS_RUNTIME,
        "async fn discover_beginning(",
        "async fn edge_probe_wave(",
    );
    for expected in [
        "stream::iter(0..BEGINNING_DISCOVERY_WIDTH)",
        ".buffer_unordered(BEGINNING_DISCOVERY_WIDTH as usize)",
        "probe_feed_payload(",
        "!feed_is_current(id)",
        "!result_view_request_is_current(view_generation)",
        "playlist.sequence == 0",
        "playlist.startup_plan(HlsStart::Beginning).is_some()",
        "while let Some(probe) = probes.next().await",
    ] {
        assert!(beginning.contains(expected), "{expected}");
    }
    let valid = beginning
        .find("playlist.startup_plan(HlsStart::Beginning).is_some()")
        .unwrap();
    let warm = beginning.find("warm_hls_prefix(").unwrap();
    let accepted = beginning[warm..].find("return Some(payload)").unwrap() + warm;
    assert!(valid < warm && warm < accepted);
    assert!(!beginning[warm..accepted].contains(".await"));
    assert!(!beginning.contains("spawn_local"));
    assert!(!beginning.contains("FEED_DISCOVERY_TIMEOUT"));
    for expected in [
        "const BEGINNING_PREFIX_TARGET_SEGMENTS: usize = 4;",
        "const EDGE_ANCHORS: [u64; 16]",
        "const EDGE_REFINEMENT_WIDTH: usize = 16;",
        "const EDGE_PROBE_ATTEMPTS: usize = 2;",
    ] {
        assert!(HLS_RUNTIME.contains(expected), "{expected}");
    }

    let edge = section(
        HLS_RUNTIME,
        "async fn discover_edge_update(",
        "async fn discover_latest_once(",
    );
    for expected in [
        "edge_probe_wave(client, owner, topic, &EDGE_ANCHORS, false, fast)",
        "(1..EDGE_REFINEMENT_WIDTH - 1)",
        "let first = latest.0 + 1;",
        ".chain(std::iter::once(upper))",
        "edge_probe_wave(client, owner, topic, &indices, true, fast)",
        "if upper.saturating_sub(latest.0) <= HISTORY_STRIDE * 2",
        "return Some(latest);",
        "if latest.0 >= upper || (latest.0, upper) == previous",
        "if let Some(next_upper)",
        "*index > latest.0 && missing[slot]",
    ] {
        assert!(edge.contains(expected), "{expected}");
    }
    assert!(!edge.contains("probe_feed_update("));
    assert!(!edge.contains("EDGE_RECOVERY_ANCHORS"));

    let wave = section(
        HLS_RUNTIME,
        "async fn edge_probe_wave(",
        "async fn discover_edge_update(",
    );
    for expected in [
        "let mut completed = vec![false; indices.len()];",
        "completed[first_unsettled..=upper]",
        ".buffer_unordered(indices.len().max(1))",
        "probe_feed_update(",
        "attempt_limit",
        "probes.next()",
        "EDGE_COLD_WAVE_TIMEOUT",
        "EDGE_WAVE_TIMEOUT",
        "let mut positive_seen = false;",
        "future::pending::<()>().left_future()",
        "future::select(probes.next(), deadline.as_mut()).await",
        "let Some(upper)",
        "break;",
    ] {
        assert!(wave.contains(expected), "{expected}");
    }
    assert!(
        wave.contains("deadline.set(async_std::task::sleep(EDGE_WAVE_TIMEOUT).right_future())")
    );
    assert!(!wave.contains("async_std::future::timeout"));

    let shared_probe = section(
        HLS_RUNTIME,
        "async fn probe_feed_update(",
        "async fn probe_feed_payload(",
    );
    assert!(shared_probe.contains("attempt_limit: Option<usize>"));
    assert!(shared_probe.contains("map_or_else(RetrieveAdmission::new"));
    assert!(shared_probe.contains("RetrieveAdmission::new_with_attempt_limit"));

    let initial = section(
        HLS_RUNTIME,
        "async fn discover_latest_once(",
        "async fn retrieve_confirmed_payload(",
    );
    assert!(initial.contains("discover_edge_update(client, owner, topic, history).await?"));
    assert!(
        initial.contains("retrieve_confirmed_payload(client, owner, topic, index, update).await")
    );
}

#[test]
fn beginning_history_and_exact_following_run_concurrently_after_media_is_ready() {
    let history = section(
        HLS_RUNTIME,
        "pub(crate) fn start_beginning_history()",
        "fn spawn_follower(",
    );
    let follow = history.find("spawn_follower(id)").unwrap();
    let successor = history.find("hls_body(client.clone(), successor, Some(id)).await").unwrap();
    let discover = history.find("let history = discover_for_view(").unwrap();
    let apply = history
        .find("apply_full_update(id, index, history)")
        .unwrap();
    assert!(follow < successor && successor < discover && discover < apply);
    assert_eq!(history.matches("spawn_follower(id)").count(), 1);
}

#[test]
fn underfilled_beginning_prefix_starts_exact_following_immediately() {
    let attach = section(
        HLS_RUNTIME,
        "pub(crate) async fn prepare_hls_feed(",
        "pub(crate) fn release_hls_runtime()",
    );
    let underfilled = attach.find("let underfilled = head").unwrap();
    let install = attach
        .find("install_snapshot(id, index, head, None)")
        .unwrap();
    let follow = attach.find("if underfilled").unwrap();
    assert!(underfilled < install && install < follow);
    assert!(attach[follow..].contains("spawn_follower(id)"));
    assert!(attach.contains("< BEGINNING_PREFIX_TARGET_SEGMENTS"));
}

#[test]
fn commit_337_edge_geometry_stays_small_and_stable() {
    assert_eq!(
        edge_anchors(),
        vec![
            0,
            1,
            7,
            255,
            511,
            1_023,
            1_535,
            1_791,
            2_047,
            4_095,
            8_191,
            16_383,
            65_535,
            262_143,
            1_048_575,
            u64::MAX,
        ]
    );
    for head in [217, 511, 1_687, 2_047, 3_798, 6_179, 6_256] {
        let (waves, probes) = contiguous_edge_waves(head);
        assert!(waves <= 4, "head {head} took {waves} waves");
        assert!(probes <= 64, "head {head} took {probes} probes");
    }
}

#[test]
fn head_proof_preserves_the_initial_lattice_and_settles_every_probe() {
    let proof = section(
        HLS_RUNTIME,
        "async fn retrieve_confirmed_payload(",
        "fn warm_hls_prefix(",
    );
    assert!(proof.contains("probe_feed_update("));
    assert!(proof.contains("Some(FEED_PROBE_ATTEMPTS)"));
    assert!(proof.contains(".buffered((HISTORY_STRIDE * 2) as usize)"));
    assert!(proof.contains("while let Some((candidate, probe)) = probes.next().await"));
    assert!(proof.contains("let lattice_residue = index % HISTORY_STRIDE;"));
    assert!(proof.contains("let mut next = index.checked_add(1)?;"));
    assert!(proof.contains("let mut probes = stream::iter(next..=end)"));
    assert!(proof.contains("if end == index.checked_add(HISTORY_STRIDE * 2)?"));
    assert!(proof.contains("next = end.checked_add(1)?;"));
    assert!(proof.contains("if transient"));
    assert!(proof.contains("lattice_residue,"));
    let history = section(
        HLS_RUNTIME,
        "async fn hls_history(",
        "async fn discover_raw_for_view(",
    );
    assert!(history.contains("lattice_residue: u64"));
    assert!(history.contains("history_indices(head_index, lattice_residue)?"));
    assert!(HLS_RUNTIME.contains("const HISTORY_FOREGROUND_PARALLEL: usize = 64;"));
}

#[test]
fn live_follower_applies_commit337_followups_in_order_with_one_lookup_ahead() {
    assert!(HLS_RUNTIME.contains("const FEED_TAIL_PROBE_BYTES: usize = 4 * 1024;"));
    assert!(HLS_RUNTIME.contains("const FEED_FOLLOW_AHEAD: u64 = 4;"));
    assert!(
        HLS_RUNTIME.contains("const FEED_POLL_INTERVAL: Duration = Duration::from_millis(400);")
    );
    assert!(HLS_RUNTIME.contains("const FEED_FRONTIER_REFRESH_INTERVAL: f64 = 15_000.0;"));

    let follower = section(
        HLS_RUNTIME,
        "fn spawn_follower(",
        "async fn fetch_hls_body_response(",
    );
    assert!(follower.contains("stream::iter(1..=FEED_FOLLOW_AHEAD)"));
    assert!(follower.contains("head.checked_add(offset)"));
    assert!(follower.contains("while let Some(candidate) = probes.next().await"));
    assert!(follower.contains(".buffered(2)"));
    assert!(follower.contains("probe_feed_payload("));
    assert!(!follower.contains("settled_payload_wave("));
    assert!(!follower.contains("payload_probe_wave("));
    assert!(!follower.contains("Vec<Option<(u64, FeedPayloadProbe)>>"));
    assert!(follower.contains("FeedPayloadProbe::Missing | FeedPayloadProbe::Transient => {"));
    assert!(follower.contains("if skipped_missing_index"));
    assert!(follower.contains("skipped_missing_index = true"));
    assert!(!follower.contains("recovered_missing_index"));
    assert!(follower.contains("let Some(appended) = appended else"));
    assert!(follower.contains("continue;"));
    assert!(follower.contains("if progressed"));

    let indices = follower
        .find("stream::iter(1..=FEED_FOLLOW_AHEAD)")
        .unwrap();
    let dispatched = follower[indices..].find("probe_feed_payload(").unwrap() + indices;
    let settled = follower[dispatched..].find(".await").unwrap() + dispatched;
    assert!(follower[dispatched..settled].contains("FEED_TAIL_PROBE_BYTES"));
    assert!(follower[dispatched..settled].contains("None"));
    let apply = follower[settled..]
        .find("apply_full_update(id, payload.index, playlist)")
        .unwrap()
        + settled;
    let stop = follower[apply..]
        .find("let Some(appended) = appended else")
        .unwrap()
        + apply;
    let progressed = follower[stop..].find("if progressed").unwrap() + stop;
    assert!(indices < dispatched && dispatched < settled && settled < apply);
    assert!(apply < stop && stop < progressed);

    assert!(follower.contains("now - last_frontier_check >= FEED_FRONTIER_REFRESH_INTERVAL"));
    assert!(follower.contains("discover_latest_once(client, owner, topic, None).await"));
    assert!(follower.contains("if index == head"));
    assert!(follower.contains("if index < head"));
    assert!(follower.contains("hls_history("));
    assert!(
        follower.find("HlsPlaylist::parse(&payload.bytes)").unwrap()
            < follower.find("let Some(history) = hls_history(").unwrap()
    );
    let progressed = follower.find("if progressed").unwrap();
    let idle_sleep = follower
        .find("async_std::task::sleep(FEED_POLL_INTERVAL).await")
        .unwrap();
    assert!(progressed < idle_sleep);
    assert!(follower.find("drop(probes)").unwrap() < idle_sleep);
    assert!(!follower[progressed..idle_sleep].contains("recover_feed_frontier"));
    assert!(!follower.contains("pace_next"));
    assert!(!follower.contains("Duration::try_from_secs_f64"));
}

#[test]
fn authenticated_tail_growth_requires_a_reference_overlap() {
    let merge = section(
        HLS_CORE,
        "pub(crate) fn merge_tail",
        "pub(crate) fn merge_playlist",
    );
    assert!(merge.contains("parse_segment_lines(text, 0)"));
    assert!(merge.contains(".or_else(||"));
    assert!(merge.contains("self.merge_segments(candidates"));

    let segments = section(HLS_CORE, "fn merge_segments(", "pub(crate) fn render(");
    assert!(segments.contains("rposition(|candidate| candidate.same_payload(current_tail))"));
    assert!(segments.contains("checked_sub(candidates[overlap].discontinuity_sequence)"));
    assert!(segments.contains("candidates[overlap].same_media(current_tail)"));
    assert!(segments.contains("candidates.into_iter().skip(overlap + 1)"));
}

#[test]
fn active_feed_treats_snapshot_endlist_as_tentative() {
    let install = section(
        HLS_RUNTIME,
        "fn install_snapshot(",
        "async fn discover_beginning(",
    );
    assert!(install.contains("playlist.finalized = false"));
    assert!(install.contains("active.terminal_candidate = playlist.finalized.then_some(index)"));

    let apply = section(HLS_RUNTIME, "fn apply_update(", "fn apply_full_update(");
    assert!(apply.contains("index <= current"));
    assert!(apply.contains("let appended = merge(playlist)?"));
    assert!(apply.contains("playlist.finalized = false"));
    assert!(apply.contains("active.terminal_candidate = terminal.then_some(index)"));

    let follow = section(
        HLS_RUNTIME,
        "fn feed_follow_context(",
        "async fn apply_deferred_update(",
    );
    assert!(follow.contains("if feed.playlist.as_ref()?.finalized"));

    let confirmation = section(
        HLS_RUNTIME,
        "fn confirm_terminal(",
        "fn live_tail_position(",
    );
    assert!(confirmation.contains("active.terminal_candidate == Some(index)"));
    assert!(confirmation.contains("playlist.merge_playlist(candidate)"));
    assert!(confirmation.contains("active.terminal_candidate = None"));
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
