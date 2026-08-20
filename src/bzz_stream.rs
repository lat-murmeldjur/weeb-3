use crate::{
    ChunkRetrieveSender, RetrieveCancelToken, RetrieveGenerationMap,
    erasure_coding::{CHUNK_SIZE, decode_span, encoded_reference_payload_len},
    feed::{FeedProbe, overscan_sequence_feed_candidate},
    manifest::{
        BzzManifestFork, MAX_PARALLEL_MANIFEST_FORKS, ParsedBzzManifest, ResolutionGuard,
        is_bzz_manifest_header, manifest_payload_size_allowed, manifest_wrapped_reference,
        parse_bzz_manifest,
    },
    mpsc,
    retrieval::{
        DecodedJoinChunk, retrieve_data, retrieve_data_range_from_root,
        retrieve_data_range_from_root_cancellable, retrieve_data_range_from_root_conservative,
        retrieve_decoded_data_root, retrieve_decoded_data_root_cancellable,
        retrieve_feed_update_at_index, retrieve_feed_update_at_index_bounded,
        retrieve_feed_update_at_index_retained_status, retrieve_feed_update_at_index_status,
        seek_latest_feed_update, seek_latest_feed_update_indexed_from,
        seek_latest_feed_update_indexed_observing_positive,
        seek_latest_feed_update_indexed_wide_bounded,
    },
    retrieve_cancel_token_current,
};

use libp2p::futures::{StreamExt, stream};
use std::{future::Future, rc::Rc, time::Duration};

const RANGE_RETRIEVE_RETRY_COUNT: usize = 2;
const RANGE_RETRIEVE_RETRY_WAIT_MS: u64 = 120;
const OBSERVED_FEED_PAYLOAD_DECODES: usize = 3;

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
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

fn strip_known_bzz_prefix(resource: &str) -> String {
    let mut resource0 = resource.trim().to_string();

    if let Some(query_pos) = resource0.find('?') {
        resource0.truncate(query_pos);
    }

    let markers = [
        "/weeb-3/#/testnet/bzz/",
        "/weeb-3/#/mainnet/bzz/",
        "/weeb-3/#/bzz/",
        "/weeb-3/testnet/bzz/",
        "/weeb-3/mainnet/bzz/",
        "/weeb-3/bzz/",
        "/#/testnet/bzz/",
        "/#/mainnet/bzz/",
        "/#/bzz/",
        "/testnet/bzz/",
        "/mainnet/bzz/",
        "/bzz/",
        "weeb-3/#/testnet/bzz/",
        "weeb-3/#/mainnet/bzz/",
        "weeb-3/#/bzz/",
        "weeb-3/testnet/bzz/",
        "weeb-3/mainnet/bzz/",
        "weeb-3/bzz/",
        "#/testnet/bzz/",
        "#/mainnet/bzz/",
        "#/bzz/",
        "testnet/bzz/",
        "mainnet/bzz/",
        "bzz/",
    ];

    for marker in markers {
        if let Some(idx) = resource0.find(marker) {
            let mut stripped = resource0[idx + marker.len()..]
                .trim_start_matches('/')
                .to_string();
            if let Some(hash_pos) = stripped.find('#') {
                stripped.truncate(hash_pos);
            }
            return stripped;
        }
    }

    if let Some(hash_pos) = resource0.find('#') {
        resource0 = resource0[hash_pos + 1..].to_string();
    }

    resource0.trim_start_matches('/').to_string()
}

fn is_reference_hex(segment: &str) -> bool {
    (segment.len() == 64 || segment.len() == 128)
        && segment.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
}

