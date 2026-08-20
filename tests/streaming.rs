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
        HLS_BACKGROUND_RANGE_MAX, HLS_LIVE_SYNC_SEGMENTS, HLS_SPARSE_HISTORY_MAX_PARALLEL,
        HlsDirectArchiveDisposition, HlsLevelTransition, HlsManifestProbe, HlsMediaPlanRegistry,
        HlsPrefetchMode, HlsProgressiveRangeAdmission, HlsProgressiveRangePlanner,
        HlsProgressiveRunway, HlsProgressiveRunwayTransition, HlsProgressiveRunways,
        HlsSequenceZeroRetry, MAX_STREAM_FEED_PAYLOAD_BYTES,
        append_hls_sequence_zero_archive_suffix, assemble_hls_sequence_zero_suffix,
        assemble_hls_sparse_history, classify_hls_level_transition, continue_hls_codec_bootstrap,
        extend_hls_sequence_zero_archive, hls_autoplay_gate_ready, hls_contiguous_buffered_ahead,
        hls_direct_archive_disposition, hls_dom_pause_is_explicit, hls_dom_play_is_explicit,
        hls_is_finalized, hls_is_long_sequence_zero_checkpoint, hls_live_frontier_is_ready,
        hls_live_tail, hls_manifest_reload_is_continuous, hls_manifest_reload_is_forward,
        hls_media_references, hls_media_sequence, hls_payload_mime,
        hls_progressive_range_admission, hls_progressive_range_reservation_fits,
        hls_progressive_runway_closed_after_mode, hls_progressive_startup_window_count,
        hls_sequence_zero_covers_head, hls_sequence_zero_ordinary_retry,
        hls_sequence_zero_retry_stays_queued, hls_sequence_zero_same_index_archive_is_reusable,
        hls_sequence_zero_sparse_tail, hls_startup_prefix_is_preferred,
        hls_tail_has_terminal_endlist, hls_target_duration, hls_timeline_rebase_position,
        hls_timeline_rebase_required, hls_verified_sequence_zero_checkpoint_tail,
        hls_verified_sequence_zero_checkpoint_tail_at_index, is_hls_manifest,
        plan_hls_sequence_zero_followup_recovery, plan_hls_sequence_zero_terminal_confirmation,
        plan_hls_sparse_forward_wave, plan_hls_sparse_history_from_lattice,
        plan_hls_sparse_history_repairs_for_attempts, plan_hls_sparse_terminal_repairs,
        prepend_hls_codec_bootstrap, probe_hls_manifest, raise_hls_target_duration,
        remember_hls_sequence_zero_retry, retain_hls_sequence_zero_retries_after,
        rewrite_hls_manifest, rewrite_hls_manifest_for_live_reload, select_hls_sequence_zero_retry,
        stream_feed_payload_len_is_supported, touch_hls_cache_lru,
    };

    const REF: &str = "919b5395bf7a59cbb3b365769de09a2b15ac5d897823dda9270259a3c038d574";
    const REF2: &str = "49428dc8819f560aa3e6226a8c1036a25c091a51d5745de381b842f73243f6d9";
    const REF3: &str = "14aec3fbbb36882d4eba4881fdaa6f2336e5d600b133d677e3f3f5c9d54d8290";
    const REF4: &str = "68d3d40b39d5f17532e928a4b62f2a58ea1b63e20da0eb4b8a7da78d45d45812";
    const OWNER: &str = "352eabdea9cb05e984a8828d2a6df3d3b5023260";
    const TOPIC: &str = "cfbbc155d709547b198638d0fb11d733359561538d8bd606a9ab257354d13bcc";

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
    fn progressive_range_planner_claims_nearest_future_references_with_three_workers() {
        let mut planner = HlsProgressiveRangePlanner::new(4, 10);
        assert_eq!(planner.worker_count(), 3);
        assert_eq!(planner.claim(), Some(5));
        assert_eq!(planner.claim(), Some(6));
        assert_eq!(planner.claim(), Some(7));
        assert_eq!(planner.claim(), None);
        assert!(planner.has_unclaimed_references());

        planner.complete(6);
        assert_eq!(planner.claim(), None);
        planner.complete(5);
        assert_eq!(planner.claim(), Some(8));
        assert_eq!(planner.claim(), Some(9));
        assert_eq!(planner.claim(), None);
        assert!(!planner.has_unclaimed_references());

        assert_eq!(HlsProgressiveRangePlanner::new(8, 10).worker_count(), 1);
        assert_eq!(HlsProgressiveRangePlanner::new(10, 10).worker_count(), 0);
    }

    #[test]
    fn progressive_range_admission_parks_pause_and_startup_but_retires_stale_work() {
        assert_eq!(
            hls_progressive_range_admission(true, HlsPrefetchMode::Inactive),
            HlsProgressiveRangeAdmission::Park
        );
        assert_eq!(
            hls_progressive_range_admission(true, HlsPrefetchMode::StartupOnly),
            HlsProgressiveRangeAdmission::Park
        );
        assert_eq!(
            hls_progressive_range_admission(true, HlsPrefetchMode::Sustained),
            HlsProgressiveRangeAdmission::Admit
        );
        assert_eq!(
            hls_progressive_range_admission(false, HlsPrefetchMode::Sustained),
            HlsProgressiveRangeAdmission::Retire
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
    fn beginning_warmup_bounds_one_two_four_and_larger_assets_to_four_windows() {
        const WINDOW: u64 = 512 * 1024;
        assert_eq!(hls_progressive_startup_window_count(1, WINDOW), 1);
        assert_eq!(hls_progressive_startup_window_count(WINDOW, WINDOW), 1);
        assert_eq!(hls_progressive_startup_window_count(WINDOW + 1, WINDOW), 2);
        assert_eq!(hls_progressive_startup_window_count(2 * WINDOW, WINDOW), 2);
        assert_eq!(hls_progressive_startup_window_count(4 * WINDOW, WINDOW), 4);
        assert_eq!(hls_progressive_startup_window_count(5 * WINDOW, WINDOW), 4);
        assert_eq!(hls_progressive_startup_window_count(0, WINDOW), 0);
        assert_eq!(hls_progressive_startup_window_count(WINDOW, 0), 0);
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
    fn dom_playback_intent_ignores_autoplay_events_and_preserves_explicit_pause() {
        assert!(!hls_dom_play_is_explicit(true));
        assert!(hls_dom_play_is_explicit(false));
        assert!(!hls_dom_pause_is_explicit(true, false));
        assert!(!hls_dom_pause_is_explicit(true, true));
        assert!(!hls_dom_pause_is_explicit(false, false));
        assert!(hls_dom_pause_is_explicit(false, true));
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
        assert_eq!(HLS_LIVE_SYNC_SEGMENTS, 8);
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
    fn live_reload_starts_eight_segments_behind_the_current_edge() {
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
        assert!(rewritten.contains("#EXT-X-START:TIME-OFFSET=-14,PRECISE=NO"));
        assert_eq!(hls_live_tail(live.as_bytes()), Some((2, 14.0)));
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
        assert!(live.contains("#EXT-X-START:TIME-OFFSET=-16,PRECISE=NO"));
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
        cached_feed_should_refresh_head, hls_snapshot_is_terminal,
        hls_terminal_peer_view_is_mature,
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
    fn terminal_confirmation_matures_before_the_population_cap() {
        assert_eq!(HLS_TERMINAL_CONFIRMATION_MIN_PRICED_PEERS, 8);
        assert!(!hls_terminal_peer_view_is_mature(1));
        assert!(!hls_terminal_peer_view_is_mature(7));
        assert!(hls_terminal_peer_view_is_mature(8));
        assert!(hls_terminal_peer_view_is_mature(200));
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
        assert!(attach.contains("HlsStart::Beginning => 0.0"));
        assert!(attach.contains("HlsStart::Live => -1.0"));
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
        assert!(play_hls.contains("let autoplay_gate_required = autoplay_allowed;"));
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
        assert!(launch.contains("hls_config(request.source.contains(\"start=live\"))"));
        let native_preload = launch.find("media.set_preload(\"auto\")").unwrap();
        let native_load = launch.find("media.load();").unwrap();
        let native_gate = launch.find("start_autoplay_buffer_gate(epoch)").unwrap();
        assert!(native_preload < native_load && native_load < native_gate);
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

        let bootstrap = source_between(player, "fn buffer_created(", "fn fragment_buffered(");
        let seek = bootstrap.find("media.set_current_time(target)").unwrap();
        let release = bootstrap.find("session.codec_pending = false").unwrap();
        let resume = bootstrap.find("autoplay(epoch, Autoplay::Resume)").unwrap();
        assert!(bootstrap.contains("finish_hls_codec_bootstrap(&source)"));
        assert!(bootstrap.contains(".split(\"&codec-bootstrap=\")"));
        assert!(bootstrap.contains("session.playback_authorized || session.resume"));
        assert!(bootstrap.contains("if resume"));
        assert!(bootstrap.contains("start_autoplay_buffer_gate(epoch)"));
        assert!(seek < release && release < resume);

        let manifest = source_between(player, "fn manifest_parsed(", "fn level_loaded(");
        assert!(manifest.contains("session.load = LoadPhase::Warmup"));
        assert!(
            manifest.find("start_at(epoch, &hls, position)").unwrap()
                < manifest.find("hls_autoplay_gate_ready(").unwrap()
        );
        assert!(manifest.contains("hls_contiguous_buffered_ahead("));
        assert!(manifest.contains(".any(|(_, live, _)| !live)"));
        assert!(manifest.contains("media.duration()"));
        assert!(!manifest.contains("autoplay_deadline"));
        assert!(manifest.contains("if session.playback_authorized"));
        assert!(manifest.contains("!session.playback_authorized"));
        assert!(manifest.contains("if session.codec_pending"));
        assert!(manifest.contains("sleep(HLS_AUTOPLAY_GATE_POLL).await"));
        assert!(manifest.contains("session.autoplay_gate_pending = true"));
        assert!(manifest.contains("start && !resume && session.autoplay_gate_required"));
        assert!(manifest.contains("session.autoplay_gate_pending = false"));
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
        let stop = rebase.find("hls.stop_load()").unwrap();
        let defer = rebase.find("Wait::Microtask.wait().await").unwrap();
        let relaunch = rebase.find("launch(request).await").unwrap();
        assert!(rebase.contains("hls_timeline_rebase_position("));
        assert!(rebase.contains("autoplay_allowed: session.autoplay_allowed"));
        assert!(rebase.contains("autoplay_gate_required: session.autoplay_gate_required"));
        assert!(rebase_event < stop && stop < defer && defer < relaunch);

        let dom = source_between(player, "fn dom_event(", "fn handle_error(");
        assert!(dom.contains("let user = hls_dom_play_is_explicit(pending)"));
        assert!(dom.contains("hls_dom_pause_is_explicit("));
        assert!(dom.contains("session.autoplay_pending,"));
        assert!(dom.contains("session.playback_authorized,"));
        assert!(dom.contains("session.playback_authorized = true"));
        assert!(dom.contains("session.playback_authorized = false"));
        assert!(dom.contains("session.autoplay_allowed = false"));
        assert!(dom.contains("emit(&media, HLS_AUTOPLAY_AUTHORIZED_EVENT)"));
        assert!(dom.contains("emit(&media, HLS_EXPLICIT_PAUSE_EVENT)"));

        let errors = source_between(player, "fn handle_error(", "fn hard_recovery(");
        assert!(errors.contains("sourceBufferName"));
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
        assert!(hard_recovery.contains("autoplay_gate_required: session.autoplay_gate_required"));
        let autoplay = source_between(player, "fn autoplay(", "fn playback_error(");
        assert!(
            autoplay.contains("matches!(intent, Autoplay::Resume) || session.autoplay_allowed")
        );
        assert!(!autoplay.contains("media.autoplay()"));
        assert!(autoplay.contains("&& !session.codec_pending"));
        assert!(autoplay.contains("if matches!(session.load, LoadPhase::Warmup)"));
        assert!(
            autoplay.find("session.autoplay_pending = true").unwrap()
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
        assert!(generation_advance.contains("HLS_SERIAL_PREFETCH_COMPLETIONS.saturating_sub(1)"));
        assert!(!timeline_advance.contains("self.completed_media_payloads = 0;"));
        let session_start = source_between(
            HLS_PLAYER,
            "fn begin_hls_prefetch_session(",
            "fn remember_authenticated_hls_startup_prefix(",
        );
        assert!(session_start.contains("session.live_start = live_start;"));
        assert!(HLS_PLAYER.contains("const HLS_EXACT_NEXT_OVERLAP_SEGMENTS: usize = 2;"));

        let cache = source_between(HLS_PLAYER, "fn load_role(", "fn finish_load(");
        assert!(cache.contains(".filter(|pending| pending.generation == generation)"));
        assert!(cache.contains("if prefetch"));
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
        assert!(
            load.contains("std::iter::once(HLS_EARLY_FEED_PREFIX_INDEX)")
                && load.contains(".chain(persisted_index.is_some().then_some(0))")
        );
        assert!(load.contains("Some(HLS_EARLY_FEED_PREFIX_INDEX)"));
        assert!(HLS_PLAYER.contains("const HLS_EARLY_FEED_PREFIX_INDEX: u64 = 7;"));
        assert!(HLS_PLAYER.contains("const HLS_EARLY_FEED_PREFIX_PREFERRED_SEGMENTS: usize = 8;"));
        let prefix_fanout = source_between(
            HLS_PLAYER,
            "async fn fan_out_authenticated_hls_prefixes(",
            "fn hls_prefix_stamp_for_feed(",
        );
        assert!(prefix_fanout.contains(">= HLS_EARLY_FEED_PREFIX_PREFERRED_SEGMENTS"));
        assert!(
            load.matches(">= HLS_EARLY_FEED_PREFIX_PREFERRED_SEGMENTS")
                .count()
                >= 1
        );
        assert!(load.matches("HLS_STARTUP_PREFIX_RESULT_GRACE").count() >= 2);
        let canonical_first = source_between(
            load,
            "Either::Right((Ok(Some(canonical)), _)) =>",
            "Either::Right((Ok(None) | Err(_), _)) =>",
        );
        assert!(canonical_first.contains("let canonical_starts_late ="));
        assert!(canonical_first.contains("if preferred.is_none()"));
        assert!(canonical_first.contains("&& canonical_starts_late"));
        assert_eq!(canonical_first.matches("prefix_ready_in.recv()").count(), 1);
        assert!(canonical_first.contains(
            "if canonical_starts_late {\n                                        return None;"
        ));
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
        assert!(live_prefetch.contains("hls_live_tail(&snapshot.body)"));
        assert!(live_prefetch.contains("session.live_start && session.stamp() == stamp"));
        assert_eq!(
            live_prefetch
                .matches("start_hls_shared_prefix_warmup(")
                .count(),
            2
        );
        assert!(!live_prefetch.contains("start_hls_payload_load("));
        assert!(live_prefetch.contains(".take(HLS_PREFETCH_BODY_MAX_PARALLEL)"));
        let tail_prefetch = live_prefetch.find(".skip(start)").unwrap();
        let codec_prefetch = live_prefetch
            .find("hls_media_sequence(&snapshot.body) == Some(0)")
            .unwrap();
        assert!(tail_prefetch < codec_prefetch);
        assert!(live_prefetch.contains("HLS_LIVE_PREFIX_WINDOW_COUNT"));
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
        assert!(beginning_wait.contains("hls_media_references(&snapshot.body).into_iter()"));
        assert!(beginning_wait.contains("let Some(reference) = references.next()"));
        assert!(
            beginning_wait
                .contains("hls_progressive_startup_window_count(size, MEDIA_STORAGE_WINDOW_BYTES)")
        );
        assert!(beginning_wait.contains("let mut windows = FuturesUnordered::new();"));
        assert!(beginning_wait.contains("for _ in 0..window_count"));
        assert!(beginning_wait.contains("if position >= size"));
        assert_eq!(
            beginning_wait
                .matches("retrieve_hls_payload_range(")
                .count(),
            1
        );
        let dispatch = beginning_wait.find("windows.push(async move").unwrap();
        let settle = beginning_wait
            .find("while let Some(window_ready) = windows.next().await")
            .unwrap();
        let publish = beginning_wait
            .rfind("hls_progressive_startup_admission_is_current")
            .unwrap();
        assert!(dispatch < settle && settle < publish);
        assert_eq!(
            beginning_wait
                .matches("hls_progressive_startup_admission_is_current")
                .count(),
            5
        );
        assert!(beginning_wait.contains("Some(generation)"));
        assert!(!beginning_wait.contains("start_hls_payload_size_probe(\n                warmup_client.clone(),\n                successor"));
        assert!(!beginning_wait.contains("prefix_len"));

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
            "pub(crate) async fn acquire_latest_raw_feed_payload_startup(",
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
        assert!(beginning_runway < overlap && overlap < play);
        assert!(prepare < overlap && overlap < play);
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
        assert!(catchup.contains("if refresh_head"));
        assert!(catchup.contains(
            "if !refresh_head && !continuation_invocation && blocked_authenticated_evidence"
        ));
        assert!(catchup.contains("persist_sequence_zero_followup_observation("));

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
        assert!(install.contains("state.confirmed_head_index = Some(head.index)"));
        assert!(install.contains("state.last_head_check = now"));
        assert!(install.contains("session.live_history_active = true"));

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
        assert!(STATIC_WORKER.contains("const SERVICE_WORKER_PROTOCOL = 7;"));
        assert!(INTERFACE_RUNTIME.contains("const SERVICE_WORKER_PROTOCOL: f64 = 7.0;"));
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
        assert!(!response.contains("await_hls_progressive_successor_admission("));
        assert!(!HLS_STREAM.contains("runways.claim_successor(plan_id, reference)"));
        assert!(!response.contains("release_hls_progressive_successor_claim("));
        assert!(HLS_STREAM.contains("let cached_backward ="));
        assert!(HLS_STREAM.contains("session.remember_progressive_replay(&reference)"));

        let planned = source_section(
            response,
            "let progressive_context =",
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
        assert!(scheduler.contains("media_prefetch_ahead_limit_bytes(media_cache_max_bytes())"));
        assert!(scheduler.contains("hls_progressive_range_reservation_fits("));
        assert!(scheduler.contains("HlsProgressiveRangePlanner::new("));
        assert!(scheduler.contains("let mut workers = FuturesUnordered::new();"));
        assert!(scheduler.contains("for _ in 0..worker_count"));
        assert!(scheduler.contains("let admission_open = Rc::new(Cell::new(true));"));
        assert!(scheduler.matches("if !admission_open.get()").count() >= 3);
        assert!(scheduler.contains("planner.has_unclaimed_references()"));
        assert!(scheduler.contains("planner.borrow_mut().complete(position)"));
        assert!(scheduler.contains("admission_open.set(false)"));
        assert!(scheduler.contains("hls_aligned_range_cached("));
        assert!(scheduler.contains("remember_hls_progressive_range_owner("));
        assert!(scheduler.contains("HlsBackgroundRangeRequest::new("));
        assert!(scheduler.contains("start = end.saturating_add(1)"));
        assert!(scheduler.contains("Some(ticket.stamp.generation)"));
        assert!(scheduler.contains("track.running = Some(ticket)"));
        assert!(scheduler.contains("track.running = None"));
        assert!(!scheduler.contains("start_hls_payload_load("));

        let reference_worker = source_section(
            scheduler,
            "async fn prefetch_hls_progressive_reference(",
            "async fn prefetch_hls_progressive_ranges(",
        );
        assert!(
            reference_worker
                .matches("await_hls_progressive_range_sustained(ticket, &admission_open)")
                .count()
                >= 2
        );
        let dispatch = reference_worker
            .find(".retrieve_hls_payload_range(")
            .unwrap();
        let owner = reference_worker[..dispatch]
            .rfind("remember_hls_progressive_range_owner(")
            .unwrap();
        let stop = reference_worker[..owner]
            .rfind("if !admission_open.get()")
            .unwrap();
        let drain = reference_worker[dispatch..].find(".await;").unwrap() + dispatch;
        let next_window = reference_worker[drain..]
            .find("start = end.saturating_add(1)")
            .unwrap()
            + drain;
        assert!(stop < owner && owner < dispatch && dispatch < drain && drain < next_window);
        assert!(
            !reference_worker[drain..next_window].contains("remember_hls_progressive_range_owner(")
        );
        assert!(!reference_worker.contains("abort("));
        assert!(!reference_worker.contains("cancel("));
        assert!(
            !reference_worker.contains("drop(lease);\n            if usize::try_from(expected)")
        );

        let ownership = source_section(
            scheduler,
            "fn adopt_hls_progressive_range_owners(",
            "fn clear_hls_progressive_range_owners(",
        );
        assert!(ownership.contains("hls_progressive_range_handoff_current(ticket)"));
        assert!(ownership.contains(".take(HLS_PROGRESSIVE_RANGE_WORKERS_PER_PLAN)"));
        assert!(ownership.contains("hls_aligned_range_cached("));
        assert!(ownership.contains("HlsProgressiveRangeOwner { ticket, position }"));
        assert!(ownership.contains("owners.retain(|owner|"));
        assert!(ownership.contains("!protected.contains(*reference)"));
        assert!(ownership.contains("!owned_references.contains(*reference)"));
        assert!(ownership.contains("evict_completed_hls_ranges(&reference, &metadata)"));
        assert!(ownership.contains("evict_completed_body(&reference)"));
        let retired_after_read = ownership
            .find("HlsProgressiveRangeAdmission::Retire")
            .unwrap();
        let defer_reference = ownership[retired_after_read..]
            .find(".retired_references")
            .unwrap()
            + retired_after_read;
        let cleanup = ownership[defer_reference..]
            .find("retire_hls_progressive_range_owners(None, &[])")
            .unwrap()
            + defer_reference;
        assert!(retired_after_read < defer_reference && defer_reference < cleanup);

        let body_eviction = source_section(
            HLS_STREAM,
            "fn evict_completed_body(",
            "fn retire_pending_bodies(",
        );
        assert!(body_eviction.contains("self.body_pending.get(reference)"));
        assert!(body_eviction.contains("self.retired_body_loads.insert("));
        assert!(body_eviction.contains("self.body_order.retain("));
        assert!(body_eviction.contains("self.bodies.remove(reference)"));
        assert!(body_eviction.contains("self.body_bytes ="));
        assert!(!body_eviction.contains("self.body_pending.remove("));

        let body_completion = source_section(HLS_STREAM, "fn finish_load(", "fn remember_body(");
        assert!(body_completion.contains("let retired ="));
        assert!(body_completion.contains("self.retired_body_loads"));
        assert!(body_completion.contains(".remove(&(reference.to_string(), generation, load_id))"));
        assert!(body_completion.contains("if !retired && let Ok(body) = &result"));
        assert!(body_completion.contains("pending.finish(result)"));

        let foreground = source_section(
            HLS_STREAM,
            "fn hls_foreground_context(",
            "fn hls_generation_current(",
        );
        assert!(foreground.contains("let cached_backward ="));
        assert!(foreground.contains("if !cached_backward"));
        assert!(foreground.contains("range_retire_position..cursor.position"));
        assert!(foreground.contains("let superseded_references ="));
        let adopt = foreground
            .find("adopt_hls_progressive_range_owners(ticket, cursor)")
            .unwrap();
        let retire = foreground
            .find("retire_hls_progressive_range_owners(")
            .unwrap();
        assert!(adopt < retire);

        let plan_retirement = source_section(
            HLS_STREAM,
            "fn retire_hls_prefetch_plan(",
            "fn invalidate_hls_prefetch_session(",
        );
        let remove_runway = plan_retirement
            .find("session.remove_progressive_plan(ticket.plan_id)")
            .unwrap();
        let cleanup = plan_retirement
            .find("retire_hls_progressive_range_owners(None, &retired_references)")
            .unwrap();
        assert!(plan_retirement.contains(".references_for_plans(&[ticket.plan_id])"));
        assert!(remove_runway < cleanup);

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
        assert!(!explicit_range.contains("HLS_FOREGROUND_MAX_ATTEMPTS"));

        let resolver = source_section(
            HLS_STREAM,
            "async fn resolve_hls_asset(",
            "fn hls_codec_bootstrap_manifest(",
        );
        assert!(resolver.contains(
            "retrieve_hls_payload_range(reference.clone(), payload_size, 0, probe_end, None, None)"
        ));

        let stamp_guard = source_section(
            HLS_STREAM,
            "fn hls_prefix_stamp_is_current(",
            "fn hls_progressive_media_candidate(",
        );
        assert!(stamp_guard.contains("session.stamp() == stamp"));
        assert!(stamp_guard.contains("!session.timeline_rebasing"));
        let warmer = source_section(
            HLS_STREAM,
            "fn start_hls_shared_prefix_warmup(",
            "fn start_beginning_snapshot_runway(",
        );
        let epoch_check = warmer
            .find("!hls_prefix_stamp_is_current(stamp)")
            .expect("full generation and timeline stamp guard");
        let range = warmer
            .find("retrieve_hls_payload_range(")
            .expect("shared range admission");
        assert!(epoch_check < range);
        assert!(warmer.contains("Some(stamp.generation)"));
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

        let progressive = source_section(
            HLS_STREAM,
            "async fn prefetch_hls_progressive_reference(",
            "async fn prefetch_hls_progressive_ranges(",
        );
        assert!(progressive.contains("HlsBackgroundRangeRequest::new(lease, move ||"));
        assert!(progressive.contains("hls_progressive_range_ticket_admission(ticket)"));
        assert!(progressive.contains("remember_hls_progressive_range_owner("));
        assert!(progressive.contains("Some(background),"));
        assert!(progressive.contains("admission_open.set(false);"));

        let live_prefix = source_section(
            HLS_STREAM,
            "fn start_hls_shared_prefix_warmup(",
            "fn start_beginning_snapshot_runway(",
        );
        assert!(live_prefix.contains("match try_hls_background_range_lease(expected)"));
        assert!(!live_prefix.contains("acquire_hls_background_range_lease("));
        assert!(!live_prefix.contains("task::sleep"));
        assert!(live_prefix.contains("hls_prefix_stamp_is_current(stamp)"));
        assert!(live_prefix.contains("HlsBackgroundRangeRequest::new(lease, move ||"));
        assert!(live_prefix.contains("Some(background),"));

        let cold = source_section(
            HLS_STREAM,
            "fn start_beginning_snapshot_runway(",
            "async fn load_feed_snapshot(",
        );
        assert!(cold.contains("acquire_hls_background_range_lease(expected, ||"));
        assert!(cold.contains("hls_progressive_startup_admission_is_current("));
        assert!(cold.contains("HlsBackgroundRangeRequest::new(lease, move ||"));
        assert!(cold.contains("Some(background),"));

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
        let resolver = source_section(
            HLS_STREAM,
            "async fn resolve_hls_asset(",
            "fn hls_codec_bootstrap_manifest(",
        );
        assert!(resolver.contains(
            "retrieve_hls_payload_range(reference.clone(), payload_size, 0, probe_end, None, None)"
        ));
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
            .find("let _ = settle_retrieve_attempt(")
            .expect("detached accounting-only settlement");
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
