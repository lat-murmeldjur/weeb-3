use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use futures::{StreamExt, future::join, stream};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Element, HtmlMediaElement};

use super::{
    HLS_LIVE_SYNC_SEGMENTS, HlsPlaylist, HlsStart, MAX_STREAM_FEED_PAYLOAD_BYTES, hls_payload_mime,
    is_hex_reference, player,
};
use crate::{
    ChunkRetrieveRequest, Weeb3,
    bzz_stream::{
        FeedPayloadRoot, decode_feed_payload_root, retrieve_feed_payload,
        retrieve_feed_payload_tail,
    },
    feed::FeedProbe,
    get_feed_address,
    interface::{service_worker_controls_bzz_requests, service_worker_scope_protocol_error},
    mpsc, normalize_feed_topic,
    retrieval::{DecodedJoinChunk, retrieve_data_range_from_root, retrieve_decoded_data_root},
    retrieval_conventions::RetrieveAdmission,
    stream::{
        FetchResponse, begin_result_view_request, clear_completed_media_ranges,
        completed_media_range_bytes, media_cache_max_bytes, replace_stream_result_view,
        result_view_request_is_current, set_auxiliary_media_cache_bytes,
    },
    stream_conventions::{
        STREAMING_ROUTE_BASE, decode_component, if_none_match_matches, route_markers,
        streaming_route_path,
    },
};

const RANGE_CACHE_HARD_MAX_BYTES: u64 = 96 * 1024 * 1024;
const BEGINNING_DISCOVERY_WIDTH: u64 = 8;
const BEGINNING_PREFIX_SEGMENTS: usize = HLS_LIVE_SYNC_SEGMENTS * 2;
const BEGINNING_PAYLOAD_BYTES: usize = 64 * 1024;
const BEGINNING_WAVE_TIMEOUT: Duration = Duration::from_millis(1_500);
const FEED_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(3_000);
const EDGE_COLD_WAVE_TIMEOUT: Duration = Duration::from_millis(4_000);
const EDGE_WAVE_TIMEOUT: Duration = Duration::from_millis(1_500);
const EDGE_REFINEMENT_WIDTH: usize = 32;
#[rustfmt::skip]
const EDGE_ANCHORS: [u64; 49] = [
    0, 32, 64, 96, 128, 160, 192, 224,
    256, 288, 320, 352, 384, 416, 448, 480,
    512, 544, 546, 1_092, 1_638, 2_184, 2_730, 3_276,
    3_822, 4_369, 4_915, 5_461, 6_007, 6_553, 7_099, 7_645, 8_192,
    16_383, 32_767, 65_535, 131_071, 262_143, 524_287, 1_048_575, 2_097_151,
    4_194_303, 8_388_607, 16_777_215, 33_554_431, 67_108_863, 268_435_455, 1_073_741_823, u64::MAX,
];
const FEED_PROBE_ATTEMPTS: usize = 2;
const EDGE_PROBE_ATTEMPTS: usize = 2;
const FEED_TAIL_PROBE_BYTES: usize = 4 * 1024;
const FEED_POLL_INTERVAL: Duration = Duration::from_millis(400);
const INITIAL_DISCOVERY_RETRY_DELAY: Duration = Duration::from_millis(100);
const HISTORY_STRIDE: u64 = 10;
const HISTORY_WINDOW_BYTES: usize = 64 * 1024;
const HISTORY_MAX_PROBES: usize = 4_096;
const HISTORY_MAX_REPAIRS: usize = 4_096;
const HISTORY_PARALLEL: usize = 64;

thread_local! {
    static RANGE_CACHE: RefCell<RangeCache> = RefCell::new(RangeCache::default());
    static FEED: RefCell<Option<FeedSession>> = const { RefCell::new(None) };
    static NEXT_FEED_ID: Cell<u64> = const { Cell::new(0) };
    static PREREQUISITE_CLOCK: Cell<u64> = const { Cell::new(0) };
    static BEGINNING_MEDIA_READY: Cell<bool> = const { Cell::new(false) };
}

#[derive(Default)]
struct RangeCache {
    ranges: HashMap<(String, u64, u64), Arc<[u8]>>,
    order: VecDeque<(String, u64, u64)>,
    bytes: u64,
}

impl RangeCache {
    fn get(&self, reference: &str, start: u64, end: u64) -> Option<Arc<[u8]>> {
        let key = (reference.to_string(), start, end);
        self.ranges.get(&key).cloned()
    }

    fn insert(&mut self, reference: String, start: u64, end: u64, bytes: Arc<[u8]>) {
        let key = (reference, start, end);
        if let Some(previous) = self.ranges.insert(key.clone(), bytes.clone()) {
            self.bytes = self.bytes.saturating_sub(previous.len() as u64);
        } else {
            self.order.push_back(key);
        }
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        self.trim();
    }

    fn trim(&mut self) {
        let maximum = media_cache_max_bytes()
            .saturating_sub(completed_media_range_bytes())
            .min(RANGE_CACHE_HARD_MAX_BYTES);
        while self.bytes > maximum {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            if let Some(bytes) = self.ranges.remove(&key) {
                self.bytes = self.bytes.saturating_sub(bytes.len() as u64);
            }
        }
        set_auxiliary_media_cache_bytes(self.bytes);
    }

