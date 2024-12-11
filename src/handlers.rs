use alloy::primitives::keccak256;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;

use byteorder::ByteOrder;
use num::{BigUint, ToPrimitive};
use prost::Message;

use std::io;
use std::io::Cursor;
use std::sync::mpsc;

use crate::stream;
use libp2p::{
    futures::{AsyncReadExt, AsyncWriteExt},
    identity::ecdsa,
    PeerId, Stream,
};

use wasm_bindgen::JsValue;

use crate::conventions::*;

use crate::weeb_3::etiquette_0;
use crate::weeb_3::etiquette_1;
use crate::weeb_3::etiquette_2;
// use crate::weeb_3::etiquette_3;
use crate::weeb_3::etiquette_4;
use crate::weeb_3::etiquette_5;
use crate::weeb_3::etiquette_6;

use crate::HANDSHAKE_PROTOCOL;
use crate::PSEUDOSETTLE_PROTOCOL;
use crate::RETRIEVAL_PROTOCOL;

pub async fn ceive(
    peer: PeerId,
    stream: &mut Stream,
    a: libp2p::core::Multiaddr,
    pk: &ecdsa::SecretKey,
    chan: &mpsc::Sender<PeerFile>,
) -> io::Result<()> {
    let mut step_0 = etiquette_1::Syn::default();

    step_0.observed_underlay = a.clone().to_vec();

    let mut bufw_0 = Vec::new();

    let step_0_len = step_0.encoded_len();

    bufw_0.reserve(step_0_len + prost::length_delimiter_len(step_0_len));
    step_0.encode_length_delimited(&mut bufw_0).unwrap();

    stream.write_all(&bufw_0).await?;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let rec_0_u = etiquette_1::SynAck::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_x) => {
            let _ = stream.close().await;
            return Ok(());
        }
    };

    let underlay = libp2p::core::Multiaddr::try_from(rec_0.syn.unwrap().observed_underlay).unwrap();

    let peer_overlay = rec_0.ack.unwrap().address.unwrap().overlay;

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
    let byteslice3 = [hsprefix.to_vec(), underlay.to_vec()].concat();
    let byteslice4 = [byteslice3, overlay.to_vec()].concat();
    let byteslice5 = [byteslice4, bufidb.to_vec()].concat();

    let signature = signer.sign_message(&byteslice5).await.unwrap();

    let mut step_1_ad = etiquette_1::BzzAddress::default();

    step_1_ad.overlay = overlay.to_vec();
    step_1_ad.underlay = underlay.to_vec();
    step_1_ad.signature = signature.as_bytes().to_vec();

    step_1.address = Some(step_1_ad);
    step_1.nonce = nonce.to_vec();
    step_1.network_id = 10_u64;
    step_1.full_node = false;
    step_1.welcome_message = "... Ara Ara ...".to_string();

    let mut bufw_1 = Vec::new();

    let step_1_len = step_1.encoded_len();

    bufw_1.reserve(step_1_len + prost::length_delimiter_len(step_1_len));
    step_1.encode_length_delimited(&mut bufw_1).unwrap();
    stream.write_all(&bufw_1).await?;

    let _ = stream.close().await;
    web_sys::console::log_1(&JsValue::from(format!("Connected Peer {:#?}!", peer)));

    chan.send(PeerFile {
        peer_id: peer,
        overlay: peer_overlay.clone(),
    })
    .unwrap();

    Ok(())
}

pub async fn pricing_handler(
    peer: PeerId,
    mut stream: Stream,
    chan: &mpsc::Sender<(PeerId, u64)>,
) -> io::Result<()> {
    web_sys::console::log_1(&JsValue::from(format!(
        "Opened Pricing handle 2 for peer !",
    )));

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
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

    stream.write_all(&buf_empty).await?;
    let _ = stream.flush().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let _ = stream.flush().await;
    let _ = stream.close().await;

    let rec_0_u = etiquette_4::AnnouncePaymentThreshold::decode_length_delimited(&mut Cursor::new(
        buf_nondiscard_0,
    ));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_x) => {
            return Ok(());
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

    Ok(())
}

pub async fn gossip_handler(
    _peer: PeerId,
    mut stream: Stream,
    chan: &mpsc::Sender<etiquette_2::BzzAddress>,
) -> io::Result<()> {
    web_sys::console::log_1(&JsValue::from(
        format!("Opened Gossip Handle 2 for peer !",),
    ));

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
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

    stream.write_all(&buf_empty).await?;
    let _ = stream.flush().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let _ = stream.flush().await;
    let _ = stream.close().await;

    let rec_0_u = etiquette_2::Peers::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_x) => {
            return Ok(());
        }
    };

    // web_sys::console::log_1(&JsValue::from(format!("Got Peers Message {:#?}!", rec_0)));

    for peer in rec_0.peers {
        web_sys::console::log_1(&JsValue::from(format!(
            "Got Peer {:#?}!",
            hex::encode(&peer.overlay)
        )));
        chan.send(peer).unwrap();
    }

    Ok(())
}

