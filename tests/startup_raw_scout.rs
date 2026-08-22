#![allow(dead_code)]

#[path = "../src/retrieval_conventions.rs"]
mod retrieval_conventions;

use futures::{FutureExt, StreamExt, future, stream::FuturesUnordered};
use retrieval_conventions::{RetrieveAdmission, SingleflightRegistry};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};

const RETRIEVAL_SOURCE: &str = include_str!("../src/retrieval.rs");
const RETRIEVAL_CONVENTIONS_SOURCE: &str = include_str!("../src/retrieval_conventions.rs");
const RETRIEVAL_PROFILE_SOURCE: &str = include_str!("../src/retrieval_profile.rs");
const HLS_RETRIEVAL_SOURCE: &str = include_str!("../src/stream_hls/retrieval.rs");
const STREAM_SOURCE: &str = include_str!("../src/stream.rs");
const BZZ_STREAM_SOURCE: &str = include_str!("../src/bzz_stream.rs");
const RUNTIME_SOURCE: &str = include_str!("../src/lib.rs");
const HLS_STREAM_SOURCE: &str = include_str!("../src/stream_hls.rs");

enum ReadyItemOrCredit<T, C> {
    Item(T),
    Credit(C),
    Pending,
}

fn ready_item_before_credit<S, C>(
    active: &mut S,
    try_credit: impl FnOnce() -> Option<C>,
) -> ReadyItemOrCredit<S::Item, C>
where
    S: futures::Stream + Unpin,
{
    match active.next().now_or_never() {
        Some(Some(item)) => ReadyItemOrCredit::Item(item),
        Some(None) | None => try_credit()
            .map(ReadyItemOrCredit::Credit)
            .unwrap_or(ReadyItemOrCredit::Pending),
    }
}

fn startup_scout_nearest_incomplete_horizon(
    horizon_count: usize,
    current: usize,
    mut incomplete: impl FnMut(usize) -> bool,
) -> Option<usize> {
    (current..horizon_count).find(|&horizon| incomplete(horizon))
}

fn startup_scout_next_admission_horizon(
    horizon_count: usize,
    earliest_incomplete_horizon: usize,
    mut ready: impl FnMut(usize) -> bool,
) -> Option<usize> {
    (earliest_incomplete_horizon..horizon_count).find(|&horizon| ready(horizon))
}

#[test]
fn ready_failed_child_prevents_returned_credit_from_starting_another_child() {
    let production = HLS_RETRIEVAL_SOURCE
        .split("fn ready_item_before_credit")
        .nth(1)
        .and_then(|source| source.split("async fn scout_cancel_token_current").next())
        .expect("HLS ready-item-before-credit helper");
    assert!(production.contains("active.next().now_or_never()"));
    assert!(production.contains("Some(Some(item)) => ReadyItemOrCredit::Item(item)"));
    assert!(production.contains("Some(None) | None => try_credit()"));

    let mut active = FuturesUnordered::new();
    active.push(future::ready(None::<()>));
    let returned_credit_available = Cell::new(true);
    let launches = Cell::new(0_usize);

    let decision = ready_item_before_credit(&mut active, || {
        returned_credit_available.replace(false).then_some(())
    });
    let admissions_open = !matches!(decision, ReadyItemOrCredit::Item(None));
    if let ReadyItemOrCredit::Credit(()) = decision
        && admissions_open
    {
        if startup_scout_next_admission_horizon(3, 0, |horizon| horizon > 0).is_some() {
            launches.set(launches.get() + 1);
        }
    }

    assert!(!admissions_open, "the ready failed child closes admission");
    assert!(
        returned_credit_available.get(),
        "child completion must be observed before returned credit is acquired"
    );
    assert_eq!(launches.get(), 0, "no subsequent child may be launched");
}

#[test]
fn hls_startup_scout_priority_is_earliest_incomplete_horizon_first() {
    let helpers = HLS_RETRIEVAL_SOURCE
        .split("fn startup_scout_nearest_incomplete_horizon(")
        .nth(1)
        .and_then(|source| source.split("enum ReadyItemOrCredit").next())
        .expect("HLS startup scout priority helpers");
    assert!(helpers.contains("(current..horizon_count).find"));
    assert!(helpers.contains("(earliest_incomplete_horizon..horizon_count).find"));

    let ready = [true, true, true, true];
    let order = (0..4)
        .map(|_| {
            startup_scout_next_admission_horizon(ready.len(), 0, |horizon| ready[horizon])
                .expect("the earliest horizon remains ready")
        })
        .collect::<Vec<_>>();
    assert_eq!(order, vec![0, 0, 0, 0]);

    let mut ready = [false, true, true, true];
    assert_eq!(
        startup_scout_next_admission_horizon(ready.len(), 0, |horizon| ready[horizon]),
        Some(1)
    );
    ready[0] = true;
    assert_eq!(
        startup_scout_next_admission_horizon(ready.len(), 0, |horizon| ready[horizon]),
        Some(0)
    );
    ready[0] = false;
    ready[1] = false;
    assert_eq!(
        startup_scout_next_admission_horizon(ready.len(), 0, |horizon| ready[horizon]),
        Some(2)
    );
}

