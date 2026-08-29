use crate::{
    erasure_coding::RedundancyLevel,
    manifest::{
        MANTARAY_PREFIX_MAX_BYTES, MANTARAY_VERSION_02, common_prefix_bytes, encode_fork,
        encode_fork_with_separator_path, ordered_indexed_forks, split_prefix_bytes,
    },
    mpsc,
    upload::{DataUploadRequest, UploadProgressSender},
    upload_data,
};
use rand::RngCore;
use serde_json::json;

fn manifest_obfuscation_key(obfuscated: bool) -> [u8; 32] {
    let mut key = [0; 32];
    if obfuscated {
        rand::thread_rng().fill_bytes(&mut key);
    }
    key
}

fn obfuscate_manifest(mut manifest: Vec<u8>) -> Vec<u8> {
    if manifest[..32].iter().any(|byte| *byte != 0) {
        let (key, content) = manifest.split_at_mut(32);
        for (index, byte) in content.iter_mut().enumerate() {
            *byte ^= key[index % key.len()];
        }
    }
    manifest
}

pub(crate) struct ManifestNode {
    pub(crate) data: Vec<u8>, // repurposed as address
    pub(crate) mime: String,
    pub(crate) filename: String,
    pub(crate) path: Vec<u8>,
}

struct ManifestUploadContext<'a> {
    obfuscated: bool,
    encrypted: bool,
    redundancy_level: RedundancyLevel,
    index: &'a str,
    errordoc: &'a str,
    data_upload_chan: &'a mpsc::Sender<DataUploadRequest>,
    progress: Option<&'a UploadProgressSender>,
}

impl ManifestUploadContext<'_> {
    async fn upload(&self, data: Vec<Vec<u8>>) -> Vec<u8> {
        upload_data(
            data,
            self.encrypted,
            self.redundancy_level,
            self.data_upload_chan,
            self.progress.cloned(),
        )
        .await
    }
}

pub async fn create_manifest(
    obfuscated: bool,
    encrypted: bool,
    redundancy_level: RedundancyLevel,
    input_forks: Vec<ManifestNode>,
    data_forks: Vec<Vec<u8>>,
    reference: Vec<u8>,
    root_manifest: bool,
    index: String,
    errordoc: String,
    data_upload_chan: &mpsc::Sender<DataUploadRequest>,
    progress: Option<UploadProgressSender>,
) -> Vec<u8> {
    let context = ManifestUploadContext {
        obfuscated,
        encrypted,
        redundancy_level,
        index: &index,
        errordoc: &errordoc,
        data_upload_chan,
        progress: progress.as_ref(),
    };

    create_manifest_bytes(
        &context,
        input_forks,
        data_forks,
        reference,
        root_manifest,
        0,
    )
    .await
}

