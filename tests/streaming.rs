#![allow(dead_code)]

#[path = "../src/feed.rs"]
mod feed;
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

    use std::{collections::HashMap, sync::Arc};
    use stream_hls::{
        HLS_ARCHIVE_MEDIA_PLAN_MAX_REFERENCES, HLS_LIVE_HISTORY_MEDIA_PLAN_MAX_REFERENCES,
        HLS_LIVE_SYNC_SEGMENTS, HLS_MEDIA_PLAN_MAX_REFERENCES,
        HLS_PROGRESSIVE_RANGE_COVERAGE_MAX_WINDOWS, HlsLevelTransition, HlsManifestProbe,
        HlsMediaPlanRegistry, HlsProgressiveRangeCoverage, HlsProgressiveRangeUpdate,
        MAX_STREAM_FEED_PAYLOAD_BYTES, append_hls_sequence_zero_archive_suffix,
        classify_hls_level_transition, continue_hls_codec_bootstrap,
        continue_hls_sequence_zero_codec_bootstrap, extend_hls_sequence_zero_archive,
        hls_cross_plan_seek_transition, hls_exact_sequence_zero_live_target,
        hls_foreground_cursor_transition, hls_is_finalized, hls_live_tail,
        hls_manifest_reload_is_continuous, hls_manifest_reload_is_forward,
        hls_media_plan_reference_limit, hls_media_references, hls_media_sequence, hls_payload_mime,
        hls_startup_prefix_is_preferred, hls_target_duration, hls_timeline_rebase_position,
        hls_timeline_rebase_required, is_hls_manifest, prepend_hls_codec_bootstrap,
        probe_hls_manifest, raise_hls_target_duration, rewrite_hls_manifest,
        rewrite_hls_manifest_for_live_reload, stream_feed_payload_len_is_supported,
    };

    const REF: &str = "919b5395bf7a59cbb3b365769de09a2b15ac5d897823dda9270259a3c038d574";
    const REF2: &str = "49428dc8819f560aa3e6226a8c1036a25c091a51d5745de381b842f73243f6d9";
    const REF3: &str = "14aec3fbbb36882d4eba4881fdaa6f2336e5d600b133d677e3f3f5c9d54d8290";
    const REF4: &str = "68d3d40b39d5f17532e928a4b62f2a58ea1b63e20da0eb4b8a7da78d45d45812";
    const OWNER: &str = "352eabdea9cb05e984a8828d2a6df3d3b5023260";
    const TOPIC: &str = "cfbbc155d709547b198638d0fb11d733359561538d8bd606a9ab257354d13bcc";

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
        assert!(String::from_utf8_lossy(&continuation).contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
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
    fn growing_sequence_zero_codec_bootstrap_keeps_its_first_discontinuity_anchor() {
        let manifest = |count: usize| {
            format!(
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n\
                 #EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-MEDIA-SEQUENCE:0\n{}",
                (0..count)
                    .map(|position| format!("#EXTINF:2.0,\n{:064x}\n", position.saturating_add(1)))
                    .collect::<String>()
            )
        };
        let initial_source = manifest(12);
        let next_source = manifest(13);
        let following_source = manifest(14);
        let initial =
            prepend_hls_codec_bootstrap(initial_source.as_bytes(), initial_source.as_bytes())
                .unwrap();
        let moving_anchor = continue_hls_codec_bootstrap(next_source.as_bytes()).unwrap();
        let next =
            continue_hls_sequence_zero_codec_bootstrap(next_source.as_bytes(), Some(4)).unwrap();
        let following =
            continue_hls_sequence_zero_codec_bootstrap(following_source.as_bytes(), Some(4))
                .unwrap();

        assert!(!hls_manifest_reload_is_continuous(&initial, &moving_anchor));
        assert!(hls_manifest_reload_is_continuous(&initial, &next));
        assert!(hls_manifest_reload_is_continuous(&next, &following));
        assert_eq!(
            String::from_utf8_lossy(&following)
                .matches("#EXT-X-DISCONTINUITY")
                .count(),
            1
        );
    }

    #[test]
    fn exact_sequence_zero_live_target_uses_manifest_edge_and_start_offset() {
        assert_eq!(
            hls_exact_sequence_zero_live_target(0, true, 9_746.6, -18.583),
            Some(9_728.017)
        );
        assert!(hls_exact_sequence_zero_live_target(0, true, 42.0, 0.0).is_none());
        assert!(hls_exact_sequence_zero_live_target(1, true, 42.0, -8.0).is_none());
        assert!(hls_exact_sequence_zero_live_target(0, false, 42.0, -8.0).is_none());
        assert!(hls_exact_sequence_zero_live_target(0, true, 42.0, 1.0).is_none());
        assert_eq!(
            hls_exact_sequence_zero_live_target(0, true, 4.0, -8.0),
            Some(0.0)
        );
        assert!(hls_exact_sequence_zero_live_target(0, true, f64::NAN, -8.0).is_none());
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
    fn explicit_seek_distinguishes_cached_targets_from_back_reads() {
        assert_eq!(
            hls_foreground_cursor_transition(10, 3, true, false),
            (false, 10, false)
        );
        assert_eq!(
            hls_foreground_cursor_transition(10, 3, true, true),
            (true, 3, true)
        );
        assert_eq!(
            hls_foreground_cursor_transition(10, 9, true, true),
            (false, 9, true)
        );
        assert_eq!(
            hls_foreground_cursor_transition(10, 9, true, false),
            (false, 10, false)
        );
        assert_eq!(
            hls_foreground_cursor_transition(10, 3, false, false),
            (true, 3, false)
        );
        assert_eq!(
            hls_foreground_cursor_transition(10, 11, true, true),
            (false, 11, false)
        );
    }

    #[test]
    fn cross_plan_seek_requires_one_distant_case_insensitive_anchor() {
        let references = ["zero", "ANCHOR", "two", "three", "target"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(hls_cross_plan_seek_transition(
            "anchor",
            &references,
            4,
            true
        ));
        assert!(!hls_cross_plan_seek_transition(
            "anchor",
            &references,
            4,
            false
        ));
        assert!(!hls_cross_plan_seek_transition(
            "missing",
            &references,
            4,
            true
        ));
        assert!(!hls_cross_plan_seek_transition(
            "anchor",
            &references,
            references.len(),
            true
        ));
        assert!(!hls_cross_plan_seek_transition(
            "anchor",
            &references,
            0,
            true
        ));
        assert!(!hls_cross_plan_seek_transition(
            "anchor",
            &references,
            2,
            true
        ));

        let duplicate = ["anchor", "one", "ANCHOR", "three", "target"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(!hls_cross_plan_seek_transition(
            "anchor", &duplicate, 4, true
        ));

        let backward = ["target", "one", "two", "three", "anchor"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(hls_cross_plan_seek_transition("ANCHOR", &backward, 0, true));
    }

    #[test]
    fn progressive_range_coverage_waits_for_contiguous_zero_based_success() {
        const WINDOW: u64 = 512;
        let size = WINDOW * 3 + 7;
        let mut coverage = HlsProgressiveRangeCoverage::default();
        coverage.begin("asset", 7, 11, size);

        assert_eq!(
            coverage.record_success("ASSET", 7, 11, size, WINDOW * 3, size - 1, WINDOW),
            HlsProgressiveRangeUpdate::Pending
        );
        assert_eq!(
            coverage.record_success("asset", 7, 11, size, WINDOW, WINDOW * 2 - 1, WINDOW),
            HlsProgressiveRangeUpdate::Pending
        );
        assert_eq!(
            coverage.record_success("asset", 7, 11, size, WINDOW, WINDOW * 2 - 1, WINDOW),
            HlsProgressiveRangeUpdate::Duplicate
        );
        assert_eq!(
            coverage.record_success("asset", 7, 11, size, 0, WINDOW - 1, WINDOW),
            HlsProgressiveRangeUpdate::Pending
        );
        assert_eq!(
            coverage.record_success("asset", 7, 11, size, WINDOW * 2, WINDOW * 3 - 1, WINDOW,),
            HlsProgressiveRangeUpdate::Complete
        );
        assert_eq!(
            coverage.record_success("asset", 7, 11, size, WINDOW * 2, WINDOW * 3 - 1, WINDOW,),
            HlsProgressiveRangeUpdate::Rejected
        );
    }

    #[test]
    fn progressive_range_coverage_rejects_gaps_failures_and_resets_by_stamp() {
        const WINDOW: u64 = 512;
        let size = WINDOW * 2;
        let mut coverage = HlsProgressiveRangeCoverage::default();
        coverage.begin("asset", 1, 3, size);

        assert_eq!(
            coverage.record_success("asset", 1, 3, size, 1, WINDOW, WINDOW),
            HlsProgressiveRangeUpdate::Rejected
        );
        assert_eq!(
            coverage.record_success("asset", 1, 3, size, 0, WINDOW - 1, WINDOW),
            HlsProgressiveRangeUpdate::Rejected
        );

        coverage.begin("asset", 2, 3, size);
        assert_eq!(
            coverage.record_success("asset", 2, 3, size, WINDOW, size - 1, WINDOW),
            HlsProgressiveRangeUpdate::Pending
        );
        assert_eq!(
            coverage.record_success("asset", 1, 3, size, 0, WINDOW - 1, WINDOW),
            HlsProgressiveRangeUpdate::Rejected
        );
        assert_eq!(
            coverage.record_success("asset", 2, 3, size, 0, WINDOW - 1, WINDOW),
            HlsProgressiveRangeUpdate::Complete
        );

        coverage.begin("asset", 2, 4, size);
        coverage.fail("asset", 2, 4);
        assert_eq!(
            coverage.record_success("asset", 2, 4, size, 0, WINDOW - 1, WINDOW),
            HlsProgressiveRangeUpdate::Rejected
        );

        coverage.begin("asset", 2, 5, size);
        assert_eq!(
            coverage.record_success("asset", 2, 5, size + 1, 0, WINDOW - 1, WINDOW),
            HlsProgressiveRangeUpdate::Rejected
        );
        assert_eq!(
            coverage.record_success("asset", 2, 5, size, 0, WINDOW - 1, WINDOW),
            HlsProgressiveRangeUpdate::Rejected
        );

        coverage.begin("asset", 3, 6, size);
        assert_eq!(
            coverage.record_success("asset", 3, 6, size, 0, WINDOW - 1, WINDOW),
            HlsProgressiveRangeUpdate::Pending
        );
        coverage.clear_unless("other", 3, 6);
        assert_eq!(
            coverage.record_success("asset", 3, 6, size, WINDOW, size - 1, WINDOW),
            HlsProgressiveRangeUpdate::Rejected
        );

        let oversized = WINDOW * (HLS_PROGRESSIVE_RANGE_COVERAGE_MAX_WINDOWS as u64 + 1);
        coverage.begin("asset", 4, 6, oversized);
        assert_eq!(
            coverage.record_success("asset", 4, 6, oversized, 0, WINDOW - 1, WINDOW),
            HlsProgressiveRangeUpdate::Rejected
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
        plans.install_with_early_overlap_limit(references.clone(), 2, false);
        let first = plans.cursor("second", &HashMap::new()).unwrap().cursor;
        plans.install_with_early_overlap_limit(references, 2, false);
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
        );
        let old = plans.cursor("shared", &HashMap::new()).unwrap().cursor;
        plans.install_with_early_overlap_limit(
            ["new-a", "shared", "new-c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            1,
            false,
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
        plans.install_with_early_overlap_limit(references.clone(), usize::MAX, false);
        let first = plans.cursor("b", &HashMap::new()).unwrap().cursor.plan.id;
        plans.install_with_early_overlap_limit(references, usize::MAX, false);
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
        );

        assert!(plans.cursor("0", &HashMap::new()).is_none());
        let tail = plans.cursor("3", &HashMap::new()).unwrap().cursor;
        assert_eq!(tail.plan.references.as_ref(), ["3", "4", "5"]);
        assert_eq!(tail.position, 0);
    }

    #[test]
    fn active_sequence_zero_live_history_uses_its_distinct_plan_limit() {
        assert_eq!(
            hls_media_plan_reference_limit(false, true, Some(0)),
            HLS_LIVE_HISTORY_MEDIA_PLAN_MAX_REFERENCES
        );
        assert_eq!(
            hls_media_plan_reference_limit(true, true, Some(0)),
            HLS_ARCHIVE_MEDIA_PLAN_MAX_REFERENCES
        );
        assert_eq!(
            hls_media_plan_reference_limit(false, true, Some(1)),
            HLS_MEDIA_PLAN_MAX_REFERENCES
        );
        assert_eq!(
            hls_media_plan_reference_limit(false, false, Some(0)),
            HLS_MEDIA_PLAN_MAX_REFERENCES
        );
    }

    #[test]
    fn active_sequence_zero_live_history_keeps_start_midpoint_and_growth_in_one_cap() {
        const INITIAL_SEGMENTS: usize = 8_200;
        let references = (0..INITIAL_SEGMENTS)
            .map(|position| format!("segment-{position}"))
            .collect::<Vec<_>>();
        let mut plans = HlsMediaPlanRegistry::new(HLS_LIVE_HISTORY_MEDIA_PLAN_MAX_REFERENCES);
        plans.install_with_early_overlap_limit(references.clone(), 2, true);

        let first = plans
            .cursor(&references[0], &HashMap::new())
            .unwrap()
            .cursor;
        let midpoint = plans
            .cursor(&references[INITIAL_SEGMENTS / 2], &HashMap::new())
            .unwrap()
            .cursor;
        let last = plans
            .cursor(&references[INITIAL_SEGMENTS - 1], &HashMap::new())
            .unwrap()
            .cursor;
        assert_eq!(
            (first.position, midpoint.position, last.position),
            (0, 4_100, 8_199)
        );
        assert_eq!(first.plan.references.len(), INITIAL_SEGMENTS);
        let initial_plan = first.plan.id;

        let mut grown = references;
        grown.push(format!("segment-{INITIAL_SEGMENTS}"));
        plans.install_with_early_overlap_limit(grown.clone(), 2, true);

        let selected = plans
            .cursor(&grown[0], &HashMap::from([(initial_plan, 0)]))
            .unwrap();
        assert_ne!(selected.cursor.plan.id, initial_plan);
        assert!(selected.superseded_plan_ids.is_empty());
        assert_eq!(selected.cursor.position, 0);
        assert_eq!(selected.cursor.plan.references.len(), grown.len());
        assert!(
            selected.cursor.plan.references.len() <= HLS_LIVE_HISTORY_MEDIA_PLAN_MAX_REFERENCES
        );
        assert_eq!(
            plans
                .cursor(&grown[grown.len() / 2], &HashMap::new())
                .unwrap()
                .cursor
                .position,
            grown.len() / 2
        );
        assert_eq!(
            plans
                .cursor(grown.last().unwrap(), &HashMap::new())
                .unwrap()
                .cursor
                .position,
            grown.len() - 1
        );
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
        );
        plans.install_with_early_overlap_limit(
            ["archive-a", "archive-b", "archive-c", "archive-d"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            3,
            false,
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
    fn media_plan_capacity_retires_old_indices_as_one_bounded_generation() {
        let mut plans = HlsMediaPlanRegistry::new(3);
        plans.install_with_early_overlap_limit(
            ["old-a", "old-b", "old-c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            1,
            false,
        );
        plans.install_with_early_overlap_limit(
            ["new-a", "new-b", "new-c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            1,
            false,
        );
        assert!(plans.cursor("old-a", &HashMap::new()).is_none());
        for (position, reference) in ["new-a", "new-b", "new-c"].into_iter().enumerate() {
            let cursor = plans.cursor(reference, &HashMap::new()).unwrap().cursor;
            assert_eq!(cursor.position, position);
            assert_eq!(cursor.plan.references.len(), 3);
        }
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
    use crate::{feed::FeedFrontierConfidence, stream_hls};

    use stream_hls::{
        FEED_FOLLOWUP_BATCH_LIMIT, HLS_LIVE_HISTORY_MAX_PARALLEL,
        HLS_LIVE_HISTORY_UNRESOLVED_MAX_PARALLEL, HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL,
        HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT, HLS_TERMINAL_CONFIRMATION_MIN_PRICED_PEERS,
        cached_feed_should_refresh_head, hls_live_frontier_observation_is_authoritative,
        hls_live_frontier_remaining_ms, hls_live_history_active_parallelism,
        hls_live_tail_prefetch_ready, hls_snapshot_is_terminal, hls_terminal_peer_view_is_mature,
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
    fn live_frontier_deadline_only_exposes_positive_finite_time() {
        assert_eq!(
            hls_live_frontier_remaining_ms(15_000.0, 7_250.0),
            Some(7_750.0)
        );
        assert!(hls_live_frontier_remaining_ms(15_000.0, 15_000.0).is_none());
        assert!(hls_live_frontier_remaining_ms(15_000.0, 15_001.0).is_none());
        assert!(hls_live_frontier_remaining_ms(f64::INFINITY, 1.0).is_none());
        assert!(hls_live_frontier_remaining_ms(15_000.0, f64::NAN).is_none());
    }

    #[test]
    fn sequence_zero_presentation_keeps_a_bounded_exact_runway() {
        assert_eq!(FEED_FOLLOWUP_BATCH_LIMIT, 4);
        assert_eq!(HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT, 64);
        assert_eq!(HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL, 4);
    }

    #[test]
    fn live_history_yields_retrieval_capacity_until_the_frontier_is_frozen() {
        assert_eq!(HLS_LIVE_HISTORY_UNRESOLVED_MAX_PARALLEL, 8);
        assert_eq!(HLS_LIVE_HISTORY_MAX_PARALLEL, 32);
        assert_eq!(hls_live_history_active_parallelism(false, 64), 8);
        assert_eq!(hls_live_history_active_parallelism(true, 64), 32);
        assert_eq!(hls_live_history_active_parallelism(false, 3), 3);
        assert_eq!(hls_live_history_active_parallelism(true, 0), 0);
        assert!(!hls_live_tail_prefetch_ready(false, false));
        assert!(hls_live_tail_prefetch_ready(false, true));
        assert!(hls_live_tail_prefetch_ready(true, false));
    }

    #[test]
    fn invalid_clocks_do_not_force_a_head_probe() {
        assert!(!cached_feed_should_refresh_head(f64::NAN, 25_000.0));
        assert!(!cached_feed_should_refresh_head(10_000.0, f64::INFINITY));
        assert!(!cached_feed_should_refresh_head(25_000.0, 10_000.0));
    }

    #[test]
    fn a_fresh_dense_guard_is_authoritative_without_a_second_scout() {
        let started = 1_000.0;
        assert!(!hls_live_frontier_observation_is_authoritative(
            FeedFrontierConfidence::Unresolved,
            1_100.0,
            started,
        ));
        assert!(hls_live_frontier_observation_is_authoritative(
            FeedFrontierConfidence::Guarded,
            1_200.0,
            started,
        ));
        assert!(!hls_live_frontier_observation_is_authoritative(
            FeedFrontierConfidence::Guarded,
            999.0,
            started,
        ));
        assert!(!hls_live_frontier_observation_is_authoritative(
            FeedFrontierConfidence::Guarded,
            f64::NAN,
            started,
        ));
    }

    #[test]
    fn exactly_confirmed_live_frontier_is_immediately_authoritative() {
        assert!(hls_live_frontier_observation_is_authoritative(
            FeedFrontierConfidence::Exact,
            1_100.0,
            1_000.0,
        ));
        assert!(!hls_live_frontier_observation_is_authoritative(
            FeedFrontierConfidence::Unresolved,
            1_100.0,
            1_000.0,
        ));
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
        assert!(attach.contains("exact_live_codec_bootstrap_ready("));
        assert!(attach.contains("initial_start_position,\n            proactive_codec_bootstrap,"));
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

        let launch = source_between(player, "async fn launch(", "fn session_from(");
        let authorization = launch
            .find("get_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)")
            .unwrap();
        let epoch = launch.find("let epoch = next_epoch()").unwrap();
        let clear_authorization = launch
            .find("remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)")
            .unwrap();
        assert!(authorization < epoch && epoch < clear_authorization);
        let native = launch.find("if !mse_supported").unwrap();
        let proactive = launch.rfind("if request.force_codec_bootstrap").unwrap();
        let codec = launch.find("let codec_token =").unwrap();
        assert!(native < proactive && proactive < codec);
        assert!(launch.contains(".push_str(&format!(\"&codec-bootstrap={epoch}\"))"));
        assert!(launch.contains("request.initial_position = 0.0"));
        assert!(launch.contains("let _ = request.media.pause();"));
        assert!(launch.contains("hls_config(!request.source.contains(\"start=live\"))"));
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
        assert!(bootstrap.contains("js_property(&tracks, \"audio\")"));
        assert!(bootstrap.contains("js_property(&tracks, \"audiovideo\")"));
        assert!(bootstrap.contains("if let Some(video) = video"));
        assert!(bootstrap.contains("session.codec_buffer_ready = true"));
        assert!(bootstrap.contains("let target = session.codec_target.take()?"));
        assert_eq!(
            bootstrap.matches("media.set_current_time(target)").count(),
            1
        );
        assert!(release < seek && seek < resume);
        assert!(!player.contains("liveSyncPosition"));

        let playing = source_between(
            player,
            "fn finish_codec_bootstrap_on_playing(",
            "fn handle_error(",
        );
        let finish = playing.find("finish_hls_codec_bootstrap(&source)").unwrap();
        let strip = playing.find(".split(\"&codec-bootstrap=\")").unwrap();
        assert!(finish < strip);
        assert!(!playing.contains("set_current_time"));
        assert!(!playing.contains("codec_required = false"));

        let readiness = source_between(
            HLS_PLAYER,
            "fn exact_live_codec_bootstrap_ready(",
            "async fn fetch_feed_response(",
        );
        assert!(readiness.contains("session.live_history_active"));
        assert!(readiness.contains("session.presentation_id == presentation_id"));
        assert!(readiness.contains("sequence_zero_feed_cache_key(owner, topic, presentation_id)"));
        assert!(readiness.contains("hls_sequence_zero_codec_bootstrap_is_supported("));

        let manifest = source_between(player, "fn manifest_parsed(", "fn level_loaded(");
        assert!(manifest.contains("session.load = LoadPhase::Warmup"));
        assert!(
            manifest.find("start_at(epoch, &hls, position)").unwrap()
                < manifest
                    .find("sleep(HLS_BEGINNING_AUTOPLAY_HEAD_START).await")
                    .unwrap()
        );
        let rebase = source_between(player, "fn level_loaded(", "fn dom_event(");
        let rebase_event = rebase
            .find("emit(&media, HLS_TIMELINE_REBASE_EVENT)")
            .unwrap();
        let stop = rebase.find("hls.stop_load()").unwrap();
        let defer = rebase.find("Wait::Microtask.wait().await").unwrap();
        let relaunch = rebase.find("launch(request).await").unwrap();
        assert!(rebase.contains("startTimeOffset"));
        assert!(rebase.contains("hls_exact_sequence_zero_live_target("));
        assert!(rebase.contains("session.codec_target.is_none()"));
        assert!(rebase.contains("apply_codec_bootstrap_target(epoch)"));
        assert!(rebase.contains("hls_timeline_rebase_position("));
        assert!(rebase_event < stop && stop < defer && defer < relaunch);

        let session = source_between(player, "impl Session {", "fn with_session<");
        assert!(session.contains("fn arm_codec_bootstrap("));
        assert!(session.contains("self.media.current_time()"));
        assert!(session.contains("self.codec_target = preserve_position"));

        let errors = source_between(player, "fn handle_error(", "fn hard_recovery(");
        assert!(errors.contains("sourceBufferName"));
        assert!(
            errors.find("let codec_bootstrap").unwrap()
                < errors
                    .find("if !js_bool_property(&data, \"fatal\")")
                    .unwrap()
        );
        assert!(errors.contains("let preserve_position = session.codec_required"));
        assert!(errors.contains("session.arm_codec_bootstrap(preserve_position)"));
        let hard = source_between(player, "fn hard_recovery(", "fn start_at(");
        assert!(hard.contains("session.arm_codec_bootstrap(true)"));
        assert!(hard.contains("clean_hls_codec_bootstrap_source(&session.source)"));
        assert!(!hard.contains(".split(\"&codec-bootstrap=\")"));
        let recovery = source_between(player, "fn run_recovery(", "fn autoplay(");
        assert!(recovery.contains("matches!(session.load, LoadPhase::Warmup)"));
        assert!(recovery.contains("codec_target: session.codec_target"));
        assert!(recovery.contains("force_codec_bootstrap: codec_required"));
        let hard_recovery = source_between(recovery, "Recovery::Hard(wait", "Recovery::Stop(");
        assert!(
            hard_recovery.find("wait.wait().await").unwrap()
                < hard_recovery.find("session.restart_position()").unwrap()
        );
        let autoplay = source_between(player, "fn autoplay(", "fn playback_error(");
        assert!(autoplay.contains("session.autoplay_allowed && session.media.autoplay()"));
        assert!(autoplay.contains("&& !session.codec_pending"));
        assert!(autoplay.contains("if matches!(session.load, LoadPhase::Warmup)"));

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
            "fn hls_config(progressive: bool) -> Object {",
            "fn swarm_load_policy() -> Object {",
        );

        assert!(config.contains("(30.0, 60.0, 32.0)"));
        assert!(config.contains("(90.0, 120.0, 96.0)"));
        assert!(config.contains(r#"number("maxBufferLength", length);"#));
        assert!(config.contains(r#"number("maxMaxBufferLength", maximum);"#));
        assert!(config.contains(r#"number("maxBufferSize", bytes * 1024.0 * 1024.0);"#));
        assert!(config.contains(r#"number("maxBufferHole", 1.0);"#));
        assert!(
            config.find("if !progressive").unwrap()
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
        assert!(HLS_PLAYER.contains(
            r#"const HLS_DOM_EVENTS: [&str; 4] = ["play", "playing", "pause", "resize"];"#
        ));
    }

    #[test]
    fn hls_prefetch_caps_current_generation_body_loads() {
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
        assert!(HLS_PLAYER.contains("progressive_successor_prefix_ready"));
        assert!(HLS_PLAYER.contains("mark_hls_progressive_successor_prefix_ready("));
        assert!(!HLS_PLAYER.contains("prefetch_authenticated_hls_prefix"));

        let buffered = source_between(HLS_PLAYER, "fn fragment_buffered(", "fn manifest_parsed(");
        assert!(buffered.contains("sleep(HLS_WARMUP_STOP_DELAY).await"));
        assert!(buffered.contains("!matches!(session.load, LoadPhase::Warmup)"));
        assert!(buffered.contains("!session.media.paused()"));

        let plans = source_between(
            HLS_PLAYER,
            "fn remember_hls_media_plan(",
            "fn cached_hls_payload(",
        );
        assert!(plans.contains("HLS_PLAYBACK.with"));
        assert!(HLS_PLAYER.contains(
            "pub(crate) const HLS_LIVE_HISTORY_MEDIA_PLAN_MAX_REFERENCES: usize = 16_384;"
        ));
        assert!(plans.contains("let media_sequence = hls_media_sequence(manifest);"));
        assert!(plans.contains("hls_media_plan_reference_limit("));
        assert!(plans.contains("playback.session.live_start"));
        assert!(plans.contains("playback.plans.resize(plan_limit)"));
        assert!(plans.contains("segments.len() - plan_limit"));
        assert!(plans.contains("segments.truncate(plan_limit)"));
        assert!(plans.contains("for segment in &segments"));
        assert!(plans.contains("install_with_early_overlap_limit("));
        assert!(
            plans.contains("install_with_early_overlap_limit(references, overlap, live_start)")
        );

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
            3
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
        assert!(
            load.contains("if live_history_presentation_id.is_some() {\n            return None;")
        );
        assert!(load.contains("start == HlsStart::Live && wait_for_live_frontier"));
        assert!(load.contains("get_connections().await"));
        assert!(load.contains("HLS_LIVE_FRONTIER_MIN_PRICED_PEERS"));
        assert!(load.contains("HLS_LIVE_FRONTIER_CONNECTION_WAIT"));
        assert!(
            HLS_PLAYER
                .contains("const HLS_LIVE_FRONTIER_MAX_WAIT: Duration = Duration::from_secs(15);")
        );
        assert!(load.contains("live_frontier_deadline_ms.filter(|_| start == HlsStart::Live)"));
        let persisted_live = source_between(
            load,
            "Some(index) if start == HlsStart::Live => {",
            "Some(index) if !sequence_zero_start_requested => {",
        );
        assert_eq!(
            persisted_live
                .matches("await_before_hls_live_frontier_deadline(")
                .count(),
            2
        );
        assert!(persisted_live.contains("hls_feed_payload_at_index_bounded("));
        assert!(persisted_live.contains("load_persisted_vod_payload("));
        let persisted_beginning = source_between(
            load,
            "Some(index) if !sequence_zero_start_requested => {",
            "                    None => None,",
        );
        assert!(!persisted_beginning.contains("await_before_hls_live_frontier_deadline("));
        let cold_live_startup = source_between(
            load,
            "None if !sequence_zero_start_requested => {",
            "                    None => {",
        );
        assert!(!cold_live_startup.contains("spawn_local"));
        assert_eq!(
            cold_live_startup
                .matches("latest_hls_feed_payload_startup(")
                .count(),
            2
        );
        assert!(
            cold_live_startup
                .contains("hls_live_frontier_remaining_ms(deadline_ms, js_sys::Date::now())?")
        );
        assert!(cold_live_startup.contains(
            "let connection_wait = HLS_LIVE_FRONTIER_CONNECTION_WAIT\n                                .min(Duration::from_secs_f64(remaining_ms / 1_000.0))"
        ));
        assert!(cold_live_startup.contains("Some(generation)"));
        assert!(
            cold_live_startup
                .matches("hls_live_frontier_remaining_ms(")
                .count()
                >= 3
        );
        assert!(
            cold_live_startup.contains("Some(Duration::from_secs_f64(remaining_ms / 1_000.0)),")
        );
        assert!(cold_live_startup.contains(")\n                                .await?"));
        assert!(cold_live_startup.contains(
            "hls_prefix_generation_for_feed(&weeb3, &owner, &topic)\n                                    != active_live_generation"
        ));
        assert!(cold_live_startup.contains(
            "} else {\n                            weeb3\n                                .latest_hls_feed_payload_startup("
        ));
        assert!(load.contains("latest_hls_feed_payload_startup("));
        assert_eq!(cold_live_startup.matches("topic.clone(),").count(), 2);
        let startup_search = source_between(
            HLS_PLAYER,
            "async fn latest_hls_feed_payload_startup(",
            "async fn hls_feed_payload_at_index(",
        );
        assert!(startup_search.contains("match maximum_wait"));
        assert!(startup_search.contains("async_std::future::timeout(maximum_wait, acquire).await"));
        assert!(startup_search.contains("live frontier deadline elapsed"));
        assert!(startup_search.contains("self.finish_progress("));
        assert!(
            startup_search
                .find("timeout(maximum_wait, acquire)")
                .unwrap()
                < startup_search.find("self.finish_progress(").unwrap()
        );
        let final_deadline_check = load
            .rfind("if let Some(deadline_ms) = active_live_frontier_deadline_ms")
            .unwrap();
        let cache_publish = load.rfind("store_feed_snapshot(&cache_key").unwrap();
        assert!(final_deadline_check < cache_publish);
        assert!(load[final_deadline_check..cache_publish].contains("!= active_live_generation"));
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
            "fn hls_prefix_generation_for_feed(",
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
        assert!(load.contains("start == HlsStart::Live && index_hint.is_none()"));
        assert!(!load.contains("await_live_frontier_snapshot("));
        assert_eq!(load.matches("prefetch_live_snapshot_start(").count(), 2);
        assert!(load.contains("if !feed_task_running"));
        let live_prefetch = source_between(
            HLS_PLAYER,
            "fn prefetch_live_snapshot_start(",
            "fn start_beginning_snapshot_runway(",
        );
        assert!(live_prefetch.contains("hls_live_tail(&snapshot.body)"));
        assert!(live_prefetch.contains("session.live_start && session.generation == generation"));
        assert_eq!(
            live_prefetch
                .matches("drop(start_hls_payload_load(")
                .count(),
            2
        );
        assert!(live_prefetch.contains(".take(HLS_PREFETCH_BODY_MAX_PARALLEL)"));
        let tail_prefetch = live_prefetch.find(".skip(start)").unwrap();
        let codec_prefetch = live_prefetch
            .find("hls_media_sequence(&snapshot.body) == Some(0)")
            .unwrap();
        assert!(tail_prefetch < codec_prefetch);
        assert!(live_prefetch[codec_prefetch..].contains("false,\n                generation,"));
        assert!(load.contains("start == HlsStart::Live && wait_for_live_frontier"));
        assert!(load.contains("let _ = schedule_initial_feed_stabilization("));
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
        let observed_stabilization = &stabilization[stabilization
            .find("let (observed_out")
            .expect("observed live frontier")..];
        assert!(observed_stabilization.contains("loaded.clone(),\n                true,"));
        let stabilization_task = source_between(
            HLS_PLAYER,
            "async fn stabilize_claimed_feed_route(",
            "fn schedule_initial_feed_stabilization(",
        );
        assert!(stabilization_task.contains("if network_current && observe_progress"));
        assert!(stabilization_task.contains("record_completed_frontier_scout("));
        assert!(stabilization_task.contains("confidence"));
        let completed_scout = source_between(
            HLS_PLAYER,
            "fn record_completed_frontier_scout(",
            "impl FeedRegistry",
        );
        assert!(completed_scout.contains("state.checking_token != checking_token"));
        assert!(completed_scout.contains("state.snapshot.index != observed_index"));
        assert!(completed_scout.contains("state.source_body.as_ref() != observed_source"));
        assert!(completed_scout.contains("!confidence.is_authoritative()"));
        assert!(completed_scout.contains("confidence,"));
        assert!(completed_scout.contains("let observed_at = js_sys::Date::now();"));
        assert!(completed_scout.contains("state.last_head_check = observed_at;"));
        assert!(completed_scout.contains("last_head_check: observed_at,"));
        assert!(completed_scout.contains("observed_at,"));
        assert!(completed_scout.contains("state.frontier_observation = Some("));
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
        assert!(refresh.contains("apply_feed_candidate("));
        assert!(reducer.contains("(!seeded && !is_hls_manifest(&candidate.source))"));
        assert!(reducer.contains("if candidate.head_confirmed"));
        assert!(reducer.contains("let proof_confirmed = if seeded"));
        assert!(reducer.contains("FeedRouteBodyUpdate::Publish(body) => (body, true)"));
        assert!(reducer.contains("FeedRouteBodyUpdate::Hold =>"));
        assert!(reducer.contains("existing.body_includes_source = body_includes_source"));
        assert!(reducer.contains(
            "candidate.terminal && body_includes_source && hls_is_finalized(body.as_ref())"
        ));
        assert!(reducer.contains("existing.last_head_check = js_sys::Date::now()"));
        assert!(refresh.contains("state.last_head_check > 0.0"));
        assert!(refresh.contains("state.confirmed_head_index.is_none()"));
        assert!(refresh.contains("record_completed_frontier_scout("));
        assert!(refresh.contains("confidence,"));
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
        assert!(follower.contains("if !skipped_missing_index"));
        assert!(
            !follower
                .contains("followup_mode == FeedFollowupMode::Canonical && !skipped_missing_index")
        );
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
        assert_eq!(
            beginning_wait
                .matches("retrieve_hls_payload_range(")
                .count(),
            2
        );
        assert!(HLS_PLAYER.contains(
            "HLS_PROGRESSIVE_SUCCESSOR_PREFIX_BYTES: u64 = MEDIA_STORAGE_WINDOW_BYTES * 3"
        ));
        assert!(!beginning_wait.contains("FuturesUnordered"));
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
            feed_response.contains(
                "rewrite_hls_sequence_zero_codec_bootstrap_at(&snapshot.body, true, None)"
            )
        );
        assert!(feed_response.contains("continue_hls_codec_bootstrap(&snapshot.body)"));
        assert!(feed_response.contains("continue_hls_sequence_zero_codec_bootstrap("));
        assert!(feed_response.contains("if current.snapshot.is_none()"));
        assert!(feed_response.contains("current.snapshot.clone()"));
        assert!(feed_response.contains("snapshot = selected"));
        assert!(feed_response.contains("current.discontinuity_anchor = discontinuity_anchor"));
        assert!(feed_response.contains("let sequence_zero_bootstrap = matches!("));
        assert!(feed_response.contains("hls_media_references(&body).into_iter().next()"));
        assert!(feed_response.contains("hls_prefix_generation_for_feed(&weeb3, &owner, &topic)"));
        assert!(feed_response.contains("start_hls_payload_load("));
        assert!(
            feed_response
                .contains("reference,\n                false,\n                generation,")
        );
        assert!(feed_response.contains("[HLS_EARLY_FEED_PREFIX_INDEX, 0]"));

        let early_decode = source_between(
            BZZ_STREAM,
            "pub(crate) async fn acquire_latest_raw_feed_payload_startup(",
            "pub(crate) async fn acquire_latest_raw_feed_payload_bounded_from",
        );
        assert!(early_decode.contains("seek_latest_feed_update_indexed_wide_bounded("));
        assert!(
            early_decode.contains("seek_sequence_feed_frontier_wide_bounded_observing_positive(")
        );
        assert!(!early_decode.contains("seek_latest_feed_update_indexed_observing_positive("));
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
            "async fn build_live_history(",
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
            "async fn build_live_history(",
            "fn feed_cache_key(",
        );
        assert!(
            HLS_PLAYER
                .contains("const HLS_LIVE_HISTORY_MAX_WAIT: Duration = Duration::from_secs(20);")
        );
        assert!(!live_history.contains("session.mode == HlsPrefetchMode::Sustained"));
        assert!(!live_history.contains("completed_media_payloads"));
        assert!(!live_history.contains("session.generation == generation"));
        assert!(live_history.contains("initial: FeedRouteSnapshot"));
        assert!(live_history.contains("frontier_started_ms: f64"));
        assert!(live_history.contains("frontier_deadline_ms: f64"));
        assert!(live_history.contains("let mut deadline_ms = frontier_deadline_ms;"));
        assert!(live_history.contains("deadline_ms: &mut f64"));
        assert!(live_history.contains("HLS_LIVE_HISTORY_MAX_WAIT.as_millis()"));
        assert!(live_history.contains("observation.observed_at <= *deadline_ms"));
        assert!(live_history.contains("*deadline_ms ="));
        assert!(
            live_history
                .contains("observation.observed_at + HLS_LIVE_HISTORY_MAX_WAIT.as_millis() as f64")
        );
        assert!(live_history.contains("let initial_prefix ="));
        assert!(live_history.contains("hls_media_sequence(&initial.body) == Some(0)"));
        assert!(live_history.contains("index: initial.index"));
        assert!(live_history.contains("HLS_EARLY_FEED_PREFIX_INDEX"));
        assert!(live_history.contains("HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT"));
        assert!(
            live_history.contains("hls_live_history_active_parallelism(false, prefix_work_items)")
        );
        assert!(
            live_history
                .contains("hls_live_history_active_parallelism(frontier_frozen, fallback_limit)")
        );
        assert!(
            live_history.contains(
                "hls_live_history_active_parallelism(frontier_frozen, batch_indices.len())"
            )
        );
        assert!(live_history.contains("await_feed_probe_wave_credit("));
        assert!(live_history.contains("hls_live_history_feed_payload_at_index("));
        assert!(!live_history.contains("hls_feed_payload_at_index_bounded("));
        assert!(live_history.contains("let mut fallback_remaining = fallback_limit;"));
        assert!(live_history.matches("refresh_frozen_frontier(").count() >= 2);
        assert!(
            live_history
                .matches("refresh_frozen_frontier(&mut frozen_frontier, &mut deadline_ms)")
                .count()
                >= 2
        );
        assert!(live_history.contains(".rev()"));
        assert!(live_history.contains("expected_index <= current_index"));
        assert!(!live_history.contains(".collect::<Vec<_>>()"));
        assert!(live_history.contains(".buffered(prefix_parallelism)"));
        assert!(live_history.contains(".buffered(batch_parallelism)"));
        assert_eq!(
            live_history
                .matches("await_before_hls_live_frontier_deadline(")
                .count(),
            2
        );
        assert!(live_history.contains("candidates.next(),"));
        assert!(live_history.contains("batch.next(),"));
        assert!(!live_history.contains("candidates.next().await"));
        assert!(!live_history.contains("batch.next().await"));
        assert!(live_history.contains("already-dispatched accounting attempts continue to drain"));
        assert!(live_history.contains("transports retain their accounting settlement path"));
        assert!(live_history.contains("if batch_failed {\n                    return false;"));
        assert!(live_history.contains("append_hls_sequence_zero_archive_suffix("));
        assert!(live_history.contains("raise_hls_target_duration(&mut archive, target_duration)"));
        assert!(live_history.contains("let mut frozen_frontier = None"));
        assert!(live_history.contains("if frozen_frontier.is_none() && frontier_idle"));
        assert!(live_history.contains("hls_live_frontier_observation_is_authoritative("));
        assert!(live_history.contains("observation.confidence"));
        assert!(live_history.contains("current_index == observation.index"));
        assert!(live_history.contains("observation.observed_at,"));
        assert!(live_history.contains("frontier_started_ms,"));
        assert!(live_history.contains("current_source.as_slice() == observation.source.as_ref()"));
        let history_publish = live_history.find("cache.insert(").unwrap();
        let final_history_guard = live_history[..history_publish]
            .rfind("if weeb3.get_network_id().await != network_id")
            .unwrap();
        assert!(final_history_guard < history_publish);
        assert!(
            live_history[final_history_guard..history_publish].contains("session_is_current()")
        );
        assert!(
            live_history[final_history_guard..history_publish].contains("get_network_id().await")
        );
        assert!(
            live_history[final_history_guard..history_publish]
                .contains("hls_live_frontier_remaining_ms(deadline_ms, js_sys::Date::now())")
        );
        assert!(live_history.contains("body_includes_source: true"));
        assert!(live_history.contains("session.live_history_active = true"));
        assert_eq!(
            HLS_PLAYER
                .matches("session.live_history_active = true")
                .count(),
            1
        );
        assert_eq!(live_history.matches("cache.insert(").count(), 1);
        assert!(!live_history.contains("extend_hls_sequence_zero_archive("));
        assert!(!live_history.contains("store_feed_snapshot("));
        assert!(!live_history.contains("remember_hls_media_plan("));
        assert!(!live_history.contains("start_hls_payload_load("));
        assert!(!live_history.contains("prefetch_live_snapshot_start("));

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
        let current = attach
            .find("let current_snapshot = FEED_ROUTE_CACHE.with")
            .unwrap();
        let play = attach.find("play_hls(").unwrap();
        assert!(beginning_runway < overlap && overlap < current && current < play);
        let history = attach.find("build_live_history(").unwrap();
        assert!(current < history && history < play);
        assert!(!attach[play..].contains("build_live_history("));
        assert!(attach.contains("let history_ready = match snapshot.as_ref()"));
        assert!(!attach.contains("let _ = build_live_history("));
        assert!(
            attach.contains("initial.finalized && hls_media_sequence(&initial.body) == Some(0)")
        );
        assert!(attach.contains("initial.clone(),"));
        assert!(attach.contains("live_frontier_started_ms,"));
        assert!(attach.contains("live_frontier_deadline_ms,"));
        let worker_after_snapshot = source_between(
            attach,
            "Either::Right((snapshot, worker_ready)) => {",
            "        if !result_view_request_is_current(view_generation)",
        );
        assert!(worker_after_snapshot.contains("if start == HlsStart::Live"));
        assert!(worker_after_snapshot.contains("if snapshot.is_none()"));
        assert!(worker_after_snapshot.contains("drop(worker_ready);"));
        assert!(worker_after_snapshot.contains(
            "await_before_hls_live_frontier_deadline(\n                            Some(live_frontier_deadline_ms),"
        ));
        assert!(worker_after_snapshot.contains("Some(true) => snapshot"));
        assert!(worker_after_snapshot.contains("None => None"));
        let beginning_worker = &worker_after_snapshot[worker_after_snapshot
            .find("} else {\n                    if !worker_ready.await")
            .unwrap()..];
        assert!(beginning_worker.contains("if !worker_ready.await"));
        assert!(!beginning_worker.contains("await_before_hls_live_frontier_deadline("));
        let failed_history = attach.find("if !history_ready").unwrap();
        let invalidate = attach[failed_history..]
            .find("invalidate_hls_prefetch_session();")
            .map(|offset| failed_history + offset)
            .unwrap();
        let stable_timeline_error = attach[failed_history..]
            .find("Could not establish a stable live timeline before the deadline")
            .map(|offset| failed_history + offset)
            .unwrap();
        assert!(history < failed_history && failed_history < invalidate);
        assert!(invalidate < stable_timeline_error && stable_timeline_error < play);
        assert!(!load.contains("await_live_frontier_snapshot("));
        assert!(!attach.contains("await_live_frontier_snapshot("));
        assert!(attach[current..play].contains("prefetch_live_snapshot_start("));
        let lifecycle = source_between(
            HLS_PLAYER,
            "fn install_hls_prefetch_lifecycle(",
            "pub(crate) fn release_hls_view()",
        );
        assert!(lifecycle.contains("retain_media_element_callback(player, \"seeking\""));
        assert!(lifecycle.contains("retain_media_element_callback(player, \"seeked\""));
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
        assert!(STATIC_WORKER.contains("const SERVICE_WORKER_PROTOCOL = 5;"));
        assert!(INTERFACE_RUNTIME.contains("const SERVICE_WORKER_PROTOCOL: f64 = 5.0;"));
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
        assert!(setup.contains("SERVICE_WORKER_SETUP_LOCK.with(std::rc::Rc::clone)"));
        assert!(setup.contains("setup_lock.lock().await"));

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
        assert!(response.contains("let runway_stamp = HLS_PLAYBACK.with"));
        assert!(response.contains("current_hls_progressive_foreground(&reference)"));
        assert!(response.contains("let progressive_start = progressive_stamp.is_some();"));
        assert!(response.contains("progressive_stamp.map(|stamp| stamp.generation)"));
        assert!(response.contains("session.progressive_current(&reference, stamp)"));
        assert!(HLS_STREAM.contains("successor_prefix_started"));
        assert!(HLS_STREAM.contains("runway.first != reference"));
    }

    #[test]
    fn non_live_active_plan_full_gets_use_strict_progressive_ranges() {
        let selector = source_section(
            HLS_STREAM,
            "fn hls_progressive_foreground_allowed(",
            "fn hls_generation_current(",
        );
        assert!(selector.contains("session.mode != HlsPrefetchMode::Inactive"));
        assert!(selector.contains("!session.live_start"));
        assert!(selector.contains("!session.timeline_rebasing"));
        assert!(selector.contains(".get(&selection.cursor.plan.id)"));
        assert!(selector.contains("track.last_foreground_position != cursor.position"));
        assert!(selector.contains("track.schedule_id == 0"));

        let response = source_section(
            HLS_STREAM,
            "async fn fetch_hls_bytes_response(",
            "fn hls_bytes_headers(",
        );
        let runway = response
            .find("&& range.is_none() && progressive_start")
            .unwrap();
        let active = response
            .find("hls_progressive_foreground_candidate(&reference)")
            .unwrap();
        let context = response[active..]
            .find("hls_foreground_context(&reference, false)")
            .unwrap()
            + active;
        let admitted = response[context..]
            .find("current_hls_progressive_foreground_for_context")
            .unwrap()
            + context;
        let resolve = response[admitted..]
            .find("resolve_hls_asset(weeb3.clone(), reference.clone()).await")
            .unwrap()
            + admitted;
        let revalidated = response[resolve..]
            .find("current_hls_progressive_foreground_for_context")
            .unwrap()
            + resolve;
        let stream = response[revalidated..]
            .find("return FetchResponse::stream(200, headers)")
            .unwrap()
            + revalidated;
        assert!(
            runway < active
                && active < context
                && context < admitted
                && admitted < resolve
                && resolve < revalidated
                && revalidated < stream
        );
        assert!(response.contains("hls_progressive_media_body_absent(&reference)"));
        assert!(response.contains("!resolved.metadata.is_manifest"));
        assert!(response.contains("resolved.prefetched_body.is_none()"));
        assert!(!response.contains("spawn_hls_exact_next_overlap("));
        assert!(response.contains("runway_stamp.is_none()"));
        assert!(response.contains("restart_hls_progressive_prefetch_stages("));
        assert!(!response.contains("body_pending.remove"));
    }

    #[test]
    fn cross_plan_progressive_seek_rotates_one_proven_timeline_atomically() {
        let track = source_section(
            HLS_STREAM,
            "struct HlsPrefetchTrack {",
            "#[derive(Clone, Copy, PartialEq, Eq)]",
        );
        assert!(track.contains("last_foreground_reference: String"));

        let runtime = source_section(
            HLS_STREAM,
            "fn hls_cross_plan_seek_transition_for_session(",
            "fn hls_progressive_foreground_candidate(",
        );
        assert!(runtime.contains("!session.seek_pending"));
        assert!(runtime.contains("session.tracks.contains_key(&cursor.plan.id)"));
        assert!(runtime.contains(".filter(|track| track.schedule_id != 0)"));
        assert!(runtime.contains(".max_by_key(|track| track.last_touch)"));
        assert!(runtime.contains("&previous.last_foreground_reference"));
        assert!(runtime.contains("hls_cross_plan_seek_transition("));
        assert!(!runtime.contains(".await"));

        let candidate = source_section(
            HLS_STREAM,
            "fn hls_progressive_foreground_candidate(",
            "fn current_hls_progressive_foreground(",
        );
        assert!(candidate.contains("track.schedule_id != 0"));
        assert!(
            candidate.contains(
                "|| hls_cross_plan_seek_transition_for_session(session, &selection.cursor)"
            )
        );
        assert!(!candidate.contains(".await"));

        let context = source_section(
            HLS_STREAM,
            "fn hls_foreground_context(",
            "fn hls_prefetch_track_positions(",
        );
        let proof = context
            .find("hls_cross_plan_seek_transition_for_session(session, cursor)")
            .unwrap();
        let generation = context[proof..]
            .find("let generation = session.advance_generation();")
            .unwrap()
            + proof;
        let clear = context[generation..]
            .find("session.tracks.clear();")
            .unwrap()
            + generation;
        let insert = context[clear..].find("session.tracks.insert(").unwrap() + clear;
        assert!(proof < generation && generation < clear && clear < insert);
        assert!(
            context
                .contains("cross_plan_seek || transition.is_some_and(|transition| transition.0)")
        );
        assert!(context.contains(".min(HLS_TWO_BODY_PREFETCH_COMPLETIONS)"));
        assert!(context.contains("cursor.position.saturating_add(1)"));
        assert!(context.contains("map(|client| (client, generation))"));
        assert!(context.contains("last_foreground_reference:"));
        assert!(context.contains("let accepted_position = transition"));
        assert!(context.contains("references.get(accepted_position)"));
        assert!(!context.contains(".await"));
    }

    #[test]
    fn progressive_restart_waits_for_bounded_contiguous_range_coverage() {
        let coverage = source_section(
            HLS_STREAM,
            "pub(crate) const HLS_PROGRESSIVE_RANGE_COVERAGE_MAX_WINDOWS",
            "#[derive(Clone, Debug, Eq, PartialEq)]\nstruct HlsSegmentIdentity",
        );
        assert!(coverage.contains("ranges: BTreeMap<u64, u64>"));
        assert!(coverage.contains("contiguous_end: u64"));
        assert!(coverage.contains("start % window_size != 0"));
        assert!(coverage.contains("expected_windows > HLS_PROGRESSIVE_RANGE_COVERAGE_MAX_WINDOWS"));
        assert!(
            coverage.contains("self.ranges.len() >= HLS_PROGRESSIVE_RANGE_COVERAGE_MAX_WINDOWS")
        );
        assert!(coverage.contains("self.ranges.remove(&self.contiguous_end)"));
        assert!(coverage.contains("self.contiguous_end == size"));

        let response = source_section(
            HLS_STREAM,
            "async fn fetch_hls_bytes_response(",
            "fn hls_bytes_headers(",
        );
        let arm = response
            .find("begin_hls_progressive_range_coverage(")
            .unwrap();
        let stream = response[arm..]
            .find("return FetchResponse::stream(200, headers)")
            .unwrap()
            + arm;
        let complete = response
            .find("complete_hls_progressive_range(&reference, stamp, size, start, end)")
            .unwrap();
        let restart = response[complete..]
            .find("restart_hls_progressive_prefetch_stages(")
            .unwrap()
            + complete;
        assert!(arm < stream && complete < restart);
        assert!(
            response
                .matches("fail_hls_progressive_range_coverage(")
                .count()
                >= 3
        );

        let session = source_section(
            HLS_STREAM,
            "impl HlsPrefetchSession {",
            "struct PendingHlsPayload",
        );
        assert!(
            session
                .matches("clear_hls_progressive_range_coverage();")
                .count()
                >= 2
        );

        let context = source_section(
            HLS_STREAM,
            "fn hls_foreground_context(",
            "fn hls_prefetch_track_positions(",
        );
        assert!(context.contains("clear_hls_progressive_range_coverage_unless(&reference, stamp)"));

        let completion = source_section(
            HLS_STREAM,
            "fn complete_hls_progressive_range(",
            "fn hls_cross_plan_seek_transition_for_session(",
        );
        assert!(completion.contains("== HlsProgressiveRangeUpdate::Complete"));
        assert!(!completion.contains(".await"));
    }

    #[test]
    fn progressive_foreground_reuses_bounded_exact_next_scheduling() {
        let overlap = source_section(
            HLS_STREAM,
            "fn spawn_hls_exact_next_overlap(",
            "fn hls_prefetch_ticket_current(",
        );
        assert!(overlap.contains(".min(HLS_EXACT_NEXT_OVERLAP_SEGMENTS)"));
        assert!(overlap.contains("HLS_EXACT_NEXT_HEAD_START"));
        assert!(overlap.contains("HLS_NEXT_RESERVE_STAGGER"));
        assert!(overlap.contains("HLS_EXACT_OVERLAP_ADMISSION_BUDGET"));
        assert!(overlap.contains("hls_playback_prefetch_admission_is_current(ticket)"));
        assert!(overlap.contains("ticket.stamp.generation"));

        let retrieve = source_section(
            HLS_STREAM,
            "async fn retrieve_hls_payload_for_playback(",
            "async fn resolve_hls_asset(",
        );
        assert!(retrieve.contains("spawn_hls_exact_next_overlap("));

        let restart = source_section(
            HLS_STREAM,
            "fn restart_hls_progressive_prefetch_stages(",
            "async fn hls_payload_size_for_prefetch(",
        );
        assert!(restart.contains("current_hls_progressive_foreground(&reference)"));
        assert!(restart.contains("if ticket.stamp != stamp"));
        assert!(restart.contains("ready_out.try_send(true)"));
        assert!(restart.contains("spawn_hls_prefetch_stages("));
    }

    #[test]
    fn hls_chunk_timeouts_keep_accounting_and_raw_singleflight_draining() {
        let attempt = source_section(
            RETRIEVAL,
            "async fn retrieve_attempt(",
            "fn chunk_address_parts(",
        );
        assert!(attempt.contains("failed_retrieve_attempt(&peer, false)"));
        assert!(attempt.contains("result_chan.try_send(terminal_result)"));

        let chunk = source_section(
            RETRIEVAL,
            "pub async fn retrieve_chunk(",
            "pub async fn retrieve_check_chunk(",
        );
        assert_eq!(chunk.matches("if !result.terminal").count(), 2);
        assert!(chunk.contains(".unwrap_or(RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS)"));
        assert!(chunk.contains(".clamp(1, RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS)"));
        assert!(chunk.contains("attempt_count < max_attempt_errors"));

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
        assert!(!clear.contains("pending_ranges"));

        let begin_hls = hls_source_section(
            "fn begin_hls_prefetch_session(",
            "fn set_hls_prefetch_mode(",
        );
        assert!(begin_hls.contains("clear_completed_bzz_media_ranges()"));
        assert!(begin_hls.contains("cache.borrow_mut().retain_completed = true"));
        assert!(!begin_hls.contains("body_pending.clear"));
        assert!(!begin_hls.contains("sequence_zero_start_requested"));

        let window = source_section(
            "async fn read_range_window(",
            "fn spawn_prefetch_media_stages(",
        );
        assert!(window.contains("&stream_key"));
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
