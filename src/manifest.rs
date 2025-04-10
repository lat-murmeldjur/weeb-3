use std::sync::mpsc;

use crate::{
    //
    get_data,
    //
    seek_latest_feed_update,
    //
    JsValue,
};

use serde_json::Value;

pub struct Fork {
    //    metadata: Value,
    pub data: Vec<u8>,
    pub mime: String,
    // pub filename: String,
    pub path: String,
}

pub async fn interpret_manifest(
    path_prefix_heritance: String,
    cd0: &Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, u8, mpsc::Sender<Vec<u8>>)>,
) -> (Vec<Fork>, String) {
    let mut ind: String = "".to_string();
    let mut ind_set = false;
    let mut manifest_encrypted = false;

    if cd0.len() == 0 {
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

    if cd0.len() < 72 {
        return (
            vec![Fork {
                data: cd0.to_vec(),
                mime: "application/octet-stream".to_string(),
                // filename: "unknown00".to_string(),
                path: "unknown00".to_string(),
            }],
            ind,
        );
    }

    let obfuscation_key = &cd0[8..40];
    let enc_obfuscation_key = hex::encode(obfuscation_key);

    let mut cd = (&cd0[..40]).to_vec();

    if enc_obfuscation_key != "0000000000000000000000000000000000000000000000000000000000000000" {
        manifest_encrypted = true;

        let creylen = obfuscation_key.len();
        let mut done = false;
        let mut i = 0;
        while !done {
            let mut k = creylen;
            if k > cd0.len() - (40 + i * creylen) {
                k = cd0.len() - (40 + i * creylen);
            };

            for j in (40 + i * creylen)..(40 + i * creylen + k) {
                cd.push(cd0[j] ^ obfuscation_key[j - 40 - i * creylen]);
            }

            i += 1;

            if !(40 + i * creylen < cd0.len()) {
                done = true;
            }
        }
    } else {
        cd = cd0.to_vec();
    }

    let mf_version = &cd[40..71];
    let enc_mf_version = hex::encode(mf_version);
    web_sys::console::log_1(&JsValue::from(format!("mf_version: {}", enc_mf_version)));

    if enc_mf_version != "5768b3b6a7db56d21d1abff40d41cebfc83448fed8d7e9b06ec0d3b073f28f"
        && enc_mf_version != "025184789d63635766d78c41900196b57d7400875ebe4d9b5d1e76bd9652a9"
    {
        return (
            vec![Fork {
                data: cd0.to_vec(),
                mime: "application/octet-stream".to_string(),
                // filename: "unknown01".to_string(),
                path: "unknown01".to_string(),
            }],
            ind,
        );
    }

    let ref_size = cd[71];
    web_sys::console::log_1(&JsValue::from(format!("ref_size: {}", ref_size)));

    let ref_delimiter = (72 + ref_size) as usize;
    let index_delimiter = (ref_delimiter + 32) as usize;

    if ref_size > 0 {
        let manifest_reference = &cd[72..ref_delimiter];
        web_sys::console::log_1(&JsValue::from(format!(
            "manifest_reference: {}",
            hex::encode(manifest_reference)
        )));
    }

    let index_bytes = &cd[ref_delimiter..index_delimiter];
    web_sys::console::log_1(&JsValue::from(format!(
        "forks_index_bytes: {}",
        hex::encode(index_bytes)
    )));

    // fork parts

    #[allow(unused_assignments)]
    let mut parts = vec![];
    let mut fork_start_current = index_delimiter;

    while cd.len() > fork_start_current {
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_start/len: {}  / {}",
            fork_start_current,
            cd.len(),
        )));

        let fork_start = fork_start_current;
        let fork_type = cd[fork_start_current];
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_type: {}",
            hex::encode(&[fork_type])
        )));

        let fork_prefix_length = cd[fork_start_current + 1];
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_prefix_length: {}",
            hex::encode(&[fork_prefix_length])
        )));

        let fork_prefix = &cd[fork_start + 2..fork_start + 2 + (fork_prefix_length as usize)];
        let string_fork_prefix = String::from_utf8(fork_prefix.to_vec()).unwrap_or("".to_string());
        web_sys::console::log_1(&JsValue::from(format!(
            "string_fork_prefix: {} {}",
            hex::encode(&string_fork_prefix),
            string_fork_prefix
        )));

        let fork_prefix_delimiter = fork_start + 32;
        let fork_reference_delimiter = fork_prefix_delimiter + (ref_size as usize);
        let fork_reference = &cd[fork_prefix_delimiter..fork_reference_delimiter];
        web_sys::console::log_1(&JsValue::from(format!(
            "fork_reference: {}",
            hex::encode(fork_reference)
        )));

        let ref_data = get_data(fork_reference.to_vec(), data_retrieve_chan).await;

        if fork_type & 16 == 16 {
            web_sys::console::log_1(&JsValue::from(format!("fork_type: metadata",)));

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
                let feed_data_soc =
                    seek_latest_feed_update(owner, topic, data_retrieve_chan, 8).await;

                let feed_data_content =
                    get_data(feed_data_soc[16..48].to_vec(), data_retrieve_chan).await;

                web_sys::console::log_1(&JsValue::from(format!(
                    "dispatch interpret manifest for reference in feed head soc ",
                )));

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

                    web_sys::console::log_1(&JsValue::from(format!(
                        "dispatch interpret manifest for with metadata fork reference with no content type",
                    )));

                    let (mut appendix_0, _discard) =
                        Box::pin(interpret_manifest(bequeath, &ref_data, data_retrieve_chan)).await;
                    parts.append(&mut appendix_0);
                    continue;
                }
            };

            // let str2 = v1.get("Filename").unwrap().as_str().unwrap();

            let mime_0 = str1.to_string();
            // let filename_0 = str2.to_string();
            if ref_data.len() > 71 {
                web_sys::console::log_1(&JsValue::from(format!(
                    "inline interpret manifest for with metadata fork reference with content type",
                )));

                let mut ref_data0 = (&ref_data[..40]).to_vec();
                let mut ref_size_a = ref_data[71];

                if manifest_encrypted {
                    let ref_data_obfuscation_key = &ref_data[8..40];

                    let ref_creylen = ref_data_obfuscation_key.len();
                    let mut done = false;
                    let mut i = 0;
                    while !done {
                        let mut k = ref_creylen;
                        if k > ref_data.len() - (40 + i * ref_creylen) {
                            k = ref_data.len() - (40 + i * ref_creylen);
                        };

                        for j in (40 + i * ref_creylen)..(40 + i * ref_creylen + k) {
                            ref_data0.push(
                                ref_data[j] ^ ref_data_obfuscation_key[j - 40 - i * ref_creylen],
                            );
                        }

                        i += 1;

                        if !(40 + i * ref_creylen < ref_data.len()) {
                            done = true;
                        }
                    }

                    ref_size_a = ref_data0[71];
                }

                if ref_data.len() > 72 + (ref_size_a as usize) {
                    let mut actual_data_address = ref_data[72..72 + (ref_size_a as usize)].to_vec();

                    if manifest_encrypted {
                        actual_data_address = ref_data0[72..72 + (ref_size_a as usize)].to_vec();
                    }

                    web_sys::console::log_1(&JsValue::from(format!(
                        "metadata_fork_reference_with_content_type_data_address {}",
                        hex::encode(&actual_data_address)
                    )));

                    let actual_data = get_data(actual_data_address, data_retrieve_chan).await;

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
            }
        }

        if fork_type & 16 == 0 {
            web_sys::console::log_1(&JsValue::from(format!("fork_type: no metadata",)));

            fork_start_current = fork_start + 32 + (ref_size as usize);
            let mut bequeath: String = String::new();
            bequeath.push_str(&path_prefix_heritance);
            bequeath.push_str(&string_fork_prefix);
            web_sys::console::log_1(&JsValue::from(format!(
                "dispatch interpret manifest for fork with no metadata",
            )));
            let (mut appendix_0, _discard) =
                Box::pin(interpret_manifest(bequeath, &ref_data, data_retrieve_chan)).await;
            parts.append(&mut appendix_0);
        }
    }

    return (parts, ind);
}
