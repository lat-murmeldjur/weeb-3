use alloy::primitives::keccak256;
use alloy::signers::Signer;
use alloy::signers::local::PrivateKeySigner;

use prost::Message;

use crate::mpsc;
use std::collections::HashMap;
use std::io::Cursor;

use crate::{OpenStreamError, StreamControl};
use libp2p::{
    PeerId, Stream, StreamProtocol,
    futures::{AsyncReadExt, AsyncWriteExt},
    swarm::ConnectionId,
};

use web3::types::U256;

use crate::conventions::*;
use async_std::sync::{Arc, Mutex};

use crate::weeb_3::etiquette_0;
use crate::weeb_3::etiquette_1;
use crate::weeb_3::etiquette_2;
use crate::weeb_3::etiquette_4;
use crate::weeb_3::etiquette_5;
use crate::weeb_3::etiquette_6;
use crate::weeb_3::etiquette_7;
use crate::weeb_3::etiquette_8;

use crate::persistence::{
    get_chequebook_address, get_chequebook_last_issued_cheque_payout, get_chequebook_signer_key,
    set_chequebook_last_issued_cheque_payout,
};
use crate::{network_profile::active_profile, on_chain::ChequebookClient};
use ethers::signers::LocalWallet;
use ethers::types::{Address as EthAddress, U256 as EthU256};

use crate::HANDSHAKE_PROTOCOL;
use crate::PSEUDOSETTLE_PROTOCOL;
use crate::PUSHSYNC_PROTOCOL;
use crate::RETRIEVAL_PROTOCOL;
use crate::SWAP_PROTOCOL;
use crate::{OutboundProtocolSession, TransportConnectionSession};

const CONTROL_PROTOCOL_MAX_FRAME_BYTES: u64 = 64 * 1024;
const HIVE_PROTOCOL_MAX_FRAME_BYTES: u64 = 128 * 1024;

fn trimmed_big_endian(bytes: &[u8]) -> Vec<u8> {
    let first = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    bytes[first..].to_vec()
}

fn decode_big_endian_u64(bytes: &[u8]) -> Option<u64> {
    let bytes = &bytes[bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len())..];
    if bytes.len() > 8 {
        return None;
    }
    let mut value = [0_u8; 8];
    value[8 - bytes.len()..].copy_from_slice(bytes);
    Some(u64::from_be_bytes(value))
}

struct OutgoingChequeState {
    beneficiary_bytes: Vec<u8>,
    beneficiary: EthAddress,
    chequebook_bytes: Vec<u8>,
    chequebook: EthAddress,
    effective_deduction: EthU256,
    cumulative_payout: EthU256,
}

async fn read_control_protocol_frame(stream: &mut Stream) -> Option<Vec<u8>> {
    read_control_protocol_frame_bounded(stream, CONTROL_PROTOCOL_MAX_FRAME_BYTES).await
}

async fn read_control_protocol_frame_bounded(stream: &mut Stream, maximum: u64) -> Option<Vec<u8>> {
    let mut frame_len = 0_u64;
    for shift in (0_u32..64).step_by(7) {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await.ok()?;
        let value = u64::from(byte[0] & 0x7f);
        if value > (u64::MAX >> shift) {
            return None;
        }
        frame_len |= value << shift;
        if frame_len > maximum {
            return None;
        }
        if byte[0] & 0x80 == 0 {
            let mut frame = vec![0_u8; usize::try_from(frame_len).ok()?];
            stream.read_exact(&mut frame).await.ok()?;
            return Some(frame);
        }
    }
    None
}

