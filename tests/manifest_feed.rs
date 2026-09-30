#![allow(dead_code)]

#[path = "support/source.rs"]
mod source;

#[path = "../src/feed.rs"]
mod feed;
#[path = "../src/manifest.rs"]
mod manifest;
#[path = "../src/stream_conventions.rs"]
mod stream_conventions;
#[path = "../src/stream_hls.rs"]
mod stream_hls;

struct ActiveProbe<'a>(&'a std::cell::Cell<usize>);

impl Drop for ActiveProbe<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

mod hls_formats {
    use crate::{
        stream_conventions::HlsStart,
        stream_hls::{HlsManifest, HlsPlaylist, HlsSource, hls_edge_wave_complete, hls_retreat_position},
    };

    fn reference(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    #[test]
    fn feed_bounds_wait_for_intervening_probes_but_not_distant_pending_negatives() {
        let missing = [false, false, false, true, false, false];
        assert!(hls_edge_wave_complete(2, &missing, &[false, true, true, true, false, false]));
        assert!(!hls_edge_wave_complete(2, &missing, &[false, true, false, true, true, true]));
        assert!(!hls_edge_wave_complete(2, &[false; 6], &[true; 6]));
        // A higher authenticated positive invalidates the earlier missing bound.
        assert!(!hls_edge_wave_complete(5, &missing, &[true; 6]));
    }

    #[test]
    fn producer_master_preserves_every_rendition_and_quoted_attribute() {
        let owner = "ab".repeat(20);
        let text = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-INDEPENDENT-SEGMENTS\n\
             #EXT-X-STREAM-INF:BANDWIDTH=700000,AVERAGE-BANDWIDTH=600000,RESOLUTION=640x360,CODECS=\"avc1.42001e,RESOLUTION=99999x99999,BANDWIDTH=999999999,mp4a.40.2\"\n\
             swarm://{owner}/lower-topic\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2800000,CODECS=\"avc1.64001f,mp4a.40.2\",RESOLUTION=1280x720\n\
             swarm://{owner}/higher-topic\n"
        );
        let Some(HlsManifest::Master(master)) = HlsManifest::parse(text.as_bytes()) else {
            panic!("master");
        };
        assert_eq!(
            master.sources().collect::<Vec<_>>(),
            [
                format!("swarm://{owner}/lower-topic"),
                format!("swarm://{owner}/higher-topic")
            ]
        );
        assert!(HlsPlaylist::parse(text.as_bytes()).is_none());
        assert_eq!(
            master.render(|uri, _| Some(uri.replace("swarm://", "/weeb-3/feeds/"))),
            text.replace("swarm://", "/weeb-3/feeds/")
        );
        assert_eq!(master.render(|_, _| None), text);
        assert_eq!(
            master.initial_source(),
            Some(format!("swarm://{owner}/higher-topic").as_str())
        );
    }

    #[test]
    fn initial_rendition_uses_resolution_then_bandwidth_and_keeps_first_ties() {
        for (variants, expected) in [
            (vec!["BANDWIDTH=9000000,RESOLUTION=640x360", "BANDWIDTH=6000000,RESOLUTION=1920x1080", "BANDWIDTH=7000000,RESOLUTION=1280x720"], 1),
            (vec!["BANDWIDTH=6000000,RESOLUTION=1920x1080", "BANDWIDTH=9000000,RESOLUTION=640x360"], 0),
            (vec!["BANDWIDTH=1000000,RESOLUTION=1280x720", "BANDWIDTH=2000000,RESOLUTION=1280x720"], 1),
            (vec!["BANDWIDTH=3000000", "BANDWIDTH=4000000"], 1),
            (vec!["BANDWIDTH=3000000", "BANDWIDTH=3000000"], 0),
            (vec!["BANDWIDTH=1000,CODECS=\"avc1.64001f,BANDWIDTH=9999999,mp4a.40.2\"", "BANDWIDTH=2000"], 1),
            (vec!["BANDWIDTH=999999999,CODECS=\"mp4a.40.2\"", "BANDWIDTH=1000000,RESOLUTION=640x360"], 1),
            (vec!["BANDWIDTH=5000000,RESOLUTION=0x1080", "BANDWIDTH=1000000,RESOLUTION=640x360"], 1),
            (vec!["BANDWIDTH=5000000,RESOLUTION=4294967296x1080", "BANDWIDTH=1000000,RESOLUTION=640x360"], 1),
        ] {
            let text = format!("#EXTM3U\n{}", variants.iter().enumerate().map(|(index, attributes)|
                format!("#EXT-X-STREAM-INF:{attributes}\nrung-{index}\n")
            ).collect::<String>());
            let Some(HlsManifest::Master(master)) = HlsManifest::parse(text.as_bytes()) else {
                panic!("master");
            };
            assert_eq!(master.initial_source(), Some(format!("rung-{expected}").as_str()), "{variants:?}");
        }
    }

    #[test]
    fn only_the_current_masters_initial_rendition_can_be_reused() {
        let owner = "ab".repeat(20);
        let old = format!("swarm://{owner}/first-rung");
        let replacement = format!("swarm://{owner}/replacement-rung");
        let source = HlsSource::parse(&old).unwrap();
        for (uris, expected) in [
            (vec![old.as_str(), replacement.as_str()], true),
            (vec![replacement.as_str(), old.as_str()], false),
            (vec![replacement.as_str()], false),
        ] {
            let text = format!("#EXTM3U\n{}", uris.iter().map(|uri|
                format!("#EXT-X-STREAM-INF:BANDWIDTH=1000\n{uri}\n")
            ).collect::<String>());
            let Some(HlsManifest::Master(master)) = HlsManifest::parse(text.as_bytes()) else {
                panic!("master");
            };
            assert_eq!(master.selects(&source), expected);
        }
    }

    #[test]
    fn master_audio_iframe_and_session_key_uris_are_rewritten_without_touching_labels() {
        let r = reference('a');
        let text = format!(
            "#EXTM3U\n\
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"URI=not,a,url\",BANDWIDTH=9999999,RESOLUTION=9999x9999,URI=\"/bytes/{r}\"\n\
            #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=9999999,RESOLUTION=9999x9999,URI=\"{r}\"\n\
            #EXT-X-SESSION-KEY:METHOD=AES-128,BANDWIDTH=9999999,RESOLUTION=9999x9999,URI=\"https://keys.example/key\"\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1000,AUDIO=\"a\"\nhttps://example.org/main.m3u8\n"
        );
        let Some(HlsManifest::Master(master)) = HlsManifest::parse(text.as_bytes()) else {
            panic!("master");
        };
        assert_eq!(master.sources().count(), 3);
        assert_eq!(
            master.initial_source(),
            Some("https://example.org/main.m3u8")
        );
        let mut seen = Vec::new();
        let rewritten = master.render(|uri, playlist| {
            seen.push((uri.to_string(), playlist));
            match HlsSource::parse(uri) {
                Some(HlsSource::Reference(reference)) => Some(format!("/local/{reference}")),
                _ => None,
            }
        });
        assert_eq!(
            seen.iter()
                .map(|(_, playlist)| *playlist)
                .collect::<Vec<_>>(),
            [true, true, false, true]
        );
        assert_eq!(rewritten.matches(&format!("URI=\"/local/{r}\"")).count(), 2);
        assert!(rewritten.contains("NAME=\"URI=not,a,url\""));
        assert!(rewritten.contains("URI=\"https://keys.example/key\""));
        assert!(rewritten.ends_with("https://example.org/main.m3u8\n"));
    }