    fn clear(&mut self) {
        self.ranges.clear();
        self.order.clear();
        self.bytes = 0;
        set_auxiliary_media_cache_bytes(0);
    }
}

struct FeedSession {
    id: u64,
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
    start: HlsStart,
    following: bool,
    index: Option<u64>,
    playlist: Option<HlsPlaylist>,
}

struct RawFeedPayload {
    index: u64,
    bytes: Vec<u8>,
    confirmed_at: Option<u64>,
    history: Option<HlsPlaylist>,
}

enum FeedPayloadProbe {
    Found(RawFeedPayload),
    Deferred(FeedPayloadRoot),
    Missing,
    Transient,
}

async fn probe_feed_update(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    index: u64,
    attempt_limit: Option<usize>,
) -> FeedProbe<Vec<u8>> {
    let address = get_feed_address(&owner.to_string(), &topic.to_string(), index);
    if address.len() != 32 {
        return FeedProbe::Missing;
    }
    let admission = attempt_limit.map_or_else(RetrieveAdmission::new, |limit| {
        RetrieveAdmission::new_with_attempt_limit(limit)
    });
    let _close_admission = admission.close_on_drop();
    let (output, input) = mpsc::unbounded();
    if client
        .chunk_port
        .0
        .try_send(ChunkRetrieveRequest {
            address,
            chan: output,
            cancel: None,
            admission: Some(admission.clone()),
            hedge_demand: None,
        })
        .is_err()
    {
        return FeedProbe::Transient;
    }
    match input.recv().await {
        Ok(update) if !update.is_empty() => FeedProbe::Found(update),
        Ok(_)
            if attempt_limit.is_none_or(|attempt_limit| {
                admission.claimed_physical_attempts() == Some(attempt_limit)
                    && admission.timed_out_physical_attempts() == Some(0)
                    && admission.confirmed_empty_physical_attempts() == Some(attempt_limit)
            }) =>
        {
            FeedProbe::Missing
        }
        Ok(_) | Err(_) => FeedProbe::Transient,
    }
}

async fn probe_feed_payload(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    index: u64,
    maximum_payload_bytes: usize,
    attempt_limit: Option<usize>,
) -> FeedPayloadProbe {
    let update = match probe_feed_update(client, owner, topic, index, attempt_limit).await {
        FeedProbe::Found(update) => update,
        FeedProbe::Missing => return FeedPayloadProbe::Missing,
        FeedProbe::Transient => return FeedPayloadProbe::Transient,
    };
    let Some(root) = decode_feed_payload_root(index, update) else {
        return FeedPayloadProbe::Transient;
    };
    if root.span() > maximum_payload_bytes as u64 {
        return FeedPayloadProbe::Deferred(root);
    }
    match retrieve_feed_payload(&root, maximum_payload_bytes, &client.chunk_port.0).await {
        Some(bytes) => FeedPayloadProbe::Found(RawFeedPayload {
            index,
            bytes,
            confirmed_at: None,
            history: None,
        }),
        None => FeedPayloadProbe::Transient,
    }
}

async fn hls_range(
    client: Arc<Weeb3>,
    reference: String,
    root: DecodedJoinChunk,
    encrypted: bool,
    start: u64,
    end: u64,
) -> Option<Arc<[u8]>> {
    if let Some(bytes) = RANGE_CACHE.with(|cache| cache.borrow_mut().get(&reference, start, end)) {
        return Some(bytes);
    }
    let bytes =
        retrieve_data_range_from_root(root, start, end, encrypted, &client.chunk_port.0).await?;
    if bytes.len() as u64 != end.checked_sub(start)?.checked_add(1)? {
        return None;
    }
    let bytes: Arc<[u8]> = Arc::from(bytes);
    RANGE_CACHE.with(|c| c.borrow_mut().insert(reference, start, end, bytes.clone()));
    Some(bytes)
}

fn next_feed_id() -> u64 {
    NEXT_FEED_ID.with(|next| {
        let id = next.get().wrapping_add(1).max(1);
        next.set(id);
        id
    })
}

fn prerequisite_timestamp() -> u64 {
    PREREQUISITE_CLOCK.with(|clock| {
        let timestamp = clock.get().saturating_add(1);
        clock.set(timestamp);
        timestamp
    })
}

fn begin_feed(client: Arc<Weeb3>, owner: String, topic: String, start: HlsStart) -> u64 {
    let id = next_feed_id();
    BEGINNING_MEDIA_READY.with(|ready| ready.set(false));
    FEED.with(|feed| {
        *feed.borrow_mut() = Some(FeedSession {
            id,
            client,
            owner,
            topic,
            start,
            following: false,
            index: None,
            playlist: None,
        });
    });
    id
}

fn feed_is_current(id: u64) -> bool {
    FEED.with(|feed| feed.borrow().as_ref().is_some_and(|feed| feed.id == id))
}

fn end_feed(id: u64) {
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        if feed.as_ref().is_some_and(|feed| feed.id == id) {
            *feed = None;
        }
    });
}

fn install_snapshot(id: u64, index: u64, mut playlist: HlsPlaylist) -> Option<()> {
    playlist.keep_growing();
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.as_mut().filter(|active| active.id == id)?;
        (active.index, active.playlist) = (Some(index), Some(playlist));
        Some(())
    })
}

