#![allow(dead_code)]

#[path = "../src/erasure_coding.rs"]
mod erasure_coding;
mod erasure_test_support {
    use crate::erasure_coding::{
        CHUNK_WITH_SPAN_SIZE, ParityEncoder, ReedSolomonError, reconstruct_data_indices,
    };

    pub(crate) fn encode_parity(
        data_shards: &[Vec<u8>],
        parity_count: usize,
    ) -> Result<Vec<Vec<u8>>, ReedSolomonError> {
        let shard_size = data_shards.first().map(Vec::len).unwrap_or_default();
        let data = data_shards.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let encoder = ParityEncoder::new_padded(&data, parity_count, shard_size)?;
        (0..encoder.parity_count())
            .map(|index| encoder.encode_shard(index))
            .collect()
    }

    pub(crate) fn reconstruct_data(
        shards: &mut [Option<Vec<u8>>],
        data_count: usize,
    ) -> Result<(), ReedSolomonError> {
        let requested = (0..data_count)
            .filter(|&index| shards.get(index).is_some_and(Option::is_none))
            .collect::<Vec<_>>();
        reconstruct_data_indices(shards, data_count, &requested)
    }

    pub(crate) fn padded_chunk(data: &[u8]) -> Option<Vec<u8>> {
        if data.len() > CHUNK_WITH_SPAN_SIZE {
            return None;
        }
        let mut padded = vec![0; CHUNK_WITH_SPAN_SIZE];
        padded[..data.len()].copy_from_slice(data);
        Some(padded)
    }
}

mod bee_compatibility {
    #![allow(dead_code)]
    use crate::erasure_coding;

    mod tree_sim_tests {
        use super::erasure_coding::{
            self, CHUNK_SIZE, CHUNK_WITH_SPAN_SIZE, ENCRYPTED_REFERENCE_SIZE, HASH_SIZE,
            RedundancyLevel, SPAN_SIZE,
        };
        use sha3_crates_io::{Digest, Keccak256};
        use std::collections::{BTreeSet, HashMap};

        type Address = [u8; HASH_SIZE];

        #[derive(Clone, Debug)]
        struct SimNode {
            reference: Vec<u8>,
            span: u64,
        }

        #[derive(Clone, Debug)]
        struct SimStore {
            chunks: HashMap<Address, Vec<u8>>,
            next_id: u64,
        }

        impl SimStore {
            fn new() -> Self {
                Self {
                    chunks: HashMap::new(),
                    next_id: 1,
                }
            }

            fn allocate_address(&mut self, parity: bool) -> Address {
                let id = self.next_id;
                self.next_id += 1;
                let mut address = [0; HASH_SIZE];
                address[..8].copy_from_slice(&id.to_le_bytes());
                address[8] = if parity { 0xec } else { 0xda };
                for (index, byte) in address[9..].iter_mut().enumerate() {
                    *byte = (id as u8)
                        .wrapping_mul(29)
                        .wrapping_add((index as u8).wrapping_mul(47));
                }
                address
            }

            fn put_data(&mut self, canonical: Vec<u8>, encrypted: bool) -> SimNode {
                assert!((SPAN_SIZE..=CHUNK_WITH_SPAN_SIZE).contains(&canonical.len()));
                let (_, span) = erasure_coding::decode_span(&canonical[..SPAN_SIZE]).unwrap();
                let address = self.allocate_address(false);
                let mut reference = address.to_vec();
                let raw = if encrypted {
                    let key = data_key(&address);
                    reference.extend_from_slice(&key);
                    let mut padded = crate::erasure_test_support::padded_chunk(&canonical).unwrap();
                    crypt_in_place(&mut padded, &key);
                    padded
                } else {
                    canonical
                };
                assert_eq!(
                    reference.len(),
                    if encrypted {
                        ENCRYPTED_REFERENCE_SIZE
                    } else {
                        HASH_SIZE
                    }
                );
                assert!(self.chunks.insert(address, raw).is_none());
                SimNode { reference, span }
            }

            fn put_parity(&mut self, raw: Vec<u8>) -> Vec<u8> {
                assert_eq!(raw.len(), CHUNK_WITH_SPAN_SIZE);
                let address = self.allocate_address(true);
                assert!(self.chunks.insert(address, raw).is_none());
                address.to_vec()
            }

            fn get(&self, reference: &[u8]) -> Option<&Vec<u8>> {
                let address = reference_address(reference).ok()?;
                self.chunks.get(&address)
            }

            fn remove(&mut self, reference: &[u8]) -> bool {
                let Ok(address) = reference_address(reference) else {
                    return false;
                };
                self.chunks.remove(&address).is_some()
            }
        }

        #[derive(Clone, Debug)]
        struct SimTree {
            store: SimStore,
            root: SimNode,
            parent_count: usize,
            carrier_promotions: usize,
            height: usize,
            encrypted: bool,
        }

        #[derive(Debug)]
        struct DecodedNode {
            level: RedundancyLevel,
            span: u64,
            payload: Vec<u8>,
        }

        fn reference_address(reference: &[u8]) -> Result<Address, String> {
            reference
                .get(..HASH_SIZE)
                .ok_or_else(|| "reference is shorter than an address".to_string())?
                .try_into()
                .map_err(|_| "invalid address length".to_string())
        }

        fn data_key(address: &Address) -> [u8; HASH_SIZE] {
            let mut key = [0; HASH_SIZE];
            for (index, byte) in key.iter_mut().enumerate() {
                *byte = address[index]
                    .wrapping_add((index as u8).wrapping_mul(31))
                    .rotate_left((index & 7) as u32)
                    ^ 0xa5;
            }
            key
        }

        fn crypt_in_place(raw: &mut [u8], key: &[u8; HASH_SIZE]) {
            for (index, byte) in raw.iter_mut().enumerate() {
                *byte ^= key[index % key.len()]
                    .wrapping_add((index as u8).wrapping_mul(17))
                    .rotate_left((index & 7) as u32);
            }
        }

        fn deterministic_file(size: usize) -> Vec<u8> {
            (0..size)
                .map(|index| {
                    let leaf = index / CHUNK_SIZE;
                    (index as u8)
                        .wrapping_mul(37)
                        .wrapping_add((leaf as u8).wrapping_mul(101))
                        ^ ((index >> 7) as u8).rotate_left((leaf & 7) as u32)
                })
                .collect()
        }

        fn leaf_node(store: &mut SimStore, payload: &[u8], encrypted: bool) -> SimNode {
            let mut canonical = (payload.len() as u64).to_le_bytes().to_vec();
            canonical.extend_from_slice(payload);
            store.put_data(canonical, encrypted)
        }

        fn parent_node(
            store: &mut SimStore,
            children: &[SimNode],
            level: RedundancyLevel,
            encrypted: bool,
        ) -> SimNode {
            assert!(children.len() >= 2);
            let span: u64 = children.iter().map(|child| child.span).sum();
            let layout = erasure_coding::reference_layout(span, level, encrypted).unwrap();
            assert_eq!(layout.data_shards, children.len());

            let data_shards: Vec<Vec<u8>> = children
                .iter()
                .map(|child| {
                    crate::erasure_test_support::padded_chunk(store.get(&child.reference).unwrap())
                        .unwrap()
                })
                .collect();
            let parity = if layout.parity_shards == 0 {
                Vec::new()
            } else {
                crate::erasure_test_support::encode_parity(&data_shards, layout.parity_shards)
                    .unwrap()
            };
            let parity_references: Vec<Vec<u8>> = parity
                .into_iter()
                .map(|raw| store.put_parity(raw))
                .collect();

            let mut payload = Vec::new();
            for child in children {
                payload.extend_from_slice(&child.reference);
            }
            for reference in &parity_references {
                payload.extend_from_slice(reference);
            }
            assert_eq!(
                payload.len(),
                erasure_coding::encoded_reference_payload_len(span, level, encrypted).unwrap()
            );

            let (split_data, split_parity) =
                erasure_coding::split_references(&payload, span, level, encrypted).unwrap();
            assert_eq!(
                split_data,
                children
                    .iter()
                    .map(|child| child.reference.clone())
                    .collect::<Vec<_>>()
            );
            assert_eq!(split_parity, parity_references);

            let mut encoded_span = span.to_le_bytes();
            if layout.parity_shards > 0 {
                erasure_coding::encode_level(&mut encoded_span, level);
            }
            let mut canonical = encoded_span.to_vec();
            canonical.extend_from_slice(&payload);
            store.put_data(canonical, encrypted)
        }