    #[test]
    fn malformed_or_mixed_master_cannot_be_mistaken_for_media() {
        let r = reference('1');
        for text in [
            "#EXTM3U\n#EXT-X-STREAM-INF:\nchild\n".to_string(),
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n".to_string(),
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n#comment\nchild\n".to_string(),
            "#EXTM3U\n#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=1\n".to_string(),
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"unclosed\n".to_string(),
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"a\",URI=\"b\"\n".to_string(),
            format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nchild\n#EXTINF:4,\n{r}\n"),
            "#EXTM3U\n#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nchild\n".to_string(),
        ] {
            assert!(HlsManifest::parse(text.as_bytes()).is_none(), "{text}");
            assert!(HlsPlaylist::parse(text.as_bytes()).is_none(), "{text}");
        }
    }

    #[test]
    fn feed_topic_encoding_index_and_content_roots_are_distinct() {
        let owner = "ab".repeat(20);
        let topic = reference('c');
        for uri in [
            format!("swarm://{owner}/{topic}"),
            format!("{owner}/{topic}"),
        ] {
            assert_eq!(
                HlsSource::parse(&uri),
                Some(HlsSource::Feed {
                    owner: owner.clone(),
                    topic: topic.clone(),
                    topic_is_hash: false,
                    index: None
                })
            );
        }
        for uri in [
            format!("/feeds/{owner}/{topic}?index=19"),
            format!("https://bee.example/feeds/{owner}/{topic}?index=19"),
            format!("/weeb-3/testnet/feeds/{owner}/{topic}?index=19"),
        ] {
            assert_eq!(
                HlsSource::parse(&uri),
                Some(HlsSource::Feed {
                    owner: owner.clone(),
                    topic: topic.clone(),
                    topic_is_hash: true,
                    index: Some(19)
                })
            );
        }
        for r in [topic, "AD".repeat(64)] {
            for uri in [
                r.clone(),
                format!("/bytes/{r}"),
                format!("https://old-gateway/bytes/{r}"),
                format!("/weeb-3/hls/bytes/{r}?start=live"),
            ] {
                assert_eq!(
                    HlsSource::parse(&uri),
                    Some(HlsSource::Reference(r.to_ascii_lowercase()))
                );
            }
        }
        for uri in [
            format!("/feeds/bad/{owner}"),
            format!("/feeds/{owner}/{}?index=-1", reference('a')),
            format!("swarm://{owner}/a/b"),
            format!("swarm://{owner}/a#index"),
            format!("swarm://{owner}/a?index=18446744073709551616"),
        ] {
            assert!(HlsSource::parse(&uri).is_none(), "{uri}");
        }
    }

