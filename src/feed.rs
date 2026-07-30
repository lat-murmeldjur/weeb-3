use std::{future::Future, time::Duration};

use futures::stream::{FuturesUnordered, StreamExt};

pub(crate) const FEED_FRONTIER_LOOKAHEAD_LEVELS: usize = 8;
// This deadline only drops the listener; dispatched retrieval/accounting still drains.
pub(crate) const FEED_FRONTIER_LOOKAHEAD_TIMEOUT: Duration = Duration::from_secs(1);
const BOUNDED_INITIAL_LEVELS: [usize; FEED_FRONTIER_LOOKAHEAD_LEVELS] = [8, 7, 6, 5, 4, 3, 2, 0];
fn probe_index(base: u64, level: usize) -> Option<u64> {
    let distance = 1_u64.checked_shl(u32::try_from(level).ok()?)?;
    base.checked_add(distance.checked_sub(1)?)
}

pub(crate) async fn seek_sequence_feed_frontier<T, Probe, ProbeFuture>(
    probe: Probe,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = Option<T>>,
{
    seek_sequence_feed_frontier_inner(None, probe, |_, _| {}, None).await
}

pub(crate) async fn seek_sequence_feed_frontier_from<T, Probe, ProbeFuture>(
    initial_latest: (u64, T),
    probe: Probe,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = Option<T>>,
{
    seek_sequence_feed_frontier_inner(Some(initial_latest), probe, |_, _| {}, None).await
}

pub(crate) async fn seek_sequence_feed_frontier_bounded_observing_positive<
    T,
    Probe,
    ProbeFuture,
    ObservePositive,
>(
    probe: Probe,
    observe_positive: ObservePositive,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = Option<T>>,
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

pub(crate) async fn seek_sequence_feed_frontier_bounded_from_observing_positive<
    T,
    Probe,
    ProbeFuture,
    ObservePositive,
>(
    initial_latest: (u64, T),
    probe: Probe,
    observe_positive: ObservePositive,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = Option<T>>,
    ObservePositive: FnMut(u64, &T),
{
    seek_sequence_feed_frontier_inner(
        Some(initial_latest),
        probe,
        observe_positive,
        Some(FEED_FRONTIER_LOOKAHEAD_TIMEOUT),
    )
    .await
}

async fn seek_sequence_feed_frontier_inner<T, Probe, ProbeFuture, ObservePositive>(
    initial_latest: Option<(u64, T)>,
    probe: Probe,
    mut observe_positive: ObservePositive,
    lookahead_timeout: Option<Duration>,
) -> (Option<(u64, T)>, u64)
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = Option<T>>,
    ObservePositive: FnMut(u64, &T),
{
    let (mut latest, mut level_limit, mut known_missing) = if let Some(latest) = initial_latest {
        (latest, FEED_FRONTIER_LOOKAHEAD_LEVELS, None)
    } else if let Some(timeout) = lookahead_timeout {
        // An authenticated higher sequence update proves the lower contiguous interval.
        let mut probes = FuturesUnordered::new();
        for level in BOUNDED_INITIAL_LEVELS {
            let index = if level == 0 {
                0
            } else {
                probe_index(0, level).expect("bounded initial feed level")
            };
            let lookup = probe(index);
            probes.push(async move {
                let payload = if level == 0 {
                    lookup.await
                } else {
                    async_std::future::timeout(timeout, lookup)
                        .await
                        .ok()
                        .flatten()
                };
                (level, index, payload)
            });
        }

        let mut completed = [false; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1];
        let mut found: [Option<(u64, T)>; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1] =
            std::array::from_fn(|_| None);
        let highest_found_level = loop {
            let Some((level, index, payload)) = probes.next().await else {
                break None;
            };
            completed[level] = true;
            if let Some(payload) = payload {
                observe_positive(index, &payload);
                found[level] = Some((index, payload));
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
                .all(|level| completed[level] && found[level].is_none());
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
                .filter(|_| completed[missing_level] && found[missing_level].is_none());
            if known_missing.is_some() {
                (latest, highest_found_level, known_missing)
            } else {
                (latest, FEED_FRONTIER_LOOKAHEAD_LEVELS, None)
            }
        }
    } else {
        let Some(first_payload) = probe(0).await else {
            return (None, 0);
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
                let payload = match lookahead_timeout {
                    Some(timeout) => async_std::future::timeout(timeout, lookup)
                        .await
                        .ok()
                        .flatten(),
                    None => lookup.await,
                };
                (level, index, payload)
            });
        }

        let mut completed = [false; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1];
        let mut found: [Option<(u64, T)>; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1] =
            std::array::from_fn(|_| None);
        let highest_found_level = loop {
            let Some((level, index, payload)) = probes.next().await else {
                break 0;
            };
            completed[level] = true;
            if let Some(payload) = payload {
                observe_positive(index, &payload);
                found[level] = Some((index, payload));
            }

            let Some(highest_found_level) = (1..=effective_level)
                .rev()
                .find(|level| found[*level].is_some())
            else {
                continue;
            };

            // Lower listeners cannot delay a proven frontier; their dispatched work still drains.
            let higher_levels_are_missing = ((highest_found_level + 1)..=effective_level)
                .all(|level| completed[level] && found[level].is_none());
            if higher_levels_are_missing {
                break highest_found_level;
            }
        };

        if highest_found_level == 0 {
            let next = latest.0.saturating_add(1);
            return (Some(latest), next);
        }

        latest = found[highest_found_level]
            .take()
            .expect("highest found feed probe must carry its index and payload");

        if highest_found_level == effective_level {
            if known_missing == latest.0.checked_add(1) {
                return (Some(latest), known_missing.unwrap_or(u64::MAX));
            }

            level_limit = FEED_FRONTIER_LOOKAHEAD_LEVELS;
            known_missing = None;
            continue;
        }

        let missing_level = highest_found_level + 1;
        known_missing = probe_index(wave_base, missing_level)
            .filter(|_| completed[missing_level] && found[missing_level].is_none());
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
