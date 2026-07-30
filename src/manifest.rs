use serde_json::Value;
use std::{cell::RefCell, rc::Rc};

pub const MANTARAY_PREFIX_MAX_BYTES: usize = 30;

const NODE_TYPE_VALUE: u8 = 2;
const NODE_TYPE_EDGE: u8 = 4;
const NODE_TYPE_WITH_PATH_SEPARATOR: u8 = 8;
const NODE_TYPE_WITH_METADATA: u8 = 16;
const METADATA_BLOCK_BYTES: usize = 32;

pub fn common_prefix_bytes(paths: &[&[u8]]) -> Option<Vec<u8>> {
    let first = *paths.first()?;
    let mut prefix_len = first.len().min(MANTARAY_PREFIX_MAX_BYTES);

    for path in &paths[1..] {
        prefix_len = prefix_len.min(path.len());
        let mut matching = 0;
        while matching < prefix_len && first[matching] == path[matching] {
            matching += 1;
        }
        prefix_len = matching;
    }

    (prefix_len > 0).then(|| first[..prefix_len].to_vec())
}

pub fn split_prefix_bytes(path: &[u8], first_capacity: usize) -> Option<Vec<Vec<u8>>> {
    if path.is_empty() || first_capacity == 0 || first_capacity > MANTARAY_PREFIX_MAX_BYTES {
        return None;
    }

    let mut prefixes = Vec::new();
    let first_len = path.len().min(first_capacity);
    prefixes.push(path[..first_len].to_vec());

    let mut offset = first_len;
    while offset < path.len() {
        let end = (offset + MANTARAY_PREFIX_MAX_BYTES).min(path.len());
        prefixes.push(path[offset..end].to_vec());
        offset = end;
    }

    Some(prefixes)
}

pub fn fork_type(prefix: &[u8], has_value: bool, has_edge: bool, has_metadata: bool) -> u8 {
    let mut node_type = 0;
    if has_value {
        node_type |= NODE_TYPE_VALUE;
    }
    if has_edge {
        node_type |= NODE_TYPE_EDGE;
    }
    // Bee excludes a leading slash from its path-separator flag.
    if prefix
        .iter()
        .position(|byte| *byte == b'/')
        .is_some_and(|index| index > 0)
    {
        node_type |= NODE_TYPE_WITH_PATH_SEPARATOR;
    }
    if has_metadata {
        node_type |= NODE_TYPE_WITH_METADATA;
    }
    node_type
}

pub fn padded_metadata_len(metadata_len: usize) -> Option<usize> {
    let with_size = metadata_len.checked_add(2)?;
    let padding = if with_size < METADATA_BLOCK_BYTES {
        METADATA_BLOCK_BYTES - with_size
    } else if with_size > METADATA_BLOCK_BYTES {
        METADATA_BLOCK_BYTES - (with_size % METADATA_BLOCK_BYTES)
    } else {
        0
    };
    let stored_len = metadata_len.checked_add(padding)?;
    (stored_len <= u16::MAX as usize).then_some(stored_len)
}

pub fn encode_fork(
    prefix: &[u8],
    reference: &[u8],
    metadata: &[u8],
    has_edge: bool,
) -> Option<Vec<u8>> {
    encode_fork_with_separator_path(prefix, reference, metadata, has_edge, prefix)
}

pub fn encode_fork_with_separator_path(
    prefix: &[u8],
    reference: &[u8],
    metadata: &[u8],
    has_edge: bool,
    separator_path: &[u8],
) -> Option<Vec<u8>> {
    if prefix.is_empty() || prefix.len() > MANTARAY_PREFIX_MAX_BYTES {
        return None;
    }
    if reference.len() != 32 && reference.len() != 64 {
        return None;
    }

    let has_metadata = !metadata.is_empty();
    let has_value = has_metadata;
    let metadata_len = if has_metadata {
        Some(padded_metadata_len(metadata.len())?)
    } else {
        None
    };

    let mut fork = Vec::with_capacity(32 + reference.len() + metadata_len.map_or(0, |len| 2 + len));
    fork.push(fork_type(separator_path, has_value, has_edge, has_metadata));
    fork.push(prefix.len() as u8);
    fork.extend_from_slice(prefix);
    fork.resize(32, 0);
    fork.extend_from_slice(reference);

    if let Some(stored_len) = metadata_len {
        fork.extend_from_slice(&(stored_len as u16).to_be_bytes());
        fork.extend_from_slice(metadata);
        fork.resize(fork.len() + stored_len - metadata.len(), b'\n');
    }

    Some(fork)
}

