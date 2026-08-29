use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use futures::{StreamExt, stream};
use wasm_bindgen_futures::spawn_local;

use super::{
    HLS_LIVE_BODY_RUNWAY_SEGMENTS, HLS_LIVE_EDGE_SEGMENTS, HlsPlaylist, HlsStart,
    MAX_STREAM_FEED_PAYLOAD_BYTES, PreparedHlsFeed, hls_payload_mime,
    hls_progressive_foreground_transition, is_hex_reference,
};
use crate::{
    ChunkRetrieveRequest, Weeb3,
    bzz_stream::{
        FeedPayloadRoot, decode_feed_payload_root, retrieve_feed_payload,
        retrieve_feed_payload_tail,
    },
    feed::FeedProbe,
    get_feed_address, mpsc, normalize_feed_topic,
    retrieval::{DecodedJoinChunk, retrieve_data_range_from_root, retrieve_decoded_data_root},
    retrieval_conventions::RetrieveAdmission,
    stream::{
        FetchResponse, clear_completed_media_ranges, completed_media_range_bytes,
        media_cache_max_bytes, result_view_request_is_current, set_auxiliary_media_cache_bytes,
    },
    stream_conventions::{
        STREAMING_ROUTE_BASE, decode_component, if_none_match_matches, route_markers,
        streaming_route_path,
    },
};

const RANGE_CACHE_HARD_MAX_BYTES: u64 = 96 * 1024 * 1024;
const BODY_PREFETCH_HORIZON: usize = HLS_LIVE_BODY_RUNWAY_SEGMENTS;
const HLS_BODY_PREFETCH_MAX_PARALLEL: usize = 3;
const BEGINNING_DISCOVERY_WIDTH: u64 = 8;
const BEGINNING_PREFIX_TARGET_SEGMENTS: usize = 4;
const BEGINNING_PAYLOAD_BYTES: usize = 64 * 1024;
const BEGINNING_WAVE_TIMEOUT: Duration = Duration::from_millis(1_500);
const EDGE_COLD_WAVE_TIMEOUT: Duration = Duration::from_millis(4_000);
const EDGE_WAVE_TIMEOUT: Duration = Duration::from_millis(1_500);
const EDGE_REFINEMENT_WIDTH: usize = 16;
#[rustfmt::skip]
const EDGE_ANCHORS: [u64; 16] = [
    0, 1, 7, 255, 511, 1_023, 1_535, 1_791,
    2_047, 4_095, 8_191, 16_383, 65_535, 262_143, 1_048_575, u64::MAX,
];
const FEED_PROBE_ATTEMPTS: usize = 2;
const EDGE_PROBE_ATTEMPTS: usize = 2;
const FEED_TAIL_PROBE_BYTES: usize = 4 * 1024;
const FEED_FOLLOW_AHEAD: u64 = 4;
const FEED_POLL_INTERVAL: Duration = Duration::from_millis(400);
const FEED_FRONTIER_REFRESH_INTERVAL: f64 = 15_000.0;
const LIVE_TAIL_FALLBACK_LIMIT: usize = 4;
const LIVE_TAIL_FALLBACK_WINDOW_MS: f64 = 300_000.0;
const INITIAL_DISCOVERY_RETRY_DELAY: Duration = Duration::from_millis(100);
const HLS_BODY_ATTEMPTS: usize = 6;
const HLS_BODY_RETRY_DELAY_MS: u64 = 75;
const HLS_NEXT_RESERVE_STAGGER: Duration = Duration::from_secs(1);
const HLS_CODEC_BOOTSTRAP_BYTES: u64 = (320 * 1024 / 188) * 188;
const HISTORY_STRIDE: u64 = 10;
const HISTORY_WINDOW_BYTES: usize = 64 * 1024;
const HISTORY_MAX_PROBES: usize = 4_096;
const HISTORY_MAX_REPAIRS: usize = 4_096;
const HISTORY_BACKGROUND_PARALLEL: usize = 16;
const HISTORY_FOREGROUND_PARALLEL: usize = 64;

thread_local! {
    static RANGE_CACHE: RefCell<RangeCache> = RefCell::new(RangeCache::default());
    static FEED: RefCell<Option<FeedSession>> = const { RefCell::new(None) };
    static NEXT_FEED_ID: Cell<u64> = const { Cell::new(0) };
    static BEGINNING_MEDIA_READY: Cell<bool> = const { Cell::new(false) };
}

#[derive(Default)]
struct RangeCache {
    epoch: u64,
    ranges: HashMap<(String, u64, u64), Bytes>,
    order: VecDeque<(String, u64, u64)>,
    bodies: HashMap<String, Bytes>,
    body_order: VecDeque<String>,
    pending_bodies: HashMap<String, PendingBody>,
    bytes: u64,
}

struct PendingBody {
    epoch: u64,
    generation: Option<u64>,
    waiters: Vec<mpsc::Sender<Option<Bytes>>>,
}

enum BodyLoad {
    Cached(Bytes),
    Wait(mpsc::Receiver<Option<Bytes>>),
    Lead(u64),
}

impl RangeCache {
    fn get(&self, reference: &str, start: u64, end: u64) -> Option<Bytes> {
        let key = (reference.to_string(), start, end);
        self.ranges.get(&key).cloned().or_else(|| {
            let body = self.bodies.get(reference)?;
            let start = usize::try_from(start).ok()?;
            let end = usize::try_from(end).ok()?.checked_add(1)?;
            body.get(start..end)?;
            Some(body.slice(start..end))
        })
    }

    fn insert(
        &mut self,
        epoch: u64,
        reference: String,
        start: u64,
        end: u64,
        bytes: Bytes,
    ) -> bool {
        if self.epoch != epoch {
            return false;
        }
        let key = (reference, start, end);
        if let Some(previous) = self.ranges.insert(key.clone(), bytes.clone()) {
            self.bytes = self.bytes.saturating_sub(previous.len() as u64);
        } else {
            self.order.push_back(key);
        }
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        self.trim();
        true
    }

    fn body_load(&mut self, reference: &str, generation: Option<u64>) -> BodyLoad {
        if let Some(body) = self.bodies.get(reference) {
            return BodyLoad::Cached(body.clone());
        }
        if let Some(pending) = self.pending_bodies.get_mut(reference) {
            let (sender, receiver) = mpsc::bounded(1);
            pending.waiters.push(sender);
            return BodyLoad::Wait(receiver);
        }
        self.pending_bodies.insert(
            reference.to_string(),
            PendingBody {
                epoch: self.epoch,
                generation,
                waiters: Vec::new(),
            },
        );
        BodyLoad::Lead(self.epoch)
    }

    fn pending_body(&mut self, reference: &str) -> Option<mpsc::Receiver<Option<Bytes>>> {
        let waiters = &mut self.pending_bodies.get_mut(reference)?.waiters;
        let (sender, receiver) = mpsc::bounded(1);
        waiters.push(sender);
        Some(receiver)
    }

