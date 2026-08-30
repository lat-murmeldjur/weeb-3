pub(crate) fn next_nonzero_generation(current: u64) -> u64 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

pub(crate) fn generation_is_newer(candidate: u64, current: u64) -> bool {
    const SERIAL_HALF_RANGE: u64 = 1_u64 << 63;

    candidate != current && candidate.wrapping_sub(current) < SERIAL_HALF_RANGE
}

pub(crate) fn latest_registered_generation(current: u64, candidate: u64) -> u64 {
    if current == 0 || generation_is_newer(candidate, current) {
        candidate
    } else {
        current
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PendingGenerationRelation {
    Join,
    RejectStale,
    Replace,
}

pub(crate) fn pending_generation_relation(
    pending_generation: u64,
    candidate_generation: u64,
) -> PendingGenerationRelation {
    if pending_generation == candidate_generation
        || pending_generation == 0
        || candidate_generation == 0
    {
        PendingGenerationRelation::Join
    } else if generation_is_newer(candidate_generation, pending_generation) {
        PendingGenerationRelation::Replace
    } else {
        PendingGenerationRelation::RejectStale
    }
}

use async_lock::{Semaphore, SemaphoreGuardArc};
use async_std::sync::Mutex;
use event_listener::Event;
use futures::{
    future::{Either, select},
    pin_mut,
};
use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
};

#[derive(Debug, Default)]
struct RetrieveCancelScope {
    latest: AtomicU64,
    changed: Event,
}

#[derive(Clone, Debug)]
pub(crate) struct RetrieveCancelToken {
    pub(crate) stream_key: Arc<str>,
    pub(crate) generation: u64,
    scope: Arc<RetrieveCancelScope>,
}

impl RetrieveCancelToken {
    pub(crate) fn is_current(&self) -> bool {
        self.scope.latest.load(Ordering::Acquire) == self.generation
    }

    pub(crate) async fn cancelled(&self) {
        let changed = self.scope.changed.listen();
        if self.is_current() {
            changed.await;
        }
    }
}

#[derive(Default)]
pub(crate) struct RetrieveCancelRegistry {
    scopes: Mutex<HashMap<Arc<str>, Arc<RetrieveCancelScope>>>,
}

impl RetrieveCancelRegistry {
    pub(crate) async fn register(
        &self,
        stream_key: String,
        generation: u64,
    ) -> Option<RetrieveCancelToken> {
        if stream_key.is_empty() || generation == 0 {
            return None;
        }

        let stream_key: Arc<str> = stream_key.into();
        let mut scopes = self.scopes.lock().await;
        let scope = scopes
            .entry(stream_key.clone())
            .or_insert_with(Arc::default)
            .clone();
        let current = scope.latest.load(Ordering::Acquire);
        let latest = latest_registered_generation(current, generation);
        if latest != current {
            scope.latest.store(latest, Ordering::Release);
            scope.changed.notify(usize::MAX);
        }
        Some(RetrieveCancelToken {
            stream_key,
            generation,
            scope,
        })
    }
}

pub(crate) fn retrieve_cancel_token_current(cancel: &Option<RetrieveCancelToken>) -> bool {
    cancel.as_ref().is_none_or(RetrieveCancelToken::is_current)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum RetrieveHedgeDemand {
    DistinctShardManaged = 0,
    Ordinary = 1,
}

/// Shared monotone demand for one raw singleflight. An ordinary waiter may
/// promote a managed leader, but a later managed waiter can never demote it.
#[derive(Debug)]
struct SharedRetrieveHedgeDemandInner {
    demand: AtomicU8,
    promoted: Event,
}

#[derive(Clone, Debug)]
pub(crate) struct SharedRetrieveHedgeDemand {
    inner: Arc<SharedRetrieveHedgeDemandInner>,
}

impl SharedRetrieveHedgeDemand {
    pub(crate) fn new(demand: RetrieveHedgeDemand) -> Self {
        Self {
            inner: Arc::new(SharedRetrieveHedgeDemandInner {
                demand: AtomicU8::new(demand as u8),
                promoted: Event::new(),
            }),
        }
    }

    pub(crate) fn current(&self) -> RetrieveHedgeDemand {
        match self.inner.demand.load(Ordering::Acquire) {
            0 => RetrieveHedgeDemand::DistinctShardManaged,
            _ => RetrieveHedgeDemand::Ordinary,
        }
    }

    pub(crate) fn promote(&self, demand: RetrieveHedgeDemand) {
        if self.inner.demand.fetch_max(demand as u8, Ordering::AcqRel) < demand as u8 {
            self.inner.promoted.notify(usize::MAX);
        }
    }

    pub(crate) async fn wait_until_ordinary(&self) {
        let promoted = self.inner.promoted.listen();
        if self.current() != RetrieveHedgeDemand::Ordinary {
            promoted.await;
        }
    }
}

pub(crate) fn retrieve_attempt_start_allowed(
    demand: RetrieveHedgeDemand,
    in_flight: usize,
    ordinary_hedge_due: bool,
) -> bool {
    in_flight == 0 || (demand == RetrieveHedgeDemand::Ordinary && ordinary_hedge_due)
}

pub(crate) fn rolling_full_group_eligible(
    requested_count: usize,
    data_count: usize,
    parity_count: usize,
    decoded_only_count: usize,
    unresolved_count: usize,
) -> bool {
    rolling_full_group_static_candidate(requested_count, data_count, parity_count)
        && unresolved_count > 0
        && decoded_only_count < parity_count
}

pub(crate) fn rolling_full_group_static_candidate(
    requested_count: usize,
    data_count: usize,
    parity_count: usize,
) -> bool {
    data_count > 0 && requested_count == data_count && parity_count > 0
}

/// One admission per coordinator turn makes the terminal check precede every
/// replacement while never exceeding the structural data-group active width.
pub(crate) fn rolling_parity_admission_count(
    elapsed_ms: u64,
    gate_ms: u64,
    terminal: bool,
    active_width: usize,
    rolling_active: usize,
    remaining_parity: usize,
) -> usize {
    if terminal || elapsed_ms < gate_ms {
        return 0;
    }
    active_width
        .saturating_sub(rolling_active)
        .min(remaining_parity)
        .min(1)
}

pub(crate) fn rolling_next_parity_index(
    data_count: usize,
    dispatched_shards: &[bool],
) -> Option<usize> {
    (data_count..dispatched_shards.len()).find(|&index| !dispatched_shards[index])
}

#[derive(Debug)]
struct RetrieveAdmissionInner {
    open: AtomicBool,
    returned_cac: AtomicBool,
    attempt_limit: Option<usize>,
    attempts_remaining: Option<AtomicUsize>,
    timed_out_attempts: Option<AtomicUsize>,
    confirmed_empty_attempts: Option<AtomicUsize>,
    closed: Event,
}

/// Closing admission cannot cancel a reservation or dispatched exchange.
#[derive(Clone, Debug)]
pub(crate) struct RetrieveAdmission {
    inner: Arc<RetrieveAdmissionInner>,
}

impl RetrieveAdmission {
    pub(crate) fn new() -> Self {
        Self::with_attempts_remaining(None)
    }

    pub(crate) fn new_with_attempt_limit(max_attempts: usize) -> Self {
        Self::with_attempts_remaining(Some(max_attempts))
    }

    fn with_attempts_remaining(attempts_remaining: Option<usize>) -> Self {
        Self {
            inner: Arc::new(RetrieveAdmissionInner {
                open: AtomicBool::new(attempts_remaining != Some(0)),
                returned_cac: AtomicBool::new(false),
                attempt_limit: attempts_remaining,
                attempts_remaining: attempts_remaining.map(AtomicUsize::new),
                timed_out_attempts: attempts_remaining.map(|_| AtomicUsize::new(0)),
                confirmed_empty_attempts: attempts_remaining.map(|_| AtomicUsize::new(0)),
                closed: Event::new(),
            }),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.inner.open.load(Ordering::SeqCst)
    }

    pub(crate) fn record_returned_cac(&self) {
        self.inner.returned_cac.store(true, Ordering::Release);
    }

    pub(crate) fn returned_cac(&self) -> bool {
        self.inner.returned_cac.load(Ordering::Acquire)
    }

    pub(crate) fn close(&self) {
        if self.inner.open.swap(false, Ordering::SeqCst) {
            self.inner.closed.notify(usize::MAX);
        }
    }

    pub(crate) fn physical_attempt_available(&self) -> bool {
        self.inner
            .attempts_remaining
            .as_ref()
            .is_none_or(|remaining| remaining.load(Ordering::SeqCst) != 0)
    }

    pub(crate) fn claimed_physical_attempts(&self) -> Option<usize> {
        self.inner
            .attempt_limit
            .zip(self.inner.attempts_remaining.as_ref())
            .map(|(limit, remaining)| limit.saturating_sub(remaining.load(Ordering::SeqCst)))
    }

    pub(crate) fn record_physical_attempt_timeout(&self) {
        if let Some(timed_out) = self.inner.timed_out_attempts.as_ref() {
            timed_out.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(crate) fn timed_out_physical_attempts(&self) -> Option<usize> {
        self.inner
            .timed_out_attempts
            .as_ref()
            .map(|timed_out| timed_out.load(Ordering::SeqCst))
    }

    pub(crate) fn record_confirmed_empty_physical_attempt(&self) {
        if let Some(confirmed_empty) = self.inner.confirmed_empty_attempts.as_ref() {
            confirmed_empty.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(crate) fn confirmed_empty_physical_attempts(&self) -> Option<usize> {
        self.inner
            .confirmed_empty_attempts
            .as_ref()
            .map(|confirmed_empty| confirmed_empty.load(Ordering::SeqCst))
    }

    /// Atomically claim one physical exchange before it is dispatched. An exhausted finite
    /// budget closes only future admission; exchanges that already claimed a slot still settle.
    pub(crate) fn try_claim_physical_attempt(&self) -> bool {
        let Some(attempts_remaining) = self.inner.attempts_remaining.as_ref() else {
            return true;
        };
        if !self.is_open() {
            return false;
        }

        let mut remaining = attempts_remaining.load(Ordering::SeqCst);
        loop {
            if remaining == 0 {
                return false;
            }

            match attempts_remaining.compare_exchange_weak(
                remaining,
                remaining - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    if remaining == 1 {
                        self.close();
                    }
                    return true;
                }
                Err(current) => remaining = current,
            }
        }
    }

    pub(crate) async fn wait_closed(&self) {
        let listener = self.inner.closed.listen();
        if self.is_open() {
            listener.await;
        }
    }

    pub(crate) fn close_on_drop(&self) -> RetrieveAdmissionGuard {
        RetrieveAdmissionGuard {
            admission: self.clone(),
        }
    }
}

#[must_use = "dropping the guard closes its retrieve admission group"]
pub(crate) struct RetrieveAdmissionGuard {
    admission: RetrieveAdmission,
}

impl Drop for RetrieveAdmissionGuard {
    fn drop(&mut self) {
        self.admission.close();
    }
}

pub(crate) fn retrieve_admission_current(
    stream_generation_current: bool,
    admission: &Option<RetrieveAdmission>,
) -> bool {
    stream_generation_current && admission.as_ref().is_none_or(RetrieveAdmission::is_open)
}

pub(crate) async fn acquire_retrieve_permit(
    semaphore: &Arc<Semaphore>,
    admission: Option<&RetrieveAdmission>,
) -> Option<SemaphoreGuardArc> {
    if admission.is_some_and(|admission| !admission.is_open()) {
        return None;
    }

    let permit = semaphore.clone().acquire_arc();
    let closed = async {
        match admission {
            Some(admission) => admission.wait_closed().await,
            None => pending::<()>().await,
        }
    };
    pin_mut!(permit, closed);

    match select(permit, closed).await {
        Either::Left((permit, _)) if admission.is_none_or(RetrieveAdmission::is_open) => {
            Some(permit)
        }
        Either::Left(_) | Either::Right(_) => None,
    }
}

use std::{collections::HashMap, hash::Hash};

pub(crate) struct SingleflightRegistration<K, A> {
    pub(crate) key: K,
    pub(crate) flight_id: u64,
    pub(crate) waiter_id: u64,
    pub(crate) shared: A,
    pub(crate) leader: bool,
}

pub(crate) struct SingleflightFlight<W, A> {
    pub(crate) shared: A,
    pub(crate) waiters: Vec<W>,
}

pub(crate) struct SingleflightRegistry<K, W, A> {
    next_flight_id: u64,
    next_waiter_id: u64,
    flights: HashMap<K, SingleflightEntry<W, A>>,
}

struct SingleflightEntry<W, A> {
    flight_id: u64,
    shared: A,
    waiters: Vec<(u64, W)>,
}

impl<K, W, A> Default for SingleflightRegistry<K, W, A> {
    fn default() -> Self {
        Self {
            next_flight_id: 0,
            next_waiter_id: 0,
            flights: HashMap::new(),
        }
    }
}

impl<K, W, A> SingleflightRegistry<K, W, A>
where
    K: Clone + Eq + Hash,
    A: Clone,
{
    pub(crate) fn register(
        &mut self,
        key: K,
        waiter: W,
        make_shared: impl FnOnce() -> A,
    ) -> SingleflightRegistration<K, A> {
        self.next_waiter_id = next_nonzero_generation(self.next_waiter_id);
        let waiter_id = self.next_waiter_id;

        let (flight_id, shared, leader) = if let Some(flight) = self.flights.get_mut(&key) {
            flight.waiters.push((waiter_id, waiter));
            (flight.flight_id, flight.shared.clone(), false)
        } else {
            self.next_flight_id = next_nonzero_generation(self.next_flight_id);
            let flight_id = self.next_flight_id;
            let shared = make_shared();
            self.flights.insert(
                key.clone(),
                SingleflightEntry {
                    flight_id,
                    shared: shared.clone(),
                    waiters: vec![(waiter_id, waiter)],
                },
            );
            (flight_id, shared, true)
        };

        SingleflightRegistration {
            key,
            flight_id,
            waiter_id,
            shared,
            leader,
        }
    }

    /// Keep a zero-waiter flight registered until its dispatched producer settles.
    pub(crate) fn remove_waiter(&mut self, key: &K, flight_id: u64, waiter_id: u64) -> Option<A> {
        let flight = self.flights.get_mut(key)?;
        if flight.flight_id != flight_id {
            return None;
        }
        let position = flight.waiters.iter().position(|(id, _)| *id == waiter_id)?;
        flight.waiters.swap_remove(position);
        if !flight.waiters.is_empty() {
            return None;
        }

        Some(flight.shared.clone())
    }

    /// A stale producer cannot detach a newer flight with the same key.
    pub(crate) fn take(&mut self, key: &K, flight_id: u64) -> Option<SingleflightFlight<W, A>> {
        if self.flights.get(key)?.flight_id != flight_id {
            return None;
        }
        let flight = self.flights.remove(key)?;
        Some(SingleflightFlight {
            shared: flight.shared,
            waiters: flight
                .waiters
                .into_iter()
                .map(|(_, waiter)| waiter)
                .collect(),
        })
    }
}
