#![allow(warnings)]

use crate::{
    apply_credit,
    // // // // // // // //
    cancel_reserve,
    // // // // // // // //
    encode_resource,
    // // // // // // // //
    get_proximity,
    // // // // // // // //
    join_all,
    // // // // // // // //
    mpsc,
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
    // // // // // // // //
    Date,
    // // // // // // // //
    Duration,
    // // // // // // // //
    HashMap,
    // // // // // // // //
    JsValue,
    // // // // // // // //
    Mutex,
    // // // // // // // //
    PeerAccounting,
    // // // // // // // //
    PeerId,
    // // // // // // // //
    RETRIEVE_ROUND_TIME,
    // // // // // // // //
};

use async_std::task;
use libp2p::futures::{stream::FuturesUnordered, Future, StreamExt};
use serde_json::Value;
use std::pin::Pin;

pub async fn retrieve_resource(
    chunk_address: &Vec<u8>,
    control: &mut stream::Control,
    peers: &Mutex<HashMap<String, PeerId>>,
    accounting: &Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
    chunk_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let cd = retrieve_data(
        chunk_address,
        control,
        peers,
        accounting,
        refresh_chan,
        chunk_retrieve_chan,
    )
    .await;

    let obfuscation_key = &cd[8..40];
    let mf_version = &cd[40..71];

    let ref_size = &cd[71];

    let ref_delimiter = (72 + ref_size) as usize;
    let actual_reference = &cd[72..ref_delimiter];
    web_sys::console::log_1(&JsValue::from(format!(
        "actual_reference: {:#?}",
        actual_reference
    )));

    let index_delimiter = (ref_delimiter + 32) as usize;
    let index = &cd[ref_delimiter..index_delimiter];
    web_sys::console::log_1(&JsValue::from(format!("index: {:x?}", index)));

    // fork parts

    let mut fork_start_current = index_delimiter;

    let fork_type = &cd[fork_start_current];

    let fork_prefix_length = &cd[fork_start_current + 1];
    let fork_prefix_delimiter = fork_start_current + 2 + 30;
    let fork_prefix = &cd[fork_start_current + 2..fork_prefix_delimiter];

    let fork_reference = &cd[fork_prefix_delimiter..fork_prefix_delimiter + 32];
    web_sys::console::log_1(&JsValue::from(format!(
        "fork_reference: {:#?}",
        fork_reference
    )));

    //    let metadata_q0 = retrieve_data(
    //        &fork_reference.to_vec(),
    //        control,
    //        peers,
    //        accounting,
    //        refresh_chan,
    //        chunk_retrieve_chan,
    //    )
    //    .await;

    let fork_metadata_bytesize: [u8; 2] = cd
        [fork_prefix_delimiter + 32..fork_prefix_delimiter + 34]
        .try_into()
        .unwrap();

    let calc_metadata_bytesize = u16::from_be_bytes(fork_metadata_bytesize) as usize;
    let fork_metadata_delimiter = fork_prefix_delimiter + 34 + calc_metadata_bytesize;

    let fork_metadata = &cd[fork_prefix_delimiter + 34..fork_metadata_delimiter];

    let v0: Value = serde_json::from_slice(fork_metadata).unwrap_or("nil".into());

    web_sys::console::log_1(&JsValue::from(format!("chunk content deser: {:#?} ", v0)));
    let str0 = v0.get("website-index-document").unwrap().as_str().unwrap();
    web_sys::console::log_1(&JsValue::from(format!("index document: {:#?} ", str0)));

    let data_address = hex::decode(str0).unwrap();
    web_sys::console::log_1(&JsValue::from(format!(
        "data address: {:#?} ",
        data_address
    )));

    let data = retrieve_data(
        &data_address,
        control,
        peers,
        accounting,
        refresh_chan,
        chunk_retrieve_chan,
    )
    .await;

    return encode_resource(data, str0.to_string());
}

pub async fn retrieve_data(
    chunk_address: &Vec<u8>,
    control: &mut stream::Control,
    peers: &Mutex<HashMap<String, PeerId>>,
    accounting: &Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
    chunk_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let orig = retrieve_chunk(chunk_address, control, peers, accounting, refresh_chan).await;
    let span = u64::from_le_bytes(orig[0..8].try_into().unwrap_or([0; 8]));
    if span <= 4096 {
        return orig;
    }

    if (orig.len() - 8) % 32 != 0 {
        return vec![];
    }

    // task::yield_now().await;

    // let mut data: Vec<u8> = vec![0; span as usize];
    let mut joiner = FuturesUnordered::new(); // ::<dyn Future<Output = Vec<u8>>> // ::<Pin<Box<dyn Future<Output = (Vec<u8>, usize)>>>>

    let subs = (orig.len() - 8) / 32;

    let mut content_holder_2: Vec<Vec<u8>> = vec![];

    for i in 0..subs {
        content_holder_2.push((&orig[8 + i * 32..8 + (i + 1) * 32]).to_vec());
    }

    for (i, addr) in content_holder_2.iter().enumerate() {
        let index = i;
        let address = addr.clone();
        let mut ctrl = control.clone();
        let handle = async move {
            return (
                retrieve_data(
                    &address,
                    &mut ctrl,
                    peers,
                    accounting,
                    refresh_chan,
                    chunk_retrieve_chan,
                )
                .await,
                index.clone(),
            );
        };
        joiner.push(handle);
    }

    let mut content_holder_3: HashMap<usize, Vec<u8>> = HashMap::new();

    while let Some((result0, result1)) = joiner.next().await {
        content_holder_3.insert(result1, result0);
    }

    // let results: Vec<(Vec<u8>, usize)> = joiner.collect().await;
    // for result in results.iter() {
    //     content_holder_3.insert(result.1, result.0);
    // }

    let mut data: Vec<u8> = Vec::new();
    for i in 0..subs {
        match content_holder_3.get(&i) {
            Some(mut data0) => {
                let mut data1 = data0.clone();
                data.append(&mut data1);
            }
            None => return vec![],
        }
    }

    return data;
}

