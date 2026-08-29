use std::{cell::RefCell, collections::VecDeque, fmt, rc::Rc};

pub const SPAN_SIZE: usize = 8;
pub const CHUNK_SIZE: usize = 4096;
pub const CHUNK_WITH_SPAN_SIZE: usize = SPAN_SIZE + CHUNK_SIZE;
pub const HASH_SIZE: usize = 32;
pub const ENCRYPTED_REFERENCE_SIZE: usize = 64;
/// Bee's eight-level hash trie permits seven wrappers above data chunks.
pub const BEE_MAX_UPLOAD_TREE_LEVELS: usize = 7;
const CODING_MATRIX_CACHE_ENTRIES: usize = 64;

type CodingMatrix = Rc<Vec<Vec<u8>>>;
type CodingMatrixKey = (usize, usize);

#[derive(Default)]
struct CodingMatrixCache {
    entries: VecDeque<(CodingMatrixKey, CodingMatrix)>,
}

impl CodingMatrixCache {
    fn get(&mut self, key: CodingMatrixKey) -> Option<CodingMatrix> {
        let index = self
            .entries
            .iter()
            .position(|(candidate, _)| *candidate == key)?;
        let entry = self.entries.remove(index)?;
        let matrix = Rc::clone(&entry.1);
        self.entries.push_back(entry);
        Some(matrix)
    }

