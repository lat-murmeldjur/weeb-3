use crate::{ChunkRetrieveSender, mpsc};

use crate::{
    //
    JsValue,
    //
    get_data,
    //
    seek_latest_feed_update,
};

use libp2p::futures::future::join_all;
use serde_json::Value;

const DEBUG_MANIFEST_LOGS: bool = false;

macro_rules! manifest_debug {
    ($($arg:tt)*) => {
        if DEBUG_MANIFEST_LOGS {
            web_sys::console::log_1(&JsValue::from(format!($($arg)*)));
        }
    };
}

pub struct Fork {
    //    metadata: Value,
    pub data: Vec<u8>,
    pub mime: String,
    // pub filename: String,
    pub path: String,
}

struct ManifestFork {
    fork_type: u8,
    prefix: String,
    reference: Vec<u8>,
    metadata: Option<Value>,
}

struct ManifestForkResult {
    parts: Vec<Fork>,
    feed_index: Option<String>,
    explicit_index: Option<String>,
}

fn child_path(path_prefix_heritance: &str, fork_prefix: &str) -> String {
    let mut bequeath: String = String::new();
    bequeath.push_str(path_prefix_heritance);
    bequeath.push_str(fork_prefix);
    bequeath
}

async fn load_manifest_fork(
    path_prefix_heritance: String,
    manifest_encrypted: bool,
    ref_size: u8,
    fork: ManifestFork,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> ManifestForkResult {
    let mut result = ManifestForkResult {
        parts: vec![],
        feed_index: None,
        explicit_index: None,
    };

    let ref_data = get_data(fork.reference, data_retrieve_chan).await;

    if fork.fork_type & 16 == 16 {
        manifest_debug!("fork_type: metadata",);

        let v1 = match fork.metadata {
            Some(v1) => v1,
            None => return result,
        };

        let owner = v1
            .get("swarm-feed-owner")
            .and_then(|str0f0| str0f0.as_str())
            .map(|owner| owner.to_string());
        let topic = v1
            .get("swarm-feed-topic")
            .and_then(|str0f1| str0f1.as_str())
            .map(|topic| topic.to_string());

        if let (Some(owner), Some(topic)) = (owner, topic) {
            let feed_data_soc = seek_latest_feed_update(owner, topic, chunk_retrieve_chan, 8).await;

            if feed_data_soc.len() >= 8 {
                let mut feed_data_content = vec![];

                let soc_wrapped_span =
                    u64::from_le_bytes(feed_data_soc[0..8].try_into().unwrap_or([0; 8]));

                if soc_wrapped_span <= 4096 {
                    feed_data_content = feed_data_soc.to_vec();
                } else {
                    let lens = (feed_data_soc.len() - 8) / ref_size as usize;

                    feed_data_content.append(&mut feed_data_soc[0..8].to_vec());

                    let mut feed_leaf_refs = Vec::with_capacity(lens);
                    for i in 0..lens {
                        let ref_start = 8 + i * ref_size as usize;
                        let ref_end = 8 + (i + 1) * ref_size as usize;

                        feed_leaf_refs.push(feed_data_soc[ref_start..ref_end].to_vec());
                    }

                    let feed_leaf_loads = feed_leaf_refs.into_iter().map(|addr| {
                        let data_retrieve_chan = data_retrieve_chan.clone();
                        async move { get_data(addr, &data_retrieve_chan).await }
                    });

                    for leaf in join_all(feed_leaf_loads).await {
                        feed_data_content.extend_from_slice(&leaf[8..]);
                    }
                }

                manifest_debug!("dispatch interpret manifest for wrapped content in feed head soc",);

                let (mut appendix_0, nondiscard) = Box::pin(interpret_manifest(
                    "".to_string(),
                    &feed_data_content,
                    data_retrieve_chan,
                    chunk_retrieve_chan,
                ))
                .await;

                result.feed_index = Some(nondiscard);
                result.parts.append(&mut appendix_0);
            }
        }

        if let Some(str0i) = v1
            .get("website-index-document")
            .and_then(|str0i| str0i.as_str())
        {
            result.explicit_index = Some(str0i.to_string());
        }

        let mime_0 = match v1.get("Content-Type").and_then(|str0| str0.as_str()) {
            Some(str1) => str1.to_string(),
            _ => {
                let bequeath = child_path(&path_prefix_heritance, &fork.prefix);

                manifest_debug!(
                    "dispatch interpret manifest for with metadata fork reference with no content type",
                );

                let (mut appendix_0, _discard) = Box::pin(interpret_manifest(
                    bequeath,
                    &ref_data,
                    data_retrieve_chan,
                    chunk_retrieve_chan,
                ))
                .await;
                result.parts.append(&mut appendix_0);
                return result;
            }
        };

        if ref_data.len() > 71 {
            manifest_debug!(
                "inline interpret manifest for with metadata fork reference with content type",
            );

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
                        ref_data0
                            .push(ref_data[j] ^ ref_data_obfuscation_key[j - 40 - i * ref_creylen]);
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

                manifest_debug!(
                    "metadata_fork_reference_with_content_type_data_address {}",
                    hex::encode(&actual_data_address)
                );

                let actual_data = get_data(actual_data_address, data_retrieve_chan).await;

                result.parts.push(Fork {
                    data: actual_data.to_vec(),
                    mime: mime_0,
                    // filename: filename_0,
                    path: child_path(&path_prefix_heritance, &fork.prefix),
                });
            }
        }
    }

    if fork.fork_type & 16 == 0 {
        manifest_debug!("fork_type: no metadata",);

        let bequeath = child_path(&path_prefix_heritance, &fork.prefix);
        manifest_debug!("dispatch interpret manifest for fork with no metadata",);
        let (mut appendix_0, _discard) = Box::pin(interpret_manifest(
            bequeath,
            &ref_data,
            data_retrieve_chan,
            chunk_retrieve_chan,
        ))
        .await;
        result.parts.append(&mut appendix_0);
    }

    result
}

