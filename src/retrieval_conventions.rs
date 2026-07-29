pub(crate) fn next_nonzero_generation(current: u64) -> u64 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

pub(crate) fn pending_load_matches(
    current_generation: u64,
    current_load_id: u64,
    expected_generation: u64,
    expected_load_id: u64,
) -> bool {
    current_generation == expected_generation && current_load_id == expected_load_id
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

pub(crate) fn outer_range_retry_count(generation: u64, stream_retry_count: usize) -> usize {
    if generation == 0 {
        0
    } else {
        stream_retry_count
    }
}

#[cfg(test)]
mod generation_tests {
    use super::{
        PendingGenerationRelation, cancel_generation_is_current, generation_is_newer,
        latest_registered_generation, next_nonzero_generation, outer_range_retry_count,
        pending_generation_relation, pending_load_matches,
    };

    #[test]
    fn advances_across_create_seek_evict_and_recreate() {
        let created = next_nonzero_generation(0);
        let sought = next_nonzero_generation(created);
        let recreated = next_nonzero_generation(sought);

        assert_eq!(created, 1);
        assert!(sought > created);
        assert!(recreated > sought);
        assert!(!cancel_generation_is_current(Some(sought), created));
        assert!(cancel_generation_is_current(Some(sought), recreated));
    }

    #[test]
    fn wrapping_never_emits_the_reserved_zero_generation() {
        assert_eq!(next_nonzero_generation(u64::MAX), 1);
        assert!(generation_is_newer(1, u64::MAX));
        assert!(!generation_is_newer(u64::MAX, 1));
        assert!(cancel_generation_is_current(Some(u64::MAX), 1));
        assert!(!cancel_generation_is_current(Some(1), u64::MAX));
        assert_eq!(latest_registered_generation(u64::MAX, 1), 1);
        assert_eq!(latest_registered_generation(1, u64::MAX), 1);
    }

    #[test]
    fn pending_generation_order_is_wrap_safe() {
        assert_eq!(latest_registered_generation(0, 1), 1);
        assert_eq!(latest_registered_generation(7, 8), 8);
        assert_eq!(latest_registered_generation(8, 7), 8);
        assert_eq!(
            pending_generation_relation(u64::MAX, 1),
            PendingGenerationRelation::Replace
        );
        assert_eq!(
            pending_generation_relation(1, u64::MAX),
            PendingGenerationRelation::RejectStale
        );
        assert_eq!(
            pending_generation_relation(7, 7),
            PendingGenerationRelation::Join
        );
        assert_eq!(
            pending_generation_relation(0, 7),
            PendingGenerationRelation::Join
        );
    }

    #[test]
    fn non_stream_ranges_do_not_retry_past_the_caller_timeout() {
        assert_eq!(outer_range_retry_count(0, 1), 0);
        assert_eq!(outer_range_retry_count(9, 1), 1);
    }

    #[test]
    fn expired_load_cannot_remove_same_generation_replacement() {
        let generation = 7;
        let expired_load = 41;
        let replacement_load = 42;

        let mut pending = Some((generation, expired_load));
        if pending.is_some_and(|(current_generation, current_load_id)| {
            pending_load_matches(
                current_generation,
                current_load_id,
                generation,
                expired_load,
            )
        }) {
            pending = None;
        }
        assert!(pending.is_none());

        pending = Some((generation, replacement_load));
        if pending.is_some_and(|(current_generation, current_load_id)| {
            pending_load_matches(
                current_generation,
                current_load_id,
                generation,
                expired_load,
            )
        }) {
            pending = None;
        }

        assert_eq!(pending, Some((generation, replacement_load)));
    }
}

use async_lock::{Semaphore, SemaphoreGuardArc};
use event_listener::Event;
use libp2p::futures::{
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

/// Monotonic, operation-local admission gate for speculative chunk requests.
///
/// Closing this gate only prevents work that has not crossed an accounting or
/// transport boundary yet. Callers that already reserved or dispatched must
/// still cancel that local reservation or drain the dispatched exchange.
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

    /// Wait until this gate closes without polling.
    pub(crate) async fn wait_closed(&self) {
        // Register before checking the state so a close cannot be missed.
        let listener = self.inner.closed.listen();
        if self.is_open() {
            listener.await;
        }
    }

    /// Ensure every early-return/error path closes the speculative group.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetrieveAdmissionState {
    /// New peer/accounting attempts may still be admitted.
    Open,
    /// Admission is closed, but dispatched attempts must finish settling.
    Draining,
    /// Admission is closed and no dispatched attempt remains.
    Closed,
}

pub(crate) fn retrieve_admission_state(
    admission_current: bool,
    in_flight: usize,
) -> RetrieveAdmissionState {
    if admission_current {
        RetrieveAdmissionState::Open
    } else if in_flight > 0 {
        RetrieveAdmissionState::Draining
    } else {
        RetrieveAdmissionState::Closed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetrievePostReserveAction {
    Dispatch,
    CancelReservation,
}

/// Recheck after a local accounting reservation and immediately before the
/// transport task is spawned. A close in this window cancels only the local
/// reservation; once dispatched, the exchange owns its full drain lifecycle.
pub(crate) fn retrieve_post_reserve_action(admission_current: bool) -> RetrievePostReserveAction {
    if admission_current {
        RetrievePostReserveAction::Dispatch
    } else {
        RetrievePostReserveAction::CancelReservation
    }
}

/// Wait for global retrieve capacity only while local admission stays open.
///
/// The returned permit marks the pre-dispatch boundary. Closing admission
/// drops queued work, while a caller that receives a permit must either cancel
/// its local reservation before dispatch or let the dispatched exchange drain.
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

#[cfg(test)]
mod admission_tests {
    use super::{
        RetrieveAdmission, RetrieveAdmissionState, RetrievePostReserveAction,
        acquire_retrieve_permit, retrieve_admission_current, retrieve_admission_state,
        retrieve_post_reserve_action,
    };
    use async_lock::Semaphore;
    use std::{
        future::Future,
        pin::pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll, Wake, Waker},
    };

    #[derive(Debug)]
    struct CountingWake(AtomicUsize);

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn close_is_monotonic_and_visible_to_every_clone() {
        let admission = RetrieveAdmission::new();
        let queued = admission.clone();

        assert!(admission.is_open());
        assert!(queued.is_open());
        admission.close();
        admission.close();

        assert!(!admission.is_open());
        assert!(!queued.is_open());
    }

    #[test]
    fn drop_guard_closes_early_return_paths() {
        let admission = RetrieveAdmission::new();
        {
            let _guard = admission.close_on_drop();
            assert!(admission.is_open());
        }

        assert!(!admission.is_open());
    }

    #[test]
    fn local_and_stream_cancellation_are_both_required_for_admission() {
        let admission = RetrieveAdmission::new();

        assert!(retrieve_admission_current(true, &None));
        assert!(!retrieve_admission_current(false, &None));
        assert!(retrieve_admission_current(true, &Some(admission.clone())));
        assert!(!retrieve_admission_current(false, &Some(admission.clone())));

        admission.close();
        assert!(!retrieve_admission_current(true, &Some(admission)));
    }

    #[test]
    fn closing_suppresses_new_attempts_but_drains_dispatched_ones() {
        assert_eq!(
            retrieve_admission_state(true, 0),
            RetrieveAdmissionState::Open
        );
        assert_eq!(
            retrieve_admission_state(true, 3),
            RetrieveAdmissionState::Open
        );
        assert_eq!(
            retrieve_admission_state(false, 3),
            RetrieveAdmissionState::Draining
        );
        assert_eq!(
            retrieve_admission_state(false, 0),
            RetrieveAdmissionState::Closed
        );
    }

    #[test]
    fn closing_after_reserve_cancels_locally_instead_of_dispatching() {
        assert_eq!(
            retrieve_post_reserve_action(true),
            RetrievePostReserveAction::Dispatch
        );
        assert_eq!(
            retrieve_post_reserve_action(false),
            RetrievePostReserveAction::CancelReservation
        );
    }

    #[test]
    fn close_wakes_pending_waiters() {
        let admission = RetrieveAdmission::new();
        let closer = admission.clone();
        let wake = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let mut context = Context::from_waker(&waker);
        let mut waiting = pin!(admission.wait_closed());

        assert_eq!(waiting.as_mut().poll(&mut context), Poll::Pending);
        closer.close();
        assert!(wake.0.load(Ordering::SeqCst) > 0);
        assert_eq!(waiting.as_mut().poll(&mut context), Poll::Ready(()));
    }

    #[test]
    fn waiting_after_close_is_immediately_ready() {
        let admission = RetrieveAdmission::new();
        admission.close();
        admission.close();

        let wake = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(wake);
        let mut context = Context::from_waker(&waker);
        let mut waiting = pin!(admission.wait_closed());

        assert_eq!(waiting.as_mut().poll(&mut context), Poll::Ready(()));
    }

    #[test]
    fn closing_drops_a_waiter_before_retrieve_capacity_is_reserved() {
        async_std::task::block_on(async {
            let semaphore = Arc::new(Semaphore::new(0));
            let admission = RetrieveAdmission::new();
            let closer = admission.clone();
            let waiting = async_std::task::spawn({
                let semaphore = semaphore.clone();
                async move { acquire_retrieve_permit(&semaphore, Some(&admission)).await }
            });

            async_std::task::yield_now().await;
            closer.close();
            assert!(waiting.await.is_none());
        });
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

    /// Remove one waiter from a specific incarnation of a keyed flight.
    ///
    /// A zero-waiter entry remains registered until its producer completes.
    /// This lets a later same-key waiter join the already-dispatched draining
    /// work instead of opening a duplicate flight. Returning the shared
    /// resource when the last waiter leaves lets the caller close only future
    /// admission. The identity check prevents a late cancellation task from
    /// touching a newer flight that reused the same key.
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

    /// Atomically detach a completed incarnation and all of its current
    /// waiters. A stale producer cannot consume a successor with the same key.
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

    #[cfg(test)]
    fn len(&self) -> usize {
        self.flights.len()
    }

    #[cfg(test)]
    fn waiter_count(&self, key: &K) -> usize {
        self.flights
            .get(key)
            .map_or(0, |flight| flight.waiters.len())
    }
}

#[cfg(test)]
mod singleflight_tests {
    use super::SingleflightRegistry;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn identical_keys_share_one_leader_and_one_resource() {
        let created = Rc::new(Cell::new(0));
        let mut flights = SingleflightRegistry::<String, usize, Rc<Cell<bool>>>::default();

        let first_created = Rc::clone(&created);
        let first = flights.register("chunk".to_string(), 3, move || {
            first_created.set(first_created.get() + 1);
            Rc::new(Cell::new(true))
        });
        let second_created = Rc::clone(&created);
        let second = flights.register("chunk".to_string(), 5, move || {
            second_created.set(second_created.get() + 1);
            Rc::new(Cell::new(true))
        });

        assert!(first.leader);
        assert!(!second.leader);
        assert_eq!(first.flight_id, second.flight_id);
        assert!(Rc::ptr_eq(&first.shared, &second.shared));
        assert_eq!(created.get(), 1);
        assert_eq!(flights.waiter_count(&"chunk".to_string()), 2);
    }

    #[test]
    fn last_waiter_closes_but_does_not_remove_the_producer_flight() {
        let mut flights = SingleflightRegistry::<u8, (), Rc<Cell<bool>>>::default();
        let first = flights.register(7, (), || Rc::new(Cell::new(true)));
        let second = flights.register(7, (), || Rc::new(Cell::new(true)));

        assert!(
            flights
                .remove_waiter(&7, first.flight_id, first.waiter_id)
                .is_none()
        );
        assert!(second.shared.get());
        let shared = flights
            .remove_waiter(&7, second.flight_id, second.waiter_id)
            .expect("last waiter returns the shared admission");
        shared.set(false);
        assert!(!first.shared.get());
        assert_eq!(flights.len(), 1);
        assert_eq!(flights.waiter_count(&7), 0);

        let follower = flights.register(7, (), || Rc::new(Cell::new(true)));
        assert!(!follower.leader);
        assert_eq!(follower.flight_id, first.flight_id);
        assert!(!follower.shared.get());

        let completed = flights
            .take(&7, first.flight_id)
            .expect("only the producer removes the flight");
        assert_eq!(completed.waiters.len(), 1);
        assert_eq!(flights.len(), 0);

        let successor = flights.register(7, (), || Rc::new(Cell::new(true)));
        assert!(successor.leader);
        assert_ne!(successor.flight_id, first.flight_id);
    }

    #[test]
    fn completion_detaches_every_waiter_atomically() {
        let mut flights = SingleflightRegistry::<u8, usize, ()>::default();
        let first = flights.register(9, 4, || ());
        flights.register(9, 2, || ());

        let mut waiters = flights
            .take(&9, first.flight_id)
            .expect("active flight")
            .waiters;
        waiters.sort_unstable();
        assert_eq!(waiters, vec![2, 4]);
        assert_eq!(flights.len(), 0);
    }

    #[test]
    fn distinct_scopes_never_share() {
        let mut flights = SingleflightRegistry::<(&str, u64), (), ()>::default();
        assert!(flights.register(("video", 1), (), || ()).leader);
        assert!(flights.register(("video", 2), (), || ()).leader);
        assert!(flights.register(("audio", 1), (), || ()).leader);
        assert_eq!(flights.len(), 3);
    }

    #[test]
    fn stale_producer_cannot_take_a_successor_with_the_same_key() {
        let mut flights = SingleflightRegistry::<u8, &'static str, Rc<Cell<bool>>>::default();
        let old = flights.register(3, "old", || Rc::new(Cell::new(true)));
        let old_shared = flights
            .remove_waiter(&3, old.flight_id, old.waiter_id)
            .expect("old flight admission is returned");
        old_shared.set(false);
        let old_flight = flights
            .take(&3, old.flight_id)
            .expect("old producer completes its own flight");
        assert_eq!(old_flight.waiters, Vec::<&'static str>::new());

        let successor = flights.register(3, "new", || Rc::new(Cell::new(true)));
        assert!(successor.leader);
        assert_ne!(old.flight_id, successor.flight_id);
        assert!(flights.take(&3, old.flight_id).is_none());
        assert!(
            flights
                .remove_waiter(&3, old.flight_id, old.waiter_id)
                .is_none()
        );
        assert_eq!(flights.waiter_count(&3), 1);

        let flight = flights
            .take(&3, successor.flight_id)
            .expect("successor remains registered");
        assert_eq!(flight.waiters, vec!["new"]);
    }
}