    #[test]
    fn new_gap_and_dates_round_trip_with_exact_timeline_and_no_fake_reference() {
        let a = reference('a');
        let b = reference('b');
        let text = format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n\
            #EXT-X-PROGRAM-DATE-TIME:2026-09-09T10:00:00.000Z\n#EXTINF:4,\n{a}\n\
            #EXT-X-GAP\n#EXT-X-PROGRAM-DATE-TIME:2026-09-09T10:00:04.000Z\n#EXTINF:4,\ngap-1\n\
            #EXT-X-DISCONTINUITY\n#EXT-X-PROGRAM-DATE-TIME:2026-09-09T10:01:00.000+00:00\n#EXTINF:4.125,\n{b}\n#EXT-X-ENDLIST\n"
        );
        let Some(HlsManifest::Media(playlist)) = HlsManifest::parse(text.as_bytes()) else {
            panic!("media");
        };
        assert_eq!(playlist.duration(), 12.125);
        assert_eq!(playlist.segments[1].reference, "gap-1");
        assert!(playlist.segments[1].gap);
        assert_eq!(playlist.segments[2].discontinuity_sequence, 1);
        assert_eq!(
            playlist.segments[2].program_date_time.as_deref(),
            Some("2026-09-09T10:01:00.000+00:00")
        );
        let rendered = playlist.render("/weeb-3/hls/bytes", HlsStart::Beginning);
        let text = std::str::from_utf8(&rendered).unwrap();
        assert!(text.contains("#EXT-X-GAP\ngap-1\n"));
        assert!(!text.contains("/bytes/gap-1"));
        assert_eq!(text.matches("#EXT-X-PROGRAM-DATE-TIME:").count(), 3);
        assert_eq!(HlsPlaylist::parse(&rendered), Some(playlist));
        assert!(HlsSource::parse("gap-1").is_none());
    }

    #[test]
    fn old_gap_order_and_undated_bare_or_gateway_media_remain_supported() {
        let a = reference('a');
        let b = reference('b');
        let text =
            format!("#EXTM3U\n#EXTINF:4,\nhttps://old/bytes/{a}\n#EXTINF:4,\n#EXT-X-GAP\n{b}\n");
        let playlist = HlsPlaylist::parse(text.as_bytes()).unwrap();
        assert_eq!(playlist.duration(), 8.0);
        assert!(
            playlist
                .segments
                .iter()
                .all(|segment| segment.program_date_time.is_none())
        );
        assert_eq!(playlist.segments[0].reference, a);
        assert_eq!(playlist.segments[1].reference, b);
        assert!(playlist.segments[1].gap);
        assert_eq!(
            HlsPlaylist::parse(&playlist.render("/local", HlsStart::Beginning)),
            Some(playlist)
        );
    }

    #[test]
    fn gap_placeholders_never_make_ordinary_media_or_an_eight_second_runway() {
        let a = reference('a');
        assert!(HlsPlaylist::parse(b"#EXTM3U\n#EXTINF:4,\ngap-1\n").is_none());
        for pending in [
            "#EXT-X-GAP\n",
            "#EXT-X-PROGRAM-DATE-TIME:2026-09-09T10:00:00Z\n",
        ] {
            assert!(
                HlsPlaylist::parse(format!("#EXTM3U\n#EXTINF:4,\n{a}\n{pending}").as_bytes())
                    .is_none()
            );
        }
        let playlist = HlsPlaylist::parse(
            format!("#EXTM3U\n#EXTINF:4,\n{a}\n#EXT-X-GAP\n#EXTINF:4,\ngap-1\n").as_bytes(),
        )
        .unwrap();
        assert!(playlist.anchored_startup_plan(0, &a).is_none());
    }

    #[test]
    fn dates_enrich_undated_overlap_without_replacing_known_authenticated_dates() {
        let a = reference('a');
        let b = reference('b');
        let base = format!("#EXTM3U\n#EXTINF:4,\n{a}\n");
        let dated =
            format!("#EXTM3U\n#EXT-X-PROGRAM-DATE-TIME:2026-09-09T10:00:00Z\n#EXTINF:4,\n{a}\n");
        let mut playlist = HlsPlaylist::parse(base.as_bytes()).unwrap();
        assert_eq!(
            playlist.merge_playlist(HlsPlaylist::parse(dated.as_bytes()).unwrap()),
            Some(0)
        );
        assert_eq!(
            playlist.segments[0].program_date_time.as_deref(),
            Some("2026-09-09T10:00:00Z")
        );
        let appended = dated.replace("10:00:00Z", "10:00:00+00:00")
            + &format!("#EXT-X-PROGRAM-DATE-TIME:2026-09-09T10:00:04Z\n#EXTINF:4,\n{b}\n");
        assert_eq!(
            playlist.merge_playlist(HlsPlaylist::parse(appended.as_bytes()).unwrap()),
            Some(1)
        );
        assert_eq!(
            playlist.segments[0].program_date_time.as_deref(),
            Some("2026-09-09T10:00:00Z")
        );
        let mut suffix = HlsPlaylist::parse(format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1\n#EXT-X-PROGRAM-DATE-TIME:2026-09-09T10:00:04+00:00\n#EXTINF:4,\n{b}\n").as_bytes()).unwrap();
        assert_eq!(suffix.merge_playlist(playlist), Some(0));
        assert_eq!(suffix.sequence, 0);
        assert_eq!(
            suffix.segments[1].program_date_time.as_deref(),
            Some("2026-09-09T10:00:04+00:00")
        );
        assert_eq!(suffix.duration(), 8.0);
    }

    #[test]
    fn legacy_feed_scheme_and_marker_case_remain_accepted() {
        let owner = "AB".repeat(20);
        let topic = "CD".repeat(32);
        assert!(matches!(
            HlsSource::parse(&format!("HTTPS://gateway/FEEDS/{owner}/{topic}?index=7")),
            Some(HlsSource::Feed {
                topic_is_hash: true,
                index: Some(7),
                ..
            })
        ));
        assert!(
            matches!(HlsSource::parse(&format!("SWARM://{owner}/RawTopic")), Some(HlsSource::Feed { topic_is_hash: false, topic, .. }) if topic == "RawTopic")
        );
    }

    fn dated_window(sequence: u64, count: usize, seam: Option<u64>) -> HlsPlaylist {
        let mut text = format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n");
        for offset in 0..count {
            let sequence = sequence + offset as u64;
            if seam == Some(sequence) {
                text.push_str("#EXT-X-DISCONTINUITY\n");
            }
            text.push_str(&format!(
                "#EXT-X-PROGRAM-DATE-TIME:2026-09-22T10:00:{:02}.000Z\n#EXTINF:0.5,\n{sequence:064x}\n",
                sequence % 60
            ));
        }
        HlsPlaylist::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn dated_live_window_starts_without_a_sequence_zero_archive() {
        let mut head = dated_window(90_000, 24, None);
        let plan = head.startup_plan(HlsStart::Live).unwrap();
        assert_eq!(plan.play_position, 0.0);
        assert_eq!(plan.runway_end, 12.0);
        assert!(!plan.codec_bootstrap);
        assert_eq!(head.merge_playlist(dated_window(90_002, 24, None)), Some(2));
        let rendered =
            String::from_utf8(head.render_with_plan("/hls/bytes", HlsStart::Live, Some(&plan)))
                .unwrap();
        assert!(rendered.contains("#EXT-X-MEDIA-SEQUENCE:90000"));
        assert!(rendered.contains(&format!("#EXT-X-START:TIME-OFFSET={:.6}", plan.play_position)));
        assert_eq!(head.duration(), 13.0);
    }

    #[test]
    fn dated_live_clock_uses_the_broadcast_origin_without_reconstructing_history() {
        let parse = |date: &str| {
            (date[11..13].parse::<f64>().unwrap() * 3_600.0
                + date[14..16].parse::<f64>().unwrap() * 60.0
                + date[17..23].parse::<f64>().unwrap()) * 1_000.0
        };
        let mut first = dated_window(0, 1, None);
        first.segments[0].program_date_time = Some("2026-09-23T10:07:25.098Z".into());
        let mut live = dated_window(13_000, 31, None);
        for segment in &mut live.segments { segment.duration = 2.0; }
        live.segments[0].program_date_time = Some("2026-09-23T17:07:25.598Z".into());
        let offset = live.timeline_offset(&first, parse).unwrap();
        assert_eq!(offset, 25_200.5);
        let local = live.startup_plan(HlsStart::Live).unwrap();
        let clock = local.clone().with_offset(offset);
        assert_eq!((clock.play_position, clock.runway_end, clock.duration),
            (25_250.5, 25_262.5, 25_262.5));
        assert_eq!(clock.clone().with_offset(offset), clock);
        assert_eq!(clock.clone().with_offset(0.0), local);
        let seek = clock.at_position(25_230.5, clock.duration, 8.0, false).unwrap();
        assert_eq!((seek.play_position, seek.runway_end, seek.timeline_offset),
            (25_230.5, 25_238.5, offset));
        assert_eq!(seek.clone().with_offset(offset), seek);
        assert_eq!(seek.with_offset(0.0).play_position, 30.0);
        assert!(live.timeline_offset(&first, |_| f64::NAN).is_none());
        first.sequence = 1;
        assert!(live.timeline_offset(&first, parse).is_none());
        first.sequence = 0;
        first.segments[0].program_date_time = Some("2026-09-23T18:07:25.098Z".into());
        assert!(live.timeline_offset(&first, parse).is_none());
        first.segments[0].program_date_time = None;
        assert!(live.timeline_offset(&first, parse).is_none());
    }

    #[test]
    fn new_dated_sequence_zero_starts_directly_while_legacy_keeps_codec_bootstrap() {
        let mut head = dated_window(0, 24, None);
        let plan = head.startup_plan(HlsStart::Live).unwrap();
        assert_eq!((plan.play_position, plan.runway_end), (0.0, 12.0));
        assert!(!plan.codec_bootstrap);
        for segment in &mut head.segments {
            segment.program_date_time = None;
        }
        let legacy = head.startup_plan(HlsStart::Live).unwrap();
        assert_eq!((legacy.play_position, legacy.runway_end), (4.0, 12.0));
        assert!(legacy.codec_bootstrap);
    }

    #[test]
    fn seek_and_restart_keep_codec_history_and_clip_only_finalized_runways() {
        let mut legacy = dated_window(0, 24, None);
        for segment in &mut legacy.segments {
            segment.program_date_time = None;
        }
        let original = legacy.startup_plan(HlsStart::Live).unwrap();
        let live = original.at_position(20.0, f64::INFINITY, 8.0, false).unwrap();
        assert_eq!((live.play_position, live.runway_end, live.duration), (20.0, 28.0, 12.0));
        assert_eq!(live.bootstrap_position, original.bootstrap_position);
        assert!(live.codec_bootstrap);
        let ended = original.at_position(20.0, 24.0, 8.0, true).unwrap();
        assert_eq!((ended.runway_end, ended.duration), (24.0, 24.0));
        assert!(original.at_position(24.0, 24.0, 8.0, true).is_none());
        for position in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(original.at_position(position, 24.0, 8.0, false).is_none());
        }
        for runway in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
            assert!(original.at_position(20.0, 24.0, runway, false).is_none());
        }
        assert_eq!(original, legacy.startup_plan(HlsStart::Live).unwrap());
    }

    #[test]
    fn every_rendition_retains_the_original_viewing_origin_for_later_rewinds() {
        let window = |sequence: u64, count: usize| {
            let mut playlist = dated_window(sequence, count, None);
            for (offset, segment) in playlist.segments.iter_mut().enumerate() {
                let seconds = (sequence + offset as u64) * 2;
                segment.duration = 2.0;
                segment.program_date_time = Some(format!(
                    "2026-09-22T10:{:02}:{:02}.000Z", seconds / 60, seconds % 60
                ));
            }
            playlist
        };
        let parse = |date: &str| {
            (date[14..16].parse::<f64>().unwrap() * 60.0
                + date[17..19].parse::<f64>().unwrap()) * 1_000.0
        };
        let mut playing = window(20, 60);
        // Rendition sequence numbers need not share an origin; PDT must match.
        playing.sequence = 1_020;
        let origin = playing.program_start(parse).unwrap();
        let seek_date = parse(playing.segments[5].program_date_time.as_deref().unwrap());
        let selected = window(50, 30);
        let start = selected.program_start(parse).unwrap();
        assert!(start > seek_date);

        // Uncovered dates must not replace the requested range with stale or newer media.
        let mut uncovered = selected.clone();
        let end = start + selected.duration() * 1_000.0;
        for date in [origin, start - 1.0, end, end + 7_200_000.0, f64::NAN] {
            assert_eq!(uncovered.retain_from_date(date, parse), None);
            assert_eq!(uncovered, selected);
        }
        let mut history = HlsPlaylist::reconstruct(vec![(10, window(15, 50))], 20, selected, 15)
            .unwrap();
        assert_eq!(history.retain_from_date(origin, parse), Some(origin));
        assert_eq!(history.sequence, 20);
        assert_eq!(history.segments[5].program_date_time, playing.segments[5].program_date_time);
        assert_eq!(history.duration(), playing.duration());
        assert!(!history.finalized);
        let rendered = history.render("/hls/bytes", HlsStart::Live);
        let loaded = HlsPlaylist::parse(&rendered).unwrap();
        assert_eq!(loaded.segments[5].program_date_time, playing.segments[5].program_date_time);
        assert!(!loaded.finalized);
        assert_eq!(history.merge_playlist(window(70, 20)), Some(10));
        assert_eq!(history.program_start(parse), Some(origin));
        assert_eq!(history.segments[5].program_date_time, playing.segments[5].program_date_time);

        // A publisher may date only the first segment of a window.
        for segment in history.segments.iter_mut().skip(1) { segment.program_date_time = None; }
        assert_eq!(history.retain_from_date(origin + 6_500.0, parse), Some(origin + 6_000.0));
        assert_eq!(history.sequence, 23);
        assert!(history.segments[0].program_date_time.is_none());
    }

    #[test]
    fn tail_recovery_applies_authenticated_distance_on_the_media_clock() {
        assert_eq!(hls_retreat_position(2.5, 1.0), Some(1.5));
        assert_eq!(hls_retreat_position(76.5, 1.0), Some(75.5));
        assert_eq!(hls_retreat_position(0.5, 1.0), None);
        for distance in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
            assert_eq!(hls_retreat_position(76.5, distance), None);
        }
    }

    #[test]
    fn a_short_dated_live_runway_remains_playable_at_a_start_gap_or_end() {
        let mut head = dated_window(0, 1, None);
        let plan = head.startup_plan(HlsStart::Live).unwrap();
        assert_eq!((plan.play_position, plan.runway_end), (0.0, 0.5));
        head.finalized = true;
        let rendered = head.render("/hls/bytes", HlsStart::Live);
        assert!(HlsPlaylist::parse(&rendered).unwrap().finalized);
        assert!(std::str::from_utf8(&rendered).unwrap().ends_with("#EXT-X-ENDLIST\n"));

        let mut head = dated_window(50, 4, None);
        head.segments[2].gap = true;
        let plan = head.startup_plan(HlsStart::Live).unwrap();
        assert_eq!((plan.play_position, plan.runway_end), (1.5, 2.0));
    }

    #[test]
    fn dated_sliding_windows_keep_seams_after_the_tag_leaves_the_window() {
        let mut active = dated_window(10, 10, Some(15));
        let next = dated_window(16, 10, None);
        assert!(active.joins(&next));
        assert_eq!(active.merge_playlist(next), Some(6));
        assert_eq!(active.segments.last().unwrap().discontinuity_sequence, 1);
        let mut incompatible = dated_window(20, 10, Some(22));
        assert!(!active.joins(&incompatible));
        assert!(active.merge_playlist(incompatible.clone()).is_none());
        incompatible.segments[2].discontinuity_sequence = 0;
        assert!(active.merge_playlist(incompatible).is_none());
    }

    #[test]
    fn dated_tail_can_include_a_seam_before_the_viewer_joined() {
        let mut active = dated_window(16, 10, None);
        let update = dated_window(10, 20, Some(15));
        let tail = update.render("/hls/bytes", HlsStart::Live);
        assert_eq!(active.merge_tail(&tail), Some(4));
        assert_eq!(active.sequence, 16);
        assert!(active.segments.iter().all(|segment| segment.discontinuity_sequence == 0));

        let update = dated_window(25, 10, Some(30));
        assert_eq!(active.merge_tail(&update.render("/hls/bytes", HlsStart::Live)), Some(5));
        assert_eq!(active.segments.last().unwrap().discontinuity_sequence, 1);
    }

    #[test]
    fn tail_growth_rejects_discontinuity_overflow_without_changing_the_playlist() {
        let mut active = dated_window(16, 10, None);
        active.discontinuity_sequence = u64::MAX;
        for segment in &mut active.segments {
            segment.discontinuity_sequence = u64::MAX;
        }
        let before = active.clone();
        let update = dated_window(25, 10, Some(30));
        assert!(active.merge_tail(&update.render("/hls/bytes", HlsStart::Live)).is_none());
        assert_eq!(active, before);
    }

    #[test]
    fn explicit_dated_discontinuity_counters_cannot_be_reinterpreted() {
        let mut active = dated_window(10, 10, Some(15));
        let next = dated_window(16, 10, None);
        let text = String::from_utf8(next.render("/hls/bytes", HlsStart::Live))
            .unwrap()
            .replace("#EXTM3U", "#EXTM3U\n#EXT-X-DISCONTINUITY-SEQUENCE:0");
        assert!(
            active
                .merge_playlist(HlsPlaylist::parse(text.as_bytes()).unwrap())
                .is_none()
        );
    }

    #[test]
    fn completed_recording_does_not_rebase_the_joined_live_window() {
        let mut active = dated_window(16, 10, None);
        let mut recording = dated_window(0, 30, Some(15));
        recording.finalized = true;
        recording.retain_from(active.sequence).unwrap();
        assert_eq!(active.merge_playlist(recording), Some(4));
        assert_eq!(active.sequence, 16);
        assert_eq!(active.duration(), 7.0);
        assert!(active.finalized);
    }
}