async fn prepare_outgoing_cheque_state(
    peer: &PeerId,
    amount: u64,
    beneficiaries: Arc<Mutex<HashMap<PeerId, (web3::types::Address, bool)>>>,
    price: U256,
    deduction: U256,
) -> Option<OutgoingChequeState> {
    let beneficiary_bytes_opt = {
        let map = beneficiaries.lock().await;
        map.get(peer).cloned()
    };
    let beneficiary_bytes = beneficiary_bytes_opt?.0.as_bytes().to_vec();
    let beneficiary = EthAddress::from_slice(&beneficiary_bytes);

    let chequebook_bytes = get_chequebook_address().await;
    if chequebook_bytes.len() != 20 {
        return None;
    }
    let chequebook = EthAddress::from_slice(&chequebook_bytes);

    let last_payout_bytes =
        get_chequebook_last_issued_cheque_payout(&chequebook_bytes, &beneficiary_bytes).await;
    let stored_cumulative_payout = if last_payout_bytes.is_empty() {
        EthU256::zero()
    } else if last_payout_bytes.len() <= 32 {
        let mut last_payout_buf = [0u8; 32];
        let start = 32 - last_payout_bytes.len();
        last_payout_buf[start..].copy_from_slice(&last_payout_bytes);
        EthU256::from_big_endian(&last_payout_buf)
    } else {
        return None;
    };

    let effective_deduction = if stored_cumulative_payout.is_zero() {
        EthU256::from(deduction)
    } else {
        EthU256::zero()
    };
    let cheque_delta = EthU256::from(amount).checked_mul(EthU256::from(price))?;
    let cumulative_payout = stored_cumulative_payout
        .checked_add(cheque_delta)?
        .checked_add(effective_deduction)?;

    return Some(OutgoingChequeState {
        beneficiary_bytes,
        beneficiary,
        chequebook_bytes,
        chequebook,
        effective_deduction,
        cumulative_payout,
    });
}

pub async fn ceive(
    peer: PeerId,
    connection_attempt_id: usize,
    connection_id: ConnectionId,
    network_id: u64,
    self_ephemeral: libp2p::core::Multiaddr,
    mut stream: Stream,
    a: libp2p::core::Multiaddr,
    signer: &PrivateKeySigner,
    chan: &mpsc::Sender<PeerFile>,
) -> bool {
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
    if stream.flush().await.is_err() {
        return false;
    }

    let Some(handshake_frame) = read_control_protocol_frame(&mut stream).await else {
        return false;
    };

    let rec_0_u = etiquette_1::SynAck::decode(&mut Cursor::new(handshake_frame));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_x) => {
            return false;
        }
    };

    let underlay = self_ephemeral.to_vec();

    let Some(ack) = rec_0.ack.as_ref() else {
        return false;
    };
    let Some(peer_address) = ack.address.as_ref() else {
        return false;
    };
    if peer_address.overlay.len() != 32 {
        return false;
    }

    let peer_overlay = peer_address.overlay.clone();
    let beneficiary = parse_address(
        &peer_address.underlay,
        &peer_address.overlay,
        &peer_address.signature,
        &peer_address.nonce,
        peer_address.timestamp,
        network_id,
        &peer_address.chequebook_address,
    );
    if beneficiary == web3::types::Address::zero() {
        return false;
    }

    let overlay;
    let signature;
    let nonce: [u8; 32] = [0; 32];
    let timestamp = (js_sys::Date::now() / 1000.0).floor() as i64;
    let chequebook_address = EMPTY_CHEQUEBOOK_ADDRESS.to_vec();
    let mut step_1 = etiquette_1::Ack::default();
    {
        let addrep = signer.address();
        let addre = addrep.to_vec();

        let bufidl = network_id.to_le_bytes();
        let byteslice = [addre.as_slice(), &bufidl].concat();

        let byteslice2 = [byteslice, (&nonce).to_vec()].concat();
        let overlayp = keccak256(byteslice2);
        overlay = overlayp;

        let byteslice5 = generate_sign_data(
            &underlay,
            overlay.as_slice(),
            network_id,
            &nonce,
            timestamp,
            &chequebook_address,
        );

        signature = signer.sign_message(&byteslice5).await.unwrap();
    }

    let mut step_1_ad = etiquette_1::BzzAddress::default();

    step_1_ad.overlay = overlay.to_vec();
    step_1_ad.underlay = underlay.to_vec();
    step_1_ad.signature = signature.as_bytes().to_vec();
    step_1_ad.nonce = nonce.to_vec();
    step_1_ad.timestamp = timestamp;
    step_1_ad.chequebook_address = chequebook_address;

    step_1.address = Some(step_1_ad);
    step_1.network_id = network_id;
    step_1.full_node = false;
    step_1.welcome_message = "... Ara Ara ...".to_string();

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
    if stream.flush().await.is_err() {
        return false;
    }

    let _ = stream.close().await;

    if chan
        .try_send(PeerFile {
            peer_id: peer,
            overlay: peer_overlay.clone(),
            beneficiary: beneficiary,
            connection_attempt_id,
            connection_id,
        })
        .is_err()
    {
        return false;
    }

    return true;
}