async fn discover_beginning(
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
) -> Option<RawFeedPayload> {
    async_std::future::timeout(FEED_DISCOVERY_TIMEOUT, async {
        loop {
            let (results, input) = mpsc::unbounded();
            for index in 0..BEGINNING_DISCOVERY_WIDTH {
                let client = client.clone();
                let owner = owner.clone();
                let topic = topic.clone();
                let results = results.clone();
                spawn_local(async move {
                    let result = probe_feed_payload(
                        &client,
                        &owner,
                        &topic,
                        index,
                        BEGINNING_PAYLOAD_BYTES,
                        Some(FEED_PROBE_ATTEMPTS),
                    )
                    .await;
                    let _ = results.try_send(result);
                });
            }
            drop(results);
            let mut best = None;
            let collect = async {
                while let Ok(probe) = input.recv().await {
                    if let FeedPayloadProbe::Found(payload) = probe
                        && let Some(playlist) = HlsPlaylist::parse(&payload.bytes)
                        && playlist.sequence == 0
                        && playlist.startup_plan(HlsStart::Beginning).is_some()
                    {
                        let prefix_ready = playlist
                            .segments
                            .iter()
                            .filter(|segment| !segment.gap)
                            .count()
                            >= BEGINNING_PREFIX_SEGMENTS;
                        if prefix_ready {
                            best = Some(payload);
                            break;
                        }
                        if best
                            .as_ref()
                            .is_none_or(|best: &RawFeedPayload| payload.index > best.index)
                        {
                            best = Some(payload);
                        }
                    }
                }
            };
            let _ = async_std::future::timeout(BEGINNING_WAVE_TIMEOUT, collect).await;
            if let Some(playable) = best {
                return Some(playable);
            }
            async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
        }
    })
    .await
    .ok()
    .flatten()
}

async fn edge_probe_wave(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    indices: &[u64],
    lower_is_known: bool,
    attempt_limit: Option<usize>,
) -> (Vec<Option<Vec<u8>>>, Vec<bool>) {
    let (results, input) = mpsc::unbounded();
    for (slot, index) in indices.iter().copied().enumerate() {
        let client = client.clone();
        let owner = owner.to_string();
        let topic = topic.to_string();
        let results = results.clone();
        spawn_local(async move {
            let result = probe_feed_update(&client, &owner, &topic, index, attempt_limit).await;
            let _ = results.try_send((slot, result));
        });
    }
    drop(results);
    let mut found = vec![None; indices.len()];
    let mut missing = vec![false; indices.len()];
    let mut completed = vec![false; indices.len()];
    let collect = async {
        while let Ok((slot, result)) = input.recv().await {
            completed[slot] = true;
            match result {
                FeedProbe::Found(update) => found[slot] = Some(update),
                FeedProbe::Missing => missing[slot] = true,
                FeedProbe::Transient => {}
            }
            let first_unsettled = found
                .iter()
                .rposition(Option::is_some)
                .map_or(0, |found| found + 1);
            if (lower_is_known || first_unsettled > 0)
                && let Some(upper) = (first_unsettled..indices.len()).find(|slot| missing[*slot])
                && completed[first_unsettled..=upper]
                    .iter()
                    .all(|settled| *settled)
            {
                break;
            }
        }
    };
    let timeout = if lower_is_known {
        EDGE_WAVE_TIMEOUT
    } else {
        EDGE_COLD_WAVE_TIMEOUT
    };
    if attempt_limit.is_none() {
        collect.await;
    } else {
        let _ = async_std::future::timeout(timeout, collect).await;
    }
    (found, missing)
}

async fn discover_edge_update(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
) -> Option<(u64, Vec<u8>)> {
    let fast = Some(EDGE_PROBE_ATTEMPTS);
    let (mut found, missing) =
        edge_probe_wave(client, owner, topic, &EDGE_ANCHORS, false, fast).await;
    let highest = found.iter().rposition(Option::is_some)?;
    let mut latest = (EDGE_ANCHORS[highest], found[highest].take()?);
    client.interface_log(format!("HLS edge anchor {}", latest.0));
    let mut upper = (highest + 1..EDGE_ANCHORS.len())
        .find(|slot| missing[*slot])
        .map(|slot| EDGE_ANCHORS[slot])?;

    loop {
        if latest.0.saturating_add(1) == upper {
            return Some(latest);
        }
        let interior = upper.saturating_sub(latest.0).saturating_sub(1);
        let count = interior.min((EDGE_REFINEMENT_WIDTH - 1) as u64) as usize;
        let indices = if count as u64 == interior {
            (latest.0 + 1..upper).collect::<Vec<_>>()
        } else {
            let first = latest.0 + 1;
            let remaining = count.saturating_sub(1);
            let divisor = (remaining + 1) as u128;
            let span = u128::from(upper.saturating_sub(first));
            std::iter::once(first)
                .chain((1..=remaining).map(|position| {
                    let offset = (span * position as u128).div_ceil(divisor) as u64;
                    first.saturating_add(offset)
                }))
                .collect::<Vec<_>>()
        };
        let mut indices = indices;
        indices.push(upper);
        let (mut found, missing) =
            edge_probe_wave(client, owner, topic, &indices, true, fast).await;
        let previous = (latest.0, upper);
        if let Some(slot) = found.iter().rposition(Option::is_some) {
            latest = (indices[slot], found[slot].take()?);
        }
        let Some(slot) = indices
            .iter()
            .enumerate()
            .position(|(slot, index)| *index > latest.0 && missing[slot])
        else {
            return None;
        };
        upper = indices[slot];
        if (latest.0, upper) == previous {
            return None;
        }
    }
}

