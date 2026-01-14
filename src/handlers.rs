use alloy::primitives::keccak256;
use alloy::signers::Signer;
use alloy::signers::local::PrivateKeySigner;

use byteorder::ByteOrder;
use num::{BigUint, ToPrimitive};
use prost::Message;

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::mpsc;

use crate::stream;
use libp2p::{
    PeerId, Stream,
    futures::{AsyncReadExt, AsyncWriteExt},
    identity::ecdsa,
};

use wasm_bindgen::JsValue;
use web3::types::U256;

use crate::conventions::*;
use async_std::sync::{Arc, Mutex};

use crate::weeb_3::etiquette_0;
use crate::weeb_3::etiquette_1;
use crate::weeb_3::etiquette_2;
// use crate::weeb_3::etiquette_3;
use crate::weeb_3::etiquette_4;
use crate::weeb_3::etiquette_5;
use crate::weeb_3::etiquette_6;
use crate::weeb_3::etiquette_7;
use crate::weeb_3::etiquette_8;

use crate::on_chain::ChequebookClient;
use crate::persistence::{get_chequebook_address, get_chequebook_signer_key};
use ethers::signers::LocalWallet;
use ethers::types::{Address as EthAddress, U256 as EthU256};

use crate::HANDSHAKE_PROTOCOL;
use crate::PSEUDOSETTLE_PROTOCOL;
use crate::PUSHSYNC_PROTOCOL;
use crate::RETRIEVAL_PROTOCOL;
use crate::SWAP_PROTOCOL;

pub async fn ceive(
    peer: PeerId,
    network_id: u64,
    self_ephemeral: libp2p::core::Multiaddr,
    mut stream: Stream,
    a: libp2p::core::Multiaddr,
    pk: &ecdsa::SecretKey,
    chan: &mpsc::Sender<PeerFile>,
) -> bool {
    web_sys::console::log_1(&JsValue::from("Handshake stage 1 0"));

    let mut step_0 = etiquette_1::Syn::default();

    step_0.observed_underlay = a.clone().to_vec();

    let mut bufw_0 = Vec::new();

    let step_0_len = step_0.encoded_len();

    bufw_0.reserve(step_0_len + prost::length_delimiter_len(step_0_len));
    step_0.encode_length_delimited(&mut bufw_0).unwrap();

    match stream.write_all(&bufw_0).await {
        Ok(_) => {}
        Err(_) => {
            return false;
        }
    };
    let _ = stream.flush().await;

    web_sys::console::log_1(&JsValue::from("Handshake stage 1 1"));

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                return false;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    web_sys::console::log_1(&JsValue::from("Handshake stage 1 2"));

    let rec_0_u = etiquette_1::SynAck::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_x) => {
            return false;
        }
    };

    web_sys::console::log_1(&JsValue::from(format!(
        "Handshake stage 1 3 2 \n {:#?}",
        rec_0
    )));

    let underlay = self_ephemeral.to_vec();

    web_sys::console::log_1(&JsValue::from("Handshake stage 1 3 3"));

    let peer_overlay = rec_0.ack.clone().unwrap().address.unwrap().overlay;

    let beneficiary = parse_address(
        &rec_0.ack.clone().unwrap().address.unwrap().underlay,
        &rec_0.ack.clone().unwrap().address.unwrap().overlay,
        &rec_0.ack.clone().unwrap().address.unwrap().signature,
        network_id,
    );

    web_sys::console::log_1(&JsValue::from("Handshake stage 1 3"));

    // web_sys::console::log_1(&JsValue::from(format!("Got underlay {}!", underlay)));

    let mut step_1 = etiquette_1::Ack::default();

    let signer: PrivateKeySigner = PrivateKeySigner::from_slice(&pk.to_bytes()).unwrap();
    let addrep = signer.address();
    let addre = addrep.to_vec();

    let mut bufidl: [u8; 8] = [0; 8];
    byteorder::LittleEndian::write_u64(&mut bufidl, 10_u64);
    let byteslice = [addre.as_slice(), &bufidl].concat();
    let nonce: [u8; 32] = [0; 32];
    let byteslice2 = [byteslice, (&nonce).to_vec()].concat();
    let overlayp = keccak256(byteslice2);
    let overlay = &overlayp;

    let hsprefix: &[u8] = &"bee-handshake-".to_string().into_bytes();

    let mut bufidb: [u8; 8] = [0; 8];
    byteorder::BigEndian::write_u64(&mut bufidb, 10_u64);
    let byteslice3 = [hsprefix.to_vec(), underlay.clone()].concat();
    let byteslice4 = [byteslice3, overlay.to_vec()].concat();
    let byteslice5 = [byteslice4, bufidb.to_vec()].concat();

    let signature = signer.sign_message(&byteslice5).await.unwrap();

    let mut step_1_ad = etiquette_1::BzzAddress::default();

    step_1_ad.overlay = overlay.to_vec();
    step_1_ad.underlay = underlay.to_vec();
    step_1_ad.signature = signature.as_bytes().to_vec();

    step_1.address = Some(step_1_ad);
    step_1.nonce = nonce.to_vec();
    step_1.network_id = network_id;
    step_1.full_node = true;
    step_1.welcome_message = "... Ara Ara ...".to_string();

    web_sys::console::log_1(&JsValue::from("Handshake stage 1 4"));

    let mut bufw_1 = Vec::new();

    let step_1_len = step_1.encoded_len();

    bufw_1.reserve(step_1_len + prost::length_delimiter_len(step_1_len));
    step_1.encode_length_delimited(&mut bufw_1).unwrap();
    match stream.write_all(&bufw_1).await {
        Ok(_) => {}
        Err(_) => {
            return false;
        }
    };
    let _ = stream.flush().await;

    web_sys::console::log_1(&JsValue::from("Handshake stage 1 5"));

    let _ = stream.close().await;

    web_sys::console::log_1(&JsValue::from("Handshake stage 1 6"));

    web_sys::console::log_1(&JsValue::from(format!(
        "Connected Peer {:#?} with address {}!",
        peer, beneficiary
    )));

    chan.send(PeerFile {
        peer_id: peer,
        overlay: peer_overlay.clone(),
        beneficiary: beneficiary,
    })
    .unwrap();

    return true;
}