mod hls_confirmation {
    use super::ActiveProbe;
    use std::{cell::{Cell, RefCell}, future::Future, task::{Context, Poll, Waker}};

    use crate::{feed::FeedProbe, stream_hls::confirm_hls_feed_head};
    use futures::future::{pending, poll_fn};

    fn settle<T>(lookup: impl Future<Output = T>) -> Poll<T> {
        let mut lookup = Box::pin(lookup);
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..100 {
            if let Poll::Ready(result) = lookup.as_mut().poll(&mut context) {
                return Poll::Ready(result);
            }
        }
        Poll::Pending
    }

    #[test]
    fn advancing_heads_release_obsolete_listeners_and_bound_concurrent_guards() {
        for (head, slow_lower) in [(105, Some(101)), (145, None)] {
            let requested = RefCell::new(Vec::new());
            let active = Cell::new(0);
            let maximum = Cell::new(0);
            let result = settle(confirm_hls_feed_head(100, 100, 20, |index| {
                requested.borrow_mut().push(index);
                active.set(active.get() + 1);
                maximum.set(maximum.get().max(active.get()));
                let active = ActiveProbe(&active);
                async move {
                    let _active = active;
                    if Some(index) == slow_lower {
                        pending().await
                    } else if index <= head {
                        FeedProbe::Found(index)
                    } else {
                        FeedProbe::Missing
                    }
                }
            }));
            assert_eq!(result, Poll::Ready(Some((head, head))));
            assert_eq!(*requested.borrow(), (101..=head + 20).collect::<Vec<_>>());
            assert!(maximum.get() > 1 && maximum.get() <= 20);
            assert_eq!(
                active.get(),
                0,
                "all finished and obsolete listeners must be released"
            );
        }
    }