pub async fn retrieve_chunk(
    chunk_address: &Vec<u8>,
    control: &mut stream::Control,
    peers: &Mutex<HashMap<String, PeerId>>,
    accounting: &Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
) -> Vec<u8> {
    let timestart = Date::now();
    let mut skiplist: HashMap<PeerId, _> = HashMap::new();
    let mut overdraftlist: HashMap<PeerId, _> = HashMap::new();

    let mut closest_overlay = "".to_string();
    let mut closest_peer_id = libp2p::PeerId::random();

    let mut selected = false;
    let mut round_commence = Date::now();
    let mut current_max_po = 0;
    let mut error_count = 0;
    let mut max_error = 8;

    let mut cd = vec![];

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
                    if skiplist.contains_key(id) {
                        continue;
                    }
                    let current_po = get_proximity(&chunk_address, &hex::decode(&ov).unwrap());

                    if current_po >= current_max_po {
                        selected = true;
                        closest_overlay = ov.clone();
                        closest_peer_id = id.clone();
                        current_max_po = current_po;
                    }
                }
            }
            if selected {
                skiplist.insert(closest_peer_id, "");
                web_sys::console::log_1(&JsValue::from(format!(
                    "Selected peer {:#?}!",
                    closest_peer_id
                )));
            } else {
                if overdraftlist.is_empty() {
                    return vec![];
                } else {
                    for (k, _v) in overdraftlist.iter() {
                        let _ =
                            refresh_chan.send((k.clone(), 10 * crate::accounting::REFRESH_RATE));
                        skiplist.remove(k);
                    }
                    overdraftlist.clear();

                    let round_now = Date::now();

                    let seg = round_now - round_commence;
                    if seg < RETRIEVE_ROUND_TIME {
                        web_sys::console::log_1(&JsValue::from(format!(
                            "Ease retrieve overdraft retries loop for {}",
                            RETRIEVE_ROUND_TIME - seg
                        )));
                        async_std::task::sleep(Duration::from_millis(
                            (RETRIEVE_ROUND_TIME - seg) as u64,
                        ))
                        .await;
                    }

                    round_commence = Date::now();

                    continue;
                }
            }

            let req_price = price(&closest_overlay, &chunk_address);

            web_sys::console::log_1(&JsValue::from(format!(
                "Attempt to reserve price {:#?} for chunk {:#?} from peer {:#?}!",
                req_price, chunk_address, closest_peer_id
            )));

            {
                let accounting_peers = accounting.lock().unwrap();
                if max_error > accounting_peers.len() {
                    max_error = accounting_peers.len();
                };
                if accounting_peers.contains_key(&closest_peer_id) {
                    let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                    let allowed = reserve(accounting_peer, req_price, refresh_chan);
                    if !allowed {
                        web_sys::console::log_1(&JsValue::from(format!(
                            "Overdraft for peer {}",
                            closest_peer_id
                        )));
                        overdraftlist.insert(closest_peer_id, "");
                    } else {
                        web_sys::console::log_1(&JsValue::from(format!(
                            "Selected peer with successful reserve {}!",
                            closest_peer_id
                        )));
                        seer = false;
                    }
                }
            }
        }

        let req_price = price(&closest_overlay, &chunk_address);

        let (chunk_out, chunk_in) = mpsc::channel::<Vec<u8>>();

        web_sys::console::log_1(&JsValue::from(format!(
            "Actually retrieving for peer {}!",
            closest_peer_id
        )));

        retrieve_handler(closest_peer_id, chunk_address.clone(), control, &chunk_out).await;

        let chunk_data = chunk_in.try_recv();
        if !chunk_data.is_err() {
            let accounting_peers = accounting.lock().unwrap();
            if accounting_peers.contains_key(&closest_peer_id) {
                let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                apply_credit(accounting_peer, req_price);
            }
        } else {
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
                vec![]
            }
        };

        // chan send?

        match chunk_data {
            Ok(_x) => {
                let contaddrd = valid_cac(&cd, chunk_address);
                if !contaddrd {
                    let socd = valid_soc(&cd, chunk_address);
                    if !socd {
                        error_count += 1;
                        cd = vec![];
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            _ => {}
        };
    }

    if cd.len() > 0 {
        web_sys::console::log_1(&JsValue::from(format!(
            "Successfully retrieved chunk from peer {:#?}!",
            closest_peer_id
        )));
    }

    let timeend = Date::now();

    web_sys::console::log_1(&JsValue::from(format!(
        "Retrieve time duration {} ms!",
        timeend - timestart
    )));

    return cd;
}

// 3ab408eea4f095bde55c1caeeac8e7fcff49477660f0a28f652f0a6d9c60d05f
// ef30a6c57b0c14d6dc7d7e035b41a88cd48440a50e920eaefa3e1620da11eca8
// 07f7a2e36a1e481de0da16f5e0647a1a11cf6a6c6fcaf89d367a7d63dbbbc8e7 ( d61aa6bbb728ab89f427d4c01d455845f44ef188fb701681b35a918fdc19a19f )
// 6dd3f101738f58d3e51f1c914723a226e6180538fed7f1f6bf10089de834e82e ( d213da296b93456148b5a971adb9e8d571daf77a6b6f5c3b997198587ca35960 )
