use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use event_listener::Event;
use futures::{FutureExt, StreamExt, future, stream};
use wasm_bindgen_futures::spawn_local;

use super::{
    HLS_BEGINNING_STARTUP_BUFFER_SECONDS, HLS_LIVE_BODY_RUNWAY_SEGMENTS, HLS_LIVE_EDGE_SEGMENTS,
    HLS_LIVE_STARTUP_BUFFER_SECONDS, HlsManifest, HlsMasterPlaylist, HlsPlaylist, HlsSource,
    HlsStart, MAX_STREAM_FEED_PAYLOAD_BYTES, PreparedHlsFeed, hls_payload_mime,
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
    retrieval::retrieve_decoded_data_root,
    retrieval_conventions::RetrieveAdmission,
    stream::{
        FetchResponse, clear_completed_media_ranges, forget_completed_reference_ranges,
        media_cache_max_bytes, read_cached_hls_range, result_view_request_is_current,
        set_auxiliary_media_cache_bytes,
    },
    stream_conventions::{
        MEDIA_STORAGE_WINDOW_BYTES, STREAMING_ROUTE_BASE, decode_component, if_none_match_matches,
        route_markers, streaming_route_path,
    },
};

const HLS_BODY_CACHE_MAX_BYTES: u64 = 32 * 1024 * 1024;
const HLS_BODY_MAX_BYTES: u64 = 96 * 1024 * 1024;
const BODY_PREFETCH_HORIZON: usize = HLS_LIVE_BODY_RUNWAY_SEGMENTS;
const HLS_BODY_PREFETCH_MAX_PARALLEL: usize = 3;
const BEGINNING_DISCOVERY_WIDTH: u64 = 8;
const BEGINNING_PREFIX_TARGET_SEGMENTS: usize = 4;
const BEGINNING_PAYLOAD_BYTES: usize = 64 * 1024;
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
const HLS_CODEC_BOOTSTRAP_BYTES: u64 = (320 * 1024 / 188) * 188;
const HISTORY_STRIDE: u64 = 10;
const HISTORY_MAX_PROBES: usize = 4_096;
const HISTORY_MAX_REPAIRS: usize = 4_096;
const HISTORY_BACKGROUND_PARALLEL: usize = 16;
const HISTORY_FOREGROUND_PARALLEL: usize = 64;

thread_local! {
    static BODY_CACHE: RefCell<BodyCache> = RefCell::new(BodyCache::default());
    static FEED: RefCell<FeedSessions> = RefCell::new(FeedSessions::default());
    static NEXT_FEED_ID: Cell<u64> = const { Cell::new(0) };
}

#[derive(Default)]
struct BodyCache {
    epoch: u64,
    bodies: HashMap<String, Bytes>,
    body_order: VecDeque<String>,
    pending_bodies: HashMap<String, PendingBody>,
    bytes: u64,
}

struct PendingBody {
    epoch: u64,
    waiters: Vec<mpsc::Sender<Option<Bytes>>>,
}

enum BodyLoad {
    Cached(Bytes),
    Wait(mpsc::Receiver<Option<Bytes>>),
    Lead(u64),
}

impl BodyCache {
    fn get(&self, reference: &str, start: u64, end: u64) -> Option<Bytes> {
        let body = self.bodies.get(reference)?;
        let start = usize::try_from(start).ok()?;
        let end = usize::try_from(end).ok()?.checked_add(1)?;
        body.get(start..end)?;
        Some(body.slice(start..end))
    }

