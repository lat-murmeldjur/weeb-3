use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    fmt,
    rc::Rc,
};

pub const SPAN_SIZE: usize = 8;
pub const CHUNK_SIZE: usize = 4096;
pub const CHUNK_WITH_SPAN_SIZE: usize = SPAN_SIZE + CHUNK_SIZE;
pub const HASH_SIZE: usize = 32;
pub const ENCRYPTED_REFERENCE_SIZE: usize = 64;
/// Bee's hash trie has intermediate levels 1 through 8, so at most seven
/// wrapping operations can be performed above the data chunks.
pub const BEE_MAX_UPLOAD_TREE_LEVELS: usize = 7;
// Full groups reuse one matrix heavily, while right-edge partial groups can
// otherwise accumulate hundreds of one-off shapes over a long browser session.
const CODING_MATRIX_CACHE_ENTRIES: usize = 64;

type CodingMatrix = Rc<Vec<Vec<u8>>>;
type CodingMatrixKey = (usize, usize);

#[derive(Clone)]
struct CachedCodingMatrix {
    matrix: CodingMatrix,
    generation: u64,
}

#[derive(Default)]
struct CodingMatrixCache {
    matrices: HashMap<CodingMatrixKey, CachedCodingMatrix>,
    order: VecDeque<(CodingMatrixKey, u64)>,
    generation: u64,
}

impl CodingMatrixCache {
    fn get(&mut self, key: CodingMatrixKey) -> Option<CodingMatrix> {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let entry = self.matrices.get_mut(&key)?;
        entry.generation = generation;
        let matrix = Rc::clone(&entry.matrix);
        self.finish_touch(key, generation);
        Some(matrix)
    }

    fn insert(&mut self, key: CodingMatrixKey, matrix: CodingMatrix) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.matrices
            .insert(key, CachedCodingMatrix { matrix, generation });
        self.finish_touch(key, generation);
    }

    fn finish_touch(&mut self, key: CodingMatrixKey, generation: u64) {
        self.order.push_back((key, generation));
        while self.matrices.len() > CODING_MATRIX_CACHE_ENTRIES {
            let Some((expired, expired_generation)) = self.order.pop_front() else {
                break;
            };
            if self
                .matrices
                .get(&expired)
                .is_some_and(|entry| entry.generation == expired_generation)
            {
                self.matrices.remove(&expired);
            }
        }

        if self.order.len() > CODING_MATRIX_CACHE_ENTRIES * 2 {
            let mut live = self
                .matrices
                .iter()
                .map(|(&key, entry)| (key, entry.generation))
                .collect::<Vec<_>>();
            live.sort_unstable_by_key(|(_, generation)| *generation);
            self.order = live.into();
        }
    }
}

thread_local! {
    static CODING_MATRIX_CACHE: RefCell<CodingMatrixCache> =
        RefCell::new(CodingMatrixCache::default());
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RedundancyLevel {
    None = 0,
    Medium = 1,
    Strong = 2,
    Insane = 3,
    Paranoid = 4,
}

impl RedundancyLevel {
    pub const DEFAULT_UPLOAD: Self = Self::Medium;
    pub const DEFAULT_DOWNLOAD: Self = Self::Paranoid;

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::None),
            1 => Some(Self::Medium),
            2 => Some(Self::Strong),
            3 => Some(Self::Insane),
            4 => Some(Self::Paranoid),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn parities(self, shards: usize, encrypted: bool) -> usize {
        if self == Self::None || shards == 0 {
            return 0;
        }

        let (shard_thresholds, parity_counts) = self.erasure_table(encrypted);
        shard_thresholds
            .iter()
            .zip(parity_counts)
            .find_map(|(&threshold, &parities)| (shards >= threshold).then_some(parities))
            .unwrap_or(0)
    }

    pub fn max_shards(self, encrypted: bool) -> usize {
        if encrypted {
            // Bee has 64 encrypted-reference branches in a 4096-byte chunk. Parity
            // references remain 32 bytes, hence the division by two after reserving
            // their space in the 128 BMT sections.
            (128 - self.parities(64, true)) / 2
        } else {
            128 - self.parities(128, false)
        }
    }

    pub fn replica_count(self) -> usize {
        [0, 2, 4, 8, 16][self as usize]
    }

    fn erasure_table(self, encrypted: bool) -> (&'static [usize], &'static [usize]) {
        match (self, encrypted) {
            (Self::None, _) => (&[], &[]),
            (Self::Medium, false) => (&MEDIUM_SHARDS, &MEDIUM_PARITIES),
            (Self::Medium, true) => (&ENC_MEDIUM_SHARDS, &ENC_MEDIUM_PARITIES),
            (Self::Strong, false) => (&STRONG_SHARDS, &STRONG_PARITIES),
            (Self::Strong, true) => (&ENC_STRONG_SHARDS, &ENC_STRONG_PARITIES),
            (Self::Insane, false) => (&INSANE_SHARDS, &INSANE_PARITIES),
            (Self::Insane, true) => (&ENC_INSANE_SHARDS, &ENC_INSANE_PARITIES),
            (Self::Paranoid, false) => (&PARANOID_SHARDS, &PARANOID_PARITIES),
            (Self::Paranoid, true) => (&ENC_PARANOID_SHARDS, &ENC_PARANOID_PARITIES),
        }
    }
}

