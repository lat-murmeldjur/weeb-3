#![allow(dead_code)]

#[path = "../src/retrieval_profile.rs"]
mod retrieval_profile;

use retrieval_profile::{
    ROLLING_GROUP_TRACE_CAP, RetrieveAttemptOutcome, RollingGroupProfileEvent,
    RollingGroupProfileInit, RollingGroupRegistration, RollingGroupTerminalReason,
    test_begin_permit_wait, test_complete_detached, test_complete_immediate,
    test_finalize_rolling_group_trace, test_log2_histogram, test_managed_to_ordinary_promotion,
    test_permit_aborted, test_permit_acquired, test_physical_attempt, test_profile,
    test_reject_before_permit, test_request, test_rolling_group, test_snapshot, test_timed_out,
};
use std::{fs, path::PathBuf};

fn source(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
}

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .and_then(|(_, tail)| tail.split_once(end))
        .map(|(section, _)| section)
        .unwrap_or_else(|| panic!("missing source section between {start:?} and {end:?}"))
}

fn rolling_init() -> RollingGroupProfileInit {
    RollingGroupProfileInit {
        anchor_at_ms: 100.0,
        requested_count: 119,
        data_count: 119,
        parity_count: 9,
        decoded_raw_count: 4,
        decoded_only_count: 2,
        miss_count: 113,
        static_candidate: true,
        dynamic_eligible: true,
        initial_cached: 1,
        initial_joined: 2,
        initial_led: 110,
        initial_active: 112,
        initial_successes: 4,
    }
}

#[test]
fn immediate_lifecycle_balances_queue_permit_attempt_and_delivery() {
    let state = test_profile(0.0, 4);
    let request = test_request(&state, 10.0, true);
    request.enqueue_result(true);
    request.dequeued();
    test_begin_permit_wait(&request, 20.0);
    let permit = test_permit_acquired(&request, 30.0);
    let attempt = test_physical_attempt(&request, 31.0);
    test_complete_immediate(attempt, RetrieveAttemptOutcome::ValidCac, 40.0, true);
    request.logical_completed(true);
    request.delivery_result(true);
    drop(permit);

    let snapshot = test_snapshot(&state, 50.0);
    assert_eq!(snapshot.tickets_created, 1);
    assert_eq!(snapshot.permit_capacity, 4);
    assert_eq!(snapshot.enqueue_accepted, 1);
    assert_eq!(snapshot.queue_current, 0);
    assert_eq!(snapshot.logical_completed, 1);
    assert_eq!(snapshot.logical_nonempty, 1);
    assert_eq!(snapshot.permit_wait_acquired, 1);
    assert_eq!(snapshot.permits_current, 0);
    assert_eq!(snapshot.permits_released, 1);
    assert_eq!(snapshot.physical_dispatched, 1);
    assert_eq!(snapshot.physical_immediate_completed, 1);
    assert_eq!(snapshot.immediate_outcomes.valid_cac, 1);
    assert_eq!(snapshot.immediate_result_send_succeeded, 1);
    assert_eq!(snapshot.by_scope.stream_scoped.enqueue_accepted, 1);
    assert_eq!(snapshot.by_scope.stream_scoped.queue_dequeued, 1);
    assert_eq!(snapshot.by_scope.stream_scoped.permit_acquired, 1);
    assert_eq!(snapshot.by_scope.stream_scoped.physical_dispatched, 1);
    assert_eq!(
        snapshot.by_scope.stream_scoped.physical_immediate_completed,
        1
    );
    assert_eq!(snapshot.by_scope.unscoped.enqueue_accepted, 0);
    assert_eq!(snapshot.queue_to_permit_acquired_ms.sum_ms, 20.0);
    assert_eq!(snapshot.permit_wait_acquired_ms.sum_ms, 10.0);
    assert_eq!(snapshot.immediate_attempt_ms.sum_ms, 9.0);
    assert_eq!(
        snapshot
            .conservation
            .accepted_minus_queue_dequeued_forward_failed,
        0
    );
    assert_eq!(
        snapshot
            .conservation
            .logical_dequeued_minus_active_completed,
        0
    );
    assert_eq!(
        snapshot
            .conservation
            .permit_wait_started_minus_current_acquired_aborted,
        0
    );
    assert_eq!(
        snapshot
            .conservation
            .permits_acquired_minus_current_released,
        0
    );
    assert_eq!(
        snapshot
            .conservation
            .physical_dispatched_minus_active_immediate_timed_out,
        0
    );
    assert_eq!(snapshot.conservation.immediate_completed_minus_outcomes, 0);
    assert_eq!(
        snapshot.conservation.immediate_completed_minus_result_sends,
        0
    );
    assert_eq!(snapshot.conservation.logical_completed_minus_deliveries, 0);
}

