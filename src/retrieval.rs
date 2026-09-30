use crate::{
    ChunkRetrieveSender, ConnectionId, Date, Duration, Mutex, OutboundProtocolSession, OverlayPeerMap,
    PeerAccounting, PeerAccountingMap, PeerId, PhysicalConnectionMap,
    RETRIEVE_CHECK_CONFIRMATION_PEERS, RefreshmentInstruction, RetrieveCancelToken, StreamControl,
    TransferPause, apply_credit, bee_replica_address, cancel_reserve, encryption_segment_key,
    erasure_coding::{
        self, BEE_MAX_UPLOAD_TREE_LEVELS, CHUNK_SIZE, CHUNK_WITH_SPAN_SIZE, HASH_SIZE,
        RedundancyLevel, ReferenceLayout, encoded_reference_payload_len, reconstruct_data_indices,
        reference_layout, split_references,
    },
    feed::{FeedProbe, seek_sequence_feed_frontier},
    get_feed_address, mpsc, oneshot, price, reserve,
    retrieval_conventions::SingleflightRegistry,
    retrieval_conventions::{
        RetrieveAdmission, RetrieveHedgeDemand, SharedRetrieveHedgeDemand,
        retrieve_admission_current, retrieve_attempt_start_allowed, rolling_full_group_eligible,
        rolling_full_group_static_candidate,
    },
    retrieve_cancel_token_current, retrieve_handler, transfer_pause_enabled, valid_cac, valid_soc,
    wait_transfer_unpaused, weeb_3::etiquette_6,
};

use async_std::sync::Arc;
use bytes::Bytes;
use hashlink::LinkedHashMap;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque, hash_map::RandomState},
    ops::Range,
    rc::Rc,
    slice::ChunksExact,
};

const RETRIEVE_HEDGE_AFTER_MS: u64 = 1_000;
const RETRIEVE_RS_HEDGE_AFTER_MS: u64 = RETRIEVE_HEDGE_AFTER_MS * 2;
const RETRIEVE_RECOVERY_EXTRA_SHARDS: usize = 2;
const RETRIEVE_RECOVERY_PROGRESSIVE_BATCH: usize = 2;
const RETRIEVE_ATTEMPT_TIMEOUT_MS: u64 = 10_000;
const RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS: usize = 20;
const RETRIEVE_DATA_GROUP_CONCURRENCY: usize = 8;
const RETRIEVE_DECODED_CHUNK_CACHE_ENTRIES: usize = 2048;

enum RetrieveAttemptResult {
    Found(Bytes, bool),
    Empty,
    Failed(PeerId, ConnectionId),
}

impl RetrieveAttemptResult {
    fn complete(self, skiplist: &mut HashMap<PeerId, Option<ConnectionId>>) -> Option<(Bytes, bool)> {
        match self {
            Self::Found(chunk, soc) => Some((chunk, soc)),
            Self::Empty => None,
            Self::Failed(peer, connection) => {
                skiplist.insert(peer, Some(connection));
                None
            }
        }
    }
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
    caddr: &[u8],
    peers: &OverlayPeerMap,
    accounting: &PeerAccountingMap,
    physical_connections: &PhysicalConnectionMap,
    skiplist: &mut HashMap<PeerId, Option<ConnectionId>>,
) -> (Option<ReservedRetrievePeer>, bool) {
    let peers = peers.lock().await.clone();
    let accounting = accounting.lock().await;
    let mut overdraft = false;
    for (&peer, proximity) in crate::closest_overlay_peers(&peers, caddr) {
        // None keeps in-flight and definitive responses excluded across reconnections.
        let previous = skiplist.get(&peer);
        if matches!(previous, Some(None)) {
            continue;
        }
        let Some(accounting_peer) = accounting.get(&peer) else {
            skiplist.entry(peer).or_insert(None);
            continue;
        };
        if let Some(Some(connection)) = previous
            && accounting_peer.lock().await.connection_id
                .is_none_or(|id| id == *connection)
        {
            continue;
        }
        let req_price = price(proximity);
        let Some(connection_id) = reserve(accounting_peer, req_price).await else {
            overdraft = true;
            continue;
        };
        skiplist.insert(peer, None);
        if let Some(session) =
            OutboundProtocolSession::capture(peer, connection_id, physical_connections.clone())
        {
            let selected = ReservedRetrievePeer {
                peer,
                price: req_price,
                accounting: accounting_peer.clone(),
                session,
            };
            return (Some(selected), false);
        }
        skiplist.insert(peer, Some(connection_id));
        cancel_reserve(accounting_peer, req_price).await;
    }
    (None, overdraft)
}

async fn retrieve_attempt(
    selected: ReservedRetrievePeer,
    caddr: Vec<u8>,
    control: StreamControl,
    refresh_chan: mpsc::Sender<RefreshmentInstruction>,
    admission: Option<RetrieveAdmission>,
    hedge_demand: Option<SharedRetrieveHedgeDemand>,
) -> RetrieveAttemptResult {
    let ReservedRetrievePeer {
        peer,
        price: req_price,
        accounting: accounting_peer,
        session,
    } = selected;
    let connection_id = session.connection_id();
    let request = etiquette_6::Request { addr: caddr };
    let retrieve_result = async_std::future::timeout(
        Duration::from_millis(RETRIEVE_ATTEMPT_TIMEOUT_MS),
        retrieve_handler(peer, &request, control, session),
    )
    .await;

    let retrieve_result = match retrieve_result {
        Ok(retrieve_result) => retrieve_result,
        Err(_) => {
            if let Some(admission) = admission.as_ref() {
                admission.record_physical_attempt_timeout();
            }
            if let Some(demand) = hedge_demand {
                demand.promote(RetrieveHedgeDemand::Ordinary);
            }
            None
        }
    };
    let confirmed_empty = retrieve_result.as_ref().is_some_and(Bytes::is_empty);
    if confirmed_empty && let Some(admission) = admission.as_ref() {
        admission.record_confirmed_empty_physical_attempt();
    }
    // Consumer cancellation never drops this owned attempt. Its original deadline ends
    // the exchange; every outcome settles the reserve before logical completion.
    if let Some(chunk) = retrieve_result {
        let (chunk_valid, soc) = verify_chunk(&request.addr, &chunk);
        if chunk_valid {
            apply_credit(&accounting_peer, req_price, &refresh_chan).await;
            return RetrieveAttemptResult::Found(chunk, soc);
        }
    }

    cancel_reserve(&accounting_peer, req_price).await;
    if confirmed_empty {
        RetrieveAttemptResult::Empty
    } else {
        RetrieveAttemptResult::Failed(peer, connection_id)
    }
}

fn chunk_address_parts(chunk_address: &[u8]) -> (&[u8], &[u8], bool) {
    if chunk_address.len() == 64 {
        return (&chunk_address[..32], &chunk_address[32..], true);
    }

    (chunk_address, &[], false)
}

fn decode_retrieved_chunk(
    chunk: Bytes,
    soc: bool,
    encryption_key: &[u8],
    encrypted: bool,
) -> Bytes {
    if encrypted {
        let ciphertext = if soc {
            chunk.get(97..).unwrap_or_default()
        } else {
            &chunk
        };
        return decrypt(ciphertext, encryption_key).into();
    }

    if soc && chunk.len() >= 97 + 8 {
        return Bytes::copy_from_slice(&chunk[97..]);
    }
    chunk
}

#[derive(Clone, Debug)]
pub(crate) struct DecodedJoinChunk {
    pub level: RedundancyLevel,
    pub span: u64,
    pub payload: Bytes,
}

#[derive(Default)]
struct CachedJoinChunk {
    raw: Option<Bytes>,
    decoded: Option<DecodedJoinChunk>,
}

#[derive(Default)]
struct DecodedChunkCache {
    chunks: LinkedHashMap<Bytes, CachedJoinChunk, RandomState>,
}

impl DecodedChunkCache {
    fn get_decoded(
        &mut self,
        reference: &[u8],
        include_raw: bool,
    ) -> Option<(DecodedJoinChunk, Option<Bytes>)> {
        let entry = self.chunks.to_back(reference)?;
        if entry.decoded.is_none() {
            entry.decoded = entry
                .raw
                .as_ref()
                .and_then(|raw| decode_shared_raw_join_chunk(raw.clone(), reference));
        }
        let decoded = entry.decoded.clone();
        let raw = include_raw.then(|| entry.raw.clone()).flatten();
        Some((decoded?, raw))
    }

    fn get_raw(&mut self, reference: &[u8]) -> Option<Bytes> {
        self.chunks.to_back(reference)?.raw.clone()
    }

    fn insert(
        &mut self,
        reference: Vec<u8>,
        raw: Option<Bytes>,
        decoded: Option<DecodedJoinChunk>,
    ) {
        let cached = self
            .chunks
            .entry(Bytes::from(reference))
            .or_insert_with(CachedJoinChunk::default);
        cached.raw = cached.raw.take().or(raw);
        cached.decoded = decoded.or(cached.decoded.take());
        while self.chunks.len() > RETRIEVE_DECODED_CHUNK_CACHE_ENTRIES {
            self.chunks.pop_front();
        }
    }
}

thread_local! {
    static RETRIEVE_DECODED_CHUNK_CACHE: RefCell<DecodedChunkCache> =
        RefCell::new(DecodedChunkCache::default());
}

pub(crate) fn cached_decoded_chunk(reference: &[u8]) -> Option<DecodedJoinChunk> {
    RETRIEVE_DECODED_CHUNK_CACHE
        .with(|cache| cache.borrow_mut().get_decoded(reference, false))
        .map(|(decoded, _)| decoded)
}

fn cached_raw_chunk(reference: &[u8]) -> Option<Bytes> {
    RETRIEVE_DECODED_CHUNK_CACHE.with(|cache| cache.borrow_mut().get_raw(reference))
}

fn remember_decoded_chunk(reference: Vec<u8>, chunk: &DecodedJoinChunk) {
    RETRIEVE_DECODED_CHUNK_CACHE.with(|cache| {
        cache.borrow_mut().insert(reference, None, Some(chunk.clone()));
    });
}

