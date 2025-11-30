#![cfg(target_arch = "wasm32")]

use crate::{Sekirei, decode_resources};
use async_std::sync::Arc;
use js_sys::Object;
use js_sys::Reflect;
use js_sys::{Array, Uint8Array};
use std::time::Duration;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{File, FilePropertyBag};

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
    pub fn start(&self, bootnode_multiaddr: String, network_id: String) {
        let s = self.inner.clone();
        spawn_local(async move {
            let s0 = s.clone();
            spawn_local(async move {
                s0.run(String::new()).await;
            });

            async_std::task::sleep(Duration::from_millis(600)).await;
            if !bootnode_multiaddr.is_empty() {
                let _ = s
                    .change_bootnode_address(bootnode_multiaddr, network_id)
                    .await;
            }
        });
    }

    #[wasm_bindgen(js_name = retrieve)]
    pub async fn retrieve(&self, address: String) -> Array {
        let raw = self.inner.acquire(address).await;
        let (mut data, indx) = decode_resources(raw);

        let out = Array::new();

        // Helper to build { path, file } objects
        fn make_entry(path: &str, file: &JsValue) -> JsValue {
            let obj = Object::new();
            Reflect::set(&obj, &JsValue::from("path"), &JsValue::from(path)).expect("set path");
            Reflect::set(&obj, &JsValue::from("file"), file).expect("set file");
            obj.into()
        }

        // Keep your "index" file first
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

import init, { Sekirei_No_103 } from "./pkg/weeb_3.js";
await init();

const node = new Sekirei_No_103();
node.start("/ip4/203.0.113.5/udp/8443/webrtc-direct/p2p/12D3K...", "10");

// retrieve now returns: Array<{ path: string, file: File }>
const entries = await node.retrieve("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

for (const { path, file } of entries) {
  console.log(path, file.name, file.type, file.size);

  const url = URL.createObjectURL(file);
  // do something with url...
}

*/
