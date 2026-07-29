#![cfg(target_arch = "wasm32")]

use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId, swarm::ConnectionId};

use alloy::primitives::keccak256;
use alloy::primitives::{Signature, normalize_v};

pub const MAX_PO: u8 = 31;
pub const SPAN_SIZE: usize = 8;

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
    pub payment_threshold: u64,
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

//  pub struct Body {
//      body: HtmlElement,
//      document: Document,
//  }
//
//  impl Body {
//      pub fn from_current_window() -> Result<Self, JsError> {
//          let document = web_sys::window()
//              .ok_or(js_error("no global `window` exists"))?
//              .document()
//              .ok_or(js_error("should have a document on window"))?;
//          let body = document
//              .body()
//              .ok_or(js_error("document should have a body"))?;
//
//          Ok(Self { body, document })
//      }
//
//      pub fn append_p(&self, msg: &str) -> Result<(), JsError> {
//          let val = self
//              .document
//              .create_element("p")
//              .map_err(|_| js_error("failed to create <p>"))?;
//          val.set_text_content(Some(msg));
//          self.body
//              .append_child(&val)
//              .map_err(|_| js_error("failed to append <p>"))?;
//
//          Ok(())
//      }
//  }
//
//  fn js_error(msg: &str) -> JsError {
//      io::Error::new(io::ErrorKind::Other, msg).into()
//  }

pub fn get_proximity(one: &[u8], other: &[u8]) -> u8 {
    let mut b: usize = (MAX_PO / 4 + 1).into();

    if b > one.len() {
        b = one.len();
    }

    if b > other.len() {
        b = other.len();
    }

    if b == 0 {
        return 0;
    }

    let m: usize = 8;
    for i in 0..b {
        let oxo = one[i] ^ other[i];

        for j in 0..m {
            if (oxo >> (7 - j)) & 0x01 != 0 {
                return (i * 8 + j).try_into().unwrap();
            }
        }
    }
    return MAX_PO;
}

const CHUNK_SIZE: usize = 4096;
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
    /// BMT padding is deterministic. Cache its root at every tree level so a
    /// short CAC pays only for the path containing actual content.
    static ZERO_BMT_NODES: [BmtHash; BMT_LEVEL_COUNT] = zero_bmt_nodes();
}