fn remember_raw_chunk(reference: Vec<u8>, raw: Bytes) {
    RETRIEVE_DECODED_CHUNK_CACHE.with(|cache| {
        cache.borrow_mut().insert(reference, Some(raw), None);
    });
}

fn transfer_is_paused(transfer_paused: &Option<Arc<TransferPause>>) -> bool {
    transfer_paused.as_ref().is_some_and(transfer_pause_enabled)
}

fn chunk_retrieve_admission_current(
    cancel: &Option<RetrieveCancelToken>,
    admission: &Option<RetrieveAdmission>,
) -> bool {
    let stream_generation_current = retrieve_cancel_token_current(cancel);
    retrieve_admission_current(stream_generation_current, admission)
}

async fn wait_retrieve_closed(
    cancel: &Option<RetrieveCancelToken>,
    admission: &Option<RetrieveAdmission>,
) {
    let cancelled = async {
        match cancel {
            Some(cancel) => cancel.cancelled().await,
            None => std::future::pending().await,
        }
    };
    let closed = async {
        match admission {
            Some(admission) => admission.wait_closed().await,
            None => std::future::pending().await,
        }
    };
    pin_mut!(cancelled, closed);
    let _ = select(cancelled, closed).await;
}

async fn recv_raw_result_cancellable(
    receiver: &mpsc::Receiver<RawFetchResult>,
    cancel: &Option<RetrieveCancelToken>,
) -> Option<RawFetchResult> {
    let Some(cancel) = cancel else {
        return receiver.recv().await.ok();
    };
    if !cancel.is_current() {
        return None;
    }

    let result = receiver.recv();
    let cancelled = cancel.cancelled();
    pin_mut!(result, cancelled);
    match select(result, cancelled).await {
        Either::Left((result, _)) if cancel.is_current() => result.ok(),
        Either::Left(_) | Either::Right(_) => None,
    }
}

struct RawFetchResult {
    index: usize,
    chunk: Bytes,
    canonical_cac: bool,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct RawFetchKey {
    runtime_scope: usize,
    request_address: Bytes,
    expected_cac: Bytes,
    cancel_scope: Option<(Arc<str>, u64)>,
}

impl RawFetchKey {
    fn new(
        runtime_scope: usize,
        request_address: &[u8],
        expected_cac: &[u8],
        cancel: &Option<RetrieveCancelToken>,
    ) -> Self {
        let request_address = Bytes::copy_from_slice(request_address);
        let expected_cac = if request_address.as_ref() == expected_cac {
            request_address.clone()
        } else {
            Bytes::copy_from_slice(expected_cac)
        };
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
}

#[derive(Clone)]
struct RawFetchShared {
    admission: RetrieveAdmission,
    hedge_demand: Option<SharedRetrieveHedgeDemand>,
    cache_references: Rc<RefCell<Vec<Vec<u8>>>>,
}

impl RawFetchShared {
    fn new(hedge_demand: RetrieveHedgeDemand) -> Self {
        Self {
            admission: RetrieveAdmission::new_with_attempt_limit(usize::MAX),
            hedge_demand: (hedge_demand == RetrieveHedgeDemand::DistinctShardManaged)
                .then(|| SharedRetrieveHedgeDemand::new(hedge_demand)),
            cache_references: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn remember_cache_reference(&self, reference: Option<&[u8]>) {
        if let Some(reference) = reference {
            let mut references = self.cache_references.borrow_mut();
            if !references.iter().any(|known| known.as_slice() == reference) {
                references.push(reference.to_vec());
            }
        }
    }
}

type RawFetchFlights = SingleflightRegistry<RawFetchKey, RawFetchWaiter, RawFetchShared>;

thread_local! {
    static RAW_FETCH_FLIGHTS: RefCell<RawFetchFlights> =
        RefCell::new(RawFetchFlights::default());
}

fn remove_raw_fetch_waiter(key: &RawFetchKey, flight_id: u64, waiter_id: u64) {
    let shared = RAW_FETCH_FLIGHTS.with_borrow_mut(|flights| {
        flights.remove_waiter(key, flight_id, waiter_id)
    });
    if let Some(shared) = shared {
        // Keep the flight registered while dispatched accounting work drains.
        shared.admission.close();
        if shared.admission.claimed_physical_attempts() == Some(0) {
            RAW_FETCH_FLIGHTS.with_borrow_mut(|flights| flights.take(key, flight_id));
        }
    }
}

struct RawFetchQueue<'a> {
    chunks: &'a ChunkRetrieveSender,
    results: &'a mpsc::Sender<RawFetchResult>,
    cancel: &'a Option<RetrieveCancelToken>,
    waiters: Vec<(RawFetchKey, u64, u64)>,
}

impl<'a> RawFetchQueue<'a> {
    fn new(
        chunks: &'a ChunkRetrieveSender,
        results: &'a mpsc::Sender<RawFetchResult>,
        cancel: &'a Option<RetrieveCancelToken>,
    ) -> Self {
        Self {
            chunks,
            results,
            cancel,
            waiters: Vec::new(),
        }
    }

    fn close(&mut self) {
        for (key, flight_id, waiter_id) in self.waiters.drain(..) {
            remove_raw_fetch_waiter(&key, flight_id, waiter_id);
        }
    }

    fn queue_data_shard(
        &mut self,
        index: usize,
        reference: &[u8],
        hedge_demand: RetrieveHedgeDemand,
    ) {
        self.queue_drained_raw_chunk(
            index,
            &reference[..HASH_SIZE],
            &reference[..HASH_SIZE],
            Some(reference),
            hedge_demand,
        )
    }

    fn queue_root_batch(
        &mut self,
        requests: &[[u8; HASH_SIZE]],
        expected_cac: &[u8],
        cache_reference: &[u8],
        next: &mut usize,
        batch: usize,
    ) {
        let end = next.saturating_add(batch).min(requests.len());
        while *next < end {
            self.queue_drained_raw_chunk(
                *next,
                &requests[*next],
                expected_cac,
                Some(cache_reference),
                RetrieveHedgeDemand::Ordinary,
            );
            *next += 1;
        }
    }

    fn queue_drained_raw_chunk(
        &mut self,
        index: usize,
        request_address: &[u8],
        expected_cac: &[u8],
        cache_reference: Option<&[u8]>,
        hedge_demand: RetrieveHedgeDemand,
    ) {
        if let Some(reference) = cache_reference
            && let Some(chunk) = cached_raw_chunk(reference)
        {
            let _ = self.results.try_send(RawFetchResult {
                index,
                chunk,
                canonical_cac: true,
            });
            return;
        }

        let key = RawFetchKey::new(
            self.chunks.runtime_scope(),
            request_address,
            expected_cac,
            self.cancel,
        );
        let registration = RAW_FETCH_FLIGHTS.with_borrow_mut(|flights| {
            flights.register(
                key,
                RawFetchWaiter {
                    index,
                    result_chan: self.results.clone(),
                },
                || RawFetchShared::new(hedge_demand),
            )
        });
        registration
            .shared
            .remember_cache_reference(cache_reference);
        if let Some(shared_demand) = registration.shared.hedge_demand.as_ref() {
            shared_demand.promote(hedge_demand);
        }

        let flight_id = registration.flight_id;
        self.waiters
            .push((registration.key.clone(), flight_id, registration.waiter_id));

        if !registration.leader {
            return;
        }

        let completion_key = registration.key;
        let _ = self.chunks.try_send(crate::ChunkRetrieveRequest {
            address: completion_key.request_address.to_vec(),
            chan: crate::ChunkRetrieveReply::Raw(RawFetchCompletion {
                key: completion_key,
                flight_id,
            }),
            cancel: self.cancel.clone(),
            admission: Some(registration.shared.admission.clone()),
            hedge_demand: registration.shared.hedge_demand.clone(),
        });
    }
}

impl Drop for RawFetchQueue<'_> {
    fn drop(&mut self) {
        self.close();
    }
}

// The detached dispatcher owns completion, including a dropped queued request.
pub(crate) struct RawFetchCompletion {
    key: RawFetchKey,
    flight_id: u64,
}

impl RawFetchCompletion {
    pub(crate) fn send(mut self, chunk: Bytes) {
        let flight_id = std::mem::replace(&mut self.flight_id, 0);
        complete_raw_fetch(&self.key, flight_id, chunk);
    }
}

impl Drop for RawFetchCompletion {
    fn drop(&mut self) {
        if self.flight_id != 0 {
            complete_raw_fetch(&self.key, self.flight_id, Bytes::new());
        }
    }
}

fn complete_raw_fetch(key: &RawFetchKey, flight_id: u64, chunk: Bytes) -> bool {
    let flight = RAW_FETCH_FLIGHTS.with_borrow_mut(|flights| flights.take(key, flight_id));
    let Some(flight) = flight else {
        return false;
    };
    flight.shared.admission.close();

    let usable = (erasure_coding::SPAN_SIZE..=CHUNK_WITH_SPAN_SIZE).contains(&chunk.len());
    let canonical_cac = usable
        && ((key.request_address.len() == HASH_SIZE
            && key.request_address == key.expected_cac
            && flight.shared.admission.returned_cac())
            || valid_cac(&chunk, &key.expected_cac));
    let delivered = if usable { chunk } else { Bytes::new() };

    // Cache ownership belongs to the physical flight, not to its current
    // logical waiters. A retired caller may have zero waiters when a canonical
    // late result arrives, and that result must still benefit a later caller.
    if canonical_cac {
        for reference in flight.shared.cache_references.borrow_mut().drain(..) {
            remember_raw_chunk(reference, delivered.clone());
        }
    }

    for (_, waiter) in flight.waiters {
        let _ = waiter.result_chan.try_send(RawFetchResult {
            index: waiter.index,
            chunk: delivered.clone(),
            canonical_cac,
        });
    }
    canonical_cac
}

fn decrypt_join_chunk(raw: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    if key.len() != HASH_SIZE {
        return None;
    }

    let mut span = *raw.first_chunk::<{ erasure_coding::SPAN_SIZE }>()?;
    let span_key = encryption_segment_key(key, (CHUNK_SIZE / key.len()) as u32);
    for (byte, mask) in span.iter_mut().zip(span_key) {
        *byte ^= mask;
    }
    let length = match u64::from_le_bytes(span) {
        length if length <= CHUNK_SIZE as u64 => raw.len().min(span.len() + length as usize),
        _ => raw.len(),
    };
    let mut plain = raw[..length].to_vec();
    plain[..span.len()].copy_from_slice(&span);

    for (segment_index, segment) in plain[erasure_coding::SPAN_SIZE..]
        .chunks_mut(key.len())
        .enumerate()
    {
        let segment_key = encryption_segment_key(key, segment_index as u32);
        for (byte, mask) in segment.iter_mut().zip(segment_key) {
            *byte ^= mask;
        }
    }
    Some(plain)
}

fn canonical_plain_chunk(plain: Bytes, encrypted: bool) -> Option<DecodedJoinChunk> {
    let (level, span) = erasure_coding::decode_span(&plain)?;
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
        payload: plain.slice(erasure_coding::SPAN_SIZE..chunk_len),
    })
}