    fn body_cached(&self, reference: &str) -> bool {
        self.bodies.contains_key(reference)
    }

    fn body_ready_or_pending(&self, reference: &str) -> bool {
        self.bodies.contains_key(reference) || self.pending_bodies.contains_key(reference)
    }

    fn pending_body_count(&self, generation: u64) -> usize {
        self.pending_bodies
            .values()
            .filter(|pending| pending.generation == Some(generation))
            .count()
    }

    fn body(&self, reference: &str) -> Option<Bytes> {
        self.bodies.get(reference).cloned()
    }

    fn finish_body(&mut self, reference: String, epoch: u64, body: Option<Bytes>) -> Option<Bytes> {
        let matches = self
            .pending_bodies
            .get(&reference)
            .is_some_and(|pending| pending.epoch == epoch);
        if !matches {
            return None;
        };
        let pending = self.pending_bodies.remove(&reference)?;
        let current = pending.epoch == self.epoch;
        let delivered = current.then_some(body).flatten();
        if let Some(body) = &delivered {
            self.ranges.retain(|(cached, _, _), bytes| {
                if cached == &reference {
                    self.bytes = self.bytes.saturating_sub(bytes.len() as u64);
                    false
                } else {
                    true
                }
            });
            self.order.retain(|(cached, _, _)| cached != &reference);
            if let Some(previous) = self.bodies.insert(reference.clone(), body.clone()) {
                self.bytes = self.bytes.saturating_sub(previous.len() as u64);
            } else {
                self.body_order.push_back(reference);
            }
            self.bytes = self.bytes.saturating_add(body.len() as u64);
            self.trim();
        }
        for waiter in pending.waiters {
            let _ = waiter.try_send(delivered.clone());
        }
        delivered
    }

    fn trim(&mut self) {
        let maximum = media_cache_max_bytes()
            .saturating_sub(completed_media_range_bytes())
            .min(RANGE_CACHE_HARD_MAX_BYTES);
        while self.bytes > maximum {
            if let Some(key) = self.order.pop_front() {
                if let Some(bytes) = self.ranges.remove(&key) {
                    self.bytes = self.bytes.saturating_sub(bytes.len() as u64);
                }
            } else if let Some(reference) = self.body_order.pop_front() {
                if let Some(bytes) = self.bodies.remove(&reference) {
                    self.bytes = self.bytes.saturating_sub(bytes.len() as u64);
                }
            } else {
                break;
            }
        }
        set_auxiliary_media_cache_bytes(self.bytes);
    }

    fn clear(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        for (_, pending) in self.pending_bodies.drain() {
            for waiter in pending.waiters {
                let _ = waiter.try_send(None);
            }
        }
        self.ranges.clear();
        self.order.clear();
        self.bodies.clear();
        self.body_order.clear();
        self.bytes = 0;
        set_auxiliary_media_cache_bytes(0);
    }
}

struct FeedSession {
    id: u64,
    view_generation: u64,
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
    start: HlsStart,
    following: bool,
    live_runway_running: bool,
    live_startup_locked: bool,
    live_foreground: Option<String>,
    beginning_foreground_position: Option<usize>,
    index: Option<u64>,
    playlist: Option<HlsPlaylist>,
    terminal_candidate: Option<u64>,
    presentation_gaps: HashSet<(u64, String)>,
    tail_fallbacks: VecDeque<f64>,
    presentation_revision: u64,
}

struct RawFeedPayload {
    index: u64,
    lattice_residue: u64,
    bytes: Vec<u8>,
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
    let address = get_feed_address(owner, topic, index);
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
            lattice_residue: index % HISTORY_STRIDE,
            bytes,
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
    join_pending_body: bool,
) -> Option<Bytes> {
    let epoch = RANGE_CACHE.with(|cache| cache.borrow().epoch);
    if let Some(bytes) = RANGE_CACHE.with(|cache| cache.borrow().get(&reference, start, end)) {
        return Some(bytes);
    }
    if join_pending_body
        && let Some(waiter) = RANGE_CACHE.with(|cache| cache.borrow_mut().pending_body(&reference))
    {
        if let Some(body) = waiter.recv().await.ok().flatten() {
            let start = usize::try_from(start).ok()?;
            let end = usize::try_from(end).ok()?.checked_add(1)?;
            body.get(start..end)?;
            return Some(body.slice(start..end));
        }
    }
    let bytes =
        retrieve_data_range_from_root(root, start, end, encrypted, &client.chunk_port.0).await?;
    if bytes.len() as u64 != end.checked_sub(start)?.checked_add(1)? {
        return None;
    }
    let bytes = Bytes::from(bytes);
    RANGE_CACHE
        .with(|cache| {
            cache
                .borrow_mut()
                .insert(epoch, reference, start, end, bytes.clone())
        })
        .then_some(bytes)
}

async fn hls_body(client: Arc<Weeb3>, reference: String, generation: Option<u64>) -> Option<Bytes> {
    let epoch = match RANGE_CACHE.with(|cache| cache.borrow_mut().body_load(&reference, generation))
    {
        BodyLoad::Cached(body) => return Some(body),
        BodyLoad::Wait(waiter) => return waiter.recv().await.ok().flatten(),
        BodyLoad::Lead(epoch) => epoch,
    };
    let body = async {
        let decoded = hex::decode(&reference).ok()?;
        let encrypted = decoded.len() == 64;
        let root = retrieve_decoded_data_root(&decoded, &client.chunk_port.0).await?;
        if root.span == 0 || root.span > RANGE_CACHE_HARD_MAX_BYTES {
            return None;
        }
        let end = root.span.checked_sub(1)?;
        let body =
            retrieve_data_range_from_root(root, 0, end, encrypted, &client.chunk_port.0).await?;
        (body.len() as u64 == end + 1).then(|| Bytes::from(body))
    }
    .await;
    RANGE_CACHE.with(|cache| cache.borrow_mut().finish_body(reference, epoch, body))
}

async fn foreground_hls_body(
    client: Arc<Weeb3>,
    reference: String,
    generation: Option<u64>,
) -> Option<Bytes> {
    for attempt in 0..HLS_BODY_ATTEMPTS {
        if let Some(body) = hls_body(client.clone(), reference.clone(), generation).await {
            return Some(body);
        }
        if attempt + 1 < HLS_BODY_ATTEMPTS {
            async_std::task::sleep(Duration::from_millis(
                HLS_BODY_RETRY_DELAY_MS * (attempt + 1) as u64,
            ))
            .await;
        }
    }
    None
}

fn prefetch_bodies(client: Arc<Weeb3>, references: Vec<String>, generation: Option<u64>) {
    spawn_local(async move {
        for (offset, reference) in references
            .into_iter()
            .take(BODY_PREFETCH_HORIZON)
            .enumerate()
        {
            if offset != 0 {
                async_std::task::sleep(HLS_NEXT_RESERVE_STAGGER).await;
            }
            let client = client.clone();
            spawn_local(async move {
                let _ = hls_body(client, reference, generation).await;
            });
        }
    });
}