#[test]
fn timeout_transfers_one_attempt_to_detached_until_late_settlement() {
    let state = test_profile(0.0, 4);
    let request = test_request(&state, 1.0, true);
    request.enqueue_result(true);
    request.dequeued();
    test_begin_permit_wait(&request, 2.0);
    let permit = test_permit_acquired(&request, 3.0);
    let attempt = test_physical_attempt(&request, 100.0);
    let detached = test_timed_out(attempt, 10_100.0, false);
    request.logical_completed(false);
    request.delivery_result(true);
    drop(permit);

    let timed_out = test_snapshot(&state, 10_101.0);
    assert_eq!(timed_out.physical_active, 0);
    assert_eq!(timed_out.physical_timed_out, 1);
    assert_eq!(timed_out.detached_outstanding, 1);
    assert_eq!(timed_out.detached_completed, 0);
    assert_eq!(timed_out.timeout_result_send_failed, 1);
    assert_eq!(
        timed_out
            .conservation
            .timed_out_minus_detached_outstanding_completed,
        0
    );
    assert_eq!(timed_out.conservation.timed_out_minus_result_sends, 0);

    test_complete_detached(detached, RetrieveAttemptOutcome::ChannelClosed, 10_600.0);
    let settled = test_snapshot(&state, 10_601.0);
    assert_eq!(settled.detached_outstanding, 0);
    assert_eq!(settled.detached_completed, 1);
    assert_eq!(settled.detached_outcomes.channel_closed, 1);
    assert_eq!(settled.detached_after_timeout_ms.sum_ms, 500.0);
    assert_eq!(settled.detached_total_attempt_ms.sum_ms, 10_500.0);
    assert_eq!(
        settled
            .conservation
            .timed_out_minus_detached_outstanding_completed,
        0
    );
    assert_eq!(settled.conservation.detached_completed_minus_outcomes, 0);
}

#[test]
fn aborted_wait_and_pre_permit_rejection_are_separate_terminal_stages() {
    let state = test_profile(0.0, 4);
    let waiting = test_request(&state, 5.0, true);
    waiting.enqueue_result(true);
    waiting.dequeued();
    test_begin_permit_wait(&waiting, 15.0);
    test_permit_aborted(&waiting, 25.0);
    waiting.logical_completed(false);
    waiting.delivery_result(true);

    let rejected = test_request(&state, 30.0, false);
    rejected.enqueue_result(true);
    rejected.dequeued();
    test_reject_before_permit(&rejected, 39.0);
    rejected.logical_completed(false);
    rejected.delivery_result(false);

    let snapshot = test_snapshot(&state, 40.0);
    assert_eq!(snapshot.permit_wait_aborted, 1);
    assert_eq!(snapshot.pre_permit_rejected, 1);
    assert_eq!(snapshot.queue_to_permit_aborted_ms.sum_ms, 20.0);
    assert_eq!(snapshot.permit_wait_aborted_ms.sum_ms, 10.0);
    assert_eq!(snapshot.queue_to_pre_permit_rejection_ms.sum_ms, 9.0);
    assert_eq!(snapshot.logical_empty, 2);
    assert_eq!(snapshot.delivery_succeeded, 1);
    assert_eq!(snapshot.delivery_failed, 1);
    assert_eq!(snapshot.by_scope.stream_scoped.logical_completed, 1);
    assert_eq!(snapshot.by_scope.unscoped.logical_completed, 1);
    assert_eq!(
        snapshot
            .conservation
            .accepted_minus_queue_dequeued_forward_failed,
        0
    );
    assert_eq!(
        snapshot
            .conservation
            .permit_wait_started_minus_current_acquired_aborted,
        0
    );
}

#[test]
fn log2_histogram_has_explicit_power_of_two_upper_bounds() {
    let histogram = test_log2_histogram(&[0.0, 0.1, 1.0, 1.1, 2.0, 2.1, 4.0, 4.1]);
    assert_eq!(histogram.count, 8);
    assert_eq!(&histogram.buckets[..4], &[3, 2, 2, 1]);

    let state = test_profile(0.0, 4);
    let snapshot = test_snapshot(&state, 0.0);
    assert_eq!(
        &snapshot.log2_bucket_upper_bounds_ms[..5],
        &[1, 2, 4, 8, 16]
    );
}

