use crate::{
    ChunkRetrieveSender, Date, Duration, HashMap, HashSet, JsValue, Mutex, PeerAccounting, PeerId,
    RETRIEVE_CHECK_CONFIRMATION_PEERS, RetrieveCancelToken, RetrieveGenerationMap, apply_credit,
    cancel_reserve, chunk_retrieve_request, encode_resources, get_feed_address, get_proximity,
    manifest::interpret_manifest, mpsc, price, reserve, retrieve_cancel_token_current,
    retrieve_handler, stream, transfer_pause_enabled, valid_cac, valid_soc,
};

use alloy::primitives::keccak256;
use async_std::sync::Arc;
use byteorder::ByteOrder;
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::atomic::AtomicBool};

const RETRIEVE_HEDGE_AFTER_MS: u64 = 360;
const RETRIEVE_ATTEMPT_TIMEOUT_MS: u64 = 10_000;
const RETRIEVE_HOT_LOOP_GUARD_MS: u64 = 25;
const RETRIEVE_DATA_DISPATCH_YIELD_EVERY: usize = 128;
const RETRIEVE_CHECK_RETRY_WAIT_MS: u64 = 160;
const RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS: usize = 20;
const RETRIEVE_DATA_MAX_BYTES: u64 = 4 * 1024 * 1024;
const DEBUG_RETRIEVAL_LOGS: bool = true;
const DEBUG_RETRIEVE_TRACE_LOGS: bool = true;

macro_rules! retrieval_debug {
    ($($arg:tt)*) => {
        if DEBUG_RETRIEVAL_LOGS {
            web_sys::console::log_1(&JsValue::from(format!($($arg)*)));
        }
    };
}

macro_rules! retrieve_trace {
    ($($arg:tt)*) => {
        if DEBUG_RETRIEVE_TRACE_LOGS {
            web_sys::console::log_1(&JsValue::from(format!("retrieve-trace: {}", format!($($arg)*))));
        }
    };
}

struct RetrieveAttemptResult {
    peer: PeerId,
    chunk: Vec<u8>,
    valid: bool,
    soc: bool,
}

use libp2p::futures::{StreamExt, stream::FuturesUnordered};

type RetrieveJoiner =
    FuturesUnordered<Pin<Box<dyn Future<Output = (Vec<usize>, Vec<u8>, Vec<u8>)>>>>;

fn queue_chunk_retrieve(
    order: Vec<usize>,
    addr: Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    joiner: &mut RetrieveJoiner,
) {
    let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
    let _ = chunk_retrieve_chan.try_send(chunk_retrieve_request(addr.clone(), chan_out));

    joiner.push(Box::pin(async move {
        let chunk = chan_in.recv().await.unwrap_or_default();
        (order, addr, chunk)
    }));
}

fn should_yield_after_chunk_dispatch(dispatched: usize) -> bool {
    dispatched % RETRIEVE_DATA_DISPATCH_YIELD_EVERY == 0
}