        fn build_tree(data: &[u8], level: RedundancyLevel, encrypted: bool) -> SimTree {
            let mut store = SimStore::new();
            let mut current: Vec<SimNode> = if data.is_empty() {
                vec![leaf_node(&mut store, &[], encrypted)]
            } else {
                data.chunks(CHUNK_SIZE)
                    .map(|payload| leaf_node(&mut store, payload, encrypted))
                    .collect()
            };
            let branching = level.max_shards(encrypted);
            let mut parent_count = 0;
            let mut carrier_promotions = 0;
            let mut height = 0;

            while current.len() > 1 {
                let full_groups = current.len() / branching;
                let remainder = current.len() % branching;
                let mut next = Vec::with_capacity(full_groups + usize::from(remainder > 0));
                let mut cursor = 0;
                for _ in 0..full_groups {
                    next.push(parent_node(
                        &mut store,
                        &current[cursor..cursor + branching],
                        level,
                        encrypted,
                    ));
                    cursor += branching;
                    parent_count += 1;
                }
                match remainder {
                    0 => {}
                    1 => {
                        next.push(current[cursor].clone());
                        carrier_promotions += 1;
                    }
                    count => {
                        next.push(parent_node(
                            &mut store,
                            &current[cursor..cursor + count],
                            level,
                            encrypted,
                        ));
                        parent_count += 1;
                    }
                }
                current = next;
                height += 1;
            }

            SimTree {
                store,
                root: current.pop().unwrap(),
                parent_count,
                carrier_promotions,
                height,
                encrypted,
            }
        }

        fn decode_data_node(
            reference: &[u8],
            raw: &[u8],
            encrypted: bool,
        ) -> Result<DecodedNode, String> {
            let expected_reference_size = if encrypted {
                ENCRYPTED_REFERENCE_SIZE
            } else {
                HASH_SIZE
            };
            if reference.len() != expected_reference_size {
                return Err(format!(
                    "data reference length {}, expected {expected_reference_size}",
                    reference.len()
                ));
            }
            if !(SPAN_SIZE..=CHUNK_WITH_SPAN_SIZE).contains(&raw.len()) {
                return Err(format!("invalid stored chunk length {}", raw.len()));
            }

            let canonical = if encrypted {
                if raw.len() != CHUNK_WITH_SPAN_SIZE {
                    return Err("encrypted shard is not padded to 4104 bytes".to_string());
                }
                let key: [u8; HASH_SIZE] = reference[HASH_SIZE..]
                    .try_into()
                    .map_err(|_| "invalid encryption key".to_string())?;
                let mut decrypted = raw.to_vec();
                crypt_in_place(&mut decrypted, &key);
                decrypted
            } else {
                raw.to_vec()
            };
            let (level, span) = erasure_coding::decode_span(&canonical[..SPAN_SIZE])
                .ok_or_else(|| "invalid encoded span".to_string())?;
            let payload_len = if span <= CHUNK_SIZE as u64 {
                span as usize
            } else {
                erasure_coding::encoded_reference_payload_len(span, level, encrypted)
                    .ok_or_else(|| "invalid parent reference layout".to_string())?
            };
            let end = SPAN_SIZE
                .checked_add(payload_len)
                .ok_or_else(|| "payload length overflow".to_string())?;
            let payload = canonical
                .get(SPAN_SIZE..end)
                .ok_or_else(|| "stored chunk is shorter than its logical payload".to_string())?
                .to_vec();
            Ok(DecodedNode {
                level,
                span,
                payload,
            })
        }

        fn recover_children(
            store: &SimStore,
            decoded: &DecodedNode,
            encrypted: bool,
        ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
            let (data_references, parity_references) = erasure_coding::split_references(
                &decoded.payload,
                decoded.span,
                decoded.level,
                encrypted,
            )
            .ok_or_else(|| "unable to split parent references".to_string())?;
            let data_count = data_references.len();
            let mut shards: Vec<Option<Vec<u8>>> = data_references
                .iter()
                .map(|reference| {
                    store
                        .get(reference)
                        .and_then(|raw| crate::erasure_test_support::padded_chunk(raw))
                })
                .chain(parity_references.iter().map(|reference| {
                    store
                        .get(reference)
                        .filter(|raw| raw.len() == CHUNK_WITH_SPAN_SIZE)
                        .cloned()
                }))
                .collect();

            if shards[..data_count].iter().any(Option::is_none) {
                if parity_references.is_empty() {
                    return Err("missing data shard without parity".to_string());
                }
                crate::erasure_test_support::reconstruct_data(&mut shards, data_count)
                    .map_err(|error| format!("erasure reconstruction failed: {error}"))?;
            }

            data_references
                .into_iter()
                .zip(shards.into_iter().take(data_count))
                .map(|(reference, raw)| {
                    raw.map(|raw| (reference, raw))
                        .ok_or_else(|| "data shard remains unavailable".to_string())
                })
                .collect()
        }

        fn join_decoded_range(
            store: &SimStore,
            decoded: DecodedNode,
            encrypted: bool,
            start: u64,
            end: u64,
        ) -> Result<Vec<u8>, String> {
            if start > end || end > decoded.span {
                return Err(format!(
                    "invalid range {start}..{end} for span {}",
                    decoded.span
                ));
            }
            if start == end {
                return Ok(Vec::new());
            }
            if decoded.span <= CHUNK_SIZE as u64 {
                return decoded
                    .payload
                    .get(start as usize..end as usize)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| "leaf range is outside its payload".to_string());
            }

            let children = recover_children(store, &decoded, encrypted)?;
            let mut output = Vec::with_capacity((end - start) as usize);
            let mut child_start = 0u64;
            for (reference, raw) in children {
                let child = decode_data_node(&reference, &raw, encrypted)?;
                let child_end = child_start
                    .checked_add(child.span)
                    .ok_or_else(|| "child span overflow".to_string())?;
                let overlap_start = start.max(child_start);
                let overlap_end = end.min(child_end);
                if overlap_start < overlap_end {
                    output.extend(join_decoded_range(
                        store,
                        child,
                        encrypted,
                        overlap_start - child_start,
                        overlap_end - child_start,
                    )?);
                }
                child_start = child_end;
                if child_start >= end {
                    break;
                }
            }
            if output.len() != (end - start) as usize {
                return Err(format!(
                    "joined {} bytes for requested {}",
                    output.len(),
                    end - start
                ));
            }
            Ok(output)
        }

        fn root_decoded(tree: &SimTree) -> Result<DecodedNode, String> {
            let raw = tree
                .store
                .get(&tree.root.reference)
                .ok_or_else(|| "root is unavailable".to_string())?;
            decode_data_node(&tree.root.reference, raw, tree.encrypted)
        }

        fn join_range(tree: &SimTree, start: u64, end: u64) -> Result<Vec<u8>, String> {
            join_decoded_range(&tree.store, root_decoded(tree)?, tree.encrypted, start, end)
        }

        fn join(tree: &SimTree) -> Result<Vec<u8>, String> {
            join_range(tree, 0, tree.root.span)
        }

