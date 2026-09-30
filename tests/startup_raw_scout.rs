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
    let width = (upper - lower).min(width as u64) as usize;
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

fn contiguous_edge_waves(head: u64, width: usize) -> (usize, usize) {
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
        let indices = refinement_indices(lower, upper, width);
        assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(indices.len() <= width);
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
    crate::source::assert_excludes(HLS_RUNTIME, &[
        "retrieve_data_payload(",
        "retrieve_data_payload_cancellable",
        "register_retrieve_cancel_token",
    ]);
}

#[test]
fn live_preparation_and_following_share_one_duration_based_owner() {
    assert!(HLS_CORE.contains("HLS_LIVE_STARTUP_BUFFER_SECONDS: f64 = 8.0"));
    let runway = section(HLS_RUNTIME, "fn body_runway_targets(", "fn prefetch_from_reference(");
    crate::source::assert_contains(runway, &[
        "seconds >= HLS_LIVE_STARTUP_BUFFER_SECONDS",
        "-> &[super::HlsSegment]",
        "&playlist.segments[position..position + length]",
        "active.body_runway_running = true",
        "active.body_runway_running = false",
        "hls_body(client.clone(), reference.clone(), Some(id))",
    ]);
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
        "!feed_is_current(id, view_generation)",
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
        "(upper - latest.0) as f64 / (HISTORY_STRIDE * 2) as f64",
        "if width <= 24",
        "width.max(EDGE_REFINEMENT_WIDTH)",
        "let width = (upper - latest.0).min(width as u64) as usize;",
        "(1..width - 1)",
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
        "hls_edge_wave_complete(first_unsettled, &missing, &completed)",
        ".buffer_unordered(indices.len().max(1))",
        "probe_feed_update(",
        "attempt_limit",
        "probes.next()",
        "EDGE_COLD_WAVE_TIMEOUT",
        "EDGE_WAVE_TIMEOUT",
        "if found.is_none()",
        "if found.as_ref().is_none_or(|(highest, _)| slot > *highest)",
        "future::pending::<()>().left_future()",
        "future::select(probes.next(), deadline.as_mut()).await",
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
    crate::source::assert_contains(shared_probe, &[
        "attempt_limit: Option<usize>",
        "map_or_else(RetrieveAdmission::new",
        "RetrieveAdmission::new_with_attempt_limit",
    ]);

    let initial = section(
        HLS_RUNTIME,
        "async fn discover_latest_once(",
        "async fn retrieve_confirmed_payload(",
    );
    assert!(initial.contains("discover_edge_update(client, owner, topic, lattice).await?"));
    assert!(
        initial.contains("retrieve_confirmed_payload(client, owner, topic, index, update).await")
    );
}

#[test]
fn beginning_history_and_exact_following_run_after_the_worker_warms_a() {
    let history = section(
        HLS_RUNTIME,
        "fn start_beginning_history(id: u64)",
        "fn spawn_follower(",
    );
    crate::source::assert_first_in_order(history, &[
        ("spawn_follower(id)", "missing source marker"),
        ("hls_body(client.clone(), successor, Some(id)).await", "missing source marker"),
        ("let history = discover_for_view(", "missing source marker"),
        ("apply_confirmed_snapshot(id, index, history, None)", "missing source marker"),
    ]);
    assert_eq!(history.matches("spawn_follower(id)").count(), 1);
}

#[test]
fn beginning_without_a_successor_starts_exact_following_immediately() {
    let attach = section(
        HLS_RUNTIME,
        "pub(crate) async fn prepare_hls_feed(",
        "pub(crate) fn release_hls_runtime()",
    );
    let needs_successor = attach.find("let needs_successor = head").unwrap();
    let install = attach
        .find("install_snapshot(id, index, head)")
        .unwrap();
    let follow = attach.find("if needs_successor").unwrap();
    assert!(needs_successor < install && install < follow);
    assert!(attach[follow..].contains("spawn_follower(id)"));
    assert!(attach.contains("filter(|segment| !segment.gap).nth(1).is_none()"));
}

#[test]
fn edge_geometry_bounds_work_for_small_masters_and_long_recordings() {
    let anchors = edge_anchors();
    assert_eq!(anchors.len(), 16);
    assert_eq!(anchors.first(), Some(&0));
    assert_eq!(anchors.last(), Some(&u64::MAX));
    assert!(anchors.windows(2).all(|pair| pair[0] < pair[1]));
    for head in [0, 1, 7, 8, 14, 15, 30] {
        assert_eq!(contiguous_edge_waves(head, 16), (1, 16), "small master {head}");
    }
    for head in [31, 63, 127, 217, 511, 1_687, 2_047, 3_798, 6_179, 6_256] {
        let (waves, probes) = contiguous_edge_waves(head, 16);
        assert!(waves <= 4, "head {head} took {waves} waves");
        assert!(probes <= 64, "head {head} took {probes} probes");
    }
}