async fn create_manifest_bytes(
    context: &ManifestUploadContext<'_>,
    input_forks: Vec<ManifestNode>,
    data_forks: Vec<Vec<u8>>,
    reference: Vec<u8>,
    root_manifest: bool,
    first_node_cutoff: usize,
) -> Vec<u8> {
    let mut fncutoff = first_node_cutoff;

    let mut manifest_bytes_vec = Vec::new();

    let mut forks = input_forks;

    forks.sort_by(|a, b| a.path.cmp(&b.path));

    let flen = forks.len();

    manifest_bytes_vec.extend_from_slice(&manifest_obfuscation_key(context.obfuscated));
    manifest_bytes_vec.extend_from_slice(&MANTARAY_VERSION_02);

    let ref_length = match reference.len() {
        0 if context.encrypted => 64,
        0 | 32 => 32,
        64 => 64,
        _ => return vec![],
    };

    manifest_bytes_vec.push(ref_length);
    manifest_bytes_vec.extend_from_slice(&reference);

    if reference.is_empty() {
        manifest_bytes_vec.resize(manifest_bytes_vec.len() + ref_length as usize, 0);
    };

    let index_bytes_start = manifest_bytes_vec.len();

    manifest_bytes_vec.resize(manifest_bytes_vec.len() + 32, 0);

    let mut fork_bases: Vec<Vec<u8>> = vec![];

    if !forks.is_empty() {
        let mut fork_groups: Vec<(u8, Vec<ManifestNode>)> = vec![];

        for fork in forks {
            let Some(&leading_byte) = fork.path.first() else {
                return vec![];
            };

            // The initial path sort keeps equal leading bytes contiguous.
            if let Some((_, group)) = fork_groups
                .last_mut()
                .filter(|(byte, _)| *byte == leading_byte)
            {
                group.push(fork);
            } else {
                fork_groups.push((leading_byte, vec![fork]));
            }
        }

        for (group_index, (_, mut group)) in fork_groups.into_iter().enumerate() {
            let paths = group
                .iter()
                .map(|fork| fork.path.as_slice())
                .collect::<Vec<_>>();
            let Some(common_prefix) = common_prefix_bytes(&paths) else {
                return vec![];
            };
            if group.len() == 1 {
                let Some(fork) = group.pop() else {
                    return vec![];
                };
                let first_capacity = if group_index == 0 && fncutoff > 0 {
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

                let mut current_data_reference = fork.data;
                let mut value_final = serde_json::to_vec(&json!({
                    "Content-Type": &fork.mime,
                    "Filename": &fork.filename,
                }))
                .unwrap();

                let tip_mf = Box::pin(create_manifest_bytes(
                    context,
                    vec![],
                    vec![],
                    current_data_reference,
                    false,
                    0,
                ))
                .await;
                if tip_mf.is_empty() {
                    return vec![];
                }

                current_data_reference = context.upload(vec![tip_mf]).await;
                if current_data_reference.is_empty() {
                    return vec![];
                }

                for i in (0..vforks.len()).rev() {
                    let current_metadata = if i == vforks.len() - 1 {
                        std::mem::take(&mut value_final)
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
                            context,
                            vec![],
                            vec![current_fork],
                            vec![],
                            false,
                            0,
                        ))
                        .await;
                        if current_manifest.is_empty() {
                            return vec![];
                        }

                        current_data_reference = context.upload(vec![current_manifest]).await;
                        if current_data_reference.is_empty() {
                            return vec![];
                        }
                    } else {
                        fork_bases.push(current_fork);
                    }
                }
            } else {
                let Some(separator_path) = group.last().map(|fork| fork.path.clone()) else {
                    return vec![];
                };
                let mut exact_value = None;
                let mut descendants = Vec::with_capacity(group.len());
                for mut fork in group {
                    if fork.path.len() == common_prefix.len() {
                        // Bee's Add semantics make the last sorted duplicate win.
                        exact_value = Some(fork);
                    } else {
                        fork.path.drain(..common_prefix.len());
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
                    context,
                    descendants,
                    vec![],
                    group_reference,
                    false,
                    fncutoff + common_prefix.len(),
                ))
                .await;
                if group_manifest.is_empty() {
                    return vec![];
                }

                fncutoff = 0;

                let group_data_reference = context.upload(vec![group_manifest]).await;
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
            "website-index-document": context.index,
            "website-error-document": context.errordoc,
        }))
        .unwrap();

        let stub_ref_size = match (flen, context.encrypted) {
            (0, _) => 0,
            (_, true) => 64,
            (_, false) => 32,
        };

        let stub_reference = context
            .upload(vec![create_stub(stub_ref_size, context.obfuscated)])
            .await;
        if stub_reference.is_empty() {
            return vec![];
        }

        let root_fork = create_fork("/", stub_reference, root_metadata);
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

    obfuscate_manifest(manifest_bytes_vec)
}

pub fn create_fork(path: &str, reference: Vec<u8>, metadata: Vec<u8>) -> Vec<u8> {
    let has_edge = metadata.is_empty();
    encode_fork(path.as_bytes(), &reference, &metadata, has_edge).unwrap_or_default()
}

pub fn create_stub(stub_ref_size: u8, obfuscated: bool) -> Vec<u8> {
    let mut manifest_bytes_vec = Vec::new();
    manifest_bytes_vec.extend_from_slice(&manifest_obfuscation_key(obfuscated));
    manifest_bytes_vec.extend_from_slice(&MANTARAY_VERSION_02);

    manifest_bytes_vec.push(stub_ref_size);
    manifest_bytes_vec.resize(manifest_bytes_vec.len() + 32 + stub_ref_size as usize, 0);
    obfuscate_manifest(manifest_bytes_vec)
}
