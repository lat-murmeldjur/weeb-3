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
    PROTOCOL_ROUND_TIME,
    //                                                                        //
    PUSH_CHUNK_CONFIRMATION_PEERS,
    //                                                                        //
    PeerAccounting,
    //                                                                        //
    PeerId,
    //                                                                        //
    apply_credit,
    //                                                                        //
    cancel_reserve,
    //                                                                        //
    content_address,
    //                                                                        //
    get_chunk,
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
    stream,
    //                                                                        //
    transfer_pause_enabled,
    //                                                                        //
};

use byteorder::ByteOrder;

use async_std::sync::Arc;

use alloy::primitives::keccak256;
use alloy::signers::Signer;
use alloy::signers::local::PrivateKeySigner;

use serde_json::json;

use wasm_bindgen::JsCast;

use libp2p::futures::{StreamExt, future::join_all, stream::FuturesUnordered};

use std::{future::Future, pin::Pin, sync::atomic::AtomicBool};

const BATCH_BUCKET_TRIALS: usize = 1024;
const STAMP_CHUNK_WINDOW: usize = 64;
const PUSH_CHUNK_ATTEMPT_RETRY_WAIT_MS: u64 = 50;
const PUSH_CHUNK_ATTEMPT_SOFT_TIMEOUT_MS: u64 = 15000;
const PUSH_CHUNK_QUEUE_WINDOW: usize = 256;
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
    Vec<Vec<u8>>,
    u8,
    Vec<u8>,
    Vec<u8>,
    Option<UploadProgressSender>,
    mpsc::Sender<Vec<u8>>,
);
type StampFuture = Pin<Box<dyn Future<Output = (usize, Option<StampedChunk>)>>>;
type ChunkPushReceiptFuture = Pin<Box<dyn Future<Output = bool>>>;
type ChunkPushReceipts = FuturesUnordered<ChunkPushReceiptFuture>;

#[derive(Clone, Debug)]
pub(crate) struct UploadProgressDelta {
    pub chunks_total_delta: u64,
    pub chunks_done_delta: u64,
}

pub(crate) type UploadProgressSender = mpsc::Sender<UploadProgressDelta>;