#[test]
fn rejected_enqueue_resolution_is_idempotent_and_scoped() {
    let state = test_profile(0.0, 4);
    let request = test_request(&state, 1.0, false);
    request.enqueue_result(false);
    request.enqueue_result(false);
    let snapshot = test_snapshot(&state, 2.0);
    assert_eq!(snapshot.tickets_created, 1);
    assert_eq!(snapshot.enqueue_accepted, 0);
    assert_eq!(snapshot.enqueue_rejected, 1);
    assert_eq!(snapshot.by_scope.unscoped.enqueue_rejected, 1);
    assert_eq!(snapshot.by_scope.stream_scoped.enqueue_rejected, 0);
}

#[test]
fn two_hop_relay_preserves_one_ticket_and_accounts_forward_failure() {
    let state = test_profile(0.0, 4);
    let forwarded = test_request(&state, 1.0, true);
    forwarded.enqueue_result(true);
    forwarded.relay_result(true);
    forwarded.dequeued();
    test_reject_before_permit(&forwarded, 2.0);
    forwarded.logical_completed(false);
    forwarded.delivery_result(true);

    let failed = test_request(&state, 3.0, false);
    failed.enqueue_result(true);
    failed.relay_result(false);

    let snapshot = test_snapshot(&state, 4.0);
    assert_eq!(snapshot.tickets_created, 2);
    assert_eq!(snapshot.enqueue_accepted, 2);
    assert_eq!(snapshot.relay_forward_succeeded, 1);
    assert_eq!(snapshot.relay_forward_failed, 1);
    assert_eq!(snapshot.queue_dequeued, 1);
    assert_eq!(snapshot.queue_current, 0);
    assert_eq!(snapshot.by_scope.stream_scoped.tickets_created, 1);
    assert_eq!(snapshot.by_scope.unscoped.tickets_created, 1);
    assert_eq!(snapshot.by_scope.stream_scoped.relay_forward_succeeded, 1);
    assert_eq!(snapshot.by_scope.unscoped.relay_forward_failed, 1);
    assert_eq!(
        snapshot
            .conservation
            .accepted_minus_queue_dequeued_forward_failed,
        0
    );

    let lib = source("src/lib.rs");
    let sender = section(
        &lib,
        "impl ChunkRetrieveSender {",
        "impl Deref for ChunkRetrieveSender",
    );
    assert_eq!(sender.matches("request_for_enqueue(").count(), 1);
    assert!(sender.contains("let relayed = request.profile.is_some();"));
    assert!(sender.contains("if !relayed {"));
    assert!(sender.contains("profile.relay_result(result.is_ok());"));
    let relay = section(
        &lib,
        "let raw_chunk_handle = async {",
        "let resolve_bzz_handle = async {",
    );
    assert_eq!(
        relay
            .matches("chunk_retrieve_chan_outgoing.try_send(incoming_request)")
            .count(),
        1
    );
}

#[test]
fn inactive_generic_profile_returns_before_ticket_allocation_and_has_no_hls_adapter() {
    let profile = source("src/retrieval_profile.rs");
    let activation = section(
        &profile,
        "pub(crate) fn activate()",
        "pub(crate) fn request_for_enqueue(stream_scoped: bool)",
    );
    let already_active = activation
        .find("RetrievalProfileActivation::Active(_)")
        .expect("idempotent generic activation");
    let first_clock = activation
        .find("RetrievalProfileState::new(now_ms(), permit_capacity)")
        .or_else(|| activation.find("RetrievalProfileState::new(\n            now_ms(),"))
        .expect("generic activation clock");
    assert!(already_active < first_clock);

    let request = section(
        &profile,
        "pub(crate) fn request_for_enqueue(stream_scoped: bool)",
        "fn active_profile_state()",
    );
    let inactive = request
        .find("RetrievalProfileActivation::Inactive => return None")
        .expect("inactive return");
    let ticket = request
        .find("Some(active.request(now_ms(), stream_scoped))")
        .expect("active ticket allocation");
    assert!(inactive < ticket);

    let rolling_start = section(
        &profile,
        "pub(crate) fn rolling_group_started(",
        "pub(crate) fn snapshot_now()",
    );
    assert!(rolling_start.contains("active_profile_state().and_then("));
    assert!(!rolling_start.contains("now_ms()"));

    for forbidden in [
        "__weeb3Hls",
        "web_sys::window",
        "js_sys::Reflect",
        "Closure",
    ] {
        assert!(
            !profile.contains(forbidden),
            "generic retrieval profile contains HLS browser adapter {forbidden}"
        );
    }
}

