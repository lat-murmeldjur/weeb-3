use crate::{
    ChunkRetrieveSender, JsValue, RetrieveCancelToken, RetrieveGenerationMap,
    cancellable_chunk_retrieve_request, chunk_retrieve_request, mpsc,
    retrieval::{
        get_chunk, get_data, retrieve_data, seek_latest_feed_update, split_chunk_references,
    },
    retrieve_cancel_token_current,
};

use libp2p::futures::{StreamExt, future::join_all, stream::FuturesUnordered};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, VecDeque},
    future::Future,
    pin::Pin,
};

const RANGE_DISPATCH_YIELD_EVERY: usize = 128;
const RANGE_RETRIEVE_DATA_PIECE_MAX_BYTES: u64 = 4 * 1024 * 1024;
const RANGE_TREE_CHUNK_CACHE_MAX: usize = 16384;

thread_local! {
    static RANGE_TREE_CHUNK_CACHE: RefCell<RangeTreeChunkCache> =
        RefCell::new(RangeTreeChunkCache::new(RANGE_TREE_CHUNK_CACHE_MAX));
}

struct RangeTreeChunkCache {
    max_entries: usize,
    order: VecDeque<Vec<u8>>,
    chunks: HashMap<Vec<u8>, Vec<u8>>,
}

impl RangeTreeChunkCache {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            order: VecDeque::new(),
            chunks: HashMap::new(),
        }
    }

    fn get(&mut self, addr: &[u8]) -> Option<Vec<u8>> {
        let chunk = self.chunks.get(addr).cloned()?;
        self.order
            .retain(|cached_addr| cached_addr.as_slice() != addr);
        self.order.push_back(addr.to_vec());
        Some(chunk)
    }

    fn put(&mut self, addr: Vec<u8>, chunk: Vec<u8>) {
        if self.max_entries == 0 {
            return;
        }

        self.order
            .retain(|cached_addr| cached_addr.as_slice() != addr.as_slice());
        self.order.push_back(addr.clone());
        self.chunks.insert(addr, chunk);

        while self.chunks.len() > self.max_entries {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.chunks.remove(&oldest);
        }
    }
}