    fn body_load(&mut self, reference: &str) -> BodyLoad {
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
                waiters: Vec::new(),
            },
        );
        BodyLoad::Lead(self.epoch)
    }

    fn body_cached(&self, reference: &str) -> bool {
        self.bodies.contains_key(reference)
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
            forget_completed_reference_ranges(&reference);
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
        let maximum = media_cache_max_bytes().min(HLS_BODY_CACHE_MAX_BYTES);
        while self.bytes > maximum {
            if let Some(reference) = self.body_order.pop_front() {
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
        self.bodies.clear();
        self.body_order.clear();
        self.bytes = 0;
        set_auxiliary_media_cache_bytes(0);
    }
}

#[derive(Default)]
struct FeedSessions {
    active: u64,
    feeds: Vec<FeedSession>,
    master: Option<(String, String, u64, HlsMasterPlaylist)>,
}

impl FeedSessions {
    fn active(&self) -> Option<&FeedSession> {
        self.get(self.active)
    }

    fn active_mut(&mut self) -> Option<&mut FeedSession> {
        self.get_mut(self.active)
    }

    fn get(&self, id: u64) -> Option<&FeedSession> {
        self.feeds.iter().find(|feed| feed.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut FeedSession> {
        self.feeds.iter_mut().find(|feed| feed.id == id)
    }

    fn find(&self, owner: &str, topic: &str, pinned: Option<u64>) -> Option<&FeedSession> {
        self.feeds
            .iter()
            .find(|feed| feed.owner == owner && feed.topic == topic && feed.pinned == pinned)
    }
}

struct FeedSession {
    id: u64,
    changed: Event,
    view_generation: u64,
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
    pinned: Option<u64>,
    start: HlsStart,
    following: bool,
    refreshing: bool,
    ready: bool,
    beginning_history_started: bool,
    body_runway_running: bool,
    live_startup_plan: Option<super::HlsStartupPlan>,
    foreground: Option<String>,
    index: Option<u64>,
    playlist: Option<HlsPlaylist>,
    terminal_candidate: Option<u64>,
    presentation_gaps: HashSet<(u64, String)>,
    tail_fallbacks: VecDeque<f64>,
    presentation_revision: u64,
}

impl Drop for FeedSession {
    fn drop(&mut self) {
        self.changed.notify(usize::MAX);
    }
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
    client: &Arc<Weeb3>,
    reference: &str,
    span: u64,
    start: u64,
    end: u64,
    admitted: &dyn Fn() -> bool,
) -> Option<Bytes> {
    let epoch = BODY_CACHE.with(|cache| cache.borrow().epoch);
    if let Some(bytes) = BODY_CACHE.with(|cache| cache.borrow().get(reference, start, end)) {
        return Some(bytes);
    }
    let current = || {
        BODY_CACHE.with(|cache| {
            let cache = cache.borrow();
            cache.epoch == epoch
                && (admitted()
                    || cache.pending_bodies.get(reference).is_some_and(|pending| {
                        pending.epoch == epoch
                            && pending.waiters.iter().any(|waiter| !waiter.is_closed())
                    }))
        })
    };
    read_cached_hls_range(client, reference, span, start, end, &current).await
}

fn body_is_current(id: u64, reference: &str) -> bool {
    FEED.with(|feed| {
        feed.borrow().active().is_some_and(|active| {
            active.id == id
                && body_runway_targets(active)
                    .iter()
                    .any(|target| !target.gap && target.reference == reference)
        })
    })
}

async fn hls_body(client: Arc<Weeb3>, reference: String, generation: Option<u64>) -> Option<Bytes> {
    if generation.is_some_and(|id| !body_is_current(id, &reference)) {
        return None;
    }
    let epoch = match BODY_CACHE.with(|cache| cache.borrow_mut().body_load(&reference)) {
        BodyLoad::Cached(body) => return Some(body),
        BodyLoad::Wait(waiter) => return waiter.recv().await.ok().flatten(),
        BodyLoad::Lead(epoch) => epoch,
    };
    let body = async {
        let decoded = hex::decode(&reference).ok()?;
        let root = retrieve_decoded_data_root(&decoded, &client.chunk_port.0).await?;
        if root.span == 0
            || root.span > HLS_BODY_MAX_BYTES
            || BODY_CACHE.with(|cache| cache.borrow().epoch != epoch)
        {
            return None;
        }
        let end = root.span.checked_sub(1)?;
        hls_range(&client, &reference, root.span, 0, end, &|| {
            generation.is_none_or(|id| body_is_current(id, &reference))
        })
        .await
    }
    .await;
    BODY_CACHE.with(|cache| cache.borrow_mut().finish_body(reference, epoch, body))
}

async fn foreground_hls_body(client: Arc<Weeb3>, reference: String) -> Option<Bytes> {
    for attempt in 0..HLS_BODY_ATTEMPTS {
        if let Some(body) = hls_body(client.clone(), reference.clone(), None).await {
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

#[rustfmt::skip]
fn live_segment_is_playable(active: &FeedSession, position: usize) -> bool {
    let Some(playlist) = active.playlist.as_ref() else { return false };
    let Some(segment) = playlist.segments.get(position) else { return false };
    let Some(sequence) = playlist.sequence.checked_add(position as u64) else { return false };
    !segment.gap && !active.presentation_gaps.iter().any(|(gap, reference)| *gap == sequence && reference == &segment.reference)
}

fn latest_live_position(active: &FeedSession) -> Option<usize> {
    let playlist = presentation_playlist(active)?;
    let (position, segment) = playlist
        .segments
        .iter()
        .enumerate()
        .rfind(|(_, segment)| !segment.gap)?;
    let sequence = playlist.sequence.checked_add(position as u64)?;
    playlist
        .anchored_startup_plan(sequence, &segment.reference)
        .map(|(_, first)| first)
}

fn body_runway_targets(active: &FeedSession) -> &[super::HlsSegment] {
    let Some(playlist) = active.playlist.as_ref() else {
        return &[];
    };
    let foreground = active.foreground.as_deref().and_then(|reference| {
        let mut positions = playlist.segments.iter().enumerate();
        let matches = |(position, segment): &(usize, &super::HlsSegment)| {
            segment.reference == reference && live_segment_is_playable(active, *position)
        };
        if active.start == HlsStart::Live {
            positions.rfind(matches)
        } else {
            positions.find(matches)
        }
        .map(|(position, _)| position)
    });
    let Some(position) = foreground.or_else(|| {
        if active.start == HlsStart::Live {
            latest_live_position(active)
        } else {
            playlist.segments.iter().position(|segment| !segment.gap)
        }
    }) else {
        return &[];
    };
    if active.start == HlsStart::Beginning {
        let length = playlist.segments[position..]
            .iter()
            .enumerate()
            .filter(|(_, segment)| !segment.gap)
            .take(BODY_PREFETCH_HORIZON)
            .last()
            .map_or(0, |(offset, _)| offset + 1);
        return &playlist.segments[position..position + length];
    }
    let mut seconds = 0.0;
    let length = playlist.segments[position..]
        .iter()
        .enumerate()
        .take_while(|(offset, segment)| {
            if seconds >= HLS_LIVE_STARTUP_BUFFER_SECONDS
                || !live_segment_is_playable(active, position + offset)
            {
                return false;
            }
            if *offset != 0 {
                seconds += segment.duration;
            }
            true
        })
        .count();
    &playlist.segments[position..position + length]
}

fn presentation_playlist(active: &FeedSession) -> Option<std::borrow::Cow<'_, HlsPlaylist>> {
    let playlist = active.playlist.as_ref()?;
    if active.presentation_gaps.is_empty() {
        return Some(std::borrow::Cow::Borrowed(playlist));
    }
    let mut presentation = playlist.clone();
    for (sequence, reference) in &active.presentation_gaps {
        presentation.mark_gap(*sequence, reference);
    }
    Some(std::borrow::Cow::Owned(presentation))
}

pub(crate) fn lock_live_startup_plan() -> Option<super::HlsStartupPlan> {
    FEED.with(|feed| {
        let feed = feed.borrow();
        let active = feed.active().filter(|active| {
            active.start == HlsStart::Live && active.live_startup_plan.is_some()
        })?;
        presentation_playlist(active)?.startup_plan(HlsStart::Live)
    })
}

fn spawn_body_runway(id: u64) {
    let Some(client) = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.active_mut().filter(|active| {
            active.id == id
                && !active.body_runway_running
                && (active.start == HlsStart::Live || active.beginning_history_started)
        })?;
        active.body_runway_running = true;
        Some(active.client.clone())
    }) else {
        return;
    };
    spawn_local(async move {
        let mut loads = stream::FuturesUnordered::new();
        let mut loaded = HashMap::<_, bool>::new();
        loop {
            let Some((changed, references, parallel)) = FEED.with(|feed| {
                let feed = feed.borrow();
                let active = feed.get(id)?;
                Some((
                    active.changed.listen(),
                    (feed.active == id)
                        .then(|| body_runway_targets(active))
                        .unwrap_or_default()
                        .iter()
                        .filter(|segment| !segment.gap)
                        .skip(usize::from(active.start == HlsStart::Beginning))
                        .map(|segment| segment.reference.clone())
                        .collect::<Vec<_>>(),
                    if active.start == HlsStart::Beginning {
                        1
                    } else {
                        HLS_BODY_PREFETCH_MAX_PARALLEL
                    },
                ))
            }) else {
                break;
            };
            loaded.retain(|reference, complete| !*complete || references.contains(reference));
            for reference in references {
                if loads.len() == parallel {
                    break;
                }
                if BODY_CACHE.with(|cache| cache.borrow().body_cached(&reference))
                    || loaded.contains_key(&reference)
                {
                    continue;
                }
                loaded.insert(reference.clone(), false);
                loads.push(
                    hls_body(client.clone(), reference.clone(), Some(id))
                        .map(move |body| (reference, body.is_some())),
                );
            }
            if loads.is_empty() {
                break;
            }
            if let future::Either::Left((Some((reference, complete)), _)) =
                future::select(loads.next(), changed).await
            {
                if complete {
                    loaded.insert(reference, true);
                } else {
                    loaded.remove(&reference);
                    if body_is_current(id, &reference) {
                        async_std::task::sleep(Duration::from_millis(HLS_BODY_RETRY_DELAY_MS))
                            .await;
                    }
                }
            }
        }
        while loads.next().await.is_some() {}
        FEED.with(|feed| {
            if let Some(active) = feed.borrow_mut().get_mut(id) {
                active.body_runway_running = false;
            }
        });
    });
}

fn prefetch_from_reference(reference: &str, cached: bool) -> bool {
    let runway = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        if feed.feeds.len() > 1 {
            let contains = |active: &&FeedSession| {
                active.playlist.as_ref().is_some_and(|playlist| {
                    playlist
                        .segments
                        .iter()
                        .any(|segment| !segment.gap && segment.reference == reference)
                })
            };
            let selected = feed
                .active()
                .filter(contains)
                .or_else(|| feed.feeds.iter().find(contains))?
                .id;
            if feed.active != selected {
                if let Some(previous) = feed.active() {
                    previous.changed.notify(usize::MAX);
                }
                feed.active = selected;
            }
        }
        let active = feed.active_mut()?;
        let playlist = active.playlist.as_ref()?;
        let matches = |segment: &super::HlsSegment| !segment.gap && segment.reference == reference;
        if active.start == HlsStart::Live {
            playlist
                .segments
                .iter()
                .enumerate()
                .rfind(|(position, segment)| {
                    matches(segment) && live_segment_is_playable(active, *position)
                })?;
            active.foreground = Some(reference.to_string());
            active.changed.notify(usize::MAX);
            return Some((active.id, true, false));
        }
        let mut playable = playlist.segments.iter().filter(|segment| !segment.gap);
        let position = playable.clone().position(matches)?;
        let (transition, position) = active
            .foreground
            .as_ref()
            .and_then(|last| {
                playable
                    .clone()
                    .position(|segment| &segment.reference == last)
            })
            .map_or((false, position), |last| {
                hls_progressive_foreground_transition(last, position, cached)
            });
        active.foreground = playable
            .nth(position)
            .map(|segment| segment.reference.clone());
        active.changed.notify(usize::MAX);
        Some((active.id, active.beginning_history_started, transition))
    });
    let Some((id, follow, transition)) = runway else {
        return false;
    };
    if follow {
        spawn_follower(id);
        spawn_body_runway(id);
    }
    transition
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
    pinned: Option<u64>,
    start: HlsStart,
    view_generation: u64,
) -> u64 {
    let id = next_feed_id();
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        if feed.feeds.is_empty() {
            feed.active = id;
        }
        feed.feeds.push(FeedSession {
            id,
            changed: Event::new(),
            view_generation,
            client,
            owner,
            topic,
            pinned,
            start,
            following: false,
            refreshing: false,
            ready: false,
            beginning_history_started: false,
            body_runway_running: false,
            live_startup_plan: None,
            foreground: None,
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
    FEED.with(|feed| feed.borrow().get(id).is_some())
}

fn end_feed(id: u64) {
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        feed.feeds.retain(|feed| feed.id != id);
    });
}

fn install_snapshot(
    id: u64,
    index: u64,
    mut playlist: HlsPlaylist,
    foreground: Option<String>,
) -> Option<()> {
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.get_mut(id)?;
        if let Some(tail) = active.playlist.take() {
            playlist.merge_playlist(tail)?;
        } else {
            active.index = Some(index);
            active.terminal_candidate = playlist.finalized.then_some(index);
            playlist.finalized = false;
        }
        active.playlist = Some(playlist);
        if foreground.is_some() {
            active.foreground = foreground;
        }
        active.changed.notify(usize::MAX);
        Some(())
    })
}

