const STREAM: &str = include_str!("../src/stream.rs");
const HLS_STREAM: &str = include_str!("../src/stream_hls.rs");

fn source_section(start: &str, end: &str) -> &'static str {
    let start = STREAM
        .find(start)
        .unwrap_or_else(|| panic!("missing stream source marker: {start}"));
    let tail = &STREAM[start..];
    let end = tail
        .find(end)
        .unwrap_or_else(|| panic!("missing stream source marker: {end}"));
    &tail[..end]
}

fn hls_source_section(start: &str, end: &str) -> &'static str {
    let start = HLS_STREAM
        .find(start)
        .unwrap_or_else(|| panic!("missing HLS stream source marker: {start}"));
    let tail = &HLS_STREAM[start..];
    let end = tail
        .find(end)
        .unwrap_or_else(|| panic!("missing HLS stream source marker: {end}"));
    &tail[..end]
}

#[test]
fn hls_range_has_one_bounded_cache_read_without_an_outer_retry() {
    let adapter = source_section(
        "pub(crate) async fn read_cached_hls_range(",
        "pub(crate) fn hls_aligned_range_cached(",
    );

    assert_eq!(adapter.matches("read_cached_range(").count(), 1);
    assert!(adapter.contains("Duration::from_millis(STREAM_RANGE_REQUEST_TIMEOUT_MS)"));
    assert!(!adapter.contains("read_cached_range_with_retry"));
    assert!(!adapter.contains("STREAM_RANGE_RETRY_COUNT"));
}

#[test]
fn ordinary_media_keeps_its_existing_retry_policy() {
    let ordinary = source_section(
        "async fn read_cached_range_with_retry(",
        "async fn read_cached_range(",
    );

    assert_eq!(ordinary.matches("read_cached_range(").count(), 1);
    assert!(ordinary.contains("generation,\n            None,"));
    assert!(ordinary.contains("STREAM_RANGE_RETRY_COUNT"));
    assert!(ordinary.contains("RANGE_RETRY_DELAY_MS"));
}

#[test]
fn scheduler_cache_probe_accepts_only_exact_storage_windows() {
    let probe = source_section(
        "pub(crate) fn hls_aligned_range_cached(",
        "pub(crate) fn hls_range_body_fully_cached(",
    );

    assert!(probe.contains("range_storage_window_for_start(start, metadata.size) != (start, end)"));
    assert!(probe.contains("cache.borrow().ranges.contains_key(&key)"));
}

#[test]
fn aligned_range_observation_is_exact_read_only_and_exposes_the_stored_generation() {
    let state = source_section(
        "pub(crate) fn range_cache_state(",
        "pub(crate) fn range_cache_observation(",
    );
    assert!(state.contains("range_cache_observation("));
    assert!(state.contains("observation.cached"));
    assert!(state.contains("observation.pending_generation.is_some()"));
    assert!(state.contains("range_cache_state_from_presence("));

    let probe = source_section(
        "pub(crate) fn range_cache_observation(",
        "pub(crate) fn evict_completed_hls_ranges(",
    );

    assert!(probe.contains("resource: &str"));
    assert!(probe.contains("generation: u64"));
    assert!(probe.contains("generation == 0"));
    assert!(probe.contains("metadata.size == 0"));
    assert!(probe.contains("start > end"));
    assert!(probe.contains("end >= metadata.size"));
    assert!(probe.contains("range_storage_window_for_start(start, metadata.size) != (start, end)"));
    assert!(probe.contains("return RangeCacheObservation::default();"));
    assert!(probe.contains("range_cache_key(resource, metadata, start, end)"));
    assert!(probe.contains("pending_range_key(&cache_key, generation)"));
    assert!(probe.contains("cache.ranges.contains_key(&cache_key)"));
    assert!(probe.contains("pending_ranges"));
    assert!(probe.contains(".get(&pending_key)"));
    assert!(probe.contains(".map(|pending| pending.generation)"));
    assert!(probe.contains("let cache = cache.borrow();"));
    assert!(!probe.contains("borrow_mut"));
    assert!(!probe.contains("pending_generation_relation"));
}