const MEDIUM_SHARDS: [usize; 8] = [95, 69, 47, 29, 15, 6, 2, 1];
const MEDIUM_PARITIES: [usize; 8] = [9, 8, 7, 6, 5, 4, 3, 2];
const ENC_MEDIUM_SHARDS: [usize; 7] = [47, 34, 23, 14, 7, 3, 1];
const ENC_MEDIUM_PARITIES: [usize; 7] = [9, 8, 7, 6, 5, 4, 3];

const STRONG_SHARDS: [usize; 18] = [
    105, 96, 87, 78, 70, 62, 54, 47, 40, 33, 27, 21, 16, 11, 7, 4, 2, 1,
];
const STRONG_PARITIES: [usize; 18] = [
    21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4,
];
const ENC_STRONG_SHARDS: [usize; 17] = [
    52, 48, 43, 39, 35, 31, 27, 23, 20, 16, 13, 10, 8, 5, 3, 2, 1,
];
const ENC_STRONG_PARITIES: [usize; 17] = [
    21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5,
];

const INSANE_SHARDS: [usize; 27] = [
    93, 88, 83, 78, 74, 69, 64, 60, 55, 51, 46, 42, 38, 34, 30, 27, 23, 20, 17, 14, 11, 9, 6, 4, 3,
    2, 1,
];
const INSANE_PARITIES: [usize; 27] = [
    31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8,
    7, 6, 5,
];
const ENC_INSANE_SHARDS: [usize; 25] = [
    46, 44, 41, 39, 37, 34, 32, 30, 27, 25, 23, 21, 19, 17, 15, 13, 11, 10, 8, 7, 5, 4, 3, 2, 1,
];
const ENC_INSANE_PARITIES: [usize; 25] = [
    31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 6,
];

const PARANOID_SHARDS: [usize; 37] = [
    37, 36, 35, 34, 33, 32, 31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14,
    13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1,
];
const PARANOID_PARITIES: [usize; 37] = [
    89, 87, 86, 84, 83, 81, 80, 78, 76, 75, 73, 71, 70, 68, 66, 65, 63, 61, 59, 58, 56, 54, 52, 50,
    48, 47, 45, 43, 40, 38, 36, 34, 31, 29, 26, 23, 19,
];
const ENC_PARANOID_SHARDS: [usize; 18] = [
    18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1,
];
const ENC_PARANOID_PARITIES: [usize; 18] = [
    87, 84, 81, 78, 75, 71, 68, 65, 61, 58, 54, 50, 47, 43, 38, 34, 29, 23,
];

pub fn encode_level(span: &mut [u8; SPAN_SIZE], level: RedundancyLevel) {
    span[SPAN_SIZE - 1] = 0x80 | level.as_u8();
}