fn decode_shared_raw_join_chunk(raw: Bytes, reference: &[u8]) -> Option<DecodedJoinChunk> {
    if reference.len() != HASH_SIZE && reference.len() != erasure_coding::ENCRYPTED_REFERENCE_SIZE {
        return None;
    }
    if !(erasure_coding::SPAN_SIZE..=CHUNK_WITH_SPAN_SIZE).contains(&raw.len()) {
        return None;
    }

    let encrypted = reference.len() == erasure_coding::ENCRYPTED_REFERENCE_SIZE;
    if encrypted {
        let plain = decrypt_join_chunk(&raw, &reference[HASH_SIZE..])?;
        canonical_plain_chunk(Bytes::from(plain), true)
    } else {
        canonical_plain_chunk(raw, false)
    }
}

async fn retrieve_raw_root_cancellable(
    reference: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: &Option<RetrieveCancelToken>,
) -> Option<(Bytes, bool)> {
    let root_cac: [u8; HASH_SIZE] = reference.get(..HASH_SIZE)?.try_into().ok()?;
    let replicas = erasure_coding::replicas(
        &root_cac,
        RedundancyLevel::DEFAULT_DOWNLOAD,
        bee_replica_address,
    )?;
    let mut requests = Vec::with_capacity(1 + replicas.len());
    requests.push(root_cac);
    requests.extend(replicas.into_iter().map(|replica| replica.address));

    let (result_out, result_in) = mpsc::unbounded::<RawFetchResult>();
    let mut raw_fetches = RawFetchQueue::new(chunk_retrieve_chan, &result_out, cancel);
    let mut next = 0usize;
    let mut completed = 0usize;
    let mut next_batch = 2usize;

    let initial = requests.len().min(3); // original CAC plus Bee's first two replicas
    if !retrieve_cancel_token_current(cancel) {
        return None;
    }
    raw_fetches.queue_root_batch(
        &requests,
        &root_cac,
        reference,
        &mut next,
        initial,
    );
    let mut hedge_started = Date::now();
    let mut hedge_due = false;

    loop {
        if !retrieve_cancel_token_current(cancel) {
            return None;
        }
        if completed == next || hedge_due {
            if next == requests.len() {
                return None;
            }
            raw_fetches.queue_root_batch(
                &requests,
                &root_cac,
                reference,
                &mut next,
                next_batch,
            );
            next_batch = next_batch.saturating_mul(2);
            hedge_started = Date::now();
            hedge_due = false;
        }

        let result = if next < requests.len() {
            let elapsed = (Date::now() - hedge_started).max(0.0) as u64;
            let remaining = RETRIEVE_HEDGE_AFTER_MS.saturating_sub(elapsed).max(1);
            match async_std::future::timeout(Duration::from_millis(remaining), result_in.recv())
                .await
            {
                Ok(result) => result.ok(),
                Err(_) => {
                    hedge_due = true;
                    continue;
                }
            }
        } else {
            recv_raw_result_cancellable(&result_in, cancel).await
        };

        let result = result?;
        completed += 1;
        let accepted = !result.chunk.is_empty() && (result.index == 0 || result.canonical_cac);
        if accepted {
            raw_fetches.close();
        }
        if !retrieve_cancel_token_current(cancel) {
            return None;
        }
        if accepted {
            return Some((result.chunk, result.canonical_cac));
        }
    }
}

pub(crate) async fn retrieve_decoded_data_root(
    data_address: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
) -> Option<DecodedJoinChunk> {
    if data_address.len() != HASH_SIZE
        && data_address.len() != erasure_coding::ENCRYPTED_REFERENCE_SIZE
    {
        return None;
    }
    if !retrieve_cancel_token_current(&cancel) {
        return None;
    }
    if let Some(root) = cached_decoded_chunk(data_address) {
        return Some(root);
    }
    let (raw, canonical_cac) =
        retrieve_raw_root_cancellable(data_address, chunk_retrieve_chan, &cancel).await?;
    if !retrieve_cancel_token_current(&cancel) {
        return None;
    }
    if canonical_cac && let Some(root) = cached_decoded_chunk(data_address) {
        return Some(root);
    }
    let root = decode_shared_raw_join_chunk(raw, data_address)?;
    if canonical_cac {
        remember_decoded_chunk(data_address.to_vec(), &root);
    }
    Some(root)
}

