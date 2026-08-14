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

pub(crate) fn cancel_generation_is_current(latest: Option<u64>, candidate: u64) -> bool {
    latest
        .map(|generation| generation == candidate || generation_is_newer(candidate, generation))
        .unwrap_or(true)
}

use async_lock::{Semaphore, SemaphoreGuardArc};
use event_listener::Event;
use futures::{
    future::{Either, select},
    pin_mut,
};
use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug)]
struct RetrieveAdmissionInner {
    open: AtomicBool,
    closed: Event,
}

/// Closing admission cannot cancel a reservation or dispatched exchange.
#[derive(Clone, Debug)]
pub(crate) struct RetrieveAdmission {
    inner: Arc<RetrieveAdmissionInner>,
}

impl RetrieveAdmission {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(RetrieveAdmissionInner {
                open: AtomicBool::new(true),
                closed: Event::new(),
            }),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.inner.open.load(Ordering::SeqCst)
    }

    pub(crate) fn close(&self) {
        if self.inner.open.swap(false, Ordering::SeqCst) {
            self.inner.closed.notify(usize::MAX);
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
    waiters: HashMap<u64, W>,
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
        self.next_waiter_id = self.next_waiter_id.wrapping_add(1);
        if self.next_waiter_id == 0 {
            self.next_waiter_id = 1;
        }
        let waiter_id = self.next_waiter_id;

        let (flight_id, shared, leader) = if let Some(flight) = self.flights.get_mut(&key) {
            flight.waiters.insert(waiter_id, waiter);
            (flight.flight_id, flight.shared.clone(), false)
        } else {
            self.next_flight_id = self.next_flight_id.wrapping_add(1);
            if self.next_flight_id == 0 {
                self.next_flight_id = 1;
            }
            let flight_id = self.next_flight_id;
            let shared = make_shared();
            self.flights.insert(
                key.clone(),
                SingleflightEntry {
                    flight_id,
                    shared: shared.clone(),
                    waiters: HashMap::from([(waiter_id, waiter)]),
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
        flight.waiters.remove(&waiter_id)?;
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
            waiters: flight.waiters.into_values().collect(),
        })
    }
}