#[derive(Clone, Debug)]
pub struct BzzResource {
    pub reference: Vec<u8>,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct BzzMetadata {
    pub data_reference: Vec<u8>,
    pub mime: String,
    pub size: u64,
    pub etag: String,
    pub path: String,
    pub target_count: usize,
}

#[derive(Clone, Debug)]
struct BzzTarget {
    data_reference: Vec<u8>,
    mime: String,
    path: String,
}

#[derive(Clone, Debug)]
struct BzzManifestFork {
    fork_type: u8,
    prefix: String,
    reference: Vec<u8>,
    metadata: Option<Value>,
}

struct ManifestTargetResult {
    targets: Vec<BzzTarget>,
    fallback_index: Option<String>,
    explicit_index: Option<String>,
}

struct ParsedBzzManifest {
    cd: Vec<u8>,
    ref_size: usize,
    forks: Vec<BzzManifestFork>,
    explicit_index: Option<String>,
}

type RangeJoiner = FuturesUnordered<Pin<Box<dyn Future<Output = RangeFetch>>>>;
type TreeJoiner = FuturesUnordered<Pin<Box<dyn Future<Output = (Vec<u8>, Vec<u8>)>>>>;

struct RangeFetch {
    subtree_start: u64,
    addr: Vec<u8>,
    payload: RangeFetchPayload,
}

enum RangeFetchPayload {
    Chunk(Vec<u8>),
    Data(Vec<u8>),
}

enum RangeChild {
    Chunk { start: u64, addr: Vec<u8> },
    Data { start: u64, addr: Vec<u8> },
}

enum RangeChunkWork {
    Skip,
    Leaf(u64, Vec<u8>),
    Children(Vec<RangeChild>),
}

fn strip_known_bzz_prefix(resource: &str) -> String {
    let mut resource0 = resource.trim().to_string();

    if let Some(query_pos) = resource0.find('?') {
        resource0.truncate(query_pos);
    }

    let markers = [
        "/weeb-3/#/bzz/",
        "/weeb-3/bzz/",
        "/#/bzz/",
        "/bzz/",
        "weeb-3/#/bzz/",
        "weeb-3/bzz/",
        "#/bzz/",
        "bzz/",
    ];

    for marker in markers {
        if let Some(idx) = resource0.find(marker) {
            let mut stripped = resource0[idx + marker.len()..]
                .trim_start_matches('/')
                .to_string();
            if let Some(hash_pos) = stripped.find('#') {
                stripped.truncate(hash_pos);
            }
            return stripped;
        }
    }

    if let Some(hash_pos) = resource0.find('#') {
        resource0 = resource0[hash_pos + 1..].to_string();
    }

    resource0.trim_start_matches('/').to_string()
}

fn is_reference_hex(segment: &str) -> bool {
    (segment.len() == 64 || segment.len() == 128)
        && segment.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
}

pub fn parse_bzz_resource(resource: &str) -> Option<BzzResource> {
    let stripped = strip_known_bzz_prefix(resource);
    let mut parts = stripped.splitn(2, '/');
    let reference_hex = parts.next().unwrap_or_default();

    if !is_reference_hex(reference_hex) {
        return None;
    }

    let reference = match hex::decode(reference_hex) {
        Ok(reference) if reference.len() == 32 || reference.len() == 64 => reference,
        _ => return None,
    };

    Some(BzzResource {
        reference,
        path: normalize_bzz_path(parts.next().unwrap_or_default()),
    })
}

pub fn bzz_reference_hex(resource: &str) -> Option<String> {
    parse_bzz_resource(resource).map(|resource| hex::encode(resource.reference))
}

pub fn normalize_bzz_path(path: &str) -> String {
    path.trim().trim_matches('/').to_string()
}

fn bzz_paths_match(left: &str, right: &str) -> bool {
    normalize_bzz_path(left) == normalize_bzz_path(right)
}

fn child_path(path_prefix_heritance: &str, fork_prefix: &str) -> String {
    let mut bequeath = String::new();
    bequeath.push_str(path_prefix_heritance);
    bequeath.push_str(fork_prefix);
    bequeath
}

fn data_span(chunk: &[u8]) -> Option<u64> {
    if chunk.len() < 8 {
        return None;
    }

    Some(u64::from_le_bytes(chunk[0..8].try_into().ok()?))
}

fn manifest_cd(cd0: &Vec<u8>) -> Option<Vec<u8>> {
    if cd0.len() < 72 {
        return None;
    }

    let obfuscation_key = &cd0[8..40];
    let encrypted = hex::encode(obfuscation_key)
        != "0000000000000000000000000000000000000000000000000000000000000000";

    let cd = if encrypted {
        let creylen = obfuscation_key.len();
        let mut cd = (&cd0[..40]).to_vec();
        let mut i = 0;
        while 40 + i * creylen < cd0.len() {
            let mut k = creylen;
            if k > cd0.len() - (40 + i * creylen) {
                k = cd0.len() - (40 + i * creylen);
            }

            for j in (40 + i * creylen)..(40 + i * creylen + k) {
                cd.push(cd0[j] ^ obfuscation_key[j - 40 - i * creylen]);
            }

            i += 1;
        }
        cd
    } else {
        cd0.to_vec()
    };

    if cd.len() < 72 {
        return None;
    }

    let mf_version = hex::encode(&cd[40..71]);
    if mf_version != "5768b3b6a7db56d21d1abff40d41cebfc83448fed8d7e9b06ec0d3b073f28f"
        && mf_version != "025184789d63635766d78c41900196b57d7400875ebe4d9b5d1e76bd9652a9"
    {
        return None;
    }

    let ref_size = cd[71] as usize;
    if ref_size != 0 && ref_size != 32 && ref_size != 64 {
        return None;
    }

    let index_delimiter = 72 + ref_size + 32;
    if cd.len() < index_delimiter {
        return None;
    }

    Some(cd)
}

fn parse_bzz_manifest(cd0: &Vec<u8>) -> Option<ParsedBzzManifest> {
    let cd = manifest_cd(cd0)?;
    let ref_size = cd[71] as usize;
    let index_delimiter = 72 + ref_size + 32;
    let mut fork_start_current = index_delimiter;
    let mut forks = vec![];
    let mut explicit_index = None;

    while cd.len() > fork_start_current {
        let fork_start = fork_start_current;
        if cd.len() < fork_start + 32 + ref_size {
            return None;
        }

        let fork_type = cd[fork_start];
        let fork_prefix_length = cd[fork_start + 1] as usize;
        if fork_prefix_length > 30 || cd.len() < fork_start + 2 + fork_prefix_length {
            return None;
        }

        let fork_prefix = &cd[fork_start + 2..fork_start + 2 + fork_prefix_length];
        let prefix = String::from_utf8(fork_prefix.to_vec()).unwrap_or_default();

        let fork_prefix_delimiter = fork_start + 32;
        let fork_reference_delimiter = fork_prefix_delimiter + ref_size;
        let reference = cd[fork_prefix_delimiter..fork_reference_delimiter].to_vec();

        let metadata = if fork_type & 16 == 16 {
            if cd.len() < fork_reference_delimiter + 2 {
                return None;
            }

            let fork_metadata_bytesize: [u8; 2] = cd
                [fork_reference_delimiter..fork_reference_delimiter + 2]
                .try_into()
                .ok()?;
            let calc_metadata_bytesize = u16::from_be_bytes(fork_metadata_bytesize) as usize;
            let fork_metadata_delimiter = fork_reference_delimiter + 2 + calc_metadata_bytesize;

            if cd.len() < fork_metadata_delimiter {
                return None;
            }

            fork_start_current = fork_metadata_delimiter;
            let fork_metadata = &cd[fork_reference_delimiter + 2..fork_metadata_delimiter];
            let parsed: Option<Value> = serde_json::from_slice(fork_metadata).ok();

            if let Some(value) = &parsed {
                if let Some(index) = value
                    .get("website-index-document")
                    .and_then(|str0i| str0i.as_str())
                {
                    explicit_index = Some(index.to_string());
                }
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
        cd,
        ref_size,
        forks,
        explicit_index,
    })
}

fn manifest_wrapped_reference(cd0: &Vec<u8>) -> Option<Vec<u8>> {
    let parsed = parse_bzz_manifest(cd0)?;
    if parsed.ref_size != 32 && parsed.ref_size != 64 {
        return None;
    }

    let reference_start = 72;
    let reference_end = reference_start + parsed.ref_size;
    let reference = parsed.cd[reference_start..reference_end].to_vec();

    if reference.iter().all(|b| *b == 0) {
        return None;
    }

    Some(reference)
}

fn subtree_capacity_for_span(span: u64, address_length: usize) -> u64 {
    let branch_count = (4096 / address_length).max(1) as u64;
    let mut capacity = 4096_u64;

    while span > capacity {
        capacity = capacity.saturating_mul(branch_count);
        if capacity == u64::MAX {
            break;
        }
    }

    capacity
}

fn should_yield_after_range_dispatch(dispatched: usize) -> bool {
    dispatched % RANGE_DISPATCH_YIELD_EVERY == 0
}

fn cached_range_tree_chunk(addr: &[u8]) -> Option<Vec<u8>> {
    RANGE_TREE_CHUNK_CACHE.with(|cache| cache.borrow_mut().get(addr))
}

fn remember_range_tree_chunk(addr: Vec<u8>, chunk: &[u8]) {
    if data_span(chunk).is_none() {
        return;
    };

    RANGE_TREE_CHUNK_CACHE.with(|cache| cache.borrow_mut().put(addr, chunk.to_vec()));
}

async fn get_range_tree_chunk(addr: Vec<u8>, chunk_retrieve_chan: &ChunkRetrieveSender) -> Vec<u8> {
    if let Some(chunk) = cached_range_tree_chunk(&addr) {
        return chunk;
    }

    let chunk = get_chunk(addr.clone(), chunk_retrieve_chan).await;
    remember_range_tree_chunk(addr, &chunk);
    chunk
}

async fn get_range_tree_chunk_cancellable(
    addr: Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
) -> Vec<u8> {
    if let Some(chunk) = cached_range_tree_chunk(&addr) {
        return chunk;
    }

    let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
    let _ = chunk_retrieve_chan.try_send(cancellable_chunk_retrieve_request(
        addr.clone(),
        chan_out,
        cancel,
    ));
    let chunk = chan_in.recv().await.unwrap_or_default();
    remember_range_tree_chunk(addr, &chunk);
    chunk
}

fn queue_range_chunk(
    subtree_start: u64,
    addr: Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    joiner: &mut RangeJoiner,
    cancel: Option<RetrieveCancelToken>,
) {
    if let Some(chunk) = cached_range_tree_chunk(&addr) {
        joiner.push(Box::pin(async move {
            RangeFetch {
                subtree_start,
                addr,
                payload: RangeFetchPayload::Chunk(chunk),
            }
        }));
        return;
    }

    let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
    let _ = chunk_retrieve_chan.try_send(cancellable_chunk_retrieve_request(
        addr.clone(),
        chan_out,
        cancel,
    ));

    joiner.push(Box::pin(async move {
        let chunk = chan_in.recv().await.unwrap_or_default();
        remember_range_tree_chunk(addr.clone(), &chunk);
        RangeFetch {
            subtree_start,
            addr,
            payload: RangeFetchPayload::Chunk(chunk),
        }
    }));
}

fn queue_range_data(
    subtree_start: u64,
    addr: Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    joiner: &mut RangeJoiner,
    cancel: Option<RetrieveCancelToken>,
    cancel_generations: Option<RetrieveGenerationMap>,
) {
    let chunk_retrieve_chan = chunk_retrieve_chan.clone();

    joiner.push(Box::pin(async move {
        let data =
            retrieve_data_piece(&addr, &chunk_retrieve_chan, cancel, cancel_generations).await;
        RangeFetch {
            subtree_start,
            addr,
            payload: RangeFetchPayload::Data(data),
        }
    }));
}

async fn retrieve_data_piece(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
    cancel_generations: Option<RetrieveGenerationMap>,
) -> Vec<u8> {
    let root_chunk = get_range_tree_chunk_cancellable(
        data_address.to_vec(),
        chunk_retrieve_chan,
        cancel.clone(),
    )
    .await;
    let Some(root_span) = data_span(&root_chunk) else {
        web_sys::console::log_1(&JsValue::from(format!(
            "chunk not found: {}",
            hex::encode(data_address),
        )));
        return vec![];
    };

    let expected_len = match usize::try_from(root_span.saturating_add(8)) {
        Ok(expected_len) => expected_len,
        Err(_) => return vec![],
    };

    if root_span <= 4096 {
        if root_chunk.len() == expected_len {
            return root_chunk;
        }

        web_sys::console::log_1(&JsValue::from(format!(
            "retrieved chunk length ({}) mismatching span ({}) + 8 for chunk {}",
            root_chunk.len(),
            root_span,
            hex::encode(data_address),
        )));
        return vec![];
    }

    if root_span > RANGE_RETRIEVE_DATA_PIECE_MAX_BYTES {
        web_sys::console::log_1(&JsValue::from(format!(
            "range data piece too large: {} bytes for chunk {}",
            root_span,
            hex::encode(data_address),
        )));
        return vec![];
    }

    let mut data = Vec::with_capacity(expected_len);
    data.extend_from_slice(&root_chunk[0..8]);
    data.extend_from_slice(
        &retrieve_payload_range_from_root(
            data_address,
            root_chunk,
            0,
            root_span - 1,
            chunk_retrieve_chan,
            cancel,
            cancel_generations,
        )
        .await,
    );

    if data.len() == expected_len {
        data
    } else {
        web_sys::console::log_1(&JsValue::from(format!(
            "retrieved data piece length ({}) not matching span ({}) + 8 for chunk {}",
            data.len(),
            root_span,
            hex::encode(data_address),
        )));
        vec![]
    }
}

fn queue_tree_chunk(
    addr: Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    joiner: &mut TreeJoiner,
) {
    if let Some(chunk) = cached_range_tree_chunk(&addr) {
        joiner.push(Box::pin(async move { (addr, chunk) }));
        return;
    }

    let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
    let _ = chunk_retrieve_chan.try_send(chunk_retrieve_request(addr.clone(), chan_out));

    joiner.push(Box::pin(async move {
        let chunk = chan_in.recv().await.unwrap_or_default();
        remember_range_tree_chunk(addr.clone(), &chunk);
        (addr, chunk)
    }));
}

fn tree_prepare_children(address_length: usize, chunk: &[u8]) -> Option<Vec<Vec<u8>>> {
    let span = data_span(chunk)?;
    if span <= 4096 {
        return Some(vec![]);
    }

    if chunk.len() < 8 + address_length || (chunk.len() - 8) % address_length != 0 {
        return None;
    }

    let capacity = subtree_capacity_for_span(span, address_length);
    let branch_count = (4096 / address_length).max(1) as u64;
    let child_capacity = (capacity / branch_count).max(1);
    let child_refs = split_chunk_references(&chunk[8..], address_length)?;
    let subtree_end = span.checked_sub(1)?;
    let mut children = vec![];

    for (child_index, child_addr) in child_refs.into_iter().enumerate() {
        let child_start = (child_index as u64).saturating_mul(child_capacity);
        if child_start > subtree_end {
            break;
        }

        let child_span = (subtree_end - child_start + 1).min(child_capacity);
        if child_span > 4096 {
            children.push(child_addr);
        }
    }

    Some(children)
}

async fn queue_tree_prepare_children(
    children: Vec<Vec<u8>>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    joiner: &mut TreeJoiner,
    dispatched: &mut usize,
) {
    for child_addr in children {
        queue_tree_chunk(child_addr, chunk_retrieve_chan, joiner);
        *dispatched += 1;
        if should_yield_after_range_dispatch(*dispatched) {
            async_std::task::yield_now().await;
        }
    }
}

pub async fn prepare_bzz_stream(
    metadata: BzzMetadata,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> bool {
    let address_length = metadata.data_reference.len();
    if address_length != 32 && address_length != 64 {
        return false;
    }

    let root_chunk =
        get_range_tree_chunk(metadata.data_reference.clone(), chunk_retrieve_chan).await;
    let Some(children) = tree_prepare_children(address_length, &root_chunk) else {
        web_sys::console::log_1(&JsValue::from(format!(
            "could not prepare bzz stream tree for {}",
            hex::encode(&metadata.data_reference),
        )));
        return false;
    };

    if children.is_empty() {
        return true;
    }

    let mut joiner = FuturesUnordered::new();
    let mut dispatched = 0usize;
    let mut prepared = 0usize;
    let mut failed = 0usize;

    queue_tree_prepare_children(children, chunk_retrieve_chan, &mut joiner, &mut dispatched).await;

    while let Some((addr, chunk)) = joiner.next().await {
        if chunk.is_empty() {
            web_sys::console::log_1(&JsValue::from(format!(
                "could not prepare bzz stream tree chunk {}",
                hex::encode(addr),
            )));
            failed += 1;
            continue;
        }

        let Some(children) = tree_prepare_children(address_length, &chunk) else {
            web_sys::console::log_1(&JsValue::from(format!(
                "invalid bzz stream tree chunk {}",
                hex::encode(addr),
            )));
            failed += 1;
            continue;
        };

        prepared += 1;
        queue_tree_prepare_children(children, chunk_retrieve_chan, &mut joiner, &mut dispatched)
            .await;
    }

    web_sys::console::log_1(&JsValue::from(format!(
        "prepared bzz stream tree {} intermediate_chunks={} failed={}",
        metadata.etag, prepared, failed,
    )));
    prepared > 0 || failed == 0
}

async fn retrieve_data_head(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    payload_bytes: u64,
) -> Vec<u8> {
    let root_chunk = get_range_tree_chunk(data_address.to_vec(), chunk_retrieve_chan).await;
    let Some(root_span) = data_span(&root_chunk) else {
        return vec![];
    };

    if root_span <= 4096 {
        return root_chunk;
    }

    let inclusive_end = (root_span + 7).min(7 + payload_bytes);
    retrieve_data_range(data_address, 0, inclusive_end, chunk_retrieve_chan).await
}

async fn get_manifest_data_if_manifest(
    reference: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    let head = retrieve_data_head(reference, chunk_retrieve_chan, 4096).await;
    parse_bzz_manifest(&head)?;

    let Some(span) = data_span(&head) else {
        return None;
    };

    if head.len() == (span + 8) as usize {
        return Some(head);
    }

    let data = retrieve_data(reference, chunk_retrieve_chan).await;
    parse_bzz_manifest(&data)?;
    Some(data)
}

async fn get_root_manifest_data_if_manifest(
    reference: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    let root_chunk = get_range_tree_chunk(reference.to_vec(), chunk_retrieve_chan).await;
    parse_bzz_manifest(&root_chunk)?;
    Some(root_chunk)
}

async fn collect_reference_targets(
    path_prefix_heritance: String,
    reference: Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> (Vec<BzzTarget>, String) {
    if reference.len() != 32 && reference.len() != 64 {
        return (vec![], String::new());
    }

    if let Some(manifest_data) =
        get_manifest_data_if_manifest(&reference, chunk_retrieve_chan).await
    {
        return Box::pin(collect_manifest_targets(
            path_prefix_heritance,
            manifest_data,
            data_retrieve_chan,
            chunk_retrieve_chan,
        ))
        .await;
    }

    (
        vec![BzzTarget {
            data_reference: reference,
            mime: "application/octet-stream".to_string(),
            path: normalize_bzz_path(&path_prefix_heritance),
        }],
        String::new(),
    )
}

async fn collect_manifest_fork_targets(
    path_prefix_heritance: String,
    ref_size: usize,
    fork: BzzManifestFork,
    data_retrieve_chan: mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: ChunkRetrieveSender,
) -> ManifestTargetResult {
    let mut result = ManifestTargetResult {
        targets: vec![],
        fallback_index: None,
        explicit_index: None,
    };

    if fork.fork_type & 16 == 16 {
        let Some(metadata) = fork.metadata else {
            return result;
        };

        if let (Some(owner), Some(topic)) = (
            metadata
                .get("swarm-feed-owner")
                .and_then(|str0f0| str0f0.as_str())
                .map(|owner| owner.to_string()),
            metadata
                .get("swarm-feed-topic")
                .and_then(|str0f1| str0f1.as_str())
                .map(|topic| topic.to_string()),
        ) {
            let feed_data_soc =
                seek_latest_feed_update(owner, topic, &chunk_retrieve_chan, 8).await;

            if feed_data_soc.len() >= 8 {
                let mut feed_data_content = vec![];
                let soc_wrapped_span =
                    u64::from_le_bytes(feed_data_soc[0..8].try_into().unwrap_or([0; 8]));

                if soc_wrapped_span <= 4096 {
                    feed_data_content = feed_data_soc.to_vec();
                } else if ref_size > 0 {
                    let lens = (feed_data_soc.len() - 8) / ref_size;
                    feed_data_content.extend_from_slice(&feed_data_soc[0..8]);

                    let feed_leaf_refs = (0..lens)
                        .map(|i| {
                            let ref_start = 8 + i * ref_size;
                            let ref_end = 8 + (i + 1) * ref_size;
                            feed_data_soc[ref_start..ref_end].to_vec()
                        })
                        .collect::<Vec<_>>();

                    let feed_leaf_loads = feed_leaf_refs.into_iter().map(|addr| {
                        let data_retrieve_chan = data_retrieve_chan.clone();
                        async move { get_data(addr, &data_retrieve_chan).await }
                    });

                    for leaf in join_all(feed_leaf_loads).await {
                        if leaf.len() > 8 {
                            feed_data_content.extend_from_slice(&leaf[8..]);
                        }
                    }
                }

                let (mut feed_targets, feed_index) = Box::pin(collect_manifest_targets(
                    String::new(),
                    feed_data_content,
                    &data_retrieve_chan,
                    &chunk_retrieve_chan,
                ))
                .await;
                result.targets.append(&mut feed_targets);
                result.fallback_index = Some(feed_index);
            }
        }

        if let Some(explicit_index) = metadata
            .get("website-index-document")
            .and_then(|str0i| str0i.as_str())
        {
            result.explicit_index = Some(explicit_index.to_string());
        }

        let bequeath = child_path(&path_prefix_heritance, &fork.prefix);
        let Some(mime) = metadata
            .get("Content-Type")
            .and_then(|str0| str0.as_str())
            .map(|mime| mime.to_string())
        else {
            let (mut child_targets, child_index) = Box::pin(collect_reference_targets(
                bequeath,
                fork.reference,
                &data_retrieve_chan,
                &chunk_retrieve_chan,
            ))
            .await;
            result.targets.append(&mut child_targets);
            if result.fallback_index.is_none() {
                result.fallback_index = Some(child_index);
            }
            return result;
        };

        let mut data_reference = fork.reference.clone();
        if let Some(ref_data) =
            get_root_manifest_data_if_manifest(&fork.reference, &chunk_retrieve_chan).await
        {
            if let Some(wrapped_reference) = manifest_wrapped_reference(&ref_data) {
                data_reference = wrapped_reference;
            }
        }

        result.targets.push(BzzTarget {
            data_reference,
            mime,
            path: normalize_bzz_path(&bequeath),
        });
        return result;
    }

    let bequeath = child_path(&path_prefix_heritance, &fork.prefix);
    let (mut child_targets, child_index) = Box::pin(collect_reference_targets(
        bequeath,
        fork.reference,
        &data_retrieve_chan,
        &chunk_retrieve_chan,
    ))
    .await;
    result.targets.append(&mut child_targets);
    result.fallback_index = Some(child_index);
    result
}

async fn collect_manifest_targets(
    path_prefix_heritance: String,
    cd0: Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> (Vec<BzzTarget>, String) {
    let Some(parsed) = parse_bzz_manifest(&cd0) else {
        return (vec![], String::new());
    };

    let mut targets = vec![];
    let mut index = parsed.explicit_index.unwrap_or_default();
    let mut fallback_index = None;

    let loads = parsed.forks.into_iter().map(|fork| {
        collect_manifest_fork_targets(
            path_prefix_heritance.clone(),
            parsed.ref_size,
            fork,
            data_retrieve_chan.clone(),
            chunk_retrieve_chan.clone(),
        )
    });

    for mut load in join_all(loads).await {
        if fallback_index.is_none() {
            fallback_index = load.fallback_index.take().filter(|index| !index.is_empty());
        }

        if let Some(explicit_index) = load.explicit_index.take() {
            index = explicit_index;
        }

        targets.append(&mut load.targets);
    }

    if index.is_empty() {
        if let Some(fallback_index) = fallback_index {
            index = fallback_index;
        }
    }

    (targets, index)
}

fn select_bzz_target(
    targets: Vec<BzzTarget>,
    requested_path: &str,
    index: &str,
) -> Option<BzzTarget> {
    if !requested_path.is_empty() {
        if let Some(target) = targets
            .iter()
            .find(|target| bzz_paths_match(&target.path, requested_path))
        {
            return Some(target.clone());
        }
    }

    if !index.is_empty() {
        if let Some(target) = targets
            .iter()
            .find(|target| bzz_paths_match(&target.path, index))
        {
            return Some(target.clone());
        }
    }

    if targets.len() == 1 {
        return targets.into_iter().next();
    }

    targets
        .iter()
        .find(|target| bzz_paths_match(&target.path, "index.html"))
        .cloned()
        .or_else(|| targets.into_iter().next())
}

pub async fn resolve_bzz(
    resource: &str,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<BzzMetadata> {
    let parsed = parse_bzz_resource(resource)?;
    let (targets, index) = collect_reference_targets(
        String::new(),
        parsed.reference,
        data_retrieve_chan,
        chunk_retrieve_chan,
    )
    .await;
    let target_count = targets.len();
    let target = select_bzz_target(targets, &parsed.path, &index)?;
    let root_chunk = get_range_tree_chunk(target.data_reference.clone(), chunk_retrieve_chan).await;
    let size = data_span(&root_chunk)?;

    Some(BzzMetadata {
        etag: format!("\"{}\"", hex::encode(&target.data_reference)),
        data_reference: target.data_reference,
        mime: target.mime,
        size,
        path: normalize_bzz_path(&target.path),
        target_count,
    })
}

pub async fn acquire_range(
    resource: &str,
    start: u64,
    end_inclusive: u64,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<(Vec<u8>, BzzMetadata)> {
    let metadata = resolve_bzz(resource, data_retrieve_chan, chunk_retrieve_chan).await?;

    acquire_resolved_range(metadata, start, end_inclusive, chunk_retrieve_chan).await
}

pub async fn acquire_range_cancellable(
    resource: &str,
    start: u64,
    end_inclusive: u64,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
    cancel_generations: RetrieveGenerationMap,
) -> Option<(Vec<u8>, BzzMetadata)> {
    let metadata = resolve_bzz(resource, data_retrieve_chan, chunk_retrieve_chan).await?;

    acquire_resolved_range_cancellable(
        metadata,
        start,
        end_inclusive,
        chunk_retrieve_chan,
        cancel,
        Some(cancel_generations),
    )
    .await
}

pub async fn acquire_resolved_range(
    metadata: BzzMetadata,
    start: u64,
    end_inclusive: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<(Vec<u8>, BzzMetadata)> {
    acquire_resolved_range_cancellable(
        metadata,
        start,
        end_inclusive,
        chunk_retrieve_chan,
        None,
        None,
    )
    .await
}

pub async fn acquire_resolved_range_cancellable(
    metadata: BzzMetadata,
    start: u64,
    end_inclusive: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
    cancel_generations: Option<RetrieveGenerationMap>,
) -> Option<(Vec<u8>, BzzMetadata)> {
    if start > end_inclusive || start >= metadata.size {
        return None;
    }

    let end_inclusive = end_inclusive.min(metadata.size.saturating_sub(1));
    let data = retrieve_data_range_cancellable(
        &metadata.data_reference,
        start + 8,
        end_inclusive + 8,
        chunk_retrieve_chan,
        cancel,
        cancel_generations,
    )
    .await;

    if data.len() != (end_inclusive - start + 1) as usize {
        return None;
    }

    Some((data, metadata))
}

pub async fn retrieve_data_range(
    data_address: &Vec<u8>,
    start: u64,
    end_inclusive: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Vec<u8> {
    retrieve_data_range_cancellable(
        data_address,
        start,
        end_inclusive,
        chunk_retrieve_chan,
        None,
        None,
    )
    .await
}

pub async fn retrieve_data_range_cancellable(
    data_address: &Vec<u8>,
    start: u64,
    end_inclusive: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
    cancel_generations: Option<RetrieveGenerationMap>,
) -> Vec<u8> {
    if start > end_inclusive {
        return vec![];
    }

    let root_chunk = get_range_tree_chunk_cancellable(
        data_address.to_vec(),
        chunk_retrieve_chan,
        cancel.clone(),
    )
    .await;
    let Some(root_span) = data_span(&root_chunk) else {
        web_sys::console::log_1(&JsValue::from(format!(
            "chunk not found: {}",
            hex::encode(data_address),
        )));
        return vec![];
    };

    let total_len = root_span + 8;
    if start >= total_len {
        return vec![];
    }

    let end_inclusive = end_inclusive.min(total_len - 1);
    let mut output = Vec::with_capacity((end_inclusive - start + 1) as usize);

    if start < 8 {
        let span_end = end_inclusive.min(7);
        output.extend_from_slice(&root_chunk[start as usize..=span_end as usize]);
        if end_inclusive < 8 {
            return output;
        }
    }

    let payload_start = start.max(8) - 8;
    let payload_end = end_inclusive - 8;
    output.extend_from_slice(
        &retrieve_payload_range_from_root(
            data_address,
            root_chunk,
            payload_start,
            payload_end,
            chunk_retrieve_chan,
            cancel,
            cancel_generations,
        )
        .await,
    );

    output
}

async fn retrieve_payload_range_from_root(
    data_address: &Vec<u8>,
    root_chunk: Vec<u8>,
    start: u64,
    end_inclusive: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
    cancel_generations: Option<RetrieveGenerationMap>,
) -> Vec<u8> {
    if start > end_inclusive {
        return vec![];
    }

    let address_length = data_address.len();
    if address_length != 32 && address_length != 64 {
        return vec![];
    }

    let mut leaves: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    let mut joiner = FuturesUnordered::new();
    let mut dispatched = 0usize;

    let Some(work) = range_chunk_work(
        address_length,
        0,
        data_address.clone(),
        root_chunk,
        start,
        end_inclusive,
    ) else {
        return vec![];
    };

    dispatch_range_work(
        work,
        chunk_retrieve_chan,
        &mut joiner,
        &mut leaves,
        &mut dispatched,
        cancel.clone(),
        cancel_generations.clone(),
    )
    .await;

    while let Some(fetch) = joiner.next().await {
        if let (Some(generations), Some(_)) = (&cancel_generations, &cancel) {
            if !retrieve_cancel_token_current(generations, &cancel).await {
                return vec![];
            }
        }

        let work = match fetch.payload {
            RangeFetchPayload::Chunk(chunk) => range_chunk_work(
                address_length,
                fetch.subtree_start,
                fetch.addr,
                chunk,
                start,
                end_inclusive,
            ),
            RangeFetchPayload::Data(data) => {
                data_fetch_work(fetch.subtree_start, fetch.addr, data, start, end_inclusive)
            }
        };

        let Some(work) = work else {
            return vec![];
        };

        dispatch_range_work(
            work,
            chunk_retrieve_chan,
            &mut joiner,
            &mut leaves,
            &mut dispatched,
            cancel.clone(),
            cancel_generations.clone(),
        )
        .await;
    }

    let mut output = Vec::with_capacity((end_inclusive - start + 1) as usize);
    for (_offset, mut leaf) in leaves {
        output.append(&mut leaf);
    }

    output
}

async fn dispatch_range_work(
    work: RangeChunkWork,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    joiner: &mut RangeJoiner,
    leaves: &mut BTreeMap<u64, Vec<u8>>,
    dispatched: &mut usize,
    cancel: Option<RetrieveCancelToken>,
    cancel_generations: Option<RetrieveGenerationMap>,
) {
    match work {
        RangeChunkWork::Skip => {}
        RangeChunkWork::Leaf(offset, bytes) => {
            leaves.insert(offset, bytes);
        }
        RangeChunkWork::Children(children) => {
            for child in children {
                if let (Some(generations), Some(_)) = (&cancel_generations, &cancel) {
                    if !retrieve_cancel_token_current(generations, &cancel).await {
                        break;
                    }
                }

                match child {
                    RangeChild::Chunk { start, addr } => {
                        queue_range_chunk(start, addr, chunk_retrieve_chan, joiner, cancel.clone());
                    }
                    RangeChild::Data { start, addr } => {
                        queue_range_data(
                            start,
                            addr,
                            chunk_retrieve_chan,
                            joiner,
                            cancel.clone(),
                            cancel_generations.clone(),
                        );
                    }
                }
                *dispatched += 1;
                if should_yield_after_range_dispatch(*dispatched) {
                    async_std::task::yield_now().await;
                }
            }
        }
    }
}

fn should_retrieve_child_with_data(child_span: u64) -> bool {
    child_span <= RANGE_RETRIEVE_DATA_PIECE_MAX_BYTES
}

fn data_fetch_work(
    subtree_start: u64,
    addr: Vec<u8>,
    data: Vec<u8>,
    start: u64,
    end_inclusive: u64,
) -> Option<RangeChunkWork> {
    let span = data_span(&data)?;
    if span == 0 {
        return Some(RangeChunkWork::Skip);
    }

    let span_usize = usize::try_from(span).ok()?;
    let payload_end = 8usize.checked_add(span_usize)?;

    if data.len() < payload_end {
        web_sys::console::log_1(&JsValue::from(format!(
            "retrieved subtree length ({}) too short for span ({}) for chunk {}",
            data.len(),
            span,
            hex::encode(addr),
        )));
        return None;
    }

    let subtree_end = subtree_start.checked_add(span)?.checked_sub(1)?;
    if subtree_end < start || subtree_start > end_inclusive {
        return Some(RangeChunkWork::Skip);
    }

    let overlap_start = start.max(subtree_start);
    let overlap_end = end_inclusive.min(subtree_end);
    let local_start = usize::try_from(overlap_start - subtree_start).ok()?;
    let local_end = usize::try_from(overlap_end - subtree_start).ok()?;

    Some(RangeChunkWork::Leaf(
        overlap_start,
        data[8 + local_start..=8 + local_end].to_vec(),
    ))
}

fn range_chunk_work(
    address_length: usize,
    subtree_start: u64,
    addr: Vec<u8>,
    chunk: Vec<u8>,
    start: u64,
    end_inclusive: u64,
) -> Option<RangeChunkWork> {
    let span = data_span(&chunk)?;

    if span == 0 {
        return Some(RangeChunkWork::Skip);
    }

    let subtree_end = subtree_start + span - 1;
    if subtree_end < start || subtree_start > end_inclusive {
        return Some(RangeChunkWork::Skip);
    }

    if span <= 4096 {
        let overlap_start = start.max(subtree_start);
        let overlap_end = end_inclusive.min(subtree_end);
        let local_start = (overlap_start - subtree_start) as usize;
        let local_end = (overlap_end - subtree_start) as usize;
        let payload = &chunk[8..];

        if payload.len() < local_end + 1 {
            return None;
        }

        return Some(RangeChunkWork::Leaf(
            overlap_start,
            payload[local_start..=local_end].to_vec(),
        ));
    }

    if chunk.len() < 8 + address_length || (chunk.len() - 8) % address_length != 0 {
        web_sys::console::log_1(&JsValue::from(format!(
            "chunk too short: {}",
            hex::encode(addr)
        )));
        return None;
    }

    let capacity = subtree_capacity_for_span(span, address_length);
    let branch_count = (4096 / address_length).max(1) as u64;
    let child_capacity = (capacity / branch_count).max(1);
    let child_refs = split_chunk_references(&chunk[8..], address_length)?;
    let mut children = vec![];

    for (child_index, child_addr) in child_refs.into_iter().enumerate() {
        let child_start = subtree_start + child_index as u64 * child_capacity;
        if child_start > subtree_end {
            break;
        }

        let child_span = (subtree_end - child_start + 1).min(child_capacity);
        let child_end = child_start + child_span - 1;

        if child_end < start || child_start > end_inclusive {
            continue;
        }

        if should_retrieve_child_with_data(child_span) {
            children.push(RangeChild::Data {
                start: child_start,
                addr: child_addr,
            });
        } else {
            children.push(RangeChild::Chunk {
                start: child_start,
                addr: child_addr,
            });
        }
    }

    Some(RangeChunkWork::Children(children))
}