pub async fn pricing_handler(peer: PeerId, mut stream: Stream, chan: &mpsc::Sender<(PeerId, u64)>) {
    web_sys::console::log_1(&JsValue::from(format!(
        "Opened pricing handle for peer {}!",
        peer
    )));

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let empty = etiquette_0::Headers::default();

    let mut buf_empty = Vec::new();

    let empty_len = empty.encoded_len();
    buf_empty.reserve(empty_len + prost::length_delimiter_len(empty_len));
    empty.encode_length_delimited(&mut buf_empty).unwrap();

    match stream.write_all(&buf_empty).await {
        Ok(_) => {}
        Err(_) => {
            return;
        }
    };
    let _ = stream.flush().await;
    let _ = stream.close().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let rec_0_u = etiquette_4::AnnouncePaymentThreshold::decode_length_delimited(&mut Cursor::new(
        buf_nondiscard_0,
    ));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_x) => {
            web_sys::console::log_1(&JsValue::from(format!("Error in protocol {:#?}!", _x)));
            return;
        }
    };

    web_sys::console::log_1(&JsValue::from(format!(
        "Got AnnouncePaymentThreshold {:#?}!",
        rec_0
    )));

    let pt = BigUint::from_bytes_be(&rec_0.payment_threshold)
        .to_u64()
        .unwrap();

    let _ = chan.send((peer, pt));
}

pub async fn gossip_handler(
    peer: PeerId,
    mut stream: Stream,
    chan: &mpsc::Sender<etiquette_2::BzzAddress>,
) {
    web_sys::console::log_1(&JsValue::from(format!(
        "Opened gossip handle for peer {}!",
        peer
    )));

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let empty = etiquette_0::Headers::default();

    let mut buf_empty = Vec::new();

    let empty_len = empty.encoded_len();
    buf_empty.reserve(empty_len + prost::length_delimiter_len(empty_len));
    empty.encode_length_delimited(&mut buf_empty).unwrap();

    match stream.write_all(&buf_empty).await {
        Ok(_) => {}
        Err(_) => {
            return;
        }
    };
    let _ = stream.flush().await;
    let _ = stream.close().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let rec_0_u = etiquette_2::Peers::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(x) => {
            web_sys::console::log_1(&JsValue::from(format!("Error in protocol {:#?}!", x)));
            return;
        }
    };

    // web_sys::console::log_1(&JsValue::from(format!("Got Peers Message {:#?}!", rec_0)));

    for peer in rec_0.peers {
        // web_sys::console::log_1(&JsValue::from(format!(
        //     "Got gossip of peer {:#?}!",
        //     hex::encode(&peer.overlay)
        // )));
        chan.send(peer).unwrap();
    }
}