#[test]
fn hls_startup_scout_deadline_advances_only_after_a_horizon_finishes() {
    let mut pending = [true, false, false];
    let mut requested = [false, true, true];
    let mut active = [0_usize, 0, 0];
    let nearest = |pending: &[bool; 3], requested: &[bool; 3], active: &[usize; 3]| {
        startup_scout_nearest_incomplete_horizon(pending.len(), 0, |horizon| {
            pending[horizon] || requested[horizon] || active[horizon] != 0
        })
    };

    assert_eq!(nearest(&pending, &requested, &active), Some(0));
    pending[0] = false;
    requested[0] = true;
    assert_eq!(nearest(&pending, &requested, &active), Some(0));
    requested[0] = false;
    active[0] = 2;
    assert_eq!(nearest(&pending, &requested, &active), Some(0));
    active[0] = 0;
    assert_eq!(nearest(&pending, &requested, &active), Some(1));

    let ready = [false, true, true];
    let active = [0_usize, 2, 0];
    let earliest = startup_scout_nearest_incomplete_horizon(ready.len(), 0, |horizon| {
        ready[horizon] || active[horizon] != 0
    })
    .expect("the completed first horizon promotes its nearest successor");
    assert_eq!(earliest, 1);
    assert_eq!(
        startup_scout_next_admission_horizon(ready.len(), earliest, |horizon| ready[horizon]),
        Some(1)
    );
}

#[test]
fn hls_retrieval_ownership_boundary_is_strict_and_one_way() {
    assert!(!RETRIEVAL_SOURCE.to_ascii_lowercase().contains("hls"));
    for forbidden in [
        "StartupRawScout",
        "RawFetchLeaderCompletion",
        "crate::stream_hls",
    ] {
        assert!(
            !RETRIEVAL_SOURCE.contains(forbidden),
            "generic retrieval source contains HLS-owned symbol {forbidden}"
        );
    }
    assert!(!RETRIEVAL_CONVENTIONS_SOURCE.contains("startup_scout"));
    for forbidden in ["__weeb3Hls", "web_sys::window", "js_sys::Reflect"] {
        assert!(
            !RETRIEVAL_PROFILE_SOURCE.contains(forbidden),
            "generic retrieval profile contains HLS browser adapter {forbidden}"
        );
    }
    for (name, source) in [
        ("stream", STREAM_SOURCE),
        ("bzz_stream", BZZ_STREAM_SOURCE),
        ("runtime", RUNTIME_SOURCE),
    ] {
        for forbidden in ["StartupRawScout", "startup_raw_seed"] {
            assert!(
                !source.contains(forbidden),
                "generic forwarding layer {name} contains {forbidden}"
            );
        }
    }

    assert!(HLS_RETRIEVAL_SOURCE.contains("pub(super) struct StartupRawScout"));
    assert!(HLS_RETRIEVAL_SOURCE.contains("enum HlsRawFetchLifecycle"));
    assert!(HLS_RETRIEVAL_SOURCE.contains("impl RawFetchLifecycle for HlsRawFetchLifecycle"));
    assert!(HLS_RETRIEVAL_SOURCE.contains("__weeb3HlsRawStartupProfileEnabled"));
    assert!(HLS_RETRIEVAL_SOURCE.contains("__weeb3HlsRetrieveProfileEnabled"));
    assert!(HLS_RETRIEVAL_SOURCE.contains("crate::{"));
    assert!(HLS_RETRIEVAL_SOURCE.contains("retrieval::{"));
}