fn prefetch_priority_runway(
    client: Arc<Weeb3>,
    mut references: Vec<String>,
    start: HlsStart,
    generation: Option<u64>,
    head_ready: bool,
) {
    references.truncate(BODY_PREFETCH_HORIZON);
    if references.is_empty() {
        return;
    }
    if start == HlsStart::Live {
        prefetch_bodies(client, references, generation);
        return;
    }
    for (offset, reference) in references.into_iter().skip(1).enumerate() {
        let client = client.clone();
        spawn_local(async move {
            async_std::task::sleep(Duration::from_secs(offset as u64 + u64::from(!head_ready)))
                .await;
            let _ = hls_body(client, reference, None).await;
        });
    }
}

fn prefetch_playlist_runway(
    client: Arc<Weeb3>,
    playlist: &HlsPlaylist,
    start: HlsStart,
    generation: Option<u64>,
) -> Vec<String> {
    let mut references: Vec<String> = match start {
        HlsStart::Beginning => playlist
            .segments
            .iter()
            .filter(|segment| !segment.gap)
            .take(BODY_PREFETCH_HORIZON)
            .map(|segment| segment.reference.clone())
            .collect(),
        HlsStart::Live => playlist
            .segments
            .iter()
            .rev()
            .filter(|segment| !segment.gap)
            .take(HLS_LIVE_EDGE_SEGMENTS)
            .map(|segment| segment.reference.clone())
            .collect(),
    };
    if start == HlsStart::Live {
        references.reverse();
    }
    prefetch_priority_runway(client, references.clone(), start, generation, false);
    references
}

#[rustfmt::skip]
fn live_segment_is_playable(active: &FeedSession, position: usize) -> bool {
    let Some(playlist) = active.playlist.as_ref() else { return false };
    let Some(segment) = playlist.segments.get(position) else { return false };
    let Some(sequence) = u64::try_from(position).ok().and_then(|offset| playlist.sequence.checked_add(offset)) else { return false };
    !segment.gap && !active.presentation_gaps.iter().any(|(gap, reference)| *gap == sequence && reference == &segment.reference)
}

fn latest_live_foreground(active: &FeedSession) -> Option<String> {
    active
        .playlist
        .as_ref()?
        .segments
        .iter()
        .enumerate()
        .rev()
        .filter(|(position, _)| live_segment_is_playable(active, *position))
        .nth(HLS_LIVE_EDGE_SEGMENTS.saturating_sub(1))
        .map(|(_, segment)| segment.reference.clone())
}

fn live_runway_targets(active: &FeedSession) -> Vec<String> {
    let Some(playlist) = active.playlist.as_ref() else {
        return Vec::new();
    };
    let foreground = active.live_foreground.as_deref().and_then(|reference| {
        playlist
            .segments
            .iter()
            .enumerate()
            .rfind(|(position, segment)| {
                live_segment_is_playable(active, *position) && segment.reference == reference
            })
            .map(|(position, _)| position)
    });
    let fallback = || {
        let reference = latest_live_foreground(active)?;
        playlist
            .segments
            .iter()
            .rposition(|segment| segment.reference == reference)
    };
    let Some(position) = foreground.or_else(fallback) else {
        return Vec::new();
    };
    playlist.segments[position..]
        .iter()
        .enumerate()
        .filter(|(offset, _)| live_segment_is_playable(active, position + offset))
        .map(|(_, segment)| segment)
        .map(|segment| segment.reference.clone())
        .take(HLS_LIVE_BODY_RUNWAY_SEGMENTS)
        .collect()
}

pub(crate) fn lock_live_startup_plan() -> Option<super::HlsStartupPlan> {
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed
            .as_mut()
            .filter(|active| active.start == HlsStart::Live)?;
        let plan = active.playlist.as_ref()?.startup_plan(HlsStart::Live)?;
        if !active.live_startup_locked {
            active.live_foreground = latest_live_foreground(active);
            active.live_startup_locked = true;
        }
        Some(plan)
    })
}

fn live_runway_context(id: u64) -> Option<(Arc<Weeb3>, Vec<String>)> {
    FEED.with(|feed| {
        let feed = feed.borrow();
        let active = feed
            .as_ref()
            .filter(|active| active.id == id && active.start == HlsStart::Live)?;
        Some((active.client.clone(), live_runway_targets(active)))
    })
}

fn spawn_live_runway(id: u64) {
    let claimed = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let Some(active) = feed
            .as_mut()
            .filter(|active| active.id == id && active.start == HlsStart::Live)
        else {
            return false;
        };
        if active.live_runway_running {
            return false;
        }
        active.live_runway_running = true;
        true
    });
    if !claimed {
        return;
    }
    spawn_local(async move {
        loop {
            let Some((client, references)) = live_runway_context(id) else {
                return;
            };
            if references.is_empty() {
                FEED.with(|feed| {
                    if let Some(active) =
                        feed.borrow_mut().as_mut().filter(|active| active.id == id)
                    {
                        active.live_runway_running = false;
                    }
                });
                return;
            }
            let complete = references
                .iter()
                .all(|reference| RANGE_CACHE.with(|cache| cache.borrow().body_cached(reference)));
            if complete && live_runway_context(id).is_some_and(|(_, current)| current == references)
            {
                FEED.with(|feed| {
                    if let Some(active) =
                        feed.borrow_mut().as_mut().filter(|active| active.id == id)
                    {
                        active.live_runway_running = false;
                    }
                });
                return;
            }
            let mut stagger = false;
            for reference in references.iter().cloned() {
                let owned =
                    RANGE_CACHE.with(|cache| cache.borrow().body_ready_or_pending(&reference));
                if owned {
                    continue;
                }
                if stagger {
                    async_std::task::sleep(HLS_NEXT_RESERVE_STAGGER).await;
                    if !live_runway_context(id).is_some_and(|(_, current)| current == references) {
                        break;
                    }
                }
                let available = RANGE_CACHE.with(|cache| {
                    let cache = cache.borrow();
                    !cache.body_ready_or_pending(&reference)
                        && cache.pending_body_count(id) < HLS_BODY_PREFETCH_MAX_PARALLEL
                });
                if available {
                    let client = client.clone();
                    spawn_local(async move {
                        let _ = hls_body(client, reference, Some(id)).await;
                    });
                    stagger = true;
                }
            }
            async_std::task::sleep(Duration::from_millis(25)).await;
        }
    });
}