fn initialize_live_head(id: u64, index: u64, head: &HlsPlaylist) -> Option<(u64, String, bool)> {
    let (position, segment) = head
        .segments
        .iter()
        .enumerate()
        .rfind(|(_, segment)| !segment.gap)?;
    let sequence = head.sequence.checked_add(position as u64)?;
    let foreground = if head.finalized {
        head.anchored_startup_plan(sequence, &segment.reference)
            .map_or(position, |(_, first)| first)
    } else {
        position
    };
    install_snapshot(
        id,
        index,
        head.clone(),
        Some(head.segments[foreground].reference.clone()),
    )?;
    spawn_body_runway(id);
    spawn_follower(id);
    Some((sequence, segment.reference.clone(), !head.finalized))
}

async fn prepare_live_plan(
    id: u64,
    view_generation: u64,
    anchor: &(u64, String, bool),
) -> Result<(u64, super::HlsStartupPlan, u64), &'static str> {
    loop {
        let (changed, prepared) = FEED.with(|feed| {
            let mut feed = feed.borrow_mut();
            let active = feed
                .get_mut(id)
                .filter(|_| result_view_request_is_current(view_generation))
                .ok_or("HLS open was superseded.")?;
            let changed = active.changed.listen();
            let playlist = active
                .playlist
                .as_ref()
                .filter(|playlist| playlist.sequence == 0)
                .ok_or("The HLS feed history could not be reconstructed.")?;
            let position = usize::try_from(anchor.0)
                .ok()
                .filter(|position| {
                    playlist
                        .segments
                        .get(*position)
                        .is_some_and(|segment| !segment.gap && segment.reference == anchor.1)
                })
                .ok_or("The initial HLS edge no longer matches its history.")?;
            let selected = playlist
                .anchored_startup_plan(anchor.0, &anchor.1)
                .filter(|(_, first)| !anchor.2 || *first == position);
            let prepared = if let Some((plan, first)) = selected {
                active.foreground = Some(playlist.segments[first].reference.clone());
                active.live_startup_plan = Some(plan.clone());
                active.changed.notify(usize::MAX);
                Some((
                    active.index.ok_or("The HLS feed index was unavailable.")?,
                    plan,
                    first as u64,
                ))
            } else {
                if !anchor.2
                    || playlist.finalized
                    || playlist.segments[position..]
                        .iter()
                        .any(|segment| segment.gap)
                {
                    return Err("The HLS feed does not contain a playable startup runway.");
                }
                None
            };
            Ok((changed, prepared))
        })?;
        if let Some(prepared) = prepared {
            spawn_body_runway(id);
            return Ok(prepared);
        }
        changed.await;
    }
}

