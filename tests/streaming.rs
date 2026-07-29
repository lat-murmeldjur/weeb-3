#![allow(dead_code)]

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
    use crate::stream_hls;

    use std::collections::{HashMap, VecDeque};
    use stream_hls::{
        HLS_LIVE_SYNC_DURATION_COUNT, HlsEarlyPrefixPolicy, HlsLevelTransition, HlsManifestProbe,
        HlsMediaPlanRegistry, HlsOrderedProbeWindow, HlsTrackRetention,
        MAX_STREAM_FEED_PAYLOAD_BYTES, classify_hls_level_transition,
        extend_hls_sequence_zero_archive, hls_foreground_cursor_transition, hls_is_finalized,
        hls_manifest_reload_is_continuous, hls_media_references, hls_media_sequence,
        hls_payload_mime, hls_prefix_admission_window_is_open, hls_prefix_stagger_remaining_ms,
        hls_startup_prefix_is_preferred, hls_timeline_rebase_required, hls_track_ids_to_prune,
        is_hls_manifest, parse_stream_catalog, probe_hls_manifest, read_forward_cache_entry,
        rewrite_hls_manifest, rewrite_hls_manifest_for_live_reload,
        stream_feed_payload_len_is_supported,
    };

    const REF: &str = "919b5395bf7a59cbb3b365769de09a2b15ac5d897823dda9270259a3c038d574";
    const REF2: &str = "49428dc8819f560aa3e6226a8c1036a25c091a51d5745de381b842f73243f6d9";
    const REF3: &str = "14aec3fbbb36882d4eba4881fdaa6f2336e5d600b133d677e3f3f5c9d54d8290";
    const REF4: &str = "68d3d40b39d5f17532e928a4b62f2a58ea1b63e20da0eb4b8a7da78d45d45812";
    const OWNER: &str = "352eabdea9cb05e984a8828d2a6df3d3b5023260";
    const TOPIC: &str = "cfbbc155d709547b198638d0fb11d733359561538d8bd606a9ab257354d13bcc";

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
        assert!(hls_timeline_rebase_required(636, true, 0, false));
        assert!(
            hls_timeline_rebase_required(636, true, 0, true),
            "a provisionally exposed archive must rebase before ENDLIST confirmation"
        );
        assert!(
            !hls_timeline_rebase_required(0, true, 0, false),
            "a sequence-zero live playlist can become finite without moving its origin"
        );
        assert!(
            !hls_timeline_rebase_required(636, true, 637, false),
            "a normal forward terminal update keeps the buffered timeline"
        );
        assert!(
            !hls_timeline_rebase_required(636, false, 0, false),
            "an already finite representation must not repeatedly rebase"
        );
    }

    #[test]
    fn rolling_archive_rebases_once_across_a_clean_session_boundary() {
        let none = HlsLevelTransition {
            rebase: false,
            terminal_ready: false,
        };
        let rebase = HlsLevelTransition {
            rebase: true,
            terminal_ready: false,
        };
        let terminal = HlsLevelTransition {
            rebase: false,
            terminal_ready: true,
        };

        // First hls.js session: a rolling window expands backwards to the full
        // provisional archive. A pending network recovery must not consume or
        // suppress that representation correction.
        assert_eq!(
            classify_hls_level_transition(None, false, false, 501, true),
            none
        );
        assert_eq!(
            classify_hls_level_transition(Some((501, true)), false, true, 0, true),
            rebase
        );

        // The clean replacement session carries only the one-shot rebase guard;
        // rendition snapshots start empty. ENDLIST then announces terminal
        // readiness without rebuilding the same sequence-zero timeline again.
        assert_eq!(
            classify_hls_level_transition(None, true, false, 0, true),
            none
        );
        assert_eq!(
            classify_hls_level_transition(Some((0, true)), true, false, 0, false),
            terminal
        );
    }

    #[test]
    fn early_prefix_stagger_counts_time_already_spent_waiting_for_feed_growth() {
        assert_eq!(hls_prefix_stagger_remaining_ms(2_000, None, None), 0);
        assert_eq!(
            hls_prefix_stagger_remaining_ms(2_000, Some(1_000.0), None),
            2_000
        );
        assert_eq!(
            hls_prefix_stagger_remaining_ms(2_000, Some(1_000.0), Some(999.0)),
            2_000
        );
        assert_eq!(
            hls_prefix_stagger_remaining_ms(2_000, Some(1_000.0), Some(1_500.25)),
            1_500
        );
        assert_eq!(
            hls_prefix_stagger_remaining_ms(2_000, Some(1_000.0), Some(3_000.0)),
            0
        );
        assert_eq!(
            hls_prefix_stagger_remaining_ms(2_000, Some(1_000.0), Some(8_600.0)),
            0
        );
        assert_eq!(
            hls_prefix_stagger_remaining_ms(2_000, Some(2_284.0), Some(7_921.0)),
            0
        );
        assert_eq!(
            hls_prefix_stagger_remaining_ms(2_000, Some(2_284.0), Some(3_000.0)),
            1_284
        );
    }

    #[test]
    fn early_prefix_window_stays_finite_until_playback_and_closes_on_generation_change() {
        assert!(hls_prefix_admission_window_is_open(
            true,
            false,
            Some(14_999.0),
            15_000.0
        ));
        assert!(!hls_prefix_admission_window_is_open(
            true,
            false,
            Some(15_000.0),
            15_000.0
        ));
        assert!(hls_prefix_admission_window_is_open(
            true,
            true,
            Some(60_000.0),
            15_000.0
        ));
        assert!(!hls_prefix_admission_window_is_open(
            false,
            true,
            Some(1_000.0),
            15_000.0
        ));
        assert!(!hls_prefix_admission_window_is_open(
            true, false, None, 15_000.0
        ));
        assert!(!hls_prefix_admission_window_is_open(
            true,
            false,
            Some(f64::NAN),
            15_000.0
        ));
        assert!(!hls_prefix_admission_window_is_open(
            true,
            false,
            Some(1_000.0),
            f64::INFINITY
        ));
    }

    #[test]
    fn authenticated_sequence_zero_startup_prefix_requires_an_unindexed_stream_route_intent() {
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
            true,
        ));

        let rolling_tentative_endlist =
            format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:636\n#EXTINF:2.0,\n{REF3}\n#EXT-X-ENDLIST\n");
        assert!(
            hls_startup_prefix_is_preferred(
                rolling_tentative_endlist.as_bytes(),
                prefix.as_bytes(),
                4,
                true,
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
                hls_startup_prefix_is_preferred(canonical.as_bytes(), prefix.as_bytes(), 4, true),
                "the direct unindexed /stream route must start from zero even when the producer leaves its rolling snapshot live, EVENT, or untagged"
            );
        }
        assert!(
            !hls_startup_prefix_is_preferred(rolling_live.as_bytes(), prefix.as_bytes(), 4, false,),
            "catalog/feed views without direct /stream intent must retain their rolling window"
        );

        let sequence_zero_vod = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXTINF:2.0,\n{REF3}\n"
        );
        assert!(
            !hls_startup_prefix_is_preferred(
                sequence_zero_vod.as_bytes(),
                prefix.as_bytes(),
                4,
                true,
            ),
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
            true,
        ));

        let master_prefix = format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\n{REF}.m3u8\n");
        assert!(
            !hls_startup_prefix_is_preferred(
                rolling_vod.as_bytes(),
                master_prefix.as_bytes(),
                4,
                true,
            ),
            "a multivariant manifest is not an authenticated media prefix"
        );
    }

    fn references(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn rolling_prefix_policy_keeps_two_ordered_lanes_and_refills_only_after_success() {
        let mut policy = HlsEarlyPrefixPolicy::new(2, 4);

        assert!(policy.observe(&references(&["a", "b", "c", "d", "e"])));
        let a = policy.next_admission(true).expect("first admission");
        assert_eq!((a.reference.as_str(), a.rolling), ("a", false));
        let b = policy.next_admission(true).expect("second active lane");
        assert_eq!((b.reference.as_str(), b.rolling), ("b", false));
        assert_eq!(policy.active_count(), 2);
        assert_eq!(policy.next_admission(true), None);

        policy.complete("a", true);
        let c = policy.next_admission(true).expect("first ordered refill");
        assert_eq!((c.reference.as_str(), c.rolling), ("c", true));
        assert_eq!(policy.active_count(), 2);
        assert_eq!(policy.next_admission(true), None);

        policy.complete("b", true);
        let d = policy.next_admission(true).expect("second ordered refill");
        assert_eq!((d.reference.as_str(), d.rolling), ("d", true));
        assert_eq!(policy.active_count(), 2);
        policy.complete("c", true);
        policy.complete("d", true);

        assert!(policy.target_complete());
        assert_eq!(policy.active_count(), 0);
        assert_eq!(policy.next_admission(true), None);
    }

    #[test]
    fn rolling_prefix_policy_never_retries_duplicates_or_rejected_admissions() {
        let mut policy = HlsEarlyPrefixPolicy::new(1, 4);
        assert!(policy.observe(&references(&["a", "a", "b", "c", "d"])));

        let a = policy.next_admission(true).expect("first A attempt");
        assert_eq!(a.reference, "a");
        policy.complete("a", true);
        let b = policy
            .next_admission(true)
            .expect("duplicate A must be skipped");
        assert_eq!(b.reference, "b");
        assert_eq!(policy.next_admission(true), None);

        policy.complete("b", true);
        let c = policy.next_admission(true).expect("C attempt");
        assert_eq!(c.reference, "c");
        policy.complete("c", true);
        assert!(policy.target_complete());
        assert_eq!(policy.next_admission(true), None);
    }

    #[test]
    fn rolling_prefix_policy_stops_after_failure_or_permanent_rejection() {
        let mut failed = HlsEarlyPrefixPolicy::new(1, 4);
        assert!(failed.observe(&references(&["a", "b", "c", "d"])));
        assert_eq!(
            failed.next_admission(true).expect("A attempt").reference,
            "a"
        );
        failed.complete("a", false);
        assert_eq!(failed.next_admission(true), None);
        assert!(!failed.target_complete());

        let mut rejected = HlsEarlyPrefixPolicy::new(1, 4);
        assert!(rejected.observe(&references(&["a", "b", "c", "d"])));
        assert_eq!(
            rejected
                .next_admission(true)
                .expect("tentative A admission")
                .reference,
            "a"
        );
        rejected.reject("a");
        assert_eq!(rejected.next_admission(true), None);
        assert!(!rejected.observe(&references(&["a", "b", "c", "d"])));
    }

    #[test]
    fn rolling_prefix_policy_ignores_shorter_or_incompatible_observations() {
        let mut policy = HlsEarlyPrefixPolicy::new(1, 4);
        assert!(policy.observe(&references(&["a", "b"])));
        assert!(!policy.observe(&references(&["a"])));
        assert!(!policy.observe(&references(&["a", "x", "c"])));
        assert!(policy.observe(&references(&["a", "b", "c", "d"])));
        assert_eq!(
            policy.next_admission(true).expect("canonical A").reference,
            "a"
        );
        policy.complete("a", true);
        assert_eq!(
            policy.next_admission(true).expect("canonical B").reference,
            "b"
        );
        policy.complete("not-active", true);
        assert_eq!(policy.next_admission(true), None);
    }

    #[test]
    fn rolling_prefix_policy_closes_permanently_when_admission_expires() {
        let mut policy = HlsEarlyPrefixPolicy::new(1, 4);
        assert!(policy.observe(&references(&["a", "b", "c", "d"])));
        assert_eq!(
            policy
                .next_admission(true)
                .expect("initial admission")
                .reference,
            "a"
        );
        assert_eq!(policy.next_admission(false), None);
        policy.complete("a", true);
        assert_eq!(policy.next_admission(true), None);
        assert_eq!(policy.active_count(), 0);
    }

    #[test]
    fn ordered_probe_window_overlaps_sizes_without_reordering_body_admission() {
        let mut window = HlsOrderedProbeWindow::new(1);
        assert_eq!(window.fill_positions(10, 6), vec![1, 2, 3, 4, 5, 6]);

        for position in [6, 4, 3, 5] {
            window.complete(position, Some(position as u64 * 100));
        }
        assert_eq!(window.next_ready(), None);

        window.complete(1, Some(100));
        assert_eq!(window.next_ready(), Some((1, Some(100))));
        assert_eq!(window.commit_ready(), Some((1, Some(100))));
        assert_eq!(window.fill_positions(10, 6), vec![7]);
        assert_eq!(window.next_ready(), None);

        window.complete(2, Some(200));
        for expected in 2..=6 {
            assert_eq!(
                window.commit_ready(),
                Some((expected, Some(expected as u64 * 100)))
            );
        }
        assert_eq!(window.fill_positions(10, 6), vec![8, 9]);
    }

    #[test]
    fn foreground_reads_demote_past_media_but_keep_duplicate_reads_local() {
        let mut order =
            VecDeque::from(["next".to_string(), "later".to_string(), "past".to_string()]);
        let mut entries = HashMap::from([
            ("past".to_string(), vec![0]),
            ("next".to_string(), vec![1]),
            ("later".to_string(), vec![2]),
        ]);

        assert_eq!(
            read_forward_cache_entry(&mut order, &entries, "past"),
            Some(vec![0])
        );
        assert_eq!(
            read_forward_cache_entry(&mut order, &entries, "past"),
            Some(vec![0]),
            "a duplicate foreground request must remain a cache hit"
        );
        order.push_back("new-lookahead".to_string());
        entries.insert("new-lookahead".to_string(), vec![3]);

        while entries.len() > 3 {
            let oldest = order.pop_front().unwrap();
            entries.remove(&oldest);
        }
        assert_eq!(
            order,
            VecDeque::from([
                "next".to_string(),
                "later".to_string(),
                "new-lookahead".to_string()
            ])
        );
        assert!(!entries.contains_key("past"));
        assert!(entries.contains_key("next"));
    }

    #[test]
    fn cached_back_reads_do_not_rewind_forward_hls_prefetch() {
        assert_eq!(
            hls_foreground_cursor_transition(14, 0, true),
            (false, 14),
            "a cached validator or back-read must retain the forward cursor"
        );
        assert_eq!(
            hls_foreground_cursor_transition(14, 13, true),
            (false, 14),
            "an adjacent cached retry must not regress the cursor"
        );
        assert_eq!(hls_foreground_cursor_transition(14, 14, true), (false, 14));
        assert_eq!(hls_foreground_cursor_transition(14, 15, true), (false, 15));
        assert_eq!(
            hls_foreground_cursor_transition(14, 16, true),
            (true, 16),
            "a non-adjacent forward request remains a real seek"
        );
        assert_eq!(
            hls_foreground_cursor_transition(14, 0, false),
            (true, 0),
            "an uncached backward request must pivot lookahead"
        );
    }

    #[test]
    fn live_plan_overlap_migrates_before_the_first_appended_fragment() {
        let mut plans = HlsMediaPlanRegistry::new(64);
        plans.install(["a", "b", "c"].into_iter().map(str::to_string).collect());
        let first = plans.cursor("a", &HashMap::new()).unwrap();
        let old_plan = first.cursor.plan_id;

        plans.install(
            ["b", "c", "d", "e"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
        let preferred = HashMap::from([(old_plan, 0)]);
        let migrated = plans.cursor("b", &preferred).unwrap();

        assert_ne!(migrated.cursor.plan_id, old_plan);
        assert_eq!(migrated.cursor.references.as_ref(), ["b", "c", "d", "e"]);
        assert_eq!(migrated.cursor.position, 0);
        assert_eq!(migrated.superseded_plan_ids, vec![old_plan]);
    }

    #[test]
    fn plan_migration_rejects_an_unrelated_rendition_with_only_one_shared_asset() {
        let mut plans = HlsMediaPlanRegistry::new(64);
        plans.install(
            ["a", "shared", "c"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
        let old_plan = plans
            .cursor("shared", &HashMap::new())
            .unwrap()
            .cursor
            .plan_id;
        plans.install(
            ["x", "shared", "z"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );

        let selected = plans
            .cursor("shared", &HashMap::from([(old_plan, 1)]))
            .unwrap();
        assert_eq!(selected.cursor.plan_id, old_plan);
        assert!(selected.superseded_plan_ids.is_empty());
    }

    #[test]
    fn duplicate_playlist_polls_do_not_create_artificial_plans() {
        let mut plans = HlsMediaPlanRegistry::new(64);
        let references = ["a", "b", "c"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        plans.install(references.clone());
        let first = plans.cursor("b", &HashMap::new()).unwrap().cursor.plan_id;
        plans.install(references);
        let second = plans.cursor("b", &HashMap::new()).unwrap().cursor.plan_id;
        assert_eq!(first, second);
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
        );
        plans.install_with_early_overlap_limit(
            ["archive-a", "archive-b", "archive-c", "archive-d"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            3,
        );

        assert_eq!(
            plans
                .cursor("rolling-a", &HashMap::new())
                .unwrap()
                .cursor
                .early_overlap_limit,
            1
        );
        assert_eq!(
            plans
                .cursor("archive-a", &HashMap::new())
                .unwrap()
                .cursor
                .early_overlap_limit,
            3
        );
    }

    #[test]
    fn stale_live_tracks_are_bounded_and_superseded_running_tracks_retire() {
        let tracks = (0_u64..100)
            .map(|plan_id| HlsTrackRetention {
                plan_id,
                last_touch: plan_id,
                running: plan_id == 0,
            })
            .collect::<Vec<_>>();
        let pruned = hls_track_ids_to_prune(&tracks, 99, &[0], 16);
        assert!(
            pruned.contains(&0),
            "a superseded observer must lose admission"
        );
        assert!(
            !pruned.contains(&99),
            "the selected live plan stays protected"
        );
        assert_eq!(tracks.len() - pruned.len(), 16);
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
        assert_eq!(HLS_LIVE_SYNC_DURATION_COUNT, 5);
        let historical = format!(
            "#EXTM3U\r\n#EXT-X-PLAYLIST-TYPE: VOD  \r\n#EXT-X-MEDIA-SEQUENCE:35\r\n\
             #EXTINF:2.0,\r\nhttps://swarm.beebridge.buzz/read/bytes/{REF}\r\n\
             #EXT-X-ENDLIST\r\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(historical.as_bytes(), "/weeb-3/hls/bytes", false)
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
            rewrite_hls_manifest_for_live_reload(sliding.as_bytes(), "/weeb-3/hls/bytes", false)
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
    fn confirmed_final_unindexed_manifest_retains_endlist() {
        let finalized = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXTINF:2.0,\n{REF}\n#EXT-X-ENDLIST\n"
        );
        let rewritten = String::from_utf8(
            rewrite_hls_manifest_for_live_reload(finalized.as_bytes(), "/weeb-3/hls/bytes", true)
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

        let conflicting = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:6\n#EXTINF:2.0,\n{REF3}\n#EXTINF:2.0,\n{REF2}\n"
        );
        assert!(!hls_manifest_reload_is_continuous(
            old.as_bytes(),
            conflicting.as_bytes()
        ));

        let regressed = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2.0,\n{REF}\n");
        assert!(!hls_manifest_reload_is_continuous(
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

        let rewritten = rewrite_hls_manifest_for_live_reload(&merged, "/weeb-3/hls/bytes", false)
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
        // RFC 8216 permits ENDLIST anywhere in a Media Playlist. This candidate
        // places it before the overlap, so suffix-only splicing must relocate it
        // instead of losing the finite-duration signal.
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

    #[test]
    fn parses_current_and_upstream_catalog_shapes_and_sorts_live_first() {
        let wrapped = br#"{
          "entries": [
            {"owner":"6F2728386F8a47ef5EBe323721188e630Ff0FdE9","topic":"vod-topic","title":"Past","state":"vod","mediaType":"video","index":69,"updatedAt":100,"duration":138.033},
            {"owner":"0X6F2728386F8a47ef5EBe323721188e630Ff0FdE9","topic":"live-topic","title":"Now","state":"live","mediatype":"audio","index":70,"updatedAt":90,"duration":"2.5"}
          ]
        }"#;
        let entries = parse_stream_catalog(wrapped).unwrap();
        assert_eq!(entries[0].title, "Now");
        assert_eq!(entries[0].owner, "6F2728386F8a47ef5EBe323721188e630Ff0FdE9");
        assert_eq!(entries[0].media_type(), "audio");
        assert_eq!(entries[0].duration, Some(2.5));
        assert_eq!(entries[1].index, Some(69));

        let array = br#"[{"owner":"6F2728386F8a47ef5EBe323721188e630Ff0FdE9","topic":"one","title":"One","timestamp":1}]"#;
        let unknown = parse_stream_catalog(array).unwrap();
        assert_eq!(unknown.len(), 1);
        assert!(!unknown[0].is_live());
        assert!(!unknown[0].is_vod());
    }

    #[test]
    fn catalog_parser_discards_bad_entries_and_rejects_bad_shapes() {
        let mixed = br#"[
          {"owner":"6F2728386F8a47ef5EBe323721188e630Ff0FdE9","topic":"good","duration":1},
          {"owner":"6F2728386F8a47ef5EBe323721188e630Ff0FdE9","topic":"external","isExternal":true},
          {"owner":"not-an-owner","topic":"bad"},
          {"owner":"6F2728386F8a47ef5EBe323721188e630Ff0FdE9","topic":"","duration":1},
          {"owner":"6F2728386F8a47ef5EBe323721188e630Ff0FdE9","topic":"bad-duration","duration":"forever"},
          null
        ]"#;
        let entries = parse_stream_catalog(mixed).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].topic, "good");
        assert_eq!(entries[1].topic, "bad-duration");
        assert_eq!(entries[1].duration, None);

        assert!(parse_stream_catalog(br#"{"entries":"not-an-array"}"#).is_none());
        assert!(parse_stream_catalog(br#"{"streams":[]}"#).is_none());
        assert_eq!(parse_stream_catalog(br#"[]"#), Some(Vec::new()));
        assert_eq!(
            parse_stream_catalog(br#"{"entries":[],"lastModified":1782960110305}"#),
            Some(Vec::new())
        );
        assert!(parse_stream_catalog(b"not json").is_none());
        assert!(parse_stream_catalog(&[0xff]).is_none());
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
        FEED_FOLLOWUP_BATCH_LIMIT, FeedFollowupMode, HLS_INITIAL_BOUNDED_RECHECK_LIMIT,
        HLS_INITIAL_EXACT_BETWEEN_RECHECKS, HLS_INITIAL_EXACT_CATCHUP_LIMIT,
        HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL, HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT,
        HLS_TERMINAL_CONFIRMATION_MIN_PRICED_PEERS, cached_feed_should_refresh_head,
        exact_feed_batch_should_refresh_head, feed_followup_batch_limit,
        feed_followup_max_parallel, feed_followup_should_refresh_head,
        hls_initial_exact_round_limit, hls_snapshot_is_terminal, hls_terminal_peer_view_is_mature,
    };

    #[test]
    fn regular_polling_stays_on_the_exact_next_index() {
        assert!(!cached_feed_should_refresh_head(10_000.0, 24_999.0));
        assert!(!exact_feed_batch_should_refresh_head(
            FEED_FOLLOWUP_BATCH_LIMIT - 1
        ));
    }

    #[test]
    fn dormant_or_saturated_readers_jump_to_the_latest_head() {
        assert!(cached_feed_should_refresh_head(10_000.0, 25_000.0));
        assert!(exact_feed_batch_should_refresh_head(
            FEED_FOLLOWUP_BATCH_LIMIT
        ));
        assert!(exact_feed_batch_should_refresh_head(
            FEED_FOLLOWUP_BATCH_LIMIT + 1
        ));
    }

    #[test]
    fn sequence_zero_presentation_keeps_a_bounded_exact_runway() {
        assert_eq!(
            feed_followup_batch_limit(FeedFollowupMode::Canonical),
            FEED_FOLLOWUP_BATCH_LIMIT
        );
        assert_eq!(
            feed_followup_batch_limit(FeedFollowupMode::SequenceZeroPresentation),
            HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT
        );
        assert_eq!(HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT, 64);
        assert_eq!(feed_followup_max_parallel(FeedFollowupMode::Canonical), 1);
        assert_eq!(
            feed_followup_max_parallel(FeedFollowupMode::SequenceZeroPresentation),
            HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL
        );
        assert_eq!(HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL, 4);

        assert!(!feed_followup_should_refresh_head(
            FeedFollowupMode::Canonical,
            FEED_FOLLOWUP_BATCH_LIMIT - 1,
            false,
        ));
        assert!(feed_followup_should_refresh_head(
            FeedFollowupMode::Canonical,
            FEED_FOLLOWUP_BATCH_LIMIT,
            false,
        ));
        assert!(!feed_followup_should_refresh_head(
            FeedFollowupMode::SequenceZeroPresentation,
            HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT,
            false,
        ));
        assert!(!feed_followup_should_refresh_head(
            FeedFollowupMode::SequenceZeroPresentation,
            usize::MAX,
            false,
        ));
        assert!(feed_followup_should_refresh_head(
            FeedFollowupMode::SequenceZeroPresentation,
            0,
            true,
        ));
    }

    #[test]
    fn initial_hls_frontier_rechecks_after_one_exact_update_then_keeps_its_fallback() {
        assert_eq!(HLS_INITIAL_BOUNDED_RECHECK_LIMIT, 2);
        assert_eq!(HLS_INITIAL_EXACT_BETWEEN_RECHECKS, 1);
        assert_eq!(HLS_INITIAL_EXACT_CATCHUP_LIMIT, 32);

        // Reproduce the public fixture's important ordering: the first bounded
        // candidate is 510. Exactly 511 may be read, but 512 must not be admitted
        // before the second bounded wave gets a chance to jump to 646.
        let mut candidate = 510_u64;
        let first_round = hls_initial_exact_round_limit(1, 0);
        assert_eq!(first_round, 1);
        for _ in 0..first_round {
            candidate += 1;
        }
        assert_eq!(candidate, 511);
        let candidate_after_second_bounded_wave = 646_u64;
        assert_eq!(candidate_after_second_bounded_wave, 646);

        // If that wave misses on a sparse peer view, the old total sequential
        // reliability budget remains available rather than being reduced to one.
        assert_eq!(hls_initial_exact_round_limit(2, 1), 31);
        assert_eq!(hls_initial_exact_round_limit(2, 32), 0);
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

mod index_hints {
    const HINT_STORE: &str = include_str!("../src/stream_hls.rs");
    const HLS_SOURCE: &str = include_str!("../src/stream_hls.rs");
    const CARGO_TOML: &str = include_str!("../Cargo.toml");

    #[test]
    fn persisted_vod_hints_are_bounded_network_scoped_rust_storage() {
        for marker in [
            "const STORAGE_KEY: &str = \"weeb3-hls-vod-index-hints-v2\";",
            "const MAX_HINTS: usize = 256;",
            "const MAX_SERIALIZED_BYTES: usize = 64 * 1024;",
            "network_id: u64",
            "normalized_topic",
            "web_sys::window()?.local_storage()",
            "serde_json::to_string(&hints)",
        ] {
            assert!(
                HINT_STORE.contains(marker),
                "missing persisted VOD hint safety marker: {marker}"
            );
        }
        assert!(CARGO_TOML.contains("'Storage',"));
        assert!(
            !HINT_STORE.contains("\"weeb3-hls-vod-index-hints-v1\""),
            "tentative v1 ENDLIST hints must not be migrated or reused"
        );
        assert!(!HINT_STORE.contains("6f2728386f8a47ef5ebe323721188e630ff0fde9"));
        assert!(!HINT_STORE.contains("/feeds/352eabdea9cb05e984a8828d2a6df3d3b5023260"));
    }

    #[test]
    fn unindexed_hints_are_exactly_authenticated_before_the_frontier_is_bypassed() {
        let loader = HLS_SOURCE
            .split("async fn load_feed_snapshot(")
            .nth(1)
            .and_then(|source| source.split("fn feed_cache_key").next())
            .expect("feed loader should remain inspectable");

        let hint_read = loader
            .find("persisted_vod_index(network_id, &owner, &topic)")
            .expect("unindexed loader must consult the persisted hint");
        let exact_read = loader[hint_read..]
            .find("hls_feed_payload_at_index(owner.clone(), topic.clone(), index)")
            .map(|offset| hint_read + offset)
            .expect("hint must use an exact SOC read");
        let manifest_check = loader[exact_read..]
            .find("is_hls_manifest(&candidate.bytes)")
            .map(|offset| exact_read + offset)
            .expect("hinted payload must be authenticated as HLS");
        let endlist_check = loader[manifest_check..]
            .find("hls_is_finalized(&candidate.bytes)")
            .map(|offset| manifest_check + offset)
            .expect("hinted payload must be a finalized VOD");
        let eviction = loader[endlist_check..]
            .find("forget_vod_index(network_id, &owner, &topic)")
            .map(|offset| endlist_check + offset)
            .expect("a failed hint must be evicted");
        let fallback = loader[eviction..]
            .find("latest_hls_feed_payload_observing_positive(")
            .map(|offset| eviction + offset)
            .expect("a failed hint must fall back to the Bee frontier");

        assert!(hint_read < exact_read);
        assert!(exact_read < manifest_check);
        assert!(manifest_check < endlist_check);
        assert!(endlist_check < eviction);
        assert!(eviction < fallback);
    }

    #[test]
    fn catalog_persistence_accepts_only_vod_entries_with_an_index() {
        let renderer = HLS_SOURCE
            .split("fn render_stream_catalog(")
            .nth(1)
            .and_then(|source| source.split("fn canonical_hls_feed_url").next())
            .expect("catalog renderer should remain inspectable");
        for marker in [
            "entry.is_vod().then_some(",
            "entry.index?",
            "normalize_feed_topic(&entry.topic)",
            "remember_catalog_vod_indices(active_profile().swarm_network_id, vod_hints)",
        ] {
            assert!(
                renderer.contains(marker),
                "missing VOD catalog hint marker: {marker}"
            );
        }
    }

    #[test]
    fn explicit_index_branch_remains_an_exact_immutable_read() {
        let loader = HLS_SOURCE
            .split("async fn load_feed_snapshot(")
            .nth(1)
            .and_then(|source| source.split("fn feed_cache_key").next())
            .expect("feed loader should remain inspectable");
        let explicit = loader
            .split("Some(index) => {")
            .nth(1)
            .and_then(|source| source.split("None => {").next())
            .expect("explicit index branch should remain inspectable");
        assert!(
            explicit.contains(".hls_feed_payload_at_index(owner.clone(), topic.clone(), index)")
        );
        assert!(explicit.contains("if loaded.index != index"));
        assert!(!explicit.contains("persisted_vod_index"));
        assert!(!explicit.contains("latest_hls_feed_payload"));

        let authenticated_store = loader
            .split("if loaded.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES")
            .nth(1)
            .expect("authenticated payload handling should remain inspectable");
        assert!(authenticated_store.contains("store_feed_snapshot(&cache_key, snapshot"));
        assert!(authenticated_store.contains("if index_hint.is_none() && snapshot.finalized"));
        assert!(authenticated_store.contains(
            "remember_authenticated_endlist_index(network_id, &owner, &topic, snapshot.index)"
        ));
        assert!(
            authenticated_store
                .find("if index_hint.is_none() && snapshot.finalized")
                .is_some_and(|guard| {
                    authenticated_store[guard..]
                        .find("remember_authenticated_endlist_index(")
                        .is_some()
                })
        );
    }
}

mod share_links {
    use crate::stream_conventions;
    use crate::stream_hls;

    use stream_conventions::{configure_streaming_routes, streaming_route_base};
    use stream_hls::{
        StreamShareNetwork, StreamShareRoute, parse_stream_share_link, stream_share_path,
        stream_share_url,
    };

    const OWNER: &str = "352eabdea9cb05e984a8828d2a6df3d3b5023260";
    const EXAMPLE_OWNER: &str = "6F2728386F8a47ef5EBe323721188e630Ff0FdE9";
    const CANONICAL_EXAMPLE_OWNER: &str = "6f2728386f8a47ef5ebe323721188e630ff0fde9";
    const CURRENT_EXAMPLE_TOPIC: &str = "0d216633-3475-4c26-8dd0-9935ef854bbc";
    const PINNED_EXAMPLE_TOPIC: &str = "ba5c0dc4-2e20-4c08-a10a-c283d32b1af0";

    #[test]
    fn exact_mainnet_link_defaults_and_canonicalizes_the_current_public_example() {
        let input = format!("/weeb-3/stream/{EXAMPLE_OWNER}/{CURRENT_EXAMPLE_TOPIC}");
        let parsed = parse_stream_share_link(&input, "/weeb-3").unwrap();

        assert_eq!(parsed.network, StreamShareNetwork::Mainnet);
        assert_eq!(parsed.owner, CANONICAL_EXAMPLE_OWNER);
        assert_eq!(parsed.topic, CURRENT_EXAMPLE_TOPIC);
        assert_eq!(parsed.index, None);
        assert_eq!(
            stream_share_path("/weeb-3", &parsed).unwrap(),
            format!("/weeb-3/stream/{CANONICAL_EXAMPLE_OWNER}/{CURRENT_EXAMPLE_TOPIC}")
        );
        assert_eq!(
            stream_share_url("https://192.168.1.2:8080", "/weeb-3", &parsed).unwrap(),
            format!(
                "https://192.168.1.2:8080/weeb-3/stream/{CANONICAL_EXAMPLE_OWNER}/{CURRENT_EXAMPLE_TOPIC}"
            )
        );
    }

    #[test]
    fn exact_pinned_mainnet_example_preserves_index_69() {
        let input = format!("/weeb-3/stream/{EXAMPLE_OWNER}/{PINNED_EXAMPLE_TOPIC}/69");
        let parsed = parse_stream_share_link(&input, "/weeb-3").unwrap();

        assert_eq!(parsed.network, StreamShareNetwork::Mainnet);
        assert_eq!(parsed.owner, CANONICAL_EXAMPLE_OWNER);
        assert_eq!(parsed.topic, PINNED_EXAMPLE_TOPIC);
        assert_eq!(parsed.index, Some(69));
        assert_eq!(
            stream_share_path("/weeb-3", &parsed).unwrap(),
            format!("/weeb-3/stream/{CANONICAL_EXAMPLE_OWNER}/{PINNED_EXAMPLE_TOPIC}/69")
        );
        assert_eq!(
            stream_share_url("https://192.168.1.2:8080", "/weeb-3", &parsed).unwrap(),
            format!(
                "https://192.168.1.2:8080/weeb-3/stream/{CANONICAL_EXAMPLE_OWNER}/{PINNED_EXAMPLE_TOPIC}/69"
            )
        );
    }

    #[test]
    fn root_relative_round_trip_preserves_stream_identity_and_network() {
        let route = StreamShareRoute::new(
            StreamShareNetwork::Testnet,
            OWNER,
            "release 7/日本語?preview#one%done",
            Some(u64::MAX),
        )
        .unwrap();

        let path = stream_share_path("/media/", &route).unwrap();
        assert_eq!(
            path,
            concat!(
                "/media/testnet/stream/",
                "352eabdea9cb05e984a8828d2a6df3d3b5023260/",
                "release%207%2F%E6%97%A5%E6%9C%AC%E8%AA%9E%3Fpreview%23one%25done/",
                "18446744073709551615"
            )
        );
        assert_eq!(parse_stream_share_link(&path, "/media"), Ok(route));
    }

    #[test]
    fn absolute_links_and_root_mounts_are_clean_and_round_trip() {
        let route = StreamShareRoute::new(
            StreamShareNetwork::Mainnet,
            format!("0x{OWNER}"),
            "stream-topic",
            None,
        )
        .unwrap();

        assert_eq!(
            stream_share_url("HTTPS://streams.example/", "/", &route).unwrap(),
            format!("https://streams.example/stream/{OWNER}/stream-topic")
        );
        assert_eq!(
            stream_share_url("http://[::1]:8080", "/", &route).unwrap(),
            format!("http://[::1]:8080/stream/{OWNER}/stream-topic")
        );
        assert_eq!(
            parse_stream_share_link(
                &format!("https://streams.example/weeb-3/stream/0x{OWNER}/stream-topic"),
                "/weeb-3/"
            ),
            Ok(route)
        );
    }

    #[test]
    fn parser_accepts_only_the_canonical_stream_route() {
        let parsed = parse_stream_share_link(
            &format!("/weeb-3/stream/{OWNER}/topic%2fpart/007"),
            "/weeb-3",
        )
        .unwrap();

        assert_eq!(parsed.network, StreamShareNetwork::Mainnet);
        assert_eq!(parsed.owner, OWNER);
        assert_eq!(parsed.topic, "topic/part");
        assert_eq!(parsed.index, Some(7));
        assert_eq!(
            stream_share_path("/weeb-3", &parsed).unwrap(),
            format!("/weeb-3/stream/{OWNER}/topic%2Fpart/7")
        );
        assert!(
            parse_stream_share_link(&format!("/weeb-3/MAINNET/STREAM/{OWNER}/topic"), "/weeb-3")
                .is_err()
        );
        assert!(
            parse_stream_share_link(&format!("/weeb-3/mainnet/stream/{OWNER}/topic"), "/weeb-3")
                .is_err()
        );
        assert!(
            parse_stream_share_link(&format!("/weeb-3/watch/{OWNER}/topic"), "/weeb-3").is_err()
        );
        assert!(parse_stream_share_link(&format!("/weeb-3/hls/{OWNER}/topic"), "/weeb-3").is_err());
        assert!(
            parse_stream_share_link(
                &format!("/weeb-3/mainnet/watch/video/{OWNER}/topic"),
                "/weeb-3"
            )
            .is_err()
        );
        assert!(
            parse_stream_share_link(
                &format!("/weeb-3/mainnet/hls/video/{OWNER}/topic"),
                "/weeb-3"
            )
            .is_err()
        );
        assert!(
            parse_stream_share_link(
                &format!("/weeb-3/testnet/watch/video/{OWNER}/topic"),
                "/weeb-3"
            )
            .is_err()
        );
        assert!(
            parse_stream_share_link(
                &format!("/weeb-3/testnet/hls/video/{OWNER}/topic"),
                "/weeb-3"
            )
            .is_err()
        );

        let testnet = parse_stream_share_link(
            &format!("/weeb-3/testnet/stream/{EXAMPLE_OWNER}/{PINNED_EXAMPLE_TOPIC}/69"),
            "/weeb-3",
        )
        .unwrap();
        assert_eq!(testnet.network, StreamShareNetwork::Testnet);
        assert_eq!(testnet.owner, CANONICAL_EXAMPLE_OWNER);
        assert_eq!(testnet.topic, PINNED_EXAMPLE_TOPIC);
        assert_eq!(testnet.index, Some(69));
        assert_eq!(
            stream_share_path("/weeb-3", &testnet).unwrap(),
            format!("/weeb-3/testnet/stream/{CANONICAL_EXAMPLE_OWNER}/{PINNED_EXAMPLE_TOPIC}/69")
        );
    }

    #[test]
    fn route_base_matching_uses_a_path_boundary() {
        let valid = format!("/app/stream/{OWNER}/topic");
        assert!(parse_stream_share_link(&valid, "/app").is_ok());

        let collision = format!("/application/stream/{OWNER}/topic");
        assert!(parse_stream_share_link(&collision, "/app").is_err());
        assert!(parse_stream_share_link(&valid, "/").is_err());
    }

    #[test]
    fn configured_npm_share_route_is_decoded_before_the_legacy_nav_scanner() {
        configure_streaming_routes("/media/weeb3-service.js", "/media").unwrap();
        let route = StreamShareRoute::new(
            StreamShareNetwork::Testnet,
            OWNER,
            "topic/with reserved %# bytes",
            Some(42),
        )
        .unwrap();
        let path = stream_share_path(&streaming_route_base(), &route).unwrap();
        assert_eq!(
            path,
            format!("/media/testnet/stream/{OWNER}/topic%2Fwith%20reserved%20%25%23%20bytes/42")
        );

        let decoded = parse_stream_share_link(&path, &streaming_route_base()).unwrap();
        assert_eq!(decoded, route);
        assert_eq!(decoded.network, StreamShareNetwork::Testnet);

        let nav = include_str!("../src/nav.rs");
        let read_routes = nav
            .split_once("pub async fn read_routes()")
            .expect("read_routes must remain available")
            .1;
        let strict_parse = read_routes
            .find("parse_resource_route(&pathname)")
            .expect("read_routes must parse the complete canonical route");
        let fallback_scan = read_routes
            .find("parse_routes_from_path(&pathname)")
            .expect("read_routes must retain its legacy path fallback");
        assert!(
            strict_parse < fallback_scan,
            "strict configured-route decoding must run before the lossy scanner"
        );

        let scanner = nav
            .split_once("fn parse_routes_from_path")
            .expect("legacy path scanner")
            .1
            .split_once("pub async fn clear_path")
            .expect("legacy path scanner boundary")
            .0;
        for removed_player_route in ["\"stream\"", "\"watch\"", "\"hls\""] {
            assert!(
                !scanner.contains(removed_player_route),
                "legacy scanner must not revive strict player route {removed_player_route}"
            );
        }
    }

    #[test]
    fn invalid_routes_are_rejected_instead_of_being_partially_interpreted() {
        let invalid = [
            format!("/weeb-3/mainnet/stream/{OWNER}/topic"),
            format!("/weeb-3/watch/{OWNER}/topic"),
            format!("/weeb-3/hls/{OWNER}/topic"),
            format!("/weeb-3/mainnet/watch/video/{OWNER}/topic"),
            format!("/weeb-3/mainnet/hls/video/{OWNER}/topic"),
            format!("/weeb-3/testnet/watch/video/{OWNER}/topic"),
            format!("/weeb-3/testnet/hls/video/{OWNER}/topic"),
            format!("/weeb-3/stream/not-an-owner/topic"),
            format!("/weeb-3/stream/{OWNER}/topic/not-a-number"),
            format!("/weeb-3/stream/{OWNER}/topic/18446744073709551616"),
            format!("/weeb-3/stream/{OWNER}/topic/1/extra"),
            format!("/weeb-3/stream/{OWNER}/topic/"),
            format!("/weeb-3/stream/{OWNER}/bad%escape"),
            format!("/weeb-3/stream/{OWNER}/%FF"),
            format!("/weeb-3/stream/{OWNER}/topic?index=7"),
            format!("/weeb-3/stream/{OWNER}/topic#fragment"),
            format!("//host/weeb-3/stream/{OWNER}/topic"),
            format!("ftp://host/weeb-3/stream/{OWNER}/topic"),
        ];

        for input in invalid {
            assert!(
                parse_stream_share_link(&input, "/weeb-3").is_err(),
                "accepted {input:?}"
            );
        }
    }

    #[test]
    fn construction_rejects_unsafe_topic_owner_base_and_origin_values() {
        for topic in ["", ".", "..", "line\nbreak"] {
            assert!(
                StreamShareRoute::new(StreamShareNetwork::Mainnet, OWNER, topic, None,).is_err(),
                "accepted topic {topic:?}"
            );
        }

        let too_long = "é".repeat(129);
        assert!(
            StreamShareRoute::new(StreamShareNetwork::Mainnet, OWNER, too_long, None,).is_err()
        );

        let route =
            StreamShareRoute::new(StreamShareNetwork::Mainnet, OWNER, "topic", None).unwrap();
        assert!(stream_share_path("/app/../escape", &route).is_err());
        assert!(stream_share_url("https://user@example.com", "/app", &route).is_err());
        assert!(stream_share_url("https://example.com/path", "/app", &route).is_err());
        assert!(stream_share_url("javascript://example.com", "/app", &route).is_err());
        assert!(stream_share_url("https://example.com:99999", "/app", &route).is_err());
        assert!(stream_share_url("https://[not-ipv6]", "/app", &route).is_err());
        assert!(stream_share_url("https://bad-.example", "/app", &route).is_err());
        assert!(stream_share_url("https://999.999.999.999", "/app", &route).is_err());
    }
}

mod routes {
    use crate::stream_conventions;

    use stream_conventions::{
        configure_streaming_routes, route_base_controls_path, streaming_route_base,
        streaming_route_path, streaming_service_worker_scope, streaming_service_worker_url,
    };

    #[test]
    fn defaults_match_the_standalone_distribution() {
        assert_eq!(streaming_route_base(), "/weeb-3");
        assert_eq!(streaming_route_path("hls/bytes"), "/weeb-3/hls/bytes");
        assert_eq!(streaming_service_worker_url(), "/weeb-3/service.js");
        assert_eq!(streaming_service_worker_scope(), "/weeb-3/");
    }

    #[test]
    fn npm_mount_can_configure_a_same_origin_worker_and_route_scope() {
        configure_streaming_routes("/media/weeb3-service.js", "/media/").unwrap();
        assert_eq!(streaming_route_base(), "/media");
        assert_eq!(streaming_route_path("/feeds"), "/media/feeds");
        assert_eq!(streaming_service_worker_url(), "/media/weeb3-service.js");
        assert_eq!(streaming_service_worker_scope(), "/media/");

        configure_streaming_routes("/weeb3-service.js", "/").unwrap();
        assert_eq!(streaming_route_path("hls/bytes"), "/hls/bytes");
        assert_eq!(streaming_service_worker_scope(), "/");
    }

    #[test]
    fn rejects_route_bases_that_could_escape_or_mismatch_scope() {
        for invalid in [
            "",
            "relative",
            "/two//parts",
            "/two/../parts",
            "/two/%2e%2e/parts",
            "/two?query",
            "/two#fragment",
            "/two\\parts",
        ] {
            assert!(
                configure_streaming_routes("/service.js", invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(configure_streaming_routes("", "/media").is_err());
    }

    #[test]
    fn route_scope_must_contain_the_client_document() {
        assert!(!route_base_controls_path("/app", "/app"));
        assert!(route_base_controls_path("/app", "/app/stream/1"));
        assert!(route_base_controls_path("/app", "/app/stream/owner/topic"));
        assert!(route_base_controls_path(
            "/app",
            "/app/testnet/stream/owner/topic"
        ));
        assert!(!route_base_controls_path("/app", "/application"));
        assert!(!route_base_controls_path("/media", "/app/index.html"));
        assert!(route_base_controls_path("/", "/anywhere"));
    }
}

mod network_routes {
    use crate::nav;
    use crate::network_profile;
    use crate::stream_conventions;

    use nav::{
        ResourceRoute, parse_networked_resource_route, parse_resource_route,
        route_network_mode_from_path,
    };
    use network_profile::NetworkMode;
    use stream_conventions::configure_streaming_routes;

    const REFERENCE: &str = "919b5395bf7a59cbb3b365769de09a2b15ac5d897823dda9270259a3c038d574";
    const OWNER: &str = "352eabdea9cb05e984a8828d2a6df3d3b5023260";
    const TOPIC: &str = "cfbbc155d709547b198638d0fb11d733359561538d8bd606a9ab257354d13bcc";
    #[test]
    fn recognized_transport_routes_default_to_mainnet_and_preserve_testnet() {
        configure_streaming_routes("/weeb-3/service.js", "/weeb-3").unwrap();

        let route_tails = [
            format!("bzz/{REFERENCE}"),
            format!("bzz/{REFERENCE}/index.html"),
            format!("bytes/{REFERENCE}"),
            format!("chunk/{REFERENCE}"),
            format!("chunks/{REFERENCE}"),
            format!("feeds/{OWNER}/{TOPIC}"),
            format!("hls/bytes/{REFERENCE}"),
        ];

        for tail in route_tails {
            assert_eq!(
                route_network_mode_from_path(&format!("/weeb-3/{tail}")),
                Some(NetworkMode::Mainnet),
                "prefix-free route did not default to mainnet: {tail}"
            );
            assert_eq!(
                route_network_mode_from_path(&format!("/weeb-3/mainnet/{tail}")),
                Some(NetworkMode::Mainnet),
                "mainnet transport alias was not accepted: {tail}"
            );
            assert_eq!(
                route_network_mode_from_path(&format!("/weeb-3/testnet/{tail}")),
                Some(NetworkMode::Testnet),
                "testnet route lost its network: {tail}"
            );
        }

        assert_eq!(
            route_network_mode_from_path("/weeb-3/bzz"),
            Some(NetworkMode::Mainnet)
        );
        assert_eq!(
            route_network_mode_from_path("/weeb-3/mainnet/bzz"),
            Some(NetworkMode::Mainnet)
        );
        assert_eq!(
            route_network_mode_from_path("/weeb-3/testnet/bzz"),
            Some(NetworkMode::Testnet)
        );
    }

    #[test]
    fn parsed_ui_resources_retain_network_identity() {
        configure_streaming_routes("/weeb-3/service.js", "/weeb-3").unwrap();

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
    fn configured_route_base_has_the_same_network_rules() {
        configure_streaming_routes("/media/weeb3-service.js", "/media").unwrap();

        assert_eq!(
            route_network_mode_from_path(&format!("/media/feeds/{OWNER}/{TOPIC}")),
            Some(NetworkMode::Mainnet)
        );
        assert_eq!(
            route_network_mode_from_path(&format!("/media/testnet/feeds/{OWNER}/{TOPIC}")),
            Some(NetworkMode::Testnet)
        );
        assert_eq!(
            parse_networked_resource_route(&format!(
                "https://streams.example/media/mainnet/bzz/{REFERENCE}/index.html"
            ))
            .map(|route| route.network),
            Some(NetworkMode::Mainnet)
        );
        assert_eq!(
            route_network_mode_from_path(&format!("/weeb-3/bytes/{REFERENCE}")),
            None,
            "a configured npm mount must not claim the default standalone base"
        );
    }

    #[test]
    fn invalid_and_removed_routes_do_not_acquire_a_network() {
        configure_streaming_routes("/weeb-3/service.js", "/weeb-3").unwrap();

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
    use crate::stream_conventions;

    const STATIC_WORKER: &str = include_str!("../static/service.js");
    const INTERFACE: &str = include_str!("../src/interface.rs");
    const INTERFACE_RUNTIME: &str = include_str!("../src/interface_runtime_conventions.rs");
    const HLS_PLAYER: &str = include_str!("../src/stream_hls.rs");
    const LIBRARY: &str = include_str!("../src/library.rs");
    const STATIC_HLS_LOADER: &str = include_str!("../static/hls_loader.js");
    const STATIC_HLS_EXAMPLE: &str = include_str!("../static/hls-stream-example.html");
    const MAIN_SERVER: &str = include_str!("../src/main.rs");
    const HAXE_BUILD: &str = include_str!("../Code_One.hx");
    const NPM_WORKFLOW: &str = include_str!("../.github/workflows/plain.yml");
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
    fn hls_javascript_boundary_is_one_lazy_static_loader() {
        let docs_root_loader = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs")
            .join("hls_loader.js");
        assert!(
            !docs_root_loader.exists(),
            "the loader source belongs in static; docs snippets are generated by Haxe"
        );

        assert!(STATIC_HLS_LOADER.contains("export async function loadHls()"));
        assert!(STATIC_HLS_LOADER.contains("import(\"hls.js\")"));
        assert!(HLS_PLAYER.contains("module = \"/static/hls_loader.js\""));

        for policy in [
            "maxBufferLength",
            "startLoad",
            "recoverMediaError",
            "AbortController",
        ] {
            assert!(
                !STATIC_HLS_LOADER.contains(policy),
                "HLS policy leaked into the JavaScript bridge: {policy}"
            );
        }
    }

    #[test]
    fn static_hls_example_opens_the_unindexed_test_feed_from_its_own_scope() {
        assert!(
            STATIC_HLS_EXAMPLE
                .contains(r#"const owner = "6F2728386F8a47ef5EBe323721188e630Ff0FdE9";"#)
        );
        assert!(
            STATIC_HLS_EXAMPLE.contains(r#"const topic = "b347b89b-933c-424f-a3d1-403bdd270b25";"#)
        );
        assert!(
            STATIC_HLS_EXAMPLE.contains(r#"<video id="stream" controls autoplay playsinline"#),
            "the application should visibly own the media element"
        );
        assert!(
            STATIC_HLS_EXAMPLE.contains("node.attachHlsStream(media, owner, topic, {")
                && STATIC_HLS_EXAMPLE.contains(r#"start: "beginning""#),
            "the example feed has no pinned index"
        );
        assert!(!STATIC_HLS_EXAMPLE.contains("const index ="));
        assert!(!STATIC_HLS_EXAMPLE.contains("renderInterface"));
        assert!(!STATIC_HLS_EXAMPLE.contains("playHlsStream"));
        assert!(STATIC_HLS_EXAMPLE.contains("node.start({ mainnet: true })"));
        assert!(STATIC_HLS_EXAMPLE.contains(r#"media.addEventListener("waiting""#));
        assert!(
            STATIC_HLS_EXAMPLE.contains(r#"media.addEventListener("weeb3-hls-manifest-ready""#)
        );
        assert!(STATIC_HLS_EXAMPLE.contains("node?.detachHlsStream()"));
        assert!(STATIC_HLS_EXAMPLE.contains(r#"new URL("service.js", directory).href"#));
        assert!(
            STATIC_HLS_EXAMPLE
                .contains(r#"const routeBase = directory.pathname.replace(/\/$/, "") || "/";"#)
        );

        assert!(MAIN_SERVER.contains(r#""/weeb-3/hls-stream-example.html","#));
        assert!(MAIN_SERVER.contains(r#"StaticFiles::get("hls-stream-example.html")"#));
        assert!(HAXE_BUILD.contains(
            r#"clientele('cp', [ './static/hls-stream-example.html', './docs/' ], count);"#
        ));
        assert!(HAXE_BUILD.contains(
            r#"clientele('git', ['add', '-f', './static/hls-stream-example.html', './static/hls_loader.js'], count);"#
        ));
        assert!(
            !NPM_WORKFLOW.contains("hls-stream-example.html"),
            "the standalone example is served/copied, not published in the npm package"
        );
    }

    #[test]
    fn npm_hls_attachment_is_a_dom_adapter_over_the_shared_rust_pipeline() {
        let adapter = source_between(
            LIBRARY,
            "pub async fn attach_hls_stream(",
            "#[wasm_bindgen(js_name = attach_hls_stream)]",
        );
        assert!(adapter.contains("media: HtmlMediaElement"));
        assert!(adapter.contains("install_service_worker_message_bridge(self.inner.clone())"));
        assert!(adapter.contains("self.boot_runtime().await"));
        assert!(adapter.contains("stream_hls::attach_hls_feed_media("));
        assert!(!adapter.contains("interface_result_view_is_mounted"));
        assert!(!adapter.contains("render_interface_shell"));
        assert!(!adapter.contains("create_element"));

        let detach = source_between(
            LIBRARY,
            "pub fn detach_hls_stream(&self)",
            "#[wasm_bindgen(js_name = detach_hls_stream)]",
        );
        assert!(detach.contains("begin_result_view_request()"));
        assert!(detach.contains("release_current_stream_view()"));

        let preparation = source_between(
            HLS_PLAYER,
            "async fn prepare_hls_feed_target(",
            "async fn attach_hls_feed_player(",
        );
        assert!(preparation.contains("service_worker_controls_bzz_requests("));
        assert!(preparation.contains("canonical_hls_feed_url("));

        let attachment = source_between(
            HLS_PLAYER,
            "async fn attach_hls_feed_player(",
            "pub(crate) async fn attach_hls_feed_media(",
        );
        assert!(attachment.contains("install_hls_prefetch_lifecycle("));
        assert!(attachment.contains("play_hls(player, &target.source)"));

        let bundled_ui = source_between(
            HLS_PLAYER,
            "async fn open_hls_feed_view_generation(",
            "fn render_stream_catalog(",
        );
        assert!(bundled_ui.contains("prepare_hls_feed_target("));
        assert!(bundled_ui.contains("attach_hls_feed_player("));
    }

    #[test]
    fn hls_open_intent_is_explicit_and_initial_warmup_gets_one_fresh_window() {
        assert!(HLS_PLAYER.contains("pub(crate) enum HlsOpenIntent"));
        assert!(HLS_PLAYER.contains("Beginning,"));
        assert!(HLS_PLAYER.contains("CurrentWindow,"));
        assert!(HLS_PLAYER.contains("index_hint.is_none() && self == Self::Beginning"));
        assert!(!HLS_PLAYER.contains("current_view_requests_hls_sequence_zero_start"));

        let warmup = source_between(
            HLS_PLAYER,
            "fn activate_hls_prefetch_warmup()",
            "fn retire_hls_prefetch_timeline()",
        );
        assert!(warmup.contains("session.mode == HlsPrefetchMode::Inactive"));
        assert!(warmup.contains("initial_warmup || timeline_rebasing"));
        assert!(
            warmup
                .contains("session.startup_deadline_ms = now_ms + HLS_INITIAL_RESPONSE_BUDGET_MS")
        );
        assert_eq!(
            warmup
                .matches("session.startup_deadline_ms = now_ms + HLS_INITIAL_RESPONSE_BUDGET_MS")
                .count(),
            1,
            "playlist reloads must not keep extending startup speculation"
        );
    }

    #[test]
    fn npm_and_bundled_interface_share_the_core_runtime_and_current_network_dial_path() {
        assert!(RUNTIME.contains("runtime_started: AtomicBool"));
        assert!(RUNTIME.contains("self.runtime_started.swap(true, Ordering::AcqRel)"));
        assert!(LIBRARY.contains("inner: Arc<Weeb3>"));
        assert!(!LIBRARY.contains("Swarm<Behaviour>"));
        assert!(LIBRARY.contains("start_weeb3_runtime(inner.clone())"));
        assert!(LIBRARY.contains("connect_bootnode_for_current_network("));
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
            "struct HlsStreamOptions",
        );
        assert!(implicit_start.contains("route_network_mode_from_location()"));
        assert!(INTERFACE_RUNTIME.contains("connect_bootnode_for_current_network("));
        assert!(RUNTIME.contains("pub(crate) async fn connect_bootnode_for_current_network("));

        let network_switch = source_between(
            RUNTIME,
            "pub async fn set_network_id(&self, id: String)",
            "async fn current_connection_generation(&self)",
        );
        let profile_sync = network_switch
            .find("if let Some(profile) = profile_for_swarm_network_id(parsed_id)")
            .expect("active network profile synchronization");
        let changed_check = network_switch
            .find("if *nid != parsed_id")
            .expect("physical network change check");
        assert!(
            profile_sync < changed_check,
            "the global secure/on-chain profile must synchronize even when a new node already stores that id"
        );
    }

    #[test]
    fn npm_data_plane_exports_cross_the_shared_runtime_barrier() {
        for (start, end) in [
            (
                "pub async fn open_stream_feed(&self",
                "#[wasm_bindgen(js_name = open_stream_feed)]",
            ),
            (
                "pub async fn attach_hls_stream(",
                "#[wasm_bindgen(js_name = attach_hls_stream)]",
            ),
            (
                "pub async fn play_hls_stream(",
                "#[wasm_bindgen(js_name = play_hls_stream)]",
            ),
            (
                "pub async fn retrieve(&self",
                "#[wasm_bindgen(js_name = retrieveBytes)]",
            ),
            (
                "pub async fn retrieve_bytes(&self",
                "#[wasm_bindgen(js_name = retrieve_bytes)]",
            ),
            (
                "pub async fn retrieve_chunk(&self",
                "#[wasm_bindgen(js_name = retrieve_chunk)]",
            ),
            (
                "pub async fn post_push_chunk_js(",
                "#[wasm_bindgen(js_name = post_push_chunk)]",
            ),
            (
                "pub async fn upload_with_redundancy(",
                "#[wasm_bindgen(js_name = post_upload)]",
            ),
            (
                "pub async fn acquire_feed(&self",
                "#[wasm_bindgen(js_name = acquire_feed)]",
            ),
            (
                "pub async fn acquire_feed_bytes(&self",
                "#[wasm_bindgen(js_name = acquire_feed_bytes)]",
            ),
        ] {
            let operation = source_between(LIBRARY, start, end);
            assert!(
                operation.contains("self.boot_runtime().await"),
                "{start} bypasses the shared npm startup barrier"
            );
        }
    }

    #[test]
    fn npm_render_mount_uses_the_startup_barrier_and_cannot_reopen_a_superseded_route() {
        let render = source_between(
            LIBRARY,
            "pub fn render_interface(&self, container: Element)",
            "#[wasm_bindgen(js_name = render_interface)]",
        );
        assert!(render.contains("self.startup_pending.fetch_add(1, Ordering::AcqRel)"));
        assert!(render.contains("let guard = serial.lock().await"));
        assert!(render.contains("route_network_mode_from_location()"));
        assert!(render.contains("startup_already_configured"));
        assert!(render.contains("!startup_already_configured || route_changed_network"));
        assert!(render.contains("pending.fetch_sub(1, Ordering::AcqRel)"));
        assert!(render.contains("Some(initial_result_generation)"));
        let dial_position = render
            .find("schedule_bootnode_dials(")
            .expect("render bootnode dial scheduling");
        let barrier_release_position = render
            .find("pending.fetch_sub(1, Ordering::AcqRel)")
            .expect("render startup barrier release");
        assert!(
            dial_position < barrier_release_position,
            "render must queue connection buildup before npm operations proceed"
        );

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
            "fn hls_config() -> Object {",
            "fn swarm_load_policy() -> Object {",
        );

        assert!(
            config
                .contains(r#"set_property(&config, "maxBufferLength", JsValue::from_f64(30.0));"#)
        );
        assert!(
            config
                .contains(r#"set_property(&config, "maxBufferLength", JsValue::from_f64(90.0));"#)
        );
        assert_eq!(config.matches("\"maxBufferLength\"").count(), 2);
        assert!(config.contains("JsValue::from_f64(32.0 * 1024.0 * 1024.0)"));
        assert!(config.contains("JsValue::from_f64(96.0 * 1024.0 * 1024.0)"));
        assert_eq!(config.matches("\"maxBufferSize\"").count(), 2);
    }

    #[test]
    fn uncached_hls_foregrounds_keep_the_stable_startup_head_start() {
        let retrieval = source_between(
            HLS_PLAYER,
            "async fn retrieve_hls_payload_for_playback(",
            "fn cached_hls_asset_metadata(",
        );
        assert!(retrieval.contains("let overlap_head_start_ms = if foreground_cached"));
        assert!(retrieval.contains("if foreground_cached"));
        assert!(
            HLS_PLAYER
                .contains("const HLS_EXACT_NEXT_HEAD_START: Duration = Duration::from_secs(1);")
        );
        assert!(
            HLS_PLAYER
                .contains("const HLS_NEXT_RESERVE_STAGGER: Duration = Duration::from_secs(1);")
        );
    }

    #[test]
    fn hls_steady_state_reserves_foreground_with_three_speculative_bodies() {
        assert!(HLS_PLAYER.contains("const HLS_PREFETCH_BODY_MAX_PARALLEL: usize = 3;"));
        let cache = source_between(HLS_PLAYER, "fn load_role(", "fn finish_load(");
        assert!(cache.contains(
            ".filter(|pending| pending.speculative && pending.generation == generation)"
        ));
        assert!(cache.contains("speculative_loads >= HLS_PREFETCH_BODY_MAX_PARALLEL"));
        assert!(cache.contains("pending.speculative = false;"));
        assert!(!HLS_PLAYER.contains("HLS_ACTIVE_BODY_MAX_PARALLEL"));

        let stages = source_between(
            HLS_PLAYER,
            "async fn prefetch_hls_media_stages(",
            "async fn retrieve_hls_payload_for_playback(",
        );
        assert!(stages.contains("loads.len() < HLS_PREFETCH_BODY_MAX_PARALLEL"));
        assert!(!stages.contains("immediate_successor_ready"));
    }

    #[test]
    fn terminal_hls_failures_retry_quickly_without_a_four_x_dead_end() {
        let policy = source_between(
            HLS_PLAYER,
            "fn swarm_load_policy() -> Object {",
            "fn device_memory_gib()",
        );
        assert!(policy.contains(r#""retryDelayMs", JsValue::from_f64(500.0)"#));

        let recovery = source_between(
            HLS_PLAYER,
            "fn handle_hls_error(",
            "fn schedule_network_recovery(",
        );
        assert!(recovery.contains("let delay = 1_000_u64.saturating_mul("));

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
        let feed = source_between(
            HLS_PLAYER,
            "async fn fetch_feed_response(",
            "async fn load_feed_snapshot(",
        );
        assert!(
            feed.contains(r#"FetchResponse::error(503, "weeb-3 did not retrieve feed update")"#)
        );
    }

    #[test]
    fn runtime_logging_is_bounded_off_the_network_hot_path() {
        assert!(RUNTIME.contains("mpsc::bounded::<String>(LOG_QUEUE_CAPACITY)"));
        assert!(RUNTIME.contains("for _ in 0..LOG_DRAIN_BATCH"));
        assert!(RUNTIME.contains("if DEBUG_RUNTIME_LOGS"));
        assert!(INTERFACE_RUNTIME.contains("logs.child_element_count() > crate::LOG_DOM_RETAINED"));
        assert!(UPLOAD.contains("logs.child_element_count() > crate::LOG_DOM_RETAINED"));
    }

    #[test]
    fn worker_dispatches_accounting_sensitive_work_once_and_tolerates_late_responses() {
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
            "function hlsRequestFlightKey(",
        );
        assert!(dispatch.contains("return messageClient(clients[0], message, timeoutMs)"));
        assert!(!dispatch.contains("Promise.race"));
        assert!(!dispatch.contains("AbortController"));

        assert!(
            INTERFACE.contains("event.stop_immediate_propagation()"),
            "one runtime bridge must claim each worker operation"
        );
        assert!(
            !INTERFACE.contains("port.post_message(&resp).unwrap()"),
            "a response arriving after its browser port closes must not trap"
        );
        assert!(
            INTERFACE.contains("let _ = port.post_message(&resp);"),
            "late response delivery must be best-effort after Rust work drains"
        );
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
        assert!(!STATIC_WORKER.contains("APP_HLS_EXAMPLE"));
        assert!(INTERFACE.contains("if active_network_id != required_network_id"));
    }

    #[test]
    fn worker_routes_only_explicit_weeb3_requests_to_the_matching_network() {
        let route_network = source_between(
            STATIC_WORKER,
            "function canonicalRouteNetworkId(pathname)",
            "function isNetworkShellPath(pathname)",
        );
        assert!(route_network.contains("first === \"testnet\" ? 10 : 1"));

        let client_selection = source_between(
            STATIC_WORKER,
            "async function requestClients(event, requestUrl, requiredNetworkId)",
            "function closeMessagePort(",
        );
        assert!(
            client_selection.contains("return firstReadyClient(candidates, requiredNetworkId)")
        );

        assert!(INTERFACE.contains("if active_network_id != required_network_id"));
        assert!(INTERFACE.contains("weeb3.service_worker_network_id()"));

        let fetch_handler = source_between(
            STATIC_WORKER,
            "self.addEventListener(\"fetch\", (event) => {",
            "function clientInScope(client)",
        );
        for explicit_route in [
            "isBzzUploadPath",
            "canonicalBzzResource",
            "canonicalRawResource",
            "canonicalFeedResource",
        ] {
            assert!(
                fetch_handler.contains(explicit_route),
                "missing explicit worker route: {explicit_route}"
            );
        }
        assert!(fetch_handler.contains("if (url.origin !== SCOPE.origin)"));
        assert!(!fetch_handler.contains("respondWith(fetch(request))"));
        assert!(!fetch_handler.contains("respondWith(fetchOrError(request))"));
        assert!(!STATIC_WORKER.contains("cache.addAll("));
        assert!(!STATIC_WORKER.contains("caches.delete("));
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
        assert!(INTERFACE_RUNTIME.contains("service_worker_protocol_request("));

        let setup = source_between(
            INTERFACE_RUNTIME,
            "pub async fn get_service_worker()",
            "fn controlled_service_worker()",
        );
        assert!(INTERFACE_RUNTIME.contains("static SERVICE_WORKER_SETUP_LOCK:"));
        assert!(setup.contains("SERVICE_WORKER_SETUP_LOCK.with(std::rc::Rc::clone)"));
        assert!(setup.contains("setup_lock.lock().await"));
        let lock_position = setup
            .find("setup_lock.lock().await")
            .expect("setup lock acquisition");
        let registration_position = setup
            .find("service_worker_registration(&service0).await")
            .expect("registration lookup");
        assert!(lock_position < registration_position);

        let control_wait = source_between(
            INTERFACE_RUNTIME,
            "async fn wait_for_service_worker_control(",
            "pub(crate) async fn service_worker_controls_bzz_requests(",
        );
        let no_controller = source_between(
            control_wait,
            "if controlled_service_worker().is_none() {",
            "// Registration, activation, controllerchange",
        );
        assert!(no_controller.contains("continue;"));
        assert!(!no_controller.contains("protocol_probes ="));
        let probe_loop = control_wait
            .split_once("for _ in 0..probes {")
            .map(|(_, body)| body)
            .expect("follow-up protocol probe loop");
        let controller_check = probe_loop
            .find("if controlled_service_worker().is_none()")
            .expect("controller check inside probe loop");
        let probe_increment = probe_loop
            .find("protocol_probes = protocol_probes.saturating_add(1)")
            .expect("follow-up probe increment");
        assert!(controller_check < probe_increment);

        let readiness = INTERFACE_RUNTIME
            .split_once("pub(crate) async fn service_worker_controls_bzz_requests(")
            .map(|(_, body)| body)
            .expect("worker readiness function");
        assert!(
            readiness.contains("Duration::from_millis(SERVICE_WORKER_CONTROL_TOTAL_TIMEOUT_MS)")
        );
        let readiness_lower = readiness.to_ascii_lowercase();
        assert!(!readiness_lower.contains("please reload"));
        assert!(!readiness_lower.contains("reload the page"));
        assert!(!INTERFACE_RUNTIME.contains("location.reload"));
    }

    #[test]
    fn direct_stream_share_routes_are_document_shells_only() {
        const OWNER: &str = "352eabdea9cb05e984a8828d2a6df3d3b5023260";

        for path in [
            format!("/weeb-3/stream/{OWNER}/topic"),
            format!("/weeb-3/stream/0x{OWNER}/topic%2Fpart"),
            format!("/weeb-3/testnet/stream/{OWNER}/topic/7"),
            "/weeb-3/stream/6F2728386F8a47ef5EBe323721188e630Ff0FdE9/0d216633-3475-4c26-8dd0-9935ef854bbc".to_string(),
            "/weeb-3/stream/6F2728386F8a47ef5EBe323721188e630Ff0FdE9/ba5c0dc4-2e20-4c08-a10a-c283d32b1af0/69".to_string(),
            "/weeb-3/stream/6F2728386F8a47ef5EBe323721188e630Ff0FdE9/b347b89b-933c-424f-a3d1-403bdd270b25".to_string(),
        ] {
            assert!(
                stream_conventions::is_direct_share_route(&path),
                "share route was not recognized: {path}"
            );
        }

        const REFERENCE: &str = "919b5395bf7a59cbb3b365769de09a2b15ac5d897823dda9270259a3c038d574";
        for path in [
            "/weeb-3/".to_string(),
            "/weeb-3/mainnet".to_string(),
            format!("/weeb-3/mainnet/stream/{OWNER}/topic"),
            format!("/weeb-3/mainnet/feed/{OWNER}/topic"),
            format!("/weeb-3/mainnet/feeds/{OWNER}/topic"),
            format!("/weeb-3/watch/{OWNER}/topic"),
            format!("/weeb-3/hls/{OWNER}/topic"),
            format!("/weeb-3/mainnet/hls/video/{OWNER}/topic"),
            format!("/weeb-3/mainnet/watch/video/{OWNER}/topic"),
            format!("/weeb-3/testnet/hls/video/{OWNER}/topic"),
            format!("/weeb-3/testnet/watch/video/{OWNER}/topic"),
            format!("/weeb-3/MAINNET/STREAM/{OWNER}/topic"),
            format!("/weeb-3/stream/not-an-owner/topic"),
            format!("/weeb-3/stream/{OWNER}"),
            format!("/weeb-3/stream/{OWNER}/topic/"),
            format!("/weeb-3/stream/{OWNER}/topic/not-a-number"),
            format!("/weeb-3/stream/{OWNER}/topic/18446744073709551616"),
            format!("/weeb-3/stream/{OWNER}/topic/1/extra"),
            format!("/weeb-3/stream/{OWNER}/bad%escape"),
            format!("/weeb-3/stream/{OWNER}/control%C2%80topic"),
            format!("/other/stream/{OWNER}/topic"),
            format!("/weeb-3/hls/bytes/{REFERENCE}"),
        ] {
            assert!(
                !stream_conventions::is_direct_share_route(&path),
                "non-share route was recognized: {path}"
            );
        }

        assert!(stream_conventions::is_document_navigation(
            "GET",
            Some("navigate"),
            Some("document"),
            Some("text/html,application/xhtml+xml")
        ));
        assert!(!stream_conventions::is_document_navigation(
            "GET",
            Some("cors"),
            Some("empty"),
            Some("text/html")
        ));
        assert!(stream_conventions::is_document_navigation(
            "GET",
            None,
            None,
            Some("text/html")
        ));
        assert!(!stream_conventions::is_document_navigation(
            "POST",
            Some("navigate"),
            Some("document"),
            Some("text/html")
        ));
        assert!(!stream_conventions::is_document_navigation(
            "GET",
            Some("navigate"),
            Some("iframe"),
            Some("text/html")
        ));
        assert!(!stream_conventions::is_document_navigation(
            "GET",
            Some("cors"),
            Some("empty"),
            Some("application/vnd.apple.mpegurl,*/*")
        ));
    }

    #[test]
    fn server_and_worker_expose_only_canonical_stream_shell_routes() {
        assert!(MAIN_SERVER.contains(".route(\"/{*wildcard}\", get(get_fallback))"));
        assert!(MAIN_SERVER.contains("is_direct_share_route(uri.path())"));
        assert!(MAIN_SERVER.contains("is_document_navigation("));

        let worker_matcher = source_between(
            STATIC_WORKER,
            "function isDirectShareShellPath(pathname)",
            "function isBzzUploadPath(pathname)",
        );
        assert!(worker_matcher.contains("kind !== \"stream\""));
        assert!(worker_matcher.contains("if (parts[0] === \"testnet\")"));
        assert!(!worker_matcher.contains("mainnet"));
        assert!(!worker_matcher.contains("watch"));
        assert!(!worker_matcher.contains("video"));

        let fetch_handler = source_between(
            STATIC_WORKER,
            "self.addEventListener(\"fetch\", (event) => {",
            "function clientInScope(client)",
        );
        assert!(fetch_handler.contains("isAppShellNavigation(request)"));
        assert!(fetch_handler.contains("isDirectShareShellPath(url.pathname)"));
        assert!(!MAIN_SERVER.contains("watch/video"));
        assert!(!STATIC_WORKER.contains("watch/video"));
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
            "async fn publish_hls_generation(",
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
            "async fn publish_hls_generation(",
            "async fn hls_payload_size(",
        );
        assert!(publish.contains("register_retrieve_cancel_token("));
        assert!(!publish.contains("retrieve_data"));
        assert!(!publish.contains("chunk_port"));
        assert!(!publish.contains("range_port"));
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
        assert!(hls_prefetch.contains("hls_prefetch_ahead_limit_bytes()"));
        assert!(hls_prefetch.contains("if planned_bytes >= ahead_limit_bytes"));
        assert!(hls_prefetch.contains("planned_bytes < ahead_limit_bytes"));
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
        assert!(begin_hls.contains("pending/dispatched reads keep their transport"));

        let window = source_section(
            "async fn read_range_window(",
            "fn spawn_prefetch_media_stages(",
        );
        assert!(window.contains("&stream_key"));
    }

    #[test]
    fn switching_from_hls_reclaims_only_completed_fragments_for_regular_media() {
        let cache = hls_source_section("impl HlsPayloadCache {", "struct PendingHlsPayload");
        let suspend = cache
            .split("fn suspend_completed_retention(")
            .nth(1)
            .expect("completed HLS suspension");
        assert!(suspend.contains("self.retain_completed = false"));
        assert!(suspend.contains("self.order.clear()"));
        assert!(suspend.contains("self.bodies.clear()"));
        assert!(suspend.contains("self.body_bytes = 0"));
        assert!(!suspend.contains("self.pending.clear()"));
        assert!(cache.contains("if self.retain_completed"));

        let transition = source_section(
            "fn replace_bzz_result_view(",
            "pub(crate) fn replace_result_view_contents(",
        );
        assert!(transition.contains("release_hls_for_bzz_view()"));
        assert!(transition.contains("release_bzz_view()"));

        let release = hls_source_section("pub(crate) fn release_hls_for_bzz_view()", "\n    }\n}");
        assert!(release.contains("release_hls_view()"));
        assert!(release.contains("suspend_completed_retention()"));
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