#[test]
fn scout_registration_is_credit_first_and_reuses_the_ordinary_raw_fetch_path() {
    let helper = HLS_RETRIEVAL_SOURCE
        .split("fn queue_startup_raw_scout_child(")
        .nth(1)
        .and_then(|source| {
            source
                .split("pub(super) async fn scout_data_ranges_cache_only_cancellable(")
                .next()
        })
        .expect("startup raw scout child helper");
    assert!(helper.contains("queue_decoded_join_child_cancellable("));
    assert!(helper.contains("Box::new(HlsRawFetchLifecycle::Scout(credit))"));
    assert!(helper.contains("let decoded ="));
    assert!(helper.contains("decoded.await"));
    assert!(helper.contains("chunk.span == child.limit"));
    assert!(
        !helper[..helper
            .find("queue_decoded_join_child_cancellable(")
            .unwrap()]
            .contains(".await")
    );

    let generic_facade = RETRIEVAL_SOURCE
        .split("pub(crate) fn queue_decoded_join_child_cancellable(")
        .nth(1)
        .and_then(|source| source.split("#[inline]\nfn decryption_segment_key").next())
        .expect("generic decoded-join child facade");
    assert!(generic_facade.contains("queue_drained_raw_chunk("));
    assert!(generic_facade.contains("Some(lifecycle)"));
    assert!(generic_facade.contains("let _waiter_guard = waiter_guard;"));
    assert!(generic_facade.contains("result_in.recv().await"));
    assert!(generic_facade.contains("cached_decoded_chunk(&reference)"));

    let traversal = HLS_RETRIEVAL_SOURCE
        .split("pub(super) async fn scout_data_ranges_cache_only_cancellable(")
        .nth(1)
        .expect("cache-only scout traversal");
    let acquire = traversal
        .find("ready_item_before_credit(&mut active")
        .expect("ready child priority before credit acquisition");
    let register = traversal[acquire..]
        .find("admit_startup_raw_scout_child(")
        .map(|offset| acquire + offset)
        .expect("scout child registration");
    assert!(acquire < register);
    assert!(traversal.contains("scout.try_acquire_credit()"));
    assert!(traversal.contains("match select(next_child, next_credit).await"));
    assert!(traversal.contains("if !scout.new_admissions_open()"));
    assert!(traversal.contains("while active.next().await.is_some() {}"));
    assert!(!traversal.contains("dispatch_group_parity("));
    assert!(!traversal.contains("dispatch_group_recovery("));
    assert!(!traversal.contains("reconstruct_data_indices("));
    assert!(traversal.contains("chunk.span != child_limit"));
    assert!(traversal.contains("*end >= root.span"));
    assert!(!traversal.contains("payload_end_inclusive.min(root.span"));
    assert!(!traversal.contains("-> bool"));
    assert!(!traversal.contains("let mut complete"));
    assert!(traversal.contains("startup_scout_nearest_incomplete_horizon("));
    assert!(traversal.contains("startup_scout_child_admissible("));
    assert!(traversal.contains("pop_startup_scout_child("));
    assert!(traversal.contains("let mut active_counts = vec![0_usize; payload_ranges.len()];"));
    assert!(traversal.contains("let mut earliest_incomplete_horizon = 0_usize;"));
    assert!(!traversal.contains("next_surplus_horizon"));
    assert!(traversal.contains("|| active_counts[horizon] != 0"));
    assert!(traversal.contains("finish_startup_raw_scout_child("));
    assert!(traversal.contains("admit_startup_raw_scout_child("));
    assert!(!traversal.contains("pop_startup_scout_child_round_robin("));
    assert!(!traversal.contains("let mut next_horizon"));
    assert!(traversal.contains("while active.next().await.is_some() {}"));
    assert!(!traversal.contains("return false;"));
    assert!(!traversal.contains("return complete"));
    assert!(!traversal.contains("let current = join_cancel_token_current"));

    let settlement = HLS_RETRIEVAL_SOURCE
        .split("fn finish_startup_raw_scout_child(")
        .nth(1)
        .and_then(|source| source.split("/// Best-effort cache-only traversal").next())
        .expect("central startup child settlement");
    let active_release = settlement.find("checked_sub(1)").expect("active release");
    let result_check = settlement
        .find("let (Some(node), Some(queue))")
        .expect("terminal child result check");
    assert!(active_release < result_check);
    assert!(settlement[result_check..].contains("scout.close_new_admissions();"));
}