#[test]
fn head_proof_preserves_the_initial_lattice_and_twenty_fresh_guards() {
    let proof = section(
        HLS_RUNTIME,
        "async fn retrieve_confirmed_payload(",
        "fn warm_hls_prefix(",
    );
    crate::source::assert_contains(proof, &[
        "probe_feed_update(",
        "Some(FEED_PROBE_ATTEMPTS)",
        "confirm_hls_feed_head(index, update, HISTORY_STRIDE * 2, |index|",
        "let lattice_residue = index % HISTORY_STRIDE;",
    ]);
    assert!(proof.find("confirm_hls_feed_head(").unwrap() < proof.find("decode_feed_payload_root(index, update)").unwrap());
    assert!(proof.contains("lattice_residue,"));
    let history = section(
        HLS_RUNTIME,
        "async fn hls_history(",
        "async fn discover_raw_for_view(",
    );
    assert!(history.contains("lattice_residue: u64"));
    assert!(history.contains("history_indices(head_index, lattice_residue, origin.is_some())?"));
    assert!(HLS_RUNTIME.contains("const HISTORY_FOREGROUND_PARALLEL: usize = 64;"));
}

#[test]
fn live_follower_applies_commit337_followups_in_order_with_one_lookup_ahead() {
    crate::source::assert_contains(HLS_RUNTIME, &[
        "const FEED_TAIL_PROBE_BYTES: usize = 4 * 1024;",
        "const FEED_FOLLOW_AHEAD: u64 = 4;",
        "const FEED_POLL_INTERVAL: Duration = Duration::from_millis(400);",
        "const FEED_FRONTIER_REFRESH_INTERVAL: f64 = 15_000.0;",
    ]);

    let follower = section(
        HLS_RUNTIME,
        "fn spawn_follower(",
        "async fn fetch_hls_body_response(",
    );
    crate::source::assert_contains(follower, &[
        "stream::iter(1..=FEED_FOLLOW_AHEAD)",
        "head.checked_add(offset)",
        "while let Some(candidate) = probes.next().await",
        ".buffered(2)",
        "probe_feed_payload(",
    ]);
    crate::source::assert_excludes(follower, &[
        "settled_payload_wave(",
        "payload_probe_wave(",
        "Vec<Option<(u64, FeedPayloadProbe)>>",
    ]);
    crate::source::assert_contains(follower, &[
        "FeedPayloadProbe::Missing | FeedPayloadProbe::Transient => {",
        "if skipped_missing_index",
        "skipped_missing_index = true",
    ]);
    assert!(!follower.contains("recovered_missing_index"));
    crate::source::assert_contains(follower, &[
        "let Some((appended, closing)) = appended else",
        "continue;",
        "if let Some(closing) = progressed",
    ]);

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
        .find("let Some((appended, closing)) = appended else")
        .unwrap()
        + apply;
    let progressed = follower[stop..].find("if let Some(closing) = progressed").unwrap() + stop;
    assert!(indices < dispatched && dispatched < settled && settled < apply);
    assert!(apply < stop && stop < progressed);

    crate::source::assert_contains(follower, &[
        "now - last_frontier_check >= FEED_FRONTIER_REFRESH_INTERVAL",
        "discover_latest_once(client, owner, topic, None).await",
        "if index < head",
        "hls_history(",
    ]);
    assert!(
        follower.find("HlsPlaylist::parse(&payload.bytes)").unwrap()
            < follower.find("let Some(history) = hls_history(").unwrap()
    );
    let progressed = follower.find("if let Some(closing) = progressed").unwrap();
    let idle_sleep = follower
        .find("async_std::task::sleep(FEED_POLL_INTERVAL).await")
        .unwrap();
    assert!(progressed < idle_sleep);
    assert!(follower.find("drop(probes)").unwrap() < idle_sleep);
    assert!(follower.find("drop(probes)").unwrap() < progressed);
    assert!(follower[progressed..idle_sleep].contains("if closing {\n                        recover_feed_frontier"));
    assert!(!follower.contains("pace_next"));
    assert!(!follower.contains("Duration::try_from_secs_f64"));
}

#[test]
fn active_feed_treats_snapshot_endlist_as_tentative() {
    let install = section(
        HLS_RUNTIME,
        "fn install_snapshot(",
        "async fn discover_beginning(",
    );
    assert!(install.contains("playlist.finalized = false"));

    let apply = section(HLS_RUNTIME, "fn apply_update(", "fn apply_full_update(");
    crate::source::assert_contains(apply, &[
        "index <= current",
        "let appended = merge(playlist)?",
        "let closing = std::mem::take(&mut playlist.finalized)",
        "Some((appended, closing))",
    ]);

    let follow = section(
        HLS_RUNTIME,
        "fn feed_follow_context(",
        "async fn apply_deferred_update(",
    );
    assert!(follow.contains("if feed.playlist.as_ref()?.finalized"));

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