fn prefetch_from_reference(reference: &str, cached: bool) -> (Option<u64>, Option<String>) {
    let runway = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.as_mut()?;
        let playlist = active.playlist.as_ref()?;
        let matches = |segment: &super::HlsSegment| !segment.gap && segment.reference == reference;
        if active.start == HlsStart::Live {
            playlist
                .segments
                .iter()
                .enumerate()
                .rfind(|(position, segment)| {
                    live_segment_is_playable(active, *position) && matches(segment)
                })?;
            active.live_foreground = Some(reference.to_string());
            return Some((active.id, None));
        }
        let playable = playlist.segments.iter().filter(|segment| !segment.gap);
        let position = playable.clone().position(matches)?;
        let references = playable
            .skip(position)
            .map(|segment| segment.reference.clone())
            .take(BODY_PREFETCH_HORIZON)
            .collect::<Vec<_>>();
        let (transition, position) = active
            .beginning_foreground_position
            .map_or((false, position), |last| {
                hls_progressive_foreground_transition(last, position, cached)
            });
        active.beginning_foreground_position = Some(position);
        let successor = transition.then(|| references.get(1)).flatten().cloned();
        Some((
            active.id,
            Some((active.client.clone(), references, successor)),
        ))
    });
    let Some((id, beginning)) = runway else {
        return (None, None);
    };
    if let Some((client, references, successor)) = beginning {
        prefetch_priority_runway(client, references, HlsStart::Beginning, None, cached);
        (None, successor)
    } else {
        spawn_live_runway(id);
        (Some(id), None)
    }
}

fn next_feed_id() -> u64 {
    NEXT_FEED_ID.with(|next| {
        let id = next.get().wrapping_add(1).max(1);
        next.set(id);
        id
    })
}