#[test]
fn raw_flight_owns_cache_refs_and_completion_before_waiter_filtering() {
    let source = RETRIEVAL_SOURCE;
    let shared = source
        .split("struct RawFetchShared {")
        .nth(1)
        .and_then(|source| source.split("type RawFetchFlights").next())
        .expect("raw shared flight state");
    assert!(shared.contains("cache_references: Rc<RefCell<HashSet<Vec<u8>>>>"));

    let completion = source
        .split("fn complete_raw_fetch(")
        .nth(1)
        .and_then(|source| source.split("fn queue_drained_raw_chunk(").next())
        .expect("raw fetch completion");
    let cache = completion
        .find("for reference in flight.shared.cache_references.borrow().iter()")
        .expect("flight-owned cache population");
    let waiter_filter = completion
        .find("for waiter in flight.waiters")
        .expect("logical waiter delivery");
    assert!(cache < waiter_filter);
    assert!(completion[cache..waiter_filter].contains("remember_raw_chunk("));

    let dispatch = source
        .split("fn queue_drained_raw_chunk(")
        .nth(1)
        .and_then(|source| source.split("fn decryption_segment_key(").next())
        .expect("ordinary raw dispatch");
    assert!(dispatch.contains("remember_cache_reference(cache_reference.as_ref())"));
    assert!(dispatch.contains("if !registration.leader"));
    assert!(dispatch.contains(".try_send(crate::ChunkRetrieveRequest"));
    let send_failure = dispatch
        .find(".is_err()")
        .expect("failed queue send handling");
    assert!(dispatch[send_failure..].contains("complete_raw_fetch("));
    assert!(dispatch[send_failure..].contains("lifecycle.complete(flight_id, canonical_cac)"));
}

#[test]
fn zero_waiter_shared_flight_retains_all_refs_until_late_cache_completion() {
    #[derive(Clone)]
    struct Shared {
        admission: RetrieveAdmission,
        refs: Rc<RefCell<HashSet<Vec<u8>>>>,
    }

    let key = ("runtime", "cac", "media", 41_u64, None::<u64>);
    let mut flights = SingleflightRegistry::default();
    let first = flights.register(key, "prefetch", || Shared {
        admission: RetrieveAdmission::new(),
        refs: Rc::new(RefCell::new(HashSet::new())),
    });
    first.shared.refs.borrow_mut().insert(vec![0x11; 32]);
    let follower = flights.register(key, "ordinary-w3", || unreachable!());
    assert!(!follower.leader);
    follower.shared.refs.borrow_mut().insert(vec![0x22; 64]);
    assert_eq!(first.flight_id, follower.flight_id);

    assert!(
        flights
            .remove_waiter(&key, first.flight_id, first.waiter_id)
            .is_none()
    );
    let shared = flights
        .remove_waiter(&key, follower.flight_id, follower.waiter_id)
        .expect("last logical waiter releases, but does not remove, shared flight");
    shared.admission.close();

    let completed = flights
        .take(&key, first.flight_id)
        .expect("late physical completion still owns its cache refs");
    assert!(completed.waiters.is_empty());
    let raw = vec![7_u8; 64];
    let mut cache = HashMap::new();
    for reference in completed.shared.refs.borrow().iter() {
        cache.insert(reference.clone(), raw.clone());
    }
    assert_eq!(cache.get(&vec![0x11; 32]), Some(&raw));
    assert_eq!(cache.get(&vec![0x22; 64]), Some(&raw));
}

#[test]
fn controller_retirement_does_not_close_an_active_raw_waiter_guard() {
    let controller = RetrieveAdmission::new();
    let waiter = RetrieveAdmission::new();
    let waiter_guard = waiter.close_on_drop();

    controller.close();
    assert!(!controller.is_open());
    assert!(waiter.is_open());

    drop(waiter_guard);
    assert!(!waiter.is_open());
}

