use std::{
    cell::{Cell, RefCell},
    collections::{HashSet, VecDeque, hash_map::RandomState},
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use event_listener::Event;
use futures::{FutureExt, StreamExt, future, stream};
use hashlink::LinkedHashMap;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use super::{
    HISTORY_STRIDE, HLS_BEGINNING_STARTUP_BUFFER_SECONDS, HLS_BODY_MAX_BYTES,
    HLS_LIVE_BODY_RUNWAY_SEGMENTS, HLS_LIVE_EDGE_SEGMENTS, HLS_LIVE_STARTUP_BUFFER_SECONDS,
    HlsManifest, HlsMasterPlaylist, HlsPlaylist, HlsSource, HlsStart,
    MAX_STREAM_FEED_PAYLOAD_BYTES, PreparedHlsFeed, confirm_hls_feed_head, history_indices,
    hls_edge_wave_complete, hls_payload_mime, hls_progressive_foreground_transition,
    is_hex_reference,
};
use crate::{
    ChunkRetrieveRequest, Weeb3,
    bzz_stream::{
        FeedPayloadRoot, decode_feed_payload_root, retrieve_feed_payload,
        retrieve_feed_payload_tail,
    },
    feed::FeedProbe,
    get_feed_address, normalize_feed_topic, oneshot,
    retrieval::retrieve_decoded_data_root,
    retrieval_conventions::RetrieveAdmission,
    stream::{
        FetchResponse, clear_completed_media_ranges, forget_completed_reference_ranges,
        media_cache_max_bytes, read_cached_hls_range, result_view_request_is_current,
        set_auxiliary_media_cache_bytes,
    },
    stream_conventions::{
        MEDIA_STARTUP_RESPONSE_BYTES, STREAMING_ROUTE_BASE, decode_component,
        if_none_match_matches, route_resource, streaming_route_path,
    },
};

const HLS_BODY_CACHE_MAX_BYTES: u64 = 32 * 1024 * 1024;
const BODY_PREFETCH_HORIZON: usize = HLS_LIVE_BODY_RUNWAY_SEGMENTS;
const BEGINNING_DISCOVERY_WIDTH: u64 = 8;
const BEGINNING_PAYLOAD_BYTES: usize = 64 * 1024;
const EDGE_COLD_WAVE_TIMEOUT: Duration = Duration::from_millis(4_000);
const EDGE_WAVE_TIMEOUT: Duration = Duration::from_millis(1_500);
const EDGE_REFINEMENT_WIDTH: usize = 16;
#[rustfmt::skip]
const EDGE_ANCHORS: [u64; 16] = [
    0, 1, 7, 15, 31, 127, 255, 511,
    1_023, 2_047, 4_095, 8_191, 16_383, 65_535, 1_048_575, u64::MAX,
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
    bodies: LinkedHashMap<String, CachedBody, RandomState>,
    bytes: u64,
}

struct CachedBody {
    bytes: Bytes,
    retrieval_ms: f64,
}

impl BodyCache {
    fn get(&self, reference: &str, start: u64, end: u64) -> Option<Bytes> {
        let body = &self.bodies.get(reference)?.bytes;
        let start = usize::try_from(start).ok()?;
        let end = usize::try_from(end).ok()?.checked_add(1)?;
        body.get(start..end)?;
        Some(body.slice(start..end))
    }

    fn body(&self, reference: &str) -> Option<Bytes> {
        self.bodies.get(reference).map(|body| body.bytes.clone())
    }

    fn finish_body(
        &mut self,
        reference: String,
        epoch: u64,
        body: Option<Bytes>,
        retrieval_ms: f64,
    ) -> Option<Bytes> {
        if epoch != self.epoch {
            return None;
        }
        if let Some(body) = &body
            && body.len() as u64 <= media_cache_max_bytes().min(HLS_BODY_CACHE_MAX_BYTES)
            && !self.bodies.contains_key(&reference)
        {
            forget_completed_reference_ranges(&reference);
            self.bodies.insert(
                reference,
                CachedBody {
                    bytes: body.clone(),
                    retrieval_ms,
                },
            );
            self.bytes = self.bytes.saturating_add(body.len() as u64);
            self.trim();
        }
        body
    }

    fn trim(&mut self) {
        let maximum = media_cache_max_bytes().min(HLS_BODY_CACHE_MAX_BYTES);
        while self.bytes > maximum {
            let Some((_, body)) = self.bodies.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(body.bytes.len() as u64);
        }
        set_auxiliary_media_cache_bytes(self.bytes);
    }

    fn clear(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.bodies.clear();
        self.bytes = 0;
        set_auxiliary_media_cache_bytes(0);
    }
}

#[derive(Default)]
struct FeedSessions {
    active: u64,
    history_changed: Event,
    origin: Option<f64>,
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
    updated_at: f64,
    playlist: Option<HlsPlaylist>,
    presentation_gaps: HashSet<(u64, String)>,
    tail_fallbacks: VecDeque<f64>,
}

impl FeedSession {
    fn position_of(&self, reference: &str) -> Option<usize> {
        let mut positions = self.playlist.as_ref()?.segments.iter().enumerate();
        let live = self.start == HlsStart::Live;
        let matches = |(position, segment): &(usize, &super::HlsSegment)| {
            segment.reference == reference
                && if live {
                    live_segment_is_playable(self, *position)
                } else {
                    !segment.gap
                }
        };
        if live {
            positions.rfind(matches)
        } else {
            positions.find(matches)
        }
        .map(|(position, _)| position)
    }
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

type LiveManifest = (u64, HlsManifest, Option<(u64, String, bool)>);

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
    let (output, input) = oneshot::channel();
    if client
        .chunk_port
        .0
        .try_send(ChunkRetrieveRequest {
            address,
            chan: crate::ChunkRetrieveReply::Channel(output),
            cancel: None,
            admission: Some(admission.clone()),
            hedge_demand: None,
        })
        .is_err()
    {
        return FeedProbe::Transient;
    }
    match input.await {
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
    feed: Option<u64>,
    admitted: &dyn Fn() -> bool,
) -> Option<Bytes> {
    let epoch = BODY_CACHE.with(|cache| cache.borrow().epoch);
    if let Some(bytes) = BODY_CACHE.with(|cache| cache.borrow().get(reference, start, end)) {
        return Some(bytes);
    }
    let current = || BODY_CACHE.with(|cache| cache.borrow().epoch == epoch) && admitted();
    let read = read_cached_hls_range(client, reference, span, start, end, &current);
    futures::pin_mut!(read);
    loop {
        let changed = feed.and_then(|id| {
            FEED.with(|feeds| feeds.borrow().get(id).map(|feed| feed.changed.listen()))
        });
        if !current() {
            return None;
        }
        let Some(changed) = changed else {
            return read.await;
        };
        if let future::Either::Left((body, _)) = future::select(read.as_mut(), changed).await {
            return body;
        }
    }
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
    if let Some(body) = BODY_CACHE.with(|cache| cache.borrow().body(&reference)) {
        return Some(body);
    }
    let epoch = BODY_CACHE.with(|cache| cache.borrow().epoch);
    let started = js_sys::Date::now();
    let body = {
        let decoded = hex::decode(&reference).ok()?;
        let root = retrieve_decoded_data_root(&decoded, &client.chunk_port.0, None).await?;
        if root.span == 0
            || root.span > HLS_BODY_MAX_BYTES
            || BODY_CACHE.with(|cache| cache.borrow().epoch != epoch)
        {
            return None;
        }
        let end = root.span.checked_sub(1)?;
        hls_range(&client, &reference, root.span, 0, end, generation, &|| {
            generation.is_none_or(|id| body_is_current(id, &reference))
        })
        .await
    };
    BODY_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .finish_body(reference, epoch, body, js_sys::Date::now() - started)
    })
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
    let foreground = active
        .foreground
        .as_deref()
        .and_then(|reference| active.position_of(reference))
        .filter(|position| playlist.sequence.checked_add(*position as u64).is_some());
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
            if !active.ready || *offset != 0 {
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

fn spawn_body_runway(id: u64) {
    let Some(client) = FEED.with_borrow_mut(|feed| {
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
        let mut loaded = HashSet::new();
        loop {
            let reference = FEED.with_borrow(|feed| {
                let active = feed.active().filter(|active| active.id == id)?;
                let playlist = active.playlist.as_ref()?;
                let first = playlist.segments.iter().find(|segment| !segment.gap)?;
                let foreground = active.foreground.as_deref().filter(|reference| {
                    active.start == HlsStart::Beginning && *reference != first.reference
                });
                let targets = body_runway_targets(active);
                loaded.retain(|reference| {
                    targets.iter().any(|segment| !segment.gap && &segment.reference == reference)
                });
                targets
                    .iter()
                    .filter(|segment| {
                        !segment.gap
                            && !loaded.contains(&segment.reference)
                            && !BODY_CACHE.with(|cache| cache.borrow().bodies.contains_key(&segment.reference))
                    })
                    .min_by_key(|segment| foreground == Some(segment.reference.as_str()))
                    .map(|segment| segment.reference.clone())
            });
            let Some(reference) = reference else {
                break;
            };
            if hls_body(client.clone(), reference.clone(), Some(id)).await.is_some() {
                loaded.insert(reference);
            } else if body_is_current(id, &reference) {
                async_std::task::sleep(Duration::from_millis(HLS_BODY_RETRY_DELAY_MS)).await;
            }
        }
        FEED.with(|feed| {
            if let Some(active) = feed.borrow_mut().get_mut(id) {
                active.body_runway_running = false;
            }
        });
    });
}

fn prefetch_from_reference(reference: &str, cached: bool) -> bool {
    let runway = FEED.with_borrow_mut(|feed| {
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
        let live = active.start == HlsStart::Live;
        let position = active.position_of(reference)?;
        let (transition, foreground) = active
            .foreground
            .as_deref()
            .filter(|_| !live || active.ready)
            .and_then(|reference| active.position_of(reference))
            .map_or((false, position), |last| {
                hls_progressive_foreground_transition(last, position, cached)
            });
        if foreground == position {
            active.foreground = Some(reference.to_string());
        }
        active.changed.notify(usize::MAX);
        Some((
            active.id,
            live || active.beginning_history_started,
            transition || live || playlist.segments[..position].iter().any(|segment| !segment.gap),
        ))
    });
    let Some((id, follow, complete)) = runway else {
        return false;
    };
    if follow {
        spawn_follower(id);
        spawn_body_runway(id);
    }
    complete
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
    FEED.with_borrow_mut(|feed| {
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
            updated_at: 0.0,
            playlist: None,
            presentation_gaps: HashSet::new(),
            tail_fallbacks: VecDeque::new(),
        });
    });
    id
}

