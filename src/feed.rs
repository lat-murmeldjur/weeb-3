use std::{future::Future, time::Duration};

use futures::stream::{FuturesUnordered, StreamExt};

pub(crate) const FEED_FRONTIER_LOOKAHEAD_LEVELS: usize = 8;
pub(crate) const FEED_FRONTIER_LOOKAHEAD_TIMEOUT: Duration = Duration::from_secs(1);
const WIDE_FEED_FRONTIER_INITIAL_TIMEOUT: Duration = Duration::from_millis(1_500);
const BOUNDED_INITIAL_LEVELS: [usize; FEED_FRONTIER_LOOKAHEAD_LEVELS] = [8, 7, 6, 5, 4, 3, 2, 0];
pub(crate) const WIDE_FEED_FRONTIER_LOOKAHEAD: usize = 16;
const WIDE_FEED_FRONTIER_RECOVERY_DISTANCE: u64 =
    WIDE_FEED_FRONTIER_LOOKAHEAD as u64 * (WIDE_FEED_FRONTIER_LOOKAHEAD as u64 + 1);
const WIDE_FEED_FRONTIER_COARSE_STRIDE: u64 =
    WIDE_FEED_FRONTIER_LOOKAHEAD as u64 * WIDE_FEED_FRONTIER_LOOKAHEAD as u64;
const WIDE_FEED_FRONTIER_MAX_OVERSCAN_WAVES: usize = 10;
const WIDE_INITIAL_INDICES: [u64; WIDE_FEED_FRONTIER_LOOKAHEAD] = [
    0,
    1,
    7,
    255,
    511,
    1_023,
    1_535,
    1_791,
    2_047,
    4_095,
    8_191,
    16_383,
    65_535,
    262_143,
    1_048_575,
    u64::MAX,
];

pub(crate) enum FeedProbe<T> {
    Found(T),
    Missing,
    Transient,
}

impl<T> FeedProbe<T> {
    pub(crate) fn into_option(self) -> Option<T> {
        match self {
            Self::Found(payload) => Some(payload),
            Self::Missing | Self::Transient => None,
        }
    }
}

impl<T> From<Option<T>> for FeedProbe<T> {
    fn from(result: Option<T>) -> Self {
        result.map_or(Self::Missing, Self::Found)
    }
}

impl<T> From<Result<Option<T>, ()>> for FeedProbe<T> {
    fn from(result: Result<Option<T>, ()>) -> Self {
        match result {
            Ok(result) => result.into(),
            Err(()) => Self::Transient,
        }
    }
}

fn probe_index(base: u64, level: usize) -> Option<u64> {
    let distance = 1_u64.checked_shl(u32::try_from(level).ok()?)?;
    base.checked_add(distance.checked_sub(1)?)
}

async fn confirm_sequence_feed_missing<T, Probe, ProbeFuture, ProbeResult>(
    probe: &Probe,
    index: u64,
    timeout: Option<Duration>,
) -> FeedProbe<T>
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = ProbeResult>,
    ProbeResult: Into<FeedProbe<T>>,
{
    let lookup = probe(index);
    if let Some(timeout) = timeout {
        return match async_std::future::timeout(timeout, lookup).await {
            Ok(result) => result.into(),
            Err(_) => FeedProbe::Transient,
        };
    }
    lookup.await.into()
}