struct PushAttemptResult {
    success: bool,
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
    soc: bool,
    address: Vec<u8>,
    stamp: Vec<u8>,
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

fn div_ceil_usize(value: usize, divisor: usize) -> usize {
    if value == 0 {
        0
    } else {
        ((value - 1) / divisor) + 1
    }
}

fn count_push_data_chunks(data: &[Vec<u8>], encryption: bool) -> u64 {
    if data.len() == 1 && data[0].len() <= 4096 {
        return 1;
    }

    let address_length = if encryption { 64 } else { 32 };
    let mut level = 0usize;
    let mut lengths: Vec<usize> = data.iter().map(Vec::len).collect();
    let mut total = 0u64;

    loop {
        let mut refs = 0usize;

        for length in &lengths {
            let chunk_count = div_ceil_usize(*length, 4096);
            for chunk_index in 0..chunk_count {
                let start = chunk_index * 4096;
                let end = ((chunk_index + 1) * 4096).min(*length);
                let chunk_len = end.saturating_sub(start);

                if chunk_len == address_length && level > 0 {
                    refs = refs.saturating_add(1);
                } else {
                    total = total.saturating_add(1);
                    refs = refs.saturating_add(1);
                }
            }
        }

        if refs <= 1 {
            return total.max(1);
        }

        lengths.clear();
        lengths.push(refs.saturating_mul(address_length));
        level = level.saturating_add(1);
    }
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

async fn flush_stamp_window(
    stamp_joiner: &mut Vec<StampFuture>,
    level_refs: &mut Vec<Vec<u8>>,
    chunk_receipts: &mut ChunkPushReceipts,
    chunk_slot_receipts: &mut ChunkPushReceipts,
    chunk_upload_chan: &ChunkUploadSender,
    progress: &Option<UploadProgressSender>,
) -> bool {
    if stamp_joiner.is_empty() {
        return true;
    }

    let mut stamped = join_all(std::mem::take(stamp_joiner)).await;
    stamped.sort_by_key(|(chunk_index, _)| *chunk_index);

    for (_, stamped_chunk) in stamped {
        let Some(stamped_chunk) = stamped_chunk else {
            return false;
        };

        let (result_chan_out, result_chan_in) = mpsc::unbounded::<bool>();
        let (slot_chan_out, slot_chan_in) = mpsc::unbounded::<bool>();

        if chunk_upload_chan
            .try_send((
                stamped_chunk.data,
                stamped_chunk.soc,
                stamped_chunk.address,
                stamped_chunk.stamp,
                result_chan_out,
                slot_chan_out,
                progress.clone(),
            ))
            .is_err()
        {
            return false;
        }

        level_refs.push(stamped_chunk.reference);
        track_chunk_push_receipt(chunk_receipts, result_chan_in);
        track_chunk_push_receipt(chunk_slot_receipts, slot_chan_in);

        if chunk_slot_receipts.len() >= PUSH_CHUNK_QUEUE_WINDOW {
            if !wait_for_next_chunk_push(chunk_slot_receipts).await {
                return false;
            }

            async_std::task::yield_now().await;
        }
    }

    true
}

async fn pushsync_attempt(
    peer: PeerId,
    req_price: u64,
    caddr: Vec<u8>,
    data: Vec<u8>,
    cstamp0: Vec<u8>,
    control: stream::Control,
    accounting: Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    refresh_chan: mpsc::Sender<(PeerId, u64)>,
    result_chan: mpsc::Sender<PushAttemptResult>,
) {
    let (chunk_out, chunk_in) = mpsc::unbounded::<bool>();

    // This task owns the protocol exchange and accounting even if push_chunk
    // starts trying another peer while this one is still waiting for a receipt.
    pushsync_handler(peer.clone(), &caddr, &data, &cstamp0, control, &chunk_out).await;

    let success = matches!(chunk_in.try_recv(), Ok(true));
    let accounting_peer = {
        let accounting_peers = accounting.lock().await;
        accounting_peers.get(&peer).cloned()
    };

    if let Some(accounting_peer) = accounting_peer {
        if success {
            apply_credit(&accounting_peer, req_price, &refresh_chan).await;
        } else {
            cancel_reserve(&accounting_peer, req_price).await;
        }
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
    pub data: Vec<Vec<u8>>,
    pub data_address: Vec<u8>,
}

pub async fn upload_resource(
    resource0: Vec<Resource>,
    encryption: bool,
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
        let core_reference = upload_data(
            r0.data,
            encryption,
            batch_owner.clone(),
            batch_id.clone(),
            &data_upload_chan,
            progress.clone(),
        )
        .await;

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

    let core_manifest0 = core_manifest.clone();

    let manifest_reference = upload_data(
        vec![core_manifest],
        encryption,
        batch_owner.clone(),
        batch_id.clone(),
        &data_upload_chan,
        progress.clone(),
    )
    .await;

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

    let wrapped_len: u64 = core_manifest0.len() as u64;
    let wrapped_span = wrapped_len.to_le_bytes();

    let mut wrapped_content = vec![];

    if core_manifest0.len() <= 4096 {
        wrapped_content = core_manifest0.clone();
    } else {
        let mut uploaded = false;
        while !uploaded {
            let crown_chunk = get_chunk(manifest_reference.clone(), &chunk_retrieve_chan).await;
            if crown_chunk.len() > 0 {
                wrapped_content = crown_chunk[8..].to_vec();
                uploaded = true;
            } else {
                async_std::task::sleep(Duration::from_millis(1000)).await;
            }
        }
    }

    let mut soc_wrapped_content: Vec<u8> = vec![];
    soc_wrapped_content.append(&mut wrapped_span.to_vec());
    soc_wrapped_content.append(&mut wrapped_content);

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
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    data_upload_chan: &mpsc::Sender<DataUploadRequest>,
    progress: Option<UploadProgressSender>,
) -> Vec<u8> {
    let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
    let mut enc_mode = 0;
    if enc {
        enc_mode = 1;
    }

    data_upload_chan
        .try_send((data, enc_mode, batch_owner, batch_id, progress, chan_out))
        .unwrap();

    let result = match chan_in.recv().await {
        Ok(result) => {
            upload_log!("Upload data returning {:#?}!", hex::encode(&result));

            result
        }
        Err(_) => vec![],
    };

    return result;
}

async fn stamp_push_chunk(
    ch_d: Vec<u8>,
    span: usize,
    encryption: bool,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    batch_bucket_limit: u32,
) -> Option<StampedChunk> {
    let mut soc = false;
    let mut encrey0 = vec![];

    let mut data0 = match encryption {
        true => {
            encrey0 = encrey();
            encrypt(span, &ch_d, &encrey0)
        }
        false => [(span as u64).to_le_bytes().to_vec(), ch_d.clone()].concat(),
    };

    let mut cstamp0: Vec<u8> = vec![];
    let mut bucket_full: bool;
    let mut cha = content_address(&data0);

    for _ in 0..BATCH_BUCKET_TRIALS {
        (cstamp0, bucket_full) = stamp_chunk(
            batch_owner.clone(),
            batch_id.clone(),
            batch_bucket_limit,
            cha.clone(),
        )
        .await;

        if !bucket_full {
            break;
        } else {
            render_log_message(&"Restamping chunk to avoid bucket overflow".to_string());
            match encryption {
                true => {
                    encrey0 = encrey();
                    data0 = encrypt(span, &ch_d, &encrey0);
                    cha = content_address(&data0);
                }
                false => {
                    soc = true;
                    let ob = encrey();
                    let idb = encrey();

                    let mut soc_wrapped_content: Vec<u8> = vec![];
                    soc_wrapped_content.append(&mut (span as u64).to_le_bytes().to_vec());
                    soc_wrapped_content.append(&mut ch_d.clone());

                    let (data00, cha0) = make_soc(&soc_wrapped_content, ob, idb).await;

                    data0 = data00;
                    cha = cha0;
                }
            }
        }
    }

    if cstamp0.len() == 0 {
        render_log_message(&"Stamp length 0".to_string());

        return None;
    }

    let reference = [cha.clone(), encrey0].concat();

    Some(StampedChunk {
        reference,
        data: data0,
        soc,
        address: cha,
        stamp: cstamp0,
    })
}

pub async fn push_data(
    mut data: Vec<Vec<u8>>,
    encryption: bool,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    batch_bucket_limit: u32,
    chunk_upload_chan: &ChunkUploadSender,
    progress: Option<UploadProgressSender>,
) -> Vec<u8> {
    let total_chunks = count_push_data_chunks(&data, encryption);
    report_upload_progress(&progress, total_chunks, 0);

    let mut span_length = 0;

    for i in &data {
        span_length += i.len();
    }

    if data.len() == 1 && data[0].len() <= 4096 {
        let mut soc = false;
        let mut encrey0 = vec![];

        let mut data0 = match encryption {
            true => {
                encrey0 = encrey();
                encrypt(span_length, &data[0], &encrey0)
            }
            false => [
                (data[0].len() as u64).to_le_bytes().to_vec(),
                data[0].clone(),
            ]
            .concat(),
        };

        let mut cstamp0: Vec<u8> = vec![];
        let mut bucket_full: bool;
        let mut cha = content_address(&data0);

        for _ in 0..BATCH_BUCKET_TRIALS {
            (cstamp0, bucket_full) = stamp_chunk(
                batch_owner.clone(),
                batch_id.clone(),
                batch_bucket_limit,
                cha.clone(),
            )
            .await;

            if !bucket_full {
                break;
            } else {
                render_log_message(&"Restamping chunk to avoid bucket overflow".to_string());
                match encryption {
                    true => {
                        encrey0 = encrey();
                        data0 = encrypt(span_length, &data[0], &encrey0);
                        cha = content_address(&data0);
                    }
                    false => {
                        soc = true;
                        let (data00, cha0) = make_soc(
                            &[span_length.to_le_bytes().to_vec(), data[0].clone()].concat(),
                            encrey(),
                            encrey(),
                        )
                        .await;

                        data0 = data00;
                        cha = cha0;
                    }
                }
            }
        }

        if cstamp0.len() == 0 {
            return vec![];
        }

        let (result_chan_out, result_chan_in) = mpsc::unbounded::<bool>();
        let (slot_chan_out, _slot_chan_in) = mpsc::unbounded::<bool>();

        if chunk_upload_chan
            .try_send((
                data0,
                soc,
                cha.clone(),
                cstamp0,
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

        return [cha, encrey0].concat();
    } else {
        let mut levels: Vec<Vec<Vec<u8>>> = Vec::new();

        let mut address_length = 32;
        if encryption {
            address_length = 64;
        }

        let mut level = 0;
        let address_fit = 4096 / address_length;
        let next_level = true;
        let mut span_carriage = 4096;

        let mut chunk_receipts: ChunkPushReceipts = FuturesUnordered::new();
        let mut chunk_slot_receipts: ChunkPushReceipts = FuturesUnordered::new();

        while next_level {
            let mut sc = 0;
            levels.push(Vec::new());
            let mut stamp_joiner: Vec<StampFuture> = Vec::new();
            let mut stamp_order = 0usize;

            for level_data in &data {
                let mut chunk_l0r = level_data.len() % 4096;
                if chunk_l0r > 0 {
                    chunk_l0r = 1;
                }
                let chunk_l0c = level_data.len() / 4096 + chunk_l0r;

                for i in 0..chunk_l0c {
                    let data_start = i * 4096 as usize;
                    let mut data_end = (i + 1) * 4096 as usize;
                    if data_end > level_data.len() {
                        data_end = level_data.len();
                    };

                    let ch_d = level_data[data_start..data_end].to_vec();

                    let mut span = span_carriage;

                    if (sc + 1) * span_carriage > span_length {
                        span = span_length - (sc * span_carriage);
                    };

                    sc += 1;

                    if chunk_l0c == 1 {
                        span = span_length;
                    }

                    if data_end - data_start == address_length && level > 0 {
                        if !flush_stamp_window(
                            &mut stamp_joiner,
                            &mut levels[level],
                            &mut chunk_receipts,
                            &mut chunk_slot_receipts,
                            chunk_upload_chan,
                            &progress,
                        )
                        .await
                        {
                            return vec![];
                        }

                        levels[level].push(ch_d);
                    } else {
                        let chunk_index = stamp_order;
                        stamp_order += 1;

                        let batch_owner0 = batch_owner.clone();
                        let batch_id0 = batch_id.clone();

                        stamp_joiner.push(Box::pin(async move {
                            (
                                chunk_index,
                                stamp_push_chunk(
                                    ch_d,
                                    span,
                                    encryption,
                                    batch_owner0,
                                    batch_id0,
                                    batch_bucket_limit,
                                )
                                .await,
                            )
                        }));

                        if stamp_joiner.len() >= STAMP_CHUNK_WINDOW {
                            if !flush_stamp_window(
                                &mut stamp_joiner,
                                &mut levels[level],
                                &mut chunk_receipts,
                                &mut chunk_slot_receipts,
                                chunk_upload_chan,
                                &progress,
                            )
                            .await
                            {
                                return vec![];
                            }
                        }
                    }
                }
            }

            if !flush_stamp_window(
                &mut stamp_joiner,
                &mut levels[level],
                &mut chunk_receipts,
                &mut chunk_slot_receipts,
                chunk_upload_chan,
                &progress,
            )
            .await
            {
                return vec![];
            }

            if levels[level].len() == 1 {
                let reference = levels[level][0].clone();
                if !wait_for_chunk_pushes(&mut chunk_receipts).await {
                    return vec![];
                }
                return reference;
            } else {
                data.clear();
                data.shrink_to_fit();
                data = vec![levels[level].concat()];
                level += 1;
                span_carriage *= address_fit;
            }
        }

        return vec![];
    }

    #[allow(unreachable_code)]
    return vec![];
}

pub async fn push_chunk(
    data: Vec<u8>,
    soc: bool,
    soc_address: Vec<u8>,
    cstamp0: Vec<u8>,
    control: stream::Control,
    peers: &Arc<Mutex<HashMap<String, PeerId>>>,
    accounting: &Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
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

        let mut selected_peer: Option<(PeerId, u64)> = None;

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

            let mut closest_overlay = "".to_string();
            let mut closest_peer_id: Option<PeerId> = None;
            let mut current_max_po = 0;
            let peer_candidates: Vec<(String, PeerId)> = {
                let peers_map = peers.lock().await;
                peers_map
                    .iter()
                    .map(|(ov, id)| (ov.clone(), id.clone()))
                    .collect()
            };

            for (ov, id) in peer_candidates {
                if skiplist.contains(&id) {
                    continue;
                }

                let current_po = get_proximity(&caddr, &hex::decode(&ov).unwrap());

                if current_po >= current_max_po {
                    closest_overlay = ov;
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
                let allowed = reserve(&accounting_peer, req_price).await;
                if !allowed {
                    overdraftlist.insert(closest_peer_id);
                } else {
                    selected_peer = Some((closest_peer_id, req_price));
                }
            }
        }

        let Some((closest_peer_id, req_price)) = selected_peer else {
            break;
        };

        if transfer_paused
            .as_ref()
            .map(transfer_pause_enabled)
            .unwrap_or(false)
        {
            let accounting_peer = {
                let accounting_peers = accounting.lock().await;
                accounting_peers.get(&closest_peer_id).cloned()
            };
            if let Some(accounting_peer) = accounting_peer {
                cancel_reserve(&accounting_peer, req_price).await;
            }
            continue;
        }

        let accounting = accounting.clone();
        let refresh_chan = refresh_chan.clone();
        let attempt_out = attempt_out.clone();
        let caddr0 = caddr.clone();
        let data0 = data.clone();
        let cstamp00 = cstamp0.clone();
        let control0 = control.clone();

        wasm_bindgen_futures::spawn_local(async move {
            pushsync_attempt(
                closest_peer_id,
                req_price,
                caddr0,
                data0,
                cstamp00,
                control0,
                accounting,
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

pub fn encrypt(span: usize, cd: &Vec<u8>, encrey: &Vec<u8>) -> Vec<u8> {
    if cd.len() < 8 {
        return vec![];
    }

    let padding_length = 4096 - cd.len();
    let mut padding = vec![];

    for _i in 0..padding_length {
        padding.push(rand::random::<u8>());
    }

    let spancred = (span as u64).to_le_bytes().to_vec();
    let concred = ([&cd[..], &padding].concat()).to_vec();
    let creylen = encrey.len();

    let mut spanbytes: Vec<u8> = vec![];
    let mut spansegmentkey0: [u8; 4] = [0; 4];
    byteorder::LittleEndian::write_u32(&mut spansegmentkey0, (4096 / creylen) as u32);
    let spansegmentkey1 =
        keccak256(keccak256([encrey.clone(), spansegmentkey0.to_vec()].concat()).to_vec()).to_vec();

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
            [encrey.clone(), contentsegmentkey0.to_vec()].concat(),
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

    return [spanbytes, content].concat();
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
    //let index_bytes = index.to_le_bytes().to_vec();
    //let owner_bytes = hex::decode(owner).unwrap();
    //let topic_bytes = hex::decode(topic).unwrap();
    //let id_bytes = keccak256([topic_bytes, index_bytes].concat()).to_vec();

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
    let _r = document
        .get_element_by_id("logsField")
        .expect("#logsField should exist")
        .dyn_ref::<web_sys::HtmlElement>()
        .unwrap()
        .prepend_with_node_1(&log_message_div)
        .unwrap();
}