        fn immediate_references(tree: &SimTree) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
            let decoded = root_decoded(tree).unwrap();
            assert!(decoded.span > CHUNK_SIZE as u64);
            erasure_coding::split_references(
                &decoded.payload,
                decoded.span,
                decoded.level,
                tree.encrypted,
            )
            .unwrap()
        }

        fn remove_shards(
            tree: &mut SimTree,
            data_references: &[Vec<u8>],
            parity_references: &[Vec<u8>],
            indices: impl IntoIterator<Item = usize>,
        ) {
            let data_count = data_references.len();
            for index in indices {
                let reference = if index < data_count {
                    &data_references[index]
                } else {
                    &parity_references[index - data_count]
                };
                assert!(tree.store.remove(reference));
            }
        }

        const LEVELS: &[RedundancyLevel] = &[
            RedundancyLevel::None,
            RedundancyLevel::Medium,
            RedundancyLevel::Strong,
            RedundancyLevel::Insane,
            RedundancyLevel::Paranoid,
        ];

        fn fixed_hex<const N: usize>(value: &str) -> [u8; N] {
            assert_eq!(value.len(), N * 2);
            let mut output = [0; N];
            for (index, byte) in output.iter_mut().enumerate() {
                let start = index * 2;
                *byte = u8::from_str_radix(&value[start..start + 2], 16).unwrap();
            }
            output
        }

        #[test]
        fn bee_replica_mining_fixture_keeps_a_partial_paranoid_plan() {
            let root = fixed_hex::<HASH_SIZE>(
                "000000000000000000000000000000000000000000000000b44e000000000000",
            );
            let owner = fixed_hex::<20>("dc5b20847f43d67928f49cd4f85d696b5a7617b5");
            let replicas = erasure_coding::replicas(&root, RedundancyLevel::Paranoid, |id| {
                let mut hasher = Keccak256::new();
                hasher.update(id);
                hasher.update(owner);
                hasher.finalize().into()
            })
            .unwrap();
            assert_eq!(replicas.len(), 15);
            assert_eq!(
                replicas
                    .iter()
                    .map(|replica| replica.id[0])
                    .collect::<Vec<_>>(),
                [0, 1, 2, 4, 5, 8, 22, 23, 3, 7, 18, 26, 40, 42, 78]
            );
        }

        #[test]
        fn in_memory_bee_tree_round_trips_all_size_and_carrier_boundaries() {
            for level in LEVELS.iter().copied() {
                for encrypted in [false, true] {
                    let k = level.max_shards(encrypted);
                    let k_bytes = k * CHUNK_SIZE;
                    let sizes = BTreeSet::from([
                        0,
                        1,
                        CHUNK_SIZE - 1,
                        CHUNK_SIZE,
                        CHUNK_SIZE + 1,
                        k_bytes - 1,
                        k_bytes,
                        k_bytes + 1,
                    ]);

                    for size in sizes {
                        let expected = deterministic_file(size);
                        let tree = build_tree(&expected, level, encrypted);
                        assert_eq!(
                            tree.store.chunks.len() as u64,
                            erasure_coding::upload_tree_chunk_count(size as u64, level, encrypted)
                                .unwrap()
                        );
                        assert_eq!(
                            tree.root.reference.len(),
                            if encrypted {
                                ENCRYPTED_REFERENCE_SIZE
                            } else {
                                HASH_SIZE
                            },
                            "root reference: level={level:?}, encrypted={encrypted}, size={size}"
                        );
                        assert_eq!(tree.root.span, size as u64);
                        assert_eq!(
                            join(&tree).unwrap(),
                            expected,
                            "whole join: level={level:?}, encrypted={encrypted}, size={size}"
                        );
                        assert_eq!(join_range(&tree, size as u64, size as u64).unwrap(), []);

                        if size > CHUNK_SIZE {
                            let start = CHUNK_SIZE - 7;
                            let end = (CHUNK_SIZE + 11).min(size);
                            assert_eq!(
                                join_range(&tree, start as u64, end as u64).unwrap(),
                                expected[start..end],
                                "cross-leaf range: level={level:?}, encrypted={encrypted}, size={size}"
                            );
                            let root = root_decoded(&tree).unwrap();
                            assert_eq!(
                                root.level, level,
                                "root marker: level={level:?}, encrypted={encrypted}, size={size}"
                            );
                        } else {
                            assert_eq!(tree.parent_count, 0);
                            assert_eq!(tree.height, 0);
                        }

                        if size == k_bytes - 1 || size == k_bytes {
                            assert_eq!(tree.height, 1);
                            assert_eq!(tree.parent_count, 1);
                            assert_eq!(tree.carrier_promotions, 0);
                        }
                        if size == k_bytes + 1 {
                            assert!(tree.height >= 2);
                            assert!(tree.parent_count >= 2);
                            assert!(tree.carrier_promotions >= 1);
                            let start = k_bytes - 9;
                            assert_eq!(
                                join_range(&tree, start as u64, size as u64).unwrap(),
                                expected[start..],
                                "carrier range: level={level:?}, encrypted={encrypted}"
                            );
                        }
                    }
                }
            }
        }

        #[test]
        fn pure_tree_plan_enforces_bees_seven_wrap_limit() {
            for level in LEVELS.iter().copied() {
                for encrypted in [false, true] {
                    let k = level.max_shards(encrypted) as u64;
                    let max_leaves = k.pow(erasure_coding::BEE_MAX_UPLOAD_TREE_LEVELS as u32);
                    let max_size = max_leaves.checked_mul(CHUNK_SIZE as u64).unwrap();
                    assert!(
                        erasure_coding::upload_tree_chunk_count(max_size, level, encrypted)
                            .is_some()
                    );
                    assert!(
                        erasure_coding::upload_tree_chunk_count(max_size + 1, level, encrypted)
                            .is_none(),
                        "level={level:?}, encrypted={encrypted} accepted an eighth wrap"
                    );
                }
            }
        }

        #[test]
        fn in_memory_bee_tree_recovers_missing_full_and_carrier_shards() {
            for level in LEVELS.iter().copied() {
                for encrypted in [false, true] {
                    let k = level.max_shards(encrypted);
                    let size = k * CHUNK_SIZE;
                    let expected = deterministic_file(size);
                    let original = build_tree(&expected, level, encrypted);
                    let (data_references, parity_references) = immediate_references(&original);
                    assert_eq!(data_references.len(), k);
                    let p = level.parities(k, encrypted);
                    assert_eq!(parity_references.len(), p);

                    if p == 0 {
                        let mut missing = original.clone();
                        remove_shards(&mut missing, &data_references, &parity_references, [0]);
                        assert!(join(&missing).is_err());
                        continue;
                    }

                    let total = k + p;
                    let erased: Vec<usize> = (0..p).map(|index| index * 73 % total).collect();
                    assert_eq!(erased.iter().copied().collect::<BTreeSet<_>>().len(), p);
                    assert!(erased.contains(&0), "the recovery case must lose data");
                    let mut recovered = original.clone();
                    remove_shards(&mut recovered, &data_references, &parity_references, erased);
                    assert_eq!(
                        join(&recovered).unwrap(),
                        expected,
                        "maximum full-group recovery: level={level:?}, encrypted={encrypted}"
                    );
                    let range_start = CHUNK_SIZE - 13;
                    let range_end = CHUNK_SIZE + 17;
                    assert_eq!(
                        join_range(&recovered, range_start as u64, range_end as u64).unwrap(),
                        expected[range_start..range_end],
                        "recovered cross-leaf range: level={level:?}, encrypted={encrypted}"
                    );

                    let over_limit: Vec<usize> = (0..=p).map(|index| index * 73 % total).collect();
                    assert_eq!(
                        over_limit.iter().copied().collect::<BTreeSet<_>>().len(),
                        p + 1
                    );
                    let mut unrecoverable = original.clone();
                    remove_shards(
                        &mut unrecoverable,
                        &data_references,
                        &parity_references,
                        over_limit,
                    );
                    assert!(join(&unrecoverable).is_err());

                    let carrier_size = size + 1;
                    let carrier_expected = deterministic_file(carrier_size);
                    let carrier_original = build_tree(&carrier_expected, level, encrypted);
                    assert!(carrier_original.carrier_promotions > 0);
                    assert!(carrier_original.height >= 2);
                    let (carrier_data, carrier_parity) = immediate_references(&carrier_original);
                    assert_eq!(carrier_data.len(), 2);
                    let carrier_p = level.parities(2, encrypted);
                    assert_eq!(carrier_parity.len(), carrier_p);
                    let mut carrier_recovered = carrier_original.clone();
                    let carrier_losses = std::iter::once(0).chain(
                        (0..carrier_p.saturating_sub(1)).map(|index| carrier_data.len() + index),
                    );
                    remove_shards(
                        &mut carrier_recovered,
                        &carrier_data,
                        &carrier_parity,
                        carrier_losses,
                    );
                    assert_eq!(
                        join(&carrier_recovered).unwrap(),
                        carrier_expected,
                        "carrier recovery: level={level:?}, encrypted={encrypted}"
                    );
                    let carrier_range_start = size - 7;
                    assert_eq!(
                        join_range(
                            &carrier_recovered,
                            carrier_range_start as u64,
                            carrier_size as u64,
                        )
                        .unwrap(),
                        carrier_expected[carrier_range_start..],
                        "recovered carrier range: level={level:?}, encrypted={encrypted}"
                    );
                }
            }
        }
    }
}

