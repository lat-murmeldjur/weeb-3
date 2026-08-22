use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use crate::{retrieval_conventions::next_nonzero_generation, stream_conventions::HlsStart};

#[cfg(target_arch = "wasm32")]
mod retrieval;

const HLS_HEADER: &str = "#EXTM3U";
const HLS_ENDLIST: &str = "#EXT-X-ENDLIST";
const HLS_GAP: &str = "#EXT-X-GAP";
const HLS_SERVER_CONTROL: &str = "#EXT-X-SERVER-CONTROL";
pub(crate) const HLS_LIVE_SYNC_SEGMENTS: usize = 8;
const HLS_LIVE_TAIL_FAILURE_THRESHOLD: u8 = 2;
const HLS_LIVE_STARTUP_PREROLL_SEGMENTS: usize = 2;

pub(crate) const HLS_SPARSE_HISTORY_STRIDE: u64 = 10;
pub(crate) const HLS_SPARSE_HISTORY_MAX_SEGMENTS: usize = 32_768;
pub(crate) const HLS_SPARSE_HISTORY_MAX_PROBES: usize = 4_096;
pub(crate) const HLS_SPARSE_HISTORY_MAX_REPAIRS: usize = 4_096;
pub(crate) const HLS_SPARSE_HISTORY_MAX_CANDIDATES: usize =
    HLS_SPARSE_HISTORY_MAX_PROBES + HLS_SPARSE_HISTORY_MAX_REPAIRS;
pub(crate) const HLS_SPARSE_HISTORY_MAX_WINDOW_BYTES: usize = 64 * 1024;
pub(crate) const HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const HLS_SPARSE_HISTORY_MAX_PARALLEL: usize = 64;
pub(crate) const HLS_SEQUENCE_ZERO_RECOVERY_BATCH: usize = 4;
pub(crate) const HLS_SEQUENCE_ZERO_DISCOVERY_MAX_PROBES: usize = 8;
pub(crate) const HLS_SEQUENCE_ZERO_DISCOVERY_MAX_GEOMETRIC_PROBES: usize = 4;
const HLS_SPARSE_HISTORY_CAPACITY_REFRESH_MS: f64 = 100.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsSequenceZeroDiscoveryObservation {
    Ready,
    Underfilled,
    Overshoot,
    Unusable,
}

pub(crate) fn classify_hls_sequence_zero_discovery(
    bytes: &[u8],
    minimum_segments: usize,
) -> HlsSequenceZeroDiscoveryObservation {
    let Some(timeline) = HlsTimeline::parse(bytes) else {
        return HlsSequenceZeroDiscoveryObservation::Unusable;
    };
    match timeline.sequence {
        0 if timeline.segments.len() >= minimum_segments => {
            HlsSequenceZeroDiscoveryObservation::Ready
        }
        0 => HlsSequenceZeroDiscoveryObservation::Underfilled,
        _ => HlsSequenceZeroDiscoveryObservation::Overshoot,
    }
}

#[derive(Debug)]
pub(crate) struct HlsSequenceZeroDiscoveryPlanner {
    next_exponent: u32,
    lower_underfilled: Option<u64>,
    upper_overshoot: Option<u64>,
    geometric_launched: usize,
    launched: HashSet<u64>,
}

impl HlsSequenceZeroDiscoveryPlanner {
    pub(crate) fn new() -> Self {
        Self {
            next_exponent: 2,
            lower_underfilled: None,
            upper_overshoot: None,
            geometric_launched: 0,
            launched: HashSet::new(),
        }
    }

    pub(crate) fn observe(&mut self, index: u64, observation: HlsSequenceZeroDiscoveryObservation) {
        match observation {
            HlsSequenceZeroDiscoveryObservation::Underfilled => {
                self.lower_underfilled = Some(
                    self.lower_underfilled
                        .map_or(index, |lower| lower.max(index)),
                );
            }
            HlsSequenceZeroDiscoveryObservation::Overshoot => {
                self.upper_overshoot =
                    Some(self.upper_overshoot.map_or(index, |upper| upper.min(index)));
            }
            HlsSequenceZeroDiscoveryObservation::Ready
            | HlsSequenceZeroDiscoveryObservation::Unusable => {}
        }
    }

    pub(crate) fn next_probe(&mut self) -> Option<u64> {
        if self.launched.len() >= HLS_SEQUENCE_ZERO_DISCOVERY_MAX_PROBES {
            return None;
        }

        if let Some(upper) = self.upper_overshoot {
            // An update can publish several segments at once, so even index zero may be a useful
            // sequence-zero checkpoint. Unusable observations establish no bound: refine the
            // complete interval below the first known overshoot unless a real underfilled lower
            // bound exists.
            let lower_inclusive = self
                .lower_underfilled
                .and_then(|lower| lower.checked_add(1))
                .unwrap_or(0);
            if upper <= lower_inclusive {
                return None;
            }
            let midpoint = lower_inclusive + (upper - lower_inclusive) / 2;
            for distance in 0..upper - lower_inclusive {
                for candidate in [
                    midpoint.checked_sub(distance),
                    midpoint.checked_add(distance),
                ] {
                    let Some(candidate) = candidate else {
                        continue;
                    };
                    if candidate >= lower_inclusive
                        && candidate < upper
                        && self.launched.insert(candidate)
                    {
                        return Some(candidate);
                    }
                }
            }
            return None;
        }

        // Keep half of the bounded probe budget available for binary refinement if a geometric
        // checkpoint has already crossed the sequence-zero boundary.
        if self.geometric_launched >= HLS_SEQUENCE_ZERO_DISCOVERY_MAX_GEOMETRIC_PROBES {
            return None;
        }
        while self.next_exponent < u64::BITS {
            let exponent = self.next_exponent;
            self.next_exponent += 1;
            let index = 1_u64.checked_shl(exponent)?.checked_sub(1)?;
            if self.launched.insert(index) {
                self.geometric_launched = self.geometric_launched.saturating_add(1);
                return Some(index);
            }
        }
        None
    }
}

impl Default for HlsSequenceZeroDiscoveryPlanner {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn hls_sparse_history_parallelism(
    priced_peers: u64,
    available_retrieve_slots: u64,
) -> usize {
    let peer_cap = usize::try_from(priced_peers)
        .unwrap_or(usize::MAX)
        .saturating_mul(4)
        .min(HLS_SPARSE_HISTORY_MAX_PARALLEL);
    (1..=peer_cap)
        .rev()
        .find(|parallel| {
            u64::try_from(*parallel).ok().is_some_and(|parallel| {
                parallel
                    .saturating_mul(2)
                    .saturating_add((parallel / 2).min(64))
                    <= available_retrieve_slots
            })
        })
        .unwrap_or(0)
}

pub(crate) fn plan_hls_sparse_forward_wave(
    lattice_origin: u64,
    first_slot: u64,
    width: usize,
) -> Option<Vec<u64>> {
    let width = width.min(HLS_SPARSE_HISTORY_MAX_PARALLEL);
    (0..width)
        .map(|position| {
            first_slot
                .checked_add(u64::try_from(position).ok()?)?
                .checked_mul(HLS_SPARSE_HISTORY_STRIDE)?
                .checked_add(lattice_origin)
        })
        .collect()
}

pub(crate) fn plan_hls_sparse_terminal_repairs(head_index: u64) -> Option<Vec<u64>> {
    let first_lattice = head_index.checked_add(HLS_SPARSE_HISTORY_STRIDE)?;
    let second_lattice = first_lattice.checked_add(HLS_SPARSE_HISTORY_STRIDE)?;
    Some(
        (head_index.checked_add(1)?..second_lattice)
            .filter(|index| *index != first_lattice)
            .collect(),
    )
}

pub(crate) fn plan_hls_sequence_zero_followup_recovery(
    head_index: u64,
    recovery_cursor: u64,
    retry_index: Option<u64>,
) -> Option<(Vec<u64>, u64)> {
    let recovery_start = head_index.checked_add(recovery_cursor)?;
    let mut targets = plan_hls_sparse_terminal_repairs(head_index)?;
    for offset in 0..HLS_SEQUENCE_ZERO_RECOVERY_BATCH {
        let index = recovery_start.checked_add(u64::try_from(offset).ok()?)?;
        if !targets.contains(&index) {
            targets.push(index);
        }
    }
    if let Some(retry_index) = retry_index
        && !targets.contains(&retry_index)
    {
        targets.push(retry_index);
    }
    let next_recovery_cursor =
        recovery_cursor.checked_add(u64::try_from(HLS_SEQUENCE_ZERO_RECOVERY_BATCH).ok()?)?;
    Some((targets, next_recovery_cursor))
}

pub(crate) fn plan_hls_sequence_zero_terminal_confirmation(head_index: u64) -> Option<Vec<u64>> {
    (1..=HLS_SPARSE_HISTORY_STRIDE.checked_mul(2)?)
        .map(|offset| head_index.checked_add(offset))
        .collect()
}

pub(crate) fn hls_tail_has_terminal_endlist(bytes: &[u8]) -> bool {
    bytes
        .split(|byte| *byte == b'\n')
        .rev()
        .find(|line| !line.iter().all(u8::is_ascii_whitespace))
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .is_some_and(|line| line == HLS_ENDLIST.as_bytes())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HlsSequenceZeroRetry {
    pub(crate) index: u64,
    pub(crate) authenticated: bool,
}

pub(crate) fn remember_hls_sequence_zero_retry(
    retries: &mut VecDeque<HlsSequenceZeroRetry>,
    index: u64,
    authenticated: bool,
    priority: bool,
    capacity: usize,
) -> bool {
    if let Some(position) = retries.iter().position(|current| current.index == index) {
        let authenticated = retries[position].authenticated || authenticated;
        if priority && position > 0 {
            retries.remove(position);
            retries.push_front(HlsSequenceZeroRetry {
                index,
                authenticated,
            });
        } else {
            retries[position].authenticated = authenticated;
        }
        return true;
    }
    if retries.len() >= capacity {
        return false;
    }
    let retry = HlsSequenceZeroRetry {
        index,
        authenticated,
    };
    if priority {
        retries.push_front(retry);
    } else {
        retries.push_back(retry);
    }
    true
}

pub(crate) fn retain_hls_sequence_zero_retries_after(
    retries: &mut VecDeque<HlsSequenceZeroRetry>,
    head_index: u64,
) {
    retries.retain(|retry| retry.index > head_index);
}

pub(crate) fn hls_sequence_zero_ordinary_retry(
    retries: &VecDeque<HlsSequenceZeroRetry>,
    deferred_present: bool,
) -> Option<HlsSequenceZeroRetry> {
    if deferred_present {
        retries.iter().copied().find(|retry| retry.authenticated)
    } else {
        retries.front().copied()
    }
}

pub(crate) fn select_hls_sequence_zero_retry(
    deferred_retry_index: Option<u64>,
    ordinary_retry: Option<HlsSequenceZeroRetry>,
    deferred_first: bool,
) -> Option<u64> {
    match (deferred_retry_index, ordinary_retry) {
        (Some(_), Some(ordinary)) if ordinary.authenticated && !deferred_first => {
            Some(ordinary.index)
        }
        (Some(deferred), _) => Some(deferred),
        (None, ordinary) => ordinary.map(|retry| retry.index),
    }
}

pub(crate) fn hls_sequence_zero_retry_stays_queued(
    authenticated: bool,
    transient: bool,
    unavailable_or_unsupported: bool,
    transferred_to_deferred: bool,
) -> bool {
    transient || (authenticated && unavailable_or_unsupported && !transferred_to_deferred)
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static HLS_VIDEO_RESOLUTION: std::cell::Cell<Option<(u32, u32)>> = const {
        std::cell::Cell::new(None)
    };
}

#[cfg(target_arch = "wasm32")]
fn set_hls_video_resolution(resolution: Option<(u32, u32)>) {
    HLS_VIDEO_RESOLUTION.with(|current| current.set(resolution));
}

#[cfg(target_arch = "wasm32")]
fn hls_video_resolution() -> Option<(u32, u32)> {
    HLS_VIDEO_RESOLUTION.with(std::cell::Cell::get)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsManifestProbe {
    Manifest,
    NotManifest,
    NeedMore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HlsMediaPlan {
    pub(crate) id: u64,
    pub(crate) references: Arc<[String]>,
    pub(crate) early_overlap_limit: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HlsMediaCursor {
    pub(crate) plan: Arc<HlsMediaPlan>,
    pub(crate) position: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HlsMediaSelection {
    pub(crate) cursor: HlsMediaCursor,
    pub(crate) superseded_plan_ids: Vec<u64>,
}

pub(crate) const HLS_AUTOPLAY_BUFFER_SECONDS: f64 = 2.0;
const HLS_BUFFER_EDGE_EPSILON_SECONDS: f64 = 0.05;

pub(crate) fn hls_autoplay_start_target(
    start: HlsStart,
    requested_position: f64,
    current_time: f64,
    live_sync_position: Option<f64>,
    ranges: &[(f64, f64)],
) -> Option<f64> {
    let valid_ranges = ranges.iter().copied().filter(|(range_start, range_end)| {
        range_start.is_finite()
            && range_end.is_finite()
            && *range_start >= 0.0
            && range_end > range_start
    });
    let target_in_or_before_range = |target: f64| -> Option<f64> {
        if !target.is_finite() || target < 0.0 {
            return None;
        }
        valid_ranges
            .clone()
            .find(|(_, range_end)| *range_end + HLS_BUFFER_EDGE_EPSILON_SECONDS >= target)
            .map(|(range_start, _)| target.max(range_start))
    };
    match start {
        HlsStart::Beginning => target_in_or_before_range(requested_position.max(0.0)),
        HlsStart::Live => {
            let (range_start, range_end) = valid_ranges.last()?;
            let in_newest_range = |target: f64| {
                (target.is_finite()
                    && target >= range_start - HLS_BUFFER_EDGE_EPSILON_SECONDS
                    && target < range_end)
                    .then_some(target.max(range_start))
            };
            live_sync_position
                .and_then(in_newest_range)
                .or_else(|| in_newest_range(current_time))
                .or(Some(range_start))
        }
    }
}

pub(crate) fn hls_live_retarget_target(
    resume_position: Option<f64>,
    live_sync_position: Option<f64>,
    finalized_playable_interval: Option<(f64, f64)>,
    ranges: &[(f64, f64)],
) -> Option<f64> {
    if let Some(target) = resume_position.filter(|target| target.is_finite() && *target >= 0.0)
        && let Some((range_start, _)) = ranges.iter().copied().find(|(start, end)| {
            start.is_finite()
                && end.is_finite()
                && *start >= 0.0
                && end > start
                && target >= *start - HLS_BUFFER_EDGE_EPSILON_SECONDS
                && target < *end
        })
    {
        return Some(target.max(range_start));
    }
    let (range_start, range_end) = ranges.iter().copied().rev().find(|(start, end)| {
        start.is_finite() && end.is_finite() && *start >= 0.0 && end > start
    })?;
    if let Some(target) = live_sync_position.filter(|target| target.is_finite() && *target >= 0.0)
        && target >= range_start - HLS_BUFFER_EDGE_EPSILON_SECONDS
        && target < range_end
    {
        return Some(target.max(range_start));
    }
    let (playable_start, playable_end) = finalized_playable_interval.filter(|(start, end)| {
        start.is_finite() && end.is_finite() && *start >= 0.0 && end > start
    })?;
    (range_end + HLS_BUFFER_EDGE_EPSILON_SECONDS >= playable_end
        && range_end > playable_start
        && range_start < playable_end)
        .then_some(
            (playable_end - HLS_AUTOPLAY_BUFFER_SECONDS)
                .max(playable_start)
                .max(range_start)
                .min(range_end),
        )
}

pub(crate) fn hls_finalized_edge_load_target(playable_interval: Option<(f64, f64)>) -> Option<f64> {
    let (playable_start, playable_end) = playable_interval.filter(|(start, end)| {
        start.is_finite() && end.is_finite() && *start >= 0.0 && end > start
    })?;
    Some(
        (playable_end - HLS_AUTOPLAY_BUFFER_SECONDS)
            .max(playable_start)
            .min(playable_end),
    )
}

pub(crate) fn hls_buffered_interval_covers(
    start: f64,
    duration: f64,
    ranges: &[(f64, f64)],
) -> bool {
    const TIMESTAMP_TOLERANCE_SECONDS: f64 = 0.25;

    if !start.is_finite() || !duration.is_finite() || start < 0.0 || duration <= 0.0 {
        return false;
    }
    let end = start + duration;
    end.is_finite()
        && ranges.iter().copied().any(|(range_start, range_end)| {
            range_start.is_finite()
                && range_end.is_finite()
                && range_start >= 0.0
                && range_end > range_start
                && range_start <= start + TIMESTAMP_TOLERANCE_SECONDS
                && range_end + TIMESTAMP_TOLERANCE_SECONDS >= end
        })
}

pub(crate) fn hls_contiguous_buffered_ahead(current_time: f64, ranges: &[(f64, f64)]) -> f64 {
    if !current_time.is_finite() || current_time < 0.0 {
        return 0.0;
    }
    let mut edge = current_time;
    for &(start, end) in ranges {
        if !start.is_finite() || !end.is_finite() || end <= start {
            continue;
        }
        if end + HLS_BUFFER_EDGE_EPSILON_SECONDS < edge {
            continue;
        }
        if start > edge + HLS_BUFFER_EDGE_EPSILON_SECONDS {
            break;
        }
        edge = edge.max(end);
    }
    (edge - current_time).max(0.0)
}

pub(crate) fn hls_autoplay_gate_ready(
    buffered_ahead: f64,
    current_time: f64,
    duration: f64,
    finalized: bool,
) -> bool {
    buffered_ahead >= HLS_AUTOPLAY_BUFFER_SECONDS
        || (finalized
            && buffered_ahead > 0.0
            && current_time.is_finite()
            && current_time >= 0.0
            && duration.is_finite()
            && duration > current_time
            && current_time + buffered_ahead + HLS_BUFFER_EDGE_EPSILON_SECONDS >= duration)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum HlsMediaKind {
    Main,
    Audio,
}

pub(crate) fn hls_select_startup_runway_plan(
    active: &mut HashMap<HlsMediaKind, u64>,
    kind: HlsMediaKind,
    plan_id: Option<u64>,
) -> Option<u64> {
    let previous = match plan_id {
        Some(plan_id) => active.insert(kind, plan_id),
        None => active.remove(&kind),
    };
    let retired = previous.filter(|previous| {
        Some(*previous) != plan_id && !active.values().any(|active| *active == *previous)
    });
    retired
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsPlayAttemptPhase {
    Pending,
    Rejecting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsPlayAttemptSettlement {
    Stale,
    Superseded,
    Commit,
    Rollback,
}

pub(crate) fn hls_play_attempt_settlement(
    active: Option<(u64, HlsPlayAttemptPhase)>,
    token: u64,
    authorized: bool,
    snapshot_current: bool,
) -> HlsPlayAttemptSettlement {
    if active != Some((token, HlsPlayAttemptPhase::Pending)) {
        return HlsPlayAttemptSettlement::Stale;
    }
    if !snapshot_current {
        return HlsPlayAttemptSettlement::Superseded;
    }
    if authorized {
        HlsPlayAttemptSettlement::Commit
    } else {
        HlsPlayAttemptSettlement::Rollback
    }
}

pub(crate) fn hls_dom_play_is_explicit(
    attempt: Option<HlsPlayAttemptPhase>,
    media_paused: bool,
    playback_authorized: bool,
) -> bool {
    attempt.is_none() && !media_paused && !playback_authorized
}

pub(crate) fn hls_dom_pause_is_explicit(
    attempt: Option<HlsPlayAttemptPhase>,
    playback_authorized: bool,
) -> bool {
    attempt.is_none() && playback_authorized
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsPrefetchMode {
    Inactive,
    StartupOnly,
    Sustained,
}

pub(crate) const HLS_BACKGROUND_RANGE_MAX: usize = 4;
pub(crate) const HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX: usize = 1;
pub(crate) const HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX: usize =
    HLS_BACKGROUND_RANGE_MAX - HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX;
pub(crate) const HLS_BEGINNING_PREFIX_MAX_WINDOWS: usize = 8;
const HLS_BEGINNING_PREFIX_SAFETY_SECONDS: f64 = 0.25;

pub(crate) fn hls_startup_retry_delay_ms(attempt: u32) -> u64 {
    75_u64.saturating_mul(1_u64 << attempt.min(4)).min(1_000)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsBeginningPrefixPhase {
    Disabled,
    AwaitManifest,
    AwaitForegroundZero,
    Supplying,
    Ready,
    Bypass,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HlsCriticalPrefixPlan {
    total_windows: usize,
    critical_windows: usize,
}

impl HlsCriticalPrefixPlan {
    pub(crate) fn new(payload_size: u64, duration_seconds: f64, window_bytes: u64) -> Option<Self> {
        if payload_size == 0
            || window_bytes == 0
            || !duration_seconds.is_finite()
            || duration_seconds <= 0.0
        {
            return None;
        }
        let total_windows = usize::try_from(payload_size.saturating_sub(1) / window_bytes + 1)
            .unwrap_or(usize::MAX);
        let protected_seconds =
            duration_seconds.min(HLS_AUTOPLAY_BUFFER_SECONDS + HLS_BEGINNING_PREFIX_SAFETY_SECONDS);
        let protected_bytes = ((payload_size as f64) * protected_seconds / duration_seconds)
            .ceil()
            .max(1.0) as u64;
        let critical_windows =
            usize::try_from(protected_bytes.saturating_sub(1) / window_bytes + 1)
                .unwrap_or(usize::MAX)
                .max(1)
                .min(total_windows)
                .min(HLS_BEGINNING_PREFIX_MAX_WINDOWS);
        Some(Self {
            total_windows,
            critical_windows,
        })
    }

    pub(crate) fn total_windows(self) -> usize {
        self.total_windows
    }

    pub(crate) fn critical_windows(self) -> usize {
        self.critical_windows
    }
}

pub(crate) fn hls_beginning_prefix_window_count(
    payload_size: u64,
    duration_seconds: f64,
    window_bytes: u64,
) -> usize {
    HlsCriticalPrefixPlan::new(payload_size, duration_seconds, window_bytes)
        .map_or(0, HlsCriticalPrefixPlan::critical_windows)
}

pub(crate) fn hls_beginning_prefix_admission(
    phase: HlsBeginningPrefixPhase,
) -> HlsProgressiveRangeAdmission {
    match phase {
        HlsBeginningPrefixPhase::Disabled
        | HlsBeginningPrefixPhase::Ready
        | HlsBeginningPrefixPhase::Bypass => HlsProgressiveRangeAdmission::Admit,
        HlsBeginningPrefixPhase::Retired => HlsProgressiveRangeAdmission::Retire,
        HlsBeginningPrefixPhase::AwaitManifest
        | HlsBeginningPrefixPhase::AwaitForegroundZero
        | HlsBeginningPrefixPhase::Supplying => HlsProgressiveRangeAdmission::Park,
    }
}

pub(crate) fn hls_beginning_raw_supply_admission(
    phase: HlsBeginningPrefixPhase,
    foreground_zero_requested: bool,
) -> HlsProgressiveRangeAdmission {
    match (phase, foreground_zero_requested) {
        (HlsBeginningPrefixPhase::AwaitForegroundZero, false) => HlsProgressiveRangeAdmission::Park,
        (HlsBeginningPrefixPhase::Supplying, true) => HlsProgressiveRangeAdmission::Admit,
        _ => HlsProgressiveRangeAdmission::Retire,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsProgressiveRangeAdmission {
    Retire,
    Park,
    Admit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsProgressiveRangePurpose {
    StartupExactNext,
    Sustained,
}

pub(crate) fn hls_progressive_range_admission(
    structurally_current: bool,
    mode: HlsPrefetchMode,
    purpose: HlsProgressiveRangePurpose,
) -> HlsProgressiveRangeAdmission {
    if !structurally_current {
        HlsProgressiveRangeAdmission::Retire
    } else {
        match mode {
            HlsPrefetchMode::Sustained => HlsProgressiveRangeAdmission::Admit,
            HlsPrefetchMode::StartupOnly
                if purpose == HlsProgressiveRangePurpose::StartupExactNext =>
            {
                HlsProgressiveRangeAdmission::Admit
            }
            HlsPrefetchMode::Inactive | HlsPrefetchMode::StartupOnly => {
                HlsProgressiveRangeAdmission::Park
            }
        }
    }
}

pub(crate) fn hls_progressive_range_reservation_fits(
    occupied_bytes: u64,
    reserved_bytes: u64,
    requested_bytes: u64,
    limit_bytes: u64,
) -> bool {
    requested_bytes > 0
        && occupied_bytes
            .checked_add(reserved_bytes)
            .and_then(|used| used.checked_add(requested_bytes))
            .is_some_and(|used| used <= limit_bytes)
}

pub(crate) fn hls_progressive_frontier_width(
    launch_position: usize,
    foreground_position: usize,
    initial_successor_complete: bool,
) -> usize {
    if foreground_position > launch_position || initial_successor_complete {
        HLS_BACKGROUND_RANGE_MAX
    } else {
        1
    }
}

pub(crate) fn hls_progressive_rolling_lane_width(
    frontier_width: usize,
    width_cap: usize,
    peer_released: bool,
) -> usize {
    if peer_released {
        HLS_BACKGROUND_RANGE_MAX
    } else {
        frontier_width.min(width_cap)
    }
}

pub(crate) fn hls_progressive_rolling_boundary_width(
    frontier_width: usize,
    first_window_cached: bool,
    current_reference_complete: bool,
    exact_foreground: bool,
) -> usize {
    if current_reference_complete
        || (exact_foreground && frontier_width == HLS_BACKGROUND_RANGE_MAX)
    {
        HLS_BACKGROUND_RANGE_MAX
    } else if first_window_cached {
        0
    } else {
        frontier_width.min(HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsProgressiveFrontierDecision {
    Continue,
    Rebase(usize),
    Advance(usize),
}

pub(crate) fn hls_progressive_frontier_decision(
    active_position: usize,
    foreground_position: usize,
    active_reference_complete: bool,
) -> HlsProgressiveFrontierDecision {
    if foreground_position > active_position {
        HlsProgressiveFrontierDecision::Rebase(foreground_position)
    } else if active_reference_complete {
        HlsProgressiveFrontierDecision::Advance(active_position.saturating_add(1))
    } else {
        HlsProgressiveFrontierDecision::Continue
    }
}

pub(crate) fn hls_progressive_foreground_transition(
    last_foreground_position: usize,
    foreground_position: usize,
    cached: bool,
) -> (bool, usize) {
    if cached && foreground_position < last_foreground_position {
        (false, last_foreground_position)
    } else {
        (
            foreground_position < last_foreground_position
                || last_foreground_position.abs_diff(foreground_position) > 1,
            foreground_position,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsOrderedWindowState {
    Cached,
    Pending,
    Active,
    Backoff,
    Absent,
}

pub(crate) fn hls_ordered_window_admissions(
    states: &[HlsOrderedWindowState],
    width: usize,
    retry_floor: Option<usize>,
) -> Vec<usize> {
    let mut occupied = 0_usize;
    let mut admissions = Vec::new();
    for (window, state) in states.iter().copied().enumerate() {
        if retry_floor.is_some_and(|floor| window > floor) {
            break;
        }
        if state == HlsOrderedWindowState::Cached {
            continue;
        }
        if state == HlsOrderedWindowState::Backoff {
            break;
        }
        if occupied >= width {
            break;
        }
        occupied = occupied.saturating_add(1);
        if state == HlsOrderedWindowState::Absent {
            admissions.push(window);
        }
    }
    admissions
}

pub(crate) fn hls_progressive_runway_closed_after_mode(
    closed: bool,
    mode: HlsPrefetchMode,
) -> bool {
    match mode {
        HlsPrefetchMode::Inactive => true,
        HlsPrefetchMode::Sustained => false,
        HlsPrefetchMode::StartupOnly => closed,
    }
}

pub(crate) fn touch_hls_cache_lru(order: &mut VecDeque<String>, reference: &str, foreground: bool) {
    order.retain(|key| key != reference);
    if foreground {
        order.push_back(reference.to_string());
    } else {
        order.push_front(reference.to_string());
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsProgressiveRunwayTransition {
    Current,
    Sequential,
    Discontinuity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HlsProgressiveRunway {
    current: String,
    successor: Option<String>,
}

impl HlsProgressiveRunway {
    pub(crate) fn new(current: String, successor: Option<String>) -> Self {
        Self { current, successor }
    }

    pub(crate) fn current(&self) -> &str {
        &self.current
    }

    pub(crate) fn successor(&self) -> Option<&str> {
        self.successor.as_deref()
    }

    pub(crate) fn advance(
        &mut self,
        reference: &str,
        successor: Option<String>,
    ) -> HlsProgressiveRunwayTransition {
        if self.current == reference {
            self.successor = successor;
            return HlsProgressiveRunwayTransition::Current;
        }
        let transition = if self.successor.as_deref() == Some(reference) {
            HlsProgressiveRunwayTransition::Sequential
        } else {
            HlsProgressiveRunwayTransition::Discontinuity
        };
        self.current = reference.to_string();
        self.successor = successor;
        transition
    }
}

#[derive(Default)]
pub(crate) struct HlsProgressiveRunways {
    startup: Option<HlsProgressiveRunway>,
    plans: HashMap<u64, HlsProgressiveRunway>,
}

impl HlsProgressiveRunways {
    pub(crate) fn set_startup(&mut self, runway: HlsProgressiveRunway) {
        self.startup = Some(runway);
    }

    pub(crate) fn startup_contains(&self, reference: &str) -> bool {
        self.startup.as_ref().is_some_and(|runway| {
            runway.current() == reference || runway.successor() == Some(reference)
        })
    }

    pub(crate) fn advance(
        &mut self,
        plan_id: u64,
        reference: &str,
        successor: Option<String>,
    ) -> HlsProgressiveRunwayTransition {
        if !self.plans.contains_key(&plan_id) {
            let runway = if self.startup_contains(reference) {
                self.startup.take().expect("matching startup runway")
            } else {
                HlsProgressiveRunway::new(reference.to_string(), successor.clone())
            };
            self.plans.insert(plan_id, runway);
        }
        self.plans
            .get_mut(&plan_id)
            .expect("plan runway inserted above")
            .advance(reference, successor)
    }

    pub(crate) fn contains(&self, plan_id: u64, reference: &str) -> bool {
        self.plans.get(&plan_id).is_some_and(|runway| {
            runway.current() == reference || runway.successor() == Some(reference)
        })
    }

    pub(crate) fn current(&self, plan_id: u64, reference: &str) -> bool {
        self.plans
            .get(&plan_id)
            .is_some_and(|runway| runway.current() == reference)
    }

    pub(crate) fn remove(&mut self, plan_id: u64) {
        self.plans.remove(&plan_id);
    }

    pub(crate) fn clear(&mut self) {
        self.startup = None;
        self.plans.clear();
    }
}

pub(crate) struct HlsMediaPlanRegistry {
    max_references: usize,
    next_plan_id: u64,
    plan_order: VecDeque<u64>,
    cursors: HashMap<String, Vec<HlsMediaCursor>>,
}

pub(crate) struct HlsEvictedMediaPlan {
    id: u64,
    references: Arc<[String]>,
}

const HLS_MEDIA_PLAN_REGISTRY_MAX_PLANS: usize = 16;
const HLS_MEDIA_PLAN_ACTIVE_TRACKS: usize = 4;

impl HlsMediaPlanRegistry {
    pub(crate) fn new(max_references: usize) -> Self {
        Self {
            max_references: max_references.max(1),
            next_plan_id: 0,
            plan_order: VecDeque::new(),
            cursors: HashMap::new(),
        }
    }

    fn resize(&mut self, max_references: usize) {
        let max_references = max_references.max(1);
        if self.max_references != max_references {
            self.max_references = max_references;
        }
    }

    pub(crate) fn install_with_early_overlap_limit(
        &mut self,
        mut references: Vec<String>,
        early_overlap_limit: usize,
        retain_tail: bool,
        protected_plan_ids: &HashSet<u64>,
    ) -> Vec<HlsEvictedMediaPlan> {
        if retain_tail && references.len() > self.max_references {
            references.drain(..references.len() - self.max_references);
        } else {
            references.truncate(self.max_references);
        }
        if references.is_empty()
            || self.cursors.get(&references[0]).is_some_and(|candidates| {
                candidates.iter().any(|cursor| {
                    cursor.position == 0
                        && cursor.plan.references.as_ref() == references
                        && cursor.plan.early_overlap_limit == early_overlap_limit
                })
            })
        {
            return Vec::new();
        }

        let compatible = self.compatible_plan_ids(&references);
        let retained_predecessor = compatible.iter().copied().max();
        let mut evicted = Vec::new();
        for plan_id in compatible {
            if Some(plan_id) != retained_predecessor && !protected_plan_ids.contains(&plan_id) {
                if let Some(references) = self.remove_plan(plan_id) {
                    evicted.push(HlsEvictedMediaPlan {
                        id: plan_id,
                        references,
                    });
                }
            }
        }
        while self.plan_order.len() >= HLS_MEDIA_PLAN_REGISTRY_MAX_PLANS {
            let Some(plan_id) = self.plan_order.iter().copied().find(|plan_id| {
                !protected_plan_ids.contains(plan_id) && Some(*plan_id) != retained_predecessor
            }) else {
                return evicted;
            };
            if let Some(references) = self.remove_plan(plan_id) {
                evicted.push(HlsEvictedMediaPlan {
                    id: plan_id,
                    references,
                });
            }
        }

        self.next_plan_id = next_nonzero_generation(self.next_plan_id);
        let plan = Arc::new(HlsMediaPlan {
            id: self.next_plan_id,
            references: references.into(),
            early_overlap_limit,
        });
        self.plan_order.push_back(plan.id);
        for (position, reference) in plan.references.iter().enumerate() {
            self.cursors
                .entry(reference.clone())
                .or_default()
                .push(HlsMediaCursor {
                    plan: plan.clone(),
                    position,
                });
        }
        evicted
    }

    fn compatible_plan_ids(&self, references: &[String]) -> HashSet<u64> {
        let mut compatible = HashSet::new();
        for (position, reference) in references.iter().enumerate() {
            let Some(candidates) = self.cursors.get(reference) else {
                continue;
            };
            for cursor in candidates {
                let previous_matches = position.checked_sub(1).is_some_and(|position| {
                    cursor
                        .position
                        .checked_sub(1)
                        .is_some_and(|cursor_position| {
                            references.get(position) == cursor.plan.references.get(cursor_position)
                        })
                });
                let next_matches = references
                    .get(position.saturating_add(1))
                    .is_some_and(|next| {
                        cursor
                            .plan
                            .references
                            .get(cursor.position.saturating_add(1))
                            == Some(next)
                    });
                if previous_matches || next_matches {
                    compatible.insert(cursor.plan.id);
                }
            }
        }
        compatible
    }

    pub(crate) fn remove_plans(&mut self, plan_ids: &[u64]) {
        for plan_id in plan_ids {
            let _ = self.remove_plan(*plan_id);
        }
    }

    fn references_for_plans(&self, plan_ids: &[u64]) -> Vec<String> {
        if plan_ids.is_empty() {
            return Vec::new();
        }
        let wanted = plan_ids.iter().copied().collect::<HashSet<_>>();
        let mut found = HashSet::new();
        let mut references = Vec::new();
        for candidates in self.cursors.values() {
            for cursor in candidates {
                if wanted.contains(&cursor.plan.id) && found.insert(cursor.plan.id) {
                    references.extend(cursor.plan.references.iter().cloned());
                    if found.len() == wanted.len() {
                        return references;
                    }
                }
            }
        }
        references
    }

    fn remove_plan(&mut self, plan_id: u64) -> Option<Arc<[String]>> {
        let references = self
            .cursors
            .values()
            .flat_map(|candidates| candidates.iter())
            .find(|cursor| cursor.plan.id == plan_id)
            .map(|cursor| cursor.plan.references.clone());
        self.plan_order.retain(|current| *current != plan_id);
        self.cursors.retain(|_, candidates| {
            candidates.retain(|cursor| cursor.plan.id != plan_id);
            !candidates.is_empty()
        });
        references
    }

    pub(crate) fn cursor(
        &self,
        reference: &str,
        preferred: &HashMap<u64, usize>,
    ) -> Option<HlsMediaSelection> {
        let candidates = self.cursors.get(reference)?;
        let preferred_index = candidates
            .iter()
            .enumerate()
            .filter_map(|(index, cursor)| {
                preferred.get(&cursor.plan.id).map(|position| {
                    (
                        cursor.position.abs_diff(*position),
                        usize::MAX - index,
                        index,
                    )
                })
            })
            .min_by_key(|candidate| (candidate.0, candidate.1))
            .map(|candidate| candidate.2);
        let selected_index = preferred_index.map_or(candidates.len() - 1, |index| {
            ((index + 1)..candidates.len())
                .rev()
                .find(|candidate| {
                    candidates[*candidate].plan.id != candidates[index].plan.id
                        && hls_media_cursors_compatible(&candidates[index], &candidates[*candidate])
                })
                .unwrap_or(index)
        });
        let cursor = candidates[selected_index].clone();
        let mut seen = HashSet::new();
        let superseded_plan_ids = candidates[..selected_index]
            .iter()
            .filter(|candidate| {
                candidate.plan.id != cursor.plan.id
                    && preferred.contains_key(&candidate.plan.id)
                    && hls_media_cursors_compatible(candidate, &cursor)
            })
            .filter_map(|candidate| seen.insert(candidate.plan.id).then_some(candidate.plan.id))
            .collect();
        Some(HlsMediaSelection {
            cursor,
            superseded_plan_ids,
        })
    }
}

fn hls_media_cursors_compatible(left: &HlsMediaCursor, right: &HlsMediaCursor) -> bool {
    let matches = |left_position, right_position| {
        left.plan
            .references
            .get(left_position)
            .is_some_and(|reference| right.plan.references.get(right_position) == Some(reference))
    };
    matches(left.position, right.position)
        && (left.position.checked_sub(1).is_some_and(|left_position| {
            right
                .position
                .checked_sub(1)
                .is_some_and(|right_position| matches(left_position, right_position))
        }) || matches(left.position + 1, right.position + 1)
            || (left.position + 1 == left.plan.references.len() && right.position == 0))
}

pub(crate) const MAX_STREAM_FEED_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn is_hls_manifest(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| {
            text.strip_prefix('\u{feff}')
                .unwrap_or(text)
                .trim_start()
                .lines()
                .next()
        })
        .map(hls_header_line)
        .unwrap_or(false)
}

pub(crate) fn hls_sequence_zero_canonical_is_supported(bytes: &[u8]) -> bool {
    let declares_master = std::str::from_utf8(bytes).ok().is_some_and(|text| {
        text.lines().any(|line| {
            let line = line.trim();
            line.starts_with("#EXT-X-STREAM-INF:")
                || line.starts_with("#EXT-X-I-FRAME-STREAM-INF:")
                || line.starts_with("#EXT-X-MEDIA:")
        })
    });
    if declares_master {
        hls_master_playlist_is_supported(bytes)
    } else {
        HlsTimeline::parse(bytes).is_some()
    }
}

fn hls_master_playlist_is_supported(bytes: &[u8]) -> bool {
    if !stream_feed_payload_len_is_supported(bytes.len()) || !is_hls_manifest(bytes) {
        return false;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut header_seen = false;
    let mut expects_variant_uri = false;
    let mut variants = 0_usize;
    for original_line in text.lines() {
        let line = original_line.trim_end_matches('\r').trim();
        if expects_variant_uri {
            if line.is_empty() || line.starts_with('#') || line.chars().any(char::is_control) {
                return false;
            }
            expects_variant_uri = false;
            variants = variants.saturating_add(1);
            continue;
        }
        if hls_header_line(line) {
            if header_seen {
                return false;
            }
            header_seen = true;
        } else if let Some(attributes) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            if attributes.trim().is_empty() {
                return false;
            }
            expects_variant_uri = true;
        } else if let Some(attributes) = line.strip_prefix("#EXT-X-I-FRAME-STREAM-INF:") {
            if attributes.trim().is_empty() || uri_attribute_value_ranges(line).len() != 1 {
                return false;
            }
            variants = variants.saturating_add(1);
        } else if let Some(attributes) = line.strip_prefix("#EXT-X-MEDIA:") {
            if attributes.trim().is_empty() {
                return false;
            }
            match uri_attribute_value_ranges(line).len() {
                0 => {}
                1 => variants = variants.saturating_add(1),
                _ => return false,
            }
        } else if line.starts_with("#EXTINF:")
            || line.starts_with("#EXT-X-MEDIA-SEQUENCE:")
            || line.starts_with("#EXT-X-TARGETDURATION:")
            || line.starts_with("#EXT-X-BYTERANGE:")
            || line.starts_with("#EXT-X-MAP:")
            || line.starts_with("#EXT-X-KEY:")
            || matches!(line, HLS_ENDLIST | "#EXT-X-DISCONTINUITY" | HLS_GAP)
        {
            return false;
        } else if !line.is_empty() && !line.starts_with('#') {
            return false;
        }
    }
    header_seen && variants > 0 && !expects_variant_uri
}

fn hls_header_line(line: &str) -> bool {
    line.strip_prefix('\u{feff}')
        .unwrap_or(line)
        .trim_end_matches('\r')
        .trim()
        == HLS_HEADER
}

pub(crate) fn probe_hls_manifest(prefix: &[u8], total_len: u64) -> HlsManifestProbe {
    if total_len > MAX_STREAM_FEED_PAYLOAD_BYTES as u64 {
        return HlsManifestProbe::NotManifest;
    }

    if u64::try_from(prefix.len()).ok() == Some(total_len) {
        return if is_hls_manifest(prefix) {
            HlsManifestProbe::Manifest
        } else {
            HlsManifestProbe::NotManifest
        };
    }

    let text = match std::str::from_utf8(prefix) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() => return HlsManifestProbe::NeedMore,
        Err(_) => return HlsManifestProbe::NotManifest,
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).trim_start();
    if text.is_empty() {
        return HlsManifestProbe::NeedMore;
    }

    if let Some(line_end) = text.find('\n') {
        return if text[..line_end].trim_end_matches('\r').trim() == HLS_HEADER {
            HlsManifestProbe::Manifest
        } else {
            HlsManifestProbe::NotManifest
        };
    }

    let partial_line = text.trim_end_matches('\r').trim();
    if HLS_HEADER.starts_with(partial_line) || partial_line == HLS_HEADER {
        HlsManifestProbe::NeedMore
    } else {
        HlsManifestProbe::NotManifest
    }
}

pub(crate) fn hls_is_finalized(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .map(|text| text.lines().any(|line| line.trim() == HLS_ENDLIST))
        .unwrap_or(false)
}

pub(crate) fn hls_startup_prefix_is_preferred(
    canonical: &[u8],
    prefix: &[u8],
    minimum_prefix_segments: usize,
) -> bool {
    if !is_hls_manifest(canonical)
        || !is_hls_manifest(prefix)
        || !hls_media_sequence(canonical).is_some_and(|sequence| sequence > 0)
        || hls_media_sequence(prefix) != Some(0)
    {
        return false;
    }

    hls_media_references(prefix).len() >= minimum_prefix_segments
}

pub(crate) fn hls_manifest_reload_is_continuous(current: &[u8], candidate: &[u8]) -> bool {
    HlsTimeline::parse(current)
        .zip(HlsTimeline::parse(candidate))
        .is_some_and(|(current, candidate)| current.is_continuous_with(&candidate))
}

pub(crate) fn hls_manifest_reload_is_forward(current: &[u8], candidate: &[u8]) -> bool {
    HlsTimeline::parse(current)
        .zip(HlsTimeline::parse(candidate))
        .is_some_and(|(current, candidate)| current.is_forward_of(&candidate))
}

pub(crate) fn hls_live_tail(bytes: &[u8]) -> Option<(usize, f64)> {
    HlsTimeline::parse(bytes)?.live_tail()
}

pub(crate) fn hls_last_playable_segment(bytes: &[u8]) -> Option<(u64, String, f64, f64)> {
    HlsTimeline::parse(bytes)?.last_playable_segment()
}

pub(crate) fn hls_live_tail_fallback_segment(
    failures: &[(u64, u64, String)],
) -> Option<(u64, u64, String)> {
    let (snapshot_index, _, _) = failures.last()?;
    let mut count = 0_u8;
    let mut earliest = None::<(u64, String)>;
    for (_, sequence, reference) in failures
        .iter()
        .rev()
        .take_while(|(candidate_index, _, _)| candidate_index == snapshot_index)
        .take(usize::from(HLS_LIVE_TAIL_FAILURE_THRESHOLD))
    {
        count = count.saturating_add(1);
        if earliest
            .as_ref()
            .is_none_or(|(current, _)| *sequence < *current)
        {
            earliest = Some((*sequence, reference.to_ascii_lowercase()));
        }
    }
    (count >= HLS_LIVE_TAIL_FAILURE_THRESHOLD)
        .then_some(earliest)
        .flatten()
        .map(|(sequence, reference)| (*snapshot_index, sequence, reference))
}

pub(crate) fn truncate_hls_live_before_segment(
    bytes: &[u8],
    sequence: u64,
    reference: &str,
) -> Option<Vec<u8>> {
    if hls_is_finalized(bytes) {
        return None;
    }
    let reference = reference.to_ascii_lowercase();
    let timeline = HlsTimeline::parse(bytes)?;
    let position = usize::try_from(sequence.checked_sub(timeline.sequence)?).ok()?;
    if timeline.segments.get(position)?.reference != reference {
        return None;
    }
    let retained_end = *timeline.uri_ends.get(position.checked_sub(1)?)?;
    let truncated = bytes.get(..retained_end)?.to_vec();
    let parsed = HlsTimeline::parse(&truncated)?;
    (parsed.sequence == timeline.sequence
        && parsed.segments == timeline.segments[..position]
        && !hls_is_finalized(&truncated))
    .then_some(truncated)
}

pub(crate) fn gap_hls_segment_range(
    bytes: &[u8],
    start_sequence: u64,
    start_reference: &str,
    end_sequence: u64,
) -> Option<Vec<u8>> {
    let timeline = HlsTimeline::parse(bytes)?;
    let start = usize::try_from(start_sequence.checked_sub(timeline.sequence)?).ok()?;
    let end = usize::try_from(end_sequence.checked_sub(timeline.sequence)?).ok()?;
    if start >= end
        || end > timeline.segments.len()
        || !timeline
            .segments
            .get(start)?
            .reference
            .eq_ignore_ascii_case(start_reference)
    {
        return None;
    }

    let line_ending: &[u8] = if bytes.windows(2).any(|window| window == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    let insertion = [HLS_GAP.as_bytes(), line_ending].concat();
    let mut output = Vec::with_capacity(
        bytes
            .len()
            .saturating_add(insertion.len().saturating_mul(end.saturating_sub(start))),
    );
    let mut copied = 0;
    for position in start..end {
        if timeline.segments[position].gap {
            continue;
        }
        let uri_start = *timeline.uri_starts.get(position)?;
        output.extend_from_slice(bytes.get(copied..uri_start)?);
        output.extend_from_slice(&insertion);
        copied = uri_start;
    }
    output.extend_from_slice(bytes.get(copied..)?);
    raise_or_insert_hls_version(&mut output, 8)?;

    let parsed = HlsTimeline::parse(&output)?;
    let mut expected = timeline.segments.clone();
    for segment in &mut expected[start..end] {
        segment.gap = true;
    }
    (parsed.sequence == timeline.sequence
        && parsed.segments == expected
        && hls_is_finalized(&output) == hls_is_finalized(bytes))
    .then_some(output)
}

pub(crate) fn present_hls_live_fallback(
    bytes: &[u8],
    cutoff_sequence: u64,
    cutoff_reference: &str,
    hidden_end_sequence: u64,
) -> Option<Vec<u8>> {
    let timeline = HlsTimeline::parse(bytes)?;
    let cutoff = usize::try_from(cutoff_sequence.checked_sub(timeline.sequence)?).ok()?;
    if !timeline
        .segments
        .get(cutoff)?
        .reference
        .eq_ignore_ascii_case(cutoff_reference)
    {
        return None;
    }
    let end_sequence = timeline.end()?;
    if hls_is_finalized(bytes) || end_sequence > hidden_end_sequence {
        gap_hls_segment_range(
            bytes,
            cutoff_sequence,
            cutoff_reference,
            hidden_end_sequence.min(end_sequence),
        )
    } else {
        truncate_hls_live_before_segment(bytes, cutoff_sequence, cutoff_reference)
    }
}

pub(crate) fn hls_live_retreat_boundary(
    bytes: &[u8],
    failed_sequence: u64,
    failed_reference: &str,
) -> Option<(u64, String)> {
    let timeline = HlsTimeline::parse(bytes)?;
    let failed_position = usize::try_from(failed_sequence.checked_sub(timeline.sequence)?).ok()?;
    if timeline.segments.get(failed_position)?.reference != failed_reference.to_ascii_lowercase() {
        return None;
    }
    let (tail_start, _) = timeline.live_tail()?;
    if failed_position < tail_start.saturating_sub(HLS_LIVE_STARTUP_PREROLL_SEGMENTS) {
        return None;
    }
    let boundary_position = tail_start.min(failed_position);
    let boundary = timeline.segments.get(boundary_position)?;
    let boundary_sequence = timeline
        .sequence
        .checked_add(u64::try_from(boundary_position).ok()?)?;
    if hls_is_finalized(bytes) {
        gap_hls_segment_range(
            bytes,
            boundary_sequence,
            &boundary.reference,
            timeline.end()?,
        )?;
    } else {
        truncate_hls_live_before_segment(bytes, boundary_sequence, &boundary.reference)?;
    }
    Some((boundary_sequence, boundary.reference.clone()))
}

pub(crate) fn extend_hls_sequence_zero_archive(
    current: &[u8],
    candidate: &[u8],
) -> Option<Vec<u8>> {
    let current_timeline = HlsTimeline::parse(current)?;
    let candidate_timeline = HlsTimeline::parse(candidate)?;
    let adjacent = current_timeline.is_adjacent_to(&candidate_timeline);
    if current_timeline.sequence != 0
        || (!current_timeline.is_continuous_with(&candidate_timeline) && !adjacent)
        || !hls_has_at_most_one_endlist(current)
        || !hls_has_at_most_one_endlist(candidate)
    {
        return None;
    }

    let candidate_sequence = candidate_timeline.sequence;
    let candidate_media_start = candidate_timeline.media_start;
    if candidate_sequence == 0 {
        let mut replacement = candidate.to_vec();
        if candidate_timeline
            .segments
            .iter()
            .any(|segment| segment.gap)
        {
            raise_or_insert_hls_version(&mut replacement, 8)?;
        }
        return stream_feed_payload_len_is_supported(replacement.len()).then_some(replacement);
    }
    if !hls_append_only_tags_are_supported(current)
        || !hls_append_only_tags_are_supported(candidate)
    {
        return None;
    }

    let current_segments = current_timeline.segments;
    let candidate_segments = candidate_timeline.segments;
    let current_len = u64::try_from(current_segments.len()).ok()?;
    let candidate_len = u64::try_from(candidate_segments.len()).ok()?;
    let current_end = current_len;
    let candidate_end = candidate_sequence.checked_add(candidate_len)?;
    if candidate_sequence > current_end || candidate_end <= current_end {
        return None;
    }

    let current_uri_ends = current_timeline.uri_ends;
    let candidate_uri_ends = candidate_timeline.uri_ends;

    let current_prefix_end = *current_uri_ends.last()?;
    let (candidate_suffix_start, appended_position) = if adjacent {
        (candidate_media_start?, 0)
    } else {
        let overlap_position = usize::try_from(
            current_end
                .checked_sub(1)?
                .checked_sub(candidate_sequence)?,
        )
        .ok()?;
        (
            *candidate_uri_ends.get(overlap_position)?,
            overlap_position.checked_add(1)?,
        )
    };
    let candidate_suffix = candidate.get(candidate_suffix_start..)?;

    let mut merged = Vec::with_capacity(
        current_prefix_end
            .saturating_add(candidate_suffix.len())
            .saturating_add(1),
    );
    merged.extend_from_slice(current.get(..current_prefix_end)?);
    if !merged.ends_with(b"\n") && !candidate_suffix.is_empty() {
        merged.push(b'\n');
    }
    merged.extend_from_slice(candidate_suffix);
    if hls_is_finalized(candidate) && !hls_is_finalized(&merged) {
        if !merged.ends_with(b"\n") {
            merged.push(b'\n');
        }
        merged.extend_from_slice(HLS_ENDLIST.as_bytes());
        merged.push(b'\n');
    }
    match (hls_target_duration(current), hls_target_duration(candidate)) {
        (Some(current), Some(candidate)) => {
            raise_hls_target_duration(&mut merged, current.max(candidate))?;
        }
        (None, None) => {}
        _ => return None,
    }
    let mut expected = current_segments;
    expected.extend_from_slice(candidate_segments.get(appended_position..)?);
    if expected.iter().any(|segment| segment.gap) {
        raise_or_insert_hls_version(&mut merged, 8)?;
    }
    if !stream_feed_payload_len_is_supported(merged.len()) {
        return None;
    }
    let merged_timeline = HlsTimeline::parse(&merged)?;
    (merged_timeline.sequence == 0 && merged_timeline.segments == expected).then_some(merged)
}

pub(crate) fn hls_target_duration(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut values = text.lines().filter_map(|line| {
        line.trim()
            .strip_prefix("#EXT-X-TARGETDURATION:")
            .map(str::trim)
    });
    match (values.next(), values.next()) {
        (Some(value), None) => value.parse().ok(),
        _ => None,
    }
}

pub(crate) fn raise_hls_target_duration(bytes: &mut Vec<u8>, target_duration: u64) -> Option<()> {
    if hls_target_duration(bytes)? >= target_duration {
        return Some(());
    }

    let text = std::str::from_utf8(bytes).ok()?;
    let marker = "#EXT-X-TARGETDURATION:";
    let mut value_range = None;
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if let Some(marker_position) = line.find(marker) {
            if value_range.is_some() {
                return None;
            }
            let value_start = marker_position.checked_add(marker.len())?;
            let raw_value = line
                .get(value_start..)?
                .trim_end_matches(['\r', '\n'])
                .trim();
            raw_value.parse::<u64>().ok()?;
            let leading = line.get(value_start..)?.find(raw_value)?;
            let start = offset.checked_add(value_start)?.checked_add(leading)?;
            value_range = Some(start..start.checked_add(raw_value.len())?);
        }
        offset = offset.checked_add(line.len())?;
    }
    bytes.splice(value_range?, target_duration.to_string().bytes());
    Some(())
}

fn raise_or_insert_hls_version(bytes: &mut Vec<u8>, minimum: u64) -> Option<()> {
    let text = std::str::from_utf8(bytes).ok()?;
    let marker = "#EXT-X-VERSION:";
    let mut value_range = None;
    let mut version = None;
    let mut header_end = None;
    let mut header_line_ending = None::<&str>;
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']).trim();
        if hls_header_line(trimmed) {
            if header_end.is_some() {
                return None;
            }
            header_end = Some(offset.checked_add(line.len())?);
            header_line_ending = Some(if line.ends_with("\r\n") { "\r\n" } else { "\n" });
        }
        if let Some(raw_value) = trimmed.strip_prefix(marker).map(str::trim) {
            if value_range.is_some() || raw_value.is_empty() {
                return None;
            }
            let parsed = raw_value.parse::<u64>().ok()?;
            let marker_position = line.find(marker)?;
            let value_start = marker_position.checked_add(marker.len())?;
            let leading = line.get(value_start..)?.find(raw_value)?;
            let start = offset.checked_add(value_start)?.checked_add(leading)?;
            value_range = Some(start..start.checked_add(raw_value.len())?);
            version = Some(parsed);
        }
        offset = offset.checked_add(line.len())?;
    }
    match (version, value_range) {
        (Some(version), Some(range)) if version < minimum => {
            bytes.splice(range, minimum.to_string().bytes());
        }
        (Some(_), Some(_)) => {}
        (None, None) => {
            let header_end = header_end?;
            let line_ending = header_line_ending?;
            bytes.splice(
                header_end..header_end,
                format!("{marker}{minimum}{line_ending}").bytes(),
            );
        }
        _ => return None,
    }
    Some(())
}

pub(crate) fn append_hls_sequence_zero_archive_suffix(
    archive: &mut Vec<u8>,
    archive_segment_count: &mut u64,
    archive_media_end: &mut usize,
    current_source: &[u8],
    candidate: &[u8],
) -> Option<()> {
    let current_timeline = HlsTimeline::parse(current_source)?;
    let candidate_timeline = HlsTimeline::parse(candidate)?;
    let adjacent = candidate_timeline.sequence == *archive_segment_count;
    if (!current_timeline.is_continuous_with(&candidate_timeline) && !adjacent)
        || !hls_append_only_tags_are_supported(candidate)
        || !hls_has_at_most_one_endlist(candidate)
    {
        return None;
    }

    let candidate_sequence = candidate_timeline.sequence;
    let candidate_media_start = candidate_timeline.media_start;
    let candidate_segments = candidate_timeline.segments;
    let candidate_len = u64::try_from(candidate_segments.len()).ok()?;
    let candidate_end = candidate_sequence.checked_add(candidate_len)?;
    if candidate_sequence > *archive_segment_count {
        return None;
    }
    if candidate_end <= *archive_segment_count {
        return Some(());
    }

    let uri_ends = candidate_timeline.uri_ends;
    let (suffix_start, appended_position) = if adjacent {
        (candidate_media_start?, 0)
    } else {
        let overlap_position = usize::try_from(
            archive_segment_count
                .checked_sub(1)?
                .checked_sub(candidate_sequence)?,
        )
        .ok()?;
        (
            *uri_ends.get(overlap_position)?,
            overlap_position.checked_add(1)?,
        )
    };
    let candidate_media_end = *uri_ends.last()?;
    let suffix = candidate.get(suffix_start..)?;
    let archive_media_end_value = *archive_media_end;
    let insert_newline =
        !archive.get(..archive_media_end_value)?.ends_with(b"\n") && !suffix.is_empty();
    let append_endlist = hls_is_finalized(candidate) && !hls_is_finalized(suffix);
    let endlist_len = if append_endlist {
        usize::from(!suffix.ends_with(b"\n"))
            .checked_add(HLS_ENDLIST.len())?
            .checked_add(1)?
    } else {
        0
    };
    let merged_len = archive_media_end_value
        .checked_add(usize::from(insert_newline))?
        .checked_add(suffix.len())?
        .checked_add(endlist_len)?;
    if !stream_feed_payload_len_is_supported(merged_len) {
        return None;
    }
    let appended_at = archive_media_end_value.checked_add(usize::from(insert_newline))?;
    let merged_media_end =
        appended_at.checked_add(candidate_media_end.checked_sub(suffix_start)?)?;
    let append = |output: &mut Vec<u8>| {
        output.truncate(archive_media_end_value);
        if insert_newline {
            output.push(b'\n');
        }
        output.extend_from_slice(suffix);
        if append_endlist {
            if !output.ends_with(b"\n") {
                output.push(b'\n');
            }
            output.extend_from_slice(HLS_ENDLIST.as_bytes());
            output.push(b'\n');
        }
    };
    let appended_segments = candidate_segments.get(appended_position..)?;
    if appended_segments.iter().any(|segment| segment.gap) {
        // VERSION precedes the archive media, so keep the uncommon GAP rewrite
        // transactional without cloning the ordinary sparse-history hot path.
        let mut merged = archive.clone();
        append(&mut merged);
        raise_or_insert_hls_version(&mut merged, 8)?;
        if !stream_feed_payload_len_is_supported(merged.len()) {
            return None;
        }
        let merged_timeline = HlsTimeline::parse(&merged)?;
        let appended_at = usize::try_from(*archive_segment_count).ok()?;
        if merged_timeline.sequence != 0
            || merged_timeline.end()? != candidate_end
            || merged_timeline.segments.get(appended_at..)? != appended_segments
        {
            return None;
        }
        *archive_media_end = *merged_timeline.uri_ends.last()?;
        *archive = merged;
    } else {
        append(archive);
        *archive_media_end = merged_media_end;
    }
    *archive_segment_count = candidate_end;
    Some(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HlsSparseHistoryPlan {
    pub(crate) head_index: u64,
    pub(crate) segment_count: u64,
    pub(crate) stride: u64,
    pub(crate) lattice_residue: u64,
    pub(crate) requested_indices: Vec<u64>,
}

pub(crate) fn plan_hls_sparse_history(
    head_index: u64,
    head: &[u8],
) -> Option<HlsSparseHistoryPlan> {
    plan_hls_sparse_history_from_lattice(head_index, head, head_index % HLS_SPARSE_HISTORY_STRIDE)
}

pub(crate) fn plan_hls_sparse_history_from_lattice(
    head_index: u64,
    head: &[u8],
    lattice_residue: u64,
) -> Option<HlsSparseHistoryPlan> {
    if lattice_residue >= HLS_SPARSE_HISTORY_STRIDE {
        return None;
    }
    let timeline = hls_complete_history_timeline(head)?;
    let segment_count = timeline.end()?;
    if segment_count == 0 || segment_count > u64::try_from(HLS_SPARSE_HISTORY_MAX_SEGMENTS).ok()? {
        return None;
    }
    if timeline.sequence == 0 {
        return Some(HlsSparseHistoryPlan {
            head_index,
            segment_count,
            stride: HLS_SPARSE_HISTORY_STRIDE,
            lattice_residue,
            requested_indices: Vec::new(),
        });
    }
    let timeline = hls_sparse_history_timeline(head)?;
    if u64::try_from(timeline.segments.len()).ok()? != HLS_SPARSE_HISTORY_STRIDE {
        return None;
    }

    let request_count = head_index
        .saturating_sub(lattice_residue)
        .checked_add(HLS_SPARSE_HISTORY_STRIDE - 1)?
        .checked_div(HLS_SPARSE_HISTORY_STRIDE)?;
    if request_count == 0 || request_count > u64::try_from(HLS_SPARSE_HISTORY_MAX_PROBES).ok()? {
        return None;
    }
    let mut requested_indices = Vec::with_capacity(usize::try_from(request_count).ok()?);
    let mut index = lattice_residue;
    while index < head_index {
        requested_indices.push(index);
        index = index.checked_add(HLS_SPARSE_HISTORY_STRIDE)?;
    }
    (index >= head_index).then_some(HlsSparseHistoryPlan {
        head_index,
        segment_count,
        stride: HLS_SPARSE_HISTORY_STRIDE,
        lattice_residue,
        requested_indices,
    })
}

pub(crate) fn hls_sparse_history_candidate_is_supported(candidate: &[u8]) -> bool {
    hls_sparse_history_timeline(candidate).is_some_and(|timeline| {
        !hls_is_finalized(candidate)
            && (1..=HLS_SPARSE_HISTORY_STRIDE)
                .contains(&u64::try_from(timeline.segments.len()).unwrap_or(u64::MAX))
    })
}

pub(crate) fn hls_sparse_history_head_is_supported(candidate: &[u8]) -> bool {
    hls_sparse_history_timeline(candidate).is_some_and(|timeline| {
        (1..=HLS_SPARSE_HISTORY_STRIDE)
            .contains(&u64::try_from(timeline.segments.len()).unwrap_or(u64::MAX))
    })
}

pub(crate) fn hls_sequence_zero_covers_head(head: &[u8], candidate: &[u8]) -> bool {
    let Some(candidate) = hls_complete_history_timeline(candidate) else {
        return false;
    };
    hls_sequence_zero_timeline_covers_head(head, &candidate)
}

pub(crate) fn hls_sequence_zero_same_index_archive_is_reusable(
    source: &[u8],
    archive: &[u8],
) -> bool {
    hls_media_sequence(archive) == Some(0)
        && hls_is_finalized(source) == hls_is_finalized(archive)
        && hls_sequence_zero_covers_head(source, archive)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsDirectArchiveDisposition {
    Stale,
    SequenceZeroCheckpoint,
    Nonterminal,
    Terminal,
    Unsupported,
}

pub(crate) fn hls_direct_archive_disposition(
    candidate_index: u64,
    current_head_index: u64,
    authenticated_ceiling: u64,
    candidate: &[u8],
) -> HlsDirectArchiveDisposition {
    if candidate_index <= current_head_index || candidate_index < authenticated_ceiling {
        return HlsDirectArchiveDisposition::Stale;
    }
    if !is_hls_manifest(candidate) {
        return HlsDirectArchiveDisposition::Unsupported;
    }
    if hls_media_sequence(candidate) == Some(0) && !hls_is_finalized(candidate) {
        return HlsDirectArchiveDisposition::SequenceZeroCheckpoint;
    }
    if hls_complete_history_timeline(candidate).is_none() {
        return HlsDirectArchiveDisposition::Unsupported;
    }
    if hls_media_sequence(candidate) != Some(0) || !hls_is_finalized(candidate) {
        return HlsDirectArchiveDisposition::Nonterminal;
    }
    HlsDirectArchiveDisposition::Terminal
}

fn hls_sequence_zero_timeline_covers_head(head: &[u8], candidate: &HlsTimeline) -> bool {
    let Some(head) = HlsTimeline::parse(head) else {
        return false;
    };
    candidate.sequence == 0 && head.is_continuous_with(candidate)
}

pub(crate) fn hls_sequence_zero_sparse_tail(candidate: &[u8]) -> Option<Vec<u8>> {
    let timeline = hls_complete_history_timeline(candidate)?;
    hls_sequence_zero_sparse_tail_from_timeline(candidate, &timeline)
}

pub(crate) fn hls_verified_sequence_zero_checkpoint_tail<'a>(
    candidate: &[u8],
    pinned_windows: impl IntoIterator<Item = &'a [u8]>,
) -> Result<Option<Vec<u8>>, ()> {
    let Some(timeline) = hls_complete_history_timeline(candidate) else {
        return Ok(None);
    };
    if timeline.sequence != 0 || hls_is_finalized(candidate) {
        return Ok(None);
    }
    if timeline.segments.len() <= HLS_SPARSE_HISTORY_STRIDE as usize {
        return Ok(None);
    }
    if !pinned_windows
        .into_iter()
        .all(|window| hls_sequence_zero_timeline_covers_head(window, &timeline))
    {
        return Err(());
    }
    Ok(hls_sequence_zero_sparse_tail_from_timeline(
        candidate, &timeline,
    ))
}

pub(crate) fn hls_verified_sequence_zero_checkpoint_tail_at_index<'a>(
    candidate_index: u64,
    candidate: &[u8],
    pinned_windows: impl IntoIterator<Item = (u64, &'a [u8])>,
) -> Result<Option<Vec<u8>>, ()> {
    hls_verified_sequence_zero_checkpoint_tail(
        candidate,
        pinned_windows
            .into_iter()
            .filter(|(index, _)| *index <= candidate_index)
            .map(|(_, window)| window),
    )
}

pub(crate) fn hls_is_long_sequence_zero_checkpoint(candidate: &[u8]) -> bool {
    !hls_is_finalized(candidate)
        && hls_complete_history_timeline(candidate).is_some_and(|timeline| {
            timeline.sequence == 0 && timeline.segments.len() > HLS_SPARSE_HISTORY_STRIDE as usize
        })
}

fn hls_sequence_zero_sparse_tail_from_timeline(
    candidate: &[u8],
    timeline: &HlsTimeline,
) -> Option<Vec<u8>> {
    if timeline.sequence != 0 || timeline.segments.len() <= HLS_SPARSE_HISTORY_STRIDE as usize {
        return None;
    }
    let tail_start = timeline
        .segments
        .len()
        .checked_sub(HLS_SPARSE_HISTORY_STRIDE as usize)?;
    let segments = timeline.segments.get(tail_start..)?;
    if segments
        .iter()
        .any(|segment| segment.byte_range.is_some() || segment.discontinuity_counter != 0)
    {
        return None;
    }
    let sequence = u64::try_from(tail_start).ok()?;
    let target_duration = hls_target_duration(candidate)?;
    let mut tail = format!(
        "{HLS_HEADER}\n#EXT-X-TARGETDURATION:{target_duration}\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n"
    );
    for segment in segments {
        tail.push_str(&format!(
            "#EXTINF:{},\n{}\n",
            f64::from_bits(segment.duration_bits),
            segment.reference
        ));
    }
    let tail = tail.into_bytes();
    hls_sparse_history_head_is_supported(&tail).then_some(tail)
}

pub(crate) fn plan_hls_sparse_history_repairs_for_attempts<'a>(
    plan: &HlsSparseHistoryPlan,
    head: &[u8],
    attempted_indices: impl IntoIterator<Item = u64>,
    successful: impl IntoIterator<Item = (u64, &'a [u8])>,
) -> Option<Vec<u64>> {
    if plan_hls_sparse_history_from_lattice(plan.head_index, head, plan.lattice_residue).as_ref()
        != Some(plan)
    {
        return None;
    }
    if plan.requested_indices.is_empty() {
        return (hls_complete_history_timeline(head)?.sequence == 0
            && attempted_indices.into_iter().next().is_none())
        .then(Vec::new);
    }
    let head_timeline = hls_sparse_history_timeline(head)?;
    if plan.head_index == 0
        || head_timeline.end()? != plan.segment_count
        || u64::try_from(head_timeline.segments.len()).ok()? != HLS_SPARSE_HISTORY_STRIDE
    {
        return None;
    }

    let mut actual_attempted = HashSet::new();
    for index in attempted_indices {
        if index >= plan.head_index
            || !actual_attempted.insert(index)
            || actual_attempted.len() > HLS_SPARSE_HISTORY_MAX_CANDIDATES
        {
            return None;
        }
    }

    let mut aggregate_bytes = head.len();
    let mut observed = Vec::new();
    for (index, bytes) in successful {
        if observed.len() >= HLS_SPARSE_HISTORY_MAX_CANDIDATES
            || index >= plan.head_index
            || bytes.len() > HLS_SPARSE_HISTORY_MAX_WINDOW_BYTES
        {
            return None;
        }
        aggregate_bytes = aggregate_bytes.checked_add(bytes.len())?;
        if aggregate_bytes > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES
            || !hls_sparse_history_candidate_is_supported(bytes)
        {
            return None;
        }
        let timeline = HlsTimeline::parse(bytes)?;
        observed.push((index, timeline.sequence, timeline.end()?));
    }
    observed.sort_by_key(|entry| entry.0);
    if observed.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return None;
    }
    let successful_indices = observed.iter().map(|entry| entry.0).collect::<HashSet<_>>();
    let missing_attempts = actual_attempted
        .iter()
        .copied()
        .filter(|index| !successful_indices.contains(index))
        .collect::<Vec<_>>();
    observed.push((
        plan.head_index,
        head_timeline.sequence,
        head_timeline.end()?,
    ));
    let mut attempted = actual_attempted;
    attempted.insert(plan.head_index);
    attempted.extend(observed.iter().map(|entry| entry.0));
    let existing_candidate_count = attempted.len().checked_sub(1)?;
    let mut repairs = HashSet::new();
    let add_interval = |start: u64, end: u64, repairs: &mut HashSet<u64>| -> Option<()> {
        for index in start..end {
            if attempted.contains(&index) || !repairs.insert(index) {
                continue;
            }
            if repairs.len() > HLS_SPARSE_HISTORY_MAX_REPAIRS
                || existing_candidate_count.checked_add(repairs.len())?
                    > HLS_SPARSE_HISTORY_MAX_CANDIDATES
            {
                return None;
            }
        }
        Some(())
    };
    for missing in missing_attempts {
        let lower = observed
            .iter()
            .rev()
            .find(|entry| entry.0 < missing)
            .copied();
        let upper = *observed.iter().find(|entry| entry.0 > missing)?;
        if lower.map_or(0, |entry| entry.2) >= upper.1 {
            continue;
        }
        add_interval(
            lower.map_or(0, |entry| entry.0.saturating_add(1)),
            upper.0,
            &mut repairs,
        )?;
    }
    if let Some(first) = observed.first()
        && first.1 > 0
    {
        add_interval(0, first.0, &mut repairs)?;
    }
    for pair in observed.windows(2) {
        if pair[0].2 < pair[1].1 {
            add_interval(pair[0].0.saturating_add(1), pair[1].0, &mut repairs)?;
        }
    }
    let mut repairs = repairs.into_iter().collect::<Vec<_>>();
    repairs.sort_unstable();
    Some(repairs)
}

pub(crate) fn assemble_hls_sparse_history<'a>(
    plan: &HlsSparseHistoryPlan,
    head: &[u8],
    candidates: impl IntoIterator<Item = (u64, &'a [u8])>,
) -> Option<Vec<u8>> {
    if plan_hls_sparse_history_from_lattice(plan.head_index, head, plan.lattice_residue).as_ref()
        != Some(plan)
    {
        return None;
    }
    let head_timeline = hls_complete_history_timeline(head)?;
    if head_timeline.end()? != plan.segment_count {
        return None;
    }
    if plan.requested_indices.is_empty() {
        return (head_timeline.sequence == 0).then(|| head.to_vec());
    }
    let head_timeline = hls_sparse_history_timeline(head)?;
    let mut aggregate_bytes = head.len();
    let mut parsed = Vec::new();
    for (index, bytes) in candidates {
        if parsed.len() >= HLS_SPARSE_HISTORY_MAX_CANDIDATES
            || index >= plan.head_index
            || bytes.len() > HLS_SPARSE_HISTORY_MAX_WINDOW_BYTES
        {
            return None;
        }
        aggregate_bytes = aggregate_bytes.checked_add(bytes.len())?;
        if aggregate_bytes > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES
            || !hls_sparse_history_candidate_is_supported(bytes)
        {
            return None;
        }
        parsed.push((index, bytes, HlsTimeline::parse(bytes)?));
    }
    if parsed.is_empty() {
        return None;
    }
    let mut indices = HashSet::new();
    if parsed.iter().any(|entry| !indices.insert(entry.0)) {
        return None;
    }
    parsed.sort_by_key(|(index, _, timeline)| {
        (
            timeline.sequence,
            timeline.end().unwrap_or(u64::MAX),
            *index,
        )
    });
    let (_, first, first_timeline) = parsed.first()?;
    let mut archive = HlsSparseHistoryArchive::new(first, first_timeline)?;
    for (_, bytes, timeline) in parsed.iter().skip(1) {
        archive.admit(bytes, timeline)?;
    }
    archive.admit(head, &head_timeline)?;
    let archive = archive.finish(plan.segment_count)?;
    (hls_is_finalized(&archive) == hls_is_finalized(head)).then_some(archive)
}

struct HlsSparseHistoryArchive {
    body: Vec<u8>,
    segments: Vec<HlsSegmentIdentity>,
    media_end: usize,
    source: Vec<u8>,
    target_duration: u64,
}

impl HlsSparseHistoryArchive {
    fn new(source: &[u8], timeline: &HlsTimeline) -> Option<Self> {
        (timeline.sequence == 0 && !timeline.segments.is_empty()).then_some(())?;
        let mut body = source.to_vec();
        let media_end = if timeline.segments.iter().any(|segment| segment.gap) {
            raise_or_insert_hls_version(&mut body, 8)?;
            if !stream_feed_payload_len_is_supported(body.len()) {
                return None;
            }
            let normalized = HlsTimeline::parse(&body)?;
            (normalized.sequence == 0 && normalized.segments == timeline.segments).then_some(())?;
            *normalized.uri_ends.last()?
        } else {
            *timeline.uri_ends.last()?
        };
        Some(Self {
            body,
            segments: timeline.segments.clone(),
            media_end,
            source: source.to_vec(),
            target_duration: hls_target_duration(source)?,
        })
    }

    fn admit(&mut self, candidate: &[u8], timeline: &HlsTimeline) -> Option<()> {
        let current_end = u64::try_from(self.segments.len()).ok()?;
        let candidate_end = timeline.end()?;
        if timeline.sequence > current_end {
            return None;
        }
        for sequence in timeline.sequence..current_end.min(candidate_end) {
            let current = usize::try_from(sequence).ok()?;
            let incoming = usize::try_from(sequence - timeline.sequence).ok()?;
            if self.segments.get(current)? != timeline.segments.get(incoming)? {
                return None;
            }
        }
        self.target_duration = self.target_duration.max(hls_target_duration(candidate)?);
        if candidate_end <= current_end {
            return Some(());
        }
        let mut segment_count = current_end;
        append_hls_sequence_zero_archive_suffix(
            &mut self.body,
            &mut segment_count,
            &mut self.media_end,
            &self.source,
            candidate,
        )?;
        let position = usize::try_from(current_end - timeline.sequence).ok()?;
        self.segments
            .extend_from_slice(timeline.segments.get(position..)?);
        (segment_count == u64::try_from(self.segments.len()).ok()?).then_some(())?;
        self.source.clear();
        self.source.extend_from_slice(candidate);
        Some(())
    }

    fn finish(mut self, expected_segment_count: u64) -> Option<Vec<u8>> {
        raise_hls_target_duration(&mut self.body, self.target_duration)?;
        if self.body.len() > MAX_STREAM_FEED_PAYLOAD_BYTES
            || !hls_has_at_most_one_endlist(&self.body)
            || u64::try_from(self.segments.len()).ok()? != expected_segment_count
        {
            return None;
        }
        let parsed = HlsTimeline::parse(&self.body)?;
        (parsed.sequence == 0
            && parsed.end() == Some(expected_segment_count)
            && parsed.segments == self.segments)
            .then_some(self.body)
    }
}

pub(crate) fn assemble_hls_sequence_zero_suffix<'a>(
    current_index: u64,
    current: &[u8],
    head_index: u64,
    head: &[u8],
    candidates: impl IntoIterator<Item = (u64, &'a [u8])>,
) -> Option<Vec<u8>> {
    if current_index > head_index || hls_is_finalized(current) {
        return None;
    }
    let current_timeline = hls_complete_history_timeline(current)?;
    let head_timeline = hls_complete_history_timeline(head)?;
    if current_timeline.sequence != 0 || !hls_sparse_history_head_is_supported(head) {
        return None;
    }

    let mut aggregate_bytes = current.len().checked_add(head.len())?;
    if aggregate_bytes > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES {
        return None;
    }
    let mut parsed = Vec::new();
    let mut indices = HashSet::new();
    for (index, bytes) in candidates {
        if parsed.len() >= HLS_SPARSE_HISTORY_MAX_CANDIDATES
            || index <= current_index
            || index >= head_index
            || !indices.insert(index)
            || !hls_sparse_history_candidate_is_supported(bytes)
        {
            return None;
        }
        aggregate_bytes = aggregate_bytes.checked_add(bytes.len())?;
        if aggregate_bytes > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES {
            return None;
        }
        parsed.push((index, bytes, HlsTimeline::parse(bytes)?));
    }
    parsed.sort_by_key(|(index, _, timeline)| {
        (
            timeline.sequence,
            timeline.end().unwrap_or(u64::MAX),
            *index,
        )
    });

    let mut archive = HlsSparseHistoryArchive::new(current, &current_timeline)?;
    for (_, bytes, timeline) in parsed {
        archive.admit(bytes, &timeline)?;
    }
    archive.admit(head, &head_timeline)?;
    if hls_is_finalized(head) && !hls_is_finalized(&archive.body) {
        if !archive.body.ends_with(b"\n") {
            archive.body.push(b'\n');
        }
        archive.body.extend_from_slice(HLS_ENDLIST.as_bytes());
        archive.body.push(b'\n');
    }
    let archive = archive.finish(head_timeline.end()?)?;
    (hls_is_finalized(&archive) == hls_is_finalized(head)
        && hls_sequence_zero_covers_head(current, &archive)
        && hls_sequence_zero_covers_head(head, &archive))
    .then_some(archive)
}

fn hls_complete_history_timeline(bytes: &[u8]) -> Option<HlsTimeline> {
    if bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES
        || !hls_has_at_most_one_endlist(bytes)
        || hls_target_duration(bytes)? == 0
    {
        return None;
    }
    let timeline = HlsTimeline::parse(bytes)?;
    if timeline.segments.is_empty()
        || timeline.segments.len() > HLS_SPARSE_HISTORY_MAX_SEGMENTS
        || timeline.segments.iter().any(|segment| {
            let duration = f64::from_bits(segment.duration_bits);
            !duration.is_finite() || duration <= 0.0
        })
    {
        return None;
    }
    Some(timeline)
}

fn hls_sparse_history_timeline(bytes: &[u8]) -> Option<HlsTimeline> {
    (bytes.len() <= HLS_SPARSE_HISTORY_MAX_WINDOW_BYTES
        && hls_append_only_tags_are_supported(bytes)
        && std::str::from_utf8(bytes).ok().is_some_and(|text| {
            !text.lines().any(|line| {
                let line = line.trim();
                line == "#EXT-X-DISCONTINUITY" || line.starts_with("#EXT-X-DISCONTINUITY-SEQUENCE:")
            })
        }))
    .then_some(())?;
    hls_complete_history_timeline(bytes)
}

fn hls_append_only_tags_are_supported(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    text.lines().all(|original_line| {
        let line = original_line.trim_end_matches('\r').trim();
        !line.starts_with("#EXT-X-")
            || [
                "#EXT-X-VERSION:",
                "#EXT-X-TARGETDURATION:",
                "#EXT-X-MEDIA-SEQUENCE:",
                "#EXT-X-DISCONTINUITY-SEQUENCE:",
                "#EXT-X-PLAYLIST-TYPE:",
                "#EXT-X-START:",
                "#EXT-X-SERVER-CONTROL:",
                "#EXT-X-ALLOW-CACHE:",
                "#EXT-X-BYTERANGE:",
            ]
            .iter()
            .any(|allowed| line.starts_with(allowed))
            || matches!(
                line,
                "#EXT-X-INDEPENDENT-SEGMENTS"
                    | "#EXT-X-DISCONTINUITY"
                    | "#EXT-X-GAP"
                    | "#EXT-X-ENDLIST"
            )
            || line.starts_with("#EXTINF:")
    })
}

fn hls_has_at_most_one_endlist(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).ok().is_some_and(|text| {
        text.lines()
            .filter(|line| line.trim() == HLS_ENDLIST)
            .take(2)
            .count()
            <= 1
    })
}

pub(crate) fn hls_media_sequence(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut values = text.lines().filter_map(|line| {
        line.trim()
            .strip_prefix("#EXT-X-MEDIA-SEQUENCE:")
            .map(str::trim)
    });
    match (values.next(), values.next()) {
        (None, None) => Some(0),
        (Some(value), None) => value.parse().ok(),
        _ => None,
    }
}

pub(crate) fn hls_timeline_rebase_required(
    previous_start_sequence: u64,
    previous_live: bool,
    candidate_start_sequence: u64,
) -> bool {
    previous_live && candidate_start_sequence < previous_start_sequence
}

pub(crate) fn hls_timeline_rebase_position(
    previous_edge: f64,
    current_time: f64,
    candidate_edge: f64,
) -> Option<f64> {
    (previous_edge.is_finite()
        && current_time.is_finite()
        && candidate_edge.is_finite()
        && previous_edge >= 0.0
        && current_time >= 0.0
        && candidate_edge >= 0.0)
        .then(|| {
            (candidate_edge - (previous_edge - current_time).max(0.0)).clamp(0.0, candidate_edge)
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HlsLevelTransition {
    pub(crate) rebase: bool,
}

pub(crate) fn classify_hls_level_transition(
    previous: Option<(u64, bool)>,
    rebase_attempted: bool,
    candidate_start_sequence: u64,
) -> HlsLevelTransition {
    HlsLevelTransition {
        rebase: !rebase_attempted
            && previous.is_some_and(|(previous_start, previous_live)| {
                hls_timeline_rebase_required(
                    previous_start,
                    previous_live,
                    candidate_start_sequence,
                )
            }),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HlsSegmentIdentity {
    reference: String,
    duration_bits: u64,
    byte_range: Option<(u64, u64)>,
    discontinuity_counter: u64,
    gap: bool,
}

struct HlsTimeline {
    sequence: u64,
    segments: Vec<HlsSegmentIdentity>,
    uri_starts: Vec<usize>,
    uri_ends: Vec<usize>,
    media_start: Option<usize>,
}

impl HlsTimeline {
    fn parse(bytes: &[u8]) -> Option<Self> {
        if !stream_feed_payload_len_is_supported(bytes.len()) || !is_hls_manifest(bytes) {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;

        let mut sequence = None;
        let mut segments = Vec::new();
        let mut uri_starts = Vec::new();
        let mut uri_ends = Vec::new();
        let mut expects_media_uri = false;
        let mut duration_bits = None::<u64>;
        let mut byte_range = None::<(u64, Option<u64>)>;
        let mut media_start = None;
        let mut previous_range_end = None::<(String, u64)>;
        let mut discontinuity_counter = 0_u64;
        let mut saw_discontinuity_sequence = false;
        let mut gap = false;
        let mut offset = 0usize;
        for original_line in text.split_inclusive('\n') {
            let line = original_line.trim();
            if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
                if sequence.is_some() {
                    return None;
                }
                sequence = Some(value.trim().parse().ok()?);
            }
            if line.starts_with("#EXTINF:") {
                if expects_media_uri {
                    return None;
                }
                let duration = line
                    .strip_prefix("#EXTINF:")?
                    .split(',')
                    .next()?
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|duration| duration.is_finite() && *duration >= 0.0)?;
                expects_media_uri = true;
                media_start.get_or_insert(offset);
                duration_bits = Some(duration.to_bits());
                byte_range = None;
            } else if let Some(value) = line.strip_prefix("#EXT-X-DISCONTINUITY-SEQUENCE:") {
                if saw_discontinuity_sequence || expects_media_uri || !segments.is_empty() {
                    return None;
                }
                discontinuity_counter = value.trim().parse().ok()?;
                saw_discontinuity_sequence = true;
            } else if line == "#EXT-X-DISCONTINUITY" {
                media_start.get_or_insert(offset);
                discontinuity_counter = discontinuity_counter.checked_add(1)?;
            } else if line == HLS_GAP {
                if gap {
                    return None;
                }
                media_start.get_or_insert(offset);
                gap = true;
            } else if let Some(value) = line.strip_prefix("#EXT-X-BYTERANGE:") {
                if !expects_media_uri || byte_range.is_some() || value.trim().is_empty() {
                    return None;
                }
                byte_range = Some(parse_hls_byte_range(value.trim())?);
            } else if hls_header_line(line) || line.starts_with('#') || line.is_empty() {
            } else if expects_media_uri {
                let reference = swarm_bytes_reference(line)?.to_ascii_lowercase();
                let effective_range = match byte_range.take() {
                    Some((length, Some(offset))) => Some((offset, length)),
                    Some((length, None)) => {
                        let (previous_reference, offset) = previous_range_end.as_ref()?;
                        if previous_reference != &reference {
                            return None;
                        }
                        Some((*offset, length))
                    }
                    None => None,
                };
                previous_range_end = match effective_range {
                    Some((offset, length)) => {
                        Some((reference.clone(), offset.checked_add(length)?))
                    }
                    None => None,
                };
                segments.push(HlsSegmentIdentity {
                    reference,
                    duration_bits: duration_bits.take()?,
                    byte_range: effective_range,
                    discontinuity_counter,
                    gap,
                });
                uri_starts.push(offset);
                uri_ends.push(offset.checked_add(original_line.len())?);
                expects_media_uri = false;
                gap = false;
            } else {
                return None;
            }
            offset = offset.checked_add(original_line.len())?;
        }
        if expects_media_uri || gap {
            return None;
        }
        Some(Self {
            sequence: sequence.unwrap_or(0),
            segments,
            uri_starts,
            uri_ends,
            media_start,
        })
    }

    fn end(&self) -> Option<u64> {
        self.sequence
            .checked_add(u64::try_from(self.segments.len()).ok()?)
    }

    fn duration_between(&self, start_sequence: u64, end_sequence: u64) -> Option<f64> {
        let start = usize::try_from(start_sequence.checked_sub(self.sequence)?).ok()?;
        let end = usize::try_from(end_sequence.checked_sub(self.sequence)?).ok()?;
        if start >= end || end > self.segments.len() {
            return None;
        }
        self.segments[start..end]
            .iter()
            .try_fold(0.0, |duration, segment| {
                let segment_duration = f64::from_bits(segment.duration_bits);
                (segment_duration.is_finite() && segment_duration > 0.0)
                    .then_some(duration + segment_duration)
            })
            .filter(|duration| duration.is_finite() && *duration > 0.0)
    }

    fn is_continuous_with(&self, candidate: &Self) -> bool {
        if self.segments.is_empty() || candidate.segments.is_empty() {
            return false;
        }
        let Some(current_end) = self.end() else {
            return false;
        };
        let Some(candidate_end) = candidate.end() else {
            return false;
        };
        if candidate_end < current_end {
            return false;
        }
        let overlap_start = self.sequence.max(candidate.sequence);
        let overlap_end = current_end.min(candidate_end);
        overlap_start < overlap_end
            && (overlap_start..overlap_end).all(|sequence| {
                let current_position = usize::try_from(sequence - self.sequence).ok();
                let candidate_position = usize::try_from(sequence - candidate.sequence).ok();
                current_position.zip(candidate_position).is_some_and(
                    |(current_position, candidate_position)| {
                        self.segments.get(current_position)
                            == candidate.segments.get(candidate_position)
                    },
                )
            })
    }

    fn is_forward_of(&self, candidate: &Self) -> bool {
        self.end()
            .zip(candidate.end())
            .is_some_and(|(current_end, candidate_end)| {
                candidate.sequence >= current_end && candidate_end > current_end
            })
    }

    fn is_adjacent_to(&self, candidate: &Self) -> bool {
        self.end() == Some(candidate.sequence) && !candidate.segments.is_empty()
    }

    fn live_tail(&self) -> Option<(usize, f64)> {
        if self.segments.is_empty() {
            return None;
        }
        let nominal_start = self.segments.len().saturating_sub(HLS_LIVE_SYNC_SEGMENTS);
        let start = if self.segments[nominal_start..]
            .iter()
            .any(|segment| !segment.gap)
        {
            nominal_start
        } else {
            self.segments
                .iter()
                .rposition(|segment| !segment.gap)?
                .saturating_sub(HLS_LIVE_SYNC_SEGMENTS.saturating_sub(1))
        };
        let duration = self.segments[start..]
            .iter()
            .try_fold(0.0, |duration, segment| {
                let segment_duration = f64::from_bits(segment.duration_bits);
                (segment_duration.is_finite() && segment_duration > 0.0)
                    .then_some(duration + segment_duration)
            })?;
        (duration.is_finite() && duration > 0.0).then_some((start, duration))
    }

    fn last_playable_interval(&self) -> Option<(f64, f64)> {
        let mut offset = 0.0;
        let mut playable = None;
        for segment in &self.segments {
            let duration = f64::from_bits(segment.duration_bits);
            if !duration.is_finite() || duration < 0.0 {
                return None;
            }
            let end = offset + duration;
            if !end.is_finite() {
                return None;
            }
            if !segment.gap && end > offset {
                playable = Some((offset, end));
            }
            offset = end;
        }
        playable
    }

    fn last_playable_segment(&self) -> Option<(u64, String, f64, f64)> {
        let mut offset = 0.0;
        let mut playable = None;
        for (position, segment) in self.segments.iter().enumerate() {
            let duration = f64::from_bits(segment.duration_bits);
            if !duration.is_finite() || duration < 0.0 {
                return None;
            }
            let end = offset + duration;
            if !end.is_finite() {
                return None;
            }
            if !segment.gap && end > offset {
                playable = Some((
                    self.sequence.checked_add(u64::try_from(position).ok()?)?,
                    segment.reference.clone(),
                    offset,
                    duration,
                ));
            }
            offset = end;
        }
        playable
    }

    fn last_playable_start_offset_from_end(&self) -> Option<f64> {
        let (playable_start, playable_end) = self.last_playable_interval()?;
        let duration = self.segments.iter().try_fold(0.0, |duration, segment| {
            let segment_duration = f64::from_bits(segment.duration_bits);
            (segment_duration.is_finite() && segment_duration >= 0.0)
                .then_some(duration + segment_duration)
        })?;
        (duration.is_finite() && duration > playable_end)
            .then_some(duration - playable_start)
            .filter(|offset| offset.is_finite() && *offset > 0.0)
    }
}

pub(crate) fn hls_sequence_zero_body_from_deferred_prefix(
    authenticated_prefix: &[u8],
    payload_span: u64,
    minimum_segments: usize,
) -> Option<Vec<u8>> {
    let prefix_len = u64::try_from(authenticated_prefix.len()).ok()?;
    if minimum_segments == 0
        || prefix_len == 0
        || prefix_len >= payload_span
        || payload_span > MAX_STREAM_FEED_PAYLOAD_BYTES as u64
        || hls_is_finalized(authenticated_prefix)
    {
        return None;
    }

    let mut complete_end = authenticated_prefix
        .iter()
        .rposition(|byte| *byte == b'\n')?
        .checked_add(1)?;
    loop {
        if let Some(timeline) = HlsTimeline::parse(authenticated_prefix.get(..complete_end)?)
            && timeline.sequence == 0
            && timeline.segments.len() >= minimum_segments
            && !hls_is_finalized(authenticated_prefix.get(..complete_end)?)
        {
            let body_end = *timeline.uri_ends.last()?;
            let body = authenticated_prefix.get(..body_end)?.to_vec();
            let body_timeline = HlsTimeline::parse(&body)?;
            return (body_timeline.sequence == 0
                && body_timeline.segments.len() == timeline.segments.len()
                && body_timeline.segments.len() >= minimum_segments
                && !hls_is_finalized(&body))
            .then_some(body);
        }
        let previous = complete_end.checked_sub(1)?;
        complete_end = authenticated_prefix
            .get(..previous)?
            .iter()
            .rposition(|byte| *byte == b'\n')?
            .checked_add(1)?;
    }
}

pub(crate) fn hls_deferred_feed_completion_matches(
    payload_span: u64,
    authenticated_prefix: &[u8],
    complete: &[u8],
) -> bool {
    u64::try_from(complete.len()).ok() == Some(payload_span)
        && u64::try_from(authenticated_prefix.len())
            .ok()
            .is_some_and(|prefix_len| prefix_len < payload_span)
        && complete.starts_with(authenticated_prefix)
}

fn hls_segment_identities(bytes: &[u8]) -> Option<Vec<HlsSegmentIdentity>> {
    Some(HlsTimeline::parse(bytes)?.segments)
}

fn parse_hls_byte_range(value: &str) -> Option<(u64, Option<u64>)> {
    let mut parts = value.split('@');
    let length = parts.next()?.trim().parse::<u64>().ok()?;
    if length == 0 {
        return None;
    }
    let offset = parts
        .next()
        .map(str::trim)
        .map(str::parse::<u64>)
        .transpose()
        .ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((length, offset))
}

pub(crate) fn hls_media_references(bytes: &[u8]) -> Vec<String> {
    if !stream_feed_payload_len_is_supported(bytes.len()) || !is_hls_manifest(bytes) {
        return Vec::new();
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };

    let mut references = Vec::new();
    let mut expects_media_uri = false;
    let mut gap = false;
    for original_line in text.lines() {
        let line = original_line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with("#EXTINF:") {
            expects_media_uri = true;
            continue;
        }

        if line == HLS_GAP {
            gap = true;
            continue;
        }

        if line.starts_with("#EXT-X-PART:") || line.starts_with("#EXT-X-PRELOAD-HINT:") {
            for (start, end) in uri_attribute_value_ranges(line) {
                if let Some(reference) = swarm_bytes_reference(&line[start..end]) {
                    push_distinct_reference(&mut references, reference);
                }
            }
            continue;
        }

        if line.starts_with('#') {
            continue;
        }

        if expects_media_uri {
            if !gap && let Some(reference) = swarm_bytes_reference(line) {
                push_distinct_reference(&mut references, reference);
            }
            expects_media_uri = false;
            gap = false;
        }
    }

    references
}

fn push_distinct_reference(references: &mut Vec<String>, reference: &str) {
    let reference = reference.to_ascii_lowercase();
    if references.last() != Some(&reference) {
        references.push(reference);
    }
}

pub(crate) fn hls_payload_mime(bytes: &[u8]) -> &'static str {
    if bytes.first() == Some(&0x47) && bytes.get(188) == Some(&0x47) {
        "video/mp2t"
    } else if bytes
        .get(4..8)
        .is_some_and(|kind| matches!(kind, b"ftyp" | b"styp" | b"moof" | b"moov"))
    {
        "video/mp4"
    } else if bytes.starts_with(b"WEBVTT") {
        "text/vtt; charset=utf-8"
    } else if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xf6 == 0xf0 {
        "audio/aac"
    } else if is_hls_manifest(bytes) {
        "application/vnd.apple.mpegurl"
    } else {
        "application/octet-stream"
    }
}

pub(crate) fn rewrite_hls_manifest(bytes: &[u8], local_bytes_base: &str) -> Option<Vec<u8>> {
    rewrite_hls_manifest_inner(bytes, local_bytes_base, false, false, HlsStart::Beginning)
}

pub(crate) fn rewrite_hls_manifest_for_live_reload(
    bytes: &[u8],
    local_bytes_base: &str,
    head_finalized: bool,
    start: HlsStart,
) -> Option<Vec<u8>> {
    rewrite_hls_manifest_inner(bytes, local_bytes_base, true, head_finalized, start)
}

pub(crate) fn prepend_hls_codec_bootstrap(
    manifest: &[u8],
    bootstrap_manifest: &[u8],
) -> Option<Vec<u8>> {
    if hls_media_sequence(manifest)? == 0 {
        return rewrite_hls_sequence_zero_codec_bootstrap(manifest, true);
    }
    if !hls_codec_bootstrap_tags_are_supported(bootstrap_manifest) {
        return None;
    }
    let current = hls_segment_identities(manifest)?;
    let mut bootstrap_segments = hls_segment_identities(bootstrap_manifest)?;
    if hls_media_sequence(bootstrap_manifest) != Some(0) {
        bootstrap_segments.reverse();
    }
    let bootstrap = bootstrap_segments.into_iter().find(|candidate| {
        !candidate.gap
            && !current
                .iter()
                .any(|segment| segment.reference == candidate.reference)
    })?;
    if bootstrap.byte_range.is_some() || bootstrap.discontinuity_counter != 0 {
        return None;
    }
    rewrite_hls_codec_bootstrap(manifest, Some(bootstrap))
}

pub(crate) fn continue_hls_codec_bootstrap(manifest: &[u8]) -> Option<Vec<u8>> {
    if hls_media_sequence(manifest)? == 0 {
        return rewrite_hls_sequence_zero_codec_bootstrap(manifest, false);
    }
    rewrite_hls_codec_bootstrap(manifest, None)
}

pub(crate) fn open_hls_codec_continuation(manifest: &[u8]) -> Option<Vec<u8>> {
    if !stream_feed_payload_len_is_supported(manifest.len()) || !is_hls_manifest(manifest) {
        return None;
    }
    let text = std::str::from_utf8(manifest).ok()?;
    let playlist_types = text
        .lines()
        .filter(|line| line.trim().starts_with("#EXT-X-PLAYLIST-TYPE:"))
        .count();
    if playlist_types > 1 {
        return None;
    }

    let had_trailing_newline = text.ends_with('\n');
    let mut output = String::with_capacity(text.len().saturating_add(32));
    let mut wrote_header = false;
    for original_line in text.lines() {
        let line = original_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed == HLS_ENDLIST {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        if hls_header_line(line) {
            if wrote_header {
                return None;
            }
            wrote_header = true;
            output.push_str(line);
            if playlist_types == 0 {
                output.push_str("\n#EXT-X-PLAYLIST-TYPE:EVENT");
            }
        } else if trimmed.starts_with("#EXT-X-PLAYLIST-TYPE:") {
            let value = trimmed.strip_prefix("#EXT-X-PLAYLIST-TYPE:")?.trim();
            if !matches!(value, "EVENT" | "VOD") {
                return None;
            }
            output.push_str("#EXT-X-PLAYLIST-TYPE:EVENT");
        } else {
            output.push_str(line);
        }
    }
    if !wrote_header {
        return None;
    }
    if had_trailing_newline {
        output.push('\n');
    }
    let output = output.into_bytes();
    (stream_feed_payload_len_is_supported(output.len())
        && !hls_is_finalized(&output)
        && std::str::from_utf8(&output).ok().is_some_and(|text| {
            text.lines()
                .any(|line| line.trim() == "#EXT-X-PLAYLIST-TYPE:EVENT")
        }))
    .then_some(output)
}

pub(crate) fn hls_codec_bootstrap_token(source: &str) -> Option<u64> {
    source
        .split_once('?')
        .map(|(_, query)| query)
        .into_iter()
        .flat_map(|query| query.split('&'))
        .find_map(|parameter| parameter.strip_prefix("codec-bootstrap="))
        .and_then(|token| token.parse::<u64>().ok())
}

fn rewrite_hls_sequence_zero_codec_bootstrap(manifest: &[u8], bootstrap: bool) -> Option<Vec<u8>> {
    if !hls_codec_bootstrap_tags_are_supported(manifest) {
        return None;
    }
    let text = std::str::from_utf8(manifest).ok()?;
    let mut versions = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#EXT-X-VERSION:").map(str::trim));
    let version = match (versions.next(), versions.next()) {
        (Some(version), None) => version.parse::<u64>().ok()?,
        _ => return None,
    };
    let target_duration = hls_target_duration(manifest)?;
    if version == 0 || target_duration == 0 {
        return None;
    }
    let segments = hls_segment_identities(manifest)?;
    if segments
        .iter()
        .any(|segment| segment.byte_range.is_some() || segment.discontinuity_counter != 0)
    {
        return None;
    }
    if segments.is_empty() {
        return None;
    }
    let tail_start = segments.len().saturating_sub(HLS_LIVE_SYNC_SEGMENTS);

    let mut output = String::with_capacity(manifest.len().saturating_add(32));
    output.push_str(HLS_HEADER);
    output.push_str(&format!(
        "\n#EXT-X-VERSION:{version}\n#EXT-X-TARGETDURATION:{target_duration}\n#EXT-X-PLAYLIST-TYPE:{}\n#EXT-X-MEDIA-SEQUENCE:0",
        if bootstrap || !hls_is_finalized(manifest) {
            "EVENT"
        } else {
            "VOD"
        }
    ));
    for (position, segment) in segments.iter().enumerate() {
        if tail_start > 0 && position == tail_start {
            output.push_str("\n#EXT-X-DISCONTINUITY");
        }
        output.push_str(&format!(
            "\n#EXTINF:{},",
            f64::from_bits(segment.duration_bits)
        ));
        if segment.gap {
            output.push_str("\n#EXT-X-GAP");
        }
        output.push_str(&format!("\n{}", segment.reference));
    }
    if !bootstrap && hls_is_finalized(manifest) {
        output.push_str("\n#EXT-X-ENDLIST");
    }
    output.push('\n');

    let output = output.into_bytes();
    (stream_feed_payload_len_is_supported(output.len())
        && hls_media_sequence(&output) == Some(0)
        && hls_segment_identities(&output)?.len() == segments.len())
    .then_some(output)
}

fn rewrite_hls_codec_bootstrap(
    manifest: &[u8],
    bootstrap: Option<HlsSegmentIdentity>,
) -> Option<Vec<u8>> {
    let sequence = hls_media_sequence(manifest)?;
    if !hls_codec_bootstrap_tags_are_supported(manifest) {
        return None;
    }
    if sequence == 0 {
        return None;
    }
    let segments = hls_segment_identities(manifest)?;
    if segments.is_empty()
        || bootstrap.as_ref().is_some_and(|bootstrap| {
            segments
                .iter()
                .any(|segment| segment.reference == bootstrap.reference)
        })
    {
        return None;
    }
    let text = std::str::from_utf8(manifest).ok()?;
    let mut discontinuity_sequence = None;
    for line in text.lines() {
        if let Some(value) = line.trim().strip_prefix("#EXT-X-DISCONTINUITY-SEQUENCE:") {
            if discontinuity_sequence.is_some() {
                return None;
            }
            discontinuity_sequence = Some(value.trim().parse::<u64>().ok()?);
        }
    }
    let continued = bootstrap.is_none();
    let rewritten_discontinuity_sequence = discontinuity_sequence
        .unwrap_or(0)
        .checked_add(u64::from(continued))?;
    let had_trailing_newline = text.ends_with('\n');
    let extra_capacity = bootstrap
        .as_ref()
        .map_or(40, |bootstrap| bootstrap.reference.len() + 96);
    let mut output = String::with_capacity(text.len() + extra_capacity);
    let mut header_seen = false;
    let mut media_sequence_seen = false;
    for line in text.lines() {
        let trimmed = line.trim_end_matches('\r').trim();
        if trimmed.starts_with("#EXT-X-DISCONTINUITY-SEQUENCE:") {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        if hls_header_line(line) {
            if header_seen {
                return None;
            }
            header_seen = true;
            output.push_str(line.trim_end_matches('\r'));
            if discontinuity_sequence.is_some() || continued {
                output.push_str(&format!(
                    "\n#EXT-X-DISCONTINUITY-SEQUENCE:{rewritten_discontinuity_sequence}"
                ));
            }
        } else if trimmed.strip_prefix("#EXT-X-MEDIA-SEQUENCE:").is_some() {
            if media_sequence_seen {
                return None;
            }
            media_sequence_seen = true;
            match bootstrap.as_ref() {
                Some(bootstrap) => {
                    output.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{}", sequence - 1));
                    output.push_str(&format!(
                        "\n#EXTINF:{},\n{}\n#EXT-X-DISCONTINUITY",
                        f64::from_bits(bootstrap.duration_bits),
                        bootstrap.reference
                    ));
                }
                None => output.push_str(line.trim_end_matches('\r')),
            }
        } else {
            output.push_str(line.trim_end_matches('\r'));
        }
    }
    if !header_seen || !media_sequence_seen {
        return None;
    }
    if had_trailing_newline {
        output.push('\n');
    }
    stream_feed_payload_len_is_supported(output.len()).then(|| output.into_bytes())
}

fn hls_codec_bootstrap_tags_are_supported(bytes: &[u8]) -> bool {
    hls_append_only_tags_are_supported(bytes)
        && std::str::from_utf8(bytes).ok().is_some_and(|text| {
            text.lines()
                .all(|line| !line.trim().starts_with("#EXT-X-BYTERANGE:"))
        })
}

fn rewrite_hls_manifest_inner(
    bytes: &[u8],
    local_bytes_base: &str,
    normalize_unindexed_feed: bool,
    head_finalized: bool,
    start: HlsStart,
) -> Option<Vec<u8>> {
    if !stream_feed_payload_len_is_supported(bytes.len()) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    if !is_hls_manifest(bytes) {
        return None;
    }

    let rewrite_vod_as_event = normalize_unindexed_feed
        && text
            .lines()
            .map(|line| line.trim_end_matches('\r'))
            .any(hls_vod_playlist_type_line);
    let provisional_unindexed_feed = normalize_unindexed_feed && !head_finalized;
    let force_archived_start =
        start == HlsStart::Beginning && (rewrite_vod_as_event || provisional_unindexed_feed);
    let strip_start = normalize_unindexed_feed && start == HlsStart::Live;
    let live_start_offset = if strip_start {
        let finalized_gap_offset = if head_finalized && hls_is_finalized(bytes) {
            HlsTimeline::parse(bytes)
                .and_then(|timeline| timeline.last_playable_start_offset_from_end())
        } else {
            None
        };
        finalized_gap_offset.or_else(|| hls_live_tail(bytes).map(|(_, duration)| duration))
    } else {
        None
    };
    let forced_start_tag = if force_archived_start {
        Some("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES".to_string())
    } else {
        live_start_offset.map(|offset| format!("#EXT-X-START:TIME-OFFSET=-{offset},PRECISE=NO"))
    };
    let has_start_tag = forced_start_tag.is_some()
        && text
            .lines()
            .map(|line| line.trim_end_matches('\r'))
            .any(hls_start_tag_line);
    let mut output = String::with_capacity(text.len() + 128);
    let had_trailing_newline = text.ends_with('\n');

    let mut wrote_line = false;
    let mut wrote_forced_start = false;
    for original_line in text.lines() {
        let line = original_line.trim_end_matches('\r');
        let rewrote_playlist_type = rewrite_vod_as_event && hls_vod_playlist_type_line(line);
        let rewritten = if rewrote_playlist_type {
            Some(Cow::Owned(hls_event_playlist_type_line(line)))
        } else if provisional_unindexed_feed && line.trim() == HLS_ENDLIST {
            None
        } else if (forced_start_tag.is_some() || strip_start) && hls_start_tag_line(line) {
            if wrote_forced_start {
                None
            } else if let Some(forced_start_tag) = forced_start_tag.as_deref() {
                wrote_forced_start = true;
                Some(Cow::Borrowed(forced_start_tag))
            } else {
                None
            }
        } else if line.trim_start().starts_with('#') {
            if server_control_attribute_start(line).is_some() {
                strip_unsupported_server_control_claims(line)
            } else {
                Some(rewrite_tag_uri_attributes(line, local_bytes_base))
            }
        } else {
            let trimmed = line.trim();
            if let Some(local_uri) = local_swarm_uri(trimmed, local_bytes_base) {
                let indentation = line.len().saturating_sub(line.trim_start().len());
                let content_end = line.trim_end().len();
                let mut rewritten = String::with_capacity(line.len() + local_bytes_base.len());
                rewritten.push_str(&line[..indentation]);
                rewritten.push_str(&local_uri);
                rewritten.push_str(&line[content_end..]);
                Some(Cow::Owned(rewritten))
            } else {
                Some(Cow::Borrowed(line))
            }
        };

        let Some(rewritten) = rewritten else {
            continue;
        };
        if wrote_line {
            output.push('\n');
        }
        output.push_str(rewritten.as_ref());
        wrote_line = true;

        let insert_start_after_line =
            rewrote_playlist_type || (!rewrite_vod_as_event && hls_header_line(line));
        if let Some(forced_start_tag) = forced_start_tag.as_deref()
            && insert_start_after_line
            && !has_start_tag
            && !wrote_forced_start
        {
            output.push('\n');
            output.push_str(forced_start_tag);
            wrote_forced_start = true;
        }
    }

    if had_trailing_newline {
        output.push('\n');
    }
    Some(output.into_bytes())
}

fn hls_vod_playlist_type_line(line: &str) -> bool {
    line.trim()
        .strip_prefix("#EXT-X-PLAYLIST-TYPE:")
        .is_some_and(|value| value.trim() == "VOD")
}

fn hls_start_tag_line(line: &str) -> bool {
    line.trim_start().starts_with("#EXT-X-START:")
}

fn hls_event_playlist_type_line(line: &str) -> String {
    let leading = line.len().saturating_sub(line.trim_start().len());
    let trailing = line.len().saturating_sub(line.trim_end().len());
    let suffix_start = line.len().saturating_sub(trailing);
    let mut rewritten = String::with_capacity(line.len().saturating_add(2));
    rewritten.push_str(&line[..leading]);
    rewritten.push_str("#EXT-X-PLAYLIST-TYPE:EVENT");
    rewritten.push_str(&line[suffix_start..]);
    rewritten
}

fn strip_unsupported_server_control_claims(line: &str) -> Option<Cow<'_, str>> {
    let attribute_start = server_control_attribute_start(line)?;
    let ranges = attribute_ranges(line, attribute_start);
    let mut removed_any = false;
    let retained = ranges
        .into_iter()
        .filter_map(|(start, end)| {
            let attribute = line.get(start..end)?;
            let name = attribute
                .split_once('=')
                .map(|(name, _)| name)
                .unwrap_or(attribute)
                .trim();
            if is_unsupported_server_control_claim(name) {
                removed_any = true;
                None
            } else if attribute.trim().is_empty() {
                None
            } else {
                Some(attribute.trim())
            }
        })
        .collect::<Vec<_>>();

    if !removed_any {
        return Some(Cow::Borrowed(line));
    }
    if retained.is_empty() {
        return None;
    }

    let mut output = String::with_capacity(line.len());
    output.push_str(&line[..attribute_start]);
    output.push_str(&retained.join(","));
    Some(Cow::Owned(output))
}

fn server_control_attribute_start(line: &str) -> Option<usize> {
    let indentation = line.len().saturating_sub(line.trim_start().len());
    let tag = line.get(indentation..)?;
    let colon = tag.find(':')?;
    tag[..colon]
        .eq_ignore_ascii_case(HLS_SERVER_CONTROL)
        .then_some(indentation + colon + 1)
}

fn is_unsupported_server_control_claim(name: &str) -> bool {
    name.eq_ignore_ascii_case("CAN-BLOCK-RELOAD")
        || name.eq_ignore_ascii_case("CAN-SKIP-UNTIL")
        || name.eq_ignore_ascii_case("CAN-SKIP-DATERANGES")
}

fn attribute_ranges(line: &str, start: usize) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut ranges = Vec::new();
    let mut attribute_start = start;
    let mut cursor = attribute_start;
    let mut in_quotes = false;

    while cursor <= bytes.len() {
        let at_end = cursor == bytes.len();
        if !at_end && bytes[cursor] == b'"' {
            in_quotes = !in_quotes;
        }
        if at_end || (bytes[cursor] == b',' && !in_quotes) {
            ranges.push((attribute_start, cursor));
            attribute_start = cursor.saturating_add(1);
        }
        cursor += 1;
    }
    ranges
}

fn rewrite_tag_uri_attributes<'a>(line: &'a str, local_bytes_base: &str) -> Cow<'a, str> {
    let replacements = uri_attribute_value_ranges(line)
        .into_iter()
        .filter_map(|(value_start, value_end)| {
            local_swarm_uri(&line[value_start..value_end], local_bytes_base)
                .map(|uri| (value_start, value_end, uri))
        })
        .collect::<Vec<_>>();
    if replacements.is_empty() {
        return Cow::Borrowed(line);
    }

    let mut output = String::with_capacity(line.len());
    let mut copied_until = 0;
    for (value_start, value_end, uri) in replacements {
        output.push_str(&line[copied_until..value_start]);
        output.push_str(&uri);
        copied_until = value_end;
    }
    output.push_str(&line[copied_until..]);
    Cow::Owned(output)
}

fn local_swarm_uri(uri: &str, local_bytes_base: &str) -> Option<String> {
    if let Some(reference) = swarm_bytes_reference(uri) {
        return Some(format!(
            "{}/{}",
            local_bytes_base.trim_end_matches('/'),
            reference
        ));
    }

    let feed_route = swarm_feed_route(uri)?;
    let bytes_base = local_bytes_base.trim_end_matches('/');
    let feed_base = bytes_base.strip_suffix("/hls/bytes")?;
    Some(format!("{feed_base}/feeds/{feed_route}"))
}

fn swarm_feed_route(uri: &str) -> Option<String> {
    if uri != uri.trim() || uri.contains('#') {
        return None;
    }
    let (path, query) = match uri.split_once('?') {
        Some((path, query)) => {
            let (name, index) = query.split_once('=')?;
            if !name.eq_ignore_ascii_case("index")
                || index.is_empty()
                || !index.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            (path, Some(query))
        }
        None => (uri, None),
    };
    if path.ends_with('/') {
        return None;
    }

    let route_path = http_route_path(path)?;
    let mut components = route_path.trim_matches('/').rsplit('/');
    let topic = components.next()?;
    let owner = components.next()?;
    let route = components.next()?;
    if !route.eq_ignore_ascii_case("feeds") || !is_hex_len(owner, 40) || !is_hex_len(topic, 64) {
        return None;
    }

    let mut local = format!("{owner}/{topic}");
    if let Some(query) = query {
        local.push('?');
        local.push_str(query);
    }
    Some(local)
}

fn http_route_path(uri: &str) -> Option<&str> {
    let first_path_part = uri.split('/').next().unwrap_or_default();
    if first_path_part.contains(':') {
        let is_http = first_path_part
            .strip_suffix(':')
            .map(|scheme| {
                scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
            })
            .unwrap_or(false);
        if !is_http
            || !uri
                .get(first_path_part.len()..)
                .is_some_and(|remainder| remainder.starts_with("//"))
        {
            return None;
        }
    }

    if let Some((scheme, remainder)) = uri.split_once("://") {
        if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
            return None;
        }
        let authority_end = remainder.find('/')?;
        if authority_end == 0 {
            return None;
        }
        Some(&remainder[authority_end..])
    } else if let Some(remainder) = uri.strip_prefix("//") {
        let authority_end = remainder.find('/')?;
        if authority_end == 0 {
            return None;
        }
        Some(&remainder[authority_end..])
    } else {
        Some(uri)
    }
}

fn uri_attribute_value_ranges(line: &str) -> Vec<(usize, usize)> {
    let Some(colon) = line.find(':') else {
        return Vec::new();
    };
    attribute_ranges(line, colon + 1)
        .into_iter()
        .filter_map(|(start, end)| uri_value_range(line, start, end))
        .collect()
}

fn uri_value_range(line: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let attribute = line.get(start..end)?;
    let equals = attribute.find('=')?;
    if !attribute[..equals].trim().eq_ignore_ascii_case("URI") {
        return None;
    }

    let raw_value = &attribute[equals + 1..];
    let leading_whitespace = raw_value.len() - raw_value.trim_start().len();
    let quoted = raw_value.get(leading_whitespace..)?;
    if !quoted.starts_with('"') {
        return None;
    }
    let value = quoted.get(1..)?;
    let closing_quote = value.find('"')?;
    if !value[closing_quote + 1..].trim().is_empty() {
        return None;
    }

    let value_start = start + equals + 1 + leading_whitespace + 1;
    Some((value_start, value_start + closing_quote))
}

fn swarm_bytes_reference(uri: &str) -> Option<&str> {
    if uri != uri.trim() {
        return None;
    }
    let without_fragment = uri.split('#').next().unwrap_or(uri);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    let normalized = without_query.trim().trim_end_matches('/');

    if is_hex_reference(normalized) {
        return suffix_slice(uri, normalized.len());
    }

    let route_path = http_route_path(normalized)?;

    let (prefix, reference) = route_path.rsplit_once('/')?;
    let (_, route) = prefix.rsplit_once('/').unwrap_or(("", prefix));
    if route.eq_ignore_ascii_case("bytes") && is_hex_reference(reference) {
        return suffix_slice(uri, reference.len());
    }

    None
}

fn suffix_slice(value: &str, suffix_len: usize) -> Option<&str> {
    let candidate_end = value.find(['?', '#']).unwrap_or(value.len());
    let candidate = value.get(..candidate_end)?.trim_end_matches('/');
    candidate.get(candidate.len().checked_sub(suffix_len)?..)
}

fn is_hex_reference(value: &str) -> bool {
    (value.len() == 64 || value.len() == 128) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn stream_feed_payload_len_is_supported(length: usize) -> bool {
    length <= MAX_STREAM_FEED_PAYLOAD_BYTES
}

pub(crate) const FEED_FOLLOWUP_BATCH_LIMIT: usize = 4;
pub(crate) const HLS_TERMINAL_CONFIRMATION_MIN_PRICED_PEERS: u64 = 8;
const FEED_HEAD_REFRESH_INTERVAL_MS: f64 = 15_000.0;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum FeedFollowupMode {
    Canonical,
    SequenceZeroPresentation,
}

pub(crate) fn hls_snapshot_is_terminal(
    has_endlist: bool,
    explicit_index: bool,
    head_confirmed: bool,
) -> bool {
    has_endlist && (explicit_index || head_confirmed)
}

pub(crate) fn hls_terminal_peer_view_is_mature(priced_peer_count: u64) -> bool {
    priced_peer_count >= HLS_TERMINAL_CONFIRMATION_MIN_PRICED_PEERS
}

pub(crate) fn cached_feed_should_refresh_head(last_head_check_ms: f64, now_ms: f64) -> bool {
    last_head_check_ms.is_finite()
        && now_ms.is_finite()
        && now_ms >= last_head_check_ms
        && now_ms - last_head_check_ms >= FEED_HEAD_REFRESH_INTERVAL_MS
}

pub(crate) fn hls_live_frontier_is_ready(
    snapshot_index: u64,
    confirmed_head_index: Option<u64>,
    last_head_check_ms: f64,
    checked_after_ms: f64,
) -> bool {
    confirmed_head_index == Some(snapshot_index)
        && last_head_check_ms.is_finite()
        && last_head_check_ms > checked_after_ms
}

const STORAGE_KEY: &str = "weeb3-hls-vod-index-hints-v2";
const MAX_HINTS: usize = 256;
const MAX_SERIALIZED_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct VodIndexHint {
    network_id: u64,
    owner: String,
    topic: String,
    index: u64,
    touched_ms: u64,
}

fn canonical_identity(
    network_id: u64,
    owner: &str,
    normalized_topic: &str,
) -> Option<(u64, String, String)> {
    let owner = owner
        .strip_prefix("0x")
        .or_else(|| owner.strip_prefix("0X"))
        .unwrap_or(owner);
    if owner.len() != 40 || !owner.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    if normalized_topic.len() != 64
        || !normalized_topic
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some((
        network_id,
        owner.to_ascii_lowercase(),
        normalized_topic.to_ascii_lowercase(),
    ))
}

fn sanitize(mut hints: Vec<VodIndexHint>) -> Vec<VodIndexHint> {
    hints.retain(|hint| canonical_identity(hint.network_id, &hint.owner, &hint.topic).is_some());
    hints.sort_by(|left, right| right.touched_ms.cmp(&left.touched_ms));

    let mut unique = Vec::<VodIndexHint>::with_capacity(hints.len().min(MAX_HINTS));
    for mut hint in hints {
        let Some((network_id, owner, topic)) =
            canonical_identity(hint.network_id, &hint.owner, &hint.topic)
        else {
            continue;
        };
        hint.network_id = network_id;
        hint.owner = owner;
        hint.topic = topic;
        if let Some(existing) = unique.iter_mut().find(|existing| {
            existing.network_id == hint.network_id
                && existing.owner == hint.owner
                && existing.topic == hint.topic
        }) {
            if hint.index > existing.index {
                existing.index = hint.index;
            }
            existing.touched_ms = existing.touched_ms.max(hint.touched_ms);
            continue;
        }
        unique.push(hint);
        if unique.len() == MAX_HINTS {
            break;
        }
    }
    unique
}

fn upsert(
    hints: &mut Vec<VodIndexHint>,
    network_id: u64,
    owner: &str,
    normalized_topic: &str,
    index: u64,
    touched_ms: u64,
) {
    let Some((network_id, owner, topic)) = canonical_identity(network_id, owner, normalized_topic)
    else {
        return;
    };
    if let Some(existing) = hints
        .iter_mut()
        .find(|hint| hint.network_id == network_id && hint.owner == owner && hint.topic == topic)
    {
        existing.index = existing.index.max(index);
        existing.touched_ms = touched_ms;
    } else {
        hints.push(VodIndexHint {
            network_id,
            owner,
            topic,
            index,
            touched_ms,
        });
    }
}

fn compact_for_storage(mut hints: Vec<VodIndexHint>) -> Option<String> {
    hints = sanitize(hints);
    loop {
        let serialized = serde_json::to_string(&hints).ok()?;
        if serialized.len() <= MAX_SERIALIZED_BYTES {
            return Some(serialized);
        }
        hints.pop()?;
    }
}

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

#[cfg(target_arch = "wasm32")]
fn read_storage() -> Vec<VodIndexHint> {
    let Some(storage) = storage() else {
        return Vec::new();
    };
    let Some(raw) = storage.get_item(STORAGE_KEY).ok().flatten() else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<VodIndexHint>>(&raw)
        .map(sanitize)
        .unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
fn write_storage(hints: Vec<VodIndexHint>) {
    let Some(storage) = storage() else {
        return;
    };
    let Some(serialized) = compact_for_storage(hints) else {
        return;
    };
    let _ = storage.set_item(STORAGE_KEY, &serialized);
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> u64 {
    js_sys::Date::now().max(0.0) as u64
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn remember_authenticated_endlist_index(
    network_id: u64,
    owner: &str,
    normalized_topic: &str,
    index: u64,
) {
    let mut hints = read_storage();
    upsert(
        &mut hints,
        network_id,
        owner,
        normalized_topic,
        index,
        now_ms(),
    );
    write_storage(hints);
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn persisted_vod_index(
    network_id: u64,
    owner: &str,
    normalized_topic: &str,
) -> Option<u64> {
    let (network_id, owner, topic) = canonical_identity(network_id, owner, normalized_topic)?;
    read_storage()
        .into_iter()
        .find(|hint| hint.network_id == network_id && hint.owner == owner && hint.topic == topic)
        .map(|hint| hint.index)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn forget_vod_index(network_id: u64, owner: &str, normalized_topic: &str) {
    let Some((network_id, owner, topic)) = canonical_identity(network_id, owner, normalized_topic)
    else {
        return;
    };
    let mut hints = read_storage();
    hints
        .retain(|hint| hint.network_id != network_id || hint.owner != owner || hint.topic != topic);
    write_storage(hints);
}

#[cfg(target_arch = "wasm32")]
fn js_error_message(error: &wasm_bindgen::JsValue) -> String {
    js_sys::Reflect::get(error, &wasm_bindgen::JsValue::from_str("message"))
        .ok()
        .and_then(|message| message.as_string())
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "unknown browser error".to_string())
}

#[cfg(target_arch = "wasm32")]
mod player {
    //! Rust owns HLS policy; `hls.js` supplies MSE playback through dynamic import.

    use std::{
        cell::RefCell,
        collections::{HashMap, HashSet},
        time::Duration,
    };

    use async_std::task::sleep;
    use js_sys::{Array, Error, Function, Object, Promise, Reflect};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{CustomEvent, CustomEventInit, Element, Event, HtmlMediaElement};

    use super::{
        HLS_BUFFER_EDGE_EPSILON_SECONDS, HLS_LIVE_SYNC_SEGMENTS, HLS_LIVE_TAIL_FAILURE_THRESHOLD,
        HlsMediaKind, HlsPlayAttemptPhase, HlsPlayAttemptSettlement, HlsStart,
        classify_hls_level_transition, hls_autoplay_gate_ready, hls_autoplay_start_target,
        hls_buffered_interval_covers, hls_contiguous_buffered_ahead, hls_dom_pause_is_explicit,
        hls_dom_play_is_explicit, hls_finalized_edge_load_target, hls_live_retarget_target,
        hls_live_tail_fallback_segment, hls_play_attempt_settlement, hls_timeline_rebase_position,
        js_error_message, swarm_bytes_reference,
    };

    const SWARM_REQUEST_TIMEOUT_MS: f64 = 240_000.0;
    const MAX_NETWORK_RECOVERY_ATTEMPTS: u8 = 2;
    const MAX_HARD_RESTART_ATTEMPTS: u8 = 2;
    const HLS_AUTOPLAY_GATE_POLL: Duration = Duration::from_millis(50);
    const HLS_WARMUP_STOP_DELAY: Duration = Duration::from_millis(500);
    const HLS_TRACK_REMOVED_ERROR_NAME: &str = "HlsJsTrackRemovedError";
    const HLS_MISSING_VIDEO_SOURCE_BUFFER_MESSAGE: &str =
        "Attempting to append to the video SourceBuffer, but it does not exist";

    const HLS_CALLBACK_EVENTS: [&str; 6] = [
        "hlsError",
        "hlsBufferCreated",
        "hlsFragLoading",
        "hlsFragBuffered",
        "hlsLevelLoaded",
        "hlsManifestParsed",
    ];
    const HLS_DOM_EVENTS: [&str; 3] = ["play", "pause", "resize"];
    const NATIVE_DOM_EVENTS: [&str; 4] = ["play", "pause", "error", "loadedmetadata"];
    pub(crate) const HLS_AUTOPLAY_AUTHORIZED_EVENT: &str = "weeb3-hls-autoplay-authorized";
    pub(crate) const HLS_EXPLICIT_PAUSE_EVENT: &str = "weeb3-hls-explicit-pause";
    pub(crate) const HLS_TIMELINE_REBASE_EVENT: &str = "weeb3-hls-timeline-rebase";
    pub(crate) const HLS_WARMUP_START_EVENT: &str = "weeb3-hls-warmup-start";
    pub(crate) const HLS_AUTOPLAY_PENDING_ATTRIBUTE: &str = "data-weeb3-hls-autoplay-pending";
    pub(crate) const HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE: &str = "data-weeb3-hls-playback-authorized";

    #[wasm_bindgen(module = "/static/hls_loader.js")]
    extern "C" {
        #[wasm_bindgen(js_name = loadHls)]
        pub(crate) fn load_hls() -> Promise;
    }

    #[wasm_bindgen]
    extern "C" {
        #[derive(Clone)]
        type Hls;

        #[wasm_bindgen(catch, method, js_name = on)]
        fn on(this: &Hls, event: &str, callback: &Function) -> Result<(), JsValue>;

        #[wasm_bindgen(catch, method, js_name = off)]
        fn off(this: &Hls, event: &str, callback: &Function) -> Result<(), JsValue>;

        #[wasm_bindgen(catch, method, js_name = loadSource)]
        fn load_source(this: &Hls, source: &str) -> Result<(), JsValue>;

        #[wasm_bindgen(catch, method, js_name = attachMedia)]
        fn attach_media(this: &Hls, media: &HtmlMediaElement) -> Result<(), JsValue>;

        #[wasm_bindgen(catch, method, js_name = startLoad)]
        fn start_load(this: &Hls) -> Result<(), JsValue>;

        #[wasm_bindgen(catch, method, js_name = startLoad)]
        fn start_load_at(this: &Hls, position: f64) -> Result<(), JsValue>;

        #[wasm_bindgen(catch, method, js_name = stopLoad)]
        fn stop_load(this: &Hls) -> Result<(), JsValue>;

        #[wasm_bindgen(catch, method, js_name = recoverMediaError)]
        fn recover_media_error(this: &Hls) -> Result<(), JsValue>;

        #[wasm_bindgen(catch, method, js_name = destroy)]
        fn destroy(this: &Hls) -> Result<(), JsValue>;
    }

    thread_local! {
        static PLAYER: RefCell<Player> = const {
            RefCell::new(Player { epoch: 0, session: None })
        };
    }

    enum Backend {
        Hls(Hls),
        Native,
    }

    struct HlsListener {
        registered: usize,
        callback: Closure<dyn FnMut(JsValue, JsValue)>,
    }

    struct DomListener {
        names: &'static [&'static str],
        registered: usize,
        callback: Closure<dyn FnMut(Event)>,
    }

    enum LoadPhase {
        Cold,
        Warmup,
        Started,
    }

    enum AutoplayGatePoll {
        Wait,
        Ready(f64),
        Stop,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum LiveRetargetTarget {
        Edge(f64),
        Resume(f64),
    }

    impl LiveRetargetTarget {
        fn position(self) -> f64 {
            match self {
                Self::Edge(position) | Self::Resume(position) => position,
            }
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum LiveRetarget {
        Inactive,
        AwaitingContinuation(LiveRetargetTarget),
        AwaitingFallback(LiveRetargetTarget),
        AwaitingTarget(LiveRetargetTarget),
    }

    impl LiveRetarget {
        fn pending(self) -> bool {
            !matches!(self, Self::Inactive)
        }

        fn load_position(self) -> Option<f64> {
            match self {
                Self::AwaitingContinuation(target)
                | Self::AwaitingFallback(target)
                | Self::AwaitingTarget(target) => Some(target.position()),
                Self::Inactive => None,
            }
        }
    }

    #[derive(Clone)]
    struct BufferedMainFragment {
        identity: (u64, String),
        start: f64,
        duration: f64,
    }

    impl BufferedMainFragment {
        fn matches(&self, candidate: &Self) -> bool {
            self.identity == candidate.identity
                && (self.start - candidate.start).abs() <= HLS_BUFFER_EDGE_EPSILON_SECONDS
                && (self.duration - candidate.duration).abs() <= HLS_BUFFER_EDGE_EPSILON_SECONDS
        }
    }

    struct CodecContinuationEdge {
        revision: u64,
        fragment: BufferedMainFragment,
    }

    #[derive(Clone, Copy)]
    struct PlayAttempt {
        token: u64,
        phase: HlsPlayAttemptPhase,
        intent: Autoplay,
        target: Option<f64>,
        live_retarget: LiveRetarget,
        resume: bool,
        resume_position: Option<f64>,
        initial_position: f64,
        rebase_position: Option<f64>,
    }

    enum PlayAttemptAction {
        Commit(HtmlMediaElement),
        Rollback {
            media: HtmlMediaElement,
            target: Option<f64>,
            superseded: bool,
        },
    }

    struct Session {
        media: HtmlMediaElement,
        source: String,
        backend: Backend,
        hls_listener: Option<HlsListener>,
        dom_listener: Option<DomListener>,
        codec_required: bool,
        codec_pending: bool,
        codec_video_buffer_ready: bool,
        codec_loading_fragments: HashSet<(u64, String)>,
        codec_bootstrap_completed: bool,
        codec_edge_position: Option<f64>,
        codec_continuation_open: bool,
        codec_continuation_revision: u64,
        codec_continuation_edge: Option<CodecContinuationEdge>,
        codec_continuation_candidate: Option<CodecContinuationEdge>,
        codec_continuation_loading_fragments: HashMap<(u64, String), u64>,
        load: LoadPhase,
        manifest_parsed: bool,
        playback_authorized: bool,
        autoplay_allowed: bool,
        play_attempt_serial: u64,
        play_attempt: Option<PlayAttempt>,
        autoplay_gate_pending: bool,
        resume: bool,
        recovery_pending: bool,
        hard_restarts: u8,
        network_recoveries: u8,
        media_recoveries: u8,
        live_tail_failures: Vec<(u64, u64, String)>,
        timeline_rebased: bool,
        initial_position: f64,
        rebase_position: Option<f64>,
        resume_position: Option<f64>,
        live_retarget: LiveRetarget,
        start: HlsStart,
        beginning_prefix_stamp: Option<super::runtime::PlaybackStamp>,
        level_snapshots: HashMap<u64, (u64, bool, Option<f64>, Option<(f64, f64)>)>,
    }

    struct Player {
        epoch: u64,
        session: Option<Session>,
    }

    struct Launch {
        media: HtmlMediaElement,
        source: String,
        loader: Option<JsFuture>,
        initial_position: f64,
        rebase_position: Option<f64>,
        resume_position: Option<f64>,
        hard_attempts: u8,
        live_tail_failures: Vec<(u64, u64, String)>,
        timeline_rebased: bool,
        resume: bool,
        autoplay_allowed: bool,
        codec_bootstrap: bool,
        codec_edge_position: Option<f64>,
        live_retarget: LiveRetarget,
        start: HlsStart,
        beginning_prefix_stamp: Option<super::runtime::PlaybackStamp>,
    }

    enum EventKind {
        Hls(String, JsValue),
        Dom(String),
    }

    enum Wait {
        Microtask,
        Millis(u64),
    }

    enum Recovery {
        Network(u64),
        Media(Hls),
        Hard(Wait, HtmlMediaElement, String, u8, bool),
        Stop(&'static str, JsValue),
    }

    enum LiveTailFallbackAction {
        SameInstance(Hls, f64, Autoplay),
        Hard,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Autoplay {
        Policy,
        Resume,
    }

    impl Autoplay {
        fn allowed(self, session: &Session) -> bool {
            match self {
                Self::Policy => session.autoplay_allowed,
                Self::Resume => session.resume,
            }
        }
    }

    impl Session {
        fn play_attempt_phase(&self) -> Option<HlsPlayAttemptPhase> {
            self.play_attempt.map(|attempt| attempt.phase)
        }

        fn play_attempt_identity(&self) -> Option<(u64, HlsPlayAttemptPhase)> {
            self.play_attempt
                .map(|attempt| (attempt.token, attempt.phase))
        }

        fn play_attempt_snapshot_current(&self, attempt: PlayAttempt) -> bool {
            self.live_retarget == attempt.live_retarget
                && self.resume == attempt.resume
                && self.resume_position == attempt.resume_position
                && self.initial_position == attempt.initial_position
                && self.rebase_position == attempt.rebase_position
                && attempt.intent.allowed(self)
                && !self.playback_authorized
                && !self.codec_pending
        }

        fn hls(&self) -> Option<&Hls> {
            match &self.backend {
                Backend::Hls(hls) => Some(hls),
                Backend::Native => None,
            }
        }

        fn restart_position(&self) -> f64 {
            if self.codec_pending {
                return 0.0;
            }
            if let Some(position) = self.live_retarget.load_position() {
                return position;
            }
            if let Some(position) = self.rebase_position {
                return position;
            }
            if let Some(position) = self.resume_position {
                return position;
            }
            let current = self.media.current_time();
            if current.is_finite()
                && current >= 0.0
                && (self.start == HlsStart::Beginning || current > 0.0)
            {
                current
            } else {
                self.initial_position
            }
        }

        fn remember_current_position(&mut self) {
            let current = self.media.current_time();
            if current.is_finite() && current >= 0.0 {
                self.resume_position = Some(current);
            }
        }

        fn dispose(mut self) {
            self.media
                .remove_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE)
                .ok();
            if let Some(listener) = self.dom_listener.take() {
                dispose_dom_listener(&self.media, listener);
            }
            match self.backend {
                Backend::Hls(hls) => {
                    dispose_hls_listener(&hls, self.hls_listener.take());
                }
                Backend::Native => {
                    let _ = self.media.pause();
                    self.media.remove_attribute("src").ok();
                    self.media.load();
                }
            }
        }
    }

    fn with_session<T>(epoch: u64, action: impl FnOnce(&mut Session) -> T) -> Option<T> {
        PLAYER.with(|player| {
            let mut player = player.try_borrow_mut().ok()?;
            (player.epoch == epoch).then_some(())?;
            Some(action(player.session.as_mut()?))
        })
    }

    fn is_current(epoch: u64) -> bool {
        PLAYER.with(|player| {
            player
                .try_borrow()
                .is_ok_and(|player| player.epoch == epoch)
        })
    }

    pub(super) fn hls_codec_bootstrap_token_is_current(token: u64) -> bool {
        PLAYER.with(|player| {
            player.try_borrow().is_ok_and(|player| {
                player.session.as_ref().is_some_and(|session| {
                    matches!(&session.backend, Backend::Hls(_))
                        && super::hls_codec_bootstrap_token(&session.source) == Some(token)
                })
            })
        })
    }

    fn next_epoch() -> u64 {
        let (epoch, retired) = PLAYER.with(|player| {
            let mut player = player.borrow_mut();
            player.epoch = player.epoch.wrapping_add(1).max(1);
            (player.epoch, player.session.take())
        });
        if let Some(session) = retired {
            session.dispose();
        }
        epoch
    }

    pub(crate) fn destroy_current_hls() {
        next_epoch();
    }

    pub(crate) async fn play_hls(
        player: &Element,
        source: &str,
        hls_loader: JsFuture,
        start: HlsStart,
        beginning_prefix_stamp: Option<super::runtime::PlaybackStamp>,
    ) -> Result<&'static str, JsValue> {
        let media = player
            .clone()
            .dyn_into::<HtmlMediaElement>()
            .map_err(|_| JsValue::from_str("HLS playback requires an HTML media element"))?;
        let source = source.trim();
        if source.is_empty() {
            return Err(JsValue::from_str(
                "HLS playback requires a non-empty source",
            ));
        }
        let autoplay_allowed = media.autoplay();
        media.set_autoplay(false);
        launch(Launch {
            media,
            source: source.to_string(),
            loader: Some(hls_loader),
            initial_position: match start {
                HlsStart::Beginning => 0.0,
                HlsStart::Live => -1.0,
            },
            rebase_position: None,
            resume_position: None,
            hard_attempts: 0,
            live_tail_failures: Vec::new(),
            timeline_rebased: false,
            resume: false,
            autoplay_allowed,
            codec_bootstrap: false,
            codec_edge_position: None,
            live_retarget: LiveRetarget::Inactive,
            start,
            beginning_prefix_stamp,
        })
        .await
    }

    async fn launch(mut request: Launch) -> Result<&'static str, JsValue> {
        request.media.set_autoplay(false);
        request.resume |= request
            .media
            .get_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
            .as_deref()
            == Some("1");
        let epoch = next_epoch();
        request.media.remove_attribute("data-weeb3-hls-mode").ok();
        request.media.remove_attribute("data-weeb3-hls-state").ok();
        request
            .media
            .remove_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE)
            .ok();
        request
            .media
            .remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
            .ok();

        let native_supported = supports_native_hls(&request.media);
        let hls_class = match match request.loader.take() {
            Some(loader) => loader.await,
            None => JsFuture::from(load_hls()).await,
        } {
            Ok(hls_class) => Some(hls_class),
            Err(error) => {
                if !is_current(epoch) {
                    return Ok("superseded");
                }
                if !native_supported {
                    return Err(error);
                }
                None
            }
        };
        if !is_current(epoch) {
            return Ok("superseded");
        }

        let mse_supported = match &hls_class {
            Some(hls_class) => match hls_is_supported(hls_class) {
                Ok(supported) => supported,
                Err(_) if native_supported => false,
                Err(error) => return Err(error),
            },
            None => false,
        };
        if !mse_supported {
            if !native_supported {
                return Err(JsValue::from_str(
                    "HLS playback is not supported by this browser",
                ));
            }
            request
                .media
                .set_attribute("data-weeb3-hls-mode", "native")?;
            let rebase = request.rebase_position.or_else(|| {
                (request.initial_position.is_finite() && request.initial_position > 0.0)
                    .then_some(request.initial_position)
            });
            if let Some(stamp) = request.beginning_prefix_stamp {
                super::runtime::bypass_hls_beginning_prefix(stamp);
            }
            let dom = install_dom_listener(&request.media, epoch, &NATIVE_DOM_EVENTS)?;
            let media = request.media.clone();
            let source = request.source.clone();
            let resume = request.resume;
            install_session(
                epoch,
                session_from(
                    &request,
                    Backend::Native,
                    (None, Some(dom)),
                    (false, false),
                    LoadPhase::Started,
                    true,
                    false,
                    rebase,
                ),
            )?;
            if request.autoplay_allowed {
                media.set_preload("auto");
            }
            media.set_src(&source);
            media.load();
            if resume {
                autoplay(epoch, Autoplay::Resume, None);
            } else {
                autoplay(epoch, Autoplay::Policy, None);
            }
            return Ok("native");
        }

        request
            .media
            .set_attribute("data-weeb3-hls-mode", "hls.js")?;
        request
            .media
            .set_attribute("data-weeb3-hls-state", "loading-manifest")?;
        let codec_bootstrap = request.codec_bootstrap;
        let codec = (codec_bootstrap, codec_bootstrap);
        if codec.1 || request.resume {
            let _ = request.media.pause();
        }
        let config = hls_config(request.start == HlsStart::Live);
        let hls_class = hls_class
            .as_ref()
            .expect("an MSE-capable load must retain the hls.js class");
        let hls = construct_hls(hls_class, &config)?;
        if codec_bootstrap {
            install_hls_codec_bootstrap_loader(hls_class, &hls)?;
        }
        let hls_listener = install_hls_listener(&hls, epoch)?;
        let dom_listener = match install_dom_listener(&request.media, epoch, &HLS_DOM_EVENTS) {
            Ok(listener) => listener,
            Err(error) => {
                dispose_hls_listener(&hls, Some(hls_listener));
                return Err(error);
            }
        };
        let media = request.media.clone();
        let source = request.source.clone();
        let rebase = request.rebase_position;
        install_session(
            epoch,
            session_from(
                &request,
                Backend::Hls(hls.clone()),
                (Some(hls_listener), Some(dom_listener)),
                codec,
                LoadPhase::Cold,
                false,
                request.resume,
                rebase,
            ),
        )?;
        let attached = hls
            .load_source(&source)
            .and_then(|_| hls.attach_media(&media));
        if let Err(error) = attached {
            if is_current(epoch) {
                destroy_current_hls();
            }
            return Err(error);
        }

        Ok("hls.js")
    }

    fn session_from(
        request: &Launch,
        backend: Backend,
        listeners: (Option<HlsListener>, Option<DomListener>),
        codec: (bool, bool),
        load: LoadPhase,
        manifest_parsed: bool,
        resume: bool,
        rebase_position: Option<f64>,
    ) -> Session {
        let live_retarget = if matches!(&backend, Backend::Hls(_)) {
            request.live_retarget
        } else {
            LiveRetarget::Inactive
        };
        Session {
            media: request.media.clone(),
            source: request.source.clone(),
            backend,
            hls_listener: listeners.0,
            dom_listener: listeners.1,
            codec_required: codec.0,
            codec_pending: codec.1,
            codec_video_buffer_ready: false,
            codec_loading_fragments: HashSet::new(),
            codec_bootstrap_completed: false,
            codec_edge_position: request.codec_edge_position,
            codec_continuation_open: false,
            codec_continuation_revision: 0,
            codec_continuation_edge: None,
            codec_continuation_candidate: None,
            codec_continuation_loading_fragments: HashMap::new(),
            load,
            manifest_parsed,
            playback_authorized: false,
            autoplay_allowed: request.autoplay_allowed,
            play_attempt_serial: 0,
            play_attempt: None,
            autoplay_gate_pending: false,
            resume,
            recovery_pending: false,
            hard_restarts: request.hard_attempts,
            network_recoveries: 0,
            media_recoveries: 0,
            live_tail_failures: request.live_tail_failures.clone(),
            timeline_rebased: request.timeline_rebased,
            initial_position: request.initial_position,
            rebase_position,
            resume_position: request.resume_position,
            live_retarget,
            start: request.start,
            beginning_prefix_stamp: request.beginning_prefix_stamp,
            level_snapshots: HashMap::new(),
        }
    }

    fn install_session(epoch: u64, session: Session) -> Result<(), JsValue> {
        let mut session = Some(session);
        PLAYER.with(|player| {
            let mut player = player.borrow_mut();
            if player.epoch == epoch {
                player.session = session.take();
            }
        });
        if let Some(session) = session {
            session.dispose();
            Err(JsValue::from_str("HLS session was superseded"))
        } else {
            Ok(())
        }
    }

    fn hls_is_supported(hls_class: &JsValue) -> Result<bool, JsValue> {
        let is_supported = Reflect::get(hls_class, &JsValue::from_str("isSupported"))?
            .dyn_into::<Function>()
            .map_err(|_| JsValue::from_str("hls.js does not expose isSupported()"))?;
        is_supported
            .call0(hls_class)?
            .as_bool()
            .ok_or_else(|| JsValue::from_str("hls.js isSupported() did not return a boolean"))
    }

    fn construct_hls(hls_class: &JsValue, config: &Object) -> Result<Hls, JsValue> {
        let constructor = hls_class
            .dyn_ref::<Function>()
            .ok_or_else(|| JsValue::from_str("hls.js did not export a constructor"))?;
        let arguments = Array::new();
        arguments.push(config.as_ref());
        Reflect::construct(constructor, &arguments).map(JsCast::unchecked_into)
    }

    fn install_hls_codec_bootstrap_loader(hls_class: &JsValue, hls: &Hls) -> Result<(), JsValue> {
        let config = js_property(hls.as_ref(), "config")
            .and_then(|config| config.dyn_into::<Object>().ok())
            .ok_or_else(|| JsValue::from_str("hls.js does not expose its active config"))?;
        let progressive_loader = js_property(config.as_ref(), "loader")
            .filter(|loader| loader.is_function())
            .ok_or_else(|| JsValue::from_str("hls.js does not expose its progressive loader"))?;
        let bootstrap_loader = js_property(hls_class, "DefaultConfig")
            .and_then(|defaults| js_property(&defaults, "loader"))
            .filter(|loader| loader.is_function())
            .ok_or_else(|| JsValue::from_str("hls.js does not expose its complete-body loader"))?;
        if Object::is(&progressive_loader, &bootstrap_loader) {
            return Ok(());
        }
        Reflect::set(
            config.as_ref(),
            &JsValue::from_str("fLoader"),
            &bootstrap_loader,
        )?
        .then_some(())
        .ok_or_else(|| JsValue::from_str("hls.js rejected its codec-bootstrap loader"))
    }

    fn restore_hls_progressive_loader(hls: &Hls) -> Result<(), JsValue> {
        let config = js_property(hls.as_ref(), "config")
            .and_then(|config| config.dyn_into::<Object>().ok())
            .ok_or_else(|| JsValue::from_str("hls.js lost its active config"))?;
        Reflect::delete_property(&config, &JsValue::from_str("fLoader"))?
            .then_some(())
            .ok_or_else(|| JsValue::from_str("hls.js rejected its progressive-loader restore"))
    }

    fn strip_hls_codec_bootstrap(source: &str) -> &str {
        source
            .split_once("&codec-bootstrap=")
            .or_else(|| source.split_once("?codec-bootstrap="))
            .map_or(source, |(clean, _)| clean)
    }

    fn with_hls_codec_bootstrap(source: &str, token: u64) -> String {
        let source = strip_hls_codec_bootstrap(source);
        let separator = if source.contains('?') { '&' } else { '?' };
        format!("{source}{separator}codec-bootstrap={token}")
    }

    fn supports_native_hls(media: &HtmlMediaElement) -> bool {
        ["application/vnd.apple.mpegurl", "application/x-mpegURL"]
            .iter()
            .any(|mime| matches!(media.can_play_type(mime).as_str(), "probably" | "maybe"))
    }

    fn install_hls_listener(hls: &Hls, epoch: u64) -> Result<HlsListener, JsValue> {
        let callback =
            Closure::<dyn FnMut(JsValue, JsValue)>::new(move |event: JsValue, data: JsValue| {
                if let Some(event) = event.as_string() {
                    dispatch(epoch, EventKind::Hls(event, data));
                }
            });
        let mut listener = HlsListener {
            registered: 0,
            callback,
        };
        for name in HLS_CALLBACK_EVENTS {
            listener.registered += 1;
            if let Err(error) = hls.on(name, listener.callback.as_ref().unchecked_ref()) {
                dispose_hls_listener(hls, Some(listener));
                return Err(error);
            }
        }
        Ok(listener)
    }

    fn install_dom_listener(
        media: &HtmlMediaElement,
        epoch: u64,
        names: &'static [&'static str],
    ) -> Result<DomListener, JsValue> {
        let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
            dispatch(epoch, EventKind::Dom(event.type_()));
        });
        let mut listener = DomListener {
            names,
            registered: 0,
            callback,
        };
        for name in listener.names {
            listener.registered += 1;
            if let Err(error) = media
                .add_event_listener_with_callback(name, listener.callback.as_ref().unchecked_ref())
            {
                dispose_dom_listener(media, listener);
                return Err(error);
            }
        }
        Ok(listener)
    }

    fn dispose_hls_listener(hls: &Hls, listener: Option<HlsListener>) {
        let Some(listener) = listener else {
            let _ = hls.destroy();
            return;
        };
        let mut detached = true;
        for name in HLS_CALLBACK_EVENTS.iter().take(listener.registered) {
            detached &= hls
                .off(name, listener.callback.as_ref().unchecked_ref())
                .is_ok();
        }
        let destroyed = hls.destroy().is_ok();
        if !detached && !destroyed {
            std::mem::forget(listener);
        }
    }

    fn dispose_dom_listener(media: &HtmlMediaElement, listener: DomListener) {
        let mut detached = true;
        for name in listener.names.iter().take(listener.registered) {
            detached &= media
                .remove_event_listener_with_callback(
                    name,
                    listener.callback.as_ref().unchecked_ref(),
                )
                .is_ok();
        }
        if !detached {
            std::mem::forget(listener);
        }
    }
    fn dispatch(epoch: u64, event: EventKind) {
        match event {
            EventKind::Hls(name, data) => match name.as_str() {
                "hlsError" => handle_error(epoch, data),
                "hlsBufferCreated" => buffer_created(epoch, &data),
                "hlsFragLoading" => fragment_loading(epoch, &data),
                "hlsFragBuffered" => fragment_buffered(epoch, &data),
                "hlsLevelLoaded" => level_loaded(epoch, &data),
                "hlsManifestParsed" => manifest_parsed(epoch),
                _ => {}
            },
            EventKind::Dom(name) => dom_event(epoch, &name),
        }
    }

    fn fragment_loading(epoch: u64, data: &JsValue) {
        if !is_current(epoch) {
            return;
        }
        let Some(fragment) = js_property(data, "frag") else {
            return;
        };
        let kind = match js_string_property(&fragment, "type").as_deref() {
            Some("main") => Some(HlsMediaKind::Main),
            Some("audio") => Some(HlsMediaKind::Audio),
            Some(_) => None,
            None => return,
        };
        let Some(reference) = js_string_property(&fragment, "url")
            .and_then(|url| swarm_bytes_reference(&url).map(str::to_ascii_lowercase))
        else {
            return;
        };
        if kind == Some(HlsMediaKind::Main)
            && let Some(sequence) = js_safe_u64_property(&fragment, "sn")
        {
            with_session(epoch, |session| {
                if session.codec_pending {
                    session
                        .codec_loading_fragments
                        .insert((sequence, reference.clone()));
                } else if session.codec_continuation_open {
                    // Fallback reserves a fresh revision before its finite playlist kick, so a
                    // fragment synchronously loaded from old details cannot be acknowledged until
                    // the subsequent LEVEL_LOADED accepts the same runtime-presented identity.
                    session.codec_continuation_loading_fragments.insert(
                        (sequence, reference.clone()),
                        session.codec_continuation_revision,
                    );
                }
            });
        }
        super::runtime::remember_hls_fragment_kind(&reference, kind);
    }

    fn buffer_created(epoch: u64, data: &JsValue) {
        let Some(track) =
            js_property(data, "tracks").and_then(|tracks| js_property(&tracks, "video"))
        else {
            return;
        };
        if let Some(resolution) = js_video_resolution(&track) {
            super::set_hls_video_resolution(Some(resolution));
        }
        with_session(epoch, |session| {
            if session.codec_pending {
                session.codec_video_buffer_ready = true;
            }
        });
    }

    fn main_swarm_fragment(data: &JsValue) -> Option<(u64, String)> {
        let fragment = js_property(data, "frag")?;
        (js_string_property(&fragment, "type").as_deref() == Some("main")).then_some(())?;
        let sequence = js_safe_u64_property(&fragment, "sn")?;
        let reference = js_string_property(&fragment, "url")
            .and_then(|url| swarm_bytes_reference(&url).map(str::to_ascii_lowercase))?;
        Some((sequence, reference))
    }

    fn buffered_main_swarm_fragment(data: &JsValue) -> Option<BufferedMainFragment> {
        let fragment = js_property(data, "frag")?;
        let identity = main_swarm_fragment(data)?;
        let start = js_finite_nonnegative_property(&fragment, "start")?;
        let duration = js_finite_positive_property(&fragment, "duration")?;
        Some(BufferedMainFragment {
            identity,
            start,
            duration,
        })
    }

    fn accepted_continuation_edge(details: &JsValue) -> Option<BufferedMainFragment> {
        let fragments = js_property(details, "fragments")?;
        Array::is_array(&fragments).then_some(())?;
        let fragments = Array::from(&fragments);
        for index in (0..fragments.length()).rev() {
            let fragment = fragments.get(index);
            if js_bool_property(&fragment, "gap").unwrap_or(false) {
                continue;
            }
            if js_string_property(&fragment, "type").is_some_and(|kind| kind != "main") {
                continue;
            }
            let sequence = js_safe_u64_property(&fragment, "sn")?;
            let reference = js_string_property(&fragment, "url")
                .and_then(|url| swarm_bytes_reference(&url).map(str::to_ascii_lowercase))?;
            let start = js_finite_nonnegative_property(&fragment, "start")?;
            let duration = js_finite_positive_property(&fragment, "duration")?;
            return Some(BufferedMainFragment {
                identity: (sequence, reference),
                start,
                duration,
            });
        }
        None
    }

    fn media_buffered_ranges(media: &HtmlMediaElement) -> Vec<(f64, f64)> {
        let ranges = media.buffered();
        let mut buffered = Vec::with_capacity(ranges.length() as usize);
        for index in 0..ranges.length() {
            if let (Ok(start), Ok(end)) = (ranges.start(index), ranges.end(index)) {
                buffered.push((start, end));
            }
        }
        buffered
    }

    fn media_buffers_fragment(media: &HtmlMediaElement, fragment: &BufferedMainFragment) -> bool {
        hls_buffered_interval_covers(
            fragment.start,
            fragment.duration,
            &media_buffered_ranges(media),
        )
    }

    fn fragment_matches_presented_edge(
        fragment: &BufferedMainFragment,
        presented: &(u64, String, f64, f64),
    ) -> bool {
        fragment.identity.0 == presented.0
            && fragment.identity.1 == presented.1
            && (fragment.duration - presented.3).abs() <= HLS_BUFFER_EDGE_EPSILON_SECONDS
    }

    fn hls_live_sync_position(hls: &Hls) -> Option<f64> {
        Reflect::get(hls.as_ref(), &JsValue::from_str("liveSyncPosition"))
            .ok()
            .and_then(|value| value.as_f64())
            .filter(|position| position.is_finite() && *position >= 0.0)
    }

    fn live_codec_retarget(
        resume_authorized: bool,
        resume_position: Option<f64>,
        edge_position: Option<f64>,
    ) -> Option<LiveRetargetTarget> {
        if resume_authorized
            && let Some(position) =
                resume_position.filter(|position| position.is_finite() && *position >= 0.0)
        {
            return Some(LiveRetargetTarget::Resume(position));
        }
        edge_position
            .filter(|position| position.is_finite() && *position >= 0.0)
            .map(LiveRetargetTarget::Edge)
    }

    fn complete_hls_codec_bootstrap(epoch: u64, data: &JsValue) -> bool {
        let Some(fragment_identity) = main_swarm_fragment(data) else {
            return false;
        };
        let bootstrap = with_session(epoch, |session| {
            if !session.codec_pending
                || !session.codec_video_buffer_ready
                || !session.codec_loading_fragments.contains(&fragment_identity)
                || session.codec_bootstrap_completed
            {
                return None;
            }
            let hls = session.hls().cloned()?;
            let media = session.media.clone();
            let source = session.source.clone();
            let start = session.start;
            let resume_position = session.resume_position;
            let resume = session.playback_authorized || session.resume;
            let autoplay_allowed = session.autoplay_allowed;
            let live_retarget = (start == HlsStart::Live)
                .then(|| live_codec_retarget(resume, resume_position, session.codec_edge_position))
                .flatten();
            if start == HlsStart::Live && live_retarget.is_none() {
                return None;
            }
            Some((
                media,
                source,
                hls,
                start,
                resume_position,
                resume,
                autoplay_allowed,
                live_retarget,
            ))
        })
        .flatten();
        let Some((
            media,
            source,
            hls,
            start,
            resume_position,
            resume,
            autoplay_allowed,
            live_retarget,
        )) = bootstrap
        else {
            return false;
        };

        if live_retarget.is_some()
            && let Err(error) = hls.stop_load()
        {
            hard_recovery(epoch, error);
            return true;
        }
        let completed = with_session(epoch, |session| {
            if !session.codec_pending
                || !session.codec_video_buffer_ready
                || !session.codec_loading_fragments.contains(&fragment_identity)
                || session.codec_bootstrap_completed
            {
                return false;
            }
            session.codec_required = true;
            session.codec_pending = false;
            session.codec_loading_fragments.clear();
            session.codec_bootstrap_completed = true;
            session.network_recoveries = 0;
            session.media_recoveries = 0;
            if let Some(target) = live_retarget {
                session.initial_position = target.position();
                session.live_retarget = LiveRetarget::AwaitingContinuation(target);
                session.level_snapshots.clear();
                session.load = LoadPhase::Warmup;
                session.codec_continuation_open = true;
                session.codec_continuation_revision = 1;
                session.codec_continuation_edge = None;
                session.codec_continuation_candidate = None;
                session.codec_continuation_loading_fragments.clear();
            }
            true
        })
        .unwrap_or(false);
        if !completed {
            return false;
        }

        super::runtime::finish_hls_codec_bootstrap(&source);
        if let Err(error) = restore_hls_progressive_loader(&hls) {
            web_sys::console::warn_2(
                &JsValue::from_str("weeb-3 could not restore progressive HLS loading"),
                &error,
            );
        }
        if !is_current(epoch) {
            return true;
        }
        if start == HlsStart::Beginning
            && let Some(target) = resume_position.filter(|target| target.is_finite())
        {
            media.set_current_time(target.max(0.0));
        }
        if let Some(target) = live_retarget {
            super::runtime::handoff_hls_codec_bootstrap_prefetch_timeline();
            media.set_current_time(target.position());
            set_state(
                &media,
                "retargeting-live",
                "HLS decoder initialized; loading the authoritative live edge.",
            );
        }
        spawn_local(async move {
            Wait::Microtask.wait().await;
            if !is_current(epoch) {
                return;
            }
            if let Some(target) = live_retarget {
                start_at(epoch, &hls, target.position());
            } else if resume {
                start_autoplay_buffer_gate(epoch, Autoplay::Resume);
            } else if autoplay_allowed {
                start_autoplay_buffer_gate(epoch, Autoplay::Policy);
            }
        });
        true
    }

    fn settle_codec_continuation(epoch: u64, source: &str, revision: u64) {
        if super::runtime::settle_hls_codec_continuation(source) {
            with_session(epoch, |session| {
                if session
                    .codec_continuation_edge
                    .as_ref()
                    .is_some_and(|edge| edge.revision == revision)
                {
                    session.codec_continuation_open = false;
                    session.codec_continuation_edge = None;
                    session.codec_continuation_candidate = None;
                    session.codec_continuation_loading_fragments.clear();
                }
            });
        }
    }

    fn settle_buffered_codec_continuation(epoch: u64, data: &JsValue) {
        let Some(fragment) = buffered_main_swarm_fragment(data) else {
            return;
        };
        let settlement = with_session(epoch, |session| {
            if session.codec_pending
                || !session.codec_continuation_open
                || !media_buffers_fragment(&session.media, &fragment)
            {
                return None;
            }
            let revision = session
                .codec_continuation_loading_fragments
                .remove(&fragment.identity)?;
            session.codec_continuation_candidate = Some(CodecContinuationEdge {
                revision,
                fragment: fragment.clone(),
            });
            let edge = session.codec_continuation_edge.as_ref()?;
            if edge.revision != revision
                || !edge.fragment.matches(&fragment)
                || super::runtime::active_hls_finalized_playable_interval().is_none()
            {
                return None;
            }
            Some((session.source.clone(), edge.revision))
        })
        .flatten();
        let Some((source, revision)) = settlement else {
            return;
        };
        settle_codec_continuation(epoch, &source, revision);
    }

    fn fragment_buffered(epoch: u64, data: &JsValue) {
        if complete_hls_codec_bootstrap(epoch, data) {
            return;
        }
        settle_buffered_codec_continuation(epoch, data);
        let action = with_session(epoch, |session| {
            session.network_recoveries = 0;
            session.media_recoveries = 0;
            let stop = matches!(session.load, LoadPhase::Warmup)
                && session.media.paused()
                && !session.codec_pending
                && !session.live_retarget.pending()
                && !session.codec_continuation_open
                && !session.autoplay_gate_pending
                && session.play_attempt.is_none();
            (session.media.clone(), stop)
        });
        let Some((media, stop)) = action else {
            return;
        };
        set_state(&media, "buffered", "HLS media buffered through weeb-3.");
        if stop {
            spawn_local(async move {
                sleep(HLS_WARMUP_STOP_DELAY).await;
                let hls = with_session(epoch, |session| {
                    if !matches!(session.load, LoadPhase::Warmup)
                        || !session.media.paused()
                        || session.codec_pending
                        || session.live_retarget.pending()
                        || session.codec_continuation_open
                        || session.autoplay_gate_pending
                        || session.play_attempt.is_some()
                    {
                        return None;
                    }
                    session.load = LoadPhase::Started;
                    session.hls().cloned()
                })
                .flatten();
                if let Some(hls) = hls
                    && let Err(error) = hls.stop_load()
                {
                    run_recovery(
                        epoch,
                        Recovery::Stop("Could not stop bounded HLS warm-up", error),
                    );
                }
            });
        }
    }

    fn manifest_parsed(epoch: u64) {
        let action = with_session(epoch, |session| {
            session.manifest_parsed = true;
            let resume = session.resume;
            let start = matches!(session.load, LoadPhase::Cold)
                && (session.live_retarget.pending()
                    || session.rebase_position.is_some()
                    || resume
                    || session.media.paused());
            let position = session.restart_position();
            let load = start.then(|| {
                session.load = LoadPhase::Warmup;
                session.hls().cloned().map(|hls| (hls, position))
            });
            let gate = start
                .then_some(if resume {
                    Autoplay::Resume
                } else {
                    Autoplay::Policy
                })
                .filter(|intent| intent.allowed(session));
            (session.media.clone(), load.flatten(), gate)
        });
        let Some((media, load, gate)) = action else {
            return;
        };
        set_state(
            &media,
            "manifest-ready",
            "HLS manifest ready through weeb-3. Press Play if autoplay is blocked.",
        );
        if let Some((hls, position)) = load {
            emit(&media, HLS_WARMUP_START_EVENT);
            if !is_current(epoch) || !start_at(epoch, &hls, position) {
                return;
            }
        }
        if let Some(intent) = gate {
            start_autoplay_buffer_gate(epoch, intent);
        } else {
            autoplay(epoch, Autoplay::Policy, None);
        }
    }

    fn start_autoplay_buffer_gate(epoch: u64, intent: Autoplay) {
        let start = with_session(epoch, |session| {
            if !intent.allowed(session)
                || session.playback_authorized
                || session.autoplay_gate_pending
            {
                return false;
            }
            session.autoplay_gate_pending = true;
            true
        })
        .unwrap_or(false);
        if !start {
            return;
        }
        spawn_local(async move {
            let target = loop {
                let poll = with_session(epoch, |session| {
                    if session.playback_authorized || !intent.allowed(session) {
                        return AutoplayGatePoll::Stop;
                    }
                    if session.codec_pending || session.play_attempt.is_some() {
                        return AutoplayGatePoll::Wait;
                    }
                    let media = &session.media;
                    let buffered = media_buffered_ranges(media);
                    let live_sync_position = (session.start == HlsStart::Live)
                        .then(|| session.hls().and_then(hls_live_sync_position))
                        .flatten();
                    let current_time = media.current_time();
                    let finalized = session
                        .level_snapshots
                        .values()
                        .any(|(_, live, _, playable)| !live || playable.is_some());
                    let finalized_playable_interval = session
                        .level_snapshots
                        .values()
                        .filter_map(|(_, _, _, playable)| *playable)
                        .reduce(|current, candidate| {
                            if candidate.1 > current.1 {
                                candidate
                            } else {
                                current
                            }
                        });
                    let requested_position = session
                        .rebase_position
                        .or(session.resume_position)
                        .unwrap_or(session.initial_position);
                    let target = match session.live_retarget {
                        LiveRetarget::AwaitingContinuation(_)
                        | LiveRetarget::AwaitingFallback(_) => None,
                        LiveRetarget::AwaitingTarget(retarget) => hls_live_retarget_target(
                            Some(retarget.position()),
                            None,
                            None,
                            &buffered,
                        ),
                        LiveRetarget::Inactive => hls_autoplay_start_target(
                            session.start,
                            requested_position,
                            current_time,
                            live_sync_position,
                            &buffered,
                        ),
                    };
                    let Some(target) = target else {
                        return AutoplayGatePoll::Wait;
                    };
                    let ready = hls_autoplay_gate_ready(
                        hls_contiguous_buffered_ahead(target, &buffered),
                        target,
                        finalized_playable_interval
                            .map_or_else(|| media.duration(), |(_, end)| end),
                        finalized,
                    );
                    if ready {
                        AutoplayGatePoll::Ready(target)
                    } else {
                        AutoplayGatePoll::Wait
                    }
                })
                .unwrap_or(AutoplayGatePoll::Stop);
                match poll {
                    AutoplayGatePoll::Ready(target) => break Some(target),
                    AutoplayGatePoll::Stop => break None,
                    AutoplayGatePoll::Wait if is_current(epoch) => {}
                    AutoplayGatePoll::Wait => break None,
                }
                sleep(HLS_AUTOPLAY_GATE_POLL).await;
            };
            let ready_to_attempt = with_session(epoch, |session| {
                session.autoplay_gate_pending = false;
                target.is_some()
                    && intent.allowed(session)
                    && !session.playback_authorized
                    && !session.codec_pending
                    && session.play_attempt.is_none()
            })
            .unwrap_or(false);
            if ready_to_attempt && let Some(target) = target {
                autoplay(epoch, intent, Some(target));
            }
        });
    }

    fn level_loaded(epoch: u64, data: &JsValue) {
        let Some(details) = js_property(data, "details") else {
            return;
        };
        let (Some(level), Some(start), Some(live)) = (
            js_safe_u64_property(data, "level"),
            js_safe_u64_property(&details, "startSN"),
            js_bool_property(&details, "live"),
        ) else {
            return;
        };
        let edge = js_property(&details, "edge")
            .and_then(|value| value.as_f64())
            .filter(|value| value.is_finite() && *value >= 0.0);
        let continuation_open = with_session(epoch, |session| {
            session.codec_continuation_open && !session.codec_pending
        })
        .unwrap_or(false);
        let accepted_edge = continuation_open
            .then(|| accepted_continuation_edge(&details))
            .flatten();
        let finalized_playable = (!live || continuation_open)
            .then(super::runtime::active_hls_finalized_playable_presentation)
            .flatten();
        let finalized_playable_interval =
            finalized_playable.as_ref().map(|(interval, _)| *interval);
        let finalized_playable_edge = finalized_playable.map(|(_, edge)| edge);
        let action = with_session(epoch, |session| {
            let continuation = session.codec_continuation_open && !session.codec_pending;
            let playable_interval = ((!live || continuation)
                .then_some(finalized_playable_interval)
                .flatten())
            .or_else(|| (!live).then(|| edge.map(|edge| (0.0, edge))).flatten());
            let previous = session
                .level_snapshots
                .insert(level, (start, live, edge, playable_interval));
            let mut settlement = None;
            let mut gate = None;
            let mut fallback_restart = None;
            let accepted_edge = accepted_edge.clone().filter(|fragment| {
                !matches!(session.live_retarget, LiveRetarget::AwaitingFallback(_))
                    || finalized_playable_edge.as_ref().is_some_and(|presented| {
                        fragment_matches_presented_edge(fragment, presented)
                    })
            });
            if continuation && let Some(fragment) = accepted_edge {
                let revision = session
                    .codec_continuation_edge
                    .as_ref()
                    .filter(|current| current.fragment.matches(&fragment))
                    .map(|current| current.revision)
                    .or_else(|| {
                        session
                            .codec_continuation_loading_fragments
                            .get(&fragment.identity)
                            .copied()
                    })
                    .unwrap_or_else(|| {
                        if matches!(
                            session.live_retarget,
                            LiveRetarget::AwaitingContinuation(_)
                                | LiveRetarget::AwaitingFallback(_)
                        ) {
                            session.codec_continuation_revision.max(1)
                        } else {
                            session.codec_continuation_revision.wrapping_add(1).max(1)
                        }
                    });
                session.codec_continuation_revision = revision;
                session.codec_continuation_edge = Some(CodecContinuationEdge {
                    revision,
                    fragment: fragment.clone(),
                });
                let crossed_barrier = match session.live_retarget {
                    LiveRetarget::AwaitingContinuation(target) => {
                        session.live_retarget = LiveRetarget::AwaitingTarget(target);
                        true
                    }
                    LiveRetarget::AwaitingFallback(target) => {
                        session.live_retarget = LiveRetarget::AwaitingTarget(target);
                        fallback_restart = session
                            .hls()
                            .cloned()
                            .map(|hls| (session.media.clone(), hls, target.position()));
                        true
                    }
                    LiveRetarget::Inactive | LiveRetarget::AwaitingTarget(_) => false,
                };
                if crossed_barrier {
                    let intent = if session.resume {
                        Autoplay::Resume
                    } else {
                        Autoplay::Policy
                    };
                    gate = intent.allowed(session).then_some(intent);
                }
                if finalized_playable_interval.is_some()
                    && session
                        .codec_continuation_candidate
                        .as_ref()
                        .is_some_and(|candidate| {
                            candidate.revision == revision
                                && fragment.matches(&candidate.fragment)
                                && media_buffers_fragment(&session.media, &candidate.fragment)
                        })
                {
                    settlement = Some((session.source.clone(), revision));
                }
            }
            if !classify_hls_level_transition(
                previous.map(|(start, live, _, _)| (start, live)),
                session.timeline_rebased,
                start,
            )
            .rebase
            {
                return (settlement, gate, fallback_restart, None);
            }
            session.timeline_rebased = true;
            session.recovery_pending = true;
            let rebase = previous
                .and_then(|(_, _, edge, _)| edge)
                .zip(edge)
                .and_then(|(old, new)| {
                    hls_timeline_rebase_position(old, session.media.current_time(), new)
                });
            let codec_bootstrap = session.codec_required;
            let source = if codec_bootstrap && session.start == HlsStart::Live {
                with_hls_codec_bootstrap(&session.source, epoch)
            } else {
                strip_hls_codec_bootstrap(&session.source).to_string()
            };
            let request = Some((
                session.media.clone(),
                Launch {
                    media: session.media.clone(),
                    source,
                    loader: None,
                    initial_position: session.initial_position,
                    rebase_position: rebase,
                    resume_position: rebase.or(session.resume_position),
                    hard_attempts: session.hard_restarts,
                    live_tail_failures: session.live_tail_failures.clone(),
                    timeline_rebased: true,
                    resume: session.playback_authorized || session.resume,
                    autoplay_allowed: session.autoplay_allowed,
                    codec_bootstrap,
                    codec_edge_position: session.codec_edge_position,
                    live_retarget: if codec_bootstrap {
                        LiveRetarget::Inactive
                    } else {
                        session.live_retarget
                    },
                    start: session.start,
                    beginning_prefix_stamp: session.beginning_prefix_stamp,
                },
                session.hls().cloned(),
            ));
            (settlement, gate, fallback_restart, request)
        });
        let Some((settlement, gate, fallback_restart, request)) = action else {
            return;
        };
        if let Some((source, revision)) = settlement {
            settle_codec_continuation(epoch, &source, revision);
        }
        if let Some((media, hls, target)) = fallback_restart {
            if let Err(error) = hls.stop_load() {
                hard_recovery(epoch, error);
                return;
            }
            media.set_current_time(target);
            spawn_local(async move {
                Wait::Microtask.wait().await;
                if is_current(epoch) {
                    start_at(epoch, &hls, target);
                }
            });
        }
        if let Some(intent) = gate {
            start_autoplay_buffer_gate(epoch, intent);
        }
        let Some((media, request, retiring)) = request else {
            return;
        };
        set_state(
            &media,
            "rebasing-timeline",
            "Complete HLS archive found; rebuilding its finalized timeline.",
        );
        emit(&media, HLS_TIMELINE_REBASE_EVENT);
        if let Some(hls) = retiring {
            let _ = hls.stop_load();
        }
        spawn_local(async move {
            Wait::Microtask.wait().await;
            if !is_current(epoch) {
                return;
            }
            if let Err(error) = launch(request).await {
                playback_error(
                    &media,
                    &format!(
                        "Could not rebuild the finalized HLS timeline: {}",
                        js_error_message(&error)
                    ),
                    &error,
                );
            }
        });
    }

    fn dom_event(epoch: u64, name: &str) {
        match name {
            "play" => {
                let action = with_session(epoch, |session| {
                    let user = hls_dom_play_is_explicit(
                        session.play_attempt_phase(),
                        session.media.paused(),
                        session.playback_authorized,
                    );
                    if !user {
                        return (session.media.clone(), None, false, false);
                    }
                    let first = matches!(session.load, LoadPhase::Cold);
                    let resume = !first && !matches!(session.load, LoadPhase::Warmup);
                    let position = session.rebase_position.unwrap_or(session.initial_position);
                    if user && (session.codec_pending || session.live_retarget.pending()) {
                        session.autoplay_allowed = true;
                        session.resume = true;
                        return (session.media.clone(), None, false, true);
                    }
                    if user || first {
                        session.load = LoadPhase::Started;
                    }
                    if user {
                        session.playback_authorized = true;
                        session.autoplay_allowed = true;
                    }
                    let load = session.hls().cloned().and_then(|hls| {
                        (first || resume).then_some((hls, first.then_some(position)))
                    });
                    (session.media.clone(), load, user, false)
                });
                let Some((media, load, user, deferred_live)) = action else {
                    return;
                };
                if deferred_live {
                    let _ = media.pause();
                    start_autoplay_buffer_gate(epoch, Autoplay::Resume);
                    return;
                }
                if user {
                    media
                        .set_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE, "1")
                        .ok();
                    emit(&media, HLS_AUTOPLAY_AUTHORIZED_EVENT);
                }
                media
                    .set_attribute("data-weeb3-hls-state", "loading-media")
                    .ok();
                if let Some((hls, position)) = load {
                    match position {
                        Some(position) => {
                            start_at(epoch, &hls, position);
                        }
                        None => {
                            if let Err(error) = hls.start_load() {
                                hard_recovery(epoch, error);
                            }
                        }
                    }
                }
            }
            "pause" => {
                let action = with_session(epoch, |session| {
                    if !hls_dom_pause_is_explicit(
                        session.play_attempt_phase(),
                        session.playback_authorized,
                    ) {
                        return None;
                    }
                    session.remember_current_position();
                    session.playback_authorized = false;
                    session.autoplay_allowed = false;
                    session.resume = false;
                    session.load = LoadPhase::Started;
                    Some((session.media.clone(), session.hls().cloned()))
                })
                .flatten();
                let Some((media, hls)) = action else { return };
                media
                    .remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
                    .ok();
                emit(&media, HLS_EXPLICIT_PAUSE_EVENT);
                if let Some(hls) = hls
                    && let Err(error) = hls.stop_load()
                {
                    run_recovery(epoch, Recovery::Stop("Could not pause HLS loading", error));
                }
            }
            "resize" => {
                let media = with_session(epoch, |session| session.media.clone());
                if let Some(resolution) =
                    media.and_then(|media| js_video_resolution(media.as_ref()))
                {
                    super::set_hls_video_resolution(Some(resolution));
                }
            }
            "error" => {
                let Some(media) = with_session(epoch, |session| session.media.clone()) else {
                    return;
                };
                let message = media
                    .error()
                    .map(|error| error.message())
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| "native HLS media error".to_string());
                playback_error(&media, &message, &JsValue::from_str(&message));
            }
            "loadedmetadata" => {
                let seek = with_session(epoch, |session| {
                    (session.media.clone(), session.rebase_position.take())
                });
                if let Some((media, Some(position))) = seek {
                    media.set_current_time(position);
                }
            }
            _ => {}
        }
    }

    fn missing_video_source_buffer_error(data: &JsValue) -> bool {
        if !js_bool_property(data, "fatal").unwrap_or(false)
            || js_string_property(data, "type").as_deref() != Some("mediaError")
            || js_string_property(data, "details").as_deref() != Some("bufferAppendError")
            || js_string_property(data, "sourceBufferName").as_deref() != Some("video")
        {
            return false;
        }
        let Some(error) = js_property(data, "error") else {
            return false;
        };
        if js_string_property(&error, "name").as_deref() != Some(HLS_TRACK_REMOVED_ERROR_NAME)
            || js_string_property(&error, "message").as_deref()
                != Some(HLS_MISSING_VIDEO_SOURCE_BUFFER_MESSAGE)
        {
            return false;
        }
        let Some(fragment) = js_property(data, "frag") else {
            return false;
        };
        js_string_property(&fragment, "type").as_deref() == Some("main")
            && js_safe_u64_property(&fragment, "sn").is_some()
            && js_string_property(&fragment, "url")
                .is_some_and(|url| swarm_bytes_reference(&url).is_some())
    }

    fn install_live_tail_fallback(epoch: u64, data: &JsValue) -> Option<LiveTailFallbackAction> {
        if !js_bool_property(data, "fatal").unwrap_or(false)
            || !matches!(
                js_string_property(data, "type").as_deref(),
                Some("mediaError" | "networkError")
            )
            || !matches!(
                js_string_property(data, "details").as_deref(),
                Some("fragLoadError" | "fragLoadTimeOut" | "fragParsingError")
            )
        {
            return None;
        }
        let Some(fragment) = js_property(data, "frag") else {
            return None;
        };
        let (Some(sequence), Some(reference)) = (
            js_safe_u64_property(&fragment, "sn"),
            js_string_property(&fragment, "url")
                .and_then(|url| swarm_bytes_reference(&url).map(str::to_ascii_lowercase)),
        ) else {
            return None;
        };
        let Some(snapshot_index) = super::runtime::active_hls_live_snapshot_index() else {
            return None;
        };
        let candidate = with_session(epoch, |session| {
            if session.start != HlsStart::Live {
                return None;
            }
            if session.live_tail_failures.len() >= usize::from(HLS_LIVE_TAIL_FAILURE_THRESHOLD) {
                session.live_tail_failures.remove(0);
            }
            session
                .live_tail_failures
                .push((snapshot_index, sequence, reference.clone()));
            hls_live_tail_fallback_segment(&session.live_tail_failures)
        })
        .flatten();
        let Some((snapshot_index, sequence, reference)) = candidate else {
            return None;
        };
        if !super::runtime::install_hls_live_tail_fallback(snapshot_index, sequence, &reference) {
            with_session(epoch, |session| session.live_tail_failures.clear());
            return None;
        }
        let target = hls_finalized_edge_load_target(
            super::runtime::active_hls_finalized_playable_interval(),
        );
        with_session(epoch, |session| {
            session.live_tail_failures.clear();
            session.network_recoveries = 0;
            session.media_recoveries = 0;
            let Some(target) = target else {
                return LiveTailFallbackAction::Hard;
            };
            if !session.codec_bootstrap_completed {
                return LiveTailFallbackAction::Hard;
            }
            let Some(hls) = session.hls().cloned() else {
                return LiveTailFallbackAction::Hard;
            };
            session.initial_position = target;
            session.rebase_position = Some(target);
            session.codec_edge_position = Some(target);
            session.codec_continuation_open = true;
            session.live_retarget =
                LiveRetarget::AwaitingFallback(LiveRetargetTarget::Edge(target));
            session.codec_continuation_revision =
                session.codec_continuation_revision.wrapping_add(1).max(1);
            session.codec_continuation_edge = None;
            session.codec_continuation_candidate = None;
            session.codec_continuation_loading_fragments.clear();
            let intent = if session.playback_authorized || session.resume {
                session.resume = true;
                Autoplay::Resume
            } else {
                Autoplay::Policy
            };
            LiveTailFallbackAction::SameInstance(hls, target, intent)
        })
    }

    fn handle_error(epoch: u64, data: JsValue) {
        // A fatal foreground fragment event is emitted only after hls.js's loader has
        // received the settled Service Worker response. The underlying retrieve is
        // therefore still allowed to finish its normal accounting path before fallback.
        let fallback = install_live_tail_fallback(epoch, &data);
        let fallback_hard = matches!(&fallback, Some(LiveTailFallbackAction::Hard));
        if let Some(LiveTailFallbackAction::SameInstance(hls, target, intent)) = fallback {
            // hls.js stops all loading before our fatal-error listener runs. This finite kick
            // restarts the same-token EVENT playlist; stale fragment events are ignored until
            // LEVEL_LOADED accepts the fallback presentation.
            if let Err(error) = hls.start_load_at(target) {
                hard_recovery(epoch, error);
                return;
            }
            start_autoplay_buffer_gate(epoch, intent);
            return;
        }
        let codec_edge_position = js_property(&data, "frag")
            .and_then(|fragment| js_finite_nonnegative_property(&fragment, "start"));
        let codec_bootstrap = missing_video_source_buffer_error(&data)
            && codec_edge_position.is_some()
            && with_session(epoch, |session| {
                if session.codec_pending
                    || session.codec_bootstrap_completed
                    || session.hard_restarts >= MAX_HARD_RESTART_ATTEMPTS
                {
                    return false;
                }
                session.codec_required = true;
                session.codec_pending = true;
                session.codec_edge_position = codec_edge_position;
                true
            })
            .unwrap_or(false);
        if fallback_hard || codec_bootstrap {
            hard_recovery(epoch, data);
            return;
        }
        if !js_bool_property(&data, "fatal").unwrap_or(false) {
            if let Some(media) = with_session(epoch, |session| session.media.clone()) {
                web_sys::console::warn_2(
                    &JsValue::from_str("weeb-3 HLS non-fatal event"),
                    playback_diagnostic(&media, &data).as_ref(),
                );
            }
            return;
        }
        match js_string_property(&data, "type").as_deref() {
            Some("networkError") => {
                let parsed =
                    with_session(epoch, |session| session.manifest_parsed).unwrap_or(false);
                if !parsed {
                    hard_recovery(epoch, data);
                    return;
                }
                let attempt = with_session(epoch, |session| {
                    if session.network_recoveries >= MAX_NETWORK_RECOVERY_ATTEMPTS {
                        return None;
                    }
                    let attempt = session.network_recoveries;
                    session.network_recoveries = session.network_recoveries.saturating_add(1);
                    Some(attempt)
                })
                .flatten();
                let recovery = match attempt {
                    Some(attempt) => Recovery::Network(
                        (1_000_u64.saturating_mul(1_u64 << u32::from(attempt))).min(30_000),
                    ),
                    None => Recovery::Stop("HLS network recovery limit reached", data),
                };
                run_recovery(epoch, recovery);
            }
            Some("mediaError") => {
                if with_session(epoch, |session| session.codec_required).unwrap_or(false) {
                    hard_recovery(epoch, data);
                    return;
                }
                let recovery = with_session(epoch, |session| {
                    if session.media_recoveries == 0 {
                        session.media_recoveries = 1;
                        session.hls().cloned()
                    } else {
                        None
                    }
                })
                .flatten();
                if let Some(hls) = recovery {
                    run_recovery(epoch, Recovery::Media(hls));
                } else {
                    hard_recovery(epoch, data);
                }
            }
            _ => hard_recovery(epoch, data),
        }
    }

    fn hard_recovery(epoch: u64, data: JsValue) {
        let decision = with_session(epoch, |session| {
            if session.hard_restarts >= MAX_HARD_RESTART_ATTEMPTS {
                return Err(());
            }
            if session.recovery_pending {
                return Ok(None);
            }
            if session.playback_authorized || session.resume {
                session.remember_current_position();
            }
            session.recovery_pending = true;
            if session.codec_required {
                session.codec_pending = true;
                session.codec_bootstrap_completed = false;
                session.codec_continuation_open = false;
                session.codec_continuation_edge = None;
                session.codec_continuation_candidate = None;
                session.codec_continuation_loading_fragments.clear();
            }
            let source = if session.codec_required && session.start == HlsStart::Live {
                with_hls_codec_bootstrap(&session.source, epoch)
            } else {
                strip_hls_codec_bootstrap(&session.source).to_string()
            };
            let fast = session.codec_required && session.hard_restarts == 0;
            Ok(Some(Recovery::Hard(
                if fast {
                    Wait::Microtask
                } else {
                    Wait::Millis(1_000)
                },
                session.media.clone(),
                source,
                session.hard_restarts.saturating_add(1),
                session.timeline_rebased,
            )))
        });
        match decision {
            Some(Ok(Some(recovery))) => run_recovery(epoch, recovery),
            Some(Err(())) => run_recovery(
                epoch,
                Recovery::Stop("HLS media remained invalid after two clean restarts", data),
            ),
            _ => {}
        }
    }

    fn start_at(epoch: u64, hls: &Hls, position: f64) -> bool {
        match hls.start_load_at(position) {
            Ok(()) => {
                with_session(epoch, |session| session.rebase_position = None);
                true
            }
            Err(error) => {
                hard_recovery(epoch, error);
                false
            }
        }
    }

    impl Wait {
        async fn wait(self) {
            match self {
                Self::Microtask => {
                    let _ = JsFuture::from(Promise::resolve(&JsValue::UNDEFINED)).await;
                }
                Self::Millis(milliseconds) => sleep(Duration::from_millis(milliseconds)).await,
            }
        }
    }

    fn run_recovery(epoch: u64, recovery: Recovery) {
        match recovery {
            Recovery::Network(delay) => {
                let scheduled = with_session(epoch, |session| {
                    if session.recovery_pending {
                        return false;
                    }
                    session.recovery_pending = true;
                    true
                })
                .unwrap_or(false);
                if !scheduled {
                    return;
                }
                spawn_local(async move {
                    sleep(Duration::from_millis(delay)).await;
                    let resume = with_session(epoch, |session| {
                        session.recovery_pending = false;
                        if matches!(session.load, LoadPhase::Warmup) || !session.media.paused() {
                            return session
                                .hls()
                                .cloned()
                                .map(|hls| (hls, session.restart_position()));
                        }
                        None
                    })
                    .flatten();
                    if let Some((hls, position)) = resume {
                        start_at(epoch, &hls, position);
                    }
                });
            }
            Recovery::Media(hls) => spawn_local(async move {
                Wait::Microtask.wait().await;
                if is_current(epoch)
                    && let Err(error) = hls.recover_media_error()
                {
                    hard_recovery(epoch, error);
                }
            }),
            Recovery::Hard(wait, media, source, attempt, timeline_rebased) => {
                spawn_local(async move {
                    wait.wait().await;
                    let request = with_session(epoch, |session| Launch {
                        media: media.clone(),
                        source,
                        loader: None,
                        initial_position: session.restart_position(),
                        rebase_position: session.rebase_position.filter(|_| !session.codec_pending),
                        resume_position: session.resume_position,
                        hard_attempts: attempt,
                        live_tail_failures: session.live_tail_failures.clone(),
                        timeline_rebased,
                        resume: session.playback_authorized || session.resume,
                        autoplay_allowed: session.autoplay_allowed,
                        codec_bootstrap: session.codec_required,
                        codec_edge_position: session.codec_edge_position,
                        live_retarget: if session.codec_required {
                            LiveRetarget::Inactive
                        } else {
                            session.live_retarget
                        },
                        start: session.start,
                        beginning_prefix_stamp: session.beginning_prefix_stamp,
                    });
                    let Some(request) = request else { return };
                    if let Err(error) = launch(request).await {
                        playback_error(
                            &media,
                            &format!(
                                "Could not restart HLS playback: {}",
                                js_error_message(&error)
                            ),
                            &error,
                        );
                    }
                })
            }
            Recovery::Stop(message, detail) => {
                let Some(media) = with_session(epoch, |session| session.media.clone()) else {
                    return;
                };
                playback_error(&media, message, &detail);
                spawn_local(async move {
                    Wait::Microtask.wait().await;
                    if is_current(epoch) {
                        destroy_current_hls();
                    }
                });
            }
        }
    }

    fn autoplay(epoch: u64, intent: Autoplay, target: Option<f64>) {
        let attempt = with_session(epoch, |session| {
            let allowed = matches!(intent, Autoplay::Resume) || session.autoplay_allowed;
            (allowed
                && !session.codec_pending
                && !session.playback_authorized
                && session.play_attempt.is_none())
            .then(|| {
                session.play_attempt_serial = session.play_attempt_serial.wrapping_add(1).max(1);
                let token = session.play_attempt_serial;
                session.play_attempt = Some(PlayAttempt {
                    token,
                    phase: HlsPlayAttemptPhase::Pending,
                    intent,
                    target,
                    live_retarget: session.live_retarget,
                    resume: session.resume,
                    resume_position: session.resume_position,
                    initial_position: session.initial_position,
                    rebase_position: session.rebase_position,
                });
                (session.media.clone(), token)
            })
        })
        .flatten();
        let Some((media, token)) = attempt else {
            return;
        };
        media
            .set_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE, "1")
            .ok();
        if let Some(target) = target {
            let current = media.current_time();
            if !current.is_finite() || (current - target).abs() > HLS_BUFFER_EDGE_EPSILON_SECONDS {
                media.set_current_time(target);
            }
        }
        spawn_local(async move {
            let result = match media.play() {
                Ok(promise) => JsFuture::from(promise).await.map(|_| ()),
                Err(error) => Err(error),
            };
            let authorized = result.is_ok();
            let action = with_session(epoch, |session| {
                let attempt = session.play_attempt?;
                let snapshot_current = session.play_attempt_snapshot_current(attempt);
                match hls_play_attempt_settlement(
                    session.play_attempt_identity(),
                    token,
                    authorized,
                    snapshot_current,
                ) {
                    HlsPlayAttemptSettlement::Stale => None,
                    HlsPlayAttemptSettlement::Commit => {
                        session.play_attempt = None;
                        if attempt.live_retarget.pending() {
                            session.live_retarget = LiveRetarget::Inactive;
                        }
                        session.resume = false;
                        session.resume_position = None;
                        session.playback_authorized = true;
                        if !matches!(session.load, LoadPhase::Started) {
                            session.load = LoadPhase::Started;
                        }
                        Some(PlayAttemptAction::Commit(session.media.clone()))
                    }
                    settlement @ (HlsPlayAttemptSettlement::Rollback
                    | HlsPlayAttemptSettlement::Superseded) => {
                        let superseded = settlement == HlsPlayAttemptSettlement::Superseded;
                        let mut rejecting = attempt;
                        rejecting.phase = HlsPlayAttemptPhase::Rejecting;
                        session.play_attempt = Some(rejecting);
                        if !superseded {
                            session.live_retarget = attempt.live_retarget;
                            session.resume = attempt.resume;
                            session.resume_position = attempt.resume_position;
                        }
                        Some(PlayAttemptAction::Rollback {
                            media: session.media.clone(),
                            target: if superseded { None } else { attempt.target },
                            superseded,
                        })
                    }
                }
            })
            .flatten();
            let Some(action) = action else {
                return;
            };
            let (media, target, superseded) = match action {
                PlayAttemptAction::Commit(media) => {
                    media.remove_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE).ok();
                    media
                        .set_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE, "1")
                        .ok();
                    emit(&media, HLS_AUTOPLAY_AUTHORIZED_EVENT);
                    return;
                }
                PlayAttemptAction::Rollback {
                    media,
                    target,
                    superseded,
                } => (media, target, superseded),
            };
            if !media.paused() {
                let _ = media.pause();
            }
            if let Some(target) = target {
                let current = media.current_time();
                if !current.is_finite()
                    || (current - target).abs() > HLS_BUFFER_EDGE_EPSILON_SECONDS
                {
                    media.set_current_time(target);
                }
            }
            // `play`/`pause` are queued DOM events. Keep the attempt guarded through
            // the next browser task so neither can be mistaken for a manual gesture.
            Wait::Millis(1).wait().await;
            let settled = with_session(epoch, |session| {
                if session.play_attempt_identity() != Some((token, HlsPlayAttemptPhase::Rejecting))
                {
                    return None;
                }
                session.play_attempt = None;
                let gate = superseded
                    .then(|| {
                        let intent = if session.resume {
                            Autoplay::Resume
                        } else {
                            Autoplay::Policy
                        };
                        (session.live_retarget.pending()
                            && intent.allowed(session)
                            && !session.playback_authorized
                            && !session.codec_pending)
                            .then_some(intent)
                    })
                    .flatten();
                Some((session.media.clone(), gate))
            })
            .flatten();
            let Some((media, gate)) = settled else {
                return;
            };
            media.remove_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE).ok();
            if superseded {
                if let Some(intent) = gate {
                    start_autoplay_buffer_gate(epoch, intent);
                }
                return;
            }
            set_state(
                &media,
                "autoplay-blocked",
                "HLS startup media is warming. Autoplay was blocked; press Play to start playback.",
            );
        });
    }

    fn playback_error(media: &HtmlMediaElement, message: &str, detail: &JsValue) {
        let error = Error::new(message);
        let _ = Reflect::set(error.as_ref(), &JsValue::from_str("cause"), detail);
        let error: JsValue = error.into();
        web_sys::console::error_2(&JsValue::from_str("weeb-3 HLS playback error"), &error);
        media_status(media, &format!("HLS playback failed: {message}"), "error");
    }

    fn media_status(media: &HtmlMediaElement, message: &str, state: &str) {
        let Some(parent) = media.parent_element() else {
            return;
        };
        let Ok(Some(status)) = parent.query_selector(".weeb3-hls-status") else {
            return;
        };
        status.set_text_content(Some(message));
        status.set_attribute("data-state", state).ok();
    }

    fn set_state(media: &HtmlMediaElement, state: &str, message: &str) {
        media.set_attribute("data-weeb3-hls-state", state).ok();
        media_status(media, message, state);
    }

    fn emit(media: &HtmlMediaElement, name: &str) {
        let init = CustomEventInit::new();
        init.set_detail(&JsValue::UNDEFINED);
        if let Ok(event) = CustomEvent::new_with_event_init_dict(name, &init) {
            let _ = media.dispatch_event(&event);
        }
    }

    fn playback_diagnostic(media: &HtmlMediaElement, data: &JsValue) -> Object {
        let diagnostic = Object::new();
        for name in ["type", "details"] {
            set_property(
                &diagnostic,
                name,
                js_property(data, name).unwrap_or(JsValue::UNDEFINED),
            );
        }
        let fragment = js_property(data, "frag");
        for (source, target) in [("sn", "fragmentSequence"), ("url", "fragmentUrl")] {
            let value = fragment
                .as_ref()
                .and_then(|fragment| js_property(fragment, source))
                .unwrap_or(JsValue::UNDEFINED);
            set_property(&diagnostic, target, value);
        }
        set_property(
            &diagnostic,
            "currentTime",
            JsValue::from_f64(media.current_time()),
        );

        let buffered = Array::new();
        let ranges = media.buffered();
        for index in 0..ranges.length() {
            if let (Ok(start), Ok(end)) = (ranges.start(index), ranges.end(index)) {
                let range = Array::new();
                range.push(&JsValue::from_f64(start));
                range.push(&JsValue::from_f64(end));
                buffered.push(&range);
            }
        }
        set_property(&diagnostic, "buffered", buffered.into());
        diagnostic
    }

    fn hls_config(live_start: bool) -> Object {
        let config = Object::new();
        set_property(&config, "enableWorker", JsValue::TRUE);
        set_property(&config, "autoStartLoad", JsValue::FALSE);
        set_property(&config, "startFragPrefetch", JsValue::FALSE);
        set_property(&config, "progressive", JsValue::TRUE);
        for name in [
            "manifestLoadPolicy",
            "playlistLoadPolicy",
            "fragLoadPolicy",
            "keyLoadPolicy",
        ] {
            set_property(&config, name, swarm_load_policy().into());
        }

        if live_start {
            set_property(
                &config,
                "liveSyncDurationCount",
                JsValue::from_f64(HLS_LIVE_SYNC_SEGMENTS as f64),
            );
        }
        let low_memory = web_sys::window()
            .and_then(|window| js_property(window.navigator().as_ref(), "deviceMemory"))
            .and_then(|memory| memory.as_f64())
            .filter(|memory| memory.is_finite())
            .is_some_and(|memory| memory <= 2.0);
        let (length, maximum, bytes) = if low_memory {
            (30.0, 60.0, 32.0)
        } else {
            (90.0, 120.0, 96.0)
        };
        let number = |name, value| set_property(&config, name, JsValue::from_f64(value));
        number("backBufferLength", 30.0);
        number("maxBufferLength", length);
        number("maxMaxBufferLength", maximum);
        number("maxBufferSize", bytes * 1024.0 * 1024.0);
        number("maxBufferHole", 1.0);
        config
    }

    fn swarm_load_policy() -> Object {
        let retry = Object::new();
        set_property(&retry, "maxNumRetry", JsValue::from_f64(1.0));
        set_property(&retry, "retryDelayMs", JsValue::from_f64(500.0));
        set_property(&retry, "maxRetryDelayMs", JsValue::from_f64(30_000.0));
        set_property(&retry, "backoff", JsValue::from_str("exponential"));

        let defaults = Object::new();
        let number = |name, value| set_property(&defaults, name, JsValue::from_f64(value));
        number("maxTimeToFirstByteMs", SWARM_REQUEST_TIMEOUT_MS + 10_000.0);
        number("maxLoadTimeMs", SWARM_REQUEST_TIMEOUT_MS + 20_000.0);
        set_property(&defaults, "timeoutRetry", JsValue::NULL);
        set_property(&defaults, "errorRetry", retry.into());

        let policy = Object::new();
        set_property(&policy, "default", defaults.into());
        policy
    }

    fn set_property(target: &Object, name: &str, value: JsValue) {
        let _ = Reflect::set(target.as_ref(), &JsValue::from_str(name), &value);
    }

    fn js_property(target: &JsValue, name: &str) -> Option<JsValue> {
        Reflect::get(target, &JsValue::from_str(name))
            .ok()
            .filter(|value| !value.is_null() && !value.is_undefined())
    }

    fn js_string_property(target: &JsValue, name: &str) -> Option<String> {
        js_property(target, name)?.as_string()
    }

    fn js_bool_property(target: &JsValue, name: &str) -> Option<bool> {
        js_property(target, name)?.as_bool()
    }

    fn js_finite_nonnegative_property(target: &JsValue, name: &str) -> Option<f64> {
        js_property(target, name)?
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
    }

    fn js_finite_positive_property(target: &JsValue, name: &str) -> Option<f64> {
        js_property(target, name)?
            .as_f64()
            .filter(|value| value.is_finite() && *value > 0.0)
    }

    fn js_safe_u64_property(target: &JsValue, name: &str) -> Option<u64> {
        const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
        let value = js_property(target, name)?.as_f64()?;
        (value.is_finite() && value >= 0.0 && value <= MAX_SAFE_INTEGER && value.fract() == 0.0)
            .then_some(value as u64)
    }

    fn js_video_resolution(target: &JsValue) -> Option<(u32, u32)> {
        for candidate in js_property(target, "metadata")
            .into_iter()
            .chain(std::iter::once(target.clone()))
        {
            for (width_name, height_name) in [("width", "height"), ("videoWidth", "videoHeight")] {
                let dimension = |name| {
                    js_safe_u64_property(&candidate, name)
                        .and_then(|value| u32::try_from(value).ok())
                        .filter(|value| *value > 0)
                };
                if let (Some(width), Some(height)) = (dimension(width_name), dimension(height_name))
                {
                    return Some((width, height));
                }
            }
        }
        None
    }
}
#[cfg(target_arch = "wasm32")]
pub(crate) use player::{
    HLS_AUTOPLAY_AUTHORIZED_EVENT, HLS_AUTOPLAY_PENDING_ATTRIBUTE, HLS_EXPLICIT_PAUSE_EVENT,
    HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE, HLS_TIMELINE_REBASE_EVENT, HLS_WARMUP_START_EVENT,
    destroy_current_hls, load_hls, play_hls,
};

#[cfg(target_arch = "wasm32")]
mod runtime {
    use super::retrieval::{
        StartupRawScout, activate_retrieval_profile_if_requested,
        scout_data_ranges_cache_only_cancellable,
    };
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::{BTreeMap, HashMap, HashSet, VecDeque},
        future::Future,
        ops::{Deref, DerefMut},
        pin::Pin,
        rc::Rc,
        time::Duration,
    };

    use async_std::sync::Arc;
    use js_sys::{Function, Reflect};
    use libp2p::futures::future::{Either, FutureExt, join, select};
    use libp2p::futures::pin_mut;
    use libp2p::futures::stream::{self, FuturesUnordered, StreamExt};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{Element, HtmlMediaElement};

    use crate::{
        Weeb3,
        bzz_stream::{
            BzzMetadata, DeferredRawFeedPayload, DeferredRawFeedPayloadPrefix, RawFeedPayload,
            RetainedRawFeedPayloadProbe, StartupRawFeedPayload, acquire_deferred_raw_feed_payload,
            acquire_deferred_raw_feed_payload_conservative,
            acquire_deferred_raw_feed_payload_prefix_conservative,
            acquire_latest_raw_feed_payload_bounded_from, acquire_latest_raw_feed_payload_from,
            acquire_latest_raw_feed_payload_startup_observing_deferred,
            acquire_latest_raw_feed_payload_startup_observing_updates,
            acquire_raw_feed_payload_at_index, acquire_raw_feed_payload_at_index_bounded,
            acquire_raw_feed_payload_at_index_retained_status,
            probe_deferred_raw_feed_payload_tail_conservative,
        },
        feed::FEED_FRONTIER_LOOKAHEAD_TIMEOUT,
        interface::{service_worker_controls_bzz_requests, service_worker_scope_protocol_error},
        mpsc,
        network_profile::active_profile,
        normalize_feed_topic, register_retrieve_cancel_token,
        retrieval::{
            CONSERVATIVE_DEFERRED_MAX_PHYSICAL_ATTEMPTS, RawFetchLifecycleFactory,
            cached_decoded_data_root, retrieve_data_payload, retrieve_data_payload_cancellable,
            retrieve_decoded_data_root,
        },
        retrieval_conventions::{PendingGenerationRelation, pending_generation_relation},
        stream::{
            FetchResponse, HlsBackgroundRangeFlightGuard, RangeCacheState,
            begin_result_view_request, clear_completed_bzz_media_ranges,
            evict_completed_hls_ranges, hls_aligned_range_cached, hls_range_body_fully_cached,
            media_cache_max_bytes, next_media_generation, range_cache_body_bytes,
            range_cache_state, read_cached_hls_range, replace_stream_result_view,
            result_view_request_is_current, retain_media_element_callback,
        },
        stream_conventions::{
            MEDIA_PREFETCH_BATCH_YIELD_MS, MEDIA_PREFETCH_MAX_PARALLEL,
            MEDIA_STARTUP_RESPONSE_BYTES, MEDIA_STORAGE_WINDOW_BYTES, STREAMING_ROUTE_BASE,
            decode_component, if_none_match_matches, if_range_allows_range,
            media_prefetch_ahead_limit_bytes, parse_single_range, plan_media_prefetch_batch,
            route_markers, streaming_route_path,
        },
        stream_retrieve_cancel_token,
    };

    type HlsAlignedRangeState = RangeCacheState;

    fn hls_aligned_range_state(
        reference: &str,
        metadata: &BzzMetadata,
        start: u64,
        end: u64,
        generation: u64,
    ) -> HlsAlignedRangeState {
        range_cache_state(
            &format!("hls:{reference}"),
            metadata,
            start,
            end,
            generation,
        )
    }

    impl Weeb3 {
        async fn retrieve_hls_payload(&self, address: String) -> Vec<u8> {
            let progress_id = self
                .start_progress(
                    "hls-segment",
                    address.clone(),
                    "retrieve",
                    None,
                    hls_segment_progress_detail(&address, None),
                )
                .await;
            let reference = match hex::decode(&address) {
                Ok(reference) => reference,
                Err(_) => {
                    self.finish_progress(&progress_id, "failed", "invalid reference", false)
                        .await;
                    return Vec::new();
                }
            };

            let bytes = retrieve_data_payload(&reference, &self.chunk_port.0).await;
            let ok = !bytes.is_empty();
            self.finish_progress(
                &progress_id,
                if ok { "complete" } else { "failed" },
                hls_segment_progress_detail(&address, u64::try_from(bytes.len()).ok()),
                ok,
            )
            .await;
            bytes
        }

        async fn retrieve_hls_payload_cancellable(
            &self,
            address: String,
            stream_key: String,
            stream_generation: u64,
        ) -> Vec<u8> {
            let Some(cancel) = stream_retrieve_cancel_token(stream_key, stream_generation) else {
                return self.retrieve_hls_payload(address).await;
            };
            let progress_id = self
                .start_progress(
                    "hls-segment",
                    address.clone(),
                    "retrieve",
                    None,
                    hls_segment_progress_detail(&address, None),
                )
                .await;
            let reference = match hex::decode(&address) {
                Ok(reference) => reference,
                Err(_) => {
                    self.finish_progress(&progress_id, "failed", "invalid reference", false)
                        .await;
                    return Vec::new();
                }
            };

            let registered = Some(cancel.clone());
            register_retrieve_cancel_token(&self.retrieve_cancel_generations, &registered).await;
            let bytes = retrieve_data_payload_cancellable(
                &reference,
                &self.chunk_port.0,
                self.retrieve_cancel_generations.clone(),
                cancel,
            )
            .await;
            let ok = !bytes.is_empty();
            self.finish_progress(
                &progress_id,
                if ok { "complete" } else { "failed" },
                hls_segment_progress_detail(&address, u64::try_from(bytes.len()).ok()),
                ok,
            )
            .await;
            bytes
        }

        async fn retrieve_hls_payload_range(
            self: &Arc<Self>,
            address: String,
            payload_size: u64,
            start: u64,
            end_inclusive: u64,
            stream_generation: Option<u64>,
            background: Option<HlsBackgroundRangeRequest>,
        ) -> Vec<u8> {
            let progress_id = self
                .start_progress(
                    "hls-segment-range",
                    format!("{} bytes={}-{}", address, start, end_inclusive),
                    "retrieve",
                    None,
                    hls_segment_progress_detail(&address, Some(payload_size)),
                )
                .await;
            let metadata = match hls_range_metadata(&address, payload_size) {
                Some(metadata) => metadata,
                None => {
                    self.finish_progress(&progress_id, "failed", "invalid reference", false)
                        .await;
                    return Vec::new();
                }
            };

            let foreground_range = background.is_none();
            let background_flight = match background {
                Some(HlsBackgroundRangeRequest { flight, admit }) => {
                    // start_progress awaits UI bookkeeping. Recheck the exact
                    // playback stamp/ticket after that yield and immediately
                    // before the shared cache chooses Cached/Wait/Lead.
                    if !admit() {
                        drop(flight);
                        self.finish_progress(
                            &progress_id,
                            "failed",
                            "background range admission retired",
                            false,
                        )
                        .await;
                        return Vec::new();
                    }
                    Some(flight)
                }
                None => None,
            };

            let startup_raw_seed = hls_beginning_raw_seed(
                &address,
                payload_size,
                start,
                end_inclusive,
                stream_generation,
                foreground_range,
            );
            let bytes = read_cached_hls_range(
                self.clone(),
                address.clone(),
                metadata,
                start,
                end_inclusive,
                stream_generation,
                HLS_STREAM_KEY.to_string(),
                background_flight,
                startup_raw_seed,
            )
            .await
            .unwrap_or_default();
            let expected = end_inclusive
                .checked_sub(start)
                .and_then(|length| length.checked_add(1))
                .and_then(|length| usize::try_from(length).ok());
            let ok = expected.is_some_and(|expected| bytes.len() == expected);
            self.finish_progress(
                &progress_id,
                if ok { "complete" } else { "failed" },
                format!(
                    "{} bytes, {}",
                    bytes.len(),
                    hls_segment_progress_detail(&address, Some(payload_size)),
                ),
                ok,
            )
            .await;
            bytes
        }

        async fn latest_hls_feed_payload_startup(
            &self,
            owner: String,
            topic: String,
            early_payloads: Option<mpsc::Sender<RawFeedPayload>>,
            early_payload_max_index: Option<u64>,
        ) -> Option<RawFeedPayload> {
            self.latest_hls_feed_payload_startup_observing_updates(
                owner,
                topic,
                early_payloads,
                early_payload_max_index,
                None,
            )
            .await
        }

        async fn latest_hls_feed_payload_startup_observing_updates(
            &self,
            owner: String,
            topic: String,
            early_payloads: Option<mpsc::Sender<RawFeedPayload>>,
            early_payload_max_index: Option<u64>,
            observed_deferred: Option<mpsc::Sender<DeferredRawFeedPayload>>,
        ) -> Option<RawFeedPayload> {
            let progress_id = self
                .start_progress(
                    "feed-frontier",
                    format!("{} topic {}", owner, topic),
                    "retrieve",
                    None,
                    "seeking bounded startup candidate from the first accounting-ready peer",
                )
                .await;
            let result = acquire_latest_raw_feed_payload_startup_observing_updates(
                owner,
                topic,
                &self.chunk_port.0,
                early_payloads,
                early_payload_max_index,
                observed_deferred,
            )
            .await
            .map(|resolved| resolved.playable);
            match result.as_ref() {
                Some(payload) => {
                    self.finish_progress(
                        &progress_id,
                        "complete",
                        format!("resolved bounded candidate index {}", payload.index),
                        true,
                    )
                    .await;
                }
                None => {
                    self.finish_progress(&progress_id, "failed", "feed update not found", false)
                        .await;
                }
            }
            result
        }

        async fn latest_hls_feed_payload_startup_observing_deferred(
            &self,
            owner: String,
            topic: String,
        ) -> Option<StartupRawFeedPayload> {
            let progress_id = self
                .start_progress(
                    "feed-frontier",
                    format!("{} topic {}", owner, topic),
                    "retrieve",
                    None,
                    "seeking bounded Live startup candidate",
                )
                .await;
            let result = acquire_latest_raw_feed_payload_startup_observing_deferred(
                owner,
                topic,
                &self.chunk_port.0,
                None,
                None,
            )
            .await;
            match result.as_ref() {
                Some(resolved) => {
                    self.finish_progress(
                        &progress_id,
                        "complete",
                        format!(
                            "resolved bounded candidate index {}",
                            resolved.playable.index
                        ),
                        true,
                    )
                    .await;
                }
                None => {
                    self.finish_progress(&progress_id, "failed", "feed update not found", false)
                        .await;
                }
            }
            result
        }

        async fn hls_feed_payload_at_index(
            &self,
            owner: String,
            topic: String,
            index: u64,
        ) -> Option<RawFeedPayload> {
            acquire_raw_feed_payload_at_index(owner, topic, index, &self.chunk_port.0).await
        }

        async fn hls_feed_payload_at_index_bounded(
            &self,
            owner: String,
            topic: String,
            index: u64,
        ) -> Option<RawFeedPayload> {
            acquire_raw_feed_payload_at_index_bounded(owner, topic, index, &self.chunk_port.0).await
        }

        async fn hls_feed_payload_at_index_retained_status(
            &self,
            owner: String,
            topic: String,
            index: u64,
        ) -> RetainedRawFeedPayloadProbe {
            acquire_raw_feed_payload_at_index_retained_status(
                owner,
                topic,
                index,
                HLS_SPARSE_HISTORY_MAX_WINDOW_BYTES,
                &self.chunk_port.0,
            )
            .await
        }

        async fn hls_feed_payload_at_index_followup_retained_status(
            &self,
            owner: String,
            topic: String,
            index: u64,
        ) -> RetainedRawFeedPayloadProbe {
            acquire_raw_feed_payload_at_index_retained_status(
                owner,
                topic,
                index,
                crate::erasure_coding::CHUNK_SIZE,
                &self.chunk_port.0,
            )
            .await
        }

        async fn hls_deferred_feed_payload(
            &self,
            deferred: DeferredRawFeedPayload,
        ) -> Option<RawFeedPayload> {
            acquire_deferred_raw_feed_payload(
                deferred,
                MAX_STREAM_FEED_PAYLOAD_BYTES,
                &self.chunk_port.0,
            )
            .await
        }

        async fn hls_deferred_feed_payload_prefix(
            &self,
            deferred: &DeferredRawFeedPayload,
        ) -> Option<DeferredRawFeedPayloadPrefix> {
            acquire_deferred_raw_feed_payload_prefix_conservative(
                deferred,
                crate::erasure_coding::CHUNK_SIZE,
                &self.chunk_port.0,
            )
            .await
        }
    }

    thread_local! {
        static FEED_ROUTE_CACHE: RefCell<FeedRegistry> = RefCell::new(FeedRegistry::new());
        static HLS_PLAYBACK: RefCell<HlsPlaybackState> = RefCell::new(HlsPlaybackState::new());
        static HLS_CODEC_BOOTSTRAP_PRESENTATION: RefCell<Option<HlsCodecBootstrapPresentation>> =
            const { RefCell::new(None) };
        static HLS_ASSET_CACHE: RefCell<HlsAssetCache> = RefCell::new(HlsAssetCache::new());
        static HLS_PROGRESSIVE_RANGE_BACKGROUND: RefCell<HlsProgressiveRangeBackground> =
            RefCell::new(HlsProgressiveRangeBackground::default());
    }

    const FEED_ROUTE_CACHE_MAX_ENTRIES: usize = 64;
    const FEED_ROUTE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
    const HLS_TERMINAL_CONFIRMATION_MIN_DELAY: Duration = Duration::from_secs(3);
    const HLS_TERMINAL_CONFIRMATION_POLL_INTERVAL: Duration = Duration::from_millis(500);
    const HLS_TERMINAL_CONFIRMATION_MAX_POLLS: usize = 18;
    const HLS_LIVE_FRONTIER_MAX_WAIT: Duration = Duration::from_secs(15);
    const HLS_LIVE_FRONTIER_CONNECTION_WAIT: Duration = Duration::from_secs(7);
    const HLS_LIVE_FRONTIER_MIN_PRICED_PEERS: u64 = 1;
    const HLS_FEED_WAVE_CREDIT_WAIT: Duration = Duration::from_secs(7);
    const HLS_FEED_WAVE_FOREGROUND_MARGIN_CHUNKS: u64 = 64;
    const HLS_FEED_WAVE_RESERVATIONS_PER_PROBE: u64 = 2;
    const HLS_ASSET_METADATA_CACHE_MAX_ENTRIES: usize = 1024;
    const HLS_PROGRESSIVE_REPLAY_MAX_REFERENCES: usize = 8;
    const HLS_PROGRESSIVE_ROUTE_MAX_REFERENCES: usize = 16;
    const HLS_ASSET_PROBE_BYTES: u64 = 512;
    const HLS_REPRESENTATION_VERSION: &str = "weeb3-hls-v2";
    const HLS_MEDIA_PLAN_MAX_REFERENCES: usize = 4096;
    const HLS_ARCHIVE_MEDIA_PLAN_MAX_REFERENCES: usize = 32768;
    const HLS_PREFETCH_TRACK_MAX_ENTRIES: usize = 16;
    const HLS_STREAM_KEY: &str = "weeb3:hls-playback";
    const HLS_PAYLOAD_SINGLEFLIGHT_MAX_WAITERS: usize = 64;
    const HLS_STARTUP_BODY_MAX_PARALLEL: usize = 1;
    const HLS_EXACT_NEXT_OVERLAP_SEGMENTS: usize = 2;
    const HLS_ROLLING_EARLY_OVERLAP_SEGMENTS: usize = 1;
    const HLS_PREFETCH_BODY_MAX_PARALLEL: usize = 3;
    const HLS_SERIAL_PREFETCH_COMPLETIONS: usize = 6;
    const HLS_TWO_BODY_PREFETCH_COMPLETIONS: usize = 10;
    const HLS_PREFETCH_PROBE_MAX_PARALLEL: usize = MEDIA_PREFETCH_MAX_PARALLEL;
    const HLS_PREFETCH_MAX_ATTEMPTS: usize = 6;
    const HLS_FOREGROUND_MAX_ATTEMPTS: usize = HLS_PREFETCH_MAX_ATTEMPTS;
    const HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS: usize = 4;
    const HLS_EARLY_FEED_PREFIX_INDEX: u64 = 7;
    const HLS_SEQUENCE_ZERO_DISCOVERY_HEDGE: Duration = Duration::from_millis(250);
    const HLS_SEQUENCE_ZERO_DISCOVERY_MAX_IN_FLIGHT: usize = 3;
    const HLS_STARTUP_PREFIX_RESULT_GRACE: Duration = Duration::from_secs(1);
    const HLS_SEQUENCE_ZERO_CANONICAL_START_GRACE: Duration = Duration::from_secs(1);
    const HLS_EXACT_NEXT_HEAD_START: Duration = Duration::from_secs(1);
    const HLS_NEXT_RESERVE_STAGGER: Duration = Duration::from_secs(1);
    const HLS_SEQUENCE_ZERO_CANONICAL_EXCLUSIVITY: Duration = Duration::from_secs(10);
    const HLS_SEQUENCE_ZERO_PROVISIONAL_GUARD_WAVES: usize = 4;
    const HLS_SEQUENCE_ZERO_RETRY_BACKLOG_MAX: usize = 32;
    const HLS_EXACT_OVERLAP_ADMISSION_BUDGET: Duration = Duration::from_secs(30);
    const HLS_INITIAL_RESPONSE_BUDGET_MS: f64 = 15_000.0;
    const HLS_PAYLOAD_RETRY_DELAY_MS: u64 = 75;
    const HLS_PAYLOAD_SIZE_RETRY_DELAY_MS: u64 = 250;
    const HLS_STARTUP_LOOKAHEAD_BYTES: u64 = 2 * MEDIA_STARTUP_RESPONSE_BYTES;
    const HLS_LIVE_PREFIX_WINDOW_COUNT: usize = 3;
    const HLS_LIVE_TAIL_FALLBACK_MAX_RETREATS: u8 = 4;
    const HLS_LIVE_TAIL_FALLBACK_MAX_HIDDEN_SECONDS: f64 = 300.0;

    fn hls_segment_progress_detail(reference: &str, size: Option<u64>) -> String {
        let mut detail = size.map_or_else(
            || "starting".to_string(),
            |size| format!("size {:.2} MB", size as f64 / 1_000_000.0),
        );
        if let Some(duration) = HLS_PLAYBACK.with(|playback| {
            playback
                .borrow()
                .durations
                .get(&reference.to_ascii_lowercase())
                .copied()
        }) {
            detail.push_str(&format!(", duration {duration} s"));
        }
        if let Some((width, height)) = super::hls_video_resolution() {
            detail.push_str(&format!(", resolution {width}x{height}"));
        }
        detail
    }

    #[derive(Clone)]
    struct FeedRouteSnapshot {
        index: u64,
        body: Arc<[u8]>,
        finalized: bool,
    }

    #[derive(Clone, Copy)]
    enum HlsCodecBootstrapManifest {
        Bootstrap(u64),
        Continuation(HlsCodecBootstrapPhase),
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum HlsCodecBootstrapPhase {
        Bootstrap,
        ContinuationOpen,
        Settled,
    }

    struct HlsCodecBootstrapPresentation {
        token: u64,
        phase: HlsCodecBootstrapPhase,
        snapshot: Option<FeedRouteSnapshot>,
    }

    #[derive(Clone)]
    struct DeferredFeedRouteSource {
        deferred: DeferredRawFeedPayload,
        payload_span: u64,
        authenticated_prefix: Arc<[u8]>,
    }

    struct DeferredSequenceZeroPresentation {
        index: u64,
        body: Arc<[u8]>,
        source: DeferredFeedRouteSource,
    }

    enum SequenceZeroStartupPrefix {
        Complete(RawFeedPayload),
        Deferred(DeferredSequenceZeroPresentation),
    }

    enum SequenceZeroDiscoveryResult {
        Ready(SequenceZeroStartupPrefix),
        Observed(HlsSequenceZeroDiscoveryObservation),
    }

    struct FeedRouteState {
        snapshot: FeedRouteSnapshot,
        source_body: Arc<[u8]>,
        deferred_source: Option<DeferredFeedRouteSource>,
        body_tracks_source: bool,
        source_endlist_confirmed: bool,
        checking_token: u64,
        confirmed_head_index: Option<u64>,
        sequence_zero_recovery_cursor: u64,
        sequence_zero_retry_indices: VecDeque<HlsSequenceZeroRetry>,
        sequence_zero_deferred_retry_index: Option<u64>,
        sequence_zero_retry_deferred_first: bool,
        sequence_zero_positive_ceiling: u64,
        last_head_check: f64,
        last_touch: f64,
    }

    struct FeedRegistry {
        next_task: u64,
        followers: HashMap<String, FeedRouteState>,
    }

    #[derive(Clone)]
    struct FeedTask {
        cache_key: String,
        token: u64,
        mode: FeedFollowupMode,
    }

    impl FeedTask {
        fn publish(&self, candidate: &RawFeedPayload, head_confirmed: bool) -> bool {
            if candidate.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES
                || !is_hls_manifest(&candidate.bytes)
            {
                return false;
            }
            apply_feed_candidate(
                &self.cache_key,
                FeedCandidate {
                    index: candidate.index,
                    source: Arc::from(candidate.bytes.clone()),
                    terminal: hls_snapshot_is_terminal(
                        hls_is_finalized(&candidate.bytes),
                        false,
                        head_confirmed,
                    ),
                    head_confirmed,
                    mode: self.mode,
                    admission: FeedCandidateAdmission::Task {
                        token: self.token,
                        expected_index: None,
                        require_confirmed_same_index: true,
                    },
                },
            )
            .is_some()
        }
    }

    impl FeedRegistry {
        fn new() -> Self {
            Self {
                next_task: 0,
                followers: HashMap::new(),
            }
        }
    }

    impl Deref for FeedRegistry {
        type Target = HashMap<String, FeedRouteState>;

        fn deref(&self) -> &Self::Target {
            &self.followers
        }
    }

    impl DerefMut for FeedRegistry {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.followers
        }
    }

    enum InitialCanonicalFeedResolution {
        Ready(crate::bzz_stream::RawFeedPayload),
        Pending(mpsc::Receiver<Option<crate::bzz_stream::RawFeedPayload>>),
        Unavailable,
    }

    struct HlsPlaybackState {
        plans: HlsMediaPlanRegistry,
        session: HlsPrefetchSession,
        durations: HashMap<String, f64>,
    }

    impl HlsPlaybackState {
        fn new() -> Self {
            Self {
                plans: HlsMediaPlanRegistry::new(HLS_MEDIA_PLAN_MAX_REFERENCES),
                session: HlsPrefetchSession::new(),
                durations: HashMap::new(),
            }
        }
    }

    #[derive(Clone)]
    struct HlsAssetMetadata {
        payload_size: u64,
        mime: &'static str,
        is_manifest: bool,
    }

    struct ResolvedHlsAsset {
        metadata: HlsAssetMetadata,
        prefetched_body: Option<Arc<[u8]>>,
    }

    #[derive(Clone, Copy, Eq, Hash, PartialEq)]
    pub(crate) struct PlaybackStamp {
        generation: u64,
        timeline_epoch: u64,
    }

    struct HlsBeginningPrefixGate {
        phase: HlsBeginningPrefixPhase,
        stamp: PlaybackStamp,
        reference: Option<String>,
        duration_seconds: Option<f64>,
        payload_size: Option<u64>,
        target_windows: usize,
        foreground_zero_requested: bool,
        foreground_zero_settled: bool,
        coordinator_started: bool,
        deadline_ms: f64,
        raw_scout: Option<StartupRawScout>,
    }

    impl HlsBeginningPrefixGate {
        fn disabled() -> Self {
            Self {
                phase: HlsBeginningPrefixPhase::Disabled,
                stamp: PlaybackStamp {
                    generation: 0,
                    timeline_epoch: 0,
                },
                reference: None,
                duration_seconds: None,
                payload_size: None,
                target_windows: 0,
                foreground_zero_requested: false,
                foreground_zero_settled: false,
                coordinator_started: false,
                deadline_ms: 0.0,
                raw_scout: None,
            }
        }

        fn for_session(stamp: PlaybackStamp, live_start: bool, deadline_ms: f64) -> Self {
            let mut gate = Self::disabled();
            gate.phase = if live_start {
                HlsBeginningPrefixPhase::Disabled
            } else {
                HlsBeginningPrefixPhase::AwaitManifest
            };
            gate.stamp = stamp;
            gate.deadline_ms = deadline_ms;
            gate
        }

        fn bypass_if_expired(&mut self, stamp: PlaybackStamp, now_ms: Option<f64>) {
            if self.stamp == stamp
                && matches!(
                    self.phase,
                    HlsBeginningPrefixPhase::AwaitManifest
                        | HlsBeginningPrefixPhase::AwaitForegroundZero
                        | HlsBeginningPrefixPhase::Supplying
                )
                && now_ms.is_some_and(|now| self.deadline_ms > 0.0 && now >= self.deadline_ms)
            {
                if let Some(scout) = &self.raw_scout {
                    scout.close_new_admissions();
                }
                self.phase = HlsBeginningPrefixPhase::Bypass;
            }
        }

        fn close_raw_scout(&self) {
            if let Some(scout) = &self.raw_scout {
                scout.close_new_admissions();
            }
        }
    }

    impl Drop for HlsBeginningPrefixGate {
        fn drop(&mut self) {
            self.close_raw_scout();
        }
    }

    fn hls_playback_stamp_token(stamp: PlaybackStamp) -> String {
        format!("{:016x}:{:016x}", stamp.generation, stamp.timeline_epoch)
    }

    fn parse_hls_playback_stamp_token(token: &str) -> Option<PlaybackStamp> {
        let (generation, timeline_epoch) = token.split_once(':')?;
        if generation.len() != 16 || timeline_epoch.len() != 16 || timeline_epoch.contains(':') {
            return None;
        }
        let generation = u64::from_str_radix(generation, 16).ok()?;
        let timeline_epoch = u64::from_str_radix(timeline_epoch, 16).ok()?;
        (generation != 0 && timeline_epoch != 0).then_some(PlaybackStamp {
            generation,
            timeline_epoch,
        })
    }

    #[derive(Clone, Copy, Eq, Hash, PartialEq)]
    struct PrefetchTicket {
        stamp: PlaybackStamp,
        plan_id: u64,
        schedule_id: u64,
    }

    struct HlsPrefetchTrack {
        schedule_id: u64,
        last_foreground_position: usize,
        range_retire_position: usize,
        running: Option<PrefetchTicket>,
        last_touch: u64,
    }

    #[derive(Default)]
    struct HlsProgressiveRangeBackground {
        active: usize,
        reserved_bytes: u64,
        owned_ranges: HashMap<HlsProgressiveRangeKey, HashSet<HlsProgressiveRangeOwner>>,
        retired_references: HashMap<String, Option<u64>>,
    }

    #[derive(Clone, Eq, Hash, PartialEq)]
    struct HlsProgressiveRangeKey {
        reference: String,
        payload_size: u64,
        start: u64,
        end: u64,
    }

    #[derive(Clone, Copy, Eq, Hash, PartialEq)]
    struct HlsProgressiveRangeOwner {
        ticket: PrefetchTicket,
        position: usize,
    }

    struct HlsBackgroundRangeLease {
        reserved_bytes: u64,
    }

    struct HlsBackgroundRangeRequest {
        flight: HlsBackgroundRangeFlightGuard,
        admit: Box<dyn FnOnce() -> bool>,
    }

    impl HlsBackgroundRangeRequest {
        fn new(lease: HlsBackgroundRangeLease, admit: impl FnOnce() -> bool + 'static) -> Self {
            Self {
                flight: HlsBackgroundRangeFlightGuard::new(move || drop(lease)),
                admit: Box::new(admit),
            }
        }
    }

    impl Drop for HlsBackgroundRangeLease {
        fn drop(&mut self) {
            HLS_PROGRESSIVE_RANGE_BACKGROUND.with(|background| {
                let mut background = background.borrow_mut();
                background.active = background.active.saturating_sub(1);
                background.reserved_bytes = background
                    .reserved_bytes
                    .saturating_sub(self.reserved_bytes);
            });
        }
    }

    enum HlsProgressiveRangeLeaseAttempt {
        Acquired(HlsBackgroundRangeLease),
        Busy,
        Budget,
        Park,
        Retire,
    }

    struct HlsPrefetchSession {
        generation: u64,
        timeline_epoch: u64,
        schedule_sequence: u64,
        track_touch_sequence: u64,
        mode: HlsPrefetchMode,
        client: Option<Arc<Weeb3>>,
        feed_identity: Option<(String, String)>,
        runways: HlsProgressiveRunways,
        progressive_routes: VecDeque<(String, u64)>,
        progressive_replays: VecDeque<String>,
        sequence_zero_runway_closed: bool,
        presentation_id: u64,
        live_start: bool,
        live_history_active: bool,
        live_tail_fallback: Option<HlsLiveTailFallback>,
        timeline_rebasing: bool,
        startup_deadline_ms: f64,
        beginning_prefix: HlsBeginningPrefixGate,
        startup_runway_kinds: HashMap<HlsMediaKind, u64>,
        fragment_kinds: HashMap<String, Option<HlsMediaKind>>,
        retired_startup_plans: HashSet<u64>,
        completed_media_payloads: usize,
        startup_overlap_plans: HashSet<u64>,
        tracks: HashMap<u64, HlsPrefetchTrack>,
    }

    #[derive(Clone)]
    struct HlsLiveTailFallback {
        snapshot_index: u64,
        sequence: u64,
        reference: String,
        hidden_end_sequence: u64,
        retreats: u8,
    }

    impl HlsPrefetchSession {
        fn new() -> Self {
            Self {
                generation: 0,
                timeline_epoch: 0,
                schedule_sequence: 0,
                track_touch_sequence: 0,
                mode: HlsPrefetchMode::Inactive,
                client: None,
                feed_identity: None,
                runways: HlsProgressiveRunways::default(),
                progressive_routes: VecDeque::new(),
                progressive_replays: VecDeque::new(),
                sequence_zero_runway_closed: false,
                presentation_id: 0,
                live_start: false,
                live_history_active: false,
                live_tail_fallback: None,
                timeline_rebasing: false,
                startup_deadline_ms: 0.0,
                beginning_prefix: HlsBeginningPrefixGate::disabled(),
                startup_runway_kinds: HashMap::new(),
                fragment_kinds: HashMap::new(),
                retired_startup_plans: HashSet::new(),
                completed_media_payloads: 0,
                startup_overlap_plans: HashSet::new(),
                tracks: HashMap::new(),
            }
        }

        fn stamp(&self) -> PlaybackStamp {
            PlaybackStamp {
                generation: self.generation,
                timeline_epoch: self.timeline_epoch,
            }
        }

        fn advance_generation(&mut self) -> u64 {
            self.beginning_prefix.close_raw_scout();
            self.beginning_prefix.phase = HlsBeginningPrefixPhase::Retired;
            self.generation = next_media_generation();
            self.runways.clear();
            self.progressive_routes.clear();
            self.progressive_replays.clear();
            self.startup_overlap_plans.clear();
            self.startup_runway_kinds.clear();
            self.fragment_kinds.clear();
            self.retired_startup_plans.clear();
            self.completed_media_payloads = if self.live_start {
                HLS_SERIAL_PREFETCH_COMPLETIONS.saturating_sub(1)
            } else {
                0
            };
            for track in self.tracks.values_mut() {
                track.running = None;
            }
            self.generation
        }

        fn advance_timeline(&mut self) -> u64 {
            self.beginning_prefix.close_raw_scout();
            self.beginning_prefix.phase = HlsBeginningPrefixPhase::Retired;
            self.timeline_epoch = next_nonzero_generation(self.timeline_epoch);
            self.runways.clear();
            self.progressive_routes.clear();
            self.progressive_replays.clear();
            self.startup_runway_kinds.clear();
            self.fragment_kinds.clear();
            self.retired_startup_plans.clear();
            self.timeline_epoch
        }

        fn ticket_current(&self, ticket: PrefetchTicket, sustained: bool) -> bool {
            (!sustained || self.mode == HlsPrefetchMode::Sustained)
                && self.mode != HlsPrefetchMode::Inactive
                && !self.timeline_rebasing
                && !self.retired_startup_plans.contains(&ticket.plan_id)
                && self.stamp() == ticket.stamp
                && self.tracks.get(&ticket.plan_id).is_some_and(|track| {
                    track.schedule_id == ticket.schedule_id && track.running == Some(ticket)
                })
        }

        fn progressive_current(&self, reference: &str, stamp: PlaybackStamp) -> bool {
            let plan_id = self.progressive_plan(reference);
            self.stamp() == stamp
                && !self.timeline_rebasing
                && !self.sequence_zero_runway_closed
                && plan_id.is_some_and(|plan_id| {
                    self.runways.contains(plan_id, reference)
                        || self
                            .progressive_replays
                            .iter()
                            .any(|replay| replay == reference)
                })
        }

        fn progressive_plan(&self, reference: &str) -> Option<u64> {
            self.progressive_routes
                .iter()
                .rev()
                .find_map(|(current, plan_id)| (current == reference).then_some(*plan_id))
        }

        fn remember_progressive_route(&mut self, reference: &str, plan_id: u64) {
            self.progressive_routes
                .retain(|(current, _)| current != reference);
            self.progressive_routes
                .push_back((reference.to_string(), plan_id));
            while self.progressive_routes.len() > HLS_PROGRESSIVE_ROUTE_MAX_REFERENCES {
                self.progressive_routes.pop_front();
            }
        }

        fn remember_progressive_replay(&mut self, reference: &str) {
            self.progressive_replays
                .retain(|current| current != reference);
            self.progressive_replays.push_back(reference.to_string());
            while self.progressive_replays.len() > HLS_PROGRESSIVE_REPLAY_MAX_REFERENCES {
                self.progressive_replays.pop_front();
            }
        }

        fn remove_progressive_plan(&mut self, plan_id: u64) {
            self.runways.remove(plan_id);
            self.progressive_routes
                .retain(|(_, current_plan)| *current_plan != plan_id);
            self.startup_runway_kinds
                .retain(|_, active_plan| *active_plan != plan_id);
            self.retired_startup_plans.remove(&plan_id);
        }

        fn select_startup_runway_plan(&mut self, kind: HlsMediaKind, plan_id: Option<u64>) -> bool {
            if let Some(plan_id) = plan_id {
                self.retired_startup_plans.remove(&plan_id);
            }
            if let Some(previous) =
                hls_select_startup_runway_plan(&mut self.startup_runway_kinds, kind, plan_id)
            {
                self.retired_startup_plans.insert(previous);
                true
            } else {
                false
            }
        }

        fn body_parallelism(&self, generation: u64) -> usize {
            if self.generation != generation
                || self.completed_media_payloads < HLS_SERIAL_PREFETCH_COMPLETIONS
            {
                1
            } else if self.completed_media_payloads < HLS_TWO_BODY_PREFETCH_COMPLETIONS {
                2
            } else {
                HLS_PREFETCH_BODY_MAX_PARALLEL
            }
        }

        fn prune_tracks(&mut self, selected: u64, superseded: &[u64]) {
            for plan in superseded {
                if *plan != selected {
                    self.tracks.remove(plan);
                    self.startup_overlap_plans.remove(plan);
                    self.remove_progressive_plan(*plan);
                }
            }
            if self.tracks.len() <= HLS_PREFETCH_TRACK_MAX_ENTRIES {
                return;
            }
            let mut inactive = self
                .tracks
                .iter()
                .filter(|(plan, track)| **plan != selected && track.running.is_none())
                .map(|(plan, track)| (*plan, track.last_touch))
                .collect::<Vec<_>>();
            inactive.sort_by_key(|entry| entry.1);
            for (plan, _) in inactive {
                if self.tracks.len() <= HLS_PREFETCH_TRACK_MAX_ENTRIES {
                    break;
                }
                self.tracks.remove(&plan);
                self.startup_overlap_plans.remove(&plan);
                self.remove_progressive_plan(plan);
            }
        }
    }

    #[derive(Clone)]
    struct HlsForegroundContext {
        stamp: PlaybackStamp,
        ticket: Option<PrefetchTicket>,
        cursor: Option<HlsMediaCursor>,
        progressive_plan_id: Option<u64>,
        progressive_owner_handoff: bool,
        progressive_retired_references: Vec<String>,
        seek_successor: Option<String>,
    }

    #[derive(Default)]
    struct HlsAssetCache {
        metadata: HashMap<String, (HlsAssetMetadata, f64)>,
        sizes: HashMap<String, u64>,
        body_order: VecDeque<String>,
        bodies: HashMap<String, Arc<[u8]>>,
        body_pending: HashMap<String, PendingHlsPayload>,
        retired_body_loads: HashSet<(String, u64, u64)>,
        size_pending: HashMap<String, Vec<mpsc::Sender<Option<u64>>>>,
        next_load_id: u64,
        body_bytes: u64,
        retain_completed: bool,
    }

    impl HlsAssetCache {
        fn new() -> Self {
            Self {
                retain_completed: true,
                ..Self::default()
            }
        }

        fn metadata(&mut self, reference: &str) -> Option<HlsAssetMetadata> {
            let (metadata, touched) = self.metadata.get_mut(reference)?;
            *touched = js_sys::Date::now();
            Some(metadata.clone())
        }

        fn remember_metadata(&mut self, reference: &str, metadata: HlsAssetMetadata) {
            if !self.metadata.contains_key(reference)
                && self.metadata.len() >= HLS_ASSET_METADATA_CACHE_MAX_ENTRIES
                && let Some(oldest) = self
                    .metadata
                    .iter()
                    .min_by(|left, right| left.1.1.total_cmp(&right.1.1))
                    .map(|entry| entry.0.clone())
            {
                self.metadata.remove(&oldest);
            }
            self.metadata
                .insert(reference.to_string(), (metadata, js_sys::Date::now()));
        }

        fn payload_size(&self, reference: &str) -> Option<u64> {
            self.bodies
                .get(reference)
                .and_then(|body| u64::try_from(body.len()).ok())
                .or_else(|| self.sizes.get(reference).copied())
                .or_else(|| {
                    self.metadata
                        .get(reference)
                        .map(|(metadata, _)| metadata.payload_size)
                })
        }

        fn remember_size(&mut self, reference: &str, size: u64) {
            if size == 0 {
                return;
            }
            if !self.sizes.contains_key(reference)
                && self.sizes.len() >= HLS_MEDIA_PLAN_MAX_REFERENCES
            {
                self.sizes.clear();
            }
            self.sizes.insert(reference.to_string(), size);
        }

        fn body(&mut self, reference: &str, foreground: bool) -> Option<Arc<[u8]>> {
            let body = self.bodies.get(reference).cloned()?;
            touch_hls_cache_lru(&mut self.body_order, reference, foreground);
            Some(body)
        }

        fn waiter(pending: &mut PendingHlsPayload) -> HlsPayloadLoadRole {
            pending.waiters.retain(|waiter| !waiter.is_closed());
            if pending.waiters.len() >= HLS_PAYLOAD_SINGLEFLIGHT_MAX_WAITERS {
                return HlsPayloadLoadRole::Reject(
                    "HLS fragment already has too many waiting requests".to_string(),
                );
            }
            let (sender, receiver) = mpsc::bounded(1);
            pending.waiters.push(sender);
            HlsPayloadLoadRole::Wait(receiver)
        }

        fn load_role(
            &mut self,
            reference: &str,
            prefetch: bool,
            generation: u64,
            prefetch_limit: usize,
        ) -> HlsPayloadLoadRole {
            if let Some(body) = self.body(reference, !prefetch) {
                return HlsPayloadLoadRole::Cached(body);
            }
            if let Some(pending) = self.body_pending.get_mut(reference) {
                match pending_generation_relation(pending.generation, generation) {
                    PendingGenerationRelation::Join => return Self::waiter(pending),
                    PendingGenerationRelation::RejectStale => {
                        return HlsPayloadLoadRole::Reject(
                            "stale HLS fragment generation".to_string(),
                        );
                    }
                    PendingGenerationRelation::Replace => {}
                }
            }
            if let Some(stale) = self.body_pending.remove(reference) {
                self.retired_body_loads.remove(&(
                    reference.to_string(),
                    stale.generation,
                    stale.load_id,
                ));
                stale.finish(Err(
                    "stale HLS fragment generation was superseded".to_string()
                ));
            }
            if prefetch
                && self
                    .body_pending
                    .values()
                    .filter(|pending| pending.generation == generation)
                    .count()
                    >= prefetch_limit
            {
                return HlsPayloadLoadRole::AtCapacity;
            }

            let (sender, receiver) = mpsc::bounded(1);
            self.next_load_id = next_nonzero_generation(self.next_load_id);
            let load_id = self.next_load_id;
            self.body_pending.insert(
                reference.to_string(),
                PendingHlsPayload {
                    generation,
                    load_id,
                    waiters: vec![sender],
                },
            );
            HlsPayloadLoadRole::Lead(receiver, load_id)
        }

        fn join_pending(&mut self, reference: &str, generation: u64) -> Option<HlsPayloadLoadRole> {
            if let Some(body) = self.body(reference, false) {
                return Some(HlsPayloadLoadRole::Cached(body));
            }
            let pending = self.body_pending.get_mut(reference)?;
            Some(
                match pending_generation_relation(pending.generation, generation) {
                    PendingGenerationRelation::Join => Self::waiter(pending),
                    PendingGenerationRelation::RejectStale => {
                        HlsPayloadLoadRole::Reject("stale HLS fragment generation".to_string())
                    }
                    PendingGenerationRelation::Replace => return None,
                },
            )
        }

        fn finish_load(
            &mut self,
            reference: &str,
            generation: u64,
            load_id: u64,
            result: Result<Arc<[u8]>, String>,
            hot: bool,
        ) -> bool {
            let retired =
                self.retired_body_loads
                    .remove(&(reference.to_string(), generation, load_id));
            let owns_pending = self.body_pending.get(reference).is_some_and(|pending| {
                pending.generation == generation && pending.load_id == load_id
            });
            if owns_pending {
                if !retired && let Ok(body) = &result {
                    self.remember_size(reference, u64::try_from(body.len()).unwrap_or(u64::MAX));
                    if self.retain_completed {
                        self.remember_body(reference.to_string(), body.clone(), hot);
                    }
                }
                if let Some(pending) = self.body_pending.remove(reference) {
                    pending.finish(result);
                }
            }
            owns_pending
        }

        fn remember_body(&mut self, reference: String, body: Arc<[u8]>, hot: bool) {
            let body_len = body.len() as u64;
            let max_bytes = hls_payload_cache_capacity_bytes();
            if body_len > max_bytes {
                return;
            }
            if let Some(previous) = self.bodies.remove(&reference) {
                self.body_bytes = self.body_bytes.saturating_sub(previous.len() as u64);
            }
            touch_hls_cache_lru(&mut self.body_order, &reference, hot);
            self.body_bytes = self.body_bytes.saturating_add(body_len);
            self.bodies.insert(reference, body);

            let max_entries = usize::try_from(
                media_cache_max_bytes()
                    .checked_div(MEDIA_STORAGE_WINDOW_BYTES)
                    .unwrap_or(1)
                    .max(1),
            )
            .unwrap_or(usize::MAX);
            while self.body_bytes > max_bytes || self.bodies.len() > max_entries {
                let Some(oldest) = self.body_order.pop_front() else {
                    break;
                };
                if let Some(previous) = self.bodies.remove(&oldest) {
                    self.body_bytes = self.body_bytes.saturating_sub(previous.len() as u64);
                }
            }
        }

        fn clear_completed_bodies(&mut self) {
            self.retain_completed = false;
            self.body_order.clear();
            self.bodies.clear();
            self.body_bytes = 0;
        }

        fn evict_completed_body(&mut self, reference: &str) {
            if let Some(pending) = self.body_pending.get(reference) {
                self.retired_body_loads.insert((
                    reference.to_string(),
                    pending.generation,
                    pending.load_id,
                ));
            }
            self.body_order.retain(|cached| cached != reference);
            let Some(body) = self.bodies.remove(reference) else {
                return;
            };
            let removed = body.len() as u64;
            self.body_bytes = self.body_bytes.saturating_sub(removed);
        }

        fn retire_pending_bodies(&mut self) {
            for (_, pending) in self.body_pending.drain() {
                pending.finish(Err(
                    "HLS fragment load was retired by a new playback session".to_string(),
                ));
            }
            self.retired_body_loads.clear();
        }
    }

    struct PendingHlsPayload {
        generation: u64,
        load_id: u64,
        waiters: Vec<mpsc::Sender<Result<Arc<[u8]>, String>>>,
    }

    impl PendingHlsPayload {
        fn finish(self, result: Result<Arc<[u8]>, String>) {
            for waiter in self
                .waiters
                .into_iter()
                .filter(|waiter| !waiter.is_closed())
            {
                let _ = waiter.try_send(result.clone());
            }
        }
    }

    enum HlsPayloadLoadRole {
        Cached(Arc<[u8]>),
        Wait(mpsc::Receiver<Result<Arc<[u8]>, String>>),
        Lead(mpsc::Receiver<Result<Arc<[u8]>, String>>, u64),
        AtCapacity,
        Reject(String),
    }

    pub(crate) fn hls_payload_cache_body_bytes() -> u64 {
        HLS_ASSET_CACHE.with(|cache| cache.borrow().body_bytes)
    }

    fn hls_payload_cache_capacity_bytes() -> u64 {
        media_cache_max_bytes().saturating_sub(range_cache_body_bytes())
    }

    fn hls_range_metadata(address: &str, payload_size: u64) -> Option<BzzMetadata> {
        Some(BzzMetadata {
            data_reference: hex::decode(address).ok()?,
            mime: "application/octet-stream".to_string(),
            size: payload_size,
            etag: hls_etag(address),
            path: address.to_string(),
            target_count: 1,
        })
    }

    async fn fetch_hls_bytes_response(
        weeb3: Arc<Weeb3>,
        reference: String,
        method: String,
        mut range: Option<String>,
        if_none_match: Option<String>,
        if_range: Option<String>,
        stream_token: Option<String>,
        local_bytes_base: String,
    ) -> FetchResponse {
        let etag = hls_etag(&reference);
        if if_none_match_matches(if_none_match.as_deref(), &etag) {
            return FetchResponse::ok(304, hls_validator_headers(&reference), None);
        }
        if range.is_some() && !if_range_allows_range(if_range.as_deref(), &etag) {
            range = None;
        }
        let explicit_stream_stamp = match stream_token.as_deref() {
            Some(token) => match parse_hls_playback_stamp_token(token) {
                Some(stamp) => Some(stamp),
                None => return FetchResponse::error(400, "invalid HLS stream token"),
            },
            None => None,
        };
        let progressive_stamp = if let Some(stamp) = explicit_stream_stamp {
            let current = HLS_PLAYBACK.with(|playback| {
                let playback = playback.borrow();
                let session = &playback.session;
                session.stamp() == stamp
                    && !session.timeline_rebasing
                    && !session.sequence_zero_runway_closed
            });
            if !current {
                return FetchResponse::error(503, "stale HLS stream token");
            }
            Some(stamp)
        } else {
            HLS_PLAYBACK.with(|playback| {
                let playback = playback.borrow();
                let session = &playback.session;
                let stamp = session.stamp();
                (stamp.generation != 0 && session.progressive_current(&reference, stamp))
                    .then_some(stamp)
            })
        };
        if method != "HEAD"
            && range.is_some()
            && explicit_stream_stamp.is_none()
            && let Some(stamp) = progressive_stamp
        {
            bypass_hls_beginning_prefix_for_direct_range(&reference, stamp);
        }
        let progressive_start = hls_progressive_media_candidate(&reference);
        let progressive_context = if method != "HEAD" && range.is_none() && progressive_start {
            let (body_cached, completed) = HLS_ASSET_CACHE.with(|cache| {
                let cache = cache.borrow();
                let body_cached = cache.bodies.contains_key(&reference);
                let completed = body_cached
                    || cache.payload_size(&reference).is_some_and(|payload_size| {
                        hls_range_metadata(&reference, payload_size).is_some_and(|metadata| {
                            hls_range_body_fully_cached(&reference, &metadata)
                        })
                    });
                (body_cached, completed)
            });
            Some((hls_foreground_context(&reference, completed), body_cached))
        } else {
            None
        };
        if let Some((context, true)) = progressive_context.as_ref()
            && let (Some(ticket), Some(cursor)) = (context.ticket, context.cursor.clone())
        {
            spawn_hls_progressive_range_prefetch(weeb3.clone(), cursor, ticket);
        }
        if let Some((context, false)) = progressive_context {
            let Some(plan_id) = context.progressive_plan_id else {
                return FetchResponse::error(503, "stale HLS progressive plan");
            };
            let payload_size = match cached_hls_payload_size(&reference) {
                Some(size) => Some(size),
                None => start_hls_payload_size_probe(weeb3.clone(), reference.clone())
                    .recv()
                    .await
                    .ok()
                    .flatten(),
            };
            let Some(payload_size) = payload_size.filter(|size| *size > 0) else {
                return FetchResponse::error(503, "weeb-3 did not resolve HLS media size");
            };
            HLS_ASSET_CACHE.with(|cache| {
                cache.borrow_mut().remember_metadata(
                    &reference,
                    HlsAssetMetadata {
                        payload_size,
                        mime: "application/octet-stream",
                        is_manifest: false,
                    },
                );
            });
            let critical_prefix_windows = ensure_hls_beginning_prefix_configured(
                weeb3.clone(),
                &reference,
                context.stamp,
                payload_size,
            );
            if !hls_progressive_response_is_current(plan_id, &reference, context.stamp) {
                return FetchResponse::error(503, "stale HLS progressive request");
            }
            if let (Some(ticket), Some(cursor)) = (context.ticket, context.cursor) {
                spawn_hls_progressive_range_prefetch(weeb3.clone(), cursor, ticket);
            }
            let mut headers = hls_bytes_headers(&reference, "application/octet-stream");
            headers.push(("Content-Length".to_string(), payload_size.to_string()));
            headers.push(("X-Weeb3-Stream-Start".to_string(), "1".to_string()));
            headers.push((
                "X-Weeb3-Stream-Token".to_string(),
                hls_playback_stamp_token(context.stamp),
            ));
            if let Some(windows) = critical_prefix_windows {
                headers.push((
                    "X-Weeb3-HLS-Critical-Prefix-Windows".to_string(),
                    windows.to_string(),
                ));
            }
            return FetchResponse::stream(200, headers);
        }
        if method != "HEAD" && range.is_none() {
            let Some(bytes) =
                retrieve_hls_payload_for_playback(weeb3.clone(), reference.clone()).await
            else {
                return FetchResponse::error(503, "weeb-3 did not retrieve resource");
            };

            let payload_size = match u64::try_from(bytes.len()) {
                Ok(size) => size,
                Err(_) => return FetchResponse::error(502, "HLS resource is too large"),
            };
            let looks_like_manifest = is_hls_manifest(&bytes);
            if looks_like_manifest && bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES {
                return FetchResponse::error(413, "HLS manifest exceeds the supported size limit");
            }
            let mime = hls_payload_mime(&bytes);
            HLS_ASSET_CACHE.with(|cache| {
                cache.borrow_mut().remember_metadata(
                    &reference,
                    HlsAssetMetadata {
                        payload_size,
                        mime,
                        is_manifest: looks_like_manifest,
                    },
                );
            });
            if looks_like_manifest {
                let Some(rewritten) = rewrite_hls_manifest(&bytes, &local_bytes_base) else {
                    return FetchResponse::error(502, "invalid nested HLS manifest");
                };
                remember_hls_media_plan(&rewritten);
                let mut headers = hls_bytes_headers(&reference, mime);
                headers.push(("Content-Length".to_string(), rewritten.len().to_string()));
                return FetchResponse::ok(200, headers, Some(rewritten));
            }

            let mut headers = hls_bytes_headers(&reference, mime);
            headers.push(("Content-Length".to_string(), bytes.len().to_string()));
            return FetchResponse::ok_shared(200, headers, bytes);
        }

        if method != "HEAD" && progressive_stamp.is_none() {
            let _ = wait_for_pending_hls_payload(&reference).await;
        }
        let streamed_range_asset =
            (explicit_stream_stamp.is_some() && method != "HEAD" && range.is_some())
                .then(|| {
                    HLS_ASSET_CACHE.with(|cache| {
                        cache
                            .borrow_mut()
                            .metadata(&reference)
                            .filter(|metadata| !metadata.is_manifest)
                            .map(|metadata| ResolvedHlsAsset {
                                metadata,
                                prefetched_body: None,
                            })
                    })
                })
                .flatten();
        let resolved = if let Some(resolved) = streamed_range_asset {
            Some(resolved)
        } else {
            resolve_hls_asset(weeb3.clone(), reference.clone()).await
        };
        let Some(resolved) = resolved else {
            return FetchResponse::error(503, "weeb-3 did not retrieve resource");
        };

        let is_manifest = resolved.metadata.is_manifest;
        let (size, mime, body) = if is_manifest {
            if resolved.metadata.payload_size > MAX_STREAM_FEED_PAYLOAD_BYTES as u64 {
                return FetchResponse::error(413, "HLS manifest exceeds the supported size limit");
            }
            let raw: Arc<[u8]> = match resolved.prefetched_body {
                Some(body) => body,
                None => Arc::from(weeb3.retrieve_hls_payload(reference.clone()).await),
            };
            if u64::try_from(raw.len()).ok() != Some(resolved.metadata.payload_size) {
                return FetchResponse::error(502, "weeb-3 returned a short nested HLS manifest");
            }
            let Some(bytes) = rewrite_hls_manifest(raw.as_ref(), &local_bytes_base) else {
                return FetchResponse::error(502, "invalid nested HLS manifest");
            };
            remember_hls_media_plan(&bytes);
            let size = match u64::try_from(bytes.len()) {
                Ok(size) => size,
                Err(_) => return FetchResponse::error(502, "HLS manifest is too large"),
            };
            (
                size,
                "application/vnd.apple.mpegurl",
                Some(Either::Left(bytes)),
            )
        } else {
            (
                resolved.metadata.payload_size,
                resolved.metadata.mime,
                resolved.prefetched_body.map(Either::Right),
            )
        };
        let mut headers = hls_bytes_headers(&reference, mime);
        if method == "HEAD" {
            headers.push(("Content-Length".to_string(), size.to_string()));
            return FetchResponse::ok(200, headers, None);
        }

        let (start, end) = match parse_single_range(range.as_deref(), size) {
            Some(Ok(range)) => range,
            Some(Err(())) | None => {
                headers.push(("Content-Range".to_string(), format!("bytes */{}", size)));
                headers.push(("Content-Length".to_string(), "0".to_string()));
                return FetchResponse::ok(416, headers, None);
            }
        };
        let beginning_foreground_zero_requested = explicit_stream_stamp.is_some_and(|stamp| {
            mark_hls_beginning_foreground_zero_requested(&reference, stamp, size, start, end)
        });
        let fail_beginning_foreground_zero = || {
            if beginning_foreground_zero_requested {
                fail_hls_beginning_foreground_zero(
                    &reference,
                    explicit_stream_stamp.expect("a marked foreground W0 has a playback stamp"),
                    size,
                    start,
                    end,
                );
            }
        };
        let expected_len = end
            .checked_sub(start)
            .and_then(|length| length.checked_add(1))
            .and_then(|length| usize::try_from(length).ok());
        let bytes = if let Some(body) = body {
            let (too_large, outside) = if is_manifest {
                (
                    "HLS manifest range is too large",
                    "HLS manifest range is outside its representation",
                )
            } else {
                (
                    "HLS range is too large",
                    "HLS range is outside its cached representation",
                )
            };
            let start_index = match usize::try_from(start) {
                Ok(start) => start,
                Err(_) => {
                    fail_beginning_foreground_zero();
                    return FetchResponse::error(502, too_large);
                }
            };
            let end_index = match usize::try_from(end.saturating_add(1)) {
                Ok(end) => end,
                Err(_) => {
                    fail_beginning_foreground_zero();
                    return FetchResponse::error(502, too_large);
                }
            };
            let body = match &body {
                Either::Left(body) => body.get(start_index..end_index),
                Either::Right(body) => body.get(start_index..end_index),
            };
            let Some(selected) = body else {
                fail_beginning_foreground_zero();
                return FetchResponse::error(502, outside);
            };
            selected.to_vec()
        } else {
            weeb3
                .retrieve_hls_payload_range(
                    reference.clone(),
                    size,
                    start,
                    end,
                    progressive_stamp.map(|stamp| stamp.generation),
                    None,
                )
                .await
        };
        if !is_manifest && !expected_len.is_some_and(|expected| bytes.len() == expected) {
            fail_beginning_foreground_zero();
            return FetchResponse::error(502, "weeb-3 returned a short HLS byte range");
        }
        if let Some(stamp) = explicit_stream_stamp {
            mark_hls_beginning_foreground_range_settled(&reference, stamp, size, start, end);
        }

        headers.push(("Content-Length".to_string(), bytes.len().to_string()));
        headers.push((
            "Content-Range".to_string(),
            format!("bytes {}-{}/{}", start, end, size),
        ));
        FetchResponse::ok(206, headers, Some(bytes))
    }

    fn hls_bytes_headers(reference: &str, mime: &str) -> Vec<(String, String)> {
        let mut headers = hls_validator_headers(reference);
        headers.extend([
            ("Content-Type".to_string(), mime.to_string()),
            ("Accept-Ranges".to_string(), "bytes".to_string()),
            (
                "Access-Control-Expose-Headers".to_string(),
                "Accept-Ranges, Content-Range, ETag".to_string(),
            ),
        ]);
        headers
    }

    fn hls_validator_headers(reference: &str) -> Vec<(String, String)> {
        vec![
            (
                "Cache-Control".to_string(),
                "private, max-age=31536000, immutable".to_string(),
            ),
            ("ETag".to_string(), hls_etag(reference)),
        ]
    }

    fn hls_etag(reference: &str) -> String {
        format!(
            "\"{}-{}\"",
            HLS_REPRESENTATION_VERSION,
            reference.to_ascii_lowercase()
        )
    }

    fn remember_hls_media_plan(manifest: &[u8]) {
        let Some(mut segments) = hls_segment_identities(manifest) else {
            return;
        };
        segments.retain(|segment| !segment.gap);
        let retired_references = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let plan_limit = if hls_is_finalized(manifest) {
                HLS_ARCHIVE_MEDIA_PLAN_MAX_REFERENCES
            } else {
                HLS_MEDIA_PLAN_MAX_REFERENCES
            };
            playback.plans.resize(plan_limit);
            if playback.session.live_start && segments.len() > plan_limit {
                segments.drain(..segments.len() - plan_limit);
            } else {
                segments.truncate(plan_limit);
            }
            if playback.durations.len().saturating_add(segments.len()) > plan_limit {
                playback.durations.clear();
            }
            for segment in &segments {
                playback.durations.insert(
                    segment.reference.clone(),
                    f64::from_bits(segment.duration_bits),
                );
            }
            let references = segments
                .into_iter()
                .map(|segment| segment.reference)
                .collect();
            let overlap = if hls_media_sequence(manifest).is_some_and(|sequence| sequence > 0) {
                HLS_ROLLING_EARLY_OVERLAP_SEGMENTS
            } else {
                HLS_EXACT_NEXT_OVERLAP_SEGMENTS
            };
            let live_start = playback.session.live_start;
            let mut recent_tracks = playback
                .session
                .tracks
                .iter()
                .map(|(plan_id, track)| (*plan_id, track.last_touch, track.running.is_some()))
                .collect::<Vec<_>>();
            recent_tracks.sort_by_key(|(_, touch, running)| (!*running, std::cmp::Reverse(*touch)));
            let protected_plan_ids = recent_tracks
                .into_iter()
                .take(HLS_MEDIA_PLAN_ACTIVE_TRACKS)
                .map(|(plan_id, _, _)| plan_id)
                .collect();
            let evicted = playback.plans.install_with_early_overlap_limit(
                references,
                overlap,
                live_start,
                &protected_plan_ids,
            );
            let mut retired_references = Vec::new();
            for plan in evicted {
                retired_references.extend(plan.references.iter().cloned());
                playback.session.tracks.remove(&plan.id);
                playback.session.startup_overlap_plans.remove(&plan.id);
                playback.session.remove_progressive_plan(plan.id);
            }
            retired_references
        });
        retire_hls_progressive_range_owners(None, &retired_references);
    }

    fn cached_hls_payload(reference: &str) -> Option<Arc<[u8]>> {
        HLS_ASSET_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .body(&reference.to_ascii_lowercase(), false)
        })
    }

    fn cached_hls_payload_size(reference: &str) -> Option<u64> {
        HLS_ASSET_CACHE.with(|cache| cache.borrow().payload_size(&reference.to_ascii_lowercase()))
    }

    fn start_hls_payload_size_probe(
        weeb3: Arc<Weeb3>,
        reference: String,
    ) -> mpsc::Receiver<Option<u64>> {
        let reference = reference.to_ascii_lowercase();
        let (sender, receiver) = mpsc::bounded(1);
        if let Some(size) = cached_hls_payload_size(&reference) {
            let _ = sender.try_send(Some(size));
            return receiver;
        }
        let start = HLS_ASSET_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some(waiters) = cache.size_pending.get_mut(&reference) {
                waiters.retain(|waiter| !waiter.is_closed());
                if waiters.len() < HLS_PAYLOAD_SINGLEFLIGHT_MAX_WAITERS {
                    waiters.push(sender);
                } else {
                    let _ = sender.try_send(None);
                }
                false
            } else {
                cache.size_pending.insert(reference.clone(), vec![sender]);
                true
            }
        });
        if start {
            spawn_local(async move {
                let size = match hex::decode(&reference) {
                    Ok(address) => retrieve_decoded_data_root(&address, &weeb3.chunk_port.0)
                        .await
                        .map(|root| root.span),
                    Err(_) => None,
                };
                HLS_ASSET_CACHE.with(|cache| {
                    let mut cache = cache.borrow_mut();
                    if let Some(size) = size {
                        cache.remember_size(&reference, size);
                    }
                    for waiter in cache.size_pending.remove(&reference).unwrap_or_default() {
                        let _ = waiter.try_send(size);
                    }
                });
            });
        }
        receiver
    }

    fn publish_hls_stream_generation(client: Arc<Weeb3>, generation: u64) {
        spawn_local(async move {
            if let Some(cancel) =
                stream_retrieve_cancel_token(HLS_STREAM_KEY.to_string(), generation)
            {
                register_retrieve_cancel_token(&client.retrieve_cancel_generations, &Some(cancel))
                    .await;
            }
        });
    }

    fn begin_hls_prefetch_session(
        client: Arc<Weeb3>,
        owner: String,
        topic: String,
        presentation_id: u64,
        live_start: bool,
    ) {
        clear_completed_bzz_media_ranges();
        clear_hls_progressive_range_owners();
        HLS_ASSET_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.retire_pending_bodies();
            cache.clear_completed_bodies();
            cache.retain_completed = true;
        });
        let generation = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            session.client = Some(client.clone());
            session.feed_identity = Some((owner.to_ascii_lowercase(), topic.to_ascii_lowercase()));
            session.sequence_zero_runway_closed = false;
            session.presentation_id = presentation_id;
            session.live_start = live_start;
            session.live_history_active = false;
            session.live_tail_fallback = None;
            let startup_deadline_ms = hls_monotonic_now_ms()
                .map(|now| now + HLS_INITIAL_RESPONSE_BUDGET_MS)
                .unwrap_or(0.0);
            session.startup_deadline_ms = startup_deadline_ms;
            session.startup_overlap_plans.clear();
            session.mode = HlsPrefetchMode::Inactive;
            session.timeline_rebasing = false;
            session.tracks.clear();
            session.advance_timeline();
            let generation = session.advance_generation();
            session.beginning_prefix = HlsBeginningPrefixGate::for_session(
                session.stamp(),
                live_start,
                startup_deadline_ms,
            );
            generation
        });
        publish_hls_stream_generation(client, generation);
    }

    fn remember_authenticated_hls_startup_prefix(
        best_prefix: &Rc<RefCell<Option<crate::bzz_stream::RawFeedPayload>>>,
        candidate: &crate::bzz_stream::RawFeedPayload,
    ) -> bool {
        let Some(timeline) = HlsTimeline::parse(&candidate.bytes) else {
            return false;
        };
        if timeline.sequence != 0 || timeline.segments.len() < HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS
        {
            return false;
        }

        let should_replace = {
            let current = best_prefix.borrow();
            current.as_ref().is_none_or(|current| {
                candidate.index > current.index
                    && hls_manifest_reload_is_continuous(&current.bytes, &candidate.bytes)
            })
        };
        if should_replace {
            *best_prefix.borrow_mut() = Some(candidate.clone());
        }
        should_replace
    }

    fn deferred_sequence_zero_presentation(
        deferred: DeferredRawFeedPayload,
        prefix: DeferredRawFeedPayloadPrefix,
    ) -> Option<DeferredSequenceZeroPresentation> {
        if deferred.index != prefix.index
            || deferred.payload_span() != prefix.payload_span
            || prefix.bytes.len() > crate::erasure_coding::CHUNK_SIZE
        {
            return None;
        }
        let body = hls_sequence_zero_body_from_deferred_prefix(
            &prefix.bytes,
            prefix.payload_span,
            HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
        )?;
        Some(DeferredSequenceZeroPresentation {
            index: prefix.index,
            body: Arc::from(body),
            source: DeferredFeedRouteSource {
                deferred,
                payload_span: prefix.payload_span,
                authenticated_prefix: Arc::from(prefix.bytes),
            },
        })
    }

    async fn sequence_zero_discovery_is_current(
        weeb3: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        network_id: u64,
        generation: u64,
    ) -> bool {
        weeb3.get_network_id().await == network_id
            && active_profile().swarm_network_id == network_id
            && hls_prefix_generation_for_feed(weeb3, owner, topic) == Some(generation)
    }

    fn sequence_zero_startup_admissions_are_open(
        admissions_open: &Rc<Cell<bool>>,
        ready: &mpsc::Sender<SequenceZeroStartupPrefix>,
    ) -> bool {
        admissions_open.get() && !ready.is_closed()
    }

    fn offer_sequence_zero_startup_prefix(
        admissions_open: &Rc<Cell<bool>>,
        ready: &mpsc::Sender<SequenceZeroStartupPrefix>,
        prefix: SequenceZeroStartupPrefix,
    ) -> bool {
        if !sequence_zero_startup_admissions_are_open(admissions_open, ready) {
            return false;
        }
        if ready.try_send(prefix).is_err() {
            return false;
        }
        admissions_open.set(false);
        true
    }

    async fn probe_sequence_zero_startup_prefix(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        network_id: u64,
        generation: u64,
        index: u64,
        admissions_open: Rc<Cell<bool>>,
        ready: mpsc::Sender<SequenceZeroStartupPrefix>,
    ) -> SequenceZeroDiscoveryResult {
        match weeb3
            .hls_feed_payload_at_index_followup_retained_status(owner.clone(), topic.clone(), index)
            .await
        {
            RetainedRawFeedPayloadProbe::Found(payload) => {
                if payload.index != index {
                    return SequenceZeroDiscoveryResult::Observed(
                        HlsSequenceZeroDiscoveryObservation::Unusable,
                    );
                }
                let observation = classify_hls_sequence_zero_discovery(
                    &payload.bytes,
                    HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
                );
                if observation == HlsSequenceZeroDiscoveryObservation::Ready {
                    SequenceZeroDiscoveryResult::Ready(SequenceZeroStartupPrefix::Complete(payload))
                } else {
                    SequenceZeroDiscoveryResult::Observed(observation)
                }
            }
            RetainedRawFeedPayloadProbe::Deferred(deferred) => {
                if deferred.index != index
                    || !sequence_zero_startup_admissions_are_open(&admissions_open, &ready)
                    || !sequence_zero_discovery_is_current(
                        &weeb3, &owner, &topic, network_id, generation,
                    )
                    .await
                {
                    return SequenceZeroDiscoveryResult::Observed(
                        HlsSequenceZeroDiscoveryObservation::Unusable,
                    );
                }
                // The retained exact probe above is always allowed to settle. The additional
                // authenticated byte-zero range is new work, so recheck the shared winner gate
                // after the asynchronous currentness check and immediately before dispatch.
                if !sequence_zero_startup_admissions_are_open(&admissions_open, &ready) {
                    return SequenceZeroDiscoveryResult::Observed(
                        HlsSequenceZeroDiscoveryObservation::Unusable,
                    );
                }
                let Some(prefix) = weeb3.hls_deferred_feed_payload_prefix(&deferred).await else {
                    return SequenceZeroDiscoveryResult::Observed(
                        HlsSequenceZeroDiscoveryObservation::Unusable,
                    );
                };
                let observation = classify_hls_sequence_zero_discovery(
                    &prefix.bytes,
                    HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
                );
                if observation != HlsSequenceZeroDiscoveryObservation::Ready {
                    return SequenceZeroDiscoveryResult::Observed(observation);
                }
                match deferred_sequence_zero_presentation(deferred, prefix) {
                    Some(presentation) => SequenceZeroDiscoveryResult::Ready(
                        SequenceZeroStartupPrefix::Deferred(presentation),
                    ),
                    None => SequenceZeroDiscoveryResult::Observed(
                        HlsSequenceZeroDiscoveryObservation::Unusable,
                    ),
                }
            }
            RetainedRawFeedPayloadProbe::Missing | RetainedRawFeedPayloadProbe::Transient => {
                SequenceZeroDiscoveryResult::Observed(HlsSequenceZeroDiscoveryObservation::Unusable)
            }
        }
    }

    fn start_sequence_zero_discovery_probe(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        network_id: u64,
        generation: u64,
        index: u64,
        completed: mpsc::Sender<(u64, SequenceZeroDiscoveryResult)>,
        admissions_open: Rc<Cell<bool>>,
        ready: mpsc::Sender<SequenceZeroStartupPrefix>,
    ) {
        // The spawned task owns the retained-status receiver through its logical terminal result.
        // Returning a different winner therefore cannot cancel dispatched Bee accounting work.
        spawn_local(async move {
            let result = probe_sequence_zero_startup_prefix(
                weeb3,
                owner,
                topic,
                network_id,
                generation,
                index,
                admissions_open,
                ready,
            )
            .await;
            let _ = completed.try_send((index, result));
        });
    }

    async fn admit_sequence_zero_discovery_probe(
        planner: &mut HlsSequenceZeroDiscoveryPlanner,
        pending: &mut HashSet<u64>,
        weeb3: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        network_id: u64,
        generation: u64,
        completed: &mpsc::Sender<(u64, SequenceZeroDiscoveryResult)>,
        admissions_open: &Rc<Cell<bool>>,
        ready: &mpsc::Sender<SequenceZeroStartupPrefix>,
    ) -> bool {
        if !sequence_zero_startup_admissions_are_open(admissions_open, ready)
            || !sequence_zero_discovery_is_current(weeb3, owner, topic, network_id, generation)
                .await
            || !sequence_zero_startup_admissions_are_open(admissions_open, ready)
        {
            return false;
        }
        let Some(index) = planner.next_probe() else {
            return false;
        };
        pending.insert(index);
        start_sequence_zero_discovery_probe(
            weeb3.clone(),
            owner.to_string(),
            topic.to_string(),
            network_id,
            generation,
            index,
            completed.clone(),
            admissions_open.clone(),
            ready.clone(),
        );
        true
    }

    async fn discover_sequence_zero_startup_prefix(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        network_id: u64,
        generation: u64,
        admissions_open: Rc<Cell<bool>>,
        ready: mpsc::Sender<SequenceZeroStartupPrefix>,
    ) {
        while weeb3.get_connections().await == 0 {
            if !sequence_zero_startup_admissions_are_open(&admissions_open, &ready)
                || !sequence_zero_discovery_is_current(
                    &weeb3, &owner, &topic, network_id, generation,
                )
                .await
            {
                return;
            }
            async_std::task::sleep(Duration::from_millis(15)).await;
        }
        // `get_connections()` yields; a replacement presentation can become active between its
        // final zero result and the first exact dispatch.
        if !sequence_zero_startup_admissions_are_open(&admissions_open, &ready)
            || !sequence_zero_discovery_is_current(&weeb3, &owner, &topic, network_id, generation)
                .await
        {
            return;
        }

        let (completed_out, completed_in) = mpsc::bounded::<(u64, SequenceZeroDiscoveryResult)>(
            HLS_SEQUENCE_ZERO_DISCOVERY_MAX_PROBES,
        );
        let mut planner = HlsSequenceZeroDiscoveryPlanner::new();
        let mut pending = HashSet::new();
        for _ in 0..HLS_SEQUENCE_ZERO_DISCOVERY_MAX_IN_FLIGHT {
            if !admit_sequence_zero_discovery_probe(
                &mut planner,
                &mut pending,
                &weeb3,
                &owner,
                &topic,
                network_id,
                generation,
                &completed_out,
                &admissions_open,
                &ready,
            )
            .await
            {
                break;
            }
        }

        loop {
            if !sequence_zero_startup_admissions_are_open(&admissions_open, &ready) {
                return;
            }
            if pending.is_empty() {
                if !admit_sequence_zero_discovery_probe(
                    &mut planner,
                    &mut pending,
                    &weeb3,
                    &owner,
                    &topic,
                    network_id,
                    generation,
                    &completed_out,
                    &admissions_open,
                    &ready,
                )
                .await
                {
                    return;
                }
            }

            let completed = Box::pin(completed_in.recv());
            let hedge = Box::pin(async_std::task::sleep(HLS_SEQUENCE_ZERO_DISCOVERY_HEDGE));
            match select(completed, hedge).await {
                Either::Left((Ok((index, result)), _)) => {
                    pending.remove(&index);
                    match result {
                        SequenceZeroDiscoveryResult::Ready(prefix) => {
                            if sequence_zero_discovery_is_current(
                                &weeb3, &owner, &topic, network_id, generation,
                            )
                            .await
                            {
                                let _ = offer_sequence_zero_startup_prefix(
                                    &admissions_open,
                                    &ready,
                                    prefix,
                                );
                            }
                            return;
                        }
                        SequenceZeroDiscoveryResult::Observed(observation) => {
                            planner.observe(index, observation);
                        }
                    }
                    while pending.len() < HLS_SEQUENCE_ZERO_DISCOVERY_MAX_IN_FLIGHT {
                        if !admit_sequence_zero_discovery_probe(
                            &mut planner,
                            &mut pending,
                            &weeb3,
                            &owner,
                            &topic,
                            network_id,
                            generation,
                            &completed_out,
                            &admissions_open,
                            &ready,
                        )
                        .await
                        {
                            break;
                        }
                    }
                }
                Either::Left((Err(_), _)) => return,
                Either::Right(((), _)) => {
                    if pending.len() < HLS_SEQUENCE_ZERO_DISCOVERY_MAX_IN_FLIGHT {
                        let _ = admit_sequence_zero_discovery_probe(
                            &mut planner,
                            &mut pending,
                            &weeb3,
                            &owner,
                            &topic,
                            network_id,
                            generation,
                            &completed_out,
                            &admissions_open,
                            &ready,
                        )
                        .await;
                    }
                }
            }
        }
    }

    fn sequence_zero_startup_prefix_is_preferred(
        canonical: &RawFeedPayload,
        prefix: &SequenceZeroStartupPrefix,
    ) -> bool {
        match prefix {
            SequenceZeroStartupPrefix::Complete(prefix) => {
                prefix.index < canonical.index
                    && hls_startup_prefix_is_preferred(
                        &canonical.bytes,
                        &prefix.bytes,
                        HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
                    )
            }
            SequenceZeroStartupPrefix::Deferred(prefix) => {
                hls_media_sequence(&canonical.bytes).is_some_and(|sequence| sequence > 0)
                    && hls_media_sequence(&prefix.body) == Some(0)
                    && hls_media_references(&prefix.body).len()
                        >= HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS
            }
        }
    }

    async fn forward_deferred_sequence_zero_prefix(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        network_id: u64,
        generation: u64,
        deferred: mpsc::Receiver<DeferredRawFeedPayload>,
        admissions_open: Rc<Cell<bool>>,
        ready: mpsc::Sender<SequenceZeroStartupPrefix>,
    ) {
        while let Ok(deferred) = deferred.recv().await {
            if !sequence_zero_startup_admissions_are_open(&admissions_open, &ready)
                || !sequence_zero_discovery_is_current(
                    &weeb3, &owner, &topic, network_id, generation,
                )
                .await
                || !sequence_zero_startup_admissions_are_open(&admissions_open, &ready)
            {
                return;
            }
            let Some(prefix) = weeb3.hls_deferred_feed_payload_prefix(&deferred).await else {
                continue;
            };
            if !sequence_zero_startup_admissions_are_open(&admissions_open, &ready)
                || !sequence_zero_discovery_is_current(
                    &weeb3, &owner, &topic, network_id, generation,
                )
                .await
            {
                return;
            }
            let Some(presentation) = deferred_sequence_zero_presentation(deferred, prefix) else {
                continue;
            };
            if offer_sequence_zero_startup_prefix(
                &admissions_open,
                &ready,
                SequenceZeroStartupPrefix::Deferred(presentation),
            ) {
                return;
            }
        }
    }

    async fn fan_out_authenticated_hls_prefixes(
        weeb3: Arc<Weeb3>,
        network_id: u64,
        early_payloads: mpsc::Receiver<crate::bzz_stream::RawFeedPayload>,
        best_prefix: Rc<RefCell<Option<crate::bzz_stream::RawFeedPayload>>>,
        prefix_ready: mpsc::Sender<SequenceZeroStartupPrefix>,
        admissions_open: Rc<Cell<bool>>,
        startup_cache_key: Option<String>,
    ) {
        while let Ok(payload) = early_payloads.recv().await {
            if weeb3.get_network_id().await != network_id
                || active_profile().swarm_network_id != network_id
            {
                return;
            }
            let accepted_prefix = remember_authenticated_hls_startup_prefix(&best_prefix, &payload);
            let may_extend_visible_prefix = startup_cache_key.as_ref().is_some_and(|cache_key| {
                FEED_ROUTE_CACHE.with(|cache| cache.borrow().contains_key(cache_key))
            });
            if may_extend_visible_prefix
                && payload.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                && is_hls_manifest(&payload.bytes)
                && let Some(cache_key) = startup_cache_key.as_deref()
            {
                let _ = store_feed_snapshot(
                    cache_key,
                    FeedRouteSnapshot {
                        index: payload.index,
                        body: Arc::from(payload.bytes.clone()),
                        finalized: false,
                    },
                    true,
                    FeedFollowupMode::SequenceZeroPresentation,
                );
            }
            if accepted_prefix {
                let _ = offer_sequence_zero_startup_prefix(
                    &admissions_open,
                    &prefix_ready,
                    SequenceZeroStartupPrefix::Complete(payload.clone()),
                );
            }
        }
    }

    fn hls_prefix_stamp_for_feed(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
    ) -> Option<PlaybackStamp> {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            (session.generation != 0
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
                && session.feed_identity.as_ref() == Some(&identity))
            .then_some(session.stamp())
        })
    }

    fn hls_prefix_generation_for_feed(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
    ) -> Option<u64> {
        hls_prefix_stamp_for_feed(client, owner, topic).map(|stamp| stamp.generation)
    }

    fn hls_prefix_stamp_is_current(stamp: PlaybackStamp) -> bool {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.stamp() == stamp
                && !session.timeline_rebasing
                && !session.sequence_zero_runway_closed
        })
    }

    fn configure_hls_beginning_prefix_manifest(
        stamp: PlaybackStamp,
        reference: &str,
        duration_seconds: f64,
    ) -> bool {
        if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
            return false;
        }
        let reference = reference.to_ascii_lowercase();
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let session_stamp = session.stamp();
            let live_start = session.live_start;
            let gate = &mut session.beginning_prefix;
            gate.bypass_if_expired(stamp, hls_monotonic_now_ms());
            if session_stamp != stamp
                || live_start
                || gate.stamp != stamp
                || !matches!(
                    gate.phase,
                    HlsBeginningPrefixPhase::AwaitManifest
                        | HlsBeginningPrefixPhase::AwaitForegroundZero
                )
                || gate
                    .reference
                    .as_ref()
                    .is_some_and(|current| current != &reference)
            {
                return false;
            }
            gate.reference = Some(reference);
            gate.duration_seconds = Some(duration_seconds);
            gate.phase = HlsBeginningPrefixPhase::AwaitForegroundZero;
            true
        })
    }

    pub(super) fn bypass_hls_beginning_prefix(stamp: PlaybackStamp) {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let session_stamp = session.stamp();
            let gate = &mut session.beginning_prefix;
            if session_stamp == stamp
                && gate.stamp == stamp
                && matches!(
                    gate.phase,
                    HlsBeginningPrefixPhase::AwaitManifest
                        | HlsBeginningPrefixPhase::AwaitForegroundZero
                        | HlsBeginningPrefixPhase::Supplying
                )
            {
                gate.close_raw_scout();
                gate.phase = HlsBeginningPrefixPhase::Bypass;
            }
        });
    }

    fn bypass_hls_beginning_prefix_for_direct_range(reference: &str, stamp: PlaybackStamp) {
        let matches_managed_reference = HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.stamp() == stamp
                && session.beginning_prefix.stamp == stamp
                && session.beginning_prefix.reference.as_deref() == Some(reference)
        });
        if matches_managed_reference {
            bypass_hls_beginning_prefix(stamp);
        }
    }

    fn ensure_hls_beginning_prefix_configured(
        weeb3: Arc<Weeb3>,
        reference: &str,
        stamp: PlaybackStamp,
        payload_size: u64,
    ) -> Option<usize> {
        let reference = reference.to_ascii_lowercase();
        let configured = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let session_stamp = session.stamp();
            let live_start = session.live_start;
            let gate = &mut session.beginning_prefix;
            gate.bypass_if_expired(stamp, hls_monotonic_now_ms());
            if session_stamp != stamp
                || live_start
                || gate.stamp != stamp
                || gate.reference.as_deref() != Some(reference.as_str())
                || !matches!(
                    gate.phase,
                    HlsBeginningPrefixPhase::AwaitForegroundZero
                        | HlsBeginningPrefixPhase::Supplying
                        | HlsBeginningPrefixPhase::Ready
                )
            {
                return None;
            }
            let duration_seconds = gate.duration_seconds?;
            let target_windows = hls_beginning_prefix_window_count(
                payload_size,
                duration_seconds,
                MEDIA_STORAGE_WINDOW_BYTES,
            );
            if target_windows == 0 {
                gate.close_raw_scout();
                gate.phase = HlsBeginningPrefixPhase::Bypass;
                return None;
            }
            if gate.payload_size.is_some_and(|size| size != payload_size)
                || (gate.target_windows > 0 && gate.target_windows != target_windows)
            {
                gate.close_raw_scout();
                gate.phase = HlsBeginningPrefixPhase::Bypass;
                return None;
            }
            gate.payload_size = Some(payload_size);
            gate.target_windows = target_windows;
            let start_raw_scout =
                if target_windows > 1 && !gate.coordinator_started && gate.raw_scout.is_none() {
                    gate.coordinator_started = true;
                    let scout = StartupRawScout::new();
                    gate.raw_scout = Some(scout.clone());
                    Some(scout)
                } else {
                    None
                };
            Some((target_windows, start_raw_scout))
        })?;
        if let Some(scout) = configured.1 {
            start_hls_beginning_raw_scout(
                weeb3,
                reference.clone(),
                payload_size,
                stamp,
                configured.0,
                scout,
            );
        }
        Some(configured.0)
    }

    fn hls_beginning_raw_supply_admission_for(
        reference: &str,
        payload_size: u64,
        target_windows: usize,
        stamp: PlaybackStamp,
    ) -> HlsProgressiveRangeAdmission {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let session_stamp = session.stamp();
            let live_start = session.live_start;
            let timeline_rebasing = session.timeline_rebasing;
            let sequence_zero_runway_closed = session.sequence_zero_runway_closed;
            let gate = &mut session.beginning_prefix;
            gate.bypass_if_expired(stamp, hls_monotonic_now_ms());
            if session_stamp != stamp
                || live_start
                || timeline_rebasing
                || sequence_zero_runway_closed
                || gate.stamp != stamp
                || gate.reference.as_deref() != Some(reference)
                || gate.payload_size != Some(payload_size)
                || target_windows <= 1
                || gate.target_windows != target_windows
            {
                return HlsProgressiveRangeAdmission::Retire;
            }
            hls_beginning_raw_supply_admission(gate.phase, gate.foreground_zero_requested)
        })
    }

    fn start_hls_beginning_raw_scout(
        weeb3: Arc<Weeb3>,
        reference: String,
        payload_size: u64,
        stamp: PlaybackStamp,
        target_windows: usize,
        scout: StartupRawScout,
    ) {
        spawn_local(async move {
            if target_windows <= 1 || !scout.new_admissions_open() {
                scout.close_new_admissions();
                return;
            }
            loop {
                match hls_beginning_raw_supply_admission_for(
                    &reference,
                    payload_size,
                    target_windows,
                    stamp,
                ) {
                    HlsProgressiveRangeAdmission::Retire => {
                        scout.close_new_admissions();
                        return;
                    }
                    HlsProgressiveRangeAdmission::Park => {
                        async_std::task::sleep(Duration::from_millis(50)).await;
                    }
                    HlsProgressiveRangeAdmission::Admit => break,
                }
            }
            let payload_ranges = (1..target_windows)
                .filter_map(|window| hls_beginning_prefix_window_bounds(payload_size, window))
                .collect::<Vec<_>>();
            if payload_ranges.len() != target_windows.saturating_sub(1) {
                scout.close_new_admissions();
                return;
            }
            let Some(data_reference) = hex::decode(&reference).ok() else {
                scout.close_new_admissions();
                return;
            };
            let Some(root) = cached_decoded_data_root(&data_reference) else {
                // A scout never spends uncredited transport on the root. The
                // preceding size probe normally left it decoded; eviction simply
                // disables this best-effort optimization for the session.
                scout.close_new_admissions();
                return;
            };
            if root.span != payload_size {
                scout.close_new_admissions();
                return;
            }
            let cancel = stream_retrieve_cancel_token(HLS_STREAM_KEY.to_string(), stamp.generation);
            if cancel.is_none() {
                scout.close_new_admissions();
                return;
            }
            register_retrieve_cancel_token(&weeb3.retrieve_cancel_generations, &cancel).await;
            if !scout.new_admissions_open()
                || hls_beginning_raw_supply_admission_for(
                    &reference,
                    payload_size,
                    target_windows,
                    stamp,
                ) != HlsProgressiveRangeAdmission::Admit
            {
                scout.close_new_admissions();
                return;
            }
            let encrypted = data_reference.len() == crate::erasure_coding::ENCRYPTED_REFERENCE_SIZE;
            scout_data_ranges_cache_only_cancellable(
                root,
                payload_ranges,
                encrypted,
                &weeb3.chunk_port.0,
                Some(weeb3.retrieve_cancel_generations.clone()),
                cancel,
                scout.clone(),
            )
            .await;
        });
    }

    fn hls_beginning_raw_seed(
        reference: &str,
        payload_size: u64,
        start: u64,
        end: u64,
        generation: Option<u64>,
        foreground: bool,
    ) -> Option<RawFetchLifecycleFactory> {
        if !foreground {
            return None;
        }
        let generation = generation?;
        let reference = reference.to_ascii_lowercase();
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            let gate = &session.beginning_prefix;
            if session.generation != generation
                || gate.stamp != session.stamp()
                || gate.reference.as_deref() != Some(reference.as_str())
                || gate.payload_size != Some(payload_size)
                || gate.target_windows <= 1
                || !gate.foreground_zero_requested
                || gate.phase != HlsBeginningPrefixPhase::Supplying
            {
                return None;
            }
            let expected_end = MEDIA_STORAGE_WINDOW_BYTES
                .saturating_sub(1)
                .min(payload_size.saturating_sub(1));
            (start == 0 && end == expected_end)
                .then(|| {
                    gate.raw_scout
                        .as_ref()
                        .map(StartupRawScout::seed_lifecycle_factory)
                })
                .flatten()
        })
    }

    fn hls_beginning_foreground_zero_range(payload_size: u64, start: u64, end: u64) -> bool {
        let expected_end = MEDIA_STORAGE_WINDOW_BYTES
            .saturating_sub(1)
            .min(payload_size.saturating_sub(1));
        payload_size > 0 && start == 0 && end == expected_end
    }

    fn mark_hls_beginning_foreground_zero_requested(
        reference: &str,
        stamp: PlaybackStamp,
        payload_size: u64,
        start: u64,
        end: u64,
    ) -> bool {
        if !hls_beginning_foreground_zero_range(payload_size, start, end) {
            return false;
        }
        let reference = reference.to_ascii_lowercase();
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let session_stamp = session.stamp();
            let gate = &mut session.beginning_prefix;
            gate.bypass_if_expired(stamp, hls_monotonic_now_ms());
            if session_stamp != stamp
                || gate.stamp != stamp
                || gate.reference.as_deref() != Some(reference.as_str())
                || gate.payload_size != Some(payload_size)
                || gate.target_windows == 0
                || !matches!(
                    gate.phase,
                    HlsBeginningPrefixPhase::AwaitForegroundZero
                        | HlsBeginningPrefixPhase::Supplying
                )
            {
                return false;
            }
            gate.foreground_zero_requested = true;
            gate.phase = HlsBeginningPrefixPhase::Supplying;
            true
        })
    }

    fn fail_hls_beginning_foreground_zero(
        reference: &str,
        stamp: PlaybackStamp,
        payload_size: u64,
        start: u64,
        end: u64,
    ) {
        if !hls_beginning_foreground_zero_range(payload_size, start, end) {
            return;
        }
        let reference = reference.to_ascii_lowercase();
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let session_stamp = session.stamp();
            let gate = &mut session.beginning_prefix;
            if session_stamp != stamp
                || gate.stamp != stamp
                || gate.reference.as_deref() != Some(reference.as_str())
                || gate.payload_size != Some(payload_size)
                || !gate.foreground_zero_requested
                || gate.phase != HlsBeginningPrefixPhase::Supplying
            {
                return;
            }
            gate.close_raw_scout();
            gate.phase = HlsBeginningPrefixPhase::Bypass;
        });
    }

    fn mark_hls_beginning_foreground_range_settled(
        reference: &str,
        stamp: PlaybackStamp,
        payload_size: u64,
        start: u64,
        end: u64,
    ) {
        let Some(metadata) = hls_range_metadata(reference, payload_size) else {
            return;
        };
        let reference = reference.to_ascii_lowercase();
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let session_stamp = session.stamp();
            let gate = &mut session.beginning_prefix;
            gate.bypass_if_expired(stamp, hls_monotonic_now_ms());
            if session_stamp != stamp
                || gate.stamp != stamp
                || gate.reference.as_deref() != Some(reference.as_str())
                || gate.payload_size != Some(payload_size)
                || gate.target_windows == 0
                || gate.phase != HlsBeginningPrefixPhase::Supplying
            {
                return;
            }
            if gate.foreground_zero_requested
                && hls_beginning_foreground_zero_range(payload_size, start, end)
            {
                gate.foreground_zero_settled = true;
            }
            if !gate.foreground_zero_requested || !gate.foreground_zero_settled {
                return;
            }
            let ready = (0..gate.target_windows).all(|window| {
                hls_beginning_prefix_window_bounds(payload_size, window).is_some_and(
                    |(window_start, window_end)| {
                        hls_aligned_range_state(
                            &reference,
                            &metadata,
                            window_start,
                            window_end,
                            stamp.generation,
                        ) == HlsAlignedRangeState::Cached
                    },
                )
            });
            if ready {
                gate.close_raw_scout();
                gate.phase = HlsBeginningPrefixPhase::Ready;
            }
        });
    }

    async fn await_hls_beginning_prefix_barrier(
        weeb3: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        expected_stamp: Option<PlaybackStamp>,
    ) -> HlsProgressiveRangeAdmission {
        let Some(expected_stamp) = expected_stamp else {
            return HlsProgressiveRangeAdmission::Admit;
        };
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        loop {
            let admission = HLS_PLAYBACK.with(|playback| {
                let mut playback = playback.borrow_mut();
                let session = &mut playback.session;
                if session.stamp() != expected_stamp
                    || !session
                        .client
                        .as_ref()
                        .is_some_and(|active| Arc::ptr_eq(active, weeb3))
                    || session.feed_identity.as_ref() != Some(&identity)
                {
                    return HlsProgressiveRangeAdmission::Retire;
                }
                session
                    .beginning_prefix
                    .bypass_if_expired(expected_stamp, hls_monotonic_now_ms());
                if session.beginning_prefix.stamp != expected_stamp {
                    HlsProgressiveRangeAdmission::Retire
                } else {
                    hls_beginning_prefix_admission(session.beginning_prefix.phase)
                }
            });
            match admission {
                HlsProgressiveRangeAdmission::Park => {
                    async_std::task::sleep(Duration::from_millis(50)).await;
                }
                HlsProgressiveRangeAdmission::Admit | HlsProgressiveRangeAdmission::Retire => {
                    return admission;
                }
            }
        }
    }

    pub(super) fn remember_hls_fragment_kind(reference: &str, kind: Option<HlsMediaKind>) {
        let reference = reference.to_ascii_lowercase();
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let kinds = &mut playback.session.fragment_kinds;
            if !kinds.contains_key(&reference)
                && kinds.len() >= HLS_PROGRESSIVE_ROUTE_MAX_REFERENCES
            {
                kinds.clear();
            }
            kinds.insert(reference, kind);
        });
    }

    fn hls_progressive_media_candidate(reference: &str) -> bool {
        let reference = reference.to_ascii_lowercase();
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.generation != 0
                && session.mode != HlsPrefetchMode::Inactive
                && !session.timeline_rebasing
                && !session.sequence_zero_runway_closed
                && playback.plans.cursors.contains_key(&reference)
        })
    }

    fn hls_progressive_response_is_current(
        plan_id: u64,
        reference: &str,
        stamp: PlaybackStamp,
    ) -> bool {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            let current = session.runways.current(plan_id, reference);
            let replay = session
                .progressive_replays
                .iter()
                .any(|replay| replay == reference);
            session.progressive_current(reference, stamp)
                && session.progressive_plan(reference) == Some(plan_id)
                && (current || replay)
        })
    }

    fn hls_progressive_startup_admission_is_current(reference: &str, stamp: PlaybackStamp) -> bool {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.stamp() == stamp
                && !session.timeline_rebasing
                && (session.runways.startup_contains(reference)
                    || session
                        .progressive_plan(reference)
                        .is_some_and(|plan_id| session.runways.current(plan_id, reference)))
        })
    }

    fn hls_presentation_for_feed(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
    ) -> Option<(u64, bool)> {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            (session.generation != 0
                && session.presentation_id != 0
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
                && session.feed_identity.as_ref() == Some(&identity))
            .then_some((session.presentation_id, session.live_history_active))
        })
    }

    fn hls_playback_prefetch_admission_is_current(ticket: PrefetchTicket) -> bool {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.stamp() == ticket.stamp
                && !session.timeline_rebasing
                && session.mode != HlsPrefetchMode::Inactive
                && !session.retired_startup_plans.contains(&ticket.plan_id)
                && session
                    .tracks
                    .get(&ticket.plan_id)
                    .is_some_and(|track| track.schedule_id == ticket.schedule_id)
                && (session.mode == HlsPrefetchMode::Sustained
                    || hls_monotonic_now_ms().is_some_and(|now| now < session.startup_deadline_ms))
        })
    }

    fn set_hls_prefetch_mode(mode: HlsPrefetchMode) {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            playback.session.sequence_zero_runway_closed = hls_progressive_runway_closed_after_mode(
                playback.session.sequence_zero_runway_closed,
                mode,
            );
            playback.session.mode = mode;
        });
    }

    fn activate_hls_prefetch_warmup() {
        let now = hls_monotonic_now_ms();
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let rebasing = session.timeline_rebasing;
            let initial = !rebasing && session.mode == HlsPrefetchMode::Inactive;
            if rebasing {
                session.advance_timeline();
                session.tracks.clear();
                session.startup_overlap_plans.clear();
                session.timeline_rebasing = false;
            }
            if (initial || rebasing)
                && let Some(now) = now
            {
                session.startup_deadline_ms = now + HLS_INITIAL_RESPONSE_BUDGET_MS;
            }
            if session.mode != HlsPrefetchMode::Sustained {
                session.mode = HlsPrefetchMode::StartupOnly;
            }
        });
    }

    fn retire_hls_prefetch_timeline() {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            session.timeline_rebasing = true;
            session.advance_timeline();
            session.tracks.clear();
            session.startup_overlap_plans.clear();
        });
    }

    fn retire_hls_prefetch_plan(ticket: PrefetchTicket) {
        let retired_references = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let references = playback.plans.references_for_plans(&[ticket.plan_id]);
            let session = &mut playback.session;
            if session.stamp() == ticket.stamp
                && session
                    .tracks
                    .get(&ticket.plan_id)
                    .is_some_and(|track| track.schedule_id == ticket.schedule_id)
            {
                session.tracks.remove(&ticket.plan_id);
                session.startup_overlap_plans.remove(&ticket.plan_id);
                session.remove_progressive_plan(ticket.plan_id);
                return references;
            }
            Vec::new()
        });
        retire_hls_progressive_range_owners(None, &retired_references);
    }

    fn invalidate_hls_prefetch_session() {
        let publish = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            session.mode = HlsPrefetchMode::Inactive;
            let generation = session.advance_generation();
            session.tracks.clear();
            let client = session.client.take();
            session.feed_identity = None;
            session.sequence_zero_runway_closed = false;
            session.presentation_id = 0;
            session.live_history_active = false;
            session.live_tail_fallback = None;
            client.map(|client| (client, generation))
        });
        retire_hls_progressive_range_owners(None, &[]);
        if let Some((client, generation)) = publish {
            publish_hls_stream_generation(client, generation);
        }
    }

    fn hls_foreground_context(reference: &str, cached: bool) -> HlsForegroundContext {
        let reference = reference.to_ascii_lowercase();
        let mut publish = None;
        let (
            stamp,
            ticket,
            cursor,
            progressive_plan_id,
            progressive_owner_handoff,
            progressive_retired_references,
            seek_successor,
        ) = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let preferred = playback
                .session
                .tracks
                .iter()
                .map(|(plan, track)| (*plan, track.last_foreground_position))
                .collect::<HashMap<_, _>>();
            let selection = playback.plans.cursor(&reference, &preferred);
            let cursor = selection.as_ref().map(|selected| selected.cursor.clone());
            let superseded = selection
                .as_ref()
                .map(|selected| selected.superseded_plan_ids.as_slice())
                .unwrap_or_default();
            let superseded_references = playback.plans.references_for_plans(superseded);
            playback.plans.remove_plans(superseded);
            let session = &mut playback.session;
            let fragment_kind = session.fragment_kinds.remove(&reference);
            if session.generation == 0 {
                session.advance_generation();
            }

            let mut seek_successor = None;
            let mut progressive_owner_handoff = !superseded.is_empty();
            let mut progressive_retired_references = superseded_references;
            let mut schedule_id = None;
            if let Some(cursor) = &cursor {
                session.retired_startup_plans.remove(&cursor.plan.id);
                let cached_backward = session.tracks.get(&cursor.plan.id).is_some_and(|track| {
                    cached && cursor.position < track.last_foreground_position
                });
                let transition = session.tracks.get(&cursor.plan.id).map(|track| {
                    hls_progressive_foreground_transition(
                        track.last_foreground_position,
                        cursor.position,
                        cached,
                    )
                });
                let runway_discontinuity = !cached_backward
                    && session.runways.plans.contains_key(&cursor.plan.id)
                    && !session.runways.contains(cursor.plan.id, &reference);
                if transition.is_some_and(|transition| transition.0) || runway_discontinuity {
                    progressive_owner_handoff = true;
                    let completed_media_payloads = session
                        .completed_media_payloads
                        .min(HLS_TWO_BODY_PREFETCH_COMPLETIONS);
                    let generation = session.advance_generation();
                    session.completed_media_payloads = completed_media_payloads;
                    seek_successor = cursor
                        .plan
                        .references
                        .get(cursor.position.saturating_add(1))
                        .cloned();
                    publish = session
                        .client
                        .as_ref()
                        .cloned()
                        .map(|client| (client, generation));
                }
                if !session.tracks.contains_key(&cursor.plan.id) {
                    progressive_owner_handoff = true;
                    session.schedule_sequence = next_nonzero_generation(session.schedule_sequence);
                    session.tracks.insert(
                        cursor.plan.id,
                        HlsPrefetchTrack {
                            schedule_id: session.schedule_sequence,
                            last_foreground_position: cursor.position,
                            range_retire_position: cursor.position,
                            running: None,
                            last_touch: 0,
                        },
                    );
                }
                session.track_touch_sequence =
                    next_nonzero_generation(session.track_touch_sequence);
                let touch = session.track_touch_sequence;
                if let Some(track) = session.tracks.get_mut(&cursor.plan.id) {
                    if !cached_backward && cursor.position > track.range_retire_position {
                        progressive_retired_references.extend(
                            cursor.plan.references[track.range_retire_position..cursor.position]
                                .iter()
                                .cloned(),
                        );
                        track.range_retire_position = cursor.position;
                    } else if !cached_backward && cursor.position < track.last_foreground_position {
                        track.range_retire_position = cursor.position;
                    }
                    track.last_foreground_position = transition
                        .map(|transition| transition.1)
                        .unwrap_or(cursor.position);
                    track.last_touch = touch;
                    schedule_id = Some(track.schedule_id);
                }

                session.prune_tracks(cursor.plan.id, superseded);

                if session.mode != HlsPrefetchMode::Inactive
                    && !session.timeline_rebasing
                    && !session.sequence_zero_runway_closed
                {
                    let successor = cursor
                        .plan
                        .references
                        .get(cursor.position.saturating_add(1))
                        .cloned();
                    if session.mode == HlsPrefetchMode::StartupOnly {
                        match fragment_kind {
                            Some(Some(kind)) => {
                                progressive_owner_handoff |= session.select_startup_runway_plan(
                                    kind,
                                    successor.as_ref().map(|_| cursor.plan.id),
                                );
                            }
                            Some(None) => {
                                if !session
                                    .startup_runway_kinds
                                    .values()
                                    .any(|active| *active == cursor.plan.id)
                                {
                                    progressive_owner_handoff |=
                                        session.retired_startup_plans.insert(cursor.plan.id);
                                }
                            }
                            None => {}
                        }
                    }
                    session.remember_progressive_route(&reference, cursor.plan.id);
                    if cached_backward {
                        session.remember_progressive_replay(&reference);
                    } else {
                        session
                            .progressive_replays
                            .retain(|replay| replay != &reference);
                        session
                            .runways
                            .advance(cursor.plan.id, &reference, successor);
                    }
                }
            }

            let stamp = session.stamp();
            let ticket = cursor
                .as_ref()
                .zip(schedule_id)
                .map(|(cursor, schedule_id)| PrefetchTicket {
                    stamp,
                    plan_id: cursor.plan.id,
                    schedule_id,
                });
            let progressive_plan_id = cursor.as_ref().map(|cursor| cursor.plan.id);
            (
                stamp,
                ticket,
                cursor,
                progressive_plan_id,
                progressive_owner_handoff,
                progressive_retired_references,
                seek_successor,
            )
        });
        if let Some((client, generation)) = publish {
            publish_hls_stream_generation(client, generation);
        }
        let context = HlsForegroundContext {
            stamp,
            ticket,
            cursor,
            progressive_plan_id,
            progressive_owner_handoff,
            progressive_retired_references,
            seek_successor,
        };
        let forward = context
            .ticket
            .zip(context.cursor.as_ref())
            .and_then(|(ticket, cursor)| {
                (!context.progressive_retired_references.is_empty())
                    .then_some((ticket, cursor.position))
            });
        if context.progressive_owner_handoff
            && let (Some(ticket), Some(cursor)) = (context.ticket, context.cursor.as_ref())
        {
            adopt_hls_progressive_range_owners(ticket, cursor);
        }
        if context.progressive_owner_handoff || !context.progressive_retired_references.is_empty() {
            retire_hls_progressive_range_owners(forward, &context.progressive_retired_references);
        }
        context
    }

    fn hls_generation_current(generation: u64) -> bool {
        HLS_PLAYBACK.with(|playback| playback.borrow().session.generation == generation)
    }

    fn hls_foreground_retry_is_current(stamp: PlaybackStamp) -> bool {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.stamp() == stamp
                && !session.timeline_rebasing
                && session.mode != HlsPrefetchMode::Inactive
        })
    }

    fn hls_monotonic_now_ms() -> Option<f64> {
        let window = web_sys::window()?;
        let performance = Reflect::get(window.as_ref(), &JsValue::from_str("performance")).ok()?;
        Reflect::get(&performance, &JsValue::from_str("now"))
            .ok()?
            .dyn_ref::<Function>()?
            .call0(&performance)
            .ok()?
            .as_f64()
            .filter(|now| now.is_finite())
    }

    fn claim_hls_exact_next_overlap(ticket: PrefetchTicket, cursor: &HlsMediaCursor) -> bool {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            if session.stamp() != ticket.stamp
                || session.timeline_rebasing
                || session.retired_startup_plans.contains(&ticket.plan_id)
                || !session
                    .tracks
                    .get(&ticket.plan_id)
                    .is_some_and(|track| track.schedule_id == ticket.schedule_id)
                || session.startup_overlap_plans.contains(&cursor.plan.id)
            {
                return false;
            }
            session.startup_overlap_plans.insert(cursor.plan.id)
        })
    }

    fn hls_prefetch_ticket_current(ticket: PrefetchTicket, sustained: bool) -> bool {
        HLS_PLAYBACK.with(|playback| playback.borrow().session.ticket_current(ticket, sustained))
    }

    fn hls_progressive_range_ticket_admission(
        ticket: PrefetchTicket,
        purpose: HlsProgressiveRangePurpose,
    ) -> HlsProgressiveRangeAdmission {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let structurally_current = session.stamp() == ticket.stamp
                && !session.timeline_rebasing
                && !session.retired_startup_plans.contains(&ticket.plan_id)
                && session.tracks.get(&ticket.plan_id).is_some_and(|track| {
                    track.schedule_id == ticket.schedule_id && track.running == Some(ticket)
                });
            let admission =
                hls_progressive_range_admission(structurally_current, session.mode, purpose);
            if admission != HlsProgressiveRangeAdmission::Admit
                || purpose != HlsProgressiveRangePurpose::StartupExactNext
            {
                return admission;
            }
            session
                .beginning_prefix
                .bypass_if_expired(ticket.stamp, hls_monotonic_now_ms());
            hls_beginning_prefix_admission(session.beginning_prefix.phase)
        })
    }

    fn hls_progressive_range_handoff_current(ticket: PrefetchTicket) -> bool {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.stamp() == ticket.stamp
                && !session.timeline_rebasing
                && !session.retired_startup_plans.contains(&ticket.plan_id)
                && session
                    .tracks
                    .get(&ticket.plan_id)
                    .is_some_and(|track| track.schedule_id == ticket.schedule_id)
        })
    }

    fn try_hls_background_range_lease(reserved_bytes: u64) -> HlsProgressiveRangeLeaseAttempt {
        try_hls_background_range_lease_with_limit(
            reserved_bytes,
            media_prefetch_ahead_limit_bytes(media_cache_max_bytes()),
        )
    }

    fn try_hls_background_range_lease_with_limit(
        reserved_bytes: u64,
        limit_bytes: u64,
    ) -> HlsProgressiveRangeLeaseAttempt {
        let occupied_bytes =
            range_cache_body_bytes().saturating_add(hls_payload_cache_body_bytes());
        HLS_PROGRESSIVE_RANGE_BACKGROUND.with(|background| {
            let mut background = background.borrow_mut();
            if background.active >= HLS_BACKGROUND_RANGE_MAX {
                return HlsProgressiveRangeLeaseAttempt::Busy;
            }
            if reserved_bytes > 0
                && !hls_progressive_range_reservation_fits(
                    occupied_bytes,
                    background.reserved_bytes,
                    reserved_bytes,
                    limit_bytes,
                )
            {
                return HlsProgressiveRangeLeaseAttempt::Budget;
            }
            background.active = background.active.saturating_add(1);
            background.reserved_bytes = background.reserved_bytes.saturating_add(reserved_bytes);
            HlsProgressiveRangeLeaseAttempt::Acquired(HlsBackgroundRangeLease { reserved_bytes })
        })
    }

    fn try_hls_progressive_range_lease(
        ticket: PrefetchTicket,
        purpose: HlsProgressiveRangePurpose,
        reserved_bytes: u64,
    ) -> HlsProgressiveRangeLeaseAttempt {
        match hls_progressive_range_ticket_admission(ticket, purpose) {
            HlsProgressiveRangeAdmission::Retire => HlsProgressiveRangeLeaseAttempt::Retire,
            HlsProgressiveRangeAdmission::Park => HlsProgressiveRangeLeaseAttempt::Park,
            HlsProgressiveRangeAdmission::Admit => {
                let limit_bytes = match purpose {
                    HlsProgressiveRangePurpose::StartupExactNext => {
                        media_cache_max_bytes().saturating_sub(MEDIA_STARTUP_RESPONSE_BYTES)
                    }
                    HlsProgressiveRangePurpose::Sustained => {
                        media_prefetch_ahead_limit_bytes(media_cache_max_bytes())
                    }
                };
                try_hls_background_range_lease_with_limit(reserved_bytes, limit_bytes)
            }
        }
    }

    fn claim_hls_progressive_range_scheduler(
        mut cursor: HlsMediaCursor,
        ticket: PrefetchTicket,
    ) -> Option<HlsMediaCursor> {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            if session.stamp() != ticket.stamp
                || session.timeline_rebasing
                || session.retired_startup_plans.contains(&ticket.plan_id)
            {
                return None;
            }
            let track = session.tracks.get_mut(&ticket.plan_id)?;
            if track.schedule_id != ticket.schedule_id || track.running.is_some() {
                return None;
            }
            cursor.position = cursor.position.max(track.last_foreground_position);
            track.running = Some(ticket);
            Some(cursor)
        })
    }

    fn finish_hls_progressive_range_scheduler(ticket: PrefetchTicket) {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            if session.stamp() == ticket.stamp
                && let Some(track) = session.tracks.get_mut(&ticket.plan_id)
                && track.schedule_id == ticket.schedule_id
                && track.running == Some(ticket)
            {
                track.running = None;
            }
        });
    }

    fn adopt_hls_progressive_range_owners(ticket: PrefetchTicket, cursor: &HlsMediaCursor) {
        if !hls_progressive_range_handoff_current(ticket) {
            return;
        }
        let mut adopted = Vec::new();
        for (position, reference) in cursor
            .plan
            .references
            .iter()
            .enumerate()
            .skip(cursor.position.saturating_add(1))
            .take(1)
        {
            let Some(payload_size) = cached_hls_payload_size(reference).filter(|size| *size > 0)
            else {
                continue;
            };
            let Some(metadata) = hls_range_metadata(reference, payload_size) else {
                continue;
            };
            let mut start = 0_u64;
            while start < payload_size {
                let end = start
                    .saturating_add(MEDIA_STORAGE_WINDOW_BYTES)
                    .saturating_sub(1)
                    .min(payload_size.saturating_sub(1));
                if hls_aligned_range_cached(reference, &metadata, start, end) {
                    adopted.push((
                        HlsProgressiveRangeKey {
                            reference: reference.clone(),
                            payload_size,
                            start,
                            end,
                        },
                        HlsProgressiveRangeOwner { ticket, position },
                    ));
                }
                start = end.saturating_add(1);
            }
        }
        HLS_PROGRESSIVE_RANGE_BACKGROUND.with(|background| {
            let mut background = background.borrow_mut();
            for (key, owner) in adopted {
                background
                    .owned_ranges
                    .entry(key)
                    .or_default()
                    .insert(owner);
            }
        });
    }

    fn remember_hls_progressive_range_owner(
        ticket: PrefetchTicket,
        purpose: HlsProgressiveRangePurpose,
        position: usize,
        reference: &str,
        payload_size: u64,
        start: u64,
        end: u64,
    ) {
        if hls_progressive_range_ticket_admission(ticket, purpose)
            == HlsProgressiveRangeAdmission::Retire
        {
            HLS_PROGRESSIVE_RANGE_BACKGROUND.with(|background| {
                background
                    .borrow_mut()
                    .retired_references
                    .insert(reference.to_string(), Some(payload_size));
            });
            retire_hls_progressive_range_owners(None, &[]);
            return;
        }
        let key = HlsProgressiveRangeKey {
            reference: reference.to_string(),
            payload_size,
            start,
            end,
        };
        let owner = HlsProgressiveRangeOwner { ticket, position };
        HLS_PROGRESSIVE_RANGE_BACKGROUND.with(|background| {
            background
                .borrow_mut()
                .owned_ranges
                .entry(key)
                .or_default()
                .insert(owner);
        });
    }

    fn retire_hls_progressive_range_owners(
        forward: Option<(PrefetchTicket, usize)>,
        retired_references: &[String],
    ) {
        let (stamp, schedules, retired_plans, protected) = HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            let schedules = session
                .tracks
                .iter()
                .map(|(plan_id, track)| (*plan_id, track.schedule_id))
                .collect::<HashMap<_, _>>();
            let mut protected = HashSet::new();
            if let Some(runway) = session.runways.startup.as_ref() {
                protected.insert(runway.current().to_string());
                protected.extend(runway.successor().map(str::to_string));
            }
            for (plan_id, runway) in &session.runways.plans {
                if session.retired_startup_plans.contains(plan_id) {
                    continue;
                }
                protected.insert(runway.current().to_string());
                protected.extend(runway.successor().map(str::to_string));
            }
            (
                session.stamp(),
                schedules,
                session.retired_startup_plans.clone(),
                protected,
            )
        });
        let retired_references = retired_references.iter().cloned().collect::<HashSet<_>>();
        let retired_sizes = retired_references
            .iter()
            .map(|reference| (reference.clone(), cached_hls_payload_size(reference)))
            .collect::<Vec<_>>();

        let retired = HLS_PROGRESSIVE_RANGE_BACKGROUND.with(|background| {
            let mut background = background.borrow_mut();
            for (reference, payload_size) in retired_sizes {
                let remembered = background.retired_references.entry(reference).or_default();
                if remembered.is_none() {
                    *remembered = payload_size;
                }
            }
            let mut newly_ownerless = Vec::new();
            for (key, owners) in &mut background.owned_ranges {
                owners.retain(|owner| {
                    let structurally_current = owner.ticket.stamp == stamp
                        && !retired_plans.contains(&owner.ticket.plan_id)
                        && schedules.get(&owner.ticket.plan_id).copied()
                            == Some(owner.ticket.schedule_id);
                    structurally_current
                        && !forward.is_some_and(|(ticket, position)| {
                            owner.ticket == ticket && owner.position < position
                        })
                });
                if owners.is_empty() {
                    newly_ownerless.push((key.reference.clone(), key.payload_size));
                }
            }
            for (reference, payload_size) in newly_ownerless {
                background
                    .retired_references
                    .entry(reference)
                    .or_insert(Some(payload_size));
            }

            let owned_references = background
                .owned_ranges
                .iter()
                .filter(|(_, owners)| !owners.is_empty())
                .map(|(key, _)| key.reference.clone())
                .collect::<HashSet<_>>();
            let retired = background
                .retired_references
                .iter()
                .filter(|(reference, _)| {
                    !protected.contains(*reference) && !owned_references.contains(*reference)
                })
                .map(|(reference, payload_size)| (reference.clone(), *payload_size))
                .collect::<Vec<_>>();
            let retired_set = retired
                .iter()
                .map(|(reference, _)| reference.clone())
                .collect::<HashSet<_>>();
            background
                .owned_ranges
                .retain(|key, _| !retired_set.contains(&key.reference));
            background
                .retired_references
                .retain(|reference, _| !retired_set.contains(reference));
            retired
        });

        for (reference, payload_size) in retired {
            if let Some(metadata) =
                payload_size.and_then(|size| hls_range_metadata(&reference, size))
            {
                evict_completed_hls_ranges(&reference, &metadata);
            }
            HLS_ASSET_CACHE.with(|cache| {
                cache.borrow_mut().evict_completed_body(&reference);
            });
        }
    }

    fn clear_hls_progressive_range_owners() {
        HLS_PROGRESSIVE_RANGE_BACKGROUND.with(|background| {
            let mut background = background.borrow_mut();
            background.owned_ranges.clear();
            background.retired_references.clear();
        });
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum HlsProgressiveWindowDispatch {
        Deferred,
        Dispatched,
        Retired,
    }

    struct HlsProgressiveWindowOutcome {
        window: usize,
        dispatch: HlsProgressiveWindowDispatch,
    }

    type HlsProgressiveWindowFuture =
        Pin<Box<dyn Future<Output = HlsProgressiveWindowOutcome> + 'static>>;

    #[derive(Clone)]
    struct HlsProgressiveRollingBoundaryState {
        first_window_complete: Rc<Cell<bool>>,
        current_reference_complete: Rc<Cell<bool>>,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum HlsProgressiveReferenceProgress {
        Complete,
        ForegroundAdvanced(usize),
        Retired,
    }

    fn hls_progressive_range_floor(ticket: PrefetchTicket) -> Option<usize> {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            if session.stamp() != ticket.stamp
                || session.timeline_rebasing
                || session.retired_startup_plans.contains(&ticket.plan_id)
            {
                return None;
            }
            let track = session.tracks.get(&ticket.plan_id)?;
            (track.schedule_id == ticket.schedule_id && track.running == Some(ticket))
                .then_some(track.last_foreground_position)
        })
    }

    fn hls_critical_prefix_plan_for_reference(
        reference: &str,
        payload_size: u64,
    ) -> Option<HlsCriticalPrefixPlan> {
        let duration_seconds = HLS_PLAYBACK.with(|playback| {
            playback
                .borrow()
                .durations
                .get(&reference.to_ascii_lowercase())
                .copied()
        })?;
        HlsCriticalPrefixPlan::new(payload_size, duration_seconds, MEDIA_STORAGE_WINDOW_BYTES)
    }

    fn queue_hls_progressive_window(
        weeb3: Arc<Weeb3>,
        reference: String,
        metadata: BzzMetadata,
        position: usize,
        window: usize,
        start: u64,
        end: u64,
        ticket: PrefetchTicket,
        purpose: HlsProgressiveRangePurpose,
        lease: HlsBackgroundRangeLease,
    ) -> HlsProgressiveWindowFuture {
        Box::pin(async move {
            let dispatch = Rc::new(Cell::new(HlsProgressiveWindowDispatch::Deferred));
            let dispatch_for_admission = dispatch.clone();
            let owner_reference = reference.clone();
            let owner_metadata = metadata.clone();
            let payload_size = metadata.size;
            let background = HlsBackgroundRangeRequest::new(lease, move || {
                match hls_progressive_range_ticket_admission(ticket, purpose) {
                    HlsProgressiveRangeAdmission::Admit => {}
                    HlsProgressiveRangeAdmission::Park => return false,
                    HlsProgressiveRangeAdmission::Retire => {
                        dispatch_for_admission.set(HlsProgressiveWindowDispatch::Retired);
                        return false;
                    }
                }
                if hls_aligned_range_state(
                    &owner_reference,
                    &owner_metadata,
                    start,
                    end,
                    ticket.stamp.generation,
                ) != HlsAlignedRangeState::Absent
                {
                    return false;
                }
                remember_hls_progressive_range_owner(
                    ticket,
                    purpose,
                    position,
                    &owner_reference,
                    payload_size,
                    start,
                    end,
                );
                dispatch_for_admission.set(HlsProgressiveWindowDispatch::Dispatched);
                true
            });
            let _bytes = weeb3
                .retrieve_hls_payload_range(
                    reference,
                    payload_size,
                    start,
                    end,
                    Some(ticket.stamp.generation),
                    Some(background),
                )
                .await;
            HlsProgressiveWindowOutcome {
                window,
                dispatch: dispatch.get(),
            }
        })
    }

    fn settle_hls_progressive_window_outcome(
        reference: &str,
        metadata: &BzzMetadata,
        position: usize,
        payload_size: u64,
        ticket: PrefetchTicket,
        purpose: HlsProgressiveRangePurpose,
        outcome: HlsProgressiveWindowOutcome,
        active_windows: &mut HashSet<usize>,
        awaiting_terminal: &mut HashSet<usize>,
        retry_attempts: &mut [u32],
        retry_after_ms: &mut [f64],
    ) -> bool {
        active_windows.remove(&outcome.window);
        let Some(retry_attempt) = retry_attempts.get_mut(outcome.window) else {
            return false;
        };
        let Some(retry_after) = retry_after_ms.get_mut(outcome.window) else {
            return false;
        };
        let Some((start, end)) = hls_beginning_prefix_window_bounds(payload_size, outcome.window)
        else {
            return false;
        };
        match hls_aligned_range_state(reference, metadata, start, end, ticket.stamp.generation) {
            HlsAlignedRangeState::Cached => {
                awaiting_terminal.remove(&outcome.window);
                *retry_attempt = 0;
                *retry_after = 0.0;
                remember_hls_progressive_range_owner(
                    ticket,
                    purpose,
                    position,
                    reference,
                    payload_size,
                    start,
                    end,
                );
            }
            HlsAlignedRangeState::Pending
                if outcome.dispatch == HlsProgressiveWindowDispatch::Dispatched =>
            {
                awaiting_terminal.insert(outcome.window);
            }
            HlsAlignedRangeState::Absent
                if outcome.dispatch == HlsProgressiveWindowDispatch::Dispatched =>
            {
                let delay = hls_startup_retry_delay_ms(*retry_attempt);
                *retry_attempt = retry_attempt.saturating_add(1);
                *retry_after = js_sys::Date::now() + delay as f64;
            }
            HlsAlignedRangeState::Pending | HlsAlignedRangeState::Absent => {}
        }
        true
    }

    async fn drain_hls_progressive_windows(
        active: &mut FuturesUnordered<HlsProgressiveWindowFuture>,
    ) {
        while active.next().await.is_some() {}
    }

    async fn resolve_hls_progressive_payload_size(
        weeb3: Arc<Weeb3>,
        reference: &str,
        position: usize,
        ticket: PrefetchTicket,
        purpose: HlsProgressiveRangePurpose,
    ) -> Result<u64, HlsProgressiveReferenceProgress> {
        let mut retry_attempt = 0_u32;
        loop {
            let Some(foreground_position) = hls_progressive_range_floor(ticket) else {
                return Err(HlsProgressiveReferenceProgress::Retired);
            };
            if let HlsProgressiveFrontierDecision::Rebase(position) =
                hls_progressive_frontier_decision(position, foreground_position, false)
            {
                return Err(HlsProgressiveReferenceProgress::ForegroundAdvanced(
                    position,
                ));
            }
            match hls_progressive_range_ticket_admission(ticket, purpose) {
                HlsProgressiveRangeAdmission::Retire => {
                    return Err(HlsProgressiveReferenceProgress::Retired);
                }
                HlsProgressiveRangeAdmission::Park => {
                    async_std::task::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                HlsProgressiveRangeAdmission::Admit => {}
            }
            if let Some(payload_size) = cached_hls_payload_size(reference).filter(|size| *size > 0)
            {
                return Ok(payload_size);
            }
            let lease = match try_hls_progressive_range_lease(ticket, purpose, 0) {
                HlsProgressiveRangeLeaseAttempt::Acquired(lease) => lease,
                HlsProgressiveRangeLeaseAttempt::Busy | HlsProgressiveRangeLeaseAttempt::Budget => {
                    async_std::task::sleep(Duration::from_millis(hls_startup_retry_delay_ms(
                        retry_attempt,
                    )))
                    .await;
                    retry_attempt = retry_attempt.saturating_add(1);
                    continue;
                }
                HlsProgressiveRangeLeaseAttempt::Park => {
                    async_std::task::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                HlsProgressiveRangeLeaseAttempt::Retire => {
                    return Err(HlsProgressiveReferenceProgress::Retired);
                }
            };
            let payload_size = start_hls_payload_size_probe(weeb3.clone(), reference.to_string())
                .recv()
                .await
                .ok()
                .flatten()
                .filter(|size| *size > 0);
            drop(lease);
            let Some(foreground_position) = hls_progressive_range_floor(ticket) else {
                return Err(HlsProgressiveReferenceProgress::Retired);
            };
            if let HlsProgressiveFrontierDecision::Rebase(position) =
                hls_progressive_frontier_decision(position, foreground_position, false)
            {
                return Err(HlsProgressiveReferenceProgress::ForegroundAdvanced(
                    position,
                ));
            }
            if let Some(payload_size) = payload_size {
                return Ok(payload_size);
            }
            async_std::task::sleep(Duration::from_millis(hls_startup_retry_delay_ms(
                retry_attempt,
            )))
            .await;
            retry_attempt = retry_attempt.saturating_add(1);
        }
    }

    async fn prefetch_hls_progressive_reference_windows(
        weeb3: Arc<Weeb3>,
        reference: String,
        position: usize,
        payload_size: u64,
        window_limit: usize,
        ticket: PrefetchTicket,
        purpose: HlsProgressiveRangePurpose,
        launch_position: usize,
        initial_successor_complete: bool,
        width_cap: usize,
        widen_when_peer_released: Option<Rc<Cell<bool>>>,
        rolling_boundary: Option<HlsProgressiveRollingBoundaryState>,
    ) -> HlsProgressiveReferenceProgress {
        let Some(metadata) = hls_range_metadata(&reference, payload_size) else {
            return HlsProgressiveReferenceProgress::Retired;
        };
        let total_windows =
            usize::try_from(payload_size.saturating_sub(1) / MEDIA_STORAGE_WINDOW_BYTES + 1)
                .unwrap_or(usize::MAX);
        let window_limit = window_limit.min(total_windows);
        if window_limit == 0 {
            return HlsProgressiveReferenceProgress::Complete;
        }

        let mut active = FuturesUnordered::<HlsProgressiveWindowFuture>::new();
        let mut active_windows = HashSet::<usize>::new();
        let mut awaiting_terminal = HashSet::<usize>::new();
        let mut retry_attempts = vec![0_u32; window_limit];
        let mut retry_after_ms = vec![0.0_f64; window_limit];
        let mut budget_attempt = 0_u32;

        loop {
            while let Some(Some(outcome)) = active.next().now_or_never() {
                if !settle_hls_progressive_window_outcome(
                    &reference,
                    &metadata,
                    position,
                    payload_size,
                    ticket,
                    purpose,
                    outcome,
                    &mut active_windows,
                    &mut awaiting_terminal,
                    &mut retry_attempts,
                    &mut retry_after_ms,
                ) {
                    drain_hls_progressive_windows(&mut active).await;
                    return HlsProgressiveReferenceProgress::Retired;
                }
            }
            let Some(foreground_position) = hls_progressive_range_floor(ticket) else {
                drain_hls_progressive_windows(&mut active).await;
                return HlsProgressiveReferenceProgress::Retired;
            };
            if let HlsProgressiveFrontierDecision::Rebase(position) =
                hls_progressive_frontier_decision(position, foreground_position, false)
            {
                drain_hls_progressive_windows(&mut active).await;
                return HlsProgressiveReferenceProgress::ForegroundAdvanced(position);
            }
            let admission = hls_progressive_range_ticket_admission(ticket, purpose);
            if admission == HlsProgressiveRangeAdmission::Retire {
                drain_hls_progressive_windows(&mut active).await;
                return HlsProgressiveReferenceProgress::Retired;
            }

            let now_ms = js_sys::Date::now();
            let mut states = Vec::with_capacity(window_limit);
            for window in 0..window_limit {
                let Some((start, end)) = hls_beginning_prefix_window_bounds(payload_size, window)
                else {
                    drain_hls_progressive_windows(&mut active).await;
                    return HlsProgressiveReferenceProgress::Retired;
                };
                let state = if active_windows.contains(&window) {
                    HlsOrderedWindowState::Active
                } else {
                    match hls_aligned_range_state(
                        &reference,
                        &metadata,
                        start,
                        end,
                        ticket.stamp.generation,
                    ) {
                        HlsAlignedRangeState::Cached => {
                            awaiting_terminal.remove(&window);
                            retry_attempts[window] = 0;
                            retry_after_ms[window] = 0.0;
                            remember_hls_progressive_range_owner(
                                ticket,
                                purpose,
                                position,
                                &reference,
                                payload_size,
                                start,
                                end,
                            );
                            HlsOrderedWindowState::Cached
                        }
                        HlsAlignedRangeState::Pending => HlsOrderedWindowState::Pending,
                        HlsAlignedRangeState::Absent => {
                            if awaiting_terminal.remove(&window) {
                                let delay = hls_startup_retry_delay_ms(retry_attempts[window]);
                                retry_attempts[window] = retry_attempts[window].saturating_add(1);
                                retry_after_ms[window] = now_ms + delay as f64;
                            }
                            if now_ms < retry_after_ms[window] {
                                HlsOrderedWindowState::Backoff
                            } else {
                                HlsOrderedWindowState::Absent
                            }
                        }
                    }
                };
                states.push(state);
            }
            let first_window_cached = states
                .first()
                .is_some_and(|state| *state == HlsOrderedWindowState::Cached);
            // Future boundary work stops after W0. Its completion releases the
            // reserved slot back to the tail; only foreground catch-up or a
            // wholly completed current reference opens the remaining prefix.
            if first_window_cached && let Some(boundary) = rolling_boundary.as_ref() {
                boundary.first_window_complete.set(true);
            }
            if states
                .iter()
                .all(|state| *state == HlsOrderedWindowState::Cached)
            {
                debug_assert!(active.is_empty());
                return HlsProgressiveReferenceProgress::Complete;
            }

            let frontier_width = hls_progressive_frontier_width(
                launch_position,
                foreground_position,
                initial_successor_complete,
            );
            let width = if let Some(boundary) = rolling_boundary.as_ref() {
                hls_progressive_rolling_boundary_width(
                    frontier_width,
                    first_window_cached,
                    boundary.current_reference_complete.get(),
                    foreground_position == position,
                )
            } else {
                hls_progressive_rolling_lane_width(
                    frontier_width,
                    width_cap,
                    widen_when_peer_released
                        .as_ref()
                        .is_some_and(|released| released.get()),
                )
            };
            let retry_floor = retry_attempts
                .iter()
                .enumerate()
                .find_map(|(window, attempts)| {
                    (*attempts > 0 && states[window] != HlsOrderedWindowState::Cached)
                        .then_some(window)
                });
            let candidates = if admission == HlsProgressiveRangeAdmission::Admit {
                hls_ordered_window_admissions(&states, width, retry_floor)
            } else {
                Vec::new()
            };
            let mut lease_block = None;
            for window in candidates {
                let Some((start, end)) = hls_beginning_prefix_window_bounds(payload_size, window)
                else {
                    drain_hls_progressive_windows(&mut active).await;
                    return HlsProgressiveReferenceProgress::Retired;
                };
                let expected = end.saturating_sub(start).saturating_add(1);
                match try_hls_progressive_range_lease(ticket, purpose, expected) {
                    HlsProgressiveRangeLeaseAttempt::Acquired(lease) => {
                        active_windows.insert(window);
                        active.push(queue_hls_progressive_window(
                            weeb3.clone(),
                            reference.clone(),
                            metadata.clone(),
                            position,
                            window,
                            start,
                            end,
                            ticket,
                            purpose,
                            lease,
                        ));
                        budget_attempt = 0;
                    }
                    HlsProgressiveRangeLeaseAttempt::Busy => {
                        lease_block = Some(MEDIA_PREFETCH_BATCH_YIELD_MS);
                        break;
                    }
                    HlsProgressiveRangeLeaseAttempt::Budget => {
                        lease_block = Some(hls_startup_retry_delay_ms(budget_attempt));
                        budget_attempt = budget_attempt.saturating_add(1);
                        break;
                    }
                    HlsProgressiveRangeLeaseAttempt::Park => {
                        lease_block = Some(50);
                        break;
                    }
                    HlsProgressiveRangeLeaseAttempt::Retire => {
                        drain_hls_progressive_windows(&mut active).await;
                        return HlsProgressiveReferenceProgress::Retired;
                    }
                }
            }

            let needs_poll = states.iter().any(|state| {
                matches!(
                    state,
                    HlsOrderedWindowState::Pending | HlsOrderedWindowState::Backoff
                )
            });
            let needs_width_transition_poll = rolling_boundary.is_some()
                || widen_when_peer_released
                    .as_ref()
                    .is_some_and(|released| !released.get());
            let wait_ms = lease_block.unwrap_or(MEDIA_PREFETCH_BATCH_YIELD_MS);
            let outcome = if active.is_empty() {
                async_std::task::sleep(Duration::from_millis(
                    if admission == HlsProgressiveRangeAdmission::Park {
                        50
                    } else {
                        wait_ms
                    },
                ))
                .await;
                None
            } else if lease_block.is_some() || needs_poll || needs_width_transition_poll {
                let next = active.next();
                let pause = async_std::task::sleep(Duration::from_millis(wait_ms));
                pin_mut!(next, pause);
                match select(next, pause).await {
                    Either::Left((outcome, _)) => outcome,
                    Either::Right(_) => None,
                }
            } else {
                active.next().await
            };
            let Some(outcome) = outcome else {
                continue;
            };
            if !settle_hls_progressive_window_outcome(
                &reference,
                &metadata,
                position,
                payload_size,
                ticket,
                purpose,
                outcome,
                &mut active_windows,
                &mut awaiting_terminal,
                &mut retry_attempts,
                &mut retry_after_ms,
            ) {
                drain_hls_progressive_windows(&mut active).await;
                return HlsProgressiveReferenceProgress::Retired;
            }
        }
    }

    async fn await_hls_progressive_rolling_boundary_admission(
        ticket: PrefetchTicket,
        launch_position: usize,
        initial_successor_complete: bool,
        current_reference_complete: Rc<Cell<bool>>,
    ) -> Result<(), HlsProgressiveReferenceProgress> {
        loop {
            let Some(foreground_position) = hls_progressive_range_floor(ticket) else {
                return Err(HlsProgressiveReferenceProgress::Retired);
            };
            if current_reference_complete.get()
                || hls_progressive_frontier_width(
                    launch_position,
                    foreground_position,
                    initial_successor_complete,
                ) == HLS_BACKGROUND_RANGE_MAX
            {
                return Ok(());
            }
            if hls_progressive_range_ticket_admission(ticket, HlsProgressiveRangePurpose::Sustained)
                == HlsProgressiveRangeAdmission::Retire
            {
                return Err(HlsProgressiveReferenceProgress::Retired);
            }
            async_std::task::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn prefetch_hls_progressive_boundary_prefix(
        weeb3: Arc<Weeb3>,
        reference: String,
        position: usize,
        ticket: PrefetchTicket,
        launch_position: usize,
        initial_successor_complete: bool,
        current_reference_complete: Rc<Cell<bool>>,
        first_window_complete: Rc<Cell<bool>>,
    ) -> HlsProgressiveReferenceProgress {
        if let Err(progress) = await_hls_progressive_rolling_boundary_admission(
            ticket,
            launch_position,
            initial_successor_complete,
            current_reference_complete.clone(),
        )
        .await
        {
            return progress;
        }
        let payload_size = match resolve_hls_progressive_payload_size(
            weeb3.clone(),
            &reference,
            position,
            ticket,
            HlsProgressiveRangePurpose::Sustained,
        )
        .await
        {
            Ok(payload_size) => payload_size,
            Err(progress) => return progress,
        };
        let boundary_windows = hls_critical_prefix_plan_for_reference(&reference, payload_size)
            .map_or(1, HlsCriticalPrefixPlan::critical_windows);
        prefetch_hls_progressive_reference_windows(
            weeb3,
            reference,
            position,
            payload_size,
            boundary_windows,
            ticket,
            HlsProgressiveRangePurpose::Sustained,
            launch_position,
            initial_successor_complete,
            HLS_PROGRESSIVE_BOUNDARY_RANGE_MAX,
            None,
            Some(HlsProgressiveRollingBoundaryState {
                first_window_complete,
                current_reference_complete,
            }),
        )
        .await
    }

    async fn prefetch_hls_progressive_rolling_pair(
        weeb3: Arc<Weeb3>,
        current_reference: String,
        current_position: usize,
        current_payload_size: u64,
        current_window_limit: usize,
        next_boundary_reference: String,
        next_boundary_position: usize,
        ticket: PrefetchTicket,
        launch_position: usize,
        initial_successor_complete: bool,
    ) -> (
        HlsProgressiveReferenceProgress,
        HlsProgressiveReferenceProgress,
    ) {
        let next_boundary_first_window_complete = Rc::new(Cell::new(false));
        let current_reference_complete = Rc::new(Cell::new(false));
        let release_tail_width = next_boundary_first_window_complete.clone();
        let boundary_first_window_complete = next_boundary_first_window_complete.clone();
        let admit_boundary_after_current = current_reference_complete.clone();
        let current_complete_on_success = current_reference_complete.clone();
        let boundary_weeb3 = weeb3.clone();
        let next_boundary = async move {
            prefetch_hls_progressive_boundary_prefix(
                boundary_weeb3,
                next_boundary_reference,
                next_boundary_position,
                ticket,
                launch_position,
                initial_successor_complete,
                admit_boundary_after_current,
                boundary_first_window_complete,
            )
            .await
        };
        let current_tail = async move {
            let progress = prefetch_hls_progressive_reference_windows(
                weeb3,
                current_reference,
                current_position,
                current_payload_size,
                current_window_limit,
                ticket,
                HlsProgressiveRangePurpose::Sustained,
                launch_position,
                initial_successor_complete,
                HLS_PROGRESSIVE_TAIL_WITH_BOUNDARY_MAX,
                Some(release_tail_width),
                None,
            )
            .await;
            if progress == HlsProgressiveReferenceProgress::Complete {
                current_complete_on_success.set(true);
            }
            progress
        };

        // Poll the boundary lane first so future W0 gets the reserved slot.
        // Once W0 settles it parks and returns that slot to the tail; if it
        // becomes foreground, the shared lease cap lets it refill only as old
        // tail flights drain. `join` keeps every admitted peer future alive.
        let (next_boundary, current_tail) = join(next_boundary, current_tail).await;
        (current_tail, next_boundary)
    }

    async fn prefetch_hls_progressive_ranges(
        weeb3: Arc<Weeb3>,
        cursor: HlsMediaCursor,
        ticket: PrefetchTicket,
    ) {
        let launch_position = cursor.position;
        let initial_successor = launch_position.saturating_add(1);
        let mut next_position = initial_successor;
        let mut initial_prefix_complete = false;
        let mut initial_successor_complete = false;

        loop {
            let Some(foreground_position) = hls_progressive_range_floor(ticket) else {
                return;
            };
            if foreground_position > launch_position {
                initial_prefix_complete = true;
            }
            if let HlsProgressiveFrontierDecision::Rebase(position) =
                hls_progressive_frontier_decision(next_position, foreground_position, false)
            {
                next_position = position;
            }
            if next_position >= cursor.plan.references.len() {
                return;
            }

            let position = next_position;
            let reference = cursor.plan.references[position].clone();
            let startup_successor = position == initial_successor && !initial_prefix_complete;
            let size_purpose = if startup_successor {
                HlsProgressiveRangePurpose::StartupExactNext
            } else {
                HlsProgressiveRangePurpose::Sustained
            };
            let payload_size = match resolve_hls_progressive_payload_size(
                weeb3.clone(),
                &reference,
                position,
                ticket,
                size_purpose,
            )
            .await
            {
                Ok(payload_size) => payload_size,
                Err(HlsProgressiveReferenceProgress::ForegroundAdvanced(position)) => {
                    initial_prefix_complete = true;
                    next_position = next_position.max(position);
                    continue;
                }
                Err(HlsProgressiveReferenceProgress::Retired) => return,
                Err(HlsProgressiveReferenceProgress::Complete) => unreachable!(),
            };

            let critical_prefix = startup_successor
                .then(|| hls_critical_prefix_plan_for_reference(&reference, payload_size))
                .flatten();
            if startup_successor {
                if let Some(prefix) = critical_prefix {
                    match prefetch_hls_progressive_reference_windows(
                        weeb3.clone(),
                        reference.clone(),
                        position,
                        payload_size,
                        prefix.critical_windows(),
                        ticket,
                        HlsProgressiveRangePurpose::StartupExactNext,
                        launch_position,
                        false,
                        HLS_BACKGROUND_RANGE_MAX,
                        None,
                        None,
                    )
                    .await
                    {
                        HlsProgressiveReferenceProgress::Complete => {
                            initial_prefix_complete = true;
                        }
                        HlsProgressiveReferenceProgress::ForegroundAdvanced(position) => {
                            initial_prefix_complete = true;
                            next_position = next_position.max(position);
                            continue;
                        }
                        HlsProgressiveReferenceProgress::Retired => return,
                    }
                } else {
                    // A malformed/missing duration cannot justify a guessed
                    // startup horizon. Sustained mode may still fill the whole
                    // nearest reference using its exact resolved size.
                    initial_prefix_complete = true;
                }
            }

            let total_windows = critical_prefix.map_or_else(
                || {
                    usize::try_from(payload_size.saturating_sub(1) / MEDIA_STORAGE_WINDOW_BYTES + 1)
                        .unwrap_or(usize::MAX)
                },
                HlsCriticalPrefixPlan::total_windows,
            );
            let next_boundary_position = position.saturating_add(1);
            if let Some(next_boundary_reference) =
                cursor.plan.references.get(next_boundary_position).cloned()
            {
                let (current_progress, next_boundary_progress) =
                    prefetch_hls_progressive_rolling_pair(
                        weeb3.clone(),
                        reference,
                        position,
                        payload_size,
                        total_windows,
                        next_boundary_reference,
                        next_boundary_position,
                        ticket,
                        launch_position,
                        initial_successor_complete,
                    )
                    .await;
                if current_progress == HlsProgressiveReferenceProgress::Retired
                    || next_boundary_progress == HlsProgressiveReferenceProgress::Retired
                {
                    return;
                }
                let foreground_advanced = [current_progress, next_boundary_progress]
                    .into_iter()
                    .filter_map(|progress| match progress {
                        HlsProgressiveReferenceProgress::ForegroundAdvanced(position) => {
                            Some(position)
                        }
                        HlsProgressiveReferenceProgress::Complete
                        | HlsProgressiveReferenceProgress::Retired => None,
                    })
                    .max();
                if let Some(position) = foreground_advanced {
                    initial_prefix_complete = true;
                    next_position = next_position.max(position);
                    continue;
                }
                debug_assert_eq!(current_progress, HlsProgressiveReferenceProgress::Complete);
                debug_assert_eq!(
                    next_boundary_progress,
                    HlsProgressiveReferenceProgress::Complete
                );
                if position == initial_successor {
                    initial_successor_complete = true;
                }
                let HlsProgressiveFrontierDecision::Advance(position) =
                    hls_progressive_frontier_decision(position, position, true)
                else {
                    unreachable!();
                };
                next_position = position;
                continue;
            }
            match prefetch_hls_progressive_reference_windows(
                weeb3.clone(),
                reference,
                position,
                payload_size,
                total_windows,
                ticket,
                HlsProgressiveRangePurpose::Sustained,
                launch_position,
                initial_successor_complete,
                HLS_BACKGROUND_RANGE_MAX,
                None,
                None,
            )
            .await
            {
                HlsProgressiveReferenceProgress::Complete => {
                    if position == initial_successor {
                        initial_successor_complete = true;
                    }
                    let HlsProgressiveFrontierDecision::Advance(position) =
                        hls_progressive_frontier_decision(position, position, true)
                    else {
                        unreachable!();
                    };
                    next_position = position;
                }
                HlsProgressiveReferenceProgress::ForegroundAdvanced(position) => {
                    initial_prefix_complete = true;
                    next_position = next_position.max(position);
                }
                HlsProgressiveReferenceProgress::Retired => return,
            }
        }
    }
    fn spawn_hls_progressive_range_prefetch(
        weeb3: Arc<Weeb3>,
        cursor: HlsMediaCursor,
        ticket: PrefetchTicket,
    ) -> bool {
        let Some(cursor) = claim_hls_progressive_range_scheduler(cursor, ticket) else {
            return false;
        };
        if cursor.position.saturating_add(1) >= cursor.plan.references.len() {
            finish_hls_progressive_range_scheduler(ticket);
            return false;
        }
        spawn_local(async move {
            prefetch_hls_progressive_ranges(weeb3, cursor, ticket).await;
            finish_hls_progressive_range_scheduler(ticket);
        });
        true
    }

    fn start_hls_payload_load(
        weeb3: Arc<Weeb3>,
        reference: String,
        prefetch: bool,
        generation: u64,
    ) -> HlsPayloadLoadRole {
        let reference = reference.to_ascii_lowercase();
        let (limit, timeline_epoch) = HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            (
                playback.session.body_parallelism(generation),
                playback.session.timeline_epoch,
            )
        });
        let role = HLS_ASSET_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .load_role(&reference, prefetch, generation, limit)
        });
        if let HlsPayloadLoadRole::Lead(_, load_id) = &role {
            let load_id = *load_id;
            spawn_local(async move {
                let bytes = weeb3
                    .retrieve_hls_payload_cancellable(
                        reference.clone(),
                        HLS_STREAM_KEY.to_string(),
                        generation,
                    )
                    .await;
                let result = if bytes.is_empty() {
                    Err(format!("weeb-3 did not retrieve HLS fragment {reference}"))
                } else {
                    Ok(Arc::<[u8]>::from(bytes))
                };
                let media = result
                    .as_ref()
                    .is_ok_and(|body| !is_hls_manifest(body.as_ref()));
                let hot = hls_generation_current(generation);
                let owned = HLS_ASSET_CACHE.with(|cache| {
                    cache
                        .borrow_mut()
                        .finish_load(&reference, generation, load_id, result, hot)
                });
                if owned && hot && media {
                    HLS_PLAYBACK.with(|playback| {
                        let mut playback = playback.borrow_mut();
                        let session = &mut playback.session;
                        if session.generation == generation
                            && session.timeline_epoch == timeline_epoch
                        {
                            session.completed_media_payloads =
                                session.completed_media_payloads.saturating_add(1);
                        }
                    });
                }
            });
        }
        role
    }

    async fn wait_hls_payload_load(role: HlsPayloadLoadRole) -> Result<Arc<[u8]>, String> {
        match role {
            HlsPayloadLoadRole::Cached(body) => Ok(body),
            HlsPayloadLoadRole::Wait(receiver) | HlsPayloadLoadRole::Lead(receiver, _) => receiver
                .recv()
                .await
                .map_err(|_| "HLS fragment load was canceled".to_string())?,
            HlsPayloadLoadRole::AtCapacity => {
                Err("HLS fragment lookahead is already at capacity".to_string())
            }
            HlsPayloadLoadRole::Reject(error) => Err(error),
        }
    }

    async fn wait_for_pending_hls_payload(reference: &str) -> Option<Arc<[u8]>> {
        let reference = reference.to_ascii_lowercase();
        let generation = HLS_PLAYBACK.with(|playback| playback.borrow().session.generation);
        let role = HLS_ASSET_CACHE
            .with(|cache| cache.borrow_mut().join_pending(&reference, generation))?;
        wait_hls_payload_load(role).await.ok()
    }

    fn spawn_hls_prefetch_stages(
        weeb3: Arc<Weeb3>,
        cursor: HlsMediaCursor,
        ticket: PrefetchTicket,
        foreground_ready: mpsc::Receiver<bool>,
    ) -> bool {
        let Some(now) = hls_monotonic_now_ms() else {
            return false;
        };
        let start = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            if session.stamp() != ticket.stamp
                || session.timeline_rebasing
                || session.mode == HlsPrefetchMode::Inactive
                || !session
                    .tracks
                    .get(&ticket.plan_id)
                    .is_some_and(|track| track.schedule_id == ticket.schedule_id)
                || (session.mode != HlsPrefetchMode::Sustained
                    && now >= session.startup_deadline_ms)
            {
                return false;
            }
            let Some(track) = session.tracks.get_mut(&ticket.plan_id) else {
                return false;
            };
            if track.running.is_some() {
                return false;
            }
            track.running = Some(ticket);
            true
        });
        if !start {
            return false;
        }

        spawn_local(async move {
            let restart_client = weeb3.clone();
            prefetch_hls_media_stages(weeb3, cursor.clone(), ticket, foreground_ready).await;
            let restart = HLS_PLAYBACK.with(|playback| {
                let mut playback = playback.borrow_mut();
                let session = &mut playback.session;
                let current = session.mode == HlsPrefetchMode::Sustained
                    && session.stamp() == ticket.stamp
                    && !session.timeline_rebasing;
                let track = session.tracks.get_mut(&ticket.plan_id)?;
                if track.running != Some(ticket) {
                    return None;
                }
                track.running = None;
                (current
                    && track.last_foreground_position > cursor.position
                    && track.last_foreground_position.saturating_add(1)
                        < cursor.plan.references.len())
                .then_some(track.last_foreground_position)
            });
            if let Some(position) = restart {
                let mut cursor = cursor;
                cursor.position = position;
                let (ready_out, ready_in) = mpsc::bounded(1);
                let _ = ready_out.try_send(true);
                spawn_hls_prefetch_stages(restart_client, cursor, ticket, ready_in);
            }
        });
        true
    }

    async fn hls_payload_size_for_prefetch(
        weeb3: Arc<Weeb3>,
        reference: String,
        ticket: PrefetchTicket,
    ) -> Option<u64> {
        for attempt in 0..HLS_PREFETCH_MAX_ATTEMPTS {
            if let Some(size) = cached_hls_payload_size(&reference) {
                return Some(size);
            }
            if !hls_prefetch_ticket_current(ticket, false) {
                return None;
            }
            let size = start_hls_payload_size_probe(weeb3.clone(), reference.clone())
                .recv()
                .await
                .ok()
                .flatten();
            if !hls_prefetch_ticket_current(ticket, false) || size.is_some() {
                return size;
            }
            if attempt + 1 < HLS_PREFETCH_MAX_ATTEMPTS {
                async_std::task::sleep(Duration::from_millis(
                    HLS_PAYLOAD_SIZE_RETRY_DELAY_MS * (attempt + 1) as u64,
                ))
                .await;
            }
        }
        None
    }

    type HlsPrefetchProbeResult = (usize, Option<u64>);
    type HlsPrefetchBodyResult = (usize, Result<Arc<[u8]>, String>);

    enum HlsPrefetchWork {
        Probe(HlsPrefetchProbeResult),
        Body(HlsPrefetchBodyResult),
    }

    struct PrefetchRun {
        ticket: PrefetchTicket,
        cursor: HlsMediaCursor,
        next_probe: usize,
        next_admit: usize,
        ready_sizes: BTreeMap<usize, Option<u64>>,
        planned_bytes: u64,
        startup: bool,
    }

    impl PrefetchRun {
        fn probe_positions(&mut self) -> std::ops::Range<usize> {
            let end = self
                .cursor
                .plan
                .references
                .len()
                .min(
                    self.next_admit
                        .saturating_add(HLS_PREFETCH_PROBE_MAX_PARALLEL),
                )
                .max(self.next_probe);
            let positions = self.next_probe..end;
            self.next_probe = end;
            positions
        }

        fn ready_size(&self) -> Option<(usize, Option<u64>)> {
            self.ready_sizes
                .get(&self.next_admit)
                .copied()
                .map(|size| (self.next_admit, size))
        }

        fn commit_size(&mut self) {
            self.ready_sizes.remove(&self.next_admit);
            self.next_admit = self.next_admit.saturating_add(1);
        }
    }

    async fn next_hls_prefetch_work<ProbeFuture, BodyFuture>(
        probes: &mut FuturesUnordered<ProbeFuture>,
        bodies: &mut FuturesUnordered<BodyFuture>,
    ) -> Option<HlsPrefetchWork>
    where
        ProbeFuture: Future<Output = HlsPrefetchProbeResult>,
        BodyFuture: Future<Output = HlsPrefetchBodyResult>,
    {
        match (probes.is_empty(), bodies.is_empty()) {
            (false, false) => {
                match select(Box::pin(probes.next()), Box::pin(bodies.next())).await {
                    Either::Left((result, _)) => result.map(HlsPrefetchWork::Probe),
                    Either::Right((result, _)) => result.map(HlsPrefetchWork::Body),
                }
            }
            (false, true) => probes.next().await.map(HlsPrefetchWork::Probe),
            (true, false) => bodies.next().await.map(HlsPrefetchWork::Body),
            (true, true) => None,
        }
    }

    async fn wait_hls_prefetch_load_with_retry(
        weeb3: Arc<Weeb3>,
        reference: String,
        role: HlsPayloadLoadRole,
        ticket: PrefetchTicket,
        position: usize,
    ) -> HlsPrefetchBodyResult {
        let mut result = wait_hls_payload_load(role).await;
        let mut attempts = 1;
        while result.is_err()
            && attempts < HLS_PREFETCH_MAX_ATTEMPTS
            && hls_prefetch_ticket_current(ticket, false)
        {
            async_std::task::sleep(Duration::from_millis(
                HLS_PAYLOAD_RETRY_DELAY_MS * attempts as u64,
            ))
            .await;
            if !hls_prefetch_ticket_current(ticket, false) {
                break;
            }
            let retry = start_hls_payload_load(
                weeb3.clone(),
                reference.clone(),
                true,
                ticket.stamp.generation,
            );
            if matches!(retry, HlsPayloadLoadRole::AtCapacity) {
                continue;
            }
            result = wait_hls_payload_load(retry).await;
            attempts += 1;
        }
        (position, result)
    }

    async fn prefetch_hls_media_stages(
        weeb3: Arc<Weeb3>,
        cursor: HlsMediaCursor,
        ticket: PrefetchTicket,
        foreground_ready: mpsc::Receiver<bool>,
    ) {
        let ahead_limit = media_prefetch_ahead_limit_bytes(hls_payload_cache_capacity_bytes());
        let startup_limit = HLS_STARTUP_LOOKAHEAD_BYTES.min(ahead_limit);
        let startup_bodies = HLS_STARTUP_BODY_MAX_PARALLEL.min(cursor.plan.early_overlap_limit);
        let first_position = cursor.position.saturating_add(1);
        let mut run = PrefetchRun {
            ticket,
            cursor,
            next_probe: first_position,
            next_admit: first_position,
            ready_sizes: BTreeMap::new(),
            planned_bytes: 0,
            startup: true,
        };
        let mut size_probes = FuturesUnordered::new();
        let mut bodies = FuturesUnordered::new();
        let mut budget_blocked = false;

        loop {
            if !hls_prefetch_ticket_current(run.ticket, !run.startup) {
                return;
            }
            let byte_limit = if run.startup {
                startup_limit
            } else {
                ahead_limit
            };
            let body_limit = if run.startup {
                startup_bodies
            } else {
                HLS_PREFETCH_BODY_MAX_PARALLEL
            };

            if run.planned_bytes >= byte_limit {
                if run.startup {
                    if foreground_ready.recv().await != Ok(true) {
                        return;
                    }
                    run.startup = false;
                    budget_blocked = false;
                    continue;
                }
                loop {
                    let completed: Option<HlsPrefetchBodyResult> = bodies.next().await;
                    let Some((_, result)) = completed else {
                        break;
                    };
                    if !hls_prefetch_ticket_current(run.ticket, true) || result.is_err() {
                        return;
                    }
                }
                return;
            }

            if !budget_blocked {
                for position in run.probe_positions() {
                    let client = weeb3.clone();
                    let reference = run.cursor.plan.references[position].clone();
                    let ticket = run.ticket;
                    size_probes.push(async move {
                        (
                            position,
                            hls_payload_size_for_prefetch(client, reference, ticket).await,
                        )
                    });
                }
            }

            let mut capacity_blocked = false;
            while !budget_blocked && bodies.len() < body_limit && run.planned_bytes < byte_limit {
                let Some((position, size)) = run.ready_size() else {
                    break;
                };
                let Some(size) = size else {
                    run.commit_size();
                    return;
                };
                let batch =
                    plan_media_prefetch_batch(run.planned_bytes, byte_limit, ahead_limit, &[size]);
                if batch.unit_count == 0 {
                    budget_blocked = true;
                    break;
                }
                let reference = run.cursor.plan.references[position].clone();
                let role = start_hls_payload_load(
                    weeb3.clone(),
                    reference.clone(),
                    true,
                    run.ticket.stamp.generation,
                );
                if matches!(role, HlsPayloadLoadRole::AtCapacity) {
                    capacity_blocked = true;
                    break;
                }
                let stagger = !run.startup && matches!(role, HlsPayloadLoadRole::Lead(_, _));
                run.commit_size();
                bodies.push(wait_hls_prefetch_load_with_retry(
                    weeb3.clone(),
                    reference,
                    role,
                    run.ticket,
                    position,
                ));
                run.planned_bytes = batch.planned_end_bytes;
                if stagger {
                    async_std::task::sleep(HLS_NEXT_RESERVE_STAGGER).await;
                    if !hls_prefetch_ticket_current(run.ticket, true) {
                        return;
                    }
                }
            }

            if capacity_blocked {
                if run.startup {
                    match select(
                        Box::pin(foreground_ready.recv()),
                        Box::pin(async_std::task::sleep(Duration::from_millis(
                            MEDIA_PREFETCH_BATCH_YIELD_MS,
                        ))),
                    )
                    .await
                    {
                        Either::Left((ready, _)) => {
                            if ready != Ok(true) {
                                return;
                            }
                            run.startup = false;
                            budget_blocked = false;
                        }
                        Either::Right(_) => {}
                    }
                } else {
                    async_std::task::sleep(Duration::from_millis(MEDIA_PREFETCH_BATCH_YIELD_MS))
                        .await;
                }
                continue;
            }

            if budget_blocked && !run.startup {
                loop {
                    let completed: Option<HlsPrefetchBodyResult> = bodies.next().await;
                    let Some((_, result)) = completed else {
                        break;
                    };
                    if !hls_prefetch_ticket_current(run.ticket, true) || result.is_err() {
                        return;
                    }
                }
                return;
            }

            let work = if run.startup {
                if size_probes.is_empty() && bodies.is_empty() {
                    if foreground_ready.recv().await != Ok(true) {
                        return;
                    }
                    run.startup = false;
                    budget_blocked = false;
                    continue;
                }
                match select(
                    Box::pin(foreground_ready.recv()),
                    Box::pin(next_hls_prefetch_work(&mut size_probes, &mut bodies)),
                )
                .await
                {
                    Either::Left((ready, _)) => {
                        if ready != Ok(true) {
                            return;
                        }
                        run.startup = false;
                        budget_blocked = false;
                        continue;
                    }
                    Either::Right((work, _)) => work,
                }
            } else {
                next_hls_prefetch_work(&mut size_probes, &mut bodies).await
            };

            match work {
                Some(HlsPrefetchWork::Probe((position, size))) => {
                    if position >= run.next_admit && position < run.next_probe {
                        run.ready_sizes.insert(position, size);
                    }
                }
                Some(HlsPrefetchWork::Body((_, Ok(_)))) => {}
                Some(HlsPrefetchWork::Body((_, Err(_)))) | None => return,
            }
        }
    }

    async fn retrieve_hls_payload_for_playback(
        weeb3: Arc<Weeb3>,
        reference: String,
    ) -> Option<Arc<[u8]>> {
        let reference = reference.to_ascii_lowercase();
        let cached = HLS_ASSET_CACHE.with(|cache| cache.borrow().bodies.contains_key(&reference));
        let context = hls_foreground_context(&reference, cached);
        let head_ready = cached;
        let foreground = start_hls_payload_load(
            weeb3.clone(),
            reference.clone(),
            false,
            context.stamp.generation,
        );

        if let (Some(ticket), Some(cursor)) = (context.ticket, context.cursor.as_ref())
            && let Some(successors) = {
                let successors = cursor
                    .plan
                    .references
                    .iter()
                    .skip(cursor.position.saturating_add(1))
                    .take(
                        cursor
                            .plan
                            .early_overlap_limit
                            .min(HLS_EXACT_NEXT_OVERLAP_SEGMENTS),
                    )
                    .cloned()
                    .collect::<Vec<_>>();
                (!successors.is_empty()).then_some(successors)
            }
            && claim_hls_exact_next_overlap(ticket, cursor)
        {
            let client = weeb3.clone();
            spawn_local(async move {
                let started = hls_monotonic_now_ms();
                let first_delay = if head_ready {
                    0
                } else {
                    HLS_EXACT_NEXT_HEAD_START.as_millis() as u64
                };
                let stagger = HLS_NEXT_RESERVE_STAGGER.as_millis() as u64;
                let retry_limit = (HLS_EXACT_OVERLAP_ADMISSION_BUDGET.as_millis() as u64)
                    / MEDIA_PREFETCH_BATCH_YIELD_MS.max(1);
                let mut retries = 0_u64;
                for (offset, successor) in successors.into_iter().enumerate() {
                    let due = first_delay.saturating_add(stagger.saturating_mul(offset as u64));
                    let remaining = started
                        .zip(hls_monotonic_now_ms())
                        .map(|(start, now)| (due as f64 - (now - start)).max(0.0).ceil() as u64)
                        .unwrap_or(due);
                    if remaining > 0 {
                        async_std::task::sleep(Duration::from_millis(remaining)).await;
                    }
                    loop {
                        if !hls_playback_prefetch_admission_is_current(ticket) {
                            return;
                        }
                        match start_hls_payload_load(
                            client.clone(),
                            successor.clone(),
                            true,
                            ticket.stamp.generation,
                        ) {
                            HlsPayloadLoadRole::AtCapacity if retries < retry_limit => {
                                retries += 1;
                                async_std::task::sleep(Duration::from_millis(
                                    MEDIA_PREFETCH_BATCH_YIELD_MS,
                                ))
                                .await;
                            }
                            HlsPayloadLoadRole::AtCapacity | HlsPayloadLoadRole::Reject(_) => {
                                return;
                            }
                            role => {
                                drop(role);
                                break;
                            }
                        }
                    }
                }
            });
        }

        let (ready_out, ready_in) = mpsc::bounded(1);
        if let (Some(ticket), Some(cursor)) = (context.ticket, context.cursor.clone()) {
            if head_ready {
                spawn_hls_prefetch_stages(weeb3.clone(), cursor, ticket, ready_in);
            } else {
                let client = weeb3.clone();
                spawn_local(async move {
                    async_std::task::sleep(HLS_EXACT_NEXT_HEAD_START).await;
                    spawn_hls_prefetch_stages(client, cursor, ticket, ready_in);
                });
            }
        }

        let mut body = wait_hls_payload_load(foreground).await;
        let mut attempts = 1;
        while body.is_err()
            && attempts < HLS_FOREGROUND_MAX_ATTEMPTS
            && hls_foreground_retry_is_current(context.stamp)
        {
            async_std::task::sleep(Duration::from_millis(
                HLS_PAYLOAD_RETRY_DELAY_MS * attempts as u64,
            ))
            .await;
            if !hls_foreground_retry_is_current(context.stamp) {
                break;
            }
            body = wait_hls_payload_load(start_hls_payload_load(
                weeb3.clone(),
                reference.clone(),
                false,
                context.stamp.generation,
            ))
            .await;
            attempts += 1;
        }

        let succeeded = body.is_ok();
        if !succeeded && let Some(ticket) = context.ticket {
            retire_hls_prefetch_plan(ticket);
        }
        if succeeded
            && hls_foreground_retry_is_current(context.stamp)
            && let Some(successor) = context.seek_successor
        {
            let successor =
                start_hls_payload_load(weeb3.clone(), successor, true, context.stamp.generation);
            let _ = wait_hls_payload_load(successor).await;
        }
        let _ = ready_out.try_send(succeeded);
        if succeeded && let (Some(ticket), Some(cursor)) = (context.ticket, context.cursor) {
            let (completed_out, completed_in) = mpsc::bounded(1);
            let _ = completed_out.try_send(true);
            spawn_hls_prefetch_stages(weeb3, cursor, ticket, completed_in);
        }
        body.ok()
    }

    async fn resolve_hls_asset(weeb3: Arc<Weeb3>, reference: String) -> Option<ResolvedHlsAsset> {
        let reference = reference.to_ascii_lowercase();
        if let Some(metadata) =
            HLS_ASSET_CACHE.with(|cache| cache.borrow_mut().metadata(&reference))
        {
            return Some(ResolvedHlsAsset {
                metadata,
                prefetched_body: cached_hls_payload(&reference),
            });
        }
        if let Some(body) = cached_hls_payload(&reference) {
            let payload_size = u64::try_from(body.len()).ok()?;
            let is_manifest = is_hls_manifest(&body);
            let metadata = HlsAssetMetadata {
                payload_size,
                mime: hls_payload_mime(&body),
                is_manifest,
            };
            HLS_ASSET_CACHE.with(|cache| {
                cache
                    .borrow_mut()
                    .remember_metadata(&reference, metadata.clone());
            });
            return Some(ResolvedHlsAsset {
                metadata,
                prefetched_body: Some(body),
            });
        }

        let payload_size = match cached_hls_payload_size(&reference) {
            Some(size) => size,
            None => start_hls_payload_size_probe(weeb3.clone(), reference.clone())
                .recv()
                .await
                .ok()
                .flatten()?,
        };
        if payload_size == 0 {
            let metadata = HlsAssetMetadata {
                payload_size,
                mime: "application/octet-stream",
                is_manifest: false,
            };
            HLS_ASSET_CACHE.with(|cache| {
                cache
                    .borrow_mut()
                    .remember_metadata(&reference, metadata.clone());
            });
            return Some(ResolvedHlsAsset {
                metadata,
                prefetched_body: None,
            });
        }
        let probe_end = payload_size
            .saturating_sub(1)
            .min(HLS_ASSET_PROBE_BYTES.saturating_sub(1));
        let probe = weeb3
            .retrieve_hls_payload_range(reference.clone(), payload_size, 0, probe_end, None, None)
            .await;
        let expected_probe_len = usize::try_from(probe_end.saturating_add(1)).ok()?;
        if probe.len() != expected_probe_len {
            return None;
        }

        let mut prefetched_body = None;
        let is_manifest =
            if payload_size > MAX_STREAM_FEED_PAYLOAD_BYTES as u64 && is_hls_manifest(&probe) {
                true
            } else {
                match probe_hls_manifest(&probe, payload_size) {
                    HlsManifestProbe::Manifest => true,
                    HlsManifestProbe::NotManifest => false,
                    HlsManifestProbe::NeedMore => {
                        let body = weeb3.retrieve_hls_payload(reference.clone()).await;
                        if u64::try_from(body.len()).ok() != Some(payload_size) {
                            return None;
                        }
                        let is_manifest = is_hls_manifest(&body);
                        prefetched_body = Some(Arc::from(body));
                        is_manifest
                    }
                }
            };
        let mime = if is_manifest {
            "application/vnd.apple.mpegurl"
        } else {
            hls_payload_mime(prefetched_body.as_deref().unwrap_or(&probe))
        };
        let metadata = HlsAssetMetadata {
            payload_size,
            mime,
            is_manifest,
        };
        HLS_ASSET_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .remember_metadata(&reference, metadata.clone());
        });
        Some(ResolvedHlsAsset {
            metadata,
            prefetched_body,
        })
    }

    fn hls_codec_bootstrap_manifest(token: u64) -> HlsCodecBootstrapManifest {
        HLS_CODEC_BOOTSTRAP_PRESENTATION.with(|presentation| {
            let mut presentation = presentation.borrow_mut();
            if presentation
                .as_ref()
                .is_none_or(|current| current.token != token)
            {
                *presentation = Some(HlsCodecBootstrapPresentation {
                    token,
                    phase: HlsCodecBootstrapPhase::Bootstrap,
                    snapshot: None,
                });
            }
            match presentation.as_ref().map(|current| current.phase) {
                Some(HlsCodecBootstrapPhase::ContinuationOpen) => {
                    HlsCodecBootstrapManifest::Continuation(
                        HlsCodecBootstrapPhase::ContinuationOpen,
                    )
                }
                Some(HlsCodecBootstrapPhase::Settled) => {
                    HlsCodecBootstrapManifest::Continuation(HlsCodecBootstrapPhase::Settled)
                }
                Some(HlsCodecBootstrapPhase::Bootstrap) | None => {
                    HlsCodecBootstrapManifest::Bootstrap(token)
                }
            }
        })
    }

    pub(super) fn finish_hls_codec_bootstrap(source: &str) {
        let Some(token) = hls_codec_bootstrap_token(source) else {
            return;
        };
        HLS_CODEC_BOOTSTRAP_PRESENTATION.with(|presentation| {
            if let Some(current) = presentation.borrow_mut().as_mut()
                && current.token == token
                && current.phase == HlsCodecBootstrapPhase::Bootstrap
            {
                current.phase = HlsCodecBootstrapPhase::ContinuationOpen;
                current.snapshot = None;
            }
        });
    }

    pub(super) fn settle_hls_codec_continuation(source: &str) -> bool {
        let Some(token) = hls_codec_bootstrap_token(source) else {
            return false;
        };
        HLS_CODEC_BOOTSTRAP_PRESENTATION.with(|presentation| {
            let mut presentation = presentation.borrow_mut();
            let Some(current) = presentation.as_mut() else {
                return false;
            };
            if current.token != token || current.phase != HlsCodecBootstrapPhase::ContinuationOpen {
                return false;
            }
            current.phase = HlsCodecBootstrapPhase::Settled;
            true
        })
    }

    fn reset_hls_codec_bootstrap() {
        HLS_CODEC_BOOTSTRAP_PRESENTATION.with(|presentation| {
            presentation.borrow_mut().take();
        });
    }

    async fn fetch_feed_response(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        index_hint: Option<u64>,
        start: HlsStart,
        codec_bootstrap: Option<HlsCodecBootstrapManifest>,
        method: String,
        local_bytes_base: String,
    ) -> FetchResponse {
        let cached_bootstrap = match codec_bootstrap {
            Some(HlsCodecBootstrapManifest::Bootstrap(token)) => HLS_CODEC_BOOTSTRAP_PRESENTATION
                .with(|presentation| {
                    presentation
                        .borrow()
                        .as_ref()
                        .filter(|current| {
                            current.token == token
                                && current.phase == HlsCodecBootstrapPhase::Bootstrap
                        })
                        .and_then(|current| current.snapshot.clone())
                }),
            _ => None,
        };
        let snapshot = match cached_bootstrap.clone() {
            Some(snapshot) => snapshot,
            None => match load_feed_snapshot(
                weeb3.clone(),
                owner.clone(),
                topic.clone(),
                index_hint,
                start,
                None,
                false,
                None,
            )
            .await
            {
                Some(snapshot) => snapshot,
                None => return FetchResponse::error(503, "weeb-3 did not retrieve feed update"),
            },
        };
        let snapshot =
            if cached_bootstrap.is_none() && index_hint.is_none() && start == HlsStart::Live {
                apply_hls_live_tail_fallback(&owner, &topic, snapshot)
            } else {
                snapshot
            };

        if !is_hls_manifest(&snapshot.body) {
            return FetchResponse::error(502, "feed update is not an HLS manifest");
        }
        let sequence_zero_bootstrap = matches!(
            codec_bootstrap,
            Some(HlsCodecBootstrapManifest::Bootstrap(_))
        ) && hls_media_sequence(&snapshot.body) == Some(0);
        let presentation = match codec_bootstrap {
            Some(HlsCodecBootstrapManifest::Bootstrap(_)) if cached_bootstrap.is_some() => None,
            Some(HlsCodecBootstrapManifest::Bootstrap(token)) => {
                let presentation = if hls_media_sequence(&snapshot.body) == Some(0) {
                    rewrite_hls_sequence_zero_codec_bootstrap(&snapshot.body, true)
                } else {
                    let mut bootstrap = None;
                    for index in [HLS_EARLY_FEED_PREFIX_INDEX, 0] {
                        if let Some(prefix) = weeb3
                            .hls_feed_payload_at_index_bounded(owner.clone(), topic.clone(), index)
                            .await
                            .filter(|prefix| hls_media_sequence(&prefix.bytes) == Some(0))
                        {
                            bootstrap = Some(prefix.bytes);
                            break;
                        }
                    }
                    let bootstrap = match bootstrap {
                        Some(bootstrap) => bootstrap,
                        None => {
                            let Some(bootstrap_index) = snapshot.index.checked_sub(1) else {
                                return FetchResponse::error(
                                    503,
                                    "HLS codec bootstrap is not available",
                                );
                            };
                            let Some(bootstrap) = load_feed_snapshot(
                                weeb3.clone(),
                                owner.clone(),
                                topic.clone(),
                                Some(bootstrap_index),
                                HlsStart::Beginning,
                                None,
                                false,
                                None,
                            )
                            .await
                            else {
                                return FetchResponse::error(
                                    503,
                                    "HLS codec bootstrap is not available",
                                );
                            };
                            bootstrap.body.to_vec()
                        }
                    };
                    prepend_hls_codec_bootstrap(&snapshot.body, &bootstrap)
                };
                let Some(presentation) = presentation else {
                    return FetchResponse::error(502, "HLS codec bootstrap is not supported");
                };
                HLS_CODEC_BOOTSTRAP_PRESENTATION.with(|state| {
                    if let Some(current) = state.borrow_mut().as_mut()
                        && current.token == token
                        && current.phase == HlsCodecBootstrapPhase::Bootstrap
                    {
                        current.snapshot = Some(FeedRouteSnapshot {
                            body: Arc::from(presentation.clone()),
                            ..snapshot.clone()
                        });
                    }
                });
                Some(presentation)
            }
            Some(HlsCodecBootstrapManifest::Continuation(_)) => {
                let Some(presentation) = continue_hls_codec_bootstrap(&snapshot.body) else {
                    return FetchResponse::error(502, "invalid HLS codec continuation");
                };
                Some(presentation)
            }
            None => None,
        };
        let body = presentation.as_deref().unwrap_or(&snapshot.body);
        let rewritten = if index_hint.is_none() {
            rewrite_hls_manifest_for_live_reload(body, &local_bytes_base, snapshot.finalized, start)
        } else {
            rewrite_hls_manifest(body, &local_bytes_base)
        };
        let Some(mut body) = rewritten else {
            return FetchResponse::error(502, "invalid HLS manifest");
        };
        if matches!(
            codec_bootstrap,
            Some(HlsCodecBootstrapManifest::Continuation(
                HlsCodecBootstrapPhase::ContinuationOpen
            ))
        ) {
            let Some(open) = open_hls_codec_continuation(&body) else {
                return FetchResponse::error(502, "invalid open HLS codec continuation");
            };
            body = open;
        }
        remember_hls_media_plan(&body);
        if sequence_zero_bootstrap
            && let Some(reference) = hls_media_references(&body).into_iter().next()
            && let Some(stamp) = hls_prefix_stamp_for_feed(&weeb3, &owner, &topic)
            && !hls_beginning_prefix_manages_reference(&reference, stamp)
        {
            start_hls_shared_prefix_warmup(weeb3.clone(), reference, stamp, 2);
        }

        let headers = vec![
            (
                "Content-Type".to_string(),
                "application/vnd.apple.mpegurl".to_string(),
            ),
            ("Cache-Control".to_string(), "no-store".to_string()),
            (
                "Swarm-Feed-Index".to_string(),
                format!("{:016x}", snapshot.index),
            ),
            (
                "Swarm-Feed-Index-Next".to_string(),
                format!("{:016x}", snapshot.index.saturating_add(1)),
            ),
            (
                "Access-Control-Expose-Headers".to_string(),
                "Swarm-Feed-Index, Swarm-Feed-Index-Next".to_string(),
            ),
            ("Accept-Ranges".to_string(), "none".to_string()),
            ("Content-Length".to_string(), body.len().to_string()),
        ];

        if method == "HEAD" {
            return FetchResponse::ok(200, headers, None);
        }

        FetchResponse::ok(200, headers, Some(body))
    }

    async fn load_persisted_vod_payload(
        weeb3: &Arc<Weeb3>,
        network_id: u64,
        owner: &str,
        topic: &str,
        index: u64,
        require_sequence_zero: bool,
    ) -> Option<crate::bzz_stream::RawFeedPayload> {
        let candidate = weeb3
            .hls_feed_payload_at_index(owner.to_string(), topic.to_string(), index)
            .await;
        match candidate {
            Some(candidate)
                if candidate.index == index
                    && candidate.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                    && is_hls_manifest(&candidate.bytes)
                    && hls_is_finalized(&candidate.bytes) =>
            {
                (!require_sequence_zero || hls_media_sequence(&candidate.bytes) == Some(0))
                    .then_some(candidate)
            }
            Some(_) => {
                forget_vod_index(network_id, owner, topic);
                None
            }
            None => None,
        }
    }

    fn prefetch_live_snapshot_start(
        weeb3: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        snapshot: &FeedRouteSnapshot,
    ) {
        let Some(stamp) = hls_prefix_stamp_for_feed(weeb3, owner, topic) else {
            return;
        };
        if !HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.live_start && session.stamp() == stamp
        }) {
            return;
        }
        let references = hls_media_references(&snapshot.body);
        let start = hls_live_tail(&snapshot.body)
            .map(|(start, _)| start)
            .unwrap_or_default();
        for reference in references
            .iter()
            .skip(start)
            .take(HLS_PREFETCH_BODY_MAX_PARALLEL)
        {
            start_hls_shared_prefix_warmup(
                weeb3.clone(),
                reference.clone(),
                stamp,
                HLS_LIVE_PREFIX_WINDOW_COUNT,
            );
        }
        if start > 0
            && hls_media_sequence(&snapshot.body) == Some(0)
            && let Some(reference) = references.first()
        {
            start_hls_shared_prefix_warmup(weeb3.clone(), reference.clone(), stamp, 2);
        }
    }

    async fn await_live_frontier_snapshot(
        cache_key: &str,
        checked_after_ms: f64,
        deadline_ms: f64,
        missing_is_terminal: bool,
    ) -> Option<FeedRouteSnapshot> {
        loop {
            let state = FEED_ROUTE_CACHE.with(|cache| {
                cache.borrow().get(cache_key).map(|state| {
                    (
                        state.snapshot.clone(),
                        state.confirmed_head_index,
                        state.last_head_check,
                    )
                })
            });
            let now = js_sys::Date::now();
            if let Some((snapshot, confirmed_head_index, last_head_check)) = state {
                if hls_live_frontier_is_ready(
                    snapshot.index,
                    confirmed_head_index,
                    last_head_check,
                    checked_after_ms,
                ) || now >= deadline_ms
                {
                    return Some(snapshot);
                }
            } else if missing_is_terminal || now >= deadline_ms {
                return None;
            }
            async_std::task::sleep(Duration::from_millis(15)).await;
        }
    }

    fn hls_beginning_prefix_manages_reference(reference: &str, stamp: PlaybackStamp) -> bool {
        let reference = reference.to_ascii_lowercase();
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            let gate = &session.beginning_prefix;
            session.stamp() == stamp
                && gate.stamp == stamp
                && gate.reference.as_deref() == Some(reference.as_str())
                && matches!(
                    gate.phase,
                    HlsBeginningPrefixPhase::AwaitForegroundZero
                        | HlsBeginningPrefixPhase::Supplying
                        | HlsBeginningPrefixPhase::Ready
                )
        })
    }

    fn start_hls_shared_prefix_warmup(
        weeb3: Arc<Weeb3>,
        reference: String,
        stamp: PlaybackStamp,
        window_count: usize,
    ) {
        let size = start_hls_payload_size_probe(weeb3.clone(), reference.clone());
        spawn_local(async move {
            let Some(size) = size.recv().await.ok().flatten().filter(|size| *size > 0) else {
                return;
            };
            let Some(metadata) = hls_range_metadata(&reference, size) else {
                return;
            };
            let mut position = 0_u64;
            for _ in 0..window_count {
                if position >= size || !hls_prefix_stamp_is_current(stamp) {
                    return;
                }
                let end = position
                    .saturating_add(MEDIA_STORAGE_WINDOW_BYTES)
                    .saturating_sub(1)
                    .min(size.saturating_sub(1));
                if hls_aligned_range_cached(&reference, &metadata, position, end) {
                    position = end.saturating_add(1);
                    continue;
                }
                let Some(expected) = end
                    .checked_sub(position)
                    .and_then(|length| length.checked_add(1))
                else {
                    return;
                };
                let lease = match try_hls_background_range_lease(expected) {
                    HlsProgressiveRangeLeaseAttempt::Acquired(lease) => lease,
                    HlsProgressiveRangeLeaseAttempt::Busy
                    | HlsProgressiveRangeLeaseAttempt::Budget
                    | HlsProgressiveRangeLeaseAttempt::Park
                    | HlsProgressiveRangeLeaseAttempt::Retire => return,
                };
                if !hls_prefix_stamp_is_current(stamp) {
                    drop(lease);
                    return;
                }
                if hls_aligned_range_cached(&reference, &metadata, position, end) {
                    drop(lease);
                    position = end.saturating_add(1);
                    continue;
                }
                let background = HlsBackgroundRangeRequest::new(lease, move || {
                    hls_prefix_stamp_is_current(stamp)
                });
                let bytes = weeb3
                    .retrieve_hls_payload_range(
                        reference.clone(),
                        size,
                        position,
                        end,
                        Some(stamp.generation),
                        Some(background),
                    )
                    .await;
                if usize::try_from(expected).ok() != Some(bytes.len()) {
                    return;
                }
                position = end.saturating_add(1);
            }
        });
    }

    fn hls_beginning_prefix_window_bounds(payload_size: u64, window: usize) -> Option<(u64, u64)> {
        let start = u64::try_from(window)
            .ok()?
            .checked_mul(MEDIA_STORAGE_WINDOW_BYTES)?;
        if start >= payload_size {
            return None;
        }
        Some((
            start,
            start
                .saturating_add(MEDIA_STORAGE_WINDOW_BYTES)
                .saturating_sub(1)
                .min(payload_size.saturating_sub(1)),
        ))
    }

    fn start_beginning_snapshot_runway(
        weeb3: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        snapshot: &FeedRouteSnapshot,
    ) {
        let Some(stamp) = hls_prefix_stamp_for_feed(weeb3, owner, topic) else {
            return;
        };
        if hls_media_sequence(&snapshot.body) != Some(0) {
            bypass_hls_beginning_prefix(stamp);
            return;
        }
        let Some(mut segments) = hls_segment_identities(&snapshot.body) else {
            bypass_hls_beginning_prefix(stamp);
            return;
        };
        segments.retain(|segment| !segment.gap);
        let mut segments = segments.into_iter();
        let Some(first) = segments.next() else {
            bypass_hls_beginning_prefix(stamp);
            return;
        };
        let managed_prefix_eligible = first.byte_range.is_none();
        let reference = first.reference;
        let duration_seconds = f64::from_bits(first.duration_bits);
        let successor = segments.next().map(|segment| segment.reference);
        if !managed_prefix_eligible {
            bypass_hls_beginning_prefix(stamp);
        } else if !configure_hls_beginning_prefix_manifest(stamp, &reference, duration_seconds) {
            bypass_hls_beginning_prefix(stamp);
            return;
        }
        let installed = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            if session.stamp() != stamp
                || session.live_start
                || session.timeline_rebasing
                || session.sequence_zero_runway_closed
            {
                return false;
            }
            session
                .runways
                .set_startup(HlsProgressiveRunway::new(reference.clone(), successor));
            true
        });
        if !installed {
            return;
        }
        if !managed_prefix_eligible {
            return;
        }

        let prefix_client = weeb3.clone();
        let size = start_hls_payload_size_probe(prefix_client.clone(), reference.clone());
        spawn_local(async move {
            let Some(size) = size.recv().await.ok().flatten().filter(|size| *size > 0) else {
                bypass_hls_beginning_prefix(stamp);
                return;
            };
            if !hls_progressive_startup_admission_is_current(&reference, stamp) {
                return;
            }
            if ensure_hls_beginning_prefix_configured(prefix_client, &reference, stamp, size)
                .is_none()
            {
                bypass_hls_beginning_prefix(stamp);
            }
        });
    }

    async fn load_feed_snapshot(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        index_hint: Option<u64>,
        start: HlsStart,
        live_frontier_deadline_ms: Option<f64>,
        defer_live_followup: bool,
        startup_deferred_out: Option<mpsc::Sender<DeferredRawFeedPayload>>,
    ) -> Option<FeedRouteSnapshot> {
        let wait_for_live_frontier = live_frontier_deadline_ms.is_some();
        let live_frontier_deadline_ms = live_frontier_deadline_ms
            .unwrap_or_else(|| js_sys::Date::now() + HLS_LIVE_FRONTIER_MAX_WAIT.as_millis() as f64);
        let owner = owner
            .trim_start_matches("0x")
            .trim_start_matches("0X")
            .to_string();
        let topic = normalize_feed_topic(&topic);
        let active_presentation =
            hls_presentation_for_feed(&weeb3, &owner, &topic).filter(|_| index_hint.is_none());
        let sequence_zero_presentation_id = active_presentation.and_then(|(presentation_id, _)| {
            (start == HlsStart::Beginning).then_some(presentation_id)
        });
        let live_history_presentation_id =
            active_presentation.and_then(|(presentation_id, history_active)| {
                (start == HlsStart::Live && history_active).then_some(presentation_id)
            });
        let sequence_zero_start_requested = sequence_zero_presentation_id.is_some();
        let canonical_cache_key = feed_cache_key(&owner, &topic, index_hint);
        let presentation_cache_id = sequence_zero_presentation_id.or(live_history_presentation_id);
        let cache_key = if let Some(presentation_id) = presentation_cache_id {
            sequence_zero_feed_cache_key(&owner, &topic, presentation_id)
        } else {
            canonical_cache_key.clone()
        };

        let cached = FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let selected_key = if cache.contains_key(&cache_key) {
                cache_key.clone()
            } else if sequence_zero_start_requested
                && cache
                    .get(&canonical_cache_key)
                    .is_some_and(|state| hls_media_sequence(&state.snapshot.body) == Some(0))
            {
                canonical_cache_key.clone()
            } else {
                return None;
            };
            let state = cache.get_mut(&selected_key)?;
            let now = js_sys::Date::now();
            let refresh_head =
                index_hint.is_none() && cached_feed_should_refresh_head(state.last_head_check, now);
            state.last_touch = now;
            Some((
                state.snapshot.clone(),
                refresh_head,
                selected_key,
                state.checking_token != 0,
                state.last_head_check,
            ))
        });

        if live_history_presentation_id.is_some() && cached.is_none() {
            return None;
        }

        if let Some((mut snapshot, refresh_head, cached_key, feed_task_running, last_head_check)) =
            cached
            && !(live_history_presentation_id.is_none()
                && start == HlsStart::Live
                && index_hint.is_none()
                && persisted_vod_index(active_profile().swarm_network_id, &owner, &topic)
                    .is_some_and(|index| index > snapshot.index))
        {
            let cached_is_sequence_zero_presentation =
                presentation_cache_id.is_some() && cached_key == cache_key;
            let cached_late_window = sequence_zero_start_requested
                && !cached_is_sequence_zero_presentation
                && hls_media_sequence(&snapshot.body).is_some_and(|sequence| sequence > 0);
            if index_hint.is_none() && !cached_late_window {
                let followup_mode = if cached_is_sequence_zero_presentation {
                    FeedFollowupMode::SequenceZeroPresentation
                } else {
                    FeedFollowupMode::Canonical
                };
                if !feed_task_running && !(defer_live_followup && start == HlsStart::Live) {
                    schedule_feed_followup(
                        weeb3.clone(),
                        cached_key.clone(),
                        owner.clone(),
                        topic.clone(),
                        refresh_head,
                        followup_mode,
                    );
                }
                if start == HlsStart::Live && !defer_live_followup {
                    if refresh_head && wait_for_live_frontier {
                        snapshot = await_live_frontier_snapshot(
                            &cached_key,
                            last_head_check,
                            live_frontier_deadline_ms,
                            false,
                        )
                        .await
                        .unwrap_or(snapshot);
                    }
                    prefetch_live_snapshot_start(&weeb3, &owner, &topic, &snapshot);
                }
                return Some(snapshot);
            }
            return Some(snapshot);
        }

        let network_id = active_profile().swarm_network_id;
        let mut authenticated_startup_prefix = None;
        let loaded = match index_hint {
            Some(index) => {
                let loaded = weeb3
                    .hls_feed_payload_at_index(owner.clone(), topic.clone(), index)
                    .await?;
                if loaded.index != index {
                    return None;
                }
                loaded
            }
            None => {
                let persisted_index = persisted_vod_index(network_id, &owner, &topic);
                let persisted = match persisted_index {
                    Some(index) if start == HlsStart::Live => {
                        let predecessor = match index.checked_sub(1) {
                            Some(previous_index) => weeb3
                                .hls_feed_payload_at_index_bounded(
                                    owner.clone(),
                                    topic.clone(),
                                    previous_index,
                                )
                                .await
                                .filter(|payload| {
                                    payload.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                                        && is_hls_manifest(&payload.bytes)
                                }),
                            None => None,
                        };
                        match predecessor {
                            Some(predecessor) => Some(predecessor),
                            None => {
                                load_persisted_vod_payload(
                                    &weeb3, network_id, &owner, &topic, index, false,
                                )
                                .await
                            }
                        }
                    }
                    Some(index) if !sequence_zero_start_requested => {
                        load_persisted_vod_payload(&weeb3, network_id, &owner, &topic, index, false)
                            .await
                    }
                    None => None,
                    Some(_) => None,
                };
                match persisted {
                    Some(loaded) => loaded,
                    None if !sequence_zero_start_requested => {
                        if start == HlsStart::Live && !defer_live_followup {
                            let still_current = async_std::future::timeout(
                                HLS_LIVE_FRONTIER_CONNECTION_WAIT,
                                async {
                                    loop {
                                        if weeb3.get_connections().await
                                            >= HLS_LIVE_FRONTIER_MIN_PRICED_PEERS
                                        {
                                            return true;
                                        }
                                        if hls_prefix_generation_for_feed(&weeb3, &owner, &topic)
                                            .is_none()
                                        {
                                            return false;
                                        }
                                        async_std::task::sleep(Duration::from_millis(15)).await;
                                    }
                                },
                            )
                            .await
                            .unwrap_or(true);
                            if !still_current {
                                return None;
                            }
                        }
                        if defer_live_followup && start == HlsStart::Live {
                            let resolved = weeb3
                                .latest_hls_feed_payload_startup_observing_deferred(
                                    owner.clone(),
                                    topic.clone(),
                                )
                                .await?;
                            if let (Some(output), Some(deferred)) =
                                (startup_deferred_out.as_ref(), resolved.observed_deferred)
                            {
                                let _ = output.try_send(deferred);
                            }
                            resolved.playable
                        } else {
                            weeb3
                                .latest_hls_feed_payload_startup(
                                    owner.clone(),
                                    topic.clone(),
                                    None,
                                    None,
                                )
                                .await?
                        }
                    }
                    None => {
                        let (early_payload_out, early_payload_in) =
                            mpsc::bounded::<crate::bzz_stream::RawFeedPayload>(16);
                        let (prefix_ready_out, prefix_ready_in) =
                            mpsc::bounded::<SequenceZeroStartupPrefix>(1);
                        let (deferred_out, deferred_in) =
                            mpsc::bounded::<DeferredRawFeedPayload>(1);
                        let prefix_admissions_open = Rc::new(Cell::new(true));
                        let best_prefix = Rc::new(RefCell::new(None));
                        spawn_local(fan_out_authenticated_hls_prefixes(
                            weeb3.clone(),
                            network_id,
                            early_payload_in,
                            best_prefix.clone(),
                            prefix_ready_out.clone(),
                            prefix_admissions_open.clone(),
                            sequence_zero_start_requested.then(|| cache_key.clone()),
                        ));
                        if sequence_zero_start_requested {
                            let Some(prefix_generation) =
                                hls_prefix_generation_for_feed(&weeb3, &owner, &topic)
                            else {
                                prefix_admissions_open.set(false);
                                return None;
                            };
                            spawn_local(forward_deferred_sequence_zero_prefix(
                                weeb3.clone(),
                                owner.clone(),
                                topic.clone(),
                                network_id,
                                prefix_generation,
                                deferred_in,
                                prefix_admissions_open.clone(),
                                prefix_ready_out.clone(),
                            ));
                            spawn_local(discover_sequence_zero_startup_prefix(
                                weeb3.clone(),
                                owner.clone(),
                                topic.clone(),
                                network_id,
                                prefix_generation,
                                prefix_admissions_open.clone(),
                                prefix_ready_out.clone(),
                            ));
                        }
                        // Dropping this listener cannot cancel the detached canonical frontier.
                        let (canonical_out, canonical_in) =
                            mpsc::bounded::<Option<crate::bzz_stream::RawFeedPayload>>(1);
                        let canonical_client = weeb3.clone();
                        let canonical_owner = owner.clone();
                        let canonical_topic = topic.clone();
                        let canonical_prefix_admissions_open = prefix_admissions_open.clone();
                        spawn_local(async move {
                            if sequence_zero_start_requested {
                                while canonical_client.get_connections().await == 0 {
                                    if hls_prefix_generation_for_feed(
                                        &canonical_client,
                                        &canonical_owner,
                                        &canonical_topic,
                                    )
                                    .is_none()
                                    {
                                        return;
                                    }
                                    async_std::task::sleep(Duration::from_millis(15)).await;
                                }
                                async_std::task::sleep(HLS_SEQUENCE_ZERO_CANONICAL_START_GRACE)
                                    .await;
                            }
                            let persisted = match persisted_index {
                                Some(index) => {
                                    load_persisted_vod_payload(
                                        &canonical_client,
                                        network_id,
                                        &canonical_owner,
                                        &canonical_topic,
                                        index,
                                        true,
                                    )
                                    .await
                                }
                                None => None,
                            };
                            let loaded = match persisted {
                                Some(payload) => Some(payload),
                                None => {
                                    let (early_payloads, early_payload_max_index) =
                                        if sequence_zero_start_requested {
                                            (
                                                Some(early_payload_out),
                                                Some(HLS_EARLY_FEED_PREFIX_INDEX),
                                            )
                                        } else {
                                            (None, None)
                                        };
                                    canonical_client
                                        .latest_hls_feed_payload_startup_observing_updates(
                                            canonical_owner,
                                            canonical_topic,
                                            early_payloads,
                                            early_payload_max_index,
                                            sequence_zero_start_requested.then_some(deferred_out),
                                        )
                                        .await
                                }
                            };
                            if loaded.as_ref().is_some_and(|payload| {
                                hls_sequence_zero_canonical_is_supported(&payload.bytes)
                                    && hls_media_sequence(&payload.bytes) == Some(0)
                            }) {
                                canonical_prefix_admissions_open.set(false);
                            }
                            let _ = canonical_out.try_send(loaded);
                        });

                        if sequence_zero_start_requested {
                            let prefix_select_in = prefix_ready_in.clone();
                            let canonical_select_in = canonical_in.clone();
                            let prefix_wait = Box::pin(prefix_select_in.recv());
                            let canonical_wait = Box::pin(canonical_select_in.recv());
                            match select(prefix_wait, canonical_wait).await {
                                Either::Left((Ok(prefix), _)) => {
                                    prefix_admissions_open.set(false);
                                    return Some(publish_sequence_zero_startup_snapshot(
                                        weeb3,
                                        cache_key,
                                        owner,
                                        topic,
                                        network_id,
                                        prefix,
                                        InitialCanonicalFeedResolution::Pending(canonical_in),
                                    ));
                                }
                                Either::Left((Err(_), _)) => {
                                    let canonical = canonical_in.recv().await.ok().flatten();
                                    prefix_admissions_open.set(false);
                                    canonical?
                                }
                                Either::Right((Ok(Some(canonical)), _)) => {
                                    if !hls_sequence_zero_canonical_is_supported(&canonical.bytes) {
                                        let prefix = match async_std::future::timeout(
                                            HLS_STARTUP_PREFIX_RESULT_GRACE,
                                            prefix_ready_in.recv(),
                                        )
                                        .await
                                        {
                                            Ok(Ok(prefix)) => prefix,
                                            _ => match best_prefix.borrow().clone() {
                                                Some(prefix) => {
                                                    SequenceZeroStartupPrefix::Complete(prefix)
                                                }
                                                None => {
                                                    prefix_admissions_open.set(false);
                                                    return None;
                                                }
                                            },
                                        };
                                        prefix_admissions_open.set(false);
                                        return Some(publish_sequence_zero_startup_snapshot(
                                            weeb3,
                                            cache_key,
                                            owner,
                                            topic,
                                            network_id,
                                            prefix,
                                            InitialCanonicalFeedResolution::Unavailable,
                                        ));
                                    }
                                    let canonical_starts_late =
                                        hls_media_sequence(&canonical.bytes)
                                            .is_some_and(|sequence| sequence > 0);
                                    let mut preferred = best_prefix
                                        .borrow()
                                        .clone()
                                        .map(SequenceZeroStartupPrefix::Complete)
                                        .filter(|prefix| {
                                            sequence_zero_startup_prefix_is_preferred(
                                                &canonical, prefix,
                                            )
                                        });
                                    if preferred.is_none()
                                        && canonical_starts_late
                                        && let Ok(Ok(prefix)) = async_std::future::timeout(
                                            HLS_STARTUP_PREFIX_RESULT_GRACE,
                                            prefix_ready_in.recv(),
                                        )
                                        .await
                                        && sequence_zero_startup_prefix_is_preferred(
                                            &canonical, &prefix,
                                        )
                                    {
                                        preferred = Some(prefix);
                                    }
                                    if preferred.is_none() {
                                        preferred = best_prefix
                                            .borrow()
                                            .clone()
                                            .map(SequenceZeroStartupPrefix::Complete)
                                            .filter(|prefix| {
                                                sequence_zero_startup_prefix_is_preferred(
                                                    &canonical, prefix,
                                                )
                                            });
                                    }
                                    if let Some(prefix) = preferred {
                                        prefix_admissions_open.set(false);
                                        return Some(publish_sequence_zero_startup_snapshot(
                                            weeb3,
                                            cache_key,
                                            owner,
                                            topic,
                                            network_id,
                                            prefix,
                                            InitialCanonicalFeedResolution::Ready(canonical),
                                        ));
                                    }
                                    if canonical_starts_late {
                                        prefix_admissions_open.set(false);
                                        return None;
                                    }
                                    prefix_admissions_open.set(false);
                                    authenticated_startup_prefix = best_prefix.borrow().clone();
                                    canonical
                                }
                                Either::Right((Ok(None) | Err(_), _)) => {
                                    let prefix = match async_std::future::timeout(
                                        HLS_STARTUP_PREFIX_RESULT_GRACE,
                                        prefix_ready_in.recv(),
                                    )
                                    .await
                                    {
                                        Ok(Ok(prefix)) => prefix,
                                        _ => match best_prefix.borrow().clone() {
                                            Some(prefix) => {
                                                SequenceZeroStartupPrefix::Complete(prefix)
                                            }
                                            None => {
                                                prefix_admissions_open.set(false);
                                                return None;
                                            }
                                        },
                                    };
                                    prefix_admissions_open.set(false);
                                    return Some(publish_sequence_zero_startup_snapshot(
                                        weeb3,
                                        cache_key,
                                        owner,
                                        topic,
                                        network_id,
                                        prefix,
                                        InitialCanonicalFeedResolution::Unavailable,
                                    ));
                                }
                            }
                        } else {
                            canonical_in.recv().await.ok().flatten()?
                        }
                    }
                }
            }
        };
        let canonical_loaded = loaded;
        let presentation_loaded = authenticated_startup_prefix
            .filter(|prefix| {
                prefix.index < canonical_loaded.index
                    && hls_startup_prefix_is_preferred(
                        &canonical_loaded.bytes,
                        &prefix.bytes,
                        HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
                    )
            })
            .unwrap_or_else(|| canonical_loaded.clone());
        let provisional_hls = index_hint.is_none()
            && canonical_loaded.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
            && is_hls_manifest(&canonical_loaded.bytes);
        let (loaded, head_confirmed) = if index_hint.is_none() && !provisional_hls {
            stabilize_initial_unindexed_hls_payload(
                weeb3.clone(),
                &owner,
                &topic,
                network_id,
                canonical_loaded.clone(),
                false,
                None,
            )
            .await
        } else {
            (presentation_loaded, false)
        };
        if weeb3.get_network_id().await != network_id
            || active_profile().swarm_network_id != network_id
        {
            return None;
        }
        if loaded.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES {
            return None;
        }
        let stabilization_seed = provisional_hls.then(|| canonical_loaded.clone());
        let finalized = hls_snapshot_is_terminal(
            hls_is_finalized(&loaded.bytes),
            index_hint.is_some(),
            head_confirmed,
        );
        let followup_mode =
            if sequence_zero_start_requested && hls_media_sequence(&loaded.bytes) == Some(0) {
                FeedFollowupMode::SequenceZeroPresentation
            } else {
                FeedFollowupMode::Canonical
            };
        let cache_key = if sequence_zero_start_requested
            && hls_media_sequence(&loaded.bytes).is_some_and(|sequence| sequence > 0)
        {
            canonical_cache_key
        } else {
            cache_key
        };
        let snapshot = FeedRouteSnapshot {
            index: loaded.index,
            body: Arc::from(loaded.bytes),
            finalized,
        };
        let snapshot =
            store_feed_snapshot(&cache_key, snapshot, index_hint.is_none(), followup_mode);
        let last_head_check = FEED_ROUTE_CACHE.with(|cache| {
            cache
                .borrow()
                .get(&cache_key)
                .map(|state| state.last_head_check)
                .unwrap_or_default()
        });
        let await_live_frontier = (!defer_live_followup
            && start == HlsStart::Live
            && index_hint.is_none()
            && stabilization_seed.is_some())
        .then_some(());
        if index_hint.is_none() && snapshot.finalized {
            remember_authenticated_endlist_index(network_id, &owner, &topic, snapshot.index);
        }
        let initial_check = if defer_live_followup && start == HlsStart::Live {
            None
        } else if let Some(initial) = stabilization_seed {
            schedule_initial_feed_stabilization(
                weeb3.clone(),
                cache_key.clone(),
                owner.clone(),
                topic.clone(),
                snapshot.index,
                initial,
                start == HlsStart::Live,
                followup_mode,
            )
        } else if index_hint.is_none() {
            schedule_feed_followup(
                weeb3.clone(),
                cache_key.clone(),
                owner.clone(),
                topic.clone(),
                false,
                followup_mode,
            );
            None
        } else {
            None
        };
        let snapshot = match await_live_frontier {
            Some(()) if initial_check.is_some() => await_live_frontier_snapshot(
                &cache_key,
                last_head_check,
                live_frontier_deadline_ms,
                true,
            )
            .await
            .unwrap_or(snapshot),
            _ => snapshot,
        };
        if start == HlsStart::Live && index_hint.is_none() && !defer_live_followup {
            prefetch_live_snapshot_start(&weeb3, &owner, &topic, &snapshot);
        }
        Some(snapshot)
    }

    async fn await_terminal_feed_confirmation_view(
        weeb3: &Weeb3,
        expected_network_id: u64,
    ) -> bool {
        async_std::task::sleep(HLS_TERMINAL_CONFIRMATION_MIN_DELAY).await;
        for poll in 0..=HLS_TERMINAL_CONFIRMATION_MAX_POLLS {
            if weeb3.get_network_id().await != expected_network_id {
                return false;
            }
            let peer_view_is_mature =
                hls_terminal_peer_view_is_mature(weeb3.get_connections().await);
            if weeb3.get_network_id().await != expected_network_id {
                return false;
            }
            if peer_view_is_mature {
                return true;
            }
            if poll == HLS_TERMINAL_CONFIRMATION_MAX_POLLS {
                break;
            }
            async_std::task::sleep(HLS_TERMINAL_CONFIRMATION_POLL_INTERVAL).await;
        }
        false
    }

    async fn confirm_terminal_feed_head(
        weeb3: Arc<Weeb3>,
        owner: &str,
        topic: &str,
        candidate: crate::bzz_stream::RawFeedPayload,
        expected_network_id: u64,
    ) -> (crate::bzz_stream::RawFeedPayload, bool) {
        if weeb3.get_network_id().await != expected_network_id {
            return (candidate, false);
        }
        let terminal = hls_is_finalized(&candidate.bytes);
        if !terminal {
            return (candidate, true);
        }
        if !await_terminal_feed_confirmation_view(&weeb3, expected_network_id).await {
            return (candidate, false);
        }

        let Some(confirmed) = acquire_latest_raw_feed_payload_from(
            owner.to_string(),
            topic.to_string(),
            candidate.clone(),
            &weeb3.chunk_port.0,
        )
        .await
        else {
            return (candidate, false);
        };
        if weeb3.get_network_id().await != expected_network_id
            || !hls_terminal_peer_view_is_mature(weeb3.get_connections().await)
        {
            return (candidate, false);
        }
        if confirmed.index < candidate.index
            || confirmed.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES
            || !is_hls_manifest(&confirmed.bytes)
        {
            return (candidate, false);
        }
        (confirmed, true)
    }

    async fn await_feed_probe_wave_credit(
        weeb3: Arc<Weeb3>,
        expected_network_id: u64,
        probe_count: usize,
        deadline_ms: f64,
    ) -> bool {
        let probe_count = u64::try_from(probe_count).unwrap_or(u64::MAX);
        let foreground_margin = (probe_count / 2).min(HLS_FEED_WAVE_FOREGROUND_MARGIN_CHUNKS);
        let required = probe_count
            .saturating_mul(HLS_FEED_WAVE_RESERVATIONS_PER_PROBE)
            .saturating_add(foreground_margin);
        loop {
            if weeb3.get_network_id().await != expected_network_id
                || active_profile().swarm_network_id != expected_network_id
            {
                return false;
            }
            if weeb3.available_retrieve_slots().await >= required {
                return true;
            }
            if js_sys::Date::now() >= deadline_ms {
                return false;
            }
            async_std::task::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn stabilize_initial_unindexed_hls_payload(
        weeb3: Arc<Weeb3>,
        owner: &str,
        topic: &str,
        network_id: u64,
        mut loaded: crate::bzz_stream::RawFeedPayload,
        observe_progress: bool,
        task: Option<&FeedTask>,
    ) -> (crate::bzz_stream::RawFeedPayload, bool) {
        if loaded.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES || !is_hls_manifest(&loaded.bytes) {
            return (loaded, false);
        }
        if weeb3.get_network_id().await != network_id
            || active_profile().swarm_network_id != network_id
        {
            return (loaded, false);
        }

        let progress_id = weeb3
            .start_progress(
                "feed-frontier",
                format!("{} topic {}", owner, topic),
                "verify",
                None,
                format!(
                    "validating exact updates after bounded candidate {}",
                    loaded.index
                ),
            )
            .await;
        let reliable = if hls_is_finalized(&loaded.bytes) {
            Some((loaded.clone(), true))
        } else if !observe_progress {
            let admission_client = weeb3.clone();
            let admission_deadline =
                js_sys::Date::now() + HLS_FEED_WAVE_CREDIT_WAIT.as_millis() as f64;
            acquire_latest_raw_feed_payload_bounded_from(
                owner.to_string(),
                topic.to_string(),
                loaded.clone(),
                false,
                &weeb3.chunk_port.0,
                move |probe_count| {
                    await_feed_probe_wave_credit(
                        admission_client.clone(),
                        network_id,
                        probe_count,
                        admission_deadline,
                    )
                },
                None,
            )
            .await
        } else {
            let (observed_out, observed_in) = mpsc::bounded::<crate::bzz_stream::RawFeedPayload>(2);
            let admission_client = weeb3.clone();
            let admission_deadline =
                js_sys::Date::now() + HLS_FEED_WAVE_CREDIT_WAIT.as_millis() as f64;
            let mut search = Box::pin(acquire_latest_raw_feed_payload_bounded_from(
                owner.to_string(),
                topic.to_string(),
                loaded.clone(),
                false,
                &weeb3.chunk_port.0,
                move |probe_count| {
                    await_feed_probe_wave_credit(
                        admission_client.clone(),
                        network_id,
                        probe_count,
                        admission_deadline,
                    )
                },
                Some(observed_out),
            ));
            loop {
                match select(search, Box::pin(observed_in.recv())).await {
                    Either::Left((result, _)) => {
                        if result.as_ref().is_some_and(|(candidate, verified)| {
                            *verified
                                && candidate.index >= loaded.index
                                && candidate.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                                && is_hls_manifest(&candidate.bytes)
                        }) {
                            break result;
                        }
                        let drain_observed = async {
                            while let Ok(candidate) = observed_in.recv().await {
                                if weeb3.get_network_id().await != network_id
                                    || active_profile().swarm_network_id != network_id
                                {
                                    break;
                                }
                                if candidate.index > loaded.index
                                    && candidate.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                                    && is_hls_manifest(&candidate.bytes)
                                {
                                    loaded = candidate;
                                    if let Some(task) = task {
                                        task.publish(&loaded, false);
                                    }
                                }
                            }
                        };
                        let _ = async_std::future::timeout(
                            FEED_FRONTIER_LOOKAHEAD_TIMEOUT,
                            drain_observed,
                        )
                        .await;
                        break result;
                    }
                    Either::Right((Ok(candidate), remaining)) => {
                        search = remaining;
                        if weeb3.get_network_id().await != network_id
                            || active_profile().swarm_network_id != network_id
                        {
                            break None;
                        }
                        if candidate.index > loaded.index
                            && candidate.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                            && is_hls_manifest(&candidate.bytes)
                        {
                            loaded = candidate;
                            if let Some(task) = task {
                                task.publish(&loaded, false);
                            }
                        }
                    }
                    Either::Right((Err(_), remaining)) => break remaining.await,
                }
            }
        };
        let network_current = weeb3.get_network_id().await == network_id
            && active_profile().swarm_network_id == network_id;
        let mut head_confirmed = false;
        let detail = match reliable {
            Some((reliable, verified))
                if network_current
                    && reliable.index >= loaded.index
                    && reliable.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                    && is_hls_manifest(&reliable.bytes) =>
            {
                loaded = reliable;
                if let Some(task) = task {
                    task.publish(&loaded, false);
                }
                if !verified {
                    format!(
                        "kept authenticated candidate {} while the frontier remained unresolved",
                        loaded.index
                    )
                } else {
                    let confirmation =
                        confirm_terminal_feed_head(weeb3.clone(), owner, topic, loaded, network_id)
                            .await;
                    loaded = confirmation.0;
                    head_confirmed = confirmation.1;
                    if head_confirmed && let Some(task) = task {
                        task.publish(&loaded, true);
                    }
                    if hls_is_finalized(&loaded.bytes) && head_confirmed {
                        format!("validated finalized VOD index {}", loaded.index)
                    } else if hls_is_finalized(&loaded.bytes) {
                        format!(
                            "kept ENDLIST index {} provisional pending at least {} priced peers and a repeated head search",
                            loaded.index, HLS_TERMINAL_CONFIRMATION_MIN_PRICED_PEERS
                        )
                    } else {
                        format!("validated reliable live head {}", loaded.index)
                    }
                }
            }
            _ => format!(
                "kept authenticated candidate {} after reliable head decoding failed",
                loaded.index
            ),
        };

        let ok =
            loaded.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES && is_hls_manifest(&loaded.bytes);
        weeb3
            .finish_progress(
                &progress_id,
                if ok { "complete" } else { "failed" },
                detail,
                ok,
            )
            .await;
        (loaded, head_confirmed)
    }

    async fn stabilize_claimed_feed_route(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        network_id: u64,
        checking_token: u64,
        initial: crate::bzz_stream::RawFeedPayload,
        observe_progress: bool,
        resume_exact_followup: bool,
        followup_mode: FeedFollowupMode,
    ) {
        if !FEED_ROUTE_CACHE.with(|cache| {
            cache
                .borrow()
                .get(&cache_key)
                .is_some_and(|state| state.checking_token == checking_token)
        }) {
            return;
        }
        let task = FeedTask {
            cache_key: cache_key.clone(),
            token: checking_token,
            mode: followup_mode,
        };
        if deferred_feed_route_source(&cache_key, checking_token).is_some() {
            task.publish(&initial, false);
        }
        let (_, head_confirmed) = stabilize_initial_unindexed_hls_payload(
            weeb3.clone(),
            &owner,
            &topic,
            network_id,
            initial,
            observe_progress,
            Some(&task),
        )
        .await;

        if deferred_feed_route_source(&cache_key, checking_token).is_some()
            && !complete_deferred_feed_route(
                weeb3.clone(),
                &cache_key,
                &owner,
                &topic,
                network_id,
                checking_token,
                &task,
            )
            .await
        {
            let _ = discard_deferred_feed_route(&cache_key, checking_token);
            return;
        }

        let network_current = weeb3.get_network_id().await == network_id
            && active_profile().swarm_network_id == network_id;
        if let Some((cache_finalized, cache_index)) =
            release_feed_route_check(&cache_key, checking_token)
            && network_current
        {
            if cache_finalized {
                remember_authenticated_endlist_index(network_id, &owner, &topic, cache_index);
            } else if resume_exact_followup {
                if head_confirmed {
                    forget_vod_index(network_id, &owner, &topic);
                }
                schedule_feed_followup(
                    weeb3,
                    cache_key,
                    owner,
                    topic,
                    !head_confirmed,
                    followup_mode,
                );
            }
        }
    }

    fn schedule_initial_feed_stabilization(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        required_cache_index: u64,
        initial: crate::bzz_stream::RawFeedPayload,
        observe_progress: bool,
        followup_mode: FeedFollowupMode,
    ) -> Option<u64> {
        let network_id = active_profile().swarm_network_id;
        let Some((_, checking_token)) =
            claim_feed_route_check(&cache_key, Some(required_cache_index))
        else {
            return None;
        };
        let prefix_stamp = hls_prefix_stamp_for_feed(&weeb3, &owner, &topic);

        spawn_local(async move {
            match await_hls_beginning_prefix_barrier(&weeb3, &owner, &topic, prefix_stamp).await {
                HlsProgressiveRangeAdmission::Admit => {
                    stabilize_claimed_feed_route(
                        weeb3,
                        cache_key,
                        owner,
                        topic,
                        network_id,
                        checking_token,
                        initial,
                        observe_progress,
                        true,
                        followup_mode,
                    )
                    .await;
                }
                HlsProgressiveRangeAdmission::Retire => {
                    let _ = release_feed_route_check(&cache_key, checking_token);
                }
                HlsProgressiveRangeAdmission::Park => unreachable!("barrier resolves parking"),
            }
        });
        Some(checking_token)
    }

    fn publish_sequence_zero_startup_snapshot(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        network_id: u64,
        prefix: SequenceZeroStartupPrefix,
        canonical: InitialCanonicalFeedResolution,
    ) -> FeedRouteSnapshot {
        let deferred_prefix = matches!(prefix, SequenceZeroStartupPrefix::Deferred(_));
        let snapshot = match prefix {
            SequenceZeroStartupPrefix::Complete(prefix) => store_feed_snapshot(
                &cache_key,
                FeedRouteSnapshot {
                    index: prefix.index,
                    body: Arc::from(prefix.bytes),
                    finalized: false,
                },
                true,
                FeedFollowupMode::SequenceZeroPresentation,
            ),
            SequenceZeroStartupPrefix::Deferred(prefix) => {
                store_deferred_sequence_zero_snapshot(&cache_key, prefix)
                    .expect("a fresh deferred presentation cache key is vacant")
            }
        };
        let canonical_token =
            claim_feed_route_check(&cache_key, Some(snapshot.index)).map(|(_, token)| token);
        let prefix_stamp = hls_prefix_stamp_for_feed(&weeb3, &owner, &topic);
        if let Some(token) = canonical_token {
            let fallback_client = weeb3.clone();
            let fallback_cache_key = cache_key.clone();
            let fallback_owner = owner.clone();
            let fallback_topic = topic.clone();
            spawn_local(async move {
                async_std::task::sleep(HLS_SEQUENCE_ZERO_CANONICAL_EXCLUSIVITY).await;
                if deferred_prefix {
                    if fallback_client.get_network_id().await != network_id
                        || active_profile().swarm_network_id != network_id
                    {
                        let _ = discard_deferred_feed_route(&fallback_cache_key, token);
                    }
                    return;
                }
                if fallback_client.get_network_id().await == network_id
                    && release_feed_route_check(&fallback_cache_key, token).is_some()
                {
                    schedule_feed_followup(
                        fallback_client,
                        fallback_cache_key,
                        fallback_owner,
                        fallback_topic,
                        false,
                        FeedFollowupMode::SequenceZeroPresentation,
                    );
                }
            });
        }
        spawn_local(async move {
            let initial = match canonical {
                InitialCanonicalFeedResolution::Ready(initial) => Some(initial),
                InitialCanonicalFeedResolution::Pending(receiver) => {
                    receiver.recv().await.ok().flatten()
                }
                InitialCanonicalFeedResolution::Unavailable => None,
            };
            if let Some(token) = canonical_token
                && await_hls_beginning_prefix_barrier(&weeb3, &owner, &topic, prefix_stamp).await
                    == HlsProgressiveRangeAdmission::Retire
            {
                if deferred_feed_route_source(&cache_key, token).is_some() {
                    let _ = discard_deferred_feed_route(&cache_key, token);
                } else {
                    let _ = release_feed_route_check(&cache_key, token);
                }
                return;
            }
            if let (Some(initial), Some(token)) = (initial, canonical_token) {
                stabilize_claimed_feed_route(
                    weeb3,
                    cache_key,
                    owner,
                    topic,
                    network_id,
                    token,
                    initial,
                    false,
                    false,
                    FeedFollowupMode::SequenceZeroPresentation,
                )
                .await;
                return;
            }
            if let Some(token) = canonical_token
                && deferred_feed_route_source(&cache_key, token).is_some()
            {
                let task = FeedTask {
                    cache_key: cache_key.clone(),
                    token,
                    mode: FeedFollowupMode::SequenceZeroPresentation,
                };
                if !complete_deferred_feed_route(
                    weeb3.clone(),
                    &cache_key,
                    &owner,
                    &topic,
                    network_id,
                    token,
                    &task,
                )
                .await
                {
                    let _ = discard_deferred_feed_route(&cache_key, token);
                    return;
                }
            }
            if canonical_token
                .is_some_and(|token| release_feed_route_check(&cache_key, token).is_some())
                && weeb3.get_network_id().await == network_id
            {
                schedule_feed_followup(
                    weeb3,
                    cache_key,
                    owner,
                    topic,
                    false,
                    FeedFollowupMode::SequenceZeroPresentation,
                );
            }
        });
        snapshot
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum LiveHistoryProbeState {
        Pending,
        InFlight,
        Found,
        Unavailable,
        Unsupported,
        Transient,
    }

    #[derive(Clone, Copy)]
    enum LiveHistoryProbeClass {
        Primary,
        Repair,
    }

    type LiveHistoryProbeFuture =
        Pin<Box<dyn Future<Output = (u64, RetainedRawFeedPayloadProbe)> + 'static>>;
    type LiveHistoryDirectFuture =
        Pin<Box<dyn Future<Output = (u64, Option<RawFeedPayload>)> + 'static>>;

    enum LiveHistoryCollectorScope {
        Startup {
            presentation_id: u64,
        },
        SequenceZeroFollowup {
            cache_key: String,
            checking_token: u64,
            index: u64,
            source_body: Arc<[u8]>,
            snapshot_body: Arc<[u8]>,
        },
    }

    #[derive(Clone)]
    struct SequenceZeroFollowupGate {
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        network_id: u64,
        checking_token: u64,
        index: u64,
        source_body: Arc<[u8]>,
        snapshot_body: Arc<[u8]>,
    }

    impl SequenceZeroFollowupGate {
        fn current(&self) -> bool {
            active_profile().swarm_network_id == self.network_id
                && sequence_zero_followup_is_current(
                    &self.weeb3,
                    &self.cache_key,
                    &self.owner,
                    &self.topic,
                )
                && FEED_ROUTE_CACHE.with(|cache| {
                    cache.borrow().get(&self.cache_key).is_some_and(|state| {
                        state.checking_token == self.checking_token
                            && state.snapshot.index == self.index
                            && Arc::ptr_eq(&state.source_body, &self.source_body)
                            && Arc::ptr_eq(&state.snapshot.body, &self.snapshot_body)
                    })
                })
        }

        async fn admitted(&self) -> bool {
            if !self.current() {
                return false;
            }
            let (network_id, available_slots) = join(
                self.weeb3.get_network_id(),
                self.weeb3.available_retrieve_slots(),
            )
            .await;
            let required = HLS_FEED_WAVE_FOREGROUND_MARGIN_CHUNKS
                .saturating_add(CONSERVATIVE_DEFERRED_MAX_PHYSICAL_ATTEMPTS);
            network_id == self.network_id && available_slots >= required && self.current()
        }
    }

    struct LiveHistoryCollector {
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        scope: LiveHistoryCollectorScope,
        network_id: u64,
        pending: VecDeque<u64>,
        states: HashMap<u64, LiveHistoryProbeState>,
        in_flight: FuturesUnordered<LiveHistoryProbeFuture>,
        windows: BTreeMap<u64, Vec<u8>>,
        retained_bytes: usize,
        primary_count: usize,
        repair_count: usize,
        direct_candidate: Option<RawFeedPayload>,
        direct_in_flight: Option<LiveHistoryDirectFuture>,
        direct_reserved_bytes: usize,
        direct_required: bool,
        direct_index: Option<u64>,
        deferred_candidate: Option<DeferredRawFeedPayload>,
        deferred_probe_indices: HashSet<u64>,
        followup_retry_indices: VecDeque<HlsSequenceZeroRetry>,
        followup_deferred_retry_index: Option<u64>,
        capacity_parallelism: usize,
        capacity_priced_peers: u64,
        capacity_checked_at: f64,
        highest_authenticated_positive_index: u64,
    }

    impl LiveHistoryCollector {
        fn new(
            weeb3: Arc<Weeb3>,
            owner: String,
            topic: String,
            presentation_id: u64,
            network_id: u64,
            initial: RawFeedPayload,
        ) -> Option<Self> {
            Self::new_with_scope(
                weeb3,
                owner,
                topic,
                network_id,
                initial,
                LiveHistoryCollectorScope::Startup { presentation_id },
            )
        }

        fn new_sequence_zero_followup(
            weeb3: Arc<Weeb3>,
            owner: String,
            topic: String,
            network_id: u64,
            initial: RawFeedPayload,
            cache_key: String,
            checking_token: u64,
            snapshot_body: Arc<[u8]>,
            source_body: Arc<[u8]>,
        ) -> Option<Self> {
            let index = initial.index;
            Self::new_with_scope(
                weeb3,
                owner,
                topic,
                network_id,
                initial,
                LiveHistoryCollectorScope::SequenceZeroFollowup {
                    cache_key,
                    checking_token,
                    index,
                    source_body,
                    snapshot_body,
                },
            )
        }

        fn new_with_scope(
            weeb3: Arc<Weeb3>,
            owner: String,
            topic: String,
            network_id: u64,
            initial: RawFeedPayload,
            scope: LiveHistoryCollectorScope,
        ) -> Option<Self> {
            (initial.bytes.len() <= HLS_SPARSE_HISTORY_MAX_WINDOW_BYTES
                && hls_sparse_history_head_is_supported(&initial.bytes))
            .then_some(())?;
            let retained_bytes = initial.bytes.len();
            let mut states = HashMap::new();
            states.insert(initial.index, LiveHistoryProbeState::Found);
            let mut windows = BTreeMap::new();
            windows.insert(initial.index, initial.bytes);
            Some(Self {
                weeb3,
                owner,
                topic,
                scope,
                network_id,
                pending: VecDeque::new(),
                states,
                in_flight: FuturesUnordered::new(),
                windows,
                retained_bytes,
                primary_count: 0,
                repair_count: 0,
                direct_candidate: None,
                direct_in_flight: None,
                direct_reserved_bytes: 0,
                direct_required: false,
                direct_index: None,
                deferred_candidate: None,
                deferred_probe_indices: HashSet::new(),
                followup_retry_indices: VecDeque::new(),
                followup_deferred_retry_index: None,
                capacity_parallelism: 0,
                capacity_priced_peers: 0,
                capacity_checked_at: 0.0,
                highest_authenticated_positive_index: initial.index,
            })
        }

        fn current(&self) -> bool {
            let session_current = match &self.scope {
                LiveHistoryCollectorScope::Startup { presentation_id } => {
                    live_history_session_is_current(
                        &self.weeb3,
                        &self.owner,
                        &self.topic,
                        *presentation_id,
                    )
                }
                LiveHistoryCollectorScope::SequenceZeroFollowup {
                    cache_key,
                    checking_token,
                    index,
                    source_body,
                    snapshot_body,
                } => {
                    sequence_zero_followup_is_current(
                        &self.weeb3,
                        cache_key,
                        &self.owner,
                        &self.topic,
                    ) && FEED_ROUTE_CACHE.with(|cache| {
                        cache.borrow().get(cache_key).is_some_and(|state| {
                            state.checking_token == *checking_token
                                && state.snapshot.index == *index
                                && Arc::ptr_eq(&state.source_body, source_body)
                                && Arc::ptr_eq(&state.snapshot.body, snapshot_body)
                        })
                    })
                }
            };
            session_current && active_profile().swarm_network_id == self.network_id
        }

        fn is_sequence_zero_followup(&self) -> bool {
            matches!(
                &self.scope,
                LiveHistoryCollectorScope::SequenceZeroFollowup { .. }
            )
        }

        fn sequence_zero_followup_gate(&self) -> Option<SequenceZeroFollowupGate> {
            let LiveHistoryCollectorScope::SequenceZeroFollowup {
                cache_key,
                checking_token,
                index,
                source_body,
                snapshot_body,
            } = &self.scope
            else {
                return None;
            };
            Some(SequenceZeroFollowupGate {
                weeb3: self.weeb3.clone(),
                cache_key: cache_key.clone(),
                owner: self.owner.clone(),
                topic: self.topic.clone(),
                network_id: self.network_id,
                checking_token: *checking_token,
                index: *index,
                source_body: source_body.clone(),
                snapshot_body: snapshot_body.clone(),
            })
        }

        fn park_deferred_followup_candidate(
            &mut self,
            deferred: DeferredRawFeedPayload,
        ) -> Result<(), String> {
            self.deferred_probe_indices.insert(deferred.index);
            self.forget_followup_retry_index(deferred.index);
            if self
                .deferred_candidate
                .as_ref()
                .is_some_and(|current| current.index >= deferred.index)
            {
                self.followup_deferred_retry_index = self
                    .deferred_candidate
                    .as_ref()
                    .map(|current| current.index);
                return Ok(());
            }
            if let Some(current) = self.deferred_candidate.take() {
                self.retained_bytes = self.retained_bytes.saturating_sub(current.retained_bytes());
            }
            let total = self
                .retained_bytes
                .checked_add(deferred.retained_bytes())
                .ok_or_else(|| {
                    "The deferred Live update byte accounting overflowed.".to_string()
                })?;
            if total > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES {
                return Err(
                    "The deferred Live update exceeded its bounded memory budget.".to_string(),
                );
            }
            self.retained_bytes = total;
            self.followup_deferred_retry_index = Some(deferred.index);
            self.deferred_candidate = Some(deferred);
            Ok(())
        }

        fn remember_followup_retry_index(
            &mut self,
            index: u64,
            authenticated: bool,
            priority: bool,
        ) -> bool {
            remember_hls_sequence_zero_retry(
                &mut self.followup_retry_indices,
                index,
                authenticated,
                priority,
                HLS_SEQUENCE_ZERO_RETRY_BACKLOG_MAX,
            )
        }

        fn forget_followup_retry_index(&mut self, index: u64) {
            self.followup_retry_indices
                .retain(|current| current.index != index);
        }

        fn remember_followup_windows_after(&mut self, index: u64) -> bool {
            let Some(first_later) = index.checked_add(1) else {
                return true;
            };
            let later = self
                .windows
                .range(first_later..)
                .map(|(index, _)| *index)
                .collect::<Vec<_>>();
            later.into_iter().all(|later_index| {
                self.remember_followup_retry_index(
                    later_index,
                    true,
                    later_index == self.highest_authenticated_positive_index,
                )
            })
        }

        fn retry_deferred_followup_candidate(&mut self, index: u64) {
            self.states.insert(index, LiveHistoryProbeState::Transient);
            self.followup_deferred_retry_index = Some(index);
        }

        fn forget_deferred_followup_retry_index(&mut self, index: u64) {
            if self.followup_deferred_retry_index == Some(index) {
                self.followup_deferred_retry_index = None;
            }
        }

        fn selected_deferred_followup_candidate(&self) -> Option<&DeferredRawFeedPayload> {
            self.deferred_candidate
                .as_ref()
                .filter(|deferred| deferred.index == self.highest_authenticated_positive_index)
        }

        fn take_selected_deferred_followup_candidate(&mut self) -> Option<DeferredRawFeedPayload> {
            let deferred = self.deferred_candidate.take()?;
            self.retained_bytes = self
                .retained_bytes
                .saturating_sub(deferred.retained_bytes());
            (deferred.index == self.highest_authenticated_positive_index).then_some(deferred)
        }

        fn unusable_probe_state(&self) -> LiveHistoryProbeState {
            if self.is_sequence_zero_followup() {
                LiveHistoryProbeState::Unsupported
            } else {
                LiveHistoryProbeState::Unavailable
            }
        }

        fn sequence_zero_followup_snapshot(&self) -> Option<&[u8]> {
            match &self.scope {
                LiveHistoryCollectorScope::SequenceZeroFollowup { snapshot_body, .. } => {
                    Some(snapshot_body)
                }
                LiveHistoryCollectorScope::Startup { .. } => None,
            }
        }

        fn head(&self) -> Option<RawFeedPayload> {
            self.windows
                .last_key_value()
                .map(|(index, bytes)| RawFeedPayload {
                    index: *index,
                    bytes: bytes.clone(),
                })
        }

        fn drop_direct_work(&mut self) {
            if self.direct_required
                && let Some(index) = self.direct_index
                && self.state(index) == Some(LiveHistoryProbeState::InFlight)
            {
                self.states.insert(index, self.unusable_probe_state());
            }
            self.direct_candidate = None;
            self.direct_in_flight = None;
            self.retained_bytes = self
                .retained_bytes
                .saturating_sub(self.direct_reserved_bytes);
            self.direct_reserved_bytes = 0;
            self.direct_required = false;
            self.direct_index = None;
        }

        fn start_deferred_direct(
            &mut self,
            deferred: DeferredRawFeedPayload,
            required: bool,
            conservative_gate: Option<SequenceZeroFollowupGate>,
        ) -> Result<(), String> {
            let index = deferred.index;
            self.highest_authenticated_positive_index =
                self.highest_authenticated_positive_index.max(index);
            if !self.states.contains_key(&index) {
                if self.states.len() >= HLS_SPARSE_HISTORY_MAX_CANDIDATES {
                    return Err(
                        "The Live history exceeded its bounded unique-index budget.".to_string()
                    );
                }
                self.states.insert(
                    index,
                    if required {
                        LiveHistoryProbeState::InFlight
                    } else {
                        LiveHistoryProbeState::Unavailable
                    },
                );
            }
            if self.direct_in_flight.is_some() || self.direct_candidate.is_some() {
                if required && !self.direct_required {
                    self.drop_direct_work();
                } else if required {
                    if self
                        .direct_index
                        .is_some_and(|current_index| index > current_index)
                    {
                        self.drop_direct_work();
                    } else {
                        self.states.insert(index, self.unusable_probe_state());
                        return Ok(());
                    }
                } else {
                    return Ok(());
                }
            }
            let span = usize::try_from(deferred.payload_span())
                .map_err(|_| "The direct Live archive span is not addressable.".to_string())?;
            let reservation = span
                .checked_add(deferred.retained_bytes())
                .ok_or_else(|| "The direct Live archive byte accounting overflowed.".to_string())?;
            let total = self
                .retained_bytes
                .checked_add(reservation)
                .ok_or_else(|| "The direct Live archive byte accounting overflowed.".to_string())?;
            if total > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES {
                return if required {
                    Err(
                        "The Live archive exceeded its bounded aggregate memory budget."
                            .to_string(),
                    )
                } else {
                    Ok(())
                };
            }
            self.retained_bytes = total;
            self.direct_reserved_bytes = reservation;
            self.direct_required = required;
            self.direct_index = Some(index);
            let client = self.weeb3.clone();
            self.direct_in_flight = Some(Box::pin(async move {
                let payload = if let Some(gate) = conservative_gate {
                    let range_gate = gate.clone();
                    acquire_deferred_raw_feed_payload_conservative(
                        deferred,
                        MAX_STREAM_FEED_PAYLOAD_BYTES,
                        &client.chunk_port.0,
                        move || {
                            let range_gate = range_gate.clone();
                            async move { range_gate.admitted().await }
                        },
                    )
                    .await
                } else {
                    client.hls_deferred_feed_payload(deferred).await
                };
                (index, payload)
            }));
            Ok(())
        }

        fn adopt_inline_direct(&mut self, payload: RawFeedPayload) -> Result<(), String> {
            if self.direct_in_flight.is_some() || self.direct_candidate.is_some() {
                if self
                    .direct_index
                    .is_some_and(|current_index| payload.index > current_index)
                {
                    self.drop_direct_work();
                } else {
                    return Ok(());
                }
            }
            let total = self
                .retained_bytes
                .checked_add(payload.bytes.len())
                .ok_or_else(|| "The inline Live archive byte accounting overflowed.".to_string())?;
            if total > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES {
                return Err(
                    "The inline Live archive exceeded its bounded memory budget.".to_string(),
                );
            }
            self.retained_bytes = total;
            self.direct_reserved_bytes = payload.bytes.len();
            self.direct_required = false;
            self.direct_index = Some(payload.index);
            self.direct_candidate = Some(payload);
            Ok(())
        }

        fn take_direct(&mut self) -> Option<RawFeedPayload> {
            let candidate = self.direct_candidate.take()?;
            let candidate_timeline = hls_complete_history_timeline(&candidate.bytes);
            let direct_shape_valid = if let Some(snapshot) = self.sequence_zero_followup_snapshot()
            {
                hls_media_sequence(&candidate.bytes) == Some(0)
                    && hls_sequence_zero_covers_head(snapshot, &candidate.bytes)
            } else {
                hls_is_finalized(&candidate.bytes)
            };
            let valid = direct_shape_valid
                && self
                    .windows
                    .last_key_value()
                    .is_some_and(|(index, _)| candidate.index >= *index)
                && candidate.index >= self.highest_authenticated_positive_index
                && candidate_timeline
                    .as_ref()
                    .is_some_and(|candidate_timeline| {
                        self.windows.values().all(|window| {
                            hls_sequence_zero_timeline_covers_head(window, candidate_timeline)
                        })
                    });
            if !valid {
                if !self.windows.contains_key(&candidate.index) {
                    self.states
                        .insert(candidate.index, self.unusable_probe_state());
                }
                self.retained_bytes = self
                    .retained_bytes
                    .saturating_sub(self.direct_reserved_bytes);
                self.direct_reserved_bytes = 0;
                self.direct_required = false;
                self.direct_index = None;
                return None;
            }
            self.windows.clear();
            self.retained_bytes = candidate.bytes.len();
            self.direct_reserved_bytes = 0;
            self.direct_required = false;
            self.direct_index = None;
            Some(candidate)
        }

        fn state(&self, index: u64) -> Option<LiveHistoryProbeState> {
            self.states.get(&index).copied()
        }

        fn enqueue(
            &mut self,
            indices: impl IntoIterator<Item = u64>,
            class: LiveHistoryProbeClass,
            priority: bool,
        ) -> Result<Vec<u64>, String> {
            let mut added = Vec::new();
            for index in indices {
                if self.states.contains_key(&index) {
                    continue;
                }
                if self.states.len() >= HLS_SPARSE_HISTORY_MAX_CANDIDATES {
                    return Err(
                        "The Live history exceeded its bounded unique-index budget.".to_string()
                    );
                }
                let count = match class {
                    LiveHistoryProbeClass::Primary => &mut self.primary_count,
                    LiveHistoryProbeClass::Repair => &mut self.repair_count,
                };
                *count = count.saturating_add(1);
                let maximum = match class {
                    LiveHistoryProbeClass::Primary => HLS_SPARSE_HISTORY_MAX_PROBES,
                    LiveHistoryProbeClass::Repair => HLS_SPARSE_HISTORY_MAX_REPAIRS,
                };
                if *count > maximum {
                    return Err("The Live history exceeded its bounded probe budget.".to_string());
                }
                self.states.insert(index, LiveHistoryProbeState::Pending);
                added.push(index);
            }
            if priority {
                for index in added.iter().rev() {
                    self.pending.push_front(*index);
                }
            } else {
                self.pending.extend(added.iter().copied());
            }
            Ok(added)
        }

        fn retry(&mut self, index: u64) {
            if self.state(index) == Some(LiveHistoryProbeState::Transient) {
                self.states.insert(index, LiveHistoryProbeState::Pending);
                self.pending.push_front(index);
            }
        }

        fn targets_settled(&self, targets: &[u64]) -> bool {
            targets.iter().all(|index| {
                matches!(
                    self.state(*index),
                    Some(
                        LiveHistoryProbeState::Found
                            | LiveHistoryProbeState::Unavailable
                            | LiveHistoryProbeState::Unsupported
                    )
                )
            })
        }

        fn targets_observed(&self, targets: &[u64]) -> bool {
            targets.iter().all(|index| {
                matches!(
                    self.state(*index),
                    Some(
                        LiveHistoryProbeState::Found
                            | LiveHistoryProbeState::Unavailable
                            | LiveHistoryProbeState::Unsupported
                            | LiveHistoryProbeState::Transient
                    )
                )
            })
        }

        fn successful_entries_before(&self, head_index: u64) -> impl Iterator<Item = (u64, &[u8])> {
            self.windows.iter().filter_map(move |(index, bytes)| {
                (*index < head_index && hls_sparse_history_candidate_is_supported(bytes))
                    .then_some((*index, bytes.as_slice()))
            })
        }

        fn resolved_indices_before(&self, head_index: u64) -> impl Iterator<Item = u64> + '_ {
            self.states.iter().filter_map(move |(index, state)| {
                (*index < head_index
                    && matches!(
                        state,
                        LiveHistoryProbeState::Found | LiveHistoryProbeState::Unavailable
                    ))
                .then_some(*index)
            })
        }

        fn verified_sequence_zero_checkpoint_tail(
            &self,
            candidate_index: u64,
            candidate: &[u8],
        ) -> Result<Option<Vec<u8>>, String> {
            let verified = if self.is_sequence_zero_followup() {
                hls_verified_sequence_zero_checkpoint_tail_at_index(
                    candidate_index,
                    candidate,
                    self.windows
                        .iter()
                        .map(|(index, window)| (*index, window.as_slice())),
                )
            } else {
                hls_verified_sequence_zero_checkpoint_tail(
                    candidate,
                    self.windows.values().map(Vec::as_slice),
                )
            };
            verified.map_err(|()| {
                "The authenticated Live checkpoint contradicts the pinned timeline.".to_string()
            })
        }

        fn remember_window(&mut self, payload: RawFeedPayload) -> Result<(), String> {
            if self.windows.contains_key(&payload.index) {
                return Ok(());
            }
            let mut total = self
                .retained_bytes
                .checked_add(payload.bytes.len())
                .ok_or_else(|| "The Live history byte accounting overflowed.".to_string())?;
            if total > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES
                && self.direct_reserved_bytes > 0
                && !self.direct_required
            {
                self.drop_direct_work();
                total = self
                    .retained_bytes
                    .checked_add(payload.bytes.len())
                    .ok_or_else(|| "The Live history byte accounting overflowed.".to_string())?;
            }
            if total > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES {
                return Err("The Live history exceeded its bounded memory budget.".to_string());
            }
            self.retained_bytes = total;
            self.windows.insert(payload.index, payload.bytes);
            Ok(())
        }

        fn accept_result(
            &mut self,
            index: u64,
            result: RetainedRawFeedPayloadProbe,
        ) -> Result<(), String> {
            if self.state(index) != Some(LiveHistoryProbeState::InFlight) {
                return Ok(());
            }
            if match &result {
                RetainedRawFeedPayloadProbe::Found(payload) => payload.index == index,
                RetainedRawFeedPayloadProbe::Deferred(deferred) => deferred.index == index,
                RetainedRawFeedPayloadProbe::Missing | RetainedRawFeedPayloadProbe::Transient => {
                    false
                }
            } {
                self.highest_authenticated_positive_index =
                    self.highest_authenticated_positive_index.max(index);
            }
            let payload = match result {
                RetainedRawFeedPayloadProbe::Found(payload) => Some(payload),
                RetainedRawFeedPayloadProbe::Deferred(deferred) => {
                    if deferred.index == index {
                        if self.is_sequence_zero_followup() {
                            self.park_deferred_followup_candidate(deferred)?;
                            self.states
                                .insert(index, LiveHistoryProbeState::Unsupported);
                        } else {
                            self.start_deferred_direct(deferred, true, None)?;
                        }
                    } else {
                        self.states.insert(index, LiveHistoryProbeState::Transient);
                    }
                    return Ok(());
                }
                RetainedRawFeedPayloadProbe::Missing => {
                    self.states
                        .insert(index, LiveHistoryProbeState::Unavailable);
                    return Ok(());
                }
                RetainedRawFeedPayloadProbe::Transient => {
                    self.states.insert(index, LiveHistoryProbeState::Transient);
                    return Ok(());
                }
            };
            let Some(payload) = payload.filter(|payload| payload.index == index) else {
                self.states.insert(index, LiveHistoryProbeState::Transient);
                return Ok(());
            };
            let finalized_sequence_zero =
                hls_media_sequence(&payload.bytes) == Some(0) && hls_is_finalized(&payload.bytes);
            let newer_than_head = self.head().is_some_and(|head| payload.index > head.index);
            if finalized_sequence_zero && newer_than_head {
                if !self
                    .head()
                    .is_some_and(|head| hls_sequence_zero_covers_head(&head.bytes, &payload.bytes))
                {
                    return Err(
                        "The authenticated Live archive contradicts the pinned timeline."
                            .to_string(),
                    );
                }
                if self.direct_reserved_bytes > 0 {
                    if self
                        .direct_index
                        .is_some_and(|current_index| current_index >= payload.index)
                    {
                        self.states.insert(index, self.unusable_probe_state());
                        return Ok(());
                    }
                    self.drop_direct_work();
                }
                let total = self
                    .retained_bytes
                    .checked_add(payload.bytes.len())
                    .ok_or_else(|| {
                        "The direct Live archive byte accounting overflowed.".to_string()
                    })?;
                if total > HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES {
                    self.states.insert(index, self.unusable_probe_state());
                    return Ok(());
                }
                self.retained_bytes = total;
                self.direct_reserved_bytes = payload.bytes.len();
                self.direct_required = false;
                self.direct_index = Some(payload.index);
                self.states.insert(index, self.unusable_probe_state());
                self.direct_candidate = Some(payload);
                return Ok(());
            }
            if finalized_sequence_zero && !self.is_sequence_zero_followup() {
                self.states
                    .insert(index, LiveHistoryProbeState::Unavailable);
                return Ok(());
            }
            if !self.is_sequence_zero_followup()
                && hls_is_long_sequence_zero_checkpoint(&payload.bytes)
                && self
                    .windows
                    .last_key_value()
                    .is_some_and(|(head_index, _)| index <= *head_index)
            {
                self.states
                    .insert(index, LiveHistoryProbeState::Unavailable);
                return Ok(());
            }
            let sparse_tail = self.verified_sequence_zero_checkpoint_tail(index, &payload.bytes)?;
            if let Some(sparse_tail) = sparse_tail {
                let use_full_checkpoint = self.is_sequence_zero_followup()
                    && self
                        .sequence_zero_followup_snapshot()
                        .is_some_and(|snapshot| {
                            hls_sequence_zero_covers_head(snapshot, &payload.bytes)
                                && self.windows.iter().all(|(window_index, window)| {
                                    *window_index > index
                                        || hls_sequence_zero_covers_head(window, &payload.bytes)
                                })
                        });
                self.remember_window(RawFeedPayload {
                    index,
                    bytes: sparse_tail,
                })?;
                self.states.insert(index, LiveHistoryProbeState::Found);
                if use_full_checkpoint
                    && self
                        .retained_bytes
                        .checked_add(payload.bytes.len())
                        .is_some_and(|total| total <= HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES)
                {
                    self.adopt_inline_direct(payload)?;
                }
                return Ok(());
            }
            if payload.bytes.len() <= HLS_SPARSE_HISTORY_MAX_WINDOW_BYTES
                && hls_sparse_history_head_is_supported(&payload.bytes)
            {
                self.remember_window(payload)?;
                self.states.insert(index, LiveHistoryProbeState::Found);
            } else if self.head().is_some_and(|head| index > head.index) {
                return Err(
                    "The authenticated Live update is not a supported HLS window.".to_string(),
                );
            } else {
                self.states.insert(index, self.unusable_probe_state());
            }
            Ok(())
        }

        async fn pump_once(&mut self) -> Result<(), String> {
            if !self.current() || self.weeb3.get_network_id().await != self.network_id {
                return Err("The Live history preparation was superseded.".to_string());
            }
            let now = js_sys::Date::now();
            if !self.capacity_checked_at.is_finite()
                || self.capacity_checked_at <= 0.0
                || now < self.capacity_checked_at
                || now - self.capacity_checked_at >= HLS_SPARSE_HISTORY_CAPACITY_REFRESH_MS
            {
                let (priced_peers, available_slots) = join(
                    self.weeb3.get_connections(),
                    self.weeb3.available_retrieve_slots(),
                )
                .await;
                self.capacity_priced_peers = priced_peers;
                let available_slots = if self.is_sequence_zero_followup() {
                    available_slots.saturating_sub(HLS_FEED_WAVE_FOREGROUND_MARGIN_CHUNKS)
                } else {
                    available_slots
                };
                self.capacity_parallelism =
                    hls_sparse_history_parallelism(priced_peers, available_slots);
                self.capacity_checked_at = now;
            }
            let parallelism = self.capacity_parallelism;
            let sequence_zero_followup = self.is_sequence_zero_followup();
            while self.in_flight.len() < parallelism {
                let Some(index) = self.pending.pop_front() else {
                    break;
                };
                if self.state(index) != Some(LiveHistoryProbeState::Pending) {
                    continue;
                }
                self.states.insert(index, LiveHistoryProbeState::InFlight);
                let client = self.weeb3.clone();
                let owner = self.owner.clone();
                let topic = self.topic.clone();
                self.in_flight.push(Box::pin(async move {
                    let result = if sequence_zero_followup {
                        client
                            .hls_feed_payload_at_index_followup_retained_status(owner, topic, index)
                            .await
                    } else {
                        client
                            .hls_feed_payload_at_index_retained_status(owner, topic, index)
                            .await
                    };
                    (index, result)
                }));
            }
            let direct_result = if let Some(direct) = self.direct_in_flight.as_mut() {
                direct.as_mut().now_or_never()
            } else {
                None
            };
            if let Some((index, payload)) = direct_result {
                self.direct_in_flight = None;
                let was_required = self.direct_required;
                if payload
                    .as_ref()
                    .is_some_and(|payload| payload.index != index)
                {
                    self.drop_direct_work();
                    return Err(
                        "The authenticated Live update index changed while decoding.".to_string(),
                    );
                }
                let disposition = payload.as_ref().map(|payload| {
                    if self.is_sequence_zero_followup()
                        && hls_media_sequence(&payload.bytes) == Some(0)
                        && !hls_is_finalized(&payload.bytes)
                    {
                        HlsDirectArchiveDisposition::SequenceZeroCheckpoint
                    } else {
                        hls_direct_archive_disposition(
                            index,
                            self.windows
                                .last_key_value()
                                .map_or(index, |(head_index, _)| *head_index),
                            self.highest_authenticated_positive_index,
                            &payload.bytes,
                        )
                    }
                });
                match disposition {
                    Some(HlsDirectArchiveDisposition::Terminal) => {
                        let candidate = payload.expect("the disposition requires a payload");
                        let Some(candidate_timeline) =
                            hls_complete_history_timeline(&candidate.bytes)
                        else {
                            self.drop_direct_work();
                            return Err("The authenticated Live archive is malformed.".to_string());
                        };
                        if !self.windows.values().all(|window| {
                            hls_sequence_zero_timeline_covers_head(window, &candidate_timeline)
                        }) {
                            self.drop_direct_work();
                            return Err(
                                "The authenticated Live archive contradicts the pinned timeline."
                                    .to_string(),
                            );
                        }
                        self.states.insert(index, self.unusable_probe_state());
                        self.direct_candidate = Some(candidate);
                    }
                    Some(HlsDirectArchiveDisposition::Stale)
                    | Some(HlsDirectArchiveDisposition::Nonterminal)
                    | Some(HlsDirectArchiveDisposition::Unsupported) => {
                        self.drop_direct_work();
                        if self.is_sequence_zero_followup() {
                            self.states
                                .insert(index, LiveHistoryProbeState::Unsupported);
                        }
                    }
                    Some(HlsDirectArchiveDisposition::SequenceZeroCheckpoint) => {
                        let checkpoint = payload.expect("the disposition requires a payload");
                        let sparse_tail = match self
                            .verified_sequence_zero_checkpoint_tail(index, &checkpoint.bytes)
                        {
                            Ok(sparse_tail) => sparse_tail,
                            Err(error) => {
                                self.drop_direct_work();
                                return Err(error);
                            }
                        };
                        let use_full_checkpoint = self
                            .sequence_zero_followup_snapshot()
                            .is_some_and(|snapshot| {
                                hls_sequence_zero_covers_head(snapshot, &checkpoint.bytes)
                                    && self.windows.iter().all(|(window_index, window)| {
                                        *window_index > index
                                            || hls_sequence_zero_covers_head(
                                                window,
                                                &checkpoint.bytes,
                                            )
                                    })
                            });
                        if let Some(sparse_tail) = sparse_tail {
                            let keep_full_checkpoint = use_full_checkpoint
                                && self
                                    .retained_bytes
                                    .checked_add(sparse_tail.len())
                                    .is_some_and(|total| {
                                        total <= HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES
                                    });
                            if !keep_full_checkpoint {
                                self.drop_direct_work();
                            }
                            self.remember_window(RawFeedPayload {
                                index,
                                bytes: sparse_tail,
                            })?;
                            self.states.insert(index, LiveHistoryProbeState::Found);
                            if keep_full_checkpoint {
                                self.direct_required = false;
                                self.direct_candidate = Some(checkpoint);
                            }
                            return Ok(());
                        }
                        self.drop_direct_work();
                    }
                    None => {
                        self.drop_direct_work();
                        if was_required {
                            self.states.insert(index, LiveHistoryProbeState::Transient);
                        }
                    }
                }
            }
            if self.in_flight.is_empty() {
                async_std::task::sleep(Duration::from_millis(25)).await;
                return Ok(());
            }
            if let Ok(Some((index, result))) =
                async_std::future::timeout(Duration::from_millis(25), self.in_flight.next()).await
            {
                self.accept_result(index, result)?;
            }
            Ok(())
        }

        async fn settle(&mut self, targets: &[u64]) -> Result<(), String> {
            let mut single_peer_ticks = 0_u16;
            while !self.targets_settled(targets) {
                let transient = targets
                    .iter()
                    .copied()
                    .filter(|index| self.state(*index) == Some(LiveHistoryProbeState::Transient))
                    .collect::<Vec<_>>();
                let priced_peers = self.capacity_priced_peers;
                let retry_transient =
                    !transient.is_empty() && (priced_peers >= 2 || single_peer_ticks >= 80);
                if retry_transient {
                    async_std::task::sleep(Duration::from_millis(HLS_PAYLOAD_RETRY_DELAY_MS)).await;
                    for index in transient {
                        self.retry(index);
                    }
                    single_peer_ticks = 0;
                } else if !transient.is_empty() {
                    single_peer_ticks = single_peer_ticks.saturating_add(1);
                }
                self.pump_once().await?;
            }
            Ok(())
        }

        async fn observe_once(&mut self, targets: &[u64]) -> Result<(), String> {
            while !self.targets_observed(targets) && self.direct_candidate.is_none() {
                self.pump_once().await?;
            }
            Ok(())
        }

        async fn observe_retained_once(&mut self, targets: &[u64]) -> Result<bool, String> {
            while !self.targets_observed(targets) {
                self.pump_once().await?;
                if self.capacity_parallelism == 0
                    && self.in_flight.is_empty()
                    && self.direct_in_flight.is_none()
                    && !self.targets_observed(targets)
                {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        async fn observe_forward_wave(
            &mut self,
            targets: &[u64],
            floor: u64,
        ) -> Result<(), String> {
            let mut single_peer_ticks = 0_u16;
            loop {
                let highest = self
                    .windows
                    .last_key_value()
                    .map(|(index, _)| *index)
                    .unwrap_or(floor);
                let unresolved = targets
                    .iter()
                    .copied()
                    .filter(|index| {
                        *index > highest
                            && !matches!(
                                self.state(*index),
                                Some(
                                    LiveHistoryProbeState::Found
                                        | LiveHistoryProbeState::Unavailable
                                )
                            )
                    })
                    .collect::<Vec<_>>();
                if unresolved.is_empty()
                    || self
                        .direct_candidate
                        .as_ref()
                        .is_some_and(|candidate| candidate.index > floor)
                {
                    return Ok(());
                }
                let transient = unresolved
                    .iter()
                    .copied()
                    .filter(|index| self.state(*index) == Some(LiveHistoryProbeState::Transient))
                    .collect::<Vec<_>>();
                let priced_peers = self.capacity_priced_peers;
                if !transient.is_empty() && (priced_peers >= 2 || single_peer_ticks >= 80) {
                    async_std::task::sleep(Duration::from_millis(HLS_PAYLOAD_RETRY_DELAY_MS)).await;
                    for index in transient {
                        self.retry(index);
                    }
                    single_peer_ticks = 0;
                } else if !transient.is_empty() {
                    single_peer_ticks = single_peer_ticks.saturating_add(1);
                }
                self.pump_once().await?;
            }
        }

        async fn admit(&mut self, index: u64) -> Result<(), String> {
            while self.state(index) == Some(LiveHistoryProbeState::Pending) {
                self.pump_once().await?;
            }
            Ok(())
        }

        async fn finish_direct(&mut self) -> Result<(), String> {
            while self.direct_in_flight.is_some() {
                self.pump_once().await?;
            }
            Ok(())
        }
    }

    fn live_history_session_is_current(
        weeb3: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        presentation_id: u64,
    ) -> bool {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.presentation_id == presentation_id
                && session.live_start
                && !session.live_history_active
                && session.feed_identity.as_ref() == Some(&identity)
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, weeb3))
        })
    }

    async fn discover_live_history_head(
        collector: &mut LiveHistoryCollector,
        initial_index: u64,
    ) -> Result<RawFeedPayload, String> {
        let mut head_index = initial_index;
        loop {
            let first_missing = head_index
                .checked_add(HLS_SPARSE_HISTORY_STRIDE)
                .ok_or_else(|| "The Live forward guard index overflowed.".to_string())?;
            let second_missing = first_missing
                .checked_add(HLS_SPARSE_HISTORY_STRIDE)
                .ok_or_else(|| "The Live forward guard index overflowed.".to_string())?;
            let guard = [first_missing, second_missing];
            collector.enqueue(guard, LiveHistoryProbeClass::Primary, true)?;
            collector.observe_forward_wave(&guard, head_index).await?;
            if collector
                .direct_candidate
                .as_ref()
                .is_some_and(|candidate| candidate.index > head_index)
                && let Some(direct) = collector.take_direct()
            {
                return Ok(direct);
            }
            if let Some(observed_head) = collector
                .windows
                .last_key_value()
                .map(|(index, _)| *index)
                .filter(|index| *index > second_missing)
            {
                head_index = observed_head;
                continue;
            }
            if guard.iter().any(|index| {
                collector.state(*index) == Some(LiveHistoryProbeState::Found)
                    && collector.windows.contains_key(index)
            }) {
                head_index = guard
                    .iter()
                    .filter(|index| collector.windows.contains_key(index))
                    .copied()
                    .max()
                    .unwrap_or(head_index);
                // Match the planned wave to the admission cap so a cold, low-peer
                // session cannot overscan hundreds of known-missing lattice slots.
                let wave = plan_hls_sparse_forward_wave(
                    head_index,
                    1,
                    collector.capacity_parallelism.max(1),
                )
                .ok_or_else(|| "The Live forward lattice index overflowed.".to_string())?;
                collector.enqueue(
                    wave.iter().rev().copied(),
                    LiveHistoryProbeClass::Primary,
                    true,
                )?;
                collector.observe_forward_wave(&wave, head_index).await?;
                if collector
                    .direct_candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.index > head_index)
                    && let Some(direct) = collector.take_direct()
                {
                    return Ok(direct);
                }
                if let Some(found) = wave
                    .iter()
                    .filter(|index| collector.windows.contains_key(index))
                    .max()
                {
                    head_index = *found;
                }
                continue;
            }
            if !guard
                .iter()
                .all(|index| collector.state(*index) == Some(LiveHistoryProbeState::Unavailable))
            {
                continue;
            }

            let dense = plan_hls_sparse_terminal_repairs(head_index)
                .ok_or_else(|| "The Live dense repair index overflowed.".to_string())?;
            collector.enqueue(dense.iter().copied(), LiveHistoryProbeClass::Primary, true)?;
            for index in dense.iter().copied() {
                collector.retry(index);
            }
            loop {
                collector.settle(&dense).await?;
                let had_direct = collector.direct_candidate.is_some();
                if let Some(direct) = collector.take_direct() {
                    return Ok(direct);
                }
                if !had_direct {
                    break;
                }
            }
            if collector.direct_required
                && collector
                    .direct_index
                    .is_some_and(|index| index > head_index)
            {
                collector.finish_direct().await?;
            }
            if let Some(direct) = collector.take_direct() {
                return Ok(direct);
            }
            if collector.direct_in_flight.is_some() {
                collector.drop_direct_work();
            }
            let dense_head = dense
                .iter()
                .filter(|index| collector.windows.contains_key(index))
                .copied()
                .max();
            if let Some(dense_head) = dense_head {
                head_index = dense_head;
                continue;
            }
            if head_index < collector.highest_authenticated_positive_index {
                return Err(
                    "The Live edge has a newer authenticated but unusable update.".to_string(),
                );
            }
            return collector
                .windows
                .get(&head_index)
                .cloned()
                .map(|bytes| RawFeedPayload {
                    index: head_index,
                    bytes,
                })
                .ok_or_else(|| "The Live head payload was not retained.".to_string());
        }
    }

    async fn assemble_live_history(
        collector: &mut LiveHistoryCollector,
        head: &RawFeedPayload,
        lattice_residue: u64,
    ) -> Result<(RawFeedPayload, Vec<u8>), String> {
        let plan = plan_hls_sparse_history_from_lattice(head.index, &head.bytes, lattice_residue)
            .ok_or_else(|| {
            "The Live feed does not expose a bounded reconstructable HLS history.".to_string()
        })?;
        if plan.requested_indices.is_empty() {
            let archive =
                assemble_hls_sparse_history(&plan, &head.bytes, std::iter::empty::<(u64, &[u8])>())
                    .ok_or_else(|| "The direct Live archive is invalid.".to_string())?;
            return Ok((head.clone(), archive));
        }
        collector.enqueue(
            plan.requested_indices.iter().copied(),
            LiveHistoryProbeClass::Primary,
            false,
        )?;
        loop {
            collector.observe_once(&plan.requested_indices).await?;
            let had_direct = collector.direct_candidate.is_some();
            if let Some(direct) = collector.take_direct() {
                return Ok((direct.clone(), direct.bytes));
            }
            if !had_direct {
                break;
            }
        }

        loop {
            if let Some(archive) = assemble_hls_sparse_history(
                &plan,
                &head.bytes,
                collector.successful_entries_before(head.index),
            ) {
                return Ok((head.clone(), archive));
            }
            let repairs = plan_hls_sparse_history_repairs_for_attempts(
                &plan,
                &head.bytes,
                collector.resolved_indices_before(head.index),
                collector.successful_entries_before(head.index),
            )
            .ok_or_else(|| {
                "The Live history contains contradictory or over-budget windows.".to_string()
            })?;
            let mut targets =
                collector.enqueue(repairs.iter().copied(), LiveHistoryProbeClass::Repair, true)?;
            for index in repairs {
                if collector.state(index) == Some(LiveHistoryProbeState::Transient) {
                    collector.retry(index);
                    targets.push(index);
                }
            }
            if targets.is_empty() {
                targets.extend(collector.states.iter().filter_map(|(index, state)| {
                    (*index < head.index && *state == LiveHistoryProbeState::Transient)
                        .then_some(*index)
                }));
                for index in targets.iter().copied() {
                    collector.retry(index);
                }
            }
            if targets.is_empty() {
                return Err("The Live history has an authenticated media-coverage gap.".to_string());
            }
            targets.sort_unstable();
            targets.dedup();
            loop {
                collector.settle(&targets).await?;
                let had_direct = collector.direct_candidate.is_some();
                if let Some(direct) = collector.take_direct() {
                    return Ok((direct.clone(), direct.bytes));
                }
                if !had_direct {
                    break;
                }
            }
        }
    }

    fn install_prepared_live_history(
        weeb3: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        presentation_id: u64,
        head: &RawFeedPayload,
        archive: Vec<u8>,
    ) -> Option<FeedRouteSnapshot> {
        if !live_history_session_is_current(weeb3, owner, topic, presentation_id)
            || hls_media_sequence(&archive) != Some(0)
            || !hls_sequence_zero_covers_head(&head.bytes, &archive)
        {
            return None;
        }
        let cache_key = sequence_zero_feed_cache_key(owner, topic, presentation_id);
        let snapshot = store_feed_snapshot(
            &cache_key,
            FeedRouteSnapshot {
                index: head.index,
                finalized: hls_is_finalized(&archive),
                body: Arc::from(archive),
            },
            true,
            FeedFollowupMode::SequenceZeroPresentation,
        );
        if snapshot.index != head.index
            || !hls_sequence_zero_covers_head(&head.bytes, &snapshot.body)
        {
            return None;
        }
        let cache_stamped = FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let state = cache.get_mut(&cache_key)?;
            if state.snapshot.index != head.index
                || state.snapshot.body.as_ref() != snapshot.body.as_ref()
                || state.deferred_source.is_some()
            {
                return None;
            }
            let now = js_sys::Date::now();
            let source_body: Arc<[u8]> = Arc::from(head.bytes.clone());
            state.source_body = source_body.clone();
            state.deferred_source = None;
            state.body_tracks_source = source_body.as_ref() == snapshot.body.as_ref();
            state.confirmed_head_index = Some(head.index);
            state.sequence_zero_recovery_cursor = 20;
            state.sequence_zero_retry_indices.clear();
            state.sequence_zero_deferred_retry_index = None;
            state.sequence_zero_retry_deferred_first = true;
            state.sequence_zero_positive_ceiling =
                state.sequence_zero_positive_ceiling.max(head.index);
            state.last_head_check = now;
            state.last_touch = now;
            state.source_endlist_confirmed =
                hls_is_finalized(&head.bytes) && hls_is_finalized(&snapshot.body);
            Some(())
        });
        cache_stamped?;
        let installed = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            if session.presentation_id == presentation_id
                && session.live_start
                && session.feed_identity.as_ref()
                    == Some(&(owner.to_ascii_lowercase(), topic.to_ascii_lowercase()))
            {
                session.live_history_active = true;
                true
            } else {
                false
            }
        });
        installed.then_some(snapshot)
    }

    async fn prepare_live_history(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        presentation_id: u64,
        initial: FeedRouteSnapshot,
        observed_deferred: Option<DeferredRawFeedPayload>,
    ) -> Result<FeedRouteSnapshot, String> {
        let network_id = active_profile().swarm_network_id;
        let initial_is_confirmed_terminal = initial.finalized;
        let initial = RawFeedPayload {
            index: initial.index,
            bytes: initial.body.to_vec(),
        };
        let initial_plan =
            plan_hls_sparse_history(initial.index, &initial.bytes).ok_or_else(|| {
                "The Live startup payload is not a supported HLS presentation.".to_string()
            })?;
        let (head, archive) = if initial_is_confirmed_terminal
            && initial_plan.requested_indices.is_empty()
        {
            (initial.clone(), initial.bytes.clone())
        } else {
            let lattice_residue = initial.index % HLS_SPARSE_HISTORY_STRIDE;
            let initial_needs_sparse_tail = !initial_is_confirmed_terminal
                && initial_plan.requested_indices.is_empty()
                && initial_plan.segment_count > HLS_SPARSE_HISTORY_STRIDE;
            let inline_direct = (initial_needs_sparse_tail && hls_is_finalized(&initial.bytes))
                .then(|| initial.clone());
            let collector_initial = if initial_needs_sparse_tail {
                RawFeedPayload {
                    index: initial.index,
                    bytes: hls_sequence_zero_sparse_tail(&initial.bytes).ok_or_else(|| {
                        "The unconfirmed Live archive has no safe sparse tail.".to_string()
                    })?,
                }
            } else {
                initial
            };
            let mut collector = LiveHistoryCollector::new(
                weeb3.clone(),
                owner.clone(),
                topic.clone(),
                presentation_id,
                network_id,
                collector_initial,
            )
            .ok_or_else(|| "The Live startup window is unsupported.".to_string())?;
            if let Some(inline_direct) = inline_direct {
                collector.adopt_inline_direct(inline_direct)?;
            }
            let direct_index = initial_plan.head_index.checked_add(1);
            let observed_replaces_direct = observed_deferred
                .as_ref()
                .is_some_and(|deferred| Some(deferred.index) == direct_index);
            let observed_is_newer = observed_deferred
                .as_ref()
                .is_some_and(|deferred| deferred.index > initial_plan.head_index);
            let mut direct_started = false;
            if !observed_replaces_direct && let Some(direct_index) = direct_index {
                collector.enqueue(
                    std::iter::once(direct_index),
                    LiveHistoryProbeClass::Primary,
                    true,
                )?;
                collector.admit(direct_index).await?;
                direct_started = true;
            }
            if observed_is_newer && let Some(observed_deferred) = observed_deferred {
                collector.start_deferred_direct(observed_deferred, true, None)?;
                direct_started = true;
            }
            if direct_started {
                collector.pump_once().await?;
            }
            let head = discover_live_history_head(&mut collector, initial_plan.head_index).await?;
            prefetch_live_snapshot_start(
                &weeb3,
                &owner,
                &topic,
                &FeedRouteSnapshot {
                    index: head.index,
                    body: Arc::from(head.bytes.clone()),
                    finalized: hls_is_finalized(&head.bytes),
                },
            );
            if hls_media_sequence(&head.bytes) == Some(0) {
                (head.clone(), head.bytes.clone())
            } else {
                assemble_live_history(&mut collector, &head, lattice_residue).await?
            }
        };
        if initial_is_confirmed_terminal && initial_plan.requested_indices.is_empty() {
            prefetch_live_snapshot_start(
                &weeb3,
                &owner,
                &topic,
                &FeedRouteSnapshot {
                    index: head.index,
                    body: Arc::from(head.bytes.clone()),
                    finalized: hls_is_finalized(&head.bytes),
                },
            );
        }
        let snapshot =
            install_prepared_live_history(&weeb3, &owner, &topic, presentation_id, &head, archive)
                .ok_or_else(|| "The Live history preparation was superseded.".to_string())?;
        prefetch_live_snapshot_start(&weeb3, &owner, &topic, &snapshot);
        Ok(snapshot)
    }

    fn feed_cache_key(owner: &str, topic: &str, index_hint: Option<u64>) -> String {
        let view = index_hint
            .map(|index| format!("index:{index:016x}"))
            .unwrap_or_else(|| "live".to_string());
        format!(
            "{}:{}:{}:{}",
            active_profile().swarm_network_id,
            owner.to_ascii_lowercase(),
            topic.to_ascii_lowercase(),
            view
        )
    }

    fn sequence_zero_feed_cache_key(owner: &str, topic: &str, presentation_id: u64) -> String {
        format!(
            "{}:sequence-zero:{presentation_id:016x}",
            feed_cache_key(owner, topic, None)
        )
    }

    enum FeedRouteBodyUpdate {
        Publish(Arc<[u8]>),
        Hold,
    }

    enum FeedCandidateAdmission {
        Seed {
            advancing: bool,
        },
        Task {
            token: u64,
            expected_index: Option<u64>,
            require_confirmed_same_index: bool,
        },
    }

    struct FeedCandidate {
        index: u64,
        source: Arc<[u8]>,
        terminal: bool,
        head_confirmed: bool,
        mode: FeedFollowupMode,
        admission: FeedCandidateAdmission,
    }

    fn feed_route_update_body(
        current: &[u8],
        current_source: &[u8],
        candidate: &Arc<[u8]>,
        followup_mode: FeedFollowupMode,
    ) -> Option<FeedRouteBodyUpdate> {
        if !is_hls_manifest(current_source) {
            return (followup_mode == FeedFollowupMode::Canonical)
                .then(|| FeedRouteBodyUpdate::Publish(candidate.clone()));
        }
        if !is_hls_manifest(candidate) {
            return None;
        }
        let continuous = hls_manifest_reload_is_continuous(current_source, candidate.as_ref());
        let adjacent_sequence_zero = !continuous
            && followup_mode == FeedFollowupMode::SequenceZeroPresentation
            && HlsTimeline::parse(current)
                .zip(HlsTimeline::parse(candidate.as_ref()))
                .is_some_and(|(current, candidate)| {
                    current.sequence == 0 && current.is_adjacent_to(&candidate)
                });
        if !continuous
            && !adjacent_sequence_zero
            && !(followup_mode == FeedFollowupMode::Canonical
                && hls_manifest_reload_is_forward(current_source, candidate.as_ref()))
        {
            return None;
        }
        match followup_mode {
            FeedFollowupMode::Canonical => Some(FeedRouteBodyUpdate::Publish(candidate.clone())),
            FeedFollowupMode::SequenceZeroPresentation
                if hls_media_sequence(current) == Some(0) =>
            {
                Some(
                    extend_hls_sequence_zero_archive(current, candidate.as_ref())
                        .map(Arc::from)
                        .map_or(FeedRouteBodyUpdate::Hold, FeedRouteBodyUpdate::Publish),
                )
            }
            FeedFollowupMode::SequenceZeroPresentation => None,
        }
    }

    fn apply_feed_candidate(
        cache_key: &str,
        candidate: FeedCandidate,
    ) -> Option<FeedRouteSnapshot> {
        let seeded = matches!(candidate.admission, FeedCandidateAdmission::Seed { .. });
        if candidate.source.len() > MAX_STREAM_FEED_PAYLOAD_BYTES
            || (!seeded && !is_hls_manifest(&candidate.source))
        {
            return None;
        }
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some(existing) = cache.get_mut(cache_key) {
                let accepted = || seeded.then(|| existing.snapshot.clone());
                if existing.snapshot.finalized || existing.snapshot.index > candidate.index {
                    existing.last_touch = js_sys::Date::now();
                    return accepted();
                }
                let completing_deferred_same_index = existing.snapshot.index == candidate.index
                    && existing.deferred_source.is_some();
                let (advancing, task) = match candidate.admission {
                    FeedCandidateAdmission::Seed { advancing } => (advancing, None),
                    FeedCandidateAdmission::Task {
                        token,
                        expected_index,
                        require_confirmed_same_index,
                    } => {
                        if existing.checking_token != token
                            || expected_index.is_some_and(|index| {
                                existing.snapshot.index != index
                                    || candidate.index < index
                                    || (candidate.index == index && !completing_deferred_same_index)
                            })
                        {
                            return None;
                        }
                        (
                            true,
                            Some((require_confirmed_same_index, candidate.head_confirmed)),
                        )
                    }
                };
                if existing.snapshot.index == candidate.index
                    && let Some(deferred) = existing.deferred_source.as_ref()
                {
                    if !hls_deferred_feed_completion_matches(
                        deferred.payload_span,
                        &deferred.authenticated_prefix,
                        &candidate.source,
                    ) || hls_media_sequence(&candidate.source) != Some(0)
                        || HlsTimeline::parse(&candidate.source).is_none()
                    {
                        return accepted();
                    }
                    let body = candidate.source.clone();
                    let finalized = candidate.terminal && hls_is_finalized(&body);
                    existing.source_body = candidate.source;
                    existing.deferred_source = None;
                    existing.body_tracks_source = true;
                    existing.source_endlist_confirmed =
                        candidate.head_confirmed && hls_is_finalized(&existing.source_body);
                    existing.snapshot = FeedRouteSnapshot {
                        index: candidate.index,
                        body,
                        finalized,
                    };
                    let now = js_sys::Date::now();
                    existing.confirmed_head_index =
                        candidate.head_confirmed.then_some(candidate.index);
                    existing.sequence_zero_recovery_cursor = 20;
                    existing.sequence_zero_retry_indices.clear();
                    existing.sequence_zero_deferred_retry_index = None;
                    existing.sequence_zero_retry_deferred_first = true;
                    existing.sequence_zero_positive_ceiling =
                        existing.sequence_zero_positive_ceiling.max(candidate.index);
                    existing.last_head_check = if candidate.head_confirmed { now } else { 0.0 };
                    existing.last_touch = now;
                    let stored = existing.snapshot.clone();
                    trim_feed_route_cache(&mut cache, cache_key);
                    return Some(stored);
                }
                if existing.snapshot.index == candidate.index {
                    if existing.source_body.as_ref() != candidate.source.as_ref()
                        || task.is_some_and(|(required, confirmed)| required && !confirmed)
                    {
                        return accepted();
                    }
                    if candidate.head_confirmed {
                        existing.source_endlist_confirmed = hls_is_finalized(&candidate.source);
                        existing.snapshot.finalized = candidate.terminal
                            && existing.body_tracks_source
                            && hls_is_finalized(&existing.snapshot.body);
                        existing.confirmed_head_index = Some(candidate.index);
                        existing.last_head_check = js_sys::Date::now();
                    } else if task.is_some_and(|(requires_confirmation, _)| !requires_confirmation)
                    {
                        existing.source_endlist_confirmed = false;
                        existing.confirmed_head_index = None;
                        existing.last_head_check = 0.0;
                    }
                    existing.last_touch = js_sys::Date::now();
                    return Some(existing.snapshot.clone());
                }
                let update = if advancing {
                    let current_source = existing
                        .deferred_source
                        .as_ref()
                        .map_or(existing.source_body.as_ref(), |_| {
                            existing.snapshot.body.as_ref()
                        });
                    let Some(update) = feed_route_update_body(
                        &existing.snapshot.body,
                        current_source,
                        &candidate.source,
                        candidate.mode,
                    ) else {
                        existing.last_touch = js_sys::Date::now();
                        return accepted();
                    };
                    update
                } else {
                    FeedRouteBodyUpdate::Publish(candidate.source.clone())
                };
                let (body, body_tracks_source) = match update {
                    FeedRouteBodyUpdate::Publish(body) => {
                        let tracks_source = body.as_ref() == candidate.source.as_ref();
                        (body, tracks_source)
                    }
                    FeedRouteBodyUpdate::Hold => (existing.snapshot.body.clone(), false),
                };
                let finalized =
                    candidate.terminal && body_tracks_source && hls_is_finalized(body.as_ref());
                existing.source_body = candidate.source;
                existing.deferred_source = None;
                existing.body_tracks_source = body_tracks_source;
                existing.source_endlist_confirmed =
                    candidate.head_confirmed && hls_is_finalized(&existing.source_body);
                existing.snapshot = FeedRouteSnapshot {
                    index: candidate.index,
                    body,
                    finalized,
                };
                let now = js_sys::Date::now();
                let proof_confirmed = if seeded {
                    finalized
                } else {
                    candidate.head_confirmed
                };
                existing.confirmed_head_index = proof_confirmed.then_some(candidate.index);
                existing.sequence_zero_recovery_cursor = 20;
                existing.sequence_zero_retry_indices.clear();
                existing.sequence_zero_deferred_retry_index = None;
                existing.sequence_zero_retry_deferred_first = true;
                existing.sequence_zero_positive_ceiling =
                    existing.sequence_zero_positive_ceiling.max(candidate.index);
                existing.last_head_check = if proof_confirmed { now } else { 0.0 };
                existing.last_touch = now;
                let stored = existing.snapshot.clone();
                trim_feed_route_cache(&mut cache, cache_key);
                return Some(stored);
            }
            let FeedCandidateAdmission::Seed { .. } = candidate.admission else {
                return None;
            };
            let snapshot = FeedRouteSnapshot {
                index: candidate.index,
                body: candidate.source.clone(),
                finalized: candidate.terminal,
            };
            let now = js_sys::Date::now();
            cache.insert(
                cache_key.to_string(),
                FeedRouteState {
                    snapshot: snapshot.clone(),
                    source_body: candidate.source,
                    deferred_source: None,
                    body_tracks_source: true,
                    source_endlist_confirmed: candidate.head_confirmed,
                    checking_token: 0,
                    confirmed_head_index: candidate.head_confirmed.then_some(candidate.index),
                    sequence_zero_recovery_cursor: 20,
                    sequence_zero_retry_indices: VecDeque::new(),
                    sequence_zero_deferred_retry_index: None,
                    sequence_zero_retry_deferred_first: true,
                    sequence_zero_positive_ceiling: candidate.index,
                    last_head_check: if candidate.head_confirmed { now } else { 0.0 },
                    last_touch: now,
                },
            );
            trim_feed_route_cache(&mut cache, cache_key);
            Some(snapshot)
        })
    }

    fn store_feed_snapshot(
        cache_key: &str,
        snapshot: FeedRouteSnapshot,
        advancing: bool,
        mode: FeedFollowupMode,
    ) -> FeedRouteSnapshot {
        apply_feed_candidate(
            cache_key,
            FeedCandidate {
                index: snapshot.index,
                source: snapshot.body,
                terminal: snapshot.finalized,
                head_confirmed: snapshot.finalized,
                mode,
                admission: FeedCandidateAdmission::Seed { advancing },
            },
        )
        .expect("a feed seed always returns a snapshot")
    }

    fn store_deferred_sequence_zero_snapshot(
        cache_key: &str,
        presentation: DeferredSequenceZeroPresentation,
    ) -> Option<FeedRouteSnapshot> {
        if u64::try_from(presentation.source.authenticated_prefix.len())
            .ok()
            .is_none_or(|prefix_len| presentation.source.payload_span <= prefix_len)
            || !presentation
                .source
                .authenticated_prefix
                .starts_with(&presentation.body)
            || hls_media_sequence(&presentation.body) != Some(0)
            || hls_media_references(&presentation.body).len()
                < HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS
            || hls_is_finalized(&presentation.body)
        {
            return None;
        }
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.contains_key(cache_key) {
                return None;
            }
            let snapshot = FeedRouteSnapshot {
                index: presentation.index,
                body: presentation.body,
                finalized: false,
            };
            let now = js_sys::Date::now();
            cache.insert(
                cache_key.to_string(),
                FeedRouteState {
                    snapshot: snapshot.clone(),
                    source_body: Arc::from(Vec::<u8>::new()),
                    deferred_source: Some(presentation.source),
                    body_tracks_source: false,
                    source_endlist_confirmed: false,
                    checking_token: 0,
                    confirmed_head_index: None,
                    sequence_zero_recovery_cursor: 20,
                    sequence_zero_retry_indices: VecDeque::new(),
                    sequence_zero_deferred_retry_index: None,
                    sequence_zero_retry_deferred_first: true,
                    sequence_zero_positive_ceiling: presentation.index,
                    last_head_check: 0.0,
                    last_touch: now,
                },
            );
            trim_feed_route_cache(&mut cache, cache_key);
            Some(snapshot)
        })
    }

    fn active_live_history_feed_cache_key() -> Option<String> {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            let (owner, topic) = session.feed_identity.as_ref()?;
            (session.live_start && session.live_history_active)
                .then(|| sequence_zero_feed_cache_key(owner, topic, session.presentation_id))
        })
    }

    pub(super) fn active_hls_live_snapshot_index() -> Option<u64> {
        let cache_key = active_live_history_feed_cache_key()?;
        FEED_ROUTE_CACHE.with(|cache| {
            cache
                .borrow()
                .get(&cache_key)
                .map(|state| state.snapshot.index)
        })
    }

    pub(super) fn active_hls_finalized_playable_presentation()
    -> Option<((f64, f64), (u64, String, f64, f64))> {
        let cache_key = active_live_history_feed_cache_key()?;
        let snapshot = FEED_ROUTE_CACHE.with(|cache| {
            cache
                .borrow()
                .get(&cache_key)
                .map(|state| state.snapshot.clone())
        })?;
        let fallback =
            HLS_PLAYBACK.with(|playback| playback.borrow().session.live_tail_fallback.clone());
        let presented = match fallback {
            Some(fallback) => Cow::Owned(present_hls_live_fallback(
                &snapshot.body,
                fallback.sequence,
                &fallback.reference,
                fallback.hidden_end_sequence,
            )?),
            None => Cow::Borrowed(snapshot.body.as_ref()),
        };
        hls_is_finalized(&presented).then_some(())?;
        let edge = hls_last_playable_segment(&presented)?;
        let interval = (edge.2, edge.2 + edge.3);
        (interval.1.is_finite() && interval.1 > interval.0).then_some((interval, edge))
    }

    pub(super) fn active_hls_finalized_playable_interval() -> Option<(f64, f64)> {
        active_hls_finalized_playable_presentation().map(|(interval, _)| interval)
    }

    pub(super) fn handoff_hls_codec_bootstrap_prefetch_timeline() {
        let now = hls_monotonic_now_ms();
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            session.timeline_rebasing = false;
            if let Some(now) = now {
                session.startup_deadline_ms = now + HLS_INITIAL_RESPONSE_BUDGET_MS;
            }
            if session.mode != HlsPrefetchMode::Sustained {
                session.mode = HlsPrefetchMode::StartupOnly;
            }
        });
        // Keep every admitted plan and owner alive. Dispatched Bee retrieves retain their
        // receivers and complete the normal timeout/late-response accounting path.
    }

    pub(super) fn install_hls_live_tail_fallback(
        expected_snapshot_index: u64,
        sequence: u64,
        reference: &str,
    ) -> bool {
        let reference = reference.to_ascii_lowercase();
        let Some(cache_key) = active_live_history_feed_cache_key() else {
            return false;
        };
        let Some(snapshot) = FEED_ROUTE_CACHE.with(|cache| {
            cache
                .borrow()
                .get(&cache_key)
                .map(|state| state.snapshot.clone())
        }) else {
            return false;
        };
        if snapshot.index != expected_snapshot_index {
            return false;
        }

        let existing =
            HLS_PLAYBACK.with(|playback| playback.borrow().session.live_tail_fallback.clone());
        let current = match existing.as_ref() {
            Some(fallback) => match present_hls_live_fallback(
                &snapshot.body,
                fallback.sequence,
                &fallback.reference,
                fallback.hidden_end_sequence,
            ) {
                Some(current) => current,
                None => return false,
            },
            None => snapshot.body.to_vec(),
        };
        let Some((window_sequence, window_reference)) =
            hls_live_retreat_boundary(&current, sequence, &reference)
        else {
            return false;
        };
        let Some(snapshot_timeline) = HlsTimeline::parse(&snapshot.body) else {
            return false;
        };
        let Some(snapshot_end) = snapshot_timeline.end() else {
            return false;
        };
        let (cutoff_sequence, cutoff_reference, hidden_end_sequence) = match existing.as_ref() {
            Some(fallback) if sequence >= fallback.hidden_end_sequence => {
                (fallback.sequence, fallback.reference.clone(), snapshot_end)
            }
            Some(fallback) if window_sequence < fallback.sequence => (
                window_sequence,
                window_reference,
                fallback.hidden_end_sequence,
            ),
            Some(_) => return false,
            None => (window_sequence, window_reference, snapshot_end),
        };

        let retreats = existing
            .as_ref()
            .map_or(1, |fallback| fallback.retreats.saturating_add(1));
        let hidden_duration = snapshot_timeline
            .duration_between(cutoff_sequence, hidden_end_sequence)
            .filter(|duration| *duration <= HLS_LIVE_TAIL_FALLBACK_MAX_HIDDEN_SECONDS);
        if retreats > HLS_LIVE_TAIL_FALLBACK_MAX_RETREATS
            || hidden_duration.is_none()
            || present_hls_live_fallback(
                &snapshot.body,
                cutoff_sequence,
                &cutoff_reference,
                hidden_end_sequence,
            )
            .is_none()
        {
            return false;
        }
        let client = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &playback.session;
            if !session.live_start || !session.live_history_active {
                return None;
            }
            let client = session.client.clone();
            playback.session.live_tail_fallback = Some(HlsLiveTailFallback {
                snapshot_index: snapshot.index,
                sequence: cutoff_sequence,
                reference: cutoff_reference.clone(),
                hidden_end_sequence,
                retreats,
            });
            client
        });
        let Some(client) = client else {
            return false;
        };
        HLS_CODEC_BOOTSTRAP_PRESENTATION.with(|presentation| {
            if let Some(current) = presentation.borrow_mut().as_mut()
                && current.phase != HlsCodecBootstrapPhase::Bootstrap
            {
                current.phase = HlsCodecBootstrapPhase::ContinuationOpen;
                current.snapshot = None;
            }
        });
        client.interface_log(format!(
            "HLS Live startup could not load segment {sequence} {reference}; hid the authenticated range {cutoff_sequence}..{hidden_end_sequence} ({:.1}s, fallback {retreats}/{HLS_LIVE_TAIL_FALLBACK_MAX_RETREATS})",
            hidden_duration.unwrap_or_default()
        ));
        true
    }

    fn apply_hls_live_tail_fallback(
        owner: &str,
        topic: &str,
        mut snapshot: FeedRouteSnapshot,
    ) -> FeedRouteSnapshot {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        let fallback = HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            (session.live_start && session.feed_identity.as_ref() == Some(&identity))
                .then(|| session.live_tail_fallback.clone())
                .flatten()
        });
        let Some(fallback) = fallback else {
            return snapshot;
        };
        if let Some(body) = present_hls_live_fallback(
            &snapshot.body,
            fallback.sequence,
            &fallback.reference,
            fallback.hidden_end_sequence,
        ) {
            HLS_PLAYBACK.with(|playback| {
                let mut playback = playback.borrow_mut();
                if let Some(current) = playback.session.live_tail_fallback.as_mut()
                    && current.sequence == fallback.sequence
                    && current.reference == fallback.reference
                    && snapshot.index >= current.snapshot_index
                {
                    current.snapshot_index = snapshot.index;
                }
            });
            snapshot.body = Arc::from(body);
            return snapshot;
        }
        let client = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            if playback.session.feed_identity.as_ref() != Some(&identity)
                || playback
                    .session
                    .live_tail_fallback
                    .as_ref()
                    .is_none_or(|current| {
                        current.sequence != fallback.sequence
                            || current.reference != fallback.reference
                    })
            {
                return None;
            }
            let client = playback.session.client.clone();
            playback.session.live_tail_fallback = None;
            client
        });
        if let Some(client) = client {
            client.interface_log(format!(
                "HLS Live fallback retired at feed index {} because its authenticated cutoff changed",
                snapshot.index
            ));
        }
        snapshot
    }

    fn trim_feed_route_cache(cache: &mut HashMap<String, FeedRouteState>, protected_key: &str) {
        let active_live_history_key = active_live_history_feed_cache_key();
        loop {
            let total_bytes = cache
                .values()
                .map(|state| {
                    state
                        .snapshot
                        .body
                        .len()
                        .saturating_add(state.source_body.len())
                        .saturating_add(state.deferred_source.as_ref().map_or(0, |deferred| {
                            deferred
                                .authenticated_prefix
                                .len()
                                .saturating_add(deferred.deferred.retained_bytes())
                        }))
                })
                .sum::<usize>();
            if cache.len() <= FEED_ROUTE_CACHE_MAX_ENTRIES
                && total_bytes <= FEED_ROUTE_CACHE_MAX_BYTES
            {
                return;
            }

            let Some(oldest) = cache
                .iter()
                .filter(|(key, state)| {
                    key.as_str() != protected_key
                        && active_live_history_key.as_ref() != Some(*key)
                        && state.checking_token == 0
                })
                .min_by(|left, right| left.1.last_touch.total_cmp(&right.1.last_touch))
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            cache.remove(&oldest);
        }
    }

    fn next_feed_route_check_token() -> u64 {
        FEED_ROUTE_CACHE.with(|registry| {
            let mut registry = registry.borrow_mut();
            let next = next_nonzero_generation(registry.next_task);
            registry.next_task = next;
            next
        })
    }

    fn claim_feed_route_check(cache_key: &str, required_index: Option<u64>) -> Option<(u64, u64)> {
        let token = next_feed_route_check_token();
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let state = cache.get_mut(cache_key)?;
            if state.snapshot.finalized
                || state.checking_token != 0
                || state.snapshot.index == u64::MAX
                || required_index.is_some_and(|index| state.snapshot.index != index)
            {
                return None;
            }
            state.checking_token = token;
            Some((state.snapshot.index, token))
        })
    }

    fn release_feed_route_check(cache_key: &str, token: u64) -> Option<(bool, u64)> {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let released = {
                let state = cache.get_mut(cache_key)?;
                if state.checking_token != token {
                    return None;
                }
                state.checking_token = 0;
                (state.snapshot.finalized, state.snapshot.index)
            };
            trim_feed_route_cache(&mut cache, cache_key);
            Some(released)
        })
    }

    fn deferred_feed_route_source(
        cache_key: &str,
        checking_token: u64,
    ) -> Option<DeferredFeedRouteSource> {
        FEED_ROUTE_CACHE.with(|cache| {
            let cache = cache.borrow();
            let state = cache.get(cache_key)?;
            let source = state.deferred_source.as_ref()?;
            (state.checking_token == checking_token
                && state.snapshot.index == source.deferred.index
                && state.source_body.is_empty()
                && source
                    .authenticated_prefix
                    .starts_with(&state.snapshot.body))
            .then(|| source.clone())
        })
    }

    fn deferred_feed_route_source_is_current(
        cache_key: &str,
        checking_token: u64,
        authenticated_prefix: &Arc<[u8]>,
    ) -> bool {
        FEED_ROUTE_CACHE.with(|cache| {
            cache.borrow().get(cache_key).is_some_and(|state| {
                state.checking_token == checking_token
                    && state.source_body.is_empty()
                    && state.deferred_source.as_ref().is_some_and(|source| {
                        Arc::ptr_eq(&source.authenticated_prefix, authenticated_prefix)
                            && state.snapshot.index == source.deferred.index
                            && source
                                .authenticated_prefix
                                .starts_with(&state.snapshot.body)
                    })
            })
        })
    }

    fn discard_deferred_feed_route(cache_key: &str, checking_token: u64) -> bool {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let removable = cache.get(cache_key).is_some_and(|state| {
                state.checking_token == checking_token && state.deferred_source.is_some()
            });
            removable && cache.remove(cache_key).is_some()
        })
    }

    async fn complete_deferred_feed_route(
        weeb3: Arc<Weeb3>,
        cache_key: &str,
        owner: &str,
        topic: &str,
        network_id: u64,
        checking_token: u64,
        task: &FeedTask,
    ) -> bool {
        let Some(source) = deferred_feed_route_source(cache_key, checking_token) else {
            return true;
        };
        let source_index = source.deferred.index;
        let authenticated_prefix = source.authenticated_prefix.clone();
        let gate_client = weeb3.clone();
        let gate_cache_key = cache_key.to_string();
        let gate_owner = owner.to_string();
        let gate_topic = topic.to_string();
        let payload = acquire_deferred_raw_feed_payload_conservative(
            source.deferred,
            MAX_STREAM_FEED_PAYLOAD_BYTES,
            &weeb3.chunk_port.0,
            move || {
                let client = gate_client.clone();
                let cache_key = gate_cache_key.clone();
                let owner = gate_owner.clone();
                let topic = gate_topic.clone();
                let authenticated_prefix = authenticated_prefix.clone();
                async move {
                    let deadline =
                        js_sys::Date::now() + HLS_FEED_WAVE_CREDIT_WAIT.as_millis() as f64;
                    let required = HLS_FEED_WAVE_FOREGROUND_MARGIN_CHUNKS
                        .saturating_add(CONSERVATIVE_DEFERRED_MAX_PHYSICAL_ATTEMPTS);
                    loop {
                        if active_profile().swarm_network_id != network_id
                            || !sequence_zero_followup_is_current(
                                &client, &cache_key, &owner, &topic,
                            )
                            || !deferred_feed_route_source_is_current(
                                &cache_key,
                                checking_token,
                                &authenticated_prefix,
                            )
                        {
                            return false;
                        }
                        let (current_network, available_slots) =
                            join(client.get_network_id(), client.available_retrieve_slots()).await;
                        if current_network == network_id
                            && available_slots >= required
                            && deferred_feed_route_source_is_current(
                                &cache_key,
                                checking_token,
                                &authenticated_prefix,
                            )
                        {
                            return true;
                        }
                        if js_sys::Date::now() >= deadline {
                            return false;
                        }
                        async_std::task::sleep(Duration::from_millis(50)).await;
                    }
                }
            },
        )
        .await;

        let completed = payload
            .is_some_and(|payload| payload.index == source_index && task.publish(&payload, false));
        completed
            || !deferred_feed_route_source_is_current(
                cache_key,
                checking_token,
                &source.authenticated_prefix,
            )
    }

    #[derive(Clone)]
    struct SequenceZeroFollowupSeed {
        index: u64,
        source_body: Arc<[u8]>,
        snapshot_body: Arc<[u8]>,
        tentative_terminal: bool,
        scan_initialized: bool,
        recovery_cursor: u64,
        retry_indices: VecDeque<HlsSequenceZeroRetry>,
        deferred_retry_index: Option<u64>,
        retry_deferred_first: bool,
        positive_ceiling: u64,
    }

    enum SequenceZeroFollowupHead {
        Sparse {
            payload: RawFeedPayload,
            continue_catchup: bool,
        },
        Direct(RawFeedPayload),
        WarmScan {
            next_recovery_cursor: u64,
            retry_indices: VecDeque<HlsSequenceZeroRetry>,
            deferred_retry_index: Option<u64>,
        },
    }

    fn sequence_zero_followup_seed(
        cache_key: &str,
        checking_token: u64,
        expected_index: u64,
    ) -> Option<SequenceZeroFollowupSeed> {
        FEED_ROUTE_CACHE.with(|cache| {
            let cache = cache.borrow();
            let state = cache.get(cache_key)?;
            if state.checking_token != checking_token
                || state.snapshot.index != expected_index
                || state.snapshot.finalized
                || state.deferred_source.is_some()
                || hls_media_sequence(&state.snapshot.body) != Some(0)
            {
                return None;
            }
            Some(SequenceZeroFollowupSeed {
                index: expected_index,
                source_body: state.source_body.clone(),
                snapshot_body: state.snapshot.body.clone(),
                tentative_terminal: !state.source_endlist_confirmed
                    && hls_is_finalized(&state.source_body)
                    && hls_is_finalized(&state.snapshot.body),
                scan_initialized: state.last_head_check.is_finite() && state.last_head_check > 0.0,
                recovery_cursor: state.sequence_zero_recovery_cursor.max(20),
                retry_indices: state.sequence_zero_retry_indices.clone(),
                deferred_retry_index: state.sequence_zero_deferred_retry_index,
                retry_deferred_first: state.sequence_zero_retry_deferred_first,
                positive_ceiling: state.sequence_zero_positive_ceiling.max(expected_index),
            })
        })
    }

    fn initialize_sequence_zero_followup_scan(
        cache_key: &str,
        checking_token: u64,
        seed: &SequenceZeroFollowupSeed,
    ) {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let Some(state) = cache.get_mut(cache_key) else {
                return;
            };
            if state.checking_token == checking_token
                && state.snapshot.index == seed.index
                && Arc::ptr_eq(&state.source_body, &seed.source_body)
                && Arc::ptr_eq(&state.snapshot.body, &seed.snapshot_body)
                && (!state.last_head_check.is_finite() || state.last_head_check <= 0.0)
            {
                let now = js_sys::Date::now();
                state.last_head_check = now;
                state.last_touch = now;
            }
        });
    }

    fn warm_sequence_zero_followup_scan(
        cache_key: &str,
        checking_token: u64,
        seed: &SequenceZeroFollowupSeed,
        next_recovery_cursor: u64,
        retry_indices: VecDeque<HlsSequenceZeroRetry>,
        deferred_retry_index: Option<u64>,
        positive_ceiling: u64,
    ) {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let Some(state) = cache.get_mut(cache_key) else {
                return;
            };
            if state.checking_token == checking_token
                && state.snapshot.index == seed.index
                && Arc::ptr_eq(&state.source_body, &seed.source_body)
                && Arc::ptr_eq(&state.snapshot.body, &seed.snapshot_body)
            {
                let now = js_sys::Date::now();
                state.sequence_zero_recovery_cursor = next_recovery_cursor;
                state.sequence_zero_retry_indices = retry_indices;
                state.sequence_zero_deferred_retry_index = deferred_retry_index;
                if state.sequence_zero_deferred_retry_index.is_none()
                    || !state
                        .sequence_zero_retry_indices
                        .iter()
                        .any(|retry| retry.authenticated)
                {
                    state.sequence_zero_retry_deferred_first = true;
                }
                state.sequence_zero_positive_ceiling =
                    state.sequence_zero_positive_ceiling.max(positive_ceiling);
                state.last_head_check = now;
                state.last_touch = now;
            }
        });
    }

    fn persist_sequence_zero_followup_observation(
        cache_key: &str,
        checking_token: u64,
        seed: &SequenceZeroFollowupSeed,
        collector: &LiveHistoryCollector,
    ) {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let Some(state) = cache.get_mut(cache_key) else {
                return;
            };
            if state.checking_token == checking_token
                && state.snapshot.index == seed.index
                && Arc::ptr_eq(&state.source_body, &seed.source_body)
                && Arc::ptr_eq(&state.snapshot.body, &seed.snapshot_body)
            {
                state.sequence_zero_retry_indices = collector.followup_retry_indices.clone();
                state.sequence_zero_deferred_retry_index = collector.followup_deferred_retry_index;
                if state.sequence_zero_deferred_retry_index.is_none()
                    || !state
                        .sequence_zero_retry_indices
                        .iter()
                        .any(|retry| retry.authenticated)
                {
                    state.sequence_zero_retry_deferred_first = true;
                }
                state.sequence_zero_positive_ceiling = state
                    .sequence_zero_positive_ceiling
                    .max(collector.highest_authenticated_positive_index);
                state.last_touch = js_sys::Date::now();
            }
        });
    }

    fn advance_sequence_zero_retry_turn(
        cache_key: &str,
        checking_token: u64,
        seed: &SequenceZeroFollowupSeed,
    ) {
        if seed.deferred_retry_index.is_none()
            || !seed.retry_indices.iter().any(|retry| retry.authenticated)
        {
            return;
        }
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let Some(state) = cache.get_mut(cache_key) else {
                return;
            };
            if state.checking_token == checking_token
                && state.snapshot.index == seed.index
                && Arc::ptr_eq(&state.source_body, &seed.source_body)
                && Arc::ptr_eq(&state.snapshot.body, &seed.snapshot_body)
            {
                state.sequence_zero_retry_deferred_first = !seed.retry_deferred_first;
            }
        });
    }

    fn finish_sequence_zero_terminal_confirmation(
        weeb3: &Arc<Weeb3>,
        cache_key: &str,
        owner: &str,
        topic: &str,
        network_id: u64,
        checking_token: u64,
        seed: &SequenceZeroFollowupSeed,
        positive_indices: &[u64],
        promote: bool,
    ) -> Option<bool> {
        if active_profile().swarm_network_id != network_id
            || !sequence_zero_followup_is_current(weeb3, cache_key, owner, topic)
        {
            return None;
        }
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let promoted = {
                let state = cache.get_mut(cache_key)?;
                if state.checking_token != checking_token
                    || state.snapshot.index != seed.index
                    || state.deferred_source.is_some()
                    || !Arc::ptr_eq(&state.source_body, &seed.source_body)
                    || !Arc::ptr_eq(&state.snapshot.body, &seed.snapshot_body)
                    || state.snapshot.finalized
                    || state.source_endlist_confirmed
                    || !hls_is_finalized(&state.source_body)
                    || !hls_is_finalized(&state.snapshot.body)
                {
                    return None;
                }
                let now = js_sys::Date::now();
                let prior_positive = state.sequence_zero_positive_ceiling > seed.index
                    || state
                        .sequence_zero_deferred_retry_index
                        .is_some_and(|index| index > seed.index)
                    || state
                        .sequence_zero_retry_indices
                        .iter()
                        .any(|retry| retry.authenticated && retry.index > seed.index);
                let promote = promote && positive_indices.is_empty() && !prior_positive;
                if promote {
                    state.snapshot.finalized = true;
                    state.source_endlist_confirmed = true;
                    state.confirmed_head_index = Some(seed.index);
                    state.sequence_zero_recovery_cursor = 20;
                    state.sequence_zero_retry_indices.clear();
                    state.sequence_zero_deferred_retry_index = None;
                    state.sequence_zero_retry_deferred_first = true;
                    state.sequence_zero_positive_ceiling =
                        state.sequence_zero_positive_ceiling.max(seed.index);
                } else {
                    let highest = positive_indices.iter().copied().max();
                    for index in positive_indices.iter().copied() {
                        let _ = remember_hls_sequence_zero_retry(
                            &mut state.sequence_zero_retry_indices,
                            index,
                            true,
                            Some(index) == highest,
                            HLS_SEQUENCE_ZERO_RETRY_BACKLOG_MAX,
                        );
                    }
                    if let Some(highest) = highest {
                        state.sequence_zero_positive_ceiling =
                            state.sequence_zero_positive_ceiling.max(highest);
                    }
                }
                state.last_head_check = now;
                state.last_touch = now;
                promote
            };
            trim_feed_route_cache(&mut cache, cache_key);
            Some(promoted)
        })
    }

    async fn confirm_tentative_sequence_zero_terminal(
        weeb3: Arc<Weeb3>,
        cache_key: &str,
        owner: &str,
        topic: &str,
        network_id: u64,
        checking_token: u64,
        seed: &SequenceZeroFollowupSeed,
    ) {
        let Some(targets) = plan_hls_sequence_zero_terminal_confirmation(seed.index) else {
            let _ = finish_sequence_zero_terminal_confirmation(
                &weeb3,
                cache_key,
                owner,
                topic,
                network_id,
                checking_token,
                seed,
                &[],
                false,
            );
            return;
        };
        let gate = SequenceZeroFollowupGate {
            weeb3: weeb3.clone(),
            cache_key: cache_key.to_string(),
            owner: owner.to_string(),
            topic: topic.to_string(),
            network_id,
            checking_token,
            index: seed.index,
            source_body: seed.source_body.clone(),
            snapshot_body: seed.snapshot_body.clone(),
        };
        if !gate.admitted().await {
            let _ = finish_sequence_zero_terminal_confirmation(
                &weeb3,
                cache_key,
                owner,
                topic,
                network_id,
                checking_token,
                seed,
                &[],
                false,
            );
            return;
        }

        let mut probes = FuturesUnordered::new();
        for index in targets {
            let client = weeb3.clone();
            let owner = owner.to_string();
            let topic = topic.to_string();
            probes.push(async move {
                let result = client
                    .hls_feed_payload_at_index_followup_retained_status(owner, topic, index)
                    .await;
                (index, result)
            });
        }
        let mut positive_indices = Vec::new();
        let mut all_missing = true;
        while let Some((index, result)) = probes.next().await {
            match result {
                RetainedRawFeedPayloadProbe::Found(_)
                | RetainedRawFeedPayloadProbe::Deferred(_) => {
                    positive_indices.push(index);
                    all_missing = false;
                }
                RetainedRawFeedPayloadProbe::Missing => {}
                RetainedRawFeedPayloadProbe::Transient => all_missing = false,
            }
        }
        if weeb3.get_network_id().await != network_id || !gate.current() {
            return;
        }
        let promoted = finish_sequence_zero_terminal_confirmation(
            &weeb3,
            cache_key,
            owner,
            topic,
            network_id,
            checking_token,
            seed,
            &positive_indices,
            all_missing,
        )
        .unwrap_or(false);
        if promoted {
            remember_authenticated_endlist_index(network_id, owner, topic, seed.index);
        }
    }

    fn take_sequence_zero_followup_direct(
        collector: &mut LiveHistoryCollector,
        initial_index: u64,
        initial_archive: &[u8],
    ) -> Result<Option<RawFeedPayload>, String> {
        let had_direct = collector.direct_candidate.is_some();
        let direct = collector.take_direct();
        if !had_direct {
            return Ok(None);
        }
        let Some(direct) = direct else {
            return Ok(None);
        };
        if direct.index < initial_index
            || hls_media_sequence(&direct.bytes) != Some(0)
            || !hls_sequence_zero_covers_head(initial_archive, &direct.bytes)
        {
            return Err(
                "The authenticated terminal Live archive contradicts the active timeline."
                    .to_string(),
            );
        }
        collector.forget_followup_retry_index(direct.index);
        collector.forget_deferred_followup_retry_index(direct.index);
        Ok(Some(direct))
    }

    async fn decode_selected_sequence_zero_terminal(
        collector: &mut LiveHistoryCollector,
    ) -> Result<(), String> {
        let Some(gate) = collector.sequence_zero_followup_gate() else {
            return Ok(());
        };
        let Some(deferred) = collector.selected_deferred_followup_candidate().cloned() else {
            return Ok(());
        };
        let deferred_index = deferred.index;
        if !gate.admitted().await {
            collector.retry_deferred_followup_candidate(deferred_index);
            return Ok(());
        }

        let chunk_port = collector.weeb3.chunk_port.0.clone();
        let mut tail = Box::pin(probe_deferred_raw_feed_payload_tail_conservative(
            &deferred,
            crate::erasure_coding::CHUNK_SIZE,
            &chunk_port,
        ));
        let tail = loop {
            if !gate.current() {
                collector.retry_deferred_followup_candidate(deferred_index);
                return Ok(());
            }
            if let Some(tail) = tail.as_mut().now_or_never() {
                break tail;
            }
            async_std::task::sleep(Duration::from_millis(25)).await;
        };
        let Some(tail) = tail else {
            collector.retry_deferred_followup_candidate(deferred_index);
            return Ok(());
        };
        if !hls_tail_has_terminal_endlist(&tail) {
            collector.take_selected_deferred_followup_candidate();
            collector
                .states
                .insert(deferred_index, LiveHistoryProbeState::Unsupported);
            collector.forget_deferred_followup_retry_index(deferred_index);
            return Ok(());
        }
        if !gate.admitted().await {
            collector.retry_deferred_followup_candidate(deferred_index);
            return Ok(());
        }
        let Some(deferred) = collector.take_selected_deferred_followup_candidate() else {
            return Ok(());
        };
        collector.start_deferred_direct(deferred, true, Some(gate))?;
        collector.finish_direct().await?;
        if collector.state(deferred_index) == Some(LiveHistoryProbeState::Transient) {
            collector.retry_deferred_followup_candidate(deferred_index);
        } else {
            collector.forget_deferred_followup_retry_index(deferred_index);
        }
        Ok(())
    }

    fn highest_appendable_sequence_zero_followup_head(
        collector: &LiveHistoryCollector,
        initial_index: u64,
        initial_archive: &[u8],
    ) -> Option<RawFeedPayload> {
        let initial_timeline = hls_complete_history_timeline(initial_archive)?;
        if initial_timeline.sequence != 0 {
            return None;
        }
        let mut segments = initial_timeline.segments;
        let mut candidates = collector
            .windows
            .iter()
            .filter(|(index, _)| **index > initial_index)
            .filter_map(|(index, bytes)| {
                hls_sparse_history_timeline(bytes).map(|timeline| (*index, bytes, timeline))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(index, _, timeline)| {
            (
                timeline.sequence,
                timeline.end().unwrap_or(u64::MAX),
                *index,
            )
        });
        let mut highest = None;
        for (index, bytes, timeline) in candidates {
            let current_end = u64::try_from(segments.len()).ok()?;
            let candidate_end = timeline.end()?;
            if timeline.sequence > current_end {
                break;
            }
            let overlap_end = current_end.min(candidate_end);
            if (timeline.sequence..overlap_end).any(|sequence| {
                let current = usize::try_from(sequence).ok();
                let incoming = usize::try_from(sequence - timeline.sequence).ok();
                current.zip(incoming).is_none_or(|(current, incoming)| {
                    segments.get(current) != timeline.segments.get(incoming)
                })
            }) {
                break;
            }
            if candidate_end > current_end {
                let position = usize::try_from(current_end - timeline.sequence).ok()?;
                segments.extend_from_slice(timeline.segments.get(position..)?);
            }
            if highest
                .as_ref()
                .is_none_or(|payload: &RawFeedPayload| index > payload.index)
            {
                highest = Some(RawFeedPayload {
                    index,
                    bytes: bytes.clone(),
                });
            }
            if hls_is_finalized(bytes) {
                break;
            }
        }
        highest
    }

    async fn discover_sequence_zero_followup_head(
        collector: &mut LiveHistoryCollector,
        initial_index: u64,
        initial_archive: &[u8],
        refresh_head: bool,
        scan_initialized: bool,
        recovery_cursor: u64,
        deferred_retry_index: Option<u64>,
        ordinary_retry: Option<HlsSequenceZeroRetry>,
        retry_deferred_first: bool,
    ) -> Result<Option<SequenceZeroFollowupHead>, String> {
        let mut probe_index = initial_index;
        let mut guard_waves = 0usize;
        if let Some(bytes) = collector
            .windows
            .get(&initial_index)
            .filter(|bytes| hls_is_finalized(bytes))
            .filter(|_| initial_index >= collector.highest_authenticated_positive_index)
            .cloned()
        {
            return Ok(Some(SequenceZeroFollowupHead::Sparse {
                payload: RawFeedPayload {
                    index: initial_index,
                    bytes,
                },
                continue_catchup: false,
            }));
        }
        if let Some(direct) =
            take_sequence_zero_followup_direct(collector, initial_index, initial_archive)?
        {
            return Ok(Some(SequenceZeroFollowupHead::Direct(direct)));
        }
        loop {
            let first_guard = probe_index
                .checked_add(HLS_SPARSE_HISTORY_STRIDE)
                .ok_or_else(|| "The Live follow-up guard index overflowed.".to_string())?;
            let second_guard = first_guard
                .checked_add(HLS_SPARSE_HISTORY_STRIDE)
                .ok_or_else(|| "The Live follow-up guard index overflowed.".to_string())?;
            let guard = [first_guard, second_guard];
            collector.enqueue(guard, LiveHistoryProbeClass::Primary, true)?;
            if !collector.observe_retained_once(&guard).await? {
                return Ok(None);
            }
            if let Some(direct) =
                take_sequence_zero_followup_direct(collector, initial_index, initial_archive)?
            {
                return Ok(Some(SequenceZeroFollowupHead::Direct(direct)));
            }

            let found = guard
                .iter()
                .filter(|index| collector.state(**index) == Some(LiveHistoryProbeState::Found))
                .filter_map(|index| {
                    collector
                        .windows
                        .get(index)
                        .map(|bytes| (*index, hls_is_finalized(bytes)))
                })
                .max_by_key(|(index, _)| *index);
            if let Some((found_index, _)) = found {
                probe_index = found_index;
                guard_waves = guard_waves.saturating_add(1);
            }
            if found.as_ref().is_some_and(|(_, finalized)| *finalized)
                && let Some(head) = highest_appendable_sequence_zero_followup_head(
                    collector,
                    initial_index,
                    initial_archive,
                )
                && hls_is_finalized(&head.bytes)
                && head.index >= collector.highest_authenticated_positive_index
            {
                return Ok(Some(SequenceZeroFollowupHead::Sparse {
                    payload: head,
                    continue_catchup: false,
                }));
            }
            let guard_blocked = guard.iter().any(|index| {
                matches!(
                    collector.state(*index),
                    Some(LiveHistoryProbeState::Transient | LiveHistoryProbeState::Unsupported)
                )
            });
            if guard_blocked {
                let appendable = (guard_waves > 0)
                    .then(|| {
                        highest_appendable_sequence_zero_followup_head(
                            collector,
                            initial_index,
                            initial_archive,
                        )
                    })
                    .flatten();
                if let Some(payload) = appendable.filter(|payload| {
                    !hls_is_finalized(&payload.bytes)
                        || payload.index >= collector.highest_authenticated_positive_index
                }) {
                    let remembered_later = collector.remember_followup_windows_after(payload.index);
                    return Ok(Some(SequenceZeroFollowupHead::Sparse {
                        continue_catchup: collector.highest_authenticated_positive_index
                            > payload.index
                            || !remembered_later,
                        payload,
                    }));
                }
                if scan_initialized && refresh_head {
                    break;
                }
                return Ok(None);
            }
            if found.is_some() {
                if guard.iter().any(|index| {
                    collector.state(*index) == Some(LiveHistoryProbeState::Unavailable)
                }) {
                    if let Some(payload) = highest_appendable_sequence_zero_followup_head(
                        collector,
                        initial_index,
                        initial_archive,
                    ) && (!hls_is_finalized(&payload.bytes)
                        || payload.index >= collector.highest_authenticated_positive_index)
                    {
                        let remembered_later =
                            collector.remember_followup_windows_after(payload.index);
                        let continue_catchup = collector.highest_authenticated_positive_index
                            > payload.index
                            || found.is_some_and(|(index, _)| index == second_guard)
                            || !remembered_later;
                        return Ok(Some(SequenceZeroFollowupHead::Sparse {
                            payload,
                            continue_catchup,
                        }));
                    }
                    break;
                }
                if guard_waves >= HLS_SEQUENCE_ZERO_PROVISIONAL_GUARD_WAVES {
                    if let Some(payload) = highest_appendable_sequence_zero_followup_head(
                        collector,
                        initial_index,
                        initial_archive,
                    ) && (!hls_is_finalized(&payload.bytes)
                        || payload.index >= collector.highest_authenticated_positive_index)
                    {
                        collector.remember_followup_windows_after(payload.index);
                        return Ok(Some(SequenceZeroFollowupHead::Sparse {
                            payload,
                            continue_catchup: true,
                        }));
                    }
                    break;
                }
                continue;
            }
            if !guard
                .iter()
                .all(|index| collector.state(*index) == Some(LiveHistoryProbeState::Unavailable))
            {
                return Ok(None);
            }
            if guard_waves > 0
                && let Some(payload) = highest_appendable_sequence_zero_followup_head(
                    collector,
                    initial_index,
                    initial_archive,
                )
                && (!hls_is_finalized(&payload.bytes)
                    || payload.index >= collector.highest_authenticated_positive_index)
            {
                let remembered_later = collector.remember_followup_windows_after(payload.index);
                let continue_catchup = collector.highest_authenticated_positive_index
                    > payload.index
                    || !remembered_later;
                return Ok(Some(SequenceZeroFollowupHead::Sparse {
                    payload,
                    continue_catchup,
                }));
            }
            break;
        }
        if !scan_initialized || !refresh_head {
            return Ok(None);
        }

        let planned_retry_index = select_hls_sequence_zero_retry(
            deferred_retry_index,
            ordinary_retry,
            retry_deferred_first,
        );
        let (proof_targets, mut next_recovery_cursor) = plan_hls_sequence_zero_followup_recovery(
            initial_index,
            recovery_cursor,
            planned_retry_index,
        )
        .ok_or_else(|| "The Live follow-up recovery index overflowed.".to_string())?;
        collector.enqueue(
            proof_targets.iter().copied(),
            LiveHistoryProbeClass::Primary,
            true,
        )?;
        if !collector.observe_retained_once(&proof_targets).await? {
            return Ok(None);
        }
        if let Some(direct) =
            take_sequence_zero_followup_direct(collector, initial_index, initial_archive)?
        {
            return Ok(Some(SequenceZeroFollowupHead::Direct(direct)));
        }
        decode_selected_sequence_zero_terminal(collector).await?;
        if let Some(direct) =
            take_sequence_zero_followup_direct(collector, initial_index, initial_archive)?
        {
            return Ok(Some(SequenceZeroFollowupHead::Direct(direct)));
        }
        if let Some(planned_retry) = ordinary_retry
            && planned_retry_index == Some(planned_retry.index)
        {
            let planned_retry_index = planned_retry.index;
            let priority = planned_retry.authenticated
                && planned_retry_index == collector.highest_authenticated_positive_index;
            let state = collector.state(planned_retry_index);
            let retry = hls_sequence_zero_retry_stays_queued(
                planned_retry.authenticated,
                state == Some(LiveHistoryProbeState::Transient),
                matches!(
                    state,
                    Some(LiveHistoryProbeState::Unavailable | LiveHistoryProbeState::Unsupported)
                ),
                collector
                    .deferred_probe_indices
                    .contains(&planned_retry_index),
            );
            collector.forget_followup_retry_index(planned_retry_index);
            if retry {
                collector.remember_followup_retry_index(
                    planned_retry_index,
                    planned_retry.authenticated,
                    priority,
                );
            }
        }
        for offset in 0..HLS_SEQUENCE_ZERO_RECOVERY_BATCH {
            let offset = u64::try_from(offset)
                .map_err(|_| "The Live follow-up recovery offset overflowed.".to_string())?;
            let cursor = recovery_cursor
                .checked_add(offset)
                .ok_or_else(|| "The Live follow-up recovery cursor overflowed.".to_string())?;
            let index = initial_index
                .checked_add(cursor)
                .ok_or_else(|| "The Live follow-up recovery index overflowed.".to_string())?;
            if collector.state(index) == Some(LiveHistoryProbeState::Transient)
                && !collector.remember_followup_retry_index(index, false, false)
            {
                next_recovery_cursor = cursor;
                break;
            }
        }
        let appendable = highest_appendable_sequence_zero_followup_head(
            collector,
            initial_index,
            initial_archive,
        );
        let remembered_later_windows = collector.remember_followup_windows_after(
            appendable
                .as_ref()
                .map_or(initial_index, |payload| payload.index),
        );
        if !remembered_later_windows {
            return Ok(Some(SequenceZeroFollowupHead::WarmScan {
                next_recovery_cursor: recovery_cursor,
                retry_indices: collector.followup_retry_indices.clone(),
                deferred_retry_index: collector.followup_deferred_retry_index,
            }));
        }
        if let Some(payload) = appendable.filter(|payload| {
            !hls_is_finalized(&payload.bytes)
                || payload.index >= collector.highest_authenticated_positive_index
        }) {
            return Ok(Some(SequenceZeroFollowupHead::Sparse {
                continue_catchup: !hls_is_finalized(&payload.bytes),
                payload,
            }));
        }
        Ok(Some(SequenceZeroFollowupHead::WarmScan {
            next_recovery_cursor,
            retry_indices: collector.followup_retry_indices.clone(),
            deferred_retry_index: collector.followup_deferred_retry_index,
        }))
    }

    fn commit_sequence_zero_followup(
        weeb3: &Arc<Weeb3>,
        cache_key: &str,
        owner: &str,
        topic: &str,
        network_id: u64,
        checking_token: u64,
        seed: &SequenceZeroFollowupSeed,
        head: &RawFeedPayload,
        archive: Vec<u8>,
        positive_ceiling: u64,
        mut retry_indices: VecDeque<HlsSequenceZeroRetry>,
        deferred_retry_index: Option<u64>,
    ) -> Option<(FeedRouteSnapshot, bool)> {
        if active_profile().swarm_network_id != network_id
            || !sequence_zero_followup_is_current(weeb3, cache_key, owner, topic)
            || head.index < seed.index
            || hls_media_sequence(&archive) != Some(0)
            || !hls_sequence_zero_covers_head(&seed.snapshot_body, &archive)
            || !hls_sequence_zero_covers_head(&head.bytes, &archive)
            || hls_is_finalized(&archive) != hls_is_finalized(&head.bytes)
        {
            return None;
        }
        retain_hls_sequence_zero_retries_after(&mut retry_indices, head.index);
        let deferred_retry_index = deferred_retry_index.filter(|index| *index > head.index);
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let stored = {
                let state = cache.get_mut(cache_key)?;
                if state.checking_token != checking_token
                    || state.snapshot.index != seed.index
                    || !Arc::ptr_eq(&state.source_body, &seed.source_body)
                    || !Arc::ptr_eq(&state.snapshot.body, &seed.snapshot_body)
                {
                    return None;
                }
                let source_body: Arc<[u8]> = Arc::from(
                    (!hls_is_finalized(&head.bytes))
                        .then(|| hls_sequence_zero_sparse_tail(&head.bytes))
                        .flatten()
                        .unwrap_or_else(|| head.bytes.clone()),
                );
                let body: Arc<[u8]> = Arc::from(archive);
                let finalized = false;
                let changed = head.index != seed.index
                    || body.as_ref() != seed.snapshot_body.as_ref()
                    || finalized != state.snapshot.finalized;
                state.source_body = source_body.clone();
                state.deferred_source = None;
                state.body_tracks_source = body.as_ref() == source_body.as_ref();
                state.source_endlist_confirmed = false;
                state.snapshot = FeedRouteSnapshot {
                    index: head.index,
                    body,
                    finalized,
                };
                let now = js_sys::Date::now();
                state.confirmed_head_index = None;
                state.sequence_zero_recovery_cursor = 20;
                state.sequence_zero_retry_indices = retry_indices;
                state.sequence_zero_deferred_retry_index = deferred_retry_index;
                if state.sequence_zero_deferred_retry_index.is_none()
                    || !state
                        .sequence_zero_retry_indices
                        .iter()
                        .any(|retry| retry.authenticated)
                {
                    state.sequence_zero_retry_deferred_first = true;
                }
                state.sequence_zero_positive_ceiling = state
                    .sequence_zero_positive_ceiling
                    .max(positive_ceiling)
                    .max(head.index);
                state.last_head_check = now;
                state.last_touch = now;
                (state.snapshot.clone(), changed)
            };
            trim_feed_route_cache(&mut cache, cache_key);
            Some(stored)
        })
    }

    async fn catch_up_sequence_zero_followup(
        weeb3: Arc<Weeb3>,
        cache_key: &str,
        owner: &str,
        topic: &str,
        network_id: u64,
        checking_token: u64,
        initial_index: u64,
        refresh_head: bool,
        continuation_invocation: bool,
    ) -> bool {
        let Some(seed) = sequence_zero_followup_seed(cache_key, checking_token, initial_index)
        else {
            return false;
        };
        if seed.tentative_terminal {
            if refresh_head {
                confirm_tentative_sequence_zero_terminal(
                    weeb3,
                    cache_key,
                    owner,
                    topic,
                    network_id,
                    checking_token,
                    &seed,
                )
                .await;
            }
            return false;
        }
        let blocked_authenticated_evidence = seed.deferred_retry_index.is_some()
            || seed.retry_indices.iter().any(|retry| retry.authenticated)
            || seed.positive_ceiling > seed.index;
        if !refresh_head && !continuation_invocation && blocked_authenticated_evidence {
            return false;
        }
        if seed.scan_initialized && refresh_head {
            advance_sequence_zero_retry_turn(cache_key, checking_token, &seed);
        }
        let source_bytes = seed.source_body.as_ref();
        let (initial_bytes, source_was_normalized) = if hls_media_sequence(source_bytes) == Some(0)
        {
            if let Some(tail) = hls_sequence_zero_sparse_tail(source_bytes) {
                (tail, true)
            } else if hls_sparse_history_head_is_supported(source_bytes) {
                (source_bytes.to_vec(), false)
            } else {
                return false;
            }
        } else if hls_sparse_history_head_is_supported(source_bytes) {
            (source_bytes.to_vec(), false)
        } else {
            return false;
        };
        let initial = RawFeedPayload {
            index: seed.index,
            bytes: initial_bytes,
        };
        let Some(mut collector) = LiveHistoryCollector::new_sequence_zero_followup(
            weeb3.clone(),
            owner.to_string(),
            topic.to_string(),
            network_id,
            initial,
            cache_key.to_string(),
            checking_token,
            seed.snapshot_body.clone(),
            seed.source_body.clone(),
        ) else {
            return false;
        };
        collector.highest_authenticated_positive_index = collector
            .highest_authenticated_positive_index
            .max(seed.positive_ceiling);
        collector.followup_retry_indices = seed.retry_indices.clone();
        collector.followup_deferred_retry_index = seed.deferred_retry_index;
        let prefetch_seed = |collector: &LiveHistoryCollector| {
            if continuation_invocation && collector.current() {
                prefetch_live_snapshot_start(
                    &weeb3,
                    owner,
                    topic,
                    &FeedRouteSnapshot {
                        index: seed.index,
                        body: seed.snapshot_body.clone(),
                        finalized: false,
                    },
                );
            }
        };
        if source_was_normalized
            && hls_is_finalized(source_bytes)
            && collector
                .adopt_inline_direct(RawFeedPayload {
                    index: seed.index,
                    bytes: source_bytes.to_vec(),
                })
                .is_err()
        {
            return false;
        }
        let discovered = discover_sequence_zero_followup_head(
            &mut collector,
            seed.index,
            &seed.snapshot_body,
            refresh_head,
            seed.scan_initialized,
            seed.recovery_cursor,
            seed.deferred_retry_index,
            hls_sequence_zero_ordinary_retry(
                &seed.retry_indices,
                seed.deferred_retry_index.is_some(),
            ),
            seed.retry_deferred_first,
        )
        .await;
        let discovered = match discovered {
            Ok(Some(discovered)) => discovered,
            Ok(None) => {
                let _ = collector.remember_followup_windows_after(seed.index);
                persist_sequence_zero_followup_observation(
                    cache_key,
                    checking_token,
                    &seed,
                    &collector,
                );
                if !seed.scan_initialized {
                    initialize_sequence_zero_followup_scan(cache_key, checking_token, &seed);
                }
                prefetch_seed(&collector);
                return false;
            }
            Err(_) => {
                prefetch_seed(&collector);
                return false;
            }
        };
        let (head, direct, continue_catchup) = match discovered {
            SequenceZeroFollowupHead::Direct(head) => {
                let terminal = hls_is_finalized(&head.bytes);
                (head, true, !terminal)
            }
            SequenceZeroFollowupHead::Sparse {
                payload,
                continue_catchup,
            } => (payload, false, continue_catchup),
            SequenceZeroFollowupHead::WarmScan {
                next_recovery_cursor,
                retry_indices,
                deferred_retry_index,
            } => {
                warm_sequence_zero_followup_scan(
                    cache_key,
                    checking_token,
                    &seed,
                    next_recovery_cursor,
                    retry_indices,
                    deferred_retry_index,
                    collector.highest_authenticated_positive_index,
                );
                prefetch_seed(&collector);
                return false;
            }
        };
        let archive = if direct {
            head.bytes.clone()
        } else if head.index == seed.index
            && hls_sequence_zero_same_index_archive_is_reusable(&head.bytes, &seed.snapshot_body)
        {
            seed.snapshot_body.to_vec()
        } else {
            let Some(archive) = assemble_hls_sequence_zero_suffix(
                seed.index,
                &seed.snapshot_body,
                head.index,
                &head.bytes,
                collector
                    .successful_entries_before(head.index)
                    .filter(|(index, _)| *index > seed.index),
            ) else {
                prefetch_seed(&collector);
                return false;
            };
            archive
        };
        if weeb3.get_network_id().await != network_id {
            return false;
        }
        let Some((snapshot, changed)) = commit_sequence_zero_followup(
            &weeb3,
            cache_key,
            owner,
            topic,
            network_id,
            checking_token,
            &seed,
            &head,
            archive,
            collector.highest_authenticated_positive_index,
            collector.followup_retry_indices.clone(),
            collector.followup_deferred_retry_index,
        ) else {
            return false;
        };
        if changed && !continue_catchup {
            prefetch_live_snapshot_start(&weeb3, owner, topic, &snapshot);
        }
        changed && continue_catchup
    }

    async fn refresh_live_feed_head(
        weeb3: Arc<Weeb3>,
        cache_key: &str,
        owner: &str,
        topic: &str,
        checking_token: u64,
        network_id: u64,
    ) -> Option<(u64, bool)> {
        let Some((initial, force_coarse)) = FEED_ROUTE_CACHE.with(|cache| {
            let cache = cache.borrow();
            let state = cache.get(cache_key)?;
            if state.checking_token != checking_token || state.deferred_source.is_some() {
                return None;
            }
            let now = js_sys::Date::now();
            Some((
                crate::bzz_stream::RawFeedPayload {
                    index: state.snapshot.index,
                    bytes: state.source_body.to_vec(),
                },
                state.last_head_check > 0.0
                    && state.last_head_check.is_finite()
                    && now >= state.last_head_check
                    && now - state.last_head_check >= FEED_HEAD_REFRESH_INTERVAL_MS * 4.0,
            ))
        }) else {
            return None;
        };
        let (latest, verified) = if hls_is_finalized(&initial.bytes) {
            (initial, true)
        } else {
            let credit_client = weeb3.clone();
            let admission_deadline =
                js_sys::Date::now() + HLS_FEED_WAVE_CREDIT_WAIT.as_millis() as f64;
            let Some(latest) = acquire_latest_raw_feed_payload_bounded_from(
                owner.to_string(),
                topic.to_string(),
                initial,
                force_coarse,
                &weeb3.chunk_port.0,
                move |probe_count| {
                    await_feed_probe_wave_credit(
                        credit_client.clone(),
                        network_id,
                        probe_count,
                        admission_deadline,
                    )
                },
                None,
            )
            .await
            else {
                return None;
            };
            latest
        };
        if latest.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES {
            return None;
        }
        let (latest, head_confirmed) = if verified {
            confirm_terminal_feed_head(weeb3.clone(), owner, topic, latest, network_id).await
        } else {
            (latest, false)
        };
        if weeb3.get_network_id().await != network_id
            || active_profile().swarm_network_id != network_id
        {
            return None;
        }

        let latest_index = latest.index;
        let accepted = apply_feed_candidate(
            cache_key,
            FeedCandidate {
                index: latest_index,
                terminal: hls_snapshot_is_terminal(
                    hls_is_finalized(&latest.bytes),
                    false,
                    head_confirmed,
                ),
                source: Arc::from(latest.bytes),
                head_confirmed,
                mode: FeedFollowupMode::Canonical,
                admission: FeedCandidateAdmission::Task {
                    token: checking_token,
                    expected_index: None,
                    require_confirmed_same_index: false,
                },
            },
        );
        if let Some(index) = accepted
            .as_ref()
            .and_then(|snapshot| snapshot.finalized.then_some(snapshot.index))
        {
            remember_authenticated_endlist_index(network_id, owner, topic, index);
        }
        if let Some(snapshot) = accepted {
            prefetch_live_snapshot_start(&weeb3, owner, topic, &snapshot);
            return Some((latest_index, head_confirmed));
        }
        None
    }

    fn sequence_zero_followup_is_current(
        weeb3: &Arc<Weeb3>,
        cache_key: &str,
        owner: &str,
        topic: &str,
    ) -> bool {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            (!session.live_start || session.live_history_active)
                && session.feed_identity.as_ref() == Some(&identity)
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, weeb3))
                && sequence_zero_feed_cache_key(owner, topic, session.presentation_id) == cache_key
        })
    }

    fn schedule_feed_followup(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        refresh_head: bool,
        followup_mode: FeedFollowupMode,
    ) {
        schedule_feed_followup_task(
            weeb3,
            cache_key,
            owner,
            topic,
            refresh_head,
            followup_mode,
            false,
        );
    }

    fn schedule_feed_followup_task(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        refresh_head: bool,
        followup_mode: FeedFollowupMode,
        continuation_invocation: bool,
    ) {
        let network_id = active_profile().swarm_network_id;
        let Some((mut current_index, checking_token)) = claim_feed_route_check(&cache_key, None)
        else {
            return;
        };
        if deferred_feed_route_source(&cache_key, checking_token).is_some() {
            let _ = discard_deferred_feed_route(&cache_key, checking_token);
            return;
        }
        let prefix_stamp = hls_prefix_stamp_for_feed(&weeb3, &owner, &topic);

        spawn_local(async move {
            if weeb3.get_network_id().await != network_id
                || active_profile().swarm_network_id != network_id
                || (followup_mode == FeedFollowupMode::SequenceZeroPresentation
                    && !sequence_zero_followup_is_current(&weeb3, &cache_key, &owner, &topic))
            {
                let _ = release_feed_route_check(&cache_key, checking_token);
                return;
            }
            if followup_mode == FeedFollowupMode::SequenceZeroPresentation {
                if await_hls_beginning_prefix_barrier(&weeb3, &owner, &topic, prefix_stamp).await
                    == HlsProgressiveRangeAdmission::Retire
                {
                    let _ = release_feed_route_check(&cache_key, checking_token);
                    return;
                }
                let resume = catch_up_sequence_zero_followup(
                    weeb3.clone(),
                    &cache_key,
                    &owner,
                    &topic,
                    network_id,
                    checking_token,
                    current_index,
                    refresh_head,
                    continuation_invocation,
                )
                .await;
                let released = release_feed_route_check(&cache_key, checking_token);
                if resume && released.is_some() {
                    schedule_feed_followup_task(
                        weeb3,
                        cache_key,
                        owner,
                        topic,
                        false,
                        followup_mode,
                        true,
                    );
                }
                return;
            }
            if refresh_head {
                match refresh_live_feed_head(
                    weeb3.clone(),
                    &cache_key,
                    &owner,
                    &topic,
                    checking_token,
                    network_id,
                )
                .await
                {
                    Some((_, true)) => {
                        let _ = release_feed_route_check(&cache_key, checking_token);
                        return;
                    }
                    Some((refreshed_index, false)) => current_index = refreshed_index,
                    None => {}
                }
            }

            let mut successful_followups = 0usize;
            let mut skipped_missing_index = false;
            let mut recovered_missing_index = false;
            let mut saw_tentative_endlist = FEED_ROUTE_CACHE.with(|cache| {
                cache.borrow().get(&cache_key).is_some_and(|state| {
                    !state.snapshot.finalized
                        && state.deferred_source.is_none()
                        && !state.source_endlist_confirmed
                        && hls_is_finalized(&state.source_body)
                })
            });
            if saw_tentative_endlist {
                if !refresh_head {
                    let _ = refresh_live_feed_head(
                        weeb3,
                        &cache_key,
                        &owner,
                        &topic,
                        checking_token,
                        network_id,
                    )
                    .await;
                }
                let _ = release_feed_route_check(&cache_key, checking_token);
                return;
            }
            let exact_indices =
                std::iter::successors(Some(current_index), |index| index.checked_add(1))
                    .skip(1)
                    .take(FEED_FOLLOWUP_BATCH_LIMIT);
            let mut exact_followups = stream::iter(exact_indices)
                .map(|next_index| {
                    let weeb3 = weeb3.clone();
                    let owner = owner.clone();
                    let topic = topic.clone();
                    async move {
                        (
                            next_index,
                            weeb3
                                .hls_feed_payload_at_index(owner, topic, next_index)
                                .await,
                        )
                    }
                })
                .buffered(1);
            while let Some((next_index, next)) = exact_followups.next().await {
                if weeb3.get_network_id().await != network_id
                    || active_profile().swarm_network_id != network_id
                {
                    break;
                }
                let Some(next) = next else {
                    if skipped_missing_index {
                        break;
                    }
                    skipped_missing_index = true;
                    continue;
                };
                if next.index != next_index || next.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES {
                    break;
                }

                let next_is_hls = is_hls_manifest(&next.bytes);
                let has_endlist = next_is_hls && hls_is_finalized(&next.bytes);
                let source_body: Arc<[u8]> = Arc::from(next.bytes);
                let accepted = apply_feed_candidate(
                    &cache_key,
                    FeedCandidate {
                        index: next.index,
                        source: source_body.clone(),
                        terminal: false,
                        head_confirmed: false,
                        mode: FeedFollowupMode::Canonical,
                        admission: FeedCandidateAdmission::Task {
                            token: checking_token,
                            expected_index: Some(current_index),
                            require_confirmed_same_index: false,
                        },
                    },
                );
                if accepted.is_none() {
                    break;
                }
                prefetch_live_snapshot_start(
                    &weeb3,
                    &owner,
                    &topic,
                    &FeedRouteSnapshot {
                        index: next.index,
                        body: source_body.clone(),
                        finalized: false,
                    },
                );
                saw_tentative_endlist |= has_endlist;
                recovered_missing_index |= skipped_missing_index;
                successful_followups = successful_followups.saturating_add(1);
                current_index = next_index;
                if has_endlist {
                    break;
                }
            }
            // Dropping tail observers cannot cancel dispatched accounting work.
            drop(exact_followups);

            if weeb3.get_network_id().await != network_id
                || active_profile().swarm_network_id != network_id
            {
                let _ = release_feed_route_check(&cache_key, checking_token);
                return;
            }

            if !refresh_head
                && (saw_tentative_endlist
                    || successful_followups >= FEED_FOLLOWUP_BATCH_LIMIT
                    || recovered_missing_index)
            {
                let _ = refresh_live_feed_head(
                    weeb3,
                    &cache_key,
                    &owner,
                    &topic,
                    checking_token,
                    network_id,
                )
                .await;
            }

            let _ = release_feed_route_check(&cache_key, checking_token);
        });
    }

    pub(crate) async fn try_fetch_response(
        weeb3: Arc<Weeb3>,
        request_url: &str,
        pathname: &str,
        method: &str,
        range: Option<&str>,
        if_none_match: Option<&str>,
        if_range: Option<&str>,
        stream_token: Option<&str>,
    ) -> Option<FetchResponse> {
        if let Some(reference) = canonical_hls_bytes_resource(pathname) {
            activate_retrieval_profile_if_requested();
            let reference = match reference {
                Ok(reference) => reference,
                Err(error) => return Some(FetchResponse::error(400, error)),
            };
            return Some(
                fetch_hls_bytes_response(
                    weeb3,
                    reference,
                    method.to_string(),
                    range.map(str::to_owned),
                    if_none_match.map(str::to_owned),
                    if_range.map(str::to_owned),
                    stream_token.map(str::to_owned),
                    local_hls_bytes_base(pathname),
                )
                .await,
            );
        }

        let (owner, topic) = canonical_feed_resource(pathname)?;
        activate_retrieval_profile_if_requested();
        let request_url = match web_sys::Url::new(request_url) {
            Ok(url) => url,
            Err(_) => return Some(FetchResponse::error(400, "invalid feed URL")),
        };
        let search = request_url.search_params();
        let feed_index = search.get("index");
        let index_hint = match feed_index {
            Some(index) => match index.parse::<u64>() {
                Ok(index) => Some(index),
                Err(_) => return Some(FetchResponse::error(400, "invalid feed index")),
            },
            None => None,
        };
        let start = match search.get("start").as_deref() {
            None => HlsStart::Beginning,
            Some("live") => HlsStart::Live,
            Some(_) => return Some(FetchResponse::error(400, "invalid HLS start")),
        };
        let codec_bootstrap = match search.get("codec-bootstrap") {
            None => None,
            Some(token) if start == HlsStart::Live && index_hint.is_none() => {
                let Ok(token) = token.parse::<u64>() else {
                    return Some(FetchResponse::error(400, "invalid HLS codec bootstrap"));
                };
                if !super::player::hls_codec_bootstrap_token_is_current(token) {
                    return Some(FetchResponse::error(409, "stale HLS codec bootstrap"));
                }
                Some(hls_codec_bootstrap_manifest(token))
            }
            Some(_) => return Some(FetchResponse::error(400, "invalid HLS codec bootstrap")),
        };
        Some(
            fetch_feed_response(
                weeb3,
                owner,
                topic,
                index_hint,
                start,
                codec_bootstrap,
                method.to_string(),
                local_hls_bytes_base(pathname),
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
            if reference.is_empty()
                || parts.any(|part| !part.is_empty())
                || !is_hex_reference(reference)
            {
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
            return Some((owner.to_string(), topic.to_string()));
        }
        None
    }

    fn local_hls_bytes_base(pathname: &str) -> String {
        let base = STREAMING_ROUTE_BASE;
        if pathname.starts_with(&format!("{base}/testnet/")) {
            streaming_route_path("testnet/hls/bytes")
        } else if pathname.starts_with(&format!("{base}/mainnet/")) {
            streaming_route_path("mainnet/hls/bytes")
        } else {
            streaming_route_path("hls/bytes")
        }
    }

    pub(crate) async fn open_hls_feed_view(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        start: HlsStart,
    ) {
        let view_generation = begin_result_view_request();
        open_hls_feed_view_generation(weeb3, owner, topic, start, view_generation).await;
    }

    pub(crate) async fn attach_hls_feed_player(
        weeb3: Arc<Weeb3>,
        player: &Element,
        owner: String,
        topic: String,
        start: HlsStart,
        view_generation: u64,
    ) -> Result<&'static str, String> {
        activate_retrieval_profile_if_requested();
        reset_hls_codec_bootstrap();
        super::set_hls_video_resolution(None);
        let hls_loader = JsFuture::from(load_hls());
        let owner = owner
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X")
            .to_string();
        let topic = normalize_feed_topic(&topic);
        if owner.len() != 40 || !owner.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("The stream feed owner is invalid.".to_string());
        }
        if !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        let mut source = format!("{}/{}/{}", streaming_route_path("feeds"), owner, topic);
        let presentation_id = view_generation;
        match start {
            HlsStart::Beginning => {}
            HlsStart::Live => {
                let cache_key = feed_cache_key(&owner, &topic, None);
                FEED_ROUTE_CACHE.with(|cache| {
                    let mut cache = cache.borrow_mut();
                    let remove = cache.get(&cache_key).is_some_and(|state| {
                        !state.snapshot.finalized
                            && state.checking_token == 0
                            && cached_feed_should_refresh_head(
                                state.last_head_check,
                                js_sys::Date::now(),
                            )
                    });
                    if remove {
                        cache.remove(&cache_key);
                    }
                });
                source.push_str("?start=live");
            }
        }
        install_hls_prefetch_lifecycle(
            player,
            weeb3.clone(),
            owner.clone(),
            topic.clone(),
            presentation_id,
            start == HlsStart::Live,
        );
        weeb3.interface_log(format!("HLS open owner={} topic={}", owner, topic));
        let worker_ready = Box::pin(service_worker_controls_bzz_requests(
            &weeb3,
            "HLS feed and segment requests",
            || result_view_request_is_current(view_generation),
        ));
        let snapshot_client = weeb3.clone();
        let snapshot_owner = owner.clone();
        let snapshot_topic = topic.clone();
        let (startup_deferred_out, startup_deferred_in) = if start == HlsStart::Live {
            let (output, input) = mpsc::bounded::<DeferredRawFeedPayload>(1);
            (Some(output), Some(input))
        } else {
            (None, None)
        };
        let snapshot_load = Box::pin(async move {
            let live_frontier_deadline_ms = (start == HlsStart::Beginning)
                .then_some(js_sys::Date::now() + HLS_LIVE_FRONTIER_MAX_WAIT.as_millis() as f64);
            let snapshot = load_feed_snapshot(
                snapshot_client.clone(),
                snapshot_owner.clone(),
                snapshot_topic.clone(),
                None,
                start,
                live_frontier_deadline_ms,
                start == HlsStart::Live,
                startup_deferred_out,
            )
            .await;
            match (start, snapshot) {
                (HlsStart::Beginning, snapshot) => {
                    if let Some(snapshot) = snapshot.as_ref() {
                        start_beginning_snapshot_runway(
                            &snapshot_client,
                            &snapshot_owner,
                            &snapshot_topic,
                            snapshot,
                        );
                    } else if let Some(stamp) = hls_prefix_stamp_for_feed(
                        &snapshot_client,
                        &snapshot_owner,
                        &snapshot_topic,
                    ) {
                        bypass_hls_beginning_prefix(stamp);
                    }
                    Ok(snapshot)
                }
                (HlsStart::Live, Some(snapshot)) => {
                    prefetch_live_snapshot_start(
                        &snapshot_client,
                        &snapshot_owner,
                        &snapshot_topic,
                        &snapshot,
                    );
                    prepare_live_history(
                        snapshot_client,
                        snapshot_owner,
                        snapshot_topic,
                        presentation_id,
                        snapshot,
                        startup_deferred_in.and_then(|input| input.try_recv().ok()),
                    )
                    .await
                    .map(Some)
                }
                (HlsStart::Live, None) => Ok(None),
            }
        });
        let snapshot = match select(worker_ready, snapshot_load).await {
            Either::Left((ready, snapshot_load)) => {
                if !ready {
                    return Err(service_worker_scope_protocol_error(
                        "HLS feed and segment requests",
                    ));
                }
                snapshot_load.await?
            }
            Either::Right((snapshot, worker_ready)) => {
                let snapshot = snapshot?;
                if !worker_ready.await {
                    return Err(service_worker_scope_protocol_error(
                        "HLS feed and segment requests",
                    ));
                }
                snapshot
            }
        };
        if !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        if start == HlsStart::Live && snapshot.is_none() {
            return Err("The HLS feed could not be loaded.".to_string());
        }
        if !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        let beginning_prefix_stamp = (start == HlsStart::Beginning)
            .then(|| hls_prefix_stamp_for_feed(&weeb3, &owner, &topic))
            .flatten();
        let mode = play_hls(player, &source, hls_loader, start, beginning_prefix_stamp)
            .await
            .map_err(|error| format!("Could not initialize HLS: {}", js_error_message(&error)))?;
        if !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        Ok(mode)
    }

    async fn open_hls_feed_view_generation(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        start: HlsStart,
        view_generation: u64,
    ) {
        render_stream_status_for_generation(
            "Preparing reload-free Service Worker routing for HLS...",
            view_generation,
        );
        let document = web_sys::window().unwrap().document().unwrap();
        let wrapper = document.create_element("section").unwrap();
        let player = create_hls_player();
        let status = document.create_element("div").unwrap();
        status.set_class_name("weeb3-hls-status");
        let status_id = format!("weeb3-hls-status-{view_generation}");
        status.set_id(&status_id);
        let _ = status.set_attribute("role", "status");
        let _ = status.set_attribute("aria-live", "polite");
        let _ = status.set_attribute("aria-atomic", "true");
        status.set_text_content(Some("Loading HLS manifest from Swarm..."));
        let _ = player.set_attribute("aria-describedby", &status_id);
        let _ = wrapper.append_child(&player);
        let _ = wrapper.append_child(&status);
        if !replace_stream_result_view(&wrapper, view_generation) {
            return;
        }

        let mode = match attach_hls_feed_player(
            weeb3,
            &player,
            owner,
            topic,
            start,
            view_generation,
        )
        .await
        {
            Ok(mode) => mode,
            Err(error) => {
                status.set_text_content(Some(&error));
                return;
            }
        };
        if !result_view_request_is_current(view_generation) {
            return;
        }
        status.set_text_content(Some(&format!(
            "HLS player attached with {mode}; loading through weeb-3. Press play if autoplay is blocked.",
        )));
    }
    fn create_hls_player() -> Element {
        let document = web_sys::window().unwrap().document().unwrap();
        let player = document.create_element("video").unwrap();
        let _ = player.set_attribute("controls", "");
        let _ = player.set_attribute("autoplay", "");
        let _ = player.set_attribute("preload", "metadata");
        let _ = player.set_attribute("aria-label", "Swarm HLS video stream");
        let _ = player.set_attribute("playsinline", "");
        let _ = player.set_attribute("style", "width:90%;max-height:75vh;");
        let media = player.unchecked_ref::<HtmlMediaElement>();
        media.set_default_muted(true);
        media.set_muted(true);
        player
    }

    fn render_stream_status_for_generation(message: &str, view_generation: u64) {
        let document = web_sys::window().unwrap().document().unwrap();
        let status = document.create_element("p").unwrap();
        status.set_text_content(Some(message));
        let _ = replace_stream_result_view(&status, view_generation);
    }

    fn install_hls_prefetch_lifecycle(
        player: &Element,
        weeb3: Arc<Weeb3>,
        normalized_owner: String,
        normalized_topic: String,
        presentation_id: u64,
        live_start: bool,
    ) {
        begin_hls_prefetch_session(
            weeb3,
            normalized_owner,
            normalized_topic,
            presentation_id,
            live_start,
        );

        let warmup_callback = Closure::<dyn FnMut()>::new(move || {
            activate_hls_prefetch_warmup();
        });
        retain_media_element_callback(player, HLS_WARMUP_START_EVENT, warmup_callback);

        let timeline_rebase_callback = Closure::<dyn FnMut()>::new(move || {
            retire_hls_prefetch_timeline();
        });
        retain_media_element_callback(player, HLS_TIMELINE_REBASE_EVENT, timeline_rebase_callback);

        let autoplay_authorized_callback = Closure::<dyn FnMut()>::new(move || {
            set_hls_prefetch_mode(HlsPrefetchMode::Sustained);
        });
        retain_media_element_callback(
            player,
            HLS_AUTOPLAY_AUTHORIZED_EVENT,
            autoplay_authorized_callback,
        );

        let explicit_pause_callback = Closure::<dyn FnMut()>::new(move || {
            set_hls_prefetch_mode(HlsPrefetchMode::Inactive);
        });
        retain_media_element_callback(player, HLS_EXPLICIT_PAUSE_EVENT, explicit_pause_callback);

        let play_player = player.clone();
        let play_callback = Closure::<dyn FnMut()>::new(move || {
            if play_player.get_attribute("data-weeb3-hls-mode").is_some() {
                // Once either backend is attached, its Session owns playback intent.
                return;
            }
            if play_player
                .get_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE)
                .as_deref()
                == Some("1")
            {
                // Chrome can emit play before its autoplay promise rejects.
                return;
            }
            play_player
                .set_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE, "1")
                .ok();
            set_hls_prefetch_mode(HlsPrefetchMode::Sustained);
        });
        retain_media_element_callback(player, "play", play_callback);

        let pause_player = player.clone();
        let pause_callback = Closure::<dyn FnMut()>::new(move || {
            if pause_player.get_attribute("data-weeb3-hls-mode").is_some() {
                // Attached player Sessions report pauses through the explicit event.
                return;
            }
            if pause_player
                .get_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE)
                .as_deref()
                == Some("1")
                || pause_player
                    .get_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
                    .as_deref()
                    != Some("1")
            {
                return;
            }
            pause_player
                .remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
                .ok();
            // Pause closes admission without cancelling dispatched accounting.
            set_hls_prefetch_mode(HlsPrefetchMode::Inactive);
        });
        retain_media_element_callback(player, "pause", pause_callback);

        // Ignore generic seeking; cursor discontinuity distinguishes real seeks
        // from hls.js startup alignment and gap traversal.
    }

    pub(crate) fn release_hls_view() {
        // View closure cannot cancel dispatched accounting work.
        reset_hls_codec_bootstrap();
        super::set_hls_video_resolution(None);
        invalidate_hls_prefetch_session();
        destroy_current_hls();
    }

    pub(crate) fn release_hls_for_bzz_view() {
        release_hls_view();
        HLS_ASSET_CACHE.with(|cache| cache.borrow_mut().clear_completed_bodies());
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use runtime::{
    attach_hls_feed_player, hls_payload_cache_body_bytes, open_hls_feed_view,
    release_hls_for_bzz_view, release_hls_view, try_fetch_response,
};
