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
        "live_runway_targets",
        "payload_probe_wave",
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
fn live_autoplay_and_persistent_body_runway_keep_separate_horizons() {
    assert!(HLS_CORE.contains("pub(crate) const HLS_LIVE_SYNC_SEGMENTS: usize = 3;"));
    assert!(HLS_CORE.contains("pub(crate) const HLS_LIVE_EDGE_SEGMENTS: usize = 3;"));
    assert!(HLS_CORE.contains("pub(crate) const HLS_LIVE_BODY_RUNWAY_SEGMENTS: usize = 4;"));
    assert!(
        HLS_RUNTIME.contains("const BODY_PREFETCH_HORIZON: usize = HLS_LIVE_BODY_RUNWAY_SEGMENTS;")
    );
    assert!(HLS_RUNTIME.contains("const HLS_BODY_PREFETCH_MAX_PARALLEL: usize = 3;"));

    let startup = section(
        HLS_CORE,
        "pub(crate) fn startup_plan",
        "pub(crate) fn merge_tail",
    );
    assert!(startup.contains("playable.len() < HLS_LIVE_SYNC_SEGMENTS"));
    assert!(startup.contains("playable.len().saturating_sub(HLS_LIVE_EDGE_SEGMENTS)"));
    assert!(startup.contains("&playable[first..first + HLS_LIVE_SYNC_SEGMENTS]"));

    let runway = section(
        HLS_RUNTIME,
        "fn live_runway_targets(",
        "fn prefetch_from_reference(",
    );
    assert!(runway.contains("active.live_foreground.as_deref()"));
    assert!(runway.contains(".rfind(|(position, segment)|"));
    assert!(runway.contains("live_segment_is_playable(active"));
    assert!(runway.contains("latest_live_foreground(active)"));
    assert!(runway.contains(".take(HLS_LIVE_BODY_RUNWAY_SEGMENTS)"));
    assert!(runway.contains("active.live_runway_running = true"));
    assert!(runway.contains("body_ready_or_pending(&reference)"));
    assert!(runway.contains("cache.pending_body_count(id)"));
    assert!(runway.contains("< HLS_BODY_PREFETCH_MAX_PARALLEL"));
    assert_eq!(
        runway
            .matches("live_runway_context(id).is_some_and(|(_, current)|")
            .count(),
        2
    );
    assert!(runway.contains("HLS_NEXT_RESERVE_STAGGER"));
    assert!(runway.contains("let _ = hls_body(client, reference, Some(id)).await"));
    assert!(runway.contains("Duration::from_millis(25)"));
    assert!(!runway.contains("buffer_unordered"));

    let body_state = section(HLS_RUNTIME, "fn body_ready_or_pending(", "fn body(");
    assert!(body_state.contains("self.bodies.contains_key(reference)"));
    assert!(body_state.contains("self.pending_bodies.contains_key(reference)"));

    let install = section(
        HLS_RUNTIME,
        "fn install_snapshot(",
        "async fn discover_beginning(",
    );
    assert!(install.contains("active.live_foreground = latest_live_foreground(active)"));
    assert!(install.contains("spawn_live_runway(id)"));

    let update = section(HLS_RUNTIME, "fn apply_update(", "fn apply_full_update(");
    assert!(update.contains("updated.0 != 0 && updated.1"));
    assert!(update.contains("spawn_live_runway(id)"));

    let foreground = section(
        HLS_RUNTIME,
        "fn prefetch_from_reference(",
        "fn next_feed_id(",
    );
    assert!(foreground.contains("active.live_foreground = Some(reference.to_string())"));
    assert!(foreground.contains("spawn_live_runway(id)"));
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
    assert!(beginning.contains("spawn_local(async move"));
    assert!(beginning.contains("results.try_send(result)"));
    assert!(beginning.contains("!feed_is_current(id)"));
    assert!(beginning.contains("!result_view_request_is_current(view_generation)"));
    assert!(beginning.contains("playlist.sequence == 0"));
    assert!(beginning.contains("playlist.startup_plan(HlsStart::Beginning).is_some()"));
    assert!(beginning.contains("playable >= BEGINNING_PREFIX_TARGET_SEGMENTS"));
    assert!(beginning.contains("collected_best.borrow_mut()"));
    assert!(beginning.contains("best.borrow_mut().take()"));
    assert!(!beginning.contains(".nth(1)"));
    assert!(beginning.contains("return Some(payload);"));
    assert!(
        beginning.contains("async_std::future::timeout(BEGINNING_WAVE_TIMEOUT, collect.as_mut())")
    );
    assert!(beginning.contains("Err(_) if best.borrow().is_none()"));
    assert!(beginning.contains("collect.await"));
    assert!(
        HLS_RUNTIME
            .contains("const BEGINNING_WAVE_TIMEOUT: Duration = Duration::from_millis(1_500);")
    );
    assert!(!beginning.contains("FEED_DISCOVERY_TIMEOUT"));
    assert!(HLS_RUNTIME.contains("const BEGINNING_PREFIX_TARGET_SEGMENTS: usize = 4;"));
    assert!(HLS_CORE.contains("pub(crate) const HLS_LIVE_SYNC_SEGMENTS: usize = 3;"));
    assert!(HLS_CORE.contains("pub(crate) const HLS_LIVE_EDGE_SEGMENTS: usize = 3;"));

    let edge = section(
        HLS_RUNTIME,
        "async fn discover_edge_update(",
        "async fn discover_latest_once(",
    );
    assert!(HLS_RUNTIME.contains("const EDGE_ANCHORS: [u64; 16]"));
    assert!(HLS_RUNTIME.contains("const EDGE_REFINEMENT_WIDTH: usize = 16;"));
    assert!(edge.contains("edge_probe_wave(client, owner, topic, &EDGE_ANCHORS, false, fast)"));
    assert!(edge.contains("interior.min((EDGE_REFINEMENT_WIDTH - 1) as u64)"));
    assert!(edge.contains("let first = latest.0 + 1;"));
    assert!(edge.contains("indices.push(upper);"));
    assert!(edge.contains("edge_probe_wave(client, owner, topic, &indices, true, fast)"));
    assert!(edge.contains("if latest.0.saturating_add(1) == upper"));
    assert!(edge.contains("return Some(latest);"));
    assert!(!edge.contains("probe_feed_update("));
    assert!(!edge.contains("EDGE_RECOVERY_ANCHORS"));
    assert!(edge.contains("if (latest.0, upper) == previous"));
    assert!(edge.contains("return None;"));

    let wave = section(
        HLS_RUNTIME,
        "async fn edge_probe_wave(",
        "async fn discover_edge_update(",
    );
    assert!(wave.contains("let mut completed = vec![false; indices.len()];"));
    assert!(wave.contains("completed[first_unsettled..=upper]"));
    assert!(wave.contains("spawn_local(async move"));
    assert!(wave.contains("probe_feed_update("));
    assert!(wave.contains("attempt_limit"));
    assert!(wave.contains("results.try_send((slot, result))"));
    assert!(wave.contains("EDGE_COLD_WAVE_TIMEOUT"));
    assert!(wave.contains("EDGE_WAVE_TIMEOUT"));
    assert!(wave.contains("let mut positive_seen = lower_is_known;"));
    assert!(wave.contains("deadline = js_sys::Date::now() + EDGE_WAVE_TIMEOUT"));
    assert!(wave.contains("let Some(upper)"));
    assert!(wave.contains("break;"));

    assert!(HLS_RUNTIME.contains("const EDGE_PROBE_ATTEMPTS: usize = 2;"));
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
        "async fn settled_update_wave(",
    );
    assert!(initial.contains("discover_edge_update(client, owner, topic).await?"));
}