    #[test]
    fn advancing_head_starts_extension_before_older_guards_finish() {
        let extension_started = Cell::new(false);
        let result = settle(confirm_hls_feed_head(100, 100, 20, |index| {
            if index == 125 {
                extension_started.set(true);
            }
            let extension_started = &extension_started;
            poll_fn(move |_| {
                if index == 120 && !extension_started.get() {
                    Poll::Pending
                } else {
                    Poll::Ready(if index == 105 {
                        FeedProbe::Found(index)
                    } else {
                        FeedProbe::Missing
                    })
                }
            })
        }));
        assert_eq!(result, Poll::Ready(Some((105, 105))));
    }

    #[test]
    fn zero_guards_return_the_existing_head_without_probes() {
        assert_eq!(settle(confirm_hls_feed_head(100, 100, 0, |_| async {
            panic!("a zero-sized guard window must not dispatch")
        })), Poll::Ready(Some((100, 100))));
    }

    #[test]
    fn every_guard_above_the_highest_positive_must_settle() {
        for withheld in 106..=125 {
            let result = settle(confirm_hls_feed_head(100, 100, 20, |index| async move {
                if index == 105 {
                    FeedProbe::Found(index)
                } else if index == withheld {
                    pending().await
                } else {
                    FeedProbe::Missing
                }
            }));
            assert_eq!(result, Poll::Pending, "unsettled guard {withheld} was skipped");
        }
    }

    #[test]
    fn a_transient_guard_cannot_prove_the_head() {
        for transient in 106..=125 {
            let result = settle(confirm_hls_feed_head(100, 100, 20, |index| async move {
                if index == 105 {
                    FeedProbe::Found(index)
                } else if index == transient {
                    FeedProbe::Transient
                } else {
                    FeedProbe::Missing
                }
            }));
            assert_eq!(result, Poll::Ready(None), "transient guard {transient} was accepted");
        }
    }

    #[test]
    fn late_lower_positives_and_transients_cannot_replace_a_newer_head() {
        let result = settle(confirm_hls_feed_head(100, 100, 20, |index| {
            let mut delayed = matches!(index, 109 | 120);
            poll_fn(move |context| {
                if std::mem::take(&mut delayed) {
                    context.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Poll::Ready(match index {
                    105 => FeedProbe::Transient,
                    109 | 110 => FeedProbe::Found(index),
                    _ => FeedProbe::Missing,
                })
            })
        }));
        assert_eq!(result, Poll::Ready(Some((110, 110))));
    }

    #[test]
    fn overflow_cannot_turn_an_incomplete_guard_window_into_a_proven_head() {
        assert_eq!(settle(confirm_hls_feed_head(u64::MAX - 19, 0, 20, |_| async {
            panic!("the overflowing guard range must not dispatch")
        })), Poll::Ready(None));
        assert_eq!(settle(confirm_hls_feed_head(u64::MAX - 20, 0, 20, |index| async move {
            if index == u64::MAX - 1 { FeedProbe::Found(1) } else { FeedProbe::Missing }
        })), Poll::Ready(None));
    }
}

mod bzz_manifest {
    #![allow(dead_code)]
    use crate::manifest;

    use manifest::{
        encode_fork, encode_fork_with_separator_path, manifest_wrapped_reference,
        ordered_indexed_forks, parse_bzz_manifest,
    };