fn begin_feed(
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
    start: HlsStart,
    view_generation: u64,
) -> u64 {
    let id = next_feed_id();
    BEGINNING_MEDIA_READY.with(|ready| ready.set(false));
    FEED.with(|feed| {
        *feed.borrow_mut() = Some(FeedSession {
            id,
            view_generation,
            client,
            owner,
            topic,
            start,
            following: false,
            live_runway_running: false,
            live_startup_locked: false,
            live_foreground: None,
            beginning_foreground_position: None,
            index: None,
            playlist: None,
            terminal_candidate: None,
            presentation_gaps: HashSet::new(),
            tail_fallbacks: VecDeque::new(),
            presentation_revision: 0,
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
    let terminal = playlist.finalized;
    playlist.finalized = false;
    let live = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.as_mut().filter(|active| active.id == id)?;
        (active.index, active.playlist) = (Some(index), Some(playlist));
        active.terminal_candidate = terminal.then_some(index);
        active.presentation_gaps.clear();
        active.tail_fallbacks.clear();
        active.presentation_revision = 0;
        if active.start == HlsStart::Live {
            active.live_startup_locked = false;
            active.live_foreground = latest_live_foreground(active);
        }
        Some(active.start == HlsStart::Live)
    })?;
    if live {
        spawn_live_runway(id);
    }
    Some(())
}

async fn discover_beginning(
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
        let best = Rc::new(RefCell::new(None::<(usize, RawFeedPayload)>));
        let collected_best = best.clone();
        let collect = async {
            while let Ok(probe) = input.recv().await {
                if let FeedPayloadProbe::Found(payload) = probe
                    && let Some(playlist) = HlsPlaylist::parse(&payload.bytes)
                    && playlist.sequence == 0
                    && playlist.startup_plan(HlsStart::Beginning).is_some()
                {
                    let playable = playlist
                        .segments
                        .iter()
                        .filter(|segment| !segment.gap)
                        .count();
                    if playable >= BEGINNING_PREFIX_TARGET_SEGMENTS {
                        return Some(payload);
                    }
                    let replace =
                        collected_best
                            .borrow()
                            .as_ref()
                            .is_none_or(|(count, current)| {
                                (playable, payload.index) > (*count, current.index)
                            });
                    if replace {
                        *collected_best.borrow_mut() = Some((playable, payload));
                    }
                }
                if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
                    return None;
                }
            }
            None
        };
        let mut collect = Box::pin(collect);
        match async_std::future::timeout(BEGINNING_WAVE_TIMEOUT, collect.as_mut()).await {
            Ok(Some(payload)) => return Some(payload),
            Ok(None) => {}
            Err(_) if best.borrow().is_none() => {
                if let Some(payload) = collect.await {
                    return Some(payload);
                }
            }
            Err(_) => {}
        }
        if let Some((_, payload)) = best.borrow_mut().take() {
            return Some(payload);
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
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
    let mut deadline = js_sys::Date::now()
        + if lower_is_known {
            EDGE_WAVE_TIMEOUT
        } else {
            EDGE_COLD_WAVE_TIMEOUT
        }
        .as_millis() as f64;
    let mut positive_seen = lower_is_known;
    while completed.iter().any(|settled| !settled) {
        let remaining = (deadline - js_sys::Date::now()).ceil();
        if remaining <= 0.0 {
            break;
        }
        let next = async_std::future::timeout(
            Duration::from_millis(remaining.min(u64::MAX as f64) as u64),
            input.recv(),
        )
        .await;
        let Ok(Ok((slot, result))) = next else {
            break;
        };
        completed[slot] = true;
        match result {
            FeedProbe::Found(update) => {
                found[slot] = Some(update);
                if !positive_seen {
                    positive_seen = true;
                    deadline = js_sys::Date::now() + EDGE_WAVE_TIMEOUT.as_millis() as f64;
                }
            }
            FeedProbe::Missing => missing[slot] = true,
            FeedProbe::Transient => {}
        }
        let first_unsettled = found
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |found| found + 1);
        if lower_is_known
            && let Some(upper) = (first_unsettled..indices.len()).find(|slot| missing[*slot])
            && completed[first_unsettled..=upper]
                .iter()
                .all(|settled| *settled)
        {
            break;
        }
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
    retrieve_confirmed_payload(client, owner, topic, index, update).await
}

async fn settled_update_wave(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    indices: &[u64],
) -> Vec<(u64, FeedProbe<Vec<u8>>)> {
    let (results, input) = mpsc::unbounded();
    for index in indices.iter().copied() {
        let client = client.clone();
        let owner = owner.to_string();
        let topic = topic.to_string();
        let results = results.clone();
        spawn_local(async move {
            let result =
                probe_feed_update(&client, &owner, &topic, index, Some(FEED_PROBE_ATTEMPTS)).await;
            let _ = results.try_send((index, result));
        });
    }
    drop(results);
    let mut settled = Vec::with_capacity(indices.len());
    while let Ok(result) = input.recv().await {
        settled.push(result);
    }
    settled.sort_by_key(|(index, _)| *index);
    settled
}

async fn retrieve_confirmed_payload(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    mut index: u64,
    mut update: Vec<u8>,
) -> Option<RawFeedPayload> {
    let lattice_residue = index % HISTORY_STRIDE;
    loop {
        let Some(first_guard) = index.checked_add(HISTORY_STRIDE) else {
            break;
        };
        let Some(second_guard) = first_guard.checked_add(HISTORY_STRIDE) else {
            break;
        };
        let guards = settled_update_wave(client, owner, topic, &[first_guard, second_guard]).await;
        let guard_transient = guards
            .iter()
            .any(|(_, probe)| matches!(probe, FeedProbe::Transient));
        if let Some((next, next_update)) = guards
            .into_iter()
            .filter_map(|(index, probe)| match probe {
                FeedProbe::Found(update) => Some((index, update)),
                FeedProbe::Missing | FeedProbe::Transient => None,
            })
            .max_by_key(|(index, _)| *index)
        {
            (index, update) = (next, next_update);
            continue;
        }

        let dense = (1..HISTORY_STRIDE * 2)
            .filter(|offset| *offset != HISTORY_STRIDE)
            .map(|offset| index.checked_add(offset))
            .collect::<Option<Vec<_>>>()?;
        let dense = settled_update_wave(client, owner, topic, &dense).await;
        let mut transient = false;
        let newest = dense
            .into_iter()
            .filter_map(|(index, probe)| match probe {
                FeedProbe::Found(update) => Some((index, update)),
                FeedProbe::Transient => {
                    transient = true;
                    None
                }
                FeedProbe::Missing => None,
            })
            .max_by_key(|(index, _)| *index);
        if let Some((next, next_update)) = newest {
            (index, update) = (next, next_update);
            continue;
        }
        if guard_transient || transient {
            return None;
        }
        break;
    }
    let root = decode_feed_payload_root(index, update)?;
    let bytes =
        retrieve_feed_payload(&root, MAX_STREAM_FEED_PAYLOAD_BYTES, &client.chunk_port.0).await?;
    Some(RawFeedPayload {
        index,
        lattice_residue,
        bytes,
    })
}

fn payload_probe_wave(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    indices: &[u64],
    attempt_limit: Option<usize>,
) -> mpsc::Receiver<(usize, u64, FeedPayloadProbe)> {
    let (results, input) = mpsc::unbounded();
    for (slot, index) in indices.iter().copied().enumerate() {
        let client = client.clone();
        let owner = owner.to_string();
        let topic = topic.to_string();
        let results = results.clone();
        spawn_local(async move {
            let result = probe_feed_payload(
                &client,
                &owner,
                &topic,
                index,
                FEED_TAIL_PROBE_BYTES,
                attempt_limit,
            )
            .await;
            let _ = results.try_send((slot, index, result));
        });
    }
    drop(results);
    input
}

async fn settled_payload_wave(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    indices: &[u64],
) -> Vec<(u64, FeedPayloadProbe)> {
    let input = payload_probe_wave(client, owner, topic, indices, Some(FEED_PROBE_ATTEMPTS));
    let mut settled = Vec::with_capacity(indices.len());
    while let Ok((_, index, result)) = input.recv().await {
        settled.push((index, result));
    }
    settled.sort_by_key(|(index, _)| *index);
    settled
}

async fn merge_history_probe(
    client: &Arc<Weeb3>,
    history: &mut HlsPlaylist,
    probe: FeedPayloadProbe,
) -> Option<(u64, usize)> {
    match probe {
        FeedPayloadProbe::Found(payload) => {
            let playlist = HlsPlaylist::parse(&payload.bytes)?;
            history
                .merge_playlist(playlist)
                .map(|appended| (payload.index, appended))
        }
        FeedPayloadProbe::Deferred(root) => {
            let index = root.index;
            if let Some(tail) =
                retrieve_feed_payload_tail(&root, FEED_TAIL_PROBE_BYTES, &client.chunk_port.0).await
                && let Some(appended) = history.merge_tail(&tail)
            {
                return Some((index, appended));
            }
            let body =
                retrieve_feed_payload(&root, MAX_STREAM_FEED_PAYLOAD_BYTES, &client.chunk_port.0)
                    .await?;
            history
                .merge_playlist(HlsPlaylist::parse(&body)?)
                .map(|appended| (index, appended))
        }
        FeedPayloadProbe::Missing | FeedPayloadProbe::Transient => None,
    }
}

async fn catch_up_history(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    mut index: u64,
    history: &mut HlsPlaylist,
) -> Result<u64, &'static str> {
    let current = || feed_is_current(id) && result_view_request_is_current(view_generation);
    loop {
        if !current() {
            return Err("HLS open was superseded.");
        }
        let indices = (1..=FEED_FOLLOW_AHEAD)
            .map(|offset| index.checked_add(offset))
            .collect::<Option<Vec<_>>>()
            .ok_or("The HLS feed index overflowed.")?;
        let wave = settled_payload_wave(client, owner, topic, &indices).await;
        let all_missing = wave
            .iter()
            .all(|(_, probe)| matches!(probe, FeedPayloadProbe::Missing));
        let had_positive = wave.iter().any(|(_, probe)| {
            matches!(
                probe,
                FeedPayloadProbe::Found(_) | FeedPayloadProbe::Deferred(_)
            )
        });
        let mut advanced = false;
        for (_, probe) in wave {
            if let Some((candidate, _)) = merge_history_probe(client, history, probe).await {
                index = candidate;
                advanced = true;
            }
        }
        if advanced {
            continue;
        }
        if all_missing {
            return current().then_some(index).ok_or("HLS open was superseded.");
        }
        if had_positive {
            return Err("A captured HLS update did not overlap its predecessor.");
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
}

#[rustfmt::skip]
async fn warm_codec_bootstrap(client: &Arc<Weeb3>, id: u64, playlist: &HlsPlaylist) {
    let Some(reference) = playlist.segments.iter().find(|segment| !segment.gap).map(|segment| segment.reference.clone()) else { return };
    if !feed_is_current(id) { return }
    let Ok(decoded) = hex::decode(&reference) else { return };
    let encrypted = decoded.len() == 64;
    let Some(root) = retrieve_decoded_data_root(&decoded, &client.chunk_port.0).await else { return };
    if !feed_is_current(id) || root.span == 0 { return }
    let prefix_end = root.span.saturating_sub(1).min(188);
    let Some(prefix) = hls_range(client.clone(), reference.clone(), root.clone(), encrypted, 0, prefix_end, false).await else { return };
    if !feed_is_current(id) || hls_payload_mime(&prefix) != "video/mp2t" { return }
    let end = root.span.min(HLS_CODEC_BOOTSTRAP_BYTES).saturating_sub(1);
    let _ = hls_range(client.clone(), reference, root, encrypted, 0, end, false).await;
}

async fn history_snapshots(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    indices: &[u64],
    codec_feed: Option<u64>,
    parallel: usize,
) -> Vec<(u64, HlsPlaylist)> {
    stream::iter(indices.iter().copied())
        .map(|index| async move {
            if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
                return None;
            }
            let bytes = match probe_feed_payload(
                client,
                owner,
                topic,
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
            let playlist = HlsPlaylist::parse(&bytes)?;
            if let Some(id) =
                codec_feed.filter(|_| index < HISTORY_STRIDE && playlist.sequence == 0)
            {
                warm_codec_bootstrap(client, id, &playlist).await;
            }
            Some((index, playlist))
        })
        .buffer_unordered(parallel)
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
    let mut ordered = snapshots
        .iter()
        .map(|(index, playlist)| (*index, playlist))
        .collect::<Vec<_>>();
    ordered.push((head_index, head));
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
    let (first_index, first) = ordered.first().copied()?;
    if first.sequence != 0 {
        add(0..first_index)?;
    }
    for pair in ordered.windows(2) {
        if !pair[0].1.joins(&pair[1].1) {
            add(pair[0].0.saturating_add(1)..pair[1].0)?;
        }
    }
    Some(repairs)
}

async fn hls_history(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    head_index: u64,
    lattice_residue: u64,
    head_bytes: &[u8],
    codec_feed: Option<u64>,
) -> Option<HlsPlaylist> {
    let head = HlsPlaylist::parse(head_bytes)?;
    if head.sequence == 0 {
        return Some(head);
    }
    let indices = (lattice_residue..head_index)
        .step_by(usize::try_from(HISTORY_STRIDE).ok()?)
        .collect::<Vec<_>>();
    if indices.is_empty() || indices.len() > HISTORY_MAX_PROBES {
        return None;
    }
    let parallel = if codec_feed.is_some() {
        HISTORY_FOREGROUND_PARALLEL
    } else {
        HISTORY_BACKGROUND_PARALLEL
    };
    #[rustfmt::skip]
    let mut snapshots = history_snapshots(id, view_generation, client, owner, topic, &indices, codec_feed, parallel).await;
    if let Some(history) = HlsPlaylist::reconstruct(snapshots.clone(), head_index, head.clone()) {
        return Some(history);
    }
    let repairs = history_repairs(&indices, &snapshots, head_index, &head)?;
    #[rustfmt::skip]
    snapshots.extend(history_snapshots(id, view_generation, client, owner, topic, &repairs, None, parallel).await);
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
) -> Option<(u64, HlsPlaylist)> {
    loop {
        let payload = discover_raw_for_view(
            id,
            view_generation,
            client.clone(),
            owner.clone(),
            topic.clone(),
        )
        .await?;
        if let Some(history) = hls_history(
            id,
            view_generation,
            &client,
            &owner,
            &topic,
            payload.index,
            payload.lattice_residue,
            &payload.bytes,
            None,
        )
        .await
        {
            return Some((payload.index, history));
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
}

fn render_active_feed(
    owner: &str,
    topic: &str,
    start: HlsStart,
    local_bytes_base: &str,
) -> Option<(u64, Vec<u8>, Option<u64>, u64)> {
    FEED.with(|feed| {
        let feed = feed.borrow();
        let feed = feed.as_ref()?;
        if feed.owner != owner || feed.topic != topic {
            return None;
        }
        let index = feed.index?;
        let playlist = feed.playlist.as_ref()?;
        let body = if start == HlsStart::Live && !feed.presentation_gaps.is_empty() {
            let mut presentation = playlist.clone();
            for (sequence, reference) in &feed.presentation_gaps {
                presentation.mark_gap(*sequence, reference);
            }
            presentation.render(local_bytes_base, start)
        } else {
            playlist.render(local_bytes_base, start)
        };
        let follower =
            (start == HlsStart::Live && !playlist.finalized && !feed.following).then_some(feed.id);
        Some((index, body, follower, feed.presentation_revision))
    })
}

fn apply_update(
    id: u64,
    index: u64,
    merge: impl FnOnce(&mut HlsPlaylist) -> Option<usize>,
) -> Option<usize> {
    let updated = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.as_mut().filter(|active| active.id == id)?;
        if active.index.is_some_and(|current| index <= current) {
            return None;
        }
        let (appended, terminal) = {
            let playlist = active.playlist.as_mut()?;
            let appended = merge(playlist)?;
            let terminal = playlist.finalized;
            playlist.finalized = false;
            (appended, terminal)
        };
        active.index = Some(index);
        active.terminal_candidate = terminal.then_some(index);
        if active.start == HlsStart::Live && !active.live_startup_locked {
            active.live_foreground = latest_live_foreground(active);
        }
        Some((appended, active.start == HlsStart::Live))
    })?;
    if updated.0 != 0 && updated.1 {
        spawn_live_runway(id);
    }
    Some(updated.0)
}

fn apply_full_update(id: u64, index: u64, candidate: HlsPlaylist) -> Option<usize> {
    apply_update(id, index, |playlist| playlist.merge_playlist(candidate))
}

fn confirm_terminal(id: u64, index: u64, candidate: HlsPlaylist) -> bool {
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let Some(active) = feed.as_mut().filter(|active| {
            active.id == id
                && active.index == Some(index)
                && active.terminal_candidate == Some(index)
                && candidate.finalized
        }) else {
            return false;
        };
        let merged = active
            .playlist
            .as_mut()
            .and_then(|playlist| playlist.merge_playlist(candidate))
            .is_some();
        let confirmed = merged
            && active
                .playlist
                .as_ref()
                .is_some_and(|playlist| playlist.finalized);
        if confirmed {
            active.terminal_candidate = None;
        }
        confirmed
    })
}

fn live_tail_position(active: &FeedSession, sequence: u64, reference: &str) -> Option<usize> {
    let playlist = active.playlist.as_ref()?;
    let position = sequence
        .checked_sub(playlist.sequence)
        .and_then(|position| usize::try_from(position).ok())?;
    let segment = playlist.segments.get(position)?;
    if !live_segment_is_playable(active, position) || segment.reference != reference {
        return None;
    }
    playlist
        .segments
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, segment)| !segment.gap)
        .take(HLS_LIVE_EDGE_SEGMENTS)
        .any(|(candidate, _)| candidate == position)
        .then_some(position)
}

