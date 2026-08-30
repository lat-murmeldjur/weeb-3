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
#[path = "support/source.rs"]
pub mod source;

mod hls_minimal {
    use crate::{
        stream_conventions::HlsStart,
        stream_hls::{
            HLS_LIVE_EDGE_SEGMENTS, HLS_LIVE_SYNC_SEGMENTS, HlsPlaylist, HlsTailFailure,
            hls_payload_mime, hls_progressive_foreground_transition, is_hls_manifest,
        },
    };

    const REFERENCES: [&str; 8] = [
        "1111111111111111111111111111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222222222222222222222222222",
        "3333333333333333333333333333333333333333333333333333333333333333",
        "4444444444444444444444444444444444444444444444444444444444444444",
        "5555555555555555555555555555555555555555555555555555555555555555",
        "6666666666666666666666666666666666666666666666666666666666666666",
        "7777777777777777777777777777777777777777777777777777777777777777",
        "8888888888888888888888888888888888888888888888888888888888888888",
    ];

    fn playlist(count: usize, finalized: bool) -> Vec<u8> {
        let mut body = String::from(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n",
        );
        for reference in REFERENCES.iter().take(count) {
            body.push_str(&format!("#EXTINF:4.166667,\n{reference}\n"));
        }
        if finalized {
            body.push_str("#EXT-X-ENDLIST\n");
        }
        body.into_bytes()
    }

    fn window(sequence: u64, references: &[&str]) -> HlsPlaylist {
        let mut body =
            format!("#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n");
        for reference in references {
            body.push_str(&format!("#EXTINF:4.0,\n{reference}\n"));
        }
        HlsPlaylist::parse(body.as_bytes()).unwrap()
    }

    #[test]
    fn live_plan_loads_the_exact_three_segment_runway() {
        assert_eq!(HLS_LIVE_SYNC_SEGMENTS, 3);
        assert_eq!(HLS_LIVE_EDGE_SEGMENTS, 3);
        let parsed = HlsPlaylist::parse(&playlist(6, false)).unwrap();
        let plan = parsed.startup_plan(HlsStart::Live).unwrap();
        assert_eq!(plan.bootstrap_position, 0.0);
        assert!(plan.codec_bootstrap);
        assert!((plan.play_position - 12.500001).abs() < 0.000_01);
        assert!((plan.runway_end - 25.000002).abs() < 0.000_01);
        assert!((plan.duration - parsed.duration()).abs() < 0.000_01);
        assert!(
            !window(4, &REFERENCES[..4])
                .startup_plan(HlsStart::Live)
                .unwrap()
                .codec_bootstrap
        );
    }

    #[test]
    fn beginning_plan_primes_only_the_first_segment() {
        let parsed = HlsPlaylist::parse(&playlist(6, false)).unwrap();
        let plan = parsed.startup_plan(HlsStart::Beginning).unwrap();
        assert_eq!(plan.bootstrap_position, 0.0);
        assert!(!plan.codec_bootstrap);
        assert_eq!(plan.play_position, 0.0);
        assert!((plan.runway_end - 4.166667).abs() < 0.000_01);
        assert!((plan.duration - parsed.duration()).abs() < 0.000_01);
    }

    #[test]
    fn commit337_foreground_transition_ignores_sequential_and_cached_backward_reads() {
        assert_eq!(
            hls_progressive_foreground_transition(4, 5, false),
            (false, 5)
        );
        assert_eq!(
            hls_progressive_foreground_transition(5, 5, false),
            (false, 5)
        );
        assert_eq!(
            hls_progressive_foreground_transition(5, 8, false),
            (true, 8)
        );
        assert_eq!(
            hls_progressive_foreground_transition(5, 2, false),
            (true, 2)
        );
        assert_eq!(
            hls_progressive_foreground_transition(5, 2, true),
            (false, 5)
        );
    }

