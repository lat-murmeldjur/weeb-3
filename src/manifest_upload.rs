use crate::{
    erasure_coding::RedundancyLevel,
    manifest::{
        MANTARAY_PREFIX_MAX_BYTES, MANTARAY_VERSION_02, common_prefix_bytes, encode_fork,
        encode_fork_with_separator_path, ordered_indexed_forks, split_prefix_bytes,
    },
    upload::{ChunkUploadSender, UploadProgressSender},
    upload_data,
};
use rand::RngCore;
use serde_json::json;
#[cfg(test)]
use wasm_bindgen_test::wasm_bindgen_test;

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

fn serialize_manifest(encrypted: bool, reference: Vec<u8>, forks: Vec<Vec<u8>>) -> Vec<u8> {
    let reference_size = match reference.len() {
        0 if encrypted => 64,
        0 | 32 => 32,
        64 => 64,
        _ => return vec![],
    };
    let Some((forks, index)) = ordered_indexed_forks(forks) else {
        return vec![];
    };

    let mut manifest = Vec::new();
    manifest.extend_from_slice(&manifest_obfuscation_key(encrypted));
    manifest.extend_from_slice(&MANTARAY_VERSION_02);
    manifest.push(reference_size);
    manifest.extend_from_slice(&reference);
    if reference.is_empty() {
        manifest.resize(manifest.len() + reference_size as usize, 0);
    }
    manifest.extend_from_slice(&index);
    for fork in forks {
        manifest.extend_from_slice(&fork);
    }
    obfuscate_manifest(manifest)
}

#[cfg(test)]
#[wasm_bindgen_test]
fn plain_manifest_serialization_matches_mantaray_layout() {
    let slash = encode_fork(b"/", &[1; 32], &[], true).unwrap();
    let a = encode_fork(b"a", &[2; 32], &[], true).unwrap();
    let manifest = serialize_manifest(false, vec![], vec![a.clone(), slash.clone()]);

    assert_eq!(&manifest[..32], &[0; 32]);
    assert_eq!(&manifest[32..63], MANTARAY_VERSION_02);
    assert_eq!(manifest[63], 32);
    assert_eq!(&manifest[64..96], &[0; 32]);
    let mut index = [0; 32];
    for &byte in b"/a" {
        index[(byte / 8) as usize] |= 1 << (byte % 8);
    }
    assert_eq!(&manifest[96..128], index);
    assert_eq!(&manifest[128..], [slash, a].concat());
}

pub(crate) struct ManifestNode {
    pub(crate) data: Vec<u8>, // repurposed as address
    pub(crate) mime: String,
    pub(crate) filename: String,
    pub(crate) path: Vec<u8>,
}

struct ManifestUploadContext<'a> {
    encrypted: bool,
    redundancy_level: RedundancyLevel,
    index: &'a str,
    errordoc: &'a str,
    chunk_upload_chan: &'a ChunkUploadSender,
    progress: Option<&'a UploadProgressSender>,
}

impl ManifestUploadContext<'_> {
    async fn upload(&self, data: Vec<Vec<u8>>) -> Vec<u8> {
        upload_data(
            data,
            self.encrypted,
            self.redundancy_level,
            self.chunk_upload_chan,
            self.progress.cloned(),
        )
        .await
    }
}

pub async fn create_manifest(
    encrypted: bool,
    redundancy_level: RedundancyLevel,
    input_forks: Vec<ManifestNode>,
    data_forks: Vec<Vec<u8>>,
    reference: Vec<u8>,
    root_manifest: bool,
    index: String,
    errordoc: String,
    chunk_upload_chan: &ChunkUploadSender,
    progress: Option<UploadProgressSender>,
) -> Vec<u8> {
    let context = ManifestUploadContext {
        encrypted,
        redundancy_level,
        index: &index,
        errordoc: &errordoc,
        chunk_upload_chan,
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
    let mut prefix_offset = first_node_cutoff;
    let mut forks = input_forks;
    forks.sort_by(|a, b| a.path.cmp(&b.path));
    let fork_count = forks.len();
    if !matches!(reference.len(), 0 | 32 | 64) {
        return vec![];
    }
    let mut fork_bases = Vec::new();

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
            if group.len() == 1 {
                let Some(fork) = group.pop() else {
                    return vec![];
                };
                let first_capacity = if group_index == 0 && prefix_offset > 0 {
                    MANTARAY_PREFIX_MAX_BYTES - (prefix_offset % MANTARAY_PREFIX_MAX_BYTES)
                } else {
                    MANTARAY_PREFIX_MAX_BYTES
                };
                let Some(prefix_parts) = split_prefix_bytes(&fork.path, first_capacity) else {
                    return vec![];
                };
                if first_capacity < MANTARAY_PREFIX_MAX_BYTES && fork.path.len() > first_capacity {
                    prefix_offset = 0;
                }

                let mut current_data_reference = fork.data;
                let mut leaf_metadata = serde_json::to_vec(&json!({
                    "Content-Type": &fork.mime,
                    "Filename": &fork.filename,
                }))
                .unwrap();

                let leaf_manifest =
                    serialize_manifest(context.encrypted, current_data_reference, vec![]);
                if leaf_manifest.is_empty() {
                    return vec![];
                }

                current_data_reference = context.upload(vec![leaf_manifest]).await;
                if current_data_reference.is_empty() {
                    return vec![];
                }

                for i in (0..prefix_parts.len()).rev() {
                    let current_metadata = if i == prefix_parts.len() - 1 {
                        std::mem::take(&mut leaf_metadata)
                    } else {
                        vec![]
                    };
                    let Some(current_fork) = encode_fork(
                        &prefix_parts[i],
                        &current_data_reference,
                        &current_metadata,
                        current_metadata.is_empty(),
                    ) else {
                        return vec![];
                    };

                    if i > 0 {
                        let current_manifest =
                            serialize_manifest(context.encrypted, vec![], vec![current_fork]);
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
                let paths = group
                    .iter()
                    .map(|fork| fork.path.as_slice())
                    .collect::<Vec<_>>();
                let Some(common_prefix) = common_prefix_bytes(&paths) else {
                    return vec![];
                };
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
                    prefix_offset + common_prefix.len(),
                ))
                .await;
                if group_manifest.is_empty() {
                    return vec![];
                }

                prefix_offset = 0;

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

        let stub_ref_size = match (fork_count, context.encrypted) {
            (0, _) => 0,
            (_, true) => 64,
            (_, false) => 32,
        };

        let stub_reference = context
            .upload(vec![create_stub(stub_ref_size, context.encrypted)])
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

    let mut forks = data_forks;
    forks.append(&mut fork_bases);
    serialize_manifest(context.encrypted, reference, forks)
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