#[test]
fn seed_and_scout_credit_ownership_follow_only_new_raw_leaders() {
    let group = RETRIEVAL_SOURCE
        .split("async fn fetch_data_group_indices_streaming(")
        .nth(1)
        .and_then(|source| source.split("struct TraversalNode").next())
        .expect("ordinary group fetch");
    let cache_hit = group
        .find("if let Some(chunk) = cached_decoded_chunk(reference)")
        .expect("decoded terminal hit");
    let queue = group[cache_hit..]
        .find("queue_initial_data_shard(index, RetrieveHedgeDemand::Ordinary)")
        .map(|offset| cache_hit + offset)
        .expect("terminal raw registration");
    assert!(group[cache_hit..queue].contains("continue;"));
    let initial_queue = group
        .split("let queue_initial_data_shard =")
        .nth(1)
        .and_then(|source| source.split("if static_rolling_candidate {").next())
        .expect("terminal raw registration helper");
    assert!(initial_queue.contains("queue_drained_raw_chunk("));
    assert!(initial_queue.contains("terminal_lifecycle_factory"));
    assert!(initial_queue.contains(".map(RawFetchLifecycleFactory::create)"));

    let dispatch = RETRIEVAL_SOURCE
        .split("fn queue_drained_raw_chunk(")
        .nth(1)
        .and_then(|source| source.split("fn decryption_segment_key(").next())
        .expect("raw queue");
    let cache_return = dispatch
        .find("if let Some(chunk) = cached_raw_chunk(reference)")
        .expect("raw cache hit");
    let registration = dispatch
        .find("let registration =")
        .expect("singleflight registration");
    let follower_return = dispatch
        .find("if !registration.leader")
        .expect("singleflight follower");
    let producer = dispatch[follower_return..]
        .find("spawn_local(async move")
        .map(|offset| follower_return + offset)
        .expect("leader completion owner");
    assert!(
        cache_return < registration && registration < follower_return && follower_return < producer
    );
    assert!(dispatch[producer..].contains("lifecycle.complete(flight_id, canonical_cac)"));

    let completion = HLS_RETRIEVAL_SOURCE
        .split("impl HlsRawFetchLifecycle {")
        .nth(1)
        .and_then(|source| {
            source
                .split("impl RawFetchLifecycle for HlsRawFetchLifecycle")
                .next()
        })
        .expect("credit completion");
    assert!(completion.contains("Self::Seed(scout) if canonical_cac => {"));
    assert!(completion.contains("scout.mint_credit();"));
    assert!(completion.contains("Self::Seed(_) => {}"));
    assert!(completion.contains("Self::Scout(mut credit) => {"));
    assert!(completion.contains("credit.leader_active = false;"));
    assert!(completion.contains("drop(credit);"));
}

#[test]
fn raw_profile_attribution_is_one_time_opt_in_and_runtime_silent_when_disabled() {
    let source = HLS_RETRIEVAL_SOURCE;
    assert_eq!(
        source.matches("StartupRawProfileTrace::activate()").count(),
        1
    );
    let scout_new = source
        .split("impl StartupRawScout {")
        .nth(1)
        .and_then(|source| source.split("pub(super) fn new() -> Self {").nth(1))
        .and_then(|source| source.split("pub(super) fn seed_lifecycle_factory").next())
        .expect("startup scout constructor");
    assert!(scout_new.contains("profile_trace: StartupRawProfileTrace::activate()"));

    let activation = source
        .split("fn activate() -> Option<Rc<Self>> {")
        .nth(1)
        .and_then(|source| source.split("fn set_number(").next())
        .expect("raw profile activation");
    assert!(activation.contains("STARTUP_RAW_PROFILE_FLAG"));
    assert!(activation.contains(".as_bool()"));
    assert!(activation.contains("enabled.then(||"));
    assert!(!activation.contains("interface_log"));
    assert!(!activation.contains("console"));

    let trace = source
        .split("const STARTUP_RAW_PROFILE_FLAG:")
        .nth(1)
        .and_then(|source| {
            source
                .split("#[derive(Clone)]\npub(super) struct StartupRawScout")
                .next()
        })
        .expect("bounded raw profile trace");
    assert!(trace.contains("const STARTUP_RAW_PROFILE_EVENT_CAP: u64 = 2_048;"));
    assert!(trace.contains(
        "const STARTUP_RAW_PROFILE_DATA_EVENT_CAP: u64 = STARTUP_RAW_PROFILE_EVENT_CAP - 1;"
    ));
    assert!(trace.contains("if emitted >= STARTUP_RAW_PROFILE_DATA_EVENT_CAP"));
    assert!(trace.contains("self.emit_terminal(\"cap-reached\""));
    assert!(trace.contains("self.emit_terminal(\"dispatch-failed\""));
    assert!(trace.contains("self.publish_terminal_reason(reason);"));
    assert!(trace.contains("self.publish_terminal_reason(\"dispatch-failed\");"));
    assert!(trace.contains("\"trace-terminal\""));
    assert!(trace.contains("CustomEvent::new_with_event_init_dict(STARTUP_RAW_PROFILE_EVENT"));
    assert!(trace.contains("\"raw-singleflight\""));
    assert!(trace.contains("\"bee_peer_attempts\""));
    assert!(trace.contains("\"retrieval_permits\""));
    assert!(trace.contains("Self::set_number(&detail, \"schema_version\", 3);"));
    assert!(
        trace.contains("Self::set_optional_u64_string(&detail, \"raw_flight_id\", raw_flight_id);")
    );
}

