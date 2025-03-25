use crate::{
    //    // // // // // // // //
    //    apply_credit,
    //    // // // // // // // //
    //    cancel_reserve,
    //    // // // // // // // //
    content_address,
    //    // // // // // // // //
    //    encode_resources,
    //    // // // // // // // //
    //    get_feed_address,
    //    // // // // // // // //
    //    get_proximity,
    //    // // // // // // // //
    //    manifest::interpret_manifest,
    //    // // // // // // // //
    mpsc,
    //    // // // // // // // //
    //    price,
    //    // // // // // // // //
    //    reserve,
    //    // // // // // // // //
    //    push_handler,
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
    //    HashSet,
    //    // // // // // // // //
    //    JsValue,
    //    // // // // // // // //
    Mutex,
    //    // // // // // // // //
    PeerAccounting,
    //    // // // // // // // //
    PeerId,
    //    // // // // // // // //
    RETRIEVE_ROUND_TIME,
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

use rand::rngs::ThreadRng;

pub async fn f0() {}

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
            if seg < RETRIEVE_ROUND_TIME {
                async_std::task::sleep(Duration::from_millis((RETRIEVE_ROUND_TIME - seg) as u64))
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

    let caddr = content_address(data0);

    //

    //    // stamp_signer: Signer,
    //    batch_id: Vec<u8>,
    //    // batch_buckets: HashMap<u32, u32>
    //    chunk_address: Vec<u8>,
    //    //

    //

    // stamp_chunk();

    return vec![];
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