pub fn parse_bzz_resource(resource: &str) -> Option<BzzResource> {
    let stripped = strip_known_bzz_prefix(resource);
    let mut parts = stripped.splitn(2, '/');
    let reference_hex = parts.next().unwrap_or_default();

    if !is_reference_hex(reference_hex) {
        return None;
    }

    let reference = match hex::decode(reference_hex) {
        Ok(reference) if reference.len() == 32 || reference.len() == 64 => reference,
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

fn bzz_paths_match(left: &str, right: &str) -> bool {
    normalize_bzz_path(left) == normalize_bzz_path(right)
}

fn child_path(path_prefix_heritance: &[u8], fork_prefix: &[u8]) -> Vec<u8> {
    let mut bequeath = Vec::with_capacity(path_prefix_heritance.len() + fork_prefix.len());
    bequeath.extend_from_slice(path_prefix_heritance);
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
    reference: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<u64> {
    Some(
        retrieve_decoded_data_root(reference, chunk_retrieve_chan)
            .await?
            .span,
    )
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
        payload: Rc::from(&data[8..end]),
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
    retrieve_embedded_payload_with_span_bounded(data, encrypted, chunk_retrieve_chan, None).await
}

async fn retrieve_embedded_payload_with_span_bounded(
    data: &[u8],
    encrypted: bool,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    maximum_span: Option<u64>,
) -> Option<(u64, Vec<u8>)> {
    let root = embedded_join_root(data, encrypted)?;
    let span = root.span;
    if !manifest_payload_size_allowed(span) || maximum_span.is_some_and(|maximum| span > maximum) {
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
pub(crate) struct RawFeedPayload {
    pub index: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct DeferredRawFeedPayload {
    pub(crate) index: u64,
    update: Vec<u8>,
    span: u64,
}

impl DeferredRawFeedPayload {
    pub(crate) fn payload_span(&self) -> u64 {
        self.span
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.update.len()
    }
}

fn deferred_raw_feed_payload_root(
    deferred: &DeferredRawFeedPayload,
) -> Option<(DecodedJoinChunk, bool)> {
    [false, true].into_iter().find_map(|encrypted| {
        let root = embedded_join_root(&deferred.update, encrypted)?;
        (root.span == deferred.span).then_some((root, encrypted))
    })
}

fn conservative_deferred_payload_range(
    payload_span: u64,
    start: u64,
    maximum_len: u64,
) -> Option<(u64, u64)> {
    if start >= payload_span
        || maximum_len == 0
        || maximum_len > crate::retrieval::CONSERVATIVE_DEFERRED_RANGE_BYTES
    {
        return None;
    }
    let len = payload_span.checked_sub(start)?.min(maximum_len);
    let end_inclusive = start.checked_add(len)?.checked_sub(1)?;
    Some((start, end_inclusive))
}

#[derive(Clone, Debug)]
pub(crate) enum RetainedRawFeedPayloadProbe {
    Found(RawFeedPayload),
    Deferred(DeferredRawFeedPayload),
    Missing,
    Transient,
}

#[derive(Clone, Debug)]
pub(crate) struct StartupRawFeedPayload {
    pub(crate) playable: RawFeedPayload,
    pub(crate) observed_deferred: Option<DeferredRawFeedPayload>,
}

fn defer_large_raw_feed_update(index: u64, update: Vec<u8>) -> Option<DeferredRawFeedPayload> {
    let span = [false, true]
        .into_iter()
        .find_map(|encrypted| embedded_join_root(&update, encrypted).map(|root| root.span))?;
    (span > CHUNK_SIZE as u64 && manifest_payload_size_allowed(span)).then_some(
        DeferredRawFeedPayload {
            index,
            update,
            span,
        },
    )
}

async fn raw_feed_payload_from_update(
    update: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    raw_feed_payload_from_update_bounded(update, chunk_retrieve_chan, None).await
}

async fn raw_feed_payload_from_update_bounded(
    update: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    maximum_span: Option<u64>,
) -> Option<Vec<u8>> {
    for encrypted in [false, true] {
        let Some((_, payload)) = retrieve_embedded_payload_with_span_bounded(
            update,
            encrypted,
            chunk_retrieve_chan,
            maximum_span,
        )
        .await
        else {
            continue;
        };
        return Some(payload);
    }
    None
}

fn decode_observed_feed_updates(
    payloads: mpsc::Sender<RawFeedPayload>,
    maximum_index: Option<u64>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> mpsc::Sender<(u64, Vec<u8>)> {
    let (updates_out, updates_in) = mpsc::bounded::<(u64, Vec<u8>)>(16);
    let chunk_retrieve_chan = chunk_retrieve_chan.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let mut decoded = updates_in
            .map(move |(index, update)| {
                let chunk_retrieve_chan = chunk_retrieve_chan.clone();
                async move {
                    if maximum_index.is_some_and(|maximum| index > maximum)
                        || decode_span(&update).is_some_and(|(_, span)| span > CHUNK_SIZE as u64)
                    {
                        return None;
                    }
                    raw_feed_payload_from_update(&update, &chunk_retrieve_chan)
                        .await
                        .map(|bytes| RawFeedPayload { index, bytes })
                }
            })
            .buffer_unordered(OBSERVED_FEED_PAYLOAD_DECODES);
        while let Some(Some(payload)) = decoded.next().await {
            if payloads.send(payload).await.is_err() {
                return;
            }
        }
    });
    updates_out
}

pub(crate) async fn acquire_raw_feed_payload_at_index(
    owner: String,
    topic: String,
    index: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<RawFeedPayload> {
    let update = retrieve_feed_update_at_index(owner, topic, index, chunk_retrieve_chan).await?;
    let bytes = raw_feed_payload_from_update(&update, chunk_retrieve_chan).await?;
    Some(RawFeedPayload { index, bytes })
}

pub(crate) async fn acquire_raw_feed_payload_at_index_retained_status(
    owner: String,
    topic: String,
    index: u64,
    maximum_payload_bytes: usize,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> RetainedRawFeedPayloadProbe {
    let update = match retrieve_feed_update_at_index_retained_status(
        owner,
        topic,
        index,
        chunk_retrieve_chan,
    )
    .await
    {
        FeedProbe::Found(update) => update,
        FeedProbe::Missing => return RetainedRawFeedPayloadProbe::Missing,
        FeedProbe::Transient => return RetainedRawFeedPayloadProbe::Transient,
    };
    let Some(maximum_span) = u64::try_from(maximum_payload_bytes).ok() else {
        return RetainedRawFeedPayloadProbe::Transient;
    };
    let Some(span) = [false, true]
        .into_iter()
        .find_map(|encrypted| embedded_join_root(&update, encrypted).map(|root| root.span))
    else {
        return RetainedRawFeedPayloadProbe::Transient;
    };
    if span > maximum_span {
        return RetainedRawFeedPayloadProbe::Deferred(DeferredRawFeedPayload {
            index,
            update,
            span,
        });
    }
    let Some(bytes) =
        raw_feed_payload_from_update_bounded(&update, chunk_retrieve_chan, Some(maximum_span))
            .await
    else {
        return RetainedRawFeedPayloadProbe::Transient;
    };
    RetainedRawFeedPayloadProbe::Found(RawFeedPayload { index, bytes })
}

pub(crate) async fn acquire_deferred_raw_feed_payload(
    deferred: DeferredRawFeedPayload,
    maximum_payload_bytes: usize,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<RawFeedPayload> {
    let maximum_span = u64::try_from(maximum_payload_bytes).ok()?;
    if deferred.span > maximum_span {
        return None;
    }
    let bytes = raw_feed_payload_from_update_bounded(
        &deferred.update,
        chunk_retrieve_chan,
        Some(maximum_span),
    )
    .await?;
    (u64::try_from(bytes.len()).ok()? == deferred.span).then_some(RawFeedPayload {
        index: deferred.index,
        bytes,
    })
}

pub(crate) async fn probe_deferred_raw_feed_payload_tail_conservative(
    deferred: &DeferredRawFeedPayload,
    maximum_tail_bytes: usize,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    let maximum_tail_bytes = u64::try_from(maximum_tail_bytes)
        .ok()?
        .min(crate::retrieval::CONSERVATIVE_DEFERRED_RANGE_BYTES);
    let (root, encrypted) = deferred_raw_feed_payload_root(deferred)?;
    if root.span == 0 || !manifest_payload_size_allowed(root.span) {
        return None;
    }
    let tail_start = root.span.saturating_sub(maximum_tail_bytes);
    let (start, end_inclusive) =
        conservative_deferred_payload_range(root.span, tail_start, maximum_tail_bytes)?;
    let expected_len = end_inclusive.checked_sub(start)?.checked_add(1)?;
    let tail = retrieve_data_range_from_root_conservative(
        root,
        start,
        end_inclusive,
        encrypted,
        chunk_retrieve_chan,
    )
    .await?;
    (u64::try_from(tail.len()).ok()? == expected_len).then_some(tail)
}

pub(crate) async fn acquire_deferred_raw_feed_payload_conservative<F, Fut>(
    deferred: DeferredRawFeedPayload,
    maximum_payload_bytes: usize,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    mut admit_range: F,
) -> Option<RawFeedPayload>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let maximum_span = u64::try_from(maximum_payload_bytes).ok()?;
    let (root, encrypted) = deferred_raw_feed_payload_root(&deferred)?;
    if root.span == 0 || root.span > maximum_span || !manifest_payload_size_allowed(root.span) {
        return None;
    }
    let span = root.span;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(usize::try_from(span).ok()?).ok()?;
    let mut start = 0u64;
    while let Some((range_start, range_end)) = conservative_deferred_payload_range(
        span,
        start,
        crate::retrieval::CONSERVATIVE_DEFERRED_RANGE_BYTES,
    ) {
        if !admit_range().await {
            return None;
        }
        let range = retrieve_data_range_from_root_conservative(
            root.clone(),
            range_start,
            range_end,
            encrypted,
            chunk_retrieve_chan,
        )
        .await?;
        let expected_len = range_end.checked_sub(range_start)?.checked_add(1)?;
        if u64::try_from(range.len()).ok()? != expected_len {
            return None;
        }
        bytes.extend_from_slice(&range);
        start = range_end.checked_add(1)?;
    }
    (start == span && u64::try_from(bytes.len()).ok()? == span).then_some(RawFeedPayload {
        index: deferred.index,
        bytes,
    })
}

pub(crate) async fn acquire_raw_feed_payload_at_index_bounded(
    owner: String,
    topic: String,
    index: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<RawFeedPayload> {
    let update =
        retrieve_feed_update_at_index_bounded(owner, topic, index, chunk_retrieve_chan).await?;
    let bytes = raw_feed_payload_from_update(&update, chunk_retrieve_chan).await?;
    Some(RawFeedPayload { index, bytes })
}

pub(crate) async fn acquire_latest_raw_feed_payload_startup(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    early_payloads: Option<mpsc::Sender<RawFeedPayload>>,
    early_payload_max_index: Option<u64>,
) -> Option<RawFeedPayload> {
    acquire_latest_raw_feed_payload_startup_observing_deferred(
        owner,
        topic,
        chunk_retrieve_chan,
        early_payloads,
        early_payload_max_index,
    )
    .await
    .map(|resolved| resolved.playable)
}

pub(crate) async fn acquire_latest_raw_feed_payload_startup_observing_deferred(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    early_payloads: Option<mpsc::Sender<RawFeedPayload>>,
    early_payload_max_index: Option<u64>,
) -> Option<StartupRawFeedPayload> {
    let early_updates = early_payloads.map(|payloads| {
        decode_observed_feed_updates(payloads, early_payload_max_index, chunk_retrieve_chan)
    });
    let (index, update) = match early_updates {
        Some(early_updates) => {
            seek_latest_feed_update_indexed_observing_positive(
                owner,
                topic,
                chunk_retrieve_chan,
                Some(early_updates),
            )
            .await?
        }
        None => {
            let (index, update) = seek_latest_feed_update_indexed_wide_bounded(
                owner.clone(),
                topic.clone(),
                chunk_retrieve_chan,
            )
            .await?;
            if decode_span(&update).is_some_and(|(_, span)| span > CHUNK_SIZE as u64)
                && let Some(previous_index) = index.checked_sub(1)
                && let Some(previous) = retrieve_feed_update_at_index_bounded(
                    owner,
                    topic,
                    previous_index,
                    chunk_retrieve_chan,
                )
                .await
                && let Some(bytes) =
                    raw_feed_payload_from_update(&previous, chunk_retrieve_chan).await
            {
                return Some(StartupRawFeedPayload {
                    playable: RawFeedPayload {
                        index: previous_index,
                        bytes,
                    },
                    observed_deferred: defer_large_raw_feed_update(index, update),
                });
            }
            (index, update)
        }
    };
    let bytes = raw_feed_payload_from_update(&update, chunk_retrieve_chan).await?;
    Some(StartupRawFeedPayload {
        playable: RawFeedPayload { index, bytes },
        observed_deferred: None,
    })
}

pub(crate) async fn acquire_latest_raw_feed_payload_bounded_from<AdmitWave, AdmitFuture>(
    owner: String,
    topic: String,
    initial: RawFeedPayload,
    force_coarse: bool,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    admit_wave: AdmitWave,
    observed_payloads: Option<mpsc::Sender<RawFeedPayload>>,
) -> Option<(RawFeedPayload, bool)>
where
    AdmitWave: Fn(usize) -> AdmitFuture,
    AdmitFuture: std::future::Future<Output = bool>,
{
    let observed_updates = observed_payloads
        .map(|payloads| decode_observed_feed_updates(payloads, None, chunk_retrieve_chan));
    let ((index, update), verified) = overscan_sequence_feed_candidate(
        (initial.index, Vec::new()),
        force_coarse,
        |index| {
            retrieve_feed_update_at_index_status(
                owner.clone(),
                topic.clone(),
                index,
                chunk_retrieve_chan,
            )
        },
        admit_wave,
        |index, update| {
            if let Some(observed_updates) = observed_updates.as_ref() {
                let _ = observed_updates.try_send((index, update.clone()));
            }
        },
    )
    .await;
    if index == initial.index {
        return Some((initial, verified));
    }
    let bytes = raw_feed_payload_from_update(&update, chunk_retrieve_chan).await?;
    Some((RawFeedPayload { index, bytes }, verified))
}

pub(crate) async fn acquire_latest_raw_feed_payload_from(
    owner: String,
    topic: String,
    initial: RawFeedPayload,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<RawFeedPayload> {
    let (index, update) =
        seek_latest_feed_update_indexed_from(owner, topic, initial.index, chunk_retrieve_chan)
            .await?;
    if index == initial.index {
        return Some(initial);
    }
    if index < initial.index {
        return None;
    }
    let bytes = raw_feed_payload_from_update(&update, chunk_retrieve_chan).await?;
    Some(RawFeedPayload { index, bytes })
}

async fn retrieve_data_head(
    reference: &Vec<u8>,
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
    reference: &Vec<u8>,
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

async fn get_root_manifest_if_manifest(
    reference: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<ParsedBzzManifest> {
    get_manifest_if_manifest(reference, chunk_retrieve_chan).await
}

pub(crate) async fn collect_reference_targets(
    path_prefix_heritance: Vec<u8>,
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
            path_prefix_heritance,
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
            path: display_bzz_path(&path_prefix_heritance),
            raw_fallback: true,
        }],
        String::new(),
    )
}

async fn collect_manifest_fork_targets(
    path_prefix_heritance: Vec<u8>,
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

    let has_edge = fork.fork_type & 4 == 4;
    if fork.fork_type & 16 == 16 {
        let Some(metadata) = fork.metadata else {
            return result;
        };

        if let (Some(owner), Some(topic)) = (
            metadata
                .get("swarm-feed-owner")
                .and_then(|str0f0| str0f0.as_str())
                .map(|owner| owner.to_string()),
            metadata
                .get("swarm-feed-topic")
                .and_then(|str0f1| str0f1.as_str())
                .map(|topic| topic.to_string()),
        ) {
            if let Some(feed_guard) = guard.descend_feed(&owner, &topic) {
                let feed_data_soc =
                    seek_latest_feed_update(owner, topic, &chunk_retrieve_chan, 8).await;

                if feed_data_soc.len() >= 8 {
                    if let Some(feed_data_content) =
                        retrieve_embedded_data(&feed_data_soc, ref_size == 64, &chunk_retrieve_chan)
                            .await
                    {
                        if let Some(feed_manifest) = parse_bzz_manifest(feed_data_content) {
                            let (mut feed_targets, feed_index) =
                                Box::pin(collect_manifest_targets(
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
                }
            }
        }

        if let Some(explicit_index) = metadata
            .get("website-index-document")
            .and_then(|str0i| str0i.as_str())
        {
            result.explicit_index = Some(explicit_index.to_string());
        }

        let bequeath = child_path(&path_prefix_heritance, &fork.prefix);
        let Some(mime) = metadata
            .get("Content-Type")
            .and_then(|str0| str0.as_str())
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
            get_root_manifest_if_manifest(&fork.reference, &chunk_retrieve_chan).await
        {
            if let Some(wrapped_reference) = manifest_wrapped_reference(root_manifest) {
                data_reference = wrapped_reference;
            }
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

    let bequeath = child_path(&path_prefix_heritance, &fork.prefix);
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
    path_prefix_heritance: Vec<u8>,
    parsed: ParsedBzzManifest,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> (Vec<BzzTarget>, String) {
    let mut targets = vec![];
    let mut index = parsed.explicit_index.unwrap_or_default();
    let mut fallback_index = None;

    let loads = parsed.forks.into_iter().map(|fork| {
        collect_manifest_fork_targets(
            path_prefix_heritance.clone(),
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
        if let Some(fallback_index) = fallback_index {
            index = fallback_index;
        }
    }

    (targets, index)
}

fn select_bzz_target(
    targets: Vec<BzzTarget>,
    requested_path: &str,
    index: &str,
) -> Option<BzzTarget> {
    if !requested_path.is_empty() {
        if let Some(target) = targets
            .iter()
            .find(|target| bzz_paths_match(&target.path, requested_path))
        {
            return Some(target.clone());
        }
    }

    if !index.is_empty() {
        if let Some(target) = targets
            .iter()
            .find(|target| bzz_paths_match(&target.path, index))
        {
            return Some(target.clone());
        }
    }

    if targets.len() == 1 {
        return targets.into_iter().next();
    }

    targets
        .iter()
        .find(|target| bzz_paths_match(&target.path, "index.html"))
        .cloned()
        .or_else(|| targets.into_iter().next())
}

fn bzz_path_bytes_match(left: &[u8], right: &[u8]) -> bool {
    normalize_bzz_path_bytes(left) == normalize_bzz_path_bytes(right)
}

fn bzz_path_starts_with(path: &[u8], prefix: &[u8]) -> bool {
    let path = normalize_bzz_path_bytes(path);
    let prefix = normalize_bzz_path_bytes(prefix);
    prefix.is_empty() || path.starts_with(prefix)
}

async fn lazy_reference_target(
    path_prefix_heritance: Vec<u8>,
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
            path_prefix_heritance,
            manifest,
            requested_path,
            chunk_retrieve_chan,
            guard,
        ))
        .await;
    }

    if requested_path.is_empty() || bzz_path_bytes_match(&path_prefix_heritance, requested_path) {
        if !guard.reserve_target() {
            return None;
        }
        return Some(BzzTarget {
            data_reference: reference,
            mime: "application/octet-stream".to_string(),
            path: display_bzz_path(&path_prefix_heritance),
            raw_fallback: true,
        });
    }

    None
}

async fn lazy_manifest_target(
    path_prefix_heritance: Vec<u8>,
    parsed: ParsedBzzManifest,
    requested_path: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> Option<BzzTarget> {
    let mut requested_paths: Vec<Vec<u8>> = Vec::new();

    if !requested_path.is_empty() {
        requested_paths.push(normalize_bzz_path_bytes(requested_path).to_vec());
    } else if let Some(index) = parsed.explicit_index.clone() {
        if !index.is_empty() {
            requested_paths.push(normalize_bzz_path(&index).into_bytes());
        }
    }

    if requested_path.is_empty() {
        requested_paths.push(b"index.html".to_vec());
    }

    requested_paths.dedup();

    for desired_path in requested_paths {
        if let Some(target) = Box::pin(lazy_manifest_target_for_path(
            path_prefix_heritance.clone(),
            parsed.ref_size,
            parsed.forks.clone(),
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
            path_prefix_heritance,
            parsed.ref_size,
            parsed.forks,
            chunk_retrieve_chan,
            guard,
        ))
        .await;
    }

    None
}

async fn lazy_manifest_target_for_path(
    path_prefix_heritance: Vec<u8>,
    ref_size: usize,
    forks: Vec<BzzManifestFork>,
    requested_path: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> Option<BzzTarget> {
    for fork in forks {
        if !guard.reserve_fork() {
            return None;
        }
        let bequeath = child_path(&path_prefix_heritance, &fork.prefix);
        let has_edge = fork.fork_type & 4 == 4;

        if !bzz_path_starts_with(requested_path, &bequeath) {
            continue;
        }

        if fork.fork_type & 16 == 16 {
            let Some(metadata) = fork.metadata.clone() else {
                continue;
            };

            if metadata.get("swarm-feed-owner").is_some()
                || metadata.get("swarm-feed-topic").is_some()
            {
                continue;
            }

            if let Some(mime) = metadata
                .get("Content-Type")
                .and_then(|str0| str0.as_str())
                .map(|mime| mime.to_string())
            {
                if !bzz_path_bytes_match(&bequeath, requested_path) && !has_edge {
                    continue;
                }

                if bzz_path_bytes_match(&bequeath, requested_path) {
                    let mut data_reference = fork.reference.clone();
                    if let Some(root_manifest) =
                        get_root_manifest_if_manifest(&fork.reference, chunk_retrieve_chan).await
                    {
                        if let Some(wrapped_reference) = manifest_wrapped_reference(root_manifest) {
                            data_reference = wrapped_reference;
                        }
                    }

                    if !guard.reserve_target() {
                        return None;
                    }
                    return Some(BzzTarget {
                        data_reference,
                        mime,
                        path: display_bzz_path(&bequeath),
                        raw_fallback: false,
                    });
                }
            }
        }

        if let Some(target) = Box::pin(lazy_reference_target(
            bequeath,
            fork.reference,
            requested_path,
            chunk_retrieve_chan,
            guard.clone(),
        ))
        .await
        {
            return Some(target);
        }

        if ref_size == 0 {
            continue;
        }
    }

    None
}

async fn lazy_first_manifest_target(
    path_prefix_heritance: Vec<u8>,
    ref_size: usize,
    forks: Vec<BzzManifestFork>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    guard: ResolutionGuard,
) -> Option<BzzTarget> {
    for fork in forks {
        if !guard.reserve_fork() {
            return None;
        }
        let bequeath = child_path(&path_prefix_heritance, &fork.prefix);

        if fork.fork_type & 16 == 16 {
            let Some(metadata) = fork.metadata.clone() else {
                continue;
            };

            if metadata.get("swarm-feed-owner").is_some()
                || metadata.get("swarm-feed-topic").is_some()
            {
                continue;
            }

            if let Some(mime) = metadata
                .get("Content-Type")
                .and_then(|str0| str0.as_str())
                .map(|mime| mime.to_string())
            {
                let mut data_reference = fork.reference.clone();
                if let Some(root_manifest) =
                    get_root_manifest_if_manifest(&fork.reference, chunk_retrieve_chan).await
                {
                    if let Some(wrapped_reference) = manifest_wrapped_reference(root_manifest) {
                        data_reference = wrapped_reference;
                    }
                }

                if !guard.reserve_target() {
                    return None;
                }
                return Some(BzzTarget {
                    data_reference,
                    mime,
                    path: display_bzz_path(&bequeath),
                    raw_fallback: false,
                });
            }
        }

        if let Some(target) = Box::pin(lazy_reference_target(
            bequeath,
            fork.reference,
            b"",
            chunk_retrieve_chan,
            guard.clone(),
        ))
        .await
        {
            return Some(target);
        }

        if ref_size == 0 {
            continue;
        }
    }

    None
}

pub async fn resolve_bzz(
    resource: &str,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<BzzMetadata> {
    let parsed = parse_bzz_resource(resource)?;
    let requested_path = normalize_bzz_path(&parsed.path).into_bytes();

    if let Some(target) = lazy_reference_target(
        Vec::new(),
        parsed.reference.clone(),
        &requested_path,
        chunk_retrieve_chan,
        ResolutionGuard::new(),
    )
    .await
    {
        let size = reference_span(&target.data_reference, chunk_retrieve_chan).await?;

        return Some(BzzMetadata {
            etag: format!("\"{}\"", hex::encode(&target.data_reference)),
            data_reference: target.data_reference,
            mime: target.mime,
            size,
            path: normalize_bzz_path(&target.path),
            target_count: 1,
        });
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

    Some(BzzMetadata {
        etag: format!("\"{}\"", hex::encode(&target.data_reference)),
        data_reference: target.data_reference,
        mime: target.mime,
        size,
        path: normalize_bzz_path(&target.path),
        target_count,
    })
}

async fn latest_feed_manifest(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<ParsedBzzManifest> {
    let feed_data_soc = seek_latest_feed_update(owner, topic, chunk_retrieve_chan, 8).await;
    if feed_data_soc.len() < 8 {
        return None;
    }

    for encrypted in [false, true] {
        if let Some(content) =
            retrieve_embedded_data(&feed_data_soc, encrypted, chunk_retrieve_chan).await
        {
            if let Some(manifest) = parse_bzz_manifest(content) {
                return Some(manifest);
            }
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
    let metadata = BzzMetadata {
        etag: format!("\"{}\"", hex::encode(&target.data_reference)),
        data_reference: target.data_reference,
        mime: target.mime,
        size,
        path: normalize_bzz_path(&target.path),
        target_count: 1,
    };

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
    acquire_resolved_range_cancellable(
        metadata,
        start,
        end_inclusive,
        chunk_retrieve_chan,
        None,
        None,
    )
    .await
}

pub async fn acquire_resolved_range_cancellable(
    metadata: BzzMetadata,
    start: u64,
    end_inclusive: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
    cancel_generations: Option<RetrieveGenerationMap>,
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
        if !range_cancel_token_current(&cancel_generations, &cancel).await {
            return None;
        }

        let data = retrieve_data_range_cancellable(
            &metadata.data_reference,
            start + 8,
            end_inclusive + 8,
            chunk_retrieve_chan,
            cancel.clone(),
            cancel_generations.clone(),
        )
        .await;

        if data.len() == expected_len {
            return Some((data, metadata));
        }

        if attempt < RANGE_RETRIEVE_RETRY_COUNT {
            if !range_cancel_token_current(&cancel_generations, &cancel).await {
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

async fn range_cancel_token_current(
    cancel_generations: &Option<RetrieveGenerationMap>,
    cancel: &Option<RetrieveCancelToken>,
) -> bool {
    if let (Some(generations), Some(_)) = (cancel_generations, cancel) {
        return retrieve_cancel_token_current(generations, cancel).await;
    }

    true
}

pub async fn retrieve_data_range_cancellable(
    data_address: &Vec<u8>,
    start: u64,
    end_inclusive: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
    cancel_generations: Option<RetrieveGenerationMap>,
) -> Vec<u8> {
    if start > end_inclusive {
        return vec![];
    }

    if !range_cancel_token_current(&cancel_generations, &cancel).await {
        return vec![];
    }

    let Some(root) = retrieve_decoded_data_root_cancellable(
        data_address,
        chunk_retrieve_chan,
        cancel_generations.clone(),
        cancel.clone(),
    )
    .await
    else {
        return vec![];
    };
    if !range_cancel_token_current(&cancel_generations, &cancel).await {
        return vec![];
    }

    let Some(total_len) = root.span.checked_add(8) else {
        return vec![];
    };
    if start >= total_len {
        return vec![];
    }

    let end_inclusive = end_inclusive.min(total_len - 1);
    let Some(output_len) = end_inclusive
        .checked_sub(start)
        .and_then(|len| len.checked_add(1))
        .and_then(|len| usize::try_from(len).ok())
    else {
        return vec![];
    };
    let span = root.span.to_le_bytes();
    if end_inclusive < 8 {
        return span[start as usize..=end_inclusive as usize].to_vec();
    }

    let payload_start = start.max(8) - 8;
    let payload_end = end_inclusive - 8;
    let encrypted = data_address.len() == 64;
    let Some(payload) = retrieve_data_range_from_root_cancellable(
        root,
        payload_start,
        payload_end,
        encrypted,
        chunk_retrieve_chan,
        cancel_generations.clone(),
        cancel.clone(),
    )
    .await
    else {
        return vec![];
    };

    if !range_cancel_token_current(&cancel_generations, &cancel).await {
        return vec![];
    }

    if start >= 8 {
        return (payload.len() == output_len)
            .then_some(payload)
            .unwrap_or_default();
    }

    let mut output = Vec::with_capacity(output_len);
    output.extend_from_slice(&span[start as usize..]);
    output.extend_from_slice(&payload);

    (output.len() == output_len)
        .then_some(output)
        .unwrap_or_default()
}
