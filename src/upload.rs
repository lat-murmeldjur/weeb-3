use crate::{
    //    // // // // // // // //
    apply_credit,
    //    // // // // // // // //
    cancel_reserve,
    //    // // // // // // // //
    content_address,
    //    // // // // // // // //
    get_proximity,
    //    // // // // // // // //
    manifest_upload::{create_manifest, Node},
    //    // // // // // // // //
    mpsc,
    //    // // // // // // // //
    price,
    //    // // // // // // // //
    pushsync_handler,
    //    // // // // // // // //
    reserve,
    //    // // // // // // // //
    stream,
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
    PeerAccounting,
    //    // // // // // // // //
    PeerId,
    //    // // // // // // // //
    PROTOCOL_ROUND_TIME,
    //    // // // // // // // //
};

use byteorder::ByteOrder;

use libp2p::futures::{stream::FuturesUnordered, StreamExt};

use alloy::primitives::keccak256;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;

use async_std::sync::Arc;

pub async fn stamp_chunk(
    // stamp_signer: Signer,
    batch_id: Vec<u8>,
    batch_buckets: Arc<Mutex<HashMap<u32, u32>>>,
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
    let mut index = 0_u32;

    {
        let mut batch_buckets_mut = batch_buckets.lock().unwrap();
        match batch_buckets_mut.get(&bucket) {
            Some(numbr) => {
                index = *numbr;
                if index > batch_bucket_limit {
                    web_sys::console::log_1(&JsValue::from(format!("Stamp bucket overuse")));

                    return vec![];
                }
            }
            _ => {}
        }
        _ = batch_buckets_mut.insert(bucket, index + 1);
    }

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

pub async fn upload_resource(
    name0: String,
    mime0: String,
    encryption: bool,
    data: &Vec<u8>,
    data_upload_chan: &mpsc::Sender<(Vec<u8>, u8, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    //

    // upload core file
    let core_reference = upload_data(data.to_vec(), encryption, data_upload_chan).await;

    web_sys::console::log_1(&JsValue::from(format!(
        "Upload resource returning {:#?}!",
        hex::encode(&core_reference)
    )));

    // return core_reference;

    let core_manifest = create_manifest(
        encryption,
        encryption,
        vec![Node {
            data: core_reference.clone(), // pub data: Vec<u8>, // repurposed as address
            mime: mime0,                  // pub mime: String,
            _filename: name0.clone(),     // pub filename: String,
            path: hex::encode(core_reference.clone()), // pub path: String,
        }], // forks
        vec![],                      // data_forks
        vec![],                      // reference
        hex::encode(core_reference), // index
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
    data: &Vec<u8>,
    encryption: bool,
    batch_id: Vec<u8>,
    batch_buckets: Arc<Mutex<HashMap<u32, u32>>>,
    batch_bucket_limit: u32,
    control: &mut stream::Control,
    peers: &Mutex<HashMap<String, PeerId>>,
    accounting: &Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
) -> Vec<u8> {
    //

    let span_length = data.len();

    if data.len() <= 4096 {
        let k = push_chunk(
            data.len(),
            data,
            encryption,
            batch_id,
            batch_buckets,
            batch_bucket_limit,
            control,
            peers,
            accounting,
            refresh_chan,
        )
        .await;

        web_sys::console::log_1(&JsValue::from(format!(
            "Pushdata returning {:#?}!",
            hex::encode(&k)
        )));

        return k;
    } else {
        // split data
        let mut address_length = 32;
        if encryption {
            address_length = 64;
        }

        let address_fit = 4096 / address_length;
        let mut done0 = false;
        let mut carry_span = 4096_u64;
        let mut iterations = 0;

        while !done0 {
            let k = span_length / carry_span as usize;
            let mut l0 = span_length % carry_span as usize;
            if l0 > 0 {
                l0 = 1;
            }

            if k + l0 <= address_fit {
                done0 = true;
                iterations = k + l0;
            } else {
                carry_span *= address_fit as u64;
            }
        }

        let mut joiner = FuturesUnordered::new(); // ::<dyn Future<Output = Vec<u8>>> // ::<Pin<Box<dyn Future<Output = (Vec<u8>, usize)>>>>

        for i in 0..iterations {
            let index = i;
            let mut ctrl = control.clone();
            let data_start = i * carry_span as usize;
            let mut data_end = (i + 1) * carry_span as usize;
            if data_end > data.len() {
                data_end = data.len();
            };
            let data_carry = data[data_start..data_end].to_vec();

            let bi0 = batch_id.clone();
            let bb0 = batch_buckets.clone();

            let handle = async move {
                return (
                    push_data(
                        &data_carry,
                        encryption,
                        bi0,
                        bb0,
                        batch_bucket_limit,
                        &mut ctrl,
                        peers,
                        accounting,
                        refresh_chan,
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

        let mut data_capstone: Vec<u8> = Vec::new();

        for i in 0..iterations {
            match content_holder_3.get(&i) {
                Some(data0) => {
                    if data0.len() > 0 {
                        data_capstone.append(&mut data0[..].to_vec());
                    } else {
                        return vec![];
                    }
                }
                None => return vec![],
            }
        }

        web_sys::console::log_1(&JsValue::from(format!(
            "Pushdata capstone data {}!",
            hex::encode(&data_capstone)
        )));

        let k = push_chunk(
            span_length,
            &data_capstone,
            encryption,
            batch_id,
            batch_buckets,
            batch_bucket_limit,
            control,
            peers,
            accounting,
            refresh_chan,
        )
        .await;

        web_sys::console::log_1(&JsValue::from(format!(
            "Pushdata returning {:#?}!",
            hex::encode(&k)
        )));

        return k;
    }

    #[allow(unreachable_code)]
    return vec![];
}

pub async fn push_chunk(
    span: usize,
    data: &Vec<u8>,
    encryption: bool,
    batch_id: Vec<u8>,
    batch_buckets: Arc<Mutex<HashMap<u32, u32>>>,
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
        data0 = encrypt(span as u64, data, encrey.to_vec());
    } else {
        data0 = [(span as u64).to_le_bytes().to_vec(), data00]
            .concat()
            .to_vec();
    }

    let caddr = content_address(data0.clone());

    let cstamp0 = stamp_chunk(
        //
        batch_id,
        batch_buckets,
        batch_bucket_limit,
        caddr.clone(),
    )
    .await;

    if cstamp0.len() == 0 {
        web_sys::console::log_1(&JsValue::from(format!(
            "Pushchunk returning empty reference for reason of bucket overuse {}",
            hex::encode(&caddr)
        )));
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

    web_sys::console::log_1(&JsValue::from(format!(
        "Pushchunk returning {:#?}!",
        hex::encode(&caddr)
    )));

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
