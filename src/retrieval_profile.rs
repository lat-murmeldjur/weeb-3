use serde::Serialize;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const LOG2_HISTOGRAM_BUCKETS: usize = 32;
pub(crate) const ROLLING_GROUP_TRACE_CAP: usize = 2_048;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Log2MillisecondsHistogram {
    pub count: u64,
    pub sum_ms: f64,
    pub max_ms: f64,
    pub buckets: [u64; LOG2_HISTOGRAM_BUCKETS],
}

impl Default for Log2MillisecondsHistogram {
    fn default() -> Self {
        Self {
            count: 0,
            sum_ms: 0.0,
            max_ms: 0.0,
            buckets: [0; LOG2_HISTOGRAM_BUCKETS],
        }
    }
}

impl Log2MillisecondsHistogram {
    fn observe(&mut self, elapsed_ms: f64) {
        let elapsed_ms = if elapsed_ms.is_finite() {
            elapsed_ms.max(0.0)
        } else {
            0.0
        };
        let rounded_ms = elapsed_ms.ceil().min(u64::MAX as f64) as u64;
        let bucket = if rounded_ms <= 1 {
            0
        } else {
            (u64::BITS - (rounded_ms - 1).leading_zeros()) as usize
        }
        .min(LOG2_HISTOGRAM_BUCKETS - 1);

        self.count += 1;
        self.sum_ms += elapsed_ms;
        self.max_ms = self.max_ms.max(elapsed_ms);
        self.buckets[bucket] += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetrieveAttemptOutcome {
    ValidCac,
    ValidSoc,
    ConfirmedEmpty,
    InvalidNonempty,
    ChannelClosed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct RetrieveAttemptOutcomes {
    pub valid_cac: u64,
    pub valid_soc: u64,
    pub confirmed_empty: u64,
    pub invalid_nonempty: u64,
    pub channel_closed: u64,
}

impl RetrieveAttemptOutcomes {
    fn record(&mut self, outcome: RetrieveAttemptOutcome) {
        match outcome {
            RetrieveAttemptOutcome::ValidCac => self.valid_cac += 1,
            RetrieveAttemptOutcome::ValidSoc => self.valid_soc += 1,
            RetrieveAttemptOutcome::ConfirmedEmpty => self.confirmed_empty += 1,
            RetrieveAttemptOutcome::InvalidNonempty => self.invalid_nonempty += 1,
            RetrieveAttemptOutcome::ChannelClosed => self.channel_closed += 1,
        }
    }

    fn total(&self) -> u64 {
        self.valid_cac
            + self.valid_soc
            + self.confirmed_empty
            + self.invalid_nonempty
            + self.channel_closed
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct RetrievalProfileConservation {
    pub accepted_minus_queue_dequeued_forward_failed: i64,
    pub logical_dequeued_minus_active_completed: i64,
    pub permit_wait_started_minus_current_acquired_aborted: i64,
    pub permits_acquired_minus_current_released: i64,
    pub physical_dispatched_minus_active_immediate_timed_out: i64,
    pub immediate_completed_minus_outcomes: i64,
    pub immediate_completed_minus_result_sends: i64,
    pub timed_out_minus_detached_outstanding_completed: i64,
    pub timed_out_minus_result_sends: i64,
    pub detached_completed_minus_outcomes: i64,
    pub logical_completed_minus_deliveries: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct RetrievalProfileScopeCounters {
    pub tickets_created: u64,
    pub enqueue_accepted: u64,
    pub enqueue_rejected: u64,
    pub relay_forward_succeeded: u64,
    pub relay_forward_failed: u64,
    pub queue_dequeued: u64,
    pub logical_completed: u64,
    pub permit_acquired: u64,
    pub physical_dispatched: u64,
    pub physical_immediate_completed: u64,
    pub physical_timed_out: u64,
    pub detached_completed: u64,
    pub immediate_result_send_succeeded: u64,
    pub immediate_result_send_failed: u64,
    pub timeout_result_send_succeeded: u64,
    pub timeout_result_send_failed: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct RetrievalProfileByScope {
    pub stream_scoped: RetrievalProfileScopeCounters,
    pub unscoped: RetrievalProfileScopeCounters,
}

impl RetrievalProfileByScope {
    fn counters_mut(&mut self, stream_scoped: bool) -> &mut RetrievalProfileScopeCounters {
        if stream_scoped {
            &mut self.stream_scoped
        } else {
            &mut self.unscoped
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) enum RollingGroupRegistration {
    Cached,
    Joined,
    Led,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RollingGroupTerminalReason {
    DirectAllReady,
    ReconstructThreshold,
    Stale,
    ChannelClosed,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RollingGroupProfileInit {
    pub anchor_at_ms: f64,
    pub requested_count: usize,
    pub data_count: usize,
    pub parity_count: usize,
    pub decoded_raw_count: usize,
    pub decoded_only_count: usize,
    pub miss_count: usize,
    pub static_candidate: bool,
    pub dynamic_eligible: bool,
    pub initial_cached: usize,
    pub initial_joined: usize,
    pub initial_led: usize,
    pub initial_active: usize,
    pub initial_successes: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub(crate) enum RollingGroupProfileEvent {
    Init {
        group_id: u64,
        at_ms: f64,
        requested_count: u64,
        data_count: u64,
        parity_count: u64,
        decoded_raw_count: u64,
        decoded_only_count: u64,
        miss_count: u64,
        static_candidate: bool,
        dynamic_eligible: bool,
        initial_cached: u64,
        initial_joined: u64,
        initial_led: u64,
        initial_active: u64,
        initial_successes: u64,
    },
    ParityAdmission {
        group_id: u64,
        decision_at_ms: f64,
        gate_elapsed_ms: u64,
        shard_index: u64,
        parity_offset: u64,
        registration: RollingGroupRegistration,
        active_before: u64,
        active_after: u64,
        successes: u64,
        completed: u64,
    },
    ParityResult {
        group_id: u64,
        at_ms: f64,
        shard_index: u64,
        parity_offset: u64,
        valid: bool,
        active_before: u64,
        active_after: u64,
        successes_before: u64,
        successes_after: u64,
        completed: u64,
    },
    Terminal {
        group_id: u64,
        at_ms: f64,
        close_at_ms: Option<f64>,
        close_reason: Option<RollingGroupTerminalReason>,
        reason: RollingGroupTerminalReason,
        successes: u64,
        completed: u64,
        active: u64,
        parity_admitted: u64,
        parity_cached: u64,
        parity_joined: u64,
        parity_led: u64,
        parity_valid: u64,
        parity_invalid: u64,
        direct_completion: bool,
        reconstructed: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct RollingGroupTraceSnapshot {
    pub schema_version: u32,
    pub activation_at_ms: f64,
    pub snapshot_at_ms: f64,
    pub cap: u64,
    pub events_attempted: u64,
    pub dropped: u64,
    pub truncated: bool,
    pub groups_started: u64,
    pub groups_dynamic_eligible: u64,
    pub groups_dynamic_ineligible: u64,
    pub groups_active: u64,
    pub groups_terminal: u64,
    pub terminal_direct_all_ready: u64,
    pub terminal_reconstruct_threshold: u64,
    pub terminal_stale: u64,
    pub terminal_channel_closed: u64,
    pub terminal_error: u64,
    pub parity_admitted: u64,
    pub parity_cached: u64,
    pub parity_joined: u64,
    pub parity_led: u64,
    pub parity_valid: u64,
    pub parity_invalid: u64,
    pub managed_to_ordinary_promotions: u64,
    pub first_managed_to_ordinary_promotion_at_ms: Option<f64>,
    pub last_managed_to_ordinary_promotion_at_ms: Option<f64>,
    pub events: Vec<RollingGroupProfileEvent>,
}

impl RollingGroupTraceSnapshot {
    fn new(activation_at_ms: f64) -> Self {
        Self {
            schema_version: 1,
            activation_at_ms,
            snapshot_at_ms: activation_at_ms,
            cap: ROLLING_GROUP_TRACE_CAP as u64,
            events_attempted: 0,
            dropped: 0,
            truncated: false,
            groups_started: 0,
            groups_dynamic_eligible: 0,
            groups_dynamic_ineligible: 0,
            groups_active: 0,
            groups_terminal: 0,
            terminal_direct_all_ready: 0,
            terminal_reconstruct_threshold: 0,
            terminal_stale: 0,
            terminal_channel_closed: 0,
            terminal_error: 0,
            parity_admitted: 0,
            parity_cached: 0,
            parity_joined: 0,
            parity_led: 0,
            parity_valid: 0,
            parity_invalid: 0,
            managed_to_ordinary_promotions: 0,
            first_managed_to_ordinary_promotion_at_ms: None,
            last_managed_to_ordinary_promotion_at_ms: None,
            events: Vec::new(),
        }
    }

    fn push_event(&mut self, event: RollingGroupProfileEvent) {
        self.events_attempted = self.events_attempted.saturating_add(1);
        if self.events.len() < ROLLING_GROUP_TRACE_CAP {
            self.events.push(event);
        } else {
            self.dropped = self.dropped.saturating_add(1);
            self.truncated = true;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct RetrievalProfileSnapshot {
    pub schema_version: u32,
    pub enabled: bool,
    pub activation_at_ms: f64,
    pub snapshot_at_ms: f64,
    pub permit_capacity: u64,
    pub log2_bucket_upper_bounds_ms: [u64; LOG2_HISTOGRAM_BUCKETS],
    pub by_scope: RetrievalProfileByScope,

    pub tickets_created: u64,
    pub enqueue_accepted: u64,
    pub enqueue_rejected: u64,
    pub relay_forward_succeeded: u64,
    pub relay_forward_failed: u64,
    pub queue_current: u64,
    pub queue_high_water: u64,
    pub queue_dequeued: u64,

    pub logical_active: u64,
    pub logical_high_water: u64,
    pub logical_completed: u64,
    pub logical_nonempty: u64,
    pub logical_empty: u64,
    pub pre_permit_rejected: u64,
    pub delivery_succeeded: u64,
    pub delivery_failed: u64,

    pub permit_wait_started: u64,
    pub permit_wait_current: u64,
    pub permit_wait_high_water: u64,
    pub permit_wait_acquired: u64,
    pub permit_wait_aborted: u64,
    pub permits_current: u64,
    pub permits_high_water: u64,
    pub permits_released: u64,

    pub physical_dispatched: u64,
    pub physical_active: u64,
    pub physical_high_water: u64,
    pub physical_immediate_completed: u64,
    pub physical_timed_out: u64,
    pub immediate_result_send_succeeded: u64,
    pub immediate_result_send_failed: u64,
    pub timeout_result_send_succeeded: u64,
    pub timeout_result_send_failed: u64,
    pub detached_outstanding: u64,
    pub detached_high_water: u64,
    pub detached_completed: u64,
    pub immediate_outcomes: RetrieveAttemptOutcomes,
    pub detached_outcomes: RetrieveAttemptOutcomes,

    pub queue_to_permit_acquired_ms: Log2MillisecondsHistogram,
    pub queue_to_permit_aborted_ms: Log2MillisecondsHistogram,
    pub queue_to_pre_permit_rejection_ms: Log2MillisecondsHistogram,
    pub permit_wait_acquired_ms: Log2MillisecondsHistogram,
    pub permit_wait_aborted_ms: Log2MillisecondsHistogram,
    pub immediate_attempt_ms: Log2MillisecondsHistogram,
    pub detached_after_timeout_ms: Log2MillisecondsHistogram,
    pub detached_total_attempt_ms: Log2MillisecondsHistogram,

    pub conservation: RetrievalProfileConservation,
}

impl RetrievalProfileSnapshot {
    fn new(activation_at_ms: f64, permit_capacity: u64) -> Self {
        let mut bucket_upper_bounds_ms = [0; LOG2_HISTOGRAM_BUCKETS];
        let mut index = 0;
        while index < LOG2_HISTOGRAM_BUCKETS {
            bucket_upper_bounds_ms[index] = 1_u64 << index;
            index += 1;
        }

        Self {
            schema_version: 1,
            enabled: true,
            activation_at_ms,
            snapshot_at_ms: activation_at_ms,
            permit_capacity,
            log2_bucket_upper_bounds_ms: bucket_upper_bounds_ms,
            by_scope: RetrievalProfileByScope::default(),
            tickets_created: 0,
            enqueue_accepted: 0,
            enqueue_rejected: 0,
            relay_forward_succeeded: 0,
            relay_forward_failed: 0,
            queue_current: 0,
            queue_high_water: 0,
            queue_dequeued: 0,
            logical_active: 0,
            logical_high_water: 0,
            logical_completed: 0,
            logical_nonempty: 0,
            logical_empty: 0,
            pre_permit_rejected: 0,
            delivery_succeeded: 0,
            delivery_failed: 0,
            permit_wait_started: 0,
            permit_wait_current: 0,
            permit_wait_high_water: 0,
            permit_wait_acquired: 0,
            permit_wait_aborted: 0,
            permits_current: 0,
            permits_high_water: 0,
            permits_released: 0,
            physical_dispatched: 0,
            physical_active: 0,
            physical_high_water: 0,
            physical_immediate_completed: 0,
            physical_timed_out: 0,
            immediate_result_send_succeeded: 0,
            immediate_result_send_failed: 0,
            timeout_result_send_succeeded: 0,
            timeout_result_send_failed: 0,
            detached_outstanding: 0,
            detached_high_water: 0,
            detached_completed: 0,
            immediate_outcomes: RetrieveAttemptOutcomes::default(),
            detached_outcomes: RetrieveAttemptOutcomes::default(),
            queue_to_permit_acquired_ms: Log2MillisecondsHistogram::default(),
            queue_to_permit_aborted_ms: Log2MillisecondsHistogram::default(),
            queue_to_pre_permit_rejection_ms: Log2MillisecondsHistogram::default(),
            permit_wait_acquired_ms: Log2MillisecondsHistogram::default(),
            permit_wait_aborted_ms: Log2MillisecondsHistogram::default(),
            immediate_attempt_ms: Log2MillisecondsHistogram::default(),
            detached_after_timeout_ms: Log2MillisecondsHistogram::default(),
            detached_total_attempt_ms: Log2MillisecondsHistogram::default(),
            conservation: RetrievalProfileConservation::default(),
        }
    }

    fn refresh_conservation(&mut self) {
        self.conservation = RetrievalProfileConservation {
            accepted_minus_queue_dequeued_forward_failed: balance(
                self.enqueue_accepted,
                self.queue_current + self.queue_dequeued + self.relay_forward_failed,
            ),
            logical_dequeued_minus_active_completed: balance(
                self.queue_dequeued,
                self.logical_active + self.logical_completed,
            ),
            permit_wait_started_minus_current_acquired_aborted: balance(
                self.permit_wait_started,
                self.permit_wait_current + self.permit_wait_acquired + self.permit_wait_aborted,
            ),
            permits_acquired_minus_current_released: balance(
                self.permit_wait_acquired,
                self.permits_current + self.permits_released,
            ),
            physical_dispatched_minus_active_immediate_timed_out: balance(
                self.physical_dispatched,
                self.physical_active + self.physical_immediate_completed + self.physical_timed_out,
            ),
            immediate_completed_minus_outcomes: balance(
                self.physical_immediate_completed,
                self.immediate_outcomes.total(),
            ),
            immediate_completed_minus_result_sends: balance(
                self.physical_immediate_completed,
                self.immediate_result_send_succeeded + self.immediate_result_send_failed,
            ),
            timed_out_minus_detached_outstanding_completed: balance(
                self.physical_timed_out,
                self.detached_outstanding + self.detached_completed,
            ),
            timed_out_minus_result_sends: balance(
                self.physical_timed_out,
                self.timeout_result_send_succeeded + self.timeout_result_send_failed,
            ),
            detached_completed_minus_outcomes: balance(
                self.detached_completed,
                self.detached_outcomes.total(),
            ),
            logical_completed_minus_deliveries: balance(
                self.logical_completed,
                self.delivery_succeeded + self.delivery_failed,
            ),
        };
    }
}

fn balance(left: u64, right: u64) -> i64 {
    i128::from(left)
        .saturating_sub(i128::from(right))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn usize_counter(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[derive(Debug)]
struct RollingGroupTraceState {
    snapshot: RollingGroupTraceSnapshot,
    next_group_id: u64,
    frozen: Option<RollingGroupTraceSnapshot>,
}

impl RollingGroupTraceState {
    fn new(activation_at_ms: f64) -> Self {
        Self {
            snapshot: RollingGroupTraceSnapshot::new(activation_at_ms),
            next_group_id: 0,
            frozen: None,
        }
    }

    fn begin_group(&mut self, init: RollingGroupProfileInit) -> Option<u64> {
        if self.frozen.is_some() {
            return None;
        }
        self.next_group_id = self.next_group_id.wrapping_add(1).max(1);
        let group_id = self.next_group_id;
        self.snapshot.groups_started = self.snapshot.groups_started.saturating_add(1);
        self.snapshot.groups_active = self.snapshot.groups_active.saturating_add(1);
        if init.dynamic_eligible {
            self.snapshot.groups_dynamic_eligible =
                self.snapshot.groups_dynamic_eligible.saturating_add(1);
        } else {
            self.snapshot.groups_dynamic_ineligible =
                self.snapshot.groups_dynamic_ineligible.saturating_add(1);
        }
        self.snapshot.push_event(RollingGroupProfileEvent::Init {
            group_id,
            at_ms: init.anchor_at_ms,
            requested_count: usize_counter(init.requested_count),
            data_count: usize_counter(init.data_count),
            parity_count: usize_counter(init.parity_count),
            decoded_raw_count: usize_counter(init.decoded_raw_count),
            decoded_only_count: usize_counter(init.decoded_only_count),
            miss_count: usize_counter(init.miss_count),
            static_candidate: init.static_candidate,
            dynamic_eligible: init.dynamic_eligible,
            initial_cached: usize_counter(init.initial_cached),
            initial_joined: usize_counter(init.initial_joined),
            initial_led: usize_counter(init.initial_led),
            initial_active: usize_counter(init.initial_active),
            initial_successes: usize_counter(init.initial_successes),
        });
        Some(group_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn parity_admitted(
        &mut self,
        group_id: u64,
        decision_at_ms: f64,
        gate_elapsed_ms: u64,
        shard_index: usize,
        parity_offset: usize,
        registration: RollingGroupRegistration,
        active_before: usize,
        active_after: usize,
        successes: usize,
        completed: usize,
    ) {
        if self.frozen.is_some() {
            return;
        }
        self.snapshot.parity_admitted = self.snapshot.parity_admitted.saturating_add(1);
        match registration {
            RollingGroupRegistration::Cached => {
                self.snapshot.parity_cached = self.snapshot.parity_cached.saturating_add(1)
            }
            RollingGroupRegistration::Joined => {
                self.snapshot.parity_joined = self.snapshot.parity_joined.saturating_add(1)
            }
            RollingGroupRegistration::Led => {
                self.snapshot.parity_led = self.snapshot.parity_led.saturating_add(1)
            }
        }
        self.snapshot
            .push_event(RollingGroupProfileEvent::ParityAdmission {
                group_id,
                decision_at_ms,
                gate_elapsed_ms,
                shard_index: usize_counter(shard_index),
                parity_offset: usize_counter(parity_offset),
                registration,
                active_before: usize_counter(active_before),
                active_after: usize_counter(active_after),
                successes: usize_counter(successes),
                completed: usize_counter(completed),
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn parity_result(
        &mut self,
        group_id: u64,
        at_ms: f64,
        shard_index: usize,
        parity_offset: usize,
        valid: bool,
        active_before: usize,
        active_after: usize,
        successes_before: usize,
        successes_after: usize,
        completed: usize,
    ) {
        if self.frozen.is_some() {
            return;
        }
        if valid {
            self.snapshot.parity_valid = self.snapshot.parity_valid.saturating_add(1);
        } else {
            self.snapshot.parity_invalid = self.snapshot.parity_invalid.saturating_add(1);
        }
        self.snapshot
            .push_event(RollingGroupProfileEvent::ParityResult {
                group_id,
                at_ms,
                shard_index: usize_counter(shard_index),
                parity_offset: usize_counter(parity_offset),
                valid,
                active_before: usize_counter(active_before),
                active_after: usize_counter(active_after),
                successes_before: usize_counter(successes_before),
                successes_after: usize_counter(successes_after),
                completed: usize_counter(completed),
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal(
        &mut self,
        group_id: u64,
        finish_at_ms: f64,
        close_at_ms: Option<f64>,
        close_reason: Option<RollingGroupTerminalReason>,
        reason: RollingGroupTerminalReason,
        successes: usize,
        completed: usize,
        active: usize,
        parity_admitted: usize,
        parity_cached: usize,
        parity_joined: usize,
        parity_led: usize,
        parity_valid: usize,
        parity_invalid: usize,
        direct_completion: bool,
        reconstructed: bool,
    ) {
        if self.frozen.is_some() {
            return;
        }
        self.snapshot.groups_active = self.snapshot.groups_active.saturating_sub(1);
        self.snapshot.groups_terminal = self.snapshot.groups_terminal.saturating_add(1);
        match reason {
            RollingGroupTerminalReason::DirectAllReady => {
                self.snapshot.terminal_direct_all_ready =
                    self.snapshot.terminal_direct_all_ready.saturating_add(1)
            }
            RollingGroupTerminalReason::ReconstructThreshold => {
                self.snapshot.terminal_reconstruct_threshold = self
                    .snapshot
                    .terminal_reconstruct_threshold
                    .saturating_add(1)
            }
            RollingGroupTerminalReason::Stale => {
                self.snapshot.terminal_stale = self.snapshot.terminal_stale.saturating_add(1)
            }
            RollingGroupTerminalReason::ChannelClosed => {
                self.snapshot.terminal_channel_closed =
                    self.snapshot.terminal_channel_closed.saturating_add(1)
            }
            RollingGroupTerminalReason::Error => {
                self.snapshot.terminal_error = self.snapshot.terminal_error.saturating_add(1)
            }
        }
        self.snapshot
            .push_event(RollingGroupProfileEvent::Terminal {
                group_id,
                at_ms: finish_at_ms,
                close_at_ms,
                close_reason,
                reason,
                successes: usize_counter(successes),
                completed: usize_counter(completed),
                active: usize_counter(active),
                parity_admitted: usize_counter(parity_admitted),
                parity_cached: usize_counter(parity_cached),
                parity_joined: usize_counter(parity_joined),
                parity_led: usize_counter(parity_led),
                parity_valid: usize_counter(parity_valid),
                parity_invalid: usize_counter(parity_invalid),
                direct_completion,
                reconstructed,
            });
    }

    fn managed_to_ordinary_promotion(&mut self, at_ms: f64) {
        if self.frozen.is_some() {
            return;
        }
        self.snapshot.managed_to_ordinary_promotions = self
            .snapshot
            .managed_to_ordinary_promotions
            .saturating_add(1);
        self.snapshot
            .first_managed_to_ordinary_promotion_at_ms
            .get_or_insert(at_ms);
        self.snapshot.last_managed_to_ordinary_promotion_at_ms = Some(at_ms);
    }

    fn finalize(&mut self, snapshot_at_ms: f64) -> RollingGroupTraceSnapshot {
        if let Some(frozen) = self.frozen.as_ref() {
            return frozen.clone();
        }
        let mut snapshot = self.snapshot.clone();
        snapshot.snapshot_at_ms = snapshot_at_ms;
        self.frozen = Some(snapshot.clone());
        snapshot
    }
}

#[derive(Clone, Copy, Debug)]
struct RollingGroupClose {
    at_ms: f64,
    reason: RollingGroupTerminalReason,
}

pub(crate) struct RollingGroupProfile {
    state: Rc<RetrievalProfileState>,
    group_id: u64,
    close: Option<RollingGroupClose>,
    successes: usize,
    completed: usize,
    active: usize,
    parity_admitted: usize,
    parity_cached: usize,
    parity_joined: usize,
    parity_led: usize,
    parity_valid: usize,
    parity_invalid: usize,
    finished: bool,
}

impl RollingGroupProfile {
    fn new(state: Rc<RetrievalProfileState>, init: RollingGroupProfileInit) -> Option<Self> {
        let group_id = state.rolling_groups.borrow_mut().begin_group(init)?;
        Some(Self {
            state,
            group_id,
            close: None,
            successes: init.initial_successes,
            completed: 0,
            active: init.initial_active,
            parity_admitted: 0,
            parity_cached: 0,
            parity_joined: 0,
            parity_led: 0,
            parity_valid: 0,
            parity_invalid: 0,
            finished: false,
        })
    }

    fn recording(&self) -> bool {
        self.state.rolling_group_trace_recording()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn parity_admitted(
        &mut self,
        decision_at_ms: f64,
        gate_elapsed_ms: u64,
        shard_index: usize,
        parity_offset: usize,
        registration: RollingGroupRegistration,
        active_before: usize,
        active_after: usize,
        successes: usize,
        completed: usize,
    ) {
        if !self.recording() {
            return;
        }
        self.progress(successes, completed, active_after);
        self.parity_admitted = self.parity_admitted.saturating_add(1);
        match registration {
            RollingGroupRegistration::Cached => {
                self.parity_cached = self.parity_cached.saturating_add(1)
            }
            RollingGroupRegistration::Joined => {
                self.parity_joined = self.parity_joined.saturating_add(1)
            }
            RollingGroupRegistration::Led => self.parity_led = self.parity_led.saturating_add(1),
        }
        self.state.rolling_groups.borrow_mut().parity_admitted(
            self.group_id,
            decision_at_ms,
            gate_elapsed_ms,
            shard_index,
            parity_offset,
            registration,
            active_before,
            active_after,
            successes,
            completed,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn parity_result_now(
        &mut self,
        shard_index: usize,
        parity_offset: usize,
        valid: bool,
        active_before: usize,
        active_after: usize,
        successes_before: usize,
        successes_after: usize,
        completed: usize,
    ) {
        if !self.recording() {
            return;
        }
        self.parity_result_at(
            now_ms(),
            shard_index,
            parity_offset,
            valid,
            active_before,
            active_after,
            successes_before,
            successes_after,
            completed,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn parity_result_at(
        &mut self,
        at_ms: f64,
        shard_index: usize,
        parity_offset: usize,
        valid: bool,
        active_before: usize,
        active_after: usize,
        successes_before: usize,
        successes_after: usize,
        completed: usize,
    ) {
        if !self.recording() {
            return;
        }
        if valid {
            self.parity_valid = self.parity_valid.saturating_add(1);
        } else {
            self.parity_invalid = self.parity_invalid.saturating_add(1);
        }
        self.state.rolling_groups.borrow_mut().parity_result(
            self.group_id,
            at_ms,
            shard_index,
            parity_offset,
            valid,
            active_before,
            active_after,
            successes_before,
            successes_after,
            completed,
        );
    }

    pub(crate) fn progress(&mut self, successes: usize, completed: usize, active: usize) {
        if !self.recording() {
            return;
        }
        self.successes = successes;
        self.completed = completed;
        self.active = active;
    }

    pub(crate) fn close_now(
        &mut self,
        reason: RollingGroupTerminalReason,
        successes: usize,
        completed: usize,
        active: usize,
    ) {
        if !self.recording() {
            return;
        }
        self.close_at(reason, now_ms(), successes, completed, active);
    }

    pub(crate) fn close_at(
        &mut self,
        reason: RollingGroupTerminalReason,
        at_ms: f64,
        successes: usize,
        completed: usize,
        active: usize,
    ) {
        if !self.recording() {
            return;
        }
        self.progress(successes, completed, active);
        self.close
            .get_or_insert(RollingGroupClose { at_ms, reason });
    }

    pub(crate) fn finish_success_now(&mut self, reconstructed: bool) {
        if !self.recording() {
            return;
        }
        self.finish_success_at(now_ms(), reconstructed);
    }

    pub(crate) fn finish_success_at(&mut self, at_ms: f64, reconstructed: bool) {
        if !self.recording() {
            return;
        }
        let reason = self
            .close
            .map(|close| close.reason)
            .unwrap_or(if reconstructed {
                RollingGroupTerminalReason::ReconstructThreshold
            } else {
                RollingGroupTerminalReason::DirectAllReady
            });
        self.finish(reason, at_ms, !reconstructed, reconstructed);
    }

    pub(crate) fn finish_stale_now(&mut self) {
        if !self.recording() {
            return;
        }
        self.finish_stale_at(now_ms());
    }

    pub(crate) fn finish_stale_at(&mut self, at_ms: f64) {
        if !self.recording() {
            return;
        }
        self.finish(RollingGroupTerminalReason::Stale, at_ms, false, false);
    }

    pub(crate) fn finish_channel_closed_now(&mut self) {
        if !self.recording() {
            return;
        }
        self.finish_channel_closed_at(now_ms());
    }

    pub(crate) fn finish_channel_closed_at(&mut self, at_ms: f64) {
        if !self.recording() {
            return;
        }
        self.finish(
            RollingGroupTerminalReason::ChannelClosed,
            at_ms,
            false,
            false,
        );
    }

    fn finish_error_at(&mut self, at_ms: f64) {
        if !self.recording() {
            return;
        }
        self.finish(RollingGroupTerminalReason::Error, at_ms, false, false);
    }

    fn finish(
        &mut self,
        reason: RollingGroupTerminalReason,
        at_ms: f64,
        direct_completion: bool,
        reconstructed: bool,
    ) {
        if self.finished || !self.recording() {
            return;
        }
        self.finished = true;
        self.state.rolling_groups.borrow_mut().terminal(
            self.group_id,
            at_ms,
            self.close.map(|close| close.at_ms),
            self.close.map(|close| close.reason),
            reason,
            self.successes,
            self.completed,
            self.active,
            self.parity_admitted,
            self.parity_cached,
            self.parity_joined,
            self.parity_led,
            self.parity_valid,
            self.parity_invalid,
            direct_completion,
            reconstructed,
        );
    }
}

impl Drop for RollingGroupProfile {
    fn drop(&mut self) {
        // Profiling is observation-only: an unfinished handle records an error
        // but never closes or otherwise mutates production admission state.
        if !self.finished && self.recording() {
            self.finish_error_at(now_ms());
        }
    }
}

#[derive(Debug)]
pub(crate) struct RetrievalProfileState {
    snapshot: RefCell<RetrievalProfileSnapshot>,
    rolling_groups: RefCell<RollingGroupTraceState>,
}

impl RetrievalProfileState {
    fn new(activation_at_ms: f64, permit_capacity: u64) -> Self {
        Self {
            snapshot: RefCell::new(RetrievalProfileSnapshot::new(
                activation_at_ms,
                permit_capacity,
            )),
            rolling_groups: RefCell::new(RollingGroupTraceState::new(activation_at_ms)),
        }
    }

    fn request(
        self: &Rc<Self>,
        enqueued_at_ms: f64,
        stream_scoped: bool,
    ) -> RetrieveProfileRequest {
        let mut snapshot = self.snapshot.borrow_mut();
        snapshot.tickets_created += 1;
        snapshot
            .by_scope
            .counters_mut(stream_scoped)
            .tickets_created += 1;
        drop(snapshot);
        RetrieveProfileRequest {
            inner: Rc::new(RetrieveProfileRequestInner {
                state: self.clone(),
                enqueued_at_ms,
                stream_scoped,
                enqueue_resolved: Cell::new(false),
                accepted: Cell::new(false),
                dequeued: Cell::new(false),
                logical_completed: Cell::new(false),
                permit_wait_started_at_ms: Cell::new(None),
                permit_wait_resolved: Cell::new(false),
            }),
        }
    }

    fn snapshot(&self, snapshot_at_ms: f64) -> RetrievalProfileSnapshot {
        let mut snapshot = self.snapshot.borrow().clone();
        snapshot.snapshot_at_ms = snapshot_at_ms;
        snapshot.refresh_conservation();
        snapshot
    }

    fn rolling_group(
        self: &Rc<Self>,
        init: RollingGroupProfileInit,
    ) -> Option<RollingGroupProfile> {
        RollingGroupProfile::new(self.clone(), init)
    }

    fn finalize_rolling_group_trace(&self, snapshot_at_ms: f64) -> RollingGroupTraceSnapshot {
        self.rolling_groups.borrow_mut().finalize(snapshot_at_ms)
    }

    fn rolling_group_trace_recording(&self) -> bool {
        self.rolling_groups.borrow().frozen.is_none()
    }

    fn managed_to_ordinary_promotion(&self, at_ms: f64) {
        self.rolling_groups
            .borrow_mut()
            .managed_to_ordinary_promotion(at_ms);
    }
}

struct RetrieveProfileRequestInner {
    state: Rc<RetrievalProfileState>,
    enqueued_at_ms: f64,
    stream_scoped: bool,
    enqueue_resolved: Cell<bool>,
    accepted: Cell<bool>,
    dequeued: Cell<bool>,
    logical_completed: Cell<bool>,
    permit_wait_started_at_ms: Cell<Option<f64>>,
    permit_wait_resolved: Cell<bool>,
}

#[derive(Clone)]
pub(crate) struct RetrieveProfileRequest {
    inner: Rc<RetrieveProfileRequestInner>,
}

impl RetrieveProfileRequest {
    pub(crate) fn enqueue_result(&self, accepted: bool) {
        self.enqueue_result_at(accepted);
    }

    fn enqueue_result_at(&self, accepted: bool) {
        if self.inner.enqueue_resolved.replace(true) {
            return;
        }
        self.inner.accepted.set(accepted);
        let mut snapshot = self.inner.state.snapshot.borrow_mut();
        if accepted {
            snapshot.enqueue_accepted += 1;
            snapshot.queue_current += 1;
            snapshot.queue_high_water = snapshot.queue_high_water.max(snapshot.queue_current);
            snapshot
                .by_scope
                .counters_mut(self.inner.stream_scoped)
                .enqueue_accepted += 1;
        } else {
            snapshot.enqueue_rejected += 1;
            snapshot
                .by_scope
                .counters_mut(self.inner.stream_scoped)
                .enqueue_rejected += 1;
        }
    }

    pub(crate) fn dequeued(&self) {
        self.dequeued_at();
    }

    pub(crate) fn relay_result(&self, succeeded: bool) {
        let mut snapshot = self.inner.state.snapshot.borrow_mut();
        if succeeded {
            snapshot.relay_forward_succeeded += 1;
            snapshot
                .by_scope
                .counters_mut(self.inner.stream_scoped)
                .relay_forward_succeeded += 1;
        } else {
            snapshot.queue_current = snapshot.queue_current.saturating_sub(1);
            snapshot.relay_forward_failed += 1;
            snapshot
                .by_scope
                .counters_mut(self.inner.stream_scoped)
                .relay_forward_failed += 1;
        }
    }

    fn dequeued_at(&self) {
        if !self.inner.accepted.get() || self.inner.dequeued.replace(true) {
            return;
        }
        let mut snapshot = self.inner.state.snapshot.borrow_mut();
        snapshot.queue_current = snapshot.queue_current.saturating_sub(1);
        snapshot.queue_dequeued += 1;
        snapshot.logical_active += 1;
        snapshot.logical_high_water = snapshot.logical_high_water.max(snapshot.logical_active);
        snapshot
            .by_scope
            .counters_mut(self.inner.stream_scoped)
            .queue_dequeued += 1;
    }

    pub(crate) fn reject_before_permit(&self) {
        self.reject_before_permit_at(now_ms());
    }

    fn reject_before_permit_at(&self, now_ms: f64) {
        let mut snapshot = self.inner.state.snapshot.borrow_mut();
        snapshot.pre_permit_rejected += 1;
        snapshot
            .queue_to_pre_permit_rejection_ms
            .observe(now_ms - self.inner.enqueued_at_ms);
    }

    pub(crate) fn begin_permit_wait(&self) {
        self.begin_permit_wait_at(now_ms());
    }

    fn begin_permit_wait_at(&self, now_ms: f64) {
        if self
            .inner
            .permit_wait_started_at_ms
            .replace(Some(now_ms))
            .is_some()
        {
            return;
        }
        let mut snapshot = self.inner.state.snapshot.borrow_mut();
        snapshot.permit_wait_started += 1;
        snapshot.permit_wait_current += 1;
        snapshot.permit_wait_high_water = snapshot
            .permit_wait_high_water
            .max(snapshot.permit_wait_current);
    }

    pub(crate) fn permit_aborted(&self) {
        self.permit_aborted_at(now_ms());
    }

    fn permit_aborted_at(&self, now_ms: f64) {
        if self.inner.permit_wait_resolved.replace(true) {
            return;
        }
        let Some(started_at_ms) = self.inner.permit_wait_started_at_ms.get() else {
            return;
        };
        let mut snapshot = self.inner.state.snapshot.borrow_mut();
        snapshot.permit_wait_current = snapshot.permit_wait_current.saturating_sub(1);
        snapshot.permit_wait_aborted += 1;
        snapshot
            .queue_to_permit_aborted_ms
            .observe(now_ms - self.inner.enqueued_at_ms);
        snapshot
            .permit_wait_aborted_ms
            .observe(now_ms - started_at_ms);
    }

    pub(crate) fn permit_acquired(&self) -> RetrieveProfilePermit {
        self.permit_acquired_at(now_ms())
    }

    fn permit_acquired_at(&self, now_ms: f64) -> RetrieveProfilePermit {
        if !self.inner.permit_wait_resolved.replace(true)
            && let Some(started_at_ms) = self.inner.permit_wait_started_at_ms.get()
        {
            let mut snapshot = self.inner.state.snapshot.borrow_mut();
            snapshot.permit_wait_current = snapshot.permit_wait_current.saturating_sub(1);
            snapshot.permit_wait_acquired += 1;
            snapshot.permits_current += 1;
            snapshot.permits_high_water = snapshot.permits_high_water.max(snapshot.permits_current);
            snapshot
                .by_scope
                .counters_mut(self.inner.stream_scoped)
                .permit_acquired += 1;
            snapshot
                .queue_to_permit_acquired_ms
                .observe(now_ms - self.inner.enqueued_at_ms);
            snapshot
                .permit_wait_acquired_ms
                .observe(now_ms - started_at_ms);
        }
        RetrieveProfilePermit {
            state: self.inner.state.clone(),
            released: false,
        }
    }

    pub(crate) fn logical_completed(&self, nonempty: bool) {
        if self.inner.logical_completed.replace(true) {
            return;
        }
        let mut snapshot = self.inner.state.snapshot.borrow_mut();
        snapshot.logical_active = snapshot.logical_active.saturating_sub(1);
        snapshot.logical_completed += 1;
        snapshot
            .by_scope
            .counters_mut(self.inner.stream_scoped)
            .logical_completed += 1;
        if nonempty {
            snapshot.logical_nonempty += 1;
        } else {
            snapshot.logical_empty += 1;
        }
    }

    pub(crate) fn delivery_result(&self, succeeded: bool) {
        let mut snapshot = self.inner.state.snapshot.borrow_mut();
        if succeeded {
            snapshot.delivery_succeeded += 1;
        } else {
            snapshot.delivery_failed += 1;
        }
    }

    pub(crate) fn physical_attempt(&self) -> RetrieveAttemptProfile {
        self.physical_attempt_at(now_ms())
    }

    fn physical_attempt_at(&self, started_at_ms: f64) -> RetrieveAttemptProfile {
        let mut snapshot = self.inner.state.snapshot.borrow_mut();
        snapshot.physical_dispatched += 1;
        snapshot.physical_active += 1;
        snapshot.physical_high_water = snapshot.physical_high_water.max(snapshot.physical_active);
        snapshot
            .by_scope
            .counters_mut(self.inner.stream_scoped)
            .physical_dispatched += 1;
        drop(snapshot);
        RetrieveAttemptProfile {
            state: self.inner.state.clone(),
            started_at_ms,
            stream_scoped: self.inner.stream_scoped,
            completed: false,
        }
    }
}

pub(crate) struct RetrieveProfilePermit {
    state: Rc<RetrievalProfileState>,
    released: bool,
}

impl Drop for RetrieveProfilePermit {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut snapshot = self.state.snapshot.borrow_mut();
        snapshot.permits_current = snapshot.permits_current.saturating_sub(1);
        snapshot.permits_released += 1;
    }
}

pub(crate) struct RetrieveAttemptProfile {
    state: Rc<RetrievalProfileState>,
    started_at_ms: f64,
    stream_scoped: bool,
    completed: bool,
}

impl RetrieveAttemptProfile {
    pub(crate) fn complete_immediate(&mut self, outcome: RetrieveAttemptOutcome) {
        self.complete_immediate_at(outcome, now_ms());
    }

    fn complete_immediate_at(&mut self, outcome: RetrieveAttemptOutcome, now_ms: f64) {
        if self.completed {
            return;
        }
        self.completed = true;
        let mut snapshot = self.state.snapshot.borrow_mut();
        snapshot.physical_active = snapshot.physical_active.saturating_sub(1);
        snapshot.physical_immediate_completed += 1;
        snapshot
            .by_scope
            .counters_mut(self.stream_scoped)
            .physical_immediate_completed += 1;
        snapshot.immediate_outcomes.record(outcome);
        snapshot
            .immediate_attempt_ms
            .observe(now_ms - self.started_at_ms);
    }

    pub(crate) fn timed_out(mut self) -> DetachedRetrieveAttemptProfile {
        self.timed_out_at(now_ms())
    }

    fn timed_out_at(&mut self, now_ms: f64) -> DetachedRetrieveAttemptProfile {
        if !self.completed {
            self.completed = true;
            let mut snapshot = self.state.snapshot.borrow_mut();
            snapshot.physical_active = snapshot.physical_active.saturating_sub(1);
            snapshot.physical_timed_out += 1;
            snapshot.detached_outstanding += 1;
            snapshot.detached_high_water = snapshot
                .detached_high_water
                .max(snapshot.detached_outstanding);
            snapshot
                .by_scope
                .counters_mut(self.stream_scoped)
                .physical_timed_out += 1;
        }
        DetachedRetrieveAttemptProfile {
            state: self.state.clone(),
            started_at_ms: self.started_at_ms,
            timed_out_at_ms: now_ms,
            stream_scoped: self.stream_scoped,
            completed: false,
        }
    }

    pub(crate) fn immediate_result_send(&self, succeeded: bool) {
        let mut snapshot = self.state.snapshot.borrow_mut();
        if succeeded {
            snapshot.immediate_result_send_succeeded += 1;
            snapshot
                .by_scope
                .counters_mut(self.stream_scoped)
                .immediate_result_send_succeeded += 1;
        } else {
            snapshot.immediate_result_send_failed += 1;
            snapshot
                .by_scope
                .counters_mut(self.stream_scoped)
                .immediate_result_send_failed += 1;
        }
    }
}

pub(crate) struct DetachedRetrieveAttemptProfile {
    state: Rc<RetrievalProfileState>,
    started_at_ms: f64,
    timed_out_at_ms: f64,
    stream_scoped: bool,
    completed: bool,
}

impl DetachedRetrieveAttemptProfile {
    pub(crate) fn complete(mut self, outcome: RetrieveAttemptOutcome) {
        self.complete_at(outcome, now_ms());
    }

    fn complete_at(&mut self, outcome: RetrieveAttemptOutcome, now_ms: f64) {
        if self.completed {
            return;
        }
        self.completed = true;
        let mut snapshot = self.state.snapshot.borrow_mut();
        snapshot.detached_outstanding = snapshot.detached_outstanding.saturating_sub(1);
        snapshot.detached_completed += 1;
        snapshot
            .by_scope
            .counters_mut(self.stream_scoped)
            .detached_completed += 1;
        snapshot.detached_outcomes.record(outcome);
        snapshot
            .detached_after_timeout_ms
            .observe(now_ms - self.timed_out_at_ms);
        snapshot
            .detached_total_attempt_ms
            .observe(now_ms - self.started_at_ms);
    }

    pub(crate) fn timeout_result_send(&self, succeeded: bool) {
        let mut snapshot = self.state.snapshot.borrow_mut();
        if succeeded {
            snapshot.timeout_result_send_succeeded += 1;
            snapshot
                .by_scope
                .counters_mut(self.stream_scoped)
                .timeout_result_send_succeeded += 1;
        } else {
            snapshot.timeout_result_send_failed += 1;
            snapshot
                .by_scope
                .counters_mut(self.stream_scoped)
                .timeout_result_send_failed += 1;
        }
    }
}

enum RetrievalProfileActivation {
    Inactive,
    Active(Rc<RetrievalProfileState>),
}

thread_local! {
    static RETRIEVAL_PROFILE: RefCell<RetrievalProfileActivation> =
        const { RefCell::new(RetrievalProfileActivation::Inactive) };
    static RETRIEVAL_PROFILE_PERMIT_CAPACITY: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn set_permit_capacity(permit_capacity: usize) {
    let permit_capacity = permit_capacity as u64;
    RETRIEVAL_PROFILE_PERMIT_CAPACITY.set(permit_capacity);
    RETRIEVAL_PROFILE.with(|profile| {
        if let RetrievalProfileActivation::Active(active) = &*profile.borrow() {
            active.snapshot.borrow_mut().permit_capacity = permit_capacity;
        }
    });
}

pub(crate) fn activate() {
    RETRIEVAL_PROFILE.with(|profile| {
        if matches!(&*profile.borrow(), RetrievalProfileActivation::Active(_)) {
            return;
        }
        let active = Rc::new(RetrievalProfileState::new(
            now_ms(),
            RETRIEVAL_PROFILE_PERMIT_CAPACITY.get(),
        ));
        *profile.borrow_mut() = RetrievalProfileActivation::Active(active);
    });
}

pub(crate) fn request_for_enqueue(stream_scoped: bool) -> Option<RetrieveProfileRequest> {
    RETRIEVAL_PROFILE.with(|profile| {
        let active = match &*profile.borrow() {
            RetrievalProfileActivation::Active(active) => active.clone(),
            RetrievalProfileActivation::Inactive => return None,
        };
        Some(active.request(now_ms(), stream_scoped))
    })
}

fn active_profile_state() -> Option<Rc<RetrievalProfileState>> {
    RETRIEVAL_PROFILE.with(|profile| match &*profile.borrow() {
        RetrievalProfileActivation::Active(active) => Some(active.clone()),
        RetrievalProfileActivation::Inactive => None,
    })
}

pub(crate) fn rolling_group_started(init: RollingGroupProfileInit) -> Option<RollingGroupProfile> {
    active_profile_state().and_then(|state| state.rolling_group(init))
}

pub(crate) fn snapshot_now() -> Option<RetrievalProfileSnapshot> {
    active_profile_state().map(|state| state.snapshot(now_ms()))
}

pub(crate) fn finalize_rolling_group_trace_now() -> Option<RollingGroupTraceSnapshot> {
    active_profile_state().map(|state| state.finalize_rolling_group_trace(now_ms()))
}

pub(crate) fn record_managed_to_ordinary_promotion() {
    let Some(state) = active_profile_state() else {
        return;
    };
    if !state.rolling_group_trace_recording() {
        return;
    }
    state.managed_to_ordinary_promotion(now_ms());
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    0.0
}

#[cfg(test)]
pub(crate) fn test_profile(
    activation_at_ms: f64,
    permit_capacity: u64,
) -> Rc<RetrievalProfileState> {
    Rc::new(RetrievalProfileState::new(
        activation_at_ms,
        permit_capacity,
    ))
}

#[cfg(test)]
pub(crate) fn test_request(
    state: &Rc<RetrievalProfileState>,
    enqueued_at_ms: f64,
    stream_scoped: bool,
) -> RetrieveProfileRequest {
    state.request(enqueued_at_ms, stream_scoped)
}

#[cfg(test)]
pub(crate) fn test_snapshot(
    state: &RetrievalProfileState,
    snapshot_at_ms: f64,
) -> RetrievalProfileSnapshot {
    state.snapshot(snapshot_at_ms)
}

#[cfg(test)]
pub(crate) fn test_rolling_group(
    state: &Rc<RetrievalProfileState>,
    init: RollingGroupProfileInit,
) -> Option<RollingGroupProfile> {
    state.rolling_group(init)
}

#[cfg(test)]
pub(crate) fn test_finalize_rolling_group_trace(
    state: &RetrievalProfileState,
    snapshot_at_ms: f64,
) -> RollingGroupTraceSnapshot {
    state.finalize_rolling_group_trace(snapshot_at_ms)
}

#[cfg(test)]
pub(crate) fn test_managed_to_ordinary_promotion(state: &RetrievalProfileState, at_ms: f64) {
    state.managed_to_ordinary_promotion(at_ms);
}

#[cfg(test)]
pub(crate) fn test_log2_histogram(values_ms: &[f64]) -> Log2MillisecondsHistogram {
    let mut histogram = Log2MillisecondsHistogram::default();
    for value_ms in values_ms {
        histogram.observe(*value_ms);
    }
    histogram
}

#[cfg(test)]
pub(crate) fn test_reject_before_permit(request: &RetrieveProfileRequest, now_ms: f64) {
    request.reject_before_permit_at(now_ms);
}

#[cfg(test)]
pub(crate) fn test_begin_permit_wait(request: &RetrieveProfileRequest, now_ms: f64) {
    request.begin_permit_wait_at(now_ms);
}

#[cfg(test)]
pub(crate) fn test_permit_aborted(request: &RetrieveProfileRequest, now_ms: f64) {
    request.permit_aborted_at(now_ms);
}

#[cfg(test)]
pub(crate) fn test_permit_acquired(
    request: &RetrieveProfileRequest,
    now_ms: f64,
) -> RetrieveProfilePermit {
    request.permit_acquired_at(now_ms)
}

#[cfg(test)]
pub(crate) fn test_physical_attempt(
    request: &RetrieveProfileRequest,
    now_ms: f64,
) -> RetrieveAttemptProfile {
    request.physical_attempt_at(now_ms)
}

#[cfg(test)]
pub(crate) fn test_complete_immediate(
    mut attempt: RetrieveAttemptProfile,
    outcome: RetrieveAttemptOutcome,
    now_ms: f64,
    result_send_succeeded: bool,
) {
    attempt.complete_immediate_at(outcome, now_ms);
    attempt.immediate_result_send(result_send_succeeded);
}

#[cfg(test)]
pub(crate) fn test_timed_out(
    mut attempt: RetrieveAttemptProfile,
    now_ms: f64,
    result_send_succeeded: bool,
) -> DetachedRetrieveAttemptProfile {
    let detached = attempt.timed_out_at(now_ms);
    detached.timeout_result_send(result_send_succeeded);
    detached
}

#[cfg(test)]
pub(crate) fn test_complete_detached(
    mut attempt: DetachedRetrieveAttemptProfile,
    outcome: RetrieveAttemptOutcome,
    now_ms: f64,
) {
    attempt.complete_at(outcome, now_ms);
}
