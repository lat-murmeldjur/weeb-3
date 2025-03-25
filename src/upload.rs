use crate::{
    //    // // // // // // // //
    apply_credit,
    //    // // // // // // // //
    cancel_reserve,
    //    // // // // // // // //
    content_address,
    //    // // // // // // // //
    //    encode_resources,
    //    // // // // // // // //
    //    get_feed_address,
    //    // // // // // // // //
    get_proximity,
    //    // // // // // // // //
    //    manifest::interpret_manifest,
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
    //    valid_cac,
    //    // // // // // // // //
    //    valid_soc,
    //    // // // // // // // //
    Date,
    //    // // // // // // // //
    Duration,
    //    // // // // // // // //
    HashMap,
    //    // // // // // // // //
    HashSet,
    //    // // // // // // // //
    //    JsValue,
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

// use libp2p::futures::{
//     // stream::FuturesUnordered,
//     //
//     // StreamExt
// };

use alloy::primitives::keccak256;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;

pub async fn stamp_chunk(
    // stamp_signer: Signer,
    batch_id: Vec<u8>,
    // batch_buckets: HashMap<u32, u32>
    chunk_address: Vec<u8>,
    //
) -> Vec<u8> {
    let stamp_signer_key = keccak256("Key To Be Persisted In Browser Localstore");
    let stamp_signer: PrivateKeySigner = match PrivateKeySigner::from_bytes(&stamp_signer_key) {
        Ok(aok) => aok,
        _ => return vec![],
    };

    let bucket = u32::from_be_bytes(chunk_address[..4].try_into().unwrap()) >> (32 - 16);
    let index = 0_u32;
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
    name: String,
    mime: String,
    encryption: bool,
    data: &Vec<u8>,
    data_upload_chan: &mpsc::Sender<(Vec<u8>, u8, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    //

    // upload core file
    let core_reference = upload_data(data.to_vec(), encryption, data_upload_chan).await;

    return core_reference;

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

pub async fn push_data(
    data: &Vec<u8>,
    encryption: bool,
    control: &mut stream::Control,
    peers: &Mutex<HashMap<String, PeerId>>,
    accounting: &Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
) -> Vec<u8> {
    //
    if data.len() <= 4096 {
        return push_chunk(
            data.len(),
            data,
            encryption,
            control,
            peers,
            accounting,
            refresh_chan,
        )
        .await;
    }

    return vec![];
}

pub async fn push_chunk(
    span: usize,
    data: &Vec<u8>,
    encryption: bool,
    control: &mut stream::Control,
    peers: &Mutex<HashMap<String, PeerId>>,
    accounting: &Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    refresh_chan: &mpsc::Sender<(PeerId, u64)>,
) -> Vec<u8> {
    //
    let mut data0 = data.to_vec().clone();

    if encryption {
        let mut encreysource = vec![];

        for i in 0..256 {
            encreysource.push(rand::random::<u8>());
        }

        let encrey = keccak256(encreysource);
        data0 = encrypt(span as u64, data, encrey.to_vec());
    }

    let caddr = content_address(data0.clone());

    //

    //    // stamp_signer: Signer,
    //    batch_id: Vec<u8>,
    //    // batch_buckets: HashMap<u32, u32>
    //    chunk_address: Vec<u8>,
    //    //

    //

    let cstamp0 = stamp_chunk(
        hex::decode("b57e46b067d21cede7432900215423e82c97823b733219b7f42e73017562a96d").unwrap(),
        caddr.clone(),
    )
    .await;

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

    let (mut storer_address, mut storer_signature, mut receipt_nonce) = (vec![], vec![], vec![]);

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

        (storer_address, storer_signature, receipt_nonce) = match receipt_values {
            Ok((ref x, ref y, ref z)) => (x.clone(), y.clone(), z.clone()),
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

    return caddr;
}

pub fn encrypt(span: u64, cd: &Vec<u8>, encrey: Vec<u8>) -> Vec<u8> {
    if cd.len() < 8 {
        return vec![];
    }

    let padding_length = 4096 - cd.len();
    let mut padding = vec![];

    for i in 0..padding_length {
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
