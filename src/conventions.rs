#![cfg(target_arch = "wasm32")]

use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId, swarm::ConnectionId};

use crate::erasure_coding::CHUNK_SIZE;
pub use crate::erasure_coding::SPAN_SIZE;
use alloy_primitives::{Signature, keccak256, normalize_v};

pub const MAX_PO: u8 = 31;

#[derive(Debug, Clone)]
pub struct PeerFile {
    pub peer_id: PeerId,
    pub overlay: Vec<u8>,
    pub beneficiary: web3::types::Address,
    pub connection_attempt_id: usize,
    pub connection_id: ConnectionId,
}

#[derive(Debug)]
pub struct PeerAccounting {
    pub balance: u64,
    pub surplus_balance: u64,
    pub threshold: u64,
    pub reserve: u64,
    pub refreshment: f64,
    pub refresh_scheduled: bool,
    pub id: PeerId,
    pub connection_id: Option<ConnectionId>,
}

pub fn try_from_multiaddr(address: &Multiaddr) -> Option<PeerId> {
    address.iter().last().and_then(|p| match p {
        Protocol::P2p(hash) => PeerId::from_multihash(hash.into()).ok(),
        _ => None,
    })
}

pub fn get_proximity(one: &[u8], other: &[u8]) -> u8 {
    let compared_bytes = usize::from(MAX_PO / 4 + 1).min(one.len()).min(other.len());
    if compared_bytes == 0 {
        return 0;
    }
    for (index, (&left, &right)) in one.iter().zip(other).take(compared_bytes).enumerate() {
        let difference = left ^ right;
        if difference != 0 {
            return u8::try_from(index * 8 + difference.leading_zeros() as usize).unwrap();
        }
    }
    MAX_PO
}

const SECTION_SIZE: usize = 32;
const SECTION2_SIZE: usize = 2 * SECTION_SIZE;
const BMT_LEAF_COUNT: usize = CHUNK_SIZE / SECTION2_SIZE;
const BMT_LEVEL_COUNT: usize = 7;

type BmtHash = [u8; SECTION_SIZE];

#[inline]
fn hash_pair(left: &BmtHash, right: &BmtHash, input: &mut [u8; SECTION2_SIZE]) -> BmtHash {
    input[..SECTION_SIZE].copy_from_slice(left);
    input[SECTION_SIZE..].copy_from_slice(right);
    keccak256(input.as_slice()).into()
}

fn zero_bmt_nodes() -> [BmtHash; BMT_LEVEL_COUNT] {
    let mut nodes = [[0u8; SECTION_SIZE]; BMT_LEVEL_COUNT];
    let mut pair = [0u8; SECTION2_SIZE];
    nodes[0] = keccak256(pair).into();
    for level in 1..BMT_LEVEL_COUNT {
        let previous = nodes[level - 1];
        nodes[level] = hash_pair(&previous, &previous, &mut pair);
    }
    nodes
}

std::thread_local! {
    static ZERO_BMT_NODES: [BmtHash; BMT_LEVEL_COUNT] = zero_bmt_nodes();
}