#[test]
fn hls_adapter_checks_the_flag_before_activation_and_installs_both_getters() {
    let hls_retrieval = source("src/stream_hls/retrieval.rs");
    let adapter = section(
        &hls_retrieval,
        "pub(super) fn activate_retrieval_profile_if_requested()",
        "struct StartupRawProfileGroup",
    );
    let flag = adapter
        .find("RETRIEVAL_PROFILE_FLAG")
        .expect("HLS profile flag lookup");
    let disabled = adapter
        .find("if !enabled ||")
        .expect("disabled/idempotent return");
    let activate = adapter
        .find("retrieval_profile::activate();")
        .expect("generic profile activation");
    let snapshot = adapter
        .find("retrieval_profile::snapshot_now()")
        .expect("snapshot getter");
    let finalizer = adapter
        .find("retrieval_profile::finalize_rolling_group_trace_now()")
        .expect("rolling trace finalizer");
    assert!(flag < disabled && disabled < activate && activate < snapshot && snapshot < finalizer);
    assert!(hls_retrieval.contains("__weeb3HlsRetrieveProfileEnabled"));
    assert!(hls_retrieval.contains("__weeb3GetHlsRetrieveProfileSnapshot"));
    assert!(hls_retrieval.contains("__weeb3FinalizeHlsRetrieveRollingGroupTrace"));
    assert_eq!(adapter.matches(".forget();").count(), 2);
    assert_eq!(adapter.matches(".is_ok()").count(), 2);

    let hls_stream = source("src/stream_hls.rs");
    let fetch = section(
        &hls_stream,
        "pub(crate) async fn try_fetch_response(",
        "fn canonical_hls_bytes_resource(",
    );
    let bytes_activation = fetch
        .find("activate_retrieval_profile_if_requested();")
        .expect("bytes endpoint activation");
    let bytes_fetch = fetch
        .find("fetch_hls_bytes_response(")
        .expect("bytes endpoint retrieval");
    assert!(bytes_activation < bytes_fetch);
    assert_eq!(
        fetch
            .matches("activate_retrieval_profile_if_requested();")
            .count(),
        2,
        "both HLS bytes and feed routes activate before retrieval"
    );

    let attach = section(
        &hls_stream,
        "pub(crate) async fn attach_hls_feed_player(",
        "async fn open_hls_feed_view_generation(",
    );
    let attach_activation = attach
        .find("activate_retrieval_profile_if_requested();")
        .expect("direct player attachment activation");
    let first_setup = attach
        .find("reset_hls_codec_bootstrap();")
        .expect("first player setup action");
    assert!(attach_activation < first_setup);
}

#[test]
fn rolling_group_records_timed_admission_result_and_reconstruction_terminal() {
    let state = test_profile(10.0, 4);
    let mut group = test_rolling_group(&state, rolling_init()).expect("rolling group");
    group.parity_admitted(
        1_100.25,
        1_000,
        119,
        0,
        RollingGroupRegistration::Joined,
        112,
        113,
        4,
        5,
    );
    group.parity_result_at(1_250.5, 119, 0, false, 113, 112, 4, 4, 6);
    group.progress(4, 6, 112);
    group.close_at(
        RollingGroupTerminalReason::ReconstructThreshold,
        1_300.0,
        119,
        121,
        0,
    );
    group.finish_success_at(1_350.0, true);

    let snapshot = test_finalize_rolling_group_trace(&state, 1_400.0);
    assert_eq!(snapshot.groups_started, 1);
    assert_eq!(snapshot.groups_dynamic_eligible, 1);
    assert_eq!(snapshot.groups_active, 0);
    assert_eq!(snapshot.groups_terminal, 1);
    assert_eq!(snapshot.terminal_reconstruct_threshold, 1);
    assert_eq!(snapshot.parity_admitted, 1);
    assert_eq!(snapshot.parity_joined, 1);
    assert_eq!(snapshot.parity_valid, 0);
    assert_eq!(snapshot.parity_invalid, 1);
    assert_eq!(snapshot.events_attempted, 4);
    assert_eq!(snapshot.events.len(), 4);

    assert!(matches!(
        &snapshot.events[0],
        RollingGroupProfileEvent::Init {
            group_id: 1,
            requested_count: 119,
            data_count: 119,
            parity_count: 9,
            initial_cached: 1,
            initial_joined: 2,
            initial_led: 110,
            ..
        }
    ));
    assert!(matches!(
        &snapshot.events[1],
        RollingGroupProfileEvent::ParityAdmission {
            group_id: 1,
            decision_at_ms,
            gate_elapsed_ms: 1_000,
            shard_index: 119,
            parity_offset: 0,
            registration: RollingGroupRegistration::Joined,
            active_before: 112,
            active_after: 113,
            ..
        } if *decision_at_ms == 1_100.25
    ));
    assert!(matches!(
        &snapshot.events[2],
        RollingGroupProfileEvent::ParityResult {
            group_id: 1,
            at_ms,
            shard_index: 119,
            parity_offset: 0,
            valid: false,
            active_before: 113,
            active_after: 112,
            successes_before: 4,
            successes_after: 4,
            completed: 6,
        } if *at_ms == 1_250.5
    ));
    assert!(matches!(
        &snapshot.events[3],
        RollingGroupProfileEvent::Terminal {
            group_id: 1,
            close_at_ms: Some(1_300.0),
            close_reason: Some(RollingGroupTerminalReason::ReconstructThreshold),
            reason: RollingGroupTerminalReason::ReconstructThreshold,
            reconstructed: true,
            direct_completion: false,
            ..
        }
    ));
}