pub(crate) async fn seek_sequence_feed_frontier<T, Probe, ProbeFuture, ProbeResult>(
    probe: Probe,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = ProbeResult>,
    ProbeResult: Into<FeedProbe<T>>,
{
    seek_sequence_feed_frontier_inner(None, probe, |_, _| {}, None).await
}

pub(crate) async fn seek_sequence_feed_frontier_from<T, Probe, ProbeFuture, ProbeResult>(
    initial_latest: (u64, T),
    probe: Probe,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = ProbeResult>,
    ProbeResult: Into<FeedProbe<T>>,
{
    seek_sequence_feed_frontier_inner(Some(initial_latest), probe, |_, _| {}, None).await
}

pub(crate) async fn seek_sequence_feed_frontier_bounded_observing_positive<
    T,
    Probe,
    ProbeFuture,
    ProbeResult,
    ObservePositive,
>(
    probe: Probe,
    observe_positive: ObservePositive,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = ProbeResult>,
    ProbeResult: Into<FeedProbe<T>>,
    ObservePositive: FnMut(u64, &T),
{
    seek_sequence_feed_frontier_inner(
        None,
        probe,
        observe_positive,
        Some(FEED_FRONTIER_LOOKAHEAD_TIMEOUT),
    )
    .await
}

pub(crate) async fn seek_sequence_feed_frontier_wide_bounded<T, Probe, ProbeFuture, ProbeResult>(
    probe: Probe,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = ProbeResult>,
    ProbeResult: Into<FeedProbe<T>>,
{
    let mut probes = FuturesUnordered::new();
    for (slot, index) in WIDE_INITIAL_INDICES.into_iter().enumerate() {
        let lookup = probe(index);
        probes.push(async move { (slot, lookup.await) });
    }
    let mut completed = [false; WIDE_FEED_FRONTIER_LOOKAHEAD];
    let mut missing = [false; WIDE_FEED_FRONTIER_LOOKAHEAD];
    let mut found: [Option<T>; WIDE_FEED_FRONTIER_LOOKAHEAD] = std::array::from_fn(|_| None);
    while found.iter().all(Option::is_none) {
        let Some((slot, result)) = probes.next().await else {
            return (None, 0);
        };
        completed[slot] = true;
        match result.into() {
            FeedProbe::Found(payload) => found[slot] = Some(payload),
            FeedProbe::Missing => missing[slot] = true,
            FeedProbe::Transient => {}
        }
    }
    let wait_for_higher = async {
        loop {
            let highest_found = found
                .iter()
                .rposition(Option::is_some)
                .expect("wide feed lookup must retain its first payload");
            if completed[highest_found + 1..].iter().all(|done| *done) {
                break;
            }
            let Some((slot, result)) = probes.next().await else {
                break;
            };
            completed[slot] = true;
            match result.into() {
                FeedProbe::Found(payload) => found[slot] = Some(payload),
                FeedProbe::Missing => missing[slot] = true,
                FeedProbe::Transient => {}
            }
        }
    };
    let _ = async_std::future::timeout(WIDE_FEED_FRONTIER_INITIAL_TIMEOUT, wait_for_higher).await;
    let highest_found = found
        .iter()
        .rposition(Option::is_some)
        .expect("wide feed lookup must retain its first payload");
    let mut latest = (
        WIDE_INITIAL_INDICES[highest_found],
        found[highest_found]
            .take()
            .expect("selected wide feed probe must carry its payload"),
    );
    drop(probes);
    let mut upper = u64::MAX;
    for slot in highest_found + 1..WIDE_FEED_FRONTIER_LOOKAHEAD {
        if !missing[slot] {
            continue;
        }
        let missing = WIDE_INITIAL_INDICES[slot];
        let Some(confirmation) = missing
            .checked_add(WIDE_FEED_FRONTIER_LOOKAHEAD as u64)
            .filter(|confirmation| *confirmation < u64::MAX)
        else {
            upper = missing;
            break;
        };
        if missing.saturating_sub(latest.0) <= WIDE_FEED_FRONTIER_RECOVERY_DISTANCE {
            upper = missing;
            break;
        }
        match async_std::future::timeout(FEED_FRONTIER_LOOKAHEAD_TIMEOUT, probe(confirmation)).await
        {
            Ok(result) => match result.into() {
                FeedProbe::Found(payload) => latest = (confirmation, payload),
                FeedProbe::Missing => {
                    upper = missing;
                    break;
                }
                FeedProbe::Transient => {}
            },
            Err(_) => {}
        }
    }

    loop {
        if latest.0 == u64::MAX {
            return (Some(latest), u64::MAX);
        }
        if latest.0.saturating_add(1) >= upper {
            let next = latest.0.saturating_add(1);
            match confirm_sequence_feed_missing(&probe, next, Some(FEED_FRONTIER_LOOKAHEAD_TIMEOUT))
                .await
            {
                FeedProbe::Found(payload) => {
                    latest = (next, payload);
                    upper = u64::MAX;
                    continue;
                }
                FeedProbe::Missing | FeedProbe::Transient => return (Some(latest), next),
            }
        }

        let interior = upper.saturating_sub(latest.0).saturating_sub(1);
        let probe_count = interior.min(WIDE_FEED_FRONTIER_LOOKAHEAD as u64) as usize;
        let divisor = (probe_count + 1) as u128;
        let span = u128::from(upper.saturating_sub(latest.0));
        let mut indices = Vec::with_capacity(probe_count);
        for position in 1..=probe_count {
            let offset = ((span * position as u128 + divisor.saturating_sub(1)) / divisor) as u64;
            indices.push(latest.0.saturating_add(offset));
        }

        let mut probes = FuturesUnordered::new();
        for (slot, index) in indices.iter().copied().enumerate() {
            let lookup = probe(index);
            probes.push(async move {
                (
                    slot,
                    async_std::future::timeout(FEED_FRONTIER_LOOKAHEAD_TIMEOUT, lookup).await,
                )
            });
        }
        let mut completed = vec![false; probe_count];
        let mut missing = vec![false; probe_count];
        let mut found = std::iter::repeat_with(|| None)
            .take(probe_count)
            .collect::<Vec<Option<T>>>();
        let selected = loop {
            let Some((slot, result)) = probes.next().await else {
                break None;
            };
            completed[slot] = true;
            if let Ok(result) = result {
                match result.into() {
                    FeedProbe::Found(payload) => found[slot] = Some(payload),
                    FeedProbe::Missing => missing[slot] = true,
                    FeedProbe::Transient => {}
                }
            }
            let Some(highest_found) = found.iter().rposition(Option::is_some) else {
                continue;
            };
            if completed[highest_found + 1..].iter().all(|done| *done) {
                break Some((
                    highest_found,
                    found[highest_found]
                        .take()
                        .expect("selected wide feed probe must carry its payload"),
                ));
            }
        };
        drop(probes);
        let previous_latest = latest.0;
        let missing = match selected {
            Some((slot, payload)) => {
                latest = (indices[slot], payload);
                (slot + 1..probe_count)
                    .find(|probe_slot| missing[*probe_slot])
                    .map(|probe_slot| indices[probe_slot])
            }
            None => (0..probe_count)
                .find(|slot| missing[*slot])
                .map(|slot| indices[slot]),
        };
        let Some(missing) = missing else {
            if latest.0 == previous_latest {
                let next = latest.0.saturating_add(1);
                return (Some(latest), next);
            }
            continue;
        };
        let Some(confirmation) = missing
            .checked_add(WIDE_FEED_FRONTIER_LOOKAHEAD as u64)
            .filter(|confirmation| *confirmation < upper)
        else {
            upper = missing;
            continue;
        };
        if missing.saturating_sub(latest.0) <= WIDE_FEED_FRONTIER_RECOVERY_DISTANCE {
            upper = missing;
            continue;
        }
        match async_std::future::timeout(FEED_FRONTIER_LOOKAHEAD_TIMEOUT, probe(confirmation)).await
        {
            Ok(result) => match result.into() {
                FeedProbe::Found(payload) => latest = (confirmation, payload),
                FeedProbe::Missing => upper = missing,
                FeedProbe::Transient if latest.0 == previous_latest => {
                    let next = latest.0.saturating_add(1);
                    return (Some(latest), next);
                }
                FeedProbe::Transient => {}
            },
            Err(_) if latest.0 == previous_latest => {
                let next = latest.0.saturating_add(1);
                return (Some(latest), next);
            }
            Err(_) => {}
        }
    }
}

pub(crate) async fn overscan_sequence_feed_candidate<
    T,
    Probe,
    ProbeFuture,
    ProbeResult,
    ObservePositive,
>(
    candidate: (u64, T),
    force_coarse: bool,
    probe: Probe,
    mut observe_positive: ObservePositive,
) -> ((u64, T), bool)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = ProbeResult>,
    ProbeResult: Into<FeedProbe<T>>,
    ObservePositive: FnMut(u64, &T),
{
    let mut latest = candidate;
    let coarse = force_coarse || WIDE_INITIAL_INDICES.contains(&latest.0);
    let strides: &[u64] = if coarse {
        &[
            WIDE_FEED_FRONTIER_COARSE_STRIDE,
            WIDE_FEED_FRONTIER_COARSE_STRIDE / 4,
            WIDE_FEED_FRONTIER_LOOKAHEAD as u64,
            1,
        ]
    } else {
        &[WIDE_FEED_FRONTIER_LOOKAHEAD as u64, 1]
    };
    let mut unresolved_frontier = false;
    let mut waves = 0;
    let mut probed_indices =
        Vec::with_capacity(WIDE_FEED_FRONTIER_LOOKAHEAD * WIDE_FEED_FRONTIER_MAX_OVERSCAN_WAVES);
    let mut resolved_indices = Vec::with_capacity(probed_indices.capacity());
    for &stride in strides {
        if stride == WIDE_FEED_FRONTIER_COARSE_STRIDE / 4 && !unresolved_frontier {
            continue;
        }
        let mut advanced_at_stride = false;
        'same_stride: loop {
            let base = latest.0;
            let mut shifted = false;
            loop {
                if waves >= WIDE_FEED_FRONTIER_MAX_OVERSCAN_WAVES {
                    return (latest, false);
                }
                let mut indices = Vec::with_capacity(WIDE_FEED_FRONTIER_LOOKAHEAD);
                let mut covered_indices = Vec::with_capacity(WIDE_FEED_FRONTIER_LOOKAHEAD);
                let mut possible = false;
                for slot in 0..WIDE_FEED_FRONTIER_LOOKAHEAD as u64 {
                    let offset = if shifted {
                        stride / 2 + slot.saturating_mul(stride)
                    } else {
                        (slot + 1).saturating_mul(stride)
                    };
                    let Some(index) = base.checked_add(offset) else {
                        continue;
                    };
                    if index <= latest.0 {
                        continue;
                    }
                    possible = true;
                    if !probed_indices.contains(&index) || resolved_indices.contains(&index) {
                        covered_indices.push(index);
                    }
                    if !probed_indices.contains(&index) {
                        indices.push(index);
                        probed_indices.push(index);
                    }
                }
                if indices.is_empty() {
                    if !possible {
                        break 'same_stride;
                    }
                    if covered_indices.is_empty() {
                        if advanced_at_stride {
                            unresolved_frontier = true;
                            break 'same_stride;
                        }
                        return (latest, false);
                    }
                    break 'same_stride;
                }
                let repeat_from = covered_indices
                    .get(covered_indices.len().saturating_sub(2))
                    .copied()
                    .unwrap_or(covered_indices[0]);
                let mut probes = FuturesUnordered::new();
                for index in indices {
                    let lookup = probe(index);
                    probes.push(async move { (index, lookup.await) });
                }
                let mut unresolved = Vec::new();
                let previous_latest = latest.0;
                while let Some((index, result)) = probes.next().await {
                    match result.into() {
                        FeedProbe::Found(payload) => {
                            resolved_indices.push(index);
                            if index > latest.0 {
                                latest = (index, payload);
                            }
                        }
                        FeedProbe::Missing => resolved_indices.push(index),
                        FeedProbe::Transient => unresolved.push(index),
                    }
                }
                let progressed = latest.0 > previous_latest;
                waves += 1;
                if unresolved.into_iter().any(|index| index > latest.0) {
                    if progressed {
                        observe_positive(latest.0, &latest.1);
                        advanced_at_stride = true;
                        unresolved_frontier = true;
                        if shifted || stride == WIDE_FEED_FRONTIER_LOOKAHEAD as u64 {
                            break 'same_stride;
                        }
                        continue 'same_stride;
                    }
                    if !shifted && stride > 1 && waves < WIDE_FEED_FRONTIER_MAX_OVERSCAN_WAVES {
                        if unresolved_frontier && stride == WIDE_FEED_FRONTIER_LOOKAHEAD as u64 {
                            break 'same_stride;
                        }
                        shifted = true;
                        continue;
                    }
                    if advanced_at_stride || (unresolved_frontier && stride > 1) {
                        unresolved_frontier = true;
                        break 'same_stride;
                    }
                    return (latest, false);
                }
                if latest.0 >= repeat_from || (stride == 1 && progressed) {
                    if progressed {
                        observe_positive(latest.0, &latest.1);
                        advanced_at_stride = true;
                    }
                    continue 'same_stride;
                }
                if progressed {
                    observe_positive(latest.0, &latest.1);
                }
                break 'same_stride;
            }
        }
    }
    if !unresolved_frontier && let Some(next) = latest.0.checked_add(1) {
        match probe(next).await.into() {
            FeedProbe::Found(payload) => {
                latest = (next, payload);
                observe_positive(latest.0, &latest.1);
                return (latest, false);
            }
            FeedProbe::Missing => {}
            FeedProbe::Transient => return (latest, false),
        }
    }
    (latest, !unresolved_frontier)
}

async fn seek_sequence_feed_frontier_inner<T, Probe, ProbeFuture, ProbeResult, ObservePositive>(
    initial_latest: Option<(u64, T)>,
    probe: Probe,
    mut observe_positive: ObservePositive,
    lookahead_timeout: Option<Duration>,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = ProbeResult>,
    ProbeResult: Into<FeedProbe<T>>,
    ObservePositive: FnMut(u64, &T),
{
    let (mut latest, mut level_limit, mut known_missing) = if let Some(latest) = initial_latest {
        (latest, FEED_FRONTIER_LOOKAHEAD_LEVELS, None)
    } else if let Some(timeout) = lookahead_timeout {
        // An authenticated higher sequence update proves the lower contiguous interval.
        let mut probes = FuturesUnordered::new();
        for level in BOUNDED_INITIAL_LEVELS.into_iter().rev() {
            let index = if level == 0 {
                0
            } else {
                probe_index(0, level).expect("bounded initial feed level")
            };
            let lookup = probe(index);
            probes.push(async move {
                let result = if level == 0 {
                    lookup.await.into()
                } else {
                    match async_std::future::timeout(timeout, lookup).await {
                        Ok(result) => result.into(),
                        Err(_) => FeedProbe::Transient,
                    }
                };
                (level, index, result)
            });
        }

        let mut completed = [false; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1];
        let mut missing = [false; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1];
        let mut found: [Option<(u64, T)>; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1] =
            std::array::from_fn(|_| None);
        let highest_found_level = loop {
            let Some((level, index, result)) = probes.next().await else {
                break None;
            };
            completed[level] = true;
            match result {
                FeedProbe::Found(payload) => {
                    observe_positive(index, &payload);
                    found[level] = Some((index, payload));
                }
                FeedProbe::Missing => missing[level] = true,
                FeedProbe::Transient => {}
            }

            let Some(highest_found_level) = BOUNDED_INITIAL_LEVELS
                .into_iter()
                .find(|level| found[*level].is_some())
            else {
                continue;
            };
            let higher_levels_are_missing = BOUNDED_INITIAL_LEVELS
                .into_iter()
                .filter(|level| *level > highest_found_level)
                .all(|level| completed[level]);
            if higher_levels_are_missing {
                break Some(highest_found_level);
            }
        };

        let Some(highest_found_level) = highest_found_level else {
            return (None, 0);
        };
        let latest = found[highest_found_level]
            .take()
            .expect("bounded initial feed probe must carry its payload");

        if highest_found_level == 0 || highest_found_level == FEED_FRONTIER_LOOKAHEAD_LEVELS {
            (latest, FEED_FRONTIER_LOOKAHEAD_LEVELS, None)
        } else {
            let missing_level = highest_found_level + 1;
            let known_missing = probe_index(0, missing_level)
                .filter(|_| completed[missing_level] && missing[missing_level]);
            if known_missing.is_some() {
                (latest, highest_found_level, known_missing)
            } else {
                (latest, FEED_FRONTIER_LOOKAHEAD_LEVELS, None)
            }
        }
    } else {
        let first_payload = match probe(0).await.into() {
            FeedProbe::Found(payload) => payload,
            FeedProbe::Missing | FeedProbe::Transient => return (None, 0),
        };
        observe_positive(0, &first_payload);
        ((0_u64, first_payload), FEED_FRONTIER_LOOKAHEAD_LEVELS, None)
    };

    loop {
        if latest.0 == u64::MAX {
            return (Some(latest), u64::MAX);
        }

        let effective_level = (1..=level_limit)
            .rev()
            .find(|level| probe_index(latest.0, *level).is_some())
            .unwrap_or(1);
        let wave_base = latest.0;
        let mut probes = FuturesUnordered::new();

        for level in (1..=effective_level).rev() {
            let Some(index) = probe_index(wave_base, level) else {
                continue;
            };
            let lookup = probe(index);
            probes.push(async move {
                let result = match lookahead_timeout {
                    Some(timeout) => match async_std::future::timeout(timeout, lookup).await {
                        Ok(result) => result.into(),
                        Err(_) => FeedProbe::Transient,
                    },
                    None => lookup.await.into(),
                };
                (level, index, result)
            });
        }

        let mut completed = [false; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1];
        let mut missing = [false; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1];
        let mut found: [Option<(u64, T)>; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1] =
            std::array::from_fn(|_| None);
        let highest_found_level = loop {
            let Some((level, index, result)) = probes.next().await else {
                break 0;
            };
            completed[level] = true;
            match result {
                FeedProbe::Found(payload) => {
                    observe_positive(index, &payload);
                    found[level] = Some((index, payload));
                }
                FeedProbe::Missing => missing[level] = true,
                FeedProbe::Transient => {}
            }

            let Some(highest_found_level) = (1..=effective_level)
                .rev()
                .find(|level| found[*level].is_some())
            else {
                continue;
            };

            // Lower listeners cannot delay a proven frontier; their dispatched work still drains.
            let higher_levels_are_missing =
                ((highest_found_level + 1)..=effective_level).all(|level| completed[level]);
            if higher_levels_are_missing {
                break highest_found_level;
            }
        };

        if highest_found_level == 0 {
            let next = latest.0.saturating_add(1);
            match confirm_sequence_feed_missing(&probe, next, lookahead_timeout).await {
                FeedProbe::Found(payload) => {
                    observe_positive(next, &payload);
                    latest = (next, payload);
                    level_limit = FEED_FRONTIER_LOOKAHEAD_LEVELS;
                    known_missing = None;
                    continue;
                }
                FeedProbe::Missing | FeedProbe::Transient => return (Some(latest), next),
            }
        }

        latest = found[highest_found_level]
            .take()
            .expect("highest found feed probe must carry its index and payload");

        if highest_found_level == effective_level {
            if let Some(missing) = known_missing
                && Some(missing) == latest.0.checked_add(1)
            {
                match confirm_sequence_feed_missing(&probe, missing, lookahead_timeout).await {
                    FeedProbe::Found(payload) => {
                        observe_positive(missing, &payload);
                        latest = (missing, payload);
                        level_limit = FEED_FRONTIER_LOOKAHEAD_LEVELS;
                        known_missing = None;
                        continue;
                    }
                    FeedProbe::Missing | FeedProbe::Transient => {
                        return (Some(latest), missing);
                    }
                }
            }

            level_limit = FEED_FRONTIER_LOOKAHEAD_LEVELS;
            known_missing = None;
            continue;
        }

        let missing_level = highest_found_level + 1;
        known_missing = probe_index(wave_base, missing_level)
            .filter(|_| completed[missing_level] && missing[missing_level]);
        if known_missing.is_none() {
            level_limit = FEED_FRONTIER_LOOKAHEAD_LEVELS;
            continue;
        }
        level_limit = highest_found_level;
    }
}

/// Bee sequence indexes are eight-byte big-endian values.
pub(crate) fn sequence_index_bytes(index: u64) -> [u8; 8] {
    index.to_be_bytes()
}

fn sequence_feed_id_with<F>(topic: &[u8], index: u64, keccak: &mut F) -> [u8; 32]
where
    F: FnMut(&[u8]) -> [u8; 32],
{
    let index = sequence_index_bytes(index);
    let mut preimage = Vec::with_capacity(topic.len() + index.len());
    preimage.extend_from_slice(topic);
    preimage.extend_from_slice(&index);
    keccak(&preimage)
}

pub(crate) fn sequence_feed_id(
    topic: &[u8],
    index: u64,
    mut keccak: impl FnMut(&[u8]) -> [u8; 32],
) -> [u8; 32] {
    sequence_feed_id_with(topic, index, &mut keccak)
}

pub(crate) fn sequence_feed_address(
    topic: &[u8],
    owner: &[u8; 20],
    index: u64,
    mut keccak: impl FnMut(&[u8]) -> [u8; 32],
) -> [u8; 32] {
    let id = sequence_feed_id_with(topic, index, &mut keccak);
    let mut preimage = [0_u8; 52];
    preimage[..32].copy_from_slice(&id);
    preimage[32..].copy_from_slice(owner);
    keccak(&preimage)
}

pub(crate) fn exact_js_feed_index(index: u64) -> Option<f64> {
    exact_u64_as_f64(index)
}

fn exact_u64_as_f64(value: u64) -> Option<f64> {
    let number = value as f64;
    const U64_UPPER_BOUND_EXCLUSIVE: f64 = 18_446_744_073_709_551_616.0;
    (number < U64_UPPER_BOUND_EXCLUSIVE && number as u64 == value).then_some(number)
}