fn bmt_root(content: &[u8]) -> Option<BmtHash> {
    if content.len() > CHUNK_SIZE {
        return None;
    }

    let effective_len = content
        .iter()
        .rposition(|&value| value != 0)
        .map_or(0, |index| index + 1);
    if effective_len == 0 {
        return Some(ZERO_BMT_NODES.with(|nodes| nodes[BMT_LEVEL_COUNT - 1]));
    }
    let occupied_leaves = effective_len.div_ceil(SECTION2_SIZE);
    let mut nodes = [[0u8; SECTION_SIZE]; BMT_LEAF_COUNT];
    let mut block = [0u8; SECTION2_SIZE];
    let mut pair = [0u8; SECTION2_SIZE];

    let full_blocks = effective_len / SECTION2_SIZE;
    for (index, section) in content[..full_blocks * SECTION2_SIZE]
        .chunks_exact(SECTION2_SIZE)
        .enumerate()
    {
        nodes[index] = keccak256(section).into();
    }
    if effective_len % SECTION2_SIZE != 0 {
        let start = full_blocks * SECTION2_SIZE;
        block[..effective_len - start].copy_from_slice(&content[start..effective_len]);
        nodes[full_blocks] = keccak256(block).into();
    }

    let mut reduce = |nodes: &mut [BmtHash; BMT_LEAF_COUNT], zero_nodes: Option<&[BmtHash]>| {
        if let Some(zero_nodes) = zero_nodes
            && occupied_leaves % 2 != 0
        {
            nodes[occupied_leaves] = zero_nodes[0];
        }

        let mut width = BMT_LEAF_COUNT;
        let mut occupied = occupied_leaves;
        let mut level = 0usize;
        while width > 1 {
            let next_width = width / 2;
            let next_occupied = occupied.div_ceil(2);

            for index in 0..next_occupied {
                nodes[index] = hash_pair(&nodes[index * 2], &nodes[index * 2 + 1], &mut pair);
            }

            level += 1;
            if next_occupied < next_width && next_occupied % 2 != 0 {
                nodes[next_occupied] = zero_nodes.expect("sparse BMT has zero nodes")[level];
            }

            width = next_width;
            occupied = next_occupied;
        }
        nodes[0]
    };

    if occupied_leaves == BMT_LEAF_COUNT {
        Some(reduce(&mut nodes, None))
    } else {
        Some(ZERO_BMT_NODES.with(|zero_nodes| reduce(&mut nodes, Some(zero_nodes))))
    }
}

fn content_address_array(chunk_content: &[u8]) -> Option<BmtHash> {
    if !(SPAN_SIZE..=SPAN_SIZE + CHUNK_SIZE).contains(&chunk_content.len()) {
        return None;
    }

    let (span, content) = chunk_content.split_at(SPAN_SIZE);
    let root = bmt_root(content)?;
    let mut hash_input = [0u8; SPAN_SIZE + SECTION_SIZE];
    hash_input[..SPAN_SIZE].copy_from_slice(span);
    hash_input[SPAN_SIZE..].copy_from_slice(&root);
    Some(keccak256(hash_input).into())
}

pub fn content_address(chunk_content: &[u8]) -> Vec<u8> {
    content_address_array(chunk_content)
        .map(|hash| hash.to_vec())
        .unwrap_or_default()
}

pub fn valid_cac(chunk_content: &[u8], address: &[u8]) -> bool {
    content_address_array(chunk_content).is_some_and(|expected| address == expected.as_slice())
}

pub fn valid_soc(chunk_content: &[u8], address: &[u8]) -> bool {
    if chunk_content.len() < 97 + SPAN_SIZE {
        return false;
    }
    let soc_address = &chunk_content[..32];
    let soc_signature = &chunk_content[32..97];
    let Some(wrapped_address) = content_address_array(&chunk_content[97..]) else {
        return false;
    };
    let mut sign_input = [0_u8; 64];
    sign_input[..32].copy_from_slice(soc_address);
    sign_input[32..].copy_from_slice(&wrapped_address);
    let to_sign = keccak256(sign_input);
    let Some(parity) = normalize_v(soc_signature[64] as u64) else {
        return false;
    };
    let sig = Signature::from_bytes_and_parity(&soc_signature[0..64], parity);
    let Ok(owner) = sig.recover_address_from_msg(to_sign) else {
        return false;
    };
    let mut address_input = [0_u8; 52];
    address_input[..32].copy_from_slice(soc_address);
    address_input[32..].copy_from_slice(owner.as_slice());
    address == keccak256(address_input).as_slice()
}

pub fn get_feed_address(owner: &str, topic: &str, index: u64) -> Vec<u8> {
    let Ok(owner_bytes) = hex::decode(strip_hex_prefix(owner)) else {
        return vec![];
    };
    let Ok(owner_bytes): Result<[u8; 20], _> = owner_bytes.try_into() else {
        return vec![];
    };
    let Ok(topic_bytes) = hex::decode(strip_hex_prefix(topic)) else {
        return vec![];
    };
    if topic_bytes.is_empty() {
        return vec![];
    }

    crate::feed::sequence_feed_address(&topic_bytes, &owner_bytes, index, |input| {
        keccak256(input).into()
    })
    .to_vec()
}