    const VERSION_02: [u8; 31] = [
        0x02, 0x51, 0x84, 0x78, 0x9d, 0x63, 0x63, 0x57, 0x66, 0xd7, 0x8c, 0x41, 0x90, 0x01, 0x96,
        0xb5, 0x7d, 0x74, 0x00, 0x87, 0x5e, 0xbe, 0x4d, 0x9b, 0x5d, 0x1e, 0x76, 0xbd, 0x96, 0x52,
        0xa9,
    ];

    fn joined_manifest(fork: &[u8]) -> Vec<u8> {
        let mut data = vec![0; 8 + 32];
        data.extend_from_slice(&VERSION_02);
        data.push(32);
        data.extend_from_slice(&[0; 32]);

        let mut index = [0_u8; 32];
        let key = fork[2];
        index[(key / 8) as usize] |= 1 << (key % 8);
        data.extend_from_slice(&index);
        data.extend_from_slice(fork);

        let span = (data.len() - 8) as u64;
        data[..8].copy_from_slice(&span.to_le_bytes());
        data
    }

    #[test]
    fn parser_preserves_split_utf8_prefix_and_value_edge_flags() {
        let metadata = br#"{"Content-Type":"text/plain","Filename":"prefix"}"#;
        let fork = encode_fork_with_separator_path(
            &[0xc3],
            &[9; 32],
            metadata,
            true,
            b"prefix/descendant",
        )
        .unwrap();

        let parsed = parse_bzz_manifest(joined_manifest(&fork)).unwrap();
        assert_eq!(parsed.forks.len(), 1);
        assert_eq!(parsed.forks[0].prefix, [0xc3]);
        assert_eq!(parsed.forks[0].fork_type, 2 | 4 | 8 | 16);
        assert_eq!(
            parsed.forks[0].metadata.as_ref().unwrap()["Filename"],
            "prefix"
        );
        assert_eq!(manifest_wrapped_reference(parsed), None);
    }

    #[test]
    fn wrapping_manifest_may_exceed_the_first_payload_chunk() {
        let forks = (0_u8..=u8::MAX)
            .map(|key| encode_fork(&[key], &[key; 32], &[], true).unwrap())
            .collect();
        let (forks, index) = ordered_indexed_forks(forks).unwrap();
        let wrapped_reference = vec![9; 32];

        let mut data = vec![0; 8 + 32];
        data.extend_from_slice(&VERSION_02);
        data.push(32);
        data.extend_from_slice(&wrapped_reference);
        data.extend_from_slice(&index);
        for fork in forks {
            data.extend_from_slice(&fork);
        }
        let span = (data.len() - 8) as u64;
        data[..8].copy_from_slice(&span.to_le_bytes());

        assert!(span > 4096);
        let parsed = parse_bzz_manifest(data).unwrap();
        assert_eq!(manifest_wrapped_reference(parsed), Some(wrapped_reference));
    }
}

mod feed_format {
    use crate::feed;

    use feed::{
        exact_js_feed_index, sequence_feed_address, sequence_feed_id, sequence_index_bytes,
    };
    use sha3_crates_io::{Digest, Keccak256};

    fn keccak(input: &[u8]) -> [u8; 32] {
        Keccak256::digest(input).into()
    }

    fn decode_array<const N: usize>(value: &str) -> [u8; N] {
        assert_eq!(value.len(), N * 2);
        core::array::from_fn(|index| {
            u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap()
        })
    }

    fn encode_hex(value: impl AsRef<[u8]>) -> String {
        value
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn sequence_indexes_use_bees_fixed_width_big_endian_encoding() {
        for (index, expected) in [
            (0, "0000000000000000"),
            (1, "0000000000000001"),
            (255, "00000000000000ff"),
            (256, "0000000000000100"),
            (65_535, "000000000000ffff"),
            (65_536, "0000000000010000"),
            (0x0102_0304_0506_0708, "0102030405060708"),
            (u64::MAX, "ffffffffffffffff"),
        ] {
            assert_eq!(encode_hex(sequence_index_bytes(index)), expected);
        }
    }

    #[test]
    fn sequence_feed_derivation_matches_bee_golden_vectors() {
        let topic: [u8; 32] = core::array::from_fn(|index| index as u8);
        let owner = decode_array("8d3766440f0d7b949a5e32995d09619a7f86e632");

        for (index, expected_id, expected_address) in [
            (
                1,
                "d36e78737663bd495b75f9fb92c8a5a62642e1621a9b7c850eec3e8023251dbf",
                "a6488909497c3cee6bbcdf155dc788c3df5b9c6c65597574281f04650e51868a",
            ),
            (
                256,
                "13a2c9d20bfd7bf124b4867a4750ec36e1db3d7748ef10c823d13b4f36bf3114",
                "af8767fc25dd77c726a423ec8ea199522a009c39fc96b76e0088230b890ed1fc",
            ),
            (
                0x0102_0304_0506_0708,
                "9a94e5aa48e5fcb391e73f94f7230050d20920e05addb694a7ba1d016dfc700a",
                "4e24de2d98f8bc2bc326cd4dd826befb153e2dfe573df143e9c22ba8871ad4d9",
            ),
        ] {
            assert_eq!(
                encode_hex(sequence_feed_id(&topic, index, keccak)),
                expected_id
            );
            assert_eq!(
                encode_hex(sequence_feed_address(&topic, &owner, index, keccak)),
                expected_address
            );
        }
    }

    #[test]
    fn index_one_no_longer_resolves_to_the_old_little_endian_address() {
        let topic: [u8; 32] = core::array::from_fn(|index| index as u8);
        let owner = decode_array("8d3766440f0d7b949a5e32995d09619a7f86e632");
        let canonical = sequence_feed_address(&topic, &owner, 1, keccak);

        assert_ne!(
            encode_hex(canonical),
            "df3a9949aa3beed1d50a8d785647815bea0b413fe4499c752fe6c968da4e3f45"
        );
    }

    #[test]
    fn javascript_number_bridge_preserves_the_logical_index_without_byte_swapping() {
        for index in [0, 1, 7, 255, 256, 65_535, 65_536, 1_000_000, 1_u64 << 53] {
            let bridged = exact_js_feed_index(index).expect("exact index must be accepted") as u64;
            assert_eq!(bridged, index);
        }

        assert!(exact_js_feed_index((1_u64 << 53) + 1).is_none());
        assert!(exact_js_feed_index(u64::MAX).is_none());
    }

    #[test]
    fn topic_bytes_are_not_artificially_restricted_to_32_bytes() {
        let owner = [0x42; 20];
        let short_topic = [0xaa, 0xbb, 0xcc];

        let expected_id = keccak(&[short_topic.as_slice(), &[0; 8]].concat());
        assert_eq!(sequence_feed_id(&short_topic, 0, keccak), expected_id);
        assert_eq!(
            sequence_feed_address(&short_topic, &owner, 0, keccak),
            keccak(&[expected_id.as_slice(), owner.as_slice()].concat())
        );
    }

    #[test]
    fn secure_signer_rejects_noncanonical_feed_identifiers() {
        let source = include_str!("../src/secure_vault.rs");

        assert!(source.contains("soc_chunk.get(..32) != Some(expected_id.as_slice())"));
        assert!(!source.contains("feedIndexBytesHex"));
        assert!(!source.contains("feedIndexEncoding"));
    }
}

mod feed_frontier {
    use super::ActiveProbe;
    use crate::feed;

