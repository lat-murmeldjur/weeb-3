use crate::{
    //
    JsValue,
    //
    //
    mpsc,
    //
    upload_data,
};

use serde_json::json;

#[derive(Clone)]
pub struct Node {
    pub data: Vec<u8>, // repurposed as address
    pub mime: String,
    pub filename: String,
    pub path: String,
}

pub async fn create_manifest(
    obfuscated: bool,
    encrypted: bool,
    input_forks: Vec<Node>,
    data_forks: Vec<Vec<u8>>,
    reference: Vec<u8>,
    root_manifest: bool,
    first_node_cutoff: usize,
    index: String,
    errordoc: String,
    data_upload_chan: &mpsc::Sender<(Vec<u8>, u8, mpsc::Sender<Vec<u8>>)>,
) -> Vec<u8> {
    let mut fncutoff = first_node_cutoff;

    let mut manifest_bytes_vec: Vec<u8> = vec![];

    let mut forks = input_forks.clone();

    forks.sort_by(|a, b| a.path.cmp(&b.path));

    let flen = forks.len();

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

    if forks.len() > 0 {
        let mut fork_groups0: Vec<(String, Vec<Node>)> = vec![];

        for forks0 in &forks {
            let path0 = forks0.path.clone();
            let leading_char = path0[0..1].to_string();

            let mut exists = false;
            for (a, b) in fork_groups0.iter_mut() {
                if *a == leading_char {
                    b.push(forks0.clone());
                    exists = true;
                    break;
                };
            }
            if !exists {
                fork_groups0.push((leading_char, vec![forks0.clone()]));
            };
        }

        let mut fork_groups1: Vec<(String, Vec<Node>)> = vec![];

        for (_, forkgroup0) in fork_groups0 {
            let mut common_prefix = forkgroup0[0].path.clone();
            for fork0 in &forkgroup0 {
                while !fork0.path.starts_with(&common_prefix) {
                    common_prefix.pop(); // Shorten the prefix
                }
            }

            fork_groups1.push((common_prefix, forkgroup0));
        }

        fork_groups1.sort_by(|a, b| a.0.cmp(&b.0));

        let mut cutoff_first_indicator = 0;
        for (common_prefix, mut forkgroup1) in fork_groups1 {
            // let mut forkgroup1 = fork_groups1[common_prefix].clone();
            forkgroup1.sort_by(|a, b| a.path.cmp(&b.path));
            cutoff_first_indicator += 1;
            if forkgroup1.len() == 1 {
                let forks0 = &forkgroup1[0];

                let mut vforks: Vec<String> = vec![];

                let path0: String = match cutoff_first_indicator == 1 && fncutoff > 0 {
                    false => forks0.path.clone(),
                    true => {
                        if forks0.path.len() > 30 - (fncutoff % 30) {
                            vforks.push(forks0.path[0..30 - (fncutoff % 30)].to_string());
                            let tr = forks0.path[30 - (fncutoff % 30)..].to_string();
                            fncutoff = 0;
                            tr
                        } else {
                            forks0.path.clone()
                        }
                    }
                };

                let mime0 = forks0.mime.clone();
                // let filename0 = forks0.filename.clone();
                let data_address0 = forks0.data.clone();

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
                        "Filename": &forks0.filename.clone(),
                }))
                .unwrap();

                let tip_mf = Box::pin(create_manifest(
                    obfuscated,
                    encrypted,
                    vec![],                 // forks
                    vec![],                 // data_forks
                    current_data_reference, // reference
                    false,                  // root manifest
                    0,                      // weird string prefix cutoff for first element
                    "".to_string(),         // index
                    "".to_string(),         // errordoc
                    data_upload_chan,
                ))
                .await;

                current_data_reference = upload_data(tip_mf, encrypted, data_upload_chan).await;

                for j in 0..vforks.len() {
                    let i = vforks.len() - 1 - j;

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
                            false,              // root manifest
                            0,                  // weird string prefix cutoff for first element
                            "".to_string(),     // index
                            "".to_string(),     // errordoc
                            data_upload_chan,
                        ))
                        .await;

                        current_data_reference =
                            upload_data(current_manifest, encrypted, data_upload_chan).await;
                    } else {
                        fork_bases.push(current_fork);
                    }
                }
            } else {
                let mut forkgroup2: Vec<Node> = vec![];
                for fork0 in forkgroup1 {
                    forkgroup2.push(Node {
                        data: fork0.data.clone(),
                        mime: fork0.mime.clone(),
                        filename: fork0.filename.clone(),
                        path: fork0.path[common_prefix.len()..].to_string(),
                    });
                }

                let group_manifest = Box::pin(create_manifest(
                    obfuscated,
                    encrypted,
                    forkgroup2,                     // forks
                    vec![],                         // data_forks
                    vec![],                         // reference
                    false,                          // root manifest
                    fncutoff + common_prefix.len(), // weird string prefix cutoff for first element
                    "".to_string(),                 // index
                    "".to_string(),                 // errordoc
                    data_upload_chan,
                ))
                .await;

                fncutoff = 0;

                let group_data_reference =
                    upload_data(group_manifest, encrypted, data_upload_chan).await;

                let group_fork = create_fork(
                    common_prefix.to_string(),
                    group_data_reference.clone(),
                    vec![],
                )
                .await;

                fork_bases.push(group_fork);
            }
        }
    }

    if root_manifest {
        let root_metadata = serde_json::to_vec(&json!({
            "website-index-document": index,
        }))
        .unwrap();

        let mut stub_ref_size: u8 = 0;
        if flen > 0 {
            if encrypted {
                stub_ref_size = 64;
            } else {
                stub_ref_size = 32;
            }
        }

        let stub_reference = upload_data(
            create_stub(stub_ref_size, obfuscated).await,
            encrypted,
            data_upload_chan,
        )
        .await;

        let mut root_fork = create_fork("/".to_string(), stub_reference, root_metadata).await;
        fork_bases_virtual.push(root_fork[0..3].to_vec());
        manifest_bytes_vec.append(&mut root_fork);
    }

    web_sys::console::log_1(&JsValue::from(format!(
        "Manifest length after node forks: {}",
        manifest_bytes_vec.len()
    )));

    fork_bases.sort_by(|a, b| forkstring(a.to_vec()).cmp(&forkstring(b.to_vec())));

    let mut kj = 0;
    for log in &fork_bases {
        web_sys::console::log_1(&JsValue::from(format!(
            "Sorted prefix: {} {}",
            kj,
            forkstring(log.to_vec())
        )));
        kj += 1;
    }

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

    {
        let obfuscation_key = &manifest_bytes_vec[0..32];
        let enc_obfuscation_key = hex::encode(obfuscation_key);

        let mut manifest_bytes_obfuscated = (&manifest_bytes_vec[..32]).to_vec();

        if enc_obfuscation_key != "0000000000000000000000000000000000000000000000000000000000000000"
        {
            let creylen = obfuscation_key.len();
            let mut done = false;
            let mut i = 0;
            while !done {
                let mut k = creylen;
                if k > manifest_bytes_vec.len() - (32 + i * creylen) {
                    k = manifest_bytes_vec.len() - (32 + i * creylen);
                };

                for j in (32 + i * creylen)..(32 + i * creylen + k) {
                    manifest_bytes_obfuscated
                        .push(manifest_bytes_vec[j] ^ obfuscation_key[j - 32 - i * creylen]);
                }

                i += 1;

                if !(32 + i * creylen < manifest_bytes_vec.len()) {
                    done = true;
                }
            }

            return manifest_bytes_obfuscated;
        }
    }

    manifest_bytes_vec
}

