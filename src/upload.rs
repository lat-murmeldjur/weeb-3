use crate::{
    //                                                                        //
    ChunkRetrieveSender,
    //                                                                        //
    Date,
    //                                                                        //
    Duration,
    //                                                                        //
    HashMap,
    //                                                                        //
    HashSet,
    //                                                                        //
    JsValue,
    //                                                                        //
    Mutex,
    //                                                                        //
    OutboundProtocolSession,
    //                                                                        //
    PROTOCOL_ROUND_TIME,
    //                                                                        //
    PUSH_CHUNK_CONFIRMATION_PEERS,
    //                                                                        //
    PeerAccounting,
    //                                                                        //
    PeerId,
    //                                                                        //
    PhysicalConnectionMap,
    //                                                                        //
    RefreshmentInstruction,
    //                                                                        //
    apply_credit,
    //                                                                        //
    cancel_reserve,
    //                                                                        //
    connection_conventions::StreamControl,
    //                                                                        //
    content_address,
    //                                                                        //
    erasure_coding::{
        CHUNK_SIZE, CHUNK_WITH_SPAN_SIZE, ParityEncoder, RedundancyLevel, encode_level,
        encoded_reference_payload_len, reference_layout, replicas, upload_tree_plan,
    },
    //                                                                        //
    get_proximity,
    //                                                                        //
    manifest_upload::{Node, create_fork, create_manifest, create_stub},
    //                                                                        //
    mpsc,
    //                                                                        //
    price,
    //                                                                        //
    pushsync_handler,
    //                                                                        //
    reserve,
    //                                                                        //
    secure_vault::{
        secure_create_feed_update_soc_with_stamp, secure_ensure_feed_owner, secure_stamp_chunk,
    },
    //                                                                        //
    seek_next_feed_update_index,
    //                                                                        //
    transfer_pause_enabled,
    //                                                                        //
    upload_conventions::FileSlicePlan,
    //                                                                        //
};

use byteorder::ByteOrder;

use async_std::sync::Arc;

use alloy::primitives::keccak256;
use alloy::signers::Signer;
use alloy::signers::local::PrivateKeySigner;

use serde_json::json;

use libp2p::futures::{StreamExt, future::join_all, stream::FuturesUnordered};

use std::{collections::VecDeque, future::Future, pin::Pin, sync::atomic::AtomicBool};

const BATCH_BUCKET_TRIALS: usize = 1024;
const STAMP_CHUNK_WINDOW: usize = 64;
const PUSH_CHUNK_ATTEMPT_RETRY_WAIT_MS: u64 = 50;
const PUSH_CHUNK_ATTEMPT_SOFT_TIMEOUT_MS: u64 = 15000;
const PUSH_CHUNK_QUEUE_WINDOW: usize = 256;
const PUSH_CHUNK_RECEIPT_WINDOW: usize = PUSH_CHUNK_QUEUE_WINDOW * 2;
const PARITY_ENCODE_YIELD_ROWS: usize = 2;
const DEBUG_UPLOAD_LOGS: bool = false;

macro_rules! upload_log {
    ($($arg:tt)*) => {
        if DEBUG_UPLOAD_LOGS {
            web_sys::console::log_1(&JsValue::from(format!($($arg)*)));
        }
    };
}

pub(crate) type ChunkUploadRequest = (
    Vec<u8>,
    bool,
    Vec<u8>,
    Vec<u8>,
    mpsc::Sender<bool>,
    mpsc::Sender<bool>,
    Option<UploadProgressSender>,
);
pub(crate) type ChunkUploadSender = mpsc::Sender<ChunkUploadRequest>;
pub(crate) type DataUploadRequest = (
    DataUploadInput,
    u8,
    RedundancyLevel,
    Vec<u8>,
    Vec<u8>,
    Option<UploadProgressSender>,
    mpsc::Sender<DataUploadResult>,
);
type StampFuture = Pin<Box<dyn Future<Output = (usize, u64, Option<StampedChunk>)>>>;
type ChunkPushReceiptFuture = Pin<Box<dyn Future<Output = bool>>>;
type ChunkPushReceipts = FuturesUnordered<ChunkPushReceiptFuture>;

pub enum ResourceData {
    Parts(Vec<Vec<u8>>),
    BrowserFile(web_sys::File),
}

pub(crate) enum DataUploadInput {
    Parts {
        parts: VecDeque<Vec<u8>>,
        data_length: u64,
    },
    BrowserFile {
        file: web_sys::File,
        slices: FileSlicePlan,
        data_length: u64,
    },
}

impl DataUploadInput {
    fn from_parts(parts: Vec<Vec<u8>>) -> Option<Self> {
        let data_length = parts.iter().try_fold(0u64, |sum, part| {
            sum.checked_add(u64::try_from(part.len()).ok()?)
        })?;
        Some(Self::Parts {
            parts: parts.into(),
            data_length,
        })
    }

    fn from_browser_file(file: web_sys::File) -> Option<Self> {
        const JS_MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

        let size = file.size();
        if !size.is_finite() || size < 0.0 || size.fract() != 0.0 || size > JS_MAX_SAFE_INTEGER {
            return None;
        }
        let data_length = size as u64;
        Some(Self::BrowserFile {
            file,
            slices: FileSlicePlan::new(data_length),
            data_length,
        })
    }

    fn data_length(&self) -> u64 {
        match self {
            Self::Parts { data_length, .. } | Self::BrowserFile { data_length, .. } => *data_length,
        }
    }