    use std::{
        cell::Cell,
        future::{Future, poll_fn},
        task::{Context, Poll, Waker},
    };

    use feed::{FEED_FRONTIER_LOOKAHEAD_LEVELS, seek_sequence_feed_frontier};
    use futures::{FutureExt, executor::block_on};

    async fn overlap_probe(index: u64, active: &Cell<usize>, maximum: &Cell<usize>) -> Option<u64> {
        active.set(active.get() + 1);
        maximum.set(maximum.get().max(active.get()));
        let _active = ActiveProbe(active);
        let mut pending = true;
        poll_fn(|context| {
            if std::mem::take(&mut pending) {
                context.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready((index <= 646).then_some(index))
            }
        })
        .await
    }

    fn frontier_probe(index: u64, head: u64, stalled: bool) -> impl Future<Output = Option<u64>> {
        poll_fn(move |_| {
            if stalled {
                Poll::Pending
            } else {
                Poll::Ready((index <= head).then_some(index))
            }
        })
    }

    fn assert_lookup_ready(
        lookup: impl Future<Output = (Option<(u64, u64)>, Option<u64>)>,
        expected_latest: u64,
    ) {
        let mut lookup = Box::pin(lookup);
        let mut context = Context::from_waker(Waker::noop());
        match lookup.as_mut().poll(&mut context) {
            Poll::Ready((latest, next)) => {
                assert_eq!(latest, Some((expected_latest, expected_latest)));
                assert_eq!(next, Some(expected_latest + 1));
            }
            Poll::Pending => panic!("irrelevant lower feed probe held up the resolved frontier"),
        }
    }

    #[test]
    fn finds_exact_sequence_frontiers_with_bees_bounded_async_policy() {
        block_on(async {
            for head in [None, Some(0), Some(1), Some(255), Some(256), Some(646)] {
                let (latest, next) = seek_sequence_feed_frontier(|index| async move {
                    head.filter(|head| index <= *head).map(|_| index)
                })
                .await
                .unwrap();

                assert_eq!(latest, head.map(|head| (head, head)));
                assert_eq!(next, head.map_or(Some(0), |head| head.checked_add(1)));
            }
        });
    }

    #[test]
    fn reliable_feed_lookup_retains_bees_anchor_first_semantics() {
        let mut lookup = Box::pin(seek_sequence_feed_frontier(|index| {
            frontier_probe(index, 646, index == 0)
        }));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(lookup.as_mut().poll(&mut context), Poll::Pending);
    }

    #[test]
    fn feed_lookup_overlaps_without_exceeding_bees_eight_listener_bound() {
        block_on(async {
            let active = Cell::new(0);
            let maximum = Cell::new(0);
            let (latest, next) = seek_sequence_feed_frontier(|index| overlap_probe(index, &active, &maximum))
                .await.unwrap();
            assert_eq!(latest.map(|(index, _)| index), Some(646));
            assert_eq!(next, Some(647));
            assert!(maximum.get() > 1, "feed probes did not overlap");
            assert!(maximum.get() <= FEED_FRONTIER_LOOKAHEAD_LEVELS);
        });
    }

    #[test]
    fn a_full_interval_does_not_wait_for_lower_probe_tails() {
        let pending_lower_indices = [1, 3, 7, 15, 31, 63, 127];
        assert_lookup_ready(
            seek_sequence_feed_frontier(|index| {
                frontier_probe(index, 255, pending_lower_indices.contains(&index))
            }).map(Result::unwrap),
            255,
        );
    }

    #[test]
    fn a_resolved_partial_interval_does_not_wait_for_lower_probe_tails() {
        assert_lookup_ready(
            seek_sequence_feed_frontier(|index| {
                frontier_probe(index, 644, matches!(index, 638 | 640))
            }).map(Result::unwrap),
            644,
        );
    }

    #[test]
    fn a_lower_transient_miss_cannot_truncate_a_proven_higher_update() {
        block_on(async {
            let missed_once = Cell::new(0usize);
            let (latest, next) = seek_sequence_feed_frontier(|index| {
                let missed_once = &missed_once;
                let mut delay_highest_once = index == 255;
                poll_fn(move |context| {
                    if std::mem::take(&mut delay_highest_once) {
                        context.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                    if index == 63 && missed_once.replace(missed_once.get() + 1) == 0 {
                        return Poll::Ready(None);
                    }
                    Poll::Ready((index <= 646).then_some(index))
                })
            })
            .await
            .unwrap();

            assert_eq!(latest.map(|(index, _)| index), Some(646));
            assert_eq!(next, Some(647));
            assert_eq!(missed_once.get(), 1);
        });
    }

    #[test]
    fn feed_lookup_distinguishes_a_missing_anchor_from_transport_failure() {
        block_on(async {
            for (first, expected) in [
                (Ok(None), Ok((None, Some(0)))),
                (Err(()), Err(())),
                (Ok(Some(9)), Ok((Some((0, 9)), None))),
            ] {
                let result = seek_sequence_feed_frontier(|index| async move {
                    if index == 0 {
                        first
                    } else {
                        Err(())
                    }
                }).await;
                assert_eq!(result, expected);
            }
        });
    }

    #[test]
    fn feed_payload_decoding_is_a_small_generic_boundary() {
        let bzz_stream = include_str!("../src/bzz_stream.rs");
        let payload = crate::source::between(
            bzz_stream,
            "pub(crate) struct FeedPayloadRoot",
            "async fn retrieve_data_head(",
        );

        crate::source::assert_contains(payload, &[
            "pub(crate) fn decode_feed_payload_root(",
            "pub(crate) async fn retrieve_feed_payload(",
            "pub(crate) async fn retrieve_feed_payload_tail(",
            "manifest_payload_size_allowed(root.span)",
            "if span > maximum_span",
            "retrieve_data_range_from_root(",
            ".min(CHUNK_SIZE as u64)",
        ]);
        assert!(!payload.contains("conservative"));
        assert!(!payload.to_ascii_lowercase().contains("hls"));
        crate::source::assert_excludes(payload, &[
            "RawFeedPayload",
            "DeferredRawFeedPayload",
            "StartupRawFeedPayload",
        ]);

        let retrieval = include_str!("../src/retrieval.rs");
        crate::source::assert_excludes(retrieval, &[
            "RetainedFeedProbePolicy",
            "retrieve_feed_update_at_index_retained_status",
            "RETRIEVE_FEED_HEDGE_ADMISSION_MS",
            "RETRIEVE_FEED_MAX_PHYSICAL_ATTEMPTS",
        ]);

        let finder = include_str!("../src/feed.rs");
        assert!(!finder.contains("seek_sequence_feed_frontier_wide_bounded"));
        assert!(!finder.contains("WIDE_FEED_FRONTIER"));
    }
}
mod manifest_format_contracts {
    use crate::manifest::*;