pub fn fork_prefix(fork: &[u8]) -> &[u8] {
    let prefix_len = fork.get(1).copied().unwrap_or_default() as usize;
    fork.get(2..2 + prefix_len).unwrap_or_default()
}

pub fn ordered_indexed_forks(mut forks: Vec<Vec<u8>>) -> Option<(Vec<Vec<u8>>, [u8; 32])> {
    if forks.iter().any(|fork| {
        let prefix = fork_prefix(fork);
        prefix.is_empty() || prefix.len() > MANTARAY_PREFIX_MAX_BYTES
    }) {
        return None;
    }

    forks.sort_by(|left, right| fork_prefix(left).cmp(fork_prefix(right)));
    if forks
        .windows(2)
        .any(|pair| fork_prefix(&pair[0])[0] == fork_prefix(&pair[1])[0])
    {
        return None;
    }

    let mut index = [0_u8; 32];
    for fork in &forks {
        let key = fork_prefix(fork)[0];
        index[(key / 8) as usize] |= 1 << (key % 8);
    }

    Some((forks, index))
}

pub const MAX_MANIFEST_FORKS: usize = 256;
pub const MAX_MANIFEST_PAYLOAD_BYTES: usize = 17 * 1024 * 1024;
pub const MAX_MANIFEST_DEPTH: usize = 256;
pub const MAX_MANIFEST_VISITS: usize = 4096;
pub const MAX_MANIFEST_FORK_VISITS: usize = 16_384;
pub const MAX_MANIFEST_TARGETS: usize = 8192;
pub const MAX_PARALLEL_MANIFEST_FORKS: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
enum ResolutionIdentity {
    Reference(Vec<u8>),
    Feed { owner: String, topic: String },
}

#[derive(Debug, Default)]
struct SharedBudget {
    visits: usize,
    forks: usize,
    targets: usize,
}

// Ancestry is path-local; visit and target budgets are shared across branches.
#[derive(Clone, Debug)]
pub struct ResolutionGuard {
    shared: Rc<RefCell<SharedBudget>>,
    ancestry: Rc<Vec<ResolutionIdentity>>,
}

impl Default for ResolutionGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolutionGuard {
    pub fn new() -> Self {
        Self {
            shared: Rc::new(RefCell::new(SharedBudget::default())),
            ancestry: Rc::new(Vec::new()),
        }
    }

    pub fn descend_reference(&self, reference: &[u8]) -> Option<Self> {
        self.descend(ResolutionIdentity::Reference(reference.to_vec()))
    }

    pub fn descend_feed(&self, owner: &str, topic: &str) -> Option<Self> {
        self.descend(ResolutionIdentity::Feed {
            owner: owner.to_string(),
            topic: topic.to_string(),
        })
    }

    fn descend(&self, identity: ResolutionIdentity) -> Option<Self> {
        if self.ancestry.len() >= MAX_MANIFEST_DEPTH
            || self.ancestry.iter().any(|ancestor| ancestor == &identity)
        {
            return None;
        }

        let mut shared = self.shared.borrow_mut();
        if shared.visits >= MAX_MANIFEST_VISITS {
            return None;
        }
        shared.visits += 1;
        drop(shared);

        let mut ancestry = self.ancestry.as_ref().clone();
        ancestry.push(identity);
        Some(Self {
            shared: Rc::clone(&self.shared),
            ancestry: Rc::new(ancestry),
        })
    }

    pub fn reserve_target(&self) -> bool {
        let mut shared = self.shared.borrow_mut();
        if shared.targets >= MAX_MANIFEST_TARGETS {
            return false;
        }
        shared.targets += 1;
        true
    }

    pub fn reserve_fork(&self) -> bool {
        let mut shared = self.shared.borrow_mut();
        if shared.forks >= MAX_MANIFEST_FORK_VISITS {
            return false;
        }
        shared.forks += 1;
        true
    }
}

pub fn manifest_payload_size_allowed(size: u64) -> bool {
    size <= MAX_MANIFEST_PAYLOAD_BYTES as u64
}

pub fn manifest_fork_keys(index: &[u8], reference_size: usize) -> Option<Vec<u8>> {
    if index.len() != 32 {
        return None;
    }
    if reference_size == 0 {
        return Some(Vec::new());
    }

    let mut keys = Vec::new();
    for (byte_index, bits) in index.iter().copied().enumerate() {
        for bit in 0..8 {
            if bits & (1 << bit) != 0 {
                keys.push((byte_index * 8 + bit) as u8);
            }
        }
    }
    (keys.len() <= MAX_MANIFEST_FORKS).then_some(keys)
}