pub async fn fresh(
    peer: PeerId,
    amount: u64,
    mut stream: Stream,
    chan: &mpsc::Sender<(PeerId, u64)>,
) {
    let empty = etiquette_0::Headers::default();

    let mut buf_empty = Vec::new();

    let empty_len = empty.encoded_len();
    buf_empty.reserve(empty_len + prost::length_delimiter_len(empty_len));
    empty.encode_length_delimited(&mut buf_empty).unwrap();

    match stream.write_all(&buf_empty).await {
        Ok(_) => {}
        Err(_) => {
            chan.send((peer, 0)).unwrap();
            return;
        }
    };
    let _ = stream.flush().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                chan.send((peer, 0)).unwrap();
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let mut step_1 = etiquette_5::Payment::default();

    step_1.amount = BigUint::from(amount).to_bytes_be();

    let mut bufw_1 = Vec::new();

    let step_1_len = step_1.encoded_len();

    bufw_1.reserve(step_1_len + prost::length_delimiter_len(step_1_len));
    step_1.encode_length_delimited(&mut bufw_1).unwrap();
    match stream.write_all(&bufw_1).await {
        Ok(_) => {}
        Err(_) => {
            chan.send((peer, 0)).unwrap();
            return;
        }
    };
    let _ = stream.flush().await;
    let _ = stream.close().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                chan.send((peer, 0)).unwrap();
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let rec_0_u =
        etiquette_5::PaymentAck::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(x) => {
            chan.send((peer, 0)).unwrap();
            web_sys::console::log_1(&JsValue::from(format!("Error in protocol {:#?}!", x)));
            return;
        }
    };

    let refr_am = BigUint::from_bytes_be(&rec_0.amount).to_u64().unwrap();

    if amount > 0 {
        chan.send((peer, refr_am)).unwrap();
    } else {
        chan.send((peer, 0)).unwrap();
    }
}

#[allow(dead_code)]
pub async fn issue(
    peer: PeerId,
    amount: u64,
    mut stream: Stream,
    chan: &mpsc::Sender<(PeerId, bool)>,
    beneficiaries: Arc<Mutex<HashMap<PeerId, (web3::types::Address, bool)>>>,
    price: U256,
    deduction: U256,
) {
    web_sys::console::log_1(&JsValue::from(format!(
        "Opened issue handle for peer {}!",
        peer
    )));

    let mut non_empty = etiquette_0::Headers::default();

    let mut price_header = etiquette_0::Header::default();
    price_header.key = "exchange".to_string();

    let mut buf = [0u8; 32];
    price.to_big_endian(&mut buf);
    price_header.value = BigUint::from_bytes_be(&buf).to_bytes_be();

    let mut deduction_header = etiquette_0::Header::default();
    deduction_header.key = "deduction".to_string();

    let mut buf = [0u8; 32];
    deduction.to_big_endian(&mut buf);
    deduction_header.value = BigUint::from_bytes_be(&buf).to_bytes_be();

    non_empty.headers = vec![price_header, deduction_header];

    let mut buf_non_empty = Vec::new();

    let non_empty_len = non_empty.encoded_len();
    buf_non_empty.reserve(non_empty_len + prost::length_delimiter_len(non_empty_len));
    non_empty
        .encode_length_delimited(&mut buf_non_empty)
        .unwrap();

    match stream.write_all(&buf_non_empty).await {
        Ok(_) => {}
        Err(_) => {
            chan.send((peer, false)).unwrap();
            web_sys::console::log_1(&JsValue::from(format!("Issue fail -2")));

            return;
        }
    };
    let _ = stream.flush().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                chan.send((peer, false)).unwrap();
                web_sys::console::log_1(&JsValue::from(format!("Issue fail -1")));

                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let beneficiary_bytes_opt = {
        let map = beneficiaries.lock().await;
        map.get(&peer).cloned()
    };
    let beneficiary_bytes = match beneficiary_bytes_opt {
        Some(b) => b.0.as_bytes().to_vec(),
        _ => {
            let _ = chan.send((peer, false)).unwrap_or(());
            web_sys::console::log_1(&JsValue::from(format!("Issue fail 0")));
            return;
        }
    };
    let beneficiary = EthAddress::from_slice(&beneficiary_bytes);

    let signer_key = get_chequebook_signer_key().await;
    if signer_key.len() != 32 {
        let _ = chan.send((peer, false)).unwrap_or(());
        web_sys::console::log_1(&JsValue::from(format!("Issue fail 1")));
        return;
    }

    let wallet = match LocalWallet::from_bytes(&signer_key) {
        Ok(w) => w,
        Err(_) => {
            let _ = chan.send((peer, false)).unwrap_or(());
            web_sys::console::log_1(&JsValue::from(format!("Issue fail 2")));
            return;
        }
    };

    let cb_addr_bytes = get_chequebook_address().await;
    if cb_addr_bytes.len() != 20 {
        let _ = chan.send((peer, false)).unwrap_or(());
        web_sys::console::log_1(&JsValue::from(format!("Issue fail 3")));
        return;
    }

    let chequebook_addr = EthAddress::from_slice(&cb_addr_bytes);

    let mut client = ChequebookClient::new(chequebook_addr, wallet, 11155111);

    let exchange_rate = EthU256::from(price);
    let deduction = EthU256::from(deduction);
    let send_amount = EthU256::from(amount)
        .checked_mul(exchange_rate)
        .and_then(|v| v.checked_add(deduction));

    let cheque_json =
        match send_amount.and_then(|amt| client.prepare_emit_cheque_bytes(beneficiary, amt)) {
            Some(b) => b,
            None => {
                let _ = chan.send((peer, false));
                web_sys::console::log_1(&"Issue fail 4".into());
                return;
            }
        };

    let mut msg = etiquette_8::EmitCheque::default();
    msg.cheque = cheque_json;

    let mut bufw = Vec::new();
    let len = msg.encoded_len();
    bufw.reserve(len + prost::length_delimiter_len(len));
    msg.encode_length_delimited(&mut bufw).unwrap();

    if let Err(_) = stream.write_all(&bufw).await {
        let _ = chan.send((peer, false));
        web_sys::console::log_1(&"Issue fail 5".into());
        return;
    }

    let _ = stream.flush().await;
    let _ = stream.close().await;

    web_sys::console::log_1(&JsValue::from(format!("Issue complete")));
    let _ = chan.send((peer, true)).unwrap_or(());
}