pub async fn retrieve_resource(
    chunk_address: &Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Vec<u8> {
    let cd = get_data(chunk_address.to_vec(), &data_retrieve_chan).await;

    let mut data_vector_e: Vec<(Vec<u8>, String, String)> = vec![];

    #[allow(unused_assignments)]
    let mut index: String = "".to_string();

    {
        let (data_vector, index0) = interpret_manifest(
            "".to_string(),
            &cd,
            &data_retrieve_chan,
            &chunk_retrieve_chan,
        )
        .await;

        index = index0;

        retrieval_debug!("manifest interpreted");

        for f in &data_vector {
            if f.data.len() > 8 {
                data_vector_e.push((f.data[8..].to_vec(), f.mime.clone(), f.path.clone()));
            };
        }
    }

    retrieval_debug!("resource entries decoded");

    if data_vector_e.len() == 0 {
        retrieval_debug!("Unable to retrieve resource case 0");

        return encode_resources(
            vec![(vec![], "not found".to_string(), "not found".to_string())],
            index,
        );
    }

    retrieval_debug!("resource encoded");

    return encode_resources(data_vector_e, index);
}

pub(crate) fn split_chunk_references(data: &[u8], address_length: usize) -> Option<Vec<Vec<u8>>> {
    if address_length == 0 || data.len() % address_length != 0 {
        return None;
    }

    Some(
        data.chunks(address_length)
            .map(|address| address.to_vec())
            .collect(),
    )
}

async fn select_retrieve_peer(
    caddr: &Vec<u8>,
    peers: &Arc<Mutex<HashMap<String, PeerId>>>,
    accounting: &Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    skiplist: &mut HashSet<PeerId>,
    overdraftlist: &mut HashSet<PeerId>,
) -> Option<(PeerId, u64)> {
    loop {
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

            let current_po = get_proximity(caddr, &hex::decode(&ov).unwrap());

            if current_po >= current_max_po || closest_peer_id.is_none() {
                closest_overlay = ov;
                closest_peer_id = Some(id);
                current_max_po = current_po;
            }
        }

        let Some(peer) = closest_peer_id else {
            return None;
        };

        skiplist.insert(peer.clone());
        let req_price = price(&closest_overlay, caddr);

        let accounting_peer = {
            let accounting_peers = accounting.lock().await;
            accounting_peers.get(&peer).cloned()
        };

        if let Some(accounting_peer) = accounting_peer {
            let allowed = reserve(&accounting_peer, req_price).await;
            if allowed {
                return Some((peer, req_price));
            }

            overdraftlist.insert(peer);
        }

        async_std::task::yield_now().await;
        async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
    }
}

fn reset_overdraft(skiplist: &mut HashSet<PeerId>, overdraftlist: &mut HashSet<PeerId>) {
    for peer in overdraftlist.drain() {
        skiplist.remove(&peer);
    }
}