    fn insert(&mut self, key: CodingMatrixKey, matrix: CodingMatrix) {
        if self.entries.len() == CODING_MATRIX_CACHE_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back((key, matrix));
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
            // Encrypted data references are 64 bytes; parity references remain 32.
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

pub fn upload_tree_chunk_count(
    data_length: u64,
    level: RedundancyLevel,
    encrypted: bool,
) -> Option<u64> {
    let chunk_size = CHUNK_SIZE as u64;
    let mut input_chunks = data_length / chunk_size;
    if !data_length.is_multiple_of(chunk_size) {
        input_chunks = input_chunks.checked_add(1)?;
    }
    input_chunks = input_chunks.max(1);

    let max_shards = u64::try_from(level.max_shards(encrypted)).ok()?;
    if max_shards < 2 {
        return None;
    }

    let mut total_chunks = input_chunks;
    let mut tree_level = 0;

    while input_chunks > 1 {
        if tree_level >= BEE_MAX_UPLOAD_TREE_LEVELS {
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

        total_chunks = total_chunks
            .checked_add(level_parents)?
            .checked_add(level_parities)?;
        input_chunks = output_chunks;
        tree_level += 1;
    }

    Some(total_chunks)
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

pub type SplitReferences = (Vec<Vec<u8>>, Vec<Vec<u8>>);

pub fn split_references(
    payload: &[u8],
    span: u64,
    level: RedundancyLevel,
    encrypted: bool,
) -> Option<SplitReferences> {
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
        .as_chunks::<HASH_SIZE>()
        .0
        .iter()
        .map(|reference| reference.to_vec())
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

    // Bee's uint8 mining loop excludes 255.
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

pub fn reconstruct_data_indices(
    shards: &mut [Option<Vec<u8>>],
    data_count: usize,
    requested_indices: &[usize],
) -> Result<(), ReedSolomonError> {
    if data_count == 0 || shards.len() <= data_count || shards.len() > 256 {
        return Err(ReedSolomonError::InvalidShardCount);
    }

    let mut requested_mask = vec![false; data_count];
    for &index in requested_indices {
        let Some(requested) = requested_mask.get_mut(index) else {
            return Err(ReedSolomonError::InvalidShardCount);
        };
        *requested = true;
    }

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
        .filter(|&data_index| shards[data_index].is_none() && requested_mask[data_index])
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

    let decode_rows = inverse_rows_for_selected(&matrix, &selected, &missing_indices)?;
    let recovered = missing_indices
        .into_iter()
        .zip(decode_rows)
        .map(|(data_index, decode_row)| {
            (
                data_index,
                code_row_slices(&decode_row, &selected_shards, shard_size),
            )
        })
        .collect::<Vec<_>>();
    drop(selected_shards);
    for (data_index, shard) in recovered {
        shards[data_index] = Some(shard);
    }
    Ok(())
}

// Decoding row i is row_i(S^-1), found by solving S^T x = e_i.
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
    let mut work = matrix;
    for (row_index, row) in work.iter_mut().enumerate() {
        row.resize(width, 0);
        row[size + row_index] = 1;
    }

    gauss_jordan(&mut work, size)?;

    Ok(work.into_iter().map(|row| row[size..].to_vec()).collect())
}

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
            let (before_target, target_and_after) = work.split_at_mut(row);
            let source = &before_target[diagonal];
            let target = &mut target_and_after[0];
            for (target, source) in target.iter_mut().zip(source) {
                *target ^= gf_mul(scale, *source);
            }
        }
    }

    for diagonal in 0..size {
        for row in 0..diagonal {
            let scale = work[row][diagonal];
            if scale == 0 {
                continue;
            }
            let (before_source, source_and_after) = work.split_at_mut(diagonal);
            let target = &mut before_source[row];
            let source = &source_and_after[0];
            for (target, source) in target.iter_mut().zip(source) {
                *target ^= gf_mul(scale, *source);
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

pub(crate) fn validated_upload_redundancy(value: u8) -> Option<RedundancyLevel> {
    RedundancyLevel::from_u8(value)
}

pub(crate) fn validated_upload_redundancy_number(value: f64) -> Option<RedundancyLevel> {
    if !value.is_finite()
        || value.fract() != 0.0
        || !(u8::MIN as f64..=u8::MAX as f64).contains(&value)
    {
        return None;
    }
    validated_upload_redundancy(value as u8)
}

pub(crate) fn upload_redundancy_from_select(value: Option<&str>) -> RedundancyLevel {
    value
        .and_then(|value| value.parse::<u8>().ok())
        .and_then(validated_upload_redundancy)
        .unwrap_or(RedundancyLevel::DEFAULT_UPLOAD)
}

pub(crate) type ResourceEntry = (Vec<u8>, String, String);

fn encoded_field_len(len: usize) -> Option<usize> {
    8usize.checked_add(len)
}

fn push_field(output: &mut Vec<u8>, bytes: &[u8]) -> Option<()> {
    let len = u64::try_from(bytes.len()).ok()?;
    output.extend_from_slice(&len.to_le_bytes());
    output.extend_from_slice(bytes);
    Some(())
}

pub(crate) fn encode_resource_bundle(
    resources: Vec<ResourceEntry>,
    index: String,
) -> Option<Vec<u8>> {
    let mut encoded_len = encoded_field_len(index.len())?;
    for (data, media_type, name) in &resources {
        encoded_len = encoded_len
            .checked_add(encoded_field_len(media_type.len())?)?
            .checked_add(encoded_field_len(name.len())?)?
            .checked_add(encoded_field_len(data.len())?)?;
    }

    let mut output = Vec::new();
    output.try_reserve_exact(encoded_len).ok()?;
    push_field(&mut output, index.as_bytes())?;
    for (data, media_type, name) in resources {
        push_field(&mut output, media_type.as_bytes())?;
        push_field(&mut output, name.as_bytes())?;
        push_field(&mut output, &data)?;
    }
    debug_assert_eq!(output.len(), encoded_len);
    Some(output)
}

fn read_len(input: &[u8], cursor: &mut usize) -> Option<usize> {
    let end = cursor.checked_add(8)?;
    let bytes: [u8; 8] = input.get(*cursor..end)?.try_into().ok()?;
    *cursor = end;
    usize::try_from(u64::from_le_bytes(bytes)).ok()
}

fn read_bytes<'a>(input: &'a [u8], cursor: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = cursor.checked_add(len)?;
    let bytes = input.get(*cursor..end)?;
    *cursor = end;
    Some(bytes)
}

fn read_string(input: &[u8], cursor: &mut usize) -> Option<String> {
    let len = read_len(input, cursor)?;
    let bytes = read_bytes(input, cursor, len)?;
    Some(String::from_utf8(bytes.to_vec()).unwrap_or_default())
}

pub(crate) fn decode_resource_bundle(input: &[u8]) -> Option<(Vec<ResourceEntry>, String)> {
    let mut cursor = 0;
    let index = read_string(input, &mut cursor)?;
    let mut resources = Vec::new();

    while cursor < input.len() {
        let media_type = read_string(input, &mut cursor)?;
        let name = read_string(input, &mut cursor)?;
        let data_len = read_len(input, &mut cursor)?;
        let data = read_bytes(input, &mut cursor, data_len)?.to_vec();
        resources.push((data, media_type, name));
    }

    Some((resources, index))
}

pub(crate) const FILE_UPLOAD_READ_WINDOW_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct FileSlicePlan {
    size: u64,
    next: u64,
}

impl FileSlicePlan {
    pub(crate) fn new(size: u64) -> Self {
        Self { size, next: 0 }
    }
}

impl Iterator for FileSlicePlan {
    type Item = (u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.size {
            return None;
        }

        let start = self.next;
        let end = start
            .saturating_add(FILE_UPLOAD_READ_WINDOW_BYTES)
            .min(self.size);
        self.next = end;
        Some((start, end))
    }
}