#[test]
fn beginning_runway_starts_exact_following_while_history_reconstructs() {
    let history = section(
        HLS_RUNTIME,
        "fn spawn_beginning_history(",
        "pub(super) fn start_beginning_history()",
    );
    let follow = history.find("spawn_follower(id)").unwrap();
    let discover = history.find("let history = discover_for_view(").unwrap();
    let apply = history
        .find("apply_full_update(id, index, history)")
        .unwrap();
    assert!(follow < discover && discover < apply);
    assert_eq!(history.matches("spawn_follower(id)").count(), 1);
}

#[test]
fn underfilled_beginning_prefix_starts_exact_following_immediately() {
    let attach = section(
        HLS_RUNTIME,
        "pub(crate) async fn attach_hls_feed_player(",
        "pub(crate) fn release_hls_view()",
    );
    let underfilled = attach.find("let underfilled_beginning = !live").unwrap();
    let install = attach
        .find("install_snapshot(id, index, playlist)")
        .unwrap();
    let follow = attach.find("if underfilled_beginning").unwrap();
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
    let settled = section(
        HLS_RUNTIME,
        "async fn settled_update_wave(",
        "async fn retrieve_confirmed_payload(",
    );
    assert!(settled.contains("spawn_local(async move"));
    assert!(settled.contains("probe_feed_update("));
    assert!(settled.contains("Some(FEED_PROBE_ATTEMPTS)"));
    assert!(settled.contains("while let Ok(result) = input.recv().await"));
    assert!(settled.contains("settled.sort_by_key"));

    let proof = section(
        HLS_RUNTIME,
        "async fn retrieve_confirmed_payload(",
        "async fn settled_payload_wave(",
    );
    assert!(proof.contains("let lattice_residue = index % HISTORY_STRIDE;"));
    assert!(proof.contains("index.checked_add(HISTORY_STRIDE)"));
    assert!(proof.contains("first_guard.checked_add(HISTORY_STRIDE)"));
    assert!(
        proof.contains("settled_update_wave(client, owner, topic, &[first_guard, second_guard])")
    );
    assert!(proof.contains("let dense = (1..HISTORY_STRIDE * 2)"));
    assert!(proof.contains(".filter(|offset| *offset != HISTORY_STRIDE)"));
    assert!(proof.contains("if guard_transient || transient"));
    assert!(proof.contains("lattice_residue,"));

    let history = section(
        HLS_RUNTIME,
        "async fn hls_history(",
        "async fn discover_raw_for_view(",
    );
    assert!(history.contains("lattice_residue: u64"));
    assert!(history.contains("let indices = (lattice_residue..head_index)"));
    assert!(HLS_RUNTIME.contains("const HISTORY_FOREGROUND_PARALLEL: usize = 64;"));
}