async fn retrieve_attempt(
    selected: (PeerId, u64),
    caddr: Vec<u8>,
    control: stream::Control,
    accounting: Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    refresh_chan: mpsc::Sender<(PeerId, u64)>,
    result_chan: mpsc::Sender<RetrieveAttemptResult>,
) {
    let (peer, req_price) = selected;
    let (chunk_out, chunk_in) = mpsc::unbounded::<Vec<u8>>();
    let chunk_hex = hex::encode(&caddr);
    let started = Date::now();

    let result = RetrieveAttemptResult {
        peer: peer.clone(),
        chunk: vec![],
        valid: false,
        soc: false,
    };

    retrieval_debug!(
        "retrieve attempt start chunk={} peer={} price={} timeout_ms={}",
        chunk_hex,
        peer,
        req_price,
        RETRIEVE_ATTEMPT_TIMEOUT_MS
    );

    let handler_peer = peer.clone();
    let handler_caddr = caddr.clone();
    wasm_bindgen_futures::spawn_local(async move {
        retrieve_handler(handler_peer, handler_caddr, control, &chunk_out).await;
    });

    let retrieve_result = async_std::future::timeout(
        Duration::from_millis(RETRIEVE_ATTEMPT_TIMEOUT_MS),
        chunk_in.recv(),
    )
    .await;

    if retrieve_result.is_err() {
        let accounting_peer = {
            let accounting_peers = accounting.lock().await;
            accounting_peers.get(&peer).cloned()
        };
        if let Some(accounting_peer) = accounting_peer {
            cancel_reserve(&accounting_peer, req_price).await
        }
        retrieval_debug!(
            "retrieve attempt timeout chunk={} peer={} elapsed_ms={} timeout_ms={}",
            chunk_hex,
            peer,
            Date::now() - started,
            RETRIEVE_ATTEMPT_TIMEOUT_MS
        );
        let _ = result_chan.try_send(result);
        return;
    }

    match retrieve_result {
        Ok(Ok(chunk)) => {
            retrieval_debug!(
                "retrieve attempt response chunk={} peer={} bytes={} elapsed_ms={}",
                chunk_hex,
                peer,
                chunk.len(),
                Date::now() - started
            );
            let (chunk_valid, soc) = verify_chunk(&caddr, &chunk);
            if chunk_valid {
                retrieval_debug!(
                    "retrieve attempt success chunk={} peer={} soc={} price={} elapsed_ms={}",
                    chunk_hex,
                    peer,
                    soc,
                    req_price,
                    Date::now() - started
                );
                let _ = result_chan.try_send(RetrieveAttemptResult {
                    peer: peer.clone(),
                    chunk,
                    valid: true,
                    soc,
                });

                let accounting_peer = {
                    let accounting_peers = accounting.lock().await;
                    accounting_peers.get(&peer).cloned()
                };
                if let Some(accounting_peer) = accounting_peer {
                    apply_credit(&accounting_peer, req_price, &refresh_chan).await;
                }
                return;
            } else {
                retrieval_debug!(
                    "retrieve attempt invalid chunk={} peer={} bytes={} elapsed_ms={}",
                    chunk_hex,
                    peer,
                    chunk.len(),
                    Date::now() - started
                );
                let accounting_peer = {
                    let accounting_peers = accounting.lock().await;
                    accounting_peers.get(&peer).cloned()
                };
                if let Some(accounting_peer) = accounting_peer {
                    cancel_reserve(&accounting_peer, req_price).await
                }
            }
        }
        Ok(Err(error)) => {
            let accounting_peer = {
                let accounting_peers = accounting.lock().await;
                accounting_peers.get(&peer).cloned()
            };
            if let Some(accounting_peer) = accounting_peer {
                cancel_reserve(&accounting_peer, req_price).await
            }
            retrieval_debug!(
                "retrieve attempt no response chunk={} peer={} error={} elapsed_ms={}",
                chunk_hex,
                peer,
                error,
                Date::now() - started
            );
        }
        Err(_) => {}
    }

    retrieval_debug!(
        "retrieve attempt failed chunk={} peer={} elapsed_ms={}",
        chunk_hex,
        peer,
        Date::now() - started
    );

    let _ = result_chan.try_send(result);
}

fn chunk_address_parts(chunk_address: &Vec<u8>) -> (Vec<u8>, Vec<u8>, bool) {
    if chunk_address.len() == 64 {
        return (
            (&chunk_address[0..32]).to_vec(),
            (&chunk_address[32..64]).to_vec(),
            true,
        );
    }

    (chunk_address.to_vec(), vec![], false)
}

fn decode_retrieved_chunk(
    chunk_address: &Vec<u8>,
    cd: Vec<u8>,
    soc: bool,
    encrey: Vec<u8>,
    encred: bool,
) -> Vec<u8> {
    if encred {
        if soc {
            if cd.len() < 97 {
                retrieval_debug!(
                    "unable to retrieve chunk {} - encrypted chunk no content",
                    hex::encode(chunk_address)
                );
                return vec![];
            }

            let cd00 = decrypt(&(&cd[97..]).to_vec(), encrey);
            if cd00.len() >= 8 {
                return cd00;
            } else {
                retrieval_debug!(
                    "unable to retrieve chunk {} - encrypted chunk no content",
                    hex::encode(chunk_address)
                );
                return vec![];
            }
        }

        return decrypt(&cd, encrey);
    }

    if soc && cd.len() >= 97 + 8 {
        return (&cd[97..]).to_vec();
    }
    if cd.len() == 0 {
        retrieval_debug!(
            "unable to retrieve chunk {} - chunk empty",
            hex::encode(chunk_address)
        );
    }

    cd
}

