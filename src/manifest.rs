use crate::{
    //
    get_feed_address,
    //
    JsValue,
    //
    RETRIEVE_ROUND_TIME,
};

use std::time::Duration;

use serde_json::Value;

use js_sys::Date;
use std::sync::mpsc;

pub struct Fork {
    //    metadata: Value,
    pub data: Vec<u8>,
    pub mime: String,
    // pub filename: String,
    pub path: String,
}

pub async fn interpret_manifest(
    path_prefix_heritance: String,
    cd: &Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
) -> (Vec<Fork>, String) {
    let mut ind: String = "".to_string();
    let mut ind_set = false;

    if cd.len() == 0 {
        return (
            vec![Fork {
                data: vec![],
                mime: "undefined".to_string(),
                // filename: "not found".to_string(),
                path: "not found".to_string(),
            }],
            ind,
        );
    }

    if cd.len() < 72 {
        web_sys::console::log_1(&JsValue::from(format!("returning early 0")));

        return (
            vec![Fork {
                data: cd.to_vec(),
                mime: "application/octet-stream".to_string(),
                // filename: "unknown00".to_string(),
                path: "unknown00".to_string(),
            }],
            ind,
        );
    }

    // commented out for later use

    //    let obfuscation_key = &cd[8..40];
    //    let enc_obfuscation_key = hex::encode(obfuscation_key);

    let mf_version = &cd[40..71];
    let enc_mf_version = hex::encode(mf_version);

    if enc_mf_version != "5768b3b6a7db56d21d1abff40d41cebfc83448fed8d7e9b06ec0d3b073f28f"
        && enc_mf_version != "025184789d63635766d78c41900196b57d7400875ebe4d9b5d1e76bd9652a9"
    {
        web_sys::console::log_1(&JsValue::from(format!("returning early 1")));
        return (
            vec![Fork {
                data: cd.to_vec(),
                mime: "application/octet-stream".to_string(),
                // filename: "unknown01".to_string(),
                path: "unknown01".to_string(),
            }],
            ind,
        );
    }

    let ref_size = cd[71];

    let ref_delimiter = (72 + ref_size) as usize;
    // let actual_reference = &cd[72..ref_delimiter];

    let index_delimiter = (ref_delimiter + 32) as usize;
    // let index = &cd[ref_delimiter..index_delimiter];

    // fork parts

    #[allow(unused_assignments)]
    let mut parts = vec![];
    let mut fork_start_current = index_delimiter;

    while cd.len() > fork_start_current {
        let fork_start = fork_start_current;
        let fork_type = cd[fork_start_current];
        let enc_fork_type = hex::encode(&[fork_type]);
        web_sys::console::log_1(&JsValue::from(format!("enc_fork_type: {}", enc_fork_type)));

        let fork_prefix_length = cd[fork_start_current + 1];

        let fork_prefix = &cd[fork_start + 2..fork_start + 2 + (fork_prefix_length as usize)];

        let string_fork_prefix = String::from_utf8(fork_prefix.to_vec()).unwrap_or("".to_string());
        web_sys::console::log_1(&JsValue::from(format!(
            "string_fork_prefix: {}",
            string_fork_prefix
        )));

        let fork_prefix_delimiter = fork_start + 32;
        let fork_reference_delimiter = fork_prefix_delimiter + (ref_size as usize);
        let fork_reference = &cd[fork_prefix_delimiter..fork_reference_delimiter];
        let enc_fork_reference = hex::encode(fork_reference);
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_reference: {}",
            enc_fork_reference
        )));

        let ref_data = get_data(fork_reference.to_vec(), data_retrieve_chan).await;

        if fork_type & 16 == 16 {
            let fork_metadata_bytesize: [u8; 2] = cd
                [fork_reference_delimiter..fork_reference_delimiter + 2]
                .try_into()
                .unwrap();

            let calc_metadata_bytesize = u16::from_be_bytes(fork_metadata_bytesize) as usize;

            let fork_metadata_delimiter = fork_reference_delimiter + 2 + calc_metadata_bytesize;
            fork_start_current = fork_metadata_delimiter;

            let fork_metadata = &cd[fork_reference_delimiter + 2..fork_metadata_delimiter];
            let enc_fork_metadata = hex::encode(fork_metadata);
            web_sys::console::log_1(&JsValue::from(format!(
                "fork_metadata: {}",
                enc_fork_metadata
            )));

            let v1: Value = serde_json::from_slice(fork_metadata).unwrap_or("nil".into());
            web_sys::console::log_1(&JsValue::from(format!("metadata json: {:#?} ", v1)));

            let mut feed = false;
            let mut owner: String = "".to_string();
            let mut topic: String = "".to_string();

            let str0f0 = v1.get("swarm-feed-owner");
            match str0f0 {
                Some(str0f0) => {
                    owner = str0f0.as_str().unwrap().to_string();
                    let str0f1 = v1.get("swarm-feed-topic");
                    match str0f1 {
                        Some(str0f1) => {
                            topic = str0f1.as_str().unwrap().to_string();
                            feed = true;
                        }
                        _ => (),
                    }
                }
                _ => (),
            };

            if feed {
                let feed_address = get_feed_address(owner, topic, 0);
                let feed_data_soc = get_data(feed_address, data_retrieve_chan).await;
                let feed_data_content =
                    get_data(feed_data_soc[16..48].to_vec(), data_retrieve_chan).await;

                let (mut appendix_0, _nondiscard) = Box::pin(interpret_manifest(
                    "".to_string(),
                    &feed_data_content,
                    data_retrieve_chan,
                ))
                .await;

                if !ind_set {
                    ind = _nondiscard;
                    ind_set = true;
                }

                parts.append(&mut appendix_0);
            }

            let str0i = v1.get("website-index-document");
            match str0i {
                Some(str0i) => {
                    ind = str0i.as_str().unwrap().to_string();
                    ind_set = true;
                }
                _ => (),
            };

            let str0 = v1.get("Content-Type");

            let str1 = match str0 {
                Some(str0) => str0.as_str().unwrap(),
                _ => {
                    let mut bequeath: String = String::new();
                    bequeath.push_str(&path_prefix_heritance);
                    bequeath.push_str(&string_fork_prefix);

                    let (mut appendix_0, _discard) =
                        Box::pin(interpret_manifest(bequeath, &ref_data, data_retrieve_chan)).await;
                    parts.append(&mut appendix_0);
                    continue;
                }
            };

            // let str2 = v1.get("Filename").unwrap().as_str().unwrap();

            let mime_0 = str1.to_string();
            // let filename_0 = str2.to_string();

            let ref_size_a = ref_data[71];
            let actual_data = get_data(
                ref_data[72..72 + (ref_size_a as usize)].to_vec(),
                data_retrieve_chan,
            )
            .await;

            let mut path_0: String = String::new();
            path_0.push_str(&path_prefix_heritance);
            path_0.push_str(&string_fork_prefix);

            parts.push(Fork {
                data: actual_data.to_vec(),
                mime: mime_0,
                // filename: filename_0,
                path: path_0,
            });
        }

        if fork_type & 16 == 0 {
            fork_start_current = fork_start + 32 + (ref_size as usize);
            let mut bequeath: String = String::new();
            bequeath.push_str(&path_prefix_heritance);
            bequeath.push_str(&string_fork_prefix);
            let (mut appendix_0, _discard) =
                Box::pin(interpret_manifest(bequeath, &ref_data, data_retrieve_chan)).await;
            parts.append(&mut appendix_0);
        }
    }

    return (parts, ind);
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