const MANIFEST_FIXED_HEADER_SIZE: usize = 72;
const MANIFEST_INDEX_SIZE: usize = 32;
const MANIFEST_VERSION_START: usize = 40;
const MANIFEST_VERSION_END: usize = 71;
const MANIFEST_REFERENCE_SIZE_OFFSET: usize = 71;

const MANTARAY_VERSION_02: [u8; 31] = [
    0x57, 0x68, 0xb3, 0xb6, 0xa7, 0xdb, 0x56, 0xd2, 0x1d, 0x1a, 0xbf, 0xf4, 0x0d, 0x41, 0xce, 0xbf,
    0xc8, 0x34, 0x48, 0xfe, 0xd8, 0xd7, 0xe9, 0xb0, 0x6e, 0xc0, 0xd3, 0xb0, 0x73, 0xf2, 0x8f,
];
const MANTARAY_VERSION_01: [u8; 31] = [
    0x02, 0x51, 0x84, 0x78, 0x9d, 0x63, 0x63, 0x57, 0x66, 0xd7, 0x8c, 0x41, 0x90, 0x01, 0x96, 0xb5,
    0x7d, 0x74, 0x00, 0x87, 0x5e, 0xbe, 0x4d, 0x9b, 0x5d, 0x1e, 0x76, 0xbd, 0x96, 0x52, 0xa9,
];

#[derive(Clone, Debug)]
pub(crate) struct BzzManifestFork {
    pub(crate) fork_type: u8,
    pub(crate) prefix: Vec<u8>,
    pub(crate) reference: Vec<u8>,
    pub(crate) metadata: Option<Value>,
}

pub(crate) struct ParsedBzzManifest {
    pub(crate) ref_size: usize,
    pub(crate) forks: Vec<BzzManifestFork>,
    pub(crate) explicit_index: Option<String>,
    wrapped_reference: Option<Vec<u8>>,
}

fn valid_reference_size(ref_size: usize) -> bool {
    matches!(ref_size, 0 | 32 | 64)
}

fn valid_manifest_version(version: &[u8]) -> bool {
    version == MANTARAY_VERSION_01 || version == MANTARAY_VERSION_02
}

fn decoded_header_byte(input: &[u8], index: usize, key: &[u8; 32]) -> u8 {
    if index < MANIFEST_VERSION_START || key.iter().all(|byte| *byte == 0) {
        input[index]
    } else {
        input[index] ^ key[(index - MANIFEST_VERSION_START) % key.len()]
    }
}

pub(crate) fn is_bzz_manifest_header(input: &[u8]) -> bool {
    if input.len() < MANIFEST_FIXED_HEADER_SIZE {
        return false;
    }

    let Ok(key) = <[u8; 32]>::try_from(&input[8..40]) else {
        return false;
    };
    let mut version = [0_u8; 31];
    for (offset, byte) in version.iter_mut().enumerate() {
        *byte = decoded_header_byte(input, MANIFEST_VERSION_START + offset, &key);
    }
    if !valid_manifest_version(&version) {
        return false;
    }

    let ref_size = decoded_header_byte(input, MANIFEST_REFERENCE_SIZE_OFFSET, &key) as usize;
    valid_reference_size(ref_size)
        && input.len()
            >= MANIFEST_FIXED_HEADER_SIZE
                .saturating_add(ref_size)
                .saturating_add(MANIFEST_INDEX_SIZE)
}

fn decrypt_manifest_in_place(input: &mut [u8]) -> Option<usize> {
    if input.len() < MANIFEST_FIXED_HEADER_SIZE {
        return None;
    }

    let key: [u8; 32] = input[8..40].try_into().ok()?;
    if key.iter().any(|byte| *byte != 0) {
        for (offset, byte) in input[MANIFEST_VERSION_START..].iter_mut().enumerate() {
            *byte ^= key[offset % key.len()];
        }
    }

    if !valid_manifest_version(&input[MANIFEST_VERSION_START..MANIFEST_VERSION_END]) {
        return None;
    }

    let ref_size = input[MANIFEST_REFERENCE_SIZE_OFFSET] as usize;
    if !valid_reference_size(ref_size) {
        return None;
    }

    let index_delimiter = MANIFEST_FIXED_HEADER_SIZE
        .checked_add(ref_size)?
        .checked_add(MANIFEST_INDEX_SIZE)?;
    (input.len() >= index_delimiter).then_some(ref_size)
}