    async fn next_part(&mut self) -> Result<Option<Vec<u8>>, String> {
        match self {
            Self::Parts { parts, .. } => Ok(parts.pop_front()),
            Self::BrowserFile { file, slices, .. } => {
                let Some((start, end)) = slices.next() else {
                    return Ok(None);
                };
                let blob = file
                    .slice_with_f64_and_f64(start as f64, end as f64)
                    .map_err(|_| format!("failed to slice upload file at {}-{}", start, end))?;
                let buffer = wasm_bindgen_futures::JsFuture::from(blob.array_buffer())
                    .await
                    .map_err(|_| format!("failed to read upload file at {}-{}", start, end))?;
                let bytes = js_sys::Uint8Array::new(&buffer);
                let expected = end - start;
                if u64::from(bytes.length()) != expected {
                    return Err(format!(
                        "upload file returned {} bytes for {} byte slice",
                        bytes.length(),
                        expected
                    ));
                }
                Ok(Some(bytes.to_vec()))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct UploadProgressDelta {
    pub chunks_total_delta: u64,
    pub chunks_done_delta: u64,
}

pub(crate) type UploadProgressSender = mpsc::Sender<UploadProgressDelta>;

struct PushAttemptResult {
    success: bool,
}

struct ReservedPushPeer {
    peer: PeerId,
    price: u64,
    accounting: Arc<Mutex<PeerAccounting>>,
    session: OutboundProtocolSession,
}

fn record_push_attempt_result(
    result: PushAttemptResult,
    in_flight: &mut usize,
    success_count: &mut usize,
    error_count: &mut usize,
) {
    *in_flight = in_flight.saturating_sub(1);
    if result.success {
        *success_count += 1;
    } else {
        *error_count += 1;
    }
}

fn drain_push_attempt_results(
    attempt_in: &mpsc::Receiver<PushAttemptResult>,
    in_flight: &mut usize,
    success_count: &mut usize,
    error_count: &mut usize,
) {
    while let Ok(result) = attempt_in.try_recv() {
        record_push_attempt_result(result, in_flight, success_count, error_count);
    }
}

struct StampedChunk {
    reference: Vec<u8>,
    data: Vec<u8>,
    canonical_data: Vec<u8>,
    raw_data: Vec<u8>,
    soc: bool,
    address: Vec<u8>,
    stamp: Vec<u8>,
}

struct TreeChunk {
    span: u64,
    reference: Vec<u8>,
    canonical_data: Vec<u8>,
    raw_data: Vec<u8>,
}

fn reset_push_overdraft(skiplist: &mut HashSet<PeerId>, overdraftlist: &mut HashSet<PeerId>) {
    for peer in overdraftlist.drain() {
        skiplist.remove(&peer);
    }
}

fn track_chunk_push_receipt(receipts: &mut ChunkPushReceipts, receipt: mpsc::Receiver<bool>) {
    receipts.push(Box::pin(
        async move { matches!(receipt.recv().await, Ok(true)) },
    ));
}

pub(crate) fn report_upload_progress(
    progress: &Option<UploadProgressSender>,
    chunks_total_delta: u64,
    chunks_done_delta: u64,
) {
    if chunks_total_delta == 0 && chunks_done_delta == 0 {
        return;
    }

    if let Some(progress) = progress {
        let _ = progress.try_send(UploadProgressDelta {
            chunks_total_delta,
            chunks_done_delta,
        });
    }
}

fn count_push_data_chunks(
    data_length: u64,
    encryption: bool,
    redundancy_level: RedundancyLevel,
) -> Option<u64> {
    upload_tree_plan(data_length, redundancy_level, encryption).map(|plan| plan.total_chunks)
}

async fn wait_for_chunk_pushes(receipts: &mut ChunkPushReceipts) -> bool {
    while let Some(success) = receipts.next().await {
        if !success {
            return false;
        }
    }

    true
}

async fn wait_for_next_chunk_push(receipts: &mut ChunkPushReceipts) -> bool {
    match receipts.next().await {
        Some(success) => success,
        None => false,
    }
}

async fn enqueue_stamped_chunk(
    stamped_chunk: StampedChunk,
    span: u64,
    chunk_receipts: &mut ChunkPushReceipts,
    chunk_slot_receipts: &mut ChunkPushReceipts,
    chunk_upload_chan: &ChunkUploadSender,
    progress: &Option<UploadProgressSender>,
) -> Option<TreeChunk> {
    let (result_chan_out, result_chan_in) = mpsc::unbounded::<bool>();
    let (slot_chan_out, slot_chan_in) = mpsc::unbounded::<bool>();

    let StampedChunk {
        reference,
        data,
        canonical_data,
        raw_data,
        soc,
        address,
        stamp,
    } = stamped_chunk;

    chunk_upload_chan
        .try_send((
            data,
            soc,
            address,
            stamp,
            result_chan_out,
            slot_chan_out,
            progress.clone(),
        ))
        .ok()?;

    track_chunk_push_receipt(chunk_receipts, result_chan_in);
    track_chunk_push_receipt(chunk_slot_receipts, slot_chan_in);

    if chunk_slot_receipts.len() >= PUSH_CHUNK_QUEUE_WINDOW {
        if !wait_for_next_chunk_push(chunk_slot_receipts).await {
            return None;
        }
        async_std::task::yield_now().await;
    }

    // Push slot feedback only bounds requests through the initial push. Final
    // retrieve-check/retry receipts may lag behind it, so keep a second bounded
    // window and drain one completed result before accepting more tree chunks.
    // Awaiting the receiver never cancels the separately spawned push/accounting task.
    if chunk_receipts.len() >= PUSH_CHUNK_RECEIPT_WINDOW
        && !wait_for_next_chunk_push(chunk_receipts).await
    {
        return None;
    }

    Some(TreeChunk {
        span,
        reference,
        canonical_data,
        raw_data,
    })
}

async fn flush_stamp_window(
    stamp_joiner: &mut Vec<StampFuture>,
    chunk_receipts: &mut ChunkPushReceipts,
    chunk_slot_receipts: &mut ChunkPushReceipts,
    chunk_upload_chan: &ChunkUploadSender,
    progress: &Option<UploadProgressSender>,
) -> Option<Vec<TreeChunk>> {
    if stamp_joiner.is_empty() {
        return Some(Vec::new());
    }

    let mut stamped = join_all(std::mem::take(stamp_joiner)).await;
    stamped.sort_by_key(|(chunk_index, _, _)| *chunk_index);
    let mut chunks = Vec::with_capacity(stamped.len());

    for (_, span, stamped_chunk) in stamped {
        let Some(stamped_chunk) = stamped_chunk else {
            return None;
        };

        chunks.push(
            enqueue_stamped_chunk(
                stamped_chunk,
                span,
                chunk_receipts,
                chunk_slot_receipts,
                chunk_upload_chan,
                progress,
            )
            .await?,
        );
    }

    Some(chunks)
}

async fn pushsync_attempt(
    selected: ReservedPushPeer,
    caddr: Vec<u8>,
    data: Vec<u8>,
    cstamp0: Vec<u8>,
    control: StreamControl,
    refresh_chan: mpsc::Sender<RefreshmentInstruction>,
    result_chan: mpsc::Sender<PushAttemptResult>,
) {
    let ReservedPushPeer {
        peer,
        price: req_price,
        accounting: accounting_peer,
        session,
    } = selected;
    let (chunk_out, chunk_in) = mpsc::unbounded::<bool>();

    // This task owns the protocol exchange and accounting even if push_chunk
    // starts trying another peer while this one is still waiting for a receipt.
    pushsync_handler(
        peer.clone(),
        &caddr,
        &data,
        &cstamp0,
        control,
        session,
        &chunk_out,
    )
    .await;

    let success = matches!(chunk_in.try_recv(), Ok(true));
    if success {
        apply_credit(&accounting_peer, req_price, &refresh_chan).await;
    } else {
        cancel_reserve(&accounting_peer, req_price).await;
    }

    let _ = result_chan.try_send(PushAttemptResult { success });
}

pub async fn stamp_chunk(
    _stamp_signer_key: Vec<u8>,
    _batch_id: Vec<u8>,
    _batch_bucket_limit: u32,
    chunk_address: Vec<u8>,
) -> (Vec<u8>, bool) {
    secure_stamp_chunk(chunk_address).await
}

pub struct Resource {
    pub path0: String,
    pub filename0: String,
    pub mime0: String,
    pub data: ResourceData,
    pub data_address: Vec<u8>,
}

pub async fn upload_resource(
    resource0: Vec<Resource>,
    encryption: bool,
    redundancy_level: RedundancyLevel,
    mut index: String,
    errordoc: String,
    feed: bool,
    topic: String,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    data_upload_chan: &mpsc::Sender<DataUploadRequest>,
    chunk_upload_chan: &ChunkUploadSender,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    progress: Option<UploadProgressSender>,
) -> Vec<u8> {
    //
    let mut node0: Vec<Node> = vec![];

    for mut r0 in resource0 {
        upload_log!("Attempt uploading resource");

        // upload core file
        let input = match r0.data {
            ResourceData::Parts(parts) => DataUploadInput::from_parts(parts),
            ResourceData::BrowserFile(file) => DataUploadInput::from_browser_file(file),
        };
        let Some(input) = input else {
            render_log_message(&format!("Upload input was invalid for {}", r0.path0));
            return vec![];
        };
        let core_reference = upload_input_with_root(
            input,
            encryption,
            redundancy_level,
            batch_owner.clone(),
            batch_id.clone(),
            &data_upload_chan,
            progress.clone(),
        )
        .await
        .reference;

        if core_reference.is_empty() {
            render_log_message(&format!(
                "Upload failed for {}; refusing to create manifest with empty data reference",
                r0.path0
            ));
            return vec![];
        }

        if r0.path0.len() == 0 {
            r0.path0 = hex::encode(&core_reference);
        }

        if index.len() == 0 {
            index = r0.path0.clone();
        };

        upload_log!(
            "Upload resource returning {:#?}!",
            hex::encode(&core_reference)
        );

        r0.data_address = core_reference;

        node0.push(Node {
            data: r0.data_address.clone(), // pub data: Vec<u8>, // repurposed as address
            mime: r0.mime0.clone(),        // pub mime: String,
            filename: r0.filename0.clone(), // pub filename: String,
            path: r0.path0.clone(),        // pub path: String,
        })
    }

    let core_manifest = create_manifest(
        encryption,
        encryption,
        redundancy_level,
        node0,  // forks
        vec![], // data_forks
        vec![], // reference
        true,   // root manifest
        0,
        index,    // index
        errordoc, // errordoc
        batch_owner.clone(),
        batch_id.clone(),
        &data_upload_chan,
        progress.clone(),
    )
    .await;

    if core_manifest.is_empty() {
        render_log_message(&"Manifest creation failed".to_string());
        return vec![];
    }

    let manifest_upload = upload_data_with_root(
        vec![core_manifest],
        encryption,
        redundancy_level,
        batch_owner.clone(),
        batch_id.clone(),
        &data_upload_chan,
        progress.clone(),
    )
    .await;
    let manifest_reference = manifest_upload.reference.clone();

    if manifest_reference.is_empty() {
        render_log_message(&"Manifest upload failed".to_string());
        return vec![];
    }

    if !feed {
        return manifest_reference;
    }

    let feed_owner = match secure_ensure_feed_owner().await {
        Some(feed_owner) => feed_owner,
        None => return vec![],
    };

    let feed_metadata = serde_json::to_vec(&json!({
        "swarm-feed-owner": hex::encode(&feed_owner),
        "swarm-feed-topic": topic,
        "swarm-feed-type": "Sequence".to_string(),

    }))
    .unwrap();

    let mut stub_ref_size = 32;

    if encryption {
        stub_ref_size = 64;
    }

    let stub_reference = upload_data(
        vec![create_stub(stub_ref_size, encryption).await],
        encryption,
        redundancy_level,
        batch_owner.clone(),
        batch_id.clone(),
        &data_upload_chan,
        progress.clone(),
    )
    .await;
    if stub_reference.is_empty() {
        return vec![];
    }

    let root_fork = create_fork("/".to_string(), stub_reference, feed_metadata).await;
    if root_fork.is_empty() {
        return vec![];
    }

    let feed_manifest = create_manifest(
        encryption,
        encryption,
        redundancy_level,
        vec![],          // forks
        vec![root_fork], // data_forks
        vec![],          // reference
        false,           // root manifest
        0,
        "".to_string(), // index
        "".to_string(), // errordoc
        batch_owner.clone(),
        batch_id.clone(),
        &data_upload_chan,
        progress.clone(),
    )
    .await;

    if feed_manifest.is_empty() {
        return vec![];
    }

    let feed_reference = upload_data(
        vec![feed_manifest],
        encryption,
        redundancy_level,
        batch_owner.clone(),
        batch_id.clone(),
        &data_upload_chan,
        progress.clone(),
    )
    .await;

    let index_up = seek_next_feed_update_index(
        hex::encode(&feed_owner),
        topic.clone(),
        &chunk_retrieve_chan,
        8,
    )
    .await;

    // A feed update wraps the exact logical root chunk. With erasure coding its
    // span carries the level marker and its payload contains mixed-width data
    // and parity references, so rebuilding it from the source length is invalid.
    let soc_wrapped_content = manifest_upload.root_data;

    let feed_update = match secure_create_feed_update_soc_with_stamp(
        topic.clone(),
        index_up,
        soc_wrapped_content,
    )
    .await
    {
        Some(feed_update) => feed_update,
        None => return vec![],
    };

    if feed_update.bucket_full {
        return vec![];
    }

    let (result_chan_out, result_chan_in) = mpsc::unbounded::<bool>();
    let (slot_chan_out, _slot_chan_in) = mpsc::unbounded::<bool>();

    if feed_update.stamp.len() == 0 {
        return vec![];
    }

    report_upload_progress(&progress, 1, 0);

    if chunk_upload_chan
        .try_send((
            feed_update.soc_chunk,
            true,
            feed_update.soc_address,
            feed_update.stamp,
            result_chan_out,
            slot_chan_out,
            progress.clone(),
        ))
        .is_err()
    {
        return vec![];
    }

    if result_chan_in.recv().await != Ok(true) {
        return vec![];
    }

    return feed_reference;
}

pub async fn upload_data(
    data: Vec<Vec<u8>>,
    enc: bool,
    redundancy_level: RedundancyLevel,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    data_upload_chan: &mpsc::Sender<DataUploadRequest>,
    progress: Option<UploadProgressSender>,
) -> Vec<u8> {
    upload_data_with_root(
        data,
        enc,
        redundancy_level,
        batch_owner,
        batch_id,
        data_upload_chan,
        progress,
    )
    .await
    .reference
}

async fn upload_data_with_root(
    data: Vec<Vec<u8>>,
    enc: bool,
    redundancy_level: RedundancyLevel,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    data_upload_chan: &mpsc::Sender<DataUploadRequest>,
    progress: Option<UploadProgressSender>,
) -> DataUploadResult {
    let Some(input) = DataUploadInput::from_parts(data) else {
        return DataUploadResult::failed();
    };
    upload_input_with_root(
        input,
        enc,
        redundancy_level,
        batch_owner,
        batch_id,
        data_upload_chan,
        progress,
    )
    .await
}

async fn upload_input_with_root(
    input: DataUploadInput,
    enc: bool,
    redundancy_level: RedundancyLevel,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    data_upload_chan: &mpsc::Sender<DataUploadRequest>,
    progress: Option<UploadProgressSender>,
) -> DataUploadResult {
    let (chan_out, chan_in) = mpsc::unbounded::<DataUploadResult>();
    let mut enc_mode = 0;
    if enc {
        enc_mode = 1;
    }

    if data_upload_chan
        .try_send((
            input,
            enc_mode,
            redundancy_level,
            batch_owner,
            batch_id,
            progress,
            chan_out,
        ))
        .is_err()
    {
        return DataUploadResult::failed();
    }

    match chan_in.recv().await {
        Ok(result) => {
            upload_log!(
                "Upload data returning {:#?}!",
                hex::encode(&result.reference)
            );

            result
        }
        Err(_) => DataUploadResult::failed(),
    }
}

fn canonical_chunk(span: &[u8; 8], payload: &[u8]) -> Vec<u8> {
    [span.as_slice(), payload].concat()
}

async fn stamp_push_chunk(
    payload: Vec<u8>,
    span: [u8; 8],
    encryption: bool,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    batch_bucket_limit: u32,
) -> Option<StampedChunk> {
    if payload.len() > CHUNK_SIZE {
        return None;
    }

    let canonical_data = canonical_chunk(&span, &payload);
    let soc = false;
    let mut key = Vec::new();
    let mut raw_data = if encryption {
        key = encrey();
        encrypt_with_span(&span, &payload, &key)
    } else {
        canonical_data.clone()
    };
    let mut data = raw_data.clone();
    let mut address = content_address(&raw_data);
    if address.len() != 32 {
        return None;
    }

    let mut stamp = None;
    for _ in 0..BATCH_BUCKET_TRIALS {
        let (candidate, bucket_full) = stamp_chunk(
            batch_owner.clone(),
            batch_id.clone(),
            batch_bucket_limit,
            address.clone(),
        )
        .await;
        if !bucket_full {
            stamp = (!candidate.is_empty()).then_some(candidate);
            break;
        }

        render_log_message(&"Restamping chunk to avoid bucket overflow".to_string());
        if encryption {
            key = encrey();
            raw_data = encrypt_with_span(&span, &payload, &key);
            data = raw_data.clone();
            address = content_address(&raw_data);
        } else {
            // Bee file trees contain CAC references. The legacy arbitrary-SOC
            // bucket-overflow fallback creates a reference Bee's joiner cannot
            // interpret, so fail this file upload cleanly instead.
            return None;
        }
    }

    Some(StampedChunk {
        reference: [address.clone(), key].concat(),
        data,
        canonical_data,
        raw_data,
        soc,
        address,
        stamp: stamp?,
    })
}

// Parity bytes are already a complete CAC (including its RS-generated span).
// They must remain CACs: wrapping one in a SOC would make its parent reference
// incompatible with Bee's erasure decoder.
async fn stamp_parity_chunk(
    raw_data: Vec<u8>,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    batch_bucket_limit: u32,
) -> Option<StampedChunk> {
    if raw_data.len() != CHUNK_WITH_SPAN_SIZE {
        return None;
    }
    let address = content_address(&raw_data);
    if address.len() != 32 {
        return None;
    }

    let (stamp, bucket_full) =
        stamp_chunk(batch_owner, batch_id, batch_bucket_limit, address.clone()).await;
    if bucket_full || stamp.is_empty() {
        return None;
    }

    Some(StampedChunk {
        reference: address.clone(),
        data: raw_data.clone(),
        canonical_data: raw_data.clone(),
        raw_data,
        soc: false,
        address,
        stamp,
    })
}

async fn build_parent_chunk(
    children: Vec<TreeChunk>,
    encryption: bool,
    redundancy_level: RedundancyLevel,
    batch_owner: &Vec<u8>,
    batch_id: &Vec<u8>,
    batch_bucket_limit: u32,
    chunk_upload_chan: &ChunkUploadSender,
    progress: &Option<UploadProgressSender>,
    chunk_receipts: &mut ChunkPushReceipts,
    chunk_slot_receipts: &mut ChunkPushReceipts,
) -> Option<TreeChunk> {
    if children.len() < 2 {
        return None;
    }

    let span = children
        .iter()
        .try_fold(0u64, |sum, child| sum.checked_add(child.span))?;
    let layout = reference_layout(span, redundancy_level, encryption)?;
    let parity_count = redundancy_level.parities(children.len(), encryption);
    if layout.data_shards != children.len() || layout.parity_shards != parity_count {
        return None;
    }
    let mut parity_references = Vec::with_capacity(parity_count);
    if parity_count > 0 {
        let data_shards = children
            .iter()
            .map(|child| child.raw_data.as_slice())
            .collect::<Vec<_>>();
        let parity_encoder =
            ParityEncoder::new_padded(&data_shards, parity_count, CHUNK_WITH_SPAN_SIZE).ok()?;
        let mut stamp_joiner: Vec<StampFuture> = Vec::new();

        for index in 0..parity_encoder.parity_count() {
            let parity = parity_encoder.encode_shard(index).ok()?;
            let owner = batch_owner.clone();
            let batch = batch_id.clone();
            stamp_joiner.push(Box::pin(async move {
                (
                    index,
                    0,
                    stamp_parity_chunk(parity, owner, batch, batch_bucket_limit).await,
                )
            }));

            if stamp_joiner.len() >= STAMP_CHUNK_WINDOW {
                let parity_chunks = flush_stamp_window(
                    &mut stamp_joiner,
                    chunk_receipts,
                    chunk_slot_receipts,
                    chunk_upload_chan,
                    progress,
                )
                .await?;
                parity_references.extend(parity_chunks.into_iter().map(|chunk| chunk.reference));
            }

            if (index + 1) % PARITY_ENCODE_YIELD_ROWS == 0 {
                async_std::task::yield_now().await;
            }
        }

        let parity_chunks = flush_stamp_window(
            &mut stamp_joiner,
            chunk_receipts,
            chunk_slot_receipts,
            chunk_upload_chan,
            progress,
        )
        .await?;
        parity_references.extend(parity_chunks.into_iter().map(|chunk| chunk.reference));
    }

    let mut span_bytes = span.to_le_bytes();
    if parity_count > 0 {
        encode_level(&mut span_bytes, redundancy_level);
    }

    let reference_bytes = if encryption { 64 } else { 32 };
    let mut payload =
        Vec::with_capacity(children.len() * reference_bytes + parity_references.len() * 32);
    for child in &children {
        if child.reference.len() != reference_bytes {
            return None;
        }
        payload.extend_from_slice(&child.reference);
    }
    for reference in &parity_references {
        if reference.len() != 32 {
            return None;
        }
        payload.extend_from_slice(reference);
    }
    if encoded_reference_payload_len(span, redundancy_level, encryption) != Some(payload.len()) {
        return None;
    }

    let stamped = stamp_push_chunk(
        payload,
        span_bytes,
        encryption,
        batch_owner.clone(),
        batch_id.clone(),
        batch_bucket_limit,
    )
    .await?;
    enqueue_stamped_chunk(
        stamped,
        span,
        chunk_receipts,
        chunk_slot_receipts,
        chunk_upload_chan,
        progress,
    )
    .await
}

async fn insert_tree_chunk(
    mut chunk: TreeChunk,
    mut level: usize,
    buffers: &mut Vec<Vec<TreeChunk>>,
    encryption: bool,
    redundancy_level: RedundancyLevel,
    batch_owner: &Vec<u8>,
    batch_id: &Vec<u8>,
    batch_bucket_limit: u32,
    chunk_upload_chan: &ChunkUploadSender,
    progress: &Option<UploadProgressSender>,
    chunk_receipts: &mut ChunkPushReceipts,
    chunk_slot_receipts: &mut ChunkPushReceipts,
) -> bool {
    let max_shards = redundancy_level.max_shards(encryption);
    loop {
        if buffers.len() <= level {
            buffers.resize_with(level + 1, Vec::new);
        }
        buffers[level].push(chunk);
        if buffers[level].len() < max_shards {
            return true;
        }

        let children = std::mem::take(&mut buffers[level]);
        let Some(parent) = build_parent_chunk(
            children,
            encryption,
            redundancy_level,
            batch_owner,
            batch_id,
            batch_bucket_limit,
            chunk_upload_chan,
            progress,
            chunk_receipts,
            chunk_slot_receipts,
        )
        .await
        else {
            return false;
        };
        chunk = parent;
        level += 1;
    }
}

fn buffered_chunk_count(buffers: &[Vec<TreeChunk>]) -> usize {
    buffers.iter().map(Vec::len).sum()
}

async fn stamp_root_replicas(
    root: &TreeChunk,
    redundancy_level: RedundancyLevel,
    batch_owner: &Vec<u8>,
    batch_id: &Vec<u8>,
    batch_bucket_limit: u32,
    chunk_upload_chan: &ChunkUploadSender,
    progress: &Option<UploadProgressSender>,
    chunk_receipts: &mut ChunkPushReceipts,
    chunk_slot_receipts: &mut ChunkPushReceipts,
) -> bool {
    if redundancy_level == RedundancyLevel::None || root.reference.len() < 32 {
        return true;
    }

    let replica_owner = hex::decode("dc5b20847f43d67928f49cd4f85d696b5a7617b5").unwrap();
    let Some(replica_plan) = replicas(&root.reference[..32], redundancy_level, |id| {
        keccak256([id.as_slice(), replica_owner.as_slice()].concat()).into()
    }) else {
        return false;
    };
    report_upload_progress(progress, replica_plan.len() as u64, 0);

    let mut replica_key = vec![0u8; 32];
    replica_key[0] = 1;
    let mut stamp_joiner: Vec<StampFuture> = Vec::new();
    for (index, replica) in replica_plan.into_iter().enumerate() {
        let root_data = root.raw_data.clone();
        let key = replica_key.clone();
        let owner = batch_owner.clone();
        let batch = batch_id.clone();
        stamp_joiner.push(Box::pin(async move {
            let (data, address) = make_soc(&root_data, key, replica.id.to_vec()).await;
            if data.is_empty() || address.as_slice() != replica.address {
                return (index, 0, None);
            }
            let (stamp, bucket_full) =
                stamp_chunk(owner, batch, batch_bucket_limit, address.clone()).await;
            let accepted_stamp = (!bucket_full && !stamp.is_empty()).then_some(stamp);
            let stamped = accepted_stamp.map(|stamp| StampedChunk {
                reference: address.clone(),
                data,
                canonical_data: root_data.clone(),
                raw_data: root_data,
                soc: true,
                address,
                stamp,
            });
            (index, 0, stamped)
        }));

        if stamp_joiner.len() >= STAMP_CHUNK_WINDOW
            && flush_stamp_window(
                &mut stamp_joiner,
                chunk_receipts,
                chunk_slot_receipts,
                chunk_upload_chan,
                progress,
            )
            .await
            .is_none()
        {
            return false;
        }
    }

    flush_stamp_window(
        &mut stamp_joiner,
        chunk_receipts,
        chunk_slot_receipts,
        chunk_upload_chan,
        progress,
    )
    .await
    .is_some()
}

pub(crate) struct DataUploadResult {
    pub reference: Vec<u8>,
    pub root_data: Vec<u8>,
}

impl DataUploadResult {
    fn failed() -> Self {
        Self {
            reference: Vec::new(),
            root_data: Vec::new(),
        }
    }
}

pub(crate) async fn push_data_input_with_root(
    mut input: DataUploadInput,
    encryption: bool,
    redundancy_level: RedundancyLevel,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    batch_bucket_limit: u32,
    chunk_upload_chan: &ChunkUploadSender,
    progress: Option<UploadProgressSender>,
) -> DataUploadResult {
    let data_length = input.data_length();
    let Some(tree_chunk_count) = count_push_data_chunks(data_length, encryption, redundancy_level)
    else {
        return DataUploadResult::failed();
    };
    report_upload_progress(&progress, tree_chunk_count, 0);

    let mut chunk_receipts: ChunkPushReceipts = FuturesUnordered::new();
    let mut chunk_slot_receipts: ChunkPushReceipts = FuturesUnordered::new();
    let mut buffers: Vec<Vec<TreeChunk>> = Vec::new();
    let mut stamp_joiner: Vec<StampFuture> = Vec::new();
    let mut leaf = Vec::with_capacity(CHUNK_SIZE);
    let mut leaf_index = 0usize;
    let mut consumed = 0u64;

    loop {
        let part = match input.next_part().await {
            Ok(Some(part)) => part,
            Ok(None) => break,
            Err(error) => {
                render_log_message(&error);
                return DataUploadResult::failed();
            }
        };
        let Some(next_consumed) = consumed.checked_add(part.len() as u64) else {
            return DataUploadResult::failed();
        };
        if next_consumed > data_length {
            return DataUploadResult::failed();
        }
        consumed = next_consumed;

        let mut offset = 0usize;
        while offset < part.len() {
            let take = (CHUNK_SIZE - leaf.len()).min(part.len() - offset);
            leaf.extend_from_slice(&part[offset..offset + take]);
            offset += take;
            if leaf.len() != CHUNK_SIZE {
                continue;
            }

            let payload = std::mem::replace(&mut leaf, Vec::with_capacity(CHUNK_SIZE));
            let owner = batch_owner.clone();
            let batch = batch_id.clone();
            let index = leaf_index;
            leaf_index += 1;
            stamp_joiner.push(Box::pin(async move {
                (
                    index,
                    CHUNK_SIZE as u64,
                    stamp_push_chunk(
                        payload,
                        (CHUNK_SIZE as u64).to_le_bytes(),
                        encryption,
                        owner,
                        batch,
                        batch_bucket_limit,
                    )
                    .await,
                )
            }));

            if stamp_joiner.len() >= STAMP_CHUNK_WINDOW {
                let Some(chunks) = flush_stamp_window(
                    &mut stamp_joiner,
                    &mut chunk_receipts,
                    &mut chunk_slot_receipts,
                    chunk_upload_chan,
                    &progress,
                )
                .await
                else {
                    return DataUploadResult::failed();
                };
                for chunk in chunks {
                    if !insert_tree_chunk(
                        chunk,
                        0,
                        &mut buffers,
                        encryption,
                        redundancy_level,
                        &batch_owner,
                        &batch_id,
                        batch_bucket_limit,
                        chunk_upload_chan,
                        &progress,
                        &mut chunk_receipts,
                        &mut chunk_slot_receipts,
                    )
                    .await
                    {
                        return DataUploadResult::failed();
                    }
                }
            }
        }
    }

    if consumed != data_length {
        return DataUploadResult::failed();
    }

    if !leaf.is_empty() || data_length == 0 {
        let span = leaf.len() as u64;
        let owner = batch_owner.clone();
        let batch = batch_id.clone();
        let index = leaf_index;
        stamp_joiner.push(Box::pin(async move {
            (
                index,
                span,
                stamp_push_chunk(
                    leaf,
                    span.to_le_bytes(),
                    encryption,
                    owner,
                    batch,
                    batch_bucket_limit,
                )
                .await,
            )
        }));
    }

    let Some(chunks) = flush_stamp_window(
        &mut stamp_joiner,
        &mut chunk_receipts,
        &mut chunk_slot_receipts,
        chunk_upload_chan,
        &progress,
    )
    .await
    else {
        return DataUploadResult::failed();
    };
    for chunk in chunks {
        if !insert_tree_chunk(
            chunk,
            0,
            &mut buffers,
            encryption,
            redundancy_level,
            &batch_owner,
            &batch_id,
            batch_bucket_limit,
            chunk_upload_chan,
            &progress,
            &mut chunk_receipts,
            &mut chunk_slot_receipts,
        )
        .await
        {
            return DataUploadResult::failed();
        }
    }

    while buffered_chunk_count(&buffers) > 1 {
        let Some(level) = buffers.iter().position(|buffer| !buffer.is_empty()) else {
            return DataUploadResult::failed();
        };
        let mut children = std::mem::take(&mut buffers[level]);
        let chunk = if children.len() == 1 {
            children.pop().unwrap()
        } else {
            let Some(parent) = build_parent_chunk(
                children,
                encryption,
                redundancy_level,
                &batch_owner,
                &batch_id,
                batch_bucket_limit,
                chunk_upload_chan,
                &progress,
                &mut chunk_receipts,
                &mut chunk_slot_receipts,
            )
            .await
            else {
                return DataUploadResult::failed();
            };
            parent
        };

        if !insert_tree_chunk(
            chunk,
            level + 1,
            &mut buffers,
            encryption,
            redundancy_level,
            &batch_owner,
            &batch_id,
            batch_bucket_limit,
            chunk_upload_chan,
            &progress,
            &mut chunk_receipts,
            &mut chunk_slot_receipts,
        )
        .await
        {
            return DataUploadResult::failed();
        }
    }

    let Some(root) = buffers.iter_mut().find_map(Vec::pop) else {
        return DataUploadResult::failed();
    };
    if !stamp_root_replicas(
        &root,
        redundancy_level,
        &batch_owner,
        &batch_id,
        batch_bucket_limit,
        chunk_upload_chan,
        &progress,
        &mut chunk_receipts,
        &mut chunk_slot_receipts,
    )
    .await
        || !wait_for_chunk_pushes(&mut chunk_receipts).await
    {
        return DataUploadResult::failed();
    }

    DataUploadResult {
        reference: root.reference,
        root_data: root.canonical_data,
    }
}

pub async fn push_chunk(
    data: Vec<u8>,
    soc: bool,
    soc_address: Vec<u8>,
    cstamp0: Vec<u8>,
    control: StreamControl,
    peers: &Arc<Mutex<HashMap<Vec<u8>, PeerId>>>,
    accounting: &Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    physical_connections: &PhysicalConnectionMap,
    refresh_chan: &mpsc::Sender<RefreshmentInstruction>,
    transfer_paused: Option<Arc<AtomicBool>>,
) -> Vec<u8> {
    if (data.len() > 4104 && !soc) || (data.len() > 4201) {
        return vec![];
    }

    let caddr = match soc {
        true => soc_address.clone(),
        false => content_address(&data),
    };

    let mut skiplist: HashSet<PeerId> = HashSet::new();
    let mut overdraftlist: HashSet<PeerId> = HashSet::new();
    let mut success_count = 0usize;
    let mut round_commence = Date::now();
    let mut error_count = 0;
    let max_error = 21 - PUSH_CHUNK_CONFIRMATION_PEERS;
    let mut in_flight = 0usize;
    let mut last_attempt_started = 0.0;
    let (attempt_out, attempt_in) = mpsc::unbounded::<PushAttemptResult>();

    while error_count < max_error && success_count < PUSH_CHUNK_CONFIRMATION_PEERS {
        drain_push_attempt_results(
            &attempt_in,
            &mut in_flight,
            &mut success_count,
            &mut error_count,
        );

        if error_count >= max_error || success_count >= PUSH_CHUNK_CONFIRMATION_PEERS {
            break;
        }

        while transfer_paused
            .as_ref()
            .map(transfer_pause_enabled)
            .unwrap_or(false)
        {
            async_std::task::sleep(Duration::from_millis(100)).await;
            drain_push_attempt_results(
                &attempt_in,
                &mut in_flight,
                &mut success_count,
                &mut error_count,
            );
        }

        if error_count >= max_error || success_count >= PUSH_CHUNK_CONFIRMATION_PEERS {
            break;
        }

        let now = Date::now();
        let due = in_flight == 0
            || now - last_attempt_started >= PUSH_CHUNK_ATTEMPT_SOFT_TIMEOUT_MS as f64;

        if !due {
            let wait_ms = (PUSH_CHUNK_ATTEMPT_SOFT_TIMEOUT_MS as f64 - (now - last_attempt_started))
                .max(PUSH_CHUNK_ATTEMPT_RETRY_WAIT_MS as f64)
                .round() as u64;

            match async_std::future::timeout(Duration::from_millis(wait_ms), attempt_in.recv())
                .await
            {
                Ok(Ok(result)) => {
                    record_push_attempt_result(
                        result,
                        &mut in_flight,
                        &mut success_count,
                        &mut error_count,
                    );
                }
                Ok(Err(_)) => break,
                Err(_) => {}
            }

            continue;
        }

        let mut selected_peer: Option<ReservedPushPeer> = None;

        while selected_peer.is_none() {
            drain_push_attempt_results(
                &attempt_in,
                &mut in_flight,
                &mut success_count,
                &mut error_count,
            );

            if error_count >= max_error || success_count >= PUSH_CHUNK_CONFIRMATION_PEERS {
                break;
            }

            let mut closest_overlay = vec![];
            let mut closest_peer_id: Option<PeerId> = None;
            let mut current_max_po = 0;
            let peer_candidates: Vec<(Vec<u8>, PeerId)> = {
                let peers_map = peers.lock().await;
                peers_map
                    .iter()
                    .map(|(overlay, id)| (overlay.clone(), *id))
                    .collect()
            };

            for (overlay, id) in peer_candidates {
                if skiplist.contains(&id) {
                    continue;
                }

                let current_po = get_proximity(&caddr, &overlay);

                if current_po >= current_max_po {
                    closest_overlay = overlay;
                    closest_peer_id = Some(id);
                    current_max_po = current_po;
                }
            }

            let Some(closest_peer_id) = closest_peer_id else {
                if !overdraftlist.is_empty() {
                    reset_push_overdraft(&mut skiplist, &mut overdraftlist);
                    async_std::task::sleep(Duration::from_millis(PUSH_CHUNK_ATTEMPT_RETRY_WAIT_MS))
                        .await;

                    continue;
                }

                let round_now = Date::now();

                let seg = round_now - round_commence;
                if seg < PROTOCOL_ROUND_TIME {
                    async_std::task::sleep(Duration::from_millis(
                        (PROTOCOL_ROUND_TIME - seg) as u64,
                    ))
                    .await;
                }

                round_commence = Date::now();

                if error_count >= max_error || success_count >= PUSH_CHUNK_CONFIRMATION_PEERS {
                    break;
                }

                continue;
            };

            skiplist.insert(closest_peer_id);

            let req_price = price(&closest_overlay, &caddr);

            let accounting_peer = {
                let accounting_peers = accounting.lock().await;
                accounting_peers.get(&closest_peer_id).cloned()
            };

            if let Some(accounting_peer) = accounting_peer {
                if let Some(connection_id) = reserve(&accounting_peer, req_price).await {
                    if let Some(session) = OutboundProtocolSession::capture(
                        closest_peer_id.clone(),
                        connection_id,
                        physical_connections.clone(),
                    ) {
                        selected_peer = Some(ReservedPushPeer {
                            peer: closest_peer_id,
                            price: req_price,
                            accounting: accounting_peer,
                            session,
                        });
                    } else {
                        cancel_reserve(&accounting_peer, req_price).await;
                    }
                } else {
                    overdraftlist.insert(closest_peer_id);
                }
            }
        }

        let Some(selected_peer) = selected_peer else {
            break;
        };

        if transfer_paused
            .as_ref()
            .map(transfer_pause_enabled)
            .unwrap_or(false)
        {
            cancel_reserve(&selected_peer.accounting, selected_peer.price).await;
            continue;
        }

        let refresh_chan = refresh_chan.clone();
        let attempt_out = attempt_out.clone();
        let caddr0 = caddr.clone();
        let data0 = data.clone();
        let cstamp00 = cstamp0.clone();
        let control0 = control.clone();

        wasm_bindgen_futures::spawn_local(async move {
            pushsync_attempt(
                selected_peer,
                caddr0,
                data0,
                cstamp00,
                control0,
                refresh_chan,
                attempt_out,
            )
            .await;
        });

        in_flight += 1;
        last_attempt_started = Date::now();
    }

    while in_flight > 0 && error_count < max_error && success_count < PUSH_CHUNK_CONFIRMATION_PEERS
    {
        match async_std::future::timeout(
            Duration::from_millis(PUSH_CHUNK_ATTEMPT_SOFT_TIMEOUT_MS),
            attempt_in.recv(),
        )
        .await
        {
            Ok(Ok(result)) => {
                record_push_attempt_result(
                    result,
                    &mut in_flight,
                    &mut success_count,
                    &mut error_count,
                );
            }
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }

    if success_count >= PUSH_CHUNK_CONFIRMATION_PEERS {
        return caddr;
    }

    upload_log!(
        "unable to push chunk {} through {} separate peers",
        hex::encode(&caddr),
        PUSH_CHUNK_CONFIRMATION_PEERS
    );

    vec![]
}

fn encrypt_with_span(span: &[u8; 8], cd: &[u8], encrey: &[u8]) -> Vec<u8> {
    if cd.len() > CHUNK_SIZE || encrey.is_empty() {
        return vec![];
    }

    let padding_length = CHUNK_SIZE - cd.len();
    let mut padding = vec![];

    for _i in 0..padding_length {
        padding.push(rand::random::<u8>());
    }

    let spancred = span.to_vec();
    let concred = ([&cd[..], &padding].concat()).to_vec();
    let creylen = encrey.len();

    let mut spanbytes: Vec<u8> = vec![];
    let mut spansegmentkey0: [u8; 4] = [0; 4];
    byteorder::LittleEndian::write_u32(&mut spansegmentkey0, (4096 / creylen) as u32);
    let spansegmentkey1 =
        keccak256(keccak256([encrey.to_vec(), spansegmentkey0.to_vec()].concat()).to_vec())
            .to_vec();

    for j in 0..8 {
        spanbytes.push(spancred[j] ^ spansegmentkey1[j])
    }

    let mut content: Vec<u8> = vec![];
    let mut done = false;
    let mut i = 0;
    while !done {
        let mut k = creylen;
        if k > concred.len() - (i * creylen) {
            k = concred.len() - (i * creylen);
        };

        let mut contentsegmentkey0: [u8; 4] = [0; 4];
        byteorder::LittleEndian::write_u32(&mut contentsegmentkey0, i as u32);
        let contentsegmentkey1 = keccak256(keccak256(
            [encrey.to_vec(), contentsegmentkey0.to_vec()].concat(),
        ))
        .to_vec();

        for j in (i * creylen)..(i * creylen + k) {
            content.push(concred[j] ^ contentsegmentkey1[j - i * creylen])
        }

        i += 1;

        if !(i * creylen < concred.len()) {
            done = true;
        }
    }

    [spanbytes, content].concat()
}

pub fn encrey() -> Vec<u8> {
    let mut encrey0 = vec![];

    for _ in 0..32 {
        encrey0.push(rand::random::<u8>());
    }

    encrey0
}

pub async fn make_soc(
    chunk_content: &Vec<u8>,
    owner: Vec<u8>,
    id_bytes: Vec<u8>,
) -> (Vec<u8>, Vec<u8>) {
    let soc_signer: PrivateKeySigner = match PrivateKeySigner::from_slice(&owner) {
        Ok(aok) => aok,
        _ => {
            upload_log!("owner key length not 32 but {}", owner.len());
            return (vec![], vec![]);
        }
    };

    let soc_address =
        keccak256([id_bytes.to_vec(), soc_signer.address().to_vec()].concat()).to_vec();

    let mut soc_content: Vec<u8> = vec![];

    soc_content.append(&mut id_bytes.clone());

    let wrapped_address = content_address(chunk_content);

    let digest = keccak256([id_bytes.clone(), wrapped_address].concat()).to_vec();

    let signature = soc_signer
        .sign_message(digest.as_slice())
        .await
        .unwrap()
        .as_bytes()
        .to_vec();

    if signature.len() != 65 {
        upload_log!("soc signature length not 64 but {}", signature.len());
        return (vec![], vec![]);
    }

    soc_content.append(&mut signature[0..65].to_vec());
    soc_content.append(&mut chunk_content.clone());

    return (soc_content, soc_address);
}

fn render_log_message(log: &String) {
    let document = web_sys::window().unwrap().document().unwrap();
    let log_message_div = document.create_element("div").unwrap();
    log_message_div.set_inner_html(&log);
    let logs = document
        .get_element_by_id("logsField")
        .expect("#logsField should exist");
    let _ = logs.prepend_with_node_1(&log_message_div);
    while logs.child_element_count() > crate::LOG_DOM_RETAINED {
        let Some(oldest) = logs.last_element_child() else {
            break;
        };
        oldest.remove();
    }
}
