use crate::{
    ChunkRetrieveSender, Date, Duration, HashMap, HashSet, Mutex, OutboundProtocolSession,
    PeerAccounting, PeerId, PhysicalConnectionMap, RETRIEVE_CHECK_CONFIRMATION_PEERS,
    RefreshmentInstruction, RetrieveCancelToken, RetrieveGenerationMap, StreamControl,
    apply_credit, cancel_reserve,
    erasure_coding::{
        self, BEE_MAX_UPLOAD_TREE_LEVELS, CHUNK_SIZE, CHUNK_WITH_SPAN_SIZE, HASH_SIZE,
        RedundancyLevel, encoded_reference_payload_len, reconstruct_data_indices, reference_layout,
        split_references,
    },
    feed::{
        FeedProbe, seek_sequence_feed_frontier,
        seek_sequence_feed_frontier_bounded_observing_positive,
    },
    get_feed_address, get_proximity, mpsc, price, reserve,
    retrieval_conventions::SingleflightRegistry,
    retrieval_conventions::{
        RetrieveAdmission, RetrieveHedgeDemand, SharedRetrieveHedgeDemand,
        retrieve_admission_current, retrieve_attempt_start_allowed, rolling_full_group_eligible,
        rolling_full_group_static_candidate, rolling_next_parity_index,
        rolling_parity_admission_count,
    },
    retrieve_cancel_token_current, retrieve_handler, transfer_pause_enabled, valid_cac, valid_soc,
};

use alloy::primitives::keccak256;
use async_std::sync::Arc;
use std::{
    cell::RefCell, collections::VecDeque, future::Future, pin::Pin, rc::Rc,
    sync::atomic::AtomicBool,
};

const RETRIEVE_HEDGE_AFTER_MS: u64 = 1_000;
const RETRIEVE_RS_HEDGE_AFTER_MS: u64 = RETRIEVE_HEDGE_AFTER_MS * 2;
const RETRIEVE_RECOVERY_EXTRA_SHARDS: usize = 2;
const RETRIEVE_RECOVERY_PROGRESSIVE_BATCH: usize = 2;
const RETRIEVE_CANCEL_POLL_MS: u64 = 100;
const RETRIEVE_ATTEMPT_TIMEOUT_MS: u64 = 10_000;
const RETRIEVE_HOT_LOOP_GUARD_MS: u64 = 25;
const RETRIEVE_CHECK_RETRY_WAIT_MS: u64 = 160;
const RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS: usize = 20;
const RETRIEVE_DATA_GROUP_CONCURRENCY: usize = 8;
const RETRIEVE_DECODED_CHUNK_CACHE_ENTRIES: usize = 2048;

struct RetrieveAttemptResult {
    peer: PeerId,
    chunk: Vec<u8>,
    valid: bool,
    soc: bool,
}

struct ReservedRetrievePeer {
    peer: PeerId,
    price: u64,
    accounting: Arc<Mutex<PeerAccounting>>,
    session: OutboundProtocolSession,
}

use libp2p::futures::{
    StreamExt,
    future::{Either, select},
    pin_mut,
    stream::FuturesUnordered,
};

async fn select_retrieve_peer(
    caddr: &Vec<u8>,
    peers: &Arc<Mutex<HashMap<Vec<u8>, PeerId>>>,
    accounting: &Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    physical_connections: &PhysicalConnectionMap,
    skiplist: &mut HashSet<PeerId>,
    overdraftlist: &mut HashSet<PeerId>,
) -> Option<ReservedRetrievePeer> {
    loop {
        let selected = {
            let peers_map = peers.lock().await;
            if peers_map.is_empty() {
                return None;
            }

            let mut closest_peer_id: Option<PeerId> = None;
            let mut closest_price = 0;
            let mut current_max_po = 0;
            for (overlay, id) in peers_map.iter() {
                if skiplist.contains(id) {
                    continue;
                }

                let current_po = get_proximity(caddr, overlay);

                if current_po >= current_max_po || closest_peer_id.is_none() {
                    closest_peer_id = Some(*id);
                    closest_price = price(overlay, caddr);
                    current_max_po = current_po;
                }
            }

            closest_peer_id.map(|peer| (peer, closest_price))
        };
        let Some((peer, req_price)) = selected else {
            return None;
        };

        skiplist.insert(peer.clone());

        let accounting_peer = {
            let accounting_peers = accounting.lock().await;
            accounting_peers.get(&peer).cloned()
        };

        if let Some(accounting_peer) = accounting_peer {
            if let Some(connection_id) = reserve(&accounting_peer, req_price).await {
                if let Some(session) = OutboundProtocolSession::capture(
                    peer.clone(),
                    connection_id,
                    physical_connections.clone(),
                ) {
                    return Some(ReservedRetrievePeer {
                        peer,
                        price: req_price,
                        accounting: accounting_peer,
                        session,
                    });
                }
                cancel_reserve(&accounting_peer, req_price).await;
                continue;
            }

            overdraftlist.insert(peer);
        }

        async_std::task::yield_now().await;
        async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
    }
}

fn reset_overdraft(skiplist: &mut HashSet<PeerId>, overdraftlist: &mut HashSet<PeerId>) {
    for peer in overdraftlist.drain() {
        skiplist.remove(&peer);
    }
}

fn failed_retrieve_attempt(peer: &PeerId) -> RetrieveAttemptResult {
    RetrieveAttemptResult {
        peer: peer.clone(),
        chunk: vec![],
        valid: false,
        soc: false,
    }
}

async fn settle_retrieve_attempt(
    peer: PeerId,
    caddr: Vec<u8>,
    req_price: u64,
    accounting_peer: Arc<Mutex<PeerAccounting>>,
    refresh_chan: mpsc::Sender<RefreshmentInstruction>,
    retrieve_result: Result<Vec<u8>, async_std::channel::RecvError>,
) -> RetrieveAttemptResult {
    match retrieve_result {
        Ok(chunk) => {
            let (chunk_valid, soc) = verify_chunk(&caddr, &chunk);
            if chunk_valid {
                apply_credit(&accounting_peer, req_price, &refresh_chan).await;
                return RetrieveAttemptResult {
                    peer,
                    chunk,
                    valid: true,
                    soc,
                };
            }

            cancel_reserve(&accounting_peer, req_price).await;
        }
        Err(_) => {
            cancel_reserve(&accounting_peer, req_price).await;
        }
    }

    failed_retrieve_attempt(&peer)
}

async fn retrieve_attempt(
    selected: ReservedRetrievePeer,
    caddr: Vec<u8>,
    control: StreamControl,
    refresh_chan: mpsc::Sender<RefreshmentInstruction>,
    result_chan: mpsc::Sender<RetrieveAttemptResult>,
    admission: Option<RetrieveAdmission>,
) {
    let ReservedRetrievePeer {
        peer,
        price: req_price,
        accounting: accounting_peer,
        session,
    } = selected;
    let (chunk_out, chunk_in) = mpsc::unbounded::<Vec<u8>>();
    let handler_peer = peer.clone();
    let handler_caddr = caddr.clone();
    wasm_bindgen_futures::spawn_local(async move {
        retrieve_handler(handler_peer, handler_caddr, control, session, &chunk_out).await;
    });

    let retrieve_result = async_std::future::timeout(
        Duration::from_millis(RETRIEVE_ATTEMPT_TIMEOUT_MS),
        chunk_in.recv(),
    )
    .await;

    match retrieve_result {
        Ok(retrieve_result) => {
            if matches!(retrieve_result.as_ref(), Ok(chunk) if chunk.is_empty())
                && let Some(admission) = admission.as_ref()
            {
                admission.record_confirmed_empty_physical_attempt();
            }
            let result = settle_retrieve_attempt(
                peer,
                caddr,
                req_price,
                accounting_peer,
                refresh_chan,
                retrieve_result,
            )
            .await;
            let _ = result_chan.try_send(result);
        }
        Err(_) => {
            if let Some(admission) = admission.as_ref() {
                admission.record_physical_attempt_timeout();
            }
            // Ten seconds is terminal for the logical retrieval, so its in-flight slot and
            // dispatcher permit can be released. The dispatched exchange still owns a reserve;
            // keep its response receiver alive in a detached accounting-only settlement task.
            let _ = result_chan.try_send(failed_retrieve_attempt(&peer));

            // A timed-out dispatched request must still settle its accounting reserve.
            wasm_bindgen_futures::spawn_local(async move {
                let retrieve_result = chunk_in.recv().await;
                let _ = settle_retrieve_attempt(
                    peer,
                    caddr,
                    req_price,
                    accounting_peer,
                    refresh_chan,
                    retrieve_result,
                )
                .await;
            });
        }
    }
}

fn chunk_address_parts(chunk_address: &Vec<u8>) -> (Vec<u8>, Vec<u8>, bool) {
    if chunk_address.len() == 64 {
        return (
            (&chunk_address[0..32]).to_vec(),
            (&chunk_address[32..64]).to_vec(),
            true,
        );
    }

    (chunk_address.to_vec(), vec![], false)
}

fn decode_retrieved_chunk(cd: Vec<u8>, soc: bool, encrey: Vec<u8>, encred: bool) -> Vec<u8> {
    if encred {
        if soc {
            if cd.len() < 97 {
                return vec![];
            }

            let cd00 = decrypt(&(&cd[97..]).to_vec(), encrey);
            if cd00.len() >= 8 {
                return cd00;
            } else {
                return vec![];
            }
        }

        return decrypt(&cd, encrey);
    }

    if soc && cd.len() >= 97 + 8 {
        return (&cd[97..]).to_vec();
    }
    cd
}

#[derive(Clone, Debug)]
pub(crate) struct DecodedJoinChunk {
    pub level: RedundancyLevel,
    pub span: u64,
    pub payload: Rc<[u8]>,
}