async fn discover_latest_once(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
) -> Option<RawFeedPayload> {
    let (index, update) = discover_edge_update(client, owner, topic).await?;
    retrieve_confirmed_payload(
        client,
        owner,
        topic,
        index,
        update,
        Some(FEED_PROBE_ATTEMPTS),
    )
    .await
}
async fn retrieve_confirmed_payload(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    mut index: u64,
    mut update: Vec<u8>,
    attempt_limit: Option<usize>,
) -> Option<RawFeedPayload> {
    loop {
        let root = decode_feed_payload_root(index, update)?;
        let bytes =
            retrieve_feed_payload(&root, MAX_STREAM_FEED_PAYLOAD_BYTES, &client.chunk_port.0)
                .await?;
        let Some(next) = index.checked_add(1) else {
            return Some(RawFeedPayload {
                index,
                bytes,
                confirmed_at: Some(prerequisite_timestamp()),
                history: None,
            });
        };
        match probe_feed_update(client, owner, topic, next, attempt_limit).await {
            FeedProbe::Found(next_update) => (index, update) = (next, next_update),
            FeedProbe::Missing => {
                return Some(RawFeedPayload {
                    index,
                    bytes,
                    confirmed_at: Some(prerequisite_timestamp()),
                    history: None,
                });
            }
            FeedProbe::Transient => return None,
        }
    }
}

