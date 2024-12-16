use crate::{
    //
    RETRIEVE_ROUND_TIME,
};

use std::time::Duration;

use serde_json::Value;

use js_sys::Date;
use std::sync::mpsc;
use wasm_bindgen::JsValue;

pub async fn interpret_manifest(
    cd: &Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> (Vec<u8>, String) {
    if cd.len() == 0 {
        return (vec![], "undefined".to_string());
    }

    if cd.len() < 72 {
        return (cd[8..].to_vec(), "application/octet-stream".to_string());
    }

    let obfuscation_key = &cd[8..40];
    let enc_obfuscation_key = hex::encode(obfuscation_key);
    web_sys::console::log_1(&JsValue::from(format!(
        "obfuscation_key: {}",
        enc_obfuscation_key
    )));

    let mf_version = &cd[40..71];
    let enc_mf_version = hex::encode(mf_version);

    if enc_mf_version != "5768b3b6a7db56d21d1abff40d41cebfc83448fed8d7e9b06ec0d3b073f28f"
        && enc_mf_version != "025184789d63635766d78c41900196b57d7400875ebe4d9b5d1e76bd9652a9"
    {
        return (cd[8..].to_vec(), "application/octet-stream".to_string());
    }

    web_sys::console::log_1(&JsValue::from(format!("mf_version: {}", enc_mf_version)));
    let ref_size = cd[71];
    let enc_ref_size = hex::encode(&[ref_size]);
    web_sys::console::log_1(&JsValue::from(format!("ref_size: {}", enc_ref_size)));

    let ref_delimiter = (72 + ref_size) as usize;
    let actual_reference = &cd[72..ref_delimiter];
    let enc_actual_reference = hex::encode(actual_reference);
    web_sys::console::log_1(&JsValue::from(format!(
        "actual_reference: {}",
        enc_actual_reference
    )));

    let index_delimiter = (ref_delimiter + 32) as usize;
    let index = &cd[ref_delimiter..index_delimiter];
    let enc_index = hex::encode(index);
    web_sys::console::log_1(&JsValue::from(format!("index: {}", enc_index)));

    // fork parts

    #[allow(unused_assignments)]
    let mut data_address = vec![];

    let mut fork_start_current = index_delimiter;

    {
        let fork_type = cd[fork_start_current];
        let enc_fork_type = hex::encode(&[fork_type]);
        web_sys::console::log_1(&JsValue::from(format!("fork_type: {}", enc_fork_type)));

        let fork_prefix_length = cd[fork_start_current + 1];
        let enc_fork_prefix_length = hex::encode(&[fork_prefix_length]);
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_prefix_length: {}",
            enc_fork_prefix_length
        )));

        let fork_prefix_delimiter = fork_start_current + 2 + 30;
        let fork_prefix = &cd[fork_start_current + 2..fork_prefix_delimiter];
        let enc_fork_prefix = hex::encode(fork_prefix);
        web_sys::console::log_1(&JsValue::from(format!("fork_prefix: {}", enc_fork_prefix)));

        let fork_reference = &cd[fork_prefix_delimiter..fork_prefix_delimiter + 32];
        let enc_fork_reference = hex::encode(fork_reference);
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_reference: {}",
            enc_fork_reference
        )));

        let fork_metadata_bytesize: [u8; 2] = cd
            [fork_prefix_delimiter + 32..fork_prefix_delimiter + 34]
            .try_into()
            .unwrap();

        let calc_metadata_bytesize = u16::from_be_bytes(fork_metadata_bytesize) as usize;
        web_sys::console::log_1(&JsValue::from(format!(
            "calc_metadata_bytesize: {} ",
            calc_metadata_bytesize
        )));

        let fork_metadata_delimiter = fork_prefix_delimiter + 34 + calc_metadata_bytesize;

        let fork_metadata = &cd[fork_prefix_delimiter + 34..fork_metadata_delimiter];
        let enc_fork_metadata = hex::encode(fork_metadata);
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_metadata: {}",
            enc_fork_metadata
        )));

        let v0: Value = serde_json::from_slice(fork_metadata).unwrap_or("nil".into());
        web_sys::console::log_1(&JsValue::from(format!("metadata json: {:#?} ", v0)));

        let str0 = v0.get("website-index-document").unwrap().as_str().unwrap();
        web_sys::console::log_1(&JsValue::from(format!("index document: {:#?} ", str0)));

        data_address = hex::decode(str0).unwrap();
        web_sys::console::log_1(&JsValue::from(format!(
            "data_address: {:#?} ",
            data_address
        )));

        fork_start_current = fork_metadata_delimiter;
    }

    #[allow(unused_assignments)]
    let mut mime = "Undefined".to_string();

    {
        let fork_type = cd[fork_start_current];
        let enc_fork_type = hex::encode(&[fork_type]);
        web_sys::console::log_1(&JsValue::from(format!("fork_type: {}", enc_fork_type)));

        let fork_prefix_length = cd[fork_start_current + 1];
        let enc_fork_prefix_length = hex::encode(&[fork_prefix_length]);
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_prefix_length: {}",
            enc_fork_prefix_length
        )));

        let fork_prefix_delimiter = fork_start_current + 2 + 30;
        let fork_prefix = &cd[fork_start_current + 2..fork_prefix_delimiter];
        let enc_fork_prefix = hex::encode(fork_prefix);
        web_sys::console::log_1(&JsValue::from(format!("fork_prefix: {}", enc_fork_prefix)));

        let fork_reference = &cd[fork_prefix_delimiter..fork_prefix_delimiter + 32];
        let enc_fork_reference = hex::encode(fork_reference);
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_reference: {}",
            enc_fork_reference
        )));

        let mdata = get_data(fork_reference.to_vec(), data_retrieve_chan).await;

        {
            let ref_size = mdata[71];
            let enc_ref_size = hex::encode(&[ref_size]);
            web_sys::console::log_1(&JsValue::from(format!("ref_size: {}", enc_ref_size)));

            let ref_delimiter = (72 + ref_size) as usize;
            let actual_reference = &mdata[72..ref_delimiter];
            let enc_actual_reference = hex::encode(actual_reference);
            web_sys::console::log_1(&JsValue::from(format!(
                "actual_reference: {}",
                enc_actual_reference
            )));

            let index_delimiter = (ref_delimiter + 32) as usize;
            let index = &mdata[ref_delimiter..index_delimiter];
            let enc_index = hex::encode(index);
            web_sys::console::log_1(&JsValue::from(format!("index: {}", enc_index)));

            let mfork_start_current = index_delimiter;

            let fork_type = mdata[mfork_start_current];
            let enc_fork_type = hex::encode(&[fork_type]);
            web_sys::console::log_1(&JsValue::from(format!("fork_type: {}", enc_fork_type)));

            let fork_prefix_length = mdata[mfork_start_current + 1];
            let enc_fork_prefix_length = hex::encode(&[fork_prefix_length]);
            web_sys::console::log_1(&JsValue::from(format!(
                "fork_prefix_length: {}",
                enc_fork_prefix_length
            )));

            let fork_prefix_delimiter = mfork_start_current + 2 + 30;
            let fork_prefix = &mdata[mfork_start_current + 2..fork_prefix_delimiter];
            let enc_fork_prefix = hex::encode(fork_prefix);
            web_sys::console::log_1(&JsValue::from(format!("fork_prefix: {}", enc_fork_prefix)));

            let fork_reference2 = &mdata[fork_prefix_delimiter..fork_prefix_delimiter + 32];
            let enc_fork_reference = hex::encode(fork_reference);
            web_sys::console::log_1(&JsValue::from(format!(
                "fork_reference: {}",
                enc_fork_reference
            )));

            let mdata2 = get_data(fork_reference2.to_vec(), data_retrieve_chan).await;

            web_sys::console::log_1(&JsValue::from(format!("mdata2.len(): {}", mdata2.len())));

            {
                let fork_metadata_bytesize: [u8; 2] = mdata2[200..202].try_into().unwrap();

                let calc_metadata_bytesize = u16::from_be_bytes(fork_metadata_bytesize) as usize;
                web_sys::console::log_1(&JsValue::from(format!(
                    "calc_metadata_bytesize: {} ",
                    calc_metadata_bytesize
                )));

                let fork_metadata_delimiter = 202 + calc_metadata_bytesize;

                let fork_metadata = &mdata2[202..fork_metadata_delimiter];
                let enc_fork_metadata = hex::encode(fork_metadata);
                web_sys::console::log_1(&JsValue::from(format!(
                    "fork_metadata: {}",
                    enc_fork_metadata
                )));

                let v1: Value = serde_json::from_slice(fork_metadata).unwrap_or("nil".into());
                web_sys::console::log_1(&JsValue::from(format!("metadata json: {:#?} ", v1)));

                let str1 = v1.get("Content-Type").unwrap().as_str().unwrap();
                web_sys::console::log_1(&JsValue::from(format!("index document: {:#?} ", str1)));

                mime = str1.to_string();
            }
        }
    }

    let data = get_data(data_address, data_retrieve_chan).await;

    if data.len() < 8 {
        return (vec![], "undefined".to_string());
    }

    return (data, mime);
}

pub async fn get_data(
    data_address: Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let (chan_out, chan_in) = mpsc::channel::<Vec<u8>>();
    data_retrieve_chan.send((data_address, chan_out)).unwrap();

    let k0 = async {
        let mut timelast: f64;
        #[allow(irrefutable_let_patterns)]
        while let that = chan_in.try_recv() {
            timelast = Date::now();
            if !that.is_err() {
                return that.unwrap();
            }

            let timenow = Date::now();
            let seg = timenow - timelast;
            if seg < RETRIEVE_ROUND_TIME {
                async_std::task::sleep(Duration::from_millis((RETRIEVE_ROUND_TIME - seg) as u64))
                    .await;
            };
        }

        return vec![];
    };

    let result = k0.await;

    return result;
}