#[test]
fn live_follower_uses_commit337_sequential_exact_followups_before_frontier_fallback() {
    assert!(HLS_RUNTIME.contains("const FEED_TAIL_PROBE_BYTES: usize = 4 * 1024;"));
    assert!(HLS_RUNTIME.contains("const FEED_FOLLOW_AHEAD: u64 = 4;"));
    assert!(
        HLS_RUNTIME.contains("const FEED_POLL_INTERVAL: Duration = Duration::from_millis(400);")
    );
    assert!(HLS_RUNTIME.contains("const FEED_FRONTIER_REFRESH_INTERVAL: f64 = 15_000.0;"));

    let raw_wave = section(
        HLS_RUNTIME,
        "fn payload_probe_wave(",
        "async fn settled_payload_wave(",
    );
    assert!(raw_wave.contains("for (slot, index) in indices.iter().copied().enumerate()"));
    assert!(raw_wave.contains("spawn_local(async move"));
    assert!(raw_wave.contains("probe_feed_payload("));
    assert!(raw_wave.contains("attempt_limit: Option<usize>"));
    assert!(raw_wave.contains("attempt_limit,"));
    assert!(raw_wave.contains("results.try_send((slot, index, result))"));
    assert!(raw_wave.contains("drop(results)"));
    assert!(raw_wave.contains("input"));

    let follower = section(
        HLS_RUNTIME,
        "fn spawn_follower(",
        "async fn fetch_hls_body_response(",
    );
    assert!(follower.contains("for offset in 1..=FEED_FOLLOW_AHEAD"));
    assert!(follower.contains("head.checked_add(offset)"));
    assert!(follower.contains("let candidate ="));
    assert!(follower.contains("probe_feed_payload("));
    assert!(follower.contains("FEED_TAIL_PROBE_BYTES, None)"));
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
        .find("for offset in 1..=FEED_FOLLOW_AHEAD")
        .unwrap();
    let dispatched = follower[indices..].find("probe_feed_payload(").unwrap() + indices;
    let settled = follower[dispatched..].find(".await;").unwrap() + dispatched;
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
    assert!(follower.contains("discover_latest_once(client, owner, topic).await"));
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
    assert!(install.contains("active.terminal_candidate = terminal.then_some(index)"));

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