pub(crate) fn live_tail_failure_identity(
    sequence: u64,
    reference: &str,
) -> Option<(u64, u64, String)> {
    FEED.with(|feed| {
        let feed = feed.borrow();
        let active = feed
            .as_ref()
            .filter(|active| active.start == HlsStart::Live)?;
        live_tail_position(active, sequence, reference)?;
        Some((active.index?, sequence, reference.to_string()))
    })
}

pub(crate) fn install_live_tail_fallback(
    snapshot: u64,
    sequence: u64,
    reference: &str,
) -> Option<f64> {
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.as_mut().filter(|feed| feed.start == HlsStart::Live)?;
        if active.index != Some(snapshot)
            || live_tail_position(active, sequence, reference).is_none()
        {
            return None;
        }
        let playlist = active.playlist.as_ref()?;
        let failed = usize::try_from(sequence.checked_sub(playlist.sequence)?).ok()?;
        let retreat = (0..failed)
            .rev()
            .find(|position| live_segment_is_playable(active, *position))?;
        let target = playlist.segments[..retreat]
            .iter()
            .map(|segment| segment.duration)
            .sum::<f64>();
        let now = js_sys::Date::now();
        while active
            .tail_fallbacks
            .front()
            .is_some_and(|at| now - at >= LIVE_TAIL_FALLBACK_WINDOW_MS)
        {
            active.tail_fallbacks.pop_front();
        }
        if !target.is_finite()
            || active.tail_fallbacks.len() >= LIVE_TAIL_FALLBACK_LIMIT
            || !active
                .presentation_gaps
                .insert((sequence, reference.to_string()))
        {
            return None;
        }
        active.tail_fallbacks.push_back(now);
        active.presentation_revision = active.presentation_revision.wrapping_add(1).max(1);
        Some(target)
    })
}