#[derive(Clone)]
struct CachedJoinChunk {
    raw: Option<Rc<[u8]>>,
    decoded: Option<DecodedJoinChunk>,
    generation: u64,
}

#[derive(Default)]
struct DecodedChunkCache {
    chunks: HashMap<Vec<u8>, CachedJoinChunk>,
    order: VecDeque<(Vec<u8>, u64)>,
    generation: u64,
}

impl DecodedChunkCache {
    fn get_decoded(&mut self, reference: &[u8]) -> Option<DecodedJoinChunk> {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let entry = self.chunks.get_mut(reference)?;
        entry.generation = generation;
        let decoded = entry.decoded.clone();
        // The ordinary fast path needs raw only when it must decode it. In particular, a decoded
        // cache hit must not clone and touch the raw cache value merely to discard it.
        let raw = decoded.is_none().then(|| entry.raw.clone()).flatten();
        self.finish_touch(reference.to_vec(), generation);

        if let Some(decoded) = decoded {
            return Some(decoded);
        }

        let decoded = decode_raw_join_chunk(raw.as_ref()?.as_ref(), reference)?;
        if let Some(entry) = self.chunks.get_mut(reference) {
            entry.decoded = Some(decoded.clone());
        }
        Some(decoded)
    }

    fn get_decoded_and_raw(
        &mut self,
        reference: &[u8],
    ) -> Option<(DecodedJoinChunk, Option<Rc<[u8]>>)> {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let entry = self.chunks.get_mut(reference)?;
        entry.generation = generation;
        let decoded = entry.decoded.clone();
        let raw = entry.raw.clone();
        self.finish_touch(reference.to_vec(), generation);

        if let Some(decoded) = decoded {
            return Some((decoded, raw));
        }

        let decoded = decode_raw_join_chunk(raw.as_ref()?.as_ref(), reference)?;
        if let Some(entry) = self.chunks.get_mut(reference) {
            entry.decoded = Some(decoded.clone());
        }
        Some((decoded, raw))
    }

    fn get_raw(&mut self, reference: &[u8]) -> Option<Rc<[u8]>> {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let entry = self.chunks.get_mut(reference)?;
        entry.generation = generation;
        let raw = entry.raw.clone();
        self.finish_touch(reference.to_vec(), generation);
        raw
    }

    fn insert_decoded(&mut self, reference: Vec<u8>, chunk: DecodedJoinChunk) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        if let Some(entry) = self.chunks.get_mut(&reference) {
            entry.decoded = Some(chunk);
            entry.generation = generation;
        } else {
            self.chunks.insert(
                reference.clone(),
                CachedJoinChunk {
                    raw: None,
                    decoded: Some(chunk),
                    generation,
                },
            );
        }
        self.finish_touch(reference, generation);
    }

    fn insert_raw(&mut self, reference: Vec<u8>, raw: Rc<[u8]>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        if let Some(entry) = self.chunks.get_mut(&reference) {
            if entry.raw.is_none() {
                entry.raw = Some(raw);
            }
            entry.generation = generation;
        } else {
            self.chunks.insert(
                reference.clone(),
                CachedJoinChunk {
                    raw: Some(raw),
                    decoded: None,
                    generation,
                },
            );
        }
        self.finish_touch(reference, generation);
    }

    fn finish_touch(&mut self, reference: Vec<u8>, generation: u64) {
        self.order.push_back((reference, generation));

        while self.chunks.len() > RETRIEVE_DECODED_CHUNK_CACHE_ENTRIES {
            let Some((expired, expired_generation)) = self.order.pop_front() else {
                break;
            };
            if self
                .chunks
                .get(&expired)
                .is_some_and(|entry| entry.generation == expired_generation)
            {
                self.chunks.remove(&expired);
            }
        }
        self.compact_order_if_needed();
    }

    fn compact_order_if_needed(&mut self) {
        if self.order.len() <= RETRIEVE_DECODED_CHUNK_CACHE_ENTRIES * 2 {
            return;
        }

        let chunks = &self.chunks;
        self.order.retain(|(reference, generation)| {
            chunks
                .get(reference)
                .is_some_and(|entry| entry.generation == *generation)
        });
    }
}

thread_local! {
    static RETRIEVE_DECODED_CHUNK_CACHE: RefCell<DecodedChunkCache> =
        RefCell::new(DecodedChunkCache::default());
}

pub(crate) fn cached_decoded_chunk(reference: &[u8]) -> Option<DecodedJoinChunk> {
    RETRIEVE_DECODED_CHUNK_CACHE.with(|cache| cache.borrow_mut().get_decoded(reference))
}

fn cached_decoded_and_raw_chunk(reference: &[u8]) -> Option<(DecodedJoinChunk, Option<Rc<[u8]>>)> {
    RETRIEVE_DECODED_CHUNK_CACHE.with(|cache| cache.borrow_mut().get_decoded_and_raw(reference))
}

fn cached_raw_chunk(reference: &[u8]) -> Option<Rc<[u8]>> {
    RETRIEVE_DECODED_CHUNK_CACHE.with(|cache| cache.borrow_mut().get_raw(reference))
}

fn remember_decoded_chunk(reference: Vec<u8>, chunk: &DecodedJoinChunk) {
    RETRIEVE_DECODED_CHUNK_CACHE.with(|cache| {
        cache.borrow_mut().insert_decoded(reference, chunk.clone());
    });
}

fn remember_raw_chunk(reference: Vec<u8>, raw: Rc<[u8]>) {
    RETRIEVE_DECODED_CHUNK_CACHE.with(|cache| {
        cache.borrow_mut().insert_raw(reference, raw);
    });
}

async fn join_cancel_token_current(
    cancel_generations: &Option<RetrieveGenerationMap>,
    cancel: &Option<RetrieveCancelToken>,
) -> bool {
    if let (Some(generations), Some(_)) = (cancel_generations, cancel) {
        return retrieve_cancel_token_current(generations, cancel).await;
    }
    true
}

async fn chunk_retrieve_admission_current(
    cancel_generations: &Option<RetrieveGenerationMap>,
    cancel: &Option<RetrieveCancelToken>,
    admission: &Option<RetrieveAdmission>,
) -> bool {
    let stream_generation_current = join_cancel_token_current(cancel_generations, cancel).await;
    retrieve_admission_current(stream_generation_current, admission)
}

async fn recv_raw_result_cancellable(
    receiver: &mpsc::Receiver<RawFetchResult>,
    cancel_generations: &Option<RetrieveGenerationMap>,
    cancel: &Option<RetrieveCancelToken>,
) -> RawFetchReceive {
    if cancel_generations.is_none() || cancel.is_none() {
        return match receiver.recv().await {
            Ok(result) => RawFetchReceive::Result(result),
            Err(_) => RawFetchReceive::ChannelClosed,
        };
    }

    loop {
        if !join_cancel_token_current(cancel_generations, cancel).await {
            return RawFetchReceive::Stale;
        }
        match async_std::future::timeout(
            Duration::from_millis(RETRIEVE_CANCEL_POLL_MS),
            receiver.recv(),
        )
        .await
        {
            Ok(Ok(result)) => return RawFetchReceive::Result(result),
            Ok(Err(_)) => return RawFetchReceive::ChannelClosed,
            Err(_) => continue,
        }
    }
}

enum RawFetchReceive {
    Result(RawFetchResult),
    Stale,
    ChannelClosed,
}

struct RawFetchResult {
    index: usize,
    chunk: Rc<[u8]>,
    canonical_cac: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RawFetchKey {
    runtime_scope: usize,
    request_address: Vec<u8>,
    expected_cac: Vec<u8>,
    cancel_scope: Option<(String, u64)>,
}

impl RawFetchKey {
    fn new(
        runtime_scope: usize,
        request_address: Vec<u8>,
        expected_cac: Vec<u8>,
        cancel: &Option<RetrieveCancelToken>,
    ) -> Self {
        Self {
            runtime_scope,
            request_address,
            expected_cac,
            cancel_scope: cancel
                .as_ref()
                .map(|cancel| (cancel.stream_key.clone(), cancel.generation)),
        }
    }
}

struct RawFetchWaiter {
    index: usize,
    result_chan: mpsc::Sender<RawFetchResult>,
    admission: RetrieveAdmission,
}

#[derive(Clone)]
struct RawFetchShared {
    admission: RetrieveAdmission,
    hedge_demand: Option<SharedRetrieveHedgeDemand>,
    cache_references: Rc<RefCell<HashSet<Vec<u8>>>>,
}

impl RawFetchShared {
    fn new(hedge_demand: RetrieveHedgeDemand) -> Self {
        Self {
            admission: RetrieveAdmission::new(),
            hedge_demand: (hedge_demand == RetrieveHedgeDemand::DistinctShardManaged)
                .then(|| SharedRetrieveHedgeDemand::new(hedge_demand)),
            cache_references: Rc::new(RefCell::new(HashSet::new())),
        }
    }

