#![cfg(target_arch = "wasm32")]

use crate::upload::{Resource, ResourceData};
use std::io::Read;

use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId, swarm::ConnectionId};

pub use crate::erasure_coding::SPAN_SIZE;
use crate::erasure_coding::{CHUNK_SIZE, HASH_SIZE};
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use web3::types::Address;

#[inline]
pub(crate) fn keccak256(input: impl AsRef<[u8]>) -> [u8; 32] {
    web3::signing::keccak256(input.as_ref())
}

pub(crate) fn eip191_hash_message(message: &[u8]) -> [u8; 32] {
    web3::signing::hash_message(message).0
}

pub(crate) fn public_key_address(key: &k256::ecdsa::VerifyingKey) -> Address {
    let public_key = key.to_encoded_point(false);
    let hash = keccak256(&public_key.as_bytes()[1..]);
    Address::from_slice(&hash[12..])
}

pub const MAX_PO: u8 = 31;
const BEE_REPLICA_OWNER: [u8; 20] = [
    0xdc, 0x5b, 0x20, 0x84, 0x7f, 0x43, 0xd6, 0x79, 0x28, 0xf4, 0x9c, 0xd4, 0xf8, 0x5d, 0x69, 0x6b,
    0x5a, 0x76, 0x17, 0xb5,
];

#[inline]
pub(crate) fn encryption_segment_key(key: &[u8], counter: u32) -> [u8; HASH_SIZE] {
    let mut seed = [0u8; HASH_SIZE + 4];
    seed[..HASH_SIZE].copy_from_slice(key);
    seed[HASH_SIZE..].copy_from_slice(&counter.to_le_bytes());
    keccak256(keccak256(seed))
}

pub(crate) fn bee_replica_address(id: &[u8; HASH_SIZE]) -> [u8; HASH_SIZE] {
    let mut input = [0u8; HASH_SIZE + BEE_REPLICA_OWNER.len()];
    input[..HASH_SIZE].copy_from_slice(id);
    input[HASH_SIZE..].copy_from_slice(&BEE_REPLICA_OWNER);
    keccak256(input)
}

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

