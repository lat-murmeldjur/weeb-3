#![allow(dead_code)]

#[path = "../src/feed.rs"]
mod feed;
#[path = "../src/manifest.rs"]
mod manifest;

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
    use crate::feed;

    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::{
        future::{Future, poll_fn},
        pin::Pin,
        task::{Context, Poll, Wake, Waker},
    };

    use feed::{
        FEED_FRONTIER_LOOKAHEAD_LEVELS, FEED_FRONTIER_LOOKAHEAD_TIMEOUT, FeedProbe,
        WIDE_FEED_FRONTIER_COARSE_STRIDE, WIDE_FEED_FRONTIER_LOOKAHEAD,
        overscan_sequence_feed_candidate, seek_sequence_feed_frontier,
        seek_sequence_feed_frontier_bounded_observing_positive, seek_sequence_feed_frontier_from,
        seek_sequence_feed_frontier_wide_bounded,
    };
    use futures::executor::block_on;

    struct OverlapProbe {
        index: u64,
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
        pending_once: bool,
        counted_active: bool,
    }

    impl Future for OverlapProbe {
        type Output = Option<u64>;

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            if !self.pending_once {
                self.pending_once = true;
                self.counted_active = true;
                let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(now, Ordering::SeqCst);
                context.waker().wake_by_ref();
                return Poll::Pending;
            }

            self.counted_active = false;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Poll::Ready((self.index <= 646).then_some(self.index))
        }
    }

    impl Drop for OverlapProbe {
        fn drop(&mut self) {
            if self.counted_active {
                self.active.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    #[derive(Clone, Copy)]
    enum DeterministicProbeResult {
        Pending,
        Ready(Option<u64>),
    }

    struct DeterministicProbe {
        result: DeterministicProbeResult,
    }

    impl Future for DeterministicProbe {
        type Output = Option<u64>;

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            match self.result {
                DeterministicProbeResult::Pending => Poll::Pending,
                DeterministicProbeResult::Ready(result) => Poll::Ready(result),
            }
        }
    }

    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    fn assert_lookup_ready(
        lookup: impl Future<Output = (Option<(u64, u64)>, u64)>,
        expected_latest: u64,
    ) {
        let mut lookup = Box::pin(lookup);
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        match lookup.as_mut().poll(&mut context) {
            Poll::Ready((latest, next)) => {
                assert_eq!(
                    latest.map(|(index, payload)| (index, payload)),
                    Some((expected_latest, expected_latest,))
                );
                assert_eq!(next, expected_latest + 1);
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
                .await;

                assert_eq!(
                    latest.map(|(index, payload)| (index, payload)),
                    head.map(|head| (head, head))
                );
                assert_eq!(next, head.map_or(0, |head| head.saturating_add(1)));
            }
        });
    }

    #[test]
    fn wide_bounded_frontier_finds_a_long_archive_in_three_bounded_waves() {
        block_on(async {
            let probes = Arc::new(AtomicUsize::new(0));
            let (latest, next) = seek_sequence_feed_frontier_wide_bounded({
                let probes = probes.clone();
                move |index| {
                    probes.fetch_add(1, Ordering::SeqCst);
                    async move { (index <= 1_704).then_some(index) }
                }
            })
            .await;

            assert_eq!(latest, Some((1_704, 1_704)));
            assert_eq!(next, 1_705);
            let probe_count = probes.load(Ordering::SeqCst);
            assert!(probe_count <= 47, "wide lookup used {probe_count} probes");
        });
    }

    #[test]
    fn wide_bounded_frontier_covers_minute_to_twelve_hour_streams() {
        block_on(async {
            for (head, maximum_probes) in [(30, 48), (10_368, 80), (21_600, 80), (86_400, 96)] {
                let probes = Arc::new(AtomicUsize::new(0));
                let (latest, next) = seek_sequence_feed_frontier_wide_bounded({
                    let probes = probes.clone();
                    move |index| {
                        probes.fetch_add(1, Ordering::SeqCst);
                        async move { (index <= head).then_some(index) }
                    }
                })
                .await;

                assert_eq!(latest, Some((head, head)));
                assert_eq!(next, head + 1);
                assert!(probes.load(Ordering::SeqCst) <= maximum_probes);
            }
        });
    }

    #[test]
    fn fast_transients_do_not_bound_short_or_twelve_hour_frontiers() {
        block_on(async {
            for (head, transient_index, maximum_probes) in
                [(30, 7, 48), (21_600, 16_383, 80), (86_400, 65_535, 96)]
            {
                let probes = Arc::new(AtomicUsize::new(0));
                let attempts = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
                let (latest, next) = seek_sequence_feed_frontier_wide_bounded({
                    let probes = probes.clone();
                    let attempts = attempts.clone();
                    move |index| {
                        probes.fetch_add(1, Ordering::SeqCst);
                        let attempts = attempts.clone();
                        async move {
                            let attempt = {
                                let mut attempts = attempts.lock().unwrap();
                                let attempt = attempts.entry(index).or_insert(0);
                                let current = *attempt;
                                *attempt += 1;
                                current
                            };
                            if index == transient_index && attempt == 0 {
                                FeedProbe::Transient
                            } else if index <= head {
                                FeedProbe::Found(index)
                            } else {
                                FeedProbe::Missing
                            }
                        }
                    }
                })
                .await;

                assert_eq!(latest, Some((head, head)));
                assert_eq!(next, head + 1);
                assert_eq!(attempts.lock().unwrap().get(&transient_index), Some(&1));
                assert!(probes.load(Ordering::SeqCst) <= maximum_probes);
            }
        });
    }

    #[test]
    fn wide_bounded_frontier_crosses_sparse_upload_holes() {
        block_on(async {
            for (head, hole) in [(10_368, 8_191), (10_368, 10_119), (21_600, 16_383)] {
                let (latest, next) = seek_sequence_feed_frontier_wide_bounded(|index| async move {
                    (index <= head && index != hole).then_some(index)
                })
                .await;

                assert_eq!(latest, Some((head, head)));
                assert_eq!(next, head + 1);
            }
        });
    }

    #[test]
    fn a_sparse_boundary_confirmation_timeout_remains_unknown() {
        block_on(async {
            let confirmation_seen = Arc::new(AtomicUsize::new(0));
            let (latest, next) = seek_sequence_feed_frontier_wide_bounded({
                let confirmation_seen = confirmation_seen.clone();
                move |index| {
                    if index == 10_135 {
                        confirmation_seen.fetch_add(1, Ordering::SeqCst);
                    }
                    DeterministicProbe {
                        result: if index == 10_135 {
                            DeterministicProbeResult::Pending
                        } else {
                            DeterministicProbeResult::Ready(
                                (index <= 10_368 && index != 10_119).then_some(index),
                            )
                        },
                    }
                }
            })
            .await;

            assert_eq!(latest, Some((10_368, 10_368)));
            assert_eq!(next, 10_369);
            assert!(confirmation_seen.load(Ordering::SeqCst) > 0);
        });
    }

    #[test]
    fn deterministic_overscan_finds_exact_heads_at_coarse_block_boundaries() {
        block_on(async {
            for head in [30, 10_368, 28_799, 28_800, 28_801] {
                let (latest, verified) = overscan_sequence_feed_candidate(
                    (0, 0),
                    true,
                    |index| async move {
                        if index <= head {
                            FeedProbe::Found(index)
                        } else {
                            FeedProbe::Missing
                        }
                    },
                    |_| async { true },
                    |_, _| {},
                )
                .await;

                assert_eq!(latest, (head, head));
                assert!(verified, "head {head} was not verified");
            }
        });
    }

    #[test]
    fn deterministic_overscan_extends_across_multiple_coarse_blocks() {
        block_on(async {
            let admitted = Arc::new(std::sync::Mutex::new(Vec::new()));
            let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (latest, verified) = overscan_sequence_feed_candidate(
                (0, 0),
                true,
                |index| async move {
                    if index <= 57_737 {
                        FeedProbe::Found(index)
                    } else {
                        FeedProbe::Missing
                    }
                },
                {
                    let admitted = admitted.clone();
                    move |count| {
                        admitted.lock().unwrap().push(count);
                        async { true }
                    }
                },
                {
                    let observed = observed.clone();
                    move |index, _| observed.lock().unwrap().push(index)
                },
            )
            .await;

            assert_eq!(latest, (57_737, 57_737));
            assert!(verified);
            assert_eq!(*admitted.lock().unwrap(), vec![240, 240, 240, 180, 1]);
            assert_eq!(
                *observed.lock().unwrap(),
                vec![28_800, 57_600, 57_720, 57_737]
            );
        });
    }

    #[test]
    fn deterministic_near_head_waves_cross_sparse_holes() {
        block_on(async {
            let admitted = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (latest, verified) = overscan_sequence_feed_candidate(
                (5_759, 5_759),
                false,
                |index| async move {
                    let hole = (5_901..=5_903).contains(&index) || index == 6_079;
                    if index <= 6_100 && !hole {
                        FeedProbe::Found(index)
                    } else {
                        FeedProbe::Missing
                    }
                },
                {
                    let admitted = admitted.clone();
                    move |count| {
                        admitted.lock().unwrap().push(count);
                        async { true }
                    }
                },
                |_, _| {},
            )
            .await;

            assert_eq!(latest, (6_100, 6_100));
            assert!(verified);
            let admitted = admitted.lock().unwrap();
            assert_eq!(admitted.len(), 24);
            assert!(admitted[..23].iter().all(|count| *count == 16));
            assert_eq!(admitted[23], 1);
        });
    }

    #[test]
    fn deterministic_overscan_leaves_a_transient_trailing_edge_unverified() {
        block_on(async {
            let admitted = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (latest, verified) = overscan_sequence_feed_candidate(
                (29, 29),
                false,
                |index| async move {
                    if index == 30 {
                        FeedProbe::Transient
                    } else {
                        FeedProbe::Missing
                    }
                },
                {
                    let admitted = admitted.clone();
                    move |count| {
                        admitted.lock().unwrap().push(count);
                        async { true }
                    }
                },
                |_, _| {},
            )
            .await;

            assert_eq!(latest, (29, 29));
            assert!(!verified);
            assert_eq!(*admitted.lock().unwrap(), vec![16]);
        });
    }

    #[test]
    fn deterministic_overscan_resolves_a_transient_on_the_overlapping_wave() {
        block_on(async {
            let index_eleven_probes = Arc::new(AtomicUsize::new(0));
            let (latest, verified) = overscan_sequence_feed_candidate(
                (0, 0),
                false,
                {
                    let index_eleven_probes = index_eleven_probes.clone();
                    move |index| {
                        let transient =
                            index == 11 && index_eleven_probes.fetch_add(1, Ordering::SeqCst) == 0;
                        async move {
                            if index <= 10 {
                                FeedProbe::Found(index)
                            } else if transient {
                                FeedProbe::Transient
                            } else {
                                FeedProbe::Missing
                            }
                        }
                    }
                },
                |_| async { true },
                |_, _| {},
            )
            .await;

            assert_eq!(latest, (10, 10));
            assert!(verified);
            assert_eq!(index_eleven_probes.load(Ordering::SeqCst), 3);
        });
    }

    #[test]
    fn deterministic_overscan_verifies_the_terminal_index_domain() {
        block_on(async {
            let (latest, verified) = overscan_sequence_feed_candidate(
                (u64::MAX - 5, u64::MAX - 5),
                false,
                |_| async { FeedProbe::<u64>::Missing },
                |_| async { true },
                |_, _| {},
            )
            .await;
            assert_eq!(latest.0, u64::MAX - 5);
            assert!(verified);

            let (latest, verified) = overscan_sequence_feed_candidate(
                (u64::MAX - 1, u64::MAX - 1),
                false,
                |index| async move { FeedProbe::Found(index) },
                |_| async { true },
                |_, _| {},
            )
            .await;
            assert_eq!(latest.0, u64::MAX);
            assert!(verified);
        });
    }

    #[test]
    fn deterministic_overscan_rechecks_the_decisive_missing_boundary() {
        block_on(async {
            let next_probes = Arc::new(AtomicUsize::new(0));
            let (latest, verified) = overscan_sequence_feed_candidate(
                (0, 0),
                false,
                {
                    let next_probes = next_probes.clone();
                    move |index| {
                        let found = index == 1 && next_probes.fetch_add(1, Ordering::SeqCst) > 0;
                        async move {
                            if found {
                                FeedProbe::Found(index)
                            } else {
                                FeedProbe::Missing
                            }
                        }
                    }
                },
                |_| async { true },
                |_, _| {},
            )
            .await;

            assert_eq!(latest, (1, 1));
            assert!(!verified);
            assert_eq!(next_probes.load(Ordering::SeqCst), 2);
        });
    }

    #[test]
    fn deterministic_overscan_preserves_uncertainty_beyond_the_dense_wave() {
        block_on(async {
            let (latest, verified) = overscan_sequence_feed_candidate(
                (0, 0),
                true,
                |index| async move {
                    if index >= 240 && index % WIDE_FEED_FRONTIER_COARSE_STRIDE == 0 {
                        FeedProbe::Transient
                    } else {
                        FeedProbe::Missing
                    }
                },
                |_| async { true },
                |_, _| {},
            )
            .await;

            assert_eq!(latest, (0, 0));
            assert!(!verified);
        });
    }

    #[test]
    fn deterministic_overscan_never_exceeds_a_coarse_wave() {
        block_on(async {
            let active = Arc::new(AtomicUsize::new(0));
            let maximum = Arc::new(AtomicUsize::new(0));
            let admitted = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (latest, verified) = overscan_sequence_feed_candidate(
                (0, 0),
                true,
                {
                    let active = active.clone();
                    let maximum = maximum.clone();
                    move |index| {
                        let lookup = OverlapProbe {
                            index,
                            active: active.clone(),
                            maximum: maximum.clone(),
                            pending_once: false,
                            counted_active: false,
                        };
                        async move {
                            match lookup.await {
                                Some(payload) => FeedProbe::Found(payload),
                                None => FeedProbe::Missing,
                            }
                        }
                    }
                },
                {
                    let admitted = admitted.clone();
                    move |count| {
                        admitted.lock().unwrap().push(count);
                        async { true }
                    }
                },
                |_, _| {},
            )
            .await;

            assert_eq!(latest, (646, 646));
            assert!(verified);
            assert_eq!(*admitted.lock().unwrap(), vec![240, 180, 1]);
            assert_eq!(maximum.load(Ordering::SeqCst), 240);
        });
    }

    #[test]
    fn deterministic_overscan_does_not_dispatch_a_rejected_wave() {
        block_on(async {
            let probes = Arc::new(AtomicUsize::new(0));
            let admitted = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (latest, verified) = overscan_sequence_feed_candidate(
                (0, 0),
                true,
                {
                    let probes = probes.clone();
                    move |index| {
                        probes.fetch_add(1, Ordering::SeqCst);
                        async move { FeedProbe::Found(index) }
                    }
                },
                {
                    let admitted = admitted.clone();
                    move |count| {
                        admitted.lock().unwrap().push(count);
                        async { false }
                    }
                },
                |_, _| {},
            )
            .await;

            assert_eq!(latest, (0, 0));
            assert!(!verified);
            assert_eq!(*admitted.lock().unwrap(), vec![240, 180]);
            assert_eq!(probes.load(Ordering::SeqCst), 0);
        });
    }

    #[test]
    fn wide_bounded_frontier_handles_boundaries() {
        block_on(async {
            for head in [
                None,
                Some(0),
                Some(1),
                Some(255),
                Some(256),
                Some(646),
                Some(1_704),
                Some(u64::MAX - 1),
                Some(u64::MAX),
            ] {
                let (latest, next) = seek_sequence_feed_frontier_wide_bounded(|index| async move {
                    head.filter(|head| index <= *head).map(|_| index)
                })
                .await;
                assert_eq!(
                    latest.map(|(index, payload)| (index, payload)),
                    head.map(|head| (head, head))
                );
                assert_eq!(next, head.map_or(0, |head| head.saturating_add(1)));
            }
        });
    }

    #[test]
    fn wide_bounded_frontier_does_not_wait_for_a_slow_zero_after_a_higher_update() {
        assert_lookup_ready(
            seek_sequence_feed_frontier_wide_bounded(|index| DeterministicProbe {
                result: if index == 0 {
                    DeterministicProbeResult::Pending
                } else {
                    DeterministicProbeResult::Ready((index <= 1_704).then_some(index))
                },
            }),
            1_704,
        );
    }

    #[test]
    fn wide_bounded_frontier_refinement_drops_irrelevant_lower_probe_tails() {
        assert_lookup_ready(
            seek_sequence_feed_frontier_wide_bounded(|index| DeterministicProbe {
                result: if index == 1_551 {
                    DeterministicProbeResult::Pending
                } else {
                    DeterministicProbeResult::Ready((index <= 1_704).then_some(index))
                },
            }),
            1_704,
        );
    }

    #[test]
    fn wide_bounded_frontier_does_not_treat_timed_out_positives_as_missing() {
        block_on(async {
            for slow_positive in [1_701, 1_703] {
                let (latest, next) =
                    seek_sequence_feed_frontier_wide_bounded(|index| DeterministicProbe {
                        result: if index == slow_positive {
                            DeterministicProbeResult::Pending
                        } else {
                            DeterministicProbeResult::Ready((index <= 1_704).then_some(index))
                        },
                    })
                    .await;

                assert_eq!(latest, Some((1_704, 1_704)));
                assert_eq!(next, 1_705);
            }
        });
    }

    #[test]
    fn wide_bounded_frontier_never_exceeds_its_probe_limit() {
        block_on(async {
            let active = Arc::new(AtomicUsize::new(0));
            let maximum = Arc::new(AtomicUsize::new(0));
            let (latest, next) = seek_sequence_feed_frontier_wide_bounded({
                let active = active.clone();
                let maximum = maximum.clone();
                move |index| OverlapProbe {
                    index,
                    active: active.clone(),
                    maximum: maximum.clone(),
                    pending_once: false,
                    counted_active: false,
                }
            })
            .await;

            assert_eq!(latest, Some((646, 646)));
            assert_eq!(next, 647);
            assert!(maximum.load(Ordering::SeqCst) > 1);
            assert!(maximum.load(Ordering::SeqCst) <= WIDE_FEED_FRONTIER_LOOKAHEAD);
        });
    }

    #[test]
    fn wide_bounded_frontier_retains_the_zero_anchor_without_higher_updates() {
        block_on(async {
            let (latest, next) =
                seek_sequence_feed_frontier_wide_bounded(|index| DeterministicProbe {
                    result: DeterministicProbeResult::Ready((index == 0).then_some(index)),
                })
                .await;

            assert_eq!(latest, Some((0, 0)));
            assert_eq!(next, 1);
        });
    }

    #[test]
    fn wide_bounded_frontier_finds_a_feed_whose_zero_update_is_missing() {
        block_on(async {
            let (latest, next) = seek_sequence_feed_frontier_wide_bounded(|index| async move {
                (index > 0 && index <= 310).then_some(index)
            })
            .await;

            assert_eq!(latest, Some((310, 310)));
            assert_eq!(next, 311);
        });
    }

    #[test]
    fn exposes_authenticated_positive_payloads_before_finishing() {
        block_on(async {
            let first_observed = Arc::new(AtomicUsize::new(usize::MAX));
            let observed_count = Arc::new(AtomicUsize::new(0));
            let first_for_callback = first_observed.clone();
            let count_for_callback = observed_count.clone();
            let (latest, next) = seek_sequence_feed_frontier_bounded_observing_positive(
                |index| async move { (index <= 646).then_some(index as usize) },
                move |index, payload| {
                    assert_eq!(index as usize, *payload);
                    if count_for_callback.fetch_add(1, Ordering::SeqCst) == 0 {
                        first_for_callback.store(*payload, Ordering::SeqCst);
                    }
                },
            )
            .await;

            assert_ne!(first_observed.load(Ordering::SeqCst), usize::MAX);
            assert!(observed_count.load(Ordering::SeqCst) > 1);
            assert_eq!(latest.map(|(index, _)| index), Some(646));
            assert_eq!(next, 647);
        });
    }

    #[test]
    fn reliable_verification_resumes_after_the_authenticated_candidate() {
        block_on(async {
            let probed = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
            let (latest, next) = seek_sequence_feed_frontier_from((285, 285), {
                let probed = probed.clone();
                move |index| {
                    probed.lock().expect("probe log").push(index);
                    async move { (index <= 646).then_some(index) }
                }
            })
            .await;

            assert_eq!(latest, Some((646, 646)));
            assert_eq!(next, 647);
            let probed = probed.lock().expect("probe log");
            assert!(!probed.is_empty());
            assert!(
                probed.iter().all(|index| *index > 285),
                "reliable resumed lookup repeated an authenticated interval: {probed:?}"
            );
        });
    }

    #[test]
    fn reliable_resumed_lookup_restores_the_seed_when_no_newer_update_exists() {
        block_on(async {
            let (latest, next) =
                seek_sequence_feed_frontier_from((285, "authenticated"), |index| async move {
                    assert!(index > 285);
                    None::<&'static str>
                })
                .await;

            assert_eq!(latest, Some((285, "authenticated")));
            assert_eq!(next, 286);
        });
    }

    #[test]
    fn reliable_resumed_lookup_advances_to_the_last_sequence_index_without_wrapping() {
        block_on(async {
            let (latest, next) = seek_sequence_feed_frontier_from(
                (u64::MAX - 1, u64::MAX - 1),
                |index| async move { (index == u64::MAX).then_some(index) },
            )
            .await;

            assert_eq!(latest, Some((u64::MAX, u64::MAX)));
            assert_eq!(next, u64::MAX);
        });
    }

    #[test]
    fn bounded_frontier_does_not_wait_for_a_slow_zero_anchor() {
        assert_lookup_ready(
            seek_sequence_feed_frontier_bounded_observing_positive(
                |index| DeterministicProbe {
                    result: if index == 0 {
                        DeterministicProbeResult::Pending
                    } else {
                        DeterministicProbeResult::Ready((index <= 646).then_some(index))
                    },
                },
                |_, _| {},
            ),
            646,
        );
    }

    #[test]
    fn bounded_initial_wave_does_not_wait_for_index_three_or_zero_after_a_higher_success() {
        assert_lookup_ready(
            seek_sequence_feed_frontier_bounded_observing_positive(
                |index| DeterministicProbe {
                    result: if matches!(index, 0 | 3) {
                        DeterministicProbeResult::Pending
                    } else {
                        DeterministicProbeResult::Ready((index <= 7).then_some(index))
                    },
                },
                |_, _| {},
            ),
            7,
        );
    }

    #[test]
    fn reliable_feed_lookup_retains_bees_anchor_first_semantics() {
        let mut lookup = Box::pin(seek_sequence_feed_frontier(|index| DeterministicProbe {
            result: if index == 0 {
                DeterministicProbeResult::Pending
            } else {
                DeterministicProbeResult::Ready((index <= 646).then_some(index))
            },
        }));
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        assert_eq!(lookup.as_mut().poll(&mut context), Poll::Pending);
    }

    #[test]
    fn overlaps_probes_but_never_exceeds_bees_eight_lookup_bound() {
        block_on(async {
            let active = Arc::new(AtomicUsize::new(0));
            let maximum = Arc::new(AtomicUsize::new(0));

            let (latest, next) = seek_sequence_feed_frontier({
                let active = active.clone();
                let maximum = maximum.clone();
                move |index| {
                    let active = active.clone();
                    let maximum = maximum.clone();
                    OverlapProbe {
                        index,
                        active,
                        maximum,
                        pending_once: false,
                        counted_active: false,
                    }
                }
            })
            .await;

            assert_eq!(latest.map(|(index, _)| index), Some(646));
            assert_eq!(next, 647);
            assert!(
                maximum.load(Ordering::SeqCst) > 1,
                "feed probes did not overlap"
            );
            assert!(
                maximum.load(Ordering::SeqCst) <= FEED_FRONTIER_LOOKAHEAD_LEVELS,
                "feed lookup exceeded Bee's bounded lookahead"
            );
        });
    }

    #[test]
    fn bounded_initial_wave_also_never_exceeds_eight_listener_futures() {
        block_on(async {
            let active = Arc::new(AtomicUsize::new(0));
            let maximum = Arc::new(AtomicUsize::new(0));

            let (latest, next) = seek_sequence_feed_frontier_bounded_observing_positive(
                {
                    let active = active.clone();
                    let maximum = maximum.clone();
                    move |index| {
                        let active = active.clone();
                        let maximum = maximum.clone();
                        OverlapProbe {
                            index,
                            active,
                            maximum,
                            pending_once: false,
                            counted_active: false,
                        }
                    }
                },
                |_, _| {},
            )
            .await;

            assert_eq!(latest.map(|(index, _)| index), Some(646));
            assert_eq!(next, 647);
            assert!(maximum.load(Ordering::SeqCst) > 1);
            assert!(
                maximum.load(Ordering::SeqCst) <= FEED_FRONTIER_LOOKAHEAD_LEVELS,
                "bounded frontier exceeded Bee's eight-listener lookahead"
            );
        });
    }

    #[test]
    fn a_full_interval_does_not_wait_for_lower_probe_tails() {
        let pending_lower_indices = [1, 3, 7, 15, 31, 63, 127];
        assert_lookup_ready(
            seek_sequence_feed_frontier(|index| DeterministicProbe {
                result: if pending_lower_indices.contains(&index) {
                    DeterministicProbeResult::Pending
                } else {
                    DeterministicProbeResult::Ready((index <= 255).then_some(index))
                },
            }),
            255,
        );
    }

    #[test]
    fn a_resolved_partial_interval_does_not_wait_for_lower_probe_tails() {
        assert_lookup_ready(
            seek_sequence_feed_frontier(|index| DeterministicProbe {
                result: if matches!(index, 638 | 640) {
                    DeterministicProbeResult::Pending
                } else {
                    DeterministicProbeResult::Ready((index <= 644).then_some(index))
                },
            }),
            644,
        );
    }

    #[test]
    fn a_lower_transient_miss_cannot_truncate_a_proven_higher_update() {
        block_on(async {
            let missed_once = Arc::new(AtomicUsize::new(0));
            let (latest, next) = seek_sequence_feed_frontier({
                let missed_once = missed_once.clone();
                move |index| {
                    let missed_once = missed_once.clone();
                    let mut delay_highest_once = index == 255;
                    poll_fn(move |context| {
                        if delay_highest_once {
                            delay_highest_once = false;
                            context.waker().wake_by_ref();
                            return Poll::Pending;
                        }
                        if index == 63 && missed_once.fetch_add(1, Ordering::SeqCst) == 0 {
                            return Poll::Ready(None);
                        }
                        Poll::Ready((index <= 646).then_some(index))
                    })
                }
            })
            .await;

            assert_eq!(latest.map(|(index, _)| index), Some(646));
            assert_eq!(next, 647);
            assert_eq!(missed_once.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn lookahead_deadline_closes_only_future_admission_and_not_dispatched_accounting() {
        let retrieval = include_str!("../src/retrieval.rs");
        let finder = include_str!("../src/feed.rs");

        assert!(retrieval.contains("retrieve_feed_update_at_index_status("));
        assert!(retrieval.contains("probe_feed_update_status(&owner, &topic, index"));
        assert!(finder.contains("confirm_sequence_feed_missing("));
        assert!(retrieval.contains("seek_sequence_feed_frontier(|index|"));
        assert_eq!(
            FEED_FRONTIER_LOOKAHEAD_TIMEOUT,
            std::time::Duration::from_secs(1)
        );
        assert!(finder.contains("async_std::future::timeout("));
        assert!(!finder.contains("RetrieveCancelToken"));
        let feed_probe = retrieval
            .split("async fn get_feed_probe_chunk_with_hedge_admission(")
            .nth(1)
            .and_then(|source| source.split("fn valid_feed_update_payload").next())
            .expect("feed probe chunk wrapper should remain inspectable");
        assert!(feed_probe.contains("let _close_admission = admission.close_on_drop();"));
        assert!(feed_probe.contains("admission: Some(admission.clone()),"));
        assert!(feed_probe.contains(".is_err()"));
        assert!(feed_probe.contains("return FeedProbe::Transient;"));
        assert!(feed_probe.contains("chan_in.recv().await"));
        assert!(feed_probe.contains("retained_feed_probe_empty_is_missing("));
        assert!(feed_probe.contains("Some(_) => FeedProbe::Transient"));
        assert!(feed_probe.contains("None => FeedProbe::Missing"));
        assert!(feed_probe.contains("Err(_) => FeedProbe::Transient"));
        assert!(finder.contains(".map_or(FeedProbe::Transient, Into::into)"));
        let attempt = retrieval
            .split("async fn retrieve_attempt(")
            .nth(1)
            .and_then(|source| source.split("fn chunk_address_parts").next())
            .expect("retrieve attempt should remain inspectable");
        assert!(attempt.contains("settle_retrieve_attempt("));
        assert!(attempt.contains("spawn_local(async move"));
    }

    #[test]
    fn retained_exact_probe_caps_hedges_without_dropping_the_response_receiver() {
        let retrieval = include_str!("../src/retrieval.rs");
        let bzz_stream = include_str!("../src/bzz_stream.rs");

        assert!(retrieval.contains("const RETRIEVE_FEED_HEDGE_ADMISSION_MS: u64 = 1_950;"));
        assert!(retrieval.contains("const RETRIEVE_FEED_MAX_PHYSICAL_ATTEMPTS: usize = 2;"));
        let retained = retrieval
            .split("async fn get_feed_probe_chunk_with_hedge_admission(")
            .nth(1)
            .and_then(|source| source.split("fn valid_feed_update_payload").next())
            .expect("retained feed probe should remain inspectable");
        let receiver = retained
            .find("let response = chan_in.recv();")
            .expect("one retained response receiver");
        let race = retained[receiver..]
            .find("select(response, close_admission).await")
            .map(|offset| receiver + offset)
            .expect("hedge admission deadline race");
        let close = retained[race..]
            .find("admission.close();")
            .map(|offset| race + offset)
            .expect("hedge admission closure");
        let retained_wait = retained[close..]
            .find("response.await")
            .map(|offset| close + offset)
            .expect("same response future retained after admission closes");
        let classify = retained[retained_wait..]
            .find("match retrieve_result")
            .map(|offset| retained_wait + offset)
            .expect("late response classification");
        assert!(
            receiver < race && race < close && close < retained_wait && retained_wait < classify
        );
        assert!(retained.contains("let _close_admission = admission.close_on_drop();"));
        assert!(retained.contains("admission: Some(admission.clone()),"));
        assert!(
            retained.contains(
                "RetrieveAdmission::new_with_attempt_limit(policy.max_physical_attempts)"
            )
        );
        assert!(retained.contains("retained_feed_probe_empty_is_missing("));
        assert!(retained.contains("Some(_) => FeedProbe::Transient"));

        let attempt = retrieval
            .split("async fn retrieve_attempt(")
            .nth(1)
            .and_then(|source| source.split("fn chunk_address_parts").next())
            .expect("physical retrieve attempt");
        let confirmed_empty = attempt
            .find("matches!(retrieve_result.as_ref(), Ok(chunk) if chunk.is_empty())")
            .expect("confirmed empty Bee response guard");
        let empty_marker = attempt[confirmed_empty..]
            .find("admission.record_confirmed_empty_physical_attempt();")
            .map(|offset| confirmed_empty + offset)
            .expect("confirmed empty response marker");
        let immediate_settlement = attempt[empty_marker..]
            .find("settle_retrieve_attempt(")
            .map(|offset| empty_marker + offset)
            .expect("immediate settlement after response classification");
        let timeout_marker = attempt
            .find("admission.record_physical_attempt_timeout();")
            .expect("finite admission timeout marker");
        let logical_timeout = attempt[timeout_marker..]
            .find("failed_retrieve_attempt(&peer)")
            .map(|offset| timeout_marker + offset)
            .expect("logical timeout notification");
        assert!(confirmed_empty < empty_marker && empty_marker < immediate_settlement);
        assert!(timeout_marker < logical_timeout);

        let retrieve_chunk = retrieval
            .split("pub async fn retrieve_chunk(")
            .nth(1)
            .and_then(|source| source.split("pub async fn retrieve_check_chunk(").next())
            .expect("logical chunk retrieval should remain inspectable");
        let claim = retrieve_chunk
            .find("!admission.try_claim_physical_attempt()")
            .expect("atomic physical-attempt claim");
        let dispatch = retrieve_chunk[claim..]
            .find("wasm_bindgen_futures::spawn_local(async move")
            .map(|offset| claim + offset)
            .expect("physical retrieve dispatch");
        assert!(retrieve_chunk[..claim].contains("RetrieveAdmission::physical_attempt_available"));
        assert!(
            retrieve_chunk[claim..dispatch]
                .contains("cancel_reserve(&selected.accounting, selected.price)")
        );
        assert!(claim < dispatch);

        let public_retrieval = retrieval
            .split("pub(crate) async fn retrieve_feed_update_at_index_retained_status(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("retrieve_feed_update_at_index_bounded(")
                    .next()
            })
            .expect("retained exact feed retrieval API");
        assert!(public_retrieval.contains("probe_feed_update_status_with_hedge_admission("));
        assert!(public_retrieval.contains("RETRIEVE_FEED_HEDGE_ADMISSION_MS"));
        assert!(public_retrieval.contains("RETRIEVE_FEED_MAX_PHYSICAL_ATTEMPTS"));
        assert!(public_retrieval.contains("FeedProbe<Vec<u8>>"));

        let raw_payload = bzz_stream
            .split("pub(crate) async fn acquire_raw_feed_payload_at_index_retained_status(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("acquire_raw_feed_payload_at_index_bounded(")
                    .next()
            })
            .expect("retained raw feed payload API");
        let exact_update = raw_payload
            .find("retrieve_feed_update_at_index_retained_status(")
            .expect("retained exact update acquisition");
        let authenticated_body = raw_payload
            .find("raw_feed_payload_from_update_bounded(")
            .expect("authenticated update body decoding");
        let deferred_above_limit = raw_payload
            .find("if span > maximum_span")
            .expect("oversized retained payload classification");
        assert!(exact_update < authenticated_body);
        assert!(exact_update < deferred_above_limit);
        assert!(deferred_above_limit < authenticated_body);
        assert!(
            raw_payload[deferred_above_limit..authenticated_body]
                .contains("return RetainedRawFeedPayloadProbe::Deferred(")
        );
        assert!(raw_payload.contains("maximum_payload_bytes: usize"));
        assert!(raw_payload.contains("Some(maximum_span)"));
        assert!(raw_payload.contains("FeedProbe::Missing"));
        assert!(raw_payload.contains("FeedProbe::Transient"));
        assert!(
            raw_payload
                .contains("RetainedRawFeedPayloadProbe::Found(RawFeedPayload { index, bytes })")
        );
        assert!(raw_payload.contains("RetainedRawFeedPayloadProbe::Deferred("));
    }

    #[test]
    fn startup_resolution_reuses_the_authenticated_large_head_observation() {
        let bzz_stream = include_str!("../src/bzz_stream.rs");
        let startup = bzz_stream
            .split(
                "pub(crate) async fn acquire_latest_raw_feed_payload_startup_observing_deferred(",
            )
            .nth(1)
            .and_then(|source| {
                source
                    .split("acquire_latest_raw_feed_payload_bounded_from")
                    .next()
            })
            .expect("startup resolution with deferred observation");

        let latest = startup
            .find("seek_latest_feed_update_indexed_wide_bounded(")
            .expect("wide authenticated startup lookup");
        let predecessor = startup[latest..]
            .find("retrieve_feed_update_at_index_bounded(")
            .map(|offset| latest + offset)
            .expect("rolling predecessor lookup");
        let deferred = startup[predecessor..]
            .find("defer_large_raw_feed_update(index, update)")
            .map(|offset| predecessor + offset)
            .expect("already observed large update retained without another exact lookup");

        assert!(latest < predecessor && predecessor < deferred);
        assert!(startup.contains("playable: RawFeedPayload"));
        assert!(startup.contains("observed_deferred: None"));
        assert_eq!(
            startup
                .matches("retrieve_feed_update_at_index_bounded(")
                .count(),
            1
        );
    }

    #[test]
    fn deferred_terminal_decoder_probes_a_bounded_tail_then_admits_serial_ranges() {
        let bzz_stream = include_str!("../src/bzz_stream.rs");
        let bounds = bzz_stream
            .split("fn conservative_deferred_payload_range(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub(crate) enum RetainedRawFeedPayloadProbe")
                    .next()
            })
            .expect("conservative deferred range bounds");
        assert!(
            bounds.contains("maximum_len > crate::retrieval::CONSERVATIVE_DEFERRED_RANGE_BYTES")
        );
        assert!(bounds.contains("payload_span.checked_sub(start)?.min(maximum_len)"));
        assert!(bounds.contains("start.checked_add(len)?.checked_sub(1)?"));

        for span in [1_u64, 4_095, 4_096, 4_097, 8_191, 8_192, 65_537] {
            let mut cursor = 0_u64;
            let mut covered = 0_u64;
            while cursor < span {
                let len = (span - cursor).min(4_096);
                let end = cursor + len - 1;
                assert!((1..=4_096).contains(&len));
                covered += end - cursor + 1;
                cursor = end + 1;
            }
            assert_eq!(cursor, span);
            assert_eq!(covered, span);
        }

        let tail = bzz_stream
            .split("pub(crate) async fn probe_deferred_raw_feed_payload_tail_conservative(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub(crate) async fn acquire_deferred_raw_feed_payload_conservative")
                    .next()
            })
            .expect("conservative deferred tail decoder");
        assert!(tail.contains("deferred_raw_feed_payload_root(deferred)"));
        assert!(tail.contains("CONSERVATIVE_DEFERRED_RANGE_BYTES"));
        assert!(tail.contains("retrieve_data_range_from_root_conservative("));
        assert!(tail.contains("tail.len()).ok()? == expected_len"));

        let full = bzz_stream
            .split("pub(crate) async fn acquire_deferred_raw_feed_payload_conservative")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub(crate) async fn acquire_raw_feed_payload_at_index_bounded(")
                    .next()
            })
            .expect("conservative deferred full decoder");
        let admission = full
            .find("if !admit_range().await")
            .expect("range admission");
        let retrieval = full[admission..]
            .find("retrieve_data_range_from_root_conservative(")
            .map(|offset| admission + offset)
            .expect("serial conservative range retrieval");
        assert!(admission < retrieval);
        assert!(full.contains("CONSERVATIVE_DEFERRED_RANGE_BYTES"));
        assert!(full.contains("range.len()).ok()? != expected_len"));
        assert!(full.contains("start == span && u64::try_from(bytes.len()).ok()? == span"));

        let ordinary = bzz_stream
            .split("pub(crate) async fn acquire_deferred_raw_feed_payload(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("probe_deferred_raw_feed_payload_tail_conservative")
                    .next()
            })
            .expect("ordinary deferred startup decoder");
        assert!(ordinary.contains("raw_feed_payload_from_update_bounded("));
        assert!(!ordinary.contains("retrieve_data_range_from_root_conservative("));
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
        for key in [b'.', b'/', b'a'] {
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
        assert_eq!(manifest_fork_keys(&index, 32).unwrap(), [0, 1, 7, 255]);
        assert_eq!(manifest_fork_keys(&index, 0).unwrap(), Vec::<u8>::new());
        assert!(manifest_fork_keys(&index[..31], 32).is_none());
    }
}
