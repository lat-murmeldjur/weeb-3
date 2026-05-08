use crate::{
    // // // // // // // //
    ChunkRetrieveSender,
    // // // // // // // //
    Date,
    // // // // // // // //
    Duration,
    // // // // // // // //
    HashMap,
    // // // // // // // //
    HashSet,
    // // // // // // // //
    JsValue,
    // // // // // // // //
    Mutex,
    // // // // // // // //
    PeerAccounting,
    // // // // // // // //
    PeerId,
    // // // // // // // //
    RETRIEVE_CHECK_CONFIRMATION_PEERS,
    // // // // // // // //
    RetrieveCancelToken,
    // // // // // // // //
    RetrieveGenerationMap,
    // // // // // // // //
    apply_credit,
    // // // // // // // //
    cancel_reserve,
    // // // // // // // //
    chunk_retrieve_request,
    // // // // // // // //
    encode_resources,
    // // // // // // // //
    get_feed_address,
    // // // // // // // //
    get_proximity,
    // // // // // // // //
    manifest::interpret_manifest,
    // // // // // // // //
    mpsc,
    // // // // // // // //
    //    persistence::{cache_chunk, retrieve_cached_chunk},
    // // // // // // // //
    price,
    // // // // // // // //
    reserve,
    // // // // // // // //
    retrieve_cancel_token_current,
    // // // // // // // //
    retrieve_handler,
    // // // // // // // //
    stream,
    // // // // // // // //
    transfer_pause_enabled,
    // // // // // // // //
    valid_cac,
    // // // // // // // //
    valid_soc,
    // // // // // // // //
};

use alloy::primitives::keccak256;
use async_std::sync::Arc;
use byteorder::ByteOrder;
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::atomic::AtomicBool};

const RETRIEVE_PEER_TIMEOUT_SECS: u64 = 10;
const RETRIEVE_HEDGE_AFTER_MS: u64 = 760;
const RETRIEVE_DATA_DISPATCH_YIELD_EVERY: usize = 128;
const RETRIEVE_CHECK_RETRY_WAIT_MS: u64 = 160;

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

        web_sys::console::log_1(&JsValue::from(format!("marker 20")));

        for f in &data_vector {
            if f.data.len() > 8 {
                data_vector_e.push((f.data[8..].to_vec(), f.mime.clone(), f.path.clone()));
            };
        }
    }

    web_sys::console::log_1(&JsValue::from(format!("marker 21")));

    if data_vector_e.len() == 0 {
        web_sys::console::log_1(&JsValue::from(format!(
            "Unable to retrieve resource case 0"
        )));

        return encode_resources(
            vec![(vec![], "not found".to_string(), "not found".to_string())],
            index,
        );
    }

    web_sys::console::log_1(&JsValue::from(format!("marker 22")));

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

    let _ = async_std::future::timeout(
        Duration::from_secs(RETRIEVE_PEER_TIMEOUT_SECS),
        retrieve_handler(peer.clone(), caddr.clone(), control, &chunk_out),
    )
    .await;

    let result = RetrieveAttemptResult {
        peer: peer.clone(),
        chunk: vec![],
        valid: false,
        soc: false,
    };

    match chunk_in.try_recv() {
        Ok(chunk) => {
            let (chunk_valid, soc) = verify_chunk(&caddr, &chunk);
            if chunk_valid {
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
                let accounting_peer = {
                    let accounting_peers = accounting.lock().await;
                    accounting_peers.get(&peer).cloned()
                };
                if let Some(accounting_peer) = accounting_peer {
                    cancel_reserve(&accounting_peer, req_price).await
                }
            }
        }
        Err(error) => {
            let accounting_peer = {
                let accounting_peers = accounting.lock().await;
                accounting_peers.get(&peer).cloned()
            };
            if let Some(accounting_peer) = accounting_peer {
                cancel_reserve(&accounting_peer, req_price).await
            }
            web_sys::console::log_1(&JsValue::from(format!(
                "unable to retrieve chunk {} error {}",
                hex::encode(&caddr),
                error
            )));
        }
    }

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
                web_sys::console::log_1(&JsValue::from(format!(
                    "unable to retrieve chunk {} - encrypted chunk no content",
                    hex::encode(chunk_address)
                )));
                return vec![];
            }

            let cd00 = decrypt(&(&cd[97..]).to_vec(), encrey);
            if cd00.len() >= 8 {
                return cd00;
            } else {
                web_sys::console::log_1(&JsValue::from(format!(
                    "unable to retrieve chunk {} - encrypted chunk no content",
                    hex::encode(chunk_address)
                )));
                return vec![];
            }
        }

        return decrypt(&cd, encrey);
    }

    if soc && cd.len() >= 97 + 8 {
        return (&cd[97..]).to_vec();
    }
    if cd.len() == 0 {
        web_sys::console::log_1(&JsValue::from(format!(
            "unable to retrieve chunk {} - chunk empty",
            hex::encode(chunk_address)
        )));
    }

    cd
}

