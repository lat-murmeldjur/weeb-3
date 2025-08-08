use crate::{
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
    PROTOCOL_ROUND_TIME,
    // // // // // // // //
    // // // // // // // //
    PeerAccounting,
    // // // // // // // //
    PeerId,
    // // // // // // // //
    apply_credit,
    // // // // // // // //
    cancel_reserve,
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
    persistence::{cache_chunk, retrieve_cached_chunk},
    // // // // // // // //
    price,
    // // // // // // // //
    reserve,
    // // // // // // // //
    retrieve_handler,
    // // // // // // // //
    stream,
    // // // // // // // //
    valid_cac,
    // // // // // // // //
    valid_soc,
};

use alloy::primitives::keccak256;
use async_std::sync::Arc;
use byteorder::ByteOrder;

use libp2p::futures::{StreamExt, stream::FuturesUnordered};

pub async fn retrieve_resource(
    chunk_address: &Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let cd = get_data(chunk_address.to_vec(), data_retrieve_chan).await;

    let mut data_vector_e: Vec<(Vec<u8>, String, String)> = vec![];

    #[allow(unused_assignments)]
    let mut index: String = "".to_string();

    {
        let (data_vector, index0) =
            interpret_manifest("".to_string(), &cd, data_retrieve_chan, chunk_retrieve_chan).await;

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

pub async fn retrieve_data(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let root_chunk = get_chunk(data_address.to_vec(), chunk_retrieve_chan).await;

    #[allow(unused_assignments)]
    let mut root_span: u64 = 0;

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

    if root_chunk.len() < 8 + address_length {
        web_sys::console::log_1(&JsValue::from(format!(
            "chunk too short: {}",
            hex::encode(data_address),
        )));
        return vec![];
    }

    let mut orig = root_chunk[8..].to_vec();

    let mut data: Vec<u8> = Vec::with_capacity((root_span + 8) as usize);
    data.append(&mut root_chunk[0..8].to_vec());

    let mut done = false;
    while !done {
        if orig.len() % address_length != 0 {
            return vec![];
        }

        let subs = orig.len() / address_length;
        let mut content_holder_2: Vec<Vec<u8>> = vec![];
        for i in 0..subs {
            content_holder_2.push((&orig[i * address_length..(i + 1) * address_length]).to_vec());
        }

        let mut joiner = FuturesUnordered::new();

        done = true;
        let mut waves_done = false;
        let mut i = 0;
        let mut content_holder_3: HashMap<usize, Vec<u8>> = HashMap::new();
        let mut content_holder_4: HashMap<usize, Vec<u8>> = HashMap::new();

        while !waves_done {
            for j in 0..128 {
                if i + j >= content_holder_2.len() {
                    waves_done = true;
                    break;
                }

                let addr = &content_holder_2[i + j];
                let index = i + j;
                let handle = async move {
                    return (
                        get_chunk(addr.clone(), chunk_retrieve_chan).await,
                        index.clone(),
                        addr.clone(),
                    );
                };
                joiner.push(handle);
            }

            while let Some((result0, result1, result2)) = joiner.next().await {
                if result0.len() > 8 {
                    let result_span = u64::from_le_bytes(result0[0..8].try_into().unwrap());
                    if result_span > 4096 {
                        done = false;
                        content_holder_3.insert(result1, result0[8..].to_vec());
                    } else {
                        content_holder_3.insert(result1, result2);
                        content_holder_4.insert(result1, result0[8..].to_vec());
                    }
                } else {
                    web_sys::console::log_1(&JsValue::from(format!(
                        "chunk not found: {}",
                        hex::encode(result2),
                    )));
                    return vec![];
                }
            }

            i = i + 128;
        }

        if !done {
            web_sys::console::log_1(&JsValue::from(format!("marker 00")));
            orig = Vec::new();
            for i in 0..subs {
                match content_holder_3.get(&i) {
                    Some(data0) => {
                        if data0.len() > 0 {
                            orig.append(&mut data0[..].to_vec());
                        } else {
                            return vec![];
                        }
                    }
                    None => return vec![],
                }
            }
            web_sys::console::log_1(&JsValue::from(format!("marker 0")));
        } else {
            web_sys::console::log_1(&JsValue::from(format!("marker 10")));
            for i in 0..subs {
                match content_holder_4.get(&i) {
                    Some(data0) => {
                        if data0.len() > 0 {
                            data.append(&mut data0[..].to_vec());
                        } else {
                            return vec![];
                        }
                    }
                    None => return vec![],
                }
            }
            web_sys::console::log_1(&JsValue::from(format!("marker 11")));
        }
    }

    if data.len() == (root_span + 8) as usize {
        return data;
    } else {
        web_sys::console::log_1(&JsValue::from(format!(
            "retrieved result length ({}) not matching span ({}) + 8 for data address {}",
            data.len(),
            root_span,
            hex::encode(data_address),
        )));
        return vec![];
    }
}

pub async fn retrieve_chunk(
    chunk_address: &Vec<u8>,
    control: stream::Control,
    peers: &Arc<Mutex<HashMap<String, PeerId>>>,
    accounting: &Arc<Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
) -> Vec<u8> {
    let mut caddr: Vec<u8> = chunk_address.to_vec();
    let mut encrey = vec![];
    let mut encred = false;

    if chunk_address.len() == 64 {
        caddr = (&chunk_address[0..32]).to_vec();
        encrey = (&chunk_address[32..64]).to_vec();
        encred = true;
    }

    #[allow(unused_assignments)]
    let mut chunk_valid = false;
    let mut soc = false;
    let mut skiplist: HashSet<PeerId> = HashSet::new();
    let mut overdraftlist: HashSet<PeerId> = HashSet::new();

    let mut closest_overlay = "".to_string();
    let mut closest_peer_id = libp2p::PeerId::random();

    #[allow(unused_assignments)]
    let mut selected = false;
    let mut round_commence = Date::now();

    #[allow(unused_assignments)]
    let mut current_max_po = 0;

    let mut error_count = 0;
    let mut max_error = 8;

    #[allow(unused_assignments)]
    let mut cd = vec![];

    cd = retrieve_cached_chunk(&caddr).await;
    if cd.len() > 0 {
        (chunk_valid, soc) = verify_chunk(&caddr, &cd);
        if chunk_valid {
            error_count = max_error;
        } else {
            cd = vec![];
        };
    };

    while error_count < max_error {
        let mut seer = true;

        while seer {
            closest_overlay = "".to_string();
            closest_peer_id = libp2p::PeerId::random();
            current_max_po = 0;
            selected = false;
            {
                let peers_map = peers.lock().unwrap();
                for (ov, id) in peers_map.iter() {
                    if skiplist.contains(id) {
                        continue;
                    }

                    let current_po = get_proximity(&caddr, &hex::decode(&ov).unwrap());

                    if current_po >= current_max_po || selected == false {
                        selected = true;
                        closest_overlay = ov.clone();
                        closest_peer_id = id.clone();
                        current_max_po = current_po;
                    }
                }
            }
            if selected {
                skiplist.insert(closest_peer_id);
            } else {
                if overdraftlist.is_empty() {
                    web_sys::console::log_1(&JsValue::from(format!(
                        "unable to retrieve chunk {} - no more peers to try",
                        hex::encode(chunk_address)
                    )));
                    return vec![];
                } else {
                    for k in overdraftlist.iter() {
                        let _ =
                            refresh_chan.send((k.clone(), 10 * crate::accounting::REFRESH_RATE));
                        skiplist.remove(k);
                    }
                    overdraftlist.clear();

                    let round_now = Date::now();

                    let seg = round_now - round_commence;
                    if seg < PROTOCOL_ROUND_TIME {
                        async_std::task::sleep(Duration::from_millis(
                            (PROTOCOL_ROUND_TIME - seg) as u64,
                        ))
                        .await;
                    }

                    round_commence = Date::now();

                    continue;
                }
            }

            let req_price = price(&closest_overlay, &caddr);

            {
                let accounting_peers = accounting.lock().unwrap();
                if max_error > accounting_peers.len() {
                    max_error = accounting_peers.len();
                };
                if accounting_peers.contains_key(&closest_peer_id) {
                    let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                    let allowed = reserve(accounting_peer, req_price, refresh_chan);
                    if !allowed {
                        overdraftlist.insert(closest_peer_id);
                    } else {
                        seer = false;
                    }
                }
            }
        }

        let req_price = price(&closest_overlay, &caddr);

        let (chunk_out, chunk_in) = mpsc::channel::<Vec<u8>>();

        retrieve_handler(closest_peer_id, caddr.clone(), control.clone(), &chunk_out).await;

        let chunk_data = chunk_in.try_recv();
        if chunk_data.is_err() {
            let accounting_peers = accounting.lock().unwrap();
            if accounting_peers.contains_key(&closest_peer_id) {
                let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                cancel_reserve(accounting_peer, req_price)
            }
        }

        cd = match chunk_data {
            Ok(ref x) => x.clone(),
            Err(_x) => {
                error_count += 1;
                let accounting_peers = accounting.lock().unwrap();
                if accounting_peers.contains_key(&closest_peer_id) {
                    let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                    cancel_reserve(accounting_peer, req_price)
                }
                web_sys::console::log_1(&JsValue::from(format!(
                    "unable to retrieve chunk {} error",
                    hex::encode(chunk_address)
                )));
                vec![]
            }
        };

        // chan send?

        match chunk_data {
            Ok(_x) => {
                (chunk_valid, soc) = verify_chunk(&caddr, &cd);
                if chunk_valid {
                    {
                        let accounting_peers = accounting.lock().unwrap();
                        if accounting_peers.contains_key(&closest_peer_id) {
                            let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                            apply_credit(accounting_peer, req_price);
                        }
                    }
                    cache_chunk(&caddr, &cd).await;
                    break;
                } else {
                    error_count += 1;
                    {
                        let accounting_peers = accounting.lock().unwrap();
                        if accounting_peers.contains_key(&closest_peer_id) {
                            let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                            cancel_reserve(accounting_peer, req_price)
                        }
                    }
                    cd = vec![];
                }
            }
            _ => {}
        };
    }

    if encred {
        if soc {
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

        let cd0 = decrypt(&cd, encrey);
        return cd0;
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
    return cd;
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
    let (chan_out, chan_in) = mpsc::channel::<Vec<u8>>();
    data_retrieve_chan.send((data_address, chan_out)).unwrap();

    let k0 = async {
        let mut timelast: f64;
        #[allow(irrefutable_let_patterns)]
        while let that = chan_in.try_recv() {
            timelast = Date::now();
            if !that.is_err() {
                return that.unwrap();
            }

            let timenow = Date::now();
            let seg = timenow - timelast;
            if seg < PROTOCOL_ROUND_TIME {
                async_std::task::sleep(Duration::from_millis((PROTOCOL_ROUND_TIME - seg) as u64))
                    .await;
            };
        }

        return vec![];
    };

    let result = k0.await;

    return result;
}

pub async fn get_chunk(
    data_address: Vec<u8>,
    chunk_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let (chan_out, chan_in) = mpsc::channel::<Vec<u8>>();
    chunk_retrieve_chan.send((data_address, chan_out)).unwrap();

    let k0 = async {
        let mut timelast: f64;
        #[allow(irrefutable_let_patterns)]
        while let that = chan_in.try_recv() {
            timelast = Date::now();
            if !that.is_err() {
                return that.unwrap();
            }

            let timenow = Date::now();
            let seg = timenow - timelast;
            if seg < PROTOCOL_ROUND_TIME {
                async_std::task::sleep(Duration::from_millis((PROTOCOL_ROUND_TIME - seg) as u64))
                    .await;
            };
        }

        return vec![];
    };

    let result = k0.await;

    return result;
}

pub async fn seek_latest_feed_update(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
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
                return (get_chunk(feed_update_address, chunk_retrieve_chan).await, j);
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
                chunk_retrieve_chan,
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
    chunk_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
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
                return (get_chunk(feed_update_address, chunk_retrieve_chan).await, j);
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