async fn catch_up_current_payload(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    mut payload: RawFeedPayload,
) -> Option<RawFeedPayload> {
    let current = || feed_is_current(id) && result_view_request_is_current(view_generation);
    loop {
        if !current() {
            return None;
        }
        let next = payload.index.checked_add(1)?;
        match probe_feed_update(client, owner, topic, next, None).await {
            FeedProbe::Missing => {
                payload.confirmed_at = Some(prerequisite_timestamp());
                return current().then_some(payload);
            }
            FeedProbe::Found(update) => {
                if let Some(newest) =
                    retrieve_confirmed_payload(client, owner, topic, next, update, None).await
                {
                    return current().then_some(newest);
                }
            }
            FeedProbe::Transient => {}
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
}

async fn history_snapshots(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    indices: Vec<u64>,
) -> Vec<(u64, HlsPlaylist)> {
    stream::iter(indices)
        .map(|index| {
            let client = client.clone();
            let owner = owner.to_string();
            let topic = topic.to_string();
            async move {
                let bytes = match probe_feed_payload(
                    &client,
                    &owner,
                    &topic,
                    index,
                    HISTORY_WINDOW_BYTES,
                    Some(FEED_PROBE_ATTEMPTS),
                )
                .await
                {
                    FeedPayloadProbe::Found(payload) => Some(payload.bytes),
                    FeedPayloadProbe::Deferred(root) => {
                        retrieve_feed_payload(
                            &root,
                            MAX_STREAM_FEED_PAYLOAD_BYTES,
                            &client.chunk_port.0,
                        )
                        .await
                    }
                    FeedPayloadProbe::Missing | FeedPayloadProbe::Transient => None,
                }?;
                Some((index, HlsPlaylist::parse(&bytes)?))
            }
        })
        .buffer_unordered(HISTORY_PARALLEL)
        .filter_map(async move |snapshot| snapshot)
        .collect()
        .await
}

fn history_repairs(
    attempted: &[u64],
    snapshots: &[(u64, HlsPlaylist)],
    head_index: u64,
    head: &HlsPlaylist,
) -> Option<Vec<u64>> {
    let mut ordered = snapshots.to_vec();
    ordered.push((head_index, head.clone()));
    ordered.sort_by_key(|(index, _)| *index);
    let attempted = attempted.iter().copied().collect::<HashSet<_>>();
    let mut repairs = Vec::new();
    let mut add = |range: std::ops::Range<u64>| -> Option<()> {
        for index in range {
            if index < head_index && !attempted.contains(&index) && !repairs.contains(&index) {
                repairs.push(index);
                if repairs.len() > HISTORY_MAX_REPAIRS {
                    return None;
                }
            }
        }
        Some(())
    };
    let (first_index, first) = ordered.first()?;
    if first.sequence != 0 {
        add(0..*first_index)?;
    }
    for pair in ordered.windows(2) {
        if !pair[0].1.joins(&pair[1].1) {
            add(pair[0].0.saturating_add(1)..pair[1].0)?;
        }
    }
    Some(repairs)
}

async fn reconstruct_history(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    head_index: u64,
    head_bytes: &[u8],
) -> Option<HlsPlaylist> {
    let head = HlsPlaylist::parse(head_bytes)?;
    if head.sequence == 0 {
        return Some(head);
    }
    let residue = head_index % HISTORY_STRIDE;
    let indices = (residue..head_index)
        .step_by(usize::try_from(HISTORY_STRIDE).ok()?)
        .collect::<Vec<_>>();
    if indices.is_empty() || indices.len() > HISTORY_MAX_PROBES {
        return None;
    }
    let mut snapshots = history_snapshots(client, owner, topic, indices.clone()).await;
    if let Some(history) = HlsPlaylist::reconstruct(snapshots.clone(), head_index, head.clone()) {
        return Some(history);
    }
    let repairs = history_repairs(&indices, &snapshots, head_index, &head)?;
    snapshots.extend(history_snapshots(client, owner, topic, repairs).await);
    HlsPlaylist::reconstruct(snapshots, head_index, head)
}

async fn discover_raw_for_view(
    id: u64,
    view_generation: u64,
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
) -> Option<RawFeedPayload> {
    loop {
        if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
            return None;
        }
        let payload = discover_latest_once(&client, &owner, &topic).await;
        if let Some(payload) = payload {
            return Some(payload);
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
}

async fn discover_for_view(
    id: u64,
    view_generation: u64,
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
) -> Option<RawFeedPayload> {
    loop {
        let mut payload = discover_raw_for_view(
            id,
            view_generation,
            client.clone(),
            owner.clone(),
            topic.clone(),
        )
        .await?;
        payload.history =
            reconstruct_history(&client, &owner, &topic, payload.index, &payload.bytes).await;
        if payload.history.is_some() {
            return Some(payload);
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
}

fn render_active_feed(
    owner: &str,
    topic: &str,
    start: HlsStart,
    local_bytes_base: &str,
) -> Option<(u64, Vec<u8>, Option<u64>)> {
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let feed = feed.as_mut()?;
        if feed.owner != owner || feed.topic != topic {
            return None;
        }
        let index = feed.index?;
        let body = feed.playlist.as_ref()?.render(local_bytes_base, start);
        let follower = (!feed.following).then_some(feed.id);
        feed.following = true;
        Some((index, body, follower))
    })
}

fn apply_update(
    id: u64,
    index: u64,
    merge: impl FnOnce(&mut HlsPlaylist) -> Option<usize>,
) -> Option<usize> {
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.as_mut().filter(|active| active.id == id)?;
        if active.index.is_some_and(|current| index <= current) {
            return None;
        }
        let playlist = active.playlist.as_mut()?;
        let appended = merge(playlist)?;
        playlist.keep_growing();
        active.index = Some(index);
        Some(appended)
    })
}

fn apply_full_update(id: u64, index: u64, candidate: HlsPlaylist) -> Option<usize> {
    apply_update(id, index, |playlist| playlist.merge_playlist(candidate))
}

fn feed_follow_context(id: u64) -> Option<(Arc<Weeb3>, String, String, HlsStart, u64)> {
    FEED.with(|feed| {
        let feed = feed.borrow();
        let feed = feed.as_ref().filter(|feed| feed.id == id)?;
        feed.playlist.as_ref()?;
        Some((
            feed.client.clone(),
            feed.owner.clone(),
            feed.topic.clone(),
            feed.start,
            feed.index?,
        ))
    })
}

async fn apply_deferred_update(
    id: u64,
    client: &Arc<Weeb3>,
    root: FeedPayloadRoot,
) -> Option<usize> {
    let index = root.index;
    if let Some(tail) =
        retrieve_feed_payload_tail(&root, FEED_TAIL_PROBE_BYTES, &client.chunk_port.0).await
        && let Some(appended) = apply_update(id, index, |playlist| playlist.merge_tail(&tail))
    {
        return Some(appended);
    }
    let body =
        retrieve_feed_payload(&root, MAX_STREAM_FEED_PAYLOAD_BYTES, &client.chunk_port.0).await?;
    apply_full_update(id, index, HlsPlaylist::parse(&body)?)
}

fn warm_appended(id: u64, appended: usize, start: HlsStart) {
    if appended == 0 || start != HlsStart::Live {
        return;
    }
    let references = FEED.with(|feed| {
        let feed = feed.borrow();
        feed.as_ref()
            .filter(|feed| feed.id == id)
            .and_then(|feed| feed.playlist.as_ref())
            .map(|playlist| {
                let first_new = playlist.segments.len().saturating_sub(appended);
                playlist
                    .segments
                    .iter()
                    .skip(first_new)
                    .filter(|segment| !segment.gap)
                    .take(HLS_LIVE_SYNC_SEGMENTS)
                    .map(|segment| segment.reference.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    for reference in references {
        player::warm_startup_reference(&reference, start);
    }
}

pub(super) fn warm_beginning_successor(reference: &str) {
    let successor = FEED.with(|feed| {
        let feed = feed.borrow();
        let playlist = feed
            .as_ref()
            .filter(|feed| feed.start == HlsStart::Beginning)?
            .playlist
            .as_ref()?;
        let current = playlist
            .segments
            .iter()
            .position(|segment| segment.reference == reference)?;
        playlist
            .segments
            .iter()
            .skip(current + 1)
            .find(|segment| !segment.gap)
            .map(|segment| segment.reference.clone())
    });
    if let Some(successor) = successor {
        player::warm_startup_reference(&successor, HlsStart::Beginning);
    }
}

fn spawn_beginning_history(
    id: u64,
    view_generation: u64,
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
) {
    spawn_local(async move {
        while feed_is_current(id)
            && result_view_request_is_current(view_generation)
            && !BEGINNING_MEDIA_READY.with(Cell::get)
        {
            async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
        }
        let Some(mut payload) =
            discover_for_view(id, view_generation, client.clone(), owner, topic).await
        else {
            return;
        };
        let Some(history) = payload.history.take() else {
            return;
        };
        if feed_is_current(id)
            && result_view_request_is_current(view_generation)
            && apply_full_update(id, payload.index, history).is_some()
        {
            client.interface_log(format!("HLS history reached index {}", payload.index));
        }
    });
}

#[rustfmt::skip]
pub(super) fn start_beginning_history() { BEGINNING_MEDIA_READY.with(|ready| ready.set(true)); }

fn spawn_follower(id: u64) {
    spawn_local(async move {
        loop {
            let Some((client, owner, topic, start, head)) = feed_follow_context(id) else {
                return;
            };
            let Some(index) = head.checked_add(1) else {
                return;
            };
            let candidate =
                probe_feed_payload(&client, &owner, &topic, index, FEED_TAIL_PROBE_BYTES, None)
                    .await;
            if !feed_is_current(id) {
                return;
            }
            let appended = match candidate {
                FeedPayloadProbe::Found(payload) => HlsPlaylist::parse(&payload.bytes)
                    .and_then(|playlist| apply_full_update(id, payload.index, playlist)),
                FeedPayloadProbe::Deferred(root) => apply_deferred_update(id, &client, root).await,
                FeedPayloadProbe::Missing | FeedPayloadProbe::Transient => None,
            };
            if let Some(appended) = appended {
                if appended != 0 {
                    client.interface_log(format!(
                        "HLS feed advanced to {index}; appended {appended} segment(s)"
                    ));
                    warm_appended(id, appended, start);
                }
                if start == HlsStart::Beginning {
                    async_std::task::sleep(FEED_POLL_INTERVAL).await;
                }
                continue;
            }
            async_std::task::sleep(FEED_POLL_INTERVAL).await;
        }
    });
}

async fn fetch_hls_body_response(
    client: Arc<Weeb3>,
    reference: String,
    method: &str,
    range: Option<&str>,
    if_none_match: Option<&str>,
) -> FetchResponse {
    let etag = format!("\"{reference}\"");
    if if_none_match_matches(if_none_match, &etag) {
        return FetchResponse::ok(304, vec![("ETag".to_string(), etag)], None);
    }
    let decoded = match hex::decode(&reference) {
        Ok(decoded) => decoded,
        Err(_) => return FetchResponse::error(400, "invalid HLS swarm reference"),
    };
    let encrypted = decoded.len() == 64;
    let Some(root) = retrieve_decoded_data_root(&decoded, &client.chunk_port.0).await else {
        return FetchResponse::error(502, format!("HLS segment {reference} was unavailable"));
    };
    if root.span == 0 {
        return FetchResponse::error(502, "HLS segment was empty");
    }
    let span = root.span;

    let mut headers = vec![
        (
            "Content-Type".to_string(),
            "application/octet-stream".to_string(),
        ),
        (
            "Cache-Control".to_string(),
            "public, max-age=31536000, immutable".to_string(),
        ),
        ("ETag".to_string(), etag),
        ("Accept-Ranges".to_string(), "bytes".to_string()),
    ];

    if let Some(range) = range {
        let Some((start, end)) = parse_hls_range(range, span) else {
            return FetchResponse::ok(
                416,
                vec![("Content-Range".to_string(), format!("bytes */{span}"))],
                None,
            );
        };
        let Some(bytes) = hls_range(client, reference, root, encrypted, start, end).await else {
            return FetchResponse::error(503, "HLS segment range was unavailable");
        };
        headers.push(("Content-Length".to_string(), bytes.len().to_string()));
        headers.push((
            "Content-Range".to_string(),
            format!("bytes {start}-{end}/{span}"),
        ));
        return if method == "HEAD" {
            FetchResponse::ok(206, headers, None)
        } else {
            FetchResponse::ok_shared(206, headers, bytes)
        };
    }

    let prefix_end = span.saturating_sub(1).min(188);
    let mime = hls_range(client, reference, root, encrypted, 0, prefix_end)
        .await
        .map_or("application/octet-stream", |p| hls_payload_mime(&p));
    headers[0].1 = mime.to_string();
    headers.push(("Content-Length".to_string(), span.to_string()));
    if method == "HEAD" {
        FetchResponse::ok(200, headers, None)
    } else {
        FetchResponse::stream(200, headers)
    }
}

fn parse_hls_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    if start.is_empty() || end.is_empty() || end.contains(',') {
        return None;
    }
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    (start <= end && end < size).then_some((start, end))
}

async fn fetch_feed_response(
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
    index: Option<u64>,
    start: HlsStart,
    method: &str,
    _if_none_match: Option<&str>,
    local_bytes_base: &str,
) -> FetchResponse {
    let mut follower = None;
    let rendered = if let Some(index) = index {
        match probe_feed_payload(
            &client,
            &owner,
            &topic,
            index,
            MAX_STREAM_FEED_PAYLOAD_BYTES,
            Some(FEED_PROBE_ATTEMPTS),
        )
        .await
        {
            FeedPayloadProbe::Found(payload) => HlsPlaylist::parse(&payload.bytes)
                .map(|playlist| (index, playlist.render(local_bytes_base, start))),
            FeedPayloadProbe::Deferred(_)
            | FeedPayloadProbe::Missing
            | FeedPayloadProbe::Transient => None,
        }
    } else if let Some((index, body, start_follower)) =
        render_active_feed(&owner, &topic, start, local_bytes_base)
    {
        follower = start_follower;
        Some((index, body))
    } else {
        discover_latest_once(&client, &owner, &topic)
            .await
            .and_then(|payload| {
                HlsPlaylist::parse(&payload.bytes)
                    .map(|playlist| (payload.index, playlist.render(local_bytes_base, start)))
            })
    };
    let Some((index, body)) = rendered else {
        return FetchResponse::error(502, "The HLS feed could not be loaded");
    };
    if let Some(id) = follower {
        spawn_follower(id);
    }
    let mode = (start == HlsStart::Live)
        .then_some("live")
        .unwrap_or("beginning");
    let etag = format!("\"hls-feed-{index}-{mode}\"");
    let headers = vec![
        (
            "Content-Type".to_string(),
            "application/vnd.apple.mpegurl".to_string(),
        ),
        ("Content-Length".to_string(), body.len().to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
        ("ETag".to_string(), etag),
    ];
    FetchResponse::ok(200, headers, (method != "HEAD").then_some(body))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn try_fetch_response(
    client: Arc<Weeb3>,
    request_url: &str,
    pathname: &str,
    method: &str,
    range: Option<&str>,
    if_none_match: Option<&str>,
    _if_range: Option<&str>,
    _stream_token: Option<&str>,
) -> Option<FetchResponse> {
    if let Some(reference) = canonical_hls_bytes_resource(pathname) {
        return Some(match reference {
            Ok(reference) => {
                fetch_hls_body_response(client, reference, method, range, if_none_match).await
            }
            Err(error) => FetchResponse::error(400, error),
        });
    }
    let (owner, topic) = canonical_feed_resource(pathname)?;
    let url = match web_sys::Url::new(request_url) {
        Ok(url) => url,
        Err(_) => return Some(FetchResponse::error(400, "invalid feed URL")),
    };
    let query = url.search_params();
    let index = match query.get("index") {
        Some(index) => match index.parse() {
            Ok(index) => Some(index),
            Err(_) => return Some(FetchResponse::error(400, "invalid feed index")),
        },
        None => None,
    };
    let start = match query.get("start").as_deref() {
        None | Some("beginning") => HlsStart::Beginning,
        Some("live") => HlsStart::Live,
        Some(_) => return Some(FetchResponse::error(400, "invalid HLS start")),
    };
    Some(
        fetch_feed_response(
            client,
            owner,
            topic,
            index,
            start,
            method,
            if_none_match,
            &local_hls_bytes_base(pathname),
        )
        .await,
    )
}

fn canonical_hls_bytes_resource(pathname: &str) -> Option<Result<String, &'static str>> {
    for marker in route_markers("hls/bytes") {
        let Some(resource) = pathname.strip_prefix(&marker) else {
            continue;
        };
        let resource = decode_component(resource.trim());
        let mut parts = resource.split('/');
        let reference = parts.next().unwrap_or_default();
        if !is_hex_reference(reference) || parts.any(|part| !part.is_empty()) {
            return Some(Err("invalid HLS swarm reference"));
        }
        return Some(Ok(reference.to_ascii_lowercase()));
    }
    None
}

fn canonical_feed_resource(pathname: &str) -> Option<(String, String)> {
    for marker in route_markers("feeds") {
        let Some(resource) = pathname.strip_prefix(&marker) else {
            continue;
        };
        let resource = decode_component(resource.trim());
        let mut parts = resource.split('/').filter(|part| !part.is_empty());
        let owner = parts
            .next()?
            .trim_start_matches("0x")
            .trim_start_matches("0X");
        let topic = parts.next()?;
        if parts.next().is_some()
            || owner.len() != 40
            || !owner.bytes().all(|byte| byte.is_ascii_hexdigit())
            || topic.len() != 64
            || !topic.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        return Some((owner.to_ascii_lowercase(), topic.to_ascii_lowercase()));
    }
    None
}

fn local_hls_bytes_base(pathname: &str) -> String {
    if pathname.starts_with(&format!("{STREAMING_ROUTE_BASE}/testnet/")) {
        streaming_route_path("testnet/hls/bytes")
    } else if pathname.starts_with(&format!("{STREAMING_ROUTE_BASE}/mainnet/")) {
        streaming_route_path("mainnet/hls/bytes")
    } else {
        streaming_route_path("hls/bytes")
    }
}

pub(crate) async fn attach_hls_feed_player(
    client: Arc<Weeb3>,
    player_element: &Element,
    owner: String,
    topic: String,
    start: HlsStart,
    view_generation: u64,
) -> Result<&'static str, String> {
    let owner = owner
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .to_ascii_lowercase();
    let topic = normalize_feed_topic(&topic);
    if owner.len() != 40 || !owner.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The stream feed owner is invalid.".to_string());
    }
    clear_completed_media_ranges();
    let id = begin_feed(client.clone(), owner.clone(), topic.clone(), start);
    let result = async {
        let loader = async {
            let result = JsFuture::from(player::load_hls()).await;
            (result, prerequisite_timestamp())
        };
        let worker_client = client.clone();
        let worker = async {
            let ready = service_worker_controls_bzz_requests(
                &worker_client,
                "HLS feed and segment requests",
                || feed_is_current(id) && result_view_request_is_current(view_generation),
            )
            .await;
            (ready, prerequisite_timestamp())
        };
        let ((worker_ready, worker_at), (hls_class, loader_at), payload) = match start {
            HlsStart::Beginning => {
                let early = discover_beginning(client.clone(), owner.clone(), topic.clone());
                spawn_beginning_history(
                    id,
                    view_generation,
                    client.clone(),
                    owner.clone(),
                    topic.clone(),
                );
                let (loader, (worker, prefix)) = join(loader, join(worker, early)).await;
                (worker, loader, prefix)
            }
            HlsStart::Live => {
                let discovery = discover_raw_for_view(
                    id,
                    view_generation,
                    client.clone(),
                    owner.clone(),
                    topic.clone(),
                );
                let (worker, (loader, discovery)) = join(worker, join(loader, discovery)).await;
                (worker, loader, discovery)
            }
        };
        if !worker_ready {
            return Err(service_worker_scope_protocol_error(
                "HLS feed and segment requests",
            ));
        }
        if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        let mut payload = payload.ok_or_else(|| "The HLS feed could not be loaded.".to_string())?;
        let live = start == HlsStart::Live;
        let confirmed_after_setup = payload
            .confirmed_at
            .is_some_and(|confirmed| confirmed > worker_at && confirmed > loader_at);
        if live && !confirmed_after_setup {
            payload =
                catch_up_current_payload(id, view_generation, &client, &owner, &topic, payload)
                    .await
                    .ok_or_else(|| "HLS open was superseded".to_string())?;
        }
        let warmed_live_references = if live {
            HlsPlaylist::parse(&payload.bytes)
                .and_then(|playlist| playlist.startup_plan(HlsStart::Live))
                .map(|plan| {
                    player::warm_live_runway(&plan);
                    plan.references
                })
        } else {
            None
        };
        let index = payload.index;
        let playlist = match payload.history.take() {
            Some(history) => history,
            None if live => reconstruct_history(&client, &owner, &topic, index, &payload.bytes)
                .await
                .ok_or_else(|| "The HLS feed history could not be reconstructed.".to_string())?,
            None => HlsPlaylist::parse(&payload.bytes)
                .filter(|playlist| playlist.sequence == 0)
                .ok_or_else(|| "The beginning HLS prefix could not be loaded.".to_string())?,
        };
        let plan = playlist
            .startup_plan(start)
            .ok_or_else(|| "The HLS feed does not contain two playable segments.".to_string())?;
        let elapsed = playlist.duration();
        install_snapshot(id, index, playlist)
            .ok_or_else(|| "HLS open was superseded".to_string())?;

        let mut source = format!("{}/{}/{}", streaming_route_path("feeds"), owner, topic);
        if live {
            source.push_str("?start=live");
        }
        client.interface_log(format!(
            "HLS open index={} elapsed={:.3}s mode={}",
            index,
            elapsed,
            if live { "live" } else { "beginning" }
        ));
        if live && warmed_live_references.as_ref() != Some(&plan.references) {
            player::warm_live_runway(&plan);
        }
        let mode = player::play_hls(player_element, &source, hls_class, plan, start)
            .map_err(|error| format!("Could not initialize HLS: {}", js_error_message(&error)))?;
        Ok(mode)
    }
    .await;
    if result.is_err() {
        end_feed(id);
        player::destroy_current_hls();
        RANGE_CACHE.with(|cache| cache.borrow_mut().clear());
    }
    result
}

pub(crate) async fn open_hls_feed_view(
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
    start: HlsStart,
) {
    let view_generation = begin_result_view_request();
    let document = web_sys::window().unwrap().document().unwrap();
    let wrapper = document.create_element("section").unwrap();
    let player = document.create_element("video").unwrap();
    player.set_attribute("controls", "").ok();
    player.set_attribute("autoplay", "").ok();
    player.set_attribute("preload", "auto").ok();
    player.set_attribute("playsinline", "").ok();
    player
        .set_attribute("style", "width:90%;max-height:75vh;")
        .ok();
    player
        .set_attribute("aria-label", "Swarm HLS video stream")
        .ok();
    if let Some(media) = player.dyn_ref::<HtmlMediaElement>() {
        media.set_default_muted(true);
        media.set_muted(true);
    }
    let status = document.create_element("div").unwrap();
    status.set_class_name("weeb3-hls-status");
    status.set_attribute("role", "status").ok();
    status.set_text_content(Some("Discovering the HLS feed edge..."));
    wrapper.append_child(&player).ok();
    wrapper.append_child(&status).ok();
    if !replace_stream_result_view(&wrapper, view_generation) {
        return;
    }
    match attach_hls_feed_player(client, &player, owner, topic, start, view_generation).await {
        Ok(mode) if result_view_request_is_current(view_generation) => {
            status.set_text_content(Some(&format!(
                "HLS player attached with {mode}; buffering through weeb-3."
            )));
        }
        Err(error) if result_view_request_is_current(view_generation) => {
            status.set_text_content(Some(&error));
            status.set_attribute("data-state", "error").ok();
        }
        _ => {}
    }
}

pub(crate) fn release_hls_view() {
    FEED.with(|feed| *feed.borrow_mut() = None);
    player::destroy_current_hls();
}

pub(crate) fn release_hls_for_bzz_view() {
    release_hls_view();
    RANGE_CACHE.with(|cache| cache.borrow_mut().clear());
}

fn js_error_message(error: &JsValue) -> String {
    js_sys::Reflect::get(error, &JsValue::from_str("message"))
        .ok()
        .and_then(|message| message.as_string())
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "unknown browser error".to_string())
}
