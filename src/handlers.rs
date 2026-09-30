use std::{cell::Cell, time::Duration};

use bytes::Bytes;
use prost::Message;

use crate::mpsc;

use crate::{PrivateKeySigner, StreamControl};
use libp2p::{
    PeerId, Stream, StreamProtocol,
    futures::{AsyncReadExt, AsyncWriteExt},
    swarm::ConnectionId,
};

use web3::types::{Address, U256};

use crate::conventions::*;
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

use crate::HANDSHAKE_PROTOCOL;
use crate::PSEUDOSETTLE_PROTOCOL;
use crate::PUSHSYNC_PROTOCOL;
use crate::RETRIEVAL_PROTOCOL;
use crate::SWAP_PROTOCOL;
use crate::{OutboundProtocolSession, PeerDialInstruction, TransportConnectionSession};

const CONTROL_PROTOCOL_MAX_FRAME_BYTES: u64 = 64 * 1024;
const HIVE_PROTOCOL_MAX_FRAME_BYTES: u64 = 128 * 1024;
const EMPTY_HEADERS_FRAME: &[u8] = &[0];

fn significant_big_endian(bytes: &[u8]) -> &[u8] {
    &bytes[bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len())..]
}

fn trimmed_big_endian(bytes: &[u8]) -> Vec<u8> {
    significant_big_endian(bytes).to_vec()
}

fn decode_big_endian_u64(bytes: &[u8]) -> Option<u64> {
    let bytes = significant_big_endian(bytes);
    if bytes.len() > 8 {
        return None;
    }
    let mut value = [0_u8; 8];
    value[8 - bytes.len()..].copy_from_slice(bytes);
    Some(u64::from_be_bytes(value))
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

async fn handshake_exchange(
    peer: PeerId,
    local_peer: PeerId,
    connection_attempt_id: usize,
    connection_id: ConnectionId,
    network_id: u64,
    mut stream: Stream,
    observed_underlay: &libp2p::core::Multiaddr,
    signer: &PrivateKeySigner,
    connected_peers: &mpsc::Sender<PeerFile>,
) -> Option<()> {
    let syn = etiquette_1::Syn {
        observed_underlay: observed_underlay.to_vec(),
    };

    let syn_frame = syn.encode_length_delimited_to_vec();
    stream.write_all(&syn_frame).await.ok()?;
    stream.flush().await.ok()?;

    let handshake_frame = read_control_protocol_frame(&mut stream).await?;
    let syn_ack = etiquette_1::SynAck::decode(handshake_frame.as_slice()).ok()?;
    let syn = syn_ack.syn?;
    let observed_underlays = crate::addresses::deserialize_underlays(&syn.observed_underlay);
    if observed_underlays.is_empty()
        || observed_underlays
            .iter()
            .any(|underlay| try_from_multiaddr(underlay).as_ref() != Some(&local_peer))
    {
        return None;
    }
    let underlay = syn.observed_underlay;

    let ack = syn_ack.ack?;
    if ack.network_id != network_id {
        return None;
    }
    let peer_address = ack.address?;
    let peer_overlay: [u8; 32] = peer_address.overlay.as_slice().try_into().ok()?;

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
        return None;
    }

    let nonce: [u8; 32] = [0; 32];
    let timestamp = (js_sys::Date::now() / 1000.0).floor() as i64;
    let chequebook_address = EMPTY_CHEQUEBOOK_ADDRESS.to_vec();
    let mut overlay_input = [0_u8; 60];
    overlay_input[..20].copy_from_slice(signer.address().as_bytes());
    overlay_input[20..28].copy_from_slice(&network_id.to_le_bytes());
    overlay_input[28..].copy_from_slice(&nonce);
    let overlay = keccak256(&overlay_input);
    let sign_data = generate_sign_data(
        &underlay,
        overlay.as_slice(),
        network_id,
        &nonce,
        timestamp,
        &chequebook_address,
    );
    let signature = signer.sign_message(&sign_data).ok()?;

    let ack = etiquette_1::Ack {
        address: Some(etiquette_1::BzzAddress {
            overlay: overlay.to_vec(),
            underlay,
            signature: signature.to_vec(),
            nonce: nonce.to_vec(),
            timestamp,
            chequebook_address,
        }),
        network_id,
        full_node: false,
        welcome_message: "... Ara Ara ...".to_string(),
    };

    let ack_frame = ack.encode_length_delimited_to_vec();
    stream.write_all(&ack_frame).await.ok()?;
    stream.flush().await.ok()?;

    let _ = stream.close().await;

    connected_peers
        .try_send(PeerFile {
            peer_id: peer,
            overlay: peer_overlay,
            beneficiary,
            connection_attempt_id,
            connection_id,
        })
        .ok()
}

