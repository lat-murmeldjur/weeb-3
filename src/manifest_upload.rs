use crate::{
    //
    manifest::Fork,
    //
    JsValue,
};

use serde_json::Value;

// pub struct Fork {
//     //    metadata: Value,
//     pub data: Vec<u8>, // repurposed as address
//     pub mime: String,
//     // pub filename: String,
//     pub path: String,
// }

pub async fn create_manifest(obfuscated: bool, encrypted: bool, forks: Vec<Fork>) -> Vec<u8> {
    let mut manifest_bytes_vec: Vec<u8> = vec![];

    for _ in 0..32 {
        if !obfuscated {
            manifest_bytes_vec.push(0_u8);
        } else {
            manifest_bytes_vec.push(rand::random::<u8>());
        }
    }

    manifest_bytes_vec.append(
        &mut hex::decode("5768b3b6a7db56d21d1abff40d41cebfc83448fed8d7e9b06ec0d3b073f28f").unwrap(),
    );

    if encrypted {
        manifest_bytes_vec.push(64_u8)
    } else {
        manifest_bytes_vec.push(32_u8)
    }
    for _ in 0..32 {
        manifest_bytes_vec.push(0_u8);
    }

    manifest_bytes_vec
}
