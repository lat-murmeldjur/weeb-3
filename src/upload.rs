use crate::{
    //    // // // // // // // //
    Date,
    //    // // // // // // // //
    Duration,
    //    // // // // // // // //
    HashMap,
    //    // // // // // // // //
    HashSet,
    //    // // // // // // // //
    JsValue,
    //    // // // // // // // //
    Mutex,
    //    // // // // // // // //
    PROTOCOL_ROUND_TIME,
    //    // // // // // // // //
    PeerAccounting,
    //    // // // // // // // //
    PeerId,
    //    // // // // // // // //
    apply_credit,
    //    // // // // // // // //
    cancel_reserve,
    //    // // // // // // // //
    content_address,
    //    // // // // // // // //
    get_proximity,
    //    // // // // // // // //
    manifest_upload::{Node, create_manifest},
    //    // // // // // // // //
    mpsc,
    //    // // // // // // // //
    persistence::bump_bucket,
    //    // // // // // // // //
    //    // // // // // // // //
    price,
    //    // // // // // // // //
    pushsync_handler,
    //    // // // // // // // //
    reserve,
    //    // // // // // // // //
    stream,
};

use byteorder::ByteOrder;

use alloy::primitives::keccak256;
use alloy::signers::Signer;
use alloy::signers::local::PrivateKeySigner;

pub async fn stamp_chunk(
    // stamp_signer: Signer,
    batch_id: Vec<u8>,
    batch_bucket_limit: u32,
    chunk_address: Vec<u8>,
    //
) -> Vec<u8> {
    let stamp_signer_key = keccak256("Key To Be Persisted In Browser Localstore");
    let stamp_signer: PrivateKeySigner = match PrivateKeySigner::from_bytes(&stamp_signer_key) {
        Ok(aok) => aok,
        _ => return vec![],
    };

    let bucket = u32::from_be_bytes(chunk_address[..4].try_into().unwrap()) >> (32 - 16);

    #[allow(unused_assignments)]
    let mut index = 0;

    let (h, index0) =
        bump_bucket(hex::encode(&batch_id).to_string() + &"__24__" + &bucket.to_string()).await;
    index = index0;

    if index > batch_bucket_limit {
        web_sys::console::log_1(&JsValue::from(format!("Stamp bucket overuse")));
        return vec![];
    };

    if !h {
        web_sys::console::log_1(&JsValue::from(format!("Stamp bucket use fail")));
        return vec![];
    };

    let index_bytes = [bucket.to_be_bytes(), index.to_be_bytes()].concat();

    let timestamp: u64 = (Date::now() as u64) * 1000000;
    let timestamp_bytes = timestamp.to_be_bytes();

    let to_sign_digest = keccak256(
        [
            chunk_address.clone(),
            batch_id.clone(),
            index_bytes.clone(),
            timestamp_bytes.to_vec(),
        ]
        .concat(),
    );

    let signature = stamp_signer
        .sign_message(to_sign_digest.as_slice())
        .await
        .unwrap()
        .as_bytes()
        .to_vec();

    let stamp = [batch_id, index_bytes, timestamp_bytes.to_vec(), signature].concat();

    stamp
}

pub struct Resource {
    pub path0: String,
    pub filename0: String,
    pub mime0: String,
    pub data: Vec<u8>,
    pub data_address: Vec<u8>,
}

pub async fn upload_resource(
    resource0: Vec<Resource>,
    encryption: bool,
    mut index: String,
    errordoc: String,
    data_upload_chan: &mpsc::Sender<(Vec<u8>, u8, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    //
    let mut node0: Vec<Node> = vec![];

    for mut r0 in resource0 {
        web_sys::console::log_1(&JsValue::from(format!("Attempt uploading resource!",)));

        // upload core file
        let core_reference = upload_data(r0.data.to_vec(), encryption, data_upload_chan).await;

        if r0.path0.len() == 0 {
            r0.path0 = hex::encode(&core_reference);
        }

        if index.len() == 0 {
            index = r0.path0.clone();
        };

        web_sys::console::log_1(&JsValue::from(format!(
            "Upload resource returning {:#?}!",
            hex::encode(&core_reference)
        )));

        r0.data_address = core_reference;

        node0.push(Node {
            data: r0.data_address.clone(), // pub data: Vec<u8>, // repurposed as address
            mime: r0.mime0.clone(),        // pub mime: String,
            filename: r0.filename0.clone(), // pub filename: String,
            path: r0.path0.clone(),        // pub path: String,
        })
    }

    // return core_reference;

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
        data_upload_chan,
    )
    .await;

    return upload_data(core_manifest, encryption, data_upload_chan).await;

    // alternatively, unpack .tar.gz and upload all files
    // {
    //
    // }

    // create manifest
    // return manifest reference
}