pub(crate) fn parse_bzz_manifest(mut input: Vec<u8>) -> Option<ParsedBzzManifest> {
    let payload_size = u64::try_from(input.len().checked_sub(8)?).ok()?;
    if !manifest_payload_size_allowed(payload_size) {
        return None;
    }

    let ref_size = decrypt_manifest_in_place(&mut input)?;
    let reference_start = MANIFEST_FIXED_HEADER_SIZE;
    let reference_end = reference_start.checked_add(ref_size)?;
    let wrapped_reference = match input[reference_start..reference_end].to_vec() {
        reference if !reference.is_empty() && reference.iter().any(|byte| *byte != 0) => {
            Some(reference)
        }
        _ => None,
    };

    let index_delimiter = reference_end.checked_add(MANIFEST_INDEX_SIZE)?;
    let fork_keys = manifest_fork_keys(&input[reference_end..index_delimiter], ref_size)?;
    if fork_keys.len() > MAX_MANIFEST_FORKS {
        return None;
    }

    let mut fork_start_current = index_delimiter;
    let mut forks = Vec::with_capacity(fork_keys.len());
    let mut explicit_index = None;

    for _fork_key in fork_keys {
        let fork_start = fork_start_current;
        if input.len() < fork_start.checked_add(32)?.checked_add(ref_size)? {
            return None;
        }

        let fork_type = input[fork_start];
        let fork_prefix_length = input[fork_start + 1] as usize;
        if fork_prefix_length == 0
            || fork_prefix_length > 30
            || input.len() < fork_start.checked_add(2)?.checked_add(fork_prefix_length)?
        {
            return None;
        }

        // Bee may split a UTF-8 code point across adjacent Mantaray edges.
        let prefix = input[fork_start + 2..fork_start + 2 + fork_prefix_length].to_vec();

        let fork_prefix_delimiter = fork_start.checked_add(32)?;
        let fork_reference_delimiter = fork_prefix_delimiter.checked_add(ref_size)?;
        let reference = input[fork_prefix_delimiter..fork_reference_delimiter].to_vec();

        let metadata = if fork_type & 16 == 16 {
            if input.len() < fork_reference_delimiter.checked_add(2)? {
                return None;
            }

            let fork_metadata_bytesize: [u8; 2] = input
                [fork_reference_delimiter..fork_reference_delimiter + 2]
                .try_into()
                .ok()?;
            let metadata_size = u16::from_be_bytes(fork_metadata_bytesize) as usize;
            let fork_metadata_delimiter = fork_reference_delimiter
                .checked_add(2)?
                .checked_add(metadata_size)?;

            if input.len() < fork_metadata_delimiter {
                return None;
            }

            fork_start_current = fork_metadata_delimiter;
            let parsed: Option<Value> = serde_json::from_slice(
                &input[fork_reference_delimiter + 2..fork_metadata_delimiter],
            )
            .ok();

            if let Some(index) = parsed
                .as_ref()
                .and_then(|value| value.get("website-index-document"))
                .and_then(Value::as_str)
            {
                explicit_index = Some(index.to_string());
            }

            parsed
        } else {
            fork_start_current = fork_reference_delimiter;
            None
        };

        forks.push(BzzManifestFork {
            fork_type,
            prefix,
            reference,
            metadata,
        });
    }

    Some(ParsedBzzManifest {
        ref_size,
        forks,
        explicit_index,
        wrapped_reference,
    })
}

pub(crate) fn manifest_wrapped_reference(parsed: ParsedBzzManifest) -> Option<Vec<u8>> {
    parsed.wrapped_reference
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn acquire_bzz_collection(
    reference: Vec<u8>,
    chunk_retrieve_chan: &crate::ChunkRetrieveSender,
) -> Vec<u8> {
    use libp2p::futures::{StreamExt, stream};

    let (targets, index) = crate::bzz_stream::collect_reference_targets(
        Vec::new(),
        reference,
        chunk_retrieve_chan,
        ResolutionGuard::new(),
    )
    .await;

    let target_count = targets.len();
    let loads = targets.into_iter().map(|target| {
        let chunk_retrieve_chan = chunk_retrieve_chan.clone();
        async move {
            let data = crate::retrieval::retrieve_data_payload(
                &target.data_reference,
                &chunk_retrieve_chan,
            )
            .await;
            if data.is_empty() {
                return None;
            }
            let path = if target.raw_fallback {
                if data.len() < 64 {
                    "unknown00".to_string()
                } else {
                    "unknown01".to_string()
                }
            } else {
                target.path
            };
            Some((data, target.mime, path))
        }
    });

    let mut loads = stream::iter(loads).buffered(MAX_PARALLEL_MANIFEST_FORKS);
    let mut resources = Vec::with_capacity(target_count);
    while let Some(resource) = loads.next().await {
        if let Some(resource) = resource {
            resources.push(resource);
        }
    }

    if resources.is_empty() {
        return crate::encode_resources(
            vec![(vec![], "not found".to_string(), "not found".to_string())],
            index,
        );
    }

    crate::encode_resources(resources, index)
}
