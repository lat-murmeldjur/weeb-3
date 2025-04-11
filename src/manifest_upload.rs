use crate::{
    //
    mpsc,
    //
    upload_data,
    //
    JsValue,
};

use serde_json::json;

pub struct Node {
    pub data: Vec<u8>, // repurposed as address
    pub mime: String,
    pub _filename: String,
    pub path: String,
}

#[allow(dead_code)]
pub async fn create_manifest(
    obfuscated: bool,
    encrypted: bool,
    forks: Vec<Node>,
    data_forks: Vec<Vec<u8>>,
    reference: Vec<u8>,
    index: String,
    data_upload_chan: &mpsc::Sender<(Vec<u8>, u8, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let mut manifest_bytes_vec: Vec<u8> = vec![];

    for _ in 0..32 {
        if !obfuscated {
            manifest_bytes_vec.push(0_u8);
        } else {
            manifest_bytes_vec.push(rand::random::<u8>());
        }
    }

    web_sys::console::log_1(&JsValue::from(format!(
        "Manifest length after obfuscation key: {}",
        manifest_bytes_vec.len()
    )));

    manifest_bytes_vec.append(
        &mut hex::decode("5768b3b6a7db56d21d1abff40d41cebfc83448fed8d7e9b06ec0d3b073f28f").unwrap(),
    );

    web_sys::console::log_1(&JsValue::from(format!(
        "Manifest length after mf version: {}",
        manifest_bytes_vec.len()
    )));

    let mut ref_length: u8 = 32;

    if encrypted {
        ref_length = 64;
    }

    if reference.len() != 0 {
        if reference.len() == 32 {
            ref_length = 32;
        } else if reference.len() == 64 {
            ref_length = 64;
        } else {
            web_sys::console::log_1(&JsValue::from(format!(
                "Manifest reference irregular length {:#?}!",
                hex::encode(&reference)
            )));
            return vec![];
        }
    }

    manifest_bytes_vec.push(ref_length);
    manifest_bytes_vec.append(&mut reference.clone());

    if reference.len() == 0 {
        for _ in 0..ref_length {
            manifest_bytes_vec.push(0_u8);
        }
    };

    web_sys::console::log_1(&JsValue::from(format!(
        "Manifest length after reference: {}",
        manifest_bytes_vec.len()
    )));

    // index bytes ?

    let index_bytes_start = manifest_bytes_vec.len();

    for _ in 0..32 {
        manifest_bytes_vec.push(0_u8);
    }

    web_sys::console::log_1(&JsValue::from(format!(
        "Manifest length after index bytes: {}",
        manifest_bytes_vec.len()
    )));

    let mut fork_bases: Vec<Vec<u8>> = vec![];
    let mut fork_bases_virtual: Vec<Vec<u8>> = vec![];

    for forks0 in &forks {
        let path0 = forks0.path.clone();
        let mime0 = forks0.mime.clone();
        // let filename0 = forks0.filename.clone();
        let data_address0 = forks0.data.clone();

        let mut vforks: Vec<String> = vec![];
        let mut section_begin;
        let mut section_end;
        let mut partial_section = path0.len() % 30;
        if partial_section > 0 {
            partial_section = 1;
        }
        for i in 0..(path0.len() / 30) + partial_section {
            section_begin = i * 30;
            section_end = (i + 1) * 30;
            if (i + 1) * 30 > path0.len() {
                section_end = path0.len();
            }
            vforks.push(path0[section_begin..section_end].to_string())
        }

        let mut current_data_reference: Vec<u8> = data_address0;
        let mut current_fork: Vec<u8>;

        let value_final = serde_json::to_vec(&json!({
                "Content-Type": mime0,
                "Filename": hex::encode(&forks0.data.clone()),
        }))
        .unwrap();

        let tip_mf = Box::pin(create_manifest(
            obfuscated,
            encrypted,
            vec![],                 // forks
            vec![],                 // data_forks
            current_data_reference, // reference
            "".to_string(),         // inde
            data_upload_chan,
        ))
        .await;

        current_data_reference = upload_data(tip_mf, encrypted, data_upload_chan).await;
        web_sys::console::log_1(&JsValue::from(format!("vfll {}", vforks.len())));
        for j in 0..vforks.len() {
            let i = vforks.len() - 1 - j;
            web_sys::console::log_1(&JsValue::from(format!("vfl {}", i)));

            //
            let mut current_metadata = vec![];
            if i == vforks.len() - 1 {
                current_metadata = value_final.clone();
            }

            current_fork = create_fork(
                vforks[i].clone(),
                current_data_reference.clone(),
                current_metadata,
            )
            .await;

            if i > 0 {
                let current_manifest = Box::pin(create_manifest(
                    obfuscated,
                    encrypted,
                    vec![],             // forks
                    vec![current_fork], // data_forks
                    vec![],             // reference
                    "".to_string(),     // inde
                    data_upload_chan,
                ))
                .await;

                current_data_reference =
                    upload_data(current_manifest, encrypted, data_upload_chan).await;
            } else {
                fork_bases.push(current_fork);
            }
        }
    }

    if forks.len() > 0 {
        let root_metadata = serde_json::to_vec(&json!({
            "website-index-document": index,
        }))
        .unwrap();

        let stub_reference = upload_data(create_stub().await, encrypted, data_upload_chan).await;

        let mut root_fork = create_fork("/".to_string(), stub_reference, root_metadata).await;
        fork_bases_virtual.push(root_fork[0..3].to_vec());
        manifest_bytes_vec.append(&mut root_fork);
    }

    web_sys::console::log_1(&JsValue::from(format!(
        "Manifest length after node forks: {}",
        manifest_bytes_vec.len()
    )));

    for f1 in &data_forks {
        manifest_bytes_vec.append(&mut f1.clone());
    }

    for f2 in &fork_bases {
        manifest_bytes_vec.append(&mut f2.clone());
    }

    web_sys::console::log_1(&JsValue::from(format!(
        "Manifest length after data node forks: {}",
        manifest_bytes_vec.len()
    )));

    // set index_bytes

    let mut bits_as_bytes = [0_u8; 32];

    for f1 in data_forks {
        let b: u8 = f1[2];

        web_sys::console::log_1(&JsValue::from(format!("######## {}", b)));

        bits_as_bytes[(b / 8) as usize] |= 1 << (b % 8);
    }

    for f2 in fork_bases {
        let b: u8 = f2[2];

        bits_as_bytes[(b / 8) as usize] |= 1 << (b % 8);
    }

    for f3 in fork_bases_virtual {
        let b: u8 = f3[2];

        bits_as_bytes[(b / 8) as usize] |= 1 << (b % 8);
    }

    for i in 0..32 {
        manifest_bytes_vec[index_bytes_start + i] = bits_as_bytes[i];
    }

    // bits_for_bytes [b/8] |= 1 << (b % 8)
    //  forks: Vec<Node>,
    //  data_forks: Vec<u8>,

    manifest_bytes_vec
}

pub async fn create_fork(path: String, reference: Vec<u8>, metadata: Vec<u8>) -> Vec<u8> {
    let mut node: Vec<u8> = vec![];

    if metadata.len() == 0 {
        node.push(4_u8);
    } else {
        node.push(18_u8);
    };

    if path.len() > 30 {
        web_sys::console::log_1(&JsValue::from(format!(
            "Fork string prefix overlength {:#?}!",
            path
        )));

        return vec![];
    } else {
        node.push(path.len() as u8);
        node.append(&mut path.as_bytes().to_vec());
        for _ in 0..(30 - path.len()) {
            node.push(0_u8);
        }
    }

    if reference.len() == 32 || reference.len() == 64 {
        node.append(&mut reference.clone());
    } else {
        for _ in 0..32 {
            node.push(0_u8);
        }
        web_sys::console::log_1(&JsValue::from(format!(
            "Manifest reference default length {:#?}!",
            hex::encode(&reference)
        )));
    }
    if metadata.len() > 0 {
        let xl0 = 2 + metadata.len();
        let mut xl1 = xl0 % 32;
        if xl1 > 0 {
            xl1 = 1;
        }
        let xl = xl0 + 32 * xl1 - (xl0 % 32);

        node.append(&mut ((xl - 2) as u16).to_be_bytes().to_vec());

        node.append(&mut metadata.clone());

        let pdl = xl - 2 - metadata.len();
        for _ in 0..pdl {
            node.push(10_u8);
        }
    }

    return node;
}

pub async fn create_stub() -> Vec<u8> {
    let mut manifest_bytes_vec: Vec<u8> = vec![];

    for _ in 0..32 {
        manifest_bytes_vec.push(0_u8);
    }

    manifest_bytes_vec.append(
        &mut hex::decode("5768b3b6a7db56d21d1abff40d41cebfc83448fed8d7e9b06ec0d3b073f28f").unwrap(),
    );

    manifest_bytes_vec.push(0_u8);

    for _ in 0..32 {
        manifest_bytes_vec.push(0_u8);
    }

    return manifest_bytes_vec;
}

// nodeType <1 byte>
// prefixLength <1 byte>
// prefix <30 byte>
// reference <32/64 bytes>
// metadataBytesSize <2 bytes>
// metadataBytes <varlen>
