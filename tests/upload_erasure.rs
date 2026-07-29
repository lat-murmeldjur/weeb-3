#![allow(dead_code)]

#[path = "../src/erasure_coding.rs"]
mod erasure_coding;
#[path = "../src/upload_conventions.rs"]
mod upload_conventions;

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
                    let mut padded = erasure_coding::padded_chunk(&canonical).unwrap();
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

        // A deterministic reversible stand-in is sufficient here: the contract under
        // test is that RS sees stored ciphertext while tree parsing sees plaintext.
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

            // Bee encodes the stored bytes: short plaintext chunks are zero padded,
            // while encrypted chunks are already a full 4104-byte ciphertext shard.
            let data_shards: Vec<Vec<u8>> = children
                .iter()
                .map(|child| {
                    erasure_coding::padded_chunk(store.get(&child.reference).unwrap()).unwrap()
                })
                .collect();
            let parity = if layout.parity_shards == 0 {
                Vec::new()
            } else {
                erasure_coding::encode_parity(&data_shards, layout.parity_shards).unwrap()
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

            // This assertion couples layout, the 64/32-byte encrypted mixed layout,
            // and the required data-before-parity ordering.
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
                        // Bee elevates a lone carrier unchanged so it can share a
                        // parent with the preceding full group's parent.
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
                        .and_then(|raw| erasure_coding::padded_chunk(raw))
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
                erasure_coding::reconstruct_data(&mut shards, data_count)
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

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        struct SymbolicNode {
            first_leaf: u64,
            end_leaf: u64,
        }

        #[derive(Clone, Debug, Default, Eq, PartialEq)]
        struct SymbolicLevel {
            wrapped_groups: Vec<usize>,
            carriers: u64,
        }

        #[derive(Clone, Debug, Eq, PartialEq)]
        struct SymbolicShape {
            levels: Vec<SymbolicLevel>,
            root: SymbolicNode,
        }

        fn symbolic_level(levels: &mut Vec<SymbolicLevel>, level: usize) -> &mut SymbolicLevel {
            if levels.len() <= level {
                levels.resize_with(level + 1, SymbolicLevel::default);
            }
            &mut levels[level]
        }

        fn symbolic_parent(
            children: Vec<SymbolicNode>,
            level: usize,
            levels: &mut Vec<SymbolicLevel>,
        ) -> SymbolicNode {
            assert!(children.len() >= 2);
            for pair in children.windows(2) {
                assert_eq!(pair[0].end_leaf, pair[1].first_leaf);
            }
            symbolic_level(levels, level)
                .wrapped_groups
                .push(children.len());
            SymbolicNode {
                first_leaf: children.first().unwrap().first_leaf,
                end_leaf: children.last().unwrap().end_leaf,
            }
        }

        // This is a literal symbolic model of upload.rs's insert_tree_chunk plus
        // its final lowest-nonempty-buffer flush. It deliberately differs in shape
        // from the closed-form planner so their agreement detects carry bugs.
        fn production_streaming_shape(leaf_count: u64, branching: usize) -> SymbolicShape {
            assert!(leaf_count > 0);
            let mut buffers: Vec<Vec<SymbolicNode>> = Vec::new();
            let mut levels = Vec::new();

            let insert = |mut node: SymbolicNode,
                          mut level: usize,
                          buffers: &mut Vec<Vec<SymbolicNode>>,
                          levels: &mut Vec<SymbolicLevel>| {
                loop {
                    if buffers.len() <= level {
                        buffers.resize_with(level + 1, Vec::new);
                    }
                    buffers[level].push(node);
                    if buffers[level].len() < branching {
                        break;
                    }
                    node = symbolic_parent(std::mem::take(&mut buffers[level]), level, levels);
                    level += 1;
                }
            };

            for leaf in 0..leaf_count {
                insert(
                    SymbolicNode {
                        first_leaf: leaf,
                        end_leaf: leaf + 1,
                    },
                    0,
                    &mut buffers,
                    &mut levels,
                );
            }

            while buffers.iter().map(Vec::len).sum::<usize>() > 1 {
                let level = buffers
                    .iter()
                    .position(|buffer| !buffer.is_empty())
                    .unwrap();
                let mut children = std::mem::take(&mut buffers[level]);
                let node = if children.len() == 1 {
                    symbolic_level(&mut levels, level).carriers += 1;
                    children.pop().unwrap()
                } else {
                    symbolic_parent(children, level, &mut levels)
                };
                insert(node, level + 1, &mut buffers, &mut levels);
            }

            let root = buffers.iter_mut().find_map(Vec::pop).unwrap();
            while levels
                .last()
                .is_some_and(|level| level.wrapped_groups.is_empty() && level.carriers == 0)
            {
                levels.pop();
            }
            SymbolicShape { levels, root }
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
                        let plan = erasure_coding::upload_tree_plan(size as u64, level, encrypted)
                            .unwrap();
                        assert_eq!(tree.parent_count as u64, plan.parent_chunks);
                        assert_eq!(tree.carrier_promotions as u64, plan.carrier_promotions);
                        assert_eq!(tree.height, plan.levels.len());
                        assert_eq!(tree.store.chunks.len() as u64, plan.total_chunks);
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
        fn pure_tree_plan_matches_production_streaming_shape_at_adversarial_boundaries() {
            for level in LEVELS.iter().copied() {
                for encrypted in [false, true] {
                    let k = level.max_shards(encrypted) as u64;
                    let k2 = k * k;
                    let leaf_counts = BTreeSet::from([
                        1,
                        2,
                        k - 1,
                        k,
                        k + 1,
                        2 * k - 1,
                        2 * k,
                        2 * k + 1,
                        k2 - 1,
                        k2,
                        k2 + 1,
                        k2 + k - 1,
                        k2 + k,
                        k2 + k + 1,
                        2 * k2 + 1,
                    ]);

                    for leaf_count in leaf_counts {
                        let data_length = leaf_count.checked_mul(CHUNK_SIZE as u64).unwrap();
                        let plan = erasure_coding::upload_tree_plan(data_length, level, encrypted)
                            .unwrap();
                        let shape = production_streaming_shape(leaf_count, k as usize);
                        assert_eq!(plan.leaf_chunks, leaf_count);
                        assert_eq!(
                            shape.root,
                            SymbolicNode {
                                first_leaf: 0,
                                end_leaf: leaf_count,
                            },
                            "root coverage: level={level:?}, encrypted={encrypted}, leaves={leaf_count}"
                        );
                        assert_eq!(
                            shape.levels.len(),
                            plan.levels.len(),
                            "height: level={level:?}, encrypted={encrypted}, leaves={leaf_count}"
                        );

                        let mut planned_parents = 0u64;
                        let mut planned_parities = 0u64;
                        let mut planned_carriers = 0u64;
                        for level_plan in &plan.levels {
                            let observed = &shape.levels[level_plan.level];
                            let observed_full = observed
                                .wrapped_groups
                                .iter()
                                .filter(|children| **children == k as usize)
                                .count() as u64;
                            let observed_partial: Vec<usize> = observed
                                .wrapped_groups
                                .iter()
                                .copied()
                                .filter(|children| *children < k as usize)
                                .collect();
                            assert_eq!(observed_full, level_plan.full_groups);
                            assert!(observed_partial.len() <= 1);
                            assert_eq!(
                                observed_partial.first().copied().unwrap_or(0),
                                level_plan.partial_group_shards
                            );
                            assert_eq!(observed.carriers, level_plan.carrier_chunks);
                            assert_eq!(
                                observed.wrapped_groups.len() as u64,
                                level_plan.parent_chunks
                            );
                            let observed_parities = observed
                                .wrapped_groups
                                .iter()
                                .map(|children| level.parities(*children, encrypted) as u64)
                                .sum::<u64>();
                            assert_eq!(observed_parities, level_plan.parity_chunks);
                            planned_parents += level_plan.parent_chunks;
                            planned_parities += level_plan.parity_chunks;
                            planned_carriers += level_plan.carrier_chunks;
                        }
                        assert_eq!(planned_parents, plan.parent_chunks);
                        assert_eq!(planned_parities, plan.parity_chunks);
                        assert_eq!(planned_carriers, plan.carrier_promotions);
                        assert_eq!(
                            plan.total_chunks,
                            leaf_count + planned_parents + planned_parities
                        );
                    }
                }
            }

            assert_eq!(
                erasure_coding::upload_tree_plan(0, RedundancyLevel::Medium, false)
                    .unwrap()
                    .leaf_chunks,
                1
            );
        }

        #[test]
        fn pure_tree_plan_matches_bee_hashtrie_carrier_fixtures() {
            // Mirrors Bee pkg/file/pipeline/hashtrie/TestRedundancy. These two
            // fixtures directly pin both plaintext and encrypted K+1 behavior.
            let fixtures = [
                (RedundancyLevel::Insane, false, 98u64, 37u64),
                (RedundancyLevel::Paranoid, true, 21u64, 116u64),
            ];
            for (level, encrypted, leaves, expected_parities) in fixtures {
                let plan =
                    erasure_coding::upload_tree_plan(leaves * CHUNK_SIZE as u64, level, encrypted)
                        .unwrap();
                assert_eq!(plan.parity_chunks, expected_parities);
                assert_eq!(plan.parent_chunks, 2);
                assert_eq!(plan.carrier_promotions, 1);
                assert_eq!(plan.levels.len(), 2);
                assert_eq!(plan.levels[0].full_groups, 1);
                assert_eq!(plan.levels[0].carrier_chunks, 1);
                assert_eq!(plan.levels[1].partial_group_shards, 2);
            }

            // Generalize the same Bee invariant to every supported contract: K+1
            // leaves create a full K group, elevate one carrier, then wrap two.
            for level in LEVELS.iter().copied() {
                for encrypted in [false, true] {
                    let k = level.max_shards(encrypted) as u64;
                    let plan = erasure_coding::upload_tree_plan(
                        (k + 1) * CHUNK_SIZE as u64,
                        level,
                        encrypted,
                    )
                    .unwrap();
                    assert_eq!(plan.parent_chunks, 2);
                    assert_eq!(plan.carrier_promotions, 1);
                    assert_eq!(
                        plan.parity_chunks,
                        level.parities(k as usize, encrypted) as u64
                            + level.parities(2, encrypted) as u64
                    );
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
                    let maximum =
                        erasure_coding::upload_tree_plan(max_size, level, encrypted).unwrap();
                    assert_eq!(
                        maximum.levels.len(),
                        erasure_coding::BEE_MAX_UPLOAD_TREE_LEVELS
                    );
                    assert_eq!(maximum.leaf_chunks, max_leaves);
                    assert!(
                        erasure_coding::upload_tree_plan(max_size + 1, level, encrypted).is_none(),
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

                    // K+1 leaves force a full lower group plus a carrier promoted into
                    // a two-child root. Recovering the lower parent exercises RS before
                    // recursively descending through the multilevel carrier layout.
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
    use crate::upload_conventions;

    use upload_conventions::{decode_resource_bundle, encode_resource_bundle};

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
    use crate::upload_conventions;

    use upload_conventions::{FILE_UPLOAD_READ_WINDOW_BYTES, FileSlicePlan};

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