pub async fn pricing_handler(
    peer: PeerId,
    mut stream: Stream,
    session: TransportConnectionSession,
    chan: &mpsc::Sender<(PeerId, u64, TransportConnectionSession)>,
) {
    if read_control_protocol_frame(&mut stream).await.is_none() {
        return;
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

    let Some(announce_frame) = read_control_protocol_frame(&mut stream).await else {
        return;
    };
    let rec_0_u = etiquette_4::AnnouncePaymentThreshold::decode(&mut Cursor::new(announce_frame));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_x) => {
            return;
        }
    };

    let Some(pt) = decode_big_endian_u64(&rec_0.payment_threshold) else {
        return;
    };

    if !session.is_current() {
        return;
    }

    let _ = chan.try_send((peer, pt, session));
}

pub async fn gossip_handler(
    _peer: PeerId,
    mut stream: Stream,
    chan: &mpsc::Sender<(etiquette_2::BzzAddress, u64)>,
    generation: u64,
) {
    if read_control_protocol_frame(&mut stream).await.is_none() {
        return;
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

    let Some(peers_frame) =
        read_control_protocol_frame_bounded(&mut stream, HIVE_PROTOCOL_MAX_FRAME_BYTES).await
    else {
        return;
    };

    let rec_0_u = etiquette_2::Peers::decode(&mut Cursor::new(peers_frame));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_) => {
            return;
        }
    };

    for peer in rec_0.peers {
        if chan.try_send((peer, generation)).is_err() {
            return;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RefreshmentOutcome {
    NotDispatched,
    Acknowledged(u64),
    AmbiguousAfterPayment,
}

pub async fn fresh(amount: u64, mut stream: Stream) -> RefreshmentOutcome {
    let empty = etiquette_0::Headers::default();

    let mut buf_empty = Vec::new();

    let empty_len = empty.encoded_len();
    buf_empty.reserve(empty_len + prost::length_delimiter_len(empty_len));
    empty.encode_length_delimited(&mut buf_empty).unwrap();

    match stream.write_all(&buf_empty).await {
        Ok(_) => {}
        Err(_) => return RefreshmentOutcome::NotDispatched,
    };
    if stream.flush().await.is_err() {
        return RefreshmentOutcome::NotDispatched;
    }

    if read_control_protocol_frame(&mut stream).await.is_none() {
        return RefreshmentOutcome::NotDispatched;
    }

    let mut step_1 = etiquette_5::Payment::default();

    step_1.amount = trimmed_big_endian(&amount.to_be_bytes());

    let mut bufw_1 = Vec::new();

    let step_1_len = step_1.encoded_len();

    bufw_1.reserve(step_1_len + prost::length_delimiter_len(step_1_len));
    step_1.encode_length_delimited(&mut bufw_1).unwrap();
    match stream.write_all(&bufw_1).await {
        Ok(_) => {}
        Err(_) => return RefreshmentOutcome::AmbiguousAfterPayment,
    };
    if stream.flush().await.is_err() || stream.close().await.is_err() {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    }

    let Some(ack_frame) = read_control_protocol_frame(&mut stream).await else {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    };
    let rec_0_u = etiquette_5::PaymentAck::decode(&mut Cursor::new(ack_frame));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_) => {
            return RefreshmentOutcome::AmbiguousAfterPayment;
        }
    };

    let Some(refr_am) = decode_big_endian_u64(&rec_0.amount) else {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    };

    if refr_am > amount {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    }
    RefreshmentOutcome::Acknowledged(refr_am)
}