pub async fn retrieve_data(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Vec<u8> {
    retrieve_trace!(
        "data start ref_len={} addr={}",
        data_address.len(),
        hex::encode(data_address)
    );
    let root_chunk = get_chunk(data_address.to_vec(), &chunk_retrieve_chan).await;

    let root_span: u64;
    if root_chunk.len() >= 8 {
        root_span = u64::from_le_bytes(root_chunk[0..8].try_into().unwrap());
    } else {
        retrieve_trace!("data root missing addr={}", hex::encode(data_address));
        retrieval_debug!("chunk not found: {}", hex::encode(data_address),);
        return vec![];
    }

    retrieve_trace!(
        "data root addr={} bytes={} span={}",
        hex::encode(data_address),
        root_chunk.len(),
        root_span
    );

    if root_span <= 4096 {
        if (root_span + 8) as usize == root_chunk.len() {
            retrieve_trace!(
                "data leaf complete addr={} bytes={}",
                hex::encode(data_address),
                root_chunk.len()
            );
            return root_chunk;
        } else {
            retrieve_trace!(
                "data leaf length mismatch addr={} bytes={} span={}",
                hex::encode(data_address),
                root_chunk.len(),
                root_span
            );
            retrieval_debug!(
                "retrieved chunk length ({}) mismatching span ({}) + 8 for chunk {}",
                root_chunk.len(),
                root_span,
                hex::encode(data_address),
            );
            return vec![];
        }
    }

    let address_length = data_address.len();

    if root_chunk.len() < 8 + address_length || (root_chunk.len() - 8) % address_length != 0 {
        retrieval_debug!("chunk too short: {}", hex::encode(data_address),);
        return vec![];
    }

    if root_span > RETRIEVE_DATA_MAX_BYTES {
        retrieve_trace!(
            "data root span too large addr={} bytes={} span={} max={}",
            hex::encode(data_address),
            root_chunk.len(),
            root_span,
            RETRIEVE_DATA_MAX_BYTES
        );
        return vec![];
    }

    let expected_len = match usize::try_from(root_span.saturating_add(8)) {
        Ok(expected_len) => expected_len,
        Err(_) => return vec![],
    };

    let mut data: Vec<u8> = Vec::with_capacity(expected_len);
    data.extend_from_slice(&root_chunk[0..8]);

    let root_refs = match split_chunk_references(&root_chunk[8..], address_length) {
        Some(addresses) => addresses,
        None => return vec![],
    };
    retrieve_trace!(
        "data tree root addr={} refs={} address_len={}",
        hex::encode(data_address),
        root_refs.len(),
        address_length
    );

    let mut joiner: RetrieveJoiner = FuturesUnordered::new();
    let mut dispatched = 0usize;
    for (index, addr) in root_refs.into_iter().enumerate() {
        queue_chunk_retrieve(vec![index], addr, chunk_retrieve_chan, &mut joiner);
        dispatched += 1;
        if should_yield_after_chunk_dispatch(dispatched) {
            async_std::task::yield_now().await;
        }
    }

    let mut leaf_chunks: BTreeMap<Vec<usize>, Vec<u8>> = BTreeMap::new();

    while let Some((order, addr, result0)) = joiner.next().await {
        if result0.len() <= 8 {
            retrieve_trace!(
                "data child missing root={} child={} order_depth={}",
                hex::encode(data_address),
                hex::encode(&addr),
                order.len()
            );
            retrieval_debug!("chunk not found: {}", hex::encode(addr),);
            return vec![];
        }

        let result_span = u64::from_le_bytes(result0[0..8].try_into().unwrap());
        retrieve_trace!(
            "data child root={} child={} bytes={} span={} order_depth={}",
            hex::encode(data_address),
            hex::encode(&addr),
            result0.len(),
            result_span,
            order.len()
        );

        if result_span > 4096 {
            let child_refs = match split_chunk_references(&result0[8..], address_length) {
                Some(addresses) => addresses,
                None => return vec![],
            };
            retrieve_trace!(
                "data child tree child={} refs={}",
                hex::encode(&addr),
                child_refs.len()
            );

            for (child_index, child_addr) in child_refs.into_iter().enumerate() {
                let mut child_order = order.clone();
                child_order.push(child_index);
                queue_chunk_retrieve(child_order, child_addr, chunk_retrieve_chan, &mut joiner);
                dispatched += 1;
                if should_yield_after_chunk_dispatch(dispatched) {
                    async_std::task::yield_now().await;
                }
            }
        } else {
            leaf_chunks.insert(order, result0[8..].to_vec());
        }
        async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
    }

    for (_order, chunk_data) in leaf_chunks {
        if chunk_data.is_empty() {
            return vec![];
        }
        data.extend_from_slice(&chunk_data);
    }

    if data.len() == expected_len {
        retrieve_trace!(
            "data complete addr={} bytes={}",
            hex::encode(data_address),
            data.len()
        );
        data
    } else {
        retrieve_trace!(
            "data final length mismatch addr={} bytes={} span={}",
            hex::encode(data_address),
            data.len(),
            root_span
        );
        retrieval_debug!(
            "retrieved result length ({}) not matching span ({}) + 8 for data address {}",
            data.len(),
            root_span,
            hex::encode(data_address),
        );
        vec![]
    }
}

pub async fn retrieve_chunk(
    chunk_address: &Vec<u8>,
    control: stream::Control,
    peers: &Arc<Mutex<HashMap<String, PeerId>>>,
    accounting: &Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
    cancel_generations: Option<RetrieveGenerationMap>,
    cancel: Option<RetrieveCancelToken>,
    transfer_paused: Option<Arc<AtomicBool>>,
) -> Vec<u8> {
    let (caddr, encrey, encred) = chunk_address_parts(chunk_address);

    let mut soc = false;
    let mut skiplist: HashSet<PeerId> = HashSet::new();
    let mut overdraftlist: HashSet<PeerId> = HashSet::new();

    let mut attempt_count = 0;
    let mut error_count = 0;

    #[allow(unused_assignments)]
    let mut cd = vec![];

    // cd = retrieve_cached_chunk(&caddr).await;
    // if cd.len() > 0 {
    //     (chunk_valid, soc) = verify_chunk(&caddr, &cd);
    //     if chunk_valid {
    //         error_count = RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS;
    //     } else {
    //         cd = vec![];
    //     };
    // };

    let (attempt_out, attempt_in) = mpsc::unbounded::<RetrieveAttemptResult>();
    let mut in_flight = 0_usize;
    let mut last_attempt_started = 0.0;

    while error_count < RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS
        && (attempt_count < RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS || in_flight > 0)
    {
        if in_flight > 0 {
            if let Ok(result) = attempt_in.try_recv() {
                in_flight = in_flight.saturating_sub(1);
                if result.valid {
                    cd = result.chunk;
                    soc = result.soc;
                    break;
                }
                error_count += 1;
                cd = vec![];

                async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
                continue;
            }
        }

        let paused = transfer_paused
            .as_ref()
            .map(transfer_pause_enabled)
            .unwrap_or(false);
        let cancelled = if let Some(cancel_generations) = &cancel_generations {
            !retrieve_cancel_token_current(cancel_generations, &cancel).await
        } else {
            false
        };

        if cancelled && in_flight == 0 {
            break;
        }

        if paused && in_flight == 0 {
            async_std::task::sleep(Duration::from_millis(100)).await;
            continue;
        }

        let now = Date::now();
        let can_start_attempt = attempt_count < RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS;
        let due = can_start_attempt
            && !paused
            && !cancelled
            && (in_flight == 0 || now - last_attempt_started >= RETRIEVE_HEDGE_AFTER_MS as f64);

        if due {
            if let Some(selected) =
                select_retrieve_peer(&caddr, peers, accounting, &mut skiplist, &mut overdraftlist)
                    .await
            {
                let cancelled_after_select = if let Some(cancel_generations) = &cancel_generations {
                    !retrieve_cancel_token_current(cancel_generations, &cancel).await
                } else {
                    false
                };

                if cancelled_after_select {
                    let accounting_peer = {
                        let accounting_peers = accounting.lock().await;
                        accounting_peers.get(&selected.0).cloned()
                    };
                    if let Some(accounting_peer) = accounting_peer {
                        cancel_reserve(&accounting_peer, selected.1).await;
                    }
                    skiplist.remove(&selected.0);
                    if in_flight == 0 {
                        break;
                    }
                    async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
                    continue;
                }

                if transfer_paused
                    .as_ref()
                    .map(transfer_pause_enabled)
                    .unwrap_or(false)
                {
                    let accounting_peer = {
                        let accounting_peers = accounting.lock().await;
                        accounting_peers.get(&selected.0).cloned()
                    };
                    if let Some(accounting_peer) = accounting_peer {
                        cancel_reserve(&accounting_peer, selected.1).await;
                    }
                    skiplist.remove(&selected.0);
                    async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
                    continue;
                }

                let control = control.clone();
                let accounting = accounting.clone();
                let refresh_chan = refresh_chan.clone();
                let attempt_out = attempt_out.clone();
                let caddr = caddr.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    retrieve_attempt(
                        selected,
                        caddr,
                        control,
                        accounting,
                        refresh_chan,
                        attempt_out,
                    )
                    .await;
                });
                attempt_count += 1;
                in_flight += 1;
                last_attempt_started = Date::now();
            } else if !overdraftlist.is_empty() {
                reset_overdraft(&mut skiplist, &mut overdraftlist);
                async_std::task::sleep(Duration::from_millis(50)).await;
                continue;
            } else if in_flight == 0 && !skiplist.is_empty() {
                break;
            } else {
                async_std::task::sleep(Duration::from_millis(50)).await;
                continue;
            }
        }

        if in_flight == 0 {
            async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
            continue;
        }

        let elapsed = Date::now() - last_attempt_started;
        let wait_ms = if !can_start_attempt || cancelled || paused {
            250
        } else {
            (RETRIEVE_HEDGE_AFTER_MS as f64 - elapsed).max(0.0).round() as u64
        };
        if wait_ms == 0 {
            async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
            continue;
        }

        match async_std::future::timeout(Duration::from_millis(wait_ms), attempt_in.recv()).await {
            Ok(Ok(result)) => {
                in_flight = in_flight.saturating_sub(1);
                if result.valid {
                    cd = result.chunk;
                    soc = result.soc;
                    break;
                }
                error_count += 1;
                cd = vec![];
            }
            Ok(Err(_)) => break,
            Err(_) => {}
        };
    }

    decode_retrieved_chunk(chunk_address, cd, soc, encrey, encred)
}