#[test]
fn raw_profile_group_metadata_is_opt_in_single_touch_and_immutable_per_scout_child() {
    let source = HLS_RETRIEVAL_SOURCE;
    let expansion = source
        .split("pub(super) async fn scout_data_ranges_cache_only_cancellable(")
        .nth(1)
        .expect("startup raw scout expansion");
    let profile_branch = expansion
        .find("if scout.profile_trace.is_some()")
        .expect("profile-only cache classification branch");
    let combined = expansion
        .find("match requested_shard_cache(&reference)")
        .expect("single-touch decoded/raw classification");
    let legacy = expansion
        .find("cached_decoded_chunk(&reference)")
        .expect("disabled legacy decoded-cache path");
    assert!(profile_branch < combined && combined < legacy);
    assert!(expansion[combined..legacy].contains("} else {"));
    assert_eq!(
        expansion
            .matches("requested_shard_cache(&reference)")
            .count(),
        1,
        "enabled attribution must classify each requested child with one combined cache touch"
    );
    assert!(expansion.contains("profile_group.get_or_insert_with(||"));
    assert!(expansion.contains("trace.new_scout_group("));
    assert!(source.contains("requested_first_index: requested_first_index as u64"));
    assert!(source.contains("requested_last_index: requested_last_index as u64"));
    assert!(expansion.contains("group.finalize_cache_classification("));
    let finalize = expansion
        .find("group.finalize_cache_classification(")
        .expect("profile cache classification finalization");
    let admission = expansion
        .find("while startup_scout_child_admissible(")
        .expect("first scout admission loop");
    assert!(
        finalize < admission,
        "group cache counts must finalize before emission"
    );

    let credit = source
        .split("struct StartupRawScoutCredit {")
        .nth(1)
        .and_then(|source| source.split("enum HlsRawFetchLifecycle").next())
        .expect("scout credit metadata owner");
    assert!(credit.contains("profile_child: Option<StartupRawProfileChild>"));
    assert!(credit.contains("fn assign_profile_child("));

    let completion = source
        .split("impl HlsRawFetchLifecycle {")
        .nth(1)
        .and_then(|source| {
            source
                .split("impl RawFetchLifecycle for HlsRawFetchLifecycle")
                .next()
        })
        .expect("raw completion attribution");
    assert!(completion.contains("Self::Seed(_) => None"));
    assert!(completion.contains("Self::Scout(credit) => credit.profile_child.as_ref()"));
    assert!(completion.contains("profile_child.as_ref(),"));
}

#[test]
fn raw_profile_classifies_cache_join_and_led_without_changing_dispatch_order() {
    let dispatch = RETRIEVAL_SOURCE
        .split("fn queue_drained_raw_chunk(")
        .nth(1)
        .and_then(|source| source.split("fn decryption_segment_key(").next())
        .expect("raw queue");
    let cache = dispatch
        .find("lifecycle.finish_registration(RawFetchRegistration::Cached, None)")
        .expect("Cached classification");
    let registration = dispatch
        .find("let registration =")
        .expect("singleflight registration");
    let joined = dispatch
        .find("lifecycle.finish_registration(RawFetchRegistration::Joined, Some(flight_id))")
        .expect("Joined classification");
    let led = dispatch
        .find("lifecycle.leader_selected()")
        .expect("Led classification");
    let send = dispatch
        .find(".try_send(crate::ChunkRetrieveRequest")
        .expect("ordinary logical retrieve enqueue");
    let accepted = dispatch
        .find("lifecycle.leader_registered(flight_id, true)")
        .expect("accepted leader dispatch attribution");
    let producer = dispatch[accepted..]
        .find("spawn_local(async move")
        .map(|offset| accepted + offset)
        .expect("detached raw producer");
    assert!(cache < registration && registration < joined && joined < led);
    assert!(led < send && send < accepted && accepted < producer);
    assert!(dispatch.contains("lifecycle.leader_registered(flight_id, false)"));
    assert!(dispatch[producer..].contains("lifecycle.complete(flight_id, canonical_cac)"));

    let result = RETRIEVAL_SOURCE
        .split("struct RawFetchResult {")
        .nth(1)
        .and_then(|source| source.split("pub(crate) trait RawFetchLifecycle").next())
        .expect("raw result transport");
    assert!(
        !result.contains("flight_id"),
        "flight attribution must not alter result/receiver transport"
    );

    let scout_child = HLS_RETRIEVAL_SOURCE
        .split("fn queue_startup_raw_scout_child(")
        .nth(1)
        .and_then(|source| {
            source
                .split("pub(super) async fn scout_data_ranges_cache_only_cancellable(")
                .next()
        })
        .expect("startup raw child");
    assert!(scout_child.contains("credit.assign_horizon(horizon.saturating_add(1));"));
    assert!(scout_child.contains("Box::new(HlsRawFetchLifecycle::Scout(credit))"));
}

