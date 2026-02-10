#![cfg(target_arch = "wasm32")]

use crate::{Sekirei, decode_resources};
use async_std::sync::Arc;
use js_sys::Object;
use js_sys::Reflect;
use js_sys::{Array, Uint8Array};
use libp2p::futures::future::join_all;
use std::time::Duration;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{File, FilePropertyBag};

#[wasm_bindgen]
pub struct BootstrapNode {
    multiaddr: String,
    usable: bool,
}

#[wasm_bindgen]
impl BootstrapNode {
    #[wasm_bindgen(constructor)]
    pub fn new(multiaddr: String, usable: bool) -> BootstrapNode {
        BootstrapNode { multiaddr, usable }
    }

    #[wasm_bindgen(getter)]
    pub fn multiaddr(&self) -> String {
        self.multiaddr.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn usable(&self) -> bool {
        self.usable
    }
}

fn resource_to_js(bytes: Vec<u8>, mime: String, path: String) -> Object {
    let obj = Object::new();
    let u8 = Uint8Array::new_with_length(bytes.len() as u32);
    u8.copy_from(&bytes);
    let _ = Reflect::set(&obj, &"body".into(), &u8);
    let _ = Reflect::set(&obj, &"mime".into(), &JsValue::from_str(&mime));
    let _ = Reflect::set(&obj, &"path".into(), &JsValue::from_str(&path));
    obj
}

fn make_js_file(bytes: Vec<u8>, mime: &str, name: &str) -> File {
    let parts = Array::new();
    let u8 = Uint8Array::new_with_length(bytes.len() as u32);
    u8.copy_from(&bytes);
    parts.push(&u8);

    let bag = FilePropertyBag::new();
    bag.set_type(mime);
    bag.set_last_modified(js_sys::Date::now());

    File::new_with_u8_array_sequence_and_options(&parts, name, &bag).unwrap()
}

#[wasm_bindgen]
pub struct SekireiNo103 {
    inner: Arc<Sekirei>,
}

#[wasm_bindgen]
impl SekireiNo103 {
    #[wasm_bindgen(constructor)]
    pub fn new() -> SekireiNo103 {
        SekireiNo103 {
            inner: Arc::new(Sekirei::new("".to_string())),
        }
    }

    #[wasm_bindgen(js_name = start)]
    pub fn start(&self, bootstrap_nodes: Vec<BootstrapNode>, network_id: String) {
        let s = self.inner.clone();

        spawn_local(async move {
            let s0 = s.clone();
            spawn_local(async move {
                s0.run(String::new()).await;
            });

            async_std::task::sleep(Duration::from_millis(600)).await;

            let futures = bootstrap_nodes.into_iter().map(|node| {
                let s_clone = s.clone();
                let nid = network_id.clone();

                async move {
                    if !node.multiaddr.is_empty() {
                        let _ = s_clone
                            .change_bootnode_address(node.multiaddr, nid, node.usable)
                            .await;
                    }
                }
            });

            join_all(futures).await;
        });
    }

    #[wasm_bindgen(js_name = retrieve)]
    pub async fn retrieve(&self, address: String) -> Array {
        let raw = self.inner.acquire(address).await;
        let (mut data, indx) = decode_resources(raw);

        let out = Array::new();

        fn make_entry(path: &str, file: &JsValue) -> JsValue {
            let obj = Object::new();
            Reflect::set(&obj, &JsValue::from("path"), &JsValue::from(path)).expect("set path");
            Reflect::set(&obj, &JsValue::from("file"), file).expect("set file");
            obj.into()
        }

        if let Some(pos) = data.iter().position(|(_, _, p)| *p == indx) {
            let (bytes, mime, path) = data.remove(pos);
            let file = make_js_file(bytes, &mime, &path); // JsValue or File→JsValue
            let entry = make_entry(&path, &file);
            out.push(&entry);
        }

        for (bytes, mime, path) in data {
            let file = make_js_file(bytes, &mime, &path);
            let entry = make_entry(&path, &file);
            out.push(&entry);
        }

        out
    }

    #[wasm_bindgen(js_name = postPushChunk)]
    pub async fn post_push_chunk_js(
        &self,
        data: Vec<u8>,          // including span, soc parts, etc, everything
        soc: bool,              // whether this is an soc
        chunk_address: Vec<u8>, // soc address if soc, cac address if cac
        stamp: Vec<u8>,         // stamp bytes
    ) -> String {
        let raw = self
            .inner
            .post_push_chunk(data, soc, chunk_address, stamp)
            .await;

        let (resources, indx) = decode_resources(raw);

        if let Some((bytes, _mime, _path)) = resources.into_iter().find(|(_, _, p)| *p == indx) {
            String::from_utf8(bytes).unwrap_or_else(|_| "Invalid UTF-8 result".to_string())
        } else {
            "No upload result returned".to_string()
        }
    }

    pub async fn upload(
        &self,
        file: File,
        encryption: bool,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        let raw = self
            .inner
            .post_upload(file, encryption, index_string, add_to_feed, feed_topic)
            .await;

        let (data, indx) = decode_resources(raw);
        let obj = Object::new();

        let _ = Reflect::set(&obj, &"reference".into(), &JsValue::from_str(&indx));

        let resources = Array::new();
        for (bytes, mime, path) in data {
            resources.push(&resource_to_js(bytes, mime, path));
        }
        let _ = Reflect::set(&obj, &"resources".into(), &resources);
        obj
    }

    #[wasm_bindgen(js_name = resetStamp)]
    pub async fn reset_stamp(&self) -> Object {
        let raw = self.inner.reset_stamp().await;
        let (data, _indx) = decode_resources(raw);
        let obj = Object::new();
        if let Some((bytes, mime, path)) = data.get(0) {
            let _ = Reflect::set(
                &obj,
                &"message".into(),
                &JsValue::from_str(&String::from_utf8_lossy(bytes)),
            );
            let _ = Reflect::set(&obj, &"mime".into(), &JsValue::from_str(mime));
            let _ = Reflect::set(&obj, &"path".into(), &JsValue::from_str(path));
        }
        obj
    }
}

/* example */

/*

import init, { SekireiNo103, BootstrapNode } from "./pkg/weeb_3.js";
await init();

const node = new SekireiNo103();

const BOOTSTRAP_NODES = [
  new BootstrapNode(
    "/ip4/167.235.96.31/tcp/32535/tls/sni/167-235-96-31.k2k4r8n9x80nshvozftjmg4klymgjtdflwxiovfx63yc6917dlrteva4.libp2p.direct/ws/p2p/QmYkyg5ZU3DzxhqfGyLYLVbk9DMdBagxe9q1AmHKNgt8ps",
    true
  ),
  // … additional bootstrap nodes …
];

node.start(BOOTSTRAP_NODES, "10");

const entries = await node.retrieve(
  "695fceb3a8c212cd123e2e40d86ec08b52fe4fe6ca46687ce9ea69b8f05471f6aa25b5d4d41bf78b1db3479c048fd5fd8137ba844604821b71786196306b68e7"
);

for (const { path, file } of entries) {
  console.log(path, file.name, file.type, file.size);
  const url = URL.createObjectURL(file);
  // use url (preview, download, etc.)
}

// upload chunk with stamp (of course fill out Uint8Arrays with valid data)

const data = new Uint8Array();
const soc = false;
const chunkAddress = new Uint8Array();
const stamp = new Uint8Array();

const result = await node.postPushChunk(data, soc, chunkAddress, stamp);

*/