pub async fn retrieve_check_chunk(
    chunk_address: &Vec<u8>,
    control: stream::Control,
    peers: &Arc<Mutex<HashMap<String, PeerId>>>,
    accounting: &Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
    transfer_paused: Option<Arc<AtomicBool>>,
) -> Vec<u8> {
    let (caddr, encrey, encred) = chunk_address_parts(chunk_address);

    let mut skiplist: HashSet<PeerId> = HashSet::new();
    let mut overdraftlist: HashSet<PeerId> = HashSet::new();
    let mut success_peers: HashSet<PeerId> = HashSet::new();

    let mut error_count = 0;
    let max_error = 21 - RETRIEVE_CHECK_CONFIRMATION_PEERS;

    #[allow(unused_assignments)]
    let mut cd = vec![];
    let mut soc = false;
    let (attempt_out, attempt_in) = mpsc::unbounded::<RetrieveAttemptResult>();

    while error_count < max_error && success_peers.len() < RETRIEVE_CHECK_CONFIRMATION_PEERS {
        while let Ok(result) = attempt_in.try_recv() {
            if result.valid {
                if success_peers.insert(result.peer) && cd.is_empty() {
                    cd = result.chunk;
                    soc = result.soc;
                }
            } else {
                error_count += 1;
            }
        }

        if error_count >= max_error || success_peers.len() >= RETRIEVE_CHECK_CONFIRMATION_PEERS {
            break;
        }

        while transfer_paused
            .as_ref()
            .map(transfer_pause_enabled)
            .unwrap_or(false)
        {
            async_std::task::sleep(Duration::from_millis(100)).await;
        }

        let Some(selected) =
            select_retrieve_peer(&caddr, peers, accounting, &mut skiplist, &mut overdraftlist)
                .await
        else {
            if !overdraftlist.is_empty() {
                reset_overdraft(&mut skiplist, &mut overdraftlist);
            }
            async_std::task::sleep(Duration::from_millis(RETRIEVE_CHECK_RETRY_WAIT_MS)).await;
            continue;
        };

        if transfer_paused
            .as_ref()
            .map(transfer_pause_enabled)
            .unwrap_or(false)
        {
            let accounting_peer = {
                let accounting_peers = accounting.lock().await;
                accounting_peers.get(&selected.0).cloned()
            };
            if let Some(accounting_peer) = accounting_peer {
                cancel_reserve(&accounting_peer, selected.1).await;
            }
            async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
            continue;
        }

        retrieve_attempt(
            selected,
            caddr.clone(),
            control.clone(),
            accounting.clone(),
            refresh_chan.clone(),
            attempt_out.clone(),
        )
        .await;
    }

    if success_peers.len() < RETRIEVE_CHECK_CONFIRMATION_PEERS {
        retrieval_debug!(
            "unable to retrieve chunk {} from {} separate peers",
            hex::encode(chunk_address),
            RETRIEVE_CHECK_CONFIRMATION_PEERS
        );
        return vec![];
    }

    decode_retrieved_chunk(chunk_address, cd, soc, encrey, encred)
}