#[test]
fn raw_profile_counters_are_raii_bound_and_conserved_in_every_credit_exit() {
    let source = HLS_RETRIEVAL_SOURCE;
    let trace = source
        .split("impl StartupRawProfileTrace {")
        .nth(1)
        .and_then(|source| {
            source
                .split("#[derive(Clone)]\npub(super) struct StartupRawScout")
                .next()
        })
        .expect("raw trace counters");
    for field in [
        "raw_leader_dispatches",
        "raw_leader_completions",
        "raw_leaders_active",
        "logical_retrieve_dispatches",
        "credits_minted",
        "credits_available",
        "credits_held",
        "credits_discarded",
        "scout_active",
    ] {
        assert!(
            trace.contains(field),
            "missing raw attribution counter {field}"
        );
    }
    assert!(trace.contains("fn credit_acquired(&self)"));
    assert!(trace.contains("fn credit_released(&self, returned: bool)"));
    assert!(trace.contains("fn available_credit_discarded(&self)"));
    assert!(trace.contains("fn raw_leader_completed(&self, scout: bool)"));

    let credit = source
        .split("struct StartupRawScoutCredit {")
        .nth(1)
        .and_then(|source| source.split("enum HlsRawFetchLifecycle").next())
        .expect("credit RAII");
    assert!(credit.contains("leader_active: bool"));
    assert!(credit.contains("impl Drop for StartupRawScoutCredit"));
    assert!(credit.contains("self.scout.return_credit();"));

    let scout = source
        .split("impl StartupRawScout {")
        .nth(1)
        .and_then(|source| source.split("fn mint_credit(&self)").next())
        .expect("startup scout close path");
    let close = scout
        .find("self.admission.close();")
        .expect("admission close");
    let drain = scout
        .find("while self.credit_in.try_recv().is_ok()")
        .expect("synchronous queued-credit drain");
    let discard = scout
        .find("trace.available_credit_discarded();")
        .expect("drained-credit accounting");
    let snapshot = scout
        .find("trace.admission_closed();")
        .expect("observable close snapshot");
    assert!(close < drain && drain < discard && discard < snapshot);
    assert!(trace.contains("self.emit(\"admission-close\""));
    assert!(trace.contains("self.emit_terminal(\"admission-closed\""));
    assert!(trace.contains("self.credits_available.get() == 0"));
    assert!(trace.contains("self.credits_held.get() == 0"));
}

#[test]
fn exact_foreground_window_zero_is_the_only_raw_credit_seed() {
    let range = STREAM_SOURCE;
    let window = range
        .split("async fn read_range_window(")
        .nth(1)
        .and_then(|source| {
            source
                .split("pub(crate) async fn read_cached_hls_range(")
                .next()
        })
        .expect("range singleflight window");
    let role = window.find("match FETCH_CACHE.with").expect("range role");
    let leader = window
        .find("if let Some(load_id) = leader_load_id")
        .expect("physical range leader");
    let seed = window
        .find("raw_fetch_lifecycle_factory,")
        .expect("neutral lifecycle factory passed to Bee traversal");
    assert!(role < leader && leader < seed);
    assert!(!window[role..leader].contains("raw_fetch_lifecycle_factory.clone()"));

    let hls = HLS_STREAM_SOURCE;
    let handoff = hls
        .split("fn hls_beginning_raw_seed(")
        .nth(1)
        .and_then(|source| {
            source
                .split("fn mark_hls_beginning_foreground_zero_requested(")
                .next()
        })
        .expect("ordered raw-scout handoff");
    assert!(handoff.contains("if !foreground"));
    assert!(handoff.contains("gate.target_windows <= 1"));
    assert!(handoff.contains("!gate.foreground_zero_requested"));
    assert!(handoff.contains("gate.phase != HlsBeginningPrefixPhase::Supplying"));
    assert!(handoff.contains("start == 0 && end == expected_end"));
    assert!(handoff.contains("gate.raw_scout"));
    assert!(handoff.contains("StartupRawScout::seed_lifecycle_factory"));
    assert!(!handoff.contains("close_raw_scout"));
    assert!(!handoff.contains("hls_range_intersects_window"));

    let retrieve = hls
        .split("async fn retrieve_hls_payload_range(")
        .nth(1)
        .and_then(|source| {
            source
                .split("async fn latest_hls_feed_payload_startup(")
                .next()
        })
        .expect("HLS range retrieval");
    let seed = retrieve
        .find("let startup_raw_seed = hls_beginning_raw_seed(")
        .expect("synchronous seed boundary");
    let registration = retrieve[seed..]
        .find("let bytes = read_cached_hls_range(")
        .map(|offset| seed + offset)
        .expect("next range registration");
    assert!(retrieve[seed..registration].contains("foreground_range,"));
    assert!(!retrieve[seed..registration].contains(".await"));
}