    fn remember_cache_reference(&self, reference: Option<&Vec<u8>>) {
        if let Some(reference) = reference {
            self.cache_references.borrow_mut().insert(reference.clone());
        }
    }
}

type RawFetchFlights = SingleflightRegistry<RawFetchKey, RawFetchWaiter, RawFetchShared>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RawFetchRegistration {
    Cached,
    Joined,
    Led,
}

thread_local! {
    static RAW_FETCH_FLIGHTS: RefCell<RawFetchFlights> =
        RefCell::new(RawFetchFlights::default());
}

fn remove_raw_fetch_waiter(key: &RawFetchKey, flight_id: u64, waiter_id: u64) {
    let shared = RAW_FETCH_FLIGHTS.with(|flights| {
        flights
            .borrow_mut()
            .remove_waiter(key, flight_id, waiter_id)
    });
    if let Some(shared) = shared {
        // Keep the flight registered while dispatched accounting work drains.
        shared.admission.close();
    }
}

fn complete_raw_fetch(key: &RawFetchKey, flight_id: u64, chunk: Vec<u8>) -> bool {
    let flight = RAW_FETCH_FLIGHTS.with(|flights| flights.borrow_mut().take(key, flight_id));
    let Some(flight) = flight else {
        return false;
    };
    flight.shared.admission.close();

    let usable = (erasure_coding::SPAN_SIZE..=CHUNK_WITH_SPAN_SIZE).contains(&chunk.len());
    let canonical_cac = usable && valid_cac(&chunk, &key.expected_cac);
    let chunk: Rc<[u8]> = chunk.into();
    let delivered = if usable { chunk } else { Rc::from([]) };

    // Cache ownership belongs to the physical flight, not to its current
    // logical waiters. A retired caller may have zero waiters when a canonical
    // late result arrives, and that result must still benefit a later caller.
    if canonical_cac {
        for reference in flight.shared.cache_references.borrow().iter() {
            remember_raw_chunk(reference.clone(), Rc::clone(&delivered));
        }
    }

    for waiter in flight.waiters {
        if !waiter.admission.is_open() {
            continue;
        }
        let _ = waiter.result_chan.try_send(RawFetchResult {
            index: waiter.index,
            chunk: Rc::clone(&delivered),
            canonical_cac,
        });
    }
    canonical_cac
}

fn queue_drained_raw_chunk(
    index: usize,
    request_address: Vec<u8>,
    expected_cac: Vec<u8>,
    cache_reference: Option<Vec<u8>>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    result_chan: &mpsc::Sender<RawFetchResult>,
    admission: &RetrieveAdmission,
    cancel: &Option<RetrieveCancelToken>,
    hedge_demand: RetrieveHedgeDemand,
) -> RawFetchRegistration {
    if let Some(reference) = cache_reference.as_ref() {
        if let Some(chunk) = cached_raw_chunk(reference) {
            let _ = result_chan.try_send(RawFetchResult {
                index,
                chunk,
                canonical_cac: true,
            });
            return RawFetchRegistration::Cached;
        }
    }

    let key = RawFetchKey::new(
        chunk_retrieve_chan.runtime_scope(),
        request_address,
        expected_cac,
        cancel,
    );
    let registration = RAW_FETCH_FLIGHTS.with(|flights| {
        flights.borrow_mut().register(
            key,
            RawFetchWaiter {
                index,
                result_chan: result_chan.clone(),
                admission: admission.clone(),
            },
            || RawFetchShared::new(hedge_demand),
        )
    });
    registration
        .shared
        .remember_cache_reference(cache_reference.as_ref());
    if let Some(shared_demand) = registration.shared.hedge_demand.as_ref() {
        shared_demand.promote(hedge_demand);
    }

    let waiter_key = registration.key.clone();
    let flight_id = registration.flight_id;
    let waiter_id = registration.waiter_id;
    let waiter_admission = admission.clone();
    wasm_bindgen_futures::spawn_local(async move {
        waiter_admission.wait_closed().await;
        remove_raw_fetch_waiter(&waiter_key, flight_id, waiter_id);
    });

    if !registration.leader {
        return RawFetchRegistration::Joined;
    }

    let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
    let completion_key = registration.key;
    if chunk_retrieve_chan
        .try_send(crate::ChunkRetrieveRequest {
            address: completion_key.request_address.clone(),
            chan: chan_out,
            cancel: cancel.clone(),
            admission: Some(registration.shared.admission.clone()),
            hedge_demand: registration.shared.hedge_demand.clone(),
        })
        .is_err()
    {
        complete_raw_fetch(&completion_key, flight_id, Vec::new());
        return RawFetchRegistration::Led;
    }

    // The detached producer lets dispatched exchanges settle after callers leave.
    wasm_bindgen_futures::spawn_local(async move {
        let chunk = chan_in.recv().await.unwrap_or_default();
        complete_raw_fetch(&completion_key, flight_id, chunk);
    });
    RawFetchRegistration::Led
}

#[inline]
fn decryption_segment_key(key: &[u8], counter: u32) -> [u8; HASH_SIZE] {
    let mut seed = [0u8; HASH_SIZE + 4];
    seed[..HASH_SIZE].copy_from_slice(key);
    seed[HASH_SIZE..].copy_from_slice(&counter.to_le_bytes());
    keccak256(keccak256(seed)).into()
}

fn decrypt_join_chunk(raw: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    if raw.len() < erasure_coding::SPAN_SIZE || key.len() != HASH_SIZE {
        return None;
    }

    let span_key = decryption_segment_key(key, (CHUNK_SIZE / key.len()) as u32);
    let mut plain = Vec::with_capacity(raw.len());
    for index in 0..erasure_coding::SPAN_SIZE {
        plain.push(raw[index] ^ span_key[index]);
    }

    for (segment_index, segment) in raw[erasure_coding::SPAN_SIZE..]
        .chunks(key.len())
        .enumerate()
    {
        let segment_key = decryption_segment_key(key, segment_index as u32);
        plain.extend(
            segment
                .iter()
                .zip(segment_key.iter())
                .map(|(&value, &mask)| value ^ mask),
        );
    }
    Some(plain)
}

fn canonical_plain_chunk(plain: &[u8], encrypted: bool) -> Option<DecodedJoinChunk> {
    let (level, span) = erasure_coding::decode_span(plain)?;
    let payload_len = if span <= CHUNK_SIZE as u64 {
        usize::try_from(span).ok()?
    } else {
        encoded_reference_payload_len(span, level, encrypted)?
    };
    let chunk_len = erasure_coding::SPAN_SIZE.checked_add(payload_len)?;
    if plain.len() < chunk_len {
        return None;
    }
    Some(DecodedJoinChunk {
        level,
        span,
        payload: Rc::from(&plain[erasure_coding::SPAN_SIZE..chunk_len]),
    })
}

pub(crate) fn decode_raw_join_chunk(raw: &[u8], reference: &[u8]) -> Option<DecodedJoinChunk> {
    if reference.len() != HASH_SIZE && reference.len() != erasure_coding::ENCRYPTED_REFERENCE_SIZE {
        return None;
    }
    if !(erasure_coding::SPAN_SIZE..=CHUNK_WITH_SPAN_SIZE).contains(&raw.len()) {
        return None;
    }

    let encrypted = reference.len() == erasure_coding::ENCRYPTED_REFERENCE_SIZE;
    if encrypted {
        let plain = decrypt_join_chunk(raw, &reference[HASH_SIZE..])?;
        canonical_plain_chunk(&plain, true)
    } else {
        canonical_plain_chunk(raw, false)
    }
}

fn bee_replica_address(id: &[u8; HASH_SIZE]) -> [u8; HASH_SIZE] {
    const OWNER: [u8; 20] = [
        0xdc, 0x5b, 0x20, 0x84, 0x7f, 0x43, 0xd6, 0x79, 0x28, 0xf4, 0x9c, 0xd4, 0xf8, 0x5d, 0x69,
        0x6b, 0x5a, 0x76, 0x17, 0xb5,
    ];
    let mut input = [0u8; HASH_SIZE + OWNER.len()];
    input[..HASH_SIZE].copy_from_slice(id);
    input[HASH_SIZE..].copy_from_slice(&OWNER);
    keccak256(input).into()
}

async fn retrieve_raw_root_cancellable(
    reference: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel_generations: &Option<RetrieveGenerationMap>,
    cancel: &Option<RetrieveCancelToken>,
) -> Option<(Rc<[u8]>, bool)> {
    let root_cac = reference.get(..HASH_SIZE)?.to_vec();
    let replicas = erasure_coding::replicas(
        &root_cac,
        RedundancyLevel::DEFAULT_DOWNLOAD,
        bee_replica_address,
    )?;
    let mut requests = Vec::with_capacity(1 + replicas.len());
    requests.push(root_cac.clone());
    requests.extend(replicas.into_iter().map(|replica| replica.address.to_vec()));

    let admission = RetrieveAdmission::new();
    let _admission_guard = admission.close_on_drop();

    let (result_out, result_in) = mpsc::unbounded::<RawFetchResult>();
    let mut next = 0usize;
    let mut dispatched = 0usize;
    let mut completed = 0usize;
    let mut next_batch = 2usize;

    let initial = requests.len().min(3); // original CAC plus Bee's first two replicas
    if !join_cancel_token_current(cancel_generations, cancel).await {
        return None;
    }
    while next < initial {
        queue_drained_raw_chunk(
            next,
            requests[next].clone(),
            root_cac.clone(),
            Some(reference.to_vec()),
            chunk_retrieve_chan,
            &result_out,
            &admission,
            cancel,
            RetrieveHedgeDemand::Ordinary,
        );
        next += 1;
        dispatched += 1;
    }
    let mut hedge_started = Date::now();

    loop {
        if !join_cancel_token_current(cancel_generations, cancel).await {
            return None;
        }
        if completed == dispatched {
            if next == requests.len() {
                return None;
            }
            if !join_cancel_token_current(cancel_generations, cancel).await {
                return None;
            }
            let end = (next + next_batch).min(requests.len());
            while next < end {
                queue_drained_raw_chunk(
                    next,
                    requests[next].clone(),
                    root_cac.clone(),
                    Some(reference.to_vec()),
                    chunk_retrieve_chan,
                    &result_out,
                    &admission,
                    cancel,
                    RetrieveHedgeDemand::Ordinary,
                );
                next += 1;
                dispatched += 1;
            }
            next_batch = next_batch.saturating_mul(2);
            hedge_started = Date::now();
        }

        let result = if next < requests.len() {
            let elapsed = (Date::now() - hedge_started).max(0.0) as u64;
            let remaining = RETRIEVE_HEDGE_AFTER_MS.saturating_sub(elapsed).max(1);
            match async_std::future::timeout(Duration::from_millis(remaining), result_in.recv())
                .await
            {
                Ok(Ok(result)) => RawFetchReceive::Result(result),
                Ok(Err(_)) => RawFetchReceive::ChannelClosed,
                Err(_) => {
                    if !join_cancel_token_current(cancel_generations, cancel).await {
                        return None;
                    }
                    let end = (next + next_batch).min(requests.len());
                    while next < end {
                        queue_drained_raw_chunk(
                            next,
                            requests[next].clone(),
                            root_cac.clone(),
                            Some(reference.to_vec()),
                            chunk_retrieve_chan,
                            &result_out,
                            &admission,
                            cancel,
                            RetrieveHedgeDemand::Ordinary,
                        );
                        next += 1;
                        dispatched += 1;
                    }
                    next_batch = next_batch.saturating_mul(2);
                    hedge_started = Date::now();
                    continue;
                }
            }
        } else {
            recv_raw_result_cancellable(&result_in, cancel_generations, cancel).await
        };

        let result = match result {
            RawFetchReceive::Result(result) => result,
            RawFetchReceive::Stale | RawFetchReceive::ChannelClosed => return None,
        };
        completed += 1;
        let accepted = !result.chunk.is_empty() && (result.index == 0 || result.canonical_cac);
        if accepted {
            admission.close();
        }
        if !join_cancel_token_current(cancel_generations, cancel).await {
            return None;
        }
        if accepted {
            return Some((result.chunk, result.canonical_cac));
        }
    }
}

pub(crate) async fn retrieve_decoded_data_root(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<DecodedJoinChunk> {
    retrieve_decoded_data_root_cancellable(data_address, chunk_retrieve_chan, None, None).await
}

pub(crate) async fn retrieve_decoded_data_root_cancellable(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel_generations: Option<RetrieveGenerationMap>,
    cancel: Option<RetrieveCancelToken>,
) -> Option<DecodedJoinChunk> {
    if data_address.len() != HASH_SIZE
        && data_address.len() != erasure_coding::ENCRYPTED_REFERENCE_SIZE
    {
        return None;
    }
    if !join_cancel_token_current(&cancel_generations, &cancel).await {
        return None;
    }
    if let Some(root) = cached_decoded_chunk(data_address) {
        return Some(root);
    }
    let (raw, canonical_cac) = retrieve_raw_root_cancellable(
        data_address,
        chunk_retrieve_chan,
        &cancel_generations,
        &cancel,
    )
    .await?;
    if !join_cancel_token_current(&cancel_generations, &cancel).await {
        return None;
    }
    if canonical_cac {
        if let Some(root) = cached_decoded_chunk(data_address) {
            return Some(root);
        }
    }
    let root = decode_raw_join_chunk(raw.as_ref(), data_address)?;
    if canonical_cac {
        remember_decoded_chunk(data_address.clone(), &root);
    }
    Some(root)
}

fn padded_rs_shard(chunk: &[u8]) -> Option<Vec<u8>> {
    if !(erasure_coding::SPAN_SIZE..=CHUNK_WITH_SPAN_SIZE).contains(&chunk.len()) {
        return None;
    }
    let mut chunk = chunk.to_vec();
    chunk.resize(CHUNK_WITH_SPAN_SIZE, 0);
    Some(chunk)
}

pub(crate) enum RequestedShardCache {
    DecodedAndRaw {
        decoded: DecodedJoinChunk,
        raw: Rc<[u8]>,
    },
    DecodedOnly(DecodedJoinChunk),
    Miss,
}

pub(crate) fn requested_shard_cache(reference: &[u8]) -> RequestedShardCache {
    let Some((decoded, raw)) = cached_decoded_and_raw_chunk(reference) else {
        return RequestedShardCache::Miss;
    };
    let canonical_raw = raw.filter(|raw| {
        (erasure_coding::SPAN_SIZE..=CHUNK_WITH_SPAN_SIZE).contains(&raw.len())
            && valid_cac(raw, &reference[..HASH_SIZE])
    });
    match canonical_raw {
        Some(raw) => RequestedShardCache::DecodedAndRaw { decoded, raw },
        None => RequestedShardCache::DecodedOnly(decoded),
    }
}

fn dispatch_group_recovery(
    data_references: &[Vec<u8>],
    parity_references: &[Vec<u8>],
    dispatched_shards: &mut [bool],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    result_out: &mpsc::Sender<RawFetchResult>,
    limit: usize,
    admission: &RetrieveAdmission,
    cancel: &Option<RetrieveCancelToken>,
) -> usize {
    let data_count = data_references.len();
    let mut count = 0usize;
    for (index, reference) in data_references.iter().enumerate() {
        if count >= limit {
            return count;
        }
        if dispatched_shards[index] {
            continue;
        }
        if let Some(chunk) = cached_raw_chunk(reference) {
            let _ = result_out.try_send(RawFetchResult {
                index,
                chunk,
                canonical_cac: true,
            });
        } else {
            let address = reference[..HASH_SIZE].to_vec();
            queue_drained_raw_chunk(
                index,
                address.clone(),
                address,
                Some(reference.clone()),
                chunk_retrieve_chan,
                result_out,
                admission,
                cancel,
                RetrieveHedgeDemand::Ordinary,
            );
        }
        dispatched_shards[index] = true;
        count += 1;
    }
    count
        + dispatch_group_parity(
            data_count,
            parity_references,
            dispatched_shards,
            chunk_retrieve_chan,
            result_out,
            limit.saturating_sub(count),
            admission,
            cancel,
        )
}

fn dispatch_group_parity(
    data_count: usize,
    parity_references: &[Vec<u8>],
    dispatched_shards: &mut [bool],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    result_out: &mpsc::Sender<RawFetchResult>,
    limit: usize,
    admission: &RetrieveAdmission,
    cancel: &Option<RetrieveCancelToken>,
) -> usize {
    let mut count = 0usize;
    for (parity_index, reference) in parity_references.iter().enumerate() {
        if count >= limit {
            break;
        }
        let index = data_count + parity_index;
        if dispatched_shards[index] {
            continue;
        }
        queue_drained_raw_chunk(
            index,
            reference.clone(),
            reference.clone(),
            None,
            chunk_retrieve_chan,
            result_out,
            admission,
            cancel,
            RetrieveHedgeDemand::Ordinary,
        );
        dispatched_shards[index] = true;
        count += 1;
    }
    count
}

fn dispatch_one_rolling_group_parity(
    data_count: usize,
    parity_references: &[Vec<u8>],
    dispatched_shards: &mut [bool],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    result_out: &mpsc::Sender<RawFetchResult>,
    admission: &RetrieveAdmission,
    cancel: &Option<RetrieveCancelToken>,
) -> Option<(usize, RawFetchRegistration)> {
    let index = rolling_next_parity_index(data_count, dispatched_shards)?;
    let reference = parity_references.get(index.checked_sub(data_count)?)?;
    let registration = queue_drained_raw_chunk(
        index,
        reference.clone(),
        reference.clone(),
        None,
        chunk_retrieve_chan,
        result_out,
        admission,
        cancel,
        RetrieveHedgeDemand::DistinctShardManaged,
    );
    dispatched_shards[index] = true;
    Some((index, registration))
}

fn recovery_top_up_count(
    data_count: usize,
    successes: usize,
    dispatched: usize,
    completed: usize,
) -> usize {
    let active = dispatched.saturating_sub(completed);
    data_count
        .saturating_add(RETRIEVE_RECOVERY_EXTRA_SHARDS)
        .saturating_sub(successes.saturating_add(active))
}

#[allow(clippy::too_many_arguments)]
fn settle_data_group_result(
    result: RawFetchResult,
    rolling: bool,
    data_count: usize,
    data_references: &[Vec<u8>],
    parity_present: bool,
    requested_mask: &[bool],
    requested_ready: &mut [bool],
    received_shards: &mut [Option<Rc<[u8]>>],
    authenticated_shards: &mut [bool],
    rolling_active_shards: &mut [bool],
    rolling_active: &mut usize,
    successes: &mut usize,
    child_emitter: &GroupChildEmitter,
) -> Option<bool> {
    if rolling && *rolling_active_shards.get(result.index)? {
        rolling_active_shards[result.index] = false;
        *rolling_active = rolling_active.checked_sub(1)?;
    }

    let canonical_for_group = result.canonical_cac || !parity_present;
    if result.chunk.is_empty() || !canonical_for_group {
        return Some(false);
    }

    let result_index = result.index;
    let result_chunk = Rc::clone(&result.chunk);
    *received_shards.get_mut(result_index)? = Some(result.chunk);
    *authenticated_shards.get_mut(result_index)? = result.canonical_cac;
    *successes = successes.checked_add(1)?;

    if result_index < data_count
        && *requested_mask.get(result_index)?
        && !*requested_ready.get(result_index)?
    {
        let reference = data_references.get(result_index)?;
        let chunk = if result.canonical_cac {
            cached_decoded_chunk(reference).or_else(|| {
                remember_raw_chunk(reference.clone(), Rc::clone(&result_chunk));
                cached_decoded_chunk(reference)
            })?
        } else {
            if parity_present {
                return None;
            }
            decode_raw_join_chunk(result_chunk.as_ref(), reference)?
        };
        child_emitter.emit(result_index, chunk);
        requested_ready[result_index] = true;
    }

    Some(true)
}

async fn fetch_data_group_indices_streaming(
    data_references: Vec<Vec<u8>>,
    parity_references: Vec<Vec<u8>>,
    encrypted: bool,
    mut requested_indices: Vec<usize>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel_generations: Option<RetrieveGenerationMap>,
    cancel: Option<RetrieveCancelToken>,
    child_emitter: GroupChildEmitter,
) -> Option<()> {
    let data_count = data_references.len();
    let total_count = data_count.checked_add(parity_references.len())?;
    let expected_data_ref_len = if encrypted {
        erasure_coding::ENCRYPTED_REFERENCE_SIZE
    } else {
        HASH_SIZE
    };
    if data_count == 0
        || total_count > 256
        || data_references
            .iter()
            .any(|reference| reference.len() != expected_data_ref_len)
        || parity_references
            .iter()
            .any(|reference| reference.len() != HASH_SIZE)
    {
        return None;
    }
    requested_indices.sort_unstable();
    requested_indices.dedup();
    if requested_indices.is_empty() {
        return Some(());
    }
    if requested_indices.iter().any(|&index| index >= data_count) {
        return None;
    }
    if !join_cancel_token_current(&cancel_generations, &cancel).await {
        return None;
    }

    let requested_count = requested_indices.len();
    let mut requested_mask = vec![false; data_count];
    for &index in &requested_indices {
        requested_mask[index] = true;
    }

    let waiter_admission = RetrieveAdmission::new();
    let _waiter_admission_guard = waiter_admission.close_on_drop();

    let (result_out, result_in) = mpsc::unbounded::<RawFetchResult>();
    let mut dispatched_shards = vec![false; total_count];
    let mut requested_ready = vec![false; data_count];
    let mut received_shards: Vec<Option<Rc<[u8]>>> = vec![None; total_count];
    let mut authenticated_shards = vec![false; total_count];
    let static_rolling_candidate = rolling_full_group_static_candidate(
        true,
        requested_count,
        data_count,
        parity_references.len(),
    );
    let mut cached_requested = Vec::new();
    let mut decoded_only_count = 0usize;
    let mut unresolved_count = 0usize;
    if static_rolling_candidate {
        cached_requested.reserve(requested_count);
        for &index in &requested_indices {
            let cached = requested_shard_cache(&data_references[index]);
            match &cached {
                RequestedShardCache::DecodedAndRaw { .. } => {}
                RequestedShardCache::DecodedOnly(_) => decoded_only_count += 1,
                RequestedShardCache::Miss => unresolved_count += 1,
            }
            cached_requested.push((index, cached));
        }
    }
    let rolling = static_rolling_candidate
        && rolling_full_group_eligible(
            true,
            requested_count,
            data_count,
            parity_references.len(),
            decoded_only_count,
            unresolved_count,
        );
    let initial_hedge_demand = if rolling {
        RetrieveHedgeDemand::DistinctShardManaged
    } else {
        RetrieveHedgeDemand::Ordinary
    };
    let mut rolling_active_shards = if rolling {
        vec![false; total_count]
    } else {
        Vec::new()
    };
    let mut rolling_active = 0usize;
    let mut successes = 0usize;
    let mut dispatched = 0usize;
    let queue_initial_data_shard = |index: usize, hedge_demand: RetrieveHedgeDemand| {
        let reference = &data_references[index];
        let address = reference[..HASH_SIZE].to_vec();
        queue_drained_raw_chunk(
            index,
            address.clone(),
            address,
            Some(reference.clone()),
            chunk_retrieve_chan,
            &result_out,
            &waiter_admission,
            &cancel,
            hedge_demand,
        )
    };
    if static_rolling_candidate {
        for (index, cached) in cached_requested {
            match cached {
                RequestedShardCache::DecodedAndRaw { decoded, raw } => {
                    child_emitter.emit(index, decoded);
                    requested_ready[index] = true;
                    if rolling {
                        received_shards[index] = Some(raw);
                        authenticated_shards[index] = true;
                        dispatched_shards[index] = true;
                        successes += 1;
                    }
                    continue;
                }
                RequestedShardCache::DecodedOnly(decoded) => {
                    child_emitter.emit(index, decoded);
                    requested_ready[index] = true;
                    continue;
                }
                RequestedShardCache::Miss => {}
            }
            let registration = queue_initial_data_shard(index, initial_hedge_demand);
            dispatched_shards[index] = true;
            dispatched += 1;
            if rolling && registration != RawFetchRegistration::Cached {
                rolling_active_shards[index] = true;
                rolling_active += 1;
            }
        }
    } else {
        // Partial groups inspect, emit, and dispatch each requested child in order without
        // cloning or validating an unneeded cached raw basis.
        for &index in &requested_indices {
            let reference = &data_references[index];
            if let Some(chunk) = cached_decoded_chunk(reference) {
                child_emitter.emit(index, chunk);
                requested_ready[index] = true;
                continue;
            }
            let _registration = queue_initial_data_shard(index, RetrieveHedgeDemand::Ordinary);
            dispatched_shards[index] = true;
            dispatched += 1;
        }
    }
    // Anchor both hedge policies after all initial registrations. This preserves the legacy
    // deadline and gives the rolling set a full hedge interval before parity can replace it.
    let started = Date::now();

    let mut completed = 0usize;
    let mut recovery_dispatched = false;

    loop {
        if rolling {
            let mut ready_results = Vec::new();
            while let Ok(result) = result_in.try_recv() {
                ready_results.push(result);
            }
            if !ready_results.is_empty() {
                if !join_cancel_token_current(&cancel_generations, &cancel).await {
                    return None;
                }
                for result in ready_results {
                    completed = completed.checked_add(1)?;
                    let settled = settle_data_group_result(
                        result,
                        rolling,
                        data_count,
                        &data_references,
                        !parity_references.is_empty(),
                        &requested_mask,
                        &mut requested_ready,
                        &mut received_shards,
                        &mut authenticated_shards,
                        &mut rolling_active_shards,
                        &mut rolling_active,
                        &mut successes,
                        &child_emitter,
                    );
                    settled?;
                }
                // Re-evaluate terminal state before any replacement admission.
                continue;
            }
        }

        let all_requested_ready = requested_indices
            .iter()
            .all(|&index| requested_ready[index]);
        let terminal = all_requested_ready || (recovery_dispatched && successes >= data_count);
        if terminal {
            waiter_admission.close();
            if !join_cancel_token_current(&cancel_generations, &cancel).await {
                return None;
            }
            break;
        }
        if !join_cancel_token_current(&cancel_generations, &cancel).await {
            return None;
        }

        if rolling {
            let mut settled_ready = false;
            while let Ok(result) = result_in.try_recv() {
                completed = completed.checked_add(1)?;
                let settled = settle_data_group_result(
                    result,
                    rolling,
                    data_count,
                    &data_references,
                    !parity_references.is_empty(),
                    &requested_mask,
                    &mut requested_ready,
                    &mut received_shards,
                    &mut authenticated_shards,
                    &mut rolling_active_shards,
                    &mut rolling_active,
                    &mut successes,
                    &child_emitter,
                );
                settled?;
                settled_ready = true;
            }
            if settled_ready {
                // Currentness was checked immediately before this synchronous drain. Re-run the
                // terminal test before spending a replacement credit.
                continue;
            }
        }

        if rolling {
            let elapsed = (Date::now() - started).max(0.0) as u64;
            let remaining_parity = (data_count..total_count)
                .filter(|&index| !dispatched_shards[index])
                .count();
            if rolling_parity_admission_count(
                elapsed,
                RETRIEVE_HEDGE_AFTER_MS,
                terminal,
                data_count,
                rolling_active,
                remaining_parity,
            ) != 0
            {
                let (index, registration) = dispatch_one_rolling_group_parity(
                    data_count,
                    &parity_references,
                    &mut dispatched_shards,
                    chunk_retrieve_chan,
                    &result_out,
                    &waiter_admission,
                    &cancel,
                )?;
                dispatched += 1;
                recovery_dispatched = true;
                if registration != RawFetchRegistration::Cached {
                    rolling_active_shards[index] = true;
                    rolling_active += 1;
                }
                // Admit at most one replacement per coordinator turn, then settle any immediately
                // ready Cached/completed result and re-check terminal state before another.
                continue;
            }
        } else if recovery_dispatched {
            let top_up = recovery_top_up_count(data_count, successes, dispatched, completed);
            if top_up > 0 {
                dispatched += dispatch_group_recovery(
                    &data_references,
                    &parity_references,
                    &mut dispatched_shards,
                    chunk_retrieve_chan,
                    &result_out,
                    top_up,
                    &waiter_admission,
                    &cancel,
                );
            }
        }

        if !rolling && completed == dispatched {
            if recovery_dispatched || parity_references.is_empty() {
                return None;
            }
            if !join_cancel_token_current(&cancel_generations, &cancel).await {
                return None;
            }
            recovery_dispatched = true;
            continue;
        }

        let result = if rolling {
            let elapsed = (Date::now() - started).max(0.0) as u64;
            if completed == dispatched
                && elapsed >= RETRIEVE_HEDGE_AFTER_MS
                && (data_count..total_count).all(|index| dispatched_shards[index])
            {
                return None;
            }
            if elapsed < RETRIEVE_HEDGE_AFTER_MS {
                let remaining = RETRIEVE_HEDGE_AFTER_MS.saturating_sub(elapsed).max(1);
                match async_std::future::timeout(Duration::from_millis(remaining), result_in.recv())
                    .await
                {
                    Ok(Ok(result)) => RawFetchReceive::Result(result),
                    Ok(Err(_)) => RawFetchReceive::ChannelClosed,
                    Err(_) => continue,
                }
            } else {
                recv_raw_result_cancellable(&result_in, &cancel_generations, &cancel).await
            }
        } else if !recovery_dispatched && parity_references.is_empty() {
            recv_raw_result_cancellable(&result_in, &cancel_generations, &cancel).await
        } else if !recovery_dispatched {
            let elapsed = (Date::now() - started).max(0.0) as u64;
            let remaining = RETRIEVE_RS_HEDGE_AFTER_MS.saturating_sub(elapsed).max(1);
            match async_std::future::timeout(Duration::from_millis(remaining), result_in.recv())
                .await
            {
                Ok(Ok(result)) => RawFetchReceive::Result(result),
                Ok(Err(_)) => RawFetchReceive::ChannelClosed,
                Err(_) => {
                    if !join_cancel_token_current(&cancel_generations, &cancel).await {
                        return None;
                    }
                    if requested_count == data_count {
                        dispatched += dispatch_group_parity(
                            data_count,
                            &parity_references,
                            &mut dispatched_shards,
                            chunk_retrieve_chan,
                            &result_out,
                            usize::MAX,
                            &waiter_admission,
                            &cancel,
                        );
                    }
                    recovery_dispatched = true;
                    continue;
                }
            }
        } else if dispatched_shards.iter().any(|dispatched| !*dispatched) {
            match async_std::future::timeout(
                Duration::from_millis(RETRIEVE_RS_HEDGE_AFTER_MS),
                result_in.recv(),
            )
            .await
            {
                Ok(Ok(result)) => RawFetchReceive::Result(result),
                Ok(Err(_)) => RawFetchReceive::ChannelClosed,
                Err(_) => {
                    if !join_cancel_token_current(&cancel_generations, &cancel).await {
                        return None;
                    }
                    dispatched += dispatch_group_recovery(
                        &data_references,
                        &parity_references,
                        &mut dispatched_shards,
                        chunk_retrieve_chan,
                        &result_out,
                        RETRIEVE_RECOVERY_PROGRESSIVE_BATCH,
                        &waiter_admission,
                        &cancel,
                    );
                    continue;
                }
            }
        } else {
            recv_raw_result_cancellable(&result_in, &cancel_generations, &cancel).await
        };

        let result = match result {
            RawFetchReceive::Result(result) => result,
            RawFetchReceive::Stale | RawFetchReceive::ChannelClosed => return None,
        };
        completed = completed.checked_add(1)?;
        if !join_cancel_token_current(&cancel_generations, &cancel).await {
            return None;
        }
        let usable = settle_data_group_result(
            result,
            rolling,
            data_count,
            &data_references,
            !parity_references.is_empty(),
            &requested_mask,
            &mut requested_ready,
            &mut received_shards,
            &mut authenticated_shards,
            &mut rolling_active_shards,
            &mut rolling_active,
            &mut successes,
            &child_emitter,
        );
        let usable = usable?;
        if !usable && !rolling && !recovery_dispatched {
            if parity_references.is_empty() {
                return None;
            }
            if !join_cancel_token_current(&cancel_generations, &cancel).await {
                return None;
            }
            recovery_dispatched = true;
            continue;
        }
    }

    for (index, raw) in received_shards.iter().take(data_count).enumerate() {
        if authenticated_shards[index]
            && let Some(raw) = raw
        {
            remember_raw_chunk(data_references[index].clone(), Rc::clone(raw));
        }
    }

    let missing_indices = requested_indices
        .iter()
        .copied()
        .filter(|&index| !requested_ready[index])
        .collect::<Vec<_>>();
    if missing_indices.is_empty() {
        return Some(());
    }
    if !join_cancel_token_current(&cancel_generations, &cancel).await {
        return None;
    }
    let mut reconstructed_shards = received_shards
        .iter()
        .map(|chunk| chunk.as_ref().and_then(|chunk| padded_rs_shard(chunk)))
        .collect::<Vec<_>>();
    reconstruct_data_indices(&mut reconstructed_shards, data_count, &missing_indices).ok()?;

    for index in missing_indices {
        let reference = &data_references[index];
        let raw = reconstructed_shards[index].take()?;
        if !valid_cac(&raw, &reference[..HASH_SIZE]) {
            return None;
        }
        remember_raw_chunk(reference.clone(), raw.into());
        child_emitter.emit(index, cached_decoded_chunk(reference)?);
    }
    Some(())
}

#[derive(Clone)]
struct TraversalNode {
    start: u64,
    depth: usize,
    chunk: DecodedJoinChunk,
}

#[derive(Clone, Copy)]
struct GroupTraversalContext {
    parent_start: u64,
    parent_span: u64,
    parent_depth: usize,
    child_capacity: u64,
    child_count: usize,
}

enum GroupFetchEvent {
    Child {
        context: GroupTraversalContext,
        index: usize,
        chunk: DecodedJoinChunk,
    },
    Terminal {
        success: bool,
    },
}

#[derive(Clone)]
struct GroupChildEmitter {
    context: GroupTraversalContext,
    events: mpsc::Sender<GroupFetchEvent>,
}

impl GroupChildEmitter {
    fn emit(&self, index: usize, chunk: DecodedJoinChunk) {
        let _ = self.events.try_send(GroupFetchEvent::Child {
            context: self.context,
            index,
            chunk,
        });
    }

    fn finish(&self, success: bool) {
        let _ = self.events.try_send(GroupFetchEvent::Terminal { success });
    }
}

type GroupJoiner = FuturesUnordered<Pin<Box<dyn Future<Output = ()>>>>;

fn allocate_join_output(payload_len: u64, prefix: &[u8]) -> Option<Vec<u8>> {
    let payload_len = usize::try_from(payload_len).ok()?;
    let len = prefix.len().checked_add(payload_len)?;
    let mut output = Vec::new();
    output.try_reserve_exact(len).ok()?;
    output.resize(len, 0);
    output.get_mut(..prefix.len())?.copy_from_slice(prefix);
    Some(output)
}

pub(crate) async fn retrieve_data_range_from_root(
    root: DecodedJoinChunk,
    payload_start: u64,
    payload_end_inclusive: u64,
    encrypted: bool,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<Vec<u8>> {
    retrieve_data_range_from_root_cancellable(
        root,
        payload_start,
        payload_end_inclusive,
        encrypted,
        chunk_retrieve_chan,
        None,
        None,
    )
    .await
}

pub(crate) async fn retrieve_data_range_from_root_cancellable(
    root: DecodedJoinChunk,
    payload_start: u64,
    payload_end_inclusive: u64,
    encrypted: bool,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel_generations: Option<RetrieveGenerationMap>,
    cancel: Option<RetrieveCancelToken>,
) -> Option<Vec<u8>> {
    retrieve_data_range_from_root_with_prefix_cancellable(
        root,
        payload_start,
        payload_end_inclusive,
        encrypted,
        chunk_retrieve_chan,
        &[],
        cancel_generations,
        cancel,
    )
    .await
}

async fn retrieve_data_range_from_root_with_prefix_cancellable(
    root: DecodedJoinChunk,
    payload_start: u64,
    payload_end_inclusive: u64,
    encrypted: bool,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    output_prefix: &[u8],
    cancel_generations: Option<RetrieveGenerationMap>,
    cancel: Option<RetrieveCancelToken>,
) -> Option<Vec<u8>> {
    if !join_cancel_token_current(&cancel_generations, &cancel).await {
        return None;
    }
    if payload_start > payload_end_inclusive || payload_start >= root.span {
        return Some(output_prefix.to_vec());
    }
    let payload_end_inclusive = payload_end_inclusive.min(root.span.checked_sub(1)?);
    let requested_len = payload_end_inclusive
        .checked_sub(payload_start)?
        .checked_add(1)?;
    let mut output = allocate_join_output(requested_len, output_prefix)?;
    let mut written = 0u64;
    let mut pending = VecDeque::from([TraversalNode {
        start: 0,
        depth: 0,
        chunk: root,
    }]);
    let mut groups: GroupJoiner = FuturesUnordered::new();
    let (group_event_out, group_event_in) = mpsc::unbounded::<GroupFetchEvent>();
    let mut active_groups = 0usize;

    while !pending.is_empty() || active_groups > 0 {
        if !join_cancel_token_current(&cancel_generations, &cancel).await {
            return None;
        }
        while groups.len() < RETRIEVE_DATA_GROUP_CONCURRENCY {
            let Some(node) = pending.pop_front() else {
                break;
            };
            if !join_cancel_token_current(&cancel_generations, &cancel).await {
                return None;
            }
            if node.depth > BEE_MAX_UPLOAD_TREE_LEVELS {
                return None;
            }

            if node.chunk.span <= CHUNK_SIZE as u64 {
                let leaf_end = node.start.checked_add(node.chunk.span)?.checked_sub(1)?;
                let copy_start = node.start.max(payload_start);
                let copy_end = leaf_end.min(payload_end_inclusive);
                if copy_start <= copy_end {
                    let source_start = usize::try_from(copy_start.checked_sub(node.start)?).ok()?;
                    let destination_start = output_prefix.len().checked_add(
                        usize::try_from(copy_start.checked_sub(payload_start)?).ok()?,
                    )?;
                    let copy_len =
                        usize::try_from(copy_end.checked_sub(copy_start)?.checked_add(1)?).ok()?;
                    let source_end = source_start.checked_add(copy_len)?;
                    let destination_end = destination_start.checked_add(copy_len)?;
                    output
                        .get_mut(destination_start..destination_end)?
                        .copy_from_slice(node.chunk.payload.get(source_start..source_end)?);
                    written = written.checked_add(copy_len as u64)?;
                }
                continue;
            }

            let layout = reference_layout(node.chunk.span, node.chunk.level, encrypted)?;
            let (data_references, parity_references) = split_references(
                node.chunk.payload.as_ref(),
                node.chunk.span,
                node.chunk.level,
                encrypted,
            )?;
            if data_references.len() != layout.data_shards
                || parity_references.len() != layout.parity_shards
            {
                return None;
            }
            let relative_start = payload_start.saturating_sub(node.start);
            let relative_end = payload_end_inclusive.checked_sub(node.start)?;
            let last_data_index = layout.data_shards.checked_sub(1)?;
            let first_index = usize::try_from(relative_start / layout.child_capacity)
                .ok()?
                .min(last_data_index);
            let last_index = usize::try_from(relative_end / layout.child_capacity)
                .ok()?
                .min(last_data_index);
            if first_index > last_index {
                return None;
            }
            let requested_indices = (first_index..=last_index).collect::<Vec<_>>();
            if !join_cancel_token_current(&cancel_generations, &cancel).await {
                return None;
            }
            let sender = chunk_retrieve_chan.clone();
            let group_cancel_generations = cancel_generations.clone();
            let group_cancel = cancel.clone();
            let emitter = GroupChildEmitter {
                context: GroupTraversalContext {
                    parent_start: node.start,
                    parent_span: node.chunk.span,
                    parent_depth: node.depth,
                    child_capacity: layout.child_capacity,
                    child_count: layout.data_shards,
                },
                events: group_event_out.clone(),
            };
            let terminal_emitter = emitter.clone();
            groups.push(Box::pin(async move {
                let success = fetch_data_group_indices_streaming(
                    data_references,
                    parity_references,
                    encrypted,
                    requested_indices,
                    &sender,
                    group_cancel_generations,
                    group_cancel,
                    emitter,
                )
                .await
                .is_some();
                terminal_emitter.finish(success);
            }));
            active_groups = active_groups.checked_add(1)?;
        }

        if active_groups == 0 {
            continue;
        }

        let event = if groups.is_empty() {
            group_event_in.recv().await.ok()?
        } else {
            let next_event = group_event_in.recv();
            let next_completion = groups.next();
            pin_mut!(next_event, next_completion);
            match select(next_event, next_completion).await {
                Either::Left((event, _)) => event.ok()?,
                Either::Right((_completion, _)) => continue,
            }
        };
        if !join_cancel_token_current(&cancel_generations, &cancel).await {
            return None;
        }

        match event {
            GroupFetchEvent::Terminal { success } => {
                active_groups = active_groups.checked_sub(1)?;
                if !success {
                    return None;
                }
            }
            GroupFetchEvent::Child {
                context,
                index,
                chunk,
            } => {
                let index = u64::try_from(index).ok()?;
                let child_offset = context.child_capacity.checked_mul(index)?;
                let child_start = context.parent_start.checked_add(child_offset)?;
                let child_limit = if index as usize + 1 == context.child_count {
                    context.parent_span.checked_sub(child_offset)?
                } else {
                    context.child_capacity
                };
                if chunk.span > child_limit {
                    return None;
                }
                let child_end = child_start.checked_add(child_limit)?.checked_sub(1)?;
                if child_start <= payload_end_inclusive && child_end >= payload_start {
                    pending.push_back(TraversalNode {
                        start: child_start,
                        depth: context.parent_depth + 1,
                        chunk,
                    });
                }
            }
        }
    }

    (written == requested_len).then_some(output)
}

async fn retrieve_data_joined(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    include_span_prefix: bool,
) -> Vec<u8> {
    let encrypted = data_address.len() == erasure_coding::ENCRYPTED_REFERENCE_SIZE;
    let Some(root) = retrieve_decoded_data_root(data_address, chunk_retrieve_chan).await else {
        return vec![];
    };
    let span = root.span;
    let span_prefix = span.to_le_bytes();
    let output_prefix: &[u8] = if include_span_prefix {
        &span_prefix
    } else {
        &[]
    };
    let data = if span == 0 {
        output_prefix.to_vec()
    } else {
        let Some(payload) = retrieve_data_range_from_root_with_prefix_cancellable(
            root,
            0,
            span - 1,
            encrypted,
            chunk_retrieve_chan,
            output_prefix,
            None,
            None,
        )
        .await
        else {
            return vec![];
        };
        payload
    };
    data
}

/// Retrieve Bee's historical joined representation (`span || payload`).
pub async fn retrieve_data(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Vec<u8> {
    retrieve_data_joined(data_address, chunk_retrieve_chan, true).await
}

/// Retrieve a Bee bytes payload without its internal span.
pub(crate) async fn retrieve_data_payload(
    data_address: &Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Vec<u8> {
    retrieve_data_joined(data_address, chunk_retrieve_chan, false).await
}

pub async fn retrieve_chunk(
    chunk_address: &Vec<u8>,
    control: StreamControl,
    peers: &Arc<Mutex<HashMap<Vec<u8>, PeerId>>>,
    accounting: &Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    physical_connections: &PhysicalConnectionMap,
    refresh_chan: &mpsc::Sender<RefreshmentInstruction>,
    cancel_generations: Option<RetrieveGenerationMap>,
    cancel: Option<RetrieveCancelToken>,
    admission: Option<RetrieveAdmission>,
    hedge_demand: Option<SharedRetrieveHedgeDemand>,
    transfer_paused: Option<Arc<AtomicBool>>,
) -> Vec<u8> {
    let (caddr, encrey, encred) = chunk_address_parts(chunk_address);

    let mut soc = false;
    let mut skiplist: HashSet<PeerId> = HashSet::new();
    let mut overdraftlist: HashSet<PeerId> = HashSet::new();

    let mut attempt_count = 0;
    let mut error_count = 0;

    #[allow(unused_assignments)]
    let mut cd = vec![];

    let (attempt_out, attempt_in) = mpsc::unbounded::<RetrieveAttemptResult>();
    let mut in_flight = 0_usize;
    let mut last_attempt_started = 0.0;

    while error_count < RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS
        && (attempt_count < RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS || in_flight > 0)
    {
        if in_flight > 0 {
            if let Ok(result) = attempt_in.try_recv() {
                in_flight = in_flight.saturating_sub(1);
                if result.valid {
                    cd = result.chunk;
                    soc = result.soc;
                    break;
                }
                error_count += 1;
                cd = vec![];

                async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
                continue;
            }
        }

        let paused = transfer_paused
            .as_ref()
            .map(transfer_pause_enabled)
            .unwrap_or(false);
        let admission_current =
            chunk_retrieve_admission_current(&cancel_generations, &cancel, &admission).await;
        if !admission_current && in_flight == 0 {
            break;
        }
        let cancelled = !admission_current;

        if paused && in_flight == 0 {
            async_std::task::sleep(Duration::from_millis(100)).await;
            continue;
        }

        let now = Date::now();
        let can_start_attempt = attempt_count < RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS
            && admission
                .as_ref()
                .is_none_or(RetrieveAdmission::physical_attempt_available);
        // Read the shared demand on every admission loop. An ordinary follower
        // can promote an already-running managed leader after dispatcher delay.
        let current_hedge_demand = hedge_demand
            .as_ref()
            .map(SharedRetrieveHedgeDemand::current)
            .unwrap_or(RetrieveHedgeDemand::Ordinary);
        let ordinary_hedge_due = now - last_attempt_started >= RETRIEVE_HEDGE_AFTER_MS as f64;
        let due = can_start_attempt
            && !paused
            && !cancelled
            && retrieve_attempt_start_allowed(current_hedge_demand, in_flight, ordinary_hedge_due);

        if due {
            if let Some(selected) = select_retrieve_peer(
                &caddr,
                peers,
                accounting,
                physical_connections,
                &mut skiplist,
                &mut overdraftlist,
            )
            .await
            {
                if !chunk_retrieve_admission_current(&cancel_generations, &cancel, &admission).await
                {
                    cancel_reserve(&selected.accounting, selected.price).await;
                    skiplist.remove(&selected.peer);
                    if in_flight == 0 {
                        break;
                    }
                    async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
                    continue;
                }

                if transfer_paused
                    .as_ref()
                    .map(transfer_pause_enabled)
                    .unwrap_or(false)
                {
                    cancel_reserve(&selected.accounting, selected.price).await;
                    skiplist.remove(&selected.peer);
                    async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
                    continue;
                }

                if admission
                    .as_ref()
                    .is_some_and(|admission| !admission.try_claim_physical_attempt())
                {
                    cancel_reserve(&selected.accounting, selected.price).await;
                    skiplist.remove(&selected.peer);
                    if in_flight == 0 {
                        break;
                    }
                    async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
                    continue;
                }

                let control = control.clone();
                let refresh_chan = refresh_chan.clone();
                let attempt_out = attempt_out.clone();
                let caddr = caddr.clone();
                let attempt_admission = admission.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    retrieve_attempt(
                        selected,
                        caddr,
                        control,
                        refresh_chan,
                        attempt_out,
                        attempt_admission,
                    )
                    .await;
                });
                attempt_count += 1;
                in_flight += 1;
                last_attempt_started = Date::now();
            } else if !overdraftlist.is_empty() {
                reset_overdraft(&mut skiplist, &mut overdraftlist);
                async_std::task::sleep(Duration::from_millis(50)).await;
                continue;
            } else if in_flight == 0 && !skiplist.is_empty() {
                break;
            } else {
                async_std::task::sleep(Duration::from_millis(50)).await;
                continue;
            }
        }

        if in_flight == 0 {
            async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
            continue;
        }

        let elapsed = Date::now() - last_attempt_started;
        let wait_ms =
            if current_hedge_demand == RetrieveHedgeDemand::DistinctShardManaged && in_flight > 0 {
                RETRIEVE_CANCEL_POLL_MS
            } else if !can_start_attempt || cancelled || paused {
                250
            } else {
                (RETRIEVE_HEDGE_AFTER_MS as f64 - elapsed).max(0.0).round() as u64
            };
        if wait_ms == 0 {
            async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
            continue;
        }

        match async_std::future::timeout(Duration::from_millis(wait_ms), attempt_in.recv()).await {
            Ok(Ok(result)) => {
                in_flight = in_flight.saturating_sub(1);
                if result.valid {
                    cd = result.chunk;
                    soc = result.soc;
                    break;
                }
                error_count += 1;
                cd = vec![];
            }
            Ok(Err(_)) => break,
            Err(_) => {}
        };
    }

    decode_retrieved_chunk(cd, soc, encrey, encred)
}

pub async fn retrieve_check_chunk(
    chunk_address: &Vec<u8>,
    control: StreamControl,
    peers: &Arc<Mutex<HashMap<Vec<u8>, PeerId>>>,
    accounting: &Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    physical_connections: &PhysicalConnectionMap,
    refresh_chan: &mpsc::Sender<RefreshmentInstruction>,
    transfer_paused: Option<Arc<AtomicBool>>,
) -> Vec<u8> {
    let (caddr, encrey, encred) = chunk_address_parts(chunk_address);

    let mut skiplist: HashSet<PeerId> = HashSet::new();
    let mut overdraftlist: HashSet<PeerId> = HashSet::new();
    let mut success_peers: HashSet<PeerId> = HashSet::new();
    let mut reported_peers: HashSet<PeerId> = HashSet::new();

    let mut error_count = 0;
    let max_error = 21 - RETRIEVE_CHECK_CONFIRMATION_PEERS;

    #[allow(unused_assignments)]
    let mut cd = vec![];
    let mut soc = false;
    let (attempt_out, attempt_in) = mpsc::unbounded::<RetrieveAttemptResult>();

    while error_count < max_error && success_peers.len() < RETRIEVE_CHECK_CONFIRMATION_PEERS {
        while let Ok(result) = attempt_in.try_recv() {
            // Each physical attempt reports exactly one logical result. A timed-out transport's
            // later response is consumed only by its detached accounting settlement task.
            if !reported_peers.insert(result.peer.clone()) {
                continue;
            }
            if result.valid {
                if success_peers.insert(result.peer) && cd.is_empty() {
                    cd = result.chunk;
                    soc = result.soc;
                }
            } else {
                error_count += 1;
            }
        }

        if error_count >= max_error || success_peers.len() >= RETRIEVE_CHECK_CONFIRMATION_PEERS {
            break;
        }

        while transfer_paused
            .as_ref()
            .map(transfer_pause_enabled)
            .unwrap_or(false)
        {
            async_std::task::sleep(Duration::from_millis(100)).await;
        }

        let Some(selected) = select_retrieve_peer(
            &caddr,
            peers,
            accounting,
            physical_connections,
            &mut skiplist,
            &mut overdraftlist,
        )
        .await
        else {
            if !overdraftlist.is_empty() {
                reset_overdraft(&mut skiplist, &mut overdraftlist);
            }
            async_std::task::sleep(Duration::from_millis(RETRIEVE_CHECK_RETRY_WAIT_MS)).await;
            continue;
        };

        if transfer_paused
            .as_ref()
            .map(transfer_pause_enabled)
            .unwrap_or(false)
        {
            cancel_reserve(&selected.accounting, selected.price).await;
            async_std::task::sleep(Duration::from_millis(RETRIEVE_HOT_LOOP_GUARD_MS)).await;
            continue;
        }

        retrieve_attempt(
            selected,
            caddr.clone(),
            control.clone(),
            refresh_chan.clone(),
            attempt_out.clone(),
            None,
        )
        .await;
    }

    if success_peers.len() < RETRIEVE_CHECK_CONFIRMATION_PEERS {
        return vec![];
    }

    decode_retrieved_chunk(cd, soc, encrey, encred)
}

pub fn verify_chunk(caddr: &Vec<u8>, cd: &Vec<u8>) -> (bool, bool) {
    let contaddrd = valid_cac(&cd, &caddr);
    if !contaddrd {
        let soc = valid_soc(&cd, &caddr);
        if !soc {
            return (false, false);
        } else {
            return (true, true);
        }
    } else {
        return (true, false);
    }
}

pub fn decrypt(cd: &Vec<u8>, encrey: Vec<u8>) -> Vec<u8> {
    if cd.len() < 8 {
        return vec![];
    }

    let spancred = (&cd[0..8]).to_vec();
    let concred = (&cd[8..]).to_vec();
    let creylen = encrey.len();

    let mut spanbytes: Vec<u8> = vec![];
    let spansegmentkey0 = ((4096 / creylen) as u32).to_le_bytes();
    let spansegmentkey1 =
        keccak256(keccak256([encrey.clone(), spansegmentkey0.to_vec()].concat()).to_vec()).to_vec();

    for j in 0..8 {
        spanbytes.push(spancred[j] ^ spansegmentkey1[j])
    }

    let mut content: Vec<u8> = vec![];
    let mut done = false;
    let mut i = 0;
    while !done {
        let mut k = creylen;
        if k > concred.len() - (i * creylen) {
            k = concred.len() - (i * creylen);
        };

        let contentsegmentkey0 = (i as u32).to_le_bytes();
        let contentsegmentkey1 = keccak256(keccak256(
            [encrey.clone(), contentsegmentkey0.to_vec()].concat(),
        ))
        .to_vec();

        for j in (i * creylen)..(i * creylen + k) {
            content.push(concred[j] ^ contentsegmentkey1[j - i * creylen])
        }

        i += 1;

        if !(i * creylen < concred.len()) {
            done = true;
        }
    }

    let mut span_decrypted = u64::from_le_bytes(spanbytes.clone().try_into().unwrap());

    if span_decrypted > 4096 {
        let mut done0 = false;
        let mut carry_span = 4096_u64;
        while !done0 {
            let k = span_decrypted / carry_span;
            let mut l0 = span_decrypted % carry_span;
            if l0 > 0 {
                l0 = 1;
            }

            if k + l0 <= 64 {
                done0 = true;
                span_decrypted = (k + l0) * 64;
            } else {
                carry_span *= 64;
            }
        }
    };

    return [spanbytes, content[..span_decrypted as usize].to_vec()].concat();
}

async fn get_feed_probe_chunk(
    data_address: Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> FeedProbe<Vec<u8>> {
    let admission = RetrieveAdmission::new();
    let _close_admission = admission.close_on_drop();
    let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
    if chunk_retrieve_chan
        .try_send(crate::ChunkRetrieveRequest {
            address: data_address,
            chan: chan_out,
            cancel: None,
            admission: Some(admission.clone()),
            hedge_demand: None,
        })
        .is_err()
    {
        return FeedProbe::Transient;
    }

    match chan_in.recv().await {
        Ok(payload) if !payload.is_empty() => FeedProbe::Found(payload),
        Ok(_) => FeedProbe::Missing,
        Err(_) => FeedProbe::Transient,
    }
}

async fn probe_feed_update_status(
    owner: &String,
    topic: &String,
    index: u64,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> FeedProbe<Vec<u8>> {
    let address = get_feed_address(owner, topic, index);
    if address.len() != 32 {
        return FeedProbe::Missing;
    }
    get_feed_probe_chunk(address, chunk_retrieve_chan).await
}

async fn seek_feed_frontier(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    early_updates: Option<mpsc::Sender<(u64, Vec<u8>)>>,
) -> (Option<(u64, Vec<u8>)>, u64) {
    match early_updates {
        Some(early_updates) => {
            seek_sequence_feed_frontier_bounded_observing_positive(
                |index| probe_feed_update_status(&owner, &topic, index, chunk_retrieve_chan),
                move |index, payload| {
                    let _ = early_updates.try_send((index, payload.clone()));
                },
            )
            .await
        }
        None => {
            seek_sequence_feed_frontier(|index| {
                probe_feed_update_status(&owner, &topic, index, chunk_retrieve_chan)
            })
            .await
        }
    }
}

pub async fn seek_latest_feed_update(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    _redundancy: u8,
) -> Vec<u8> {
    seek_latest_feed_update_indexed(owner, topic, chunk_retrieve_chan)
        .await
        .map(|(_, payload)| payload)
        .unwrap_or_default()
}

pub(crate) async fn seek_latest_feed_update_indexed(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Option<(u64, Vec<u8>)> {
    seek_latest_feed_update_indexed_observing_positive(owner, topic, chunk_retrieve_chan, None)
        .await
}

pub(crate) async fn seek_latest_feed_update_indexed_observing_positive(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    early_updates: Option<mpsc::Sender<(u64, Vec<u8>)>>,
) -> Option<(u64, Vec<u8>)> {
    match seek_feed_frontier(owner, topic, chunk_retrieve_chan, early_updates).await {
        (Some((index, payload)), _) => Some((index, payload)),
        (None, _) => None,
    }
}

pub async fn seek_next_feed_update_index(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    _redundancy: u8,
) -> u64 {
    let (_latest, next_index) = seek_feed_frontier(owner, topic, chunk_retrieve_chan, None).await;
    next_index
}

#[cfg(test)]
mod raw_fetch_tests {
    use super::*;

    #[test]
    fn zero_waiter_late_success_caches_every_full_reference_owned_by_the_flight() {
        let mut raw = 29_u64.to_le_bytes().to_vec();
        raw.extend_from_slice(b"late raw cache payload");
        let expected_cac = keccak256(&raw).to_vec();
        let plain_reference = expected_cac.clone();
        let mut encrypted_reference = expected_cac.clone();
        encrypted_reference.extend_from_slice(&[0x5a; HASH_SIZE]);
        let key = RawFetchKey::new(
            usize::MAX - 17,
            expected_cac.clone(),
            expected_cac,
            &None,
            None,
        );
        let admission = RetrieveAdmission::new();
        let (result_chan, _result_in) = mpsc::unbounded();
        let registration = RAW_FETCH_FLIGHTS.with(|flights| {
            flights.borrow_mut().register(
                key.clone(),
                RawFetchWaiter {
                    index: 0,
                    result_chan,
                    admission: admission.clone(),
                },
                || RawFetchShared::new(RetrieveHedgeDemand::Ordinary),
            )
        });
        registration
            .shared
            .remember_cache_reference(Some(&plain_reference));
        registration
            .shared
            .remember_cache_reference(Some(&encrypted_reference));
        remove_raw_fetch_waiter(&key, registration.flight_id, registration.waiter_id);

        assert!(complete_raw_fetch(
            &key,
            registration.flight_id,
            raw.clone()
        ));
        assert_eq!(
            cached_raw_chunk(&plain_reference).as_deref(),
            Some(raw.as_slice())
        );
        assert_eq!(
            cached_raw_chunk(&encrypted_reference).as_deref(),
            Some(raw.as_slice())
        );
    }
}