pub async fn trieve(
    _peer: PeerId,
    chunk_address: Vec<u8>,
    mut stream: Stream,
    chan: &mpsc::Sender<Vec<u8>>,
) {
    let empty = etiquette_0::Headers::default();

    let mut buf_empty = Vec::new();

    let empty_len = empty.encoded_len();
    buf_empty.reserve(empty_len + prost::length_delimiter_len(empty_len));
    empty.encode_length_delimited(&mut buf_empty).unwrap();

    match stream.write_all(&buf_empty).await {
        Ok(_) => {}
        Err(_) => {
            return;
        }
    };
    let _ = stream.flush().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let mut step_1 = etiquette_6::Request::default();

    step_1.addr = chunk_address;

    let mut bufw_1 = Vec::new();

    let step_1_len = step_1.encoded_len();

    bufw_1.reserve(step_1_len + prost::length_delimiter_len(step_1_len));
    step_1.encode_length_delimited(&mut bufw_1).unwrap();
    match stream.write_all(&bufw_1).await {
        Ok(_) => {}
        Err(_) => {
            return;
        }
    };
    let _ = stream.flush().await;
    let _ = stream.close().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let rec_0_u =
        etiquette_6::Delivery::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(x) => {
            web_sys::console::log_1(&JsValue::from(format!("Error in protocol {:#?}!", x)));
            {
                return;
            };
        }
    };

    let rec_1 = rec_0.data;

    chan.send(rec_1).unwrap();
}

pub async fn connection_handler(
    peer: PeerId,
    network_id: u64,
    self_ephemeral: libp2p::core::Multiaddr,
    mut control: stream::Control,
    a: &libp2p::core::Multiaddr,
    pk: &ecdsa::SecretKey,
    chan: &mpsc::Sender<PeerFile>,
) -> bool {
    web_sys::console::log_1(&JsValue::from("Handshake stage 0"));

    let stream = match control.open_stream(peer, HANDSHAKE_PROTOCOL).await {
        Ok(stream) => stream,
        Err(error @ stream::OpenStreamError::UnsupportedProtocol(_)) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            return false;
        }
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            return false;
        }
    };

    web_sys::console::log_1(&JsValue::from("Handshake stage 1"));

    if !ceive(
        peer,
        network_id,
        self_ephemeral,
        stream,
        a.clone(),
        &pk.clone(),
        chan,
    )
    .await
    {
        web_sys::console::log_1(&JsValue::from("Handshake protocol failed"));
        return false;
    }

    web_sys::console::log_1(&JsValue::from("Handshake stage 2"));

    web_sys::console::log_1(&JsValue::from(format!(
        "Handshake complete for peer: {}!",
        peer
    )));

    web_sys::console::log_1(&JsValue::from("Handshake stage 3"));

    return true;
}