/// Calculate the 4 KiB Swarm BMT root without materialising the padded chunk
/// or allocating every hash-tree node separately.
fn bmt_root(content: &[u8]) -> Option<BmtHash> {
    if content.len() > CHUNK_SIZE {
        return None;
    }

    // Trailing zero bytes are indistinguishable from BMT padding. Ignoring them
    // lets short and zero-heavy chunks reuse whole known-zero subtrees.
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
        if let Some(zero_nodes) = zero_nodes {
            // Only an odd occupied frontier reads one absent right sibling;
            // the rest of the zero tail never participates in a hash.
            if occupied_leaves % 2 != 0 {
                nodes[occupied_leaves] = zero_nodes[0];
            }
        }

        let mut width = BMT_LEAF_COUNT;
        let mut occupied = occupied_leaves;
        let mut level = 0usize;
        while width > 1 {
            let next_width = width / 2;
            let next_occupied = occupied.div_ceil(2);

            // Writing parents over the beginning of the same array is safe:
            // parent i consumes children 2i and 2i+1, which no later parent can
            // overwrite before they are read.
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

pub fn valid_soc(chunk_content: &Vec<u8>, address: &Vec<u8>) -> bool {
    //

    if chunk_content.len() < 97 + SPAN_SIZE {
        return false;
    }
    let soc_address = chunk_content[0..32].to_vec();
    let soc_signature = chunk_content[32..97].to_vec();

    let wrapped_address = content_address(&chunk_content[97..]);

    let to_sign = keccak256([soc_address.clone(), wrapped_address].concat()).to_vec();
    let parity: bool = match normalize_v(soc_signature[64] as u64) {
        Some(par) => par,
        _ => {
            return false;
        }
    };
    let sig = Signature::from_bytes_and_parity(&soc_signature[0..64], parity);

    let owner = match sig.recover_address_from_msg(to_sign) {
        Ok(ow) => ow,
        _ => {
            return false;
        }
    };

    let address_constructed = keccak256([soc_address, owner.as_slice().to_vec()].concat()).to_vec();

    if *address == address_constructed {
        return true;
    };

    return false;
    //
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
    crate::upload_conventions::encode_resource_bundle(data_array, indx).unwrap_or_default()
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
    crate::upload_conventions::decode_resource_bundle(&encoded_data).unwrap_or_default()
}

pub async fn read_file(file: web_sys::File) -> Vec<Vec<u8>> {
    let fils = file.size();
    let partition_size = 69001216.0_f64;

    if fils <= partition_size {
        let content_buf = match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
            Ok(buf) => buf,
            Err(_) => return vec![],
        };

        let content_u8a = js_sys::Uint8Array::new(&content_buf);

        return vec![content_u8a.to_vec()];
    } else {
        let mut content: Vec<Vec<u8>> = vec![];

        let mut start = 0.0_f64;
        let mut end = partition_size;
        let mut going = true;

        while going {
            let content_slice = file.slice_with_f64_and_f64(start, end);

            let content_buf = match content_slice {
                Ok(b) => match wasm_bindgen_futures::JsFuture::from(b.array_buffer()).await {
                    Ok(buf) => buf,
                    Err(_) => return vec![],
                },
                Err(_) => return vec![],
            };

            let content_u8a = js_sys::Uint8Array::new(&content_buf);

            let slice = content_u8a.to_vec();
            content.push(slice);

            start = end;
            end += partition_size;

            if end >= fils {
                end = fils;
            }

            if start >= fils {
                going = false;
            }
        }

        return content;
    }
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
    let mut out = b"bee-handshake-".to_vec();
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

fn recover_address(signature: &[u8], message: &[u8]) -> Vec<u8> {
    if signature.len() != 65 {
        return Vec::new();
    }

    let parity = match normalize_v(signature[64] as u64) {
        Some(p) => p,
        None => return Vec::new(),
    };

    let sig = Signature::from_bytes_and_parity(&signature[0..64], parity);

    let address = match sig.recover_address_from_msg(message) {
        Ok(addr) => addr,
        Err(_) => return Vec::new(),
    };

    address.as_slice().to_vec()
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
    let recovered = recover_address(signature, &sign_data);

    if recovered.len() != 20 {
        return web3::types::Address::zero();
    }

    web3::types::Address::from_slice(&recovered)
}

#[cfg(test)]
mod tests {
    use super::{
        BmtHash, CHUNK_SIZE, SECTION2_SIZE, SPAN_SIZE, bmt_root, content_address, valid_cac,
    };
    use alloy::primitives::keccak256;

    fn reference_tree_root(content: &[u8]) -> BmtHash {
        assert!(content.len() >= SECTION2_SIZE);
        assert!(content.len().is_power_of_two());

        let mut level = content
            .chunks_exact(SECTION2_SIZE)
            .map(|section| BmtHash::from(keccak256(section)))
            .collect::<Vec<_>>();
        while level.len() > 1 {
            level = level
                .chunks_exact(2)
                .map(|children| {
                    let mut input = [0u8; SECTION2_SIZE];
                    input[..32].copy_from_slice(&children[0]);
                    input[32..].copy_from_slice(&children[1]);
                    keccak256(input).into()
                })
                .collect();
        }
        level[0]
    }

    // Deliberately straightforward and allocation-heavy. This mirrors Bee's
    // definition rather than the sparse/in-place production implementation.
    fn reference_bmt_root(content: &[u8]) -> BmtHash {
        assert!(content.len() <= CHUNK_SIZE);
        let mut padded = [0u8; CHUNK_SIZE];
        padded[..content.len()].copy_from_slice(content);
        reference_tree_root(&padded)
    }

    fn reference_content_address(chunk: &[u8]) -> Vec<u8> {
        let (span, content) = chunk.split_at(SPAN_SIZE);
        let root = reference_bmt_root(content);
        let mut input = [0u8; SPAN_SIZE + 32];
        input[..SPAN_SIZE].copy_from_slice(span);
        input[SPAN_SIZE..].copy_from_slice(&root);
        keccak256(input).to_vec()
    }

    fn patterned_payload(size: usize, seed: u8) -> Vec<u8> {
        let mut payload = (0..size)
            .map(|index| {
                (index as u8)
                    .wrapping_mul(73)
                    .wrapping_add(seed)
                    .wrapping_add(1)
            })
            .collect::<Vec<_>>();
        if let Some(last) = payload.last_mut() {
            *last = seed.max(1);
        }
        payload
    }

    fn chunk_with_span(payload: &[u8], span: u64) -> Vec<u8> {
        let mut chunk = Vec::with_capacity(SPAN_SIZE + payload.len());
        chunk.extend_from_slice(&span.to_le_bytes());
        chunk.extend_from_slice(payload);
        chunk
    }

    #[test]
    fn empty_payload_is_a_valid_cac() {
        let chunk = vec![0; SPAN_SIZE];
        let address = content_address(&chunk);

        assert_eq!(address.len(), 32);
        assert!(valid_cac(&chunk, &address));
    }

    #[test]
    fn bee_cac_golden_vectors_are_unchanged() {
        for (payload, expected) in [
            (
                b"foo".as_slice(),
                "2387e8e7d8a48c2a9339c97c1dc3461a9a7aa07e994c5cb8b38fd7c1b3e6ea48",
            ),
            (
                b"greaterthanspan".as_slice(),
                "27913f1bdb6e8e52cbd5a5fd4ab577c857287edf6969b41efe926b51de0f4f23",
            ),
        ] {
            let chunk = chunk_with_span(payload, payload.len() as u64);
            assert_eq!(hex::encode(content_address(&chunk)), expected);
        }
    }

    #[test]
    fn optimized_bmt_matches_independent_reference_at_every_boundary() {
        let sizes = [
            0, 1, 7, 8, 31, 32, 33, 63, 64, 65, 95, 127, 128, 129, 255, 256, 257, 511, 512, 513,
            1023, 1024, 1025, 2047, 2048, 2049, 4095, 4096,
        ];

        for (case, size) in sizes.into_iter().enumerate() {
            let payload = patterned_payload(size, case as u8 + 1);
            let span = (size as u64)
                .wrapping_mul(0x0102_0304_0506_0708)
                .wrapping_add(case as u64);
            let chunk = chunk_with_span(&payload, span);
            let expected_root = reference_bmt_root(&payload);
            let expected_address = reference_content_address(&chunk);

            assert_eq!(bmt_root(&payload), Some(expected_root), "size {size}");
            assert_eq!(content_address(&chunk), expected_address, "size {size}");
            assert!(valid_cac(&chunk, &expected_address), "size {size}");
        }
    }

    #[test]
    fn optimized_bmt_matches_reference_for_dispersed_lengths_and_zero_tails() {
        let mut state = 0x9e37_79b9u32;
        for case in 0..96u64 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let size = (state as usize) % (CHUNK_SIZE + 1);
            let mut payload = patterned_payload(size, state as u8);
            if !payload.is_empty() {
                let zero_tail = (state as usize >> 16) % (payload.len().min(257) + 1);
                let zero_start = payload.len() - zero_tail;
                payload[zero_start..].fill(0);
            }
            let chunk = chunk_with_span(&payload, case.rotate_left(17) ^ 0xa5a5_5a5a);

            assert_eq!(bmt_root(&payload), Some(reference_bmt_root(&payload)));
            assert_eq!(content_address(&chunk), reference_content_address(&chunk));
        }
    }

    #[test]
    fn invalid_cac_inputs_fail_without_hash_allocations_or_panics() {
        assert!(content_address(&[]).is_empty());
        assert!(content_address(&[0; SPAN_SIZE - 1]).is_empty());
        assert!(content_address(&vec![0; SPAN_SIZE + CHUNK_SIZE + 1]).is_empty());
        assert!(bmt_root(&vec![0; CHUNK_SIZE + 1]).is_none());

        let chunk = chunk_with_span(b"payload", 7);
        let address = content_address(&chunk);
        assert!(!valid_cac(&chunk, &address[..31]));
        assert!(!valid_cac(&chunk, &[0; 32]));

        let mut changed = chunk.clone();
        changed[0] ^= 1;
        assert!(!valid_cac(&changed, &address));
    }
}