pub fn encode_resources(data_array: Vec<(Vec<u8>, String, String)>, indx: String) -> Vec<u8> {
    crate::erasure_coding::encode_resource_bundle(data_array, indx).unwrap_or_default()
}

pub(crate) fn normalize_feed_topic(topic: &str) -> String {
    let trimmed = topic.trim();
    let unprefixed = strip_hex_prefix(trimmed);

    match hex::decode(unprefixed) {
        Ok(topic_bytes) if topic_bytes.len() == 32 => hex::encode(topic_bytes),
        _ => hex::encode(keccak256(trimmed)),
    }
}

pub(crate) fn strip_hex_prefix(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

pub(crate) fn upload_result(message: &str, index: &str) -> Vec<u8> {
    encode_resources(
        vec![(
            message.as_bytes().to_vec(),
            "text/plain".to_string(),
            "... result ...".to_string(),
        )],
        index.to_string(),
    )
}

pub fn decode_resources(encoded_data: Vec<u8>) -> (Vec<(Vec<u8>, String, String)>, String) {
    crate::erasure_coding::decode_resource_bundle(&encoded_data).unwrap_or_default()
}

pub async fn read_file(file: web_sys::File) -> Vec<u8> {
    let file_size = file.size();
    let partition_size = 69001216.0_f64;
    if file_size > usize::MAX as f64 {
        return vec![];
    }

    let mut content = Vec::with_capacity(file_size as usize);
    let mut start = 0.0_f64;
    while start < file_size {
        let end = (start + partition_size).min(file_size);
        let Ok(slice) = file.slice_with_f64_and_f64(start, end) else {
            return vec![];
        };
        let Ok(buffer) = wasm_bindgen_futures::JsFuture::from(slice.array_buffer()).await else {
            return vec![];
        };
        let bytes = js_sys::Uint8Array::new(&buffer);
        let offset = content.len();
        content.resize(offset + bytes.length() as usize, 0);
        bytes.copy_to(&mut content[offset..]);
        start = end;
    }
    content
}

pub const EMPTY_CHEQUEBOOK_ADDRESS: [u8; 20] = [0; 20];

pub fn generate_sign_data(
    underlay: &[u8],
    overlay: &[u8],
    network_id: u64,
    nonce: &[u8],
    timestamp: i64,
    chequebook_address: &[u8],
) -> Vec<u8> {
    let cheque_len = if chequebook_address.is_empty() {
        EMPTY_CHEQUEBOOK_ADDRESS.len()
    } else {
        chequebook_address.len()
    };
    let mut out = Vec::with_capacity(
        b"bee-handshake-".len() + underlay.len() + overlay.len() + 8 + nonce.len() + 8 + cheque_len,
    );
    out.extend_from_slice(b"bee-handshake-");
    out.extend_from_slice(underlay);
    out.extend_from_slice(overlay);
    out.extend_from_slice(&network_id.to_be_bytes());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&(timestamp as u64).to_be_bytes());
    if chequebook_address.is_empty() {
        out.extend_from_slice(&EMPTY_CHEQUEBOOK_ADDRESS);
    } else {
        out.extend_from_slice(chequebook_address);
    }
    out
}

fn recover_address(signature: &[u8], message: &[u8]) -> Option<alloy_primitives::Address> {
    if signature.len() != 65 {
        return None;
    }

    let parity = normalize_v(signature[64] as u64)?;

    let sig = Signature::from_bytes_and_parity(&signature[0..64], parity);
    sig.recover_address_from_msg(message).ok()
}

pub fn parse_address(
    underlay: &[u8],
    overlay: &[u8],
    signature: &[u8],
    nonce: &[u8],
    timestamp: i64,
    network_id: u64,
    chequebook_address: &[u8],
) -> web3::types::Address {
    let sign_data = generate_sign_data(
        underlay,
        overlay,
        network_id,
        nonce,
        timestamp,
        chequebook_address,
    );
    recover_address(signature, &sign_data)
        .map(|address| web3::types::Address::from_slice(address.as_slice()))
        .unwrap_or_default()
}