pub async fn upload_data(
    data: Vec<u8>,
    enc: bool,
    data_upload_chan: &mpsc::Sender<(Vec<u8>, u8, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let (chan_out, chan_in) = mpsc::channel::<Vec<u8>>();
    let mut enc_mode = 0;
    if enc {
        enc_mode = 1;
    }

    data_upload_chan.send((data, enc_mode, chan_out)).unwrap();

    let k0 = async {
        let mut timelast: f64;
        #[allow(irrefutable_let_patterns)]
        while let that = chan_in.try_recv() {
            timelast = Date::now();
            if !that.is_err() {
                let t = that.unwrap();

                web_sys::console::log_1(&JsValue::from(format!(
                    "Upload data returning {:#?}!",
                    hex::encode(&t)
                )));

                return t;
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

pub async fn push_data(
    data: Vec<u8>,
    encryption: bool,
    chunk_upload_chan: &mpsc::Sender<(usize, Vec<u8>, u8, Vec<u8>)>,
) -> Vec<u8> {
    //
    let mode = match encryption {
        false => 0,
        true => 1,
    };

    let span_length = data.len();

    if data.len() <= 4096 {
        let k =
            content_address(&[(data.len() as u64).to_le_bytes().to_vec(), data.clone()].concat());
        let _ = chunk_upload_chan.send((data.len(), data, mode, k.clone()));

        // web_sys::console::log_1(&JsValue::from(format!(
        //     "push_data returning {:#?}!",
        //     hex::encode(&k)
        // )));

        return k;
    } else {
        let mut levels: Vec<Vec<Vec<u8>>> = Vec::new();

        let mut address_length = 32;
        if encryption {
            address_length = 64;
        }

        let mut level_data = data;

        let mut level = 0;
        let address_fit = 4096 / address_length;
        let next_level = true;
        let mut span_carriage = 4096;

        while next_level {
            levels.push(Vec::new());

            let mut chunk_l0r = level_data.len() % 4096;
            if chunk_l0r > 0 {
                chunk_l0r = 1;
            }
            let chunk_l0c = level_data.len() / 4096 + chunk_l0r;

            web_sys::console::log_1(&JsValue::from(format!(
                "level  {} chunk count : {}   ln {}!",
                level, //
                chunk_l0c,
                level_data.len()
            )));

            if chunk_l0c == 1 {
                for j in 0..level_data.len() / 32 {
                    web_sys::console::log_1(&JsValue::from(format!(
                        "top level chunk  {}",
                        hex::encode(&level_data[j * 32..(j + 1) * 32])
                    )));
                }
            }

            let mut count_yield = 0;

            for i in 0..chunk_l0c {
                count_yield += 1;
                if count_yield > 128 {
                    async_std::task::yield_now().await;
                    async_std::task::sleep(Duration::from_millis(100)).await;
                    count_yield = 0;
                }

                let data_start = 4096 * i as usize;
                let mut data_end = 4096 * (i + 1) as usize;
                if data_end > level_data.len() {
                    data_end = level_data.len();
                };

                let mut span = span_carriage;

                if (i + 1) * span_carriage > span_length {
                    span = span_length - (i * span_carriage);

                    web_sys::console::log_1(&JsValue::from(format!(
                        "last chunk span : {} span_carriage : {}!",
                        span, span_carriage,
                    )));
                };

                if chunk_l0c == 1 {
                    span = span_length;
                    web_sys::console::log_1(&JsValue::from(format!("top chunk span : {}!", span)));
                }

                let ch_d = level_data[data_start..data_end].to_vec();

                if ch_d.len() == address_length && level > 0 {
                    levels[level].push(ch_d);
                    web_sys::console::log_1(&JsValue::from(format!("partition level difference!")));
                } else {
                    let cha = content_address(
                        &[(span as u64).to_le_bytes().to_vec(), ch_d.clone()].concat(),
                    );

                    if i % 10000 == 0 {
                        web_sys::console::log_1(&JsValue::from(format!(
                            "dispatching iter {} {}",
                            i,
                            hex::encode(&cha)
                        )));
                    }
                    levels[level].push(cha.clone());

                    // (span as u64).to_le_bytes().to_vec(),

                    let _ = chunk_upload_chan.send((span, ch_d, mode, cha));
                }
            }

            if levels[level].len() == 1 {
                return levels[level][0].clone();
            } else {
                level_data = levels[level].concat();
                level += 1;

                web_sys::console::log_1(&JsValue::from(format!(
                    "level change data len : {} level : {}!",
                    level_data.len(),
                    level,
                )));

                span_carriage *= address_fit;
            }
        }

        return vec![];
    }

    #[allow(unreachable_code)]
    return vec![];
}

pub async fn push_chunk(
    span: usize,
    data: &Vec<u8>,
    encryption: bool,
    batch_id: Vec<u8>,
    batch_bucket_limit: u32,
    control: &mut stream::Control,
    peers: &Mutex<HashMap<String, PeerId>>,
    accounting: &Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
) -> Vec<u8> {
    if data.len() > 4096 {
        web_sys::console::log_1(&JsValue::from(format!(
            "Pushchunk returning empty reference for reason of data overlength!"
        )));

        return vec![];
    }

    //
    let data00 = data.to_vec();

    #[allow(unused_assignments)]
    let mut data0: Vec<u8> = vec![];

    if encryption {
        let mut encreysource = vec![];

        for _i in 0..256 {
            encreysource.push(rand::random::<u8>());
        }

        let encrey = keccak256(encreysource);
        data0 = encrypt(span as u64, &data, encrey.to_vec());
    } else {
        data0 = [(span as u64).to_le_bytes().to_vec(), data00]
            .concat()
            .to_vec();
    }

    let caddr = content_address(&data0.clone());

    let cstamp0 = stamp_chunk(
        //
        batch_id,
        batch_bucket_limit,
        caddr.clone(),
    )
    .await;

    if cstamp0.len() == 0 {
        web_sys::console::log_1(&JsValue::from(format!(
            "Pushchunk returning empty reference for reason of bucket overuse {}",
            hex::encode(&caddr)
        )));

        return vec![];
    }

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

    let (mut _storer_address, mut _storer_signature, mut _receipt_nonce) = (vec![], vec![], vec![]);

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
                    web_sys::console::log_1(&JsValue::from(format!(
                        "Pushchunk returning empty reference for reason of no peers to push to {}",
                        hex::encode(&caddr)
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

        let (chunk_out, chunk_in) = mpsc::channel::<(Vec<u8>, Vec<u8>, Vec<u8>)>();

        pushsync_handler(
            closest_peer_id,
            caddr.clone(),
            data0.clone(),
            cstamp0.clone(),
            control,
            &chunk_out,
        )
        .await;

        let receipt_values = chunk_in.try_recv();
        if receipt_values.is_err() {
            let accounting_peers = accounting.lock().unwrap();
            if accounting_peers.contains_key(&closest_peer_id) {
                let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                cancel_reserve(accounting_peer, req_price)
            }
        }

        (_storer_address, _storer_signature, _receipt_nonce) = match receipt_values {
            Ok((ref _x, ref _y, ref _z)) => {
                let accounting_peers = accounting.lock().unwrap();
                if accounting_peers.contains_key(&closest_peer_id) {
                    let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                    apply_credit(accounting_peer, req_price);
                }
                break; // move this to receipt validation later
                #[allow(unreachable_code)]
                (_x.clone(), _y.clone(), _z.clone())
            }
            Err(_x) => {
                error_count += 1;
                let accounting_peers = accounting.lock().unwrap();
                if accounting_peers.contains_key(&closest_peer_id) {
                    let accounting_peer = accounting_peers.get(&closest_peer_id).unwrap();
                    cancel_reserve(accounting_peer, req_price)
                }
                (vec![], vec![], vec![])
            }
        };
    }

    // validate receipt

    // web_sys::console::log_1(&JsValue::from(format!(
    //     "Pushchunk returning {:#?}!",
    //     hex::encode(&caddr)
    // )));

    return caddr;
}

pub fn encrypt(span: u64, cd: &Vec<u8>, encrey: Vec<u8>) -> Vec<u8> {
    if cd.len() < 8 {
        return vec![];
    }

    let padding_length = 4096 - cd.len();
    let mut padding = vec![];

    for _i in 0..padding_length {
        padding.push(rand::random::<u8>());
    }

    let spancred = span.to_le_bytes().to_vec();
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

    return [spanbytes, content[..].to_vec()].concat();
}