pub async fn pricing_handler(
    peer: PeerId,
    mut stream: Stream,
    session: TransportConnectionSession,
    pricing_updates: &mpsc::Sender<(PeerId, u64, TransportConnectionSession)>,
) {
    if read_control_protocol_frame(&mut stream).await.is_none()
        || stream.write_all(EMPTY_HEADERS_FRAME).await.is_err()
    {
        return;
    }
    let _ = stream.flush().await;
    let _ = stream.close().await;

    let Some(announce_frame) = read_control_protocol_frame(&mut stream).await else {
        return;
    };
    let Ok(announcement) = etiquette_4::AnnouncePaymentThreshold::decode(announce_frame.as_slice())
    else {
        return;
    };

    let Some(payment_threshold) = decode_big_endian_u64(&announcement.payment_threshold) else {
        return;
    };

    if !session.is_current() {
        return;
    }
    let _ = pricing_updates.try_send((peer, payment_threshold, session));
}

pub async fn gossip_handler(
    mut stream: Stream,
    peer_dials: &mpsc::Sender<PeerDialInstruction>,
    generation: u64,
) {
    if read_control_protocol_frame(&mut stream).await.is_none()
        || stream.write_all(EMPTY_HEADERS_FRAME).await.is_err()
    {
        return;
    }
    let _ = stream.flush().await;
    let _ = stream.close().await;

    let Some(peers_frame) =
        read_control_protocol_frame_bounded(&mut stream, HIVE_PROTOCOL_MAX_FRAME_BYTES).await
    else {
        return;
    };

    let Ok(peers) = etiquette_2::Peers::decode(peers_frame.as_slice()) else {
        return;
    };

    for peer in peers.peers {
        if peer_dials
            .send(PeerDialInstruction {
                underlay: peer.underlay,
                generation,
                retry: false,
                bootnode: false,
            })
            .await
            .is_err()
        {
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

async fn refreshment_exchange(
    amount: u64,
    mut stream: Stream,
    timeout_outcome: &Cell<RefreshmentOutcome>,
) -> RefreshmentOutcome {
    if stream.write_all(EMPTY_HEADERS_FRAME).await.is_err() {
        return RefreshmentOutcome::NotDispatched;
    }
    if stream.flush().await.is_err() {
        return RefreshmentOutcome::NotDispatched;
    }

    if read_control_protocol_frame(&mut stream).await.is_none() {
        return RefreshmentOutcome::NotDispatched;
    }

    let payment = etiquette_5::Payment {
        amount: trimmed_big_endian(&amount.to_be_bytes()),
    };

    let payment_frame = payment.encode_length_delimited_to_vec();
    timeout_outcome.set(RefreshmentOutcome::AmbiguousAfterPayment);
    if stream.write_all(&payment_frame).await.is_err() {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    }
    if stream.flush().await.is_err() || stream.close().await.is_err() {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    }

    let Some(ack_frame) = read_control_protocol_frame(&mut stream).await else {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    };
    let Ok(ack) = etiquette_5::PaymentAck::decode(ack_frame.as_slice()) else {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    };

    let Some(acknowledged_amount) = decode_big_endian_u64(&ack.amount) else {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    };

    if acknowledged_amount > amount {
        return RefreshmentOutcome::AmbiguousAfterPayment;
    }
    RefreshmentOutcome::Acknowledged(acknowledged_amount)
}

async fn cheque_exchange(
    amount: u64,
    mut stream: Stream,
    beneficiary: Address,
    price: U256,
    deduction: U256,
) -> Option<()> {
    let signer_key = get_chequebook_signer_key().await;
    if signer_key.len() != 32 {
        return None;
    }

    let wallet = PrivateKeySigner::from_slice(&signer_key).ok()?;
    let (chequebook, effective_deduction, cumulative_payout) = {
        let chequebook_bytes = get_chequebook_address().await;
        if chequebook_bytes.len() != 20 {
            return None;
        }
        let chequebook = Address::from_slice(&chequebook_bytes);
        let last_payout_bytes =
            get_chequebook_last_issued_cheque_payout(chequebook.as_bytes(), beneficiary.as_bytes())
                .await;
        let stored_cumulative_payout = match last_payout_bytes.len() {
            0 => U256::zero(),
            1..=32 => U256::from_big_endian(&last_payout_bytes),
            _ => return None,
        };
        let effective_deduction = if stored_cumulative_payout.is_zero() {
            deduction
        } else {
            U256::zero()
        };
        let cheque_delta = U256::from(amount).checked_mul(price)?;
        let cumulative_payout = stored_cumulative_payout
            .checked_add(cheque_delta)?
            .checked_add(effective_deduction)?;
        (chequebook, effective_deduction, cumulative_payout)
    };

    let mut buf = [0u8; 32];
    price.to_big_endian(&mut buf);
    let price_header = etiquette_0::Header {
        key: "exchange".to_string(),
        value: trimmed_big_endian(&buf),
    };

    let mut buf = [0u8; 32];
    effective_deduction.to_big_endian(&mut buf);
    let deduction_header = etiquette_0::Header {
        key: "deduction".to_string(),
        value: trimmed_big_endian(&buf),
    };
    let non_empty = etiquette_0::Headers {
        headers: vec![price_header, deduction_header],
    };

    let buf_non_empty = non_empty.encode_length_delimited_to_vec();

    stream.write_all(&buf_non_empty).await.ok()?;
    let _ = stream.flush().await;

    read_control_protocol_frame(&mut stream).await?;

    let client = ChequebookClient::new(chequebook, wallet, active_profile().wallet_chain_id);

    let cheque_json = client.prepare_emit_cheque_bytes(beneficiary, cumulative_payout)?;

    let msg = etiquette_8::EmitCheque {
        cheque: cheque_json,
    };

    let bufw = msg.encode_length_delimited_to_vec();

    stream.write_all(&bufw).await.ok()?;

    let _ = stream.flush().await;

    let mut cumulative_payout_bytes = [0u8; 32];
    cumulative_payout.to_big_endian(&mut cumulative_payout_bytes);
    let saved = set_chequebook_last_issued_cheque_payout(
        chequebook.as_bytes(),
        beneficiary.as_bytes(),
        &cumulative_payout_bytes,
    )
    .await;
    let _ = stream.close().await;
    saved.then_some(())
}

pub async fn connection_handler(
    peer: PeerId,
    local_peer: PeerId,
    connection_attempt_id: usize,
    connection_id: ConnectionId,
    physical_connections: crate::PhysicalConnectionMap,
    network_id: u64,
    mut control: StreamControl,
    observed_underlay: &libp2p::core::Multiaddr,
    signer: &PrivateKeySigner,
    connected_peers: &mpsc::Sender<PeerFile>,
) -> bool {
    let Ok(stream) = control.open_stream(peer, HANDSHAKE_PROTOCOL).await else {
        return false;
    };
    let Some(session) =
        TransportConnectionSession::capture(peer, connection_id, physical_connections)
    else {
        drop(stream);
        return false;
    };

    handshake_exchange(
        peer,
        local_peer,
        connection_attempt_id,
        session.connection_id(),
        network_id,
        stream,
        observed_underlay,
        signer,
        connected_peers,
    )
    .await
    .is_some()
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
    let Ok(stream) = control.open_stream(peer, protocol).await else {
        return None;
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
    let timeout_outcome = Cell::new(RefreshmentOutcome::NotDispatched);
    async_std::future::timeout(Duration::from_secs(10), async {
        let Some(stream) =
            open_current_outbound_stream(peer, control, PSEUDOSETTLE_PROTOCOL, &session).await
        else {
            return RefreshmentOutcome::NotDispatched;
        };
        refreshment_exchange(amount, stream, &timeout_outcome).await
    })
    .await
    .unwrap_or_else(|_| timeout_outcome.get())
}

pub async fn issue_handler(
    peer: PeerId,
    amount: u64,
    control: StreamControl,
    session: OutboundProtocolSession,
    beneficiary: Address,
    price: U256,
    deduction: U256,
) -> bool {
    let Some(stream) = open_current_outbound_stream(peer, control, SWAP_PROTOCOL, &session).await
    else {
        return false;
    };

    cheque_exchange(amount, stream, beneficiary, price, deduction)
        .await
        .is_some()
}

pub async fn retrieve_handler(
    peer: PeerId,
    request: &etiquette_6::Request,
    control: StreamControl,
    session: OutboundProtocolSession,
) -> Option<Bytes> {
    let mut stream = open_current_outbound_stream(peer, control, RETRIEVAL_PROTOCOL, &session).await?;
    if stream.write_all(EMPTY_HEADERS_FRAME).await.is_err() {
        return None;
    }

    read_control_protocol_frame(&mut stream).await?;

    let request_frame = request.encode_length_delimited_to_vec();
    if stream.write_all(&request_frame).await.is_err() {
        return None;
    }
    let _ = stream.close().await;

    let delivery = read_control_protocol_frame(&mut stream).await?;
    decode_retrieval_delivery(delivery)
}

fn decode_retrieval_delivery(delivery: Vec<u8>) -> Option<Bytes> {
    let etiquette_6::Delivery { data, .. } =
        etiquette_6::Delivery::decode(Bytes::from(delivery)).ok()?;
    Some(data)
}

pub async fn pushsync_handler(
    peer: PeerId,
    chunk_address: Vec<u8>,
    chunk_content: Vec<u8>,
    chunk_stamp: Vec<u8>,
    control: StreamControl,
    session: OutboundProtocolSession,
) -> bool {
    let Some(stream) =
        open_current_outbound_stream(peer, control, PUSHSYNC_PROTOCOL, &session).await
    else {
        return false;
    };

    pushsync_exchange(chunk_address, chunk_content, chunk_stamp, stream)
        .await
        .is_some()
}

async fn pushsync_exchange(
    chunk_address: Vec<u8>,
    chunk_content: Vec<u8>,
    chunk_stamp: Vec<u8>,
    mut stream: Stream,
) -> Option<()> {
    stream.write_all(EMPTY_HEADERS_FRAME).await.ok()?;
    let _ = stream.flush().await;

    read_control_protocol_frame(&mut stream).await?;

    let delivery = etiquette_7::Delivery {
        address: chunk_address,
        data: chunk_content,
        stamp: chunk_stamp,
    };

    let delivery_frame = delivery.encode_length_delimited_to_vec();
    stream.write_all(&delivery_frame).await.ok()?;
    stream.flush().await.ok()?;

    let _ = stream.close().await;

    let receipt_frame = read_control_protocol_frame(&mut stream).await?;
    let receipt = etiquette_7::Receipt::decode(receipt_frame.as_slice()).ok()?;

    (receipt.err.is_empty() && receipt.address == delivery.address && !receipt.signature.is_empty())
        .then_some(())
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn delivery_preserves_protobuf_fields_and_empty_data() {
        let fixtures: &[(&[u8], &[u8])] = &[
            (b"", b""),
            (b"\x0a\x03cat", b"cat"),
            (b"\x12\x03\x01\x02\x03\x0a\x03cat\x20\x07", b"cat"),
            (b"\x0a\x03cat\x12\x03\x01\x02\x03\x1a\x03err", b"cat"),
            (b"\x1a\x03err", b""),
            (b"\x0a\x03cat\x0a\x03dog", b"dog"),
        ];
        for (frame, expected) in fixtures {
            assert_eq!(
                decode_retrieval_delivery(frame.to_vec()).as_deref(),
                Some(*expected)
            );
        }
    }

    #[wasm_bindgen_test]
    fn delivery_rejects_truncated_and_invalid_fields() {
        let fixtures: &[&[u8]] = &[
            b"\x0a\x03ca",
            b"\x0a\x80",
            b"\x0a\x03cat\x12\x03\x01\x02",
            b"\x0a\x03cat\x1a\x03er",
            b"\x0a\x03cat\x1a\x01\xff",
            b"\x00",
        ];
        for frame in fixtures {
            assert!(decode_retrieval_delivery(frame.to_vec()).is_none());
        }
    }

    #[wasm_bindgen_test]
    fn delivery_reuses_frame_allocation_after_dropping_stamp() {
        let mut frame = Vec::with_capacity(4214);
        frame.extend_from_slice(b"\x0a\x80\x20");
        frame.extend(std::iter::repeat_n(0xab, 4096));
        frame.extend_from_slice(b"\x12\x71");
        frame.extend(std::iter::repeat_n(0xcd, 113));
        let allocation = frame.as_ptr();
        let capacity = frame.capacity();

        let data = decode_retrieval_delivery(frame).unwrap();

        assert_eq!(data, vec![0xab; 4096]);
        assert_eq!(data.as_ptr(), allocation.wrapping_add(3));
        let data = Vec::from(data);
        assert_eq!(data.as_ptr(), allocation);
        assert_eq!(data.capacity(), capacity);
    }
}