pub async fn interpret_manifest(
    path_prefix_heritance: String,
    cd0: &Vec<u8>,
    data_retrieve_chan: &mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
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
    manifest_debug!("obfuscation_key: {}", enc_obfuscation_key);

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
    manifest_debug!("mf_version: {}", enc_mf_version);

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
    manifest_debug!("ref_size: {}", ref_size);

    let ref_delimiter = (72 + ref_size) as usize;
    let index_delimiter = (ref_delimiter + 32) as usize;

    if ref_size > 0 {
        let manifest_reference = &cd[72..ref_delimiter];
        manifest_debug!("manifest_reference: {}", hex::encode(manifest_reference));
    }

    let index_bytes = &cd[ref_delimiter..index_delimiter];
    manifest_debug!("forks_index_bytes: {}", hex::encode(index_bytes));

    // Parse all fork descriptors first, then load their references concurrently.

    let mut forks = vec![];
    let mut fork_start_current = index_delimiter;

    while cd.len() > fork_start_current {
        manifest_debug!("fork_start/len: {}  / {}", fork_start_current, cd.len(),);

        let fork_start = fork_start_current;
        let fork_type = cd[fork_start_current];
        manifest_debug!("fork_type: {}", hex::encode(&[fork_type]));

        let fork_prefix_length = cd[fork_start_current + 1];
        manifest_debug!("fork_prefix_length: {}", hex::encode(&[fork_prefix_length]));

        let fork_prefix = &cd[fork_start + 2..fork_start + 2 + (fork_prefix_length as usize)];
        let string_fork_prefix = String::from_utf8(fork_prefix.to_vec()).unwrap_or("".to_string());
        manifest_debug!("string_fork_prefix: {}", string_fork_prefix);

        let fork_prefix_delimiter = fork_start + 32;
        let fork_reference_delimiter = fork_prefix_delimiter + (ref_size as usize);
        let fork_reference = &cd[fork_prefix_delimiter..fork_reference_delimiter];
        manifest_debug!("fork_reference____: {}", hex::encode(fork_reference));

        let metadata = if fork_type & 16 == 16 {
            let fork_metadata_bytesize: [u8; 2] = cd
                [fork_reference_delimiter..fork_reference_delimiter + 2]
                .try_into()
                .unwrap();

            let calc_metadata_bytesize = u16::from_be_bytes(fork_metadata_bytesize) as usize;

            let fork_metadata_delimiter = fork_reference_delimiter + 2 + calc_metadata_bytesize;
            fork_start_current = fork_metadata_delimiter;

            let fork_metadata = &cd[fork_reference_delimiter + 2..fork_metadata_delimiter];
            let enc_fork_metadata = hex::encode(fork_metadata);
            manifest_debug!("fork_metadata: {}", enc_fork_metadata);

            let v1: Value = serde_json::from_slice(fork_metadata).unwrap_or("nil".into());
            manifest_debug!("metadata json: {:#?} ", v1);
            Some(v1)
        } else {
            fork_start_current = fork_start + 32 + (ref_size as usize);
            None
        };

        forks.push(ManifestFork {
            fork_type,
            prefix: string_fork_prefix,
            reference: fork_reference.to_vec(),
            metadata,
        });
    }

    let loads = forks.into_iter().map(|fork| {
        let path_prefix_heritance = path_prefix_heritance.clone();
        async move {
            load_manifest_fork(
                path_prefix_heritance,
                manifest_encrypted,
                ref_size,
                fork,
                data_retrieve_chan,
                chunk_retrieve_chan,
            )
            .await
        }
    });

    let mut parts = vec![];
    let mut feed_index = None;

    for mut load in join_all(loads).await {
        if feed_index.is_none() {
            feed_index = load.feed_index.take();
        }

        if let Some(explicit_index) = load.explicit_index.take() {
            ind = explicit_index;
            ind_set = true;
        }

        parts.append(&mut load.parts);
    }

    if !ind_set {
        if let Some(index) = feed_index {
            ind = index;
        }
    }

    return (parts, ind);
}
