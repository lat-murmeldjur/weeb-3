use crate::{
    apply_credit,
    // // // // // // // //
    cancel_reserve,
    // // // // // // // //
    encode_resources,
    // // // // // // // //
    get_proximity,
    // // // // // // // //
    manifest::interpret_manifest,
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
    HashSet,
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

use libp2p::futures::{stream::FuturesUnordered, StreamExt};

pub async fn retrieve_resource(
    chunk_address: &Vec<u8>,
    control: &mut stream::Control,
    peers: &Mutex<HashMap<String, PeerId>>,
    accounting: &Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let cd = retrieve_data(
        chunk_address,
        control,
        peers,
        accounting,
        refresh_chan,
        //
    )
    .await;

    let (data_vector, index) = interpret_manifest("".to_string(), &cd, data_retrieve_chan).await;
    let mut data_vector_e: Vec<(Vec<u8>, String, String)> = vec![];

    for f in &data_vector {
        if f.data.len() > 8 {
            data_vector_e.push((f.data[8..].to_vec(), f.mime.clone(), f.path.clone()));
        };
    }

    if data_vector_e.len() == 0 {
        return encode_resources(
            vec![(vec![], "not found".to_string(), "not found".to_string())],
            index,
        );
    }

    return encode_resources(data_vector_e, index);
}

pub async fn retrieve_data(
    chunk_address: &Vec<u8>,
    control: &mut stream::Control,
    peers: &Mutex<HashMap<String, PeerId>>,
    accounting: &Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
    // chunk_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let orig = retrieve_chunk(chunk_address, control, peers, accounting, refresh_chan).await;
    if orig.len() < 8 {
        return vec![];
    }

    let span = u64::from_le_bytes(orig[0..8].try_into().unwrap_or([0; 8]));
    if span <= 4096 {
        return orig;
    }

    if (orig.len() - 8) % 32 != 0 {
        return vec![];
    }

    // task::yield_now().await;

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
                    // chunk_retrieve_chan,
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
    data.append(&mut orig[0..8].to_vec());
    for i in 0..subs {
        match content_holder_3.get(&i) {
            Some(data0) => {
                if data0.len() > 0 {
                    data.append(&mut data0[8..].to_vec());
                } else {
                    return vec![];
                }
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
                    if skiplist.contains(id) {
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
                skiplist.insert(closest_peer_id);
            } else {
                if overdraftlist.is_empty() {
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
                    if seg < RETRIEVE_ROUND_TIME {
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

        let req_price = price(&closest_overlay, &chunk_address);

        let (chunk_out, chunk_in) = mpsc::channel::<Vec<u8>>();

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

    return cd;
}

//
//
//
// 3ab408eea4f095bde55c1caeeac8e7fcff49477660f0a28f652f0a6d9c60d05f
// ef30a6c57b0c14d6dc7d7e035b41a88cd48440a50e920eaefa3e1620da11eca8
// 07f7a2e36a1e481de0da16f5e0647a1a11cf6a6c6fcaf89d367a7d63dbbbc8e7 ( d61aa6bbb728ab89f427d4c01d455845f44ef188fb701681b35a918fdc19a19f )
// 6dd3f101738f58d3e51f1c914723a226e6180538fed7f1f6bf10089de834e82e ( d213da296b93456148b5a971adb9e8d571daf77a6b6f5c3b997198587ca35960 )
// 908fb0f1f4b1a173f422bdbf35e9cc9ba0dae0799ff688978c6077df7ad57f54
// 595f0537cebc3d0ea0d145d19297ae793d9b01ab560d07f6583b8b9dc39cecb3
// 9540c03a36fbacb12a8fdb3ab1fbda7e43958bef44fb965bca5521053d7dfd89
// b255e98a86f783f612ed8ccae2701a58421960a745e73356bb94aec7fe4b6caf
// fad8c208043b864866d157f7465847d4af75307f6382b573dec41bcbbb16bf13
//
// 9372d6006de7d4dcc054191e2bae19acb13f8e199ecdb12afa7d55eab4c12599
// 358d47f9e1b2e99d2f20166ea1c70387949a2e78b286e1636649b95857bca617
// 46e8c135283b21e78b135a526c72c3f6f2cbf3aee31087f3fba1f332b5739a1c
// 02943a6a3d69ff8dda5016b24b7fbf69908dfe058a4647d23d8d69daff838494
//
// 17618f9a17eac7fa5bba2bc0705ae33fc242a1e1c069b7f1b4f310f5125e812c
// 17618f9a17eac7fa5bba2bc0705ae33fc242a1e1c069b7f1b4f310f5125e812c
//