pub async fn create_fork(path: String, reference: Vec<u8>, metadata: Vec<u8>) -> Vec<u8> {
    let mut node: Vec<u8> = vec![];

    if metadata.len() == 0 {
        if path.contains("/") {
            node.push(12_u8);
        } else {
            node.push(4_u8);
        }
    } else {
        if path.contains("/") && path.len() > 1 {
            node.push(26_u8);
        } else {
            node.push(18_u8);
        }
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

        // if metadataJSONBytesSizeWithSize < 32
        // paddingLength := 32 - metadataJSONBytesSizeWithSize
        // } else if metadataJSONBytesSizeWithSize > 32
        // paddingLength := 32 - metadataJSONBytesSizeWithSize % 32

        //

        let xl1 = match xl0 < 32 {
            true => 32 - xl0,
            false => 32 - (xl0 % 32),
        };

        let xl = xl0 + xl1;

        node.append(&mut ((xl - 2) as u16).to_be_bytes().to_vec());
        node.append(&mut metadata.clone());

        let pdl = xl - 2 - metadata.len();
        for _ in 0..pdl {
            node.push(10_u8);
        }
    }

    return node;
}

pub async fn create_stub(stub_ref_size: u8, obfuscated: bool) -> Vec<u8> {
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

    manifest_bytes_vec.push(stub_ref_size);

    for _ in 0..32 {
        manifest_bytes_vec.push(0_u8);
    }

    for _ in 0..stub_ref_size {
        manifest_bytes_vec.push(0_u8);
    }

    {
        let obfuscation_key = &manifest_bytes_vec[0..32];
        let enc_obfuscation_key = hex::encode(obfuscation_key);

        let mut manifest_bytes_obfuscated = (&manifest_bytes_vec[..32]).to_vec();

        if enc_obfuscation_key != "0000000000000000000000000000000000000000000000000000000000000000"
        {
            let creylen = obfuscation_key.len();
            let mut done = false;
            let mut i = 0;
            while !done {
                let mut k = creylen;
                if k > manifest_bytes_vec.len() - (32 + i * creylen) {
                    k = manifest_bytes_vec.len() - (32 + i * creylen);
                };

                for j in (32 + i * creylen)..(32 + i * creylen + k) {
                    manifest_bytes_obfuscated
                        .push(manifest_bytes_vec[j] ^ obfuscation_key[j - 32 - i * creylen]);
                }

                i += 1;

                if !(32 + i * creylen < manifest_bytes_vec.len()) {
                    done = true;
                }
            }

            return manifest_bytes_obfuscated;
        }
    }

    return manifest_bytes_vec;
}

// nodeType <1 byte>
// prefixLength <1 byte>
// prefix <30 byte>
// reference <32/64 bytes>
// metadataBytesSize <2 bytes>
// metadataBytes <varlen>

fn forkstring(fork: Vec<u8>) -> String {
    let pl = fork[1] as usize;
    let prefix = fork[2..2 + pl].to_vec();
    String::from_utf8(prefix).unwrap_or("".to_string())
}