pub async fn retrieve_data(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Vec<u8> {
    let root_chunk = get_chunk(data_address.to_vec(), &chunk_retrieve_chan).await;

    let root_span: u64;
    if root_chunk.len() >= 8 {
        root_span = u64::from_le_bytes(root_chunk[0..8].try_into().unwrap());
    } else {
        web_sys::console::log_1(&JsValue::from(format!(
            "chunk not found: {}",
            hex::encode(data_address),
        )));
        return vec![];
    }

    if root_span <= 4096 {
        if (root_span + 8) as usize == root_chunk.len() {
            return root_chunk;
        } else {
            web_sys::console::log_1(&JsValue::from(format!(
                "retrieved chunk length ({}) mismatching span ({}) + 8 for chunk {}",
                root_chunk.len(),
                root_span,
                hex::encode(data_address),
            )));
            return vec![];
        }
    }

    let address_length = data_address.len();

    if root_chunk.len() < 8 + address_length || (root_chunk.len() - 8) % address_length != 0 {
        web_sys::console::log_1(&JsValue::from(format!(
            "chunk too short: {}",
            hex::encode(data_address),
        )));
        return vec![];
    }

    let mut data: Vec<u8> = Vec::with_capacity((root_span + 8) as usize);
    data.extend_from_slice(&root_chunk[0..8]);

    let root_refs = match split_chunk_references(&root_chunk[8..], address_length) {
        Some(addresses) => addresses,
        None => return vec![],
    };

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
            web_sys::console::log_1(&JsValue::from(format!(
                "chunk not found: {}",
                hex::encode(addr),
            )));
            return vec![];
        }

        let result_span = u64::from_le_bytes(result0[0..8].try_into().unwrap());

        if result_span > 4096 {
            let child_refs = match split_chunk_references(&result0[8..], address_length) {
                Some(addresses) => addresses,
                None => return vec![],
            };

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
    }

    for (_order, chunk_data) in leaf_chunks {
        if chunk_data.is_empty() {
            return vec![];
        }
        data.extend_from_slice(&chunk_data);
    }

    if data.len() == (root_span + 8) as usize {
        data
    } else {
        web_sys::console::log_1(&JsValue::from(format!(
            "retrieved result length ({}) not matching span ({}) + 8 for data address {}",
            data.len(),
            root_span,
            hex::encode(data_address),
        )));
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

    let mut error_count = 0;
    let max_error = 20;

    #[allow(unused_assignments)]
    let mut cd = vec![];

    // cd = retrieve_cached_chunk(&caddr).await;
    // if cd.len() > 0 {
    //     (chunk_valid, soc) = verify_chunk(&caddr, &cd);
    //     if chunk_valid {
    //         error_count = max_error;
    //     } else {
    //         cd = vec![];
    //     };
    // };

    let (attempt_out, attempt_in) = mpsc::unbounded::<RetrieveAttemptResult>();
    let mut in_flight = 0_usize;
    let mut last_attempt_started = 0.0;

    while error_count < max_error {
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
        let due = !paused
            && !cancelled
            && (in_flight == 0 || now - last_attempt_started >= RETRIEVE_HEDGE_AFTER_MS as f64);

        if due {
            reset_overdraft(&mut skiplist, &mut overdraftlist);
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
                    if in_flight == 0 {
                        break;
                    }
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
                in_flight += 1;
                last_attempt_started = Date::now();
            } else if overdraftlist.is_empty() {
                async_std::task::sleep(Duration::from_millis(50)).await;
                continue;
            } else {
                reset_overdraft(&mut skiplist, &mut overdraftlist);
                async_std::task::sleep(Duration::from_millis(50)).await;
                continue;
            }
        }

        if in_flight == 0 {
            continue;
        }

        let elapsed = Date::now() - last_attempt_started;
        let wait_ms = if cancelled || paused {
            250
        } else {
            (RETRIEVE_HEDGE_AFTER_MS as f64 - elapsed).max(0.0).round() as u64
        };
        if wait_ms == 0 {
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
    let max_error = 19;

    #[allow(unused_assignments)]
    let mut cd = vec![];
    let mut soc = false;

    while error_count < max_error && success_peers.len() < RETRIEVE_CHECK_CONFIRMATION_PEERS {
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
            error_count += 1;
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
            continue;
        }

        let (attempt_out, attempt_in) = mpsc::unbounded::<RetrieveAttemptResult>();
        retrieve_attempt(
            selected,
            caddr.clone(),
            control.clone(),
            accounting.clone(),
            refresh_chan.clone(),
            attempt_out,
        )
        .await;

        match attempt_in.recv().await {
            Ok(result) if result.valid => {
                if success_peers.insert(result.peer) && cd.len() == 0 {
                    cd = result.chunk;
                    soc = result.soc;
                }
            }
            _ => {
                error_count += 1;
            }
        }
    }

    if success_peers.len() < RETRIEVE_CHECK_CONFIRMATION_PEERS {
        web_sys::console::log_1(&JsValue::from(format!(
            "unable to retrieve chunk {} from {} separate peers",
            hex::encode(chunk_address),
            RETRIEVE_CHECK_CONFIRMATION_PEERS
        )));
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

pub async fn seek_latest_feed_update(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    redundancy: u8,
) -> Vec<u8> {
    let mut largest_found = 0;
    let mut smallest_not_found = u64::MAX;
    let mut lower_bound = 0;
    let mut upper_bound = 2_u64.pow(redundancy.into());
    let mut _exact_ = false;

    while !_exact_ {
        async_std::task::yield_now().await;
        async_std::task::sleep(Duration::from_millis(50)).await;

        let angle = upper_bound - lower_bound;
        let mut joiner = FuturesUnordered::new(); // ::<dyn Future<Output = Vec<u8>>> // ::<Pin<Box<dyn Future<Output = (Vec<u8>, usize)>>>>

        let mut i = 0;

        // dispatch probes

        while lower_bound + i <= upper_bound {
            let j = lower_bound + i;
            let feed_update_address = get_feed_address(&owner, &topic, j);
            let handle = async move {
                return (
                    get_chunk(feed_update_address, &chunk_retrieve_chan).await,
                    j,
                );
            };
            joiner.push(handle);

            if i == 0 || angle <= (redundancy as u64) {
                i += 1;
            } else {
                i *= 2;
            }
        }

        // receive results, update scores

        while let Some((result0, result1)) = joiner.next().await {
            if result0.len() == 0 && smallest_not_found > result1 {
                smallest_not_found = result1;
            }
            if result0.len() > 0 && largest_found < result1 {
                largest_found = result1;
            }
        }

        // if _exact_ frontier found return corresponding data

        if largest_found + 1 == smallest_not_found {
            return get_chunk(
                get_feed_address(&owner, &topic, largest_found),
                &chunk_retrieve_chan,
            )
            .await;
        }

        // search above previous record height

        lower_bound = largest_found + 1;

        // if smallest not found update was higher than current zone lower bound, narrow search between these values

        if smallest_not_found > lower_bound {
            upper_bound = smallest_not_found;
        } else {
            // exit if largest found stayed zero and smallest not found is also zero

            if smallest_not_found == 0 && largest_found == 0 {
                return vec![];
            }

            // if we had a missing update below the record found height, discard hole and start from scratch regarding potential height

            smallest_not_found = u64::MAX;

            // set upper bound to search redundancy based limit

            upper_bound = lower_bound + 2_u64.pow(redundancy.into());
        }
    }

    return vec![];
}

pub async fn seek_next_feed_update_index(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    redundancy: u8,
) -> u64 {
    let mut largest_found = 0;
    let mut smallest_not_found = u64::MAX;
    let mut lower_bound = 0;
    let mut upper_bound = 2_u64.pow(redundancy.into());
    let mut _exact_ = false;

    while !_exact_ {
        async_std::task::yield_now().await;
        async_std::task::sleep(Duration::from_millis(50)).await;

        let angle = upper_bound - lower_bound;
        let mut joiner = FuturesUnordered::new(); // ::<dyn Future<Output = Vec<u8>>> // ::<Pin<Box<dyn Future<Output = (Vec<u8>, usize)>>>>

        let mut i = 0;

        // dispatch probes

        while lower_bound + i <= upper_bound {
            let j = lower_bound + i;
            let feed_update_address = get_feed_address(&owner, &topic, j);
            let handle = async move {
                return (
                    get_chunk(feed_update_address, &chunk_retrieve_chan).await,
                    j,
                );
            };
            joiner.push(handle);

            if i == 0 || angle <= (redundancy as u64) {
                i += 1;
            } else {
                i *= 2;
            }
        }

        // receive results, update scores

        while let Some((result0, result1)) = joiner.next().await {
            if result0.len() == 0 && smallest_not_found > result1 {
                smallest_not_found = result1;
            }
            if result0.len() > 0 && largest_found < result1 {
                largest_found = result1;
            }
        }

        // if _exact_ frontier found return corresponding data

        if largest_found + 1 == smallest_not_found {
            web_sys::console::log_1(&JsValue::from(format!(
                "EXPLICIT HEAD {}",
                smallest_not_found
            )));

            return smallest_not_found;
        }

        // search above previous record height

        lower_bound = largest_found + 1;

        // if smallest not found update was higher than current zone lower bound, narrow search between these values

        if smallest_not_found > lower_bound {
            upper_bound = smallest_not_found;
        } else {
            // exit if largest found stayed zero and smallest not found is also zero

            if smallest_not_found == 0 && largest_found == 0 {
                return 0;
            }

            // if we had a missing update below the record found height, discard hole and start from scratch regarding potential height

            smallest_not_found = u64::MAX;

            // set upper bound to search redundancy based limit

            upper_bound = lower_bound + 2_u64.pow(redundancy.into());
        }
    }

    return 0;
}

//
// 166875e18d6754e468f231c8545322eaff22a0e3ec939fc25b296c4ce31dd654
//