#[test]
fn unfinished_group_drop_records_error_without_touching_production_admission() {
    let state = test_profile(0.0, 4);
    let group = test_rolling_group(&state, rolling_init()).expect("rolling group");
    drop(group);

    let snapshot = test_finalize_rolling_group_trace(&state, 200.0);
    assert_eq!(snapshot.groups_started, 1);
    assert_eq!(snapshot.groups_active, 0);
    assert_eq!(snapshot.groups_terminal, 1);
    assert_eq!(snapshot.terminal_error, 1);
    assert!(matches!(
        snapshot.events.last(),
        Some(RollingGroupProfileEvent::Terminal {
            reason: RollingGroupTerminalReason::Error,
            close_at_ms: None,
            ..
        })
    ));

    let profile_source = source("src/retrieval_profile.rs");
    let drop_impl = section(
        &profile_source,
        "impl Drop for RollingGroupProfile",
        "pub(crate) struct RetrievalProfileState",
    );
    assert!(drop_impl.contains("self.finish_error_at(now_ms())"));
    assert!(!drop_impl.contains("close()"));
    assert!(!drop_impl.contains("cancel"));
    assert!(!drop_impl.contains("RetrieveAdmission"));
}

#[test]
fn one_shot_finalizer_freezes_events_counters_and_timestamp() {
    let state = test_profile(0.0, 4);
    let mut group = test_rolling_group(&state, rolling_init()).expect("rolling group");
    test_managed_to_ordinary_promotion(&state, 75.0);
    test_managed_to_ordinary_promotion(&state, 90.0);
    let frozen = test_finalize_rolling_group_trace(&state, 150.0);
    assert_eq!(frozen.groups_started, 1);
    assert_eq!(frozen.groups_active, 1);
    assert_eq!(frozen.managed_to_ordinary_promotions, 2);
    assert_eq!(frozen.first_managed_to_ordinary_promotion_at_ms, Some(75.0));
    assert_eq!(frozen.last_managed_to_ordinary_promotion_at_ms, Some(90.0));

    group.parity_admitted(
        1_100.0,
        1_000,
        119,
        0,
        RollingGroupRegistration::Led,
        112,
        113,
        4,
        0,
    );
    group.finish_success_at(1_200.0, false);
    test_managed_to_ordinary_promotion(&state, 1_250.0);
    assert!(test_rolling_group(&state, rolling_init()).is_none());

    let repeated = test_finalize_rolling_group_trace(&state, 9_999.0);
    assert_eq!(repeated, frozen);
    assert_eq!(repeated.snapshot_at_ms, 150.0);
    assert_eq!(repeated.groups_active, 1);
    assert_eq!(repeated.groups_terminal, 0);
    assert_eq!(repeated.parity_admitted, 0);
    assert_eq!(repeated.managed_to_ordinary_promotions, 2);
}

