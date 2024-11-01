#![allow(warnings)]
#![cfg(target_arch = "wasm32")]

use std::io;

use libp2p::{Multiaddr, PeerId};

use libp2p::multiaddr::Protocol;
use wasm_bindgen::prelude::*;
use web_sys::{Document, HtmlElement};

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
    fn from_current_window() -> Result<Self, JsError> {
        let document = web_sys::window()
            .ok_or(js_error("no global `window` exists"))?
            .document()
            .ok_or(js_error("should have a document on window"))?;
        let body = document
            .body()
            .ok_or(js_error("document should have a body"))?;

        Ok(Self { body, document })
    }

    fn append_p(&self, msg: &str) -> Result<(), JsError> {
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