#[test]
fn completed_range_state_has_priority_over_an_older_pending_generation() {
    let decision = source_section(
        "fn range_cache_state_from_presence(",
        "pub(crate) fn range_cache_state(",
    );
    let completed = decision.find("if completed").unwrap();
    let pending = decision.find("else if pending").unwrap();

    assert!(completed < pending);
    assert!(decision[completed..pending].contains("RangeCacheState::Cached"));
    assert!(decision[pending..].contains("RangeCacheState::Pending"));
    assert!(decision[pending..].contains("RangeCacheState::Absent"));

    let hls_wrapper = hls_source_section(
        "type HlsAlignedRangeState = RangeCacheState;",
        "impl Weeb3 {",
    );
    assert!(hls_wrapper.contains("fn hls_aligned_range_state("));
    assert!(hls_wrapper.contains("range_cache_state("));
    assert!(hls_wrapper.contains("&format!(\"hls:{reference}\")"));

    let keying = source_section("fn pending_range_key(", "fn range_cache_prefix(");
    assert!(keying.contains("if generation == 0"));
    assert!(keying.contains("\"{cache_key}|pending:media\""));
    assert!(!keying.contains("{generation}"));
}

#[test]
fn rolling_hls_supply_partitions_the_existing_four_range_budget() {
    assert!(HLS_STREAM.contains("pub(crate) const HLS_BACKGROUND_RANGE_MAX: usize = 4;"));
    assert!(HLS_STREAM.contains("pub(crate) const HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX: usize = 1;"));
    assert!(HLS_STREAM.contains("HLS_BACKGROUND_RANGE_MAX - HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX;"));

    let pair = hls_source_section(
        "async fn prefetch_hls_progressive_boundary_prefix(",
        "async fn prefetch_hls_progressive_ranges(",
    );
    assert!(pair.contains("HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX"));
    assert!(pair.contains("HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX"));
    assert!(pair.contains("Some(HlsProgressiveRollingBoundaryState {"));
    assert!(pair.contains("Some(release_tail_width),\n                None,"));
    assert!(pair.contains("join(next_boundary, current_tail).await"));

    let windows = hls_source_section(
        "async fn prefetch_hls_progressive_reference_windows(",
        "async fn prefetch_hls_progressive_boundary_prefix(",
    );
    assert!(windows.contains("hls_progressive_rolling_boundary_width("));
    assert!(windows.contains("boundary.first_window_complete.set(true)"));
    assert!(windows.contains("foreground_position == position"));
    assert!(windows.contains("needs_width_transition_poll"));

    let range_plan = hls_source_section(
        "async fn prefetch_hls_progressive_ranges(",
        "fn spawn_hls_progressive_range_prefetch(",
    );
    assert!(range_plan.contains("let next_boundary_position = position.saturating_add(1)"));
    assert!(range_plan.contains(".get(next_boundary_position)"));
    assert!(!range_plan.contains("saturating_add(2)"));

    let limiter = hls_source_section(
        "fn try_hls_background_range_lease_with_limit(",
        "fn try_hls_progressive_range_lease(",
    );
    assert!(limiter.contains("background.active >= HLS_BACKGROUND_RANGE_MAX"));
    assert!(limiter.contains("background.active = background.active.saturating_add(1)"));
    assert!(limiter.contains("background.reserved_bytes"));
}

#[test]
fn cold_start_future_work_has_no_uncredited_w1_range_lease() {
    let supply = hls_source_section(
        "fn ensure_hls_beginning_prefix_configured(",
        "async fn await_hls_beginning_prefix_barrier(",
    );
    assert!(!supply.contains("hls_beginning_adjacent_range"));
    assert!(!supply.contains("start_hls_beginning_adjacent_range"));
    assert!(!supply.contains("try_hls_background_range_lease("));
    assert!(!supply.contains("HlsBackgroundRangeRequest"));
    assert!(!supply.contains(".retrieve_hls_payload_range("));
    assert!(!supply.contains("HLS_BEGINNING_SPECULATIVE"));

    let physical = source_section(
        "async fn read_range_window(",
        "pub(crate) async fn read_cached_hls_range(",
    );
    assert!(physical.contains("RangeLoadRole::Wait(receiver)"));
    assert!(physical.contains("drop(background_flight.take())"));
    assert!(physical.contains("RangeLoadRole::Lead(receiver, load_id)"));
    assert!(physical.contains("let background_flight = background_flight.take()"));
    assert!(physical.contains("let _background_flight = background_flight"));
    assert!(physical.contains("spawn_local(async move"));
    assert!(physical.contains("STREAM_RANGE_REQUEST_TIMEOUT_MS"));
    assert!(physical.contains("timed out retrieving range"));
    assert!(physical.contains("finish_pending_range("));
}