fn zero_bmt_nodes() -> [BmtHash; BMT_LEVEL_COUNT] {
    let mut nodes = [[0u8; SECTION_SIZE]; BMT_LEVEL_COUNT];
    let mut pair = [0u8; SECTION2_SIZE];
    nodes[0] = keccak256(pair);
    for level in 1..BMT_LEVEL_COUNT {
        pair[..SECTION_SIZE].copy_from_slice(&nodes[level - 1]);
        pair[SECTION_SIZE..].copy_from_slice(&nodes[level - 1]);
        nodes[level] = keccak256(pair);
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
    let mut nodes = [[0u8; SECTION_SIZE]; BMT_LEAF_COUNT];
    let mut block = [0u8; SECTION2_SIZE];

    let full_blocks = effective_len / SECTION2_SIZE;
    for (index, section) in content[..full_blocks * SECTION2_SIZE]
        .chunks_exact(SECTION2_SIZE)
        .enumerate()
    {
        nodes[index] = keccak256(section);
    }
    if effective_len % SECTION2_SIZE != 0 {
        let start = full_blocks * SECTION2_SIZE;
        block[..effective_len - start].copy_from_slice(&content[start..effective_len]);
        nodes[full_blocks] = keccak256(block);
    }

    Some(ZERO_BMT_NODES.with(|zero_nodes| {
        let mut occupied = effective_len.div_ceil(SECTION2_SIZE);
        for zero in &zero_nodes[..BMT_LEVEL_COUNT - 1] {
            if occupied % 2 != 0 {
                nodes[occupied] = *zero;
            }
            occupied = occupied.div_ceil(2);
            for index in 0..occupied {
                let start = index * SECTION2_SIZE;
                nodes[index] =
                    keccak256(&nodes.as_flattened()[start..start + SECTION2_SIZE]);
            }
        }
        nodes[0]
    }))
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
    Some(keccak256(hash_input))
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
    let Some(owner) = recover_address(soc_signature, to_sign.as_slice()) else {
        return false;
    };
    let mut address_input = [0_u8; 52];
    address_input[..32].copy_from_slice(soc_address);
    address_input[32..].copy_from_slice(owner.as_bytes());
    address == keccak256(address_input).as_slice()
}

pub fn get_feed_address(owner: &str, topic: &str, index: u64) -> Vec<u8> {
    let mut owner_bytes = [0_u8; 20];
    if hex::decode_to_slice(strip_hex_prefix(owner), &mut owner_bytes).is_err() {
        return vec![];
    }
    let Ok(topic_bytes) = hex::decode(strip_hex_prefix(topic)) else {
        return vec![];
    };
    if topic_bytes.is_empty() {
        return vec![];
    }

    crate::feed::sequence_feed_address(&topic_bytes, &owner_bytes, index, |input| {
        keccak256(input)
    })
    .to_vec()
}

pub fn encode_resources(data_array: Vec<(Vec<u8>, String, String)>, indx: String) -> Vec<u8> {
    crate::erasure_coding::encode_resource_bundle(data_array, indx).unwrap_or_default()
}

pub(crate) fn normalize_feed_topic(topic: &str) -> String {
    let trimmed = topic.trim();
    let unprefixed = strip_hex_prefix(trimmed);
    let mut bytes = [0_u8; 32];

    if hex::decode_to_slice(unprefixed, &mut bytes).is_ok() {
        hex::encode(bytes)
    } else {
        hex::encode(keccak256(trimmed))
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

pub(crate) fn tar_resources(content: &[u8]) -> std::io::Result<Vec<Resource>> {
    let mut archive = tar::Archive::new(content);
    Ok(archive
        .entries()?
        .filter_map(|entry| {
            let mut entry = entry.ok()?;
            if !entry.header().entry_type().is_file() {
                return None;
            }
            let path = entry.path().ok()?.into_owned();
            let filename = path.file_name()?.to_str()?.to_string();
            let path = path.into_os_string().into_string().ok()?;
            let path = path.strip_prefix("./").unwrap_or(&path).to_string();
            let mime = mime_guess::from_path(&path).first_raw()?;
            let mime = if mime.starts_with("text/") {
                format!("{mime}; charset=utf-8")
            } else {
                mime.to_string()
            };
            let mut data = Vec::new();
            entry.read_to_end(&mut data).ok()?;
            Some(Resource {
                path,
                filename,
                mime,
                data: ResourceData::Parts(vec![data]),
            })
        })
        .collect())
}

pub async fn read_file(file: web_sys::File) -> Vec<u8> {
    let file_size = file.size();
    let partition_size = crate::erasure_coding::FILE_UPLOAD_READ_WINDOW_BYTES as f64;
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

fn recover_address(signature: &[u8], message: &[u8]) -> Option<Address> {
    let signature: &[u8; 65] = signature.try_into().ok()?;
    let mut parity = match signature[64] {
        0 | 1 => signature[64] == 1,
        27 | 28 | 35.. => signature[64] % 2 == 0,
        _ => return None,
    };
    let mut sig = Signature::from_slice(&signature[..64]).ok()?;
    if let Some(normalized) = sig.normalize_s() {
        sig = normalized;
        parity = !parity;
    }
    VerifyingKey::recover_from_prehash(
        &eip191_hash_message(message),
        &sig,
        RecoveryId::new(parity, false),
    )
    .ok()
    .map(|key| public_key_address(&key))
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
    recover_address(signature, &sign_data).unwrap_or_default()
}

#[cfg(test)]
mod hash_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn sparse_bmt_matches_the_full_tree_at_every_section_boundary() {
        let data: Vec<u8> = (0..CHUNK_SIZE).map(|index| (index % 251) as u8).collect();
        for boundary in (0..=CHUNK_SIZE).step_by(SECTION2_SIZE) {
            for length in [
                boundary.saturating_sub(1),
                boundary,
                (boundary + 1).min(CHUNK_SIZE),
            ] {
                let mut padded = vec![0; CHUNK_SIZE];
                padded[..length].copy_from_slice(&data[..length]);
                let mut level = padded;
                while level.len() > SECTION_SIZE {
                    level = level
                        .chunks_exact(SECTION2_SIZE)
                        .flat_map(web3::signing::keccak256)
                        .collect();
                }
                assert_eq!(bmt_root(&data[..length]).unwrap().as_slice(), level);
                let mut zero_tail = data[..length].to_vec();
                zero_tail.resize(CHUNK_SIZE, 0);
                assert_eq!(bmt_root(&zero_tail).unwrap().as_slice(), level);
            }
        }
        assert_eq!(bmt_root(&[]), bmt_root(&[0; CHUNK_SIZE]));
        assert!(bmt_root(&[0; CHUNK_SIZE + 1]).is_none());
        assert!(!valid_cac(&[0; SPAN_SIZE - 1], &[0; HASH_SIZE]));
    }

    #[wasm_bindgen_test]
    fn existing_keccak_backends_agree_at_rate_boundaries() {
        for length in [0, 32, 40, 64, 135, 136, 137, CHUNK_SIZE] {
            let input: Vec<u8> = (0..length).map(|index| index as u8).collect();
            assert_eq!(alloy_primitives::keccak256(&input).0, keccak256(&input));
        }
    }
}