mod resource_codec {
    #![allow(dead_code)]
    use crate::erasure_coding::{decode_resource_bundle, encode_resource_bundle};

    fn push_field(encoded: &mut Vec<u8>, bytes: &[u8]) {
        encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        encoded.extend_from_slice(bytes);
    }

    fn encode_fixture(index: &[u8], entries: &[(&[u8], &[u8], &[u8])]) -> Vec<u8> {
        let mut encoded = Vec::new();
        push_field(&mut encoded, index);
        for (media_type, name, data) in entries {
            push_field(&mut encoded, media_type);
            push_field(&mut encoded, name);
            push_field(&mut encoded, data);
        }
        encoded
    }

    #[test]
    fn decodes_multiple_entries_and_empty_payloads() {
        let encoded = encode_fixture(
            b"reference",
            &[
                (b"text/plain", b"hello.txt", b"hello"),
                (b"application/octet-stream", b"empty.bin", b""),
            ],
        );

        let (entries, index) = decode_resource_bundle(&encoded).expect("valid bundle");
        assert_eq!(index, "reference");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            (b"hello".to_vec(), "text/plain".into(), "hello.txt".into())
        );
        assert_eq!(
            entries[1],
            (
                Vec::new(),
                "application/octet-stream".into(),
                "empty.bin".into()
            )
        );
    }

    #[test]
    fn encoder_round_trips_without_entry_temporaries() {
        let resources = vec![
            (b"payload".to_vec(), "text/plain".into(), "file.txt".into()),
            (Vec::new(), String::new(), "empty".into()),
        ];
        let encoded = encode_resource_bundle(resources.clone(), "root".into()).expect("encodes");
        let expected = encode_fixture(
            b"root",
            &[
                (b"text/plain", b"file.txt", b"payload"),
                (b"", b"empty", b""),
            ],
        );

        assert_eq!(encoded, expected);
        assert_eq!(
            decode_resource_bundle(&encoded),
            Some((resources, "root".into()))
        );
    }

    #[test]
    fn truncated_fields_never_panic_or_partially_decode() {
        let encoded = encode_fixture(b"root", &[(b"text/plain", b"file.txt", b"payload")]);
        let index_only_end = 8 + b"root".len();

        for end in 0..encoded.len() {
            let result = std::panic::catch_unwind(|| decode_resource_bundle(&encoded[..end]));
            assert!(result.is_ok(), "decoder panicked at byte boundary {end}");

            if end == index_only_end {
                assert_eq!(result.unwrap(), Some((Vec::new(), "root".into())));
            } else {
                assert!(
                    result.unwrap().is_none(),
                    "accepted incomplete field at byte boundary {end}"
                );
            }
        }
    }

    #[test]
    fn rejects_lengths_larger_than_the_remaining_input() {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode_resource_bundle(&encoded), None);

        let mut trailing = encode_fixture(b"root", &[]);
        trailing.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode_resource_bundle(&trailing), None);
    }

    #[test]
    fn preserves_empty_string_fallback_for_invalid_utf8() {
        let encoded = encode_fixture(&[0xff], &[(&[0xfe], &[0xfd], b"data")]);
        let (entries, index) = decode_resource_bundle(&encoded).expect("structurally valid bundle");

        assert_eq!(index, "");
        assert_eq!(
            entries,
            vec![(b"data".to_vec(), String::new(), String::new())]
        );
    }
}

mod upload_slicing {
    #![allow(dead_code)]
    use crate::erasure_coding::{FILE_UPLOAD_READ_WINDOW_BYTES, FileSlicePlan};

    const SWARM_PAYLOAD_BYTES: u64 = 4096;

    #[test]
    fn file_slice_plan_is_contiguous_and_bounded() {
        let sizes = [
            0,
            1,
            SWARM_PAYLOAD_BYTES - 1,
            SWARM_PAYLOAD_BYTES,
            FILE_UPLOAD_READ_WINDOW_BYTES - 1,
            FILE_UPLOAD_READ_WINDOW_BYTES,
            FILE_UPLOAD_READ_WINDOW_BYTES + 1,
            FILE_UPLOAD_READ_WINDOW_BYTES * 3 + 17,
        ];

        for size in sizes {
            let slices = FileSlicePlan::new(size).collect::<Vec<_>>();
            let mut cursor = 0;
            for &(start, end) in &slices {
                assert_eq!(start, cursor, "gap or overlap for size {size}");
                assert!(start < end);
                assert!(end - start <= FILE_UPLOAD_READ_WINDOW_BYTES);
                cursor = end;
            }
            assert_eq!(cursor, size);
            assert_eq!(slices.is_empty(), size == 0);
        }
    }

    #[test]
    fn bounded_parts_preserve_exact_swarm_leaf_boundaries() {
        let sizes = [
            0,
            1,
            SWARM_PAYLOAD_BYTES - 1,
            SWARM_PAYLOAD_BYTES,
            SWARM_PAYLOAD_BYTES + 1,
            FILE_UPLOAD_READ_WINDOW_BYTES - 1,
            FILE_UPLOAD_READ_WINDOW_BYTES,
            FILE_UPLOAD_READ_WINDOW_BYTES + 1,
            FILE_UPLOAD_READ_WINDOW_BYTES * 2 + SWARM_PAYLOAD_BYTES + 37,
        ];

        for size in sizes {
            let mut streamed = Vec::new();
            let mut pending = 0u64;
            for (start, end) in FileSlicePlan::new(size) {
                let mut remaining = end - start;
                while remaining > 0 {
                    let take = (SWARM_PAYLOAD_BYTES - pending).min(remaining);
                    pending += take;
                    remaining -= take;
                    if pending == SWARM_PAYLOAD_BYTES {
                        streamed.push(pending);
                        pending = 0;
                    }
                }
            }
            if pending > 0 || size == 0 {
                streamed.push(pending);
            }

            let full = size / SWARM_PAYLOAD_BYTES;
            let remainder = size % SWARM_PAYLOAD_BYTES;
            let mut direct = vec![SWARM_PAYLOAD_BYTES; full as usize];
            if remainder > 0 || size == 0 {
                direct.push(remainder);
            }
            assert_eq!(streamed, direct, "leaf plan changed for size {size}");
        }
    }
}
mod erasure_contracts {
    use crate::erasure_coding::*;
    use crate::erasure_test_support::{encode_parity, padded_chunk, reconstruct_data};