fn dispatch_group_shards(
    data_references: ChunksExact<'_, u8>,
    parity_references: ChunksExact<'_, u8>,
    dispatched_shards: &mut [bool],
    raw_fetches: &mut RawFetchQueue<'_>,
    start_index: usize,
    limit: usize,
    hedge_demand: RetrieveHedgeDemand,
) -> usize {
    let data_count = data_references.len();
    let mut count = 0usize;
    for (index, reference) in data_references
        .chain(parity_references)
        .enumerate()
        .skip(start_index)
    {
        if count >= limit {
            break;
        }
        if dispatched_shards[index] {
            continue;
        }
        if index < data_count {
            raw_fetches.queue_data_shard(index, reference, hedge_demand);
        } else {
            raw_fetches.queue_drained_raw_chunk(index, reference, reference, None, hedge_demand);
        }
        dispatched_shards[index] = true;
        count += 1;
    }
    count
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

async fn fetch_data_group_indices_streaming(
    payload: Bytes,
    layout: ReferenceLayout,
    encrypted: bool,
    requested_indices: Range<usize>,
    chunk_retrieve_chan: ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
    child_emitter: GroupChildEmitter,
) -> Option<()> {
    let (data_references, parity_references) = split_references(&payload, layout, encrypted)?;
    let data_count = data_references.len();
    let parity_count = parity_references.len();
    let total_count = data_count.checked_add(parity_count)?;
    if data_count == 0 || total_count > 256 {
        return None;
    }
    if !retrieve_cancel_token_current(&cancel) {
        return None;
    }

    let requested_count = requested_indices.len();

    let (result_out, result_in) = mpsc::unbounded::<RawFetchResult>();
    let mut raw_fetches = RawFetchQueue::new(&chunk_retrieve_chan, &result_out, &cancel);
    let mut dispatched_shards = vec![false; total_count];
    let mut requested_ready = vec![false; data_count];
    let mut received_shards: Vec<Option<Bytes>> = vec![None; total_count];
    let static_rolling_candidate =
        rolling_full_group_static_candidate(requested_count, data_count, parity_count);
    let mut cached_requested = Vec::new();
    let mut decoded_only_count = 0usize;
    let mut unresolved_count = 0usize;
    if static_rolling_candidate {
        cached_requested.reserve(requested_count);
        for index in requested_indices.clone() {
            let reference = data_references.clone().nth(index)?;
            let cached = RETRIEVE_DECODED_CHUNK_CACHE
                .with_borrow_mut(|cache| cache.get_decoded(reference, true));
            match &cached {
                Some((_, Some(_))) => {}
                Some((_, None)) => decoded_only_count += 1,
                None => unresolved_count += 1,
            }
            cached_requested.push(cached);
        }
    }
    let rolling = static_rolling_candidate
        && rolling_full_group_eligible(
            requested_count,
            data_count,
            parity_count,
            decoded_only_count,
            unresolved_count,
        );
    let initial_hedge_demand = if rolling {
        RetrieveHedgeDemand::DistinctShardManaged
    } else {
        RetrieveHedgeDemand::Ordinary
    };
    let mut successes = 0usize;
    let mut dispatched = 0usize;
    let mut cached_requested = cached_requested.into_iter();
    for index in requested_indices.clone() {
        let reference = data_references.clone().nth(index)?;
        let cached = if static_rolling_candidate {
            cached_requested.next().flatten()
        } else {
            cached_decoded_chunk(reference).map(|decoded| (decoded, None))
        };
        if let Some((decoded, raw)) = cached {
            child_emitter.emit(index, decoded);
            requested_ready[index] = true;
            if rolling && let Some(raw) = raw {
                received_shards[index] = Some(raw);
                dispatched_shards[index] = true;
                successes += 1;
            }
            continue;
        }
        raw_fetches.queue_data_shard(index, reference, initial_hedge_demand);
        dispatched_shards[index] = true;
        dispatched += 1;
    }
    drop(cached_requested);
    // Anchor both hedge policies after all initial registrations. This preserves the legacy
    // deadline and gives the rolling set a full hedge interval before parity can replace it.
    let started = Date::now();
    let hedge_after = if rolling {
        RETRIEVE_HEDGE_AFTER_MS
    } else {
        RETRIEVE_RS_HEDGE_AFTER_MS
    };

    let mut completed = 0usize;
    let mut recovery_dispatched = false;
    let mut next_result: Option<RawFetchResult> = None;

    loop {
        if !retrieve_cancel_token_current(&cancel) {
            return None;
        }
        while let Some(result) = next_result.take().or_else(|| {
            if rolling {
                result_in.try_recv().ok()
            } else {
                None
            }
        }) {
            completed = completed.checked_add(1)?;
            if result.chunk.is_empty() || (!result.canonical_cac && parity_count > 0) {
                if !rolling && !recovery_dispatched {
                    if parity_count == 0 {
                        return None;
                    }
                    recovery_dispatched = true;
                }
                continue;
            }

            let index = result.index;
            let raw = received_shards.get_mut(index)?.insert(result.chunk);
            successes = successes.checked_add(1)?;
            if index < data_count && requested_indices.contains(&index) && !requested_ready[index] {
                let reference = data_references.clone().nth(index)?;
                let chunk = if result.canonical_cac {
                    cached_decoded_chunk(reference).or_else(|| {
                        remember_raw_chunk(reference.to_vec(), raw.clone());
                        cached_decoded_chunk(reference)
                    })?
                } else {
                    decode_shared_raw_join_chunk(raw.clone(), reference)?
                };
                child_emitter.emit(index, chunk);
                requested_ready[index] = true;
            }
        }

        // Re-evaluate terminal state before any replacement admission.
        let all_requested_ready = requested_indices
            .clone()
            .all(|index| requested_ready[index]);
        let terminal = all_requested_ready || (recovery_dispatched && successes >= data_count);
        if terminal {
            raw_fetches.close();
            if !retrieve_cancel_token_current(&cancel) {
                return None;
            }
            break;
        }
        let hedge_due = rolling && (Date::now() - started).max(0.0) as u64 >= hedge_after;
        if hedge_due {
            let active = dispatched.checked_sub(completed)?;
            let queued = dispatch_group_shards(
                data_references.clone(),
                parity_references.clone(),
                &mut dispatched_shards,
                &mut raw_fetches,
                data_count,
                data_count.checked_sub(active)?,
                RetrieveHedgeDemand::DistinctShardManaged,
            );
            dispatched += queued;
            recovery_dispatched |= queued > 0;
        } else if !rolling && recovery_dispatched {
            let top_up = recovery_top_up_count(data_count, successes, dispatched, completed);
            dispatched += dispatch_group_shards(
                data_references.clone(),
                parity_references.clone(),
                &mut dispatched_shards,
                &mut raw_fetches,
                0,
                top_up,
                RetrieveHedgeDemand::Ordinary,
            );
        }

        if completed == dispatched && (!rolling || hedge_due) {
            return None;
        }

        let waiting_for_hedge = if rolling {
            !hedge_due
        } else {
            !recovery_dispatched
        };
        let wait_ms = if waiting_for_hedge && parity_count > 0 {
            let elapsed = (Date::now() - started).max(0.0) as u64;
            Some(hedge_after.saturating_sub(elapsed).max(1))
        } else if !rolling && recovery_dispatched && dispatched < total_count {
            Some(RETRIEVE_RS_HEDGE_AFTER_MS)
        } else {
            None
        };
        let received = match wait_ms {
            Some(milliseconds) => {
                async_std::future::timeout(Duration::from_millis(milliseconds), result_in.recv())
                    .await
                    .map(Result::ok)
            }
            None => Ok(recv_raw_result_cancellable(&result_in, &cancel).await),
        };
        match received {
            Ok(result) => next_result = Some(result?),
            Err(_) => {
                if !retrieve_cancel_token_current(&cancel) {
                    return None;
                }
                if !rolling {
                    if waiting_for_hedge {
                        if requested_count == data_count {
                            dispatched += dispatch_group_shards(
                                data_references.clone(),
                                parity_references.clone(),
                                &mut dispatched_shards,
                                &mut raw_fetches,
                                data_count,
                                usize::MAX,
                                RetrieveHedgeDemand::Ordinary,
                            );
                        }
                        recovery_dispatched = true;
                    } else {
                        dispatched += dispatch_group_shards(
                            data_references.clone(),
                            parity_references.clone(),
                            &mut dispatched_shards,
                            &mut raw_fetches,
                            0,
                            RETRIEVE_RECOVERY_PROGRESSIVE_BATCH,
                            RetrieveHedgeDemand::Ordinary,
                        );
                    }
                }
            }
        }
    }

    let missing_indices = requested_indices
        .filter(|&index| !requested_ready[index])
        .collect::<Vec<_>>();
    if missing_indices.is_empty() {
        return Some(());
    }
    reconstruct_data_indices(
        &mut received_shards,
        data_count,
        &missing_indices,
        CHUNK_WITH_SPAN_SIZE,
    )
    .ok()?;

    for index in missing_indices {
        let reference = data_references.clone().nth(index)?;
        let raw = received_shards[index].take()?;
        if !valid_cac(&raw, &reference[..HASH_SIZE]) {
            return None;
        }
        remember_raw_chunk(reference.to_vec(), raw);
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

struct GroupChildEmitter {
    context: GroupTraversalContext,
    events: mpsc::Sender<(GroupTraversalContext, usize, DecodedJoinChunk)>,
}

impl GroupChildEmitter {
    fn emit(&self, index: usize, chunk: DecodedJoinChunk) {
        let _ = self.events.try_send((self.context, index, chunk));
    }
}

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
    output_prefix: &[u8],
    cancel: Option<RetrieveCancelToken>,
) -> Option<Vec<u8>> {
    if !retrieve_cancel_token_current(&cancel) {
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
    let mut groups = FuturesUnordered::new();
    let (group_event_out, group_event_in) = mpsc::unbounded();

    loop {
        if !retrieve_cancel_token_current(&cancel) {
            return None;
        }
        while groups.len() < RETRIEVE_DATA_GROUP_CONCURRENCY {
            let Some(node) = pending.pop_front() else {
                break;
            };
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
            let requested_indices = first_index..last_index + 1;
            let sender = chunk_retrieve_chan.clone();
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
            groups.push(fetch_data_group_indices_streaming(
                node.chunk.payload,
                layout,
                encrypted,
                requested_indices,
                sender,
                group_cancel,
                emitter,
            ));
        }

        // A completed group may still have children waiting to be traversed.
        if groups.is_empty() && group_event_in.is_empty() {
            break;
        }

        let (context, index, chunk) = if groups.is_empty() {
            group_event_in.recv().await.ok()?
        } else {
            let next_event = group_event_in.recv();
            let next_completion = groups.next();
            pin_mut!(next_event, next_completion);
            match select(next_event, next_completion).await {
                Either::Left((event, _)) => event.ok()?,
                Either::Right((completion, _)) => {
                    completion??;
                    continue;
                }
            }
        };
        if !retrieve_cancel_token_current(&cancel) {
            return None;
        }

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

    (written == requested_len).then_some(output)
}

async fn retrieve_data_joined(
    data_address: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
    include_span_prefix: bool,
) -> Vec<u8> {
    let encrypted = data_address.len() == erasure_coding::ENCRYPTED_REFERENCE_SIZE;
    let Some(root) = retrieve_decoded_data_root(data_address, chunk_retrieve_chan, None).await else {
        return vec![];
    };
    let span = root.span;
    let span_prefix = span.to_le_bytes();
    let output_prefix: &[u8] = if include_span_prefix {
        &span_prefix
    } else {
        &[]
    };
    if span == 0 {
        output_prefix.to_vec()
    } else {
        let Some(payload) = retrieve_data_range_from_root(
            root,
            0,
            span - 1,
            encrypted,
            chunk_retrieve_chan,
            output_prefix,
            None,
        )
        .await
        else {
            return vec![];
        };
        payload
    }
}

/// Retrieve Bee's historical joined representation (`span || payload`).
pub async fn retrieve_data(
    data_address: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Vec<u8> {
    retrieve_data_joined(data_address, chunk_retrieve_chan, true).await
}

/// Retrieve a Bee bytes payload without its internal span.
pub(crate) async fn retrieve_data_payload(
    data_address: &[u8],
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Vec<u8> {
    retrieve_data_joined(data_address, chunk_retrieve_chan, false).await
}

pub async fn retrieve_chunk(
    chunk_address: &[u8],
    control: StreamControl,
    peers: &OverlayPeerMap,
    accounting: &PeerAccountingMap,
    physical_connections: &PhysicalConnectionMap,
    refresh_chan: &mpsc::Sender<RefreshmentInstruction>,
    cancel: Option<RetrieveCancelToken>,
    admission: Option<RetrieveAdmission>,
    hedge_demand: Option<SharedRetrieveHedgeDemand>,
    transfer_paused: Option<Arc<TransferPause>>,
) -> Bytes {
    let (caddr, encryption_key, encrypted) = chunk_address_parts(chunk_address);

    let mut skiplist = HashMap::new();

    let mut attempt_count = 0;

    let mut retrieved = None;

    let (attempt_out, attempt_in) = mpsc::unbounded::<RetrieveAttemptResult>();
    let mut in_flight = 0_usize;
    let mut last_attempt_started = 0.0;
    let mut credit_changed = None;

    while attempt_count < RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS || in_flight > 0 {
        if in_flight > 0
            && let Ok(result) = attempt_in.try_recv()
        {
            in_flight = in_flight.saturating_sub(1);
            if let Some(success) = result.complete(&mut skiplist) {
                retrieved = Some(success);
                break;
            }

            continue;
        }

        let paused = transfer_is_paused(&transfer_paused);
        let admission_current = chunk_retrieve_admission_current(&cancel, &admission);
        if !admission_current && in_flight == 0 {
            break;
        }
        let cancelled = !admission_current;

        if paused && in_flight == 0 {
            let resumed = wait_transfer_unpaused(
                transfer_paused.as_ref().expect("paused transfer exists"),
            );
            let cancelled = wait_retrieve_closed(&cancel, &admission);
            pin_mut!(resumed, cancelled);
            if !matches!(select(resumed, cancelled).await, Either::Left(((), _))) {
                break;
            }
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
            let (selected, retry) = select_retrieve_peer(
                caddr,
                peers,
                accounting,
                physical_connections,
                &mut skiplist,
            )
            .await;
            if let Some(selected) = selected {
                credit_changed = None;
                let cancelled = !chunk_retrieve_admission_current(&cancel, &admission);
                let paused = !cancelled && transfer_is_paused(&transfer_paused);
                if cancelled
                    || paused
                    || admission
                        .as_ref()
                        .is_some_and(|admission| !admission.try_claim_physical_attempt())
                {
                    cancel_reserve(&selected.accounting, selected.price).await;
                    skiplist.remove(&selected.peer);
                    if !paused && in_flight == 0 {
                        break;
                    }
                    continue;
                }

                let control = control.clone();
                let refresh_chan = refresh_chan.clone();
                let attempt_out = attempt_out.clone();
                let caddr = caddr.to_vec();
                let attempt_admission = admission.clone();
                let attempt_hedge = hedge_demand.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let result = retrieve_attempt(
                        selected,
                        caddr,
                        control,
                        refresh_chan,
                        attempt_admission,
                        attempt_hedge,
                    )
                    .await;
                    let _ = attempt_out.try_send(result);
                });
                attempt_count += 1;
                in_flight += 1;
                last_attempt_started = Date::now();
            } else {
                if !retry && in_flight == 0 && !skiplist.is_empty() {
                    break;
                }
                if credit_changed.is_none() {
                    credit_changed = Some(crate::CREDIT_AVAILABLE.listen());
                    continue; // Recheck after subscribing, without allocating on successful selection.
                }
            }
        }

        if in_flight == 0 && credit_changed.is_none() {
            break;
        }

        let wake = async {
            if let Some(changed) = credit_changed.take() {
                let closed = wait_retrieve_closed(&cancel, &admission);
                pin_mut!(changed, closed);
                let _ = select(changed, closed).await;
            } else if current_hedge_demand == RetrieveHedgeDemand::DistinctShardManaged {
                hedge_demand
                    .as_ref()
                    .expect("managed demand is always shared")
                    .wait_until_ordinary()
                    .await;
            } else if !can_start_attempt || cancelled {
                std::future::pending::<()>().await;
            } else if paused {
                wait_transfer_unpaused(transfer_paused.as_ref().expect("paused transfer exists"))
                    .await;
            } else {
                let elapsed = Date::now() - last_attempt_started;
                let wait_ms = (RETRIEVE_HEDGE_AFTER_MS as f64 - elapsed).max(0.0).ceil() as u64;
                async_std::task::sleep(Duration::from_millis(wait_ms)).await;
            }
        };
        let result = attempt_in.recv();
        pin_mut!(result, wake);
        let result = match select(result, wake).await {
            Either::Left((result, _)) => result.ok(),
            Either::Right(_) => continue,
        };
        let Some(result) = result else { break };
        in_flight = in_flight.saturating_sub(1);
        if let Some(success) = result.complete(&mut skiplist) {
            retrieved = Some(success);
            break;
        }
    }

    let Some((chunk, soc)) = retrieved else {
        return Bytes::new();
    };
    if !soc && let Some(admission) = admission {
        admission.record_returned_cac();
    }
    decode_retrieved_chunk(chunk, soc, encryption_key, encrypted)
}

pub async fn retrieve_check_chunk(
    chunk_address: &[u8],
    control: StreamControl,
    peers: &OverlayPeerMap,
    accounting: &PeerAccountingMap,
    physical_connections: &PhysicalConnectionMap,
    refresh_chan: &mpsc::Sender<RefreshmentInstruction>,
    transfer_paused: Option<Arc<TransferPause>>,
) -> Vec<u8> {
    let (caddr, encryption_key, encrypted) = chunk_address_parts(chunk_address);

    let mut skiplist = HashMap::new();
    let mut successes = 0;
    let mut error_count = 0;
    let max_error = 21 - RETRIEVE_CHECK_CONFIRMATION_PEERS;

    let mut retrieved = None;
    let mut credit_changed = None;

    while error_count < max_error && successes < RETRIEVE_CHECK_CONFIRMATION_PEERS {
        if let Some(paused) = &transfer_paused {
            wait_transfer_unpaused(paused).await;
        }

        let (Some(selected), _) = select_retrieve_peer(
            caddr,
            peers,
            accounting,
            physical_connections,
            &mut skiplist,
        )
        .await
        else {
            if let Some(changed) = credit_changed.take() {
                changed.await;
            }
            credit_changed = Some(crate::CREDIT_AVAILABLE.listen());
            continue;
        };
        credit_changed = None;

        if transfer_is_paused(&transfer_paused) {
            cancel_reserve(&selected.accounting, selected.price).await;
            continue;
        }

        let result = retrieve_attempt(
            selected,
            caddr.to_vec(),
            control.clone(),
            refresh_chan.clone(),
            None,
            None,
        )
        .await;
        if let Some(result) = result.complete(&mut skiplist) {
            successes += 1;
            retrieved.get_or_insert(result);
        } else {
            error_count += 1;
        }
    }

    if successes < RETRIEVE_CHECK_CONFIRMATION_PEERS {
        return vec![];
    }

    let Some((chunk, soc)) = retrieved else {
        return vec![];
    };
    decode_retrieved_chunk(chunk, soc, encryption_key, encrypted).into()
}

pub fn verify_chunk(caddr: &[u8], cd: &[u8]) -> (bool, bool) {
    if valid_cac(cd, caddr) {
        (true, false)
    } else {
        let soc = valid_soc(cd, caddr);
        (soc, soc)
    }
}

pub fn decrypt(chunk: &[u8], encryption_key: &[u8]) -> Vec<u8> {
    let Some(mut decrypted) = decrypt_join_chunk(chunk, encryption_key) else {
        return vec![];
    };
    let mut span_decrypted = u64::from_le_bytes(
        decrypted[..erasure_coding::SPAN_SIZE]
            .try_into()
            .expect("decrypt_join_chunk validated the span"),
    );

    if span_decrypted > CHUNK_SIZE as u64 {
        let mut carry_span = CHUNK_SIZE as u64;
        loop {
            let carried = span_decrypted.div_ceil(carry_span);
            if carried <= 64 {
                span_decrypted = carried * 64;
                break;
            }
            carry_span *= 64;
        }
    }

    let decrypted_len = erasure_coding::SPAN_SIZE + span_decrypted as usize;
    if decrypted_len > decrypted.len() {
        return vec![];
    }
    decrypted.truncate(decrypted_len);
    decrypted
}

async fn get_feed_probe_chunk(
    data_address: Vec<u8>,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> FeedProbe<Vec<u8>> {
    let admission = RetrieveAdmission::new_with_attempt_limit(usize::MAX);
    let _close_admission = admission.close_on_drop();
    let (chan_out, chan_in) = oneshot::channel();
    if chunk_retrieve_chan
        .try_send(crate::ChunkRetrieveRequest {
            address: data_address,
            chan: crate::ChunkRetrieveReply::Channel(chan_out),
            cancel: None,
            admission: Some(admission.clone()),
            hedge_demand: None,
        })
        .is_err()
    {
        return FeedProbe::Transient;
    }

    match chan_in.await {
        Ok(payload) if !payload.is_empty() => FeedProbe::Found(payload),
        Ok(_) if admission.claimed_physical_attempts().is_some_and(|attempts| {
                attempts > 0 && admission.confirmed_empty_physical_attempts() == Some(attempts)
            }) => FeedProbe::Missing,
        Ok(_) | Err(_) => FeedProbe::Transient,
    }
}

async fn probe_feed_update_status(
    owner: &str,
    topic: &str,
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
) -> Result<(Option<(u64, Vec<u8>)>, Option<u64>), ()> {
    seek_sequence_feed_frontier(|index| {
        probe_feed_update_status(&owner, &topic, index, chunk_retrieve_chan)
    })
    .await
}

pub async fn seek_latest_feed_update(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Result<Option<Vec<u8>>, ()> {
    let (latest, _) = seek_feed_frontier(owner, topic, chunk_retrieve_chan).await?;
    Ok(latest.map(|(_, payload)| payload))
}

pub async fn seek_next_feed_update_index(
    owner: String,
    topic: String,
    chunk_retrieve_chan: &ChunkRetrieveSender,
) -> Result<u64, ()> {
    seek_feed_frontier(owner, topic, chunk_retrieve_chan)
        .await?.1.ok_or(())
}

#[cfg(test)]
mod decrypt_tests {
    use super::*;

    fn encrypted_chunk(span: u64, content: &[u8], key: &[u8; HASH_SIZE], pad: bool) -> Vec<u8> {
        let mut chunk = span.to_le_bytes().to_vec();
        chunk.extend_from_slice(content);
        if pad {
            chunk.resize(CHUNK_WITH_SPAN_SIZE, 0xa5);
        }

        let span_key = encryption_segment_key(key, (CHUNK_SIZE / HASH_SIZE) as u32);
        for (byte, mask) in chunk[..erasure_coding::SPAN_SIZE].iter_mut().zip(span_key) {
            *byte ^= mask;
        }
        for (counter, segment) in chunk[erasure_coding::SPAN_SIZE..]
            .chunks_mut(HASH_SIZE)
            .enumerate()
        {
            let segment_key = encryption_segment_key(key, counter as u32);
            for (byte, mask) in segment.iter_mut().zip(segment_key) {
                *byte ^= mask;
            }
        }
        chunk
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn encrypted_cac_and_soc_payloads_have_identical_canonical_output() {
        let key = [0x5a; HASH_SIZE];
        let pattern = b"retrieved payload";
        let plain_reference = vec![0x11; HASH_SIZE];
        let (address, plain_key, encrypted) = chunk_address_parts(&plain_reference);
        assert_eq!(address, plain_reference);
        assert!(plain_key.is_empty());
        assert!(!encrypted);

        let mut encrypted_reference = vec![0x11; HASH_SIZE];
        encrypted_reference.extend_from_slice(&key);
        let (address, extracted_key, encrypted) = chunk_address_parts(&encrypted_reference);
        assert_eq!(address, &encrypted_reference[..HASH_SIZE]);
        assert_eq!(extracted_key, key);
        assert!(encrypted);
        for length in [0, 1, 17, 31, 32, 33, 64, 135, 136, 137, 4095, 4096] {
            let payload: Vec<_> = pattern.iter().copied().cycle().take(length).collect();
            let ciphertext: Bytes = encrypted_chunk(length as u64, &payload, &key, true).into();
            let mut expected = (length as u64).to_le_bytes().to_vec();
            expected.extend_from_slice(&payload);
            let canonical =
                decode_shared_raw_join_chunk(ciphertext.clone(), &encrypted_reference).unwrap();
            assert_eq!(canonical.level, RedundancyLevel::None);
            assert_eq!(canonical.span, length as u64);
            assert_eq!(canonical.payload.as_ref(), payload.as_slice());

            let mut soc = vec![0x33; 97];
            soc.extend_from_slice(&ciphertext);
            for (chunk, soc) in [(ciphertext.clone(), false), (soc.into(), true)] {
                assert_eq!(decode_retrieved_chunk(chunk, soc, extracted_key, true), expected);
            }
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn malformed_or_short_ciphertext_returns_empty_without_panicking() {
        let key = [0x2c; HASH_SIZE];
        assert!(decrypt(&[], &key).is_empty());
        assert!(decrypt(&[0; erasure_coding::SPAN_SIZE - 1], &key).is_empty());
        assert!(decrypt(&[0; erasure_coding::SPAN_SIZE], &key[..HASH_SIZE - 1]).is_empty());

        let undersized = encrypted_chunk(2, b"x", &key, false);
        assert!(decrypt(&undersized, &key).is_empty());
        assert!(
            decode_retrieved_chunk(
                vec![0; 97 + erasure_coding::SPAN_SIZE - 1].into(),
                true,
                &key,
                true
            )
            .is_empty()
        );

        let empty = encrypted_chunk(0, &[], &key, false);
        assert_eq!(decrypt(&empty, &key), 0_u64.to_le_bytes());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn multilevel_spans_keep_the_span_and_normalize_reference_payload_width() {
        let key = [0xb7; HASH_SIZE];
        let content = (0..CHUNK_SIZE)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();

        for (span, expected_payload_len) in [
            (CHUNK_SIZE as u64 + 1, 2 * 64),
            (CHUNK_SIZE as u64 * 64, CHUNK_SIZE),
            (CHUNK_SIZE as u64 * 64 + 1, 2 * 64),
        ] {
            let ciphertext = encrypted_chunk(span, &content, &key, false);
            let decrypted = decrypt(&ciphertext, &key);
            assert_eq!(&decrypted[..erasure_coding::SPAN_SIZE], &span.to_le_bytes());
            assert_eq!(
                &decrypted[erasure_coding::SPAN_SIZE..],
                &content[..expected_payload_len]
            );
        }
    }
}

#[cfg(test)]
mod raw_fetch_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn plain_chunk(payload: &[u8]) -> Bytes {
        let mut raw = (payload.len() as u64).to_le_bytes().to_vec();
        raw.extend_from_slice(payload);
        raw.into()
    }

    #[wasm_bindgen_test]
    async fn root_batches_preserve_order_exhaustion_hedging_and_replica_success() {
        let raw = plain_chunk(b"root batch ordering and canonical replica");
        let address = crate::content_address(&raw);
        let replicas = erasure_coding::replicas(
            &address,
            RedundancyLevel::DEFAULT_DOWNLOAD,
            bee_replica_address,
        )
        .unwrap();
        let plan = std::iter::once(address.as_slice())
            .chain(replicas.iter().map(|replica| replica.address.as_slice()))
            .collect::<Vec<_>>();
        let (chunks, requests) = crate::chunk_retrieve_channel();
        let cancel = None;
        let root = retrieve_raw_root_cancellable(&address, &chunks, &cancel);
        pin_mut!(root);
        assert!(futures::poll!(&mut root).is_pending());
        for expected in &plan[..3] {
            let request = requests.try_recv().unwrap();
            assert_eq!(request.address, *expected);
            request.chan.send(Bytes::new());
        }
        assert!(requests.is_empty());
        let exhausted_at = Date::now();
        assert!(futures::poll!(&mut root).is_pending());
        let mut pending = Vec::new();
        for expected in &plan[3..5] {
            let request = requests.try_recv().unwrap();
            assert_eq!(request.address, *expected);
            pending.push(request);
        }
        assert!(requests.is_empty());
        let first = async_std::future::timeout(Duration::from_secs(2), async {
            let next = requests.recv();
            pin_mut!(next);
            match select(&mut root, next).await {
                Either::Right((Ok(request), _)) => request,
                _ => panic!("pending replicas must cause the next root hedge"),
            }
        })
        .await
        .unwrap();
        assert!(Date::now() - exhausted_at >= RETRIEVE_HEDGE_AFTER_MS as f64);
        assert_eq!(first.address, plan[5]);
        for expected in &plan[6..9] {
            let request = requests.try_recv().unwrap();
            assert_eq!(request.address, *expected);
            pending.push(request);
        }
        assert!(requests.is_empty());
        first.chan.send(raw.clone());
        assert_eq!(
            async_std::future::timeout(Duration::from_millis(500), root)
                .await
                .unwrap(),
            Some((raw, true)),
        );
    }

    #[wasm_bindgen_test]
    async fn feed_read_preserves_failure_and_explicit_missing_results() {
        for (attempts, empty, dropped, malformed) in [
            (0, 0, false, false),
            (2, 1, false, false),
            (2, 2, false, false),
            (2, 2, true, false),
            (1, 0, false, true),
        ] {
            let (chunks, requests) = crate::chunk_retrieve_channel();
            let confirmed_missing = attempts > 0 && empty == attempts && !dropped;
            let lookup = async {
                let result = crate::bzz_stream::acquire_latest_feed(
                    "11".repeat(20), "22".repeat(32), &chunks,
                ).await;
                let next = seek_next_feed_update_index("11".repeat(20), "22".repeat(32), &chunks).await;
                assert_eq!(next, if confirmed_missing { Ok(0) } else { Err(()) });
                result
            };
            let respond = async {
                let mut first = malformed;
                while let Ok(request) = requests.recv().await {
                    let admission = request.admission.as_ref().unwrap();
                    for _ in 0..attempts {
                        assert!(admission.try_claim_physical_attempt());
                    }
                    for _ in 0..empty {
                        admission.record_confirmed_empty_physical_attempt();
                    }
                    if dropped {
                        drop(request);
                    } else {
                        request.chan.send(if std::mem::take(&mut first) {
                            plain_chunk(b"bad")
                        } else {
                            Bytes::new()
                        });
                    }
                }
            };
            pin_mut!(lookup, respond);
            let Either::Left((result, _)) = select(lookup, respond).await else {
                panic!("responder ended first")
            };
            assert_eq!(result.is_ok(), confirmed_missing);
            assert!(result.ok().flatten().is_none());
        }
    }

    #[wasm_bindgen_test]
    async fn selection_skips_only_permanent_peers_and_preserves_reserve_rollback() {
        let ids = (0..200_u8)
            .map(|index| PeerId::from_bytes(&[0, 1, index]).unwrap())
            .collect::<Vec<_>>();
        let peers: OverlayPeerMap = Arc::new(Mutex::new(Arc::new(
            ids.iter()
                .enumerate()
                .map(|(i, peer)| ([i as u8; 32], *peer))
                .collect(),
        )));
        let accounting: PeerAccountingMap = Arc::default();
        let physical: PhysicalConnectionMap = Arc::default();
        for (index, &peer) in ids.iter().enumerate() {
            let connection_id = libp2p::swarm::ConnectionId::new_unchecked(index);
            accounting.lock().await.insert(
                peer,
                Arc::new(Mutex::new(PeerAccounting {
                    balance: 0,
                    surplus_balance: 0,
                    threshold: 0,
                    reserve: 0,
                    refreshment: 0.0,
                    refresh_scheduled: false,
                    id: peer,
                    connection_id: Some(connection_id),
                })),
            );
            crate::record_physical_connection_established(&physical, &peer, connection_id);
        }
        let mut skiplist = HashMap::new();
        let (selected, retry) =
            select_retrieve_peer(&[0; 32], &peers, &accounting, &physical, &mut skiplist).await;
        assert!(selected.is_none() && retry);
        assert_eq!(skiplist.capacity(), 0);

        accounting.lock().await.remove(&ids[0]);
        physical.lock().unwrap().remove(&ids[1]);
        for index in [1, 3] {
            accounting.lock().await[&ids[index]].lock().await.threshold = u64::MAX;
        }
        for _ in 0..2 {
            let (selected, retry) =
                select_retrieve_peer(&[0; 32], &peers, &accounting, &physical, &mut skiplist).await;
            let selected = selected.unwrap();
            assert_eq!(selected.peer, ids[3]);
            assert!(!retry);
            assert_eq!(skiplist, HashMap::from([
                (ids[0], None),
                (ids[1], Some(libp2p::swarm::ConnectionId::new_unchecked(1))),
                (ids[3], None),
            ]));
            cancel_reserve(&selected.accounting, selected.price).await;
            assert_eq!(selected.accounting.lock().await.reserve, 0);
            skiplist.remove(&selected.peer);
        }
        assert_eq!(accounting.lock().await[&ids[1]].lock().await.reserve, 0);
        skiplist.insert(ids[3], None);
        for index in [2, 4] {
            accounting.lock().await[&ids[index]].lock().await.threshold = u64::MAX;
        }
        // Recovered nearer credit is eligible before the next unattempted farther peer.
        for index in [2, 4] {
            let (selected, retry) =
                select_retrieve_peer(&[0; 32], &peers, &accounting, &physical, &mut skiplist).await;
            let selected = selected.unwrap();
            assert_eq!(selected.peer, ids[index]);
            assert!(!retry);
            cancel_reserve(&selected.accounting, selected.price).await;
        }
        let (selected, retry) =
            select_retrieve_peer(&[0; 32], &peers, &accounting, &physical, &mut skiplist).await;
        assert!(selected.is_none() && retry);
        assert_eq!(skiplist, ids[..5].iter().enumerate().map(|(index, peer)| {
            (*peer, (index == 1).then(|| libp2p::swarm::ConnectionId::new_unchecked(1)))
        }).collect());
        Arc::make_mut(&mut *peers.lock().await).retain(|overlay, _| overlay[0] < 5);
        let (selected, retry) =
            select_retrieve_peer(&[0; 32], &peers, &accounting, &physical, &mut skiplist).await;
        assert!(selected.is_none() && !retry);
        let peer = ids[1];
        let original = libp2p::swarm::ConnectionId::new_unchecked(1);
        crate::record_physical_connection_established(&physical, &peer, original);
        assert!(select_retrieve_peer(&[0; 32], &peers, &accounting, &physical, &mut skiplist).await.0.is_none());
        crate::record_physical_connection_closed(&physical, &peer, original);
        let account = accounting.lock().await.remove(&peer).unwrap();
        assert!(select_retrieve_peer(&[0; 32], &peers, &accounting, &physical, &mut skiplist).await.0.is_none());
        assert_eq!(skiplist[&peer], Some(original), "missing accounting must not make a transient exclusion permanent");
        accounting.lock().await.insert(peer, account.clone());

        // Each outcome starts an independent flight with the same failed original session.
        for (outcome, found) in [
            (RetrieveAttemptResult::Empty, false),
            (RetrieveAttemptResult::Found(Bytes::from_static(b"confirmed"), false), true),
        ] {
            let mut exclusions = skiplist.clone();
            let first = libp2p::swarm::ConnectionId::new_unchecked(100);
            account.lock().await.connection_id = Some(first);
            crate::record_physical_connection_established(&physical, &peer, first);
            let selected = select_retrieve_peer(&[0; 32], &peers, &accounting, &physical, &mut exclusions).await.0.unwrap();
            assert_eq!(selected.peer, peer);
            cancel_reserve(&selected.accounting, selected.price).await;
            assert_eq!(outcome.complete(&mut exclusions).is_some(), found);
            crate::record_physical_connection_closed(&physical, &peer, first);
            let later = libp2p::swarm::ConnectionId::new_unchecked(101);
            account.lock().await.connection_id = Some(later);
            crate::record_physical_connection_established(&physical, &peer, later);
            assert!(select_retrieve_peer(&[0; 32], &peers, &accounting, &physical, &mut exclusions).await.0.is_none());
            assert_eq!(account.lock().await.reserve, 0);
            crate::record_physical_connection_closed(&physical, &peer, later);
        }
    }

    #[wasm_bindgen_test]
    fn parity_batches_fill_only_free_slots_and_skip_admitted_shards() {
        for (data_count, parity_count, active) in
            [(119, 9, 119), (119, 9, 110), (39, 89, 1), (20, 87, 1)]
        {
            let data = vec![0; data_count * HASH_SIZE];
            let parity = (0..parity_count)
                .flat_map(|index| [index as u8; HASH_SIZE])
                .collect::<Vec<_>>();
            let mut dispatched = vec![false; data_count + parity_count];
            let (chunks, requests) = crate::chunk_retrieve_channel();
            let (results, incoming) = mpsc::unbounded();
            let cancel = None;
            let mut queue = RawFetchQueue::new(&chunks, &results, &cancel);
            let mut next = data_count;
            for limit in [data_count - active, 1] {
                let queued = dispatch_group_shards(
                    data.chunks_exact(HASH_SIZE),
                    parity.chunks_exact(HASH_SIZE),
                    &mut dispatched,
                    &mut queue,
                    data_count,
                    limit,
                    RetrieveHedgeDemand::DistinctShardManaged,
                );
                assert_eq!(queued, limit.min(data_count + parity_count - next));
                for index in next..next + queued {
                    let request = requests.try_recv().unwrap();
                    assert_eq!(
                        request.hedge_demand.unwrap().current(),
                        RetrieveHedgeDemand::DistinctShardManaged
                    );
                    request.chan.send(Bytes::new());
                    assert_eq!(incoming.try_recv().unwrap().index, index);
                }
                next += queued;
                assert!(requests.is_empty());
            }
        }

        let data = [0x61; HASH_SIZE * 2];
        let parity = [0x62; HASH_SIZE * 2];
        let (chunks, requests) = crate::chunk_retrieve_channel();
        let (results, incoming) = mpsc::unbounded();
        let cancel = None;
        let mut queue = RawFetchQueue::new(&chunks, &results, &cancel);
        let mut dispatched = [true, false, false, false];
        assert_eq!(
            dispatch_group_shards(
                data.chunks_exact(HASH_SIZE),
                parity.chunks_exact(HASH_SIZE),
                &mut dispatched,
                &mut queue,
                0,
                2,
                RetrieveHedgeDemand::Ordinary,
            ),
            2
        );
        for (index, address) in [(1, &data[HASH_SIZE..]), (2, &parity[..HASH_SIZE])] {
            let request = requests.try_recv().unwrap();
            assert_eq!(request.address, address);
            assert_eq!(
                request
                    .hedge_demand
                    .as_ref()
                    .map(SharedRetrieveHedgeDemand::current)
                    .unwrap_or(RetrieveHedgeDemand::Ordinary),
                RetrieveHedgeDemand::Ordinary
            );
            request.chan.send(Bytes::new());
            assert_eq!(incoming.try_recv().unwrap().index, index);
        }
        assert!(requests.is_empty());
    }

    #[wasm_bindgen_test]
    async fn ready_children_finish_the_group_before_an_expired_parity_hedge() {
        let raw = [
            plain_chunk(b"first streamed child"),
            plain_chunk(b"last streamed child"),
        ];
        let mut references: Vec<_> = raw
            .iter()
            .flat_map(|chunk| crate::content_address(chunk))
            .collect();
        references.extend_from_slice(&[0xa9; HASH_SIZE]);
        let (chunks, requests) = crate::chunk_retrieve_channel();
        let (events, children) = mpsc::unbounded();
        let emitter = GroupChildEmitter {
            context: GroupTraversalContext {
                parent_start: 0,
                parent_span: 8192,
                parent_depth: 0,
                child_capacity: 4096,
                child_count: 2,
            },
            events,
        };
        let group = fetch_data_group_indices_streaming(
            references.into(),
            ReferenceLayout {
                data_shards: 2,
                parity_shards: 1,
                child_capacity: 4096,
            },
            false,
            0..2,
            chunks.clone(),
            None,
            emitter,
        );
        pin_mut!(group);
        assert!(futures::poll!(&mut group).is_pending());
        let first = requests.try_recv().unwrap();
        let last = requests.try_recv().unwrap();
        assert!(requests.is_empty());
        first.chan.send(raw[0].clone());
        assert!(futures::poll!(&mut group).is_pending());
        assert_eq!(children.try_recv().unwrap().1, 0);
        assert!(children.is_empty());

        // Queue the terminal data result after the hedge is due but before
        // polling the group; it must win before any parity is admitted.
        async_std::task::sleep(Duration::from_millis(RETRIEVE_HEDGE_AFTER_MS + 25)).await;
        last.chan.send(raw[1].clone());
        assert_eq!(group.await, Some(()));
        assert_eq!(children.try_recv().unwrap().1, 1);
        assert!(children.is_empty());
        assert!(requests.is_empty());
    }

    #[wasm_bindgen_test]
    async fn partial_group_recovers_or_cancels_without_abandoning_owned_attempts() {
        for cancel_group in [false, true] {
            let raw: Vec<_> = (0..3)
                .map(|index| plain_chunk(&[0xd1, index, cancel_group as u8]))
                .collect();
            let references: Vec<_> = raw
                .iter()
                .map(|chunk| Bytes::from(crate::content_address(chunk)))
                .collect();
            let encoder = erasure_coding::ParityEncoder::new_padded(
                &raw.iter().map(Bytes::as_ref).collect::<Vec<_>>(),
                2,
                CHUNK_WITH_SPAN_SIZE,
            )
            .unwrap();
            let parity: Vec<Bytes> = (0..2)
                .map(|index| encoder.encode_shard(index).unwrap().into())
                .collect();
            let payload = references
                .iter()
                .flat_map(|reference| reference.iter().copied())
                .chain(
                    parity
                        .iter()
                        .flat_map(|chunk| crate::content_address(chunk)),
                )
                .collect::<Vec<_>>();
            let registry = crate::retrieval_conventions::RetrieveCancelRegistry::default();
            let cancel = registry.register("group recovery fixture".into(), 1).await;
            let (chunks, requests) = crate::chunk_retrieve_channel();
            let (events, children) = mpsc::unbounded();
            let emitter = GroupChildEmitter {
                context: GroupTraversalContext {
                    parent_start: 0,
                    parent_span: 12288,
                    parent_depth: 0,
                    child_capacity: 4096,
                    child_count: 3,
                },
                events,
            };
            let group = fetch_data_group_indices_streaming(
                payload.into(),
                ReferenceLayout {
                    data_shards: 3,
                    parity_shards: 2,
                    child_capacity: 4096,
                },
                false,
                0..1,
                chunks.clone(),
                cancel,
                emitter,
            );
            pin_mut!(group);
            assert!(futures::poll!(&mut group).is_pending());
            requests.try_recv().unwrap().chan.send(Bytes::new());
            assert!(futures::poll!(&mut group).is_pending());
            let data = requests.try_recv().unwrap();
            let late = requests.try_recv().unwrap();
            assert_eq!(late.address, references[2].as_ref());
            let admission = late.admission.clone().unwrap();
            assert!(admission.try_claim_physical_attempt());
            data.chan.send(raw[1].clone());
            for raw_parity in parity {
                requests.try_recv().unwrap().chan.send(raw_parity);
            }
            assert!(requests.is_empty());
            if cancel_group {
                registry.register("group recovery fixture".into(), 2).await;
            }
            assert_eq!(group.await, (!cancel_group).then_some(()));
            assert!(!admission.is_open());
            if !cancel_group {
                let (_, index, recovered) = children.try_recv().unwrap();
                assert_eq!(index, 0);
                assert_eq!(
                    recovered.payload.as_ref(),
                    &raw[0][erasure_coding::SPAN_SIZE..]
                );
            }
            assert!(children.is_empty());
            assert!(requests.is_empty());
            late.chan.send(raw[2].clone());
            assert_eq!(cached_raw_chunk(&references[2]), Some(raw[2].clone()));
        }
    }

    #[wasm_bindgen_test]
    async fn cached_join_drains_children_after_its_final_group_completes() {
        let expected = [vec![0x31; CHUNK_SIZE], vec![0xa7; CHUNK_SIZE]].concat();
        let mut references = Vec::new();
        for payload in expected.chunks(CHUNK_SIZE) {
            let raw = plain_chunk(payload);
            let address = crate::content_address(&raw);
            references.extend_from_slice(&address);
            remember_raw_chunk(address, raw);
        }
        let root = DecodedJoinChunk {
            level: RedundancyLevel::None,
            span: expected.len() as u64,
            payload: Bytes::from(references),
        };
        let (chunks, requests) = crate::chunk_retrieve_channel();
        let span_prefix = root.span.to_le_bytes();
        for (length, prefix) in [
            (expected.len(), &[][..]),
            (expected.len(), span_prefix.as_slice()),
            (CHUNK_SIZE, span_prefix.as_slice()),
        ] {
            let result = retrieve_data_range_from_root(
                root.clone(),
                0,
                length as u64 - 1,
                false,
                &chunks,
                prefix,
                None,
            )
            .await;
            assert_eq!(result, Some([prefix, &expected[..length]].concat()));
        }
        assert_eq!(
            crate::bzz_stream::retrieve_embedded_data(&0_u64.to_le_bytes(), false, &chunks).await,
            Some(0_u64.to_le_bytes().to_vec())
        );
        assert!(requests.is_empty());
    }

    #[wasm_bindgen_test]
    fn raw_reply_drop_success_and_stale_drop_settle_only_their_own_flight() {
        let raw = plain_chunk(b"direct raw completion contract");
        let address = crate::content_address(&raw);
        let (chunks, requests) = crate::chunk_retrieve_channel();
        let (results, incoming) = mpsc::unbounded();
        let cancel = None;
        let mut queue = RawFetchQueue::new(&chunks, &results, &cancel);
        queue.queue_data_shard(4, &address, RetrieveHedgeDemand::Ordinary);
        let request = requests.try_recv().unwrap();
        let crate::ChunkRetrieveReply::Raw(completion) = request.chan else {
            panic!("raw request must own its completion");
        };
        let stale = RawFetchCompletion {
            key: completion.key.clone(),
            flight_id: completion.flight_id,
        };
        drop(completion);
        let failed = incoming.try_recv().unwrap();
        assert_eq!(failed.index, 4);
        assert!(failed.chunk.is_empty());

        queue.queue_data_shard(5, &address, RetrieveHedgeDemand::Ordinary);
        let request = requests.try_recv().unwrap();
        drop(stale);
        assert!(incoming.try_recv().is_err());
        request.chan.send(raw.clone());
        let success = incoming.try_recv().unwrap();
        assert_eq!(success.index, 5);
        assert_eq!(success.chunk, raw);
        assert_eq!(success.chunk.as_ptr(), raw.as_ptr());
        assert!(success.canonical_cac);
        assert!(incoming.try_recv().is_err());

        drop(requests);
        queue.queue_data_shard(6, &[0x6f; HASH_SIZE], RetrieveHedgeDemand::Ordinary);
        let rejected = incoming.try_recv().unwrap();
        assert_eq!(rejected.index, 6);
        assert!(rejected.chunk.is_empty());
    }

    #[wasm_bindgen_test]
    fn channel_reply_drop_remains_distinct_from_confirmed_empty() {
        let (sender, mut receiver) = oneshot::channel();
        drop(crate::ChunkRetrieveReply::Channel(sender));
        assert!(receiver.try_recv().is_err());

        let (sender, mut receiver) = oneshot::channel();
        crate::ChunkRetrieveReply::Channel(sender).send(Bytes::new());
        assert_eq!(receiver.try_recv(), Ok(Some(Vec::new())));
    }

    #[wasm_bindgen_test]
    fn decoded_cache_lru_reuses_the_stored_bytes_key() {
        let reference = vec![0x3c; HASH_SIZE];
        let mut cache = DecodedChunkCache::default();
        cache.insert(reference.clone(), Some(Bytes::from_static(b"raw")), None);
        assert!(cache.get_decoded(&reference, false).is_none());
        assert!(cache.chunks[reference.as_slice()].decoded.is_none());

        let stored_key_ptr = cache
            .chunks
            .get_key_value(reference.as_slice())
            .expect("slice lookup should find the Bytes key")
            .0
            .as_ptr();
        assert_eq!(cache.chunks.back().unwrap().0.as_ptr(), stored_key_ptr);

        cache.insert(reference.clone(), Some(Bytes::from_static(b"ignored")), None);
        assert_eq!(cache.chunks.back().unwrap().0.as_ptr(), stored_key_ptr);
        assert_eq!(
            cache.get_raw(&reference).as_deref(),
            Some(b"raw".as_slice())
        );
        assert_eq!(cache.chunks.back().unwrap().0.as_ptr(), stored_key_ptr);
    }

    #[wasm_bindgen_test]
    fn decoded_cache_evicts_the_oldest_entry_after_raw_and_failed_decode_hits() {
        let reference = |index: usize| {
            let mut key = vec![0; HASH_SIZE];
            key[..8].copy_from_slice(&(index as u64).to_le_bytes());
            key
        };
        let mut cache = DecodedChunkCache::default();
        for index in 0..RETRIEVE_DECODED_CHUNK_CACHE_ENTRIES {
            cache.insert(reference(index), Some(Bytes::from_static(b"raw")), None);
        }
        assert!(cache.get_decoded(&reference(0), true).is_none());
        assert!(cache.get_raw(&reference(1)).is_some());
        cache.insert(
            reference(RETRIEVE_DECODED_CHUNK_CACHE_ENTRIES),
            Some(Bytes::from_static(b"next")),
            None,
        );
        assert_eq!(cache.chunks.len(), RETRIEVE_DECODED_CHUNK_CACHE_ENTRIES);
        assert!(cache.get_raw(&reference(0)).is_some());
        assert!(cache.get_raw(&reference(1)).is_some());
        assert!(cache.get_raw(&reference(2)).is_none());
    }

    #[wasm_bindgen_test]
    fn unencrypted_decoded_payload_shares_the_cached_raw_backing() {
        let raw = plain_chunk(b"cat");
        let mut cache = DecodedChunkCache::default();
        cache.insert(vec![0; HASH_SIZE], Some(raw.clone()), None);
        let (decoded, cached_raw) = cache.get_decoded(&[0; HASH_SIZE], true).unwrap();

        assert_eq!(decoded.payload.as_ref(), b"cat");
        assert_eq!(
            raw.as_ptr().wrapping_add(erasure_coding::SPAN_SIZE),
            decoded.payload.as_ptr()
        );
        assert_eq!(cached_raw.unwrap().as_ptr(), raw.as_ptr());
        let (hit, omitted_raw) = cache.get_decoded(&[0; HASH_SIZE], false).unwrap();
        assert_eq!(hit.payload.as_ptr(), decoded.payload.as_ptr());
        assert!(omitted_raw.is_none());
        cache.chunks.clear();
        cache.insert(vec![0; HASH_SIZE], None, Some(decoded));
        let (_, missing_raw) = cache.get_decoded(&[0; HASH_SIZE], true).unwrap();
        assert!(missing_raw.is_none());
    }

    #[wasm_bindgen_test]
    fn zero_waiter_late_success_caches_every_full_reference_owned_by_the_flight() {
        let raw = plain_chunk(b"late raw cache payload");
        let expected_cac = crate::content_address(&raw);
        let plain_reference = expected_cac.clone();
        let mut encrypted_reference = expected_cac.clone();
        encrypted_reference.extend_from_slice(&[0x5a; HASH_SIZE]);
        let key = RawFetchKey::new(usize::MAX - 17, &expected_cac, &expected_cac, &None);
        assert_eq!(key.request_address.as_ptr(), key.expected_cac.as_ptr());
        let (result_chan, _result_in) = mpsc::unbounded();
        let registration = RAW_FETCH_FLIGHTS.with(|flights| {
            flights.borrow_mut().register(
                key.clone(),
                RawFetchWaiter {
                    index: 0,
                    result_chan,
                },
                || RawFetchShared::new(RetrieveHedgeDemand::Ordinary),
            )
        });
        registration
            .shared
            .remember_cache_reference(Some(&plain_reference));
        registration
            .shared
            .remember_cache_reference(Some(&plain_reference));
        registration
            .shared
            .remember_cache_reference(Some(&encrypted_reference));
        assert_eq!(registration.shared.cache_references.borrow().len(), 2);
        assert!(registration.shared.admission.try_claim_physical_attempt());
        remove_raw_fetch_waiter(&key, registration.flight_id, registration.waiter_id);

        RawFetchCompletion {
            key,
            flight_id: registration.flight_id,
        }
        .send(raw.clone());
        assert_eq!(
            cached_raw_chunk(&plain_reference).as_deref(),
            Some(raw.as_ref())
        );
        assert_eq!(
            cached_raw_chunk(&encrypted_reference).as_deref(),
            Some(raw.as_ref())
        );
    }
}

#[cfg(test)]
#[path = "../tests/support/retrieval_credit.rs"]
mod credit_tests;