#[test]
fn stale_and_channel_closed_are_distinct_terminal_causes() {
    let state = test_profile(0.0, 4);
    let mut stale = test_rolling_group(&state, rolling_init()).expect("stale group");
    stale.finish_stale_at(200.0);
    let mut closed = test_rolling_group(&state, rolling_init()).expect("closed group");
    closed.finish_channel_closed_at(250.0);

    let snapshot = test_finalize_rolling_group_trace(&state, 300.0);
    assert_eq!(snapshot.groups_terminal, 2);
    assert_eq!(snapshot.terminal_stale, 1);
    assert_eq!(snapshot.terminal_channel_closed, 1);
    assert_eq!(snapshot.terminal_error, 0);
    assert!(snapshot.events.iter().any(|event| matches!(
        event,
        RollingGroupProfileEvent::Terminal {
            reason: RollingGroupTerminalReason::Stale,
            ..
        }
    )));
    assert!(snapshot.events.iter().any(|event| matches!(
        event,
        RollingGroupProfileEvent::Terminal {
            reason: RollingGroupTerminalReason::ChannelClosed,
            ..
        }
    )));
}

#[test]
fn rolling_trace_cap_is_honest_and_cumulative_algebra_survives_truncation() {
    let state = test_profile(0.0, 4);
    let groups = ROLLING_GROUP_TRACE_CAP / 2 + 1;
    for index in 0..groups {
        let mut init = rolling_init();
        init.anchor_at_ms = index as f64;
        let mut group = test_rolling_group(&state, init).expect("rolling group before freeze");
        group.finish_success_at(index as f64 + 0.5, false);
    }

    let snapshot = test_finalize_rolling_group_trace(&state, groups as f64 + 1.0);
    assert_eq!(snapshot.events.len(), ROLLING_GROUP_TRACE_CAP);
    assert_eq!(snapshot.events_attempted, (groups * 2) as u64);
    assert_eq!(
        snapshot.events_attempted,
        snapshot.events.len() as u64 + snapshot.dropped
    );
    assert_eq!(snapshot.dropped, 2);
    assert!(snapshot.truncated);
    assert_eq!(snapshot.groups_started, groups as u64);
    assert_eq!(
        snapshot.groups_started,
        snapshot.groups_active + snapshot.groups_terminal
    );
    assert_eq!(snapshot.groups_terminal, snapshot.terminal_direct_all_ready);
    assert_eq!(
        snapshot.parity_admitted,
        snapshot.parity_cached + snapshot.parity_joined + snapshot.parity_led
    );
}

#[test]
fn timeout_hook_preserves_the_single_receiver_timeout_send_and_detached_awaits() {
    let retrieval = source("src/retrieval.rs");
    let attempt = section(
        &retrieval,
        "async fn retrieve_attempt(",
        "fn chunk_address_parts(",
    );
    assert_eq!(attempt.matches("chunk_in.recv()").count(), 2);
    assert_eq!(attempt.matches("result_chan.try_send(").count(), 2);
    assert_eq!(attempt.matches("async_std::future::timeout(").count(), 1);
    assert!(attempt.contains("Duration::from_millis(RETRIEVE_ATTEMPT_TIMEOUT_MS)"));
    assert!(
        attempt.contains("let detached_profile = profile.map(RetrieveAttemptProfile::timed_out);")
    );
    assert!(attempt.contains("let retrieve_result = chunk_in.recv().await;"));
    assert!(!attempt.contains("chunk_in.clone()"));
    assert!(!attempt.contains("select("));

    let settlement = attempt
        .rfind("settle_retrieve_attempt(")
        .expect("detached accounting settlement");
    let completion = attempt
        .rfind("profile.complete(")
        .expect("detached completion counter");
    assert!(settlement < completion);
}

#[test]
fn dispatcher_keeps_the_original_semaphore_guard_scope() {
    let lib = source("src/lib.rs");
    let dispatcher = section(
        &lib,
        "let retrieve_chunk_handle = async {",
        "let hive_joiner = async {",
    );
    assert_eq!(
        dispatcher
            .matches("let Some(_permit) = retrieval_conventions::acquire_retrieve_permit(")
            .count(),
        1
    );
    assert!(dispatcher.contains("let _profile_permit ="));
    assert!(dispatcher.contains(".map(|profile| profile.permit_acquired());"));
    let real_guard = dispatcher
        .find("let Some(_permit) = retrieval_conventions::acquire_retrieve_permit(")
        .expect("real semaphore guard");
    let profile_guard = dispatcher
        .find("let _profile_permit =")
        .expect("profile permit guard");
    assert!(real_guard < profile_guard);
    assert!(dispatcher.contains("no await separates its counter update"));
    assert!(!dispatcher.contains("drop(_permit)"));
    assert!(!dispatcher.contains("drop(_profile_permit)"));
    assert!(!dispatcher.contains("SemaphoreGuardArc"));
}