pub fn verify_chunk(caddr: &Vec<u8>, cd: &Vec<u8>) -> (bool, bool) {
    let contaddrd = valid_cac(&cd, &caddr);
    if !contaddrd {
        let soc = valid_soc(&cd, &caddr);
        if !soc {
            return (false, false);
        } else {
            return (true, true);
        }
    } else {
        return (true, false);
    }
}

pub fn decrypt(cd: &Vec<u8>, encrey: Vec<u8>) -> Vec<u8> {
    if cd.len() < 8 {
        return vec![];
    }

    let spancred = (&cd[0..8]).to_vec();
    let concred = (&cd[8..]).to_vec();
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

    let mut span_decrypted = u64::from_le_bytes(spanbytes.clone().try_into().unwrap());

    if span_decrypted > 4096 {
        let mut done0 = false;
        let mut carry_span = 4096_u64;
        while !done0 {
            let k = span_decrypted / carry_span;
            let mut l0 = span_decrypted % carry_span;
            if l0 > 0 {
                l0 = 1;
            }

            if k + l0 <= 64 {
                done0 = true;
                span_decrypted = (k + l0) * 64;
            } else {
                carry_span *= 64;
            }
        }
    };

    return [spanbytes, content[..span_decrypted as usize].to_vec()].concat();
}