pub async fn issue(
    peer: PeerId,
    amount: u64,
    mut stream: Stream,
    chan: &mpsc::Sender<(PeerId, bool)>,
    beneficiaries: Arc<Mutex<HashMap<PeerId, (web3::types::Address, bool)>>>,
    price: U256,
    deduction: U256,
) {
    let signer_key = get_chequebook_signer_key().await;
    if signer_key.len() != 32 {
        let _ = chan.try_send((peer, false));
        return;
    }

    let wallet = match LocalWallet::from_bytes(&signer_key) {
        Ok(w) => w,
        Err(_) => {
            let _ = chan.try_send((peer, false));
            return;
        }
    };

    let cheque_state =
        match prepare_outgoing_cheque_state(&peer, amount, beneficiaries.clone(), price, deduction)
            .await
        {
            Some(state) => state,
            None => {
                let _ = chan.try_send((peer, false));
                return;
            }
        };

    let mut non_empty = etiquette_0::Headers::default();

    let mut price_header = etiquette_0::Header::default();
    price_header.key = "exchange".to_string();

    let mut buf = [0u8; 32];
    price.to_big_endian(&mut buf);
    price_header.value = trimmed_big_endian(&buf);

    let mut deduction_header = etiquette_0::Header::default();
    deduction_header.key = "deduction".to_string();

    let mut buf = [0u8; 32];
    cheque_state.effective_deduction.to_big_endian(&mut buf);
    deduction_header.value = trimmed_big_endian(&buf);

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
            let _ = chan.try_send((peer, false));
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
                let _ = chan.try_send((peer, false));
                return;
            }
        };
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let client = ChequebookClient::new(
        cheque_state.chequebook,
        wallet,
        active_profile().wallet_chain_id,
    );

    let cheque_json = match client
        .prepare_emit_cheque_bytes(cheque_state.beneficiary, cheque_state.cumulative_payout)
    {
        Some(cheque_data) => cheque_data,
        None => {
            let _ = chan.try_send((peer, false));
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
        let _ = chan.try_send((peer, false));
        return;
    }

    let _ = stream.flush().await;

    let mut cumulative_payout_buf = [0u8; 32];
    cheque_state
        .cumulative_payout
        .to_big_endian(&mut cumulative_payout_buf);
    let cumulative_payout_bytes = cumulative_payout_buf.to_vec();
    if !set_chequebook_last_issued_cheque_payout(
        &cheque_state.chequebook_bytes,
        &cheque_state.beneficiary_bytes,
        &cumulative_payout_bytes,
    )
    .await
    {
        let _ = stream.close().await;
        let _ = chan.try_send((peer, false));
        return;
    }

    let _ = stream.close().await;

    let _ = chan.try_send((peer, true));
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

    if read_control_protocol_frame(&mut stream).await.is_none() {
        return;
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

    let Some(delivery_frame) = read_control_protocol_frame(&mut stream).await else {
        return;
    };
    let rec_0_u = etiquette_6::Delivery::decode(&mut Cursor::new(delivery_frame));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_) => {
            return;
        }
    };

    let rec_1 = rec_0.data;

    if chan.try_send(rec_1).is_err() {}
}

pub async fn connection_handler(
    peer: PeerId,
    connection_attempt_id: usize,
    session: TransportConnectionSession,
    network_id: u64,
    self_ephemeral: libp2p::core::Multiaddr,
    mut control: StreamControl,
    a: &libp2p::core::Multiaddr,
    signer: &PrivateKeySigner,
    chan: &mpsc::Sender<PeerFile>,
) -> bool {
    if !session.is_current() {
        return false;
    }
    let stream = match control.open_stream(peer, HANDSHAKE_PROTOCOL).await {
        Ok(stream) => stream,
        Err(OpenStreamError::UnsupportedProtocol(_)) => {
            return false;
        }
        Err(_) => {
            return false;
        }
    };
    if !session.is_current() {
        drop(stream);
        return false;
    }

    if !ceive(
        peer,
        connection_attempt_id,
        session.connection_id(),
        network_id,
        self_ephemeral,
        stream,
        a.clone(),
        signer,
        chan,
    )
    .await
    {
        return false;
    }

    return true;
}