pub async fn fresh(
    peer: PeerId,
    amount: u64,
    stream: &mut Stream,
    chan: &mpsc::Sender<(PeerId, u64)>,
) -> io::Result<()> {
    web_sys::console::log_1(&JsValue::from(format!(
        "Opened Refresh Handle 2 for peer ! {}",
        amount
    )));

    let empty = etiquette_0::Headers::default();

    let mut buf_empty = Vec::new();

    let empty_len = empty.encoded_len();
    buf_empty.reserve(empty_len + prost::length_delimiter_len(empty_len));
    empty.encode_length_delimited(&mut buf_empty).unwrap();

    stream.write_all(&buf_empty).await?;
    let _ = stream.flush().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
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
    stream.write_all(&bufw_1).await?;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let _ = stream.close().await;
    let rec_0_u =
        etiquette_5::PaymentAck::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_x) => {
            return Ok(());
        }
    };

    let refr_am = BigUint::from_bytes_be(&rec_0.amount).to_u64().unwrap();

    web_sys::console::log_1(&JsValue::from(format!(
        "Accepted Refresh {:#?} from peer {:#?}!",
        refr_am, peer
    )));

    if amount > 0 {
        chan.send((peer, refr_am)).unwrap();
    }

    Ok(())
}

pub async fn trieve(
    peer: PeerId,
    chunk_address: Vec<u8>,
    stream: &mut Stream,
    chan: &mpsc::Sender<Vec<u8>>,
) -> io::Result<()> {
    web_sys::console::log_1(&JsValue::from(format!(
        "Opened Retrieve Handle 2 for peer !",
    )));

    let empty = etiquette_0::Headers::default();

    let mut buf_empty = Vec::new();

    let empty_len = empty.encoded_len();
    buf_empty.reserve(empty_len + prost::length_delimiter_len(empty_len));
    empty.encode_length_delimited(&mut buf_empty).unwrap();

    stream.write_all(&buf_empty).await?;
    let _ = stream.flush().await;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
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
    stream.write_all(&bufw_1).await?;

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let _ = stream.close().await;

    let rec_0_u =
        etiquette_6::Delivery::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_x) => {
            return Ok(());
        }
    };

    let rec_1 = rec_0.data;

    web_sys::console::log_1(&JsValue::from(format!(
        "Got chunk {:#?} from peer {:#?}!",
        rec_0.stamp, peer
    )));

    chan.send(rec_1).unwrap();

    Ok(())
}

pub async fn connection_handler(
    peer: PeerId,
    control: &mut stream::Control,
    a: &libp2p::core::Multiaddr,
    pk: &ecdsa::SecretKey,
    chan: &mpsc::Sender<PeerFile>,
) {
    let mut stream = match control.open_stream(peer, HANDSHAKE_PROTOCOL).await {
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

    if let Err(e) = ceive(peer, &mut stream, a.clone(), &pk.clone(), chan).await {
        web_sys::console::log_1(&JsValue::from("Handshake protocol failed"));
        web_sys::console::log_1(&JsValue::from(format!("{}", e)));
        return;
    }

    web_sys::console::log_1(&JsValue::from(format!("{} Handshake complete!", peer)));

    web_sys::console::log_1(&JsValue::from(format!("Closing handler 1")));
}

pub async fn refresh_handler(
    peer: PeerId,
    amount: u64,
    control: &mut stream::Control,
    chan: &mpsc::Sender<(PeerId, u64)>,
) {
    let mut stream = match control.open_stream(peer, PSEUDOSETTLE_PROTOCOL).await {
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

    if let Err(e) = fresh(peer, amount, &mut stream, chan).await {
        web_sys::console::log_1(&JsValue::from("Refresh protocol failed"));
        web_sys::console::log_1(&JsValue::from(format!("{}", e)));
        return;
    }

    web_sys::console::log_1(&JsValue::from(format!("Refresh complete for {}!", peer)));
}

pub async fn retrieve_handler(
    peer: PeerId,
    chunk_address: Vec<u8>,
    control: &mut stream::Control,
    chan: &mpsc::Sender<Vec<u8>>,
) {
    let mut stream = match control.open_stream(peer, RETRIEVAL_PROTOCOL).await {
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

    if let Err(e) = trieve(peer, chunk_address, &mut stream, chan).await {
        web_sys::console::log_1(&JsValue::from("Retrieve protocol failed"));
        web_sys::console::log_1(&JsValue::from(format!("{}", e)));
        return;
    }

    web_sys::console::log_1(&JsValue::from(format!("{} Retrieve complete!", peer)));
}