pub async fn get_data(
    data_address: Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
    let _ = data_retrieve_chan.try_send((data_address, chan_out));

    return chan_in.recv().await.unwrap_or_default();
}

pub async fn get_chunk(
    data_address: Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Vec<u8> {
    let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
    let _ = chunk_retrieve_chan.try_send(chunk_retrieve_request(data_address, chan_out));

    return chan_in.recv().await.unwrap_or_default();
}

fn valid_feed_update_payload(data: &[u8]) -> bool {
    !data.is_empty()
}

fn feed_update_payload_shape(data: &[u8]) -> String {
    if data.len() < 8 {
        return "short".to_string();
    }
    let span = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
    let expected_inline_len = span
        .checked_add(8)
        .and_then(|len| usize::try_from(len).ok());

    if span <= 4096 {
        return format!(
            "inline span={} expected={} ok={}",
            span,
            expected_inline_len
                .map(|len| len.to_string())
                .unwrap_or_else(|| "overflow".to_string()),
            expected_inline_len == Some(data.len())
        );
    }

    let ref_payload_len = data.len().saturating_sub(8);
    format!(
        "refs span={} payload={} refs32={} refs64={}",
        span,
        ref_payload_len,
        ref_payload_len > 0 && ref_payload_len % 32 == 0,
        ref_payload_len > 0 && ref_payload_len % 64 == 0
    )
}

async fn probe_feed_update(
    owner: &String,
    topic: &String,
    index: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    let payload = get_chunk(get_feed_address(owner, topic, index), chunk_retrieve_chan).await;
    let found = valid_feed_update_payload(&payload);
    retrieve_trace!(
        "feed probe index={} found={} bytes={} shape={}",
        index,
        found,
        payload.len(),
        feed_update_payload_shape(&payload)
    );

    if found { Some(payload) } else { None }
}

async fn seek_feed_frontier(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> (Option<(u64, Vec<u8>)>, u64) {
    let Some(first_payload) = probe_feed_update(&owner, &topic, 0, chunk_retrieve_chan).await
    else {
        retrieve_trace!("feed frontier empty first_missing=0");
        return (None, 0);
    };

    let mut latest = (0, first_payload);
    let mut lower_missing_candidate = 1_u64;
    let mut upper_missing = 1_u64;

    loop {
        match probe_feed_update(&owner, &topic, upper_missing, chunk_retrieve_chan).await {
            Some(payload) => {
                latest = (upper_missing, payload);
                lower_missing_candidate = match upper_missing.checked_add(1) {
                    Some(next) => next,
                    None => return (Some(latest), u64::MAX),
                };
                upper_missing = match upper_missing.checked_mul(2) {
                    Some(next) if next >= lower_missing_candidate => next,
                    _ => u64::MAX,
                };

                if upper_missing == u64::MAX {
                    if let Some(payload) =
                        probe_feed_update(&owner, &topic, upper_missing, chunk_retrieve_chan).await
                    {
                        latest = (upper_missing, payload);
                        return (Some(latest), u64::MAX);
                    }
                    break;
                }
            }
            None => break,
        }
    }

    retrieve_trace!(
        "feed frontier bracket latest={} first_missing_candidate={} upper_missing={}",
        latest.0,
        lower_missing_candidate,
        upper_missing
    );

    let mut low = lower_missing_candidate;
    let mut high = upper_missing;

    while low < high {
        let mid = low + (high - low) / 2;
        retrieve_trace!("feed frontier binary low={} mid={} high={}", low, mid, high);

        if let Some(payload) = probe_feed_update(&owner, &topic, mid, chunk_retrieve_chan).await {
            latest = (mid, payload);
            low = match mid.checked_add(1) {
                Some(next) => next,
                None => return (Some(latest), u64::MAX),
            };
        } else {
            high = mid;
        }
    }

    retrieve_trace!("feed frontier exact latest={} next={}", latest.0, low);
    (Some(latest), low)
}

pub async fn seek_latest_feed_update(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    _redundancy: u8,
) -> Vec<u8> {
    match seek_feed_frontier(owner, topic, chunk_retrieve_chan).await {
        (Some((index, payload)), next_index) => {
            retrieve_trace!("feed latest exact index={} next={}", index, next_index);
            payload
        }
        (None, _) => vec![],
    }
}

pub async fn seek_next_feed_update_index(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    _redundancy: u8,
) -> u64 {
    let (_latest, next_index) = seek_feed_frontier(owner, topic, chunk_retrieve_chan).await;
    retrieve_trace!("feed next exact index={}", next_index);
    retrieval_debug!("EXPLICIT HEAD {}", next_index);
    next_index
}

//
// 166875e18d6754e468f231c8545322eaff22a0e3ec939fc25b296c4ce31dd654
//
