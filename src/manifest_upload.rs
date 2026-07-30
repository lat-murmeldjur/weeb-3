use crate::{
    erasure_coding::RedundancyLevel,
    manifest::{
        MANTARAY_PREFIX_MAX_BYTES, common_prefix_bytes, encode_fork,
        encode_fork_with_separator_path, ordered_indexed_forks, split_prefix_bytes,
    },
    mpsc,
    upload::{DataUploadRequest, UploadProgressSender},
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

#[derive(Clone)]
struct BytePathNode {
    data: Vec<u8>,
    mime: String,
    filename: String,
    path: Vec<u8>,
}

pub async fn create_manifest(
    obfuscated: bool,
    encrypted: bool,
    redundancy_level: RedundancyLevel,
    input_forks: Vec<Node>,
    data_forks: Vec<Vec<u8>>,
    reference: Vec<u8>,
    root_manifest: bool,
    first_node_cutoff: usize,
    index: String,
    errordoc: String,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    data_upload_chan: &mpsc::Sender<DataUploadRequest>,
    progress: Option<UploadProgressSender>,
) -> Vec<u8> {
    let input_forks = input_forks
        .into_iter()
        .map(|node| BytePathNode {
            data: node.data,
            mime: node.mime,
            filename: node.filename,
            path: node.path.into_bytes(),
        })
        .collect();

    create_manifest_bytes(
        obfuscated,
        encrypted,
        redundancy_level,
        input_forks,
        data_forks,
        reference,
        root_manifest,
        first_node_cutoff,
        index,
        errordoc,
        batch_owner,
        batch_id,
        data_upload_chan,
        progress,
    )
    .await
}

async fn create_manifest_bytes(
    obfuscated: bool,
    encrypted: bool,
    redundancy_level: RedundancyLevel,
    input_forks: Vec<BytePathNode>,
    data_forks: Vec<Vec<u8>>,
    reference: Vec<u8>,
    root_manifest: bool,
    first_node_cutoff: usize,
    index: String,
    errordoc: String,
    batch_owner: Vec<u8>,
    batch_id: Vec<u8>,
    data_upload_chan: &mpsc::Sender<DataUploadRequest>,
    progress: Option<UploadProgressSender>,
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

    manifest_bytes_vec.append(
        &mut hex::decode("5768b3b6a7db56d21d1abff40d41cebfc83448fed8d7e9b06ec0d3b073f28f").unwrap(),
    );

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
            return vec![];
        }
    }

    manifest_bytes_vec.push(ref_length);
    manifest_bytes_vec.append(&mut reference.clone());

    if reference.is_empty() {
        for _ in 0..ref_length {
            manifest_bytes_vec.push(0_u8);
        }
    };

    let index_bytes_start = manifest_bytes_vec.len();

    for _ in 0..32 {
        manifest_bytes_vec.push(0_u8);
    }

    let mut fork_bases: Vec<Vec<u8>> = vec![];

    if !forks.is_empty() {
        let mut fork_groups0: Vec<(u8, Vec<BytePathNode>)> = vec![];

        for fork in &forks {
            let Some(&leading_byte) = fork.path.first() else {
                return vec![];
            };

            if let Some((_, group)) = fork_groups0
                .iter_mut()
                .find(|(byte, _)| *byte == leading_byte)
            {
                group.push(fork.clone());
            } else {
                fork_groups0.push((leading_byte, vec![fork.clone()]));
            }
        }

        let mut fork_groups1: Vec<(Vec<u8>, Vec<BytePathNode>)> = vec![];
        for (_, forkgroup0) in fork_groups0 {
            let paths = forkgroup0
                .iter()
                .map(|fork| fork.path.as_slice())
                .collect::<Vec<_>>();
            let Some(common_prefix) = common_prefix_bytes(&paths) else {
                return vec![];
            };
            fork_groups1.push((common_prefix, forkgroup0));
        }

        fork_groups1.sort_by(|a, b| a.0.cmp(&b.0));

        let mut cutoff_first_indicator = 0;
        for (common_prefix, mut forkgroup1) in fork_groups1 {
            forkgroup1.sort_by(|a, b| a.path.cmp(&b.path));
            cutoff_first_indicator += 1;
            if forkgroup1.len() == 1 {
                let fork = &forkgroup1[0];
                let first_capacity = if cutoff_first_indicator == 1 && fncutoff > 0 {
                    MANTARAY_PREFIX_MAX_BYTES - (fncutoff % MANTARAY_PREFIX_MAX_BYTES)
                } else {
                    MANTARAY_PREFIX_MAX_BYTES
                };
                let Some(vforks) = split_prefix_bytes(&fork.path, first_capacity) else {
                    return vec![];
                };
                if first_capacity < MANTARAY_PREFIX_MAX_BYTES && fork.path.len() > first_capacity {
                    fncutoff = 0;
                }

                let mut current_data_reference = fork.data.clone();
                let value_final = serde_json::to_vec(&json!({
                    "Content-Type": &fork.mime,
                    "Filename": &fork.filename,
                }))
                .unwrap();

                let tip_mf = Box::pin(create_manifest_bytes(
                    obfuscated,
                    encrypted,
                    redundancy_level,
                    vec![],
                    vec![],
                    current_data_reference,
                    false,
                    0,
                    String::new(),
                    String::new(),
                    batch_owner.clone(),
                    batch_id.clone(),
                    data_upload_chan,
                    progress.clone(),
                ))
                .await;
                if tip_mf.is_empty() {
                    return vec![];
                }

                current_data_reference = upload_data(
                    vec![tip_mf],
                    encrypted,
                    redundancy_level,
                    batch_owner.clone(),
                    batch_id.clone(),
                    data_upload_chan,
                    progress.clone(),
                )
                .await;
                if current_data_reference.is_empty() {
                    return vec![];
                }

                for i in (0..vforks.len()).rev() {
                    let current_metadata = if i == vforks.len() - 1 {
                        value_final.clone()
                    } else {
                        vec![]
                    };
                    let Some(current_fork) = encode_fork(
                        &vforks[i],
                        &current_data_reference,
                        &current_metadata,
                        current_metadata.is_empty(),
                    ) else {
                        return vec![];
                    };

                    if i > 0 {
                        let current_manifest = Box::pin(create_manifest_bytes(
                            obfuscated,
                            encrypted,
                            redundancy_level,
                            vec![],
                            vec![current_fork],
                            vec![],
                            false,
                            0,
                            String::new(),
                            String::new(),
                            batch_owner.clone(),
                            batch_id.clone(),
                            data_upload_chan,
                            progress.clone(),
                        ))
                        .await;
                        if current_manifest.is_empty() {
                            return vec![];
                        }

                        current_data_reference = upload_data(
                            vec![current_manifest],
                            encrypted,
                            redundancy_level,
                            batch_owner.clone(),
                            batch_id.clone(),
                            data_upload_chan,
                            progress.clone(),
                        )
                        .await;
                        if current_data_reference.is_empty() {
                            return vec![];
                        }
                    } else {
                        fork_bases.push(current_fork);
                    }
                }
            } else {
                let separator_path = forkgroup1
                    .last()
                    .map(|fork| fork.path.clone())
                    .unwrap_or_else(|| common_prefix.clone());
                let mut exact_value = None;
                let mut descendants = Vec::with_capacity(forkgroup1.len());
                for mut fork in forkgroup1 {
                    if fork.path.len() == common_prefix.len() {
                        // Bee's Add semantics make the last sorted duplicate win.
                        exact_value = Some(fork);
                    } else {
                        fork.path = fork.path[common_prefix.len()..].to_vec();
                        descendants.push(fork);
                    }
                }

                let has_edge = !descendants.is_empty();
                let (group_reference, group_metadata) = if let Some(exact) = exact_value {
                    let metadata = serde_json::to_vec(&json!({
                        "Content-Type": exact.mime,
                        "Filename": exact.filename,
                    }))
                    .unwrap();
                    (exact.data, metadata)
                } else {
                    (vec![], vec![])
                };

                let group_manifest = Box::pin(create_manifest_bytes(
                    obfuscated,
                    encrypted,
                    redundancy_level,
                    descendants,
                    vec![],
                    group_reference,
                    false,
                    fncutoff + common_prefix.len(),
                    String::new(),
                    String::new(),
                    batch_owner.clone(),
                    batch_id.clone(),
                    data_upload_chan,
                    progress.clone(),
                ))
                .await;
                if group_manifest.is_empty() {
                    return vec![];
                }

                fncutoff = 0;

                let group_data_reference = upload_data(
                    vec![group_manifest],
                    encrypted,
                    redundancy_level,
                    batch_owner.clone(),
                    batch_id.clone(),
                    data_upload_chan,
                    progress.clone(),
                )
                .await;
                if group_data_reference.is_empty() {
                    return vec![];
                }

                let Some(group_fork) = encode_fork_with_separator_path(
                    &common_prefix,
                    &group_data_reference,
                    &group_metadata,
                    has_edge,
                    &separator_path,
                ) else {
                    return vec![];
                };

                fork_bases.push(group_fork);
            }
        }
    }

    if root_manifest {
        let root_metadata = serde_json::to_vec(&json!({
            "website-index-document": index,
            "website-error-document": errordoc,
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
            vec![create_stub(stub_ref_size, obfuscated).await],
            encrypted,
            redundancy_level,
            batch_owner.clone(),
            batch_id.clone(),
            data_upload_chan,
            progress.clone(),
        )
        .await;
        if stub_reference.is_empty() {
            return vec![];
        }

        let root_fork = create_fork("/".to_string(), stub_reference, root_metadata).await;
        if root_fork.is_empty() {
            return vec![];
        }
        fork_bases.push(root_fork);
    }

    let mut serialized_forks = data_forks;
    serialized_forks.append(&mut fork_bases);
    let Some((serialized_forks, index_bytes)) = ordered_indexed_forks(serialized_forks) else {
        return vec![];
    };

    manifest_bytes_vec[index_bytes_start..index_bytes_start + index_bytes.len()]
        .copy_from_slice(&index_bytes);

    for mut fork in serialized_forks {
        manifest_bytes_vec.append(&mut fork);
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
    let has_edge = metadata.is_empty();
    encode_fork(path.as_bytes(), &reference, &metadata, has_edge).unwrap_or_default()
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