async fn open_current_outbound_stream(
    peer: PeerId,
    mut control: StreamControl,
    protocol: StreamProtocol,
    session: &OutboundProtocolSession,
) -> Option<Stream> {
    if !session.is_current() {
        return None;
    }
    let stream = match control.open_stream(peer, protocol).await {
        Ok(stream) => stream,
        Err(OpenStreamError::UnsupportedProtocol(_)) => {
            return None;
        }
        Err(_) => {
            return None;
        }
    };
    if !session.is_current() {
        drop(stream);
        return None;
    }
    Some(stream)
}

pub async fn refresh_handler(
    peer: PeerId,
    amount: u64,
    control: StreamControl,
    session: OutboundProtocolSession,
) -> RefreshmentOutcome {
    let Some(stream) =
        open_current_outbound_stream(peer, control, PSEUDOSETTLE_PROTOCOL, &session).await
    else {
        return RefreshmentOutcome::NotDispatched;
    };

    fresh(amount, stream).await
}

pub async fn issue_handler(
    peer: PeerId,
    amount: u64,
    control: StreamControl,
    session: OutboundProtocolSession,
    chan: &mpsc::Sender<(PeerId, bool)>,
    beneficiaries: Arc<Mutex<HashMap<PeerId, (web3::types::Address, bool)>>>,
    price: U256,
    deduction: U256,
) {
    let Some(stream) = open_current_outbound_stream(peer, control, SWAP_PROTOCOL, &session).await
    else {
        let _ = chan.try_send((peer, false));
        return;
    };

    issue(peer, amount, stream, chan, beneficiaries, price, deduction).await;
}

pub async fn retrieve_handler(
    peer: PeerId,
    chunk_address: Vec<u8>,
    control: StreamControl,
    session: OutboundProtocolSession,
    chan: &mpsc::Sender<Vec<u8>>,
) {
    let Some(stream) =
        open_current_outbound_stream(peer, control, RETRIEVAL_PROTOCOL, &session).await
    else {
        return;
    };

    trieve(peer, chunk_address, stream, chan).await;
}

pub async fn pushsync_handler(
    peer: PeerId,
    chunk_address: &Vec<u8>,
    chunk_content: &Vec<u8>,
    chunk_stamp: &Vec<u8>,
    control: StreamControl,
    session: OutboundProtocolSession,
    chan: &mpsc::Sender<bool>,
) {
    let Some(stream) =
        open_current_outbound_stream(peer, control, PUSHSYNC_PROTOCOL, &session).await
    else {
        return;
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
            let _ = chan.try_send(false);
            return;
        }
    };
    let _ = stream.flush().await;

    if read_control_protocol_frame(&mut stream).await.is_none() {
        let _ = chan.try_send(false);
        return;
    }

    let mut step_1 = etiquette_7::Delivery::default();

    step_1.address = chunk_address.to_vec();
    step_1.data = chunk_content.to_vec();
    step_1.stamp = chunk_stamp.to_vec();

    let bufw_1 = step_1.encode_length_delimited_to_vec();
    if stream.write_all(&bufw_1).await.is_err() || stream.flush().await.is_err() {
        let _ = chan.try_send(false);
        return;
    }

    let _ = stream.close().await;

    let Some(receipt_frame) = read_control_protocol_frame(&mut stream).await else {
        let _ = chan.try_send(false);
        return;
    };
    let rec_0_u = etiquette_7::Receipt::decode(&mut Cursor::new(receipt_frame));

    let rec_0 = match rec_0_u {
        Ok(x) => x,
        Err(_) => {
            let _ = chan.try_send(false);
            return;
        }
    };

    if !rec_0.err.is_empty() {
        let _ = chan.try_send(false);
        return;
    }

    if rec_0.address.as_slice() != chunk_address.as_slice() {
        let _ = chan.try_send(false);
        return;
    }

    if rec_0.signature.is_empty() {
        let _ = chan.try_send(false);
        return;
    }

    let _ = chan.try_send(true);
}