fn feed_is_current(id: u64, view_generation: u64) -> bool {
    FEED.with(|feed| feed.borrow().get(id).is_some())
        && result_view_request_is_current(view_generation)
}

fn end_feed(id: u64) {
    FEED.with_borrow_mut(|feed| {
        feed.feeds.retain(|feed| feed.id != id);
    });
}

fn install_snapshot(
    id: u64,
    index: u64,
    mut playlist: HlsPlaylist,
) -> Option<()> {
    FEED.with_borrow_mut(|feed| {
        let active = feed.get_mut(id)?;
        if let Some(tail) = active.playlist.take() {
            playlist.merge_playlist(tail)?;
        } else {
            active.index = Some(index);
            playlist.finalized = false;
        }
        active.playlist = Some(playlist);
        active.updated_at = js_sys::Date::now();
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
    apply_confirmed_snapshot(
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
        let (changed, prepared) = FEED.with_borrow_mut(|feed| {
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
                .filter(|(_, first)| !anchor.2 || playlist.finalized || *first == position);
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
    while feed_is_current(id, view_generation) {
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
            if !feed_is_current(id, view_generation) {
                return None;
            }
            if let FeedPayloadProbe::Found(payload) = probe {
                match HlsManifest::parse(&payload.bytes) {
                    Some(HlsManifest::Master(_)) => {
                        return discover_raw_for_view(id, view_generation, client, owner, topic).await;
                    }
                    Some(HlsManifest::Media(playlist))
                        if playlist.sequence == 0
                            && playlist.startup_plan(HlsStart::Beginning).is_some() =>
                    {
                        warm_hls_prefix(
                            client.clone(),
                            id,
                            view_generation,
                            &playlist,
                            HlsStart::Beginning,
                        );
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
) -> (Option<(usize, Vec<u8>)>, Vec<bool>) {
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
    let mut found = None;
    let mut missing = vec![false; indices.len()];
    let mut completed = vec![false; indices.len()];
    let deadline = if lower_is_known {
        future::pending::<()>().left_future()
    } else {
        async_std::task::sleep(EDGE_COLD_WAVE_TIMEOUT).right_future()
    };
    futures::pin_mut!(deadline);
    while let future::Either::Left((Some((slot, result)), _)) =
        future::select(probes.next(), deadline.as_mut()).await
    {
        completed[slot] = true;
        match result {
            FeedProbe::Found(update) => {
                if found.is_none() {
                    deadline.set(async_std::task::sleep(EDGE_WAVE_TIMEOUT).right_future());
                }
                if found.as_ref().is_none_or(|(highest, _)| slot > *highest) {
                    found = Some((slot, update));
                }
            }
            FeedProbe::Missing => missing[slot] = true,
            FeedProbe::Transient => {}
        }
        let first_unsettled = found.as_ref().map_or(0, |(slot, _)| slot + 1);
        if (found.is_some() || lower_is_known)
            && hls_edge_wave_complete(first_unsettled, &missing, &completed)
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
    lattice: Option<&Cell<Option<u64>>>,
) -> Option<(u64, Vec<u8>)> {
    let fast = Some(EDGE_PROBE_ATTEMPTS);
    let (found, missing) =
        edge_probe_wave(client, owner, topic, &EDGE_ANCHORS, false, fast).await;
    let (highest, update) = found?;
    let mut latest = (EDGE_ANCHORS[highest], update);
    client.interface_log(format!("HLS edge anchor {}", latest.0));
    let mut upper = (highest + 1..EDGE_ANCHORS.len())
        .find(|slot| missing[*slot])
        .map(|slot| EDGE_ANCHORS[slot])?;

    // Keep the first positive lattice across confirmation retries.
    if let Some(lattice) = lattice
        && lattice.get().is_none()
    {
        lattice.set(Some(latest.0 % HISTORY_STRIDE));
    }
    // Aim for two refinement waves within three original waves' probe budget.
    let width = ((upper - latest.0) as f64 / (HISTORY_STRIDE * 2) as f64)
        .sqrt()
        .ceil() as usize
        + 1;
    let width = if width <= 24 {
        width.max(EDGE_REFINEMENT_WIDTH)
    } else {
        EDGE_REFINEMENT_WIDTH
    };
    loop {
        if upper.saturating_sub(latest.0) <= HISTORY_STRIDE * 2 {
            return Some(latest);
        }
        let width = (upper - latest.0).min(width as u64) as usize;
        let first = latest.0 + 1;
        let span = u128::from(upper - first);
        let divisor = (width - 1) as u128;
        let indices = std::iter::once(first)
            .chain(
                (1..width - 1)
                    .map(|position| first + (span * position as u128).div_ceil(divisor) as u64),
            )
            .chain(std::iter::once(upper))
            .collect::<Vec<_>>();
        let (found, missing) =
            edge_probe_wave(client, owner, topic, &indices, true, fast).await;
        let previous = (latest.0, upper);
        if let Some((slot, update)) = found {
            latest = (indices[slot], update);
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
    lattice: Option<&Cell<Option<u64>>>,
) -> Option<RawFeedPayload> {
    let (index, update) = discover_edge_update(client, owner, topic, lattice).await?;
    retrieve_confirmed_payload(client, owner, topic, index, update).await
}

async fn retrieve_confirmed_payload(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    index: u64,
    update: Vec<u8>,
) -> Option<RawFeedPayload> {
    let lattice_residue = index % HISTORY_STRIDE;
    let (index, update) = confirm_hls_feed_head(index, update, HISTORY_STRIDE * 2, |index| {
        probe_feed_update(client, owner, topic, index, Some(FEED_PROBE_ATTEMPTS))
    }).await?;
    let root = decode_feed_payload_root(index, update)?;
    let bytes =
        retrieve_feed_payload(&root, MAX_STREAM_FEED_PAYLOAD_BYTES, &client.chunk_port.0).await?;
    Some(RawFeedPayload {
        index,
        lattice_residue,
        bytes,
    })
}

fn warm_hls_prefix(
    client: Arc<Weeb3>,
    id: u64,
    view_generation: u64,
    playlist: &HlsPlaylist,
    start: HlsStart,
) {
    let Some(segment) = playlist
        .segments
        .iter()
        .find(|segment| !segment.gap)
        .cloned()
    else {
        return;
    };
    spawn_local(async move {
        let current = || feed_is_current(id, view_generation);
        let epoch = BODY_CACHE.with(|cache| cache.borrow().epoch);
        let started = js_sys::Date::now();
        let body = async {
            if !current() {
                return None;
            }
            let decoded = hex::decode(&segment.reference).ok()?;
            let root = retrieve_decoded_data_root(&decoded, &client.chunk_port.0, None).await?;
            if !current() || root.span == 0 {
                return None;
            }
            let maximum = if start == HlsStart::Beginning {
                ((root.span as f64
                    * (HLS_BEGINNING_STARTUP_BUFFER_SECONDS / segment.duration).min(1.0))
                .ceil() as u64)
                    .clamp(1, root.span.min(MEDIA_STARTUP_RESPONSE_BYTES))
            } else if root.span <= HLS_BODY_MAX_BYTES {
                root.span
            } else {
                return None;
            };
            hls_range(
                &client,
                &segment.reference,
                root.span,
                0,
                maximum - 1,
                Some(id),
                &current,
            )
            .await
        }
        .await;
        if start == HlsStart::Beginning {
            start_beginning_history(id);
        } else {
            let _ = BODY_CACHE.with(|cache| {
                cache.borrow_mut().finish_body(
                    segment.reference,
                    epoch,
                    body,
                    js_sys::Date::now() - started,
                )
            });
        }
    });
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
    origin: Option<f64>,
) -> Vec<(u64, HlsPlaylist)> {
    let probes = stream::iter(indices.iter().copied()).map(|index| async move {
        if !feed_is_current(id, view_generation) {
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
    });
    let mut probes = if origin.is_some() {
        probes.buffered(parallel).left_stream()
    } else {
        probes.buffer_unordered(parallel).right_stream()
    };
    let mut snapshots = Vec::new();
    while let Some(snapshot) = probes.next().await {
        if let Some((index, playlist)) = snapshot {
            if *warm_pending
                && playlist.sequence == 0
                && playlist.segments.iter().any(|segment| !segment.gap)
                && feed_is_current(id, view_generation)
            {
                *warm_pending = false;
                warm_hls_prefix(
                    client.clone(),
                    id,
                    view_generation,
                    &playlist,
                    HlsStart::Live,
                );
            }
            let covered = origin.is_some_and(|date| {
                playlist
                    .program_start(js_sys::Date::parse)
                    .is_some_and(|start| start <= date)
            });
            snapshots.push((index, playlist));
            if covered {
                break;
            }
        }
    }
    snapshots
}

fn history_repairs(
    attempted: &[u64],
    snapshots: &mut [(u64, HlsPlaylist)],
    head_index: u64,
    from_beginning: bool,
) -> Option<Vec<u64>> {
    HlsPlaylist::sort_snapshots(snapshots);
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
    let (first_index, first) = snapshots.first()?;
    if from_beginning && first.sequence != 0 {
        add(0..*first_index)?;
    }
    for pair in snapshots.windows(2) {
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
    mut head: HlsPlaylist,
    parallel: usize,
    mut warm_pending: bool,
    earlier: Option<f64>,
) -> Option<HlsPlaylist> {
    let head_start = head.program_start(js_sys::Date::parse);
    let origin = earlier.or_else(|| FEED.with_borrow(|feed| {
        if feed.get(id)?.start != HlsStart::Live || head_start.is_none() {
            return None;
        }
        let first = feed.feeds.iter().find(|feed| feed.ready)?;
        first.playlist.as_ref()?.program_start(js_sys::Date::parse)
    }));
    if head.sequence != 0
        && origin
            .zip(head_start)
            .is_none_or(|(origin, start)| origin < start)
    {
        let mut indices = history_indices(head_index, lattice_residue, origin.is_some())?;
        if origin.is_some() {
            indices.reverse();
        }
        let mut snapshots = history_snapshots(
            id,
            view_generation,
            client,
            owner,
            topic,
            &indices,
            parallel,
            &mut warm_pending,
            origin,
        )
        .await;
        indices.sort_unstable();
        snapshots.push((head_index, head));
        let repairs = history_repairs(&indices, &mut snapshots, head_index, origin.is_none())?;
        let start = if origin.is_some() {
            snapshots.first()?.1.sequence
        } else {
            0
        };
        let (_, latest) = snapshots.pop()?;
        snapshots.extend(
            history_snapshots(
                id,
                view_generation,
                client,
                owner,
                topic,
                &repairs,
                parallel,
                &mut warm_pending,
                None,
            )
            .await,
        );
        head = HlsPlaylist::reconstruct(snapshots, head_index, latest, start)?;
    }
    if warm_pending && head.sequence == 0 {
        warm_hls_prefix(client.clone(), id, view_generation, &head, HlsStart::Live);
    }
    if let Some(origin) = origin {
        let date = head.retain_from_date(origin, js_sys::Date::parse)?;
        head.segments[0].program_date_time.get_or_insert_with(|| {
            js_sys::Date::new(&JsValue::from_f64(date))
                .to_iso_string()
                .into()
        });
    }
    Some(head)
}

async fn discover_raw_for_view(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
) -> Option<RawFeedPayload> {
    loop {
        if !feed_is_current(id, view_generation) {
            return None;
        }
        if let Some(payload) = discover_latest_once(client, owner, topic, None).await {
            return Some(payload);
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
}

async fn discover_live_with_rendition(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
) -> Result<(LiveManifest, Option<(u64, HlsPlaylist)>), &'static str> {
    let current = || feed_is_current(id, view_generation);
    let lattice = Cell::new(None);
    let (payload, selected, provisional) = loop {
        if !current() {
            return Err("HLS open was superseded");
        }
        if let Some((index, update)) =
            discover_edge_update(client, owner, topic, Some(&lattice)).await
        {
            // A leaf-sized hint is already inside the authenticated update.
            // Larger playlists proceed directly to confirmation without another read.
            let selected = async {
                let root = decode_feed_payload_root(index, update.clone())?;
                let bytes =
                    retrieve_feed_payload(&root, FEED_TAIL_PROBE_BYTES, &client.chunk_port.0)
                        .await?;
                let HlsManifest::Master(master) = HlsManifest::parse(&bytes)? else {
                    return None;
                };
                HlsSource::parse(master.initial_source()?)
            }
            .await;
            let matches = |payload: &RawFeedPayload| {
                matches!(HlsManifest::parse(&payload.bytes), Some(HlsManifest::Master(master))
                    if selected.as_ref().is_some_and(|source| master.selects(source)))
            };
            let provisional = async {
                let HlsSource::Feed {
                    owner,
                    topic,
                    topic_is_hash,
                    index: None,
                } = selected.as_ref()?
                else {
                    return None;
                };
                let topic = if *topic_is_hash {
                    topic.clone()
                } else {
                    hex::encode(crate::conventions::keccak256(topic.as_bytes()))
                };
                discover_latest_once(client, owner, &topic, None).await
            };
            let confirmed = retrieve_confirmed_payload(client, owner, topic, index, update);
            let (payload, provisional) =
                match future::select(Box::pin(confirmed), Box::pin(provisional)).await {
                    future::Either::Left((payload, pending)) => {
                        let provisional = if payload.as_ref().is_some_and(matches) {
                            pending.await
                        } else {
                            None
                        };
                        (payload, provisional)
                    }
                    future::Either::Right((provisional, pending)) => (pending.await, provisional),
                };
            if let Some(payload) = payload {
                break (payload, selected, provisional);
            }
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    };
    if !current() {
        return Err("HLS open was superseded");
    }
    let manifest =
        HlsManifest::parse(&payload.bytes).ok_or("The HLS manifest could not be read.")?;
    let index = payload.index;
    let HlsManifest::Media(head) = manifest else {
        // A coarse master is only a hint; the confirmed master owns selection.
        let rendition = selected.zip(provisional).and_then(|(selected, payload)| {
            let HlsManifest::Master(master) = &manifest else {
                return None;
            };
            if !master.selects(&selected) {
                return None;
            }
            let playlist = HlsPlaylist::parse(&payload.bytes)?;
            playlist.has_timeline().then_some((payload.index, playlist))
        });
        return Ok(((index, manifest, None), rendition));
    };
    // Dated windows already carry the common timeline used by every rendition.
    // Reading the broadcast's entire history delays live playback as it grows.
    if head.has_timeline() {
        return Ok(((index, HlsManifest::Media(head), None), None));
    }
    let anchor = initialize_live_head(id, index, &head)
        .ok_or("The HLS feed does not contain a playable startup runway.")?;
    let history = hls_history(
        id,
        view_generation,
        client,
        owner,
        topic,
        index,
        lattice.get().unwrap_or(payload.lattice_residue),
        head,
        HISTORY_FOREGROUND_PARALLEL,
        true,
        None,
    )
    .await
    .ok_or("The HLS feed history could not be reconstructed.")?;
    if !current() {
        return Err("HLS open was superseded");
    }
    Ok(((index, HlsManifest::Media(history), Some(anchor)), None))
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
            discover_raw_for_view(id, view_generation, client, owner, topic).await?;
        if let Some(head) = HlsPlaylist::parse(&payload.bytes) {
            if let Some(history) = hls_history(
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
        }
        async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
    }
}

fn has_dated_runway(playlist: &HlsPlaylist, date: f64, required: usize) -> bool {
    let mut clock = playlist.program_start(js_sys::Date::parse).unwrap_or(f64::NAN);
    let mut runway = 0;
    for segment in &playlist.segments {
        let start = segment.program_date_time.as_deref().map(js_sys::Date::parse).unwrap_or(clock);
        if runway != 0 && !((start - clock).abs() <= 1.0) {
            return false;
        }
        clock = start + segment.duration * 1_000.0;
        if runway != 0 || date >= start && date < clock {
            if segment.gap { return false; }
            runway += 1;
            if runway == if playlist.finalized { required.min(2) } else { required } { return true; }
        }
    }
    false
}

fn dated_end(playlist: &HlsPlaylist) -> Option<f64> {
    playlist.segments.iter().try_fold(playlist.program_start(js_sys::Date::parse)?, |clock, segment| {
        let start = segment.program_date_time.as_deref().map(js_sys::Date::parse).unwrap_or(clock);
        let end = start + segment.duration * 1_000.0;
        (start.is_finite() && start + 1.0 >= clock && end.is_finite()).then_some(end)
    })
}

async fn discover_at_date(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    date: f64,
    required: usize,
) -> Option<Result<(u64, HlsPlaylist), u64>> {
    let payload = discover_latest_once(client, owner, topic, None).await?;
    let head = HlsPlaylist::parse(&payload.bytes)?;
    if !head.has_timeline() {
        return discover_for_view(id, view_generation, client, owner, topic).await.map(Ok);
    }
    if has_dated_runway(&head, date, required) {
        return Some(Ok((payload.index, head)));
    }
    // Only a fresh confirmed frontier may prove this quality ended before the target.
    if head.finalized && date >= dated_end(&head)? { return Some(Err(payload.index)); }
    if head.program_start(js_sys::Date::parse)? <= date {
        return None;
    }
    let (mut lower, mut upper) = (0, payload.index);
    let mut found = None;
    // Dated windows locate the playhead without reading every earlier update.
    while lower < upper && feed_is_current(id, view_generation) {
        let index = lower + (upper - lower) / 2;
        let FeedPayloadProbe::Found(payload) = probe_feed_payload(
            client, owner, topic, index, MAX_STREAM_FEED_PAYLOAD_BYTES,
            Some(FEED_PROBE_ATTEMPTS),
        ).await else {
            return None;
        };
        let playlist = HlsPlaylist::parse(&payload.bytes)?;
        if playlist.program_start(js_sys::Date::parse)? > date {
            upper = index;
        } else {
            if has_dated_runway(&playlist, date, required) {
                found = Some((index, playlist));
            }
            lower = index + 1;
        }
    }
    found.map(Ok)
}

fn render_active_feed(
    owner: &str,
    topic: &str,
    pinned: Option<u64>,
    start: HlsStart,
    local_bytes_base: &str,
) -> Option<(u64, Vec<u8>, Option<u64>, u64)> {
    FEED.with_borrow(|feed| {
        let feed = feed.find(owner, topic, pinned)?;
        let index = feed.index?;
        let playlist = if start == HlsStart::Live {
            presentation_playlist(feed)?
        } else {
            std::borrow::Cow::Borrowed(feed.playlist.as_ref()?)
        };
        let body =
            playlist.render_with_plan(local_bytes_base, start, feed.live_startup_plan.as_ref());
        let follower =
            (start == HlsStart::Live && !playlist.finalized && !feed.following).then_some(feed.id);
        Some((index, body, follower, feed.presentation_gaps.len() as u64))
    })
}

fn apply_update(
    id: u64,
    index: u64,
    merge: impl FnOnce(&mut HlsPlaylist) -> Option<usize>,
) -> Option<(usize, bool)> {
    let (appended, closing) = FEED.with_borrow_mut(|feed| {
        let active = feed.get_mut(id)?;
        if active.index.is_some_and(|current| index <= current) {
            return None;
        }
        let playlist = active.playlist.as_mut()?;
        let appended = merge(playlist)?;
        let closing = std::mem::take(&mut playlist.finalized);
        active.index = Some(index);
        active.updated_at = js_sys::Date::now();
        active.changed.notify(usize::MAX);
        Some((appended, closing))
    })?;
    if appended != 0 {
        spawn_body_runway(id);
    }
    Some((appended, closing))
}

fn retain_feed_start(id: u64, candidate: &mut HlsPlaylist) -> Option<()> {
    FEED.with_borrow(|feed| {
        let active = feed.get(id)?;
        if let Some(playlist) = active
            .playlist
            .as_ref()
            .filter(|playlist| playlist.has_timeline())
        {
            candidate.retain_from(playlist.sequence)?;
        }
        Some(())
    })
}

fn apply_full_update(id: u64, index: u64, mut candidate: HlsPlaylist) -> Option<(usize, bool)> {
    retain_feed_start(id, &mut candidate)?;
    apply_update(id, index, |playlist| playlist.merge_playlist(candidate))
}

// Only fresh frontier-confirmed results or immutable complete views enter here.
fn apply_confirmed_snapshot(
    id: u64,
    index: u64,
    mut candidate: HlsPlaylist,
    foreground: Option<String>,
) -> Option<usize> {
    retain_feed_start(id, &mut candidate)?;
    let appended = FEED.with_borrow_mut(|feed| {
        let active = feed.get_mut(id).filter(|active| {
            result_view_request_is_current(active.view_generation)
                && active.index.is_none_or(|current| index >= current)
        })?;
        let appended = if let Some(playlist) = active.playlist.as_mut() {
            candidate.finalized |= active.index == Some(index) && playlist.finalized;
            playlist.merge_playlist(candidate)?
        } else {
            active.playlist = Some(candidate);
            0
        };
        active.index = Some(index);
        active.updated_at = js_sys::Date::now();
        if foreground.is_some() {
            active.foreground = foreground;
        }
        active.changed.notify(usize::MAX);
        Some(appended)
    })?;
    if appended != 0 {
        spawn_body_runway(id);
    }
    Some(appended)
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
    FEED.with_borrow(|feed| {
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
    FEED.with_borrow_mut(|feed| {
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
        // Return a distance: the page owns this rendition's media-clock offset.
        let target = playlist.segments[retreat..failed].iter().map(|segment| segment.duration).sum::<f64>();
        let now = js_sys::Date::now();
        while active
            .tail_fallbacks
            .front()
            .is_some_and(|at| now - at >= LIVE_TAIL_FALLBACK_WINDOW_MS)
        {
            active.tail_fallbacks.pop_front();
        }
        if !target.is_finite() || active.tail_fallbacks.len() >= LIVE_TAIL_FALLBACK_LIMIT
            || !active
                .presentation_gaps
                .insert((sequence, reference.to_string()))
        {
            return None;
        }
        active.tail_fallbacks.push_back(now);
        active.changed.notify(usize::MAX);
        Some(target)
    })
}

fn feed_follow_context(id: u64) -> Option<(Arc<Weeb3>, String, String, u64, u64)> {
    FEED.with_borrow(|feed| {
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
) -> Option<(usize, bool)> {
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

fn start_beginning_history(id: u64) {
    let context = FEED.with_borrow_mut(|feed| {
        let active = feed.get_mut(id).filter(|active| {
            result_view_request_is_current(active.view_generation)
                && active.start == HlsStart::Beginning
                && !active.beginning_history_started
        })?;
        active.beginning_history_started = true;
        let successor = active
            .playlist
            .iter()
            .flat_map(|playlist| &playlist.segments)
            .filter(|segment| !segment.gap)
            .nth(1)
            .map(|segment| segment.reference.clone());
        Some((
            active.view_generation,
            active.client.clone(),
            active.owner.clone(),
            active.topic.clone(),
            successor,
        ))
    });
    let Some((view_generation, client, owner, topic, successor)) = context else {
        return;
    };
    spawn_body_runway(id);
    spawn_local(async move {
        if !feed_is_current(id, view_generation) {
            return;
        }
        spawn_follower(id);
        if let Some(successor) = successor {
            let _ = hls_body(client.clone(), successor, Some(id)).await;
        }
        if !feed_is_current(id, view_generation) {
            return;
        }
        let history = discover_for_view(id, view_generation, &client, &owner, &topic).await;
        if let Some((index, history)) = history
            && apply_confirmed_snapshot(id, index, history, None).is_some()
        {
            client.interface_log(format!("HLS history reached index {index}"));
        }
    });
}

fn spawn_follower(id: u64) {
    let claimed = FEED.with_borrow_mut(|feed| {
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
                let mut progressed = None;
                let mut skipped_missing_index = false;
                let mut probes = stream::iter(1..=FEED_FOLLOW_AHEAD)
                    .map(async |offset| {
                        let index = head.checked_add(offset)?;
                        Some((
                            index,
                            probe_feed_payload(
                                &client,
                                &owner,
                                &topic,
                                index,
                                FEED_TAIL_PROBE_BYTES,
                                None,
                            )
                            .await,
                        ))
                    })
                    .buffered(2);
                while let Some(candidate) = probes.next().await {
                    let Some((index, candidate)) = candidate else {
                        return;
                    };
                    if !FEED.with_borrow(|feed| feed.active == id && feed.get(id).is_some()) {
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
                    let Some((appended, closing)) = appended else {
                        break;
                    };
                    progressed = Some(closing);
                    if appended != 0 {
                        client.interface_log(format!(
                            "HLS feed advanced to {index}; appended {appended} segment(s)"
                        ));
                    }
                    if closing {
                        break;
                    }
                }
                drop(probes);
                if let Some(closing) = progressed {
                    if closing {
                        recover_feed_frontier(id, &client, &owner, &topic).await;
                    }
                    last_frontier_check = js_sys::Date::now();
                    continue;
                }

                async_std::task::sleep(FEED_POLL_INTERVAL).await;
                if !FEED.with_borrow(|feed| feed.active == id && feed.get(id).is_some()) {
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
    if index < head {
        return false;
    }
    let appended = if let Some(appended) = HlsPlaylist::parse(&payload.bytes)
        .and_then(|playlist| apply_confirmed_snapshot(id, index, playlist, None))
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
        let Some(appended) = apply_confirmed_snapshot(id, index, history, None) else {
            return false;
        };
        appended
    };
    if feed_follow_context(id).is_none() {
        client.interface_log(format!("HLS feed finalized at {index}"));
    } else if appended != 0 {
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
    reference: &str,
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
        (200, 0, body.len(), None)
    };
    let mut headers = hls_body_headers(
        format!("\"{reference}\""),
        mime,
        (end - start) as u64,
        range,
    );
    BODY_CACHE.with(|cache| {
        if let Some(body) = cache.borrow().bodies.get(reference)
            && body.retrieval_ms.is_finite()
            && body.retrieval_ms > 0.0
        {
            headers.push((
                "X-Weeb3-Retrieval-Ms".to_string(),
                body.retrieval_ms.to_string(),
            ));
        }
    });
    Some(if method == "HEAD" {
        FetchResponse::ok(status, headers, None)
    } else if start == 0 && end == body.len() {
        FetchResponse::ok_shared(status, headers, body)
    } else {
        body.get(start..end)?;
        FetchResponse::ok_shared(status, headers, body.slice(start..end))
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
    let whole_media_get = method == "GET" && range.is_none();
    let body = BODY_CACHE.with(|cache| cache.borrow().body(&reference));
    let cached = whole_media_get && body.is_some();
    let complete_body = whole_media_get
        && (codec_bootstrap || prefetch_from_reference(&reference, cached));
    if let Some(body) = body
        && let Some(response) =
            hls_body_response(body, codec_bootstrap, method, range, &reference)
    {
        return response;
    }
    let decoded = match hex::decode(&reference) {
        Ok(decoded) => decoded,
        Err(_) => return FetchResponse::error(400, "invalid HLS swarm reference"),
    };
    let root = retrieve_decoded_data_root(&decoded, &client.chunk_port.0, None).await;

    if complete_body && root.as_ref().is_none_or(|root| root.span <= HLS_BODY_MAX_BYTES) {
        let Some(body) = foreground_hls_body(client.clone(), reference.clone()).await else {
            return FetchResponse::error(503, "HLS segment body was unavailable");
        };
        return hls_body_response(body, codec_bootstrap, method, None, &reference)
            .unwrap_or_else(|| FetchResponse::error(503, "HLS segment body was unavailable"));
    }

    let Some(root) = root else {
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
        let Some(bytes) = hls_range(&client, &reference, span, start, end, None, &|| true).await
        else {
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
        hls_range(&client, &reference, span, 0, prefix_end, None, &|| true)
            .await
            .map_or("application/octet-stream", |prefix| {
                hls_payload_mime(&prefix)
            })
    } else {
        "application/octet-stream"
    };
    let headers = hls_body_headers(etag, mime, span, None);
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
    at: Option<f64>,
    method: &str,
    local_bytes_base: &str,
) -> FetchResponse {
    let immutable = index.is_some() || owner.is_empty();
    let mut requested_feed = FEED.with(|feed| {
        feed.borrow()
            .find(&owner, &topic, index)
            .map(|feed| feed.id)
    });
    let master = FEED.with_borrow(|feed| {
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
        let context = FEED.with_borrow(|feed| {
            if let Some(active) = feed.find(&owner, &topic, index) {
                return (!immutable && at.is_some_and(|date| !active.ready || active.refreshing
                    || active.playlist.as_ref().is_some_and(|playlist|
                        playlist.has_timeline() && !has_dated_runway(playlist, date, 4))))
                    .then_some((Some(active.id), active.view_generation));
            }
            let (_, _, _, master) = feed.master.as_ref()?;
            let source = local_feed_source(&owner, &topic, index, local_bytes_base, start);
            master
                .sources()
                .any(|candidate| {
                    local_hls_source(candidate, local_bytes_base, start, true).as_deref()
                        == Some(&source)
                })
                .then(|| feed.active().map(|active| (None, active.view_generation)))
                .flatten()
        });
        if let Some((existing, view_generation)) = context {
            if existing.is_some_and(|id| FEED.with_borrow(|feed| feed.active == id)) {
                return FetchResponse::error(409, "The active HLS window does not cover this time");
            }
            let existing = existing.filter(|id| {
                let ready = FEED.with_borrow(|feed|
                    feed.get(*id).is_some_and(|feed| feed.ready && !feed.refreshing));
                if !ready { end_feed(*id); }
                ready
            });
            let id = existing.unwrap_or_else(|| begin_feed(
                client.clone(),
                owner.clone(),
                topic.clone(),
                index,
                start,
                view_generation,
            ));
            requested_feed = Some(id);
            FEED.with_borrow_mut(|feed| feed.get_mut(id).unwrap().refreshing = true);
            let loaded = if immutable {
                load_fixed_manifest(id, view_generation, &client, &owner, &topic, index)
                    .await
                    .and_then(|(index, manifest)| match manifest {
                        HlsManifest::Media(history) => Some((index, history)),
                        HlsManifest::Master(_) => None,
                    })
            } else if let Some(date) = at {
                discover_at_date(id, view_generation, &client, &owner, &topic, date, 4).await.and_then(Result::ok)
            } else {
                discover_for_view(id, view_generation, &client, &owner, &topic).await
            };
            FEED.with_borrow_mut(|feed| {
                if let Some(active) = feed.get_mut(id) {
                    active.refreshing = false;
                    active.changed.notify(usize::MAX);
                }
            });
            if let Some((index, mut history)) = loaded
                && result_view_request_is_current(view_generation)
            {
                let plan = history.startup_plan(start);
                history.finalized |= immutable;
                let installed = FEED.with_borrow_mut(|feed| {
                    if existing.is_some() && feed.active == id { return None; }
                    let active = feed.get_mut(id)?;
                    active.playlist = Some(history);
                    active.index = Some(index);
                    active.foreground = None;
                    active.presentation_gaps.clear();
                    active.tail_fallbacks.clear();
                    active.updated_at = js_sys::Date::now();
                    active.live_startup_plan = plan;
                    active.beginning_history_started = true;
                    active.ready = true;
                    active.changed.notify(usize::MAX);
                    Some(())
                });
                if installed.is_none() {
                    if existing.is_none() { end_feed(id); }
                    return FetchResponse::error(409, "The HLS rendition is no longer active");
                }
            } else {
                if existing.is_none() { end_feed(id); }
                return FetchResponse::error(502, "The HLS rendition history could not be loaded");
            }
        }
        let refresh = FEED.with(|feed| {
            if context.is_some() || immutable || at.is_some() {
                return None;
            }
            let mut feed = feed.borrow_mut();
            let id = feed.find(&owner, &topic, index)?.id;
            if feed.active == id {
                return None;
            }
            let active = feed.get_mut(id).filter(|active| {
                !active.refreshing
                    && active.playlist.as_ref().is_some_and(|playlist| {
                        !playlist.has_timeline() && js_sys::Date::now() - active.updated_at
                            >= playlist.target_duration.max(1) as f64 * 1_000.0
                    })
            })?;
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
    while let Some(changed) = FEED.with_borrow(|feed| {
        let active = feed
            .find(&owner, &topic, index)
            .filter(|active| active.refreshing || !active.ready)?;
        Some(active.changed.listen())
    }) {
        changed.await;
    }
    if requested_feed.is_some_and(|id| FEED.with(|feed| feed.borrow().get(id).is_none())) {
        return FetchResponse::error(409, "The HLS rendition is no longer active");
    }
    if !immutable && at.is_some_and(|date| FEED.with_borrow(|feed|
        feed.find(&owner, &topic, index).and_then(|feed| feed.playlist.as_ref())
            .is_some_and(|playlist| playlist.has_timeline() && !has_dated_runway(playlist, date, 4)))) {
        return FetchResponse::error(409, "The HLS window does not cover this time");
    }
    let rendered = if master.is_some() {
        master
    } else if let Some(rendered) =
        render_active_feed(&owner, &topic, index, start, local_bytes_base)
    {
        Some(rendered)
    } else {
        load_manifest(&client, &owner, &topic, index)
            .await
            .map(|(index, manifest)| {
                let body = match manifest {
                    HlsManifest::Media(playlist) => playlist.render(local_bytes_base, start),
                    HlsManifest::Master(master) => master.render(|source, playlist|
                        local_hls_source(source, local_bytes_base, start, playlist)).into_bytes(),
                };
                (index, body, None, 0)
            })
    };
    let Some((resolved_index, body, follower, revision)) = rendered else {
        return FetchResponse::error(502, "The HLS feed could not be loaded");
    };
    if let Some(id) = follower {
        spawn_follower(id);
    }
    let mode = (start == HlsStart::Live)
        .then_some("live")
        .unwrap_or("beginning");
    let etag = format!("\"hls-feed-{resolved_index}-{mode}-{revision}\"");
    let mut headers = vec![
        (
            "Content-Type".to_string(),
            "application/vnd.apple.mpegurl".to_string(),
        ),
        ("Content-Length".to_string(), body.len().to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
        ("ETag".to_string(), etag),
    ];
    FEED.with_borrow(|feed| {
        if let Some(playlist) = feed.find(&owner, &topic, index)
            .and_then(|feed| feed.playlist.as_ref())
            && let Some(start) = playlist.program_start(js_sys::Date::parse)
        {
            headers.push(("X-Weeb3-Program-Start".into(), start.to_string()));
            headers.push(("X-Weeb3-Program-End".into(), (start + playlist.duration() * 1_000.0).to_string()));
        }
    });
    FetchResponse::ok(200, headers, (method != "HEAD").then_some(body))
}

pub(crate) async fn try_fetch_response(
    client: Arc<Weeb3>,
    request_url: &str,
    pathname: &str,
    method: &str,
    range: Option<&str>,
    if_none_match: Option<&str>,
) -> Option<FetchResponse> {
    let (owner, topic, index, start, at) = if let Some(reference) = canonical_hls_bytes_resource(pathname) {
        let reference = match reference {
            Ok(reference) => reference,
            Err(error) => return Some(FetchResponse::error(400, error)),
        };
        let query = web_sys::Url::new(request_url)
            .ok()
            .map(|url| url.search_params());
        let flag = |name| query.as_ref().and_then(|query| query.get(name)).as_deref() == Some("1");
        if !flag("playlist") {
            return Some(
                fetch_hls_body_response(
                    client,
                    reference,
                    flag("bootstrap"),
                    method,
                    range,
                    if_none_match,
                )
                .await,
            );
        }
        let start =
            FEED.with_borrow(|feed| feed.active().map_or(HlsStart::Beginning, |feed| feed.start));
        (String::new(), reference, None, start, None)
    } else {
        let (owner, topic) = canonical_feed_resource(pathname)?;
        let url = match web_sys::Url::new(request_url) {
            Ok(url) => url,
            Err(_) => return Some(FetchResponse::error(400, "invalid feed URL")),
        };
        let query = url.search_params();
        let index = match query.get("index").map(|index| index.parse()).transpose() {
            Ok(index) => index,
            Err(_) => return Some(FetchResponse::error(400, "invalid feed index")),
        };
        let start = match query.get("start").as_deref() {
            None | Some("beginning") => HlsStart::Beginning,
            Some("live") => HlsStart::Live,
            Some(_) => return Some(FetchResponse::error(400, "invalid HLS start")),
        };
        let at = match query.get("at").map(|date| date.parse::<f64>()).transpose() {
            Ok(at) if at.is_none_or(f64::is_finite) => at,
            _ => return Some(FetchResponse::error(400, "invalid HLS date")),
        };
        (owner, topic, index, start, at)
    };
    Some(
        fetch_feed_response(
            client,
            owner,
            topic,
            index,
            start,
            at,
            method,
            &local_hls_bytes_base(pathname),
        )
        .await,
    )
}

fn canonical_hls_bytes_resource(pathname: &str) -> Option<Result<String, &'static str>> {
    let resource = decode_component(route_resource(pathname, "hls/bytes/")?.trim());
    let mut parts = resource.split('/');
    let reference = parts.next().unwrap_or_default();
    if !is_hex_reference(reference) || parts.any(|part| !part.is_empty()) {
        return Some(Err("invalid HLS swarm reference"));
    }
    Some(Ok(reference.to_ascii_lowercase()))
}

fn canonical_feed_resource(pathname: &str) -> Option<(String, String)> {
    let resource = decode_component(route_resource(pathname, "feeds/")?.trim());
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
    Some((owner.to_ascii_lowercase(), topic.to_ascii_lowercase()))
}

fn local_hls_bytes_base(pathname: &str) -> String {
    let suffix = match pathname.strip_prefix(STREAMING_ROUTE_BASE) {
        Some(path) if path.starts_with("/testnet/") => "testnet/hls/bytes",
        Some(path) if path.starts_with("/mainnet/") => "mainnet/hls/bytes",
        _ => "hls/bytes",
    };
    streaming_route_path(suffix)
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

fn hls_feed_source(source: &str) -> Option<(String, String, Option<u64>)> {
    match HlsSource::parse(source)? {
        HlsSource::Reference(reference) => Some((String::new(), reference, None)),
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
            Some((owner, topic, index))
        }
    }
}

fn local_hls_source(
    source: &str,
    bytes_base: &str,
    start: HlsStart,
    playlist: bool,
) -> Option<String> {
    let (owner, topic, index) = hls_feed_source(source)?;
    Some(if owner.is_empty() && !playlist {
        format!("{bytes_base}/{topic}")
    } else {
        local_feed_source(&owner, &topic, index, bytes_base, start)
    })
}

async fn load_manifest(
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    pinned: Option<u64>,
) -> Option<(u64, HlsManifest)> {
    if owner.is_empty() {
        let bytes = hls_body(client.clone(), topic.to_string(), None).await?;
        return Some((0, HlsManifest::parse(&bytes)?));
    }
    let payload = if let Some(index) = pinned {
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
        payload
    } else {
        discover_latest_once(client, owner, topic, None).await?
    };
    Some((payload.index, HlsManifest::parse(&payload.bytes)?))
}

async fn load_fixed_manifest(
    id: u64,
    view_generation: u64,
    client: &Arc<Weeb3>,
    owner: &str,
    topic: &str,
    pinned: Option<u64>,
) -> Option<(u64, HlsManifest)> {
    let (index, manifest) = load_manifest(client, owner, topic, pinned).await?;
    let manifest = match manifest {
        HlsManifest::Media(head) if !owner.is_empty() => HlsManifest::Media(
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
        let mut rendition = None;
        loop {
            let id = begin_feed(client.clone(), owner.clone(), topic.clone(), pinned, start, view_generation);
            FEED.with(|feed| feed.borrow_mut().active = id);
            let immutable = pinned.is_some() || owner.is_empty();
            let (index, manifest, anchor) = if immutable {
                let (index, manifest) = load_fixed_manifest(id, view_generation,
                    &client, &owner, &topic, pinned).await
                    .ok_or("The HLS playlist history could not be loaded.")?;
                (index, manifest, None)
            } else if let Some((index, playlist)) = rendition.take() {
                (index, HlsManifest::Media(playlist), None)
            } else if start == HlsStart::Live {
                let (confirmed, provisional) = discover_live_with_rendition(
                    id,
                    view_generation,
                    &client,
                    &owner,
                    &topic,
                )
                .await?;
                rendition = provisional;
                confirmed
            } else {
                let payload = discover_beginning(id, view_generation, &client, &owner, &topic).await
                    .ok_or("The HLS feed could not be loaded.")?;
                (payload.index, HlsManifest::parse(&payload.bytes).ok_or("The HLS manifest is invalid.")?, None)
            };
            if !feed_is_current(id, view_generation) {
                return Err("HLS open was superseded".to_string());
            }
            let head = match manifest {
                HlsManifest::Media(head) => head,
                HlsManifest::Master(master) => {
                    if initial_source.is_some() { return Err("Nested HLS master playlists are invalid.".to_string()); }
                    let initial = master.initial_source().ok_or("The HLS master has no video rendition.")?;
                    let selected = hls_feed_source(initial).ok_or("The HLS rendition source is invalid.")?;
                    initial_source = Some(local_feed_source(&selected.0, &selected.1, selected.2, &bytes_base, start));
                    FEED.with(|feed| feed.borrow_mut().master = Some((owner.clone(), topic.clone(), index, master)));
                    end_feed(id);
                    (owner, topic, pinned) = selected;
                    continue;
                }
            };
            let needs_successor = head.segments.iter().filter(|segment| !segment.gap).nth(1).is_none();
            let offset = if start == HlsStart::Live && !owner.is_empty()
                && head.sequence != 0 && head.has_timeline() {
                loop {
                    if !feed_is_current(id, view_generation) {
                        return Err("HLS open was superseded".to_string());
                    }
                    if let Ok(FeedPayloadProbe::Found(first)) = async_std::future::timeout(
                        EDGE_WAVE_TIMEOUT, probe_feed_payload(&client, &owner, &topic, 0,
                            BEGINNING_PAYLOAD_BYTES, Some(FEED_PROBE_ATTEMPTS))).await
                    {
                        break HlsPlaylist::parse(&first.bytes)
                            .and_then(|first| head.timeline_offset(&first, js_sys::Date::parse))
                            .ok_or("The broadcast origin does not match its live timeline.")?;
                    }
                    async_std::task::sleep(INITIAL_DISCOVERY_RETRY_DELAY).await;
                }
            } else { 0.0 };
            if !feed_is_current(id, view_generation) {
                return Err("HLS open was superseded".to_string());
            }
            let finalized = start == HlsStart::Live && head.finalized;
            FEED.with(|feed| feed.borrow_mut().origin = head.program_start(js_sys::Date::parse)
                .map(|date| date - offset * 1_000.0));
            let plan = head.startup_plan(start);
            install_snapshot(id, index, head)
                .ok_or("The HLS feed history did not match its live updates.")?;
            let (index, plan, start_sequence) = if let Some(anchor) = &anchor {
                prepare_live_plan(id, view_generation, anchor).await?
            } else {
                let plan = plan.ok_or("The HLS feed does not contain a playable startup runway.")?;
                FEED.with(|feed| {
                    if let Some(active) = feed.borrow_mut().get_mut(id) {
                        if start == HlsStart::Live { active.live_startup_plan = Some(plan.clone()); }
                        if immutable || finalized {
                            active.playlist.as_mut().unwrap().finalized = true;
                            active.beginning_history_started = true;
                        }
                    }
                });
                if needs_successor && !immutable { spawn_follower(id); }
                (index, plan, 0)
            };
            if start == HlsStart::Live {
                spawn_body_runway(id);
            }
            if !feed_is_current(id, view_generation) {
                return Err("HLS open was superseded".to_string());
            }
            let plan = plan.with_offset(offset);
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
                let mode = if start == HlsStart::Live {
                    "live"
                } else {
                    "beginning"
                };
                format!("HLS open index={index} elapsed={elapsed:.3}s mode={mode}")
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

pub(super) async fn prepare_history(source: &str, position: f64) -> Option<Result<f64, &'static str>> {
    if !position.is_finite() || position < 0.0 { return None; }
    let url = web_sys::Url::new(source).ok()?;
    let path = url.pathname();
    let (owner, topic) = canonical_feed_resource(&path).or_else(|| {
        Some((String::new(), canonical_hls_bytes_resource(&path)?.ok()?))
    })?;
    let pinned = url.search_params().get("index").map(|index| index.parse::<u64>()).transpose().ok()?;
    let (origin, start, pending) = FEED.with_borrow(|feed| {
        feed.history_changed.notify(usize::MAX);
        let active = feed.find(&owner, &topic, pinned).filter(|active| active.ready)?;
        let (origin, index) = (feed.origin?, active.index?);
        let head = active.playlist.as_ref()?;
        let start = head.program_start(js_sys::Date::parse)?;
        Some((origin, start, (!has_dated_runway(head, origin + position * 1_000.0, 1)).then(||
            (feed.history_changed.listen(), active.id, active.view_generation,
                active.client.clone(), index, head.clone()))))
    })?;
    let Some((cancelled, id, view, client, index, head)) = pending
        else { return Some(Ok((start - origin) / 1_000.0)); };
    let date = origin + position * 1_000.0;
    let forward = date >= start;
    // Expire the lookup before the page's reply timeout; an abandoned lookup must not commit.
    let lookup = async_std::future::timeout(Duration::from_secs(120), async {
        let (index, history) = if forward {
            if pinned.is_some() || owner.is_empty() {
                return (date >= dated_end(&head)?).then_some(Err(index));
            }
            let (index, mut history) = match discover_at_date(
                id, view, &client, &owner, &topic, date, 1).await? {
                Ok(history) => history,
                Err(index) => return Some(Err(index)),
            };
            let start = history.retain_from_date(date, js_sys::Date::parse)?;
            history.segments[0].program_date_time.get_or_insert_with(||
                js_sys::Date::new(&JsValue::from_f64(start)).to_iso_string().into());
            (index, history)
        } else {
            (index, hls_history(id, view, &client, &owner, &topic, index,
                index % HISTORY_STRIDE, head, HISTORY_FOREGROUND_PARALLEL, false, Some(date)).await?)
        };
        // Make the next few segments available before the first frame resumes.
        let mut seconds = 0.0;
        let mut bodies = stream::iter(history.segments.iter().take_while(|segment| {
            let needed = seconds < HLS_LIVE_STARTUP_BUFFER_SECONDS;
            seconds += segment.duration;
            needed
        }).filter(|segment| !segment.gap))
            .map(|segment| hls_body(client.clone(), segment.reference.clone(), None)).buffered(2);
        while bodies.next().await.is_some() {}
        drop(bodies);
        Some(Ok((index, history)))
    });
    let future::Either::Right((history, cancelled)) = future::select(cancelled, Box::pin(lookup)).await
        else { return None; };
    if cancelled.now_or_never().is_some() { return None; }
    let (index, history) = match history.ok()?? {
        Ok((index, history)) => (index, Some(history)),
        Err(index) => (index, None),
    };
    let start = FEED.with_borrow_mut(|feed| {
        if !result_view_request_is_current(view) {
            return None;
        }
        let active = feed.get_mut(id)?;
        if forward {
            let current = active.playlist.as_ref()?;
            if has_dated_runway(current, date, 1) {
                return current.program_start(js_sys::Date::parse).map(Ok);
            }
            if active.index.is_some_and(|current| current > index) { return None; }
        }
        let Some(mut history) = history else {
            return Some(Err("This quality ends before the requested time. Choose another quality or seek earlier."));
        };
        if !forward { history.merge_playlist(active.playlist.as_ref()?.clone())?; }
        let start = history.program_start(js_sys::Date::parse)?;
        if forward {
            active.index = Some(index);
            active.updated_at = js_sys::Date::now();
            active.presentation_gaps.clear();
            active.tail_fallbacks.clear();
        }
        active.foreground = history.segments.first().map(|segment| segment.reference.clone());
        active.live_startup_plan = history.startup_plan(HlsStart::Beginning);
        active.playlist = Some(history);
        active.changed.notify(usize::MAX);
        // Other renditions rebuild lazily against the newly extended origin.
        feed.feeds.retain(|feed| feed.id == id);
        feed.active = id;
        Some(Ok(start))
    })?;
    let start = match start { Ok(start) => start, Err(error) => return Some(Err(error)) };
    spawn_body_runway(id);
    if forward { spawn_follower(id); }
    Some(Ok((start - origin) / 1_000.0))
}

pub(crate) fn release_hls_runtime() {
    FEED.with_borrow_mut(|feed| {
        feed.history_changed.notify(usize::MAX);
        *feed = FeedSessions::default();
    });
}

pub(crate) fn clear_hls_runtime_cache() {
    BODY_CACHE.with(|cache| cache.borrow_mut().clear());
    clear_completed_media_ranges();
}
