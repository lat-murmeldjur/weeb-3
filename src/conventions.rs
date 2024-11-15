#![allow(warnings)]
#![cfg(target_arch = "wasm32")]

use std::io;

use zerocopy::IntoByteSlice;

use alloy::primitives::{keccak256, FixedBytes};

use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};

use wasm_bindgen::prelude::*;
use web_sys::{
    console, Document, EventListener, HtmlButtonElement, HtmlElement, HtmlParagraphElement,
};

pub const MAX_PO: u8 = 31;
pub const SPAN_SIZE: usize = 8;

// pub fn a() {}

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
        b = one.len();
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
    let (mut something, mut something2) = chunk_content.split_at(SPAN_SIZE);

    let usomething: u64 = u64::from_le_bytes(something.try_into().unwrap());

    web_sys::console::log_1(&JsValue::from(format!(
        "Chunk content span type check {:#?}!",
        usomething,
    )));

    web_sys::console::log_1(&JsValue::from(format!(
        "Chunk content hash type check {:#?}!",
        address,
    )));

    let contenthash = hasher_0(&something2.to_vec());

    web_sys::console::log_1(&JsValue::from(format!(
        "Chunk content hash type 1 {:#?}!",
        contenthash,
    )));

    let chunk_address = keccak256([something, &contenthash].concat()).to_vec();
    web_sys::console::log_1(&JsValue::from(format!(
        "Chunk content hash type 2 {:#?}!",
        chunk_address,
    )));

    if *chunk_address == **address {
        web_sys::console::log_1(&JsValue::from(format!(
            "Chunk content address correct {:?}!",
            chunk_address,
        )));

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

    for i in 0..padding {
        content.push(zerobyte)
    }

    web_sys::console::log_1(&JsValue::from(format!(
        "Hasher length type {:#?}!",
        content.len(),
    )));

    return hasher_1(&content, content.len());
}

pub fn hasher_1(content_in: &Vec<u8>, length: usize) -> Vec<u8> {
    let mut lengthof = length;
    let mut coefficient = 1;
    let mut content_holder = content_in.clone();
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

    if content_holder_2.len() > 1 {
        web_sys::console::log_1(&JsValue::from(format!(
            "Chunk content level type panic {:#?} {:#?} !",
            content_holder_2.len(),
            coefficient
        )));
    }

    return content_holder_2[0].clone();
}

pub fn valid_soc(chunk_content: &Vec<u8>, address: &Vec<u8>) -> bool {
    //
    return false;
    //
}

// 3ab408eea4f095bde55c1caeeac8e7fcff49477660f0a28f652f0a6d9c60d05f