fn feed_follow_context(id: u64) -> Option<(Arc<Weeb3>, String, String, u64, u64)> {
    FEED.with(|feed| {
        let feed = feed.borrow();
        let feed = feed.as_ref().filter(|feed| feed.id == id)?;
        if feed.playlist.as_ref()?.finalized {
            return None;
        }
        Some((
            feed.client.clone(),
            feed.owner.clone(),
            feed.topic.clone(),
            feed.index?,
            feed.view_generation,
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
        if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
            return;
        }
        spawn_follower(id);
        let history = discover_for_view(
            id,
            view_generation,
            client.clone(),
            owner.clone(),
            topic.clone(),
        )
        .await;
        if let Some((index, history)) = history
            && feed_is_current(id)
            && result_view_request_is_current(view_generation)
            && apply_full_update(id, index, history).is_some()
        {
            client.interface_log(format!("HLS history reached index {index}"));
        }
    });
}

#[rustfmt::skip]
pub(crate) fn start_beginning_history() { BEGINNING_MEDIA_READY.with(|ready| ready.set(true)); }

fn spawn_follower(id: u64) {
    let claimed = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let Some(active) = feed
            .as_mut()
            .filter(|active| active.id == id && !active.following)
        else {
            return false;
        };
        active.following = true;
        true
    });
    if !claimed {
        return;
    }
    spawn_local(async move {
        let mut last_frontier_check = js_sys::Date::now();
        loop {
            let Some((client, owner, topic, head, _)) = feed_follow_context(id) else {
                return;
            };
            let mut progressed = false;
            let mut skipped_missing_index = false;
            for offset in 1..=FEED_FOLLOW_AHEAD {
                let Some(index) = head.checked_add(offset) else {
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
                    FeedPayloadProbe::Deferred(root) => {
                        apply_deferred_update(id, &client, root).await
                    }
                    FeedPayloadProbe::Missing | FeedPayloadProbe::Transient => {
                        if skipped_missing_index {
                            break;
                        }
                        skipped_missing_index = true;
                        continue;
                    }
                };
                let Some(appended) = appended else {
                    break;
                };
                progressed = true;
                if appended != 0 {
                    client.interface_log(format!(
                        "HLS feed advanced to {index}; appended {appended} segment(s)"
                    ));
                }
            }
            if progressed {
                last_frontier_check = js_sys::Date::now();
                continue;
            }

            async_std::task::sleep(FEED_POLL_INTERVAL).await;
            if !feed_is_current(id) {
                return;
            }

            let now = js_sys::Date::now();
            if now - last_frontier_check >= FEED_FRONTIER_REFRESH_INTERVAL {
                last_frontier_check = now;
                if recover_feed_frontier(id, &client, &owner, &topic).await {
                    continue;
                }
            }
        }
    });
}

async fn recover_feed_frontier(id: u64, client: &Arc<Weeb3>, owner: &str, topic: &str) -> bool {
    let Some((_, _, _, head, view_generation)) = feed_follow_context(id) else {
        return false;
    };
    let Some(payload) = discover_latest_once(client, owner, topic).await else {
        return false;
    };
    let index = payload.index;
    if index == head {
        let confirmed = HlsPlaylist::parse(&payload.bytes)
            .is_some_and(|playlist| confirm_terminal(id, index, playlist));
        if confirmed {
            client.interface_log(format!("HLS feed finalized at {index}"));
        }
        return confirmed;
    }
    if index < head {
        return false;
    }
    let appended = if let Some(appended) = HlsPlaylist::parse(&payload.bytes)
        .and_then(|playlist| apply_full_update(id, index, playlist))
    {
        appended
    } else {
        let Some(history) = hls_history(
            id,
            view_generation,
            client,
            owner,
            topic,
            index,
            payload.lattice_residue,
            &payload.bytes,
            None,
        )
        .await
        else {
            return false;
        };
        let Some(appended) = apply_full_update(id, index, history) else {
            return false;
        };
        appended
    };
    if appended != 0 {
        client.interface_log(format!("HLS frontier recovered at {index}"));
    }
    true
}

fn cached_hls_body_response(
    reference: &str,
    codec_bootstrap: bool,
    method: &str,
    range: Option<&str>,
    etag: String,
) -> Option<FetchResponse> {
    let body = RANGE_CACHE.with(|cache| cache.borrow().body(reference))?;
    let span = body.len() as u64;
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
    Some(if let Some(range) = range {
        let (start, end) = parse_hls_range(range, span)?;
        let slice_start = usize::try_from(start).ok()?;
        let slice_end = usize::try_from(end).ok()?.checked_add(1)?;
        body.get(slice_start..slice_end)?;
        headers.push((
            "Content-Length".to_string(),
            (slice_end - slice_start).to_string(),
        ));
        headers.push((
            "Content-Range".to_string(),
            format!("bytes {start}-{end}/{span}"),
        ));
        if method == "HEAD" {
            FetchResponse::ok(206, headers, None)
        } else {
            FetchResponse::ok_shared_slice(206, headers, body, slice_start, slice_end)?
        }
    } else {
        let mime = codec_bootstrap
            .then(|| hls_payload_mime(&body))
            .unwrap_or("application/octet-stream");
        headers[0].1 = mime.to_string();
        let response_span = if codec_bootstrap && mime == "video/mp2t" {
            span.min(HLS_CODEC_BOOTSTRAP_BYTES)
        } else {
            span
        };
        headers.push(("Content-Length".to_string(), response_span.to_string()));
        if method == "HEAD" {
            FetchResponse::ok(200, headers, None)
        } else if response_span == span {
            FetchResponse::ok_shared(200, headers, body)
        } else {
            FetchResponse::ok_shared_slice(
                200,
                headers,
                body,
                0,
                usize::try_from(response_span).ok()?,
            )?
        }
    })
}

