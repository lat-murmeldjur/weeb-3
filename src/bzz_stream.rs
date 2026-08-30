use crate::{
    ChunkRetrieveSender, RetrieveCancelToken,
    erasure_coding::{CHUNK_SIZE, decode_span, encoded_reference_payload_len},
    manifest::{
        BzzManifestFork, MAX_PARALLEL_MANIFEST_FORKS, NODE_TYPE_EDGE, NODE_TYPE_WITH_METADATA,
        ParsedBzzManifest, ResolutionGuard, is_bzz_manifest_header, manifest_payload_size_allowed,
        manifest_wrapped_reference, parse_bzz_manifest,
    },
    retrieval::{
        DecodedJoinChunk, retrieve_data, retrieve_data_range_from_root,
        retrieve_data_range_from_root_cancellable, retrieve_decoded_data_root,
        retrieve_decoded_data_root_cancellable, seek_latest_feed_update,
    },
    retrieve_cancel_token_current,
    stream_conventions::{is_swarm_reference_hex, streaming_route_path},
};

use libp2p::futures::{StreamExt, stream};
use std::time::Duration;

const RANGE_RETRIEVE_RETRY_COUNT: usize = 2;
const RANGE_RETRIEVE_RETRY_WAIT_MS: u64 = 120;

#[derive(Debug)]
pub struct BzzResource {
    pub reference: Vec<u8>,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct BzzMetadata {
    pub data_reference: Vec<u8>,
    pub mime: String,
    pub size: u64,
    pub etag: String,
    pub path: String,
    pub target_count: usize,
}

#[derive(Debug)]
pub(crate) struct BzzTarget {
    pub(crate) data_reference: Vec<u8>,
    pub(crate) mime: String,
    pub(crate) path: String,
    pub(crate) raw_fallback: bool,
}

struct ManifestTargetResult {
    targets: Vec<BzzTarget>,
    fallback_index: Option<String>,
    explicit_index: Option<String>,
}

fn strip_known_bzz_prefix(resource: &str) -> &str {
    let resource = resource.trim();
    let resource = resource.split_once('?').map_or(resource, |(path, _)| path);
    let path = resource
        .split_once('#')
        .map_or(resource, |(_, route)| route)
        .trim_start_matches('/');
    let reference = path.split('/').next().unwrap_or_default();
    if is_swarm_reference_hex(reference) {
        return path;
    }
    path.strip_prefix("bzz/")
        .or_else(|| path.rsplit_once("/bzz/").map(|(_, resource)| resource))
        .unwrap_or(path)
}

pub fn parse_bzz_resource(resource: &str) -> Option<BzzResource> {
    let stripped = strip_known_bzz_prefix(resource);
    let mut parts = stripped.splitn(2, '/');
    let reference_hex = parts.next().unwrap_or_default();

    let reference = match hex::decode(reference_hex) {
        Ok(reference) if matches!(reference.len(), 32 | 64) => reference,
        _ => return None,
    };

    Some(BzzResource {
        reference,
        path: normalize_bzz_path(parts.next().unwrap_or_default()),
    })
}

pub fn bzz_reference_hex(resource: &str) -> Option<String> {
    parse_bzz_resource(resource).map(|resource| hex::encode(resource.reference))
}

pub fn normalize_bzz_path(path: &str) -> String {
    path.trim().trim_matches('/').to_string()
}

pub(crate) fn canonical_bzz_url(
    resource: &str,
    resolved_path: &str,
    empty_path: Option<&str>,
) -> Option<String> {
    let reference = bzz_reference_hex(resource)?;
    let requested_path = resource
        .split_once(&reference)
        .map(|(_, tail)| normalize_bzz_path(tail))
        .unwrap_or_default();
    let resolved_path = normalize_bzz_path(resolved_path);
    let path = if !requested_path.is_empty()
        && (resolved_path.is_empty() || requested_path == resolved_path)
    {
        requested_path
    } else {
        resolved_path
    };
    let path = if path.is_empty() {
        empty_path.unwrap_or_default()
    } else {
        &path
    };
    let route = match crate::network_profile::active_profile().mode {
        crate::network_profile::NetworkMode::Mainnet => "bzz",
        crate::network_profile::NetworkMode::Testnet => "testnet/bzz",
    };
    let prefix = streaming_route_path(route);
    if path.is_empty() || path.starts_with("unknown") || path == "not found" {
        Some(format!("{prefix}/{reference}"))
    } else {
        Some(format!("{prefix}/{reference}/{path}"))
    }
}

fn bzz_paths_match(left: &str, right: &str) -> bool {
    left.trim().trim_matches('/') == right.trim().trim_matches('/')
}

fn child_path(path_prefix: &[u8], fork_prefix: &[u8]) -> Vec<u8> {
    let mut bequeath = Vec::with_capacity(path_prefix.len() + fork_prefix.len());
    bequeath.extend_from_slice(path_prefix);
    bequeath.extend_from_slice(fork_prefix);
    bequeath
}

fn normalize_bzz_path_bytes(path: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = path.len();
    while start < end && (path[start].is_ascii_whitespace() || path[start] == b'/') {
        start += 1;
    }
    while end > start && (path[end - 1].is_ascii_whitespace() || path[end - 1] == b'/') {
        end -= 1;
    }
    &path[start..end]
}

fn display_bzz_path(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

async fn reference_span(
    reference: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<u64> {
    retrieve_decoded_data_root(reference, chunk_retrieve_chan)
        .await
        .map(|root| root.span)
}

fn embedded_join_root(data: &[u8], encrypted: bool) -> Option<DecodedJoinChunk> {
    let (level, span) = decode_span(data)?;
    let payload_len = if span <= CHUNK_SIZE as u64 {
        usize::try_from(span).ok()?
    } else {
        encoded_reference_payload_len(span, level, encrypted)?
    };
    let end = 8usize.checked_add(payload_len)?;
    if data.len() != end {
        return None;
    }

    Some(DecodedJoinChunk {
        level,
        span,
        payload: bytes::Bytes::copy_from_slice(&data[8..end]),
    })
}

pub(crate) async fn retrieve_embedded_data(
    data: &[u8],
    encrypted: bool,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    let (span, payload) =
        retrieve_embedded_payload_with_span(data, encrypted, chunk_retrieve_chan).await?;
    let capacity = usize::try_from(span.checked_add(8)?).ok()?;
    let mut joined = Vec::with_capacity(capacity);
    joined.extend_from_slice(&span.to_le_bytes());
    joined.extend_from_slice(&payload);
    (joined.len() == capacity).then_some(joined)
}

async fn retrieve_embedded_payload_with_span(
    data: &[u8],
    encrypted: bool,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<(u64, Vec<u8>)> {
    let root = embedded_join_root(data, encrypted)?;
    let span = root.span;
    if !manifest_payload_size_allowed(span) {
        return None;
    }
    let payload = if span == 0 {
        Vec::new()
    } else {
        retrieve_data_range_from_root(
            root,
            0,
            span.checked_sub(1)?,
            encrypted,
            chunk_retrieve_chan,
        )
        .await?
    };

    (u64::try_from(payload.len()).ok()? == span).then_some((span, payload))
}

#[derive(Clone, Debug)]
pub(crate) struct FeedPayloadRoot {
    pub(crate) index: u64,
    root: DecodedJoinChunk,
    encrypted: bool,
}

impl FeedPayloadRoot {
    pub(crate) fn span(&self) -> u64 {
        self.root.span
    }
}

pub(crate) fn decode_feed_payload_root(index: u64, update: Vec<u8>) -> Option<FeedPayloadRoot> {
    [false, true].into_iter().find_map(|encrypted| {
        let root = embedded_join_root(&update, encrypted)?;
        manifest_payload_size_allowed(root.span).then_some(FeedPayloadRoot {
            index,
            root,
            encrypted,
        })
    })
}

pub(crate) async fn retrieve_feed_payload(
    payload: &FeedPayloadRoot,
    maximum_payload_bytes: usize,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    let maximum_span = u64::try_from(maximum_payload_bytes).ok()?;
    let span = payload.root.span;
    if span > maximum_span {
        return None;
    }
    if span == 0 {
        return Some(Vec::new());
    }
    let bytes = retrieve_data_range_from_root(
        payload.root.clone(),
        0,
        span.checked_sub(1)?,
        payload.encrypted,
        chunk_retrieve_chan,
    )
    .await?;
    (u64::try_from(bytes.len()).ok()? == span).then_some(bytes)
}

pub(crate) async fn retrieve_feed_payload_tail(
    payload: &FeedPayloadRoot,
    maximum_tail_bytes: usize,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    let maximum_tail_bytes = u64::try_from(maximum_tail_bytes)
        .ok()?
        .min(CHUNK_SIZE as u64);
    let span = payload.root.span;
    if span == 0 || maximum_tail_bytes == 0 {
        return None;
    }
    let start = span.saturating_sub(maximum_tail_bytes);
    let end_inclusive = span.checked_sub(1)?;
    let expected_len = end_inclusive.checked_sub(start)?.checked_add(1)?;
    let tail = retrieve_data_range_from_root(
        payload.root.clone(),
        start,
        end_inclusive,
        payload.encrypted,
        chunk_retrieve_chan,
    )
    .await?;
    (u64::try_from(tail.len()).ok()? == expected_len).then_some(tail)
}

async fn retrieve_data_head(
    reference: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    let encrypted = reference.len() == 64;
    if !encrypted && reference.len() != 32 {
        return None;
    }

    let root = retrieve_decoded_data_root(reference, chunk_retrieve_chan).await?;
    let span = root.span;
    let head_len = span.min(CHUNK_SIZE as u64);
    let payload = if head_len == 0 {
        Vec::new()
    } else {
        retrieve_data_range_from_root(
            root,
            0,
            head_len.checked_sub(1)?,
            encrypted,
            chunk_retrieve_chan,
        )
        .await?
    };

    let mut head = Vec::with_capacity(8 + payload.len());
    head.extend_from_slice(&span.to_le_bytes());
    head.extend_from_slice(&payload);
    Some(head)
}

async fn get_manifest_if_manifest(
    reference: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<ParsedBzzManifest> {
    let head = retrieve_data_head(reference, chunk_retrieve_chan).await?;
    if !is_bzz_manifest_header(&head) {
        return None;
    }
    let span = u64::from_le_bytes(head.get(..8)?.try_into().ok()?);
    if !manifest_payload_size_allowed(span) {
        return None;
    }
    if head.len() == usize::try_from(span.checked_add(8)?).ok()? {
        return parse_bzz_manifest(head);
    }

    let data = retrieve_data(reference, chunk_retrieve_chan).await;
    parse_bzz_manifest(data)
}

pub(crate) async fn collect_reference_targets(
    path_prefix: Vec<u8>,
    reference: Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> (Vec<BzzTarget>, String) {
    if reference.len() != 32 && reference.len() != 64 {
        return (vec![], String::new());
    }
    let Some(guard) = guard.descend_reference(&reference) else {
        return (vec![], String::new());
    };

    if let Some(manifest) = get_manifest_if_manifest(&reference, chunk_retrieve_chan).await {
        return Box::pin(collect_manifest_targets(
            path_prefix,
            manifest,
            chunk_retrieve_chan,
            guard,
        ))
        .await;
    }

    if !guard.reserve_target() {
        return (vec![], String::new());
    }

    (
        vec![BzzTarget {
            data_reference: reference,
            mime: "application/octet-stream".to_string(),
            path: display_bzz_path(&path_prefix),
            raw_fallback: true,
        }],
        String::new(),
    )
}

async fn collect_manifest_fork_targets(
    path_prefix: Vec<u8>,
    ref_size: usize,
    fork: BzzManifestFork,
    chunk_retrieve_chan: ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> ManifestTargetResult {
    let mut result = ManifestTargetResult {
        targets: vec![],
        fallback_index: None,
        explicit_index: None,
    };
    if !guard.reserve_fork() {
        return result;
    }

    let has_edge = fork.fork_type & NODE_TYPE_EDGE == NODE_TYPE_EDGE;
    if fork.fork_type & NODE_TYPE_WITH_METADATA == NODE_TYPE_WITH_METADATA {
        let Some(metadata) = fork.metadata else {
            return result;
        };

        if let (Some(owner), Some(topic)) = (
            metadata
                .get("swarm-feed-owner")
                .and_then(serde_json::Value::as_str)
                .map(|owner| owner.to_string()),
            metadata
                .get("swarm-feed-topic")
                .and_then(serde_json::Value::as_str)
                .map(|topic| topic.to_string()),
        ) && let Some(feed_guard) = guard.descend_feed(&owner, &topic)
        {
            let feed_data_soc = seek_latest_feed_update(owner, topic, &chunk_retrieve_chan).await;

            if feed_data_soc.len() >= 8
                && let Some(feed_data_content) =
                    retrieve_embedded_data(&feed_data_soc, ref_size == 64, &chunk_retrieve_chan)
                        .await
                && let Some(feed_manifest) = parse_bzz_manifest(feed_data_content)
            {
                let (mut feed_targets, feed_index) = Box::pin(collect_manifest_targets(
                    Vec::new(),
                    feed_manifest,
                    &chunk_retrieve_chan,
                    feed_guard,
                ))
                .await;
                result.targets.append(&mut feed_targets);
                result.fallback_index = Some(feed_index);
            }
        }

        if let Some(explicit_index) = metadata
            .get("website-index-document")
            .and_then(serde_json::Value::as_str)
        {
            result.explicit_index = Some(explicit_index.to_string());
        }

        let bequeath = child_path(&path_prefix, &fork.prefix);
        let Some(mime) = metadata
            .get("Content-Type")
            .and_then(serde_json::Value::as_str)
            .map(|mime| mime.to_string())
        else {
            let (mut child_targets, child_index) = Box::pin(collect_reference_targets(
                bequeath,
                fork.reference,
                &chunk_retrieve_chan,
                guard,
            ))
            .await;
            result.targets.append(&mut child_targets);
            if result.fallback_index.is_none() {
                result.fallback_index = Some(child_index);
            }
            return result;
        };

        let mut data_reference = fork.reference.clone();
        if let Some(root_manifest) =
            get_manifest_if_manifest(&fork.reference, &chunk_retrieve_chan).await
            && let Some(wrapped_reference) = manifest_wrapped_reference(root_manifest)
        {
            data_reference = wrapped_reference;
        }

        if guard.reserve_target() {
            result.targets.push(BzzTarget {
                data_reference,
                mime,
                path: display_bzz_path(&bequeath),
                raw_fallback: false,
            });
        }

        if has_edge {
            let (mut child_targets, child_index) = Box::pin(collect_reference_targets(
                bequeath,
                fork.reference,
                &chunk_retrieve_chan,
                guard,
            ))
            .await;
            result.targets.append(&mut child_targets);
            if result.fallback_index.is_none() {
                result.fallback_index = Some(child_index);
            }
        }
        return result;
    }

    let bequeath = child_path(&path_prefix, &fork.prefix);
    let (mut child_targets, child_index) = Box::pin(collect_reference_targets(
        bequeath,
        fork.reference,
        &chunk_retrieve_chan,
        guard,
    ))
    .await;
    result.targets.append(&mut child_targets);
    result.fallback_index = Some(child_index);
    result
}

async fn collect_manifest_targets(
    path_prefix: Vec<u8>,
    parsed: ParsedBzzManifest,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> (Vec<BzzTarget>, String) {
    let mut targets = vec![];
    let mut index = parsed.explicit_index.unwrap_or_default();
    let mut fallback_index = None;

    let loads = parsed.forks.into_iter().map(|fork| {
        collect_manifest_fork_targets(
            path_prefix.clone(),
            parsed.ref_size,
            fork,
            chunk_retrieve_chan.clone(),
            guard.clone(),
        )
    });

    let mut loads = stream::iter(loads).buffered(MAX_PARALLEL_MANIFEST_FORKS);
    while let Some(mut load) = loads.next().await {
        if fallback_index.is_none() {
            fallback_index = load.fallback_index.take().filter(|index| !index.is_empty());
        }

        if let Some(explicit_index) = load.explicit_index.take() {
            index = explicit_index;
        }

        targets.append(&mut load.targets);
    }

    if index.is_empty() {
        index = fallback_index.unwrap_or_default();
    }

    (targets, index)
}

fn select_bzz_target(
    mut targets: Vec<BzzTarget>,
    requested_path: &str,
    index: &str,
) -> Option<BzzTarget> {
    for candidate in [requested_path, index, "index.html"] {
        if candidate.is_empty() {
            continue;
        }
        if let Some(position) = targets
            .iter()
            .position(|target| bzz_paths_match(&target.path, candidate))
        {
            return Some(targets.swap_remove(position));
        }
    }
    targets.into_iter().next()
}

fn target_metadata(target: BzzTarget, size: u64, target_count: usize) -> BzzMetadata {
    BzzMetadata {
        etag: format!("\"{}\"", hex::encode(&target.data_reference)),
        data_reference: target.data_reference,
        mime: target.mime,
        size,
        path: normalize_bzz_path(&target.path),
        target_count,
    }
}

fn bzz_path_bytes_match(left: &[u8], right: &[u8]) -> bool {
    normalize_bzz_path_bytes(left) == normalize_bzz_path_bytes(right)
}

fn bzz_path_starts_with(path: &[u8], prefix: &[u8]) -> bool {
    let path = normalize_bzz_path_bytes(path);
    let prefix = normalize_bzz_path_bytes(prefix);
    prefix.is_empty() || path.starts_with(prefix)
}

async fn metadata_fork_target(
    fork: &BzzManifestFork,
    path: &[u8],
    mime: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: &ResolutionGuard,
) -> Option<BzzTarget> {
    let mut data_reference = fork.reference.clone();
    if let Some(root_manifest) =
        get_manifest_if_manifest(&fork.reference, chunk_retrieve_chan).await
        && let Some(wrapped_reference) = manifest_wrapped_reference(root_manifest)
    {
        data_reference = wrapped_reference;
    }
    guard.reserve_target().then(|| BzzTarget {
        data_reference,
        mime,
        path: display_bzz_path(path),
        raw_fallback: false,
    })
}

async fn lazy_reference_target(
    path_prefix: Vec<u8>,
    reference: Vec<u8>,
    requested_path: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> Option<BzzTarget> {
    if reference.len() != 32 && reference.len() != 64 {
        return None;
    }
    let guard = guard.descend_reference(&reference)?;

    if let Some(manifest) = get_manifest_if_manifest(&reference, chunk_retrieve_chan).await {
        return Box::pin(lazy_manifest_target(
            path_prefix,
            manifest,
            requested_path,
            chunk_retrieve_chan,
            guard,
        ))
        .await;
    }

    if requested_path.is_empty() || bzz_path_bytes_match(&path_prefix, requested_path) {
        if !guard.reserve_target() {
            return None;
        }
        return Some(BzzTarget {
            data_reference: reference,
            mime: "application/octet-stream".to_string(),
            path: display_bzz_path(&path_prefix),
            raw_fallback: true,
        });
    }

    None
}

async fn lazy_manifest_target(
    path_prefix: Vec<u8>,
    parsed: ParsedBzzManifest,
    requested_path: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> Option<BzzTarget> {
    let mut requested_paths: Vec<Vec<u8>> = Vec::new();

    if !requested_path.is_empty() {
        requested_paths.push(normalize_bzz_path_bytes(requested_path).to_vec());
    } else if let Some(index) = parsed.explicit_index.as_deref()
        && !index.is_empty()
    {
        requested_paths.push(normalize_bzz_path(index).into_bytes());
    }

    if requested_path.is_empty() {
        requested_paths.push(b"index.html".to_vec());
    }

    requested_paths.dedup();

    for desired_path in requested_paths {
        if let Some(target) = Box::pin(lazy_manifest_target_for_path(
            &path_prefix,
            &parsed.forks,
            &desired_path,
            chunk_retrieve_chan,
            guard.clone(),
        ))
        .await
        {
            return Some(target);
        }
    }

    if requested_path.is_empty() {
        return Box::pin(lazy_first_manifest_target(
            &path_prefix,
            &parsed.forks,
            chunk_retrieve_chan,
            guard,
        ))
        .await;
    }

    None
}

async fn lazy_manifest_target_for_path(
    path_prefix: &[u8],
    forks: &[BzzManifestFork],
    requested_path: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> Option<BzzTarget> {
    for fork in forks {
        if !guard.reserve_fork() {
            return None;
        }
        let bequeath = child_path(path_prefix, &fork.prefix);
        let has_edge = fork.fork_type & NODE_TYPE_EDGE == NODE_TYPE_EDGE;

        if !bzz_path_starts_with(requested_path, &bequeath) {
            continue;
        }

        if fork.fork_type & NODE_TYPE_WITH_METADATA == NODE_TYPE_WITH_METADATA {
            let Some(metadata) = fork.metadata.as_ref() else {
                continue;
            };

            if metadata.get("swarm-feed-owner").is_some()
                || metadata.get("swarm-feed-topic").is_some()
            {
                continue;
            }

            if let Some(mime) = metadata
                .get("Content-Type")
                .and_then(serde_json::Value::as_str)
                .map(|mime| mime.to_string())
            {
                let exact_path = bzz_path_bytes_match(&bequeath, requested_path);
                if !exact_path && !has_edge {
                    continue;
                }

                if exact_path {
                    return metadata_fork_target(
                        fork,
                        &bequeath,
                        mime,
                        chunk_retrieve_chan,
                        &guard,
                    )
                    .await;
                }
            }
        }

        if let Some(target) = Box::pin(lazy_reference_target(
            bequeath,
            fork.reference.clone(),
            requested_path,
            chunk_retrieve_chan,
            guard.clone(),
        ))
        .await
        {
            return Some(target);
        }
    }

    None
}

async fn lazy_first_manifest_target(
    path_prefix: &[u8],
    forks: &[BzzManifestFork],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> Option<BzzTarget> {
    for fork in forks {
        if !guard.reserve_fork() {
            return None;
        }
        let bequeath = child_path(path_prefix, &fork.prefix);

        if fork.fork_type & NODE_TYPE_WITH_METADATA == NODE_TYPE_WITH_METADATA {
            let Some(metadata) = fork.metadata.as_ref() else {
                continue;
            };

            if metadata.get("swarm-feed-owner").is_some()
                || metadata.get("swarm-feed-topic").is_some()
            {
                continue;
            }

            if let Some(mime) = metadata
                .get("Content-Type")
                .and_then(serde_json::Value::as_str)
                .map(|mime| mime.to_string())
            {
                return metadata_fork_target(fork, &bequeath, mime, chunk_retrieve_chan, &guard)
                    .await;
            }
        }

        if let Some(target) = Box::pin(lazy_reference_target(
            bequeath,
            fork.reference.clone(),
            b"",
            chunk_retrieve_chan,
            guard.clone(),
        ))
        .await
        {
            return Some(target);
        }
    }

    None
}

pub async fn resolve_bzz(
    resource: &str,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<BzzMetadata> {
    let parsed = parse_bzz_resource(resource)?;

    if let Some(target) = lazy_reference_target(
        Vec::new(),
        parsed.reference.clone(),
        parsed.path.as_bytes(),
        chunk_retrieve_chan,
        ResolutionGuard::new(),
    )
    .await
    {
        let size = reference_span(&target.data_reference, chunk_retrieve_chan).await?;

        return Some(target_metadata(target, size, 1));
    }

    let (targets, index) = collect_reference_targets(
        Vec::new(),
        parsed.reference,
        chunk_retrieve_chan,
        ResolutionGuard::new(),
    )
    .await;
    let target_count = targets.len();
    let target = select_bzz_target(targets, &parsed.path, &index)?;
    let size = reference_span(&target.data_reference, chunk_retrieve_chan).await?;

    Some(target_metadata(target, size, target_count))
}

async fn latest_feed_manifest(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<ParsedBzzManifest> {
    let feed_data_soc = seek_latest_feed_update(owner, topic, chunk_retrieve_chan).await;
    if feed_data_soc.len() < 8 {
        return None;
    }

    for encrypted in [false, true] {
        if let Some(content) =
            retrieve_embedded_data(&feed_data_soc, encrypted, chunk_retrieve_chan).await
            && let Some(manifest) = parse_bzz_manifest(content)
        {
            return Some(manifest);
        }
    }

    None
}

pub async fn acquire_latest_feed(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<(Vec<u8>, BzzMetadata)> {
    let guard = ResolutionGuard::new().descend_feed(&owner, &topic)?;
    let feed_manifest = latest_feed_manifest(owner, topic, chunk_retrieve_chan).await?;
    let target = Box::pin(lazy_manifest_target(
        Vec::new(),
        feed_manifest,
        b"",
        chunk_retrieve_chan,
        guard,
    ))
    .await?;
    let size = reference_span(&target.data_reference, chunk_retrieve_chan).await?;
    let metadata = target_metadata(target, size, 1);

    if size == 0 {
        return Some((vec![], metadata));
    }

    acquire_resolved_range(metadata, 0, size - 1, chunk_retrieve_chan).await
}

pub async fn acquire_resolved_range(
    metadata: BzzMetadata,
    start: u64,
    end_inclusive: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<(Vec<u8>, BzzMetadata)> {
    acquire_resolved_range_cancellable(metadata, start, end_inclusive, chunk_retrieve_chan, None)
        .await
}

pub async fn acquire_resolved_range_cancellable(
    metadata: BzzMetadata,
    start: u64,
    end_inclusive: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
) -> Option<(Vec<u8>, BzzMetadata)> {
    if start > end_inclusive || start >= metadata.size {
        return None;
    }

    let end_inclusive = end_inclusive.min(metadata.size.saturating_sub(1));
    let expected_len = end_inclusive
        .checked_sub(start)?
        .checked_add(1)
        .and_then(|len| usize::try_from(len).ok())?;

    for attempt in 0..=RANGE_RETRIEVE_RETRY_COUNT {
        if !retrieve_cancel_token_current(&cancel) {
            return None;
        }

        let data = if let Some(root) = retrieve_decoded_data_root_cancellable(
            &metadata.data_reference,
            chunk_retrieve_chan,
            cancel.clone(),
        )
        .await
        {
            retrieve_data_range_from_root_cancellable(
                root,
                start,
                end_inclusive,
                metadata.data_reference.len() == 64,
                chunk_retrieve_chan,
                cancel.clone(),
            )
            .await
            .unwrap_or_default()
        } else {
            Vec::new()
        };

        if !retrieve_cancel_token_current(&cancel) {
            return None;
        }

        if data.len() == expected_len {
            return Some((data, metadata));
        }

        if attempt < RANGE_RETRIEVE_RETRY_COUNT {
            if !retrieve_cancel_token_current(&cancel) {
                return None;
            }

            async_std::task::sleep(Duration::from_millis(
                RANGE_RETRIEVE_RETRY_WAIT_MS * (attempt as u64 + 1),
            ))
            .await;
        }
    }

    None
}