#[test]
fn credited_raw_supply_opens_after_w0_request_and_fail_closes_failed_or_stale_w0() {
    let supply = hls_source_section(
        "fn ensure_hls_beginning_prefix_configured(",
        "async fn await_hls_beginning_prefix_barrier(",
    );
    let raw = {
        let start = supply.find("fn start_hls_beginning_raw_scout(").unwrap();
        let tail = &supply[start..];
        let end = tail.find("fn hls_beginning_raw_seed(").unwrap();
        &tail[..end]
    };
    let admission = raw.find("hls_beginning_raw_supply_admission_for(").unwrap();
    let traversal = raw
        .find("let payload_ranges = (1..target_windows)")
        .unwrap();

    assert!(admission < traversal);
    assert!(raw.contains("HlsProgressiveRangeAdmission::Park"));
    assert!(raw.contains("HlsProgressiveRangeAdmission::Admit => break"));
    assert!(raw.contains("register_retrieve_cancel_token("));
    assert!(
        raw.matches("hls_beginning_raw_supply_admission_for(")
            .count()
            >= 2
    );
    assert!(supply.contains("gate.foreground_zero_requested = true"));
    assert!(supply.contains("gate.foreground_zero_settled = true"));

    let response = hls_source_section(
        "async fn fetch_hls_bytes_response(",
        "fn hls_bytes_headers(",
    );
    let mark_requested = response
        .find("let beginning_foreground_zero_requested =")
        .unwrap();
    let foreground_dispatch = response[mark_requested..]
        .find(".retrieve_hls_payload_range(")
        .map(|offset| mark_requested + offset)
        .unwrap();
    let fail_closure = response[mark_requested..]
        .find("fail_hls_beginning_foreground_zero(")
        .map(|offset| mark_requested + offset)
        .unwrap();
    let reject_short = response
        .find("if !is_manifest && !expected_len.is_some_and")
        .unwrap();
    let fail_closed = response[reject_short..]
        .find("fail_beginning_foreground_zero();")
        .map(|offset| reject_short + offset)
        .unwrap();
    let mark_settled = response
        .find("mark_hls_beginning_foreground_range_settled(")
        .unwrap();
    assert!(mark_requested < fail_closure && fail_closure < foreground_dispatch);
    assert!(foreground_dispatch < reject_short);
    assert!(reject_short < fail_closed && fail_closed < mark_settled);
    assert_eq!(
        response
            .matches("fail_beginning_foreground_zero();")
            .count(),
        4
    );

    let admission_for = hls_source_section(
        "fn hls_beginning_raw_supply_admission_for(",
        "fn start_hls_beginning_raw_scout(",
    );
    assert!(admission_for.contains("session_stamp != stamp"));
    assert!(admission_for.contains("return HlsProgressiveRangeAdmission::Retire"));

    let generation = hls_source_section(
        "fn advance_generation(&mut self)",
        "fn advance_timeline(&mut self)",
    );
    let close_stale = generation
        .find("self.beginning_prefix.close_raw_scout()")
        .unwrap();
    let replace_generation = generation
        .find("self.generation = next_media_generation()")
        .unwrap();
    assert!(close_stale < replace_generation);

    let timeline = hls_source_section(
        "fn advance_timeline(&mut self)",
        "fn remember_progressive_route(",
    );
    let close_stale = timeline
        .find("self.beginning_prefix.close_raw_scout()")
        .unwrap();
    let replace_timeline = timeline
        .find("self.timeline_epoch = next_nonzero_generation")
        .unwrap();
    assert!(close_stale < replace_timeline);

    let fail = hls_source_section(
        "fn fail_hls_beginning_foreground_zero(",
        "fn mark_hls_beginning_foreground_range_settled(",
    );
    assert!(fail.contains("session_stamp != stamp"));
    assert!(fail.contains("gate.reference.as_deref()"));
    assert!(fail.contains("gate.close_raw_scout()"));
    assert!(fail.contains("gate.phase = HlsBeginningPrefixPhase::Bypass"));
}
