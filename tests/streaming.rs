#![allow(dead_code)]

#[path = "../src/retrieval_conventions.rs"]
mod retrieval_conventions;
#[path = "../src/stream_conventions.rs"]
mod stream_conventions;
#[path = "../src/stream_hls.rs"]
mod stream_hls;

mod network_profile {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum NetworkMode {
        Testnet,
        Mainnet,
    }
}

#[path = "../src/nav.rs"]
mod nav;

mod hls {
    use crate::stream_conventions::HlsStart;
    use crate::stream_hls;

    use std::{
        collections::{HashMap, HashSet, VecDeque},
        sync::Arc,
    };
    use stream_hls::{
        HLS_BACKGROUND_RANGE_MAX, HLS_BEGINNING_PREFIX_MAX_WINDOWS, HLS_LIVE_SYNC_SEGMENTS,
        HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX, HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX,
        HLS_SEQUENCE_ZERO_DISCOVERY_MAX_PROBES, HLS_SPARSE_HISTORY_MAX_PARALLEL,
        HlsBeginningPrefixPhase, HlsCriticalPrefixPlan, HlsDirectArchiveDisposition,
        HlsLevelTransition, HlsManifestProbe, HlsMediaKind, HlsMediaPlanRegistry,
        HlsOrderedWindowState, HlsPlayAttemptPhase, HlsPlayAttemptSettlement, HlsPrefetchMode,
        HlsProgressiveFrontierDecision, HlsProgressiveRangeAdmission, HlsProgressiveRangePurpose,
        HlsProgressiveRunway, HlsProgressiveRunwayTransition, HlsProgressiveRunways,
        HlsSequenceZeroDiscoveryObservation, HlsSequenceZeroDiscoveryPlanner, HlsSequenceZeroRetry,
        MAX_STREAM_FEED_PAYLOAD_BYTES, append_hls_sequence_zero_archive_suffix,
        assemble_hls_sequence_zero_suffix, assemble_hls_sparse_history,
        classify_hls_level_transition, classify_hls_sequence_zero_discovery,
        continue_hls_codec_bootstrap, extend_hls_sequence_zero_archive, gap_hls_segment_range,
        hls_autoplay_gate_ready, hls_autoplay_start_target, hls_beginning_prefix_admission,
        hls_beginning_prefix_barrier_admission, hls_beginning_prefix_window_count,
        hls_beginning_raw_supply_admission, hls_buffered_interval_covers,
        hls_codec_bootstrap_token, hls_contiguous_buffered_ahead,
        hls_deferred_feed_completion_matches, hls_direct_archive_disposition,
        hls_dom_pause_is_explicit, hls_dom_play_is_explicit, hls_finalized_edge_load_target,
        hls_is_finalized, hls_is_long_sequence_zero_checkpoint, hls_last_playable_segment,
        hls_live_autoplay_runway, hls_live_autoplay_runway_ready,
        hls_live_body_schedule_should_spawn, hls_live_frontier_is_ready,
        hls_live_prefetch_references, hls_live_retarget_target, hls_live_retreat_boundary,
        hls_live_startup_holds_generation, hls_live_tail, hls_live_tail_fallback_segment,
        hls_manifest_reload_is_continuous, hls_manifest_reload_is_forward, hls_media_references,
        hls_media_sequence, hls_ordered_window_admissions, hls_payload_mime,
        hls_play_attempt_settlement, hls_progressive_foreground_transition,
        hls_progressive_frontier_decision, hls_progressive_frontier_width,
        hls_progressive_range_admission, hls_progressive_range_reservation_fits,
        hls_progressive_rolling_boundary_width, hls_progressive_rolling_lane_width,
        hls_progressive_runway_closed_after_mode, hls_select_startup_runway_plan,
        hls_sequence_zero_body_from_deferred_prefix, hls_sequence_zero_canonical_is_supported,
        hls_sequence_zero_covers_head, hls_sequence_zero_ordinary_retry,
        hls_sequence_zero_retry_stays_queued, hls_sequence_zero_same_index_archive_is_reusable,
        hls_sequence_zero_sparse_tail, hls_startup_prefix_is_preferred, hls_startup_retry_delay_ms,
        hls_tail_has_terminal_endlist, hls_target_duration, hls_timeline_rebase_position,
        hls_timeline_rebase_required, hls_verified_sequence_zero_checkpoint_tail,
        hls_verified_sequence_zero_checkpoint_tail_at_index, is_hls_manifest,
        open_hls_codec_continuation, plan_hls_sequence_zero_followup_recovery,
        plan_hls_sequence_zero_live_followup, plan_hls_sequence_zero_terminal_confirmation,
        plan_hls_sparse_forward_wave, plan_hls_sparse_history_from_lattice,
        plan_hls_sparse_history_repairs_for_attempts, plan_hls_sparse_terminal_repairs,
        prepend_hls_codec_bootstrap, present_hls_live_fallback, probe_hls_manifest,
        raise_hls_target_duration, remember_hls_sequence_zero_retry,
        retain_hls_sequence_zero_retries_after, rewrite_hls_manifest,
        rewrite_hls_manifest_for_live_reload, select_hls_sequence_zero_retry,
        stream_feed_payload_len_is_supported, touch_hls_cache_lru,
        truncate_hls_live_before_segment,
    };

    const REF: &str = "919b5395bf7a59cbb3b365769de09a2b15ac5d897823dda9270259a3c038d574";
    const REF2: &str = "49428dc8819f560aa3e6226a8c1036a25c091a51d5745de381b842f73243f6d9";
    const REF3: &str = "14aec3fbbb36882d4eba4881fdaa6f2336e5d600b133d677e3f3f5c9d54d8290";
    const REF4: &str = "68d3d40b39d5f17532e928a4b62f2a58ea1b63e20da0eb4b8a7da78d45d45812";
    const OWNER: &str = "352eabdea9cb05e984a8828d2a6df3d3b5023260";
    const TOPIC: &str = "cfbbc155d709547b198638d0fb11d733359561538d8bd606a9ab257354d13bcc";

    fn hls_last_playable_interval(bytes: &[u8]) -> Option<(f64, f64)> {
        hls_last_playable_segment(bytes).map(|(_, _, start, duration)| (start, start + duration))
    }

    fn sparse_manifest(sequence: u64, count: u64, finalized: bool) -> Vec<u8> {
        let mut manifest =
            format!("#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n");
        for position in sequence..sequence.saturating_add(count) {
            manifest.push_str(&format!(
                "#EXTINF:2.0,\n{:064x}\n",
                position.saturating_add(1)
            ));
        }
        if finalized {
            manifest.push_str("#EXT-X-ENDLIST\n");
        }
        manifest.into_bytes()
    }

    struct RollingAdmissions {
        current_tail: Vec<usize>,
        next_boundary: Vec<usize>,
    }

    fn rolling_admissions(
        frontier_width: usize,
        current_states: &[HlsOrderedWindowState],
        current_retry_floor: Option<usize>,
        next_boundary_states: Option<&[HlsOrderedWindowState]>,
        next_boundary_retry_floor: Option<usize>,
        boundary_is_exact_foreground: bool,
    ) -> RollingAdmissions {
        let frontier_width = frontier_width.min(HLS_BACKGROUND_RANGE_MAX);
        let current_complete = current_states
            .iter()
            .all(|state| *state == HlsOrderedWindowState::Cached);
        let reserve_next_boundary = frontier_width == HLS_BACKGROUND_RANGE_MAX
            && next_boundary_states.is_some_and(|states| {
                states
                    .iter()
                    .any(|state| *state != HlsOrderedWindowState::Cached)
            });
        let first_boundary_window_cached = next_boundary_states
            .and_then(|states| states.first())
            .is_some_and(|state| *state == HlsOrderedWindowState::Cached);
        let current_width = if boundary_is_exact_foreground {
            0
        } else if reserve_next_boundary && !current_complete && !first_boundary_window_cached {
            frontier_width.saturating_sub(HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX)
        } else {
            frontier_width
        };
        RollingAdmissions {
            current_tail: hls_ordered_window_admissions(
                current_states,
                current_width,
                current_retry_floor,
            ),
            next_boundary: if reserve_next_boundary {
                hls_ordered_window_admissions(
                    next_boundary_states.unwrap_or_default(),
                    hls_progressive_rolling_boundary_width(
                        frontier_width,
                        first_boundary_window_cached,
                        current_complete,
                        boundary_is_exact_foreground,
                    ),
                    next_boundary_retry_floor,
                )
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn rolling_progressive_runway_hands_off_a_through_d_and_rejects_stale_completion() {
        let mut runway = HlsProgressiveRunway::new(REF.to_string(), Some(REF2.to_string()));
        assert_eq!(runway.current(), REF);
        assert_eq!(runway.successor(), Some(REF2));

        assert_eq!(
            runway.advance(REF2, Some(REF3.to_string())),
            HlsProgressiveRunwayTransition::Sequential
        );
        assert_eq!(runway.current(), REF2);
        assert_eq!(runway.successor(), Some(REF3));

        assert_eq!(
            runway.advance(REF3, Some(REF4.to_string())),
            HlsProgressiveRunwayTransition::Sequential
        );
        assert_eq!(runway.current(), REF3);
        assert_eq!(runway.successor(), Some(REF4));
        assert_eq!(
            runway.advance(REF4, None),
            HlsProgressiveRunwayTransition::Sequential
        );
        assert_eq!(runway.current(), REF4);
        assert_eq!(runway.successor(), None);

        assert_eq!(
            runway.advance(REF, Some(REF2.to_string())),
            HlsProgressiveRunwayTransition::Discontinuity
        );
        assert_eq!(
            runway.advance(REF, Some(REF2.to_string())),
            HlsProgressiveRunwayTransition::Current
        );
    }

    #[test]
    fn progressive_window_frontier_is_nearest_first_and_preserves_the_cold_ramp() {
        assert_eq!(hls_progressive_frontier_width(4, 4, false), 1);
        assert_eq!(hls_progressive_frontier_width(4, 5, false), 4);
        assert_eq!(hls_progressive_frontier_width(4, 4, true), 4);
        assert_eq!(
            hls_progressive_frontier_decision(5, 5, false),
            HlsProgressiveFrontierDecision::Continue,
            "foreground catch-up equality keeps filling the current reference tail"
        );
        assert_eq!(
            hls_progressive_frontier_decision(5, 8, false),
            HlsProgressiveFrontierDecision::Rebase(8),
            "a multi-reference catch-up rebases to the exact foreground reference"
        );
        assert_eq!(
            hls_progressive_frontier_decision(5, 5, true),
            HlsProgressiveFrontierDecision::Advance(6),
            "only completion permits a claim in the next reference"
        );
        assert_eq!(
            hls_progressive_foreground_transition(5, 4, false),
            (true, 4),
            "an uncached adjacent backward request retires the old ticket at ref 4"
        );
        assert_eq!(
            hls_progressive_foreground_transition(5, 4, true),
            (false, 5),
            "cached backward replay preserves the current runway and monotonic floor"
        );
        assert_eq!(
            hls_progressive_foreground_transition(5, 6, false),
            (false, 6)
        );
        assert_eq!(
            hls_progressive_foreground_transition(5, 7, false),
            (true, 7)
        );

        let states = [
            HlsOrderedWindowState::Cached,
            HlsOrderedWindowState::Pending,
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
        ];
        assert_eq!(
            hls_ordered_window_admissions(&states, 1, None),
            Vec::<usize>::new()
        );
        assert_eq!(
            hls_ordered_window_admissions(&states, HLS_BACKGROUND_RANGE_MAX, None),
            vec![2, 3, 4]
        );
        assert_eq!(
            hls_ordered_window_admissions(
                &[
                    HlsOrderedWindowState::Cached,
                    HlsOrderedWindowState::Backoff,
                    HlsOrderedWindowState::Absent,
                ],
                HLS_BACKGROUND_RANGE_MAX,
                Some(1),
            ),
            Vec::<usize>::new(),
            "a terminally absent retry blocks every later window"
        );
        assert_eq!(
            hls_ordered_window_admissions(
                &[
                    HlsOrderedWindowState::Cached,
                    HlsOrderedWindowState::Cached,
                    HlsOrderedWindowState::Absent,
                    HlsOrderedWindowState::Absent,
                ],
                HLS_BACKGROUND_RANGE_MAX,
                Some(2),
            ),
            vec![2],
            "a due retry is the only new admission until it succeeds"
        );
        let simultaneous_ready = [
            HlsOrderedWindowState::Cached,
            HlsOrderedWindowState::Active,
            HlsOrderedWindowState::Backoff,
            HlsOrderedWindowState::Active,
            HlsOrderedWindowState::Absent,
        ];
        assert_eq!(
            hls_ordered_window_admissions(&simultaneous_ready, HLS_BACKGROUND_RANGE_MAX, Some(2),),
            Vec::<usize>::new(),
            "a ready success cannot return capacity past a simultaneously ready failure"
        );
    }

    #[test]
    fn rolling_boundary_uses_future_w0_then_parks_or_promotes() {
        assert_eq!(
            hls_progressive_rolling_boundary_width(HLS_BACKGROUND_RANGE_MAX, false, false, false,),
            HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX,
            "a future boundary admits only W0"
        );
        assert_eq!(
            hls_progressive_rolling_boundary_width(HLS_BACKGROUND_RANGE_MAX, true, false, false,),
            0,
            "after W0 is cached, a future boundary parks without admitting W1"
        );
        assert_eq!(
            hls_progressive_rolling_boundary_width(HLS_BACKGROUND_RANGE_MAX, true, false, true,),
            HLS_BACKGROUND_RANGE_MAX,
            "the exact foreground boundary fills the remaining critical prefix"
        );
        assert_eq!(
            hls_progressive_rolling_boundary_width(HLS_BACKGROUND_RANGE_MAX, true, true, false,),
            HLS_BACKGROUND_RANGE_MAX,
            "a wholly complete current reference releases the boundary remainder"
        );
        assert_eq!(
            hls_progressive_rolling_boundary_width(1, false, false, true),
            1,
            "foreground promotion cannot bypass the cold width-one frontier"
        );
        assert_eq!(
            hls_progressive_rolling_lane_width(
                HLS_BACKGROUND_RANGE_MAX,
                HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX,
                false,
            ),
            HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX
        );
        assert_eq!(
            hls_progressive_rolling_lane_width(
                HLS_BACKGROUND_RANGE_MAX,
                HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX,
                true,
            ),
            HLS_BACKGROUND_RANGE_MAX,
            "W0 completion returns the reserved slot to the current tail"
        );

        let old_tail_active = HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX;
        let globally_available = HLS_BACKGROUND_RANGE_MAX.saturating_sub(old_tail_active);
        assert_eq!(globally_available, HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX);
        assert_eq!(
            old_tail_active + globally_available,
            HLS_BACKGROUND_RANGE_MAX,
            "promotion consumes only slots released under the existing global lease cap"
        );
    }

    #[test]
    fn rolling_progressive_supply_reserves_one_nearest_boundary_lane_after_handoff() {
        assert_eq!(HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX, 1);
        assert_eq!(HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX, 3);
        assert_eq!(
            HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX + HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX,
            HLS_BACKGROUND_RANGE_MAX
        );

        let delayed_tail = [
            HlsOrderedWindowState::Active,
            HlsOrderedWindowState::Active,
            HlsOrderedWindowState::Active,
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
        ];
        let mut next_prefix = [
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
        ];

        let cold_tail = [HlsOrderedWindowState::Absent, HlsOrderedWindowState::Absent];
        let cold = rolling_admissions(1, &cold_tail, None, Some(&next_prefix), None, false);
        assert_eq!(cold.current_tail, vec![0]);
        assert!(
            cold.next_boundary.is_empty(),
            "the reserved boundary lane is disabled while the cold frontier is width one"
        );

        let future = rolling_admissions(
            HLS_BACKGROUND_RANGE_MAX,
            &delayed_tail,
            None,
            Some(&next_prefix),
            None,
            false,
        );
        assert!(future.current_tail.is_empty());
        assert_eq!(future.next_boundary, vec![0]);
        assert_eq!(
            delayed_tail
                .iter()
                .filter(|state| **state == HlsOrderedWindowState::Active)
                .count()
                + future.current_tail.len()
                + future.next_boundary.len(),
            HLS_BACKGROUND_RANGE_MAX,
            "three delayed tail flights plus future W0 stay within cap four"
        );
        next_prefix[0] = HlsOrderedWindowState::Cached;

        let released = rolling_admissions(
            HLS_BACKGROUND_RANGE_MAX,
            &delayed_tail,
            None,
            Some(&next_prefix),
            None,
            false,
        );
        assert_eq!(released.current_tail, vec![3]);
        assert!(
            released.next_boundary.is_empty(),
            "future W1 and later remain parked after W0 returns the fourth tail slot"
        );

        let caught = rolling_admissions(
            HLS_BACKGROUND_RANGE_MAX,
            &delayed_tail,
            None,
            Some(&next_prefix),
            None,
            true,
        );
        assert!(caught.current_tail.is_empty());
        assert_eq!(caught.next_boundary, vec![1, 2]);
        assert_eq!(
            HLS_BACKGROUND_RANGE_MAX.saturating_sub(
                delayed_tail
                    .iter()
                    .filter(|state| **state == HlsOrderedWindowState::Active)
                    .count(),
            ),
            1,
            "the global lease admits only one promoted boundary flight until an old-tail receiver drains"
        );

        let current_complete = [HlsOrderedWindowState::Cached; 5];
        let boundary_after_handoff = [
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
        ];
        let handed_off = rolling_admissions(
            HLS_BACKGROUND_RANGE_MAX,
            &current_complete,
            None,
            Some(&boundary_after_handoff),
            None,
            false,
        );
        assert!(handed_off.current_tail.is_empty());
        assert_eq!(
            handed_off.next_boundary,
            vec![0, 1, 2],
            "once the current tail is wholly cached, its three slots transfer to the boundary"
        );
    }

    #[test]
    fn rolling_boundary_pending_and_failure_block_duplicates_and_farther_fanout() {
        let current = [
            HlsOrderedWindowState::Active,
            HlsOrderedWindowState::Active,
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
        ];
        let pending_boundary = [
            HlsOrderedWindowState::Cached,
            HlsOrderedWindowState::Pending,
            HlsOrderedWindowState::Absent,
        ];
        let pending = rolling_admissions(
            HLS_BACKGROUND_RANGE_MAX,
            &current,
            None,
            Some(&pending_boundary),
            None,
            false,
        );
        assert_eq!(pending.current_tail, vec![2, 3]);
        assert!(
            pending.next_boundary.is_empty(),
            "a shared Pending boundary flight occupies the reserved lane without redispatch"
        );

        let failed_boundary = [
            HlsOrderedWindowState::Cached,
            HlsOrderedWindowState::Backoff,
            HlsOrderedWindowState::Absent,
        ];
        let failed = rolling_admissions(
            HLS_BACKGROUND_RANGE_MAX,
            &current,
            None,
            Some(&failed_boundary),
            Some(1),
            false,
        );
        assert_eq!(failed.current_tail, vec![2, 3]);
        assert!(
            failed.next_boundary.is_empty(),
            "a failed nearest boundary remains a serial blocker; the result has no farther lane"
        );
        let promoted_failure = rolling_admissions(
            HLS_BACKGROUND_RANGE_MAX,
            &current,
            None,
            Some(&failed_boundary),
            Some(1),
            true,
        );
        assert!(promoted_failure.current_tail.is_empty());
        assert!(
            promoted_failure.next_boundary.is_empty(),
            "promotion preserves the terminal backoff blocker instead of fanning out later windows"
        );
        assert_eq!(
            hls_progressive_range_admission(
                false,
                HlsPrefetchMode::Sustained,
                HlsProgressiveRangePurpose::Sustained,
            ),
            HlsProgressiveRangeAdmission::Retire,
            "both rolling lanes share the ticket currentness gate"
        );
    }

    #[test]
    fn progressive_range_admission_parks_pause_and_startup_but_retires_stale_work() {
        assert_eq!(
            hls_progressive_range_admission(
                true,
                HlsPrefetchMode::Inactive,
                HlsProgressiveRangePurpose::StartupExactNext,
            ),
            HlsProgressiveRangeAdmission::Park
        );
        assert_eq!(
            hls_progressive_range_admission(
                true,
                HlsPrefetchMode::StartupOnly,
                HlsProgressiveRangePurpose::StartupExactNext,
            ),
            HlsProgressiveRangeAdmission::Admit
        );
        assert_eq!(
            hls_progressive_range_admission(
                true,
                HlsPrefetchMode::StartupOnly,
                HlsProgressiveRangePurpose::Sustained,
            ),
            HlsProgressiveRangeAdmission::Park
        );
        assert_eq!(
            hls_progressive_range_admission(
                true,
                HlsPrefetchMode::Sustained,
                HlsProgressiveRangePurpose::Sustained,
            ),
            HlsProgressiveRangeAdmission::Admit
        );
        assert_eq!(
            hls_progressive_range_admission(
                false,
                HlsPrefetchMode::Sustained,
                HlsProgressiveRangePurpose::Sustained,
            ),
            HlsProgressiveRangeAdmission::Retire
        );
    }

    #[test]
    fn startup_exact_next_admission_is_independent_of_the_speed_deadline() {
        assert_eq!(
            hls_progressive_range_admission(
                true,
                HlsPrefetchMode::StartupOnly,
                HlsProgressiveRangePurpose::StartupExactNext,
            ),
            HlsProgressiveRangeAdmission::Admit
        );
    }

    #[test]
    fn autoplay_start_target_tracks_the_requested_timeline_instead_of_zero() {
        assert_eq!(
            hls_autoplay_start_target(HlsStart::Beginning, 0.0, 0.0, None, &[(0.067, 4.2)],),
            Some(0.067)
        );
        assert_eq!(
            hls_autoplay_start_target(HlsStart::Live, -1.0, 0.0, Some(3120.0), &[(3116.0, 3125.0)],),
            Some(3120.0)
        );
        assert_eq!(
            hls_autoplay_start_target(
                HlsStart::Live,
                -1.0,
                0.0,
                None,
                &[(3000.0, 3004.0), (3116.0, 3125.0)],
            ),
            Some(3116.0)
        );
        assert_eq!(
            hls_autoplay_start_target(
                HlsStart::Live,
                -1.0,
                3002.0,
                Some(3130.0),
                &[(3000.0, 3004.0), (3116.0, 3125.0)],
            ),
            Some(3116.0),
            "an obsolete island and an out-of-range liveSync must not pull Live backward"
        );
        assert_eq!(
            hls_autoplay_start_target(
                HlsStart::Live,
                -1.0,
                3121.0,
                Some(3130.0),
                &[(3000.0, 3004.0), (3116.0, 3125.0)],
            ),
            Some(3121.0),
            "currentTime is usable only when it is already in the newest island"
        );
        assert_eq!(
            hls_autoplay_start_target(HlsStart::Beginning, 0.0, 0.0, None, &[]),
            None
        );
    }

    #[test]
    fn live_codec_retarget_never_accepts_the_bootstrap_island_as_the_live_edge() {
        assert_eq!(
            hls_live_retarget_target(None, Some(100.0), None, &[(0.0, 4.0)]),
            None
        );
        assert_eq!(
            hls_live_retarget_target(None, Some(100.0), None, &[(0.0, 4.0), (96.0, 104.0)],),
            Some(100.0)
        );
        assert_eq!(
            hls_live_retarget_target(None, None, Some((96.0, 100.0)), &[(0.0, 4.0)]),
            None
        );
        assert_eq!(
            hls_live_retarget_target(None, None, Some((96.0, 100.0)), &[(0.0, 100.0)]),
            Some(98.0),
            "a stopped stream may use only a fully buffered authoritative tail"
        );
        assert_eq!(
            hls_live_retarget_target(
                Some(50.0),
                Some(100.0),
                None,
                &[(0.0, 4.0), (48.0, 60.0), (96.0, 104.0)],
            ),
            Some(50.0),
            "an established Live recovery resumes its buffered DVR position"
        );
    }

    #[test]
    fn finalized_live_retarget_uses_the_last_non_gap_interval_for_short_tail_readiness() {
        let manifest = format!(
            "#EXTM3U\n#EXT-X-VERSION:8\n#EXT-X-TARGETDURATION:4\n\
             #EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4.0,\n{REF}\n\
             #EXTINF:1.0,\n{REF2}\n#EXT-X-GAP\n#EXTINF:4.0,\n{REF3}\n\
             #EXT-X-GAP\n#EXTINF:4.0,\n{REF4}\n#EXT-X-ENDLIST\n"
        );
        let playable = hls_last_playable_interval(manifest.as_bytes()).unwrap();
        assert_eq!(playable, (4.0, 5.0));
        let target = hls_live_retarget_target(None, None, Some(playable), &[(0.0, 5.0)])
            .expect("the final playable second is buffered");
        assert_eq!(target, 4.0);
        let buffered = hls_contiguous_buffered_ahead(target, &[(0.0, 5.0)]);
        assert!(hls_autoplay_gate_ready(buffered, target, playable.1, true));
        assert!(
            !hls_autoplay_gate_ready(buffered, target, 13.0, true),
            "canonical GAP duration is not the playable readiness edge"
        );
    }

    #[test]
    fn progressive_range_reservations_are_overflow_safe_and_share_one_byte_limit() {
        assert_eq!(HLS_BACKGROUND_RANGE_MAX, 4);
        assert!(hls_progressive_range_reservation_fits(8, 4, 3, 15));
        assert!(!hls_progressive_range_reservation_fits(8, 4, 4, 15));
        assert!(!hls_progressive_range_reservation_fits(8, 4, 0, 15));
        assert!(!hls_progressive_range_reservation_fits(
            u64::MAX,
            0,
            1,
            u64::MAX
        ));
    }

    #[test]
    fn beginning_prefix_targets_two_seconds_plus_safety_and_stays_bounded() {
        const WINDOW: u64 = 512 * 1024;
        assert_eq!(
            hls_beginning_prefix_window_count(4_426_084, 4.166_667, WINDOW),
            5,
            "the profiled first segment needs five windows for 2s plus safety"
        );
        assert_eq!(hls_beginning_prefix_window_count(1, 4.0, WINDOW), 1);
        assert_eq!(
            hls_beginning_prefix_window_count(100 * WINDOW, 2.0, WINDOW),
            HLS_BEGINNING_PREFIX_MAX_WINDOWS
        );
        assert_eq!(hls_beginning_prefix_window_count(0, 4.0, WINDOW), 0);
        assert_eq!(hls_beginning_prefix_window_count(WINDOW, 0.0, WINDOW), 0);
        assert_eq!(
            hls_beginning_prefix_window_count(WINDOW, f64::NAN, WINDOW),
            0
        );
        assert_eq!(hls_beginning_prefix_window_count(WINDOW, 4.0, 0), 0);
        let plan = HlsCriticalPrefixPlan::new(4_426_084, 4.166_667, WINDOW).unwrap();
        assert_eq!(plan.critical_windows(), 5);
        assert_eq!(plan.total_windows(), 9);
    }

    #[test]
    fn pending_and_active_windows_occupy_frontier_slots_without_redispatch() {
        let states = [
            HlsOrderedWindowState::Pending,
            HlsOrderedWindowState::Active,
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
            HlsOrderedWindowState::Absent,
        ];
        assert_eq!(
            hls_ordered_window_admissions(&states, HLS_BACKGROUND_RANGE_MAX, None),
            vec![2, 3]
        );
    }

    #[test]
    fn beginning_prefix_barrier_releases_only_ready_bypass_or_live_disabled() {
        for phase in [
            HlsBeginningPrefixPhase::AwaitManifest,
            HlsBeginningPrefixPhase::AwaitForegroundZero,
            HlsBeginningPrefixPhase::Supplying,
        ] {
            assert_eq!(
                hls_beginning_prefix_admission(phase),
                HlsProgressiveRangeAdmission::Park
            );
        }
        for phase in [
            HlsBeginningPrefixPhase::Disabled,
            HlsBeginningPrefixPhase::Ready,
            HlsBeginningPrefixPhase::Bypass,
        ] {
            assert_eq!(
                hls_beginning_prefix_admission(phase),
                HlsProgressiveRangeAdmission::Admit
            );
        }
        assert_eq!(
            hls_beginning_prefix_admission(HlsBeginningPrefixPhase::Retired),
            HlsProgressiveRangeAdmission::Retire
        );

        assert_eq!(
            hls_beginning_prefix_barrier_admission(true, false, HlsBeginningPrefixPhase::Retired,),
            HlsProgressiveRangeAdmission::Admit,
            "a Live follow-up must not inherit a retired beginning-only gate after rebasing",
        );
        assert_eq!(
            hls_beginning_prefix_barrier_admission(false, false, HlsBeginningPrefixPhase::Ready,),
            HlsProgressiveRangeAdmission::Retire,
        );
        assert_eq!(
            hls_beginning_prefix_barrier_admission(
                false,
                true,
                HlsBeginningPrefixPhase::AwaitForegroundZero,
            ),
            HlsProgressiveRangeAdmission::Park,
        );
    }

    #[test]
    fn beginning_raw_supply_opens_after_exact_foreground_zero_is_requested() {
        use HlsProgressiveRangeAdmission::{Admit, Park, Retire};

        assert_eq!(
            hls_beginning_raw_supply_admission(HlsBeginningPrefixPhase::AwaitForegroundZero, false,),
            Park,
            "the raw scout cannot run before the foreground W0 request",
        );
        assert_eq!(
            hls_beginning_raw_supply_admission(HlsBeginningPrefixPhase::Supplying, true,),
            Admit,
            "credit-gated raw work opens while W0 is still settling",
        );
        assert_eq!(
            hls_beginning_raw_supply_admission(HlsBeginningPrefixPhase::Supplying, true,),
            Admit,
            "credited raw work remains open after W0 settles",
        );
        for phase in [
            HlsBeginningPrefixPhase::Disabled,
            HlsBeginningPrefixPhase::AwaitManifest,
            HlsBeginningPrefixPhase::Ready,
            HlsBeginningPrefixPhase::Bypass,
            HlsBeginningPrefixPhase::Retired,
        ] {
            assert_eq!(hls_beginning_raw_supply_admission(phase, true), Retire,);
        }
        assert_eq!(
            hls_beginning_raw_supply_admission(HlsBeginningPrefixPhase::Supplying, false,),
            Retire,
            "an impossible settled-without-request state cannot admit work",
        );
    }

    #[test]
    fn progressive_runways_keep_interleaved_video_and_audio_plans_independent() {
        let mut runways = HlsProgressiveRunways::default();
        runways.set_startup(HlsProgressiveRunway::new(
            "video-0".to_string(),
            Some("video-1".to_string()),
        ));

        assert_eq!(
            runways.advance(1, "video-0", Some("video-1".to_string())),
            HlsProgressiveRunwayTransition::Current
        );
        assert_eq!(
            runways.advance(2, "audio-0", Some("audio-1".to_string())),
            HlsProgressiveRunwayTransition::Current
        );
        assert_eq!(
            runways.advance(1, "video-1", Some("video-2".to_string())),
            HlsProgressiveRunwayTransition::Sequential
        );
        assert_eq!(
            runways.advance(2, "audio-1", Some("audio-2".to_string())),
            HlsProgressiveRunwayTransition::Sequential
        );
        assert!(runways.current(1, "video-1"));
        assert!(runways.contains(1, "video-2"));
        assert!(runways.current(2, "audio-1"));
        assert!(runways.contains(2, "audio-2"));
    }

    #[test]
    fn autoplay_gate_requires_contiguous_buffer_for_beginning_and_live() {
        assert_eq!(
            hls_contiguous_buffered_ahead(0.0, &[(0.0, 1.0), (1.03, 2.4)]),
            2.4
        );
        assert_eq!(
            hls_contiguous_buffered_ahead(0.0, &[(0.0, 1.0), (1.2, 4.0)]),
            1.0
        );
        assert_eq!(
            hls_contiguous_buffered_ahead(1.5, &[(0.0, 2.25), (2.25, 3.0)]),
            1.5
        );
        assert!(!hls_autoplay_gate_ready(1.99, 0.0, 20.0, false));
        assert!(hls_autoplay_gate_ready(2.0, 0.0, 20.0, false));
        assert!(!hls_autoplay_gate_ready(1.5, 0.0, 1.5, false));
        assert!(hls_autoplay_gate_ready(1.46, 0.0, 1.5, true));
        assert!(!hls_autoplay_gate_ready(1.44, 0.0, 1.5, true));
        assert!(hls_autoplay_gate_ready(0.96, 3.0, 4.0, true));
        assert!(!hls_autoplay_gate_ready(1.5, 0.0, f64::INFINITY, true));
        assert!(!hls_autoplay_gate_ready(f64::NAN, 0.0, 1.5, true));
    }

    #[test]
    fn live_autoplay_pins_and_requires_three_playable_tail_fragments() {
        let fragments = (0..10)
            .map(|index| (index as f64 * 4.0, 4.0, false))
            .collect::<Vec<_>>();
        let runway = hls_live_autoplay_runway(&fragments).unwrap();
        assert_eq!(runway, vec![(28.0, 4.0), (32.0, 4.0), (36.0, 4.0)]);
        assert!(!hls_live_autoplay_runway_ready(&runway, &[(28.0, 36.0)]));
        assert!(hls_live_autoplay_runway_ready(&runway, &[(28.0, 40.0)]));

        let mut gapped = fragments;
        gapped[8].2 = true;
        assert_eq!(
            hls_live_autoplay_runway(&gapped),
            Some(vec![(24.0, 4.0), (28.0, 4.0), (36.0, 4.0)]),
            "a GAP slot is not an impossible MediaSource-buffer requirement"
        );
        assert_eq!(hls_live_autoplay_runway(&gapped[..2]), None);
    }

    #[test]
    fn live_prefetch_selects_the_last_three_playable_fragment_bodies() {
        let fifth = format!("{:064x}", 5);
        let live = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:40\n\
             #EXTINF:4.0,\n{REF}\n#EXTINF:4.0,\n{REF2}\n\
             #EXT-X-GAP\n#EXTINF:4.0,\n{REF3}\n\
             #EXTINF:4.0,\n{REF4}\n#EXTINF:4.0,\n{fifth}\n"
        );

        assert_eq!(
            hls_live_prefetch_references(live.as_bytes(), None),
            [REF2.to_string(), REF4.to_string(), fifth],
            "a missing GAP body must not consume one of the three live prefetch slots"
        );
        assert_eq!(
            hls_live_prefetch_references(live.as_bytes(), Some(REF)),
            [REF.to_string(), REF2.to_string(), REF4.to_string()],
            "once playback has a foreground cursor, prefetch must follow it instead of the edge"
        );
        assert!(hls_live_prefetch_references(b"not a manifest", None).is_empty());
    }

    #[test]
    fn live_body_schedule_deduplicates_active_runways_and_retries_incomplete_ones() {
        let old = [REF.to_string(), REF2.to_string(), REF3.to_string()];
        let next = [REF2.to_string(), REF3.to_string(), REF4.to_string()];

        assert!(!hls_live_body_schedule_should_spawn(
            &old,
            &[],
            false,
            false
        ));
        assert!(!hls_live_body_schedule_should_spawn(
            &old, &old, true, false
        ));
        assert!(!hls_live_body_schedule_should_spawn(
            &old, &old, false, true
        ));
        assert!(hls_live_body_schedule_should_spawn(
            &old, &old, false, false
        ));
        assert!(hls_live_body_schedule_should_spawn(
            &old, &next, true, false
        ));
        assert!(!hls_live_body_schedule_should_spawn(
            &old, &next, true, true
        ));
    }

    #[test]
    fn live_startup_holds_generation_only_during_open_pre_sustained_admission() {
        for live_start in [false, true] {
            for body_admission_open in [false, true] {
                for mode in [
                    HlsPrefetchMode::Inactive,
                    HlsPrefetchMode::StartupOnly,
                    HlsPrefetchMode::Sustained,
                ] {
                    for timeline_rebasing in [false, true] {
                        assert_eq!(
                            hls_live_startup_holds_generation(
                                live_start,
                                body_admission_open,
                                mode,
                                timeline_rebasing,
                            ),
                            live_start
                                && body_admission_open
                                && mode != HlsPrefetchMode::Sustained
                                && !timeline_rebasing,
                            "live={live_start}, admission_open={body_admission_open}, \
                             mode={mode:?}, rebasing={timeline_rebasing}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn startup_runway_replaces_only_the_owner_for_the_same_media_kind() {
        let mut active = HashMap::new();

        assert_eq!(
            hls_select_startup_runway_plan(&mut active, HlsMediaKind::Main, Some(11),),
            None
        );
        assert_eq!(
            hls_select_startup_runway_plan(&mut active, HlsMediaKind::Audio, Some(22),),
            None
        );
        assert_eq!(
            hls_select_startup_runway_plan(&mut active, HlsMediaKind::Main, Some(33),),
            Some(11)
        );
        assert_eq!(active.get(&HlsMediaKind::Main), Some(&33));
        assert_eq!(active.get(&HlsMediaKind::Audio), Some(&22));

        assert_eq!(
            hls_select_startup_runway_plan(&mut active, HlsMediaKind::Main, None,),
            Some(33)
        );

        hls_select_startup_runway_plan(&mut active, HlsMediaKind::Main, Some(44));
        hls_select_startup_runway_plan(&mut active, HlsMediaKind::Audio, Some(44));
        assert_eq!(
            hls_select_startup_runway_plan(&mut active, HlsMediaKind::Main, None,),
            None,
            "a plan shared by audio must not retire when main stops using it"
        );
        assert_eq!(active.get(&HlsMediaKind::Audio), Some(&44));

        hls_select_startup_runway_plan(&mut active, HlsMediaKind::Main, Some(44));
        assert_eq!(
            hls_select_startup_runway_plan(&mut active, HlsMediaKind::Main, Some(55),),
            None,
            "audio still keeps the former shared plan active"
        );
        assert_eq!(active.get(&HlsMediaKind::Main), Some(&55));
        assert_eq!(active.get(&HlsMediaKind::Audio), Some(&44));
    }

    #[test]
    fn startup_retry_backoff_is_bounded() {
        assert_eq!(
            (0..7).map(hls_startup_retry_delay_ms).collect::<Vec<_>>(),
            [75, 150, 300, 600, 1_000, 1_000, 1_000]
        );
    }

    #[test]
    fn dom_playback_intent_ignores_autoplay_events_and_preserves_explicit_pause() {
        for phase in [HlsPlayAttemptPhase::Pending, HlsPlayAttemptPhase::Rejecting] {
            assert!(!hls_dom_play_is_explicit(Some(phase), false, false));
            assert!(!hls_dom_play_is_explicit(Some(phase), true, false));
            assert!(!hls_dom_pause_is_explicit(Some(phase), false));
            assert!(!hls_dom_pause_is_explicit(Some(phase), true));
        }
        assert!(
            hls_dom_play_is_explicit(None, false, false),
            "a real manual play after an explicit pause remains authorized"
        );
        assert!(
            !hls_dom_play_is_explicit(None, false, true),
            "a delayed internal play after promise success is not a second authorization"
        );
        assert!(
            !hls_dom_play_is_explicit(None, true, false),
            "a queued play event observed after rollback paused the media is internal"
        );
        assert!(!hls_dom_pause_is_explicit(None, false));
        assert!(hls_dom_pause_is_explicit(None, true));
    }

    #[test]
    fn autoplay_transaction_commits_or_rolls_back_only_the_matching_pending_token() {
        let pending = Some((41, HlsPlayAttemptPhase::Pending));
        assert_eq!(
            hls_play_attempt_settlement(pending, 41, true, true),
            HlsPlayAttemptSettlement::Commit
        );
        assert_eq!(
            hls_play_attempt_settlement(pending, 41, false, true),
            HlsPlayAttemptSettlement::Rollback
        );
        assert_eq!(
            hls_play_attempt_settlement(pending, 40, true, true),
            HlsPlayAttemptSettlement::Stale,
            "a late promise cannot settle the current attempt"
        );
        assert_eq!(
            hls_play_attempt_settlement(Some((41, HlsPlayAttemptPhase::Rejecting)), 41, true, true,),
            HlsPlayAttemptSettlement::Stale,
            "a promise cannot commit after rollback has begun"
        );
        assert_eq!(
            hls_play_attempt_settlement(None, 41, false, true),
            HlsPlayAttemptSettlement::Stale
        );

        let retarget_a = 3120_u64;
        let retarget_b = 3130_u64;
        assert_eq!(
            hls_play_attempt_settlement(pending, 41, true, retarget_a == retarget_b),
            HlsPlayAttemptSettlement::Superseded,
            "promise A cannot authorize after the session retargets to B"
        );
    }

    #[test]
    fn progressive_runway_admission_closes_on_pause_and_reopens_on_resume() {
        let closed = hls_progressive_runway_closed_after_mode(false, HlsPrefetchMode::Inactive);
        assert!(closed);
        assert!(hls_progressive_runway_closed_after_mode(
            closed,
            HlsPrefetchMode::StartupOnly
        ));
        assert!(!hls_progressive_runway_closed_after_mode(
            closed,
            HlsPrefetchMode::Sustained
        ));
    }

    #[test]
    fn hls_cache_lru_keeps_foreground_bodies_at_the_retained_end() {
        let mut order = VecDeque::new();
        touch_hls_cache_lru(&mut order, "background", false);
        touch_hls_cache_lru(&mut order, "foreground", true);
        touch_hls_cache_lru(&mut order, "other", false);
        touch_hls_cache_lru(&mut order, "background", true);
        assert_eq!(
            order.into_iter().collect::<Vec<_>>(),
            ["other", "foreground", "background"]
        );
    }

    #[test]
    fn live_frontier_wait_requires_confirmation_for_the_published_index() {
        assert!(hls_live_frontier_is_ready(48, Some(48), 20.0, 10.0));
        assert!(!hls_live_frontier_is_ready(49, Some(48), 20.0, 10.0));
        assert!(!hls_live_frontier_is_ready(48, None, 20.0, 10.0));
        assert!(!hls_live_frontier_is_ready(48, Some(48), 10.0, 10.0));
    }

    #[test]
    fn stopped_live_fallback_requires_consecutive_evidence_from_the_newest_snapshot() {
        let first = format!("{:064x}", 101);
        let second = format!("{:064x}", 102);
        let next = format!("{:064x}", 103);
        let mut replayed = vec![(517, 760, first.clone())];
        assert_eq!(hls_live_tail_fallback_segment(&replayed), None);
        replayed.push((517, 760, first.clone()));
        assert_eq!(
            hls_live_tail_fallback_segment(&replayed),
            Some((517, 760, first.clone())),
            "replaying earlier good media must not erase the first settled tail failure"
        );

        let failures = vec![(517, 760, first.clone()), (517, 761, second)];
        assert_eq!(
            hls_live_tail_fallback_segment(&failures),
            Some((517, 760, first))
        );

        let mut advanced = failures;
        advanced.push((518, 762, next.clone()));
        assert_eq!(
            hls_live_tail_fallback_segment(&advanced),
            None,
            "an older snapshot failure must not authenticate a new snapshot fallback"
        );
        advanced.push((518, 763, format!("{:064x}", 104)));
        assert_eq!(
            hls_live_tail_fallback_segment(&advanced),
            Some((518, 762, next))
        );
    }

    #[test]
    fn stopped_live_fallback_cuts_the_exact_failed_sn_and_remains_reloadable() {
        let repeated = format!("{:064x}", 99);
        let reference = |position: u64| {
            if matches!(position, 2 | 16 | 18) {
                repeated.clone()
            } else {
                format!("{:064x}", position.saturating_add(1))
            }
        };
        let manifest = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n{}",
            (0..22)
                .map(|position| format!("#EXTINF:4.0,\n{}\n", reference(position)))
                .collect::<String>()
        );

        let (boundary_sequence, boundary_reference) =
            hls_live_retreat_boundary(manifest.as_bytes(), 18, &repeated).unwrap();
        assert_eq!(boundary_sequence, 18);
        assert_eq!(boundary_reference, reference(18));
        assert_eq!(
            hls_live_retreat_boundary(manifest.as_bytes(), 21, &reference(21)),
            Some((19, reference(19))),
            "the three-segment tail retains its two-segment startup preroll"
        );
        let first_retreat = truncate_hls_live_before_segment(
            manifest.as_bytes(),
            boundary_sequence,
            &boundary_reference,
        )
        .unwrap();
        assert_eq!(hls_media_sequence(&first_retreat), Some(0));
        assert_eq!(hls_media_references(&first_retreat).len(), 18);
        assert_eq!(hls_media_references(&first_retreat)[2], repeated);

        let (boundary_sequence, boundary_reference) =
            hls_live_retreat_boundary(&first_retreat, 16, &repeated).unwrap();
        assert_eq!(boundary_sequence, 15);
        let second_retreat = present_hls_live_fallback(
            manifest.as_bytes(),
            boundary_sequence,
            &boundary_reference,
            22,
        )
        .unwrap();
        assert_eq!(hls_media_references(&second_retreat).len(), 15);
        let last_retained = reference(14);
        assert_eq!(
            hls_media_references(&second_retreat)
                .last()
                .map(String::as_str),
            Some(last_retained.as_str())
        );
        assert!(!hls_is_finalized(&second_retreat));
        assert!(
            hls_live_retreat_boundary(manifest.as_bytes(), 9, &repeated).is_none(),
            "the fragment sequence and reference must identify the same exact segment"
        );

        let rewritten = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                &second_retreat,
                "/weeb-3/hls/bytes",
                false,
                HlsStart::Live,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(rewritten.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(rewritten.contains("#EXT-X-START:TIME-OFFSET=-12"));
        assert!(!rewritten.contains("#EXT-X-ENDLIST"));

        let advanced_source = format!(
            "{manifest}#EXTINF:4.0,\n{}\n#EXTINF:4.0,\n{}\n",
            reference(22),
            reference(23)
        );
        let advanced = present_hls_live_fallback(
            advanced_source.as_bytes(),
            boundary_sequence,
            &boundary_reference,
            22,
        )
        .unwrap();
        assert_eq!(
            advanced,
            gap_hls_segment_range(
                advanced_source.as_bytes(),
                boundary_sequence,
                &boundary_reference,
                22,
            )
            .unwrap()
        );
        let advanced_text = String::from_utf8(advanced.clone()).unwrap();
        assert_eq!(advanced_text.matches("#EXT-X-GAP").count(), 7);
        assert_eq!(advanced_text.matches("#EXT-X-VERSION:").count(), 1);
        assert!(advanced_text.contains("#EXT-X-VERSION:8\n"));
        assert_eq!(
            hls_media_references(&advanced),
            (0..15).chain(22..24).map(reference).collect::<Vec<_>>()
        );
        assert!(!hls_is_finalized(&advanced));

        let finalized_source = format!("{manifest}#EXT-X-ENDLIST\n");
        assert_eq!(
            hls_live_retreat_boundary(finalized_source.as_bytes(), 18, &reference(18)),
            Some((18, reference(18))),
            "a settled hls.js priming-fragment failure just before the nominal tail is exact evidence"
        );
        assert_eq!(
            hls_live_retreat_boundary(finalized_source.as_bytes(), 17, &reference(17)),
            Some((17, reference(17)))
        );
        assert_eq!(
            hls_live_retreat_boundary(finalized_source.as_bytes(), 16, &reference(16)),
            None,
            "a finalized fallback must remain bounded to the tail plus its startup preroll"
        );
        let finalized = present_hls_live_fallback(
            finalized_source.as_bytes(),
            boundary_sequence,
            &boundary_reference,
            22,
        )
        .unwrap();
        assert!(hls_is_finalized(&finalized));
        assert_eq!(
            String::from_utf8(finalized.clone())
                .unwrap()
                .matches("#EXT-X-GAP")
                .count(),
            7
        );
        let rewritten_finalized = rewrite_hls_manifest_for_live_reload(
            &finalized,
            "/weeb-3/hls/bytes",
            true,
            HlsStart::Live,
        )
        .unwrap();
        assert!(hls_is_finalized(&rewritten_finalized));
        assert!(!finalized_source.contains("#EXT-X-GAP"));

        let pre_tail =
            present_hls_live_fallback(finalized_source.as_bytes(), 12, &reference(12), 22).unwrap();
        assert!(hls_is_finalized(&pre_tail));
        assert_eq!(hls_last_playable_interval(&pre_tail), Some((44.0, 48.0)));

        let replaced_cutoff = advanced_source.replacen(&boundary_reference, &reference(60), 1);
        assert!(
            present_hls_live_fallback(
                replaced_cutoff.as_bytes(),
                boundary_sequence,
                &boundary_reference,
                22,
            )
            .is_none()
        );

        let without_version = advanced_source.replacen("#EXT-X-VERSION:3\n", "", 1);
        let inserted = gap_hls_segment_range(
            without_version.as_bytes(),
            boundary_sequence,
            &boundary_reference,
            22,
        )
        .unwrap();
        assert!(
            String::from_utf8(inserted)
                .unwrap()
                .starts_with("#EXTM3U\n#EXT-X-VERSION:8\n")
        );

        let bom_without_version = ["\u{feff}".as_bytes(), without_version.as_bytes()].concat();
        let bom_inserted = gap_hls_segment_range(
            &bom_without_version,
            boundary_sequence,
            &boundary_reference,
            22,
        )
        .unwrap();
        assert!(
            String::from_utf8(bom_inserted)
                .unwrap()
                .starts_with("\u{feff}#EXTM3U\n#EXT-X-VERSION:8\n")
        );

        let crlf = advanced_source.replace('\n', "\r\n");
        let crlf_gapped =
            gap_hls_segment_range(crlf.as_bytes(), boundary_sequence, &boundary_reference, 22)
                .unwrap();
        let crlf_gapped = String::from_utf8(crlf_gapped).unwrap();
        assert!(crlf_gapped.contains("#EXT-X-VERSION:8\r\n"));
        assert!(!crlf_gapped.contains("#EXT-X-VERSION:8\n"));

        let duplicate_version = advanced_source.replacen(
            "#EXT-X-VERSION:3\n",
            "#EXT-X-VERSION:3\n#EXT-X-VERSION:4\n",
            1,
        );
        assert!(
            gap_hls_segment_range(
                duplicate_version.as_bytes(),
                boundary_sequence,
                &boundary_reference,
                22,
            )
            .is_none()
        );
    }

    #[test]
    fn finalized_trailing_gap_rewrite_starts_at_the_last_playable_segment() {
        let manifest = format!(
            "#EXTM3U\n#EXT-X-VERSION:8\n#EXT-X-TARGETDURATION:4\n\
             #EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4.0,\n{REF}\n\
             #EXTINF:1.0,\n{REF2}\n#EXT-X-GAP\n#EXTINF:4.0,\n{REF3}\n\
             #EXT-X-GAP\n#EXTINF:4.0,\n{REF4}\n#EXT-X-ENDLIST\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                manifest.as_bytes(),
                "/weeb-3/hls/bytes",
                true,
                HlsStart::Live,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(rewritten.contains("#EXT-X-START:TIME-OFFSET=-9,PRECISE=NO"));
        assert!(rewritten.contains("#EXT-X-ENDLIST"));

        let healthy = format!(
            "#EXTM3U\n#EXT-X-VERSION:8\n#EXT-X-TARGETDURATION:4\n\
             #EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4.0,\n{REF}\n\
             #EXTINF:1.0,\n{REF2}\n#EXT-X-ENDLIST\n"
        );
        let healthy = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                healthy.as_bytes(),
                "/weeb-3/hls/bytes",
                true,
                HlsStart::Live,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(healthy.contains("#EXT-X-START:TIME-OFFSET=-5,PRECISE=NO"));
    }

    #[test]
    fn hls_gap_ordering_skips_absent_media_and_rejects_ambiguous_tags() {
        let first = format!("{:064x}", 1);
        let second = format!("{:064x}", 2);
        let third = format!("{:064x}", 3);
        let both_orders = format!(
            "#EXTM3U\n#EXT-X-VERSION:8\n#EXT-X-TARGETDURATION:4\n\
             #EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-GAP\n#EXTINF:4.0,\n{first}\n\
             #EXTINF:4.0,\n#EXT-X-GAP\n{second}\n#EXTINF:4.0,\n{third}\n"
        );
        assert_eq!(hls_media_references(both_orders.as_bytes()), [third]);
        assert_eq!(hls_live_tail(both_orders.as_bytes()), Some((0, 12.0)));
        assert_eq!(
            hls_last_playable_interval(both_orders.as_bytes()),
            Some((8.0, 12.0))
        );

        let duplicate = both_orders.replacen(
            "#EXT-X-GAP\n#EXTINF:4.0,",
            "#EXT-X-GAP\n#EXTINF:4.0,\n#EXT-X-GAP",
            1,
        );
        assert_eq!(hls_live_tail(duplicate.as_bytes()), None);

        let dangling = format!("{both_orders}#EXT-X-GAP\n");
        assert_eq!(hls_live_tail(dangling.as_bytes()), None);
    }

    #[test]
    fn live_codec_bootstrap_uses_the_recent_predecessor_segment() {
        let bootstrap = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:303\n#EXTINF:4.166667,\n{REF}\n#EXTINF:4.166667,\n{REF2}\n"
        );
        let live = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:304\n#EXTINF:4.166667,\n{REF2}\n#EXTINF:4.166667,\n{REF3}\n"
        );
        let augmented = prepend_hls_codec_bootstrap(live.as_bytes(), bootstrap.as_bytes())
            .expect("a late rolling window can use its preceding feed update");
        let augmented = String::from_utf8(augmented).unwrap();

        assert!(augmented.contains("#EXT-X-MEDIA-SEQUENCE:303"));
        assert!(augmented.contains(&format!(
            "#EXTINF:4.166667,\n{REF}\n#EXT-X-DISCONTINUITY\n#EXTINF:4.166667,\n{REF2}"
        )));
        assert_eq!(
            hls_media_references(augmented.as_bytes()),
            [REF, REF2, REF3]
        );
    }

    #[test]
    fn live_codec_bootstrap_uses_the_first_sequence_zero_segment() {
        let bootstrap = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4.166667,\n{REF}\n#EXTINF:4.166667,\n{REF2}\n"
        );
        let live = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:304\n#EXTINF:4.166667,\n{REF2}\n#EXTINF:4.166667,\n{REF3}\n"
        );
        let augmented = prepend_hls_codec_bootstrap(live.as_bytes(), bootstrap.as_bytes()).unwrap();

        assert_eq!(hls_media_references(&augmented), [REF, REF2, REF3]);
    }

    #[test]
    fn live_codec_bootstrap_continuation_preserves_reload_identity() {
        let fourth = format!("{:064x}", 4);
        let bootstrap = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:303\n#EXTINF:4.0,\n{REF}\n#EXTINF:4.0,\n{REF2}\n"
        );
        let live = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:304\n#EXTINF:4.0,\n{REF2}\n#EXTINF:4.0,\n{REF3}\n"
        );
        let next = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:305\n#EXTINF:4.0,\n{REF3}\n#EXTINF:4.0,\n{fourth}\n"
        );
        let augmented = prepend_hls_codec_bootstrap(live.as_bytes(), bootstrap.as_bytes()).unwrap();
        let continuation = continue_hls_codec_bootstrap(next.as_bytes()).unwrap();

        assert!(hls_manifest_reload_is_continuous(&augmented, &continuation));
        assert!(
            String::from_utf8(continuation)
                .unwrap()
                .contains("#EXT-X-DISCONTINUITY-SEQUENCE:1")
        );
    }

    #[test]
    fn full_sequence_zero_archive_bootstraps_from_its_codec_bearing_first_segment() {
        let segment =
            |position: u64| format!("#EXTINF:2.0,\n{:064x}\n", position.saturating_add(1));
        let full = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n{}#EXT-X-ENDLIST\n",
            (0..12).map(segment).collect::<String>()
        );

        let bootstrap = prepend_hls_codec_bootstrap(full.as_bytes(), full.as_bytes()).unwrap();
        let continuation = continue_hls_codec_bootstrap(full.as_bytes()).unwrap();

        assert_eq!(hls_media_sequence(&bootstrap), Some(0));
        assert_eq!(
            hls_media_references(&bootstrap),
            (0..12)
                .map(|position| format!("{:064x}", position + 1))
                .collect::<Vec<_>>()
        );
        assert!(!hls_is_finalized(&bootstrap));
        assert!(String::from_utf8_lossy(&bootstrap).contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert_eq!(
            String::from_utf8_lossy(&bootstrap)
                .matches("#EXT-X-DISCONTINUITY")
                .count(),
            1
        );
        assert_eq!(hls_media_sequence(&continuation), Some(0));
        assert_eq!(hls_media_references(&continuation).len(), 12);
        assert!(hls_is_finalized(&continuation));
        assert_eq!(
            String::from_utf8_lossy(&continuation)
                .matches("#EXT-X-DISCONTINUITY")
                .count(),
            1
        );
        assert!(hls_manifest_reload_is_continuous(&bootstrap, &continuation));
        assert!(!classify_hls_level_transition(Some((0, true)), false, 0).rebase);
    }

    #[test]
    fn open_codec_continuation_preserves_finalized_gap_start_and_only_defers_endlist() {
        let finalized = format!(
            "#EXTM3U\n#EXT-X-VERSION:8\n#EXT-X-TARGETDURATION:4\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXTINF:4.0,\n{REF}\n#EXTINF:1.0,\n{REF2}\n\
             #EXT-X-GAP\n#EXTINF:4.0,\n{REF3}\n\
             #EXT-X-GAP\n#EXTINF:4.0,\n{REF4}\n#EXT-X-ENDLIST\n"
        );
        let continuation = continue_hls_codec_bootstrap(finalized.as_bytes()).unwrap();
        let settled = rewrite_hls_manifest_for_live_reload(
            &continuation,
            "/weeb-3/hls/bytes/",
            true,
            HlsStart::Live,
        )
        .unwrap();
        let open = open_hls_codec_continuation(&settled).unwrap();
        let settled = String::from_utf8(settled).unwrap();
        let open = String::from_utf8(open).unwrap();

        assert!(settled.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(settled.contains("#EXT-X-ENDLIST"));
        assert!(open.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(!open.contains("#EXT-X-ENDLIST"));
        let settled_start = settled
            .lines()
            .find(|line| line.starts_with("#EXT-X-START:"))
            .unwrap();
        let open_start = open
            .lines()
            .find(|line| line.starts_with("#EXT-X-START:"))
            .unwrap();
        assert_eq!(open_start, settled_start);
        assert_eq!(open.matches("#EXT-X-GAP").count(), 2);
        assert_eq!(hls_media_references(open.as_bytes()).len(), 2);

        let nonzero = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:751\n#EXTINF:4.0,\n{REF}\n\
             #EXTINF:6.0,\n{REF2}\n#EXT-X-ENDLIST\n"
        );
        assert_eq!(
            hls_last_playable_segment(nonzero.as_bytes()),
            Some((752, REF2.to_string(), 4.0, 6.0))
        );
    }

    #[test]
    fn codec_handoff_targets_and_buffer_acknowledgement_fail_closed() {
        assert_eq!(
            hls_codec_bootstrap_token("/feed?start=live&codec-bootstrap=73"),
            Some(73)
        );
        assert_eq!(hls_codec_bootstrap_token("/feed?start=live"), None);
        assert_eq!(hls_codec_bootstrap_token("/feed?codec-bootstrap=old"), None);

        assert_eq!(
            hls_finalized_edge_load_target(Some((96.0, 100.0))),
            Some(98.0)
        );
        assert_eq!(hls_finalized_edge_load_target(Some((4.0, 5.0))), Some(4.0));
        assert_eq!(hls_finalized_edge_load_target(None), None);
        assert_eq!(hls_finalized_edge_load_target(Some((5.0, 4.0))), None);

        assert!(hls_buffered_interval_covers(96.0, 4.0, &[(96.067, 100.0)]));
        assert!(!hls_buffered_interval_covers(96.0, 4.0, &[(96.0, 99.0)]));
        assert!(!hls_buffered_interval_covers(96.0, 4.0, &[(0.0, 4.0)]));
        assert!(!hls_buffered_interval_covers(f64::NAN, 4.0, &[(0.0, 4.0)]));
    }

    #[test]
    fn codec_bootstrap_rejects_segment_state_it_cannot_preserve() {
        let live = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:8\n#EXTINF:4.0,\n{REF2}\n");
        let encrypted = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:4.0,\n{REF}\n"
        );
        let mapped = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.0,\n{REF}\n"
        );

        assert!(prepend_hls_codec_bootstrap(live.as_bytes(), encrypted.as_bytes()).is_none());
        assert!(prepend_hls_codec_bootstrap(live.as_bytes(), mapped.as_bytes()).is_none());
        assert!(continue_hls_codec_bootstrap(encrypted.as_bytes()).is_none());
        assert!(continue_hls_codec_bootstrap(mapped.as_bytes()).is_none());

        let segment =
            |position: u64| format!("#EXTINF:2.0,\n{:064x}\n", position.saturating_add(1));
        let short = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n\
             #EXT-X-MEDIA-SEQUENCE:0\n{}",
            (0..8).map(segment).collect::<String>()
        );
        let discontinuous = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n\
             #EXT-X-MEDIA-SEQUENCE:0\n{}#EXT-X-DISCONTINUITY\n{}",
            (0..4).map(segment).collect::<String>(),
            (4..12).map(segment).collect::<String>()
        );
        let short_bootstrap =
            prepend_hls_codec_bootstrap(short.as_bytes(), short.as_bytes()).unwrap();
        let short_continuation = continue_hls_codec_bootstrap(short.as_bytes()).unwrap();
        assert_eq!(hls_media_sequence(&short_bootstrap), Some(0));
        assert_eq!(hls_media_references(&short_bootstrap).len(), 8);
        assert!(hls_manifest_reload_is_continuous(
            &short_bootstrap,
            &short_continuation
        ));
        assert!(continue_hls_codec_bootstrap(discontinuous.as_bytes()).is_none());
    }

    #[test]
    fn codec_bootstrap_keeps_discontinuity_sequence_before_media() {
        let bootstrap = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4.0,\n{REF}\n");
        let live = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:8\n#EXT-X-DISCONTINUITY-SEQUENCE:4\n#EXTINF:4.0,\n{REF2}\n"
        );
        let augmented = String::from_utf8(
            prepend_hls_codec_bootstrap(live.as_bytes(), bootstrap.as_bytes()).unwrap(),
        )
        .unwrap();
        let continuation =
            String::from_utf8(continue_hls_codec_bootstrap(live.as_bytes()).unwrap()).unwrap();

        assert!(
            augmented.find("#EXT-X-DISCONTINUITY-SEQUENCE:4").unwrap()
                < augmented.find("#EXT-X-MEDIA-SEQUENCE:7").unwrap()
        );
        assert!(continuation.contains("#EXT-X-DISCONTINUITY-SEQUENCE:5"));
    }

    #[test]
    fn media_sequence_defaults_to_zero_and_parses_decimal_zero_consistently() {
        let absent = format!("#EXTM3U\n#EXTINF:2.0,\n{REF}\n");
        let zero_padded = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:000\n#EXTINF:2.0,\n{REF}\n");
        let positive = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:7\n#EXTINF:2.0,\n{REF}\n");
        let duplicate = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.0,\n{REF}\n"
        );
        let malformed =
            format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:not-a-number\n#EXTINF:2.0,\n{REF}\n");

        assert_eq!(hls_media_sequence(absent.as_bytes()), Some(0));
        assert_eq!(hls_media_sequence(zero_padded.as_bytes()), Some(0));
        assert_eq!(hls_media_sequence(positive.as_bytes()), Some(7));
        assert_eq!(hls_media_sequence(duplicate.as_bytes()), None);
        assert_eq!(hls_media_sequence(malformed.as_bytes()), None);

        let master = format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n{REF}.m3u8\n");
        assert_eq!(hls_media_sequence(master.as_bytes()), Some(0));
        assert!(
            hls_media_references(master.as_bytes()).is_empty(),
            "an absent media sequence must not turn a multivariant URI into a media-prefix body"
        );
    }

    #[test]
    fn archive_that_expands_a_rolling_window_requires_a_timeline_rebase() {
        assert!(hls_timeline_rebase_required(636, true, 0));
        assert!(
            !hls_timeline_rebase_required(0, true, 0),
            "a sequence-zero live playlist can become finite without moving its origin"
        );
        assert!(
            !hls_timeline_rebase_required(636, true, 637),
            "a normal forward terminal update keeps the buffered timeline"
        );
        assert!(
            !hls_timeline_rebase_required(636, false, 0),
            "an already finite representation must not repeatedly rebase"
        );
    }

    #[test]
    fn rolling_archive_rebases_once_across_a_clean_session_boundary() {
        let none = HlsLevelTransition { rebase: false };
        let rebase = HlsLevelTransition { rebase: true };

        assert_eq!(classify_hls_level_transition(None, false, 501), none);
        assert_eq!(
            classify_hls_level_transition(Some((501, true)), false, 0),
            rebase
        );
        assert_eq!(
            classify_hls_level_transition(Some((501, true)), true, 0),
            none
        );
    }

    #[test]
    fn timeline_rebase_preserves_distance_from_the_live_edge() {
        let position = hls_timeline_rebase_position(16.016, 13.503, 3_192.362).unwrap();
        assert!((position - 3_189.849).abs() < 0.000_001);
        assert_eq!(hls_timeline_rebase_position(10.0, 12.0, 20.0), Some(20.0));
        assert_eq!(hls_timeline_rebase_position(f64::NAN, 1.0, 2.0), None);
    }

    #[test]
    fn authenticated_sequence_zero_startup_prefix_replaces_a_late_canonical_window() {
        let prefix = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXTINF:2.0,\n{REF}\n#EXTINF:2.0,\n{REF2}\n\
             #EXTINF:2.0,\n{REF3}\n#EXTINF:2.0,\n{REF4}\n"
        );
        let rolling_vod = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:636\n\
             #EXTINF:2.0,\n{REF3}\n"
        );
        assert!(hls_startup_prefix_is_preferred(
            rolling_vod.as_bytes(),
            prefix.as_bytes(),
            4,
        ));

        let rolling_tentative_endlist =
            format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:636\n#EXTINF:2.0,\n{REF3}\n#EXT-X-ENDLIST\n");
        assert!(
            hls_startup_prefix_is_preferred(
                rolling_tentative_endlist.as_bytes(),
                prefix.as_bytes(),
                4,
            ),
            "an authenticated ENDLIST is useful for startup before mutable-head confirmation"
        );

        let rolling_event = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-MEDIA-SEQUENCE:636\n\
             #EXTINF:2.0,\n{REF3}\n"
        );
        let rolling_live = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:636\n#EXTINF:2.0,\n{REF3}\n");
        for canonical in [&rolling_event, &rolling_live] {
            assert!(
                hls_startup_prefix_is_preferred(canonical.as_bytes(), prefix.as_bytes(), 4),
                "the direct unindexed /stream route must start from zero even when the producer leaves its rolling snapshot live, EVENT, or untagged"
            );
        }

        let sequence_zero_vod = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXTINF:2.0,\n{REF3}\n"
        );
        assert!(
            !hls_startup_prefix_is_preferred(sequence_zero_vod.as_bytes(), prefix.as_bytes(), 4,),
            "a canonical sequence-zero view needs no presentation override"
        );

        let short_prefix = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXTINF:2.0,\n{REF}\n#EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
        );
        assert!(!hls_startup_prefix_is_preferred(
            rolling_vod.as_bytes(),
            short_prefix.as_bytes(),
            4,
        ));

        let master_prefix = format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\n{REF}.m3u8\n");
        assert!(
            !hls_startup_prefix_is_preferred(rolling_vod.as_bytes(), master_prefix.as_bytes(), 4,),
            "a multivariant manifest is not an authenticated media prefix"
        );
    }

    #[test]
    fn adaptive_sequence_zero_discovery_classifies_only_complete_media_checkpoints() {
        let malformed_segments = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n{}",
            (0_u64..4)
                .map(|position| format!(
                    "#EXTINF:not-a-duration,\n{:064x}\n",
                    position.saturating_add(1)
                ))
                .collect::<String>()
        );
        assert_eq!(hls_media_references(malformed_segments.as_bytes()).len(), 4);
        assert_eq!(
            classify_hls_sequence_zero_discovery(b"not an HLS manifest", 4),
            HlsSequenceZeroDiscoveryObservation::Unusable
        );
        assert_eq!(
            classify_hls_sequence_zero_discovery(malformed_segments.as_bytes(), 4),
            HlsSequenceZeroDiscoveryObservation::Unusable
        );
        assert_eq!(
            classify_hls_sequence_zero_discovery(
                b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.0,\n",
                4,
            ),
            HlsSequenceZeroDiscoveryObservation::Unusable
        );
        let master = format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n{REF}.m3u8\n");
        assert_eq!(
            classify_hls_sequence_zero_discovery(master.as_bytes(), 4),
            HlsSequenceZeroDiscoveryObservation::Unusable
        );
        assert_eq!(
            classify_hls_sequence_zero_discovery(&sparse_manifest(0, 3, false), 4),
            HlsSequenceZeroDiscoveryObservation::Underfilled
        );
        assert_eq!(
            classify_hls_sequence_zero_discovery(&sparse_manifest(0, 4, false), 4),
            HlsSequenceZeroDiscoveryObservation::Ready
        );
        assert_eq!(
            classify_hls_sequence_zero_discovery(&sparse_manifest(1, 4, false), 4),
            HlsSequenceZeroDiscoveryObservation::Overshoot
        );
    }

    #[test]
    fn sequence_zero_canonical_arbitration_rejects_non_hls_and_malformed_bodies() {
        let master = format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n{REF}.m3u8\n");
        assert!(hls_sequence_zero_canonical_is_supported(master.as_bytes()));
        assert!(!hls_sequence_zero_canonical_is_supported(
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n"
        ));
        assert!(!hls_sequence_zero_canonical_is_supported(
            b"ordinary UTF-8 without an HLS header"
        ));
        assert!(!hls_sequence_zero_canonical_is_supported(&[
            0xff, 0xfe, 0xfd
        ]));
        assert!(!hls_sequence_zero_canonical_is_supported(
            b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:not-a-number\n"
        ));
        assert!(!hls_sequence_zero_canonical_is_supported(
            b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:not-a-duration,\n"
        ));
        assert!(!hls_sequence_zero_canonical_is_supported(
            b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.0,\n"
        ));
        assert!(hls_sequence_zero_canonical_is_supported(&sparse_manifest(
            0, 1, false
        )));
        assert!(hls_sequence_zero_canonical_is_supported(&sparse_manifest(
            9, 1, false
        )));
    }

    #[test]
    fn adaptive_sequence_zero_discovery_reaches_the_e81_checkpoint_without_rate_assumptions() {
        let mut planner = HlsSequenceZeroDiscoveryPlanner::new();
        assert_eq!(planner.next_probe(), Some(3));
        planner.observe(3, HlsSequenceZeroDiscoveryObservation::Underfilled);
        assert_eq!(planner.next_probe(), Some(7));
        planner.observe(7, HlsSequenceZeroDiscoveryObservation::Underfilled);
        assert_eq!(planner.next_probe(), Some(15));
        assert_eq!(
            classify_hls_sequence_zero_discovery(&sparse_manifest(0, 5, false), 4),
            HlsSequenceZeroDiscoveryObservation::Ready
        );
    }

    #[test]
    fn adaptive_sequence_zero_discovery_reserves_half_its_budget_for_refinement() {
        let mut planner = HlsSequenceZeroDiscoveryPlanner::new();
        let geometric = std::iter::from_fn(|| planner.next_probe()).collect::<Vec<_>>();
        assert_eq!(geometric, vec![3, 7, 15, 31]);
        assert!(planner.next_probe().is_none());

        planner.observe(31, HlsSequenceZeroDiscoveryObservation::Overshoot);
        let refinement = std::iter::from_fn(|| planner.next_probe()).collect::<Vec<_>>();
        assert_eq!(refinement.len(), 4);
        assert!(refinement.iter().all(|index| *index < 31));

        let all = geometric
            .into_iter()
            .chain(refinement)
            .collect::<HashSet<_>>();
        assert_eq!(all.len(), HLS_SEQUENCE_ZERO_DISCOVERY_MAX_PROBES);
    }

    #[test]
    fn adaptive_sequence_zero_discovery_refines_the_full_unknown_overshoot_interval() {
        let mut planner = HlsSequenceZeroDiscoveryPlanner::new();
        assert_eq!(planner.next_probe(), Some(3));
        planner.observe(3, HlsSequenceZeroDiscoveryObservation::Unusable);
        assert_eq!(planner.next_probe(), Some(7));
        planner.observe(7, HlsSequenceZeroDiscoveryObservation::Overshoot);

        let mut probes = vec![3, 7];
        while let Some(index) = planner.next_probe() {
            probes.push(index);
            planner.observe(index, HlsSequenceZeroDiscoveryObservation::Unusable);
        }
        assert_eq!(probes.len(), HLS_SEQUENCE_ZERO_DISCOVERY_MAX_PROBES);
        assert_eq!(
            probes.iter().copied().collect::<HashSet<_>>().len(),
            probes.len()
        );
        assert!(probes.iter().all(|index| *index < 8));
        assert!(probes.contains(&0) && probes.contains(&1) && probes.contains(&2));
    }

    #[test]
    fn adaptive_sequence_zero_discovery_uses_real_bounds_and_ignores_missing_probes() {
        let mut missing = HlsSequenceZeroDiscoveryPlanner::new();
        assert_eq!(missing.next_probe(), Some(3));
        missing.observe(3, HlsSequenceZeroDiscoveryObservation::Unusable);
        assert_eq!(missing.next_probe(), Some(7));
        missing.observe(7, HlsSequenceZeroDiscoveryObservation::Unusable);
        assert_eq!(missing.next_probe(), Some(15));

        let mut bracketed = HlsSequenceZeroDiscoveryPlanner::new();
        for index in [3, 7, 15] {
            assert_eq!(bracketed.next_probe(), Some(index));
            bracketed.observe(index, HlsSequenceZeroDiscoveryObservation::Underfilled);
        }
        assert_eq!(bracketed.next_probe(), Some(31));
        bracketed.observe(31, HlsSequenceZeroDiscoveryObservation::Overshoot);
        assert_eq!(bracketed.next_probe(), Some(23));
    }

    #[test]
    fn authenticated_deferred_prefix_presents_only_complete_sequence_zero_segments() {
        let complete = sparse_manifest(0, 5, false);
        let mut authenticated_prefix = complete.clone();
        authenticated_prefix.extend_from_slice(b"#EXTINF:2.0,\npartial-reference");
        let payload_span = u64::try_from(authenticated_prefix.len()).unwrap() + 512;

        assert_eq!(
            hls_sequence_zero_body_from_deferred_prefix(&authenticated_prefix, payload_span, 4),
            Some(complete.clone())
        );
        assert!(
            hls_sequence_zero_body_from_deferred_prefix(
                &authenticated_prefix,
                u64::try_from(authenticated_prefix.len()).unwrap(),
                4,
            )
            .is_none(),
            "a complete body must stay on the RawFeedPayload path"
        );
        assert!(
            hls_sequence_zero_body_from_deferred_prefix(
                &sparse_manifest(0, 3, false),
                payload_span,
                4,
            )
            .is_none()
        );
        assert!(
            hls_sequence_zero_body_from_deferred_prefix(
                &sparse_manifest(1, 5, false),
                payload_span,
                4,
            )
            .is_none()
        );

        let mut terminal_prefix = sparse_manifest(0, 5, true);
        terminal_prefix.extend_from_slice(b"trailing-partial");
        assert!(
            hls_sequence_zero_body_from_deferred_prefix(
                &terminal_prefix,
                u64::try_from(terminal_prefix.len()).unwrap() + 1,
                4,
            )
            .is_none(),
            "a bounded prefix must not erase an authenticated ENDLIST"
        );
    }

    #[test]
    fn deferred_feed_completion_requires_exact_span_and_entire_authenticated_prefix() {
        let prefix = b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n";
        let mut complete = prefix.to_vec();
        complete.extend_from_slice(b"#EXTINF:2.0,\nsegment\n");
        let span = u64::try_from(complete.len()).unwrap();
        assert!(hls_deferred_feed_completion_matches(
            span, prefix, &complete
        ));

        let mut mismatched = complete.clone();
        mismatched[prefix.len() - 1] ^= 1;
        assert!(!hls_deferred_feed_completion_matches(
            span,
            prefix,
            &mismatched
        ));
        assert!(!hls_deferred_feed_completion_matches(
            span + 1,
            prefix,
            &complete
        ));
        assert!(!hls_deferred_feed_completion_matches(
            u64::try_from(prefix.len()).unwrap(),
            prefix,
            prefix
        ));
    }

    #[test]
    fn media_plan_cursors_preserve_playlist_order_and_share_plan_storage() {
        let mut plans = HlsMediaPlanRegistry::new(16);
        plans.install_with_early_overlap_limit(
            ["first", "second", "third"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            2,
            false,
            &HashSet::new(),
        );
        let first = plans.cursor("first", &HashMap::new()).unwrap().cursor;
        let second = plans.cursor("second", &HashMap::new()).unwrap().cursor;
        let third = plans.cursor("third", &HashMap::new()).unwrap().cursor;
        assert_eq!((first.position, second.position, third.position), (0, 1, 2));
        assert!(Arc::ptr_eq(&first.plan, &second.plan));
        assert!(Arc::ptr_eq(&second.plan, &third.plan));
        assert_eq!(first.plan.references.as_ref(), ["first", "second", "third"]);
    }

    #[test]
    fn duplicate_plan_installation_preserves_the_existing_shared_plan() {
        let mut plans = HlsMediaPlanRegistry::new(16);
        let references = ["first", "second", "third"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        plans.install_with_early_overlap_limit(references.clone(), 2, false, &HashSet::new());
        let first = plans.cursor("second", &HashMap::new()).unwrap().cursor;
        plans.install_with_early_overlap_limit(references, 2, false, &HashSet::new());
        let second = plans.cursor("second", &HashMap::new()).unwrap().cursor;
        assert_eq!(first.plan.id, second.plan.id);
        assert!(Arc::ptr_eq(&first.plan, &second.plan));
    }

    #[test]
    fn preferred_plan_keeps_an_active_unrelated_rendition() {
        let mut plans = HlsMediaPlanRegistry::new(16);
        plans.install_with_early_overlap_limit(
            ["old-a", "shared", "old-c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            1,
            false,
            &HashSet::new(),
        );
        let old = plans.cursor("shared", &HashMap::new()).unwrap().cursor;
        plans.install_with_early_overlap_limit(
            ["new-a", "shared", "new-c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            1,
            false,
            &HashSet::new(),
        );
        let latest = plans.cursor("shared", &HashMap::new()).unwrap().cursor;
        let preferred = plans
            .cursor("shared", &HashMap::from([(old.plan.id, old.position)]))
            .unwrap();
        assert_ne!(latest.plan.id, old.plan.id);
        assert_eq!(preferred.cursor.plan.id, old.plan.id);
        assert!(preferred.superseded_plan_ids.is_empty());
    }

    #[test]
    fn live_plan_overlap_migrates_before_the_first_appended_fragment() {
        let mut plans = HlsMediaPlanRegistry::new(64);
        plans.install_with_early_overlap_limit(
            ["a", "b", "c"].into_iter().map(str::to_string).collect(),
            usize::MAX,
            false,
            &HashSet::new(),
        );
        let first = plans.cursor("a", &HashMap::new()).unwrap();
        let old_plan = first.cursor.plan.id;

        plans.install_with_early_overlap_limit(
            ["b", "c", "d", "e"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            usize::MAX,
            false,
            &HashSet::new(),
        );
        let preferred = HashMap::from([(old_plan, 0)]);
        let migrated = plans.cursor("b", &preferred).unwrap();

        assert_ne!(migrated.cursor.plan.id, old_plan);
        assert_eq!(
            migrated.cursor.plan.references.as_ref(),
            ["b", "c", "d", "e"]
        );
        assert_eq!(migrated.cursor.position, 0);
        assert_eq!(migrated.superseded_plan_ids, vec![old_plan]);
    }

    #[test]
    fn plan_migration_rejects_an_unrelated_rendition_with_only_one_shared_asset() {
        let mut plans = HlsMediaPlanRegistry::new(64);
        plans.install_with_early_overlap_limit(
            ["a", "shared", "c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            usize::MAX,
            false,
            &HashSet::new(),
        );
        let old_plan = plans
            .cursor("shared", &HashMap::new())
            .unwrap()
            .cursor
            .plan
            .id;
        plans.install_with_early_overlap_limit(
            ["x", "shared", "z"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            usize::MAX,
            false,
            &HashSet::new(),
        );

        let selected = plans
            .cursor("shared", &HashMap::from([(old_plan, 1)]))
            .unwrap();
        assert_eq!(selected.cursor.plan.id, old_plan);
        assert!(selected.superseded_plan_ids.is_empty());
    }

    #[test]
    fn duplicate_playlist_polls_do_not_create_artificial_plans() {
        let mut plans = HlsMediaPlanRegistry::new(64);
        let references = ["a", "b", "c"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        plans.install_with_early_overlap_limit(
            references.clone(),
            usize::MAX,
            false,
            &HashSet::new(),
        );
        let first = plans.cursor("b", &HashMap::new()).unwrap().cursor.plan.id;
        plans.install_with_early_overlap_limit(references, usize::MAX, false, &HashSet::new());
        let second = plans.cursor("b", &HashMap::new()).unwrap().cursor.plan.id;
        assert_eq!(first, second);
    }

    #[test]
    fn oversized_live_media_plans_retain_the_recent_tail() {
        let mut plans = HlsMediaPlanRegistry::new(3);
        plans.install_with_early_overlap_limit(
            (0..6).map(|position| position.to_string()).collect(),
            usize::MAX,
            true,
            &HashSet::new(),
        );

        assert!(plans.cursor("0", &HashMap::new()).is_none());
        let tail = plans.cursor("3", &HashMap::new()).unwrap().cursor;
        assert_eq!(tail.plan.references.as_ref(), ["3", "4", "5"]);
        assert_eq!(tail.position, 0);
    }

    #[test]
    fn media_plans_retain_their_bounded_early_overlap_limit() {
        let mut plans = HlsMediaPlanRegistry::new(64);
        plans.install_with_early_overlap_limit(
            ["rolling-a", "rolling-b", "rolling-c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            1,
            false,
            &HashSet::new(),
        );
        plans.install_with_early_overlap_limit(
            ["archive-a", "archive-b", "archive-c", "archive-d"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            3,
            false,
            &HashSet::new(),
        );

        assert_eq!(
            plans
                .cursor("rolling-a", &HashMap::new())
                .unwrap()
                .cursor
                .plan
                .early_overlap_limit,
            1
        );
        assert_eq!(
            plans
                .cursor("archive-a", &HashMap::new())
                .unwrap()
                .cursor
                .plan
                .early_overlap_limit,
            3
        );
    }

    #[test]
    fn disjoint_media_plans_do_not_globally_evict_each_other() {
        let mut plans = HlsMediaPlanRegistry::new(3);
        plans.install_with_early_overlap_limit(
            ["old-a", "old-b", "old-c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            1,
            false,
            &HashSet::new(),
        );
        plans.install_with_early_overlap_limit(
            ["new-a", "new-b", "new-c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            1,
            false,
            &HashSet::new(),
        );
        assert!(plans.cursor("old-a", &HashMap::new()).is_some());
        for (position, reference) in ["new-a", "new-b", "new-c"].into_iter().enumerate() {
            let cursor = plans.cursor(reference, &HashMap::new()).unwrap().cursor;
            assert_eq!(cursor.position, position);
            assert_eq!(cursor.plan.references.len(), 3);
        }
    }

    #[test]
    fn rolling_video_plan_preserves_the_active_unrelated_audio_plan() {
        let mut plans = HlsMediaPlanRegistry::new(4);
        plans.install_with_early_overlap_limit(
            ["video-0", "video-1"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            usize::MAX,
            false,
            &HashSet::new(),
        );
        plans.install_with_early_overlap_limit(
            ["audio-0", "audio-1"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            usize::MAX,
            false,
            &HashSet::new(),
        );
        let audio = plans.cursor("audio-1", &HashMap::new()).unwrap().cursor;
        plans.install_with_early_overlap_limit(
            ["video-1", "video-2"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            usize::MAX,
            false,
            &HashSet::new(),
        );

        let selected = plans
            .cursor("audio-1", &HashMap::from([(audio.plan.id, audio.position)]))
            .unwrap();
        assert_eq!(selected.cursor.plan.id, audio.plan.id);
        assert!(plans.cursor("video-2", &HashMap::new()).is_some());
    }

    #[test]
    fn plan_registry_rollover_evicts_inactive_whole_plans_but_preserves_active_tracks() {
        let mut plans = HlsMediaPlanRegistry::new(4);
        plans.install_with_early_overlap_limit(
            vec!["active-video".to_string()],
            usize::MAX,
            false,
            &HashSet::new(),
        );
        let video = plans
            .cursor("active-video", &HashMap::new())
            .unwrap()
            .cursor
            .plan
            .id;
        plans.install_with_early_overlap_limit(
            vec!["active-audio".to_string()],
            usize::MAX,
            false,
            &HashSet::new(),
        );
        let audio = plans
            .cursor("active-audio", &HashMap::new())
            .unwrap()
            .cursor
            .plan
            .id;
        let protected = HashSet::from([video, audio]);
        for index in 0..20 {
            plans.install_with_early_overlap_limit(
                vec![format!("inactive-{index}")],
                usize::MAX,
                false,
                &protected,
            );
        }

        assert!(plans.cursor("active-video", &HashMap::new()).is_some());
        assert!(plans.cursor("active-audio", &HashMap::new()).is_some());
        assert!(plans.cursor("inactive-0", &HashMap::new()).is_none());
        assert!(plans.cursor("inactive-19", &HashMap::new()).is_some());
    }

    #[test]
    fn recognizes_and_rewrites_baked_swarm_gateway_uris() {
        let manifest = format!(
            "#EXTM3U\r\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://gateway.invalid/read/bytes/{REF2}\"\r\n#EXTINF:2.0,\r\nhttps://swarm.beebridge.buzz/read/bytes/{REF}\r\n#EXTINF:2.0,\r\n{REF2}\r\n#EXT-X-ENDLIST\r\n"
        );

        let rewritten = rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes").unwrap();
        let rewritten = String::from_utf8(rewritten).unwrap();

        assert!(rewritten.contains(&format!("URI=\"/weeb-3/hls/bytes/{REF2}\"")));
        assert!(rewritten.contains(&format!("/weeb-3/hls/bytes/{REF}")));
        assert!(rewritten.contains(&format!("/weeb-3/hls/bytes/{REF2}")));
        assert!(!rewritten.contains("swarm.beebridge.buzz"));
        assert!(hls_is_finalized(rewritten.as_bytes()));
    }

    #[test]
    fn provisional_historical_endlist_stays_reloadable_until_the_head_is_confirmed() {
        assert_eq!(HLS_LIVE_SYNC_SEGMENTS, 3);
        let historical = format!(
            "#EXTM3U\r\n#EXT-X-PLAYLIST-TYPE: VOD  \r\n#EXT-X-MEDIA-SEQUENCE:35\r\n\
             #EXTINF:2.0,\r\nhttps://swarm.beebridge.buzz/read/bytes/{REF}\r\n\
             #EXT-X-ENDLIST\r\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                historical.as_bytes(),
                "/weeb-3/hls/bytes",
                false,
                HlsStart::Beginning,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(rewritten.contains("#EXT-X-PLAYLIST-TYPE:EVENT  \n"));
        assert!(!rewritten.contains("PLAYLIST-TYPE: VOD"));
        assert_eq!(
            rewritten
                .matches("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES")
                .count(),
            1
        );
        assert!(!rewritten.contains("#EXT-X-ENDLIST"));
        assert!(!hls_is_finalized(rewritten.as_bytes()));
        assert!(rewritten.contains(&format!("/weeb-3/hls/bytes/{REF}")));
    }

    #[test]
    fn provisional_sliding_window_starts_at_its_first_available_segment() {
        let sliding = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:7\n#EXTINF:2.0,\n{REF}\n");
        let rewritten = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                sliding.as_bytes(),
                "/weeb-3/hls/bytes",
                false,
                HlsStart::Beginning,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(rewritten.starts_with("#EXTM3U\n#EXT-X-START:TIME-OFFSET=0,PRECISE=YES\n"));
        assert_eq!(
            rewritten
                .matches("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES")
                .count(),
            1
        );
        assert!(!rewritten.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
    }

    #[test]
    fn provisional_reload_replaces_duplicate_producer_start_tags_exactly_once() {
        let producer_offsets = format!(
            "#EXTM3U\n#EXT-X-START:TIME-OFFSET=42\n#EXT-X-MEDIA-SEQUENCE:9\n\
             #EXT-X-START:TIME-OFFSET=-6,PRECISE=NO\n#EXTINF:2.0,\n{REF}\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                producer_offsets.as_bytes(),
                "/weeb-3/hls/bytes",
                false,
                HlsStart::Beginning,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            rewritten
                .matches("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES")
                .count(),
            1
        );
        assert_eq!(rewritten.matches("#EXT-X-START:").count(), 1);
        assert!(!rewritten.contains("TIME-OFFSET=42"));
        assert!(!rewritten.contains("TIME-OFFSET=-6"));
    }

    #[test]
    fn live_reload_starts_three_segments_behind_the_current_edge() {
        let segments = [1.0, 1.5, 2.0, 2.5, 1.0, 1.5, 2.0, 2.5, 1.0, 1.5]
            .into_iter()
            .enumerate()
            .map(|(index, duration)| format!("#EXTINF:{duration},\n{index:064x}\n"))
            .collect::<String>();
        let live = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-START:TIME-OFFSET=42\n\
             #EXT-X-MEDIA-SEQUENCE:35\n#EXT-X-START:TIME-OFFSET=-6\n\
             {segments}#EXT-X-ENDLIST\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                live.as_bytes(),
                "/weeb-3/hls/bytes",
                false,
                HlsStart::Live,
            )
            .unwrap(),
        )
        .unwrap();

        assert!(rewritten.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(rewritten.contains("#EXT-X-MEDIA-SEQUENCE:35"));
        assert_eq!(rewritten.matches("#EXT-X-START:").count(), 1);
        assert!(rewritten.contains("#EXT-X-START:TIME-OFFSET=-5,PRECISE=NO"));
        assert_eq!(hls_live_tail(live.as_bytes()), Some((7, 5.0)));
        assert!(!rewritten.contains("TIME-OFFSET=42"));
        assert!(!rewritten.contains("TIME-OFFSET=-6"));
        assert!(!rewritten.contains("#EXT-X-ENDLIST"));
        assert!(rewritten.contains(
            "/weeb-3/hls/bytes/0000000000000000000000000000000000000000000000000000000000000008"
        ));
    }

    #[test]
    fn bom_crlf_live_reload_preserves_unrecognized_lines_and_final_newline() {
        let media = (0..3)
            .map(|index| format!("#EXTINF:2.0,\r\n{index:064x}\r\n"))
            .collect::<String>();
        let source = format!(
            "\u{feff}#EXTM3U\r\n# retained comment  \r\n#EXT-X-VENDOR: value  \r\n\
             #EXT-X-MEDIA-SEQUENCE:9\r\n{media}#EXT-X-ENDLIST\r\n"
        );
        let rewritten = rewrite_hls_manifest_for_live_reload(
            source.as_bytes(),
            "/weeb-3/hls/bytes",
            false,
            HlsStart::Live,
        )
        .unwrap();
        let media = (0..3)
            .map(|index| format!("#EXTINF:2.0,\n/weeb-3/hls/bytes/{index:064x}\n"))
            .collect::<String>();
        assert_eq!(
            String::from_utf8(rewritten).unwrap(),
            format!(
                "\u{feff}#EXTM3U\n#EXT-X-START:TIME-OFFSET=-6,PRECISE=NO\n\
                 # retained comment  \n#EXT-X-VENDOR: value  \n\
                 #EXT-X-MEDIA-SEQUENCE:9\n{media}"
            )
        );
    }

    #[test]
    fn confirmed_final_unindexed_manifest_retains_endlist() {
        let segments = (0..10)
            .map(|value| format!("#EXTINF:2.0,\n{value:064x}\n"))
            .collect::<String>();
        let finalized = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:2\n\
             #EXT-X-MEDIA-SEQUENCE:0\n{segments}#EXT-X-ENDLIST\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                finalized.as_bytes(),
                "/weeb-3/hls/bytes",
                true,
                HlsStart::Beginning,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(rewritten.contains("#EXT-X-PLAYLIST-TYPE:EVENT\n"));
        assert!(!rewritten.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert_eq!(rewritten.matches("#EXT-X-ENDLIST").count(), 1);
        assert!(hls_is_finalized(rewritten.as_bytes()));
        assert_eq!(
            rewritten
                .matches("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES")
                .count(),
            1
        );

        let live = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                finalized.as_bytes(),
                "/weeb-3/hls/bytes",
                true,
                HlsStart::Live,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(live.contains("#EXT-X-START:TIME-OFFSET=-6,PRECISE=NO"));
        assert!(!live.contains("TIME-OFFSET=0"));
        assert!(hls_is_finalized(live.as_bytes()));

        let short = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:2\n\
             #EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.0,\n{REF}\n#EXT-X-ENDLIST\n"
        );
        let short = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(
                short.as_bytes(),
                "/weeb-3/hls/bytes",
                true,
                HlsStart::Live,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(short.contains("#EXT-X-START:TIME-OFFSET=-2,PRECISE=NO"));
    }

    #[test]
    fn exact_manifest_rewrite_preserves_playlist_lifecycle_tags() {
        let exact = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-START:TIME-OFFSET=42\n\
             #EXT-X-MEDIA-SEQUENCE:7\n#EXTINF:2.0,\n\
             https://swarm.beebridge.buzz/read/bytes/{REF}\n#EXT-X-ENDLIST\n"
        );
        let rewritten =
            String::from_utf8(rewrite_hls_manifest(exact.as_bytes(), "/weeb-3/hls/bytes").unwrap())
                .unwrap();
        assert!(rewritten.contains("#EXT-X-PLAYLIST-TYPE:VOD\n"));
        assert!(!rewritten.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(rewritten.contains("#EXT-X-START:TIME-OFFSET=42\n"));
        assert!(!rewritten.contains("TIME-OFFSET=0"));
        assert_eq!(rewritten.matches("#EXT-X-ENDLIST").count(), 1);
        assert!(rewritten.contains(&format!("/weeb-3/hls/bytes/{REF}")));
    }

    #[test]
    fn feed_reload_requires_an_overlapping_unchanged_media_timeline() {
        let old = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2.0,\n{REF}\n#EXTINF:2.0,\n{REF2}\n"
        );
        let rolling = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:6\n#EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
        );
        assert!(hls_manifest_reload_is_continuous(
            old.as_bytes(),
            rolling.as_bytes()
        ));

        let archive_prefix = (0..5)
            .map(|value| format!("#EXTINF:2.0,\n{value:064x}\n"))
            .collect::<String>();
        let full_archive = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n\
             {archive_prefix}#EXTINF:2.0,\n{REF}\n#EXTINF:2.0,\n{REF2}\n#EXT-X-ENDLIST\n"
        );
        assert!(hls_manifest_reload_is_continuous(
            old.as_bytes(),
            full_archive.as_bytes()
        ));

        let jumped = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:20\n#EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
        );
        assert!(!hls_manifest_reload_is_continuous(
            old.as_bytes(),
            jumped.as_bytes()
        ));
        assert!(hls_manifest_reload_is_forward(
            old.as_bytes(),
            jumped.as_bytes()
        ));

        let conflicting = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:6\n#EXTINF:2.0,\n{REF3}\n#EXTINF:2.0,\n{REF2}\n"
        );
        assert!(!hls_manifest_reload_is_continuous(
            old.as_bytes(),
            conflicting.as_bytes()
        ));
        assert!(!hls_manifest_reload_is_forward(
            old.as_bytes(),
            conflicting.as_bytes()
        ));

        let regressed = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2.0,\n{REF}\n");
        assert!(!hls_manifest_reload_is_continuous(
            old.as_bytes(),
            regressed.as_bytes()
        ));
        assert!(!hls_manifest_reload_is_forward(
            old.as_bytes(),
            regressed.as_bytes()
        ));

        let ranged_old = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2.0,\n\
             #EXT-X-BYTERANGE:1000@0\n{REF}\n"
        );
        let ranged_changed = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2.0,\n\
             #EXT-X-BYTERANGE:1000@1000\n{REF}\n#EXTINF:2.0,\n{REF2}\n"
        );
        let ranged_extended = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2.0,\n\
             #EXT-X-BYTERANGE:1000@0\n{REF}\n#EXTINF:2.0,\n{REF2}\n"
        );
        assert!(hls_manifest_reload_is_continuous(
            ranged_old.as_bytes(),
            ranged_extended.as_bytes()
        ));
        assert!(
            !hls_manifest_reload_is_continuous(ranged_old.as_bytes(), ranged_changed.as_bytes()),
            "the same Swarm reference with a different byte range is a different segment"
        );

        let implicit_old = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2.0,\n\
             #EXT-X-BYTERANGE:1000@0\n{REF}\n#EXTINF:2.0,\n\
             #EXT-X-BYTERANGE:1000\n{REF}\n"
        );
        let explicit_successor = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:6\n#EXTINF:2.0,\n\
             #EXT-X-BYTERANGE:1000@1000\n{REF}\n#EXTINF:2.0,\n{REF2}\n"
        );
        assert!(
            hls_manifest_reload_is_continuous(
                implicit_old.as_bytes(),
                explicit_successor.as_bytes()
            ),
            "implicit and explicit spellings of the same effective range must agree"
        );

        let discontinuous_old = format!(
            "#EXTM3U\n#EXT-X-DISCONTINUITY-SEQUENCE:4\n#EXT-X-MEDIA-SEQUENCE:5\n\
             #EXTINF:2.0,\n{REF}\n#EXT-X-DISCONTINUITY\n#EXTINF:2.0,\n{REF2}\n"
        );
        let discontinuous_successor = format!(
            "#EXTM3U\n#EXT-X-DISCONTINUITY-SEQUENCE:5\n#EXT-X-MEDIA-SEQUENCE:6\n\
             #EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
        );
        assert!(
            hls_manifest_reload_is_continuous(
                discontinuous_old.as_bytes(),
                discontinuous_successor.as_bytes()
            ),
            "explicit discontinuity sequence must match the effective overlapping counter"
        );

        let discontinuity_mismatch = format!(
            "#EXTM3U\n#EXT-X-DISCONTINUITY-SEQUENCE:4\n#EXT-X-MEDIA-SEQUENCE:6\n\
             #EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
        );
        assert!(
            !hls_manifest_reload_is_continuous(
                discontinuous_old.as_bytes(),
                discontinuity_mismatch.as_bytes()
            ),
            "the same sequence and URI with a different discontinuity counter cannot be merged"
        );

        let unsupported_old =
            b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2.0,\n/ordinary/media.ts\n";
        assert!(
            !hls_manifest_reload_is_continuous(unsupported_old, rolling.as_bytes()),
            "filtered or unsupported segment URIs must not shift sequence positions"
        );

        let changed_duration = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:6\n#EXTINF:3.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
        );
        assert!(
            !hls_manifest_reload_is_continuous(old.as_bytes(), changed_duration.as_bytes()),
            "an overlapping segment with a different duration changes the presentation timeline"
        );
    }

    #[test]
    fn live_history_appends_locally_across_a_proven_feed_hole() {
        let segment = |sequence: u64| format!("#EXTINF:2.0,\n{sequence:064x}\n");
        let prefix = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n{}",
            (0..3).map(segment).collect::<String>()
        );
        let bridged = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:3\n#EXT-X-MEDIA-SEQUENCE:2\n{}",
            (2..5).map(segment).collect::<String>()
        );
        let nonoverlapping = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:3\n#EXT-X-MEDIA-SEQUENCE:3\n#EXT-X-DISCONTINUITY\n{}",
            (3..5).map(segment).collect::<String>()
        );
        let mut adjacent = prefix.as_bytes().to_vec();
        let mut adjacent_count = 3;
        let mut adjacent_media_end = adjacent.len();
        append_hls_sequence_zero_archive_suffix(
            &mut adjacent,
            &mut adjacent_count,
            &mut adjacent_media_end,
            prefix.as_bytes(),
            nonoverlapping.as_bytes(),
        )
        .expect("an exactly adjacent rolling window proves that no media segment was skipped");
        assert_eq!(adjacent_count, 5);
        assert!(
            std::str::from_utf8(&adjacent)
                .unwrap()
                .contains("#EXT-X-DISCONTINUITY\n#EXTINF:2.0,")
        );
        assert_eq!(
            hls_media_references(&adjacent),
            (0..5)
                .map(|sequence| format!("{sequence:064x}"))
                .collect::<Vec<_>>()
        );

        let mut archive = prefix.into_bytes();
        let current_source = archive.clone();
        let mut segment_count = 3;
        let mut media_end = archive.len();
        append_hls_sequence_zero_archive_suffix(
            &mut archive,
            &mut segment_count,
            &mut media_end,
            &current_source,
            bridged.as_bytes(),
        )
        .expect("the later rolling window proves a one-update hole is safe to bridge");
        raise_hls_target_duration(
            &mut archive,
            hls_target_duration(bridged.as_bytes()).unwrap(),
        )
        .unwrap();

        assert_eq!(segment_count, 5);
        assert_eq!(hls_target_duration(&archive), Some(3));
        assert_eq!(
            hls_media_references(&archive),
            (0..5)
                .map(|sequence| format!("{sequence:064x}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            extend_hls_sequence_zero_archive(&current_source, bridged.as_bytes())
                .and_then(|merged| hls_target_duration(&merged)),
            Some(3),
            "steady-state sequence-zero reloads must retain a later maximum target duration"
        );
    }

    #[test]
    fn adjacent_archive_append_preserves_gap_before_extinf() {
        let prefix = sparse_manifest(0, 2, false);
        let missing = format!("{:064x}", 3);
        let available = format!("{:064x}", 4);
        let adjacent = format!(
            "#EXTM3U\n#EXT-X-VERSION:8\n#EXT-X-TARGETDURATION:2\n\
             #EXT-X-MEDIA-SEQUENCE:2\n#EXT-X-GAP\n#EXTINF:2.0,\n{missing}\n\
             #EXTINF:2.0,\n{available}\n"
        );
        let extended = extend_hls_sequence_zero_archive(&prefix, adjacent.as_bytes())
            .expect("an adjacent merge must retain the GAP and its required protocol version");
        let mut archive = prefix.clone();
        let mut segment_count = 2;
        let mut media_end = archive.len();
        append_hls_sequence_zero_archive_suffix(
            &mut archive,
            &mut segment_count,
            &mut media_end,
            &prefix,
            adjacent.as_bytes(),
        )
        .expect("an adjacent suffix must retain a GAP that precedes its first EXTINF");

        assert_eq!(segment_count, 4);
        for merged in [&extended, &archive] {
            let text = String::from_utf8(merged.clone()).unwrap();
            assert!(text.contains("#EXT-X-GAP\n#EXTINF:2.0,"));
            assert_eq!(text.matches("#EXT-X-VERSION:").count(), 1);
            assert!(text.contains("#EXT-X-VERSION:8\n"));
            assert_eq!(
                hls_media_references(merged),
                [
                    format!("{:064x}", 1),
                    format!("{:064x}", 2),
                    available.clone()
                ]
            );
        }

        let replacement_without_version =
            String::from_utf8(extended.clone())
                .unwrap()
                .replacen("#EXT-X-VERSION:8\n", "", 1);
        let replacement = extend_hls_sequence_zero_archive(
            replacement_without_version.as_bytes(),
            replacement_without_version.as_bytes(),
        )
        .expect("a sequence-zero GAP replacement must insert its own required version");
        assert!(
            String::from_utf8(replacement)
                .unwrap()
                .starts_with("#EXTM3U\n#EXT-X-VERSION:8\n")
        );

        let followup = sparse_manifest(4, 1, false);
        append_hls_sequence_zero_archive_suffix(
            &mut archive,
            &mut segment_count,
            &mut media_end,
            adjacent.as_bytes(),
            &followup,
        )
        .expect("the normalized media cursor must permit a later ordinary in-place append");
        assert_eq!(segment_count, 5);
        assert_eq!(
            hls_media_references(&archive),
            [
                format!("{:064x}", 1),
                format!("{:064x}", 2),
                available,
                format!("{:064x}", 5)
            ]
        );
    }

    #[test]
    fn overlapping_archive_append_elevates_gap_version_failure_atomically() {
        let prefix = String::from_utf8(sparse_manifest(0, 3, false))
            .unwrap()
            .replacen("#EXTM3U\n", "#EXTM3U\n#EXT-X-VERSION:3\n", 1);
        let overlap = format!(
            "#EXTM3U\n#EXT-X-VERSION:8\n#EXT-X-TARGETDURATION:2\n\
             #EXT-X-MEDIA-SEQUENCE:2\n#EXTINF:2.0,\n{:064x}\n\
             #EXT-X-GAP\n#EXTINF:2.0,\n{:064x}\n#EXTINF:2.0,\n{:064x}\n",
            3, 4, 5
        );
        let extended = extend_hls_sequence_zero_archive(prefix.as_bytes(), overlap.as_bytes())
            .expect("an overlapping merge must elevate the retained header version");
        let mut archive = prefix.as_bytes().to_vec();
        let mut segment_count = 3;
        let mut media_end = archive.len();
        append_hls_sequence_zero_archive_suffix(
            &mut archive,
            &mut segment_count,
            &mut media_end,
            prefix.as_bytes(),
            overlap.as_bytes(),
        )
        .expect("an overlapping append must elevate the retained header version");

        assert_eq!(segment_count, 5);
        for merged in [&extended, &archive] {
            let text = String::from_utf8(merged.clone()).unwrap();
            assert_eq!(text.matches("#EXT-X-VERSION:").count(), 1);
            assert!(text.contains("#EXT-X-VERSION:8\n"));
            assert_eq!(
                hls_media_references(merged),
                [
                    format!("{:064x}", 1),
                    format!("{:064x}", 2),
                    format!("{:064x}", 3),
                    format!("{:064x}", 5)
                ]
            );
        }

        let duplicate_version = prefix.replacen(
            "#EXT-X-VERSION:3\n",
            "#EXT-X-VERSION:3\n#EXT-X-VERSION:4\n",
            1,
        );
        let mut rejected_archive = duplicate_version.as_bytes().to_vec();
        let mut rejected_count = 3;
        let mut rejected_media_end = rejected_archive.len();
        let before = (rejected_archive.clone(), rejected_count, rejected_media_end);
        assert!(
            append_hls_sequence_zero_archive_suffix(
                &mut rejected_archive,
                &mut rejected_count,
                &mut rejected_media_end,
                duplicate_version.as_bytes(),
                overlap.as_bytes(),
            )
            .is_none(),
            "ambiguous retained version state must reject before committing the append"
        );
        assert_eq!(
            (rejected_archive, rejected_count, rejected_media_end),
            before
        );
    }

    #[test]
    fn sequence_zero_followup_assembles_one_append_only_suffix() {
        let current = sparse_manifest(0, 20, false);
        let bridge = sparse_manifest(20, 10, false);
        let head = sparse_manifest(30, 10, true);
        let archive = assemble_hls_sequence_zero_suffix(
            19,
            &current,
            39,
            &head,
            std::iter::once((29, bridge.as_slice())),
        )
        .expect("the retained stride bridge extends the cached sequence-zero archive once");

        assert_eq!(hls_media_sequence(&archive), Some(0));
        assert!(hls_is_finalized(&archive));
        assert_eq!(
            hls_media_references(&archive),
            (1..=40)
                .map(|sequence| format!("{sequence:064x}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn sequence_zero_followup_commits_the_highest_appendable_guard_before_a_consumed_hole() {
        let current = sparse_manifest(0, 8, false);
        let appendable = sparse_manifest(8, 10, false);
        let beyond_hole = sparse_manifest(28, 10, false);

        assert!(
            assemble_hls_sequence_zero_suffix(7, &current, 17, &appendable, std::iter::empty(),)
                .is_some(),
            "the first retained guard is safe to publish provisionally"
        );
        assert!(
            assemble_hls_sequence_zero_suffix(
                7,
                &current,
                37,
                &beyond_hole,
                std::iter::once((17, appendable.as_slice())),
            )
            .is_none(),
            "a farther positive must not hide the appendable prefix across a consumed gap"
        );
    }

    #[test]
    fn sparse_live_discovery_leaps_from_a_stale_lower_bound_and_bounds_terminal_repair() {
        let wave = plan_hls_sparse_forward_wave(2_047, 1, HLS_SPARSE_HISTORY_MAX_PARALLEL)
            .expect("bounded forward wave");
        assert_eq!(wave.len(), 64);
        assert_eq!(wave.first(), Some(&2_057));
        assert_eq!(wave.last(), Some(&2_687));
        assert!(wave.windows(2).all(|pair| pair[1] - pair[0] == 10));

        let dense = plan_hls_sparse_terminal_repairs(3_797).expect("bounded terminal repair");
        assert_eq!(dense.len(), 18);
        assert_eq!(dense.first(), Some(&3_798));
        assert_eq!(dense.last(), Some(&3_816));
        assert!(!dense.contains(&3_807));
        assert!(!dense.contains(&3_817));
    }

    #[test]
    fn sequence_zero_live_followup_probes_the_three_segment_frontier() {
        assert_eq!(
            plan_hls_sequence_zero_live_followup(40),
            Some(vec![41, 42, 43])
        );
        assert_eq!(plan_hls_sequence_zero_live_followup(u64::MAX), None);
        assert_eq!(
            plan_hls_sequence_zero_live_followup(u64::MAX - 3),
            Some(vec![u64::MAX - 2, u64::MAX - 1, u64::MAX])
        );
    }

    #[test]
    fn sequence_zero_recovery_moves_past_blockers_with_one_bounded_retry() {
        let (targets, next_cursor) = plan_hls_sequence_zero_followup_recovery(7, 20, Some(42))
            .expect("bounded recovery plan");
        assert_eq!(next_cursor, 24);
        assert_eq!(targets.len(), 23);
        assert_eq!(targets.iter().filter(|index| **index == 42).count(), 1);
        assert!(targets.contains(&8));
        assert!(targets.contains(&26));
        assert!(targets.contains(&27));
        assert!(targets.contains(&30));
        assert!(targets.len() + 2 <= 25, "two guards share the due budget");

        let (deduplicated, _) = plan_hls_sequence_zero_followup_recovery(7, 20, Some(27))
            .expect("a repeated transient stays unique");
        assert_eq!(deduplicated.len(), 22);
        assert_eq!(deduplicated.iter().filter(|index| **index == 27).count(), 1);
        assert!(plan_hls_sequence_zero_followup_recovery(u64::MAX, 20, None).is_none());
    }

    #[test]
    fn sequence_zero_terminal_confirmation_is_one_exact_bounded_neighborhood() {
        let targets = plan_hls_sequence_zero_terminal_confirmation(4_000)
            .expect("bounded terminal confirmation");
        assert_eq!(targets, (4_001..=4_020).collect::<Vec<_>>());
        assert_eq!(targets.len() * 2, 40);
        assert!(plan_hls_sequence_zero_terminal_confirmation(u64::MAX - 19).is_none());
    }

    #[test]
    fn conservative_deferred_tail_requires_endlist_as_the_last_exact_line() {
        assert!(hls_tail_has_terminal_endlist(
            b"segment.ts\n#EXT-X-ENDLIST\n\n"
        ));
        assert!(hls_tail_has_terminal_endlist(
            b"segment.ts\r\n#EXT-X-ENDLIST\r\n"
        ));
        assert!(hls_tail_has_terminal_endlist(
            b"\xfftruncated-prefix\n#EXT-X-ENDLIST\n"
        ));
        assert!(!hls_tail_has_terminal_endlist(
            b"#EXT-X-ENDLIST\nsegment.ts\n"
        ));
        assert!(!hls_tail_has_terminal_endlist(b"#EXT-X-ENDLIST \n"));
        assert!(!hls_tail_has_terminal_endlist(b"#EXT-X-ENDLIST\n#TAG\n"));
        assert!(!hls_tail_has_terminal_endlist(b"\xff#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn sequence_zero_retry_backlog_is_bounded_prioritized_and_pruned() {
        let mut retries = VecDeque::new();
        assert!(remember_hls_sequence_zero_retry(
            &mut retries,
            11,
            false,
            false,
            2
        ));
        assert!(remember_hls_sequence_zero_retry(
            &mut retries,
            12,
            false,
            false,
            2
        ));
        let unchanged = retries.clone();
        assert!(!remember_hls_sequence_zero_retry(
            &mut retries,
            13,
            true,
            true,
            2
        ));
        assert_eq!(retries, unchanged);

        assert!(remember_hls_sequence_zero_retry(
            &mut retries,
            12,
            true,
            true,
            2
        ));
        assert_eq!(retries.front().map(|retry| retry.index), Some(12));
        assert!(retries.front().is_some_and(|retry| retry.authenticated));

        retain_hls_sequence_zero_retries_after(&mut retries, 11);
        assert_eq!(
            retries,
            VecDeque::from([HlsSequenceZeroRetry {
                index: 12,
                authenticated: true,
            }])
        );

        let authenticated = HlsSequenceZeroRetry {
            index: 12,
            authenticated: true,
        };
        let transient = HlsSequenceZeroRetry {
            index: 13,
            authenticated: false,
        };
        assert_eq!(
            select_hls_sequence_zero_retry(Some(99), Some(authenticated), true),
            Some(99)
        );
        assert_eq!(
            select_hls_sequence_zero_retry(Some(99), Some(authenticated), false),
            Some(12)
        );
        assert_eq!(
            select_hls_sequence_zero_retry(Some(99), Some(transient), false),
            Some(99)
        );
        assert_eq!(
            select_hls_sequence_zero_retry(None, Some(authenticated), false),
            Some(12)
        );
        let mixed = VecDeque::from([transient, authenticated]);
        let fair_authenticated = hls_sequence_zero_ordinary_retry(&mixed, true);
        assert_eq!(fair_authenticated, Some(authenticated));
        assert_eq!(
            select_hls_sequence_zero_retry(Some(99), fair_authenticated, false),
            Some(12),
            "an unauthenticated front entry must not hide the authenticated bridge turn"
        );
        assert_eq!(
            hls_sequence_zero_ordinary_retry(&mixed, false),
            Some(transient)
        );
        assert!(hls_sequence_zero_retry_stays_queued(
            false, true, false, false
        ));
        assert!(hls_sequence_zero_retry_stays_queued(
            true, false, true, false
        ));
        assert!(!hls_sequence_zero_retry_stays_queued(
            false, false, true, false
        ));
        assert!(!hls_sequence_zero_retry_stays_queued(
            true, false, true, true
        ));
    }

    #[test]
    fn sparse_history_keeps_the_startup_lattice_when_dense_finds_an_off_residue_head() {
        let head = sparse_manifest(3_788, 10, false);
        let plan = plan_hls_sparse_history_from_lattice(3_798, &head, 7)
            .expect("a ten-segment rolling head supports the startup lattice");
        assert_eq!(plan.lattice_residue, 7);
        assert_eq!(plan.requested_indices.first(), Some(&7));
        assert_eq!(plan.requested_indices.last(), Some(&3_797));
        assert!(plan.requested_indices.iter().all(|index| index % 10 == 7));
    }

    #[test]
    fn sequence_zero_direct_archive_must_cover_every_authenticated_window() {
        let pinned = sparse_manifest(2_038, 10, false);
        let head = sparse_manifest(3_788, 10, false);
        let complete = sparse_manifest(0, 3_798, true);
        let shorter = sparse_manifest(0, 3_797, true);
        assert!(hls_sequence_zero_covers_head(&head, &complete));
        assert!(!hls_sequence_zero_covers_head(&head, &shorter));

        let mut conflicting = String::from_utf8(complete).unwrap();
        conflicting = conflicting.replacen(
            &format!("{:064x}", 2_041_u64),
            &format!("{:064x}", u64::MAX),
            1,
        );
        assert!(hls_sequence_zero_covers_head(&head, conflicting.as_bytes()));
        assert!(!hls_sequence_zero_covers_head(
            &pinned,
            conflicting.as_bytes()
        ));
    }

    #[test]
    fn large_nonterminal_and_stale_deferred_updates_are_nonblocking_edge_evidence() {
        let current_edge = 2_048_u64;
        let large_nonterminal = sparse_manifest(0, current_edge + 1, false);
        assert!(large_nonterminal.len() > 64 * 1024);
        assert_eq!(
            hls_direct_archive_disposition(
                current_edge,
                current_edge - 1,
                current_edge,
                &large_nonterminal,
            ),
            HlsDirectArchiveDisposition::SequenceZeroCheckpoint
        );
        let sparse_tail = hls_sequence_zero_sparse_tail(&large_nonterminal)
            .expect("a large sequence-zero checkpoint has a bounded rolling tail");
        let plan = plan_hls_sparse_history_from_lattice(
            current_edge,
            &sparse_tail,
            (current_edge - 1) % 10,
        )
        .expect("the demoted checkpoint remains reconstructable");
        let windows = plan
            .requested_indices
            .iter()
            .map(|index| {
                let count = index.saturating_add(1).min(10);
                (*index, sparse_manifest(index + 1 - count, count, false))
            })
            .collect::<Vec<_>>();
        let archive = assemble_hls_sparse_history(
            &plan,
            &sparse_tail,
            windows
                .iter()
                .map(|(index, bytes)| (*index, bytes.as_slice())),
        )
        .expect("the current-edge checkpoint tail assembles with its stride history");
        assert_eq!(
            hls_media_references(&archive),
            hls_media_references(&large_nonterminal)
        );

        let small_stale_checkpoint = sparse_manifest(0, 11, false);
        assert!(small_stale_checkpoint.len() <= 64 * 1024);
        assert!(hls_is_long_sequence_zero_checkpoint(
            &small_stale_checkpoint
        ));
        assert_eq!(
            hls_direct_archive_disposition(17, 37, 37, &small_stale_checkpoint),
            HlsDirectArchiveDisposition::Stale
        );

        let stale_terminal = sparse_manifest(0, 2_048, true);
        assert_eq!(
            hls_direct_archive_disposition(2_048, 3_797, 3_798, &stale_terminal),
            HlsDirectArchiveDisposition::Stale
        );
    }

    #[test]
    fn short_sequence_zero_prefix_is_a_sparse_window_not_a_checkpoint() {
        let prefix = sparse_manifest(0, 8, false);
        let later_window = sparse_manifest(28, 10, false);

        assert_eq!(
            hls_verified_sequence_zero_checkpoint_tail(
                &prefix,
                std::iter::once(later_window.as_slice()),
            ),
            Ok(None),
            "a normal early publisher window must not be required to cover the current head"
        );
        assert!(!hls_is_long_sequence_zero_checkpoint(&prefix));
    }

    #[test]
    fn long_sequence_zero_checkpoint_still_must_match_every_pinned_window() {
        let checkpoint = sparse_manifest(0, 38, false);
        let pinned = sparse_manifest(28, 10, false);
        let tail = hls_verified_sequence_zero_checkpoint_tail(
            &checkpoint,
            std::iter::once(pinned.as_slice()),
        )
        .expect("the matching checkpoint is not contradictory")
        .expect("a long checkpoint has a sparse tail");
        assert_eq!(hls_media_references(&tail), hls_media_references(&pinned));

        let mut conflicting = String::from_utf8(pinned).unwrap();
        conflicting = conflicting.replacen(
            &format!("{:064x}", 31_u64),
            &format!("{:064x}", u64::MAX),
            1,
        );
        assert_eq!(
            hls_verified_sequence_zero_checkpoint_tail(
                &checkpoint,
                std::iter::once(conflicting.as_bytes()),
            ),
            Err(()),
            "a long checkpoint must never overwrite a conflicting authenticated timeline"
        );
    }

    #[test]
    fn delayed_followup_checkpoint_ignores_only_later_feed_windows() {
        let checkpoint = sparse_manifest(0, 18, false);
        let prior = sparse_manifest(8, 10, false);
        let later = sparse_manifest(18, 10, false);

        let tail = hls_verified_sequence_zero_checkpoint_tail_at_index(
            17,
            &checkpoint,
            [(17, prior.as_slice()), (27, later.as_slice())],
        )
        .expect("a later out-of-order guard cannot contradict an earlier checkpoint")
        .expect("the long checkpoint is retained as a sparse tail");
        assert_eq!(hls_media_references(&tail), hls_media_references(&prior));
        assert_eq!(
            hls_verified_sequence_zero_checkpoint_tail(
                &checkpoint,
                [prior.as_slice(), later.as_slice()],
            ),
            Err(()),
            "startup's all-pinned validation remains strict"
        );
    }

    #[test]
    fn full_sequence_zero_checkpoint_bridges_a_consumed_feed_gap_before_tail_updates() {
        let current = sparse_manifest(0, 8, false);
        let checkpoint = sparse_manifest(0, 26, false);
        let tail = hls_verified_sequence_zero_checkpoint_tail_at_index(
            25,
            &checkpoint,
            std::iter::once((7, current.as_slice())),
        )
        .expect("the checkpoint agrees with the active archive")
        .expect("the checkpoint also retains its bounded sparse tail");

        assert!(hls_sequence_zero_covers_head(&current, &checkpoint));
        assert_eq!(hls_media_sequence(&tail), Some(16));
        assert_eq!(hls_media_references(&checkpoint).len(), 26);
        assert_eq!(hls_media_references(&tail).len(), 10);
    }

    #[test]
    fn same_index_terminal_archive_is_reusable_only_with_matching_endlist_parity() {
        let source = sparse_manifest(20, 10, true);
        let archive = sparse_manifest(0, 30, true);
        let tentative = sparse_manifest(0, 30, false);

        assert!(hls_sequence_zero_same_index_archive_is_reusable(
            &source, &archive
        ));
        assert!(!hls_sequence_zero_same_index_archive_is_reusable(
            &source, &tentative
        ));
    }

    #[test]
    fn sparse_repair_skips_consumed_feed_holes_when_media_coverage_is_proven() {
        let lower = sparse_manifest(0, 10, false);
        let abutting_head = sparse_manifest(10, 10, false);
        let abutting_plan = plan_hls_sparse_history_from_lattice(27, &abutting_head, 7)
            .expect("abutting sparse plan");
        let no_repairs = plan_hls_sparse_history_repairs_for_attempts(
            &abutting_plan,
            &abutting_head,
            [7, 17],
            [(7, lower.as_slice())],
        )
        .expect("a consumed feed index with proven media coverage is harmless");
        assert!(no_repairs.is_empty());

        let gapped_head = sparse_manifest(20, 10, false);
        let gapped_plan =
            plan_hls_sparse_history_from_lattice(27, &gapped_head, 7).expect("gapped sparse plan");
        let repairs = plan_hls_sparse_history_repairs_for_attempts(
            &gapped_plan,
            &gapped_head,
            [7, 17],
            [(7, lower.as_slice())],
        )
        .expect("an uncovered media interval requires dense repair");
        assert_eq!(repairs.len(), 18);
        assert!(!repairs.contains(&17));
        assert_eq!(repairs.first(), Some(&8));
        assert_eq!(repairs.last(), Some(&26));
    }

    #[test]
    fn sparse_history_repairs_and_assembles_across_a_consumed_lattice_index() {
        let feed_7 = sparse_manifest(0, 8, false);
        let feed_17 = sparse_manifest(8, 10, false);
        let head_37 = sparse_manifest(28, 10, false);
        let plan = plan_hls_sparse_history_from_lattice(37, &head_37, 7)
            .expect("head uses the startup residue");
        assert_eq!(plan.requested_indices, [7, 17, 27]);

        let repairs = plan_hls_sparse_history_repairs_for_attempts(
            &plan,
            &head_37,
            [7, 17, 27],
            [(7, feed_7.as_slice()), (17, feed_17.as_slice())],
        )
        .expect("the missing lattice update has bounded dense repair intervals");
        assert_eq!(repairs, (18..27).chain(28..37).collect::<Vec<_>>());

        let feed_18 = sparse_manifest(9, 10, false);
        let feed_28 = sparse_manifest(19, 10, false);
        let archive = assemble_hls_sparse_history(
            &plan,
            &head_37,
            [
                (7, feed_7.as_slice()),
                (17, feed_17.as_slice()),
                (18, feed_18.as_slice()),
                (28, feed_28.as_slice()),
            ],
        )
        .expect("dense bridge candidates cover the exact sequence-zero history");
        assert_eq!(hls_media_sequence(&archive), Some(0));
        assert_eq!(
            hls_media_references(&archive),
            (0..38)
                .map(|position| format!("{:064x}", position + 1))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn incremental_archive_append_is_ordered_and_failure_atomic() {
        let segments = |start, end| {
            (start..end)
                .map(|index| format!("#EXTINF:2.0,\n{index:064x}\n"))
                .collect::<String>()
        };
        let window = |start, end, finalized| {
            format!(
                "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{start}\n{}{}",
                segments(start, end),
                if finalized { "#EXT-X-ENDLIST\n" } else { "" }
            )
        };
        let initial = window(0, 4, false);
        let first = window(2, 6, false);
        let second = window(5, 9, true);
        let mut archive = initial.clone().into_bytes();
        let mut count = 4;
        let mut media_end = archive.len();

        append_hls_sequence_zero_archive_suffix(
            &mut archive,
            &mut count,
            &mut media_end,
            initial.as_bytes(),
            first.as_bytes(),
        )
        .unwrap();
        append_hls_sequence_zero_archive_suffix(
            &mut archive,
            &mut count,
            &mut media_end,
            first.as_bytes(),
            second.as_bytes(),
        )
        .unwrap();
        assert_eq!(count, 9);
        assert_eq!(
            hls_media_references(&archive),
            (0..9)
                .map(|index| format!("{index:064x}"))
                .collect::<Vec<_>>()
        );
        assert!(hls_is_finalized(&archive));

        let before = (archive.clone(), count, media_end);
        let conflicting = window(8, 11, false).replacen("#EXTINF:2.0,", "#EXTINF:3.0,", 1);
        assert!(
            append_hls_sequence_zero_archive_suffix(
                &mut archive,
                &mut count,
                &mut media_end,
                second.as_bytes(),
                conflicting.as_bytes(),
            )
            .is_none()
        );
        assert_eq!((archive, count, media_end), before);
    }

    #[test]
    fn sequence_zero_event_growth_appends_only_authenticated_rolling_suffixes() {
        let segment = |sequence: u64| {
            format!("#EXTINF:2.000000,\nhttps://swarm.beebridge.buzz/read/bytes/{sequence:064x}\n")
        };
        let current_segments = (0..5).map(segment).collect::<String>();
        let current = format!(
            "#EXTM3U\r\n#EXT-X-VERSION:3\r\n#EXT-X-TARGETDURATION:2\r\n\
             #EXT-X-PLAYLIST-TYPE:VOD\r\n#EXT-X-MEDIA-SEQUENCE:0\r\n{}",
            current_segments.replace('\n', "\r\n")
        );
        let rolling_segments = (2..7).map(segment).collect::<String>();
        let rolling = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n\
             #EXT-X-MEDIA-SEQUENCE:2\n{rolling_segments}"
        );

        let merged = extend_hls_sequence_zero_archive(current.as_bytes(), rolling.as_bytes())
            .expect("simple overlapping rolling window should extend the sequence-zero archive");
        assert_eq!(hls_media_sequence(&merged), Some(0));
        assert_eq!(
            hls_media_references(&merged),
            (0..7)
                .map(|sequence| format!("{sequence:064x}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            std::str::from_utf8(&merged)
                .unwrap()
                .matches("#EXTM3U")
                .count(),
            1
        );

        let rewritten = rewrite_hls_manifest_for_live_reload(
            &merged,
            "/weeb-3/hls/bytes",
            false,
            HlsStart::Beginning,
        )
        .expect("merged archive remains a valid provisional HLS representation");
        let rewritten = std::str::from_utf8(&rewritten).unwrap();
        assert!(rewritten.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(rewritten.contains("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES"));
        assert!(!rewritten.contains("#EXT-X-ENDLIST"));

        let final_segments = (0..7).map(segment).collect::<String>();
        let finalized = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n\
             {final_segments}#EXT-X-ENDLIST\n"
        );
        assert_eq!(
            extend_hls_sequence_zero_archive(&merged, finalized.as_bytes()),
            Some(finalized.into_bytes()),
            "the authenticated sequence-zero archive replaces the synthetic prefix exactly"
        );
    }

    #[test]
    fn sequence_zero_event_growth_preserves_early_endlist_and_rejects_malformed_terminal_tags() {
        let current = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXTINF:2.0,\n{REF}\n#EXTINF:2.0,\n{REF2}\n"
        );
        let early_endlist = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1\n#EXT-X-ENDLIST\n\
             #EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
        );
        let merged = extend_hls_sequence_zero_archive(current.as_bytes(), early_endlist.as_bytes())
            .expect("an early but valid ENDLIST should survive append-only synthesis");
        let merged = std::str::from_utf8(&merged).unwrap();
        assert_eq!(merged.matches("#EXT-X-ENDLIST").count(), 1);
        assert!(merged.trim_end().ends_with("#EXT-X-ENDLIST"));

        for malformed in [
            format!(
                "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1\n#EXT-X-ENDLIST-FOO\n\
                 #EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
            ),
            format!(
                "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1\n#EXT-X-INDEPENDENT-SEGMENTS:YES\n\
                 #EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
            ),
            format!(
                "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1\n#EXT-X-ENDLIST\n#EXT-X-ENDLIST\n\
                 #EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF3}\n"
            ),
        ] {
            assert!(
                extend_hls_sequence_zero_archive(current.as_bytes(), malformed.as_bytes())
                    .is_none(),
                "malformed or duplicate valueless tags must not enter a synthesized playlist"
            );
        }
    }

    #[test]
    fn sequence_zero_event_growth_holds_stateful_or_nonoverlapping_windows() {
        let current = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-KEY:METHOD=AES-128,URI=\"{REF3}\"\n\
             #EXTINF:2.0,\n{REF}\n#EXTINF:2.0,\n{REF2}\n"
        );
        let rolling = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1\n#EXT-X-KEY:METHOD=AES-128,URI=\"{REF3}\"\n\
             #EXTINF:2.0,\n{REF2}\n#EXTINF:2.0,\n{REF4}\n"
        );
        assert!(
            extend_hls_sequence_zero_archive(current.as_bytes(), rolling.as_bytes()).is_none(),
            "stateful KEY/MAP-style windows wait for the complete archive instead of being spliced"
        );

        let simple_current = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.0,\n{REF}\n");
        let jumped = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:9\n#EXTINF:2.0,\n{REF2}\n");
        assert!(
            extend_hls_sequence_zero_archive(simple_current.as_bytes(), jumped.as_bytes())
                .is_none(),
            "a nonoverlapping authenticated window cannot move the sequence-zero presentation"
        );
    }

    #[test]
    fn rewrites_multivariant_playlist_references() {
        let manifest = format!(
            "#EXTM3U\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"{REF2}\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO=\"audio\"\n\
             {REF}\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes").unwrap(),
        )
        .unwrap();

        assert!(rewritten.contains(&format!(
            "TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"/weeb-3/hls/bytes/{REF2}\""
        )));
        assert!(rewritten.contains(&format!("\n/weeb-3/hls/bytes/{REF}\n")));
    }

    #[test]
    fn extracts_only_ordered_swarm_media_fragments_for_lookahead() {
        let manifest = format!(
            "#EXTM3U\n\
             #EXT-X-KEY:METHOD=AES-128,URI=\"/weeb-3/hls/bytes/{REF2}\"\n\
             #EXTINF:2.0,\n\
             #EXT-X-BYTERANGE:1000@0\n\
             /weeb-3/hls/bytes/{REF}\n\
             #EXTINF:2.0,\n\
             https://gateway.invalid/read/bytes/{REF2}\n\
             #EXTINF:2.0,\n\
             /ordinary/media.ts\n"
        );

        assert_eq!(
            hls_media_references(manifest.as_bytes()),
            vec![REF.to_string(), REF2.to_string()]
        );
    }

    #[test]
    fn lookahead_excludes_master_variants_but_includes_low_latency_parts() {
        let manifest = format!(
            "#EXTM3U\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"/weeb-3/hls/bytes/{REF2}\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO=\"audio\"\n\
             /weeb-3/hls/bytes/{REF}\n\
             #EXT-X-PART:DURATION=0.5,URI=\"/weeb-3/hls/bytes/{REF2}\"\n\
             #EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"/weeb-3/hls/bytes/{REF}\"\n"
        );

        assert_eq!(
            hls_media_references(manifest.as_bytes()),
            vec![REF2.to_string(), REF.to_string()]
        );
    }

    #[test]
    fn rewrites_nested_bee_feed_playlists_to_the_local_reader() {
        let manifest = format!(
            "#EXTM3U\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"https://gateway.invalid/read/feeds/{OWNER}/{TOPIC}?index=7\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO=\"audio\"\n\
             /feeds/{OWNER}/{TOPIC}\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest(manifest.as_bytes(), "/mounted/weeb/testnet/hls/bytes").unwrap(),
        )
        .unwrap();

        assert!(rewritten.contains(&format!(
            "URI=\"/mounted/weeb/testnet/feeds/{OWNER}/{TOPIC}?index=7\""
        )));
        assert!(rewritten.contains(&format!("\n/mounted/weeb/testnet/feeds/{OWNER}/{TOPIC}\n")));
        assert!(!rewritten.contains("gateway.invalid"));
    }

    #[test]
    fn leaves_malformed_or_unsupported_feed_queries_external() {
        let manifest = format!(
            "#EXTM3U\n\
             https://gateway.invalid/feeds/{OWNER}/{TOPIC}?at=123\n\
             https://gateway.invalid/feeds/{OWNER}/{TOPIC}/child\n\
             https://gateway.invalid/feed/{OWNER}/{TOPIC}\n"
        );
        let rewritten = rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes").unwrap();
        assert_eq!(String::from_utf8(rewritten).unwrap(), manifest);
    }

    #[test]
    fn rewrites_media_maps_keys_and_low_latency_parts() {
        let manifest = format!(
            "#EXTM3U\n\
             #EXT-X-TARGETDURATION:2\n\
             #EXT-X-MEDIA-SEQUENCE:0\n\
             #EXT-X-MAP:URI=\"https://gateway.invalid/bytes/{REF2}\",BYTERANGE=\"1024@0\"\n\
             #EXT-X-KEY:METHOD=AES-128,URI=\"/read/bytes/{REF}\"\n\
             #EXT-X-PART:DURATION=0.5,URI=\"{REF2}\",BYTERANGE=\"512@1024\"\n\
             #EXTINF:2,\n\
             {REF}\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes").unwrap(),
        )
        .unwrap();

        assert!(rewritten.contains(&format!(
            "URI=\"/weeb-3/hls/bytes/{REF2}\",BYTERANGE=\"1024@0\""
        )));
        assert!(rewritten.contains(&format!("METHOD=AES-128,URI=\"/weeb-3/hls/bytes/{REF}\"")));
        assert!(rewritten.contains(&format!(
            "DURATION=0.5,URI=\"/weeb-3/hls/bytes/{REF2}\",BYTERANGE=\"512@1024\""
        )));
    }

    #[test]
    fn downgrades_unsupported_low_latency_server_capabilities() {
        let manifest = format!(
            "#EXTM3U\n\
             #EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,HOLD-BACK=6,CAN-SKIP-UNTIL=24.0,CAN-SKIP-DATERANGES=YES,PART-HOLD-BACK=1.5\n\
             #EXT-X-PART-INF:PART-TARGET=0.5\n\
             #EXT-X-PART:DURATION=0.5,URI=\"{REF}\"\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes").unwrap(),
        )
        .unwrap();

        assert!(rewritten.contains("#EXT-X-SERVER-CONTROL:HOLD-BACK=6,PART-HOLD-BACK=1.5"));
        assert!(!rewritten.contains("CAN-BLOCK-RELOAD"));
        assert!(!rewritten.contains("CAN-SKIP-UNTIL"));
        assert!(!rewritten.contains("CAN-SKIP-DATERANGES"));
        assert!(rewritten.contains(&format!("URI=\"/weeb-3/hls/bytes/{REF}\"")));
    }

    #[test]
    fn omits_server_control_when_only_unsupported_claims_remain() {
        let manifest = format!(
            "#EXTM3U\n\
             #EXT-X-SERVER-CONTROL:can-block-reload=YES,CAN-SKIP-UNTIL=12\n\
             #EXTINF:2,\n\
             {REF}\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes").unwrap(),
        )
        .unwrap();

        assert!(!rewritten.contains("SERVER-CONTROL"));
        assert_eq!(rewritten.matches("#EXTM3U").count(), 1);
        assert!(rewritten.contains("#EXTINF:2,"));
        assert!(rewritten.ends_with(&format!("/weeb-3/hls/bytes/{REF}\n")));
    }

    #[test]
    fn preserves_supported_and_lookalike_server_control_attributes() {
        let manifest = "#EXTM3U\n#EXT-X-SERVER-CONTROL:HOLD-BACK=6,X-CAN-BLOCK-RELOAD=YES,CAN-BLOCK-RELOAD-X=YES\n";
        let rewritten = String::from_utf8(
            rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes").unwrap(),
        )
        .unwrap();

        assert_eq!(rewritten, manifest);
    }

    #[test]
    fn leaves_non_swarm_external_uris_untouched() {
        let manifest = format!(
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://keys.invalid/key.bin\"\n#EXT-X-SESSION-DATA:DATA-ID=\"XURI\",XURI=\"https://cdn.invalid/bytes/{REF}\"\n#EXTINF:2,\nhttps://cdn.invalid/segment.ts\nhttps://cdn.invalid/media/{REF}\n"
        );
        let rewritten = rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes").unwrap();
        assert_eq!(String::from_utf8(rewritten).unwrap(), manifest);
    }

    #[test]
    fn rewrites_only_exact_uri_attributes_and_preserves_formatting() {
        let manifest = format!(
            "#EXTM3U\n#EXT-X-MAP:XURI=\"https://gateway.invalid/bytes/{REF}\", URI = \"https://gateway.invalid/read/bytes/{REF2}?download=1\",NOTE=\"URI=https://gateway.invalid/bytes/{REF}\"\n#EXT-X-KEY:URI=\" https://gateway.invalid/bytes/{REF} \"\n  https://gateway.invalid/bytes/{REF}  \n"
        );

        let rewritten = String::from_utf8(
            rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes/").unwrap(),
        )
        .unwrap();
        assert!(rewritten.contains(&format!("XURI=\"https://gateway.invalid/bytes/{REF}\"")));
        assert!(rewritten.contains(&format!(" URI = \"/weeb-3/hls/bytes/{REF2}\"")));
        assert!(rewritten.contains(&format!("NOTE=\"URI=https://gateway.invalid/bytes/{REF}\"")));
        assert!(rewritten.contains(&format!("URI=\" https://gateway.invalid/bytes/{REF} \"")));
        assert!(rewritten.contains(&format!("  /weeb-3/hls/bytes/{REF}  \n")));
    }

    #[test]
    fn rejects_lookalike_and_malformed_swarm_routes() {
        let encrypted = format!("{REF}{REF2}");
        let manifest = format!(
            "#EXTM3U\nhttps://cdn.invalid/media/{REF}\nhttps://cdn.invalid/byte/{REF}\nhttps://cdn.invalid/bzz/{REF}\nhttps://cdn.invalid/bytes/{REF}/child\nhttps://cdn.invalid/bytes/{REF}0\nhttps:\\gateway.invalid\\bytes\\{REF}\nhttps:/gateway.invalid/bytes/{REF}\nftp://gateway.invalid/bytes/{REF}\n#EXT-X-KEY:XURI=\"https://cdn.invalid/bytes/{REF}\",URI=https://cdn.invalid/bytes/{REF}\n#EXTINF:2,\nhttps://gateway.invalid/read/bytes/{encrypted}#fragment\n"
        );

        let rewritten = String::from_utf8(
            rewrite_hls_manifest(manifest.as_bytes(), "/weeb-3/hls/bytes").unwrap(),
        )
        .unwrap();
        assert!(rewritten.contains(&format!("https://cdn.invalid/media/{REF}")));
        assert!(rewritten.contains(&format!("https://cdn.invalid/byte/{REF}")));
        assert!(rewritten.contains(&format!("https://cdn.invalid/bzz/{REF}")));
        assert!(rewritten.contains(&format!("https://cdn.invalid/bytes/{REF}/child")));
        assert!(rewritten.contains(&format!("https://cdn.invalid/bytes/{REF}0")));
        assert!(rewritten.contains(&format!("https:\\gateway.invalid\\bytes\\{REF}")));
        assert!(rewritten.contains(&format!("https:/gateway.invalid/bytes/{REF}")));
        assert!(rewritten.contains(&format!("ftp://gateway.invalid/bytes/{REF}")));
        assert!(rewritten.contains(&format!("URI=https://cdn.invalid/bytes/{REF}")));
        assert!(rewritten.ends_with(&format!("/weeb-3/hls/bytes/{encrypted}\n")));

        for malformed in [
            format!("#EXTM3U\nhttps://bytes/{REF}\n"),
            format!("#EXTM3U\n//bytes/{REF}\n"),
            format!("#EXTM3U\nhttp:///bytes/{REF}\n"),
        ] {
            assert_eq!(
                rewrite_hls_manifest(malformed.as_bytes(), "/weeb-3/hls/bytes"),
                Some(malformed.into_bytes())
            );
        }
    }

    #[test]
    fn rejects_non_hls_and_invalid_utf8() {
        assert!(!is_hls_manifest(br#"{"entries":[]}"#));
        assert!(!is_hls_manifest(b"#EXTM3U-NOT-A-PLAYLIST\n"));
        assert!(is_hls_manifest(b"\xef\xbb\xbf  #EXTM3U\r\n"));
        assert!(rewrite_hls_manifest(br#"{"entries":[]}"#, "/weeb-3/hls/bytes").is_none());
        assert!(rewrite_hls_manifest(&[0xff, 0xfe], "/weeb-3/hls/bytes").is_none());
    }

    #[test]
    fn prefix_probe_is_conservative_at_truncated_hls_boundaries() {
        assert_eq!(
            probe_hls_manifest(&[b' '; 512], (MAX_STREAM_FEED_PAYLOAD_BYTES as u64) + 1),
            HlsManifestProbe::NotManifest
        );
        assert_eq!(
            probe_hls_manifest(b"#EXTM3U\n#EXT-X-", 100),
            HlsManifestProbe::Manifest
        );
        assert_eq!(
            probe_hls_manifest(b" \xef\xbb\xbf", 100),
            HlsManifestProbe::NotManifest
        );
        assert_eq!(
            probe_hls_manifest(b"\xef\xbb", 100),
            HlsManifestProbe::NeedMore
        );
        assert_eq!(
            probe_hls_manifest(b"   #EXTM", 100),
            HlsManifestProbe::NeedMore
        );
        assert_eq!(
            probe_hls_manifest(b"   #EXTM3U", 100),
            HlsManifestProbe::NeedMore
        );
        assert_eq!(
            probe_hls_manifest(b"#EXTM3U-NOT\n", 100),
            HlsManifestProbe::NotManifest
        );
        assert_eq!(
            probe_hls_manifest(b"\x47binary", 100),
            HlsManifestProbe::NotManifest
        );
        assert_eq!(
            probe_hls_manifest(b"#EXTM3U", 7),
            HlsManifestProbe::Manifest
        );
    }

    #[test]
    fn stream_feed_payload_length_is_bounded() {
        assert!(stream_feed_payload_len_is_supported(
            MAX_STREAM_FEED_PAYLOAD_BYTES
        ));
        assert!(!stream_feed_payload_len_is_supported(
            MAX_STREAM_FEED_PAYLOAD_BYTES + 1
        ));
    }

    #[test]
    fn sniffs_hls_asset_types_without_mislabeling_keys() {
        let mut ts = vec![0; 377];
        ts[0] = 0x47;
        ts[188] = 0x47;
        assert_eq!(hls_payload_mime(&ts), "video/mp2t");
        assert_eq!(hls_payload_mime(b"\0\0\0\x18ftypisom"), "video/mp4");
        assert_eq!(hls_payload_mime(b"WEBVTT\n\n"), "text/vtt; charset=utf-8");
        assert_eq!(hls_payload_mime(&[0xff, 0xf1, 0x50]), "audio/aac");
        assert_eq!(
            hls_payload_mime(b"#EXTM3U\n#EXT-X-VERSION:3\n"),
            "application/vnd.apple.mpegurl"
        );
        assert_eq!(hls_payload_mime(&[0x47; 16]), "application/octet-stream");
        assert_eq!(hls_payload_mime(&[0x13; 16]), "application/octet-stream");
    }
}

mod http_range {
    use crate::stream_conventions;

    use stream_conventions::{if_none_match_matches, if_range_allows_range, parse_single_range};

    #[test]
    fn parses_closed_open_and_suffix_ranges() {
        assert_eq!(parse_single_range(None, 100), None);
        assert_eq!(parse_single_range(Some("bytes=2-9"), 100), Some(Ok((2, 9))));
        assert_eq!(
            parse_single_range(Some("bytes=90-"), 100),
            Some(Ok((90, 99)))
        );
        assert_eq!(
            parse_single_range(Some("bytes=-10"), 100),
            Some(Ok((90, 99)))
        );
        assert_eq!(
            parse_single_range(Some("BYTES=-200"), 100),
            Some(Ok((0, 99)))
        );
        assert_eq!(
            parse_single_range(Some(" bytes=2-999 "), 100),
            Some(Ok((2, 99)))
        );
    }

    #[test]
    fn rejects_every_malformed_or_unsatisfiable_supplied_range() {
        for range in [
            "",
            "items=0-1",
            "bytes",
            "bytes=",
            "bytes=-",
            "bytes=-0",
            "bytes=0-1,4-5",
            "bytes=100-",
            "bytes=8-7",
            "bytes=+1-2",
            "bytes= 1-2",
            "bytes=1 -2",
            "bytes=1- 2",
            "bytes=1-2-3",
            "bytes=a-b",
        ] {
            assert_eq!(
                parse_single_range(Some(range), 100),
                Some(Err(())),
                "{range}"
            );
        }
        assert_eq!(parse_single_range(Some("bytes=0-0"), 0), Some(Err(())));
    }

    #[test]
    fn conditional_entity_tags_follow_get_and_range_comparison_rules() {
        let current = "\"weeb3-hls-v1-deadbeef\"";

        assert!(if_none_match_matches(Some(current), current));
        assert!(if_none_match_matches(
            Some("W/\"weeb3-hls-v1-deadbeef\""),
            current
        ));
        assert!(if_none_match_matches(Some("\"other\", *"), current));
        assert!(if_none_match_matches(Some("*"), ""));
        assert!(!if_none_match_matches(Some("\"other\""), current));
        assert!(!if_none_match_matches(None, current));

        assert!(if_range_allows_range(Some(current), current));
        assert!(if_range_allows_range(None, current));
        assert!(!if_range_allows_range(
            Some("W/\"weeb3-hls-v1-deadbeef\""),
            current
        ));
        assert!(!if_range_allows_range(Some("\"other\""), current));
        assert!(!if_range_allows_range(
            Some("Wed, 21 Oct 2015 07:28:00 GMT"),
            current
        ));
    }
}

mod prefetch {
    use crate::stream_conventions;

    use stream_conventions::{
        MEDIA_CACHE_FALLBACK_BYTES, MEDIA_CACHE_HARD_MAX_BYTES, MEDIA_CACHE_MIN_BYTES,
        MEDIA_PREFETCH_ACTIVE_HEADROOM_BYTES, MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES,
        MEDIA_PREFETCH_BATCH_YIELD_MS, MEDIA_PREFETCH_MAX_PARALLEL, MEDIA_PREFETCH_STAGE_BYTES,
        MEDIA_STARTUP_RESPONSE_BYTES, MEDIA_STORAGE_WINDOW_BYTES, MIB_BYTES,
        media_cache_budget_bytes, media_prefetch_ahead_limit_bytes, media_prefetch_stage_targets,
        plan_media_prefetch_batch,
    };

    #[test]
    fn shared_constants_preserve_the_regular_media_policy() {
        assert_eq!(MEDIA_STORAGE_WINDOW_BYTES, MIB_BYTES / 2);
        assert_eq!(MEDIA_STARTUP_RESPONSE_BYTES, 8 * MIB_BYTES);
        assert_eq!(
            MEDIA_PREFETCH_ACTIVE_HEADROOM_BYTES,
            8 * MIB_BYTES + 3 * (MIB_BYTES / 2)
        );
        assert_eq!(MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES, 96 * MIB_BYTES);
        assert_eq!(MEDIA_CACHE_FALLBACK_BYTES, 96 * MIB_BYTES);
        assert_eq!(MEDIA_PREFETCH_MAX_PARALLEL, 4);
        assert_eq!(MEDIA_PREFETCH_BATCH_YIELD_MS, 25);
        assert_eq!(
            MEDIA_PREFETCH_STAGE_BYTES,
            [
                4 * MIB_BYTES,
                4 * MIB_BYTES,
                4 * MIB_BYTES,
                4 * MIB_BYTES,
                8 * MIB_BYTES,
                8 * MIB_BYTES,
                8 * MIB_BYTES,
                8 * MIB_BYTES,
                16 * MIB_BYTES,
                32 * MIB_BYTES,
            ]
        );
    }

    #[test]
    fn memory_signals_produce_a_bounded_shared_cache_budget() {
        assert_eq!(
            media_cache_budget_bytes(Some((160 * MIB_BYTES) as f64), Some(8.0)),
            MEDIA_CACHE_MIN_BYTES
        );
        assert_eq!(
            media_cache_budget_bytes(Some((320 * MIB_BYTES) as f64), Some(8.0)),
            64 * MIB_BYTES
        );
        assert_eq!(
            media_cache_budget_bytes(Some((1024 * MIB_BYTES) as f64), Some(0.25)),
            MEDIA_CACHE_HARD_MAX_BYTES
        );

        assert_eq!(
            media_cache_budget_bytes(None, Some(0.3125)),
            MEDIA_CACHE_MIN_BYTES
        );
        assert_eq!(media_cache_budget_bytes(None, Some(0.625)), 64 * MIB_BYTES);
        assert_eq!(
            media_cache_budget_bytes(None, Some(2.0)),
            MEDIA_CACHE_HARD_MAX_BYTES
        );
    }

    #[test]
    fn heap_signal_has_priority_and_invalid_signals_fall_through() {
        assert_eq!(
            media_cache_budget_bytes(Some((160 * MIB_BYTES) as f64), Some(8.0)),
            MEDIA_CACHE_MIN_BYTES,
            "a valid heap limit must take priority over device memory"
        );
        assert_eq!(
            media_cache_budget_bytes(Some(f64::NAN), Some(0.625)),
            64 * MIB_BYTES
        );
        assert_eq!(
            media_cache_budget_bytes(Some(0.0), Some(0.625)),
            64 * MIB_BYTES
        );
        assert_eq!(
            media_cache_budget_bytes(Some(f64::INFINITY), Some(f64::NEG_INFINITY)),
            MEDIA_CACHE_FALLBACK_BYTES
        );
        assert_eq!(
            media_cache_budget_bytes(None, None),
            MEDIA_CACHE_FALLBACK_BYTES
        );
    }

    #[test]
    fn lookahead_reserves_foreground_and_window_edge_headroom() {
        assert_eq!(
            media_prefetch_ahead_limit_bytes(32 * MIB_BYTES),
            22 * MIB_BYTES + MIB_BYTES / 2
        );
        assert_eq!(
            media_prefetch_ahead_limit_bytes(64 * MIB_BYTES),
            54 * MIB_BYTES + MIB_BYTES / 2
        );
        assert_eq!(
            media_prefetch_ahead_limit_bytes(MEDIA_CACHE_FALLBACK_BYTES),
            86 * MIB_BYTES + MIB_BYTES / 2
        );
        assert_eq!(
            media_prefetch_ahead_limit_bytes(128 * MIB_BYTES),
            MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES
        );
        assert_eq!(
            media_prefetch_ahead_limit_bytes(MEDIA_PREFETCH_ACTIVE_HEADROOM_BYTES - 1),
            0
        );
    }

    #[test]
    fn staged_targets_are_cumulative_and_clip_once() {
        assert_eq!(
            media_prefetch_stage_targets(64 * MIB_BYTES),
            [4, 8, 12, 16, 24, 32, 40, 48, 64]
                .map(|mib| mib * MIB_BYTES)
                .to_vec()
        );
        assert_eq!(
            media_prefetch_stage_targets(96 * MIB_BYTES),
            [4, 8, 12, 16, 24, 32, 40, 48, 64, 96]
                .map(|mib| mib * MIB_BYTES)
                .to_vec()
        );
        assert_eq!(
            media_prefetch_stage_targets(22 * MIB_BYTES + MIB_BYTES / 2),
            [
                4 * MIB_BYTES,
                8 * MIB_BYTES,
                12 * MIB_BYTES,
                16 * MIB_BYTES,
                22 * MIB_BYTES + MIB_BYTES / 2,
            ]
        );
        assert!(media_prefetch_stage_targets(0).is_empty());
        assert_eq!(
            media_prefetch_stage_targets(u64::MAX).last().copied(),
            Some(MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES)
        );
    }

    #[test]
    fn one_batch_handles_fixed_windows_and_atomic_hls_fragments() {
        let windows = [MEDIA_STORAGE_WINDOW_BYTES; 8];
        let first_windows = plan_media_prefetch_batch(0, 4 * MIB_BYTES, 64 * MIB_BYTES, &windows);
        assert_eq!(first_windows.unit_count, MEDIA_PREFETCH_MAX_PARALLEL);
        assert_eq!(first_windows.additional_bytes, 2 * MIB_BYTES);
        assert_eq!(first_windows.planned_end_bytes, 2 * MIB_BYTES);

        let second_windows = plan_media_prefetch_batch(
            first_windows.planned_end_bytes,
            4 * MIB_BYTES,
            64 * MIB_BYTES,
            &windows,
        );
        assert_eq!(second_windows.unit_count, MEDIA_PREFETCH_MAX_PARALLEL);
        assert_eq!(second_windows.planned_end_bytes, 4 * MIB_BYTES);

        let fragment = 2_419_372;
        let fragments = [fragment; 8];
        let hls = plan_media_prefetch_batch(0, 4 * MIB_BYTES, 64 * MIB_BYTES, &fragments);
        assert_eq!(hls.unit_count, 2);
        assert_eq!(hls.additional_bytes, fragment * 2);
        assert!(
            hls.planned_end_bytes >= 4 * MIB_BYTES,
            "the final indivisible fragment may cross a stage target"
        );
    }

    #[test]
    fn atomic_batches_never_cross_the_hard_byte_cap() {
        let mib = MIB_BYTES;
        let plan = plan_media_prefetch_batch(3 * mib, 8 * mib, 6 * mib, &[2 * mib, mib]);
        assert_eq!(plan.unit_count, 2);
        assert_eq!(plan.planned_end_bytes, 6 * mib);

        let blocked = plan_media_prefetch_batch(5 * mib, 8 * mib, 6 * mib, &[2 * mib, mib]);
        assert_eq!(blocked.unit_count, 0);
        assert_eq!(blocked.additional_bytes, 0);
        assert_eq!(blocked.planned_end_bytes, 5 * mib);

        let oversized = plan_media_prefetch_batch(0, 4 * mib, 64 * mib, &[65 * mib]);
        assert_eq!(oversized.unit_count, 0);
        assert_eq!(oversized.planned_end_bytes, 0);
    }

    #[test]
    fn malformed_or_overflowing_units_stop_ordered_planning() {
        let zero = plan_media_prefetch_batch(0, 4 * MIB_BYTES, 64 * MIB_BYTES, &[0, MIB_BYTES]);
        assert_eq!(zero.unit_count, 0);
        assert!(zero.unit_count <= MEDIA_PREFETCH_MAX_PARALLEL);

        let overflow = plan_media_prefetch_batch(u64::MAX - 1, u64::MAX, u64::MAX, &[2, MIB_BYTES]);
        assert_eq!(overflow.unit_count, 0);
        assert_eq!(overflow.planned_end_bytes, u64::MAX - 1);
    }
}

mod feed_followup {
    use crate::stream_hls;

    use stream_hls::{
        FEED_FOLLOWUP_BATCH_LIMIT, HLS_TERMINAL_CONFIRMATION_MIN_PRICED_PEERS,
        cached_feed_should_refresh_head, hls_prepared_live_history_is_terminal,
        hls_snapshot_is_terminal, hls_terminal_peer_view_is_mature,
    };

    #[test]
    fn regular_polling_stays_on_the_exact_next_index() {
        assert!(!cached_feed_should_refresh_head(10_000.0, 24_999.0));
    }

    #[test]
    fn dormant_readers_jump_to_the_latest_head() {
        assert!(cached_feed_should_refresh_head(10_000.0, 25_000.0));
    }

    #[test]
    fn canonical_followup_keeps_its_bounded_exact_runway() {
        assert_eq!(FEED_FOLLOWUP_BATCH_LIMIT, 4);
    }

    #[test]
    fn invalid_clocks_do_not_force_a_head_probe() {
        assert!(!cached_feed_should_refresh_head(f64::NAN, 25_000.0));
        assert!(!cached_feed_should_refresh_head(10_000.0, f64::INFINITY));
        assert!(!cached_feed_should_refresh_head(25_000.0, 10_000.0));
    }

    #[test]
    fn endlist_alone_does_not_finalize_an_unindexed_feed_snapshot() {
        assert!(!hls_snapshot_is_terminal(true, false, false));
        assert!(!hls_snapshot_is_terminal(false, true, true));
        assert!(hls_snapshot_is_terminal(true, true, false));
        assert!(hls_snapshot_is_terminal(true, false, true));
    }

    #[test]
    fn prepared_live_history_only_finalizes_the_same_confirmed_head() {
        assert!(!hls_prepared_live_history_is_terminal(true, false, 41, 41,));
        assert!(!hls_prepared_live_history_is_terminal(true, true, 41, 42,));
        assert!(!hls_prepared_live_history_is_terminal(false, true, 41, 41,));
        assert!(hls_prepared_live_history_is_terminal(true, true, 41, 41,));
    }

    #[test]
    fn terminal_confirmation_matures_before_the_population_cap() {
        assert_eq!(HLS_TERMINAL_CONFIRMATION_MIN_PRICED_PEERS, 8);
        assert!(!hls_terminal_peer_view_is_mature(1));
        assert!(!hls_terminal_peer_view_is_mature(7));
        assert!(hls_terminal_peer_view_is_mature(8));
        assert!(hls_terminal_peer_view_is_mature(200));
    }

    #[test]
    fn tentative_terminal_followup_only_resumes_for_newer_authenticated_evidence() {
        use std::collections::VecDeque;

        use stream_hls::{
            HlsSequenceZeroRetry, hls_sequence_zero_has_newer_authenticated_evidence,
        };

        let mut retries = VecDeque::new();
        assert!(!hls_sequence_zero_has_newer_authenticated_evidence(
            41, 41, None, &retries,
        ));

        retries.push_back(HlsSequenceZeroRetry {
            index: 42,
            authenticated: false,
        });
        assert!(!hls_sequence_zero_has_newer_authenticated_evidence(
            41, 41, None, &retries,
        ));
        retries.push_back(HlsSequenceZeroRetry {
            index: 43,
            authenticated: true,
        });
        assert!(hls_sequence_zero_has_newer_authenticated_evidence(
            41, 41, None, &retries,
        ));
        assert!(hls_sequence_zero_has_newer_authenticated_evidence(
            41,
            41,
            Some(44),
            &VecDeque::new(),
        ));
        assert!(hls_sequence_zero_has_newer_authenticated_evidence(
            41,
            45,
            None,
            &VecDeque::new(),
        ));
    }
}

mod share_links {
    use crate::stream_conventions::{HlsStart, StreamShareRoute, parse_stream_share_link};

    const OWNER: &str = "352eabdea9cb05e984a8828d2a6df3d3b5023260";
    const MIXED_CASE_OWNER: &str = "6F2728386F8a47ef5EBe323721188e630Ff0FdE9";
    const CANONICAL_OWNER: &str = "6f2728386f8a47ef5ebe323721188e630ff0fde9";
    const TOPIC: &str = "0d216633-3475-4c26-8dd0-9935ef854bbc";

    #[test]
    fn parses_the_exact_mainnet_stream_routes() {
        let parsed =
            parse_stream_share_link(&format!("/weeb-3/stream/{MIXED_CASE_OWNER}/{TOPIC}")).unwrap();

        assert_eq!(parsed.owner, CANONICAL_OWNER);
        assert_eq!(parsed.topic, TOPIC);
        assert_eq!(parsed.start, HlsStart::Beginning);
        assert_eq!(
            parse_stream_share_link(&format!("/weeb-3/live/stream/{MIXED_CASE_OWNER}/{TOPIC}"))
                .unwrap()
                .start,
            HlsStart::Live
        );
        assert_eq!(
            parse_stream_share_link(&format!("stream/{OWNER}/topic%2Fpart%20%E6%97%A5")),
            StreamShareRoute::new(OWNER, "topic/part 日")
        );
    }

    #[test]
    fn construction_rejects_invalid_owner_and_topic_values() {
        assert_eq!(
            StreamShareRoute::new(MIXED_CASE_OWNER, TOPIC)
                .unwrap()
                .owner,
            CANONICAL_OWNER
        );

        for owner in [
            "",
            "not-an-owner",
            "0x352eabdea9cb05e984a8828d2a6df3d3b5023260",
        ] {
            assert!(StreamShareRoute::new(owner, TOPIC).is_err());
        }
        for topic in ["", ".", "..", "line\nbreak"] {
            assert!(
                StreamShareRoute::new(OWNER, topic).is_err(),
                "accepted topic {topic:?}"
            );
        }
        assert!(StreamShareRoute::new(OWNER, "é".repeat(129)).is_err());
    }

    #[test]
    fn rejects_aliases_indices_urls_and_malformed_paths() {
        let invalid = [
            format!("/weeb-3/mainnet/stream/{OWNER}/{TOPIC}"),
            format!("/weeb-3/testnet/stream/{OWNER}/{TOPIC}"),
            format!("/weeb-3/mainnet/live/stream/{OWNER}/{TOPIC}"),
            format!("/weeb-3/testnet/live/stream/{OWNER}/{TOPIC}"),
            format!("/weeb-3/stream/{OWNER}/{TOPIC}/69"),
            format!("/weeb-3/live/{OWNER}/{TOPIC}"),
            format!("/weeb-3/live/stream/{OWNER}/{TOPIC}/"),
            format!("/weeb-3/watch/video/{OWNER}/{TOPIC}"),
            format!("/weeb-3/hls/{OWNER}/{TOPIC}"),
            format!("/weeb-3/stream/not-an-owner/{TOPIC}"),
            format!("/weeb-3/stream/{OWNER}"),
            format!("/weeb-3/stream/{OWNER}/{TOPIC}/"),
            format!("/weeb-3/stream/{OWNER}/bad%escape"),
            format!("/weeb-3/stream/{OWNER}/%FF"),
            format!("/weeb-3/stream/{OWNER}/{TOPIC}?index=7"),
            format!("/weeb-3/live/stream/{OWNER}/{TOPIC}?index=7"),
            format!("/weeb-3/stream/{OWNER}/{TOPIC}#fragment"),
            format!("//host/weeb-3/stream/{OWNER}/{TOPIC}"),
            format!("https://host/weeb-3/stream/{OWNER}/{TOPIC}"),
            format!("/other/stream/{OWNER}/{TOPIC}"),
        ];

        for input in invalid {
            assert!(
                parse_stream_share_link(&input).is_err(),
                "accepted {input:?}"
            );
        }
    }
}

mod routes {
    use crate::stream_conventions::{
        STREAMING_ROUTE_BASE, STREAMING_SERVICE_WORKER_SCOPE, STREAMING_SERVICE_WORKER_URL,
        streaming_route_path,
    };

    #[test]
    fn standalone_routes_are_fixed() {
        assert_eq!(STREAMING_ROUTE_BASE, "/weeb-3");
        assert_eq!(streaming_route_path("hls/bytes"), "/weeb-3/hls/bytes");
        assert_eq!(streaming_route_path("/feeds"), "/weeb-3/feeds");
        assert_eq!(STREAMING_SERVICE_WORKER_URL, "/weeb-3/service.js");
        assert_eq!(STREAMING_SERVICE_WORKER_SCOPE, "/weeb-3/");
    }
}

mod network_routes {
    use crate::{
        nav::{
            ResourceRoute, parse_networked_resource_route, parse_resource_route,
            route_network_mode_from_path,
        },
        network_profile::NetworkMode,
        stream_conventions::HlsStart,
    };

    const REFERENCE: &str = "919b5395bf7a59cbb3b365769de09a2b15ac5d897823dda9270259a3c038d574";
    const OWNER: &str = "352eabdea9cb05e984a8828d2a6df3d3b5023260";
    const TOPIC: &str = "cfbbc155d709547b198638d0fb11d733359561538d8bd606a9ab257354d13bcc";

    #[test]
    fn existing_transport_routes_keep_their_network_rules() {
        for tail in [
            format!("bzz/{REFERENCE}"),
            format!("bzz/{REFERENCE}/index.html"),
            format!("bytes/{REFERENCE}"),
            format!("chunks/{REFERENCE}"),
        ] {
            assert_eq!(
                route_network_mode_from_path(&format!("/weeb-3/{tail}")),
                Some(NetworkMode::Mainnet)
            );
            assert_eq!(
                route_network_mode_from_path(&format!("/weeb-3/mainnet/{tail}")),
                Some(NetworkMode::Mainnet)
            );
            assert_eq!(
                route_network_mode_from_path(&format!("/weeb-3/testnet/{tail}")),
                Some(NetworkMode::Testnet)
            );
        }

        assert_eq!(
            route_network_mode_from_path("/weeb-3/bzz"),
            Some(NetworkMode::Mainnet)
        );
        assert_eq!(
            route_network_mode_from_path("/weeb-3/testnet/bzz"),
            Some(NetworkMode::Testnet)
        );
    }

    #[test]
    fn existing_resource_routes_retain_network_identity() {
        let mainnet =
            parse_networked_resource_route(&format!("/weeb-3/bytes/{REFERENCE}")).unwrap();
        assert_eq!(mainnet.network, NetworkMode::Mainnet);
        assert_eq!(
            mainnet.resource,
            ResourceRoute::Bytes(REFERENCE.to_string())
        );

        let explicit_mainnet =
            parse_networked_resource_route(&format!("/weeb-3/mainnet/bytes/{REFERENCE}")).unwrap();
        assert_eq!(explicit_mainnet.network, NetworkMode::Mainnet);
        assert_eq!(explicit_mainnet.resource, mainnet.resource);

        let testnet =
            parse_networked_resource_route(&format!("/weeb-3/testnet/bytes/{REFERENCE}")).unwrap();
        assert_eq!(testnet.network, NetworkMode::Testnet);
        assert_eq!(testnet.resource, mainnet.resource);
        assert_eq!(
            parse_resource_route(&format!("/weeb-3/testnet/bytes/{REFERENCE}")),
            Some(ResourceRoute::Bytes(REFERENCE.to_string()))
        );
    }

    #[test]
    fn exact_stream_routes_are_mainnet_hls() {
        let route =
            parse_networked_resource_route(&format!("/weeb-3/stream/{OWNER}/{TOPIC}")).unwrap();
        assert_eq!(route.network, NetworkMode::Mainnet);
        assert_eq!(
            route.resource,
            ResourceRoute::Hls {
                owner: OWNER.to_string(),
                topic: TOPIC.to_string(),
                start: HlsStart::Beginning,
            }
        );
        let live = parse_networked_resource_route(&format!("/weeb-3/live/stream/{OWNER}/{TOPIC}"))
            .unwrap();
        assert_eq!(live.network, NetworkMode::Mainnet);
        assert_eq!(
            live.resource,
            ResourceRoute::Hls {
                owner: OWNER.to_string(),
                topic: TOPIC.to_string(),
                start: HlsStart::Live,
            }
        );

        for alias in [
            format!("/weeb-3/mainnet/stream/{OWNER}/{TOPIC}"),
            format!("/weeb-3/testnet/stream/{OWNER}/{TOPIC}"),
            format!("/weeb-3/mainnet/live/stream/{OWNER}/{TOPIC}"),
            format!("/weeb-3/stream/{OWNER}/{TOPIC}/69"),
        ] {
            assert!(parse_networked_resource_route(&alias).is_none());
        }
    }

    #[test]
    fn invalid_routes_do_not_acquire_a_network() {
        for path in [
            "/weeb-3/",
            "/weeb-3/testnetish/bzz",
            "/weeb-3/testnetish/bytes/919b",
            "/weeb-3/bytes/not-a-reference",
            "/weeb-3/hls/bytes/not-a-reference",
            "/weeb-3/mainnet/stream/352eabdea9cb05e984a8828d2a6df3d3b5023260/topic",
        ] {
            assert_eq!(
                route_network_mode_from_path(path),
                None,
                "invalid route unexpectedly selected a network: {path}"
            );
        }
    }
}
mod service_worker {
    const STATIC_WORKER: &str = include_str!("../static/service.js");
    const INTERFACE: &str = include_str!("../src/interface.rs");
    const INTERFACE_RUNTIME: &str = include_str!("../src/interface_runtime_conventions.rs");
    const BZZ_STREAM: &str = include_str!("../src/bzz_stream.rs");
    const HLS_PLAYER: &str = include_str!("../src/stream_hls.rs");
    const LIBRARY: &str = include_str!("../src/library.rs");
    const NAV: &str = include_str!("../src/nav.rs");
    const STATIC_404: &str = include_str!("../static/404.html");
    const STATIC_HLS_LOADER: &str = include_str!("../static/hls_loader.js");
    const STATIC_EXAMPLE: &str = include_str!("../static/example.html");
    const HLS_STREAM_EXAMPLE: &str = include_str!("../static/hls-stream-example.html");
    const MAIN_SERVER: &str = include_str!("../src/main.rs");
    const HAXE_BUILD: &str = include_str!("../Code_One.hx");
    const NPM_WORKFLOW: &str = include_str!("../.github/workflows/plain.yml");
    const NPM_README: &str = include_str!("../README.npm.md");
    const RUNTIME: &str = include_str!("../src/lib.rs");
    const UPLOAD: &str = include_str!("../src/upload.rs");

    fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        source
            .split_once(start)
            .and_then(|(_, tail)| tail.split_once(end))
            .map(|(body, _)| body)
            .unwrap_or_else(|| panic!("missing source section between {start:?} and {end:?}"))
    }

    #[test]
    fn hls_player_keeps_one_lazy_loader_and_ordered_session_lifecycle() {
        assert!(
            !std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs")
                .join("hls_loader.js")
                .exists()
        );
        assert!(STATIC_HLS_LOADER.contains("export async function loadHls()"));
        assert!(STATIC_HLS_LOADER.contains("import(\"hls.js\")"));
        assert!(HLS_PLAYER.contains("module = \"/static/hls_loader.js\""));
        let attach = source_between(
            HLS_PLAYER,
            "pub(crate) async fn attach_hls_feed_player(",
            "async fn open_hls_feed_view_generation(",
        );
        let loader_start = attach.find("JsFuture::from(load_hls())").unwrap();
        let worker_ready = attach.find("service_worker_controls_bzz_requests").unwrap();
        let lifecycle = attach.find("install_hls_prefetch_lifecycle(").unwrap();
        let snapshot = attach
            .find("let snapshot_load = Box::pin(async move")
            .unwrap();
        let overlap = attach
            .find("match select(worker_ready, snapshot_load).await")
            .unwrap();
        let play = attach.find("let mode = play_hls(").unwrap();
        assert!(loader_start < worker_ready);
        assert!(lifecycle < worker_ready && lifecycle < snapshot);
        assert!(worker_ready < overlap && snapshot < overlap && overlap < play);
        let play_call = &attach[play..attach[play..].find(".await").unwrap() + play];
        for argument in [
            "player",
            "&source",
            "hls_loader",
            "start",
            "beginning_prefix_stamp",
            "presentation_id",
        ] {
            assert!(play_call.contains(argument), "missing {argument}");
        }
        assert!(!attach.contains("autoplay_deadline"));
        assert!(!attach.contains("hls_feed_payload_at_index_bounded("));
        assert!(!attach.contains("join(current, bootstrap).await"));
        assert!(!attach.contains("codec-bootstrap"));
        let player = source_between(
            HLS_PLAYER,
            "mod player {",
            "#[cfg(target_arch = \"wasm32\")]\npub(crate) use player::{",
        );
        assert!(player.contains("struct Session {"));
        assert!(player.contains("enum EventKind {"));
        assert!(player.contains("enum Recovery {"));
        assert!(!player.contains("AbortController"));
        assert!(!player.contains("register_retrieve_cancel_token"));
        assert!(!player.contains("retrieve_data_payload_cancellable"));
        assert!(player.contains(
            "const NATIVE_DOM_EVENTS: [&str; 4] = [\"play\", \"pause\", \"error\", \"loadedmetadata\"]"
        ));

        let play_hls = source_between(player, "pub(crate) async fn play_hls(", "async fn launch(");
        let capture_autoplay = play_hls
            .find("let autoplay_allowed = media.autoplay()")
            .unwrap();
        let disable_native = play_hls.find("media.set_autoplay(false)").unwrap();
        let explicit_policy = play_hls.find("autoplay_allowed,").unwrap();
        assert!(capture_autoplay < disable_native && disable_native < explicit_policy);
        assert!(!play_hls.contains("autoplay_allowed: true"));
        assert!(play_hls.contains("HlsStart::Beginning => 0.0"));
        assert!(play_hls.contains("HlsStart::Live => -1.0"));
        assert!(!play_hls.contains("autoplay_gate_required"));
        assert!(!play_hls.contains("source.contains(\"start=live\")"));

        let launch = source_between(player, "async fn launch(", "fn session_from(");
        let authorization = launch
            .find("get_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)")
            .unwrap();
        let epoch = launch.find("let epoch = next_epoch()").unwrap();
        let clear_authorization = launch
            .find("remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)")
            .unwrap();
        assert!(authorization < epoch && epoch < clear_authorization);
        assert!(launch.contains("request.media.set_autoplay(false);"));
        assert!(launch.contains("let _ = request.media.pause();"));
        assert!(launch.contains("hls_config(request.start == HlsStart::Live)"));
        let native_preload = launch.find("media.set_preload(\"auto\")").unwrap();
        let native_load = launch.find("media.load();").unwrap();
        let native_play = launch
            .find("autoplay(epoch, Autoplay::Policy, None)")
            .unwrap();
        assert!(native_preload < native_load && native_load < native_play);
        assert!(!launch.contains("start_autoplay_buffer_gate(epoch)"));
        let hls_listener = launch
            .find("let hls_listener = install_hls_listener")
            .unwrap();
        let dom_listener = launch
            .find("let dom_listener = match install_dom_listener")
            .unwrap();
        let session = launch.rfind("install_session(").unwrap();
        let source = launch.find(".load_source(&source)").unwrap();
        let media = launch.find("hls.attach_media(&media)").unwrap();
        assert!(hls_listener < dom_listener && dom_listener < session);
        assert!(session < source && source < media);

        let epoch_change = source_between(
            player,
            "fn next_epoch()",
            "pub(crate) fn destroy_current_hls",
        );
        assert!(
            epoch_change.find("player.session.take()").unwrap()
                < epoch_change.find("session.dispose()").unwrap()
        );
        let install = source_between(player, "fn install_session(", "fn hls_is_supported(");
        assert!(install.contains("if let Some(session) = session"));
        assert!(install.contains("session.dispose();"));

        let dispose = source_between(
            player,
            "fn dispose_hls_listener(",
            "fn dispose_dom_listener(",
        );
        assert!(
            dispose.find(".off(").unwrap() < dispose.find("let destroyed = hls.destroy()").unwrap()
        );
        assert!(dispose.contains("if !detached && !destroyed"));
        let dom_dispose = source_between(player, "fn dispose_dom_listener(", "fn dispatch(");
        assert!(dom_dispose.contains("remove_event_listener_with_callback"));
        assert!(dom_dispose.contains("std::mem::forget(listener)"));

        let buffer_created =
            source_between(player, "fn buffer_created(", "fn main_swarm_fragment(");
        assert!(buffer_created.contains("session.codec_video_buffer_ready = true"));
        assert!(!buffer_created.contains("finish_hls_codec_bootstrap"));
        assert!(!buffer_created.contains("restore_hls_progressive_loader"));
        let bootstrap = source_between(
            player,
            "fn complete_hls_codec_bootstrap(",
            "fn fragment_buffered(",
        );
        let matched = bootstrap
            .find("codec_loading_fragments.contains(&fragment_identity)")
            .unwrap();
        let release = bootstrap.find("session.codec_pending = false").unwrap();
        let finish = bootstrap
            .find("finish_hls_codec_bootstrap(&source)")
            .unwrap();
        let restore = bootstrap
            .find("restore_hls_progressive_loader(&hls)")
            .unwrap();
        let handoff = bootstrap
            .find("handoff_hls_codec_bootstrap_prefetch_timeline()")
            .unwrap();
        let stop = bootstrap.find("let Err(error) = hls.stop_load()").unwrap();
        let seek = bootstrap
            .find("media.set_current_time(target.position())")
            .unwrap();
        let microtask = bootstrap.find("Wait::Microtask.wait().await").unwrap();
        let restart = bootstrap
            .find("start_at(epoch, &hls, target.position())")
            .unwrap();
        assert!(bootstrap.contains("finish_hls_codec_bootstrap(&source)"));
        assert!(!bootstrap.contains("strip_hls_codec_bootstrap"));
        assert!(!bootstrap.contains("load_source"));
        assert!(!bootstrap.contains("session.source ="));
        assert!(bootstrap.contains("start == HlsStart::Beginning"));
        assert!(bootstrap.contains("resume_position.filter(|target| target.is_finite())"));
        assert!(!bootstrap.contains("Reflect::get"));
        assert!(bootstrap.contains("session.playback_authorized || session.resume"));
        assert!(bootstrap.contains("if resume"));
        assert!(bootstrap.contains("start_autoplay_buffer_gate(epoch, Autoplay::Policy)"));
        assert!(!bootstrap.contains("autoplay(epoch,"));
        assert!(matched < stop && stop < release && release < finish && finish < restore);
        assert!(restore < handoff && handoff < seek && seek < microtask && microtask < restart);

        let manifest = source_between(player, "fn manifest_parsed(", "fn level_loaded(");
        assert!(manifest.contains("session.load = LoadPhase::Warmup"));
        assert!(
            manifest.find("start_at(epoch, &hls, position)").unwrap()
                < manifest.find("hls_autoplay_gate_ready(").unwrap()
        );
        assert!(manifest.contains("hls_contiguous_buffered_ahead("));
        assert!(manifest.contains("hls_autoplay_start_target("));
        assert!(manifest.contains("hls_live_sync_position"));
        assert!(!manifest.contains("hls_startup_runway_ready"));
        assert!(manifest.contains(".any(|(_, live, _, playable)| !live || playable.is_some())"));
        assert!(manifest.contains("let finalized_playable_interval ="));
        assert!(manifest.contains(".filter_map(|(_, _, _, playable)| *playable)"));
        assert!(!manifest.contains(".filter(|(_, live, _, _)| !*live)"));
        assert!(manifest.contains(".map_or_else(|| media.duration(), |(_, end)| end)"));
        assert!(!manifest.contains("autoplay_deadline"));
        assert!(manifest.contains("let position = session.restart_position()"));
        assert!(manifest.contains("intent.allowed(session)"));
        assert!(manifest.contains("session.playback_authorized || !intent.allowed(session)"));
        assert!(manifest.contains("if session.codec_pending"));
        assert!(manifest.contains("sleep(HLS_AUTOPLAY_GATE_POLL).await"));
        assert!(manifest.contains("session.autoplay_gate_pending = true"));
        assert!(manifest.contains("start_autoplay_buffer_gate(epoch, intent)"));
        assert!(manifest.contains("session.autoplay_gate_pending = false"));
        assert!(
            manifest.find("AutoplayGatePoll::Ready(target)").unwrap()
                < manifest
                    .rfind("autoplay(epoch, intent, Some(target))")
                    .unwrap()
        );
        assert!(!manifest.contains("Backend::Native) && media.duration().is_finite()"));
        assert!(!manifest.contains("sleep(Duration::from_millis(500))"));
        let attach = source_between(
            HLS_PLAYER,
            "pub(crate) async fn attach_hls_feed_player(",
            "async fn open_hls_feed_view_generation(",
        );
        assert!(!attach.contains("HLS_BEGINNING_AUTOPLAY_DEADLINE_MS"));
        let rebase = source_between(player, "fn level_loaded(", "fn dom_event(");
        let rebase_event = rebase
            .find("emit(&media, HLS_TIMELINE_REBASE_EVENT)")
            .unwrap();
        let stop = rebase[rebase_event..].find("hls.stop_load()").unwrap() + rebase_event;
        let defer = rebase[stop..].find("Wait::Microtask.wait().await").unwrap() + stop;
        let relaunch = rebase[defer..].find("launch(request).await").unwrap() + defer;
        assert!(rebase.contains("hls_timeline_rebase_position("));
        assert!(rebase.contains("autoplay_allowed: session.autoplay_allowed"));
        assert!(rebase.contains("let codec_bootstrap = session.codec_required"));
        assert!(rebase.contains("codec_bootstrap,"));
        assert!(rebase.contains("start: session.start"));
        assert!(rebase.contains("resume_position: rebase.or(session.resume_position)"));
        assert!(rebase.contains("resume: session.playback_authorized"));
        assert!(rebase_event < stop && stop < defer && defer < relaunch);

        let dom = source_between(player, "fn dom_event(", "fn handle_error(");
        assert!(dom.contains("hls_dom_play_is_explicit("));
        assert!(dom.contains("hls_dom_pause_is_explicit("));
        assert!(dom.contains("session.play_attempt_phase(),"));
        assert!(dom.contains("session.media.paused(),"));
        assert!(dom.contains("session.playback_authorized,"));
        assert!(dom.contains("session.playback_authorized,"));
        assert!(dom.contains("session.playback_authorized = true"));
        assert!(dom.contains("session.playback_authorized = false"));
        assert!(dom.contains("session.autoplay_allowed = false"));
        assert!(
            dom.find("session.remember_current_position()").unwrap()
                < dom.find("session.playback_authorized = false").unwrap()
        );
        assert!(dom.contains("emit(&media, HLS_AUTOPLAY_AUTHORIZED_EVENT)"));
        assert!(dom.contains("emit(&media, HLS_EXPLICIT_PAUSE_EVENT)"));

        let errors = source_between(player, "fn handle_error(", "fn hard_recovery(");
        assert!(errors.contains("missing_video_source_buffer_error(&data)"));
        assert!(!errors.contains("source.contains(\"start=live\")"));
        assert!(!errors.contains("session.initial_position = 0.0"));
        assert!(
            errors.find("let codec_bootstrap").unwrap()
                < errors
                    .find("if !js_bool_property(&data, \"fatal\")")
                    .unwrap()
        );
        let recovery = source_between(player, "fn run_recovery(", "fn autoplay(");
        assert!(recovery.contains("matches!(session.load, LoadPhase::Warmup)"));
        let hard_recovery = source_between(recovery, "Recovery::Hard(wait", "Recovery::Stop(");
        assert!(
            hard_recovery.find("wait.wait().await").unwrap()
                < hard_recovery.find("session.restart_position()").unwrap()
        );
        assert!(hard_recovery.contains("autoplay_allowed: session.autoplay_allowed"));
        assert!(hard_recovery.contains("codec_bootstrap: session.codec_required"));
        assert!(hard_recovery.contains("start: session.start"));
        assert!(hard_recovery.contains("resume_position: session.resume_position"));

        let restart = source_between(
            player,
            "fn restart_position(&self)",
            "fn remember_current_position",
        );
        assert!(
            restart.find("if self.codec_pending").unwrap()
                < restart
                    .find("if let Some(position) = self.rebase_position")
                    .unwrap()
        );
        assert!(
            restart
                .find("if let Some(position) = self.resume_position")
                .unwrap()
                < restart
                    .find("let current = self.media.current_time()")
                    .unwrap()
        );
        assert!(!restart.contains("self.playback_authorized"));
        assert!(restart.contains("self.start == HlsStart::Beginning || current > 0.0"));
        let autoplay = source_between(player, "fn autoplay(", "fn playback_error(");
        assert!(
            autoplay.contains("matches!(intent, Autoplay::Resume) || session.autoplay_allowed")
        );
        assert!(!autoplay.contains("media.autoplay()"));
        assert!(autoplay.contains("&& !session.codec_pending"));
        assert!(autoplay.contains("if !matches!(session.load, LoadPhase::Started)"));
        assert!(
            autoplay
                .find("session.play_attempt = Some(PlayAttempt")
                .unwrap()
                < autoplay.find("media.play()").unwrap()
        );

        for policy in [
            "maxBufferLength",
            "startLoad",
            "recoverMediaError",
            "AbortController",
        ] {
            assert!(!STATIC_HLS_LOADER.contains(policy));
        }
    }

    #[test]
    fn autoplay_attempt_is_transactional_across_dom_events_and_promise_settlement() {
        let session = source_between(HLS_PLAYER, "struct Session {", "struct Player {");
        assert!(session.contains("play_attempt_serial: u64"));
        assert!(session.contains("play_attempt: Option<PlayAttempt>"));
        assert!(!session.contains("autoplay_pending"));

        let gate = source_between(
            HLS_PLAYER,
            "fn start_autoplay_buffer_gate(",
            "fn level_loaded(",
        );
        assert!(gate.contains("autoplay(epoch, intent, Some(target))"));
        assert!(!gate.contains("session.live_retarget = LiveRetarget::Inactive"));
        let wait_for_attempt = gate
            .find("session.codec_pending || session.play_attempt.is_some()")
            .unwrap();
        let target_snapshot = gate
            .find("let target = match session.live_retarget")
            .unwrap();
        assert!(wait_for_attempt < target_snapshot);

        let dom = source_between(HLS_PLAYER, "fn dom_event(", "fn handle_error(");
        let guard = dom.find("if !user {").unwrap();
        let cold = dom
            .find("let first = matches!(session.load, LoadPhase::Cold)")
            .unwrap();
        assert!(
            guard < cold,
            "an internal play event must not mutate load state"
        );
        assert!(dom.contains("return (session.media.clone(), None, false, false)"));
        assert!(dom.contains("session.play_attempt_phase()"));

        let autoplay = source_between(HLS_PLAYER, "fn autoplay(", "fn playback_error(");
        let stage = autoplay
            .find("session.play_attempt = Some(PlayAttempt")
            .unwrap();
        let seek = autoplay.find("media.set_current_time(target)").unwrap();
        let play = autoplay.find("media.play()").unwrap();
        assert!(stage < seek && seek < play);

        let snapshot = source_between(
            HLS_PLAYER,
            "fn play_attempt_snapshot_current(",
            "fn hls(&self)",
        );
        for exact in [
            "self.live_retarget == attempt.live_retarget",
            "self.resume == attempt.resume",
            "self.resume_position == attempt.resume_position",
            "self.initial_position == attempt.initial_position",
            "self.rebase_position == attempt.rebase_position",
            "attempt.intent.allowed(self)",
            "!self.playback_authorized",
            "!self.codec_pending",
        ] {
            assert!(
                snapshot.contains(exact),
                "missing transaction match: {exact}"
            );
        }

        let snapshot_check = autoplay
            .find("let snapshot_current = session.play_attempt_snapshot_current(attempt)")
            .unwrap();
        let settlement = autoplay.find("hls_play_attempt_settlement(").unwrap();
        let commit = autoplay.find("HlsPlayAttemptSettlement::Commit").unwrap();
        let consume = autoplay[commit..]
            .find("session.play_attempt = None")
            .unwrap()
            + commit;
        let retarget = autoplay[consume..]
            .find("session.live_retarget = LiveRetarget::Inactive")
            .unwrap()
            + consume;
        let authorize = autoplay[retarget..]
            .find("session.playback_authorized = true")
            .unwrap()
            + retarget;
        assert!(snapshot_check < settlement && settlement < commit);
        assert!(commit < consume && consume < retarget && retarget < authorize);

        let rollback = autoplay.find("HlsPlayAttemptSettlement::Rollback").unwrap();
        let rejecting = autoplay[rollback..]
            .find("rejecting.phase = HlsPlayAttemptPhase::Rejecting")
            .unwrap()
            + rollback;
        let preserve_current = autoplay[rejecting..].find("if !superseded {").unwrap() + rejecting;
        let restore_retarget = autoplay[preserve_current..]
            .find("session.live_retarget = attempt.live_retarget")
            .unwrap()
            + preserve_current;
        let restore_resume = autoplay[restore_retarget..]
            .find("session.resume = attempt.resume")
            .unwrap()
            + restore_retarget;
        let pause = autoplay[restore_resume..]
            .find("let _ = media.pause()")
            .unwrap()
            + restore_resume;
        let restore_target = autoplay[pause..]
            .find("media.set_current_time(target)")
            .unwrap()
            + pause;
        let drain = autoplay[restore_target..]
            .find("Wait::Millis(1).wait().await")
            .unwrap()
            + restore_target;
        let exact_rejection = autoplay[drain..]
            .find("Some((token, HlsPlayAttemptPhase::Rejecting))")
            .unwrap()
            + drain;
        let clear = autoplay[exact_rejection..]
            .find("session.play_attempt = None")
            .unwrap()
            + exact_rejection;
        let resume_current_gate = autoplay[clear..]
            .find("start_autoplay_buffer_gate(epoch, intent)")
            .unwrap()
            + clear;
        assert!(autoplay.contains("HlsPlayAttemptSettlement::Superseded"));
        assert!(autoplay.contains("target: if superseded { None } else { attempt.target }"));
        assert!(rollback < rejecting && rejecting < preserve_current);
        assert!(preserve_current < restore_retarget);
        assert!(restore_retarget < restore_resume && restore_resume < pause);
        assert!(pause < restore_target && restore_target < drain);
        assert!(drain < exact_rejection && exact_rejection < clear);
        assert!(clear < resume_current_gate);
    }

    #[test]
    fn stopped_live_recovery_is_snapshot_local_and_ignores_codec_bootstrap_buffering() {
        let codec_evidence = source_between(
            HLS_PLAYER,
            "fn missing_video_source_buffer_error(",
            "fn install_live_tail_fallback(",
        );
        for exact in [
            "js_bool_property(data, \"fatal\")",
            "Some(\"mediaError\")",
            "Some(\"bufferAppendError\")",
            "Some(\"video\")",
            "HLS_TRACK_REMOVED_ERROR_NAME",
            "HLS_MISSING_VIDEO_SOURCE_BUFFER_MESSAGE",
        ] {
            assert!(codec_evidence.contains(exact), "missing {exact}");
        }
        let exact_main = source_between(
            HLS_PLAYER,
            "fn main_swarm_fragment(",
            "fn complete_hls_codec_bootstrap(",
        );
        for exact in [
            "Some(\"main\")",
            "js_safe_u64_property(&fragment, \"sn\")",
            "js_string_property(&fragment, \"url\")",
            "swarm_bytes_reference(&url)",
        ] {
            assert!(exact_main.contains(exact), "missing {exact}");
        }
        assert!(
            HLS_PLAYER
                .contains("const HLS_TRACK_REMOVED_ERROR_NAME: &str = \"HlsJsTrackRemovedError\";")
        );
        assert!(
            HLS_PLAYER.contains(
                "\"Attempting to append to the video SourceBuffer, but it does not exist\""
            )
        );
        assert!(!codec_evidence.contains("contains(\"video SourceBuffer\")"));

        let evidence = source_between(
            HLS_PLAYER,
            "fn install_live_tail_fallback(",
            "fn handle_error(",
        );
        assert!(evidence.contains("js_bool_property(data, \"fatal\")"));
        assert!(evidence.contains("fragLoadError"));
        assert!(evidence.contains("fragLoadTimeOut"));
        assert!(evidence.contains("fragParsingError"));
        assert!(!evidence.contains("bufferAppendError"));
        assert!(evidence.contains("js_safe_u64_property(&fragment, \"sn\")"));
        assert!(evidence.contains("js_string_property(&fragment, \"url\")"));
        assert!(evidence.contains("active_hls_live_snapshot_index()"));
        assert!(evidence.contains("hls_live_tail_fallback_segment"));
        assert!(evidence.contains("session.live_tail_failures"));
        assert!(evidence.contains("session.live_startup_runway = None"));
        assert!(evidence.contains("hls_finalized_edge_load_target("));
        assert!(evidence.contains("if !session.codec_bootstrap_completed"));
        assert!(!evidence.contains("!session.codec_continuation_open"));
        assert!(evidence.contains("session.codec_continuation_open = true"));
        assert!(evidence.contains(".wrapping_add(1)"));
        assert!(evidence.contains("LiveRetarget::AwaitingFallback"));
        assert!(evidence.contains("LiveTailFallbackAction::SameInstance(hls, target, intent)"));
        assert!(!evidence.contains("media_started"));
        assert!(!evidence.contains("bufferAppendError"));

        let errors = source_between(HLS_PLAYER, "fn handle_error(", "fn hard_recovery(");
        assert!(errors.contains("session.codec_bootstrap_completed"));
        assert!(errors.contains("hls.start_load_at(target)"));
        assert!(!errors.contains("start_load_at(-1"));
        assert!(!errors.contains("recover_media_error"));

        let presented_edge = source_between(
            HLS_PLAYER,
            "fn fragment_matches_presented_edge(",
            "fn hls_live_sync_position(",
        );
        assert!(presented_edge.contains("fragment.identity.0 == presented.0"));
        assert!(presented_edge.contains("fragment.identity.1 == presented.1"));
        assert!(presented_edge.contains("fragment.duration - presented.3"));
        assert!(!presented_edge.contains("fragment.start - presented.2"));

        let level = source_between(HLS_PLAYER, "fn level_loaded(", "fn dom_event(");
        let runtime_edge = level
            .find("active_hls_finalized_playable_presentation")
            .unwrap();
        let validate = level
            .find("fragment_matches_presented_edge(fragment, presented)")
            .unwrap();
        let barrier = level
            .find("LiveRetarget::AwaitingFallback(target) =>")
            .unwrap();
        let target = level[barrier..]
            .find("session.live_retarget = LiveRetarget::AwaitingTarget(target)")
            .unwrap()
            + barrier;
        let stop = level[target..].find("hls.stop_load()").unwrap() + target;
        let seek = level[stop..]
            .find("media.set_current_time(target)")
            .unwrap()
            + stop;
        let microtask = level[seek..].find("Wait::Microtask.wait().await").unwrap() + seek;
        let restart = level[microtask..]
            .find("start_at(epoch, &hls, target)")
            .unwrap()
            + microtask;
        assert!(runtime_edge < validate && validate < barrier && barrier < target);
        assert!(target < stop && stop < seek && seek < microtask && microtask < restart);

        let buffered = source_between(HLS_PLAYER, "fn fragment_buffered(", "fn manifest_parsed(");
        assert!(!buffered.contains("media_started"));
        assert!(!buffered.contains("live_tail_failures"));
        let hard_restart = source_between(
            HLS_PLAYER,
            "Recovery::Hard(wait, media, source, attempt, timeline_rebased) =>",
            "Recovery::Stop(message, detail) =>",
        );
        assert!(hard_restart.contains("live_tail_failures: session.live_tail_failures.clone()"));

        let install = source_between(
            HLS_PLAYER,
            "pub(super) fn install_hls_live_tail_fallback(",
            "fn apply_hls_live_tail_fallback(",
        );
        assert!(install.contains("present_hls_live_fallback("));
        assert!(install.contains("hls_live_retreat_boundary(&current"));
        assert!(install.contains("sequence >= fallback.hidden_end_sequence"));
        assert!(install.contains("HLS_LIVE_TAIL_FALLBACK_MAX_RETREATS"));
        assert!(install.contains("HLS_LIVE_TAIL_FALLBACK_MAX_HIDDEN_SECONDS"));
        assert!(install.contains("snapshot.index != expected_snapshot_index"));
        assert!(!install.contains("|| snapshot.finalized"));
        let clear_startup_runway = install
            .find("session.clear_live_startup_body_runway()")
            .unwrap();
        let fallback_state = install.find("session.live_tail_fallback = Some").unwrap();
        let reopen = install
            .find("current.phase = HlsCodecBootstrapPhase::ContinuationOpen")
            .unwrap();
        assert!(clear_startup_runway < fallback_state && fallback_state < reopen);
        assert!(!install.contains("reset_hls_codec_bootstrap"));
        assert!(!install.contains("reset_hls_prefetch_timeline_plans"));
        assert!(!install.contains("clear_hls_progressive_range_owners"));

        let playable = source_between(
            HLS_PLAYER,
            "pub(super) fn active_hls_finalized_playable_presentation(",
            "pub(super) fn handoff_hls_codec_bootstrap_prefetch_timeline(",
        );
        assert!(playable.contains("present_hls_live_fallback("));
        assert!(playable.contains("hls_is_finalized(&presented)"));
        assert!(playable.contains("hls_last_playable_segment(&presented)"));
        assert!(playable.contains("active_hls_finalized_playable_interval"));
        assert!(playable.contains(".map(|(interval, _)| interval)"));

        let apply = source_between(
            HLS_PLAYER,
            "fn apply_hls_live_tail_fallback(",
            "fn trim_feed_route_cache(",
        );
        assert!(apply.contains("present_hls_live_fallback("));
        assert!(apply.contains("current.snapshot_index = snapshot.index"));
        assert!(apply.contains("playback.session.live_tail_fallback = None"));
        assert!(apply.contains("return snapshot"));
        assert!(!apply.contains("snapshot.finalized = false"));
        assert!(!apply.contains("snapshot.index > fallback.snapshot_index"));
        assert!(!apply.contains("#EXT-X-ENDLIST"));
        assert!(!apply.contains("reset_hls_prefetch_timeline_plans"));
        assert!(!apply.contains("clear_hls_progressive_range_owners"));
        assert!(!apply.contains("playback.plans.clear"));

        let fetch = source_between(
            HLS_PLAYER,
            "async fn fetch_feed_response(",
            "async fn load_feed_snapshot(",
        );
        assert!(
            fetch.find("apply_hls_live_tail_fallback").unwrap()
                < fetch.find("if !is_hls_manifest").unwrap()
        );
        assert!(fetch.contains(
            "cached_bootstrap.is_none() && index_hint.is_none() && start == HlsStart::Live"
        ));
        assert!(
            fetch.find("apply_hls_live_tail_fallback").unwrap()
                < fetch
                    .find("current.snapshot = Some(FeedRouteSnapshot")
                    .unwrap()
        );
    }

    #[test]
    fn hls_example_embeds_a_caller_owned_video() {
        assert!(HLS_STREAM_EXAMPLE.contains(r#"import init, { Weeb3No103 } from "./weeb_3.js""#));
        assert!(
            HLS_STREAM_EXAMPLE
                .contains(r#"<video id="stream" controls autoplay muted playsinline>"#)
        );
        assert!(HLS_STREAM_EXAMPLE.contains("node.start()"));
        assert!(HLS_STREAM_EXAMPLE.contains("node.attachStream(video, owner, topic, start)"));
        assert!(HLS_STREAM_EXAMPLE.contains(r#"attach("beginning")"#));
        assert!(HLS_STREAM_EXAMPLE.contains(r#"attach("live")"#));
        assert!(!HLS_STREAM_EXAMPLE.contains("renderInterface"));
        assert!(!HLS_STREAM_EXAMPLE.contains("history.replaceState"));
        let interface_player = source_between(
            HLS_PLAYER,
            "fn create_hls_player() -> Element {",
            "fn render_stream_status_for_generation(",
        );
        assert!(interface_player.contains("media.set_default_muted(true)"));
        assert!(interface_player.contains("media.set_muted(true)"));
        assert!(
            MAIN_SERVER.contains(r#".route("/weeb-3/hls-stream-example.html", get(get_example))"#)
        );
        assert!(HAXE_BUILD.contains("cp', [ './static/hls-stream-example.html', './docs/' ]"));
        assert!(!HAXE_BUILD.contains("rm', [ '-f', './docs/hls-stream-example.html' ]"));
        assert!(!HAXE_BUILD.contains("git', ['add', '-f', './static/hls-stream-example.html'"));
        assert!(!HAXE_BUILD.contains("git', ['add', '-f', './static/hls_loader.js'"));
        assert!(!STATIC_EXAMPLE.contains("hls.js"));
        assert!(!NPM_README.contains("one active `Weeb3No103` node and HLS session"));

        for removed in [
            "openStreamFeed",
            "open_stream_feed",
            "playHlsStream",
            "play_hls_stream",
            "attachHlsStream",
            "attach_hls_stream",
            "detachHlsStream",
            "detach_hls_stream",
            "configureStreamingRoutes",
            "configure_streaming_routes",
            "uploadRedundancyOptions",
            "defaultUploadRedundancyLevel",
        ] {
            assert!(!LIBRARY.contains(removed), "library exports {removed}");
            assert!(
                !HLS_STREAM_EXAMPLE.contains(removed),
                "HLS example uses removed API {removed}"
            );
            assert!(
                !NPM_WORKFLOW.contains(removed),
                "npm workflow exports {removed}"
            );
        }
    }

    #[test]
    fn npm_and_interface_share_the_core_runtime_and_network_dial_path() {
        assert!(RUNTIME.contains("runtime_started: AtomicBool"));
        assert!(RUNTIME.contains("self.runtime_started.swap(true, Ordering::AcqRel)"));
        assert!(LIBRARY.contains("inner: Arc<Weeb3>"));
        assert!(!LIBRARY.contains("Swarm<Behaviour>"));
        assert!(LIBRARY.contains("start_weeb3_runtime(inner.clone())"));
        assert!(LIBRARY.contains("connect_bootnodes_for_current_network("));

        let npm_start = source_between(
            LIBRARY,
            "fn schedule_start(&self, options: StartOptions)",
            "async fn boot_runtime(&self)",
        );
        assert!(npm_start.contains("install_service_worker_message_bridge(self.inner.clone())"));
        assert!(npm_start.contains("get_service_worker().await"));

        let implicit_start = source_between(
            LIBRARY,
            "fn start_options_from_js(options: Option<JsValue>)",
            "async fn call_promise(",
        );
        assert!(implicit_start.contains("route_network_mode_from_location()"));
        assert!(INTERFACE_RUNTIME.contains("connect_bootnodes_for_current_network("));
        assert!(RUNTIME.contains("pub(crate) async fn connect_bootnodes_for_current_network("));

        let mount = source_between(
            INTERFACE,
            "pub(crate) async fn mount_interface_after_service_worker_bridge_install(",
            "let mut last_progress_revision",
        );
        assert!(
            mount.find("connect_all_bootnode_settings(").unwrap()
                < mount.find("if read_initial_routes").unwrap()
        );
    }

    #[test]
    fn npm_data_plane_uses_the_shared_startup_barrier() {
        for (start, end) in [
            (
                "pub async fn retrieve(&self",
                "#[wasm_bindgen(js_name = retrieveBytes)]",
            ),
            ("pub async fn post_push_chunk_js(", "pub async fn upload("),
            (
                "pub async fn upload_with_redundancy(",
                "#[wasm_bindgen(js_name = postUploadBytes)]",
            ),
            (
                "pub async fn acquire_feed_bytes(&self",
                "#[wasm_bindgen(js_name = batchState)]",
            ),
            (
                "pub async fn attach_stream(",
                "#[wasm_bindgen(js_name = networkState)]",
            ),
        ] {
            assert!(
                source_between(LIBRARY, start, end).contains("self.boot_runtime().await"),
                "{start} bypasses the shared startup barrier"
            );
        }
        let attach = source_between(
            LIBRARY,
            "pub async fn attach_stream(",
            "#[wasm_bindgen(js_name = networkState)]",
        );
        assert!(attach.contains("attach_hls_feed_player("));
        assert!(attach.contains("release_current_stream_view()"));
        assert!(HLS_PLAYER.contains("source.push_str(\"?start=live\")"));
    }

    #[test]
    fn npm_render_mount_cannot_reopen_a_superseded_route() {
        let render = source_between(
            LIBRARY,
            "pub fn render_interface(&self, container: Element)",
            "#[wasm_bindgen(js_name = attachStream)]",
        );
        assert!(render.contains("self.startup_pending.fetch_add(1, Ordering::AcqRel)"));
        assert!(render.contains("let guard = serial.lock().await"));
        assert!(render.contains("route_network_mode_from_location()"));
        assert!(render.contains("pending.fetch_sub(1, Ordering::AcqRel)"));
        assert!(render.contains("Some(initial_result_generation)"));

        let dial = render.find("schedule_bootnode_dials(").unwrap();
        let release = render
            .find("pending.fetch_sub(1, Ordering::AcqRel)")
            .unwrap();
        assert!(dial < release);

        let mount = source_between(
            INTERFACE,
            "pub(crate) async fn mount_interface_after_service_worker_bridge_install(",
            "let mut last_progress_revision",
        );
        assert!(mount.contains("initial_result_generation: Option<u64>"));
        assert!(mount.contains("result_view_request_is_current(generation)"));
    }

    #[test]
    fn hls_buffer_targets_are_device_aware() {
        let config = source_between(
            HLS_PLAYER,
            "fn hls_config(live_start: bool) -> Object {",
            "fn swarm_load_policy() -> Object {",
        );

        assert!(config.contains("(30.0, 60.0, 32.0)"));
        assert!(config.contains("(90.0, 120.0, 96.0)"));
        assert!(config.contains(r#"number("maxBufferLength", length);"#));
        assert!(config.contains(r#"number("maxMaxBufferLength", maximum);"#));
        assert!(config.contains(r#"number("maxBufferSize", bytes * 1024.0 * 1024.0);"#));
        assert!(config.contains(r#"set_property(&config, "progressive", JsValue::TRUE)"#));
        assert!(
            config.find("if live_start").unwrap()
                < config.find("\"liveSyncDurationCount\"").unwrap()
        );
        assert!(config.contains("\"liveSyncDurationCount\""));
        assert!(config.contains("HLS_LIVE_SYNC_SEGMENTS"));
        assert!(!config.contains("maxLiveSyncPlaybackRate"));
        assert!(!config.contains("\"liveMaxLatencyDurationCount\""));
        assert!(!config.contains("\"liveSyncDuration\""));
        assert!(!config.contains("liveDurationInfinity"));
    }

    #[test]
    fn codec_bootstrap_temporarily_uses_the_complete_fragment_loader() {
        let launch = source_between(
            HLS_PLAYER,
            "async fn launch(mut request: Launch)",
            "fn session_from(",
        );
        let codec = launch.find("let codec_bootstrap =").unwrap();
        let config = launch
            .find("hls_config(request.start == HlsStart::Live)")
            .unwrap();
        let construct = launch.find("let hls = construct_hls(").unwrap();
        let install = launch
            .find("install_hls_codec_bootstrap_loader(hls_class, &hls)")
            .unwrap();
        let load = launch.find(".load_source(&source)").unwrap();
        assert!(codec < config && config < construct && construct < install && install < load);

        let loaders = source_between(
            HLS_PLAYER,
            "fn install_hls_codec_bootstrap_loader(",
            "fn supports_native_hls(",
        );
        assert!(loaders.contains("js_property(config.as_ref(), \"loader\")"));
        assert!(loaders.contains("js_property(hls_class, \"DefaultConfig\")"));
        assert!(loaders.contains("Object::is(&progressive_loader, &bootstrap_loader)"));
        assert!(loaders.contains("JsValue::from_str(\"fLoader\")"));
        assert!(loaders.contains("Reflect::delete_property"));
        assert!(loaders.contains("fn with_hls_codec_bootstrap("));
        assert!(loaders.contains("if source.contains('?') { '&' } else { '?' }"));
        assert!(!loaders.contains("JsValue::from_bool"));

        let buffer_created =
            source_between(HLS_PLAYER, "fn buffer_created(", "fn main_swarm_fragment(");
        assert!(buffer_created.contains("session.codec_video_buffer_ready = true"));
        assert!(!buffer_created.contains("restore_hls_progressive_loader"));
        assert!(!buffer_created.contains("load_source"));

        let buffered = source_between(
            HLS_PLAYER,
            "fn complete_hls_codec_bootstrap(",
            "fn fragment_buffered(",
        );
        let restore = buffered
            .find("restore_hls_progressive_loader(&hls)")
            .unwrap();
        let seek = buffered
            .find("media.set_current_time(target.max(0.0))")
            .unwrap();
        let release = buffered.find("session.codec_pending = false").unwrap();
        assert!(release < restore && restore < seek);
        assert!(buffered.contains("main_swarm_fragment(data)"));
        assert!(buffered.contains("codec_loading_fragments.contains(&fragment_identity)"));
        assert!(buffered.contains("session.codec_bootstrap_completed = true"));
        assert!(HLS_PLAYER.contains("\"hlsFragBuffered\" => fragment_buffered(epoch, &data)"));
        assert!(
            !source_between(HLS_PLAYER, "fn handle_error(", "fn hard_recovery(")
                .contains("!session.source.contains(\"start=live\")")
        );
    }

    #[test]
    fn codec_readiness_is_per_hls_mse_and_every_destructive_relaunch_reboots() {
        assert_eq!(
            HLS_PLAYER.matches("Launch {").count(),
            4,
            "one declaration and exactly three construction sites must carry codec state"
        );
        let initial = source_between(
            HLS_PLAYER,
            "pub(crate) async fn play_hls(",
            "async fn launch(mut request: Launch)",
        );
        assert!(initial.contains("codec_bootstrap: false"));
        assert!(initial.contains("codec_edge_position: None"));
        assert!(!initial.contains("codec_bootstrap_completed:"));

        let session = source_between(HLS_PLAYER, "fn session_from(", "fn install_session(");
        assert!(session.contains("codec_required: codec.0"));
        assert!(session.contains("codec_pending: codec.1"));
        assert!(session.contains("codec_bootstrap_completed: false"));
        assert!(!session.contains("request.codec_bootstrap_completed"));

        let current_token = source_between(
            HLS_PLAYER,
            "pub(super) fn hls_codec_bootstrap_token_is_current(",
            "fn next_epoch()",
        );
        assert!(current_token.contains("matches!(&session.backend, Backend::Hls(_))"));
        assert!(
            current_token.contains("hls_codec_bootstrap_token(&session.source) == Some(token)")
        );

        let route = source_between(
            HLS_PLAYER,
            "pub(crate) async fn try_fetch_response(",
            "fn canonical_hls_bytes_resource(",
        );
        let parsed_token = route.find("let Ok(token) = token.parse::<u64>()").unwrap();
        let active_guard = route
            .find("hls_codec_bootstrap_token_is_current(token)")
            .unwrap();
        let stale = route.find("stale HLS codec bootstrap").unwrap();
        let mutate = route.find("hls_codec_bootstrap_manifest(token)").unwrap();
        let retrieve = route.find("fetch_feed_response(").unwrap();
        assert!(parsed_token < active_guard && active_guard < stale && stale < mutate);
        assert!(mutate < retrieve);

        let rebase = source_between(HLS_PLAYER, "fn level_loaded(", "fn dom_event(");
        assert!(rebase.contains("let codec_bootstrap = session.codec_required"));
        assert!(rebase.contains("with_hls_codec_bootstrap(&session.source, epoch)"));
        assert!(rebase.contains("codec_bootstrap,"));
        assert!(rebase.contains("LiveRetarget::Inactive"));
        assert!(!rebase.contains("codec_bootstrap_completed:"));

        let recovery = source_between(HLS_PLAYER, "Recovery::Hard(wait", "Recovery::Stop(");
        assert!(recovery.contains("codec_bootstrap: session.codec_required"));
        assert!(recovery.contains("LiveRetarget::Inactive"));
        assert!(recovery.contains("codec_edge_position: session.codec_edge_position"));
        assert!(!recovery.contains("codec_bootstrap_completed:"));

        let completion = source_between(
            HLS_PLAYER,
            "fn complete_hls_codec_bootstrap(",
            "fn fragment_buffered(",
        );
        let exact_match = completion
            .find("codec_loading_fragments.contains(&fragment_identity)")
            .unwrap();
        let completed_guard = completion
            .find("|| session.codec_bootstrap_completed")
            .unwrap();
        let stop = completion.find("let Err(error) = hls.stop_load()").unwrap();
        let pending_flip = completion.find("session.codec_pending = false").unwrap();
        let completed_flip = completion
            .find("session.codec_bootstrap_completed = true")
            .unwrap();
        let finish = completion
            .find("finish_hls_codec_bootstrap(&source)")
            .unwrap();
        let restore = completion
            .find("restore_hls_progressive_loader(&hls)")
            .unwrap();
        let same_token_restart = completion
            .find("start_at(epoch, &hls, target.position())")
            .unwrap();
        assert!(exact_match < completed_guard);
        assert!(completed_guard < stop && stop < pending_flip);
        assert!(pending_flip < completed_flip && completed_flip < finish);
        assert!(finish < restore && restore < same_token_restart);
        assert!(completion.contains("session.codec_loading_fragments.clear()"));
        assert!(!completion.contains("strip_hls_codec_bootstrap"));
        assert!(!completion.contains("load_source"));
        assert!(!completion.contains("session.source ="));

        let errors = source_between(HLS_PLAYER, "fn handle_error(", "fn hard_recovery(");
        let completed_rejection = errors.find("|| session.codec_bootstrap_completed").unwrap();
        let rearm = errors.find("session.codec_required = true").unwrap();
        let edge = errors
            .find("session.codec_edge_position = codec_edge_position")
            .unwrap();
        let ordinary_media = errors.find("Some(\"mediaError\") =>").unwrap();
        assert!(completed_rejection < rearm && rearm < edge && edge < ordinary_media);
        let codec_hard = errors[ordinary_media..]
            .find("if with_session(epoch, |session| session.codec_required)")
            .unwrap()
            + ordinary_media;
        let hard = errors[codec_hard..]
            .find("hard_recovery(epoch, data)")
            .unwrap()
            + codec_hard;
        let ordinary_recover = errors[hard..].find("Recovery::Media(hls)").unwrap() + hard;
        assert!(ordinary_media < codec_hard && codec_hard < hard && hard < ordinary_recover);
        assert!(errors[ordinary_media..].contains("hard_recovery(epoch, data)"));

        let hard = source_between(HLS_PLAYER, "fn hard_recovery(", "fn start_at(");
        assert!(hard.contains("if session.codec_required"));
        assert!(hard.contains("session.codec_pending = true"));
        assert!(hard.contains("session.codec_bootstrap_completed = false"));
        assert!(hard.contains("with_hls_codec_bootstrap(&session.source, epoch)"));
    }

    #[test]
    fn codec_bootstrap_loads_sequence_zero_before_restoring_the_saved_position() {
        let restart = source_between(
            HLS_PLAYER,
            "fn restart_position(&self)",
            "fn remember_current_position",
        );
        let pending = restart.find("if self.codec_pending").unwrap();
        let zero = restart[pending..].find("return 0.0").unwrap() + pending;
        let retained = restart
            .find("if let Some(position) = self.resume_position")
            .unwrap();
        assert!(pending < zero && zero < retained);

        let manifest = source_between(
            HLS_PLAYER,
            "fn manifest_parsed(",
            "fn start_autoplay_buffer_gate(",
        );
        assert!(manifest.contains("let position = session.restart_position()"));
        assert!(!manifest.contains(".or(session.resume_position)"));
        assert!(manifest.contains("start_at(epoch, &hls, position)"));

        let buffer = source_between(
            HLS_PLAYER,
            "fn complete_hls_codec_bootstrap(",
            "fn fragment_buffered(",
        );
        let saved = buffer.find("session.resume_position").unwrap();
        let restore = buffer
            .find("media.set_current_time(target.max(0.0))")
            .unwrap();
        let release = buffer.find("session.codec_pending = false").unwrap();
        assert!(saved < release && release < restore);

        let recovery = source_between(HLS_PLAYER, "Recovery::Hard(wait", "Recovery::Stop(");
        assert!(recovery.contains("initial_position: session.restart_position()"));
        assert!(recovery.contains("resume_position: session.resume_position"));
        assert!(recovery.contains("codec_bootstrap: session.codec_required"));
        assert!(recovery.contains("codec_edge_position: session.codec_edge_position"));
        assert!(!recovery.contains("codec_bootstrap_completed:"));
    }

    #[test]
    fn live_codec_bootstrap_continues_the_same_token_and_mse_before_playback_gate() {
        let buffer = source_between(
            HLS_PLAYER,
            "fn complete_hls_codec_bootstrap(",
            "fn fragment_buffered(",
        );
        let stop = buffer.find("let Err(error) = hls.stop_load()").unwrap();
        let pending = buffer
            .find("session.live_retarget = LiveRetarget::AwaitingContinuation(target)")
            .unwrap();
        let finish = buffer.find("finish_hls_codec_bootstrap(&source)").unwrap();
        let handoff = buffer
            .find("handoff_hls_codec_bootstrap_prefetch_timeline()")
            .unwrap();
        let seek = buffer
            .find("media.set_current_time(target.position())")
            .unwrap();
        let microtask = buffer.find("Wait::Microtask.wait().await").unwrap();
        let transition = buffer
            .find("start_at(epoch, &hls, target.position())")
            .unwrap();
        let gate = buffer
            .find("start_autoplay_buffer_gate(epoch, Autoplay::Resume)")
            .unwrap();
        assert!(stop < pending && pending < finish && finish < handoff);
        assert!(handoff < seek && seek < microtask && microtask < transition && transition < gate);
        assert!(!buffer.contains("strip_hls_codec_bootstrap"));
        assert!(!buffer.contains("load_source"));
        assert!(!buffer.contains("session.source ="));
        assert!(
            buffer.contains(
                "live_codec_retarget(resume, resume_position, session.codec_edge_position)"
            )
        );
        assert!(buffer.contains("session.level_snapshots.clear()"));
        assert!(buffer.contains("session.load = LoadPhase::Warmup"));
        assert!(buffer.contains("session.codec_bootstrap_completed = true"));
        assert!(buffer.contains("session.codec_continuation_open = true"));
        assert!(buffer.contains("session.codec_continuation_revision = 1"));
        assert!(!buffer.contains("autoplay(epoch,"));

        let warmup_stop =
            source_between(HLS_PLAYER, "fn fragment_buffered(", "fn manifest_parsed(");
        assert!(
            warmup_stop
                .matches("session.live_retarget.pending()")
                .count()
                >= 2
        );
        assert!(
            warmup_stop
                .matches("session.codec_continuation_open")
                .count()
                >= 2
        );
        assert!(warmup_stop.matches("session.autoplay_gate_pending").count() >= 2);
        assert!(warmup_stop.matches("session.play_attempt").count() >= 2);
        assert!(warmup_stop.contains("&& !session.codec_pending"));
        assert!(warmup_stop.contains("|| session.codec_pending"));

        let logical_handoff = source_between(
            HLS_PLAYER,
            "pub(super) fn handoff_hls_codec_bootstrap_prefetch_timeline(",
            "pub(super) fn install_hls_live_tail_fallback(",
        );
        assert!(logical_handoff.contains("session.startup_deadline_ms = now"));
        assert!(logical_handoff.contains("session.mode = HlsPrefetchMode::StartupOnly"));
        assert!(!logical_handoff.contains("reset_hls_prefetch_timeline_plans"));
        assert!(!logical_handoff.contains("clear_hls_progressive_range_owners"));
        assert!(!logical_handoff.contains("playback.plans.clear"));
        assert!(!logical_handoff.contains("session.tracks.clear"));
        assert!(!logical_handoff.contains("advance_generation"));

        let phases = source_between(
            HLS_PLAYER,
            "enum HlsCodecBootstrapPhase {",
            "struct HlsCodecBootstrapPresentation",
        );
        assert!(phases.contains("Bootstrap"));
        assert!(phases.contains("ContinuationOpen"));
        assert!(phases.contains("Settled"));

        let finish = source_between(
            HLS_PLAYER,
            "pub(super) fn finish_hls_codec_bootstrap(",
            "pub(super) fn settle_hls_codec_continuation(",
        );
        assert!(finish.contains("current.phase == HlsCodecBootstrapPhase::Bootstrap"));
        assert!(finish.contains("current.phase = HlsCodecBootstrapPhase::ContinuationOpen"));
        assert!(finish.contains("current.snapshot = None"));
        let settle = source_between(
            HLS_PLAYER,
            "pub(super) fn settle_hls_codec_continuation(",
            "fn reset_hls_codec_bootstrap(",
        );
        assert!(settle.contains("current.phase != HlsCodecBootstrapPhase::ContinuationOpen"));
        assert!(settle.contains("current.phase = HlsCodecBootstrapPhase::Settled"));

        let response = source_between(
            HLS_PLAYER,
            "async fn fetch_feed_response(",
            "async fn load_feed_snapshot(",
        );
        let normal_rewrite = response
            .find("rewrite_hls_manifest_for_live_reload(")
            .unwrap();
        let open = response.find("open_hls_codec_continuation(&body)").unwrap();
        assert!(normal_rewrite < open);
        assert!(response.contains("current.phase == HlsCodecBootstrapPhase::Bootstrap"));

        let restart = source_between(
            HLS_PLAYER,
            "fn restart_position(&self)",
            "fn remember_current_position",
        );
        let live = restart.find("match self.live_retarget").unwrap();
        let edge = restart[live..]
            .find("target.startup_position(self.live_startup_runway.as_ref())")
            .unwrap()
            + live;
        let runway = restart
            .find("if self.live_startup_runway_required")
            .unwrap();
        let runway_target = restart[runway..].find("return runway.target").unwrap() + runway;
        let saved = restart
            .find("if let Some(position) = self.resume_position")
            .unwrap();
        assert!(live < edge && edge < runway && runway < runway_target && runway_target < saved);

        let manifest = source_between(
            HLS_PLAYER,
            "fn manifest_parsed(",
            "fn start_autoplay_buffer_gate(",
        );
        assert!(manifest.contains("let position = session.restart_position()"));
        assert!(manifest.contains("start_at(epoch, &hls, position)"));
        assert!(!manifest.contains("LiveRetarget::AwaitingContinuation"));

        let autoplay_gate = source_between(
            HLS_PLAYER,
            "fn start_autoplay_buffer_gate(",
            "fn level_loaded(",
        );
        assert!(autoplay_gate.contains("match session.live_retarget"));
        assert!(autoplay_gate.contains("LiveRetarget::AwaitingContinuation(_)"));
        assert!(autoplay_gate.contains("LiveRetarget::AwaitingFallback(_)"));
        assert!(autoplay_gate.contains("LiveRetarget::AwaitingTarget(retarget)"));
        assert!(autoplay_gate.contains("hls_live_retarget_target("));
        assert!(autoplay_gate.contains("retarget.startup_position(live_startup_runway)"));
        let edge_target = source_between(
            autoplay_gate,
            "LiveRetarget::AwaitingTarget(retarget)",
            "LiveRetarget::Inactive",
        );
        assert_eq!(edge_target.matches("None,").count(), 2);
        assert!(autoplay_gate.contains("session.live_startup_runway_required"));
        assert!(autoplay_gate.contains("hls_live_autoplay_runway_ready("));
        assert!(!autoplay_gate.contains("session.live_retarget = LiveRetarget::Inactive"));
        assert!(
            autoplay_gate
                .find("AutoplayGatePoll::Ready(target)")
                .unwrap()
                < autoplay_gate
                    .find("autoplay(epoch, intent, Some(target))")
                    .unwrap()
        );

        let level = source_between(HLS_PLAYER, "fn level_loaded(", "fn dom_event(");
        assert!(level.contains("live_level_autoplay_runway(&details)"));
        let runway_lock = source_between(
            level,
            "let mut startup_restart = None",
            "if !classify_hls_level_transition(",
        );
        let continuation = runway_lock
            .find("session.live_retarget = LiveRetarget::AwaitingTarget(target)")
            .unwrap();
        let lock = runway_lock
            .find("if session.live_startup_runway_required")
            .unwrap();
        assert!(continuation < lock);
        assert!(runway_lock.contains("session.live_startup_runway.is_none()"));
        assert!(runway_lock.contains("session.play_attempt.is_none()"));
        assert!(runway_lock.contains("LiveRetarget::Inactive | LiveRetarget::AwaitingTarget(_)"));
        assert!(runway_lock.contains("session.live_startup_runway = Some(candidate)"));
        assert!(
            runway_lock.contains("Some((session.presentation_id, candidate.references.clone()))")
        );
        assert!(runway_lock.contains("LiveRetargetTarget::Edge(_)"));
        assert!(runway_lock.contains("session.manifest_parsed"));
        assert!(runway_lock.contains("startup_restart = session"));
        let startup_restart = source_between(
            level,
            "if let Some((media, hls, target)) = startup_restart",
            "if let Some(intent) = gate",
        );
        assert!(startup_restart.contains("hls.stop_load()"));
        assert!(startup_restart.contains("media.set_current_time(target)"));
        assert!(startup_restart.contains("start_at(epoch, &hls, target)"));
        assert!(level.contains("live_startup_runway: None"));

        let retarget = source_between(
            HLS_PLAYER,
            "impl LiveRetargetTarget {",
            "enum LiveRetarget {",
        );
        assert!(retarget.contains("(Self::Edge(_), Some(runway)) => runway.target"));
        assert!(retarget.contains("Self::Resume(position)"));

        let dom = source_between(HLS_PLAYER, "fn dom_event(", "fn handle_error(");
        assert!(dom.contains("|| session.live_startup_runway_required"));
        assert!(dom.contains("start_autoplay_buffer_gate(epoch, Autoplay::Resume)"));

        let recovery = source_between(HLS_PLAYER, "Recovery::Hard(wait", "Recovery::Stop(");
        assert!(recovery.contains("live_retarget: if session.codec_required"));
        assert!(recovery.contains("LiveRetarget::Inactive"));
        assert!(
            recovery.contains("live_startup_runway_required: session.live_startup_runway_required")
        );
        assert!(recovery.contains("live_startup_runway: session.live_startup_runway.clone()"));
        let session = source_between(HLS_PLAYER, "fn session_from(", "fn install_session(");
        assert!(session.contains("request.live_retarget"));
        assert!(session.contains("let hls_backend = matches!(&backend, Backend::Hls(_))"));
        assert!(session.contains("hls_backend && request.live_startup_runway_required"));
        assert!(session.contains("request.live_startup_runway_required"));
        assert!(session.contains("request.live_startup_runway.clone()"));
        assert!(session.contains("presentation_id: request.presentation_id"));

        let player_session = source_between(HLS_PLAYER, "struct Session {", "struct Player {");
        let launch = source_between(HLS_PLAYER, "struct Launch {", "enum EventKind {");
        assert!(player_session.contains("presentation_id: u64"));
        assert!(launch.contains("presentation_id: u64"));
        assert_eq!(
            HLS_PLAYER
                .matches("presentation_id: session.presentation_id")
                .count(),
            2,
            "timeline and hard relaunches must retain the presentation owner"
        );
        assert!(level.contains("lock_hls_live_startup_body_runway(presentation_id, references)"));

        let runtime_lock = source_between(
            HLS_PLAYER,
            "pub(super) fn lock_hls_live_startup_body_runway(",
            "pub(super) fn unlock_hls_live_startup_body_runway(",
        );
        let runtime_unlock = source_between(
            HLS_PLAYER,
            "pub(super) fn unlock_hls_live_startup_body_runway(",
            "fn activate_hls_prefetch_warmup(",
        );
        for contract in [runtime_lock, runtime_unlock] {
            assert!(contract.contains("expected_presentation_id: u64"));
            let nonzero = contract.find("expected_presentation_id == 0").unwrap();
            let owner = contract
                .find("session.presentation_id != expected_presentation_id")
                .unwrap();
            let mutation = contract.find("session.live_startup_body_runway").unwrap();
            assert!(nonzero < owner && owner < mutation);
        }
        let errors = source_between(HLS_PLAYER, "fn handle_error(", "fn hard_recovery(");
        let hard = source_between(HLS_PLAYER, "fn hard_recovery(", "fn start_at(");
        assert!(errors.contains("unlock_hls_live_startup_body_runway(presentation_id)"));
        assert!(hard.contains("unlock_hls_live_startup_body_runway(presentation_id)"));

        let autoplay = source_between(HLS_PLAYER, "fn autoplay(", "fn set_state(");
        let authorize = autoplay.find("session.playback_authorized = true").unwrap();
        let release = autoplay
            .find("session.live_startup_runway_required = false")
            .unwrap();
        assert!(authorize < release);
        assert!(autoplay.contains("session.live_startup_runway = None"));
    }

    #[test]
    fn codec_bootstrap_defers_explicit_play_until_same_token_level_barrier() {
        let dom = source_between(HLS_PLAYER, "fn dom_event(", "fn handle_error(");
        let defer = dom.find("&& (session.codec_pending").unwrap();
        let remember = dom[defer..].find("session.resume = true").unwrap() + defer;
        let pause = dom.find("let _ = media.pause()").unwrap();
        let gate = dom
            .find("start_autoplay_buffer_gate(epoch, Autoplay::Resume)")
            .unwrap();
        assert!(defer < remember && remember < pause && pause < gate);

        let manifest = source_between(
            HLS_PLAYER,
            "fn manifest_parsed(",
            "fn start_autoplay_buffer_gate(",
        );
        let pending = manifest.find("session.live_retarget.pending()").unwrap();
        let load = manifest.find("session.load = LoadPhase::Warmup").unwrap();
        let start = manifest.find("start_at(epoch, &hls, position)").unwrap();
        assert!(pending < load && load < start);
        assert!(!manifest.contains("LiveRetarget::AwaitingContinuation(target)"));
        assert!(!manifest.contains("LiveRetarget::AwaitingTarget(target)"));

        let level = source_between(HLS_PLAYER, "fn level_loaded(", "fn dom_event(");
        assert!(level.contains("let playable_interval = ((!live || continuation)"));
        assert!(level.contains("let finalized_playable = (!live || continuation_open)"));
        assert!(level.contains("let accepted_edge = continuation_open"));
        assert!(level.contains(".then(|| accepted_continuation_edge(&details))"));
        let details = level.find("accepted_continuation_edge(&details)").unwrap();
        let latched_revision = level.find("codec_continuation_loading_fragments").unwrap();
        let barrier = level
            .find("LiveRetarget::AwaitingContinuation(target) =>")
            .unwrap();
        let crossed = level
            .find("session.live_retarget = LiveRetarget::AwaitingTarget(target)")
            .unwrap();
        let gate = level
            .find("gate = intent.allowed(session).then_some(intent)")
            .unwrap();
        assert!(details < latched_revision && latched_revision < barrier);
        assert!(barrier < crossed && crossed < gate);
        assert!(level.contains(".get(&fragment.identity)"));
        assert!(level.contains(".map(|current| current.revision)"));

        let loading = source_between(HLS_PLAYER, "fn fragment_loading(", "fn buffer_created(");
        assert!(loading.contains("session.codec_continuation_revision"));
        assert!(loading.contains("codec_continuation_loading_fragments"));
        assert!(loading.contains("Fallback reserves a fresh revision"));

        let buffered = source_between(
            HLS_PLAYER,
            "fn settle_buffered_codec_continuation(",
            "fn fragment_buffered(",
        );
        let consume = buffered
            .find("codec_continuation_loading_fragments")
            .unwrap();
        let candidate = buffered
            .find("session.codec_continuation_candidate = Some(CodecContinuationEdge")
            .unwrap();
        let revision_match = buffered.find("edge.revision != revision").unwrap();
        let exact_match = buffered.find("!edge.fragment.matches(&fragment)").unwrap();
        assert!(consume < candidate && candidate < revision_match && revision_match < exact_match);
        assert!(buffered.contains("media_buffers_fragment(&session.media, &fragment)"));
    }

    #[test]
    fn hls_fragment_kind_replaces_only_the_matching_startup_obligation() {
        let callbacks = source_between(
            HLS_PLAYER,
            "const HLS_CALLBACK_EVENTS:",
            "const HLS_DOM_EVENTS:",
        );
        assert!(callbacks.contains("\"hlsFragLoading\""));

        let fragment = source_between(HLS_PLAYER, "fn fragment_loading(", "fn buffer_created(");
        assert!(fragment.contains("Some(\"main\") => Some(HlsMediaKind::Main)"));
        assert!(fragment.contains("Some(\"audio\") => Some(HlsMediaKind::Audio)"));
        assert!(fragment.contains("swarm_bytes_reference(&url)"));
        assert!(fragment.contains("remember_hls_fragment_kind(&reference, kind)"));

        let foreground = source_between(
            HLS_PLAYER,
            "fn hls_foreground_context(",
            "fn hls_generation_current(",
        );
        let selection = foreground
            .find("playback.plans.cursor(&reference, &preferred)")
            .unwrap();
        let kind = foreground
            .find("session.fragment_kinds.remove(&reference)")
            .unwrap();
        assert!(
            selection < kind,
            "kind must apply to the cursor selected for this request"
        );
        assert!(foreground.contains("Some(Some(kind)) =>"));
        assert!(foreground.contains("session.select_startup_runway_plan("));
        assert!(foreground.contains("Some(None) =>"));
        assert!(!foreground.contains("startup_runway_ready"));
        assert!(foreground.contains("session.retired_startup_plans.insert(cursor.plan.id)"));
        assert!(foreground.contains("session.retired_startup_plans.remove(&cursor.plan.id)"));
        assert!(foreground.contains("progressive_owner_handoff |="));
        assert!(foreground.contains(
            "retire_hls_progressive_range_owners(forward, &context.progressive_retired_references)"
        ));

        let replacement = source_between(
            HLS_PLAYER,
            "fn select_startup_runway_plan(",
            "fn body_parallelism(",
        );
        assert!(replacement.contains("self.retired_startup_plans.insert(previous)"));
        assert!(replacement.contains("-> bool"));
        assert!(!replacement.contains("remove_progressive_plan(previous)"));
    }

    #[test]
    fn hls_segment_progress_reports_media_details() {
        let detail = source_between(
            HLS_PLAYER,
            "fn hls_segment_progress_detail(",
            "#[derive(Clone)]\n    struct FeedRouteSnapshot",
        );
        assert!(detail.contains("size {:.2} MB"));
        assert!(detail.contains("duration {duration} s"));
        assert!(detail.contains("resolution {width}x{height}"));
        assert!(detail.contains("HLS_PLAYBACK.with"));
        assert!(detail.contains(".durations"));
        assert!(
            HLS_PLAYER
                .contains(r#"const HLS_DOM_EVENTS: [&str; 3] = ["play", "pause", "resize"];"#)
        );

        let range = source_between(
            HLS_PLAYER,
            "async fn retrieve_hls_payload_range(",
            "async fn latest_hls_feed_payload_startup(",
        );
        assert_eq!(range.matches("hls_segment_progress_detail(").count(), 2);
        assert_eq!(range.matches("&address, Some(payload_size)").count(), 2);
        assert!(range.contains("address.clone()"));
        assert!(range.contains("\"{} bytes, {}\""));
    }

    #[test]
    fn hls_prefetch_caps_current_generation_body_loads() {
        assert!(STATIC_WORKER.contains("const HLS_STREAM_LOOKAHEAD_CHUNKS = 4;"));
        assert!(STATIC_WORKER.contains(
            "const lookahead = stagedStart ? HLS_STREAM_LOOKAHEAD_CHUNKS : STREAM_LOOKAHEAD_CHUNKS;"
        ));
        assert!(!STATIC_WORKER.contains("stagedWindows"));
        assert!(
            HLS_PLAYER
                .contains("const HLS_EXACT_NEXT_HEAD_START: Duration = Duration::from_secs(1);")
        );
        assert!(
            HLS_PLAYER
                .contains("const HLS_NEXT_RESERVE_STAGGER: Duration = Duration::from_secs(1);")
        );
        assert!(HLS_PLAYER.contains("const HLS_STARTUP_BODY_MAX_PARALLEL: usize = 1;"));
        assert!(HLS_PLAYER.contains(
            "const HLS_STARTUP_LOOKAHEAD_BYTES: u64 = 2 * MEDIA_STARTUP_RESPONSE_BYTES;"
        ));
        assert!(HLS_PLAYER.contains("const HLS_PREFETCH_BODY_MAX_PARALLEL: usize = 3;"));
        assert!(HLS_PLAYER.contains("const HLS_SERIAL_PREFETCH_COMPLETIONS: usize = 6;"));
        assert!(HLS_PLAYER.contains("const HLS_TWO_BODY_PREFETCH_COMPLETIONS: usize = 10;"));
        assert!(!HLS_PLAYER.contains("HLS_LIVE_HISTORY_START_COMPLETIONS"));
        let generation_advance = source_between(
            HLS_PLAYER,
            "fn advance_generation(&mut self) -> u64",
            "fn advance_timeline(&mut self) -> u64",
        );
        let timeline_advance = source_between(
            HLS_PLAYER,
            "fn advance_timeline(&mut self) -> u64",
            "#[derive(Clone)]\n    struct HlsForegroundContext",
        );
        assert!(generation_advance.contains("if self.live_start"));
        assert!(generation_advance.contains("self.startup_overlap_plans.clear();"));
        assert!(!generation_advance.contains("startup_runway_ready"));
        assert!(!timeline_advance.contains("startup_runway_ready"));
        assert!(generation_advance.contains("self.clear_live_startup_body_runway();"));
        assert!(timeline_advance.contains("self.clear_live_startup_body_runway();"));
        assert!(timeline_advance.contains("fn clear_live_startup_body_runway("));
        assert!(timeline_advance.contains("self.live_startup_body_runway = None;"));
        assert!(timeline_advance.contains("self.invalidate_live_body_schedule();"));
        assert!(timeline_advance.contains(
            "self.live_body_schedule_id = next_nonzero_generation(self.live_body_schedule_id);"
        ));
        assert!(timeline_advance.contains("self.live_body_schedule_running = false;"));
        assert!(timeline_advance.contains("self.live_body_runway.clear();"));
        assert!(timeline_advance.contains("fn claim_live_body_schedule("));
        assert!(timeline_advance.contains("hls_live_body_schedule_should_spawn("));
        assert!(timeline_advance.contains("fn live_body_schedule_current("));
        assert!(timeline_advance.contains("&& self.live_body_runway == runway"));
        assert!(timeline_advance.contains("fn finish_live_body_schedule("));
        assert!(timeline_advance.contains("self.live_body_schedule_id == schedule_id"));
        assert!(generation_advance.contains("HLS_TWO_BODY_PREFETCH_COMPLETIONS"));
        assert!(!timeline_advance.contains("self.completed_media_payloads = 0;"));
        let session_start = source_between(
            HLS_PLAYER,
            "fn begin_hls_prefetch_session(",
            "fn remember_authenticated_hls_startup_prefix(",
        );
        assert!(session_start.contains("session.live_start = live_start;"));
        assert!(!HLS_PLAYER.contains("hls_startup_runway_ready"));
        assert!(!HLS_PLAYER.contains("startup_runway_ready_plans"));
        assert!(!HLS_PLAYER.contains("startup_runway_plans"));
        assert!(HLS_PLAYER.contains(
            "const HLS_EXACT_NEXT_OVERLAP_SEGMENTS: usize = HLS_LIVE_SYNC_SEGMENTS - 1;"
        ));

        let cache = source_between(HLS_PLAYER, "fn load_role(", "fn finish_load(");
        assert!(cache.contains(".filter(|pending| pending.generation == generation)"));
        assert!(cache.contains("if enforce_capacity"));
        assert!(cache.contains(">= prefetch_limit"));
        assert!(!cache.contains("pending.speculative"));

        let admission = source_between(
            HLS_PLAYER,
            "fn start_hls_payload_load(",
            "async fn wait_hls_payload_load(",
        );
        let parallelism = source_between(
            HLS_PLAYER,
            "fn body_parallelism(&self, generation: u64)",
            "fn prune_tracks(",
        );
        assert!(
            parallelism
                .contains("session.completed_media_payloads < HLS_SERIAL_PREFETCH_COMPLETIONS")
                || parallelism
                    .contains("self.completed_media_payloads < HLS_SERIAL_PREFETCH_COMPLETIONS")
        );
        assert!(
            parallelism
                .contains("session.completed_media_payloads < HLS_TWO_BODY_PREFETCH_COMPLETIONS")
                || parallelism
                    .contains("self.completed_media_payloads < HLS_TWO_BODY_PREFETCH_COMPLETIONS")
        );
        assert!(admission.contains("session.timeline_epoch == timeline_epoch"));
        assert!(admission.contains("let owned = HLS_ASSET_CACHE.with"));

        let foreground_retry = source_between(
            HLS_PLAYER,
            "fn hls_foreground_retry_is_current(",
            "fn hls_monotonic_now_ms()",
        );
        assert!(foreground_retry.contains("session.mode != HlsPrefetchMode::Inactive"));

        let stages = source_between(
            HLS_PLAYER,
            "async fn prefetch_hls_media_stages(",
            "async fn retrieve_hls_payload_for_playback(",
        );
        assert!(stages.contains("bodies.len() < body_limit"));
        assert!(stages.contains("matches!(role, HlsPayloadLoadRole::Lead(_, _))"));
        assert!(!stages.contains("HlsPayloadLoadRole::Wait(_) |"));
        assert!(HLS_PLAYER.contains(".min(HLS_TWO_BODY_PREFETCH_COMPLETIONS)"));
        assert!(
            HLS_PLAYER.contains("session.completed_media_payloads = completed_media_payloads;")
        );
        assert!(HLS_PLAYER.contains("seek_successor"));
        assert!(HLS_PLAYER.contains("wait_hls_payload_load(successor).await"));
        assert!(HLS_PLAYER.contains("spawn_hls_progressive_range_prefetch("));
        assert!(!HLS_PLAYER.contains("progressive_successor_prefix_ready"));
        assert!(!HLS_PLAYER.contains("mark_hls_progressive_successor_prefix_ready("));
        assert!(!HLS_PLAYER.contains("take_hls_progressive_successor("));
        assert!(!HLS_PLAYER.contains("prefetch_authenticated_hls_prefix"));

        let buffered = source_between(HLS_PLAYER, "fn fragment_buffered(", "fn manifest_parsed(");
        assert!(buffered.contains("sleep(HLS_WARMUP_STOP_DELAY).await"));
        assert!(buffered.contains("!matches!(session.load, LoadPhase::Warmup)"));
        assert!(buffered.contains("!session.media.paused()"));
        assert!(buffered.contains("!session.autoplay_gate_pending"));

        let level_loaded = source_between(HLS_PLAYER, "fn level_loaded(", "fn dom_event(");
        let startup_runway_lock = source_between(
            level_loaded,
            "if session.live_startup_runway_required",
            "if !classify_hls_level_transition(",
        );
        assert!(
            startup_runway_lock.contains("&& !session.codec_pending"),
            "live runway retargeting must not abort an in-flight codec bootstrap"
        );

        let plans = source_between(
            HLS_PLAYER,
            "fn remember_hls_media_plan(",
            "fn cached_hls_payload(",
        );
        assert!(plans.contains("HLS_PLAYBACK.with"));
        assert!(plans.contains("HLS_ARCHIVE_MEDIA_PLAN_MAX_REFERENCES"));
        assert!(plans.contains("playback.plans.resize(plan_limit)"));
        assert!(plans.contains("segments.len() - plan_limit"));
        assert!(plans.contains("segments.truncate(plan_limit)"));
        assert!(plans.contains("for segment in &segments"));
        assert!(plans.contains("install_with_early_overlap_limit("));
        assert!(plans.contains("&protected_plan_ids,"));
        assert!(plans.contains("playback.session.remove_progressive_plan(plan.id)"));

        let prefetch_lifecycle = source_between(
            HLS_PLAYER,
            "fn install_hls_prefetch_lifecycle(",
            "pub(crate) fn release_hls_view(",
        );
        assert_eq!(
            prefetch_lifecycle
                .matches("set_hls_prefetch_mode(HlsPrefetchMode::Inactive)")
                .count(),
            2
        );
        assert_eq!(
            prefetch_lifecycle
                .matches("set_hls_prefetch_mode(HlsPrefetchMode::Sustained)")
                .count(),
            2
        );
        assert_eq!(
            prefetch_lifecycle
                .matches("get_attribute(\"data-weeb3-hls-mode\")")
                .count(),
            2
        );
        assert_eq!(prefetch_lifecycle.matches(".is_some()").count(), 2);
        assert!(!prefetch_lifecycle.contains("== Some(\"hls.js\")"));
        assert!(prefetch_lifecycle.contains("HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE"));
        let mode_transition = source_between(
            HLS_PLAYER,
            "fn set_hls_prefetch_mode(",
            "fn activate_hls_prefetch_warmup(",
        );
        assert!(mode_transition.contains("hls_progressive_runway_closed_after_mode("));
    }

    #[test]
    fn live_foreground_generation_hold_invalidates_only_the_selected_plan() {
        let foreground = source_between(
            HLS_PLAYER,
            "fn hls_foreground_context(",
            "fn hls_generation_current(",
        );
        let discontinuity = source_between(
            foreground,
            "if transition.is_some_and(|transition| transition.0) || runway_discontinuity {",
            "if !session.tracks.contains_key(&cursor.plan.id)",
        );
        let owner_handoff = discontinuity
            .find("progressive_owner_handoff = true")
            .unwrap();
        let hold_predicate = discontinuity
            .find("let hold_generation = hls_live_startup_holds_generation(")
            .unwrap();
        let held = source_between(discontinuity, "if hold_generation {", "} else {");
        let normal = discontinuity.split_once("} else {").unwrap().1;
        let bump = held
            .find("session.schedule_sequence =\n                            next_nonzero_generation(session.schedule_sequence)")
            .unwrap();
        let replacement = held.find("track.schedule_id = schedule_id").unwrap();
        let cancel = held.find("track.running = None").unwrap();
        let overlap = held
            .find("session.startup_overlap_plans.remove(&cursor.plan.id)")
            .unwrap();
        let seek_successor = normal.find("seek_successor = cursor").unwrap();
        let advance = normal
            .find("let generation = session.advance_generation()")
            .unwrap();
        let publish_capture = normal.find("publish = session").unwrap();
        assert!(owner_handoff < hold_predicate);
        assert!(bump < replacement && replacement < cancel && cancel < overlap);
        assert!(seek_successor < advance && advance < publish_capture);
        for argument in [
            "session.live_start",
            "session.live_body_admission_open",
            "session.mode",
            "session.timeline_rebasing",
        ] {
            assert!(
                discontinuity[hold_predicate..].contains(argument),
                "generation hold is missing {argument}"
            );
        }
        assert!(!held.contains("seek_successor ="));
        assert!(!held.contains("session.advance_generation()"));
        assert!(!held.contains("publish ="));
        assert!(!normal.contains("session.schedule_sequence"));
        assert!(!normal.contains("track.running = None"));
        assert!(!normal.contains("startup_overlap_plans.remove"));
        assert_eq!(normal.matches("session.advance_generation()").count(), 1);
        assert_eq!(normal.matches("publish = session").count(), 1);
        assert_eq!(
            foreground.matches("publish_hls_stream_generation(").count(),
            1
        );

        let payload_start = source_between(
            HLS_PLAYER,
            "fn start_hls_payload_load(",
            "async fn wait_hls_payload_load(",
        );
        let live_startup = payload_start.find("let live_startup =").unwrap();
        let base_limit = payload_start[live_startup..]
            .find("let mut limit = session.body_parallelism(generation)")
            .unwrap()
            + live_startup;
        let foreground_reservation = payload_start[base_limit..]
            .find("if prefetch && live_startup {")
            .unwrap()
            + base_limit;
        let capacity = payload_start[foreground_reservation..]
            .find("prefetch || live_startup")
            .unwrap()
            + foreground_reservation;
        let cache_admission = payload_start[capacity..]
            .find(".load_role(&reference, prefetch, enforce_capacity, generation, limit)")
            .unwrap()
            + capacity;
        assert!(live_startup < base_limit && base_limit < foreground_reservation);
        assert!(foreground_reservation < capacity && capacity < cache_admission);
        assert!(payload_start.contains("session.mode != HlsPrefetchMode::Sustained"));
        assert!(
            payload_start.contains("limit.min(HLS_PREFETCH_BODY_MAX_PARALLEL.saturating_sub(1))")
        );
        assert!(HLS_PLAYER.contains("const HLS_PREFETCH_BODY_MAX_PARALLEL: usize = 3;"));

        let lock = source_between(
            HLS_PLAYER,
            "pub(super) fn lock_hls_live_startup_body_runway(",
            "pub(super) fn unlock_hls_live_startup_body_runway(",
        );
        let exact_pin = lock
            .find("session.live_startup_body_runway = Some(references.clone())")
            .unwrap();
        let exact_schedule = lock
            .find("start_hls_live_body_runway_schedule(client, stamp, references)")
            .unwrap();
        assert!(exact_pin < exact_schedule);

        let admission_mode = source_between(
            HLS_PLAYER,
            "fn hls_live_body_admission_mode(",
            "async fn fetch_hls_bytes_response(",
        );
        for guard in [
            "session.live_start",
            "session.live_body_admission_open",
            "session.generation == generation",
            "!session.timeline_rebasing",
        ] {
            assert!(admission_mode.contains(guard), "missing {guard}");
        }
        assert!(admission_mode.contains(".then_some(session.mode)"));

        let foreground_admission = source_between(
            HLS_PLAYER,
            "async fn admit_hls_foreground_payload_load(",
            "async fn retrieve_hls_payload_for_playback(",
        );
        let at_capacity = foreground_admission
            .find("if !matches!(&role, HlsPayloadLoadRole::AtCapacity)")
            .unwrap();
        let recheck = foreground_admission
            .find("hls_live_body_admission_mode(generation)")
            .unwrap();
        let wait = foreground_admission
            .find("Some(HlsPrefetchMode::Inactive | HlsPrefetchMode::StartupOnly)")
            .unwrap();
        let sleep = foreground_admission[wait..]
            .find("async_std::task::sleep(Duration::from_millis(25)).await")
            .unwrap()
            + wait;
        let sustained = foreground_admission
            .find("Some(HlsPrefetchMode::Sustained) => {}")
            .unwrap();
        let stale = foreground_admission.find("None => return None").unwrap();
        assert!(at_capacity < recheck && recheck < wait && wait < sleep);
        assert!(sleep < sustained && sustained < stale);
        assert!(!foreground_admission[sustained..stale].contains("sleep"));
        assert!(!foreground_admission[sustained..stale].contains("break"));
        assert_eq!(foreground_admission.matches("return None").count(), 1);
        assert!(!foreground_admission.contains("HLS_FOREGROUND_MAX_ATTEMPTS"));
        assert!(!foreground_admission.contains("attempts"));
        assert!(!foreground_admission.contains("retry_limit"));
        assert!(!foreground_admission.contains("deadline"));

        let retrieve = source_between(
            HLS_PLAYER,
            "async fn retrieve_hls_payload_for_playback(",
            "async fn resolve_hls_asset(",
        );
        assert_eq!(
            retrieve
                .matches("admit_hls_foreground_payload_load(")
                .count(),
            2,
            "initial and every post-error foreground admission must share the capacity gate"
        );
        let initial_admission = retrieve.find("admit_hls_foreground_payload_load(").unwrap();
        let initial_wait = retrieve[initial_admission..]
            .find("let mut body = wait_hls_payload_load(foreground).await")
            .unwrap()
            + initial_admission;
        let attempts = retrieve[initial_wait..]
            .find("let mut attempts = 1")
            .unwrap()
            + initial_wait;
        let retry_loop = retrieve[attempts..].find("while body.is_err()").unwrap() + attempts;
        let retry_admission = retrieve[retry_loop..]
            .find("admit_hls_foreground_payload_load(")
            .unwrap()
            + retry_loop;
        let retry_wait = retrieve[retry_admission..]
            .find("body = wait_hls_payload_load(foreground).await")
            .unwrap()
            + retry_admission;
        let consume_attempt = retrieve[retry_wait..].find("attempts += 1").unwrap() + retry_wait;
        assert!(initial_admission < initial_wait && initial_wait < attempts);
        assert!(attempts < retry_loop && retry_loop < retry_admission);
        assert!(retry_admission < retry_wait && retry_wait < consume_attempt);
        assert_eq!(retrieve.matches("attempts += 1").count(), 1);
        assert!(retrieve.contains("attempts < HLS_FOREGROUND_MAX_ATTEMPTS"));

        let retry_current = source_between(
            HLS_PLAYER,
            "fn hls_foreground_retry_is_current(",
            "fn hls_monotonic_now_ms(",
        );
        assert!(retry_current.contains("session.stamp() == stamp"));
        assert!(retry_current.contains("!session.timeline_rebasing"));
        assert!(retry_current.contains("session.mode != HlsPrefetchMode::Inactive"));
        assert!(retry_current.contains("session.live_start && session.live_body_admission_open"));

        let session_start = source_between(
            HLS_PLAYER,
            "fn begin_hls_prefetch_session(",
            "fn remember_authenticated_hls_startup_prefix(",
        );
        let live = session_start
            .find("session.live_start = live_start")
            .unwrap();
        let prewarmup_admission = session_start[live..]
            .find("session.live_body_admission_open = live_start")
            .unwrap()
            + live;
        let prewarmup_inactive = session_start[prewarmup_admission..]
            .find("session.mode = HlsPrefetchMode::Inactive")
            .unwrap()
            + prewarmup_admission;
        assert!(live < prewarmup_admission && prewarmup_admission < prewarmup_inactive);

        let mode_transition = source_between(
            HLS_PLAYER,
            "fn set_hls_prefetch_mode(",
            "fn activate_hls_prefetch_warmup(",
        );
        assert!(
            mode_transition
                .contains("session.live_body_admission_open = mode != HlsPrefetchMode::Inactive")
        );
        let set_mode = mode_transition.find("session.mode = mode").unwrap();
        let set_admission = mode_transition
            .find("session.live_body_admission_open = mode != HlsPrefetchMode::Inactive")
            .unwrap();
        assert!(set_mode < set_admission);

        let lifecycle = source_between(
            HLS_PLAYER,
            "fn install_hls_prefetch_lifecycle(",
            "pub(crate) fn release_hls_view(",
        );
        let explicit_pause =
            source_between(lifecycle, "let explicit_pause_callback", "let play_player");
        let detached_pause =
            source_between(lifecycle, "let pause_callback", "// Ignore generic seeking");
        assert!(explicit_pause.contains("set_hls_prefetch_mode(HlsPrefetchMode::Inactive)"));
        assert!(detached_pause.contains("set_hls_prefetch_mode(HlsPrefetchMode::Inactive)"));
    }

    #[test]
    fn transient_hls_failures_retry_without_a_four_x_dead_end() {
        let policy = source_between(
            HLS_PLAYER,
            "fn swarm_load_policy() -> Object {",
            "fn set_property(",
        );
        assert!(policy.contains(r#""retryDelayMs", JsValue::from_f64(500.0)"#));

        let recovery = source_between(HLS_PLAYER, "fn handle_error(", "fn hard_recovery(");
        assert!(recovery.contains("(1_000_u64.saturating_mul("));

        let response = source_between(
            HLS_PLAYER,
            "async fn fetch_hls_bytes_response(",
            "fn hls_bytes_headers(",
        );
        assert_eq!(
            response
                .matches(r#"FetchResponse::error(503, "weeb-3 did not retrieve resource")"#)
                .count(),
            2
        );
        assert!(
            response
                .contains(r#"FetchResponse::error(503, "weeb-3 did not resolve HLS media size")"#)
        );
        let feed = source_between(
            HLS_PLAYER,
            "async fn fetch_feed_response(",
            "async fn load_feed_snapshot(",
        );
        assert!(
            feed.contains(r#"FetchResponse::error(503, "weeb-3 did not retrieve feed update")"#)
        );
        assert!(
            feed.contains(r#"FetchResponse::error(502, "feed update is not an HLS manifest")"#)
        );
        assert!(!feed.contains("serde_json"));
        assert!(!feed.contains("application/octet-stream"));
    }

    #[test]
    fn live_start_waits_for_a_continuous_frontier() {
        let load = source_between(
            HLS_PLAYER,
            "async fn load_feed_snapshot(",
            "async fn await_terminal_feed_confirmation_view(",
        );
        assert!(load.contains("None if !sequence_zero_start_requested"));
        assert!(load.contains("let live_history_presentation_id ="));
        assert!(load.contains("start == HlsStart::Live && history_active"));
        assert!(load.contains("sequence_zero_presentation_id.or(live_history_presentation_id)"));
        assert!(load.contains("if start == HlsStart::Live && !defer_live_followup"));
        assert!(load.contains("get_connections().await"));
        assert!(load.contains("HLS_LIVE_FRONTIER_MIN_PRICED_PEERS"));
        assert!(load.contains("HLS_LIVE_FRONTIER_CONNECTION_WAIT"));
        assert!(
            HLS_PLAYER
                .contains("const HLS_LIVE_FRONTIER_MAX_WAIT: Duration = Duration::from_secs(15);")
        );
        assert!(load.contains("latest_hls_feed_payload_startup("));
        assert!(load.contains("latest_hls_feed_payload_startup_observing_deferred("));
        assert!(load.contains("startup_deferred_out.as_ref()"));
        let canonical_startup = source_between(
            load,
            "let (early_payloads, early_payload_max_index) =",
            "let _ = canonical_out.try_send(loaded);",
        );
        assert!(canonical_startup.contains("if sequence_zero_start_requested"));
        assert!(canonical_startup.contains("Some(early_payload_out)"));
        assert!(canonical_startup.contains("Some(HLS_EARLY_FEED_PREFIX_INDEX)"));
        assert!(canonical_startup.contains("(None, None)"));
        assert!(canonical_startup.contains("early_payloads,\n"));
        assert!(canonical_startup.contains("early_payload_max_index,"));
        assert!(load.contains("discover_sequence_zero_startup_prefix("));
        assert!(load.contains("prefix_admissions_open.clone()"));
        assert!(!load.contains("HLS_EARLY_FEED_PREFIX_TARGET_INDEX"));
        assert!(!load.contains("reliable_indices"));
        assert!(load.contains("Some(HLS_EARLY_FEED_PREFIX_INDEX)"));
        assert!(HLS_PLAYER.contains("const HLS_EARLY_FEED_PREFIX_INDEX: u64 = 7;"));
        assert!(HLS_PLAYER.contains("const HLS_SEQUENCE_ZERO_DISCOVERY_MAX_IN_FLIGHT: usize = 3;"));
        let prefix_fanout = source_between(
            HLS_PLAYER,
            "async fn fan_out_authenticated_hls_prefixes(",
            "fn hls_prefix_stamp_for_feed(",
        );
        let prefix_memory = source_between(
            HLS_PLAYER,
            "fn remember_authenticated_hls_startup_prefix(",
            "fn deferred_sequence_zero_presentation(",
        );
        assert!(prefix_memory.contains("HlsTimeline::parse(&candidate.bytes)"));
        assert!(prefix_memory.contains("timeline.sequence != 0"));
        assert!(
            prefix_memory
                .contains("timeline.segments.len() < HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS")
        );
        assert!(prefix_fanout.contains("if accepted_prefix"));
        assert!(prefix_fanout.contains("offer_sequence_zero_startup_prefix("));
        assert!(load.contains("sequence_zero_startup_prefix_is_preferred("));
        assert!(load.matches("HLS_STARTUP_PREFIX_RESULT_GRACE").count() >= 2);
        let canonical_first = source_between(
            load,
            "Either::Right((Ok(Some(canonical)), _)) =>",
            "Either::Right((Ok(None) | Err(_), _)) =>",
        );
        assert!(canonical_first.contains("let canonical_starts_late ="));
        assert!(canonical_first.contains("if preferred.is_none()"));
        assert!(canonical_first.contains("&& canonical_starts_late"));
        assert_eq!(canonical_first.matches("prefix_ready_in.recv()").count(), 2);
        assert!(canonical_first.contains("if canonical_starts_late {"));
        assert!(canonical_first.contains("return None;"));
        let invalid_canonical = source_between(
            canonical_first,
            "if !hls_sequence_zero_canonical_is_supported(&canonical.bytes)",
            "let canonical_starts_late =",
        );
        assert!(invalid_canonical.contains("HLS_STARTUP_PREFIX_RESULT_GRACE"));
        assert!(invalid_canonical.contains("prefix_ready_in.recv()"));
        assert!(invalid_canonical.contains("best_prefix.borrow().clone()"));
        assert!(invalid_canonical.contains("InitialCanonicalFeedResolution::Unavailable"));
        assert!(
            canonical_startup.contains("hls_sequence_zero_canonical_is_supported(&payload.bytes)")
        );
        assert!(canonical_startup.contains("hls_media_sequence(&payload.bytes) == Some(0)"));
        assert!(canonical_startup.contains("canonical_prefix_admissions_open.set(false)"));
        assert!(load.contains("Some(index) if !sequence_zero_start_requested"));
        assert!(load.contains("load_persisted_vod_payload("));
        assert!(load.contains("index_hint.is_none() && !provisional_hls"));
        assert!(load.contains("provisional_hls.then(|| canonical_loaded.clone())"));
        assert!(!load.contains("startup_head_verified"));
        assert!(load.contains("let await_live_frontier ="));
        assert!(load.contains("start == HlsStart::Live && index_hint.is_none()"));
        assert!(load.contains("await_live_frontier_snapshot("));
        assert!(load.contains("live_frontier_deadline_ms"));
        assert_eq!(load.matches("prefetch_live_snapshot_start(").count(), 2);
        let selected_live_window = source_between(
            load,
            "let snapshot = match await_live_frontier",
            "Some(snapshot)",
        );
        assert_eq!(
            selected_live_window
                .matches("prefetch_live_snapshot_start(")
                .count(),
            1
        );
        assert!(load.contains("if !feed_task_running"));
        let live_prefetch = source_between(
            HLS_PLAYER,
            "fn prefetch_live_snapshot_start(",
            "async fn await_live_frontier_snapshot(",
        );
        let live_body_scheduler = source_between(
            HLS_PLAYER,
            "fn start_hls_live_body_runway_schedule(",
            "fn prefetch_live_snapshot_start(",
        );
        assert!(live_prefetch.contains("hls_live_tail(&snapshot.body)"));
        assert!(live_prefetch.contains("session.live_start && session.stamp() == stamp"));
        assert!(live_prefetch.contains("session.mode == HlsPrefetchMode::Sustained"));
        assert_eq!(
            live_prefetch
                .matches("start_hls_shared_prefix_warmup(")
                .count(),
            1
        );
        assert!(live_prefetch.contains(
            "hls_live_prefetch_references(&snapshot.body, foreground_reference.as_deref())"
        ));
        assert!(live_prefetch.contains("session.live_startup_body_runway.clone()"));
        assert!(live_prefetch.contains("locked_body_runway.unwrap_or_else"));
        assert!(live_prefetch.contains("start_hls_live_body_runway_schedule("));
        assert!(live_body_scheduler.contains("let body_runway_ready = !body_runway.is_empty()"));
        assert!(live_body_scheduler.contains("claim_live_body_schedule("));
        assert!(live_body_scheduler.contains("let Some(schedule_id) = schedule_id"));
        assert!(live_body_scheduler.contains("spawn_local(async move"));
        assert!(live_body_scheduler.contains("let mut stagger_next_leader = false"));
        assert!(live_body_scheduler.contains("HLS_NEXT_RESERVE_STAGGER"));
        assert!(live_body_scheduler.contains("live_body_schedule_current("));
        assert!(live_body_scheduler.contains("finish_live_body_schedule(stamp, schedule_id)"));
        assert!(live_body_scheduler.contains("hls_payload_body_cached(reference)"));
        assert!(live_body_scheduler.contains("'runway: loop"));
        assert!(
            live_body_scheduler
                .matches("hls_payload_body_ready_or_joinable(&reference, stamp.generation)")
                .count()
                >= 2,
            "a failed pending runway body must become eligible for admission again"
        );
        assert!(!live_prefetch.contains("session.live_body_runway.contains(&reference)"));
        assert!(live_body_scheduler.contains("start_hls_payload_load("));
        assert!(live_body_scheduler.contains("weeb3.clone()"));
        assert!(live_body_scheduler.contains("matches!(&role, HlsPayloadLoadRole::Lead(_, _))"));
        assert!(live_body_scheduler.contains("HlsPayloadLoadRole::AtCapacity =>"));
        assert!(live_body_scheduler.contains("Duration::from_millis(25)"));
        assert!(live_body_scheduler.contains("HlsPayloadLoadRole::Reject(_) => break 'runway"));
        assert!(live_prefetch.contains(".max_by_key(|(_, track)| track.last_touch)"));
        assert!(live_prefetch.contains("runway.current().to_string()"));
        let tail_prefetch = live_prefetch.find("let body_runway =").unwrap();
        let codec_prefetch = live_prefetch
            .find("hls_media_sequence(&snapshot.body) == Some(0)")
            .unwrap();
        assert!(tail_prefetch < codec_prefetch);
        assert!(live_prefetch[codec_prefetch..].contains("reference.clone(), stamp, 2"));
        let live_frontier = source_between(
            HLS_PLAYER,
            "async fn await_live_frontier_snapshot(",
            "fn start_beginning_snapshot_runway(",
        );
        assert!(live_frontier.contains("state.confirmed_head_index"));
        assert!(live_frontier.contains("hls_live_frontier_is_ready("));
        assert!(!live_frontier.contains("frontier_refinement"));
        assert!(!live_frontier.contains("initial_check"));
        assert!(live_frontier.contains("missing_is_terminal || now >= deadline_ms"));
        assert!(!HLS_PLAYER.contains("checking_token != token && snapshot_index > initial_index"));
        assert!(live_frontier.contains("now >= deadline_ms"));
        assert!(!live_frontier.contains("provisional_index"));
        assert!(load.contains("refresh_head && wait_for_live_frontier"));
        assert!(
            load.contains("let initial_check = if defer_live_followup && start == HlsStart::Live")
        );
        assert!(load.contains("Some(()) if initial_check.is_some()"));
        let stabilization = source_between(
            HLS_PLAYER,
            "async fn stabilize_initial_unindexed_hls_payload",
            "async fn stabilize_claimed_feed_route(",
        );
        assert_eq!(
            stabilization
                .matches("acquire_latest_raw_feed_payload_bounded_from(")
                .count(),
            2
        );
        assert!(stabilization.contains("else if !observe_progress"));
        assert!(stabilization.contains("FEED_FRONTIER_LOOKAHEAD_TIMEOUT"));
        assert!(stabilization.contains("active_profile().swarm_network_id != network_id"));
        assert!(!stabilization.contains("acquire_raw_feed_payload_at_index_bounded("));
        let initial_stabilization = source_between(
            HLS_PLAYER,
            "fn schedule_initial_feed_stabilization(",
            "fn publish_sequence_zero_startup_snapshot(",
        );
        assert!(initial_stabilization.contains(") -> Option<u64>"));
        assert!(initial_stabilization.contains("return None"));
        assert!(initial_stabilization.contains("Some(checking_token)"));
        let confirmation = source_between(
            HLS_PLAYER,
            "async fn confirm_terminal_feed_head(",
            "async fn stabilize_initial_unindexed_hls_payload",
        );
        assert!(confirmation.contains("if !terminal"));
        assert!(confirmation.contains("acquire_latest_raw_feed_payload_from("));
        assert!(
            confirmation.find("if !terminal").unwrap()
                < confirmation
                    .find("acquire_latest_raw_feed_payload_from(")
                    .unwrap()
        );
        let refresh = source_between(
            HLS_PLAYER,
            "async fn refresh_live_feed_head(",
            "fn schedule_feed_followup(",
        );
        let reducer = source_between(
            HLS_PLAYER,
            "fn apply_feed_candidate(",
            "fn store_feed_snapshot(",
        );
        assert!(refresh.contains("acquire_latest_raw_feed_payload_bounded_from("));
        assert!(refresh.contains("await_feed_probe_wave_credit("));
        assert!(refresh.contains("mode: FeedFollowupMode::Canonical"));
        assert!(!refresh.contains("FeedFollowupMode::SequenceZeroPresentation"));
        assert!(refresh.contains("apply_feed_candidate("));
        assert!(reducer.contains("(!seeded && !is_hls_manifest(&candidate.source))"));
        assert!(reducer.contains("if candidate.head_confirmed"));
        assert!(reducer.contains("let proof_confirmed = if seeded"));
        assert!(reducer.contains("existing.last_head_check = js_sys::Date::now()"));
        assert!(refresh.contains("state.last_head_check > 0.0"));
        assert!(refresh.contains("return Some((latest_index, head_confirmed))"));
        assert!(refresh.contains("prefetch_live_snapshot_start(&weeb3, owner, topic, &snapshot)"));
        assert!(
            refresh
                .find("acquire_latest_raw_feed_payload_bounded_from(")
                .unwrap()
                < refresh.find("apply_feed_candidate(").unwrap()
        );
        assert!(reducer.contains("last_head_check: if candidate.head_confirmed"));
        assert!(reducer.contains("existing.last_head_check = 0.0"));
        let follower = source_between(
            HLS_PLAYER,
            "fn schedule_feed_followup(",
            "pub(crate) async fn try_fetch_response(",
        );
        assert!(follower.contains("let mut skipped_missing_index = false;"));
        let sequence_zero_branch = follower
            .find("if followup_mode == FeedFollowupMode::SequenceZeroPresentation")
            .unwrap();
        let retained_catchup = follower.find("catch_up_sequence_zero_followup(").unwrap();
        let legacy_refresh = follower.find("if refresh_head").unwrap();
        assert!(sequence_zero_branch < retained_catchup && retained_catchup < legacy_refresh);
        assert!(follower[retained_catchup..legacy_refresh].contains("return;"));
        let continuation = &follower[sequence_zero_branch..legacy_refresh];
        let resume = continuation.find("let resume =").unwrap();
        let release = continuation
            .find("let released = release_feed_route_check(")
            .unwrap();
        let released = continuation
            .find("if resume && released.is_some()")
            .unwrap();
        let reschedule = continuation[released..]
            .find("schedule_feed_followup_task(")
            .map(|position| position + released)
            .unwrap();
        assert!(resume < release && release < released && released < reschedule);
        assert!(continuation[reschedule..].contains("true,"));
        assert!(follower.contains(".take(FEED_FOLLOWUP_BATCH_LIMIT)"));
        assert!(follower.contains(".buffered(1)"));
        assert!(!follower.contains("consecutive_sequence_zero_missing"));
        assert!(follower.contains("sequence_zero_followup_is_current("));
        assert!(!follower.contains("head_refresh_unresolved"));
        assert!(follower.contains("current_index = refreshed_index"));
        assert!(follower.contains("if !refresh_head"));
        assert!(follower.contains("recovered_missing_index |= skipped_missing_index;"));
        assert!(
            follower.find("if accepted.is_none()").unwrap()
                < follower.find("prefetch_live_snapshot_start(").unwrap()
        );
        assert!(follower.contains("drop(exact_followups);"));
        assert!(
            follower
                .matches("active_profile().swarm_network_id != network_id")
                .count()
                >= 2
        );
        let beginning_wait = source_between(
            HLS_PLAYER,
            "fn start_beginning_snapshot_runway(",
            "async fn load_feed_snapshot(",
        );
        assert!(beginning_wait.contains("hls_segment_identities(&snapshot.body)"));
        assert!(beginning_wait.contains("segments.retain(|segment| !segment.gap)"));
        assert!(beginning_wait.contains("configure_hls_beginning_prefix_manifest("));
        assert!(beginning_wait.contains("start_hls_payload_size_probe("));
        assert!(beginning_wait.contains("ensure_hls_beginning_prefix_configured("));
        assert!(beginning_wait.contains("hls_progressive_startup_admission_is_current("));
        assert!(!beginning_wait.contains("retrieve_hls_payload_range("));
        assert!(!beginning_wait.contains("FuturesUnordered"));

        let critical_supply = source_between(
            HLS_PLAYER,
            "fn ensure_hls_beginning_prefix_configured(",
            "async fn await_hls_beginning_prefix_barrier(",
        );
        assert!(critical_supply.contains("let payload_ranges = (1..target_windows)"));
        assert!(critical_supply.contains("scout_data_ranges_cache_only_cancellable("));
        assert!(critical_supply.contains("if !foreground"));
        assert!(critical_supply.contains("start == 0 && end == expected_end"));
        assert!(critical_supply.contains("gate.foreground_zero_settled = true"));
        assert!(critical_supply.contains("let ready = (0..gate.target_windows).all"));
        assert!(!critical_supply.contains("hls_beginning_adjacent_range"));
        assert!(!critical_supply.contains("try_hls_background_range_lease("));
        assert!(!critical_supply.contains("HlsBackgroundRangeRequest"));
        let raw_scout = source_between(
            critical_supply,
            "fn start_hls_beginning_raw_scout(",
            "fn hls_beginning_raw_seed(",
        );
        assert!(
            raw_scout
                .find("hls_beginning_raw_supply_admission_for(")
                .unwrap()
                < raw_scout
                    .find("let payload_ranges = (1..target_windows)")
                    .unwrap()
        );
        assert!(!critical_supply.contains("FuturesUnordered"));

        assert!(initial_stabilization.contains("await_hls_beginning_prefix_barrier("));
        assert!(follower.contains("await_hls_beginning_prefix_barrier("));
        let prefix_barrier = source_between(
            HLS_PLAYER,
            "async fn await_hls_beginning_prefix_barrier(",
            "pub(super) fn remember_hls_fragment_kind(",
        );
        assert!(prefix_barrier.contains("hls_beginning_prefix_barrier_admission("));
        assert!(prefix_barrier.contains("session.live_start"));
        assert!(
            prefix_barrier
                .find("session.stamp() != expected_stamp")
                .unwrap()
                < prefix_barrier
                    .find("hls_beginning_prefix_barrier_admission(")
                    .unwrap(),
            "the Live bypass must not keep work from a stale playback timeline",
        );

        let feed_response = source_between(
            HLS_PLAYER,
            "async fn fetch_feed_response(",
            "async fn load_persisted_vod_payload(",
        );
        assert!(feed_response.contains("snapshot.index.checked_sub(1)"));
        assert!(feed_response.contains("Some(bootstrap_index)"));
        assert!(feed_response.contains("hls_media_sequence(&snapshot.body) == Some(0)"));
        assert!(
            feed_response
                .contains("rewrite_hls_sequence_zero_codec_bootstrap(&snapshot.body, true)")
        );
        assert!(feed_response.contains("continue_hls_codec_bootstrap(&snapshot.body)"));
        assert!(feed_response.contains("let sequence_zero_bootstrap = matches!("));
        assert!(feed_response.contains("hls_media_references(&body).into_iter().next()"));
        assert!(feed_response.contains("hls_prefix_stamp_for_feed(&weeb3, &owner, &topic)"));
        assert!(
            feed_response
                .contains("start_hls_shared_prefix_warmup(weeb3.clone(), reference, stamp, 2)")
        );
        assert!(!feed_response.contains("start_hls_payload_load("));
        assert!(feed_response.contains("[HLS_EARLY_FEED_PREFIX_INDEX, 0]"));

        let early_decode = source_between(
            BZZ_STREAM,
            "pub(crate) async fn acquire_latest_raw_feed_payload_startup_observing_deferred(",
            "pub(crate) async fn acquire_latest_raw_feed_payload_bounded_from",
        );
        assert!(early_decode.contains("seek_latest_feed_update_indexed_wide_bounded("));
        assert!(!early_decode.contains("overscan_sequence_feed_candidate("));
        assert!(BZZ_STREAM.contains("maximum_index.is_some_and(|maximum| index > maximum)"));
        assert!(early_decode.contains("index.checked_sub(1)"));
        assert!(!BZZ_STREAM.contains("decode_observed_frontier_updates"));
        let observed_decode = source_between(
            BZZ_STREAM,
            "fn decode_observed_feed_updates(",
            "pub(crate) async fn acquire_raw_feed_payload_at_index(",
        );
        assert!(observed_decode.contains("buffer_unordered(OBSERVED_FEED_PAYLOAD_DECODES)"));
        assert!(observed_decode.contains("span > CHUNK_SIZE as u64"));
        let observed_frontier = source_between(
            BZZ_STREAM,
            "pub(crate) async fn acquire_latest_raw_feed_payload_bounded_from",
            "pub(crate) async fn acquire_latest_raw_feed_payload_from(",
        );
        assert!(observed_frontier.contains("observed_updates.try_send((index, update.clone()))"));
        assert!(stabilization.contains("select(search, Box::pin(observed_in.recv()))"));
        assert!(stabilization.contains("await_feed_probe_wave_credit"));
        assert!(stabilization.contains("Some(observed_out)"));
        let sequence_zero_startup = source_between(
            HLS_PLAYER,
            "fn publish_sequence_zero_startup_snapshot(",
            "enum LiveHistoryProbeState",
        );
        assert_eq!(
            sequence_zero_startup
                .matches("schedule_feed_followup(")
                .count(),
            2
        );
        assert!(
            sequence_zero_startup
                .contains("async_std::task::sleep(HLS_SEQUENCE_ZERO_CANONICAL_EXCLUSIVITY).await")
        );
        assert!(
            sequence_zero_startup
                .find("claim_feed_route_check(")
                .unwrap()
                < sequence_zero_startup.find("spawn_local(").unwrap()
        );
        assert!(sequence_zero_startup.contains("stabilize_claimed_feed_route("));
        assert!(sequence_zero_startup.contains(
            "initial,\n                    false,\n                    false,\n                    FeedFollowupMode::SequenceZeroPresentation"
        ));
        let claimed_stabilization = source_between(
            HLS_PLAYER,
            "async fn stabilize_claimed_feed_route(",
            "fn schedule_initial_feed_stabilization(",
        );
        assert!(claimed_stabilization.contains("} else if resume_exact_followup {"));
        let unavailable = &sequence_zero_startup[sequence_zero_startup
            .find("InitialCanonicalFeedResolution::Unavailable")
            .unwrap()..];
        assert!(
            unavailable.find("release_feed_route_check(").unwrap()
                < unavailable.find("schedule_feed_followup(").unwrap(),
            "the unavailable fallback must release canonical exclusivity before following"
        );
        let live_history = source_between(
            HLS_PLAYER,
            "struct LiveHistoryCollector",
            "fn feed_cache_key(",
        );
        assert!(live_history.contains("FuturesUnordered<LiveHistoryProbeFuture>"));
        assert!(live_history.contains("collector.capacity_parallelism.max(1)"));
        assert!(live_history.contains(
            "self.capacity_parallelism == 0\n                    && self.in_flight.is_empty()\n                    && self.direct_in_flight.is_none()\n                    && !self.targets_observed(targets)"
        ));
        assert!(live_history.contains("wave.iter().rev().copied()"));
        assert!(live_history.contains("collector.observe_forward_wave(&wave, head_index)"));
        assert!(live_history.contains("plan_hls_sparse_terminal_repairs(head_index)"));
        assert!(live_history.contains("initial_is_confirmed_terminal"));
        assert!(live_history.contains("hls_sequence_zero_sparse_tail(&initial.bytes)"));
        assert!(live_history.contains("hls_complete_history_timeline(&candidate.bytes)"));
        assert!(
            live_history
                .contains("hls_sequence_zero_timeline_covers_head(window, candidate_timeline)")
        );
        assert!(live_history.contains("session.live_history_active = true"));
        assert!(!HLS_PLAYER.contains("start_live_history_accumulator"));

        let attach = source_between(
            HLS_PLAYER,
            "pub(crate) async fn attach_hls_feed_player(",
            "async fn open_hls_feed_view_generation(",
        );
        assert!(attach.contains("let presentation_id = view_generation;"));
        assert!(attach.contains("HlsStart::Beginning => {}"));
        assert!(attach.contains("HlsStart::Live =>"));
        assert!(attach.contains("!state.snapshot.finalized"));
        assert!(attach.contains("state.last_head_check"));
        assert!(attach.contains("source.push_str(\"?start=live\")"));
        assert!(!attach.contains("\n                0\n"));
        assert!(
            attach.find("load_feed_snapshot(").unwrap() < attach.find("play_hls(").unwrap(),
            "feed discovery must overlap the already-started hls.js import"
        );
        let beginning_runway = attach.find("start_beginning_snapshot_runway(").unwrap();
        let overlap = attach
            .find("match select(worker_ready, snapshot_load).await")
            .unwrap();
        let play = attach.find("play_hls(").unwrap();
        let prepare = attach.find("prepare_live_history(").unwrap();
        let live_edge_warmup = attach.find("prefetch_live_snapshot_start(").unwrap();
        assert!(beginning_runway < overlap && overlap < play);
        assert!(live_edge_warmup < prepare && prepare < overlap && overlap < play);
        assert_eq!(attach.matches("prefetch_live_snapshot_start(").count(), 1);
        assert!(attach.contains("if start == HlsStart::Live"));
        assert!(attach.contains("mpsc::bounded::<DeferredRawFeedPayload>(1)"));
        assert!(attach.contains("(None, None)"));
        assert!(attach.contains("startup_deferred_in.and_then"));
        assert!(attach.contains("start == HlsStart::Live && snapshot.is_none()"));
        assert!(load.matches("await_live_frontier_snapshot(").count() >= 2);
        assert!(!attach.contains("await_live_frontier_snapshot("));
        assert!(!attach.contains("start_live_history_accumulator("));
        let player_start = source_between(HLS_PLAYER, "async fn launch(", "fn session_from(");
        assert!(
            player_start
                .find("get_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)")
                .unwrap()
                < player_start
                    .find("remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)")
                    .unwrap(),
            "an early Play click must survive feed warmup and player attachment"
        );

        let fetch = source_between(
            HLS_PLAYER,
            "pub(crate) async fn try_fetch_response(",
            "fn canonical_hls_bytes_resource(",
        );
        assert!(fetch.contains("None => HlsStart::Beginning"));
        assert!(fetch.contains(r#"Some("live") => HlsStart::Live"#));
        assert!(fetch.contains("invalid HLS start"));
    }

    #[test]
    fn adaptive_beginning_discovery_preserves_accounting_and_closes_new_admissions_on_a_winner() {
        let shared_gate = source_between(
            HLS_PLAYER,
            "fn sequence_zero_startup_admissions_are_open(",
            "fn offer_sequence_zero_startup_prefix(",
        );
        assert!(shared_gate.contains("admissions_open.get()"));
        assert!(shared_gate.contains("!ready.is_closed()"));
        let offer = source_between(
            HLS_PLAYER,
            "fn offer_sequence_zero_startup_prefix(",
            "async fn probe_sequence_zero_startup_prefix(",
        );
        let publish = offer.find("ready.try_send(prefix)").unwrap();
        let close = offer.find("admissions_open.set(false)").unwrap();
        assert!(publish < close);
        assert!(offer.contains("sequence_zero_startup_admissions_are_open("));

        let probe = source_between(
            HLS_PLAYER,
            "async fn probe_sequence_zero_startup_prefix(",
            "fn start_sequence_zero_discovery_probe(",
        );
        assert!(probe.contains("hls_feed_payload_at_index_followup_retained_status("));
        assert!(probe.contains("payload.index != index"));
        assert!(probe.contains("deferred.index != index"));
        let deferred = source_between(
            probe,
            "RetainedRawFeedPayloadProbe::Deferred(deferred) =>",
            "RetainedRawFeedPayloadProbe::Missing",
        );
        let current = deferred
            .find("sequence_zero_discovery_is_current(")
            .unwrap();
        let second_open = deferred[current..]
            .find("sequence_zero_startup_admissions_are_open(")
            .unwrap()
            + current;
        let range = deferred
            .find("hls_deferred_feed_payload_prefix(&deferred)")
            .unwrap();
        assert!(current < second_open && second_open < range);

        let worker = source_between(
            HLS_PLAYER,
            "fn start_sequence_zero_discovery_probe(",
            "async fn admit_sequence_zero_discovery_probe(",
        );
        assert!(worker.contains("spawn_local(async move"));
        assert!(worker.contains("probe_sequence_zero_startup_prefix("));
        assert!(worker.contains(".await;"));
        assert!(worker.contains("completed.try_send((index, result))"));
        assert!(!worker.contains("abort("));
        assert!(!worker.contains("cancel("));

        let admission = source_between(
            HLS_PLAYER,
            "async fn admit_sequence_zero_discovery_probe(",
            "async fn discover_sequence_zero_startup_prefix(",
        );
        let first_open = admission
            .find("sequence_zero_startup_admissions_are_open(")
            .unwrap();
        let current = admission
            .find("sequence_zero_discovery_is_current(")
            .unwrap();
        let second_open = admission[current..]
            .find("sequence_zero_startup_admissions_are_open(")
            .unwrap()
            + current;
        let dispatch = admission
            .find("start_sequence_zero_discovery_probe(")
            .unwrap();
        assert!(first_open < current && current < second_open && second_open < dispatch);

        let manager = source_between(
            HLS_PLAYER,
            "async fn discover_sequence_zero_startup_prefix(",
            "fn sequence_zero_startup_prefix_is_preferred(",
        );
        assert!(manager.contains("for _ in 0..HLS_SEQUENCE_ZERO_DISCOVERY_MAX_IN_FLIGHT"));
        assert!(manager.contains("pending.len() < HLS_SEQUENCE_ZERO_DISCOVERY_MAX_IN_FLIGHT"));
        assert!(manager.contains("HLS_SEQUENCE_ZERO_DISCOVERY_HEDGE"));
        assert!(manager.contains("planner.observe(index, observation)"));
        assert!(manager.contains("offer_sequence_zero_startup_prefix("));
        assert!(manager.contains("get_connections().await == 0"));
        let post_connections = manager.find("// `get_connections()` yields").unwrap();
        let first_dispatch = manager
            .find("for _ in 0..HLS_SEQUENCE_ZERO_DISCOVERY_MAX_IN_FLIGHT")
            .unwrap();
        assert!(
            manager[post_connections..first_dispatch]
                .contains("sequence_zero_discovery_is_current(")
        );
    }

    #[test]
    fn deferred_beginning_prefix_cannot_enter_complete_feed_state_or_normal_followup() {
        let store = source_between(
            HLS_PLAYER,
            "fn store_deferred_sequence_zero_snapshot(",
            "fn active_live_history_feed_cache_key()",
        );
        assert!(store.contains("source_body: Arc::from(Vec::<u8>::new())"));
        assert!(store.contains("deferred_source: Some(presentation.source)"));
        assert!(!store.contains("RawFeedPayload"));

        let reducer = source_between(
            HLS_PLAYER,
            "fn apply_feed_candidate(",
            "fn store_feed_snapshot(",
        );
        let deferred_admission = reducer.find("let completing_deferred_same_index").unwrap();
        let equality_exception = reducer
            .find("candidate.index == index && !completing_deferred_same_index")
            .unwrap();
        let proof = reducer
            .find("hls_deferred_feed_completion_matches(")
            .unwrap();
        let clear = reducer.find("existing.deferred_source = None;").unwrap();
        let ordinary_same_index = reducer[clear + 1..]
            .find("if existing.snapshot.index == candidate.index {")
            .map(|position| position + clear + 1)
            .unwrap();
        assert!(deferred_admission < equality_exception && equality_exception < proof);
        assert!(proof < clear && clear < ordinary_same_index);
        assert!(reducer.contains("existing.checking_token != token"));
        assert!(reducer.contains("deferred.payload_span"));
        assert!(reducer.contains("&deferred.authenticated_prefix"));

        let completion = source_between(
            HLS_PLAYER,
            "async fn complete_deferred_feed_route(",
            "#[derive(Clone)]\n    struct SequenceZeroFollowupSeed",
        );
        assert!(completion.contains("acquire_deferred_raw_feed_payload_conservative("));
        assert!(completion.contains("payload.index == source_index"));
        assert!(completion.contains("task.publish(&payload, false)"));
        assert!(completion.contains("CONSERVATIVE_DEFERRED_MAX_PHYSICAL_ATTEMPTS"));
        assert!(!completion.contains("timeout("));

        let publication = source_between(
            HLS_PLAYER,
            "fn publish_sequence_zero_startup_snapshot(",
            "#[derive(Clone, Copy, Eq, PartialEq)]",
        );
        let unavailable = &publication[publication
            .find("InitialCanonicalFeedResolution::Unavailable")
            .unwrap()..];
        assert!(unavailable.contains("complete_deferred_feed_route("));
        assert!(unavailable.contains("discard_deferred_feed_route("));
        let completion = unavailable.find("complete_deferred_feed_route(").unwrap();
        let normal_release = unavailable[completion..]
            .find("release_feed_route_check(")
            .map(|position| position + completion)
            .unwrap();
        assert!(completion < normal_release);
        let exclusivity = source_between(
            publication,
            "async_std::task::sleep(HLS_SEQUENCE_ZERO_CANONICAL_EXCLUSIVITY).await",
            "let initial = match canonical",
        );
        assert!(exclusivity.contains("if deferred_prefix"));
        assert!(exclusivity.contains("return;"));

        let seed = source_between(
            HLS_PLAYER,
            "fn sequence_zero_followup_seed(",
            "fn initialize_sequence_zero_followup_scan(",
        );
        let refresh = source_between(
            HLS_PLAYER,
            "async fn refresh_live_feed_head(",
            "fn sequence_zero_followup_is_current(",
        );
        let scheduler = source_between(
            HLS_PLAYER,
            "fn schedule_feed_followup_task(",
            "pub(crate) async fn try_fetch_response(",
        );
        assert!(seed.contains("state.deferred_source.is_some()"));
        assert!(refresh.contains("state.deferred_source.is_some()"));
        assert!(scheduler.contains("discard_deferred_feed_route(&cache_key, checking_token)"));
        assert!(scheduler.contains("state.deferred_source.is_none()"));
    }

    #[test]
    fn live_sparse_preparation_is_edge_first_bounded_and_beginning_isolated() {
        let discover = source_between(
            HLS_PLAYER,
            "async fn discover_live_history_head(",
            "async fn assemble_live_history(",
        );
        let guard = discover
            .find("let guard = [first_missing, second_missing]")
            .unwrap();
        let wave = discover.find("plan_hls_sparse_forward_wave(").unwrap();
        assert!(
            guard < wave,
            "a true edge must not dispatch a 64-probe wave"
        );
        assert!(discover.contains("wave.iter().rev().copied()"));
        assert!(discover.contains("plan_hls_sparse_terminal_repairs(head_index)"));
        assert!(discover.contains("candidate.index > head_index"));
        assert!(discover.contains("highest_authenticated_positive_index"));
        assert!(!discover.contains("timeout("));

        let collector = source_between(
            HLS_PLAYER,
            "struct LiveHistoryCollector",
            "fn live_history_session_is_current(",
        );
        assert!(collector.contains("hls_complete_history_timeline(&candidate.bytes)"));
        assert!(
            collector
                .contains("hls_sequence_zero_timeline_covers_head(window, candidate_timeline)")
        );
        assert!(collector.contains("candidate.index >= *index"));
        assert!(collector.contains("candidate.index >= self.highest_authenticated_positive_index"));
        assert!(collector.contains("candidate.index > floor"));
        assert!(collector.contains("direct.as_mut().now_or_never()"));
        assert!(collector.contains("direct_required"));
        assert!(collector.contains("deferred.retained_bytes()"));
        let accepted_probe = source_between(collector, "fn accept_result(", "async fn pump_once(");
        let deferred_probe = source_between(
            accepted_probe,
            "RetainedRawFeedPayloadProbe::Deferred(deferred) => {",
            "RetainedRawFeedPayloadProbe::Missing => {",
        );
        assert!(deferred_probe.contains("if self.is_sequence_zero_followup()"));
        assert!(deferred_probe.contains("self.park_deferred_followup_candidate(deferred)"));
        assert!(deferred_probe.contains("LiveHistoryProbeState::Unsupported"));
        assert_eq!(deferred_probe.matches("start_deferred_direct(").count(), 1);
        assert!(
            deferred_probe
                .find("LiveHistoryProbeState::Unsupported")
                .unwrap()
                < deferred_probe.find("start_deferred_direct(").unwrap(),
            "post-start catch-up must park a deferred body before the startup-only decode path"
        );
        let parked_deferred = source_between(
            collector,
            "fn park_deferred_followup_candidate(",
            "fn remember_followup_retry_index(",
        );
        assert!(parked_deferred.contains("self.followup_deferred_retry_index"));
        assert!(!parked_deferred.contains("self.followup_retry_indices"));
        assert!(parked_deferred.contains("self.forget_followup_retry_index(deferred.index)"));
        assert!(!parked_deferred.contains("self.remember_followup_retry_index"));
        assert!(parked_deferred.contains("self.followup_deferred_retry_index"));
        assert!(
            !parked_deferred.contains("deferred.index < self.highest_authenticated_positive_index")
        );
        assert!(
            !accepted_probe.contains(".is_some_and(|deferred_index| index > deferred_index)"),
            "a later positive must not erase an older authenticated bridge"
        );
        let nonterminal_direct = &collector[collector
            .find("Some(HlsDirectArchiveDisposition::Stale)")
            .unwrap()
            ..collector
                .find("Some(HlsDirectArchiveDisposition::SequenceZeroCheckpoint)")
                .unwrap()];
        assert!(nonterminal_direct.contains("self.drop_direct_work();"));
        assert!(!nonterminal_direct.contains("return Err("));
        let checkpoint_direct = &collector[collector
            .find("Some(HlsDirectArchiveDisposition::SequenceZeroCheckpoint)")
            .unwrap()
            ..collector.find("None =>").unwrap()];
        assert!(
            checkpoint_direct
                .contains("verified_sequence_zero_checkpoint_tail(index, &checkpoint.bytes)")
        );
        assert!(checkpoint_direct.contains("self.remember_window(RawFeedPayload"));
        assert!(checkpoint_direct.contains("LiveHistoryProbeState::Found"));
        let sparse_tail_branch = checkpoint_direct.find("if let Some(sparse_tail)").unwrap();
        assert!(
            !checkpoint_direct[sparse_tail_branch..].contains("if use_full_checkpoint {"),
            "a nonfinal checkpoint without a synthesizable sparse tail must stay unsupported"
        );
        assert!(
            collector
                .matches("verified_sequence_zero_checkpoint_tail(")
                .count()
                >= 3
        );
        let checkpoint_verifier = source_between(
            HLS_PLAYER,
            "pub(crate) fn hls_verified_sequence_zero_checkpoint_tail",
            "pub(crate) fn hls_is_long_sequence_zero_checkpoint",
        );
        let short_window = checkpoint_verifier
            .find("timeline.segments.len() <= HLS_SPARSE_HISTORY_STRIDE as usize")
            .unwrap();
        let cross_window = checkpoint_verifier
            .find("hls_sequence_zero_timeline_covers_head(window, &timeline)")
            .unwrap();
        assert!(
            short_window < cross_window,
            "normal short sequence-zero windows must bypass archive-wide checkpoint validation"
        );
        assert!(
            collector
                .find("hls_is_long_sequence_zero_checkpoint(&payload.bytes)")
                .unwrap()
                < collector
                    .find("self.verified_sequence_zero_checkpoint_tail(index, &payload.bytes)")
                    .unwrap(),
            "a late small checkpoint must be discarded before cross-window verification"
        );

        let sequence_zero_catchup = source_between(
            HLS_PLAYER,
            "async fn discover_sequence_zero_followup_head(",
            "fn commit_sequence_zero_followup(",
        );
        let live_wave = sequence_zero_catchup
            .find("plan_hls_sequence_zero_live_followup(initial_index)")
            .unwrap();
        let ordinary_return = sequence_zero_catchup.find("if !refresh_head {").unwrap();
        let sparse_guard = sequence_zero_catchup
            .find("checked_add(HLS_SPARSE_HISTORY_STRIDE)")
            .unwrap();
        assert!(live_wave < ordinary_return && ordinary_return < sparse_guard);
        assert!(sequence_zero_catchup.contains("live_frontier_found"));
        assert!(sequence_zero_catchup.contains("plan_hls_sequence_zero_followup_recovery("));
        assert!(sequence_zero_catchup.contains("highest_appendable_sequence_zero_followup_head"));
        assert!(
            sequence_zero_catchup
                .contains("initial_index >= collector.highest_authenticated_positive_index")
        );
        assert!(sequence_zero_catchup.contains("if scan_initialized && refresh_head"));
        assert!(
            sequence_zero_catchup
                .contains("retry_indices: collector.followup_retry_indices.clone()")
        );
        assert!(sequence_zero_catchup.contains("select_hls_sequence_zero_retry("));
        assert!(sequence_zero_catchup.contains("retry_deferred_first"));
        assert!(sequence_zero_catchup.contains("planned_retry_index == Some(planned_retry.index)"));
        assert!(sequence_zero_catchup.contains("planned_retry.authenticated"));
        assert!(sequence_zero_catchup.contains("LiveHistoryProbeState::Unavailable"));
        assert!(sequence_zero_catchup.contains("LiveHistoryProbeState::Unsupported"));
        assert!(sequence_zero_catchup.contains(".deferred_probe_indices"));
        assert!(sequence_zero_catchup.contains(".contains(&planned_retry_index)"));
        assert!(
            !sequence_zero_catchup
                .contains("forget_deferred_followup_retry_index(planned_retry_index)")
        );
        assert!(sequence_zero_catchup.contains("|| !remembered_later"));
        assert!(
            sequence_zero_catchup
                .find("collector.observe_retained_once(&proof_targets)")
                .unwrap()
                < sequence_zero_catchup
                    .find("decode_selected_sequence_zero_terminal(collector)")
                    .unwrap(),
            "the conservative terminal decoder must wait for every admitted proof target"
        );
        assert!(
            !sequence_zero_catchup.contains("LiveHistoryProbeClass::Repair"),
            "post-start catch-up must never expand a bounded retained proof into an exact repair wave"
        );
        let warm_scan = source_between(
            HLS_PLAYER,
            "fn warm_sequence_zero_followup_scan(",
            "fn take_sequence_zero_followup_direct(",
        );
        assert!(warm_scan.contains("state.sequence_zero_recovery_cursor = next_recovery_cursor"));
        assert!(warm_scan.contains("state.sequence_zero_retry_indices = retry_indices"));
        assert!(
            warm_scan.contains("state.sequence_zero_deferred_retry_index = deferred_retry_index")
        );
        assert!(warm_scan.contains(".max(positive_ceiling)"));
        assert!(warm_scan.contains("fn persist_sequence_zero_followup_observation("));
        assert!(
            warm_scan
                .contains("state.sequence_zero_retry_deferred_first = !seed.retry_deferred_first")
        );
        let terminal_finish = source_between(
            HLS_PLAYER,
            "fn finish_sequence_zero_terminal_confirmation(",
            "async fn confirm_tentative_sequence_zero_terminal(",
        );
        assert!(terminal_finish.contains("state.sequence_zero_positive_ceiling > seed.index"));
        assert!(terminal_finish.contains("retry.authenticated && retry.index > seed.index"));
        assert!(
            terminal_finish.contains(
                "let promote = promote && positive_indices.is_empty() && !prior_positive"
            )
        );
        assert!(terminal_finish.contains("state.snapshot.finalized = true"));
        assert!(terminal_finish.contains("state.source_endlist_confirmed = true"));
        let terminal_confirmation = source_between(
            HLS_PLAYER,
            "async fn confirm_tentative_sequence_zero_terminal(",
            "fn take_sequence_zero_followup_direct(",
        );
        let admission = terminal_confirmation
            .find("if !gate.admitted().await")
            .unwrap();
        let dispatch = terminal_confirmation.find("for index in targets").unwrap();
        let drain = terminal_confirmation
            .find("while let Some((index, result)) = probes.next().await")
            .unwrap();
        assert!(admission < dispatch && dispatch < drain);
        assert!(
            terminal_confirmation
                .contains("plan_hls_sequence_zero_terminal_confirmation(seed.index)")
        );
        assert!(
            terminal_confirmation.contains(
                "hls_feed_payload_at_index_followup_retained_status(owner, topic, index)"
            )
        );
        assert!(!terminal_confirmation.contains("acquire_deferred_raw_feed_payload"));
        assert!(!terminal_confirmation.contains("probe_deferred_raw_feed_payload"));
        assert!(!terminal_confirmation.contains("decode_selected_sequence_zero_terminal"));
        let conservative_terminal = source_between(
            HLS_PLAYER,
            "async fn decode_selected_sequence_zero_terminal(",
            "fn highest_appendable_sequence_zero_followup_head(",
        );
        assert!(conservative_terminal.contains("selected_deferred_followup_candidate()"));
        assert!(conservative_terminal.contains("gate.admitted().await"));
        assert!(
            conservative_terminal
                .matches("retry_deferred_followup_candidate(deferred_index)")
                .count()
                >= 3,
            "capacity, tail transport, and ownership interruptions must preserve an exact retry"
        );
        assert!(
            conservative_terminal.contains("probe_deferred_raw_feed_payload_tail_conservative(")
        );
        assert!(conservative_terminal.contains("hls_tail_has_terminal_endlist"));
        assert!(conservative_terminal.contains("take_selected_deferred_followup_candidate()"));
        let absent_tail = conservative_terminal
            .find("let Some(tail) = tail else")
            .unwrap();
        let nonterminal_tail = conservative_terminal
            .find("if !hls_tail_has_terminal_endlist(&tail)")
            .unwrap();
        assert!(
            conservative_terminal[absent_tail..nonterminal_tail]
                .contains("retry_deferred_followup_candidate(deferred_index)"),
            "a transport failure is retryable, not an authenticated nonterminal result"
        );
        let second_admission = conservative_terminal[nonterminal_tail..]
            .find("if !gate.admitted().await")
            .map(|position| position + nonterminal_tail)
            .unwrap();
        assert!(
            conservative_terminal[nonterminal_tail..second_admission]
                .contains("LiveHistoryProbeState::Unsupported"),
            "only a successfully authenticated tail without ENDLIST is permanently unsupported"
        );
        assert!(
            conservative_terminal.contains("start_deferred_direct(deferred, true, Some(gate))")
        );
        let conservative_gate = source_between(
            HLS_PLAYER,
            "struct SequenceZeroFollowupGate",
            "struct LiveHistoryCollector",
        );
        assert!(conservative_gate.contains("CONSERVATIVE_DEFERRED_MAX_PHYSICAL_ATTEMPTS"));
        assert!(conservative_gate.contains("HLS_FEED_WAVE_FOREGROUND_MARGIN_CHUNKS"));
        let deferred_decoder = source_between(
            collector,
            "fn start_deferred_direct(",
            "fn adopt_inline_direct(",
        );
        assert!(deferred_decoder.contains("acquire_deferred_raw_feed_payload_conservative("));
        assert!(deferred_decoder.contains("range_gate.admitted().await"));

        let retained_startup = source_between(
            HLS_PLAYER,
            "async fn hls_feed_payload_at_index_retained_status(",
            "async fn hls_feed_payload_at_index_followup_retained_status(",
        );
        assert!(retained_startup.contains("HLS_SPARSE_HISTORY_MAX_WINDOW_BYTES"));
        let retained_followup = source_between(
            HLS_PLAYER,
            "async fn hls_feed_payload_at_index_followup_retained_status(",
            "async fn hls_deferred_feed_payload(",
        );
        assert!(retained_followup.contains("crate::erasure_coding::CHUNK_SIZE"));
        assert!(
            collector.contains("let sequence_zero_followup = self.is_sequence_zero_followup()")
        );
        assert!(collector.contains("if sequence_zero_followup"));
        assert!(collector.contains("hls_feed_payload_at_index_followup_retained_status("));
        assert!(
            collector.contains("hls_feed_payload_at_index_retained_status(owner, topic, index)")
        );

        let commit = source_between(
            HLS_PLAYER,
            "fn commit_sequence_zero_followup(",
            "async fn catch_up_sequence_zero_followup(",
        );
        assert!(commit.contains("retain_hls_sequence_zero_retries_after"));
        assert!(commit.contains("state.sequence_zero_retry_indices = retry_indices"));
        assert!(commit.contains("deferred_retry_index.filter(|index| *index > head.index)"));
        assert!(commit.contains("state.sequence_zero_deferred_retry_index"));
        assert!(!commit.contains("(!head_confirmed && hls_is_finalized(&archive))"));
        assert!(
            commit.find("let finalized = false").unwrap()
                < commit.find("let changed = head.index").unwrap()
        );
        let catchup = source_between(
            HLS_PLAYER,
            "async fn catch_up_sequence_zero_followup(",
            "async fn refresh_live_feed_head(",
        );
        assert!(catchup.contains("let Some(mut seed) = sequence_zero_followup_seed("));
        let tentative = source_between(
            catchup,
            "if seed.tentative_terminal {",
            "if seed.scan_initialized && refresh_head {",
        );
        let confirm = tentative
            .find("confirm_tentative_sequence_zero_terminal(")
            .unwrap();
        let confirm_end = tentative[confirm..].find(".await;").unwrap() + confirm + ".await;".len();
        let reseed = tentative.find("let Some(refreshed_seed) =").unwrap();
        let evidence = tentative
            .find("hls_sequence_zero_has_newer_authenticated_evidence(")
            .unwrap();
        let install = tentative.find("seed = refreshed_seed;").unwrap();
        assert!(confirm < reseed && reseed < evidence && evidence < install);
        assert!(tentative[confirm..reseed].contains("weeb3.clone()"));
        assert!(tentative[reseed..].contains("sequence_zero_followup_seed("));
        assert!(!tentative[confirm_end..reseed].contains("return false;"));
        assert!(
            install
                < catchup
                    .find("let source_bytes = seed.source_body.as_ref()")
                    .unwrap(),
        );
        assert!(catchup.contains("collector.followup_retry_indices = seed.retry_indices.clone()"));
        assert!(
            catchup.contains("collector.followup_deferred_retry_index = seed.deferred_retry_index")
        );
        assert!(catchup.contains("collector.followup_retry_indices.clone(),"));
        assert!(catchup.contains("collector.followup_deferred_retry_index,"));
        assert!(!catchup.contains("confirm_terminal_feed_head("));
        assert!(
            catchup.find("if seed.tentative_terminal").unwrap()
                < catchup
                    .find("let source_bytes = seed.source_body.as_ref()")
                    .unwrap()
        );
        assert!(tentative.contains("if !refresh_head"));
        assert!(!catchup.contains("blocked_authenticated_evidence"));
        assert!(catchup.contains("persist_sequence_zero_followup_observation("));
        assert!(catchup.contains(".max(seed.positive_ceiling)"));

        let prepare = source_between(
            HLS_PLAYER,
            "async fn prepare_live_history(",
            "fn feed_cache_key(",
        );
        assert!(prepare.contains("initial_is_confirmed_terminal"));
        assert!(prepare.contains("initial_needs_sparse_tail"));
        assert!(prepare.contains("hls_sequence_zero_sparse_tail(&initial.bytes)"));
        assert!(prepare.contains("Some(deferred.index) == direct_index"));
        assert!(prepare.contains("deferred.index > initial_plan.head_index"));
        assert!(prepare.contains("start_deferred_direct(observed_deferred, true, None)"));
        assert!(prepare.contains("collector.admit(direct_index).await"));
        assert!(
            prepare.find("collector.admit(direct_index).await").unwrap()
                < prepare
                    .find("start_deferred_direct(observed_deferred, true, None)")
                    .unwrap()
        );
        assert!(!prepare.contains("async_std::future::timeout"));

        let load = source_between(
            HLS_PLAYER,
            "async fn load_feed_snapshot(",
            "async fn await_terminal_feed_confirmation_view(",
        );
        assert!(load.contains("if start == HlsStart::Live && !defer_live_followup"));
        assert!(load.contains("latest_hls_feed_payload_startup_observing_deferred("));
        assert!(load.contains("start == HlsStart::Live && !defer_live_followup"));
        assert!(load.contains("live_history_presentation_id.is_none()"));

        let install = source_between(
            HLS_PLAYER,
            "fn install_prepared_live_history(",
            "async fn prepare_live_history(",
        );
        assert!(install.contains("finalized: prepared_finalized"));
        assert!(install.contains(
            "state.source_endlist_confirmed = prepared_finalized && hls_is_finalized(&head.bytes)"
        ));
        assert!(install.contains("state.confirmed_head_index = Some(head.index)"));
        assert!(install.contains("state.last_head_check = now"));
        assert!(install.contains("session.live_history_active = true"));
        assert!(prepare.contains("hls_prepared_live_history_is_terminal("));

        let trimming = source_between(
            HLS_PLAYER,
            "fn active_live_history_feed_cache_key(",
            "fn next_feed_route_check_token(",
        );
        assert!(trimming.contains("session.live_start && session.live_history_active"));
        assert!(trimming.contains("active_live_history_key.as_ref() != Some(*key)"));

        let attach = source_between(
            HLS_PLAYER,
            "pub(crate) async fn attach_hls_feed_player(",
            "async fn open_hls_feed_view_generation(",
        );
        assert!(attach.contains("if start == HlsStart::Live"));
        assert!(attach.contains("(None, None)"));
        assert!(attach.contains("(HlsStart::Beginning, snapshot)"));
        assert!(attach.contains("start_beginning_snapshot_runway("));
        assert!(attach.contains("prefetch_live_snapshot_start("));
        assert!(
            attach.find("prefetch_live_snapshot_start(").unwrap()
                < attach.find("prepare_live_history(").unwrap()
        );
        assert!(attach.contains("start == HlsStart::Live && snapshot.is_none()"));
    }

    #[test]
    fn runtime_logging_is_bounded_off_the_network_hot_path() {
        assert!(RUNTIME.contains("mpsc::bounded::<String>(LOG_QUEUE_CAPACITY)"));
        assert!(RUNTIME.contains("for _ in 0..LOG_DRAIN_BATCH"));
        assert!(!RUNTIME.contains("DEBUG_RUNTIME_LOGS"));
        assert!(INTERFACE_RUNTIME.contains("logs.child_element_count() > crate::LOG_DOM_RETAINED"));
        assert!(UPLOAD.contains("logs.child_element_count() > crate::LOG_DOM_RETAINED"));
    }

    #[test]
    fn accounting_sensitive_work_is_dispatched_once_and_may_finish_late() {
        let selection = source_between(
            STATIC_WORKER,
            "async function firstReadyClient(candidates, requiredNetworkId)",
            "async function requestClients(",
        );
        assert!(selection.contains("return [candidates[0]]"));
        assert!(selection.contains("return [candidates[match + 1]]"));

        let dispatch = source_between(
            STATIC_WORKER,
            "function messageFirstClient(clients, message, timeoutMs = FETCH_TIMEOUT_MS)",
            "function toUint8Array(body)",
        );
        assert!(dispatch.contains("return messageClient(clients[0], message, timeoutMs)"));
        assert!(!dispatch.contains("Promise.race"));
        assert!(!dispatch.contains("AbortController"));
        assert!(dispatch.contains("const existing = HLS_REQUEST_FLIGHTS.get(key)"));
        assert!(dispatch.contains("if (existing)"));
        assert!(dispatch.contains("return existing"));

        assert!(INTERFACE.contains("event.stop_immediate_propagation()"));
        assert!(!INTERFACE.contains("port.post_message(&resp).unwrap()"));
        assert!(INTERFACE.contains("let _ = port.post_message(&resp);"));
    }

    #[test]
    fn top_level_hls_requests_skip_only_the_redundant_runtime_probe() {
        let selection = source_between(
            STATIC_WORKER,
            "async function requestClients(event, requestUrl, requiredNetworkId)",
            "function closeMessagePort(",
        );
        assert!(selection.contains("directHlsRequest"));
        assert!(selection.contains("isTopLevelClient(eventClient)"));
        assert!(selection.contains("clientInScope(eventClient)"));
        assert!(selection.contains("return [eventClient]"));
        assert!(selection.contains("return firstReadyClient(candidates, requiredNetworkId)"));
        assert!(INTERFACE.contains("if active_network_id != required_network_id"));
    }

    #[test]
    fn worker_routes_only_explicit_requests_to_the_matching_network() {
        let route_network = source_between(
            STATIC_WORKER,
            "function canonicalRouteNetworkId(pathname)",
            "function isNetworkShellPath(pathname)",
        );
        assert!(route_network.contains("first === \"testnet\" ? 10 : 1"));

        let fetch_handler = source_between(
            STATIC_WORKER,
            "self.addEventListener(\"fetch\", (event) => {",
            "function clientInScope(client)",
        );
        for route in [
            "isBzzUploadPath",
            "canonicalBzzResource",
            "canonicalRawResource",
            "canonicalFeedResource",
        ] {
            assert!(
                fetch_handler.contains(route),
                "missing worker route {route}"
            );
        }
        assert!(fetch_handler.contains("if (url.origin !== SCOPE.origin)"));
        assert!(!fetch_handler.contains("respondWith(fetch(request))"));
        assert!(!fetch_handler.contains("respondWith(fetchOrError(request))"));
        assert!(!STATIC_WORKER.contains("cache.addAll("));
        assert!(!STATIC_WORKER.contains("caches.delete("));

        let raw_routes = source_between(
            STATIC_WORKER,
            "function canonicalRawResource(url)",
            "function canonicalFeedResource(url)",
        );
        assert!(raw_routes.contains("for (const [marker, rawType] of rawRouteMarkers())"));
        assert!(raw_routes.contains(r#"rawType === "hls-bytes" && !isSwarmReference(resource)"#));
        assert!(raw_routes.contains("return resource;"));
    }

    #[test]
    fn first_visit_claims_worker_control_without_reloading() {
        assert!(STATIC_WORKER.contains(r#"const SERVICE_WORKER_MARKER = "forwarder-default20";"#));
        assert!(STATIC_WORKER.contains("const SERVICE_WORKER_PROTOCOL = 8;"));
        assert!(INTERFACE_RUNTIME.contains("const SERVICE_WORKER_PROTOCOL: f64 = 8.0;"));
        assert!(STATIC_WORKER.contains("event.data?.type === \"WEEB3_CLAIM\""));
        assert!(STATIC_WORKER.contains("type: \"WEEB3_CLAIMED\""));
        assert!(INTERFACE_RUNTIME.contains("request_service_worker_claim(&active).await"));
        assert!(
            INTERFACE_RUNTIME
                .contains("service worker still activating for {}; retrying without a reload")
        );

        let setup = source_between(
            INTERFACE_RUNTIME,
            "pub async fn get_service_worker()",
            "fn controlled_service_worker()",
        );
        assert!(setup.contains("SERVICE_WORKER_SETUP_LOCK.with(Arc::clone)"));
        assert!(setup.contains("setup_lock.lock_arc().await"));
        assert!(setup.contains("setup_lock.try_lock_arc()"));

        let readiness = INTERFACE_RUNTIME
            .split_once("pub(crate) async fn service_worker_controls_bzz_requests(")
            .map(|(_, body)| body)
            .unwrap();
        assert!(
            readiness.contains("Duration::from_millis(SERVICE_WORKER_CONTROL_TOTAL_TIMEOUT_MS)")
        );
        let readiness = readiness.to_ascii_lowercase();
        assert!(!readiness.contains("please reload"));
        assert!(!readiness.contains("reload the page"));
        assert!(!INTERFACE_RUNTIME.contains("location.reload"));
    }

    #[test]
    fn server_and_worker_expose_only_the_exact_stream_shell_routes() {
        assert!(
            MAIN_SERVER.contains(r#".route("/weeb-3/stream/{owner}/{topic}", get(get_stream))"#)
        );
        assert!(
            MAIN_SERVER
                .contains(r#".route("/weeb-3/live/stream/{owner}/{topic}", get(get_stream))"#)
        );
        assert!(MAIN_SERVER.contains(r#".route("/{*wildcard}", get(get_404))"#));
        assert!(!MAIN_SERVER.contains("watch/video"));
        assert!(MAIN_SERVER.contains("/weeb-3/hls-stream-example.html"));

        let worker_matcher = source_between(
            STATIC_WORKER,
            "function isDirectShareShellPath(pathname)",
            "function isBzzUploadPath(pathname)",
        );
        assert!(worker_matcher.contains(r#"parts[0] === "live""#));
        assert!(worker_matcher.contains("parts.length === streamOffset + 3"));
        assert!(worker_matcher.contains(r#"parts[streamOffset] === "stream""#));
        assert!(worker_matcher.contains("/^[a-fA-F0-9]{40}$/"));
        assert!(!worker_matcher.contains("testnet"));
        assert!(!worker_matcher.contains("mainnet"));
        assert!(!worker_matcher.contains("index"));

        let fetch_handler = source_between(
            STATIC_WORKER,
            "self.addEventListener(\"fetch\", (event) => {",
            "function clientInScope(client)",
        );
        assert!(fetch_handler.contains("isAppShellNavigation(request)"));
        assert!(fetch_handler.contains("isDirectShareShellPath(url.pathname)"));
        assert!(!STATIC_WORKER.contains("watch/video"));
    }

    #[test]
    fn github_pages_hash_handoff_restores_the_direct_stream_path() {
        assert!(STATIC_404.contains(r#"window.location.replace("/weeb-3/#" + path"#));
        assert!(NAV.contains(r##"let Some(route) = hash.strip_prefix("#/")"##));
        assert!(NAV.contains(r#"format!("{STREAMING_ROUTE_BASE}/{route}")"#));
        assert!(NAV.contains("history.replace_state_with_url("));
        let startup = source_between(
            INTERFACE,
            "pub async fn interweeb(",
            "pub(crate) async fn mount_interface(",
        );
        assert!(
            startup.find("clear_hash_route();").unwrap()
                < startup.find("route_network_mode_from_location()").unwrap()
        );
    }
}
mod hls_payload_cancellation {
    const RETRIEVAL: &str = include_str!("../src/retrieval.rs");
    const STREAM: &str = include_str!("../src/stream.rs");
    const HLS_STREAM: &str = include_str!("../src/stream_hls.rs");

    fn source_section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let start = source
            .find(start)
            .unwrap_or_else(|| panic!("missing source marker: {start}"));
        let tail = &source[start..];
        let end = tail
            .find(end)
            .unwrap_or_else(|| panic!("missing source marker: {end}"));
        &tail[..end]
    }

    #[test]
    fn full_payload_join_threads_generation_through_the_zero_prefix_path() {
        let join = source_section(
            RETRIEVAL,
            "async fn retrieve_data_joined_cancellable(",
            "/// Retrieve Bee's historical joined representation",
        );
        assert!(join.contains("retrieve_decoded_data_root_cancellable("));
        assert!(join.contains("cancel_generations.clone()"));
        assert!(join.contains("cancel.clone()"));
        assert!(join.contains("let output_prefix: &[u8] = if include_span_prefix"));
        assert!(join.contains("retrieve_data_range_from_root_with_prefix_cancellable("));
        assert!(join.contains("cancel_generations,"));
        assert!(join.contains("cancel,"));

        let payload = source_section(
            RETRIEVAL,
            "pub(crate) async fn retrieve_data_payload_cancellable(",
            "pub async fn retrieve_chunk(",
        );
        assert!(payload.contains("retrieve_data_joined_cancellable("));
        assert!(payload.contains("false,"));
        assert!(payload.contains("Some(cancel_generations)"));
        assert!(payload.contains("Some(cancel)"));
    }

    #[test]
    fn weeb3_registers_the_payload_generation_before_traversal() {
        let payload = source_section(
            HLS_STREAM,
            "async fn retrieve_hls_payload_cancellable(",
            "async fn retrieve_hls_payload_range(",
        );
        let registration = payload
            .find("register_retrieve_cancel_token(")
            .expect("payload generation registration");
        let traversal = payload
            .find("retrieve_data_payload_cancellable(")
            .expect("cancellable payload traversal");
        assert!(registration < traversal);
        assert!(payload.contains("self.retrieve_cancel_generations.clone()"));
    }

    #[test]
    fn publishing_a_generation_does_not_start_retrieval() {
        let publish = source_section(
            HLS_STREAM,
            "fn publish_hls_stream_generation(",
            "fn begin_hls_prefetch_session(",
        );
        assert!(publish.contains("register_retrieve_cancel_token("));
        assert!(!publish.contains("retrieve_data"));
        assert!(!publish.contains("chunk_port"));
        assert!(!publish.contains("range_port"));
    }

    #[test]
    fn progressive_ranges_share_the_beginning_runway_generation() {
        let response = source_section(
            HLS_STREAM,
            "async fn fetch_hls_bytes_response(",
            "fn hls_bytes_headers(",
        );
        assert!(response.contains("let progressive_stamp = if let Some(stamp)"));
        assert!(response.contains("hls_progressive_media_candidate(&reference)"));
        assert!(response.contains("progressive_stamp.map(|stamp| stamp.generation)"));
        assert!(response.contains("session.progressive_current(&reference, stamp)"));
        assert!(response.contains("parse_hls_playback_stamp_token(token)"));
        assert!(response.contains("session.stamp() == stamp"));
        assert!(response.contains("X-Weeb3-Stream-Token"));
        assert!(STREAM.contains("js_string_property(obj, \"streamToken\")"));
        assert!(STREAM.contains("stream_token.as_deref()"));
        assert!(response.contains("hls_range_body_fully_cached(&reference, &metadata)"));
        let progressive_candidate = response
            .find("let progressive_start = hls_progressive_media_candidate(&reference)")
            .unwrap();
        assert!(
            response[progressive_candidate..]
                .contains("&& !hls_live_complete_body_candidate(&reference)")
        );
        let pending_body_gate = response[progressive_candidate..]
            .find("hls_payload_body_ready_or_joinable(&reference, generation)")
            .unwrap()
            + progressive_candidate;
        let progressive_plan = response
            .find("let context = hls_foreground_context")
            .unwrap();
        assert!(progressive_candidate < pending_body_gate && pending_body_gate < progressive_plan);
        assert_eq!(
            response
                .matches("hls_payload_body_ready_or_joinable(&reference, generation)")
                .count(),
            2,
            "the stream-open path must recheck after yielding to its size probe"
        );
        assert!(HLS_STREAM.contains("let cached_backward ="));
        assert!(HLS_STREAM.contains("session.remember_progressive_replay(&reference)"));

        let planned = source_section(
            response,
            "let progressive_generation =",
            "if method != \"HEAD\" && range.is_none() {",
        );
        assert!(planned.contains("start_hls_payload_size_probe("));
        assert!(planned.contains("is_manifest: false"));
        assert!(planned.contains("spawn_hls_progressive_range_prefetch("));
        assert!(planned.contains("X-Weeb3-Stream-Start"));
        assert!(
            planned
                .find("spawn_hls_progressive_range_prefetch(")
                .unwrap()
                < planned.find("FetchResponse::stream(200, headers)").unwrap()
        );
        assert!(!planned.contains("resolve_hls_asset("));
        assert!(!planned.contains("retrieve_hls_payload_for_playback("));

        let scheduler = source_section(
            HLS_STREAM,
            "fn hls_progressive_range_ticket_admission(",
            "fn start_hls_payload_load(",
        );
        assert!(scheduler.contains("session.stamp() == ticket.stamp"));
        assert!(scheduler.contains("!session.timeline_rebasing"));
        assert!(scheduler.contains("track.schedule_id == ticket.schedule_id"));
        assert!(scheduler.contains("track.running == Some(ticket)"));
        assert!(scheduler.contains("HlsProgressiveRangeAdmission::Park"));
        assert!(scheduler.contains("Duration::from_millis(50)"));
        assert!(scheduler.contains("background.active >= HLS_BACKGROUND_RANGE_MAX"));
        assert!(scheduler.contains("range_cache_body_bytes()"));
        assert!(scheduler.contains("hls_payload_cache_body_bytes()"));
        assert!(scheduler.contains("hls_progressive_range_reservation_fits("));
        assert!(scheduler.contains("HlsProgressiveRangePurpose::StartupExactNext"));
        assert!(
            scheduler
                .contains("media_cache_max_bytes().saturating_sub(MEDIA_STARTUP_RESPONSE_BYTES)")
        );
        assert!(scheduler.contains("track.running = Some(ticket)"));
        assert!(scheduler.contains("track.running = None"));
        assert!(!scheduler.contains("HlsProgressiveRangePlanner"));
        assert!(!scheduler.contains("admission_open"));
        assert!(!scheduler.contains("HLS_STARTUP_EXACT_NEXT_WINDOWS"));

        let ticket_admission = source_section(
            scheduler,
            "fn hls_progressive_range_ticket_admission(",
            "fn hls_progressive_range_handoff_current(",
        );
        assert!(ticket_admission.contains("hls_progressive_range_admission("));
        assert!(ticket_admission.contains("beginning_prefix"));
        assert!(ticket_admission.contains("bypass_if_expired"));
        assert!(ticket_admission.contains("hls_beginning_prefix_admission("));

        let size_resolver = source_section(
            scheduler,
            "async fn resolve_hls_progressive_payload_size(",
            "async fn prefetch_hls_progressive_reference_windows(",
        );
        assert!(size_resolver.contains("hls_progressive_range_floor(ticket)"));
        assert_eq!(
            size_resolver
                .matches("hls_progressive_frontier_decision(position, foreground_position, false)")
                .count(),
            2,
            "size resolution rechecks the exact foreground after every network await"
        );
        assert!(size_resolver.contains("start_hls_payload_size_probe("));
        assert!(size_resolver.contains("HlsProgressiveRangeLeaseAttempt::Budget"));
        assert!(size_resolver.contains("hls_startup_retry_delay_ms("));
        assert!(size_resolver.contains("HlsProgressiveRangeAdmission::Park"));
        assert!(!size_resolver.contains("return None"));

        let queue = source_section(
            scheduler,
            "fn queue_hls_progressive_window(",
            "fn settle_hls_progressive_window_outcome(",
        );
        let admission = queue
            .find("hls_progressive_range_ticket_admission(ticket, purpose)")
            .unwrap();
        let exact_state = queue.find("hls_aligned_range_state(").unwrap();
        let owner = queue.find("remember_hls_progressive_range_owner(").unwrap();
        let dispatched = queue
            .find("HlsProgressiveWindowDispatch::Dispatched")
            .unwrap();
        let retrieve = queue.find(".retrieve_hls_payload_range(").unwrap();
        assert!(admission < exact_state && exact_state < owner);
        assert!(owner < dispatched && dispatched < retrieve);
        assert!(queue.contains("HlsBackgroundRangeRequest::new(lease, move ||"));
        assert!(queue.contains("!= HlsAlignedRangeState::Absent"));
        assert!(queue.contains("Some(ticket.stamp.generation)"));
        assert!(queue.contains("Some(background)"));

        let settlement = source_section(
            scheduler,
            "fn settle_hls_progressive_window_outcome(",
            "async fn drain_hls_progressive_windows(",
        );
        assert!(settlement.contains("active_windows.remove(&outcome.window)"));
        assert!(settlement.contains("HlsAlignedRangeState::Cached"));
        assert!(settlement.contains("HlsAlignedRangeState::Pending"));
        assert!(settlement.contains("awaiting_terminal.insert(outcome.window)"));
        assert!(settlement.contains("HlsAlignedRangeState::Absent"));
        assert!(settlement.contains("hls_startup_retry_delay_ms(*retry_attempt)"));
        assert!(settlement.contains("retry_attempt.saturating_add(1)"));

        let reference = source_section(
            scheduler,
            "async fn prefetch_hls_progressive_reference_windows(",
            "async fn prefetch_hls_progressive_ranges(",
        );
        let ready = reference
            .find("while let Some(Some(outcome)) = active.next().now_or_never()")
            .unwrap();
        let floor = reference[ready..]
            .find("hls_progressive_range_floor(ticket)")
            .unwrap()
            + ready;
        let states = reference[floor..].find("let mut states").unwrap() + floor;
        let ordered = reference[states..]
            .find("hls_ordered_window_admissions(&states, width, retry_floor)")
            .unwrap()
            + states;
        let lease = reference[ordered..]
            .find("try_hls_progressive_range_lease(ticket, purpose, expected)")
            .unwrap()
            + ordered;
        assert!(ready < floor && floor < states && states < ordered && ordered < lease);
        assert!(reference.contains("hls_progressive_frontier_width("));
        assert!(reference.contains("let retry_floor = retry_attempts"));
        assert!(reference.contains("HlsOrderedWindowState::Backoff"));
        assert!(reference.contains("HlsAlignedRangeState::Pending"));
        assert!(reference.contains("awaiting_terminal.remove(&window)"));
        assert!(reference.contains("drain_hls_progressive_windows(&mut active).await"));
        assert!(reference.contains("HlsProgressiveRangeLeaseAttempt::Budget"));
        assert!(reference.contains("HlsProgressiveRangeLeaseAttempt::Park"));
        assert!(!reference.contains("abort("));
        assert!(!reference.contains("cancel("));

        let range_plan = source_section(
            scheduler,
            "async fn prefetch_hls_progressive_ranges(",
            "fn spawn_hls_progressive_range_prefetch(",
        );
        assert!(range_plan.contains("let initial_successor = launch_position.saturating_add(1)"));
        assert!(range_plan.contains(
            "hls_progressive_frontier_decision(next_position, foreground_position, false)"
        ));
        assert!(range_plan.contains("next_position = position;"));
        assert!(range_plan.contains("hls_critical_prefix_plan_for_reference("));
        let startup = range_plan
            .find("HlsProgressiveRangePurpose::StartupExactNext")
            .unwrap();
        let critical = range_plan[startup..]
            .find("prefix.critical_windows()")
            .unwrap()
            + startup;
        let sustained = range_plan[critical..]
            .find("HlsProgressiveRangePurpose::Sustained")
            .unwrap()
            + critical;
        let completion = range_plan[sustained..]
            .find("HlsProgressiveReferenceProgress::Complete =>")
            .unwrap()
            + sustained;
        let advance = range_plan[completion..]
            .find("hls_progressive_frontier_decision(position, position, true)")
            .unwrap()
            + completion;
        assert!(startup < critical && critical < sustained);
        assert!(sustained < completion && completion < advance);
        assert!(range_plan.contains("HlsCriticalPrefixPlan::total_windows"));
        assert!(range_plan.contains("initial_successor_complete = true"));
        assert!(!range_plan.contains("saturating_add(1).max(foreground_position)"));
        assert!(!range_plan.contains("HLS_STARTUP_EXACT_NEXT_WINDOWS"));
        assert!(!range_plan.contains("HlsProgressiveRangePlanner"));

        let range_retrieve = source_section(
            HLS_STREAM,
            "async fn retrieve_hls_payload_range(",
            "async fn latest_hls_feed_payload_startup(",
        );
        let ui = range_retrieve.find(".start_progress(").unwrap();
        let admit = range_retrieve.find("if !admit()").unwrap();
        let cache_role = range_retrieve.find("read_cached_hls_range(").unwrap();
        assert!(ui < admit && admit < cache_role);

        let ownership = source_section(
            scheduler,
            "fn adopt_hls_progressive_range_owners(",
            "fn clear_hls_progressive_range_owners(",
        );
        assert!(ownership.contains("hls_progressive_range_handoff_current(ticket)"));
        assert!(ownership.contains(".take(1)"));
        assert!(ownership.contains("hls_aligned_range_cached("));
        assert!(ownership.contains("session.retired_startup_plans.clone()"));
        assert!(ownership.contains("protected.extend(session.live_body_runway.iter().cloned())"));
        assert!(ownership.contains("HlsProgressiveRangeOwner { ticket, position }"));
        assert!(ownership.contains("owners.retain(|owner|"));
        assert!(ownership.contains("!protected.contains(*reference)"));
        assert!(ownership.contains("!owned_references.contains(*reference)"));
        assert!(ownership.contains("evict_completed_hls_ranges(&reference, &metadata)"));
        assert!(ownership.contains("evict_completed_body(&reference)"));

        let foreground = source_section(
            HLS_STREAM,
            "fn hls_foreground_context(",
            "fn hls_generation_current(",
        );
        assert!(foreground.contains("let cached_backward ="));
        let transition = foreground
            .find("hls_progressive_foreground_transition(")
            .unwrap();
        let invalidate = foreground[transition..]
            .find("session.advance_generation()")
            .unwrap()
            + transition;
        let update_floor = foreground[invalidate..]
            .find("track.last_foreground_position = transition")
            .unwrap()
            + invalidate;
        assert!(transition < invalidate && invalidate < update_floor);
        assert!(foreground.contains("range_retire_position..cursor.position"));
        assert!(foreground.contains("adopt_hls_progressive_range_owners(ticket, cursor)"));
        assert!(foreground.contains("retire_hls_progressive_range_owners("));
        assert!(foreground.contains("if cached_backward"));
        assert!(foreground.contains("session.remember_progressive_replay(&reference)"));

        let explicit_range = source_section(
            response,
            "let bytes = if let Some(body) = body",
            "headers.push((\"Content-Length\".to_string(), bytes.len().to_string()))",
        );
        assert_eq!(
            explicit_range
                .matches("retrieve_hls_payload_range(")
                .count(),
            1
        );
        assert!(!explicit_range.contains("for attempt in"));

        let stamp_guard = source_section(
            HLS_STREAM,
            "fn hls_prefix_stamp_is_current(",
            "fn hls_progressive_media_candidate(",
        );
        assert!(stamp_guard.contains("session.stamp() == stamp"));
        assert!(stamp_guard.contains("!session.timeline_rebasing"));
    }

    #[test]
    fn sustained_rolling_pair_is_adjacent_bounded_current_and_drain_safe() {
        let boundary = source_section(
            HLS_STREAM,
            "async fn await_hls_progressive_rolling_boundary_admission(",
            "async fn prefetch_hls_progressive_ranges(",
        );
        assert!(boundary.contains("HlsProgressiveRangePurpose::Sustained"));
        assert!(boundary.contains("map_or(1, HlsCriticalPrefixPlan::critical_windows)"));
        assert!(boundary.contains("HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX"));
        assert!(boundary.contains("HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX"));
        assert!(boundary.contains("Some(release_tail_width)"));
        assert!(boundary.contains("Some(HlsProgressiveRollingBoundaryState {"));
        assert!(boundary.contains("Some(release_tail_width),\n                None,"));
        assert!(boundary.contains("current_complete_on_success.set(true)"));
        let authorization = boundary
            .find("await_hls_progressive_rolling_boundary_admission(")
            .unwrap();
        let size_probe = boundary
            .find("resolve_hls_progressive_payload_size(")
            .unwrap();
        assert!(authorization < size_probe);
        assert!(boundary.contains("current_reference_complete.get()"));
        assert!(boundary.contains("== HLS_BACKGROUND_RANGE_MAX"));
        let next_future = boundary.find("let next_boundary = async move").unwrap();
        let current_future = boundary.find("let current_tail =").unwrap();
        let joined = boundary
            .find("join(next_boundary, current_tail).await")
            .unwrap();
        assert!(next_future < current_future && current_future < joined);
        assert!(!boundary.contains("select(next_boundary"));
        assert!(!boundary.contains("abort("));
        assert!(!boundary.contains("cancel("));

        let range_plan = source_section(
            HLS_STREAM,
            "async fn prefetch_hls_progressive_ranges(",
            "fn spawn_hls_progressive_range_prefetch(",
        );
        let adjacent = range_plan
            .find("let next_boundary_position = position.saturating_add(1)")
            .unwrap();
        let pair = range_plan
            .find("prefetch_hls_progressive_rolling_pair(")
            .unwrap();
        assert!(adjacent < pair);
        assert!(range_plan.contains(".get(next_boundary_position)"));
        assert!(!range_plan.contains("saturating_add(2)"));
        assert!(range_plan.contains("HlsProgressiveReferenceProgress::Retired"));
        assert!(range_plan.contains("ForegroundAdvanced(position)"));

        let reference = source_section(
            HLS_STREAM,
            "async fn prefetch_hls_progressive_reference_windows(",
            "async fn prefetch_hls_progressive_boundary_prefix(",
        );
        assert!(reference.contains("hls_progressive_range_ticket_admission(ticket, purpose)"));
        assert!(reference.contains("HlsOrderedWindowState::Pending"));
        assert!(reference.contains("HlsOrderedWindowState::Backoff"));
        assert!(reference.contains("let retry_floor = retry_attempts"));
        assert!(reference.contains("drain_hls_progressive_windows(&mut active).await"));
        assert!(reference.contains("hls_progressive_range_floor(ticket)"));
        assert!(reference.contains("hls_progressive_frontier_decision("));
        assert!(reference.contains("hls_progressive_rolling_lane_width("));
        assert!(reference.contains("hls_progressive_rolling_boundary_width("));
        assert!(reference.contains("first_window_cached"));
        assert!(reference.contains("boundary.first_window_complete.set(true)"));
        let release_tail = reference
            .find("boundary.first_window_complete.set(true)")
            .unwrap();
        let complete_return = reference[release_tail..]
            .find("return HlsProgressiveReferenceProgress::Complete")
            .unwrap()
            + release_tail;
        assert!(
            release_tail < complete_return,
            "W0 releases the tail even when it is the boundary's only required window"
        );
        assert!(reference.contains("foreground_position == position"));
        assert!(reference.contains("needs_width_transition_poll"));
    }

    #[test]
    fn background_hls_range_capacity_follows_only_the_physical_singleflight_leader() {
        let guard = source_section(
            STREAM,
            "pub(crate) struct HlsBackgroundRangeFlightGuard",
            "#[derive(Debug)]\nstruct RangeReadError",
        );
        assert!(guard.contains("on_settled: Option<Box<dyn FnOnce()>>"));
        assert!(guard.contains("impl Drop for HlsBackgroundRangeFlightGuard"));
        assert!(guard.contains("self.on_settled.take()"));
        assert!(guard.contains("on_settled();"));

        let cached_reader = source_section(
            STREAM,
            "async fn read_cached_range(",
            "async fn read_range_window(",
        );
        assert!(cached_reader.contains("background HLS range must be one aligned storage window"));
        assert!(cached_reader.contains("background_flight,"));

        let window = source_section(
            STREAM,
            "async fn read_range_window(",
            "pub(crate) async fn read_cached_hls_range(",
        );
        let role = window.find("range_load_role(").unwrap();
        let wait_release = window
            .find("RangeLoadRole::Wait(receiver) => {\n            drop(background_flight.take());")
            .unwrap();
        let transfer = window
            .find("let background_flight = background_flight.take();")
            .unwrap();
        let spawn = window[transfer..].find("spawn_local(async move").unwrap() + transfer;
        let retained = window[spawn..]
            .find("let _background_flight = background_flight;")
            .unwrap()
            + spawn;
        let physical = window[retained..]
            .find(".acquire_resolved_stream_range(")
            .unwrap()
            + retained;
        let completion = window[physical..].find("finish_pending_range(").unwrap() + physical;
        assert!(role < wait_release && role < transfer);
        assert!(
            transfer < spawn && spawn < retained && retained < physical && physical < completion
        );
        assert_eq!(window.matches("drop(background_flight.take())").count(), 1);
        assert!(!window.contains("fail_pending_ranges"));

        let adapter = source_section(
            STREAM,
            "pub(crate) async fn read_cached_hls_range(",
            "pub(crate) fn hls_aligned_range_cached(",
        );
        assert!(adapter.contains("background_flight: Option<HlsBackgroundRangeFlightGuard>"));
        assert!(adapter.contains("background_flight,"));

        let range_method = source_section(
            HLS_STREAM,
            "async fn retrieve_hls_payload_range(",
            "async fn latest_hls_feed_payload_startup(",
        );
        let progress = range_method.find(".start_progress(").unwrap();
        let progress_await = range_method[progress..].find(".await;").unwrap() + progress;
        let final_admission = range_method.find("if !admit()").unwrap();
        let rejected_release = range_method.find("drop(flight);").unwrap();
        let cache_dispatch = range_method
            .find("let bytes = read_cached_hls_range(")
            .unwrap();
        assert!(progress_await < final_admission && final_admission < cache_dispatch);
        assert!(final_admission < rejected_release && rejected_release < cache_dispatch);
        assert!(range_method.contains("Some(HlsBackgroundRangeRequest { flight, admit })"));

        let limiter = source_section(
            HLS_STREAM,
            "fn try_hls_background_range_lease(",
            "fn claim_hls_progressive_range_scheduler(",
        );
        assert!(limiter.contains("background.active >= HLS_BACKGROUND_RANGE_MAX"));
        assert!(limiter.contains("background.reserved_bytes"));
        assert!(limiter.contains("range_cache_body_bytes()"));
        assert!(limiter.contains("hls_payload_cache_body_bytes()"));

        let queue = source_section(
            HLS_STREAM,
            "fn queue_hls_progressive_window(",
            "fn settle_hls_progressive_window_outcome(",
        );
        assert!(queue.contains("HlsBackgroundRangeRequest::new(lease, move ||"));
        assert!(queue.contains("hls_progressive_range_ticket_admission("));
        assert!(queue.contains("remember_hls_progressive_range_owner("));
        assert!(queue.contains("Some(background)"));
        assert!(queue.contains("!= HlsAlignedRangeState::Absent"));

        let progressive = source_section(
            HLS_STREAM,
            "async fn prefetch_hls_progressive_reference_windows(",
            "async fn prefetch_hls_progressive_ranges(",
        );
        let ready = progressive
            .find("while let Some(Some(outcome)) = active.next().now_or_never()")
            .unwrap();
        let states = progressive[ready..].find("let mut states").unwrap() + ready;
        let admission = progressive[states..]
            .find("hls_ordered_window_admissions(&states, width, retry_floor)")
            .unwrap()
            + states;
        let lease = progressive[admission..]
            .find("try_hls_progressive_range_lease(ticket, purpose, expected)")
            .unwrap()
            + admission;
        assert!(ready < states && states < admission && admission < lease);
        assert!(progressive.contains("HlsAlignedRangeState::Pending"));
        assert!(progressive.contains("HlsOrderedWindowState::Pending"));
        assert!(progressive.contains("active_windows.contains(&window)"));
        assert!(progressive.contains("HlsOrderedWindowState::Backoff"));
        assert!(progressive.contains("let retry_floor = retry_attempts"));
        assert!(progressive.contains("drain_hls_progressive_windows(&mut active).await"));

        let live_prefix = source_section(
            HLS_STREAM,
            "fn start_hls_shared_prefix_warmup(",
            "fn hls_beginning_prefix_window_bounds(",
        );
        assert!(live_prefix.contains("match try_hls_background_range_lease(expected)"));
        assert!(!live_prefix.contains("acquire_hls_background_range_lease("));
        assert!(!live_prefix.contains("task::sleep"));
        assert!(live_prefix.contains("hls_prefix_stamp_is_current(stamp)"));
        assert!(live_prefix.contains("HlsBackgroundRangeRequest::new(lease, move ||"));
        assert!(live_prefix.contains("Some(background)"));

        let cold = source_section(
            HLS_STREAM,
            "fn start_beginning_snapshot_runway(",
            "async fn load_feed_snapshot(",
        );
        assert!(cold.contains("hls_segment_identities(&snapshot.body)"));
        assert!(cold.contains("configure_hls_beginning_prefix_manifest("));
        assert!(cold.contains("ensure_hls_beginning_prefix_configured("));
        assert!(!cold.contains("retrieve_hls_payload_range("));
        assert!(HLS_STREAM.contains("pub(crate) const HLS_BACKGROUND_RANGE_MAX: usize = 4;"));

        let supply = source_section(
            HLS_STREAM,
            "fn ensure_hls_beginning_prefix_configured(",
            "async fn await_hls_beginning_prefix_barrier(",
        );
        assert!(supply.contains("let payload_ranges = (1..target_windows)"));
        assert!(supply.contains("payload_ranges.len() != target_windows.saturating_sub(1)"));
        assert!(supply.contains("scout_data_ranges_cache_only_cancellable("));
        assert!(supply.contains("if !foreground"));
        assert!(supply.contains("start == 0 && end == expected_end"));
        assert!(supply.contains("gate.foreground_zero_requested = true"));
        assert!(supply.contains("gate.foreground_zero_settled = true"));
        assert!(supply.contains("(0..gate.target_windows).all"));
        let ready_check = supply
            .find("let ready = (0..gate.target_windows).all")
            .unwrap();
        let close = supply[ready_check..]
            .find("gate.close_raw_scout()")
            .unwrap()
            + ready_check;
        let ready_phase = supply[close..]
            .find("gate.phase = HlsBeginningPrefixPhase::Ready")
            .unwrap()
            + close;
        assert!(ready_check < close && close < ready_phase);
        assert!(!supply.contains("hls_beginning_adjacent_range"));
        assert!(!supply.contains("start_hls_beginning_adjacent_range"));
        assert!(!supply.contains("try_hls_background_range_lease("));
        assert!(!supply.contains("HlsBackgroundRangeRequest"));
        let raw_scout = source_section(
            supply,
            "fn start_hls_beginning_raw_scout(",
            "fn hls_beginning_raw_seed(",
        );
        assert!(
            raw_scout
                .find("hls_beginning_raw_supply_admission_for(")
                .unwrap()
                < raw_scout
                    .find("let payload_ranges = (1..target_windows)")
                    .unwrap()
        );
        assert!(raw_scout.contains("HlsProgressiveRangeAdmission::Park"));
        assert!(raw_scout.contains("HlsProgressiveRangeAdmission::Admit => break"));
        assert!(!supply.contains("speculative_occupancy"));
        assert!(!supply.contains("supervisor"));
        assert!(!supply.contains("get_connections().await /"));

        let response = source_section(
            HLS_STREAM,
            "async fn fetch_hls_bytes_response(",
            "fn hls_bytes_headers(",
        );
        let foreground_range = source_section(
            response,
            "let bytes = if let Some(body) = body",
            "headers.push((\"Content-Length\".to_string(), bytes.len().to_string()))",
        );
        assert!(foreground_range.contains(
            "progressive_stamp.map(|stamp| stamp.generation),\n                    None,"
        ));
        assert!(response.contains("mark_hls_beginning_foreground_zero_requested("));
        assert!(response.contains("mark_hls_beginning_foreground_range_settled("));
        assert!(response.contains("X-Weeb3-HLS-Critical-Prefix-Windows"));
        assert!(response.contains("let streamed_range_asset ="));
    }
    #[test]
    fn beginning_prefix_bypasses_routes_without_a_tokenized_whole_response() {
        let cold = source_section(
            HLS_STREAM,
            "fn start_beginning_snapshot_runway(",
            "async fn load_feed_snapshot(",
        );
        let ranged = cold
            .find("let managed_prefix_eligible = first.byte_range.is_none();")
            .unwrap();
        let bypass = cold[ranged..]
            .find("if !managed_prefix_eligible {")
            .unwrap()
            + ranged;
        let configure = cold
            .find("configure_hls_beginning_prefix_manifest(")
            .unwrap();
        let size_probe = cold.find("start_hls_payload_size_probe(").unwrap();
        assert!(ranged < bypass && bypass < configure && configure < size_probe);
        assert!(cold.contains("if !managed_prefix_eligible {\n            return;\n        }"));

        let launch = source_section(HLS_STREAM, "async fn launch(", "fn session_from(");
        let native = launch.find("if !mse_supported {").unwrap();
        let native_bypass = launch[native..]
            .find("super::runtime::bypass_hls_beginning_prefix(stamp);")
            .unwrap()
            + native;
        let set_src = launch[native..].find("media.set_src(&source);").unwrap() + native;
        assert!(native < native_bypass && native_bypass < set_src);

        let attach = source_section(
            HLS_STREAM,
            "pub(crate) async fn attach_hls_feed_player(",
            "async fn open_hls_feed_view_generation(",
        );
        assert!(attach.contains("hls_prefix_stamp_for_feed(&weeb3, &owner, &topic)"));
        let play = attach.find("let mode = play_hls(").unwrap();
        let play_call = &attach[play..attach[play..].find(".await").unwrap() + play];
        for argument in ["beginning_prefix_stamp", "presentation_id"] {
            assert!(play_call.contains(argument), "missing {argument}");
        }

        let response = source_section(
            HLS_STREAM,
            "async fn fetch_hls_bytes_response(",
            "fn hls_bytes_headers(",
        );
        let direct_range = response.find("explicit_stream_stamp.is_none()").unwrap();
        let direct_bypass = response[direct_range..]
            .find("bypass_hls_beginning_prefix_for_direct_range(&reference, stamp);")
            .unwrap()
            + direct_range;
        assert!(direct_range < direct_bypass);
    }

    #[test]
    fn startup_exact_next_waits_for_terminal_singleflight_state_before_retrying() {
        let progressive = source_section(
            HLS_STREAM,
            "async fn prefetch_hls_progressive_reference_windows(",
            "async fn prefetch_hls_progressive_ranges(",
        );
        assert!(progressive.contains("HlsAlignedRangeState::Pending"));
        assert!(progressive.contains("HlsOrderedWindowState::Pending"));
        assert!(progressive.contains("active_windows.contains(&window)"));
        assert!(progressive.contains("awaiting_terminal.remove(&window)"));
        assert!(progressive.contains("HlsOrderedWindowState::Backoff"));
        assert!(progressive.contains("let retry_floor = retry_attempts"));
        assert!(progressive.contains("hls_ordered_window_admissions(&states, width, retry_floor)"));

        let settlement = source_section(
            HLS_STREAM,
            "fn settle_hls_progressive_window_outcome(",
            "async fn drain_hls_progressive_windows(",
        );
        let pending = settlement.find("HlsAlignedRangeState::Pending").unwrap();
        let await_terminal = settlement[pending..]
            .find("awaiting_terminal.insert(outcome.window)")
            .unwrap()
            + pending;
        let absent = settlement[await_terminal..]
            .find("HlsAlignedRangeState::Absent")
            .unwrap()
            + await_terminal;
        let retry = settlement[absent..]
            .find("hls_startup_retry_delay_ms(*retry_attempt)")
            .unwrap()
            + absent;
        assert!(pending < await_terminal && await_terminal < absent && absent < retry);

        let ready = progressive
            .find("while let Some(Some(outcome)) = active.next().now_or_never()")
            .unwrap();
        let states = progressive[ready..].find("let mut states").unwrap() + ready;
        let admission = progressive[states..]
            .find("hls_ordered_window_admissions(&states, width, retry_floor)")
            .unwrap()
            + states;
        assert!(
            ready < states && states < admission,
            "terminal outcomes are latched before a returned slot can admit later work"
        );

        let range_state = source_section(
            STREAM,
            "pub(crate) enum RangeCacheState",
            "pub(crate) fn evict_completed_hls_ranges(",
        );
        assert!(range_state.contains("RangeCacheState::Cached"));
        assert!(range_state.contains("RangeCacheState::Pending"));
        assert!(range_state.contains("RangeCacheState::Absent"));
        assert!(range_state.contains("Every nonzero generation shares this cache slot"));
        assert!(range_state.contains("pub(crate) fn range_cache_state("));
        assert!(range_state.contains("pub(crate) fn range_cache_observation("));
        assert!(range_state.contains("resource: &str"));
        assert!(range_state.contains("pending_ranges"));
        assert!(range_state.contains(".get(&pending_key)"));
        assert!(range_state.contains(".map(|pending| pending.generation)"));

        let singleflight = source_section(
            STREAM,
            "async fn read_cached_range(",
            "pub(crate) async fn read_cached_hls_range(",
        );
        assert!(singleflight.contains("Keep the shared slot while its detached transport drains"));
        assert!(singleflight.contains("finish_pending_range("));
    }
    #[test]
    fn hls_chunk_timeouts_release_logical_work_but_keep_accounting_draining() {
        let attempt = source_section(
            RETRIEVAL,
            "async fn retrieve_attempt(",
            "fn chunk_address_parts(",
        );
        assert!(attempt.contains("failed_retrieve_attempt(&peer)"));
        assert!(!attempt.contains("result_chan.try_send(terminal_result)"));
        let detached = attempt
            .rfind("let result = settle_retrieve_attempt(")
            .expect("detached accounting-only settlement");
        let detached_completion = attempt[detached..]
            .find("profile.complete(")
            .map(|index| detached + index)
            .expect("detached telemetry completes after accounting settlement");
        assert!(detached < detached_completion);
        assert!(!attempt[detached..].contains("result_chan.try_send("));

        let chunk = source_section(
            RETRIEVAL,
            "pub async fn retrieve_chunk(",
            "pub async fn retrieve_check_chunk(",
        );
        assert!(!chunk.contains("result.terminal"));

        let waiter = source_section(
            RETRIEVAL,
            "fn remove_raw_fetch_waiter(",
            "fn complete_raw_fetch(",
        );
        assert!(waiter.contains("admission.close()"));
        assert!(!waiter.contains(".take("));
        let completion = source_section(
            RETRIEVAL,
            "fn complete_raw_fetch(",
            "fn queue_drained_raw_chunk(",
        );
        assert!(completion.contains(".take(key, flight_id)"));
    }
}

mod stream_reader_concurrency {
    const STREAMING_PLAYER: &str = include_str!("../src/stream.rs");
    const HLS_STREAM: &str = include_str!("../src/stream_hls.rs");
    const PREFETCH_POLICY: &str = include_str!("../src/stream_conventions.rs");

    const MIB_BYTES: u64 = 1024 * 1024;
    const STARTUP_RESPONSE_BYTES: u64 = 8 * MIB_BYTES;
    const SEEK_REQUEST_GAP_BYTES: u64 = 6 * MIB_BYTES;
    const SEEK_RESET_GAP_BYTES: u64 = 16 * MIB_BYTES;

    fn source_section(start: &str, end: &str) -> &'static str {
        let start = STREAMING_PLAYER
            .find(start)
            .unwrap_or_else(|| panic!("missing streaming-player source marker: {start}"));
        let tail = &STREAMING_PLAYER[start..];
        let end = tail
            .find(end)
            .unwrap_or_else(|| panic!("missing streaming-player source marker: {end}"));
        &tail[..end]
    }

    fn hls_source_section(start: &str, end: &str) -> &'static str {
        let start = HLS_STREAM
            .find(start)
            .unwrap_or_else(|| panic!("missing HLS source marker: {start}"));
        let tail = &HLS_STREAM[start..];
        let end = tail
            .find(end)
            .unwrap_or_else(|| panic!("missing HLS source marker: {end}"));
        &tail[..end]
    }

    fn request_is_seek(
        previous_anchor: Option<u64>,
        previous_high_water: i64,
        previous_request_start: u64,
        start: u64,
    ) -> bool {
        let is_request_jump = previous_anchor.is_some()
            && start.saturating_add(SEEK_REQUEST_GAP_BYTES) < previous_request_start;
        previous_anchor.is_some()
            && (is_request_jump
                || start.saturating_add(SEEK_RESET_GAP_BYTES) < previous_anchor.unwrap_or(0)
                || start as i64 > previous_high_water + SEEK_RESET_GAP_BYTES as i64)
    }

    #[test]
    fn waiter_timeout_keeps_the_draining_singleflight_registered() {
        let window = source_section(
            "async fn read_range_window(",
            "fn spawn_prefetch_media_stages(",
        );
        assert!(window.contains("RangeReadError::waiter_timeout(error)"));
        assert!(window.contains("Keep the shared slot while its detached transport drains."));

        let response = source_section(
            "async fn fetch_bzz_response(",
            "async fn resolve_bzz_cached(",
        );
        assert!(response.contains("if !error.waiter_timed_out"));
        assert!(
            response.contains("note_media_range_failure")
                && response.contains("return FetchResponse::error(503, error.message)")
        );
    }

    #[test]
    fn hls_ranges_reuse_the_completed_window_cache_and_pending_singleflight() {
        let adapter = source_section(
            "pub(crate) async fn read_cached_hls_range(",
            "pub(crate) fn hls_range_body_fully_cached(",
        );
        let activate = adapter
            .find("activate_external_media_generation")
            .expect("HLS media generation activation");
        let shared = adapter
            .find("read_cached_range(")
            .expect("shared range reader");
        assert!(activate < shared);
        assert!(adapter.contains("async_std::future::timeout("));
        assert!(adapter.contains("Duration::from_millis(STREAM_RANGE_REQUEST_TIMEOUT_MS)"));
        assert!(adapter.contains("then_some(cancel_stream_key)"));
        assert!(!adapter.contains("read_cached_range_with_retry"));

        let completion = source_section(
            "pub(crate) fn hls_aligned_range_cached(",
            "fn spawn_prefetch_media_stages(",
        );
        assert!(completion.contains("range_storage_windows_for_span("));
        assert!(completion.contains("range_cache_key(&resource, metadata, start, end)"));
        assert!(completion.contains("cache.ranges.contains_key(&key)"));

        let eviction = source_section(
            "pub(crate) fn evict_completed_hls_ranges(",
            "pub(crate) fn hls_range_body_fully_cached(",
        );
        assert!(eviction.contains("cache.range_order.retain("));
        assert!(eviction.contains("cache.ranges.remove(&key)"));
        assert!(eviction.contains("cache.range_bytes ="));
        assert!(eviction.contains("cache.media_states.remove(&media_key)"));
        assert!(!eviction.contains("pending_ranges"));
        assert!(!eviction.contains("finish_pending_range"));
        assert!(!eviction.contains("fail_pending_ranges"));

        let window = source_section(
            "async fn read_range_window(",
            "pub(crate) async fn read_cached_hls_range(",
        );
        assert!(window.contains("range_load_role(&cache_key, &pending_key, generation)"));
        assert!(window.contains("let media_key = media_state_key(&resource, &metadata)"));
        assert!(window.contains("cancel_stream_key.unwrap_or_else(|| media_key.clone())"));
        assert!(window.contains("&media_key,\n                        generation,"));
    }

    #[test]
    fn a_terminal_window_failure_does_not_fail_sibling_windows() {
        let failure = source_section(
            "fn note_media_range_failure(",
            "async fn read_cached_range_with_retry(",
        );
        assert!(failure.contains("state.mark_failure(start)"));
        assert!(!failure.contains("fail_pending_ranges_with_prefix"));
        assert!(!failure.contains("reset_media_state"));
    }

    #[test]
    fn normal_startup_continuation_is_not_a_seek() {
        let previous_high_water = STARTUP_RESPONSE_BYTES as i64 - 1;
        assert!(!request_is_seek(
            Some(0),
            previous_high_water,
            0,
            STARTUP_RESPONSE_BYTES,
        ));
        assert!(request_is_seek(
            Some(0),
            previous_high_water,
            0,
            40 * MIB_BYTES,
        ));
        assert!(request_is_seek(
            Some(40 * MIB_BYTES),
            48 * MIB_BYTES as i64,
            40 * MIB_BYTES,
            0,
        ));

        let begin = source_section("fn begin_media_range(", "fn response_range_for_request(");
        assert!(begin.contains(
            "start.saturating_add(STREAM_SEEK_REQUEST_GAP_BYTES) < previous_request_start"
        ));
        assert!(!begin.contains(
            "start > previous_request_start.saturating_add(STREAM_SEEK_REQUEST_GAP_BYTES)"
        ));
        assert!(begin.contains("start as i64 > previous_high_water"));
    }

    #[test]
    fn prefetch_budget_reserves_the_active_response() {
        let budget = source_section(
            "fn stream_prefetch_ahead_limit_bytes()",
            "#[derive(Clone)]\nstruct MediaRangeState",
        );
        assert!(budget.contains("media_prefetch_ahead_limit_bytes(media_cache_max_bytes())"));
        assert!(PREFETCH_POLICY.contains("MEDIA_PREFETCH_ACTIVE_HEADROOM_BYTES"));
        assert!(
            PREFETCH_POLICY
                .contains("MEDIA_STARTUP_RESPONSE_BYTES + 3 * MEDIA_STORAGE_WINDOW_BYTES")
        );
        assert!(PREFETCH_POLICY.contains(".saturating_sub(MEDIA_PREFETCH_ACTIVE_HEADROOM_BYTES)"));
        assert!(PREFETCH_POLICY.contains(".min(MEDIA_PREFETCH_AHEAD_HARD_LIMIT_BYTES)"));

        let prefetch = source_section(
            "async fn prefetch_media_stages(",
            "async fn prefetch_media_windows(",
        );
        assert!(prefetch.contains("stream_prefetch_ahead_limit_bytes()"));
        assert!(prefetch.contains("media_prefetch_stage_targets(ahead_limit_bytes)"));

        let hls_prefetch = hls_source_section(
            "async fn prefetch_hls_media_stages(",
            "async fn retrieve_hls_payload_for_playback(",
        );
        assert!(hls_prefetch.contains("media_prefetch_ahead_limit_bytes("));
        assert!(hls_prefetch.contains("hls_payload_cache_capacity_bytes()"));
        assert!(hls_prefetch.contains("run.planned_bytes >= byte_limit"));
        assert!(hls_prefetch.contains("run.planned_bytes < byte_limit"));
        assert!(hls_prefetch.contains("plan_media_prefetch_batch("));
        assert!(!hls_prefetch.contains("join_all(probes)"));
    }

    #[test]
    fn switching_from_regular_media_reclaims_only_completed_ranges_for_hls() {
        let remember = source_section("fn remember_range(", "fn clear_completed_ranges(");
        assert!(remember.contains("state.generation == generation"));
        assert!(remember.contains("if generation > 0"));

        let clear = source_section("fn clear_completed_ranges(", "fn range_load_role(");
        assert!(clear.contains("self.range_order.clear()"));
        assert!(clear.contains("self.ranges.clear()"));
        assert!(clear.contains("self.range_bytes = 0"));
        assert!(clear.contains("for state in self.media_states.values_mut()"));
        assert!(clear.contains("state.generation = next_media_generation()"));
        assert!(!clear.contains("pending_ranges"));

        let begin_hls = hls_source_section(
            "fn begin_hls_prefetch_session(",
            "fn set_hls_prefetch_mode(",
        );
        assert!(begin_hls.contains("clear_completed_bzz_media_ranges()"));
        assert!(begin_hls.contains("cache.retire_pending_bodies()"));
        assert!(begin_hls.contains("cache.clear_completed_bodies()"));
        assert!(begin_hls.contains("cache.retain_completed = true"));
        assert!(!begin_hls.contains("body_pending.clear"));
        assert!(!begin_hls.contains("sequence_zero_start_requested"));

        let window = source_section(
            "async fn read_range_window(",
            "fn spawn_prefetch_media_stages(",
        );
        assert!(window.contains("stream_key.clone()"));
        assert!(window.contains("&media_key"));
    }

    #[test]
    fn switching_from_hls_reclaims_only_completed_fragments_for_regular_media() {
        let cache = hls_source_section("impl HlsAssetCache {", "struct PendingHlsPayload");
        assert!(cache.contains("if self.retain_completed"));

        let transition = source_section(
            "fn replace_bzz_result_view(",
            "pub(crate) fn replace_result_view_contents(",
        );
        assert!(transition.contains("release_hls_for_bzz_view()"));
        assert!(transition.contains("release_bzz_view()"));

        let release = hls_source_section("pub(crate) fn release_hls_for_bzz_view()", "\n    }\n}");
        assert!(release.contains("release_hls_view()"));
        assert!(release.contains("clear_completed_bodies()"));

        let clear = hls_source_section(
            "fn clear_completed_bodies(&mut self)",
            "\n    }\n\n    struct PendingHlsPayload",
        );
        assert!(clear.contains("self.retain_completed = false"));
        assert!(clear.contains("self.body_order.clear()"));
        assert!(clear.contains("self.bodies.clear()"));
        assert!(clear.contains("self.body_bytes = 0"));
        assert!(!clear.contains("body_pending.clear()"));
        assert!(!clear.contains("size_pending.clear()"));
    }

    #[test]
    fn stable_and_seekable_reads_have_separate_pending_slots() {
        let keying = source_section("fn pending_range_key(", "fn range_cache_prefix(");
        assert!(keying.contains("\"{cache_key}|pending:stable\""));
        assert!(keying.contains("\"{cache_key}|pending:media\""));

        let window = source_section(
            "async fn read_range_window(",
            "fn spawn_prefetch_media_stages(",
        );
        assert!(window.contains("range_load_role(&cache_key, &pending_key, generation)"));
        assert!(window.contains("leader_cache_key.clone()"));
        assert!(window.contains("&leader_pending_key"));
    }

    #[test]
    fn media_pause_stops_future_batches_without_failing_dispatched_windows() {
        let suspend = source_section(
            "fn suspend_bzz_fetch_resource_prefetch(",
            "fn suspend_bzz_fetch_url_prefetch(",
        );
        assert!(suspend.contains("state.generation = next_media_generation()"));
        assert!(suspend.contains("state.prefetch_running = false"));
        assert!(!suspend.contains("fail_pending_ranges_with_prefix"));
        assert!(!suspend.contains("finish_pending_range"));

        let lifecycle = source_section(
            "fn install_bzz_media_prefetch_lifecycle(",
            "fn install_playback_notifications(",
        );
        assert!(lifecycle.contains("[\"pause\", \"seeking\"]"));
        assert!(lifecycle.contains("suspend_bzz_fetch_url_prefetch(&src)"));
        assert!(!lifecycle.contains("AbortController"));
        assert!(!lifecycle.contains(".abort("));
    }
}