    #[test]
    fn live_plan_offsets_include_gaps_without_bootstrapping_a_nonzero_window() {
        let body = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:4\n\
             #EXTINF:1,\n{}\n#EXTINF:2,\n#EXT-X-GAP\n{}\n\
             #EXTINF:3,\n{}\n#EXTINF:4,\n{}\n#EXTINF:5,\n{}\n",
            REFERENCES[0], REFERENCES[1], REFERENCES[2], REFERENCES[3], REFERENCES[4]
        );
        let plan = HlsPlaylist::parse(body.as_bytes())
            .unwrap()
            .startup_plan(HlsStart::Live)
            .unwrap();
        assert_eq!(plan.bootstrap_position, 0.0);
        assert!(!plan.codec_bootstrap);
        assert_eq!(plan.play_position, 3.0);
        assert_eq!(plan.runway_end, 15.0);
        assert_eq!(plan.duration, 15.0);
    }

    #[test]
    fn authenticated_tail_growth_requires_overlap_and_appends_only_new_media() {
        let mut parsed = HlsPlaylist::parse(&playlist(4, false)).unwrap();
        let tail = format!(
            "truncated\n#EXTINF:4.166667,\n{}\n#EXTINF:4.166667,\n{}\n#EXTINF:4.166667,\n{}\n",
            REFERENCES[3], REFERENCES[4], REFERENCES[5]
        );
        assert_eq!(parsed.merge_tail(tail.as_bytes()), Some(2));
        assert_eq!(parsed.segments.len(), 6);

        let unrelated = "truncated\n#EXTINF:4.0,\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n";
        assert!(parsed.merge_tail(unrelated.as_bytes()).is_none());

        let aligned = format!(
            "#EXTINF:4.166667,\n{}\n#EXTINF:4.166667,\n{}\n",
            REFERENCES[5], "7777777777777777777777777777777777777777777777777777777777777777"
        );
        assert_eq!(parsed.merge_tail(aligned.as_bytes()), Some(1));

        let rewritten_overlap = format!(
            "#EXTINF:9.0,\n{}\n#EXTINF:4.166667,\n{}\n",
            REFERENCES[6], REFERENCES[7]
        );
        assert!(parsed.merge_tail(rewritten_overlap.as_bytes()).is_none());
    }

    #[test]
    fn active_feed_keeps_a_tentative_endlist_reloadable() {
        let mut active = HlsPlaylist::parse(&playlist(3, true)).unwrap();
        assert!(active.finalized);
        active.finalized = false;
        assert!(!active.finalized);
        let rendered =
            String::from_utf8(active.render("/weeb-3/hls/bytes", HlsStart::Beginning)).unwrap();
        assert!(rendered.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(!rendered.contains("#EXT-X-ENDLIST"));
        assert_eq!(rendered.matches("?start=beginning&startup=1").count(), 1);
        assert_eq!(rendered.matches("?start=beginning").count(), 3);

        let next = HlsPlaylist::parse(&playlist(4, true)).unwrap();
        assert_eq!(active.merge_playlist(next), Some(1));
        assert!(active.finalized);
        active.finalized = false;
        assert!(!active.finalized);
    }

    #[test]
    fn rejected_playlist_does_not_mutate_the_active_target_duration() {
        let mut parsed = HlsPlaylist::parse(&playlist(3, false)).unwrap();
        let unrelated = HlsPlaylist::parse(
            b"#EXTM3U\n#EXT-X-TARGETDURATION:99\n#EXTINF:98.0,\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        )
        .unwrap();
        assert!(parsed.merge_playlist(unrelated).is_none());
        assert_eq!(parsed.target_duration, 4);
    }

    #[test]
    fn rendering_preserves_elapsed_duration_and_targets_two_behind_the_edge() {
        let parsed = HlsPlaylist::parse(&playlist(6, false)).unwrap();
        let rendered =
            String::from_utf8(parsed.render("/weeb-3/hls/bytes", HlsStart::Live)).unwrap();
        assert!(rendered.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(rendered.contains("#EXT-X-START:TIME-OFFSET=-12.500001,PRECISE=NO"));
        assert_eq!(rendered.matches("/weeb-3/hls/bytes/").count(), 6);
        assert_eq!(rendered.matches("?start=live").count(), 6);
        assert_eq!(rendered.matches("?start=live&bootstrap=1").count(), 1);
        assert!(rendered.contains(&format!(
            "{}/{}?start=live&bootstrap=1",
            "/weeb-3/hls/bytes", REFERENCES[0]
        )));
        assert!(!rendered.contains("#EXT-X-DISCONTINUITY"));
        assert!((parsed.duration() - 25.000002).abs() < 0.000_01);
    }

    #[test]
    fn gapped_live_tail_keeps_elapsed_time_and_uses_the_required_manifest_version() {
        let mut parsed = HlsPlaylist::parse(&playlist(6, false)).unwrap();
        let elapsed = parsed.duration();
        parsed.segments.last_mut().unwrap().gap = true;
        let rendered =
            String::from_utf8(parsed.render("/weeb-3/hls/bytes", HlsStart::Live)).unwrap();
        assert!(rendered.contains("#EXT-X-VERSION:8"));
        assert_eq!(rendered.matches("#EXT-X-GAP").count(), 1);
        assert_eq!(parsed.duration(), elapsed);
    }

    #[test]
    fn live_tail_fallback_requires_two_strikes_from_the_exact_same_fragment() {
        let mut failures = HlsTailFailure::default();
        let first = REFERENCES[4];
        let second = REFERENCES[5];

        assert!(!failures.record(17, 44, first));
        assert!(failures.record(17, 44, first));

        failures.clear();
        assert!(!failures.record(17, 44, first));
        assert!(
            !failures.record(18, 44, first),
            "a new snapshot is a new strike"
        );
        assert!(failures.record(18, 44, first));

        assert!(
            !failures.record(18, 45, first),
            "a new sequence resets the evidence"
        );
        assert!(failures.record(18, 45, first));

        assert!(
            !failures.record(18, 45, second),
            "a new reference resets the evidence"
        );
        assert!(failures.record(18, 45, second));
    }

    #[test]
    fn presentation_gap_never_mutates_authenticated_history() {
        let canonical = HlsPlaylist::parse(&playlist(6, false)).unwrap();
        let authenticated = canonical.clone();
        let elapsed = canonical.duration();
        let failed_sequence = canonical.sequence + 4;
        let failed_reference = canonical.segments[4].reference.clone();

        let mut presentation = canonical.clone();
        assert!(presentation.mark_gap(failed_sequence, &failed_reference));
        assert_eq!(canonical, authenticated);
        assert!(!canonical.segments[4].gap);
        assert!(presentation.segments[4].gap);
        assert_eq!(presentation.duration(), elapsed);
        let rendered =
            String::from_utf8(presentation.render("/weeb-3/hls/bytes", HlsStart::Live)).unwrap();
        assert!(rendered.contains("#EXT-X-VERSION:8"));
        assert_eq!(rendered.matches("#EXT-X-GAP").count(), 1);

        let mut advanced = canonical;
        assert_eq!(
            advanced.merge_playlist(HlsPlaylist::parse(&playlist(8, false)).unwrap()),
            Some(2),
            "presentation state must not poison authenticated overlap"
        );
        assert!(advanced.segments.iter().all(|segment| !segment.gap));
        let mut advanced_presentation = advanced.clone();
        assert!(advanced_presentation.mark_gap(failed_sequence, &failed_reference));
        assert_eq!(advanced_presentation.segments.len(), 8);
        assert_eq!(advanced_presentation.duration(), advanced.duration());
    }

    #[test]
    fn live_refresh_extends_elapsed_time_without_rebasing_playback() {
        let mut active = HlsPlaylist::parse(&playlist(6, false)).unwrap();
        let initial_plan = active.startup_plan(HlsStart::Live).unwrap();
        let initial_duration = active.duration();
        let original_timeline = active.segments.clone();

        let candidate = HlsPlaylist::parse(&playlist(8, false)).unwrap();
        assert_eq!(active.merge_playlist(candidate), Some(2));
        assert_eq!(&active.segments[..6], original_timeline.as_slice());
        assert!((active.duration() - initial_duration - 8.333334).abs() < 0.000_01);

        let refreshed_plan = active.startup_plan(HlsStart::Live).unwrap();
        assert!(
            (refreshed_plan.play_position - initial_plan.play_position - 8.333334).abs() < 0.000_01
        );
        assert!((refreshed_plan.runway_end - active.duration()).abs() < 0.000_01);
        let rendered =
            String::from_utf8(active.render("/weeb-3/hls/bytes", HlsStart::Live)).unwrap();
        assert!(rendered.contains("#EXT-X-START:TIME-OFFSET=-12.500001,PRECISE=NO"));
        assert_eq!(rendered.matches("/weeb-3/hls/bytes/").count(), 8);
        assert!(!rendered.contains("#EXT-X-DISCONTINUITY"));
    }

    #[test]
    fn rolling_windows_reconstruct_sequence_zero_and_reject_holes() {
        let first = window(0, &REFERENCES[..4]);
        let middle = window(3, &REFERENCES[3..7]);
        let head = window(5, &REFERENCES[5..]);
        let history =
            HlsPlaylist::reconstruct(vec![(5, first.clone()), (15, middle)], 25, head.clone())
                .unwrap();
        assert_eq!(history.sequence, 0);
        assert_eq!(history.segments.len(), 8);
        assert_eq!(history.duration(), 32.0);

        let disconnected = window(5, &[REFERENCES[7]]);
        assert!(HlsPlaylist::reconstruct(vec![(5, first)], 25, disconnected).is_none());
    }

    #[test]
    fn rolling_discontinuity_sequence_preserves_absolute_overlap_identity() {
        let first = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXT-X-DISCONTINUITY-SEQUENCE:4\n#EXTINF:4.0,\n{}\n#EXT-X-DISCONTINUITY\n#EXTINF:4.0,\n{}\n",
            REFERENCES[0], REFERENCES[1]
        );
        let next = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:6\n#EXT-X-DISCONTINUITY-SEQUENCE:5\n#EXTINF:4.0,\n{}\n#EXTINF:4.0,\n{}\n",
            REFERENCES[1], REFERENCES[2]
        );
        let mut active = HlsPlaylist::parse(first.as_bytes()).unwrap();
        assert_eq!(
            active.merge_playlist(HlsPlaylist::parse(next.as_bytes()).unwrap()),
            Some(1)
        );

        let wrong_epoch = next.replace("DISCONTINUITY-SEQUENCE:5", "DISCONTINUITY-SEQUENCE:4");
        assert!(
            active
                .merge_playlist(HlsPlaylist::parse(wrong_epoch.as_bytes()).unwrap())
                .is_none()
        );
        let tail = format!(
            "truncated\n#EXTINF:4.0,\n{}\n#EXTINF:4.0,\n{}\n",
            REFERENCES[2], REFERENCES[3]
        );
        assert_eq!(active.merge_tail(tail.as_bytes()), Some(1));
        assert_eq!(active.segments.last().unwrap().discontinuity_sequence, 5);
        let rendered = String::from_utf8(active.render("/hls/bytes", HlsStart::Live)).unwrap();
        assert!(rendered.contains("#EXT-X-DISCONTINUITY-SEQUENCE:4"));
    }

    #[test]
    fn manifest_and_payload_detection_cover_the_producer_formats() {
        let finalized = playlist(3, true);
        assert!(is_hls_manifest(&finalized));
        let parsed = HlsPlaylist::parse(&finalized).unwrap();
        assert!(parsed.finalized);
        assert_eq!(parsed.sequence, 0);
        let mut transport_stream = vec![0_u8; 376];
        transport_stream[0] = 0x47;
        transport_stream[188] = 0x47;
        assert_eq!(hls_payload_mime(&transport_stream), "video/mp2t");
    }

    #[test]
    fn release_hls_core_stays_small_and_free_of_the_removed_policy_engines() {
        const CORE: &str = include_str!("../src/stream_hls.rs");
        const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
        const RUNTIME: &str = include_str!("../src/stream_hls/runtime.rs");
        let lines = [CORE, PLAYER, RUNTIME]
            .iter()
            .map(|source| {
                source
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count()
            })
            .sum::<usize>();
        assert!(
            lines < 4_000,
            "minimal HLS core grew to {lines} nonblank lines"
        );
        const PAGE: &str = include_str!("../src/stream_hls/page_bridge.rs");
        let page_lines = PAGE.lines().filter(|line| !line.trim().is_empty()).count();
        assert!(
            page_lines < 350,
            "HLS page boundary grew to {page_lines} nonblank lines"
        );
        let worker_split_lines = [
            include_str!("../src/stream_hls/worker_bridge.rs"),
            include_str!("../src/stream_hls/protocol.rs"),
        ]
        .iter()
        .map(|source| {
            source
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
        })
        .sum::<usize>();
        assert!(
            worker_split_lines < 200,
            "HLS worker boundary grew to {worker_split_lines} nonblank lines"
        );
        assert!(!RUNTIME.contains("HtmlMediaElement"));
        assert!(!RUNTIME.contains("SharedNodeClient"));
        let history = RUNTIME
            .split_once("async fn hls_history(")
            .unwrap()
            .1
            .split_once("async fn discover_raw_for_view(")
            .unwrap()
            .0;
        assert!(history.contains("HlsPlaylist::reconstruct(snapshots.clone()"));
        assert!(
            history.find("HlsPlaylist::reconstruct(snapshots.clone()")
                < history.find("let repairs = history_repairs(")
        );
        assert!(!RUNTIME.contains("snapshots.to_vec()"));
        assert!(!CORE.contains("candidate.clone()).is_some()"));
        assert!(PAGE.contains("pub(crate) async fn attach_hls_feed_player("));
        assert!(PAGE.contains("pub(crate) async fn open_hls_feed_view("));
        for removed in [
            "HlsMediaPlanRegistry",
            "HlsProgressiveRunway",
            "SparseHistory",
            "range lease",
            "codec-bootstrap=",
        ] {
            assert!(!format!("{CORE}{PLAYER}{RUNTIME}").contains(removed));
        }
    }

    #[test]
    fn network_recovery_and_native_fallback_keep_the_planned_gate() {
        const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
        assert!(PLAYER.contains("RecoverNetwork(Hls, f64)"));
        assert!(PLAYER.contains("ReloadSource(Hls, String)"));
        assert!(PLAYER.contains("HardRestart(String)"));
        assert!(PLAYER.contains("const MAX_HARD_RESTARTS: u8 = 2"));
        assert!(PLAYER.contains("hard_restarts: u8"));
        assert!(PLAYER.contains("callback: Closure<dyn FnMut(JsValue, JsValue)>"));
        assert!(PLAYER.contains("hls_class: JsValue"));
        assert!(PLAYER.contains("player.plan.play_position"));
        assert!(PLAYER.contains("Action::RecoverNetwork(hls, position)"));
        assert!(PLAYER.contains("hls.start_load_at(position)"));
        assert!(PLAYER.contains("finish_hls_action(id"));
        assert!(PLAYER.contains("Action::ReloadSource(hls, source) =>"));
        assert!(PLAYER.contains("hls.load_source(&source)"));
        assert!(!PLAYER.contains("missing_video_source_buffer_error"));
        assert!(!PLAYER.contains("RestartAfterEvent"));
        assert!(!PLAYER.contains("HlsJsTrackRemovedError"));
        assert!(PLAYER.contains("player.codec_bootstrap_pending = player.plan.codec_bootstrap"));
        assert!(PLAYER.contains("fn hard_restart(id: u64, message: String)"));
        assert!(PLAYER.contains("construct_hls(&player.hls_class"));
        assert!(PLAYER.contains("std::mem::replace(&mut player.hls"));
        assert!(PLAYER.contains("remove_hls_events(&player.hls"));
        assert!(PLAYER.contains("let _ = retired.destroy()"));
        assert!(PLAYER.contains("hls.attach_media(&media)"));
        assert!(PLAYER.contains("fn is_current_hls(id: u64, hls: &Hls)"));
        let restart = PLAYER
            .split_once("fn hard_restart(id: u64, message: String)")
            .unwrap()
            .1
            .split_once("fn start_beginning_history_when_safe(")
            .unwrap()
            .0;
        assert!(restart.contains("player.ready.then(|| player.media.current_time())"));
        assert!(!PLAYER.contains("SeekGate"));
        assert!(restart.contains(".unwrap_or(player.plan.play_position)"));
        assert!(!restart.contains("player.plan.duration = player.plan.duration.max(duration)"));
        assert!(!restart.contains("reload_position\n            .take()"));
        assert!(restart.contains("(position + runway).min(player.plan.duration)"));
        assert!(restart.find("std::mem::replace") < restart.find("reload_position = None"));
        assert!(!restart.contains("player.seek"));
        assert!(!restart.contains("tail_failure.clear()"));
        assert!(PLAYER.contains("return play_native(id, media, source, plan, start)"));
        assert!(PLAYER.contains("media.set_src(source)"));
        assert!(PLAYER.contains("handle_native_event"));
        assert!(PLAYER.contains("buffered_covers("));
        assert!(PLAYER.contains(".load_source(source)"));
        assert!(PLAYER.contains(".and_then(|_| hls.attach_media(&media))"));
    }

    #[test]
    fn rust_body_prefetch_singleflights_the_live_runway() {
        const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
        const RUNTIME: &str = include_str!("../src/stream_hls/runtime.rs");
        assert!(!PLAYER.contains("startup_request("));
        assert!(!PLAYER.contains("warm_live_runway"));
        assert!(!PLAYER.contains("warm_reference_head"));
        assert!(PLAYER.contains("set(&config, \"startFragPrefetch\", JsValue::FALSE)"));
        assert!(
            RUNTIME.contains("const BODY_PREFETCH_HORIZON: usize = HLS_LIVE_BODY_RUNWAY_SEGMENTS;")
        );
        assert!(RUNTIME.contains("pending_bodies: HashMap<String, PendingBody>"));
        assert!(RUNTIME.contains("generation: Option<u64>"));

        let ownership = RUNTIME
            .split_once("fn body_load(")
            .unwrap()
            .1
            .split_once("\n    fn pending_body(")
            .unwrap()
            .0;
        assert!(ownership.contains("BodyLoad::Cached(body.clone())"));
        assert!(ownership.contains("pending.waiters.push(sender)"));
        assert!(ownership.contains("BodyLoad::Wait(receiver)"));
        assert!(ownership.contains("BodyLoad::Lead(self.epoch)"));

        let body = RUNTIME
            .split_once("async fn hls_body(")
            .unwrap()
            .1
            .split_once("\n}\n\nfn prefetch_bodies(")
            .unwrap()
            .0;
        assert!(body.contains("BodyLoad::Wait(waiter) => return waiter.recv().await"));
        assert!(body.contains("BodyLoad::Lead(epoch) => epoch"));
        assert!(body.contains("retrieve_data_range_from_root(root, 0, end"));
        assert!(body.contains("finish_body(reference, epoch, body)"));

        let range = RUNTIME
            .split_once("async fn hls_range(")
            .unwrap()
            .1
            .split_once("\n}\n\nasync fn hls_body(")
            .unwrap()
            .0;
        assert!(range.contains("pending_body(&reference)"));
        assert!(range.contains("waiter.recv().await"));
        assert!(range.contains("body.get(start..end)"));

        let runway = RUNTIME
            .split_once("fn prefetch_playlist_runway(")
            .unwrap()
            .1
            .split_once("\n}\n\nfn prefetch_from_reference(")
            .unwrap()
            .0;
        assert!(runway.contains("HlsStart::Live => playlist"));
        assert!(runway.contains(".rev()"));
        assert!(runway.contains(".take(HLS_LIVE_EDGE_SEGMENTS)"));
        assert!(runway.contains("references.reverse()"));
        assert!(runway.contains(
            "prefetch_priority_runway(client, references.clone(), start, generation, false)"
        ));
    }

    #[test]
    fn player_has_one_gated_autoplay_path_and_room_for_a_growing_buffer() {
        const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
        let suppress = PLAYER
            .find("let restore_autoplay = suspend_autoplay(&media);")
            .unwrap();
        let attach = PLAYER
            .find(".and_then(|_| hls.attach_media(&media))")
            .unwrap();
        assert!(
            suppress < attach,
            "autoplay must be disabled before MSE attach"
        );
        assert!(PLAYER.contains("media.remove_attribute(\"autoplay\")"));
        assert!(
            PLAYER.contains(
                "playback_start_position(&player.media, &player.plan, player.live, false)"
            )
        );
        assert!(PLAYER.contains("event == \"durationchange\""));
        assert!(PLAYER.contains("MediaAction::Begin(position) => begin_playback"));
        assert!(PLAYER.contains("event == \"timeupdate\""));
        assert!(!PLAYER.contains("Ok(_) => set_state(&media, \"playing\""));
        assert!(PLAYER.contains("(180.0, 600.0)"));
        assert!(PLAYER.contains("const LIVE_RUNWAY_BUFFER: (f64, f64) = (90.0, 120.0)"));
        assert!(PLAYER.contains("set(&config, \"autoStartLoad\", JsValue::FALSE)"));
        assert!(PLAYER.contains("set(&config, \"startFragPrefetch\", JsValue::FALSE)"));
        assert!(PLAYER.contains("set(&config, \"progressive\", JsValue::TRUE)"));
        assert!(PLAYER.contains(
            "duration.is_finite() && duration + BUFFER_EPSILON_SECONDS >= plan.duration"
        ));
        assert!(PLAYER.contains("buffered_covers(media, plan.play_position, buffer_end)"));
        assert_eq!(PLAYER.matches("media.play()").count(), 1);
    }

    #[test]
    fn hls_seeks_use_the_media_cursor_without_a_generic_pause_gate() {
        const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
        assert!(PLAYER.contains(
            "const MEDIA_LIFECYCLE_EVENTS: [&str; 3] = [\"play\", \"timeupdate\", \"durationchange\"]"
        ));
        for pause_gate in [
            "\"seeking\"",
            "\"seeked\"",
            "SeekGate",
            "settle_seek",
            "HLS seek runway",
        ] {
            assert!(!PLAYER.contains(pause_gate));
        }
        let lifecycle = PLAYER
            .split_once("fn media_action(")
            .unwrap()
            .1
            .split_once("fn apply_media_action(")
            .unwrap()
            .0;
        assert!(lifecycle.contains("event == \"play\" && !ready"));
        assert!(lifecycle.contains("event == \"timeupdate\""));
        assert!(lifecycle.contains("*clock_origin + CLOCK_ADVANCE_EPSILON_SECONDS"));
    }

    #[test]
    fn codec_bootstrap_primes_from_the_first_playable_segment() {
        const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
        const RUNTIME: &str = include_str!("../src/stream_hls/runtime.rs");
        const WORKER_BRIDGE: &str = include_str!("../src/stream_hls/worker_bridge.rs");
        assert!(!PLAYER.contains("hlsBufferCreated"));
        assert!(PLAYER.contains("codec_bootstrap_pending"));
        assert!(!PLAYER.contains("has_video_track"));
        assert!(PLAYER.contains("is_main_fragment(data)"));
        assert!(PLAYER.contains("player.plan.bootstrap_position"));
        assert!(PLAYER.contains("player.plan.play_position"));
        assert!(PLAYER.contains("hlsBufferAppended"));
        assert!(PLAYER.contains("const MAX_CONSECUTIVE_MEDIA_RECOVERIES: u8 = 1"));
        assert!(PLAYER.contains("fragLoadError"));
        assert!(PLAYER.contains("fragLoadTimeOut"));
        assert!(PLAYER.contains("fragParsingError"));
        assert!(PLAYER.contains("failed_media_identity(data)"));
        assert!(PLAYER.contains("resolve_remote_tail_failure("));
        assert!(
            PLAYER.contains("super::page_bridge::resolve_live_tail_failure(sequence, &reference)")
        );
        assert!(WORKER_BRIDGE.contains(".record(snapshot, sequence, &reference)"));
        assert!(RUNTIME.contains("pub(crate) fn live_tail_failure_identity("));
        assert!(RUNTIME.contains("pub(crate) fn install_live_tail_fallback("));
        assert!(RUNTIME.contains("let retreat = (0..failed)"));
        assert!(RUNTIME.contains("const LIVE_TAIL_FALLBACK_LIMIT: usize = 4"));
        assert!(RUNTIME.contains("const LIVE_TAIL_FALLBACK_WINDOW_MS: f64 = 300_000.0"));
        assert!(RUNTIME.contains("active.tail_fallbacks.len() >= LIVE_TAIL_FALLBACK_LIMIT"));
        assert!(RUNTIME.contains("Some(target)"));
        assert!(RUNTIME.contains("presentation_gaps"));
        assert!(
            RUNTIME.contains("if start == HlsStart::Live && !feed.presentation_gaps.is_empty()")
        );
        assert!(RUNTIME.contains("let mut presentation = playlist.clone()"));
        assert!(RUNTIME.contains("playlist.render(local_bytes_base, start)"));
        assert!(RUNTIME.contains("presentation.mark_gap(*sequence, reference)"));
        assert!(RUNTIME.contains(".take(HLS_LIVE_EDGE_SEGMENTS)"));
        assert!(PLAYER.contains("player.source.clone()"));
        assert!(PLAYER.contains("Action::ReloadSource("));
        assert!(PLAYER.contains("player.reload_position = Some(target)"));
        assert!(
            PLAYER
                .contains("fatal_recovery_action(player, error_type.as_deref(), details, restart)")
        );
        assert!(PLAYER.contains("if player.reload_position.is_some()"));
        assert!(PLAYER.contains("return Action::HardRestart(details)"));
        assert!(!PLAYER.contains("internal_seek"));
        assert!(PLAYER.contains("player.consecutive_media_recoveries += 1"));
        assert!(PLAYER.contains("player.consecutive_media_recoveries = 0"));
        assert!(PLAYER.contains("if event == \"hlsFragBuffered\""));

        let manifest = PLAYER.find("\"hlsManifestParsed\" =>").unwrap();
        let bootstrap = PLAYER[manifest..]
            .find("player.plan.bootstrap_position")
            .unwrap()
            + manifest;
        let recovery = PLAYER[manifest..]
            .find("player.reload_position.take()")
            .unwrap()
            + manifest;
        let handoff = PLAYER
            .find("\"hlsFragBuffered\" if player.codec_bootstrap_pending")
            .unwrap();
        let tail = PLAYER[handoff..].find("player.plan.play_position").unwrap() + handoff;
        assert!(
            manifest < recovery && recovery < bootstrap && bootstrap < handoff && handoff < tail
        );

        let reload = PLAYER
            .split_once("Action::ReloadSource(hls, source) =>")
            .unwrap()
            .1
            .split_once("Action::RecoverMedia(hls) =>")
            .unwrap()
            .0;
        assert!(reload.contains("hls.stop_load()"));
        assert!(reload.contains("hls.load_source(&source)"));
        assert!(!reload.contains("media.set_current_time(position)"));
        assert!(PLAYER.contains("player.media.set_current_time(position)"));
        assert!(reload.contains("finish_hls_action("));
        assert!(!reload.contains("start_load_at"));

        let lifecycle = PLAYER
            .split_once("fn handle_media_event(")
            .unwrap()
            .1
            .split_once("fn media_action(")
            .unwrap()
            .0;
        assert!(!lifecycle.contains("seeking"));
        assert!(!lifecycle.contains("seeked"));
        let duration_retry = PLAYER
            .split_once("if event == \"durationchange\"")
            .unwrap()
            .1
            .split_once("let ready = player.ready;")
            .unwrap()
            .0;
        assert!(duration_retry.contains("&& !player.codec_bootstrap_pending"));
        let ready_gate = handoff + PLAYER[handoff..].find("if !player.ready =>").unwrap();
        let handoff_path = &PLAYER[handoff..ready_gate];
        let cleared = handoff_path
            .find("player.codec_bootstrap_pending = false")
            .unwrap();
        let recheck = handoff_path
            .find("playback_start_position(&player.media, &player.plan, player.live, false)")
            .unwrap();
        let play = handoff_path
            .find("Action::Play(player.media.clone(), position)")
            .unwrap();
        assert!(cleared < recheck && recheck < play);
        assert!(
            handoff_path.contains("Action::Retarget(\n                        player.hls.clone()")
        );
        assert!(PLAYER.contains("Action::Retarget(hls, media, position) =>"));
        assert!(PLAYER.contains("hls.stop_load()"));
        assert!(PLAYER.contains("let target = position + BUFFER_EPSILON_SECONDS"));
        assert!(PLAYER.contains("media.set_current_time(target)"));
        assert!(PLAYER.contains("hls.start_load_at(target)"));
        assert!(PLAYER.contains("if start == HlsStart::Live {\n        LIVE_RUNWAY_BUFFER"));
        assert!(PLAYER.matches("media.set_current_time(position)").count() >= 1);
        let play_gate = &PLAYER[PLAYER.find("fn begin_playback(").unwrap()
            ..PLAYER.find("fn resume_playback(").unwrap()];
        assert!(play_gate.contains("!current.is_finite()"));
        assert!(play_gate.contains("> BUFFER_EPSILON_SECONDS + CLOCK_ADVANCE_EPSILON_SECONDS"));
    }

    #[test]
    fn live_follow_prefetches_new_segments_without_rebasing_the_player() {
        const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
        const RUNTIME: &str = include_str!("../src/stream_hls/runtime.rs");
        assert!(!PLAYER.contains("pending_live_plan"));
        assert!(!PLAYER.contains("hlsLevelUpdated"));
        assert!(!PLAYER.contains("hlsLevelLoaded"));
        assert!(!PLAYER.contains("queue_live_plan"));
        assert!(!RUNTIME.contains("queue_live_plan"));
        assert!(!PLAYER.contains("live_runway_refresh_pending"));
        assert!(!PLAYER.contains("loaded_live_runway"));
        assert!(!PLAYER.contains("latest_live_duration"));
        assert!(!PLAYER.contains("live_startup_pending"));
        assert!(!RUNTIME.contains("warm_reference_head"));
        assert!(PLAYER.contains("live_runway_locked"));
        assert!(PLAYER.contains("lock_latest_live_plan(player)"));
        assert!(RUNTIME.contains("pub(crate) fn lock_live_startup_plan()"));
        assert!(RUNTIME.contains("active.live_foreground = latest_live_foreground(active)"));

        let lock = PLAYER
            .split_once("fn lock_latest_live_plan(")
            .unwrap()
            .1
            .split_once("fn playback_runway(")
            .unwrap()
            .0;
        let optional_plan = lock.find("if let Some(plan) = plan").unwrap();
        let locked = lock.find("player.live_runway_locked = true").unwrap();
        let fallback = lock.find("playback_start_position(").unwrap();
        assert!(optional_plan < locked && locked < fallback);
        assert!(lock.contains("let changed = !was_locked"));
        assert!(lock.contains("} else if changed {"));
        assert!(!lock.contains("Could not lock the live HLS startup position"));

        let update = RUNTIME
            .split_once("fn apply_update(")
            .unwrap()
            .1
            .split_once("\n}\n\nfn apply_full_update(")
            .unwrap()
            .0;
        assert!(update.contains("let appended = merge(playlist)?"));
        assert!(update.contains("playlist.finalized = false"));
        assert!(update.contains("Some((appended, active.start == HlsStart::Live))"));
        assert!(update.contains("if updated.0 != 0 && updated.1"));
        assert!(update.contains("spawn_live_runway(id)"));
    }

    #[test]
    fn live_start_accepts_a_full_buffered_runway_when_hls_skips_the_planned_one() {
        const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
        let gate = PLAYER
            .split_once("fn playback_start_position(")
            .unwrap()
            .1
            .split_once("fn playback_runway(")
            .unwrap()
            .0;
        assert!(gate.contains("let buffer_end = plan.runway_end"));
        assert!(gate.contains("let runway = plan.runway_end - plan.play_position"));
        assert!(gate.contains("(0..ranges.length()).rev().find_map"));
        assert!(gate.contains("let candidate = range_start.max(plan.play_position)"));
        assert!(gate.contains("range_end + BUFFER_EPSILON_SECONDS >= candidate + runway"));
        assert!(gate.contains(".then_some(candidate)"));
    }

    #[test]
    fn beginning_and_seek_extend_the_rust_owned_body_runway() {
        const PLAYER: &str = include_str!("../src/stream_hls/player.rs");
        const RUNTIME: &str = include_str!("../src/stream_hls/runtime.rs");
        assert!(!PLAYER.contains("hlsFragLoading"));
        assert!(!PLAYER.contains("fragment_reference("));
        assert!(PLAYER.contains("set(&config, \"startFragPrefetch\", JsValue::FALSE)"));

        let successor = RUNTIME
            .split_once("fn prefetch_from_reference(")
            .unwrap()
            .1
            .split_once("\n}\n\nfn next_feed_id(")
            .unwrap()
            .0;
        assert!(successor.contains(".position(matches)"));
        assert!(successor.contains(".rfind(|(position, segment)|"));
        assert!(successor.contains("live_segment_is_playable(active, *position)"));
        assert!(successor.contains("let playable = playlist.segments.iter().filter"));
        assert!(successor.contains("playable\n            .skip(position)"));
        assert!(successor.contains(".take(BODY_PREFETCH_HORIZON)"));
        assert!(successor.contains("active.live_foreground = Some(reference.to_string())"));
        assert!(successor.contains("Some((active.id, None))"));
        assert!(successor.contains(
            "prefetch_priority_runway(client, references, HlsStart::Beginning, None, cached)"
        ));
        assert!(successor.contains("hls_progressive_foreground_transition"));
        assert!(successor.contains("let playable = playlist.segments.iter().filter"));
        assert!(successor.contains("let successor = transition"));
        assert!(successor.contains("spawn_live_runway(id)"));

        let response = RUNTIME
            .split_once("async fn fetch_hls_body_response(")
            .unwrap()
            .1
            .split_once("\n}\n\nfn parse_hls_range(")
            .unwrap()
            .0;
        assert!(response.contains("method == \"GET\" && range.is_none() && !codec_bootstrap"));
        assert!(response.contains("prefetch_from_reference(&reference, cached)"));
        assert!(response.contains("if let Some(successor) = seek_successor"));
        assert!(response.contains("foreground_hls_body(client.clone(), reference.clone(), None)"));
        assert!(response.contains("hls_body(client.clone(), successor, None).await"));
        assert!(response.contains("let mime = if codec_bootstrap"));
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
        assert!(if_none_match_matches(Some(""), " "));
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
    use crate::source::between as source_between;

    const STATIC_WORKER: &str = include_str!("../static/service.js");
    const INTERFACE: &str = include_str!("../src/interface.rs");
    const INTERFACE_RUNTIME: &str = include_str!("../src/interface_runtime_conventions.rs");
    const LIBRARY: &str = include_str!("../src/library.rs");
    const NAV: &str = include_str!("../src/nav.rs");
    const STATIC_404: &str = include_str!("../static/404.html");
    const MAIN_SERVER: &str = include_str!("../src/main.rs");

    #[test]
    fn interface_mount_waits_for_the_shared_node_before_reading_routes() {
        let npm_start = source_between(
            LIBRARY,
            "fn schedule_start(&self, options: StartOptions)",
            "async fn boot_runtime(&self)",
        );
        assert!(!npm_start.contains("install_service_worker_message_bridge"));
        assert!(npm_start.contains("get_service_worker().await"));
        assert!(npm_start.contains("configure_shared_node(&inner"));

        let implicit_start = source_between(
            LIBRARY,
            "fn start_options_from_js(options: Option<JsValue>)",
            "async fn call_promise(",
        );
        assert!(implicit_start.contains("route_network_mode_from_location()"));
        assert!(INTERFACE_RUNTIME.contains("weeb3.configure(expected_network_id, dial_requests)"));
        let mount = source_between(
            INTERFACE,
            "pub(crate) async fn mount_interface_with_generation(",
            "let mut last_progress_revision",
        );
        assert!(
            mount.find("weeb3.ensure().await").unwrap()
                < mount.find("let Some(route) = read_route()").unwrap()
        );
        assert!(!mount.contains("connect_all_bootnode_settings("));
        assert!(!mount.contains("if read_initial_routes"));
    }

    #[test]
    fn optional_interface_callback_is_retained_without_polling_it() {
        assert!(INTERFACE.contains("let _retained_message_callback = if let Some(button)"));
        assert!(INTERFACE.contains("Some(callback_prereq)"));
        assert!(!INTERFACE.contains("retained_message_callbacks"));
        assert!(!INTERFACE.contains("_retained_callback_count"));
    }

    #[test]
    fn every_network_switch_retires_the_active_stream_before_reconfiguration() {
        let npm_start = source_between(
            LIBRARY,
            "fn schedule_start(&self, options: StartOptions)",
            "async fn boot_runtime(&self)",
        );
        let npm_release = npm_start
            .find("crate::stream::release_current_stream_view()")
            .expect("npm network switch stream release");
        let npm_switch = npm_start
            .find("configure_shared_node(&inner")
            .expect("npm network switch");
        assert!(npm_release < npm_switch);

        let settings = source_between(
            INTERFACE_RUNTIME,
            "pub(super) async fn apply_network_settings_and_connect(",
            "pub(super) fn current_network_id_input()",
        );
        let settings_release = settings
            .find("crate::stream::release_current_stream_view()")
            .expect("settings network switch stream release");
        let settings_switch = settings
            .find("connect_all_bootnode_settings(")
            .expect("settings network switch");
        assert!(settings_release < settings_switch);

        let api_switch = source_between(
            LIBRARY,
            "pub async fn switch_network(&self, mode: String)",
            "#[wasm_bindgen(js_name = retrieve)]",
        );
        let api_release = api_switch
            .find("crate::stream::release_current_stream_view()")
            .expect("switchNetwork stream release");
        let api_set = api_switch
            .find("configure_shared_node(&self.inner")
            .expect("switchNetwork network switch");
        assert!(api_release < api_set);
    }

    #[test]
    fn npm_render_mount_cannot_reopen_a_superseded_route() {
        let render = source_between(
            LIBRARY,
            "pub fn render_interface(&self, container: Element)",
            "#[wasm_bindgen(js_name = attachStream)]",
        );
        assert!(render.contains("self.startup.begin()"));
        assert!(render.contains("let guard = startup.serial.lock().await"));
        assert!(render.contains("route_network_mode_from_location()"));
        assert!(render.contains("startup.finish(None)"));
        assert!(render.contains("Some(initial_result_generation)"));

        let dial = render.find("configure_shared_node(&s").unwrap();
        let release = render.find("startup.finish(None)").unwrap();
        assert!(dial < release);

        let mount = source_between(
            INTERFACE,
            "pub(crate) async fn mount_interface_with_generation(",
            "let mut last_progress_revision",
        );
        assert!(mount.contains("initial_result_generation: Option<u64>"));
        assert!(mount.contains("result_view_request_is_current(generation)"));
    }

    #[test]
    fn originating_window_relay_is_cached_after_one_network_checked_ping() {
        let selection = source_between(
            STATIC_WORKER,
            "function isStableWindowClient(client)",
            "function closeMessagePort(",
        );
        assert!(selection.contains("async function windowNetworkId(client)"));
        assert!(selection.contains("{ type: \"WEEB3_CLIENT_PING\" }"));
        assert!(selection.contains("response?.type !== \"WEEB3_CLIENT_PONG\""));
        assert!(selection.contains("response?.sharedWorkerProtocol"));
        assert!(
            selection.contains("async function originatingWindow(clientId, resultingClientId)")
        );
        assert!(selection.contains("for (const id of [clientId, resultingClientId])"));
        assert!(selection.contains("windowNetworkDiscoveries.get(client.id)"));
        assert!(selection.contains("cachedWindowNetworks.set(client.id, networkId)"));
        assert!(selection.contains("allowFallback = true"));
        assert!(selection.contains("includeUncontrolled: false"));
        assert!(!selection.contains("includeUncontrolled: true"));
        assert_eq!(STATIC_WORKER.matches("WEEB3_CLIENT_PING").count(), 1);

        let fetch = source_between(
            STATIC_WORKER,
            "self.addEventListener(\"fetch\", (event) => {",
            "function isStableWindowClient(client)",
        );
        assert!(fetch.contains("event.clientId, event.resultingClientId"));
        assert!(
            STATIC_WORKER.contains("requestClient(networkId, clientId, resultingClientId, false)")
        );
    }

    #[test]
    fn worker_routes_only_explicit_requests_to_the_matching_network() {
        let route_network = source_between(
            STATIC_WORKER,
            "function canonicalRouteNetworkId(pathname)",
            "function isNetworkShellPath(pathname)",
        );
        assert!(route_network.contains("if (!pathname.startsWith(SCOPE_PATH))"));
        assert!(route_network.contains("const relative = pathname.substring(SCOPE_PATH.length)"));
        assert!(
            route_network
                .contains("relative === \"testnet\" || relative.startsWith(\"testnet/\") ? 10 : 1")
        );

        let fetch_handler = source_between(
            STATIC_WORKER,
            "self.addEventListener(\"fetch\", (event) => {",
            "function isStableWindowClient(client)",
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
        assert!(raw_routes.contains("for (const [marker, rawType] of RAW_ROUTE_MARKERS)"));
        assert!(raw_routes.contains(r#"rawType === "hls-bytes" && !isSwarmReference(resource)"#));
        assert!(raw_routes.contains("return resource;"));
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
            "function isStableWindowClient(client)",
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
mod stream_reader_concurrency {
    use crate::source::between;

    const STREAMING_PLAYER: &str = include_str!("../src/stream.rs");
    const PREFETCH_POLICY: &str = include_str!("../src/stream_conventions.rs");

    const MIB_BYTES: u64 = 1024 * 1024;
    const STARTUP_RESPONSE_BYTES: u64 = 8 * MIB_BYTES;
    const SEEK_REQUEST_GAP_BYTES: u64 = 6 * MIB_BYTES;
    const SEEK_RESET_GAP_BYTES: u64 = 16 * MIB_BYTES;

    fn source_section(start: &str, end: &str) -> &'static str {
        between(STREAMING_PLAYER, start, end)
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
    fn stable_and_seekable_reads_have_separate_pending_slots() {
        let keying = source_section(
            "fn pending_range_key(",
            "fn range_storage_window_for_start(",
        );
        assert!(keying.contains("\"{cache_key}|pending:stable\""));
        assert!(keying.contains("\"{cache_key}|pending:media\""));

        let window = source_section(
            "async fn read_range_window(",
            "fn spawn_prefetch_media_stages(",
        );
        assert!(window.contains("range_load_role(&cache_key, &pending_key, generation)"));
        let moved_key = window.find("let leader_cache_key = cache_key;").unwrap();
        let remembered = window.find("remember_range(").unwrap();
        let completed = window.find("finish_pending_range(").unwrap();
        assert!(moved_key < remembered && remembered < completed);
        assert!(window[completed..].contains("&leader_pending_key"));
    }

    #[test]
    fn page_media_events_do_not_mutate_the_worker_owned_fetch_cache() {
        assert!(!STREAMING_PLAYER.contains("ACTIVE_BZZ_MEDIA_URL"));
        assert!(!STREAMING_PLAYER.contains("reset_bzz_fetch_url_activity"));
        assert!(!STREAMING_PLAYER.contains("suspend_bzz_fetch_url_prefetch"));
        assert!(!STREAMING_PLAYER.contains("BZZ_MEDIA_PLAYING"));

        let retry = source_section("fn install_play_retries(", "fn schedule_media_retry(");
        assert!(retry.contains("MediaRetryState"));
        assert!(!retry.contains("FETCH_CACHE"));
    }
}