    #[derive(Clone, Copy)]
    struct TableCase {
        name: &'static str,
        level: RedundancyLevel,
        encrypted: bool,
        thresholds: &'static [usize],
        parities: &'static [usize],
        max_shards: usize,
        recovery_shards: usize,
    }

    const TABLE_CASES: &[TableCase] = &[
        TableCase {
            name: "medium/plain",
            level: RedundancyLevel::Medium,
            encrypted: false,
            thresholds: &[95, 69, 47, 29, 15, 6, 2, 1],
            parities: &[9, 8, 7, 6, 5, 4, 3, 2],
            max_shards: 119,
            recovery_shards: 2,
        },
        TableCase {
            name: "medium/encrypted",
            level: RedundancyLevel::Medium,
            encrypted: true,
            thresholds: &[47, 34, 23, 14, 7, 3, 1],
            parities: &[9, 8, 7, 6, 5, 4, 3],
            max_shards: 59,
            recovery_shards: 3,
        },
        TableCase {
            name: "strong/plain",
            level: RedundancyLevel::Strong,
            encrypted: false,
            thresholds: &[
                105, 96, 87, 78, 70, 62, 54, 47, 40, 33, 27, 21, 16, 11, 7, 4, 2, 1,
            ],
            parities: &[
                21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4,
            ],
            max_shards: 107,
            recovery_shards: 4,
        },
        TableCase {
            name: "strong/encrypted",
            level: RedundancyLevel::Strong,
            encrypted: true,
            thresholds: &[
                52, 48, 43, 39, 35, 31, 27, 23, 20, 16, 13, 10, 8, 5, 3, 2, 1,
            ],
            parities: &[
                21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5,
            ],
            max_shards: 53,
            recovery_shards: 3,
        },
        TableCase {
            name: "insane/plain",
            level: RedundancyLevel::Insane,
            encrypted: false,
            thresholds: &[
                93, 88, 83, 78, 74, 69, 64, 60, 55, 51, 46, 42, 38, 34, 30, 27, 23, 20, 17, 14, 11,
                9, 6, 4, 3, 2, 1,
            ],
            parities: &[
                31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11,
                10, 9, 8, 7, 6, 5,
            ],
            max_shards: 97,
            recovery_shards: 4,
        },
        TableCase {
            name: "insane/encrypted",
            level: RedundancyLevel::Insane,
            encrypted: true,
            thresholds: &[
                46, 44, 41, 39, 37, 34, 32, 30, 27, 25, 23, 21, 19, 17, 15, 13, 11, 10, 8, 7, 5, 4,
                3, 2, 1,
            ],
            parities: &[
                31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11,
                10, 9, 8, 6,
            ],
            max_shards: 48,
            recovery_shards: 3,
        },
        TableCase {
            name: "paranoid/plain",
            level: RedundancyLevel::Paranoid,
            encrypted: false,
            thresholds: &[
                37, 36, 35, 34, 33, 32, 31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17,
                16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1,
            ],
            parities: &[
                89, 87, 86, 84, 83, 81, 80, 78, 76, 75, 73, 71, 70, 68, 66, 65, 63, 61, 59, 58, 56,
                54, 52, 50, 48, 47, 45, 43, 40, 38, 36, 34, 31, 29, 26, 23, 19,
            ],
            max_shards: 39,
            recovery_shards: 8,
        },
        TableCase {
            name: "paranoid/encrypted",
            level: RedundancyLevel::Paranoid,
            encrypted: true,
            thresholds: &[
                18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1,
            ],
            parities: &[
                87, 84, 81, 78, 75, 71, 68, 65, 61, 58, 54, 50, 47, 43, 38, 34, 29, 23,
            ],
            max_shards: 20,
            recovery_shards: 8,
        },
    ];

    const ALL_LEVELS: &[RedundancyLevel] = &[
        RedundancyLevel::None,
        RedundancyLevel::Medium,
        RedundancyLevel::Strong,
        RedundancyLevel::Insane,
        RedundancyLevel::Paranoid,
    ];

    fn table_parities(case: TableCase, shards: usize) -> usize {
        case.thresholds
            .iter()
            .zip(case.parities)
            .find_map(|(&threshold, &parities)| (shards >= threshold).then_some(parities))
            .unwrap_or(0)
    }

    fn deterministic_shards(count: usize, size: usize) -> Vec<Vec<u8>> {
        (0..count)
            .map(|shard| {
                (0..size)
                    .map(|offset| {
                        let seed = shard
                            .wrapping_mul(0x9e37)
                            .wrapping_add(offset.wrapping_mul(0x79b9))
                            .wrapping_add(shard ^ offset);
                        (seed as u8).rotate_left(((shard + offset) & 7) as u32)
                    })
                    .collect()
            })
            .collect()
    }

    fn assert_recovers(
        case: TableCase,
        data: &[Vec<u8>],
        parity: &[Vec<u8>],
        erased: impl IntoIterator<Item = usize>,
    ) {
        let mut shards: Vec<Option<Vec<u8>>> =
            data.iter().chain(parity).cloned().map(Some).collect();
        for index in erased {
            shards[index] = None;
        }
        reconstruct_data(&mut shards, data.len())
            .unwrap_or_else(|error| panic!("{} reconstruction failed: {error}", case.name));
        for (index, expected) in data.iter().enumerate() {
            assert_eq!(
                shards[index].as_ref(),
                Some(expected),
                "{} data shard {index} was reconstructed incorrectly",
                case.name
            );
        }
    }

    #[test]
    fn every_bee_redundancy_table_entry_and_gap_matches() {
        for case in TABLE_CASES {
            assert_eq!(
                case.thresholds.len(),
                case.parities.len(),
                "{} malformed test contract",
                case.name
            );
            assert_eq!(
                case.level.max_shards(case.encrypted),
                case.max_shards,
                "{} maximum data-shard count",
                case.name
            );

            // Checking every input (rather than just thresholds) catches off-by-one
            // errors in each descending Bee table interval.
            for shards in 0..=256 {
                assert_eq!(
                    case.level.parities(shards, case.encrypted),
                    table_parities(*case, shards),
                    "{} parity count for {shards} data shards",
                    case.name
                );
            }
        }

        for encrypted in [false, true] {
            assert_eq!(
                RedundancyLevel::None.max_shards(encrypted),
                if encrypted { 64 } else { 128 }
            );
            for shards in 0..=256 {
                assert_eq!(RedundancyLevel::None.parities(shards, encrypted), 0);
            }
        }
    }

    #[test]
    fn every_mode_recovers_each_loss_count_through_p() {
        for case in TABLE_CASES {
            let data = deterministic_shards(case.recovery_shards, 37);
            let parity_count = case.level.parities(data.len(), case.encrypted);
            let parity = encode_parity(&data, parity_count).unwrap();

            for loss_count in 1..=parity_count {
                assert_recovers(*case, &data, &parity, 0..loss_count);
            }

            // A co-prime stride scatters the maximum allowed losses across both
            // data and parity positions instead of testing only a prefix.
            let total = data.len() + parity.len();
            let scattered = (0..parity_count).map(|index| (index * 73) % total);
            assert_recovers(*case, &data, &parity, scattered);
        }
    }