async fn discover_beginning(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
) -> Option<RawFeedPayload> {
    while feed_is_current(id) && result_view_request_is_current(view_generation) {
        let mut probes = Box::pin(
            stream::iter(0..BEGINNING_DISCOVERY_WIDTH)
                .map(|index| {
                    probe_feed_payload(
                        client,
                        owner,
                        topic,
                        index,
                        BEGINNING_PAYLOAD_BYTES,
                        Some(FEED_PROBE_ATTEMPTS),
                    )
                })
                .buffer_unordered(BEGINNING_DISCOVERY_WIDTH as usize),
        );
        while let Some(probe) = probes.next().await {
            if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
                return None;
            }
            if let FeedPayloadProbe::Found(payload) = probe {
                match HlsManifest::parse(&payload.bytes) {
                    Some(HlsManifest::Master(_)) => return Some(payload),
                    Some(HlsManifest::Media(playlist))
                        if playlist.sequence == 0
                            && playlist.startup_plan(HlsStart::Beginning).is_some() =>
                    {
                        warm_hls_prefix(client.clone(), id, &playlist, HlsStart::Beginning);
                        return Some(payload);
                    }
                    _ => {}
                }
            }
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
    None
}
async fn edge_probe_wave(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    indices: &[u64],
    lower_is_known: bool,
    attempt_limit: Option<usize>,
) -> (Vec<Option<Vec<u8>>>, Vec<bool>) {
    let mut probes = Box::pin(
        stream::iter(indices.iter().copied().enumerate())
            .map(|(slot, index)| async move {
                (
                    slot,
                    probe_feed_update(client, owner, topic, index, attempt_limit).await,
                )
            })
            .buffer_unordered(indices.len().max(1)),
    );
    let mut found = vec![None; indices.len()];
    let mut missing = vec![false; indices.len()];
    let mut completed = vec![false; indices.len()];
    let deadline = if lower_is_known {
        future::pending::<()>().left_future()
    } else {
        async_std::task::sleep(EDGE_COLD_WAVE_TIMEOUT).right_future()
    };
    futures::pin_mut!(deadline);
    let mut positive_seen = lower_is_known;
    while let future::Either::Left((Some((slot, result)), _)) =
        future::select(probes.next(), deadline.as_mut()).await
    {
        completed[slot] = true;
        match result {
            FeedProbe::Found(update) => {
                found[slot] = Some(update);
                if !positive_seen {
                    positive_seen = true;
                    deadline.set(async_std::task::sleep(EDGE_WAVE_TIMEOUT).right_future());
                }
            }
            FeedProbe::Missing => missing[slot] = true,
            FeedProbe::Transient => {}
        }
        let first_unsettled = found
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |found| found + 1);
        if !lower_is_known
            && positive_seen
            && completed[first_unsettled..].iter().all(|settled| *settled)
        {
            break;
        }
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

// Coarse bounds only select an authenticated positive; the fresh twenty-guard
// confirmation below establishes the edge, including growth past an old upper.
async fn discover_edge_update(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    history: Option<&mpsc::Sender<u64>>,
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

    if let Some(history) = history {
        let _ = history.try_send(latest.0);
    }
    loop {
        if upper.saturating_sub(latest.0) <= HISTORY_STRIDE * 2 {
            return Some(latest);
        }
        let first = latest.0 + 1;
        let span = u128::from(upper - first);
        let divisor = (EDGE_REFINEMENT_WIDTH - 1) as u128;
        let indices = std::iter::once(first)
            .chain(
                (1..EDGE_REFINEMENT_WIDTH - 1)
                    .map(|position| first + (span * position as u128).div_ceil(divisor) as u64),
            )
            .chain(std::iter::once(upper))
            .collect::<Vec<_>>();
        let (mut found, missing) =
            edge_probe_wave(client, owner, topic, &indices, true, fast).await;
        let previous = (latest.0, upper);
        if let Some(slot) = found.iter().rposition(Option::is_some) {
            latest = (indices[slot], found[slot].take()?);
        }
        if let Some(next_upper) = indices
            .iter()
            .enumerate()
            .find_map(|(slot, index)| (*index > latest.0 && missing[slot]).then_some(*index))
        {
            upper = next_upper;
        }
        // Fresh dense guards resolve a stalled search or a published old upper.
        if latest.0 >= upper || (latest.0, upper) == previous {
            return Some(latest);
        }
    }
}

async fn discover_latest_once(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    history: Option<&mpsc::Sender<u64>>,
) -> Option<RawFeedPayload> {
    let (index, update) = discover_edge_update(client, owner, topic, history).await?;
    retrieve_confirmed_payload(client, owner, topic, index, update).await
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
        // Confirm the entire frontier together, including both history-stride guards.
        // A transient probe never establishes the end of the feed.
        let dense = index.checked_add(1)?..=index.checked_add(HISTORY_STRIDE * 2)?;
        let mut probes = stream::iter(dense)
            .map(|index| async move {
                (
                    index,
                    probe_feed_update(client, owner, topic, index, Some(FEED_PROBE_ATTEMPTS)).await,
                )
            })
            .buffered((HISTORY_STRIDE * 2) as usize);
        let mut transient = false;
        let mut newest = None;
        while let Some((index, probe)) = probes.next().await {
            match probe {
                FeedProbe::Found(update) => newest = Some((index, update)),
                FeedProbe::Transient => transient = true,
                FeedProbe::Missing => {}
            }
        }
        if let Some((next, next_update)) = newest {
            (index, update) = (next, next_update);
            continue;
        }
        if transient {
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

fn warm_hls_prefix(client: Arc<Weeb3>, id: u64, playlist: &HlsPlaylist, start: HlsStart) {
    let Some(mut segment) = playlist
        .segments
        .iter()
        .find(|segment| !segment.gap)
        .cloned()
    else {
        return;
    };
    spawn_local(async move {
        for ordinal in 0..if start == HlsStart::Beginning { 2 } else { 1 } {
            if ordinal == 1 {
                let Some(successor) = beginning_successor(id).await else {
                    return;
                };
                segment = successor;
            }
            if !feed_is_current(id) {
                return;
            }
            let Ok(decoded) = hex::decode(&segment.reference) else {
                return;
            };
            let Some(root) = retrieve_decoded_data_root(&decoded, &client.chunk_port.0).await
            else {
                return;
            };
            if !feed_is_current(id) || root.span == 0 {
                return;
            }
            let maximum = if start == HlsStart::Beginning {
                let Some(bytes) = beginning_prefix_bytes(root.span, segment.duration) else {
                    return;
                };
                bytes
            } else {
                root.span.min(HLS_CODEC_BOOTSTRAP_BYTES)
            };
            // Protect A first; admit only one exact successor window at a time.
            let width = if ordinal == 0 {
                maximum
            } else {
                MEDIA_STORAGE_WINDOW_BYTES
            };
            for offset in (0..maximum).step_by(width as usize) {
                if !feed_is_current(id)
                    || hls_range(
                        &client,
                        &segment.reference,
                        root.span,
                        offset,
                        (offset + width).min(maximum) - 1,
                        &|| feed_is_current(id),
                    )
                    .await
                    .is_none()
                {
                    return;
                }
            }
        }
    });
}

fn beginning_prefix_bytes(span: u64, duration: f64) -> Option<u64> {
    if span == 0 || !duration.is_finite() || duration <= 0.0 {
        return None;
    }
    let bytes = ((span as f64) * duration.min(HLS_BEGINNING_STARTUP_BUFFER_SECONDS + 0.25)
        / duration)
        .ceil() as u64;
    Some(
        (bytes.div_ceil(MEDIA_STORAGE_WINDOW_BYTES).clamp(1, 8) * MEDIA_STORAGE_WINDOW_BYTES)
            .min(span),
    )
}

async fn beginning_successor(id: u64) -> Option<super::HlsSegment> {
    loop {
        let (changed, successor, finalized) = FEED.with(|feed| {
            let feed = feed.borrow();
            let active = feed.get(id)?;
            let changed = active.changed.listen();
            let playlist = active.playlist.as_ref();
            Some((
                changed,
                playlist.and_then(|playlist| {
                    playlist
                        .segments
                        .iter()
                        .filter(|segment| !segment.gap)
                        .nth(1)
                        .cloned()
                }),
                playlist.is_some_and(|playlist| playlist.finalized),
            ))
        })?;
        if successor.is_some() || finalized {
            return successor;
        }
        changed.await;
    }
}

async fn history_snapshots(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    indices: &[u64],
    parallel: usize,
    warm_pending: &mut bool,
) -> Vec<(u64, HlsPlaylist)> {
    stream::iter(indices.iter().copied())
        .map(|index| async move {
            if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
                return None;
            }
            let FeedPayloadProbe::Found(payload) = probe_feed_payload(
                client,
                owner,
                topic,
                index,
                MAX_STREAM_FEED_PAYLOAD_BYTES,
                Some(FEED_PROBE_ATTEMPTS),
            )
            .await
            else {
                return None;
            };
            Some((index, HlsPlaylist::parse(&payload.bytes)?))
        })
        .buffer_unordered(parallel)
        .filter_map(async move |snapshot| snapshot)
        .inspect(|(_, playlist)| {
            if *warm_pending
                && playlist.sequence == 0
                && playlist.segments.iter().any(|segment| !segment.gap)
                && feed_is_current(id)
                && result_view_request_is_current(view_generation)
            {
                *warm_pending = false;
                warm_hls_prefix(client.clone(), id, playlist, HlsStart::Live);
            }
        })
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
    let mut repairs = Vec::new();
    let mut add = |range: std::ops::Range<u64>| -> Option<()> {
        for index in range {
            if index < head_index && attempted.binary_search(&index).is_err() {
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

fn history_indices(head_index: u64, residue: u64) -> Option<Vec<u64>> {
    let count = head_index.saturating_sub(residue).div_ceil(HISTORY_STRIDE);
    if count == 0 || count > HISTORY_MAX_PROBES as u64 {
        return None;
    }
    Some(
        (residue..head_index)
            .step_by(HISTORY_STRIDE as usize)
            .collect(),
    )
}

async fn hls_history(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    head_index: u64,
    lattice_residue: u64,
    head: HlsPlaylist,
    parallel: usize,
    mut warm_pending: bool,
    prefix: Option<(u64, Vec<(u64, HlsPlaylist)>)>,
) -> Option<HlsPlaylist> {
    if head.sequence == 0 {
        if warm_pending {
            warm_hls_prefix(client.clone(), id, &head, HlsStart::Live);
        }
        return Some(head);
    }
    let indices = history_indices(head_index, lattice_residue)?;
    let (until, mut snapshots) = prefix.unwrap_or_default();
    snapshots.retain(|(index, _)| *index < head_index);
    let remaining = &indices[indices.partition_point(|index| *index < until)..];
    #[rustfmt::skip]
    snapshots.extend(history_snapshots(id, view_generation, client, owner, topic, remaining, parallel, &mut warm_pending).await);
    let repairs = history_repairs(&indices, &snapshots, head_index, &head)?;
    #[rustfmt::skip]
    snapshots.extend(history_snapshots(id, view_generation, client, owner, topic, &repairs, parallel, &mut warm_pending).await);
    HlsPlaylist::reconstruct(snapshots, head_index, head)
}

async fn discover_raw_for_view(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    history: Option<&mpsc::Sender<u64>>,
) -> Option<RawFeedPayload> {
    loop {
        if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
            return None;
        }
        if let Some(payload) = discover_latest_once(client, owner, topic, history).await {
            return Some(payload);
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
}

async fn discover_live_history(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
) -> Result<(u64, HlsManifest, Option<(u64, String, bool)>), &'static str> {
    let current = || feed_is_current(id) && result_view_request_is_current(view_generation);
    let (output, input) = mpsc::bounded(1);
    let mut warm_pending = true;
    let prefix = async {
        let index: u64 = input.recv().await.ok()?;
        drop(input);
        let residue = index % HISTORY_STRIDE;
        let indices = history_indices(index, residue).unwrap_or_default();
        let until = indices
            .last()
            .map_or(residue, |index| index + HISTORY_STRIDE);
        let snapshots = history_snapshots(
            id,
            view_generation,
            client,
            owner,
            topic,
            &indices,
            HISTORY_FOREGROUND_PARALLEL,
            &mut warm_pending,
        )
        .await;
        Some((residue, until, snapshots))
    };
    let discovery = async {
        let payload =
            discover_raw_for_view(id, view_generation, client, owner, topic, Some(&output)).await;
        drop(output);
        if !current() {
            return Err("HLS open was superseded");
        }
        let payload = payload.ok_or("The HLS feed could not be loaded.")?;
        let manifest =
            HlsManifest::parse(&payload.bytes).ok_or("The HLS manifest could not be read.")?;
        let anchor = match &manifest {
            HlsManifest::Media(head) => Some(
                initialize_live_head(id, payload.index, head)
                    .ok_or("The HLS feed does not contain a playable startup runway.")?,
            ),
            HlsManifest::Master(_) => None,
        };
        Ok((payload.index, manifest, anchor))
    };
    let (prefix, discovered) = future::join(prefix, discovery).await;
    let (index, manifest, anchor) = discovered?;
    let HlsManifest::Media(head) = manifest else {
        return Ok((index, manifest, None));
    };
    let (residue, until, snapshots) = prefix.ok_or("HLS open was superseded")?;
    let history = hls_history(
        id,
        view_generation,
        client,
        owner,
        topic,
        index,
        residue,
        head,
        HISTORY_FOREGROUND_PARALLEL,
        warm_pending,
        Some((until, snapshots)),
    )
    .await
    .ok_or("The HLS feed history could not be reconstructed.")?;
    if !current() {
        return Err("HLS open was superseded");
    }
    Ok((index, HlsManifest::Media(history), anchor))
}

async fn discover_for_view(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
) -> Option<(u64, HlsPlaylist)> {
    loop {
        let payload =
            discover_raw_for_view(id, view_generation, client, owner, topic, None).await?;
        if let Some(head) = HlsPlaylist::parse(&payload.bytes)
            && let Some(history) = hls_history(
                id,
                view_generation,
                client,
                owner,
                topic,
                payload.index,
                payload.lattice_residue,
                head,
                HISTORY_BACKGROUND_PARALLEL,
                false,
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
    pinned: Option<u64>,
    start: HlsStart,
    local_bytes_base: &str,
) -> Option<(u64, Vec<u8>, Option<u64>, u64)> {
    FEED.with(|feed| {
        let feed = feed.borrow();
        let feed = feed.find(owner, topic, pinned)?;
        let index = feed.index?;
        let playlist = feed.playlist.as_ref()?;
        let body = if start == HlsStart::Live {
            presentation_playlist(feed)?.render_with_plan(
                local_bytes_base,
                start,
                feed.live_startup_plan.as_ref(),
            )
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
    let appended = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.get_mut(id)?;
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
        active.changed.notify(usize::MAX);
        Some(appended)
    })?;
    if appended != 0 {
        spawn_body_runway(id);
    }
    Some(appended)
}

fn apply_full_update(id: u64, index: u64, candidate: HlsPlaylist) -> Option<usize> {
    apply_update(id, index, |playlist| playlist.merge_playlist(candidate))
}

fn confirm_terminal(id: u64, index: u64, candidate: HlsPlaylist) -> bool {
    FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let Some(active) = feed.get_mut(id).filter(|active| {
            active.index == Some(index)
                && active.terminal_candidate == Some(index)
                && candidate.finalized
        }) else {
            return false;
        };
        let confirmed = active
            .playlist
            .as_mut()
            .and_then(|playlist| playlist.merge_playlist(candidate))
            .is_some();
        if confirmed {
            active.terminal_candidate = None;
            active.changed.notify(usize::MAX);
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
        let active = feed.active().filter(|active| {
            active.start == HlsStart::Live && active.live_startup_plan.is_some()
        })?;
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
        let active = feed
            .active_mut()
            .filter(|feed| feed.start == HlsStart::Live && feed.live_startup_plan.is_some())?;
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
        active.changed.notify(usize::MAX);
        Some(target)
    })
}

fn feed_follow_context(id: u64) -> Option<(Arc<Weeb3>, String, String, u64, u64)> {
    FEED.with(|feed| {
        let feed = feed.borrow();
        let feed = feed.get(id)?;
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

pub(crate) fn start_beginning_history() {
    let context = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        let active = feed.active_mut().filter(|active| {
            active.start == HlsStart::Beginning && !active.beginning_history_started
        })?;
        active.beginning_history_started = true;
        let mut references = active
            .playlist
            .iter()
            .flat_map(|playlist| &playlist.segments)
            .filter(|segment| !segment.gap)
            .map(|segment| segment.reference.clone());
        Some((
            active.id,
            active.view_generation,
            active.client.clone(),
            active.owner.clone(),
            active.topic.clone(),
            references.next(),
            references.next(),
        ))
    });
    let Some((id, view_generation, client, owner, topic, foreground, successor)) = context else {
        return;
    };
    if let Some(reference) = foreground {
        prefetch_from_reference(&reference, true);
    }
    spawn_local(async move {
        if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
            return;
        }
        spawn_follower(id);
        if let Some(successor) = successor {
            let _ = hls_body(client.clone(), successor, Some(id)).await;
        }
        if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
            return;
        }
        let history = discover_for_view(id, view_generation, &client, &owner, &topic).await;
        if let Some((index, history)) = history
            && feed_is_current(id)
            && result_view_request_is_current(view_generation)
            && apply_full_update(id, index, history).is_some()
        {
            client.interface_log(format!("HLS history reached index {index}"));
        }
    });
}

fn spawn_follower(id: u64) {
    let claimed = FEED.with(|feed| {
        let mut feed = feed.borrow_mut();
        if feed.active != id {
            return false;
        }
        let Some(active) = feed.get_mut(id).filter(|active| !active.following) else {
            return false;
        };
        active.following = true;
        true
    });
    if !claimed {
        return;
    }
    spawn_local(async move {
        async {
            let mut last_frontier_check = js_sys::Date::now();
            loop {
                if !FEED.with(|feed| feed.borrow().active == id) {
                    return;
                }
                let Some((client, owner, topic, head, _)) = feed_follow_context(id) else {
                    return;
                };
                let mut progressed = false;
                let mut skipped_missing_index = false;
                for offset in 1..=FEED_FOLLOW_AHEAD {
                    let Some(index) = head.checked_add(offset) else {
                        return;
                    };
                    let candidate = probe_feed_payload(
                        &client,
                        &owner,
                        &topic,
                        index,
                        FEED_TAIL_PROBE_BYTES,
                        None,
                    )
                    .await;
                    if !FEED
                        .with(|feed| feed.borrow().active == id && feed.borrow().get(id).is_some())
                    {
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
                if !FEED.with(|feed| feed.borrow().active == id && feed.borrow().get(id).is_some())
                {
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
        }
        .await;
        FEED.with(|feed| {
            if let Some(active) = feed.borrow_mut().get_mut(id) {
                active.following = false;
            }
        });
    });
}

async fn recover_feed_frontier(id: u64, client: &Arc<Weeb3>, owner: &str, topic: &str) -> bool {
    let Some((_, _, _, head, view_generation)) = feed_follow_context(id) else {
        return false;
    };
    let Some(payload) = discover_latest_once(client, owner, topic, None).await else {
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
        let Some(head) = HlsPlaylist::parse(&payload.bytes) else {
            return false;
        };
        let Some(history) = hls_history(
            id,
            view_generation,
            client,
            owner,
            topic,
            index,
            payload.lattice_residue,
            head,
            HISTORY_BACKGROUND_PARALLEL,
            false,
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

fn hls_body_headers(
    etag: String,
    mime: &str,
    length: u64,
    range: Option<(u64, u64, u64)>,
) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Content-Type".to_string(), mime.to_string()),
        (
            "Cache-Control".to_string(),
            "public, max-age=31536000, immutable".to_string(),
        ),
        ("ETag".to_string(), etag),
        ("Accept-Ranges".to_string(), "bytes".to_string()),
    ];
    headers.push(("Content-Length".to_string(), length.to_string()));
    if let Some((start, end, span)) = range {
        headers.push((
            "Content-Range".to_string(),
            format!("bytes {start}-{end}/{span}"),
        ));
    }
    headers
}

fn hls_body_response(
    body: Bytes,
    codec_bootstrap: bool,
    method: &str,
    range: Option<&str>,
    etag: String,
) -> Option<FetchResponse> {
    let span = body.len() as u64;
    let mut mime = "application/octet-stream";
    let (status, start, end, range) = if let Some(range) = range {
        let (start, end) = parse_hls_range(range, span)?;
        (
            206,
            start as usize,
            end as usize + 1,
            Some((start, end, span)),
        )
    } else {
        if codec_bootstrap {
            mime = hls_payload_mime(&body);
        }
        let end = if codec_bootstrap && mime == "video/mp2t" {
            body.len().min(HLS_CODEC_BOOTSTRAP_BYTES as usize)
        } else {
            body.len()
        };
        (200, 0, end, None)
    };
    let headers = hls_body_headers(etag, mime, (end - start) as u64, range);
    Some(if method == "HEAD" {
        FetchResponse::ok(status, headers, None)
    } else if start == 0 && end == body.len() {
        FetchResponse::ok_shared(status, headers, body)
    } else {
        FetchResponse::ok_shared_slice(status, headers, body, start, end)?
    })
}

async fn fetch_hls_body_response(
    client: Arc<Weeb3>,
    reference: String,
    codec_bootstrap: bool,
    method: &str,
    range: Option<&str>,
    if_none_match: Option<&str>,
) -> FetchResponse {
    let etag = format!("\"{reference}\"");
    if if_none_match_matches(if_none_match, &etag) {
        return FetchResponse::ok(304, vec![("ETag".to_string(), etag)], None);
    }
    let whole_media_get = method == "GET" && range.is_none() && !codec_bootstrap;
    let cached = whole_media_get && BODY_CACHE.with(|cache| cache.borrow().body_cached(&reference));
    let seek_transition = whole_media_get && prefetch_from_reference(&reference, cached);
    if !seek_transition
        && let Some(body) = BODY_CACHE.with(|cache| cache.borrow().body(&reference))
        && let Some(response) =
            hls_body_response(body, codec_bootstrap, method, range, etag.clone())
    {
        return response;
    }
    if seek_transition {
        let Some(body) = foreground_hls_body(client.clone(), reference.clone()).await else {
            return FetchResponse::error(503, "HLS segment body was unavailable");
        };
        return hls_body_response(body, false, method, None, etag)
            .unwrap_or_else(|| FetchResponse::error(503, "HLS segment body was unavailable"));
    }
    let decoded = match hex::decode(&reference) {
        Ok(decoded) => decoded,
        Err(_) => return FetchResponse::error(400, "invalid HLS swarm reference"),
    };
    let Some(root) = retrieve_decoded_data_root(&decoded, &client.chunk_port.0).await else {
        return FetchResponse::error(502, format!("HLS segment {reference} was unavailable"));
    };
    if root.span == 0 {
        return FetchResponse::error(502, "HLS segment was empty");
    }
    let span = root.span;

    if let Some(range) = range {
        let Some((start, end)) = parse_hls_range(range, span) else {
            return FetchResponse::ok(
                416,
                vec![("Content-Range".to_string(), format!("bytes */{span}"))],
                None,
            );
        };
        let Some(bytes) = hls_range(&client, &reference, span, start, end, &|| true).await else {
            return FetchResponse::error(503, "HLS segment range was unavailable");
        };
        let headers = hls_body_headers(
            etag,
            "application/octet-stream",
            bytes.len() as u64,
            Some((start, end, span)),
        );
        return if method == "HEAD" {
            FetchResponse::ok(206, headers, None)
        } else {
            FetchResponse::ok_shared(206, headers, bytes)
        };
    }

    let mime = if codec_bootstrap {
        let prefix_end = span.saturating_sub(1).min(188);
        hls_range(&client, &reference, span, 0, prefix_end, &|| true)
            .await
            .map_or("application/octet-stream", |prefix| {
                hls_payload_mime(&prefix)
            })
    } else {
        "application/octet-stream"
    };
    let response_span = if codec_bootstrap && mime == "video/mp2t" {
        span.min(HLS_CODEC_BOOTSTRAP_BYTES)
    } else {
        span
    };
    let headers = hls_body_headers(etag, mime, response_span, None);
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
    let immutable = index.is_some() || owner.is_empty();
    let mut requested_feed = FEED.with(|feed| {
        feed.borrow()
            .find(&owner, &topic, index)
            .map(|feed| feed.id)
    });
    let master = FEED.with(|feed| {
        let feed = feed.borrow();
        let (master_owner, master_topic, master_index, master) = feed.master.as_ref()?;
        (master_owner == &owner
            && master_topic == &topic
            && index.is_none_or(|index| index == *master_index))
        .then(|| {
            (
                *master_index,
                master
                    .render(|source, playlist| {
                        local_hls_source(source, local_bytes_base, start, playlist)
                    })
                    .into_bytes(),
                None,
                0,
            )
        })
    });
    if master.is_none() {
        let context = FEED.with(|feed| {
            let feed = feed.borrow();
            if feed.find(&owner, &topic, index).is_some() {
                return None;
            }
            let (_, _, _, master) = feed.master.as_ref()?;
            let source = local_feed_source(&owner, &topic, index, local_bytes_base, start);
            master
                .sources()
                .any(|candidate| {
                    local_hls_source(candidate, local_bytes_base, start, true).as_deref()
                        == Some(&source)
                })
                .then(|| feed.active().map(|active| active.view_generation))
                .flatten()
        });
        if let Some(view_generation) = context {
            let id = begin_feed(
                client.clone(),
                owner.clone(),
                topic.clone(),
                index,
                start,
                view_generation,
            );
            requested_feed = Some(id);
            let loaded = if immutable {
                load_fixed_manifest(id, view_generation, &client, &owner, &topic, index)
                    .await
                    .and_then(|(index, manifest)| match manifest {
                        HlsManifest::Media(history) => Some((index, history)),
                        HlsManifest::Master(_) => None,
                    })
            } else {
                discover_for_view(id, view_generation, &client, &owner, &topic).await
            };
            if let Some((index, history)) = loaded
                && result_view_request_is_current(view_generation)
            {
                let plan = history.startup_plan(start);
                if install_snapshot(id, index, history, None).is_none() {
                    end_feed(id);
                    return FetchResponse::error(409, "The HLS rendition is no longer active");
                }
                FEED.with(|feed| {
                    if let Some(active) = feed.borrow_mut().get_mut(id) {
                        if immutable {
                            active.playlist.as_mut().unwrap().finalized = true;
                        }
                        active.live_startup_plan = plan;
                        active.beginning_history_started = true;
                        active.ready = true;
                        active.changed.notify(usize::MAX);
                    }
                });
            } else {
                end_feed(id);
                return FetchResponse::error(502, "The HLS rendition history could not be loaded");
            }
        }
        let refresh = FEED.with(|feed| {
            if context.is_some() || immutable {
                return None;
            }
            let mut feed = feed.borrow_mut();
            let id = feed.find(&owner, &topic, index)?.id;
            if feed.active == id {
                return None;
            }
            let active = feed
                .get_mut(id)
                .filter(|active| !active.refreshing && active.playlist.is_some())?;
            active.refreshing = true;
            Some(id)
        });
        if let Some(id) = refresh {
            recover_feed_frontier(id, &client, &owner, &topic).await;
            FEED.with(|feed| {
                if let Some(active) = feed.borrow_mut().get_mut(id) {
                    active.refreshing = false;
                    active.changed.notify(usize::MAX);
                }
            });
        }
    }
    while let Some(changed) = FEED.with(|feed| {
        let feed = feed.borrow();
        let active = feed
            .find(&owner, &topic, index)
            .filter(|active| active.refreshing || !active.ready)?;
        Some(active.changed.listen())
    }) {
        changed.await;
    }
    if requested_feed.is_some_and(|id| !feed_is_current(id)) {
        return FetchResponse::error(409, "The HLS rendition is no longer active");
    }
    let rendered = if master.is_some() {
        master
    } else if let Some(rendered) =
        render_active_feed(&owner, &topic, index, start, local_bytes_base)
    {
        Some(rendered)
    } else if owner.is_empty() {
        hls_body(client, topic, None).await.and_then(|bytes| {
            render_manifest(&bytes, local_bytes_base, start).map(|body| (0, body, None, 0))
        })
    } else if let Some(index) = index {
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
            FeedPayloadProbe::Found(payload) => {
                render_manifest(&payload.bytes, local_bytes_base, start)
                    .map(|body| (index, body, None, 0))
            }
            FeedPayloadProbe::Deferred(_)
            | FeedPayloadProbe::Missing
            | FeedPayloadProbe::Transient => None,
        }
    } else {
        discover_latest_once(&client, &owner, &topic, None)
            .await
            .and_then(|payload| {
                render_manifest(&payload.bytes, local_bytes_base, start)
                    .map(|body| (payload.index, body, None, 0))
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

        if query
            .as_ref()
            .and_then(|query| query.get("playlist"))
            .as_deref()
            == Some("1")
        {
            return Some(match reference {
                Ok(reference) => {
                    let start = FEED.with(|feed| {
                        feed.borrow()
                            .active()
                            .map_or(HlsStart::Beginning, |feed| feed.start)
                    });
                    fetch_feed_response(
                        client,
                        String::new(),
                        reference,
                        None,
                        start,
                        method,
                        &local_hls_bytes_base(pathname),
                    )
                    .await
                }
                Err(error) => FetchResponse::error(400, error),
            });
        }

        return Some(match reference {
            Ok(reference) => {
                fetch_hls_body_response(
                    client,
                    reference,
                    codec_bootstrap,
                    method,
                    range,
                    if_none_match,
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

fn local_feed_source(
    owner: &str,
    topic: &str,
    index: Option<u64>,
    bytes_base: &str,
    start: HlsStart,
) -> String {
    if owner.is_empty() {
        return format!("{bytes_base}/{topic}?playlist=1");
    }
    let base = bytes_base.strip_suffix("hls/bytes").unwrap_or_default();
    let mode = if start == HlsStart::Live {
        "live"
    } else {
        "beginning"
    };
    let mut source = format!("{base}feeds/{owner}/{topic}?start={mode}");
    if let Some(index) = index {
        source.push_str(&format!("&index={index}"));
    }
    source
}

fn local_hls_source(
    source: &str,
    bytes_base: &str,
    start: HlsStart,
    playlist: bool,
) -> Option<String> {
    match HlsSource::parse(source)? {
        HlsSource::Reference(reference) => Some(format!(
            "{bytes_base}/{reference}{}",
            if playlist { "?playlist=1" } else { "" }
        )),
        HlsSource::Feed {
            owner,
            topic,
            topic_is_hash,
            index,
        } => {
            let topic = if topic_is_hash {
                topic.to_ascii_lowercase()
            } else {
                hex::encode(crate::conventions::keccak256(topic.as_bytes()))
            };
            Some(local_feed_source(
                &owner.to_ascii_lowercase(),
                &topic,
                index,
                bytes_base,
                start,
            ))
        }
    }
}

fn render_manifest(bytes: &[u8], bytes_base: &str, start: HlsStart) -> Option<Vec<u8>> {
    Some(match HlsManifest::parse(bytes)? {
        HlsManifest::Media(playlist) => playlist.render(bytes_base, start),
        HlsManifest::Master(master) => master
            .render(|source, playlist| local_hls_source(source, bytes_base, start, playlist))
            .into_bytes(),
    })
}

async fn load_fixed_manifest(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    pinned: Option<u64>,
) -> Option<(u64, HlsManifest)> {
    if owner.is_empty() {
        let body = hls_body(client.clone(), topic.to_string(), None).await?;
        return Some((0, HlsManifest::parse(&body)?));
    }
    let index = pinned?;
    let FeedPayloadProbe::Found(payload) = probe_feed_payload(
        client,
        owner,
        topic,
        index,
        MAX_STREAM_FEED_PAYLOAD_BYTES,
        Some(FEED_PROBE_ATTEMPTS),
    )
    .await
    else {
        return None;
    };
    let manifest = match HlsManifest::parse(&payload.bytes)? {
        HlsManifest::Media(head) => HlsManifest::Media(
            hls_history(
                id,
                view_generation,
                client,
                owner,
                topic,
                index,
                index % HISTORY_STRIDE,
                head,
                HISTORY_FOREGROUND_PARALLEL,
                false,
                None,
            )
            .await?,
        ),
        master => master,
    };
    Some((index, manifest))
}

pub(crate) async fn prepare_hls_feed(
    client: Arc<Weeb3>,
    owner: String,
    topic: String,
    start: HlsStart,
    view_generation: u64,
) -> Result<PreparedHlsFeed, String> {
    let mut owner = owner
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .to_ascii_lowercase();
    let mut topic = normalize_feed_topic(&topic);
    if owner.len() != 40 || !owner.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The stream feed owner is invalid.".to_string());
    }
    clear_completed_media_ranges();
    let bytes_base = streaming_route_path(if client.service_worker_network_id() == 10 {
        "testnet/hls/bytes"
    } else {
        "hls/bytes"
    });
    let source = local_feed_source(&owner, &topic, None, &bytes_base, start);
    let result = async {
        let mut initial_source = None;
        let mut pinned = None;
        loop {
            let id = begin_feed(client.clone(), owner.clone(), topic.clone(), pinned, start, view_generation);
            FEED.with(|feed| feed.borrow_mut().active = id);
            let immutable = pinned.is_some() || owner.is_empty();
            let (index, manifest, anchor) = if immutable {
                let (index, manifest) = load_fixed_manifest(id, view_generation,
                    &client, &owner, &topic, pinned).await
                    .ok_or("The HLS playlist history could not be loaded.")?;
                (index, manifest, None)
            } else if start == HlsStart::Live {
                discover_live_history(id, view_generation, &client, &owner, &topic).await?
            } else {
                let payload = discover_beginning(id, view_generation, &client, &owner, &topic).await
                    .ok_or("The HLS feed could not be loaded.")?;
                (payload.index, HlsManifest::parse(&payload.bytes).ok_or("The HLS manifest is invalid.")?, None)
            };
            if !feed_is_current(id) || !result_view_request_is_current(view_generation) {
                return Err("HLS open was superseded".to_string());
            }
            let head = match manifest {
                HlsManifest::Media(head) => head,
                HlsManifest::Master(master) => {
                    if initial_source.is_some() { return Err("Nested HLS master playlists are invalid.".to_string()); }
                    let initial = master.initial_source().ok_or("The HLS master has no video rendition.")?;
                    initial_source = Some(local_hls_source(initial, &bytes_base, start, true)
                        .ok_or("The HLS rendition source is invalid.")?);
                    let selected = HlsSource::parse(initial).ok_or("The HLS rendition source is invalid.")?;
                    FEED.with(|feed| feed.borrow_mut().master = Some((owner.clone(), topic.clone(), index, master)));
                    end_feed(id);
                    match selected {
                        HlsSource::Reference(selected) => { owner.clear(); topic = selected; }
                        HlsSource::Feed { owner: selected_owner, topic: selected_topic, topic_is_hash, index } => {
                            owner = selected_owner.to_ascii_lowercase();
                            topic = if topic_is_hash { selected_topic.to_ascii_lowercase() }
                                else { hex::encode(crate::conventions::keccak256(selected_topic.as_bytes())) };
                            pinned = index;
                        }
                    }
                    continue;
                }
            };
            let underfilled = head.segments.iter().filter(|segment| !segment.gap).count() < BEGINNING_PREFIX_TARGET_SEGMENTS;
            let plan = head.startup_plan(start);
            install_snapshot(id, index, head, None)
                .ok_or("The HLS feed history did not match its live updates.")?;
            let (index, plan, start_sequence) = if let Some(anchor) = &anchor {
                prepare_live_plan(id, view_generation, anchor).await?
            } else {
                let plan = plan.ok_or("The HLS feed does not contain a playable startup runway.")?;
                FEED.with(|feed| {
                    if let Some(active) = feed.borrow_mut().get_mut(id) {
                        if start == HlsStart::Live { active.live_startup_plan = Some(plan.clone()); }
                        if immutable {
                            active.playlist.as_mut().unwrap().finalized = true;
                            active.beginning_history_started = true;
                        }
                    }
                });
                if underfilled && !immutable { spawn_follower(id); }
                (index, plan, 0)
            };
            let elapsed = plan.duration;
            FEED.with(|feed| {
                if let Some(active) = feed.borrow_mut().get_mut(id) {
                    active.ready = true;
                    active.changed.notify(usize::MAX);
                }
            });
            client.interface_log(if let Some((sequence, reference, _)) = anchor {
                format!("HLS open index={index} elapsed={elapsed:.3}s mode=live anchor={sequence} anchor_ref={reference} start_seq={start_sequence} start={:.6}s end={:.6}s", plan.play_position, plan.runway_end)
            } else {
                format!("HLS open index={index} elapsed={elapsed:.3}s mode=beginning")
            });
            return Ok(PreparedHlsFeed { source, initial_source, plan });
        }
    }.await;
    if result.is_err() && result_view_request_is_current(view_generation) {
        release_hls_runtime();
        clear_hls_runtime_cache();
    }
    result
}
pub(crate) fn release_hls_runtime() {
    FEED.with(|feed| *feed.borrow_mut() = FeedSessions::default());
}

pub(crate) fn clear_hls_runtime_cache() {
    BODY_CACHE.with(|cache| cache.borrow_mut().clear());
    clear_completed_media_ranges();
}