async fn fetch_hls_body_response(
    client: Arc<Weeb3>,
    reference: String,
    codec_bootstrap: bool,
    method: &str,
    range: Option<&str>,
    if_none_match: Option<&str>,
    live: bool,
) -> FetchResponse {
    let etag = format!("\"{reference}\"");
    if if_none_match_matches(if_none_match, &etag) {
        return FetchResponse::ok(304, vec![("ETag".to_string(), etag)], None);
    }
    let whole_media_get = method == "GET" && range.is_none() && !codec_bootstrap;
    let cached =
        whole_media_get && RANGE_CACHE.with(|cache| cache.borrow().body_cached(&reference));
    let (body_generation, seek_successor) = whole_media_get
        .then(|| prefetch_from_reference(&reference, cached))
        .unwrap_or_default();
    if let Some(successor) = seek_successor {
        if foreground_hls_body(client.clone(), reference.clone(), None)
            .await
            .is_none()
        {
            return FetchResponse::error(503, "HLS segment body was unavailable");
        }
        let _ = hls_body(client.clone(), successor, None).await;
        return cached_hls_body_response(&reference, false, method, None, etag)
            .unwrap_or_else(|| FetchResponse::error(503, "HLS segment body was unavailable"));
    }
    if let Some(response) =
        cached_hls_body_response(&reference, codec_bootstrap, method, range, etag.clone())
    {
        return response;
    }
    if live && method == "GET" && range.is_none() && !codec_bootstrap {
        if foreground_hls_body(client.clone(), reference.clone(), body_generation)
            .await
            .is_none()
        {
            return FetchResponse::error(503, "HLS segment body was unavailable");
        }
        return cached_hls_body_response(&reference, false, method, None, etag)
            .unwrap_or_else(|| FetchResponse::error(503, "HLS segment body was unavailable"));
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
        let Some(bytes) = hls_range(client, reference, root, encrypted, start, end, !live).await
        else {
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

    let mime = if codec_bootstrap {
        let prefix_end = span.saturating_sub(1).min(188);
        hls_range(client, reference, root, encrypted, 0, prefix_end, !live)
            .await
            .map_or("application/octet-stream", |prefix| {
                hls_payload_mime(&prefix)
            })
    } else {
        "application/octet-stream"
    };
    headers[0].1 = mime.to_string();
    let response_span = if codec_bootstrap && mime == "video/mp2t" {
        span.min(HLS_CODEC_BOOTSTRAP_BYTES)
    } else {
        span
    };
    headers.push(("Content-Length".to_string(), response_span.to_string()));
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
    local_bytes_base: &str,
) -> FetchResponse {
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
                .map(|playlist| (index, playlist.render(local_bytes_base, start), None, 0)),
            FeedPayloadProbe::Deferred(_)
            | FeedPayloadProbe::Missing
            | FeedPayloadProbe::Transient => None,
        }
    } else if let Some((index, body, start_follower, revision)) =
        render_active_feed(&owner, &topic, start, local_bytes_base)
    {
        Some((index, body, start_follower, revision))
    } else {
        discover_latest_once(&client, &owner, &topic)
            .await
            .and_then(|payload| {
                HlsPlaylist::parse(&payload.bytes).map(|playlist| {
                    (
                        payload.index,
                        playlist.render(local_bytes_base, start),
                        None,
                        0,
                    )
                })
            })
    };
    let Some((index, body, follower, revision)) = rendered else {
        return FetchResponse::error(502, "The HLS feed could not be loaded");
    };
    if let Some(id) = follower {
        spawn_follower(id);
    }
    let mode = (start == HlsStart::Live)
        .then_some("live")
        .unwrap_or("beginning");
    let etag = format!("\"hls-feed-{index}-{mode}-{revision}\"");
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
        let query = web_sys::Url::new(request_url)
            .ok()
            .map(|url| url.search_params());
        let codec_bootstrap = query
            .as_ref()
            .and_then(|query| query.get("bootstrap"))
            .as_deref()
            == Some("1");
        let live = query
            .as_ref()
            .and_then(|query| query.get("start"))
            .as_deref()
            == Some("live");
        return Some(match reference {
            Ok(reference) => {
                fetch_hls_body_response(
                    client,
                    reference,
                    codec_bootstrap,
                    method,
                    range,
                    if_none_match,
                    live,
                )
                .await
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

pub(crate) async fn prepare_hls_feed(
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
    start: HlsStart,
    view_generation: u64,
) -> Result<PreparedHlsFeed, String> {
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
    let id = begin_feed(
        client.clone(),
        owner.clone(),
        topic.clone(),
        start,
        view_generation,
    );
    let result = async {
        let payload = match start {
            HlsStart::Beginning => {
                spawn_beginning_history(
                    id,
                    view_generation,
                    client.clone(),
                    owner.clone(),
                    topic.clone(),
                );
                discover_beginning(
                    id,
                    view_generation,
                    client.clone(),
                    owner.clone(),
                    topic.clone(),
                )
                .await
            }
            HlsStart::Live => {
                let payload = discover_raw_for_view(
                    id,
                    view_generation,
                    client.clone(),
                    owner.clone(),
                    topic.clone(),
                )
                .await;
                if let Some(payload) = &payload
                    && let Some(playlist) = HlsPlaylist::parse(&payload.bytes)
                {
                    prefetch_playlist_runway(client.clone(), &playlist, HlsStart::Live, Some(id));
                }
                payload
            }
        };
        if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        let payload = payload.ok_or_else(|| "The HLS feed could not be loaded.".to_string())?;
        let live = start == HlsStart::Live;
        let mut playlist = if live {
            hls_history(
                id,
                view_generation,
                &client,
                &owner,
                &topic,
                payload.index,
                payload.lattice_residue,
                &payload.bytes,
                Some(id),
            )
            .await
            .ok_or_else(|| "The HLS feed history could not be reconstructed.".to_string())?
        } else {
            HlsPlaylist::parse(&payload.bytes)
                .filter(|playlist| playlist.sequence == 0)
                .ok_or_else(|| "The beginning HLS prefix could not be loaded.".to_string())?
        };
        let index = if live {
            catch_up_history(
                id,
                view_generation,
                &client,
                &owner,
                &topic,
                payload.index,
                &mut playlist,
            )
            .await
            .map_err(str::to_string)?
        } else {
            payload.index
        };
        let plan = playlist.startup_plan(start).ok_or_else(|| {
            "The HLS feed does not contain a playable startup runway.".to_string()
        })?;
        let underfilled_beginning = !live
            && playlist
                .segments
                .iter()
                .filter(|segment| !segment.gap)
                .count()
                < BEGINNING_PREFIX_TARGET_SEGMENTS;
        let elapsed = plan.duration;
        install_snapshot(id, index, playlist)
            .ok_or_else(|| "HLS open was superseded".to_string())?;
        if underfilled_beginning {
            spawn_follower(id);
        }

        let feed_route = if client.service_worker_network_id() == 10 {
            "testnet/feeds"
        } else {
            "feeds"
        };
        let mut source = format!("{}/{}/{}", streaming_route_path(feed_route), owner, topic);
        if live {
            source.push_str("?start=live");
        }
        client.interface_log(format!(
            "HLS open index={} elapsed={:.3}s mode={}",
            index,
            elapsed,
            if live { "live" } else { "beginning" }
        ));
        Ok(PreparedHlsFeed { source, plan })
    }
    .await;
    if result.is_err() && feed_is_current(id) {
        end_feed(id);
        RANGE_CACHE.with(|cache| cache.borrow_mut().clear());
    }
    result
}

pub(crate) fn release_hls_runtime() {
    FEED.with(|feed| *feed.borrow_mut() = None);
}

pub(crate) fn clear_hls_runtime_cache() {
    RANGE_CACHE.with(|cache| cache.borrow_mut().clear());
}
