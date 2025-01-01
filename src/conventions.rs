#![cfg(target_arch = "wasm32")]

use std::collections::HashMap;
use std::io;

use alloy::primitives::keccak256;

use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};

use wasm_bindgen::prelude::*;
use web_sys::{Document, HtmlElement};

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Entry {
    pub reference: String,
    pub meta: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub entries: HashMap<String, Entry>,
}

pub const MAX_PO: u8 = 31;
pub const SPAN_SIZE: usize = 8;

#[derive(Debug, Clone)]
pub struct PeerFile {
    pub peer_id: PeerId,
    pub overlay: Vec<u8>,
}

#[derive(Debug)]
pub struct PeerAccounting {
    pub balance: u64,
    pub threshold: u64,
    pub reserve: u64,
    pub refreshment: f64,
    pub id: PeerId,
}

pub fn try_from_multiaddr(address: &Multiaddr) -> Option<PeerId> {
    address.iter().last().and_then(|p| match p {
        Protocol::P2p(hash) => PeerId::from_multihash(hash.into()).ok(),
        _ => None,
    })
}

pub struct Body {
    body: HtmlElement,
    document: Document,
}

impl Body {
    pub fn from_current_window() -> Result<Self, JsError> {
        let document = web_sys::window()
            .ok_or(js_error("no global `window` exists"))?
            .document()
            .ok_or(js_error("should have a document on window"))?;
        let body = document
            .body()
            .ok_or(js_error("document should have a body"))?;

        Ok(Self { body, document })
    }

    pub fn append_p(&self, msg: &str) -> Result<(), JsError> {
        let val = self
            .document
            .create_element("p")
            .map_err(|_| js_error("failed to create <p>"))?;
        val.set_text_content(Some(msg));
        self.body
            .append_child(&val)
            .map_err(|_| js_error("failed to append <p>"))?;

        Ok(())
    }
}

fn js_error(msg: &str) -> JsError {
    io::Error::new(io::ErrorKind::Other, msg).into()
}

pub fn get_proximity(one: &Vec<u8>, other: &Vec<u8>) -> u8 {
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

pub fn valid_cac(chunk_content: &Vec<u8>, address: &Vec<u8>) -> bool {
    //
    if chunk_content.len() < SPAN_SIZE {
        return false;
    }

    let (something, something2) = chunk_content.split_at(SPAN_SIZE);

    let contenthash = hasher_0(&something2.to_vec());

    let chunk_address = keccak256([something, &contenthash].concat()).to_vec();

    if *chunk_address == **address {
        // web_sys::console::log_1(&JsValue::from(format!(
        //     "Chunk content address correct {:?}!",
        //     chunk_address,
        // )));

        return true;
    }

    web_sys::console::log_1(&JsValue::from(format!(
        "Chunk non content addressed {:?}!",
        chunk_address,
    )));

    return false;
    //
}

const SECTION_SIZE: usize = 32;
const SECTION2_SIZE: usize = 2 * SECTION_SIZE;
const DIFF: usize = 0;

pub fn hasher_0(content_in: &Vec<u8>) -> Vec<u8> {
    let mut content = content_in.clone();

    let padding = 4096 - (content.len() - DIFF);
    let zerobyte: u8 = 0;

    for _ in 0..padding {
        content.push(zerobyte)
    }

    return hasher_1(&content, content.len());
}

pub fn hasher_1(content_in: &Vec<u8>, length: usize) -> Vec<u8> {
    let mut lengthof = length;
    let mut coefficient = 1;
    let content_holder = content_in.clone();
    let mut content_holder_2 = vec![];
    let mut content_holder_3 = vec![];

    let input_sections = content_in.len() / (coefficient * SECTION2_SIZE);
    for i in 0..input_sections {
        //
        content_holder_2.push(
            keccak256(content_holder[i * SECTION2_SIZE..(i + 1) * SECTION2_SIZE].to_vec()).to_vec(),
        );
        //
    }

    while lengthof > SECTION2_SIZE {
        coefficient *= 2;
        lengthof /= 2;

        let input_sections = content_in.len() / (coefficient * SECTION2_SIZE);
        for i in 0..input_sections {
            //
            content_holder_3.push(keccak256(content_holder_2[i * 2..i * 2 + 2].concat()).to_vec());
            //
        }

        content_holder_2 = content_holder_3;
        content_holder_3 = vec![];
    }

    return content_holder_2[0].clone();
}

pub fn valid_soc(_chunk_content: &Vec<u8>, _address: &Vec<u8>) -> bool {
    //
    return false;
    //
}

pub fn encode_resource(data: Vec<u8>, str0: String) -> Vec<u8> {
    let str_b = str0.as_bytes();
    let len_b: u64 = str_b.len() as u64;
    let a = len_b.to_le_bytes();
    [a.as_slice(), str_b, &data].concat()
}

pub fn decode_resource(encoded_data: Vec<u8>) -> (Vec<u8>, String) {
    if encoded_data.len() < 8 {
        return (vec![], "undefined".to_string());
    };

    let string_length: usize =
        u64::from_le_bytes(encoded_data[0..8].try_into().unwrap_or([0; 8])) as usize;

    if encoded_data.len() < 8 + string_length {
        return (vec![], "undefined".to_string());
    };

    let string =
        String::from_utf8(encoded_data[8..8 + string_length].to_vec()).unwrap_or("".to_string());

    let mut data = Vec::new();
    if encoded_data.len() > 8 + string_length {
        data = encoded_data[8 + string_length..].to_vec();
    };
    (data, string)
}