    #[test]
    fn every_full_group_recovers_the_maximum_allowed_losses() {
        for case in TABLE_CASES {
            let data = deterministic_shards(case.max_shards, 19);
            let parity_count = case.level.parities(data.len(), case.encrypted);
            let parity = encode_parity(&data, parity_count).unwrap();
            assert_eq!(
                data.len() + parity.len(),
                if case.encrypted {
                    case.max_shards + parity_count
                } else {
                    128
                },
                "{} full group size",
                case.name
            );

            assert_recovers(*case, &data, &parity, 0..parity_count);

            let total = data.len() + parity.len();
            let scattered = (0..parity_count).map(|index| (index * 73) % total);
            assert_recovers(*case, &data, &parity, scattered);
        }
    }

    #[test]
    fn reference_layout_covers_leaf_and_carrier_boundaries() {
        for level in ALL_LEVELS {
            for encrypted in [false, true] {
                let branching = level.max_shards(encrypted) as u64;
                assert!(reference_layout(0, *level, encrypted).is_none());
                assert!(reference_layout(CHUNK_SIZE as u64, *level, encrypted).is_none());

                let first = reference_layout(CHUNK_SIZE as u64 + 1, *level, encrypted).unwrap();
                assert_eq!(first.data_shards, 2);
                assert_eq!(first.child_capacity, CHUNK_SIZE as u64);
                assert_eq!(
                    first.parity_shards,
                    level.parities(2, encrypted),
                    "level {level:?}, encrypted={encrypted}"
                );

                let full_span = branching * CHUNK_SIZE as u64;
                let full = reference_layout(full_span, *level, encrypted).unwrap();
                assert_eq!(full.data_shards, branching as usize);
                assert_eq!(full.child_capacity, CHUNK_SIZE as u64);
                assert_eq!(
                    encoded_reference_payload_len(full_span, *level, encrypted).unwrap(),
                    full.data_shards
                        * if encrypted {
                            ENCRYPTED_REFERENCE_SIZE
                        } else {
                            HASH_SIZE
                        }
                        + full.parity_shards * HASH_SIZE
                );
                assert!(
                    encoded_reference_payload_len(full_span, *level, encrypted).unwrap()
                        <= CHUNK_SIZE
                );

                let carried = reference_layout(full_span + 1, *level, encrypted).unwrap();
                assert_eq!(carried.data_shards, 2);
                assert_eq!(carried.child_capacity, full_span);

                let second_full_span = branching * full_span;
                let second_full = reference_layout(second_full_span, *level, encrypted).unwrap();
                assert_eq!(second_full.data_shards, branching as usize);
                assert_eq!(second_full.child_capacity, full_span);

                let second_carried =
                    reference_layout(second_full_span + 1, *level, encrypted).unwrap();
                assert_eq!(second_carried.data_shards, 2);
                assert_eq!(second_carried.child_capacity, second_full_span);

                assert!(reference_layout(u64::MAX, *level, encrypted).is_none());
            }
        }
    }

    #[test]
    fn reference_layout_stays_bounded_at_every_bee_tree_level() {
        // Exercise the same capacity transitions at every level Bee can emit,
        // rather than only at K and K^2.  Besides catching off-by-one carrier
        // errors, this pins the mixed encrypted layout invariant: 64-byte data
        // references followed by 32-byte parity references must always fit in
        // one 4096-byte intermediate chunk.
        for level in ALL_LEVELS {
            for encrypted in [false, true] {
                let branching = level.max_shards(encrypted) as u64;
                let data_reference_size = if encrypted {
                    ENCRYPTED_REFERENCE_SIZE
                } else {
                    HASH_SIZE
                };
                let mut child_capacity = CHUNK_SIZE as u64;

                for depth in 1..=BEE_MAX_UPLOAD_TREE_LEVELS {
                    let capacity = child_capacity.checked_mul(branching).unwrap();
                    for (span, expected_data_shards) in [
                        (child_capacity + 1, 2usize),
                        (capacity - 1, branching as usize),
                        (capacity, branching as usize),
                    ] {
                        let layout = reference_layout(span, *level, encrypted).unwrap();
                        assert_eq!(
                            layout.child_capacity, child_capacity,
                            "child capacity: level={level:?}, encrypted={encrypted}, depth={depth}, span={span}"
                        );
                        assert_eq!(
                            layout.data_shards, expected_data_shards,
                            "data shards: level={level:?}, encrypted={encrypted}, depth={depth}, span={span}"
                        );
                        assert_eq!(
                            layout.parity_shards,
                            level.parities(expected_data_shards, encrypted)
                        );

                        let payload_len = encoded_reference_payload_len(span, *level, encrypted)
                            .expect("valid Bee level must have a representable payload");
                        assert_eq!(
                            payload_len,
                            expected_data_shards * data_reference_size
                                + layout.parity_shards * HASH_SIZE
                        );
                        assert!(
                            payload_len <= CHUNK_SIZE,
                            "oversized parent: level={level:?}, encrypted={encrypted}, depth={depth}, span={span}, payload={payload_len}"
                        );
                    }

                    // Crossing a full level is Bee's carrier boundary.  Check
                    // it when the next multiplication is representable; the
                    // overflow case must instead be rejected without wrapping.
                    if capacity.checked_mul(branching).is_some() {
                        let carried = reference_layout(capacity + 1, *level, encrypted).unwrap();
                        assert_eq!(carried.child_capacity, capacity);
                        assert_eq!(carried.data_shards, 2);
                    } else {
                        assert!(reference_layout(capacity + 1, *level, encrypted).is_none());
                    }

                    child_capacity = capacity;
                }
            }
        }
    }