    #[test]
    fn groups_and_splits_paths_as_raw_utf8_bytes() {
        let first = "éclair".as_bytes();
        let second = "être".as_bytes();
        assert_eq!(common_prefix_bytes(&[first, second]), Some(vec![0xc3]));

        let path = format!("{}é", "a".repeat(29));
        let prefixes = split_prefix_bytes(path.as_bytes(), 30).unwrap();
        assert_eq!(prefixes.iter().map(Vec::len).collect::<Vec<_>>(), [30, 1]);
        assert_eq!(prefixes.concat(), path.as_bytes());
    }

    #[test]
    fn prefix_path_is_encoded_as_a_value_bearing_edge() {
        let paths = [b"foo".as_slice(), b"foobar".as_slice()];
        let prefix = common_prefix_bytes(&paths).unwrap();
        assert_eq!(prefix, b"foo");
        assert_eq!(&paths[0][prefix.len()..], b"");
        assert_eq!(&paths[1][prefix.len()..], b"bar");

        let metadata = br#"{"Content-Type":"text/plain"}"#;
        let fork = encode_fork(&prefix, &[7; 32], metadata, true).unwrap();
        assert_eq!(fork[0], 2 | 4 | 16);

        let directory_fork =
            encode_fork_with_separator_path(&prefix, &[7; 32], metadata, true, b"foo/bar").unwrap();
        assert_eq!(directory_fork[0], 2 | 4 | 8 | 16);
    }

    #[test]
    fn metadata_padding_and_limit_match_bee() {
        // Two size bytes plus 30 metadata bytes is exactly one block and must
        // not receive the extra block emitted by the old implementation.
        let metadata = vec![b'x'; 30];
        let fork = encode_fork(b"a", &[1; 32], &metadata, false).unwrap();
        assert_eq!(u16::from_be_bytes([fork[64], fork[65]]), 30);
        assert_eq!(fork.len(), 32 + 32 + 2 + 30);

        assert!(encode_fork(b"a", &[1; 32], &vec![b'x'; u16::MAX as usize], false).is_none());
    }

    fn test_fork(prefix: &[u8], marker: u8) -> Vec<u8> {
        encode_fork(prefix, &[marker; 32], &[], true).unwrap()
    }

    #[test]
    fn fork_bodies_and_index_share_bees_byte_order() {
        let (forks, index) = ordered_indexed_forks(vec![
            test_fork(b"a", 3),
            test_fork(b"/", 2),
            test_fork(b".hidden", 1),
        ])
        .unwrap();

        assert_eq!(
            forks
                .iter()
                .map(|fork| fork_prefix(fork)[0])
                .collect::<Vec<_>>(),
            [b'.', b'/', b'a']
        );
        for key in *b"./a" {
            assert_ne!(index[(key / 8) as usize] & (1 << (key % 8)), 0);
        }
    }

    #[test]
    fn colliding_or_malformed_forks_are_rejected() {
        assert!(
            ordered_indexed_forks(vec![test_fork(b"/", 1), test_fork(b"/child", 2),]).is_none()
        );
        assert!(ordered_indexed_forks(vec![vec![0, 1]]).is_none());

        let mut overlong = vec![0, (MANTARAY_PREFIX_MAX_BYTES + 1) as u8];
        overlong.resize(2 + MANTARAY_PREFIX_MAX_BYTES + 1, b'x');
        assert!(ordered_indexed_forks(vec![overlong]).is_none());
    }
}

mod manifest_resolution_contracts {
    use crate::manifest::*;

    #[test]
    fn cycles_are_path_local_but_budget_is_shared() {
        let guard = ResolutionGuard::new();
        let root = guard.descend_reference(&[1; 32]).unwrap();
        let child = root.descend_reference(&[2; 32]).unwrap();
        assert!(child.descend_reference(&[1; 32]).is_none());

        // The same child is valid on a separate branch of a DAG.
        assert!(root.descend_reference(&[2; 32]).is_some());
    }

    #[test]
    fn feed_cycles_and_depth_are_bounded() {
        let guard = ResolutionGuard::new();
        let feed = guard.descend_feed("owner", "topic").unwrap();
        assert!(feed.descend_feed("owner", "topic").is_none());

        let mut depth = ResolutionGuard::new();
        for value in 0..MAX_MANIFEST_DEPTH {
            depth = depth
                .descend_reference(&(value as u64).to_le_bytes())
                .unwrap();
        }
        assert!(depth.descend_reference(b"over-depth").is_none());
    }

    #[test]
    fn global_visit_and_target_budgets_are_hard_limits() {
        let guard = ResolutionGuard::new();
        for value in 0..MAX_MANIFEST_VISITS {
            assert!(
                guard
                    .descend_reference(&(value as u64).to_le_bytes())
                    .is_some()
            );
        }
        assert!(guard.descend_reference(b"over-budget").is_none());

        for _ in 0..MAX_MANIFEST_FORK_VISITS {
            assert!(guard.reserve_fork());
        }
        assert!(!guard.reserve_fork());

        for _ in 0..MAX_MANIFEST_TARGETS {
            assert!(guard.reserve_target());
        }
        assert!(!guard.reserve_target());
    }

    #[test]
    fn manifest_size_and_fork_index_follow_mantaray_bounds() {
        assert!(manifest_payload_size_allowed(
            MAX_MANIFEST_PAYLOAD_BYTES as u64
        ));
        assert!(!manifest_payload_size_allowed(
            MAX_MANIFEST_PAYLOAD_BYTES as u64 + 1
        ));

        let mut index = [0u8; 32];
        index[0] = 0b1000_0011;
        index[31] = 0b1000_0000;
        assert_eq!(manifest_fork_count(&index, 32), Some(4));
        assert_eq!(manifest_fork_count(&index, 0), Some(0));
        assert!(manifest_fork_count(&index[..31], 32).is_none());
    }
}