#[test]
fn raw_coordinator_targets_every_derived_future_horizon_without_success_close() {
    let hls = HLS_STREAM_SOURCE;
    let configure = hls
        .split("fn ensure_hls_beginning_prefix_configured(")
        .nth(1)
        .and_then(|source| source.split("fn start_hls_beginning_raw_scout(").next())
        .expect("startup scout configuration");
    assert!(configure.contains("if target_windows > 1"));
    assert!(configure.contains("&& !gate.coordinator_started"));
    assert!(configure.contains("&& gate.raw_scout.is_none()"));

    let start = hls
        .split("fn start_hls_beginning_raw_scout(")
        .nth(1)
        .and_then(|source| source.split("fn hls_beginning_raw_seed(").next())
        .expect("startup raw coordinator");

    let target_gate = start
        .find("if target_windows <= 1 || !scout.new_admissions_open()")
        .expect("protected-prefix/current admission gate");
    let horizons = start
        .find("let payload_ranges = (1..target_windows)")
        .expect("derived future horizons");
    let bounds = start[horizons..]
        .find("hls_beginning_prefix_window_bounds(payload_size, window)")
        .map(|offset| horizons + offset)
        .expect("exact horizon bounds");
    let cardinality = start[bounds..]
        .find("payload_ranges.len() != target_windows.saturating_sub(1)")
        .map(|offset| bounds + offset)
        .expect("one horizon per nonzero critical window");
    let exact_root = start
        .find("root.span != payload_size")
        .expect("exact root span");
    let coordinator = start[exact_root..]
        .find("scout_data_ranges_cache_only_cancellable(")
        .map(|offset| exact_root + offset)
        .expect("multi-horizon traversal");

    assert!(
        target_gate < horizons
            && horizons < bounds
            && bounds < cardinality
            && cardinality < exact_root
            && exact_root < coordinator
    );
    assert_eq!(
        start
            .matches("scout_data_ranges_cache_only_cancellable(")
            .count(),
        1
    );
    assert!(start[coordinator..].contains("payload_ranges,"));
    assert!(!start[coordinator..].contains("scout.close_new_admissions()"));
}

#[test]
fn scout_uses_exact_stream_generation_and_retirement_only_closes_new_admissions() {
    let hls = HLS_STREAM_SOURCE;
    let start = hls
        .split("fn start_hls_beginning_raw_scout(")
        .nth(1)
        .and_then(|source| source.split("fn hls_beginning_raw_seed(").next())
        .expect("startup scout launcher");
    assert!(
        start
            .contains("stream_retrieve_cancel_token(HLS_STREAM_KEY.to_string(), stamp.generation)")
    );
    assert!(start.contains("cached_decoded_data_root(&data_reference)"));
    assert!(start.contains("root.span != payload_size"));
    assert!(!start.contains("retrieve_decoded_data_root_cancellable("));
    assert!(start.contains("scout_data_ranges_cache_only_cancellable("));
    assert!(!start.contains("cancel: None"));

    let admission = HLS_RETRIEVAL_SOURCE
        .split("impl StartupRawScout {")
        .nth(1)
        .and_then(|source| source.split("struct StartupRawScoutCredit").next())
        .expect("scout admission");
    assert!(admission.contains("self.admission.close();"));
    assert!(admission.contains("self.admission.wait_closed()"));
    assert!(!admission.contains("waiter_admission"));

    let helper = HLS_RETRIEVAL_SOURCE
        .split("fn queue_startup_raw_scout_child(")
        .nth(1)
        .and_then(|source| {
            source
                .split("pub(super) async fn scout_data_ranges_cache_only_cancellable(")
                .next()
        })
        .expect("scout raw helper");
    assert!(helper.contains("cancel,"));
    assert!(helper.contains("Box::new(HlsRawFetchLifecycle::Scout(credit))"));
}