pub async fn refresh_handler(
    peer: PeerId,
    amount: u64,
    control: stream::Control,
    chan: &mpsc::Sender<(PeerId, u64)>,
) {
    let stream = match control
        .clone()
        .open_stream(peer, PSEUDOSETTLE_PROTOCOL)
        .await
    {
        Ok(stream) => stream,
        Err(error @ stream::OpenStreamError::UnsupportedProtocol(_)) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            chan.send((peer, 0)).unwrap();
            return;
        }
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            chan.send((peer, 0)).unwrap();
            return;
        }
    };

    fresh(peer, amount, stream, chan).await;
}

#[allow(dead_code)]
pub async fn issue_handler(
    peer: PeerId,
    amount: u64,
    control: stream::Control,
    chan: &mpsc::Sender<(PeerId, bool)>,
    beneficiaries: Arc<Mutex<HashMap<PeerId, (web3::types::Address, bool)>>>,
    price: U256,
    deduction: U256,
) {
    let stream = match control.clone().open_stream(peer, SWAP_PROTOCOL).await {
        Ok(stream) => stream,
        Err(error @ stream::OpenStreamError::UnsupportedProtocol(_)) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            let _ = chan.send((peer, false));
            return;
        }
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            let _ = chan.send((peer, false));
            return;
        }
    };

    issue(peer, amount, stream, chan, beneficiaries, price, deduction).await;
}

pub async fn retrieve_handler(
    peer: PeerId,
    chunk_address: Vec<u8>,
    control: stream::Control,
    chan: &mpsc::Sender<Vec<u8>>,
) {
    let stream = match control.clone().open_stream(peer, RETRIEVAL_PROTOCOL).await {
        Ok(stream) => stream,
        Err(error @ stream::OpenStreamError::UnsupportedProtocol(_)) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            return;
        }
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            return;
        }
    };

    trieve(peer, chunk_address, stream, chan).await;
}

pub async fn pushsync_handler(
    peer: PeerId,
    chunk_address: &Vec<u8>,
    chunk_content: &Vec<u8>,
    chunk_stamp: &Vec<u8>,
    control: stream::Control,
    chan: &mpsc::Sender<bool>,
) {
    let stream = match control.clone().open_stream(peer, PUSHSYNC_PROTOCOL).await {
        Ok(stream) => stream,
        Err(error @ stream::OpenStreamError::UnsupportedProtocol(_)) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            return;
        }
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            return;
        }
    };

    sync(
        peer,
        chunk_address,
        chunk_content,
        chunk_stamp,
        stream,
        chan,
    )
    .await;
}

pub async fn sync(
    _peer: PeerId,
    chunk_address: &Vec<u8>,
    chunk_content: &Vec<u8>,
    chunk_stamp: &Vec<u8>,
    mut stream: Stream,
    chan: &mpsc::Sender<bool>,
) {
    let empty = etiquette_0::Headers::default();
    let mut buf_empty = Vec::new();

    let empty_len = empty.encoded_len();
    buf_empty.reserve(empty_len + prost::length_delimiter_len(empty_len));
    empty.encode_length_delimited(&mut buf_empty).unwrap();

    match stream.write_all(&buf_empty).await {
        Ok(_) => {}
        Err(_) => {
            chan.send(false).unwrap();
            return;
        }
    };
    let _ = stream.flush().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                chan.send(false).unwrap();
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let mut step_1 = etiquette_7::Delivery::default();

    step_1.address = chunk_address.to_vec();
    step_1.data = chunk_content.to_vec();
    step_1.stamp = chunk_stamp.to_vec();

    let bufw_1 = step_1.encode_length_delimited_to_vec();

    let mut i = 0;
    loop {
        let mut j = i + 255;
        if j > bufw_1.len() {
            j = bufw_1.len();
        };
        match stream.write(&bufw_1[i..j].to_vec()).await {
            Ok(_) => {}
            Err(_) => {
                chan.send(false).unwrap();
                return;
            }
        };
        let _ = stream.flush().await;
        i = i + 255;
        if j == bufw_1.len() {
            break;
        };
    }

    let _ = stream.close().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = match stream.read(&mut buf_discard_0).await {
            Ok(a) => a,
            Err(_) => {
                chan.send(false).unwrap();
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let rec_0_u = etiquette_7::Receipt::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0));

    let _rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(x) => {
            web_sys::console::log_1(&JsValue::from(format!("Error in protocol {:#?}!", x)));
            chan.send(false).unwrap();
            return;
        }
    };

    //    web_sys::console::log_1(&JsValue::from(format!(
    //        "Got receipt {:#?} with err {} from peer {}!",
    //        hex::encode(&rec_0.address),
    //        rec_0.err,
    //        _peer
    //    )));

    chan.send(true).unwrap();
}
