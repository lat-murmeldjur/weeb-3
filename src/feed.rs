use std::future::Future;

use futures::stream::{FuturesUnordered, StreamExt};

pub(crate) const FEED_FRONTIER_LOOKAHEAD_LEVELS: usize = 8;

pub(crate) enum FeedProbe<T> {
    Found(T),
    Missing,
    Transient,
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

async fn collect_feed_wave<T>(
    probes: &mut FuturesUnordered<impl Future<Output = (usize, u64, FeedProbe<T>)>>,
    maximum_level: usize,
) -> Option<(usize, (u64, T), Option<u64>)> {
    let mut completed = [false; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1];
    let mut missing = [None; FEED_FRONTIER_LOOKAHEAD_LEVELS + 1];
    let mut highest = 0;
    let mut found = None;
    while let Some((level, index, result)) = probes.next().await {
        completed[level] = true;
        match result {
            FeedProbe::Found(payload) if level > highest => {
                highest = level;
                found = Some((index, payload));
            }
            FeedProbe::Missing => missing[level] = Some(index),
            _ => {}
        }
        // Lower listeners cannot delay a proven frontier; their dispatched work still drains.
        if found.is_some() && (highest + 1..=maximum_level).all(|level| completed[level]) {
            return Some((
                highest,
                found.unwrap(),
                missing.get(highest + 1).copied().flatten(),
            ));
        }
    }
    None
}

pub(crate) async fn seek_sequence_feed_frontier<T, Probe, ProbeFuture, ProbeResult>(
    probe: Probe,
) -> Result<(Option<(u64, T)>, Option<u64>), ()>
where
    Probe: Fn(u64) -> ProbeFuture,
    ProbeFuture: Future<Output = ProbeResult>,
    ProbeResult: Into<FeedProbe<T>>,
{
    let first_payload = match probe(0).await.into() {
        FeedProbe::Found(payload) => payload,
        FeedProbe::Missing => return Ok((None, Some(0))),
        FeedProbe::Transient => return Err(()),
    };
    let mut latest = (0_u64, first_payload);
    let mut level_limit = FEED_FRONTIER_LOOKAHEAD_LEVELS;
    let mut known_missing = None;

    loop {
        if latest.0 == u64::MAX {
            return Ok((Some(latest), None));
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
                let result = lookup.await.into();
                (level, index, result)
            });
        }

        let next = match collect_feed_wave(&mut probes, effective_level).await {
            Some((highest_found_level, found, missing)) => {
                latest = found;
                if highest_found_level != effective_level {
                    known_missing = missing;
                    level_limit = if missing.is_some() {
                        highest_found_level
                    } else {
                        FEED_FRONTIER_LOOKAHEAD_LEVELS
                    };
                    continue;
                }
                level_limit = FEED_FRONTIER_LOOKAHEAD_LEVELS;
                let Some(next) = known_missing.take()
                    .filter(|missing| Some(*missing) == latest.0.checked_add(1))
                else {
                    continue;
                };
                next
            }
            None => latest.0.saturating_add(1),
        };
        match probe(next).await.into() {
            FeedProbe::Found(payload) => {
                latest = (next, payload);
                level_limit = FEED_FRONTIER_LOOKAHEAD_LEVELS;
                known_missing = None;
            }
            FeedProbe::Missing => return Ok((Some(latest), Some(next))),
            FeedProbe::Transient => return Ok((Some(latest), None)),
        }
    }
}

/// Bee sequence indexes are eight-byte big-endian values.
pub(crate) fn sequence_index_bytes(index: u64) -> [u8; 8] {
    index.to_be_bytes()
}

pub(crate) fn sequence_feed_id(
    topic: &[u8],
    index: u64,
    mut keccak: impl FnMut(&[u8]) -> [u8; 32],
) -> [u8; 32] {
    let index = sequence_index_bytes(index);
    let mut preimage = Vec::with_capacity(topic.len() + index.len());
    preimage.extend_from_slice(topic);
    preimage.extend_from_slice(&index);
    keccak(&preimage)
}

pub(crate) fn sequence_feed_address(
    topic: &[u8],
    owner: &[u8; 20],
    index: u64,
    mut keccak: impl FnMut(&[u8]) -> [u8; 32],
) -> [u8; 32] {
    let id = sequence_feed_id(topic, index, &mut keccak);
    let mut preimage = [0_u8; 52];
    preimage[..32].copy_from_slice(&id);
    preimage[32..].copy_from_slice(owner);
    keccak(&preimage)
}

pub(crate) fn exact_js_feed_index(index: u64) -> Option<f64> {
    let number = index as f64;
    const U64_UPPER_BOUND_EXCLUSIVE: f64 = 18_446_744_073_709_551_616.0;
    (number < U64_UPPER_BOUND_EXCLUSIVE && number as u64 == index).then_some(number)
}