    #[test]
    fn reference_payload_splits_data_before_plain_parity_references() {
        for level in ALL_LEVELS {
            for encrypted in [false, true] {
                let span = level.max_shards(encrypted) as u64 * CHUNK_SIZE as u64;
                let (data_count, parity_count) = reference_count(span, *level, encrypted).unwrap();
                let data_reference_size = if encrypted {
                    ENCRYPTED_REFERENCE_SIZE
                } else {
                    HASH_SIZE
                };
                let mut payload = Vec::new();
                for index in 0..data_count {
                    payload.extend(vec![(index + 1) as u8; data_reference_size]);
                }
                for index in 0..parity_count {
                    payload.extend(vec![0x80 | index as u8; HASH_SIZE]);
                }

                assert_eq!(
                    payload.len(),
                    encoded_reference_payload_len(span, *level, encrypted).unwrap()
                );
                let (data, parity) = split_references(&payload, span, *level, encrypted).unwrap();
                assert_eq!(data.len(), data_count);
                assert_eq!(parity.len(), parity_count);
                for (index, reference) in data.iter().enumerate() {
                    assert_eq!(reference, &vec![(index + 1) as u8; data_reference_size]);
                }
                for (index, reference) in parity.iter().enumerate() {
                    assert_eq!(reference, &vec![0x80 | index as u8; HASH_SIZE]);
                }

                let mut too_long = payload.clone();
                too_long.push(0);
                assert!(split_references(&too_long, span, *level, encrypted).is_none());
                assert!(
                    split_references(&payload[..payload.len() - 1], span, *level, encrypted)
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn span_marker_handles_all_levels_and_logical_size_edges() {
        assert_eq!(RedundancyLevel::DEFAULT_UPLOAD, RedundancyLevel::Medium);
        assert_eq!(RedundancyLevel::DEFAULT_DOWNLOAD, RedundancyLevel::Paranoid);
        for (value, level) in ALL_LEVELS.iter().copied().enumerate() {
            assert_eq!(RedundancyLevel::from_u8(value as u8), Some(level));
            assert_eq!(level.as_u8(), value as u8);
            assert_eq!(level.replica_count(), [0, 2, 4, 8, 16][value]);
        }
        assert_eq!(RedundancyLevel::from_u8(5), None);
        assert_eq!(RedundancyLevel::from_u8(u8::MAX), None);

        for level in ALL_LEVELS[1..].iter().copied() {
            for logical_size in [0, 1, CHUNK_SIZE as u64, u32::MAX as u64, (1u64 << 56) - 1] {
                let mut span = logical_size.to_le_bytes();
                encode_level(&mut span, level);
                assert_eq!(span[7], 0x80 | level.as_u8());
                assert_eq!(decode_span(&span), Some((level, logical_size)));
            }
        }

        assert_eq!(decode_span(&[]), None);
        assert_eq!(decode_span(&[0; SPAN_SIZE - 1]), None);
        assert_eq!(
            decode_span(&0x8000_0000_0000_0000u64.to_le_bytes()),
            Some((RedundancyLevel::None, 0x8000_0000_0000_0000))
        );
        assert_eq!(decode_span(&0x8500_0000_0000_0000u64.to_le_bytes()), None);
    }

    #[test]
    fn parity_has_a_stable_klauspost_compatible_golden_vector() {
        // For systematic Vandermonde k=2 the first two parity rows are
        // [3, 2] and [2, 3] in GF(2^8), using polynomial 0x11d.
        let data = vec![
            vec![0x00, 0x01, 0x02, 0x03, 0x10, 0x20, 0x80, 0xff],
            vec![0xff, 0x80, 0x20, 0x10, 0x03, 0x02, 0x01, 0x00],
        ];
        let parity = encode_parity(&data, 2).unwrap();
        assert_eq!(
            parity,
            vec![
                vec![0xe3, 0x1e, 0x46, 0x25, 0x36, 0x64, 0x9f, 0x1c],
                vec![0x1c, 0x9f, 0x64, 0x36, 0x25, 0x46, 0x1e, 0xe3],
            ]
        );
        assert_eq!(encode_parity(&data, 2).unwrap(), parity);
    }

    #[test]
    fn padding_and_reed_solomon_validation_cover_chunk_edges() {
        for size in [
            0,
            1,
            SPAN_SIZE,
            CHUNK_WITH_SPAN_SIZE - 1,
            CHUNK_WITH_SPAN_SIZE,
        ] {
            let data: Vec<u8> = (0..size).map(|index| index as u8).collect();
            let padded = padded_chunk(&data).unwrap();
            assert_eq!(padded.len(), CHUNK_WITH_SPAN_SIZE);
            assert_eq!(&padded[..size], data);
            assert!(padded[size..].iter().all(|byte| *byte == 0));
        }
        assert!(padded_chunk(&vec![0; CHUNK_WITH_SPAN_SIZE + 1]).is_none());

        assert_eq!(
            encode_parity(&[], 1),
            Err(ReedSolomonError::InvalidShardCount)
        );
        assert_eq!(
            encode_parity(&[vec![1]], 0),
            Err(ReedSolomonError::InvalidShardCount)
        );
        assert_eq!(
            encode_parity(&vec![vec![1]; 256], 1),
            Err(ReedSolomonError::InvalidShardCount)
        );
        assert_eq!(
            encode_parity(&[vec![1]], usize::MAX),
            Err(ReedSolomonError::InvalidShardCount)
        );
        let padded_source = [1u8];
        assert_eq!(
            ParityEncoder::new_padded(&[padded_source.as_slice()], usize::MAX, 1).err(),
            Some(ReedSolomonError::InvalidShardCount)
        );
        let maximum_total = encode_parity(&[vec![0x5a]], 255).unwrap();
        assert_eq!(maximum_total.len(), 255);
        assert!(maximum_total.iter().all(|shard| shard == &[0x5a]));
        assert_eq!(
            encode_parity(&[vec![]], 1),
            Err(ReedSolomonError::InvalidShardSize)
        );
        assert_eq!(
            encode_parity(&[vec![1], vec![1, 2]], 1),
            Err(ReedSolomonError::InvalidShardSize)
        );

        assert_eq!(
            reconstruct_data(&mut [Some(vec![1])], 1),
            Err(ReedSolomonError::InvalidShardCount)
        );
        assert_eq!(
            reconstruct_data(&mut [Some(vec![1]), None, None], 2),
            Err(ReedSolomonError::TooFewShards)
        );
        assert_eq!(
            reconstruct_data(&mut [Some(vec![1]), Some(vec![1, 2]), None], 2),
            Err(ReedSolomonError::InvalidShardSize)
        );
        assert_eq!(
            reconstruct_data_indices(&mut [Some(vec![1]), Some(vec![2])], 1, &[1]),
            Err(ReedSolomonError::InvalidShardCount)
        );
        assert_eq!(
            reconstruct_data_indices(&mut [Some(vec![1]), None, None], 2, &[1]),
            Err(ReedSolomonError::TooFewShards)
        );
        assert_eq!(
            reconstruct_data_indices(&mut [Some(vec![1]), Some(vec![2, 3]), None], 2, &[0]),
            Err(ReedSolomonError::InvalidShardSize)
        );
    }

    #[test]
    fn virtual_zero_padding_matches_explicit_bee_shards() {
        let short = vec![
            deterministic_shards(1, SPAN_SIZE)[0].clone(),
            deterministic_shards(1, 137)[0].clone(),
            deterministic_shards(1, CHUNK_WITH_SPAN_SIZE)[0].clone(),
        ];
        let explicit = short
            .iter()
            .map(|shard| padded_chunk(shard).unwrap())
            .collect::<Vec<_>>();
        let expected = encode_parity(&explicit, 3).unwrap();
        let slices = short.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let encoder = ParityEncoder::new_padded(&slices, 3, CHUNK_WITH_SPAN_SIZE).unwrap();
        let actual = (0..encoder.parity_count())
            .map(|index| encoder.encode_shard(index).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(actual, expected);
    }

    #[test]
    fn replica_scheduler_covers_every_dispersion_level() {
        let root = [0xabu8; HASH_SIZE];
        let bit_reversal_order = [
            0, 128, 64, 192, 32, 96, 160, 224, 16, 48, 80, 112, 144, 176, 208, 240,
        ];
        for level in ALL_LEVELS.iter().copied() {
            let mut calls = 0;
            let replicas = replicas(&root, level, |id| {
                calls += 1;
                *id
            })
            .unwrap();
            assert_eq!(replicas.len(), level.replica_count());
            assert_eq!(
                replicas
                    .iter()
                    .map(|replica| replica.id[0])
                    .collect::<Vec<_>>(),
                bit_reversal_order[..level.replica_count()]
            );
            assert!(replicas.iter().all(|replica| replica.id[1..] == root[1..]));
            assert!(replicas.iter().all(|replica| replica.id == replica.address));
            if level == RedundancyLevel::None {
                assert_eq!(calls, 0);
            }
        }

        assert!(replicas(&root[..HASH_SIZE - 1], RedundancyLevel::Medium, |id| *id).is_none());
        let short = replicas(&root, RedundancyLevel::Medium, |_| [0; HASH_SIZE]).unwrap();
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].id[0], 0);
    }

    #[test]
    fn target_rhs_reconstruction_exhaustively_matches_full_recovery() {
        // Exhaust every recoverable erasure pattern and every requested-data
        // subset for small matrices. This covers present targets, missing
        // targets, duplicates through the dedicated test above, data/parity
        // loss combinations, and the all-data equivalence case.
        for data_count in 1..=5 {
            for parity_count in 1..=3 {
                let data = deterministic_shards(data_count, 31);
                let parity = encode_parity(&data, parity_count).unwrap();
                let encoded = data
                    .iter()
                    .chain(&parity)
                    .cloned()
                    .map(Some)
                    .collect::<Vec<_>>();
                let total_count = encoded.len();

                for erasure_mask in 0usize..(1usize << total_count) {
                    if erasure_mask.count_ones() as usize > parity_count {
                        continue;
                    }

                    let mut unavailable = encoded.clone();
                    for (index, shard) in unavailable.iter_mut().enumerate() {
                        if erasure_mask & (1usize << index) != 0 {
                            *shard = None;
                        }
                    }

                    let mut fully_recovered = unavailable.clone();
                    reconstruct_data(&mut fully_recovered, data_count).unwrap();

                    for requested_mask in 0usize..(1usize << data_count) {
                        let requested = (0..data_count)
                            .filter(|&index| requested_mask & (1usize << index) != 0)
                            .collect::<Vec<_>>();
                        let mut targeted = unavailable.clone();
                        reconstruct_data_indices(&mut targeted, data_count, &requested).unwrap();

                        for index in 0..data_count {
                            let should_exist = unavailable[index].is_some()
                                || requested_mask & (1usize << index) != 0;
                            if should_exist {
                                assert_eq!(
                                    targeted[index], fully_recovered[index],
                                    "data={data_count} parity={parity_count} erasures={erasure_mask:#x} requested={requested_mask:#x} index={index}"
                                );
                            } else {
                                assert!(
                                    targeted[index].is_none(),
                                    "unrequested missing shard was materialized: data={data_count} parity={parity_count} erasures={erasure_mask:#x} requested={requested_mask:#x} index={index}"
                                );
                            }
                        }
                        assert_eq!(
                            &targeted[data_count..],
                            &unavailable[data_count..],
                            "parity changed: data={data_count} parity={parity_count} erasures={erasure_mask:#x} requested={requested_mask:#x}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn target_rhs_recovers_maximum_loss_bee_groups_for_every_mode() {
        for case in TABLE_CASES {
            let data = deterministic_shards(case.max_shards, 19);
            let parity_count = case.level.parities(data.len(), case.encrypted);
            let parity = encode_parity(&data, parity_count).unwrap();
            let mut unavailable = data
                .iter()
                .chain(&parity)
                .cloned()
                .map(Some)
                .collect::<Vec<_>>();

            // Erasing a full parity-count prefix exercises the maximum legal
            // loss budget. Paranoid modes lose every data shard and therefore
            // decode solely from high-index parity rows.
            for shard in &mut unavailable[..parity_count] {
                *shard = None;
            }
            let last_missing_data = parity_count.min(data.len()) - 1;
            let requested = [last_missing_data, 0];

            let mut fully_recovered = unavailable.clone();
            reconstruct_data(&mut fully_recovered, data.len())
                .unwrap_or_else(|error| panic!("{} full recovery failed: {error}", case.name));
            let mut targeted = unavailable.clone();
            reconstruct_data_indices(&mut targeted, data.len(), &requested)
                .unwrap_or_else(|error| panic!("{} targeted recovery failed: {error}", case.name));

            for &index in &requested {
                assert_eq!(
                    targeted[index], fully_recovered[index],
                    "{} requested data shard {index}",
                    case.name
                );
            }
            if let Some(unrequested) =
                (0..parity_count.min(data.len())).find(|index| !requested.contains(index))
            {
                assert!(
                    targeted[unrequested].is_none(),
                    "{} materialized unrequested data shard {unrequested}",
                    case.name
                );
            }
            assert_eq!(
                &targeted[data.len()..],
                &unavailable[data.len()..],
                "{} targeted recovery changed parity shards",
                case.name
            );
        }
    }
}
mod upload_redundancy {
    use crate::erasure_coding::RedundancyLevel;
    use crate::erasure_coding::{
        upload_redundancy_from_number, upload_redundancy_from_select, validated_upload_redundancy,
        validated_upload_redundancy_number,
    };

    const UPLOAD_INTERFACE_HTML: &str = include_str!("../static/index.html");
    const SERVICE_WORKER_JS: &str = include_str!("../static/service.js");
    const LIBRARY_RS: &str = include_str!("../src/library.rs");
    const LIB_RS: &str = include_str!("../src/lib.rs");

    #[test]
    fn strict_validation_accepts_only_bee_levels() {
        for (value, level) in [
            RedundancyLevel::None,
            RedundancyLevel::Medium,
            RedundancyLevel::Strong,
            RedundancyLevel::Insane,
            RedundancyLevel::Paranoid,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(validated_upload_redundancy(value as u8), Some(level));
        }
        assert_eq!(validated_upload_redundancy(5), None);
        assert_eq!(validated_upload_redundancy(u8::MAX), None);
    }

    #[test]
    fn select_values_fall_back_to_medium() {
        assert_eq!(
            upload_redundancy_from_select(Some("0")),
            RedundancyLevel::None
        );
        assert_eq!(
            upload_redundancy_from_select(Some("4")),
            RedundancyLevel::Paranoid
        );
        for malformed in [None, Some(""), Some("-1"), Some("5"), Some("1.0")] {
            assert_eq!(
                upload_redundancy_from_select(malformed),
                RedundancyLevel::Medium
            );
        }
    }

    #[test]
    fn javascript_numbers_must_be_finite_integral_bee_values() {
        assert_eq!(
            validated_upload_redundancy_number(2.0),
            Some(RedundancyLevel::Strong)
        );
        assert_eq!(
            upload_redundancy_from_number(Some(2.0)),
            RedundancyLevel::Strong
        );
        for malformed in [
            None,
            Some(-1.0),
            Some(-255.0),
            Some(1.5),
            Some(5.0),
            Some(257.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
        ] {
            if let Some(value) = malformed {
                assert_eq!(validated_upload_redundancy_number(value), None);
            }
            assert_eq!(
                upload_redundancy_from_number(malformed),
                RedundancyLevel::Medium
            );
        }
    }

    #[test]
    fn built_in_and_npm_rendered_interface_have_the_medium_default_dropdown() {
        let select_start = UPLOAD_INTERFACE_HTML
            .find(r#"<select id="uploadRedundancyLevel""#)
            .expect("upload redundancy selector");
        let select_end = UPLOAD_INTERFACE_HTML[select_start..]
            .find("</select>")
            .map(|offset| select_start + offset)
            .expect("upload redundancy selector end");
        let select = &UPLOAD_INTERFACE_HTML[select_start..select_end];

        let mut cursor = 0;
        for value in 0..=4 {
            let marker = format!(r#"<option value="{value}""#);
            let position = select[cursor..]
                .find(&marker)
                .map(|offset| cursor + offset)
                .unwrap_or_else(|| panic!("missing level {value} option"));
            assert!(position >= cursor, "dropdown order changed");
            cursor = position + marker.len();
        }
        assert!(select.contains(r#"<option value="1" selected>Medium — recommended</option>"#));
        assert!(UPLOAD_INTERFACE_HTML.contains("Higher levels improve loss recovery"));
    }

    #[test]
    fn explicit_wasm_upload_levels_are_validated_before_integer_coercion() {
        assert!(LIBRARY_RS.contains(
            r##"#[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: f64"##
        ));
        assert!(!LIBRARY_RS.contains(
            r##"#[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: u8"##
        ));
        assert!(
            !LIB_RS
                .contains(r##"#[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")]"##)
        );
        assert!(LIB_RS.contains("validated_upload_redundancy_number(redundancy_level)"));
        assert!(LIBRARY_RS.contains("validated_upload_redundancy_number(redundancy_level)"));
    }

    #[test]
    fn service_worker_redundancy_header_uses_strict_base_ten_parsing() {
        for marker in [
            "function parseUploadRedundancyHeader(value)",
            "value === null || value === \"\"",
            r#"/^[0-9]+$/.test(value)"#,
            "Number.isSafeInteger(level) && level <= 4",
            "parsedRedundancy === null",
        ] {
            assert!(SERVICE_WORKER_JS.contains(marker), "missing {marker}");
        }
    }
}