pub fn decode_span(span: &[u8]) -> Option<(RedundancyLevel, u64)> {
    let mut decoded: [u8; SPAN_SIZE] = span.get(..SPAN_SIZE)?.try_into().ok()?;
    let level = if decoded[SPAN_SIZE - 1] > 0x80 {
        let level = RedundancyLevel::from_u8(decoded[SPAN_SIZE - 1] & 0x7f)?;
        decoded[SPAN_SIZE - 1] = 0;
        level
    } else {
        RedundancyLevel::None
    };

    Some((level, u64::from_le_bytes(decoded)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReferenceLayout {
    pub data_shards: usize,
    pub parity_shards: usize,
    pub child_capacity: u64,
}

/// Pure description of one Bee hash-trie level during upload.
///
/// A remainder of one is carried unchanged to the next level. Every other
/// non-empty group is wrapped into a parent and, when enabled, gets its own
/// parity group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadTreeLevelPlan {
    pub level: usize,
    pub input_chunks: u64,
    pub full_groups: u64,
    pub partial_group_shards: usize,
    pub carrier_chunks: u64,
    pub parent_chunks: u64,
    pub parity_chunks: u64,
    pub output_chunks: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadTreePlan {
    pub leaf_chunks: u64,
    pub parent_chunks: u64,
    pub parity_chunks: u64,
    pub carrier_promotions: u64,
    /// Leaves, parents, and parity chunks. Root replicas are deliberately not
    /// included because they are scheduled only after the root is known.
    pub total_chunks: u64,
    pub levels: Vec<UploadTreeLevelPlan>,
}

/// Plan the exact grouping performed by Bee's streaming hash trie.
///
/// This is intentionally independent of addresses and transport so upload
/// progress and compatibility tests share one overflow-checked source of truth.
pub fn upload_tree_plan(
    data_length: u64,
    level: RedundancyLevel,
    encrypted: bool,
) -> Option<UploadTreePlan> {
    let chunk_size = CHUNK_SIZE as u64;
    let mut input_chunks = data_length / chunk_size;
    if data_length % chunk_size != 0 {
        input_chunks = input_chunks.checked_add(1)?;
    }
    input_chunks = input_chunks.max(1);

    let max_shards = u64::try_from(level.max_shards(encrypted)).ok()?;
    if max_shards < 2 {
        return None;
    }

    let leaf_chunks = input_chunks;
    let mut parent_chunks = 0u64;
    let mut parity_chunks = 0u64;
    let mut carrier_promotions = 0u64;
    let mut total_chunks = leaf_chunks;
    let mut levels = Vec::new();

    while input_chunks > 1 {
        if levels.len() >= BEE_MAX_UPLOAD_TREE_LEVELS {
            return None;
        }
        let full_groups = input_chunks / max_shards;
        let remainder = input_chunks % max_shards;
        let partial_group_shards = if remainder > 1 {
            usize::try_from(remainder).ok()?
        } else {
            0
        };
        let partial_groups = u64::from(partial_group_shards != 0);
        let carrier_chunks = u64::from(remainder == 1);
        let level_parents = full_groups.checked_add(partial_groups)?;
        let full_parities = full_groups.checked_mul(
            u64::try_from(level.parities(usize::try_from(max_shards).ok()?, encrypted)).ok()?,
        )?;
        let partial_parities = if partial_group_shards == 0 {
            0
        } else {
            u64::try_from(level.parities(partial_group_shards, encrypted)).ok()?
        };
        let level_parities = full_parities.checked_add(partial_parities)?;
        let output_chunks = level_parents.checked_add(carrier_chunks)?;
        if output_chunks >= input_chunks {
            return None;
        }

        levels.push(UploadTreeLevelPlan {
            level: levels.len(),
            input_chunks,
            full_groups,
            partial_group_shards,
            carrier_chunks,
            parent_chunks: level_parents,
            parity_chunks: level_parities,
            output_chunks,
        });
        parent_chunks = parent_chunks.checked_add(level_parents)?;
        parity_chunks = parity_chunks.checked_add(level_parities)?;
        carrier_promotions = carrier_promotions.checked_add(carrier_chunks)?;
        total_chunks = total_chunks
            .checked_add(level_parents)?
            .checked_add(level_parities)?;
        input_chunks = output_chunks;
    }

    Some(UploadTreePlan {
        leaf_chunks,
        parent_chunks,
        parity_chunks,
        carrier_promotions,
        total_chunks,
        levels,
    })
}

pub fn reference_layout(
    span: u64,
    level: RedundancyLevel,
    encrypted: bool,
) -> Option<ReferenceLayout> {
    if span <= CHUNK_SIZE as u64 {
        return None;
    }

    let max_shards = level.max_shards(encrypted);
    if max_shards == 0 {
        return None;
    }

    let branching = max_shards as u64;
    let mut branch_size = CHUNK_SIZE as u64;
    let mut branch_level = 1usize;
    while branch_size < span {
        branch_size = branch_size.checked_mul(branching)?;
        branch_level += 1;
    }

    let mut reference_size = CHUNK_SIZE as u64;
    for _ in 1..branch_level.saturating_sub(1) {
        reference_size = reference_size.checked_mul(branching)?;
    }

    let data_shards_u64 = span.checked_add(reference_size - 1)? / reference_size;
    let data_shards = usize::try_from(data_shards_u64).ok()?;
    let parity_shards = level.parities(data_shards, encrypted);
    Some(ReferenceLayout {
        data_shards,
        parity_shards,
        child_capacity: reference_size,
    })
}

pub fn reference_count(
    span: u64,
    level: RedundancyLevel,
    encrypted: bool,
) -> Option<(usize, usize)> {
    let layout = reference_layout(span, level, encrypted)?;
    Some((layout.data_shards, layout.parity_shards))
}

pub fn encoded_reference_payload_len(
    span: u64,
    level: RedundancyLevel,
    encrypted: bool,
) -> Option<usize> {
    let (data_shards, parity_shards) = reference_count(span, level, encrypted)?;
    let data_reference_size = if encrypted {
        ENCRYPTED_REFERENCE_SIZE
    } else {
        HASH_SIZE
    };
    data_shards
        .checked_mul(data_reference_size)?
        .checked_add(parity_shards.checked_mul(HASH_SIZE)?)
}

pub fn split_references(
    payload: &[u8],
    span: u64,
    level: RedundancyLevel,
    encrypted: bool,
) -> Option<(Vec<Vec<u8>>, Vec<Vec<u8>>)> {
    let (data_count, parity_count) = reference_count(span, level, encrypted)?;
    let data_reference_size = if encrypted {
        ENCRYPTED_REFERENCE_SIZE
    } else {
        HASH_SIZE
    };
    let data_bytes = data_count.checked_mul(data_reference_size)?;
    let parity_bytes = parity_count.checked_mul(HASH_SIZE)?;
    if payload.len() != data_bytes.checked_add(parity_bytes)? {
        return None;
    }

    let data = payload[..data_bytes]
        .chunks_exact(data_reference_size)
        .map(<[u8]>::to_vec)
        .collect();
    let parity = payload[data_bytes..]
        .chunks_exact(HASH_SIZE)
        .map(<[u8]>::to_vec)
        .collect();
    Some((data, parity))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReedSolomonError {
    InvalidShardCount,
    InvalidShardSize,
    TooFewShards,
    SingularMatrix,
}

pub struct ParityEncoder<'a> {
    data_shards: Vec<&'a [u8]>,
    matrix: Rc<Vec<Vec<u8>>>,
    shard_size: usize,
    data_count: usize,
    parity_count: usize,
}

impl<'a> ParityEncoder<'a> {
    #[cfg(test)]
    pub fn new(data_shards: &'a [Vec<u8>], parity_count: usize) -> Result<Self, ReedSolomonError> {
        let data_count = data_shards.len();
        let total_count = data_count
            .checked_add(parity_count)
            .ok_or(ReedSolomonError::InvalidShardCount)?;
        if data_count == 0 || parity_count == 0 || total_count > 256 {
            return Err(ReedSolomonError::InvalidShardCount);
        }

        let shard_size = data_shards[0].len();
        if shard_size == 0 || data_shards.iter().any(|shard| shard.len() != shard_size) {
            return Err(ReedSolomonError::InvalidShardSize);
        }

        Ok(Self {
            data_shards: data_shards.iter().map(Vec::as_slice).collect(),
            matrix: cached_coding_matrix(data_count, total_count)?,
            shard_size,
            data_count,
            parity_count,
        })
    }

    pub fn new_padded(
        data_shards: &[&'a [u8]],
        parity_count: usize,
        shard_size: usize,
    ) -> Result<Self, ReedSolomonError> {
        let data_count = data_shards.len();
        let total_count = data_count
            .checked_add(parity_count)
            .ok_or(ReedSolomonError::InvalidShardCount)?;
        if data_count == 0 || parity_count == 0 || total_count > 256 {
            return Err(ReedSolomonError::InvalidShardCount);
        }
        if shard_size == 0
            || data_shards
                .iter()
                .any(|shard| shard.is_empty() || shard.len() > shard_size)
        {
            return Err(ReedSolomonError::InvalidShardSize);
        }

        Ok(Self {
            data_shards: data_shards.to_vec(),
            matrix: cached_coding_matrix(data_count, total_count)?,
            shard_size,
            data_count,
            parity_count,
        })
    }

    pub fn parity_count(&self) -> usize {
        self.parity_count
    }

    pub fn encode_shard(&self, parity_index: usize) -> Result<Vec<u8>, ReedSolomonError> {
        if parity_index >= self.parity_count {
            return Err(ReedSolomonError::InvalidShardCount);
        }
        Ok(code_row_slices(
            &self.matrix[self.data_count + parity_index],
            &self.data_shards,
            self.shard_size,
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Replica {
    pub id: [u8; HASH_SIZE],
    pub address: [u8; HASH_SIZE],
}

pub fn replicas<F>(
    root_address: &[u8],
    level: RedundancyLevel,
    mut address_for_id: F,
) -> Option<Vec<Replica>>
where
    F: FnMut(&[u8; HASH_SIZE]) -> [u8; HASH_SIZE],
{
    let root_address: [u8; HASH_SIZE] = root_address.try_into().ok()?;
    let target = level.replica_count();
    if target == 0 {
        return Some(vec![]);
    }

    let mut queue: [Option<Replica>; 16] = [None; 16];
    let mut existing_neighborhoods = [false; 30];
    let mut sizes = [0usize, 2, 4, 8, 16];
    let mut output = Vec::with_capacity(target);
    let mut emitted = 0usize;

    // Bee's uint8 mining loop intentionally stops before 255.
    for candidate_index in 0..u8::MAX {
        if emitted >= target {
            break;
        }

        let mut id = root_address;
        id[0] = candidate_index;
        let candidate = Replica {
            id,
            address: address_for_id(&id),
        };
        let (depth, _) = add_replica(
            candidate,
            level,
            &mut queue,
            &mut existing_neighborhoods,
            &mut sizes,
        );
        if depth == 0 {
            continue;
        }

        let mut added = 0usize;
        for slot in &queue[emitted..] {
            let Some(replica) = slot else {
                break;
            };
            output.push(*replica);
            added += 1;
        }
        emitted += added;
    }

    // Bee stops mining when the uint8 candidate space reaches 255 and keeps
    // the replicas found so far. A valid root therefore still has a usable
    // (occasionally short) plan instead of making upload/root lookup fail.
    Some(output)
}

fn add_replica(
    candidate: Replica,
    level: RedundancyLevel,
    queue: &mut [Option<Replica>; 16],
    existing_neighborhoods: &mut [bool; 30],
    sizes: &mut [usize; 5],
) -> (usize, usize) {
    if level == RedundancyLevel::None {
        return (0, 0);
    }

    let depth = level.as_u8() as usize;
    let index_bases = [0usize, 2, 6, 14];
    let neighborhood = index_bases[depth - 1] + (candidate.address[0] >> (8 - depth)) as usize;
    if existing_neighborhoods[neighborhood] {
        return (0, 0);
    }
    existing_neighborhoods[neighborhood] = true;

    let lower = RedundancyLevel::from_u8(level.as_u8() - 1).expect("lower redundancy level");
    let (mut covered, mut offset) =
        add_replica(candidate, lower, queue, existing_neighborhoods, sizes);
    if covered == 0 {
        offset = sizes[depth - 1];
        sizes[depth - 1] += 1;
        queue[offset] = Some(candidate);
        covered = level.replica_count();
    }
    (covered, offset)
}

impl fmt::Display for ReedSolomonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidShardCount => "invalid Reed-Solomon shard count",
            Self::InvalidShardSize => "inconsistent Reed-Solomon shard size",
            Self::TooFewShards => "too few Reed-Solomon shards",
            Self::SingularMatrix => "singular Reed-Solomon matrix",
        };
        f.write_str(message)
    }
}

#[cfg(test)]
pub fn encode_parity(
    data_shards: &[Vec<u8>],
    parity_count: usize,
) -> Result<Vec<Vec<u8>>, ReedSolomonError> {
    let encoder = ParityEncoder::new(data_shards, parity_count)?;
    let mut parity = Vec::with_capacity(encoder.parity_count());
    for parity_index in 0..encoder.parity_count() {
        parity.push(encoder.encode_shard(parity_index)?);
    }
    Ok(parity)
}

#[cfg(test)]
pub fn reconstruct_data(
    shards: &mut [Option<Vec<u8>>],
    data_count: usize,
) -> Result<(), ReedSolomonError> {
    reconstruct_data_targets(shards, data_count, None)
}

/// Reconstruct only the missing data shards named by `requested_indices`.
///
/// Present data shards are left untouched, duplicate indices are harmless, and
/// missing data shards outside the requested set remain `None`. The complete
/// shard set is still used as input to Reed-Solomon decoding, since any
/// recovery requires `data_count` available shards.
pub fn reconstruct_data_indices(
    shards: &mut [Option<Vec<u8>>],
    data_count: usize,
    requested_indices: &[usize],
) -> Result<(), ReedSolomonError> {
    reconstruct_data_targets(shards, data_count, Some(requested_indices))
}

fn reconstruct_data_targets(
    shards: &mut [Option<Vec<u8>>],
    data_count: usize,
    requested_indices: Option<&[usize]>,
) -> Result<(), ReedSolomonError> {
    if data_count == 0 || shards.len() <= data_count || shards.len() > 256 {
        return Err(ReedSolomonError::InvalidShardCount);
    }

    let requested_mask = if let Some(requested_indices) = requested_indices {
        let mut requested_mask = vec![false; data_count];
        for &index in requested_indices {
            let Some(requested) = requested_mask.get_mut(index) else {
                return Err(ReedSolomonError::InvalidShardCount);
            };
            *requested = true;
        }
        Some(requested_mask)
    } else {
        None
    };

    let shard_size = shards
        .iter()
        .find_map(|shard| shard.as_ref().map(Vec::len))
        .ok_or(ReedSolomonError::TooFewShards)?;
    if shard_size == 0
        || shards
            .iter()
            .flatten()
            .any(|shard| shard.len() != shard_size)
    {
        return Err(ReedSolomonError::InvalidShardSize);
    }

    let selected: Vec<usize> = shards
        .iter()
        .enumerate()
        .filter_map(|(index, shard)| shard.as_ref().map(|_| index))
        .take(data_count)
        .collect();
    if selected.len() < data_count {
        return Err(ReedSolomonError::TooFewShards);
    }

    let missing_indices = (0..data_count)
        .filter(|&data_index| {
            shards[data_index].is_none()
                && requested_mask
                    .as_ref()
                    .is_none_or(|requested| requested[data_index])
        })
        .collect::<Vec<_>>();
    if missing_indices.is_empty() {
        return Ok(());
    }

    let matrix = cached_coding_matrix(data_count, shards.len())?;
    let selected_shards: Vec<&[u8]> = selected
        .iter()
        .map(|&index| {
            shards[index]
                .as_ref()
                .expect("selected shard exists")
                .as_slice()
        })
        .collect();

    let recovered = if requested_mask.is_some() {
        let decode_rows = inverse_rows_for_selected(&matrix, &selected, &missing_indices)?;
        missing_indices
            .into_iter()
            .zip(decode_rows)
            .map(|(data_index, decode_row)| {
                (
                    data_index,
                    code_row_slices(&decode_row, &selected_shards, shard_size),
                )
            })
            .collect::<Vec<_>>()
    } else {
        // Preserve the full-reconstruction path, including direct use of the
        // complete inverse without cloning its missing rows.
        let sub_matrix = selected
            .iter()
            .map(|&index| matrix[index].clone())
            .collect::<Vec<_>>();
        let data_decode_matrix = invert(sub_matrix)?;
        missing_indices
            .into_iter()
            .map(|data_index| {
                (
                    data_index,
                    code_row_slices(
                        &data_decode_matrix[data_index],
                        &selected_shards,
                        shard_size,
                    ),
                )
            })
            .collect::<Vec<_>>()
    };
    drop(selected_shards);
    for (data_index, shard) in recovered {
        shards[data_index] = Some(shard);
    }
    Ok(())
}

/// Return only selected rows of the inverse decoding matrix.
///
/// If `S` contains the coding-matrix rows for the available shards, decoding
/// data row `i` needs `row_i(S^-1)`. Rather than augmenting `S` with an entire
/// identity matrix, solve `S^T x = e_i` for each requested row. For `k`
/// requested rows this uses an `n x (n + k)` workspace and does not clone the
/// `n x n` selected submatrix.
fn inverse_rows_for_selected(
    coding_matrix: &[Vec<u8>],
    selected: &[usize],
    requested_rows: &[usize],
) -> Result<Vec<Vec<u8>>, ReedSolomonError> {
    let size = selected.len();
    if size == 0
        || requested_rows.iter().any(|&row| row >= size)
        || selected.iter().any(|&row| {
            coding_matrix
                .get(row)
                .is_none_or(|coefficients| coefficients.len() != size)
        })
    {
        return Err(ReedSolomonError::SingularMatrix);
    }
    if requested_rows.is_empty() {
        return Ok(vec![]);
    }

    let width = size
        .checked_add(requested_rows.len())
        .ok_or(ReedSolomonError::InvalidShardCount)?;
    let mut work = vec![vec![0; width]; size];

    // Transpose the selected coding rows directly into the elimination
    // workspace. Column `selected_column` represents one available shard.
    for (selected_column, &coding_row) in selected.iter().enumerate() {
        let coefficients = &coding_matrix[coding_row];
        for original_row in 0..size {
            work[original_row][selected_column] = coefficients[original_row];
        }
    }
    for (target_column, &requested_row) in requested_rows.iter().enumerate() {
        work[requested_row][size + target_column] = 1;
    }

    gauss_jordan(&mut work, size)?;

    let mut rows = vec![vec![0; size]; requested_rows.len()];
    for (target_column, row) in rows.iter_mut().enumerate() {
        for selected_row in 0..size {
            row[selected_row] = work[selected_row][size + target_column];
        }
    }
    Ok(rows)
}

#[cfg(test)]
pub fn padded_chunk(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() > CHUNK_WITH_SPAN_SIZE {
        return None;
    }
    let mut padded = vec![0; CHUNK_WITH_SPAN_SIZE];
    padded[..data.len()].copy_from_slice(data);
    Some(padded)
}

fn cached_coding_matrix(
    data_count: usize,
    total_count: usize,
) -> Result<Rc<Vec<Vec<u8>>>, ReedSolomonError> {
    let key = (data_count, total_count);
    if let Some(matrix) = CODING_MATRIX_CACHE.with(|cache| cache.borrow_mut().get(key)) {
        return Ok(matrix);
    }

    let matrix = Rc::new(coding_matrix(data_count, total_count)?);
    CODING_MATRIX_CACHE.with(|cache| {
        cache.borrow_mut().insert(key, Rc::clone(&matrix));
    });
    Ok(matrix)
}

fn coding_matrix(data_count: usize, total_count: usize) -> Result<Vec<Vec<u8>>, ReedSolomonError> {
    if data_count == 0 || total_count < data_count || total_count > 256 {
        return Err(ReedSolomonError::InvalidShardCount);
    }

    let mut vandermonde = vec![vec![0; data_count]; total_count];
    for (row_index, row) in vandermonde.iter_mut().enumerate() {
        for (column_index, value) in row.iter_mut().enumerate() {
            *value = gf_pow(row_index as u8, column_index);
        }
    }

    let top = vandermonde[..data_count].to_vec();
    let top_inverse = invert(top)?;
    Ok(matrix_multiply(&vandermonde, &top_inverse))
}

fn code_row_slices(coefficients: &[u8], inputs: &[&[u8]], shard_size: usize) -> Vec<u8> {
    let mut output = vec![0; shard_size];
    for (&coefficient, input) in coefficients.iter().zip(inputs) {
        match coefficient {
            0 => {}
            1 => {
                for (out, &value) in output.iter_mut().zip(input.iter()) {
                    *out ^= value;
                }
            }
            _ => {
                let products = &GF_MUL[coefficient as usize];
                for (out, &value) in output.iter_mut().zip(input.iter()) {
                    *out ^= products[value as usize];
                }
            }
        }
    }
    output
}

fn matrix_multiply(left: &[Vec<u8>], right: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let rows = left.len();
    let columns = right[0].len();
    let inner = right.len();
    let mut result = vec![vec![0; columns]; rows];
    for row in 0..rows {
        for column in 0..columns {
            let mut value = 0;
            for index in 0..inner {
                value ^= gf_mul(left[row][index], right[index][column]);
            }
            result[row][column] = value;
        }
    }
    result
}

fn invert(matrix: Vec<Vec<u8>>) -> Result<Vec<Vec<u8>>, ReedSolomonError> {
    let size = matrix.len();
    if size == 0 || matrix.iter().any(|row| row.len() != size) {
        return Err(ReedSolomonError::SingularMatrix);
    }

    let width = size
        .checked_mul(2)
        .ok_or(ReedSolomonError::InvalidShardCount)?;
    let mut work = vec![vec![0; width]; size];
    for row in 0..size {
        work[row][..size].copy_from_slice(&matrix[row]);
        work[row][size + row] = 1;
    }

    gauss_jordan(&mut work, size)?;

    Ok(work.into_iter().map(|row| row[size..].to_vec()).collect())
}

/// Reduce the square matrix in the first `size` columns to identity while
/// applying the same GF(256) row operations to any right-hand-side columns.
fn gauss_jordan(work: &mut [Vec<u8>], size: usize) -> Result<(), ReedSolomonError> {
    if size == 0 || work.len() != size {
        return Err(ReedSolomonError::SingularMatrix);
    }
    let width = work[0].len();
    if width < size || work.iter().any(|row| row.len() != width) {
        return Err(ReedSolomonError::SingularMatrix);
    }

    for diagonal in 0..size {
        if work[diagonal][diagonal] == 0 {
            let replacement = (diagonal + 1..size)
                .find(|&row| work[row][diagonal] != 0)
                .ok_or(ReedSolomonError::SingularMatrix)?;
            work.swap(diagonal, replacement);
        }

        let pivot = work[diagonal][diagonal];
        if pivot != 1 {
            let scale = gf_div(1, pivot);
            for value in &mut work[diagonal] {
                *value = gf_mul(*value, scale);
            }
        }

        for row in diagonal + 1..size {
            let scale = work[row][diagonal];
            if scale == 0 {
                continue;
            }
            for column in 0..width {
                work[row][column] ^= gf_mul(scale, work[diagonal][column]);
            }
        }
    }

    for diagonal in 0..size {
        for row in 0..diagonal {
            let scale = work[row][diagonal];
            if scale == 0 {
                continue;
            }
            for column in 0..width {
                work[row][column] ^= gf_mul(scale, work[diagonal][column]);
            }
        }
    }

    Ok(())
}

fn gf_pow(value: u8, exponent: usize) -> u8 {
    let mut result = 1u8;
    for _ in 0..exponent {
        result = gf_mul(result, value);
    }
    result
}

fn gf_div(numerator: u8, denominator: u8) -> u8 {
    assert!(denominator != 0, "division by zero in GF(256)");
    if numerator == 0 {
        return 0;
    }

    let mut inverse = 1u8;
    let mut base = denominator;
    let mut exponent = 254u16;
    while exponent > 0 {
        if exponent & 1 != 0 {
            inverse = gf_mul(inverse, base);
        }
        base = gf_mul(base, base);
        exponent >>= 1;
    }
    gf_mul(numerator, inverse)
}

const fn gf_mul(mut left: u8, mut right: u8) -> u8 {
    let mut result = 0u8;
    let mut bit = 0;
    while bit < 8 {
        if right & 1 != 0 {
            result ^= left;
        }
        let high = left & 0x80;
        left <<= 1;
        if high != 0 {
            left ^= 0x1d;
        }
        right >>= 1;
        bit += 1;
    }
    result
}

const fn multiplication_table() -> [[u8; 256]; 256] {
    let mut table = [[0; 256]; 256];
    let mut left = 0;
    while left < 256 {
        let mut right = 0;
        while right < 256 {
            table[left][right] = gf_mul(left as u8, right as u8);
            right += 1;
        }
        left += 1;
    }
    table
}

static GF_MUL: [[u8; 256]; 256] = multiplication_table();

#[cfg(test)]
mod tests {
    use super::*;

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
    fn coding_matrix_cache_is_bounded_and_keeps_hot_entries() {
        CODING_MATRIX_CACHE.with(|cache| *cache.borrow_mut() = CodingMatrixCache::default());

        let hot = cached_coding_matrix(1, 2).unwrap();
        // Fill the cache exactly, then touch the oldest entry before forcing
        // one eviction. Active encoders retain their Rc even when their cache
        // entry is eventually evicted.
        for total_count in 3..=CODING_MATRIX_CACHE_ENTRIES + 1 {
            cached_coding_matrix(1, total_count).unwrap();
        }
        assert!(Rc::ptr_eq(&hot, &cached_coding_matrix(1, 2).unwrap()));
        cached_coding_matrix(1, CODING_MATRIX_CACHE_ENTRIES + 2).unwrap();

        CODING_MATRIX_CACHE.with(|cache| {
            let cache = cache.borrow();
            assert_eq!(cache.matrices.len(), CODING_MATRIX_CACHE_ENTRIES);
            assert!(cache.matrices.contains_key(&(1, 2)));
            assert!(!cache.matrices.contains_key(&(1, 3)));
            assert!(cache.order.len() <= CODING_MATRIX_CACHE_ENTRIES * 2);
        });

        assert_eq!(hot.as_ref(), &vec![vec![1], vec![1]]);
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

        assert_eq!(
            inverse_rows_for_selected(&[], &[], &[]),
            Err(ReedSolomonError::SingularMatrix)
        );
        assert_eq!(
            inverse_rows_for_selected(&[vec![1]], &[0], &[1]),
            Err(ReedSolomonError::SingularMatrix)
        );
        assert_eq!(
            inverse_rows_for_selected(&[vec![1]], &[1], &[0]),
            Err(ReedSolomonError::SingularMatrix)
        );
        assert_eq!(
            inverse_rows_for_selected(&[vec![1]], &[0], &[]).unwrap(),
            Vec::<Vec<u8>>::new()
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
    fn bee_redundancy_tables_and_branching_match() {
        assert_eq!(RedundancyLevel::Medium.max_shards(false), 119);
        assert_eq!(RedundancyLevel::Medium.max_shards(true), 59);
        assert_eq!(RedundancyLevel::Strong.max_shards(false), 107);
        assert_eq!(RedundancyLevel::Insane.max_shards(false), 97);
        assert_eq!(RedundancyLevel::Paranoid.max_shards(false), 39);
        assert_eq!(RedundancyLevel::Medium.parities(2, false), 3);
        assert_eq!(RedundancyLevel::Paranoid.parities(39, false), 89);

        let first_level = reference_layout(4097, RedundancyLevel::Medium, false).unwrap();
        assert_eq!(first_level.data_shards, 2);
        assert_eq!(first_level.parity_shards, 3);
        assert_eq!(first_level.child_capacity, 4096);

        let carried = reference_layout(119 * 4096 + 1, RedundancyLevel::Medium, false).unwrap();
        assert_eq!(carried.data_shards, 2);
        assert_eq!(carried.child_capacity, 119 * 4096);
    }

    #[test]
    fn span_level_round_trip_preserves_length() {
        let mut span = 123_456_789u64.to_le_bytes();
        encode_level(&mut span, RedundancyLevel::Insane);
        assert_eq!(span[7], 0x83);
        assert_eq!(
            decode_span(&span),
            Some((RedundancyLevel::Insane, 123_456_789))
        );
    }

    #[test]
    fn parity_recovers_missing_data() {
        let data = vec![
            (0..64).map(|value| value as u8).collect::<Vec<_>>(),
            (0..64)
                .map(|value| (value as u8).wrapping_mul(3))
                .collect::<Vec<_>>(),
            (0..64)
                .map(|value| (value as u8).wrapping_add(91))
                .collect::<Vec<_>>(),
        ];
        let parity = encode_parity(&data, 3).unwrap();
        let mut shards: Vec<Option<Vec<u8>>> =
            data.iter().cloned().chain(parity).map(Some).collect();
        shards[0] = None;
        shards[2] = None;
        reconstruct_data(&mut shards, 3).unwrap();
        assert_eq!(shards[0].as_ref(), Some(&data[0]));
        assert_eq!(shards[2].as_ref(), Some(&data[2]));
    }

    #[test]
    fn targeted_reconstruction_only_materializes_requested_missing_data() {
        let data = deterministic_shards(7, 73);
        let parity = encode_parity(&data, 4).unwrap();
        let mut shards: Vec<Option<Vec<u8>>> =
            data.iter().cloned().chain(parity).map(Some).collect();
        shards[0] = None;
        shards[2] = None;
        shards[5] = None;
        shards[8] = None;
        let unavailable = shards.clone();

        reconstruct_data_indices(&mut shards, data.len(), &[5, 2, 5]).unwrap();

        assert!(
            shards[0].is_none(),
            "unrequested data must not be materialized"
        );
        assert_eq!(shards[2].as_ref(), Some(&data[2]));
        assert_eq!(shards[5].as_ref(), Some(&data[5]));
        assert_eq!(
            &shards[data.len()..],
            &unavailable[data.len()..],
            "targeted data recovery must not synthesize parity shards"
        );
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
    fn klauspost_matrix_inverse_vector_matches() {
        let inverse = invert(vec![
            vec![56, 23, 98],
            vec![3, 100, 200],
            vec![45, 201, 123],
        ])
        .unwrap();
        assert_eq!(
            inverse,
            vec![vec![175, 133, 33], vec![130, 13, 245], vec![112, 35, 126]]
        );
    }

    #[test]
    fn target_rhs_rows_match_full_inverse_for_every_small_selection_and_target_subset() {
        for data_count in 1..=5 {
            let total_count = data_count + 2;
            let matrix = coding_matrix(data_count, total_count).unwrap();
            for selection_mask in 0usize..(1usize << total_count) {
                if selection_mask.count_ones() as usize != data_count {
                    continue;
                }
                let selected = (0..total_count)
                    .filter(|&index| selection_mask & (1usize << index) != 0)
                    .collect::<Vec<_>>();
                let full_inverse = invert(
                    selected
                        .iter()
                        .map(|&index| matrix[index].clone())
                        .collect(),
                )
                .unwrap();

                for target_mask in 0usize..(1usize << data_count) {
                    let targets = (0..data_count)
                        .filter(|&index| target_mask & (1usize << index) != 0)
                        .collect::<Vec<_>>();
                    let targeted = inverse_rows_for_selected(&matrix, &selected, &targets).unwrap();
                    let expected = targets
                        .iter()
                        .map(|&index| full_inverse[index].clone())
                        .collect::<Vec<_>>();
                    assert_eq!(
                        targeted, expected,
                        "data={data_count} selection={selection_mask:#x} targets={target_mask:#x}"
                    );
                }
            }
        }
    }

    #[test]
    fn replica_scheduler_preserves_bee_dispersion_order() {
        let root = [7u8; HASH_SIZE];
        let replicas = replicas(&root, RedundancyLevel::Strong, |id| *id).unwrap();
        let candidate_bytes: Vec<u8> = replicas.iter().map(|replica| replica.id[0]).collect();
        assert_eq!(candidate_bytes, vec![0, 128, 64, 192]);
    }

    #[test]
    fn full_medium_group_encodes_and_recovers() {
        let data: Vec<Vec<u8>> = (0..119)
            .map(|shard| {
                (0..CHUNK_WITH_SPAN_SIZE)
                    .map(|offset| (shard as u8).wrapping_mul(17) ^ offset as u8)
                    .collect()
            })
            .collect();
        let parity = encode_parity(&data, 9).unwrap();
        assert_eq!(parity.len(), 9);
        assert!(
            parity
                .iter()
                .all(|shard| shard.len() == CHUNK_WITH_SPAN_SIZE)
        );

        let mut shards: Vec<Option<Vec<u8>>> =
            data.iter().cloned().chain(parity).map(Some).collect();
        for missing in [0, 17, 58, 118] {
            shards[missing] = None;
        }

        let mut targeted = shards.clone();
        reconstruct_data_indices(&mut targeted, 119, &[118, 0]).unwrap();
        assert_eq!(targeted[0].as_ref(), Some(&data[0]));
        assert_eq!(targeted[118].as_ref(), Some(&data[118]));
        assert!(targeted[17].is_none());
        assert!(targeted[58].is_none());

        reconstruct_data(&mut shards, 119).unwrap();
        for recovered in [0, 17, 58, 118] {
            assert_eq!(shards[recovered].as_ref(), Some(&data[recovered]));
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
