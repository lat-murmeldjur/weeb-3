use serde::{Deserialize, Deserializer, Serialize};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

use crate::stream_conventions::normalize_route_base;

const HLS_HEADER: &str = "#EXTM3U";
const HLS_ENDLIST: &str = "#EXT-X-ENDLIST";
const HLS_SERVER_CONTROL: &str = "#EXT-X-SERVER-CONTROL";
pub(crate) const HLS_LIVE_SYNC_DURATION_COUNT: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsManifestProbe {
    Manifest,
    NotManifest,
    NeedMore,
}

pub(crate) fn hls_prefix_stagger_remaining_ms(
    minimum_interval_ms: u64,
    last_admission_ms: Option<f64>,
    now_ms: Option<f64>,
) -> u64 {
    let Some(last_admission_ms) = last_admission_ms else {
        return 0;
    };
    let Some(now_ms) = now_ms else {
        return minimum_interval_ms;
    };
    if !last_admission_ms.is_finite() || !now_ms.is_finite() || now_ms < last_admission_ms {
        return minimum_interval_ms;
    }

    let remaining_ms = minimum_interval_ms as f64 - (now_ms - last_admission_ms);
    if remaining_ms <= 0.0 {
        0
    } else {
        remaining_ms.ceil().min(minimum_interval_ms as f64) as u64
    }
}

pub(crate) fn hls_prefix_admission_window_is_open(
    generation_current: bool,
    playback_active: bool,
    now_ms: Option<f64>,
    startup_deadline_ms: f64,
) -> bool {
    generation_current
        && (playback_active
            || now_ms.is_some_and(|now_ms| {
                now_ms.is_finite()
                    && startup_deadline_ms.is_finite()
                    && now_ms < startup_deadline_ms
            }))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HlsEarlyPrefixAdmission {
    pub(crate) reference: String,
    pub(crate) rolling: bool,
}

pub(crate) struct HlsEarlyPrefixPolicy {
    active_limit: usize,
    target_limit: usize,
    canonical: Vec<String>,
    attempted: HashSet<String>,
    active: HashSet<String>,
    succeeded: HashSet<String>,
    next_refill_position: usize,
    refill_credits: usize,
    closed: bool,
}

impl HlsEarlyPrefixPolicy {
    pub(crate) fn new(active_limit: usize, target_limit: usize) -> Self {
        let active_limit = active_limit.max(1);
        Self {
            active_limit,
            target_limit: target_limit.max(active_limit),
            canonical: Vec::new(),
            attempted: HashSet::new(),
            active: HashSet::new(),
            succeeded: HashSet::new(),
            next_refill_position: active_limit,
            refill_credits: 0,
            closed: false,
        }
    }

    pub(crate) fn observe(&mut self, references: &[String]) -> bool {
        if self.closed {
            return false;
        }
        let observed = references
            .iter()
            .take(self.target_limit)
            .cloned()
            .collect::<Vec<_>>();
        if observed.is_empty() {
            return false;
        }
        if self.canonical.is_empty() || observed.starts_with(&self.canonical) {
            self.canonical = observed;
            true
        } else {
            false
        }
    }

    pub(crate) fn target_complete(&self) -> bool {
        !self.closed
            && self.canonical.len() >= self.target_limit
            && self.active.is_empty()
            && self
                .canonical
                .iter()
                .take(self.target_limit)
                .all(|reference| self.succeeded.contains(reference))
    }

    pub(crate) fn next_admission(
        &mut self,
        admission_current: bool,
    ) -> Option<HlsEarlyPrefixAdmission> {
        if !admission_current {
            self.closed = true;
            self.active.clear();
            return None;
        }
        if self.closed || self.active.len() >= self.active_limit {
            return None;
        }

        for reference in self.canonical.iter().take(self.active_limit) {
            if self.attempted.insert(reference.clone()) {
                self.active.insert(reference.clone());
                return Some(HlsEarlyPrefixAdmission {
                    reference: reference.clone(),
                    rolling: false,
                });
            }
        }

        while self.refill_credits > 0 && self.next_refill_position < self.canonical.len() {
            let position = self.next_refill_position;
            self.next_refill_position = self.next_refill_position.saturating_add(1);
            let reference = self.canonical[position].clone();
            if self.attempted.insert(reference.clone()) {
                self.refill_credits = self.refill_credits.saturating_sub(1);
                self.active.insert(reference.clone());
                return Some(HlsEarlyPrefixAdmission {
                    reference,
                    rolling: true,
                });
            }
        }
        None
    }

    pub(crate) fn reject(&mut self, reference: &str) {
        self.active.remove(reference);
        self.closed = true;
    }

    pub(crate) fn complete(&mut self, reference: &str, succeeded: bool) {
        if !self.active.remove(reference) {
            return;
        }
        if succeeded {
            if self.succeeded.insert(reference.to_string()) {
                self.refill_credits = self.refill_credits.saturating_add(1);
            }
        } else {
            self.closed = true;
        }
    }

    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        self.active.len()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HlsMediaCursor {
    pub(crate) plan_id: u64,
    pub(crate) references: Arc<[String]>,
    pub(crate) position: usize,
    pub(crate) early_overlap_limit: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HlsMediaSelection {
    pub(crate) cursor: HlsMediaCursor,
    pub(crate) superseded_plan_ids: Vec<u64>,
}

/// Bounded registry of ordered media-fragment plans.
///
/// Live HLS playlist reloads commonly overlap the previous window. Selection
/// migrates an active cursor to the newest plan only when the two sequences
/// have an adjacent overlap (or the old tail is the new head), which avoids
/// confusing unrelated renditions that happen to share one immutable asset.
pub(crate) struct HlsMediaPlanRegistry {
    max_references: usize,
    next_plan_id: u64,
    cursor_count: usize,
    cursors: HashMap<String, Vec<HlsMediaCursor>>,
}

impl HlsMediaPlanRegistry {
    pub(crate) fn new(max_references: usize) -> Self {
        Self {
            max_references: max_references.max(1),
            next_plan_id: 0,
            cursor_count: 0,
            cursors: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn install(&mut self, references: Vec<String>) {
        self.install_with_early_overlap_limit(references, usize::MAX);
    }

    pub(crate) fn install_with_early_overlap_limit(
        &mut self,
        mut references: Vec<String>,
        early_overlap_limit: usize,
    ) {
        references.truncate(self.max_references);
        if references.is_empty() {
            return;
        }

        // A live manifest may be polled several times before its feed advances.
        // Reusing the identical plan avoids artificial plan/track churn.
        if self.cursors.get(&references[0]).is_some_and(|candidates| {
            candidates.iter().any(|cursor| {
                cursor.position == 0
                    && cursor.references.as_ref() == references.as_slice()
                    && cursor.early_overlap_limit == early_overlap_limit
            })
        }) {
            return;
        }

        if self.cursor_count.saturating_add(references.len()) > self.max_references {
            self.cursors.clear();
            self.cursor_count = 0;
        }

        self.next_plan_id = next_nonzero_plan_id(self.next_plan_id);
        let plan_id = self.next_plan_id;
        let references: Arc<[String]> = references.into();
        for (position, reference) in references.iter().enumerate() {
            self.cursors
                .entry(reference.clone())
                .or_default()
                .push(HlsMediaCursor {
                    plan_id,
                    references: references.clone(),
                    position,
                    early_overlap_limit,
                });
            self.cursor_count = self.cursor_count.saturating_add(1);
        }
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
                let position = preferred.get(&cursor.plan_id)?;
                Some((
                    cursor.position.abs_diff(*position),
                    usize::MAX - index,
                    index,
                ))
            })
            .min_by_key(|candidate| (candidate.0, candidate.1))
            .map(|candidate| candidate.2);

        let selected_index = if let Some(preferred_index) = preferred_index {
            let preferred_cursor = &candidates[preferred_index];
            candidates
                .iter()
                .enumerate()
                .skip(preferred_index.saturating_add(1))
                .rev()
                .find(|(_, candidate)| {
                    candidate.plan_id != preferred_cursor.plan_id
                        && hls_media_cursors_compatible(preferred_cursor, candidate)
                })
                .map(|(index, _)| index)
                .unwrap_or(preferred_index)
        } else {
            candidates.len().checked_sub(1)?
        };
        let cursor = candidates[selected_index].clone();

        let mut superseded_plan_ids = Vec::new();
        let mut seen = HashSet::new();
        for candidate in candidates.iter().take(selected_index) {
            if candidate.plan_id != cursor.plan_id
                && preferred.contains_key(&candidate.plan_id)
                && hls_media_cursors_compatible(candidate, &cursor)
                && seen.insert(candidate.plan_id)
            {
                superseded_plan_ids.push(candidate.plan_id);
            }
        }

        Some(HlsMediaSelection {
            cursor,
            superseded_plan_ids,
        })
    }
}

fn next_nonzero_plan_id(current: u64) -> u64 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

fn hls_media_cursors_compatible(left: &HlsMediaCursor, right: &HlsMediaCursor) -> bool {
    if left.references.get(left.position) != right.references.get(right.position) {
        return false;
    }

    let previous_matches = left.position.checked_sub(1).is_some_and(|left_position| {
        right.position.checked_sub(1).is_some_and(|right_position| {
            left.references.get(left_position) == right.references.get(right_position)
        })
    });
    let next_matches = left
        .references
        .get(left.position.saturating_add(1))
        .is_some_and(|left_reference| {
            right.references.get(right.position.saturating_add(1)) == Some(left_reference)
        });
    let old_tail_is_new_head =
        left.position.saturating_add(1) == left.references.len() && right.position == 0;

    previous_matches || next_matches || old_tail_is_new_head
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HlsTrackRetention {
    pub(crate) plan_id: u64,
    pub(crate) last_touch: u64,
    pub(crate) running: bool,
}

/// Choose obsolete/inactive HLS prefetch tracks to remove.
///
/// Superseded live-plan tracks are retired even while their observer is still
/// running: the underlying retrieval leader is detached and continues to
/// settle, while removing the ticket only stops future admission and retries.
/// The selected plan and unrelated running rendition tracks remain protected.
pub(crate) fn hls_track_ids_to_prune(
    tracks: &[HlsTrackRetention],
    selected_plan_id: u64,
    superseded_plan_ids: &[u64],
    max_entries: usize,
) -> Vec<u64> {
    let superseded = superseded_plan_ids.iter().copied().collect::<HashSet<_>>();
    let mut prune = tracks
        .iter()
        .filter(|track| track.plan_id != selected_plan_id && superseded.contains(&track.plan_id))
        .map(|track| track.plan_id)
        .collect::<HashSet<_>>();

    let mut retained = tracks.len().saturating_sub(prune.len());
    if retained > max_entries {
        let mut inactive = tracks
            .iter()
            .filter(|track| {
                track.plan_id != selected_plan_id
                    && !track.running
                    && !prune.contains(&track.plan_id)
            })
            .copied()
            .collect::<Vec<_>>();
        inactive.sort_by_key(|track| track.last_touch);
        for track in inactive {
            if retained <= max_entries {
                break;
            }
            if prune.insert(track.plan_id) {
                retained = retained.saturating_sub(1);
            }
        }
    }

    let mut prune = prune.into_iter().collect::<Vec<_>>();
    prune.sort_unstable();
    prune
}

/// Read a forward-playback cache entry while demoting already consumed media
/// to the eviction end. Duplicate/retry/back-seek reads remain local until the
/// next insertion needs space, at which point past media is evicted before
/// completed future lookahead.
pub(crate) fn read_forward_cache_entry<T: Clone>(
    order: &mut VecDeque<String>,
    entries: &HashMap<String, T>,
    key: &str,
) -> Option<T> {
    let value = entries.get(key)?.clone();
    order.retain(|cached_key| cached_key != key);
    order.push_front(key.to_string());
    Some(value)
}

/// Classify an ordered HLS request without letting a cached validator,
/// duplicate, or back-read rewind the forward playback cursor.
///
/// A non-adjacent forward request is still a seek even when it was prefetched.
/// An uncached backward request is also a real seek and pivots lookahead.
pub(crate) fn hls_foreground_cursor_transition(
    last_position: usize,
    requested_position: usize,
    cached: bool,
) -> (bool, usize) {
    if cached && requested_position < last_position {
        return (false, last_position);
    }

    (
        last_position.abs_diff(requested_position) > 1,
        requested_position,
    )
}

pub(crate) struct HlsOrderedProbeWindow {
    next_probe_position: usize,
    next_admit_position: usize,
    ready: BTreeMap<usize, Option<u64>>,
}

impl HlsOrderedProbeWindow {
    pub(crate) fn new(first_position: usize) -> Self {
        Self {
            next_probe_position: first_position,
            next_admit_position: first_position,
            ready: BTreeMap::new(),
        }
    }

    pub(crate) fn fill_positions(
        &mut self,
        reference_count: usize,
        max_outstanding: usize,
    ) -> Vec<usize> {
        let mut positions = Vec::new();
        while self.next_probe_position < reference_count
            && self
                .next_probe_position
                .saturating_sub(self.next_admit_position)
                < max_outstanding
        {
            positions.push(self.next_probe_position);
            self.next_probe_position = self.next_probe_position.saturating_add(1);
        }
        positions
    }

    pub(crate) fn complete(&mut self, position: usize, size: Option<u64>) {
        if position >= self.next_admit_position && position < self.next_probe_position {
            self.ready.insert(position, size);
        }
    }

    pub(crate) fn next_ready(&self) -> Option<(usize, Option<u64>)> {
        self.ready
            .get(&self.next_admit_position)
            .copied()
            .map(|size| (self.next_admit_position, size))
    }

    pub(crate) fn commit_ready(&mut self) -> Option<(usize, Option<u64>)> {
        let position = self.next_admit_position;
        let size = self.ready.remove(&position)?;
        self.next_admit_position = self.next_admit_position.saturating_add(1);
        Some((position, size))
    }
}

/// A stream feed should only contain a catalog or an HLS playlist. Capping it
/// before parsing and before copying the joined CAC payload prevents a bogus
/// feed from causing an unbounded allocation while still leaving ample room
/// for very long VOD playlists.
pub(crate) const MAX_STREAM_FEED_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct StreamCatalogEntry {
    pub owner: String,
    pub topic: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub state: String,
    #[serde(default, alias = "isExternal")]
    pub is_external: bool,
    #[serde(default, alias = "mediaType", alias = "mediatype")]
    pub media_type: String,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub duration: Option<f64>,
    #[serde(default)]
    pub index: Option<u64>,
    #[serde(default)]
    pub timestamp: Option<u64>,
    #[serde(default, alias = "createdAt")]
    pub created_at: Option<u64>,
    #[serde(default, alias = "updatedAt")]
    pub updated_at: Option<u64>,
}

impl StreamCatalogEntry {
    pub(crate) fn is_live(&self) -> bool {
        self.state.eq_ignore_ascii_case("live")
    }

    pub(crate) fn is_vod(&self) -> bool {
        self.state.eq_ignore_ascii_case("vod")
    }

    pub(crate) fn media_type(&self) -> &'static str {
        if self.media_type.eq_ignore_ascii_case("audio") {
            "audio"
        } else {
            "video"
        }
    }

    fn sort_timestamp(&self) -> u64 {
        self.timestamp
            .or(self.updated_at)
            .or(self.created_at)
            .unwrap_or_default()
    }
}

pub(crate) fn parse_stream_catalog(bytes: &[u8]) -> Option<Vec<StreamCatalogEntry>> {
    if !stream_feed_payload_len_is_supported(bytes.len()) {
        return None;
    }

    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let entries = match value {
        serde_json::Value::Array(entries) => entries,
        serde_json::Value::Object(mut object) => match object.remove("entries")? {
            serde_json::Value::Array(entries) => entries,
            _ => return None,
        },
        _ => return None,
    };

    let mut entries: Vec<StreamCatalogEntry> = entries
        .into_iter()
        .filter_map(|entry| {
            let mut entry: StreamCatalogEntry = serde_json::from_value(entry).ok()?;
            if entry.is_external {
                return None;
            }
            let owner = entry
                .owner
                .strip_prefix("0x")
                .or_else(|| entry.owner.strip_prefix("0X"))
                .unwrap_or(&entry.owner);
            if !is_hex_len(owner, 40) || entry.topic.trim().is_empty() || entry.topic.len() > 256 {
                return None;
            }
            // Bee's owner path takes the unprefixed 20-byte address.
            if owner.len() != entry.owner.len() {
                entry.owner = owner.to_owned();
            }
            Some(entry)
        })
        .collect();

    entries.sort_by(|left, right| {
        right
            .is_live()
            .cmp(&left.is_live())
            .then_with(|| right.sort_timestamp().cmp(&left.sort_timestamp()))
            .then_with(|| {
                right
                    .index
                    .unwrap_or_default()
                    .cmp(&left.index.unwrap_or_default())
            })
    });

    Some(entries)
}

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
        .map(|line| line.trim_end_matches('\r').trim() == HLS_HEADER)
        .unwrap_or(false)
}

/// Classify a prefix without mistaking a truncated first line for a binary
/// asset. Range and HEAD requests can therefore sniff ordinary media cheaply
/// while retaining exact semantics for unusually padded or split HLS headers.
pub(crate) fn probe_hls_manifest(prefix: &[u8], total_len: u64) -> HlsManifestProbe {
    // Unsupported oversized manifests must not turn an ambiguous prefix into
    // an unbounded full-tree join during an HLS HEAD or Range request.
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

/// Prefer an authenticated sequence-zero prefix over a late archived window.
///
/// Some stream producers publish rolling VOD snapshots before the final
/// sequence-zero archive. Playing that late window first gives it timestamp
/// zero, so discovering the archive later requires rebuilding MediaSource and
/// visibly skips the beginning. The caller supplies the route intent: direct
/// unindexed `/stream` links start from the beginning, while feed/catalog live
/// views keep their selected rolling semantics. A short prefix is not useful
/// enough to replace the selected frontier candidate.
pub(crate) fn hls_startup_prefix_is_preferred(
    canonical: &[u8],
    prefix: &[u8],
    minimum_prefix_segments: usize,
    sequence_zero_start_requested: bool,
) -> bool {
    if !sequence_zero_start_requested
        || !is_hls_manifest(canonical)
        || !is_hls_manifest(prefix)
        || !hls_media_sequence(canonical).is_some_and(|sequence| sequence > 0)
        || hls_media_sequence(prefix) != Some(0)
    {
        return false;
    }

    hls_media_references(prefix).len() >= minimum_prefix_segments
}

/// Decide whether a newer media-playlist snapshot can replace the one already
/// exposed to an active player without moving its timeline.
///
/// Feed frontier probes may jump across more updates than the producer's live
/// window retains. Such a non-overlapping snapshot is authenticated, but
/// publishing it would make hls.js seek to a different window. A rolling
/// successor overlaps by media sequence, while the terminal full archive
/// contains the current rolling window at the same sequence numbers.
pub(crate) fn hls_manifest_reload_is_continuous(current: &[u8], candidate: &[u8]) -> bool {
    let Some(current_sequence) = hls_media_sequence(current) else {
        return false;
    };
    let Some(candidate_sequence) = hls_media_sequence(candidate) else {
        return false;
    };
    let Some(current_segments) = hls_segment_identities(current) else {
        return false;
    };
    let Some(candidate_segments) = hls_segment_identities(candidate) else {
        return false;
    };
    if current_segments.is_empty() || candidate_segments.is_empty() {
        return false;
    }

    let Ok(current_len) = u64::try_from(current_segments.len()) else {
        return false;
    };
    let Ok(candidate_len) = u64::try_from(candidate_segments.len()) else {
        return false;
    };
    let Some(current_end) = current_sequence.checked_add(current_len) else {
        return false;
    };
    let Some(candidate_end) = candidate_sequence.checked_add(candidate_len) else {
        return false;
    };
    if candidate_end < current_end {
        return false;
    }
    let overlap_start = current_sequence.max(candidate_sequence);
    let overlap_end = current_end.min(candidate_end);
    if overlap_start >= overlap_end {
        return false;
    }

    (overlap_start..overlap_end).all(|sequence| {
        let current_position = usize::try_from(sequence - current_sequence).ok();
        let candidate_position = usize::try_from(sequence - candidate_sequence).ok();
        current_position.zip(candidate_position).is_some_and(
            |(current_position, candidate_position)| {
                current_segments.get(current_position) == candidate_segments.get(candidate_position)
            },
        )
    })
}

/// Extend a start-at-zero archive with a simple overlapping rolling window.
///
/// The direct archived-stream route is exposed as an EVENT playlist until its
/// authenticated final update is confirmed. EVENT history is append-only, so
/// replacing it with a sliding window would make hls.js discard the playhead
/// and seek to that window's first fragment. For classic segment playlists we
/// can instead retain the sequence-zero prefix and append only the candidate's
/// new, identity-checked suffix. Manifests with stateful segment tags are held
/// for the final sequence-zero archive rather than synthesized unsafely.
pub(crate) fn extend_hls_sequence_zero_archive(
    current: &[u8],
    candidate: &[u8],
) -> Option<Vec<u8>> {
    if hls_media_sequence(current) != Some(0)
        || !hls_manifest_reload_is_continuous(current, candidate)
        || !hls_has_at_most_one_endlist(current)
        || !hls_has_at_most_one_endlist(candidate)
    {
        return None;
    }

    let candidate_sequence = hls_media_sequence(candidate)?;
    if candidate_sequence == 0 {
        return Some(candidate.to_vec());
    }
    if !hls_append_only_tags_are_supported(current)
        || !hls_append_only_tags_are_supported(candidate)
    {
        return None;
    }

    let current_segments = hls_segment_identities(current)?;
    let candidate_segments = hls_segment_identities(candidate)?;
    let current_len = u64::try_from(current_segments.len()).ok()?;
    let candidate_len = u64::try_from(candidate_segments.len()).ok()?;
    let current_end = current_len;
    let candidate_end = candidate_sequence.checked_add(candidate_len)?;
    if candidate_sequence >= current_end || candidate_end <= current_end {
        return None;
    }

    let current_uri_ends = hls_segment_uri_line_ends(current)?;
    let candidate_uri_ends = hls_segment_uri_line_ends(candidate)?;
    if current_uri_ends.len() != current_segments.len()
        || candidate_uri_ends.len() != candidate_segments.len()
    {
        return None;
    }

    let overlap_position = usize::try_from(
        current_end
            .checked_sub(1)?
            .checked_sub(candidate_sequence)?,
    )
    .ok()?;
    let appended_position = overlap_position.checked_add(1)?;
    let current_prefix_end = *current_uri_ends.last()?;
    let candidate_suffix_start = *candidate_uri_ends.get(overlap_position)?;
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
    // EXT-X-ENDLIST may legally occur anywhere in a Media Playlist. The
    // rolling candidate's copy can therefore sit before the overlapping URI
    // where the synthetic suffix starts. Preserve its terminal meaning in a
    // canonical trailing position instead of silently returning a finalized
    // cache entry whose visible body still looks live.
    if hls_is_finalized(candidate) && !hls_is_finalized(&merged) {
        if !merged.ends_with(b"\n") {
            merged.push(b'\n');
        }
        merged.extend_from_slice(HLS_ENDLIST.as_bytes());
        merged.push(b'\n');
    }
    if !stream_feed_payload_len_is_supported(merged.len()) {
        return None;
    }

    let mut expected = current_segments;
    expected.extend_from_slice(candidate_segments.get(appended_position..)?);
    (hls_media_sequence(&merged) == Some(0)
        && hls_segment_identities(&merged).as_ref() == Some(&expected))
    .then_some(merged)
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
                "#EXT-X-INDEPENDENT-SEGMENTS" | "#EXT-X-DISCONTINUITY" | "#EXT-X-ENDLIST"
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

fn hls_segment_uri_line_ends(bytes: &[u8]) -> Option<Vec<usize>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut uri_ends = Vec::new();
    let mut expects_media_uri = false;
    let mut offset = 0usize;
    for raw_line in text.split_inclusive('\n') {
        let line = raw_line
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .trim();
        if line.starts_with("#EXTINF:") {
            if expects_media_uri {
                return None;
            }
            expects_media_uri = true;
        } else if line.starts_with('#') || line.is_empty() {
            // Segment-scoped tags may occur between EXTINF and its URI.
        } else if expects_media_uri {
            swarm_bytes_reference(line)?;
            uri_ends.push(offset.checked_add(raw_line.len())?);
            expects_media_uri = false;
        }
        offset = offset.checked_add(raw_line.len())?;
    }
    (!expects_media_uri).then_some(uri_ends)
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

/// An HLS client cannot merge an archive that grows backward from an
/// already-buffered rolling window without moving the presentation origin.
///
/// hls.js aligns overlapping sequence numbers to their existing media
/// timestamps. When a rolling `636..645` playlist is replaced by a complete
/// `0..645` archive, that would put the newly discovered prefix at negative
/// time and leave the MediaSource duration near the old rolling-window length.
/// The unindexed route may expose the archive provisionally before its ENDLIST
/// is head-confirmed, so the player must rebuild on the backwards expansion
/// itself rather than waiting for the later live-to-finite transition.
pub(crate) fn hls_timeline_rebase_required(
    previous_start_sequence: u64,
    previous_live: bool,
    candidate_start_sequence: u64,
    _candidate_live: bool,
) -> bool {
    previous_live && candidate_start_sequence < previous_start_sequence
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HlsLevelTransition {
    pub(crate) rebase: bool,
    pub(crate) terminal_ready: bool,
}

/// Classify one rendition-local hls.js level update.
///
/// A backwards archive expansion supersedes an already scheduled recovery:
/// rebuilding the player creates a new session id, so the old recovery timer
/// cannot act on the replacement session. Suppressing the rebase here would
/// consume the sequence-zero snapshot and leave no later transition capable
/// of correcting the MediaSource origin.
pub(crate) fn classify_hls_level_transition(
    previous: Option<(u64, bool)>,
    rebase_attempted: bool,
    _recovery_pending: bool,
    candidate_start_sequence: u64,
    candidate_live: bool,
) -> HlsLevelTransition {
    HlsLevelTransition {
        rebase: !rebase_attempted
            && previous.is_some_and(|(previous_start, previous_live)| {
                hls_timeline_rebase_required(
                    previous_start,
                    previous_live,
                    candidate_start_sequence,
                    candidate_live,
                )
            }),
        terminal_ready: !candidate_live,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HlsSegmentIdentity {
    reference: String,
    duration_bits: u64,
    /// Effective `(offset, length)`, not merely the source spelling. This
    /// distinguishes implicit offsets and makes equal identities independent
    /// of whitespace or decimal formatting.
    byte_range: Option<(u64, u64)>,
    /// Effective discontinuity counter, matching the `cc` value hls.js uses
    /// when it merges overlapping media sequence numbers.
    discontinuity_counter: u64,
}

fn hls_segment_identities(bytes: &[u8]) -> Option<Vec<HlsSegmentIdentity>> {
    if !stream_feed_payload_len_is_supported(bytes.len()) || !is_hls_manifest(bytes) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;

    let mut segments = Vec::new();
    let mut expects_media_uri = false;
    let mut duration_bits = None::<u64>;
    let mut byte_range = None::<(u64, Option<u64>)>;
    let mut previous_range_end = None::<(String, u64)>;
    let mut discontinuity_counter = 0_u64;
    let mut saw_discontinuity_sequence = false;
    for original_line in text.lines() {
        let line = original_line.trim_end_matches('\r').trim();
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
            duration_bits = Some(duration.to_bits());
            byte_range = None;
        } else if let Some(value) = line.strip_prefix("#EXT-X-DISCONTINUITY-SEQUENCE:") {
            if saw_discontinuity_sequence || expects_media_uri || !segments.is_empty() {
                return None;
            }
            discontinuity_counter = value.trim().parse().ok()?;
            saw_discontinuity_sequence = true;
        } else if line == "#EXT-X-DISCONTINUITY" {
            discontinuity_counter = discontinuity_counter.checked_add(1)?;
        } else if let Some(value) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            if !expects_media_uri || byte_range.is_some() || value.trim().is_empty() {
                return None;
            }
            byte_range = Some(parse_hls_byte_range(value.trim())?);
        } else if line.starts_with('#') || line.is_empty() {
            // Tags such as DISCONTINUITY may sit between EXTINF and its URI.
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
                Some((offset, length)) => Some((reference.clone(), offset.checked_add(length)?)),
                None => None,
            };
            segments.push(HlsSegmentIdentity {
                reference,
                duration_bits: duration_bits.take()?,
                byte_range: effective_range,
                discontinuity_counter,
            });
            expects_media_uri = false;
        } else {
            // A multivariant URI or malformed mixed playlist cannot establish
            // media-sequence continuity for an active rendition.
            return None;
        }
    }
    (!expects_media_uri).then_some(segments)
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

/// Return the ordered immutable Swarm references that carry media fragments.
///
/// Variant playlists, alternate renditions, keys, and initialization maps are
/// deliberately excluded. The caller uses this sequence for a small rolling
/// playback lookahead, so following a master-playlist URI or a repeated key
/// would prefetch the wrong part of the presentation. Low-latency media parts
/// are included because they are fragment payloads in their own right.
pub(crate) fn hls_media_references(bytes: &[u8]) -> Vec<String> {
    if !stream_feed_payload_len_is_supported(bytes.len()) || !is_hls_manifest(bytes) {
        return Vec::new();
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };

    let mut references = Vec::new();
    let mut expects_media_uri = false;
    for original_line in text.lines() {
        let line = original_line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with("#EXTINF:") {
            expects_media_uri = true;
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
            // Tags such as EXT-X-BYTERANGE and EXT-X-DISCONTINUITY may occur
            // between EXTINF and its media URI, so retain the pending marker.
            continue;
        }

        if expects_media_uri {
            if let Some(reference) = swarm_bytes_reference(line) {
                push_distinct_reference(&mut references, reference);
            }
            expects_media_uri = false;
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

/// Best-effort media type for assets referenced by an HLS playlist.
///
/// The same Bee `/bytes` route may carry transport streams, fragmented MP4,
/// audio, subtitles, initialization maps, or encryption keys. Keep unknown
/// data as octet-stream while recognizing common native-player formats.
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
    rewrite_hls_manifest_inner(bytes, local_bytes_base, false, false)
}

/// Rewrite an unindexed feed snapshot for a player that must keep polling.
///
/// Some archived stream producers emitted `PLAYLIST-TYPE:VOD` before the
/// owner-authenticated ENDLIST update was published. Serving that intermediate
/// declaration unchanged can make an HLS client treat a provisional snapshot
/// as immutable. A historical snapshot may also contain ENDLIST even though a
/// newer feed update exists, so only expose ENDLIST after the route has
/// confirmed that snapshot as the feed head. Keep the unindexed representation
/// EVENT when the producer declared VOD so its playlist type never changes
/// across reloads. Provisional snapshots explicitly start at the first segment
/// they contain, including sliding windows, while the feed frontier is still
/// being discovered.
pub(crate) fn rewrite_hls_manifest_for_live_reload(
    bytes: &[u8],
    local_bytes_base: &str,
    head_finalized: bool,
) -> Option<Vec<u8>> {
    rewrite_hls_manifest_inner(bytes, local_bytes_base, true, head_finalized)
}

fn rewrite_hls_manifest_inner(
    bytes: &[u8],
    local_bytes_base: &str,
    normalize_unindexed_feed: bool,
    head_finalized: bool,
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
    let force_archived_start = rewrite_vod_as_event || provisional_unindexed_feed;
    let has_start_tag = force_archived_start
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
        } else if force_archived_start && hls_start_tag_line(line) {
            if wrote_forced_start {
                None
            } else {
                wrote_forced_start = true;
                Some(Cow::Borrowed("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES"))
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
            rewrote_playlist_type || (!rewrite_vod_as_event && line.trim() == HLS_HEADER);
        if force_archived_start && insert_start_after_line && !has_start_tag && !wrote_forced_start
        {
            output.push('\n');
            output.push_str("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES");
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

/// Remove delivery capabilities that the Swarm feed bridge cannot honor.
///
/// In particular, advertising blocking reload or delta-update support causes
/// LL-HLS clients to send `_HLS_msn`, `_HLS_part`, and `_HLS_skip` requests.
/// The bridge currently serves whole feed snapshots, so keep latency-related
/// attributes such as HOLD-BACK while suppressing those unsupported promises.
/// If no attributes remain, omit the tag instead of emitting an invalid empty
/// attribute list.
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

/// Locate quoted values of attributes named exactly `URI`. Parsing the
/// comma-separated attribute list (instead of searching for `URI="`) avoids
/// rewriting `X-URI`, `NOTURI`, or text embedded inside another quoted value.
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
    // Whitespace inside a quoted URI attribute is part of its value. Do not
    // silently normalize it into a different, locally routed resource.
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

    // Only Bee's plural `/bytes/<reference>` path is safe to reinterpret.
    // In particular, do not turn an arbitrary URL that merely ends in a hash,
    // a `/bzz` route, or a backslash-obfuscated path into a local request.
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

fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let parsed = match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(number)) => number.as_f64(),
        Some(serde_json::Value::String(value)) => value.trim().parse::<f64>().ok(),
        Some(_) => None,
    }
    .filter(|value| value.is_finite() && *value >= 0.0);

    Ok(parsed)
}

const MAX_TOPIC_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StreamShareNetwork {
    Mainnet,
    Testnet,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StreamShareRoute {
    pub network: StreamShareNetwork,
    pub owner: String,
    pub topic: String,
    pub index: Option<u64>,
}

impl StreamShareRoute {
    pub(crate) fn new(
        network: StreamShareNetwork,
        owner: impl Into<String>,
        topic: impl Into<String>,
        index: Option<u64>,
    ) -> Result<Self, String> {
        let mut route = Self {
            network,
            owner: owner.into(),
            topic: topic.into(),
            index,
        };
        validate_route(&route)?;
        route.owner = route
            .owner
            .strip_prefix("0x")
            .or_else(|| route.owner.strip_prefix("0X"))
            .unwrap_or(&route.owner)
            .to_ascii_lowercase();
        Ok(route)
    }
}

pub(crate) fn stream_share_path(
    route_base: &str,
    route: &StreamShareRoute,
) -> Result<String, String> {
    let route_base = normalize_route_base(route_base)?;
    validate_route(route)?;

    let topic = encode_path_segment(&route.topic);
    let mut path = match route.network {
        StreamShareNetwork::Mainnet => {
            format!("{route_base}/stream/{}/{topic}", route.owner)
        }
        StreamShareNetwork::Testnet => {
            format!("{route_base}/testnet/stream/{}/{topic}", route.owner)
        }
    };
    if let Some(index) = route.index {
        path.push('/');
        path.push_str(&index.to_string());
    }
    Ok(path)
}

pub(crate) fn stream_share_url(
    origin: &str,
    route_base: &str,
    route: &StreamShareRoute,
) -> Result<String, String> {
    let origin = normalize_http_origin(origin)?;
    Ok(format!("{origin}{}", stream_share_path(route_base, route)?))
}

pub(crate) fn parse_stream_share_link(
    input: &str,
    route_base: &str,
) -> Result<StreamShareRoute, String> {
    let route_base = normalize_route_base(route_base)?;
    let path = share_path_from_input(input)?;
    let route_path = if route_base.is_empty() {
        path
    } else {
        path.strip_prefix(&route_base)
            .and_then(|tail| tail.strip_prefix('/'))
            .ok_or_else(|| "stream share route is outside the configured route base".to_string())?
    };
    let route_path = route_path.trim_start_matches('/');
    let parts: Vec<&str> = route_path.split('/').collect();
    if parts.iter().any(|part| part.is_empty()) {
        return Err("stream share route has an invalid path shape".into());
    }

    let (network, owner_offset) = match parts.as_slice() {
        [kind, ..] if *kind == "stream" && matches!(parts.len(), 3 | 4) => {
            (StreamShareNetwork::Mainnet, 1)
        }
        [network, kind, ..]
            if *network == "testnet" && *kind == "stream" && matches!(parts.len(), 4 | 5) =>
        {
            (StreamShareNetwork::Testnet, 2)
        }
        _ => return Err("stream share route has an invalid path shape".into()),
    };
    let owner = parts[owner_offset].to_string();
    let topic = decode_path_segment(parts[owner_offset + 1])?;
    let index = if let Some(index) = parts.get(owner_offset + 2) {
        if !index.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("stream share index must be an unsigned decimal integer".into());
        }
        Some(
            index
                .parse::<u64>()
                .map_err(|_| "stream share index is out of range".to_string())?,
        )
    } else {
        None
    };

    StreamShareRoute::new(network, owner, topic, index)
}

fn validate_route(route: &StreamShareRoute) -> Result<(), String> {
    if route.owner.trim() != route.owner {
        return Err("stream share owner must not contain surrounding whitespace".into());
    }
    let owner_hex = route
        .owner
        .strip_prefix("0x")
        .or_else(|| route.owner.strip_prefix("0X"))
        .unwrap_or(&route.owner);
    if owner_hex.len() != 40 || !owner_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("stream share owner must be a 20-byte hexadecimal address".into());
    }

    if route.topic.is_empty() || route.topic.len() > MAX_TOPIC_BYTES {
        return Err(format!(
            "stream share topic must contain 1 to {MAX_TOPIC_BYTES} UTF-8 bytes"
        ));
    }
    if route.topic.chars().any(char::is_control) {
        return Err("stream share topic must not contain control characters".into());
    }
    // URL implementations normalize literal and percent-encoded dot segments.
    // There is no portable path-segment representation for these two values.
    if matches!(route.topic.as_str(), "." | "..") {
        return Err("stream share topic cannot be a URL dot segment".into());
    }
    Ok(())
}

fn encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

fn decode_path_segment(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            let high = bytes
                .get(cursor + 1)
                .and_then(|byte| hex_value(*byte))
                .ok_or_else(|| "stream share topic has an invalid percent escape".to_string())?;
            let low = bytes
                .get(cursor + 2)
                .and_then(|byte| hex_value(*byte))
                .ok_or_else(|| "stream share topic has an invalid percent escape".to_string())?;
            decoded.push((high << 4) | low);
            cursor += 3;
        } else {
            decoded.push(bytes[cursor]);
            cursor += 1;
        }
    }

    String::from_utf8(decoded)
        .map_err(|_| "stream share topic percent escapes are not valid UTF-8".to_string())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn share_path_from_input(input: &str) -> Result<&str, String> {
    let input = input.trim();
    if input.is_empty() || input.chars().any(char::is_control) {
        return Err("stream share link must be non-empty and contain no control characters".into());
    }
    if input.contains(['?', '#', '\\']) {
        return Err("stream share link must be a clean path without query or fragment".into());
    }

    if input.starts_with('/') {
        if input.starts_with("//") {
            return Err("scheme-relative stream share links are not supported".into());
        }
        return Ok(input);
    }

    let scheme_end = input
        .find("://")
        .ok_or_else(|| "stream share link must be absolute or root-relative".to_string())?;
    let scheme = &input[..scheme_end];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err("stream share link must use HTTP or HTTPS".into());
    }
    let remainder = &input[scheme_end + 3..];
    let path_start = remainder.find('/').unwrap_or(remainder.len());
    validate_authority(&remainder[..path_start])?;
    if path_start == remainder.len() {
        Ok("/")
    } else {
        Ok(&remainder[path_start..])
    }
}

fn normalize_http_origin(origin: &str) -> Result<String, String> {
    let origin = origin.trim();
    if origin.is_empty()
        || origin.chars().any(char::is_control)
        || origin.contains(['?', '#', '\\'])
    {
        return Err("stream share origin must be a clean HTTP(S) origin".into());
    }

    let scheme_end = origin
        .find("://")
        .ok_or_else(|| "stream share origin must use HTTP or HTTPS".to_string())?;
    let scheme = &origin[..scheme_end];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err("stream share origin must use HTTP or HTTPS".into());
    }

    let remainder = &origin[scheme_end + 3..];
    let authority = remainder.strip_suffix('/').unwrap_or(remainder);
    if authority.contains('/') {
        return Err("stream share origin must not contain a path".into());
    }
    validate_authority(authority)?;
    Ok(format!("{}://{authority}", scheme.to_ascii_lowercase()))
}

fn validate_authority(authority: &str) -> Result<(), String> {
    if authority.is_empty() || authority.contains('@') || authority.chars().any(char::is_whitespace)
    {
        return Err("stream share URL has an invalid authority".into());
    }

    if let Some(ipv6) = authority.strip_prefix('[') {
        let close = ipv6
            .find(']')
            .ok_or_else(|| "stream share URL has an invalid IPv6 authority".to_string())?;
        if close == 0 || ipv6[..close].parse::<Ipv6Addr>().is_err() {
            return Err("stream share URL has an invalid IPv6 authority".into());
        }
        let suffix = &ipv6[close + 1..];
        if !suffix.is_empty() {
            validate_port(
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| "stream share URL has an invalid authority".to_string())?,
            )?;
        }
        return Ok(());
    }

    if authority.matches(':').count() > 1 {
        return Err("IPv6 stream share origins must use brackets".into());
    }
    let (host, port) = authority
        .rsplit_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    validate_host(host)?;
    if let Some(port) = port {
        validate_port(port)?;
    }
    Ok(())
}

fn validate_host(host: &str) -> Result<(), String> {
    if host.is_empty() || !host.is_ascii() || host.len() > 253 || host.contains(['[', ']']) {
        return Err("stream share URL has an invalid host".into());
    }
    if host.parse::<Ipv4Addr>().is_ok() {
        return Ok(());
    }
    if host
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return Err("stream share URL has an invalid IPv4 address".into());
    }

    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty()
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || !label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                || !label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
    {
        return Err("stream share URL has an invalid host".into());
    }
    Ok(())
}

fn validate_port(port: &str) -> Result<(), String> {
    if port.is_empty()
        || !port.bytes().all(|byte| byte.is_ascii_digit())
        || port.parse::<u16>().is_err()
    {
        return Err("stream share URL has an invalid port".into());
    }
    Ok(())
}

pub(crate) const FEED_FOLLOWUP_BATCH_LIMIT: usize = 4;
pub(crate) const HLS_INITIAL_EXACT_CATCHUP_LIMIT: usize = 32;
pub(crate) const HLS_INITIAL_EXACT_BETWEEN_RECHECKS: usize = 1;
pub(crate) const HLS_INITIAL_BOUNDED_RECHECK_LIMIT: usize = 2;
pub(crate) const HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT: usize = 64;
pub(crate) const HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL: usize = 4;
/// One peer is enough to begin retrieval, but not enough to treat a negative
/// lookup as proof that a sequence feed has no higher update. Eight priced
/// peers match Bee's sequence-finder probe width without waiting for the
/// separate 200-connection population target.
pub(crate) const HLS_TERMINAL_CONFIRMATION_MIN_PRICED_PEERS: u64 = 8;
const FEED_DORMANT_REFRESH_MS: f64 = 15_000.0;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum FeedFollowupMode {
    Canonical,
    SequenceZeroPresentation,
}

/// Decide whether an HLS snapshot may be represented as terminal.
///
/// ENDLIST is authoritative for an explicitly pinned immutable playlist. On
/// the mutable unindexed route it is terminal only after the reliable sequence
/// finder confirms that the same update is the current feed head.
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

pub(crate) fn cached_feed_should_refresh_head(last_touch_ms: f64, now_ms: f64) -> bool {
    last_touch_ms.is_finite()
        && now_ms.is_finite()
        && now_ms >= last_touch_ms
        && now_ms - last_touch_ms >= FEED_DORMANT_REFRESH_MS
}

pub(crate) fn exact_feed_batch_should_refresh_head(successes: usize) -> bool {
    successes >= FEED_FOLLOWUP_BATCH_LIMIT
}

pub(crate) fn feed_followup_batch_limit(mode: FeedFollowupMode) -> usize {
    match mode {
        FeedFollowupMode::Canonical => FEED_FOLLOWUP_BATCH_LIMIT,
        FeedFollowupMode::SequenceZeroPresentation => HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT,
    }
}

pub(crate) fn feed_followup_max_parallel(mode: FeedFollowupMode) -> usize {
    match mode {
        FeedFollowupMode::Canonical => 1,
        FeedFollowupMode::SequenceZeroPresentation => HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL,
    }
}

pub(crate) fn feed_followup_should_refresh_head(
    mode: FeedFollowupMode,
    successes: usize,
    saw_tentative_endlist: bool,
) -> bool {
    saw_tentative_endlist
        || (mode == FeedFollowupMode::Canonical && exact_feed_batch_should_refresh_head(successes))
}

/// Bound contiguous exact reads before the next initial head recheck.
///
/// The first authenticated startup candidate can be far behind even though it
/// is immediately useful for playback. One exact adjacency read validates
/// continuity, then a second bounded Bee frontier wave can jump near the head.
/// Once both bounded waves have run, retain the existing 32-update sequential
/// fallback for sparse or temporarily inconsistent peer views.
pub(crate) fn hls_initial_exact_round_limit(
    bounded_rechecks: usize,
    exact_updates: usize,
) -> usize {
    let remaining = HLS_INITIAL_EXACT_CATCHUP_LIMIT.saturating_sub(exact_updates);
    if bounded_rechecks < HLS_INITIAL_BOUNDED_RECHECK_LIMIT {
        remaining.min(HLS_INITIAL_EXACT_BETWEEN_RECHECKS)
    } else {
        remaining
    }
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

/// Remember exact indices advertised by authenticated stream-catalog feeds.
///
/// The catalog assertion is only a hint. The unindexed read path validates
/// the owner-signed exact SOC and confirms the current feed head before
/// treating an ENDLIST as terminal.
#[cfg(target_arch = "wasm32")]
pub(crate) fn remember_catalog_vod_indices(
    network_id: u64,
    entries: impl IntoIterator<Item = (String, String, u64)>,
) {
    let touched_ms = now_ms();
    let mut hints = read_storage();
    for (owner, normalized_topic, index) in entries.into_iter().take(MAX_HINTS) {
        upsert(
            &mut hints,
            network_id,
            &owner,
            &normalized_topic,
            index,
            touched_ms,
        );
    }
    write_storage(hints);
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn remember_catalog_vod_indices(
    _network_id: u64,
    _entries: impl IntoIterator<Item = (String, String, u64)>,
) {
}

/// Remember an exact feed index only after its owner-authenticated payload was
/// confirmed to be the current feed head and a finalized HLS manifest.
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn remember_authenticated_endlist_index(
    _network_id: u64,
    _owner: &str,
    _normalized_topic: &str,
    _index: u64,
) {
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn persisted_vod_index(
    _network_id: u64,
    _owner: &str,
    _normalized_topic: &str,
) -> Option<u64> {
    None
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn forget_vod_index(_network_id: u64, _owner: &str, _normalized_topic: &str) {}

#[cfg(test)]
mod stream_index_hint_tests {
    use super::*;

    const OWNER: &str = "6F2728386F8a47ef5EBe323721188e630Ff0FdE9";
    const TOPIC: &str = "e440540de2dc0ce0112f27889b168994deba607a21e25ca57b7ea37c430472cf";

    #[test]
    fn identities_are_network_scoped_and_canonical() {
        assert_eq!(
            canonical_identity(1, OWNER, TOPIC),
            Some((1, OWNER.to_ascii_lowercase(), TOPIC.to_string()))
        );
        assert!(canonical_identity(1, "not-an-owner", TOPIC).is_none());
        assert!(canonical_identity(1, OWNER, "uuid-is-not-normalized").is_none());
    }

    #[test]
    fn newer_catalog_snapshots_cannot_downgrade_a_vod_hint() {
        let mut hints = Vec::new();
        upsert(&mut hints, 1, OWNER, TOPIC, 646, 10);
        upsert(&mut hints, 1, OWNER, TOPIC, 69, 20);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].index, 646);
        assert_eq!(hints[0].touched_ms, 20);
    }

    #[test]
    fn sanitizing_deduplicates_and_bounds_persisted_catalog_data() {
        let hints = (0..(MAX_HINTS + 20))
            .map(|index| VodIndexHint {
                network_id: 1,
                owner: OWNER.to_string(),
                topic: format!("{index:064x}"),
                index: index as u64,
                touched_ms: index as u64,
            })
            .collect();
        let sanitized = sanitize(hints);
        assert_eq!(sanitized.len(), MAX_HINTS);
        assert_eq!(sanitized[0].index, (MAX_HINTS + 19) as u64);
    }

    #[test]
    fn serialized_hint_store_has_a_hard_byte_limit() {
        let hints = (0..MAX_HINTS)
            .map(|index| VodIndexHint {
                network_id: 1,
                owner: OWNER.to_ascii_lowercase(),
                topic: format!("{index:064x}"),
                index: index as u64,
                touched_ms: index as u64,
            })
            .collect();
        let serialized = compact_for_storage(hints).unwrap();
        assert!(serialized.len() <= MAX_SERIALIZED_BYTES);
    }
}

#[cfg(target_arch = "wasm32")]
mod player {
    //! Rust-owned HLS browser lifecycle.
    //!
    //! `hls.js` remains the playback engine on MSE browsers, but all application
    //! policy and session ownership live here. A tiny static loader supplies the
    //! one browser primitive Wasm cannot call directly: dynamic `import()`.

    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        time::Duration,
    };

    use async_std::task::sleep;
    use js_sys::{Array, Error, Function, Object, Promise, Reflect};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{CustomEvent, CustomEventInit, Element, Event, HtmlMediaElement};

    use super::{HLS_LIVE_SYNC_DURATION_COUNT, classify_hls_level_transition};

    const SWARM_REQUEST_TIMEOUT_MS: f64 = 240_000.0;
    const MAX_NETWORK_RECOVERY_ATTEMPTS: u8 = 2;
    const MAX_HARD_RESTART_ATTEMPTS: u8 = 2;

    const HLS_ERROR_EVENT: &str = "hlsError";
    const HLS_FRAGMENT_BUFFERED_EVENT: &str = "hlsFragBuffered";
    const HLS_LEVEL_LOADED_EVENT: &str = "hlsLevelLoaded";
    const HLS_MANIFEST_PARSED_EVENT: &str = "hlsManifestParsed";
    const HLS_NETWORK_ERROR: &str = "networkError";
    const HLS_MEDIA_ERROR: &str = "mediaError";
    pub(crate) const HLS_AUTOPLAY_AUTHORIZED_EVENT: &str = "weeb3-hls-autoplay-authorized";
    pub(crate) const HLS_EXPLICIT_PAUSE_EVENT: &str = "weeb3-hls-explicit-pause";
    pub(crate) const HLS_TIMELINE_REBASE_EVENT: &str = "weeb3-hls-timeline-rebase";
    pub(crate) const HLS_WARMUP_START_EVENT: &str = "weeb3-hls-warmup-start";
    pub(crate) const HLS_AUTOPLAY_PENDING_ATTRIBUTE: &str = "data-weeb3-hls-autoplay-pending";
    pub(crate) const HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE: &str = "data-weeb3-hls-playback-authorized";

    #[wasm_bindgen(module = "/static/hls_loader.js")]
    extern "C" {
        #[wasm_bindgen(js_name = loadHls)]
        fn load_hls() -> Promise;
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
        static NEXT_SESSION_ID: Cell<u64> = const { Cell::new(0) };
        static ACTIVE_PLAYER_REQUEST: Cell<u64> = const { Cell::new(0) };
        static CURRENT_SESSION: RefCell<Option<HlsSession>> = const { RefCell::new(None) };
    }

    enum PlayerMode {
        Hls(Hls),
        Native,
    }

    struct DomCallback {
        event_name: &'static str,
        callback: Closure<dyn FnMut(Event)>,
    }

    struct HlsCallback {
        event_name: &'static str,
        callback: Closure<dyn FnMut(JsValue, JsValue)>,
    }

    struct HlsSession {
        id: u64,
        media: HtmlMediaElement,
        source: String,
        mode: PlayerMode,
        dom_callbacks: Vec<DomCallback>,
        hls_callbacks: Vec<HlsCallback>,
        autoplay_pending: bool,
        hard_restart_attempts: u8,
        level_snapshots: HashMap<u64, (u64, bool)>,
        load_started: bool,
        manifest_parsed: bool,
        media_recovery_attempts: u8,
        network_recovery_attempts: u8,
        playback_authorized: bool,
        recovery_pending: bool,
        resume_authorized_playback: bool,
        timeline_rebase_attempted: bool,
        warmup_active: bool,
    }

    impl HlsSession {
        fn hls(&self) -> Option<&Hls> {
            match &self.mode {
                PlayerMode::Hls(hls) => Some(hls),
                PlayerMode::Native => None,
            }
        }

        fn dispose(mut self) {
            quarantine_dom_callbacks(&self.media, &mut self.dom_callbacks);

            match &self.mode {
                PlayerMode::Hls(hls) => {
                    // hls.js also drops its own event subscriptions and detaches MSE.
                    // This does not abort any request already handed to the
                    // Service Worker/Rust accounting path.
                    destroy_hls_and_quarantine_callbacks(hls, &mut self.hls_callbacks);
                }
                PlayerMode::Native => {
                    // Prevent only future native playlist requests. Requests that
                    // already crossed the worker bridge continue settling in Rust.
                    let _ = self.media.pause();
                    self.media.remove_attribute("src").ok();
                    self.media.load();
                }
            }
        }
    }

    fn next_session_id() -> u64 {
        NEXT_SESSION_ID.with(|sequence| {
            let mut next = sequence.get().wrapping_add(1);
            if next == 0 {
                next = 1;
            }
            sequence.set(next);
            next
        })
    }

    fn with_session_mut<T>(
        session_id: u64,
        action: impl FnOnce(&mut HlsSession) -> T,
    ) -> Option<T> {
        CURRENT_SESSION.with(|current| {
            let Ok(mut current) = current.try_borrow_mut() else {
                return None;
            };
            let session = current.as_mut()?;
            if session.id != session_id {
                return None;
            }
            Some(action(session))
        })
    }

    fn session_is_current(session_id: u64) -> bool {
        CURRENT_SESSION.with(|current| {
            current
                .try_borrow()
                .ok()
                .and_then(|current| current.as_ref().map(|session| session.id == session_id))
                .unwrap_or(false)
        })
    }

    fn dispose_current_session() {
        if let Some(session) = CURRENT_SESSION.with(|current| current.borrow_mut().take()) {
            session.dispose();
        }
    }

    fn begin_player_request() -> u64 {
        let session_id = next_session_id();
        ACTIVE_PLAYER_REQUEST.with(|active| active.set(session_id));
        dispose_current_session();
        session_id
    }

    fn player_request_is_current(session_id: u64) -> bool {
        ACTIVE_PLAYER_REQUEST.with(|active| active.get() == session_id)
    }

    pub(crate) fn destroy_current_hls() {
        ACTIVE_PLAYER_REQUEST.with(|active| active.set(next_session_id()));
        dispose_current_session();
    }

    pub(crate) async fn play_hls(player: &Element, source: &str) -> Result<&'static str, JsValue> {
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
        start_hls_request(media, source.to_string(), 0, false, false).await
    }

    async fn start_hls_request(
        media: HtmlMediaElement,
        source: String,
        hard_restart_attempts: u8,
        timeline_rebase_attempted: bool,
        resume_authorized_playback: bool,
    ) -> Result<&'static str, JsValue> {
        let session_id = begin_player_request();
        media.remove_attribute("data-weeb3-hls-mode").ok();
        media.remove_attribute("data-weeb3-hls-state").ok();
        media.remove_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE).ok();
        media
            .remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
            .ok();

        let native_supported = supports_native_hls(&media);
        let hls_class = match JsFuture::from(load_hls()).await {
            Ok(hls_class) => Some(hls_class),
            Err(error) => {
                if !player_request_is_current(session_id) {
                    return Ok("superseded");
                }
                if !native_supported {
                    return Err(error);
                }
                None
            }
        };
        if !player_request_is_current(session_id) {
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
            attach_native_hls(
                media,
                source,
                session_id,
                hard_restart_attempts,
                timeline_rebase_attempted,
            )?;
            if resume_authorized_playback {
                resume_authorized_hls_playback(session_id);
            }
            return Ok("native");
        }

        media.set_attribute("data-weeb3-hls-mode", "hls.js")?;
        media.set_attribute("data-weeb3-hls-state", "loading-manifest")?;
        let config = hls_config();
        let hls = construct_hls(
            hls_class
                .as_ref()
                .expect("an MSE-capable load must retain the hls.js class"),
            &config,
        )?;
        let mut hls_callbacks = register_hls_callbacks(&hls, session_id)?;
        let dom_callbacks = match register_hls_dom_callbacks(&media, session_id) {
            Ok(callbacks) => callbacks,
            Err(error) => {
                destroy_hls_and_quarantine_callbacks(&hls, &mut hls_callbacks);
                return Err(error);
            }
        };

        CURRENT_SESSION.with(|current| {
            *current.borrow_mut() = Some(HlsSession {
                id: session_id,
                media: media.clone(),
                source: source.clone(),
                mode: PlayerMode::Hls(hls),
                dom_callbacks,
                hls_callbacks,
                autoplay_pending: false,
                hard_restart_attempts,
                level_snapshots: HashMap::new(),
                load_started: false,
                manifest_parsed: false,
                media_recovery_attempts: 0,
                network_recovery_attempts: 0,
                playback_authorized: false,
                recovery_pending: false,
                resume_authorized_playback,
                timeline_rebase_attempted,
                warmup_active: false,
            });
        });

        let attached = with_session_mut(session_id, |session| session.hls().cloned())
            .flatten()
            .ok_or_else(|| JsValue::from_str("HLS session was superseded"))
            .and_then(|hls| {
                hls.load_source(&source)?;
                hls.attach_media(&media)
            });
        if let Err(error) = attached {
            if session_is_current(session_id) {
                destroy_current_hls();
            }
            return Err(error);
        }

        Ok("hls.js")
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

    fn attach_native_hls(
        media: HtmlMediaElement,
        source: String,
        session_id: u64,
        hard_restart_attempts: u8,
        timeline_rebase_attempted: bool,
    ) -> Result<(), JsValue> {
        media.set_attribute("data-weeb3-hls-mode", "native")?;
        let mut dom_callbacks = Vec::with_capacity(2);
        let callback = Closure::<dyn FnMut(Event)>::new(move |_| {
            let Some((media, message)) = with_session_mut(session_id, |session| {
                let message = session
                    .media
                    .error()
                    .map(|error| error.message())
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| "native HLS media error".to_string());
                (session.media.clone(), message)
            }) else {
                return;
            };
            report_playback_error(&media, &message, &JsValue::from_str(&message));
        });
        register_dom_callback(&media, "error", callback, &mut dom_callbacks)?;

        let manifest_ready = Closure::<dyn FnMut(Event)>::new(move |_| {
            let Some(media) = with_session_mut(session_id, |session| session.media.clone()) else {
                return;
            };
            dispatch_custom_event(&media, "weeb3-hls-manifest-ready", &JsValue::UNDEFINED);
        });
        register_dom_callback(&media, "loadedmetadata", manifest_ready, &mut dom_callbacks)?;

        CURRENT_SESSION.with(|current| {
            *current.borrow_mut() = Some(HlsSession {
                id: session_id,
                media: media.clone(),
                source: source.clone(),
                mode: PlayerMode::Native,
                dom_callbacks,
                hls_callbacks: Vec::new(),
                autoplay_pending: false,
                hard_restart_attempts,
                level_snapshots: HashMap::new(),
                load_started: true,
                manifest_parsed: true,
                media_recovery_attempts: 0,
                network_recovery_attempts: 0,
                playback_authorized: false,
                recovery_pending: false,
                resume_authorized_playback: false,
                timeline_rebase_attempted,
                warmup_active: false,
            });
        });

        media.set_src(&source);
        media.load();
        maybe_autoplay(session_id);
        Ok(())
    }

    fn supports_native_hls(media: &HtmlMediaElement) -> bool {
        ["application/vnd.apple.mpegurl", "application/x-mpegURL"]
            .iter()
            .any(|mime| matches!(media.can_play_type(mime).as_str(), "probably" | "maybe"))
    }

    fn register_hls_callbacks(hls: &Hls, session_id: u64) -> Result<Vec<HlsCallback>, JsValue> {
        let mut retained = Vec::with_capacity(4);

        let error = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |_, data| {
            handle_hls_error(session_id, data);
        });
        register_hls_callback(hls, HLS_ERROR_EVENT, error, &mut retained)?;

        let fragment_buffered = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |_, _| {
            let action = with_session_mut(session_id, |session| {
                session.network_recovery_attempts = 0;
                session.media_recovery_attempts = 0;
                let stop_warmup = session.warmup_active && session.media.paused();
                if stop_warmup {
                    session.warmup_active = false;
                }
                (
                    session.media.clone(),
                    stop_warmup.then(|| session.hls().cloned()).flatten(),
                )
            });
            if let Some((media, warmup_hls)) = action {
                media.set_attribute("data-weeb3-hls-state", "buffered").ok();
                set_playback_status(&media, "HLS media buffered through weeb-3.", "buffered");
                if let Some(hls) = warmup_hls
                    && let Err(error) = hls.stop_load()
                {
                    stop_with_error(session_id, "Could not stop bounded HLS warm-up", error);
                }
            }
        });
        register_hls_callback(
            hls,
            HLS_FRAGMENT_BUFFERED_EVENT,
            fragment_buffered,
            &mut retained,
        )?;

        let level_loaded = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |_, data| {
            handle_hls_level_loaded(session_id, &data);
        });
        register_hls_callback(hls, HLS_LEVEL_LOADED_EVENT, level_loaded, &mut retained)?;

        let manifest_parsed = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |_, _| {
            let ready = with_session_mut(session_id, |session| {
                session.manifest_parsed = true;
                let resume_authorized = session.resume_authorized_playback;
                let startup_hls =
                    if !session.load_started && (resume_authorized || session.media.paused()) {
                        session.load_started = true;
                        session.warmup_active = true;
                        session.hls().cloned().map(|hls| (hls, resume_authorized))
                    } else {
                        None
                    };
                (session.media.clone(), startup_hls)
            });
            if let Some((media, startup_hls)) = ready {
                media
                    .set_attribute("data-weeb3-hls-state", "manifest-ready")
                    .ok();
                set_playback_status(
                    &media,
                    "HLS manifest ready through weeb-3. Press Play if autoplay is blocked.",
                    "manifest-ready",
                );
                dispatch_custom_event(&media, "weeb3-hls-manifest-ready", &JsValue::UNDEFINED);
                if let Some((hls, resume_authorized)) = startup_hls {
                    dispatch_custom_event(&media, HLS_WARMUP_START_EVENT, &JsValue::UNDEFINED);
                    if !session_is_current(session_id) {
                        return;
                    }
                    if let Err(error) = hls.start_load_at(-1.0) {
                        schedule_hard_restart(session_id, error);
                        return;
                    }
                    if resume_authorized {
                        resume_authorized_hls_playback(session_id);
                    }
                }
                maybe_autoplay(session_id);
            }
        });
        register_hls_callback(
            hls,
            HLS_MANIFEST_PARSED_EVENT,
            manifest_parsed,
            &mut retained,
        )?;

        Ok(retained)
    }

    fn handle_hls_level_loaded(session_id: u64, data: &JsValue) {
        let Some(level) = js_safe_u64_property(data, "level") else {
            return;
        };
        let Some(details) = js_property(data, "details") else {
            return;
        };
        let Some(start_sequence) = js_safe_u64_property(&details, "startSN") else {
            return;
        };
        let Some(live) = js_bool_property(&details, "live") else {
            return;
        };

        let outcome = with_session_mut(session_id, |session| {
            // Media sequence numbers are local to a rendition. Comparing snapshots
            // from different ABR levels could mistake an ordinary quality switch
            // for an archive expansion.
            let previous = session
                .level_snapshots
                .insert(level, (start_sequence, live));
            let transition = classify_hls_level_transition(
                previous,
                session.timeline_rebase_attempted,
                session.recovery_pending,
                start_sequence,
                live,
            );
            let terminal_media = transition.terminal_ready.then(|| session.media.clone());

            if !transition.rebase {
                return (None, terminal_media);
            }

            // This is an expected representation transition, not a playback
            // failure. Keep its one-shot guard separate from the outer error
            // restart budget and remember whether a prior user gesture authorized
            // playback so the rebuilt MediaSource can resume. It also supersedes
            // any recovery already scheduled for this old session; that timer is
            // session-id guarded and cannot affect the replacement.
            session.timeline_rebase_attempted = true;
            session.recovery_pending = true;
            (
                Some((
                    session.media.clone(),
                    session.source.clone(),
                    session.hard_restart_attempts,
                    session.playback_authorized,
                    session.hls().cloned(),
                )),
                terminal_media,
            )
        });
        let Some((restart, terminal_media)) = outcome else {
            return;
        };
        if let Some(media) = terminal_media {
            // MANIFEST_PARSED normally fires once. A later ENDLIST transition is
            // also a manifest-ready state change: notify the Rust route owner so
            // it can pin the authenticated final feed index in the share URL.
            dispatch_custom_event(&media, "weeb3-hls-manifest-ready", &JsValue::UNDEFINED);
        }
        let Some((media, source, hard_restart_attempts, resume_authorized, retiring_hls)) = restart
        else {
            return;
        };

        media
            .set_attribute("data-weeb3-hls-state", "rebasing-timeline")
            .ok();
        set_playback_status(
            &media,
            "Complete HLS archive found; rebuilding its timeline from the beginning.",
            "rebasing-timeline",
        );
        // The Rust prefetch scheduler keeps the immutable-content generation so a
        // sequence-zero body already in flight remains joinable. It only retires
        // future admissions owned by the superseded rolling-window plans.
        dispatch_custom_event(&media, HLS_TIMELINE_REBASE_EVENT, &JsValue::UNDEFINED);
        // Stop the retiring loader synchronously inside LEVEL_LOADED so it cannot
        // dispatch a new same-URL Service Worker flight in the callback-to-destroy
        // gap. A request already bridged into Rust is owned by its detached leader
        // and still drains through retrieval and accounting; stop_load only closes
        // future browser-side admission.
        if let Some(hls) = retiring_hls {
            let _ = hls.stop_load();
        }

        // Let the hls.js LEVEL_LOADED callback stack unwind before dropping the
        // instance that owns it. Destroying that instance prevents only future
        // loader admission; Service Worker requests already dispatched into Rust
        // continue through accounting and settle normally.
        spawn_local(async move {
            let _ = JsFuture::from(Promise::resolve(&JsValue::UNDEFINED)).await;
            if !session_is_current(session_id) {
                return;
            }
            if let Err(error) = start_hls_request(
                media.clone(),
                source,
                hard_restart_attempts,
                true,
                resume_authorized,
            )
            .await
            {
                report_playback_error(
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

    fn register_hls_callback(
        hls: &Hls,
        event_name: &'static str,
        callback: Closure<dyn FnMut(JsValue, JsValue)>,
        retained: &mut Vec<HlsCallback>,
    ) -> Result<(), JsValue> {
        if let Err(error) = hls.on(event_name, callback.as_ref().unchecked_ref::<Function>()) {
            retained.push(HlsCallback {
                event_name,
                callback,
            });
            destroy_hls_and_quarantine_callbacks(hls, retained);
            return Err(error);
        }
        retained.push(HlsCallback {
            event_name,
            callback,
        });
        Ok(())
    }

    fn destroy_hls_and_quarantine_callbacks(hls: &Hls, retained: &mut Vec<HlsCallback>) {
        let mut callbacks_detached = true;
        for registered in retained.iter() {
            callbacks_detached &= hls
                .off(
                    registered.event_name,
                    registered.callback.as_ref().unchecked_ref(),
                )
                .is_ok();
        }
        let destroyed = hls.destroy().is_ok();
        if !callbacks_detached && !destroyed {
            // A broken JS instance may still retain these callbacks. Quarantine
            // them instead of leaving JS with wrappers whose Rust Closure state
            // has been dropped.
            std::mem::forget(std::mem::take(retained));
        } else {
            retained.clear();
        }
    }

    fn register_dom_callback(
        media: &HtmlMediaElement,
        event_name: &'static str,
        callback: Closure<dyn FnMut(Event)>,
        retained: &mut Vec<DomCallback>,
    ) -> Result<(), JsValue> {
        let result =
            media.add_event_listener_with_callback(event_name, callback.as_ref().unchecked_ref());
        retained.push(DomCallback {
            event_name,
            callback,
        });
        if let Err(error) = result {
            quarantine_dom_callbacks(media, retained);
            return Err(error);
        }
        Ok(())
    }

    fn quarantine_dom_callbacks(media: &HtmlMediaElement, retained: &mut Vec<DomCallback>) {
        let mut quarantined = Vec::new();
        for registered in std::mem::take(retained) {
            if media
                .remove_event_listener_with_callback(
                    registered.event_name,
                    registered.callback.as_ref().unchecked_ref(),
                )
                .is_err()
            {
                quarantined.push(registered);
            }
        }
        if !quarantined.is_empty() {
            // A DOM target that refused to detach a listener may still call it.
            // Keep only those live wasm-bindgen closures quarantined.
            std::mem::forget(quarantined);
        }
    }

    fn register_hls_dom_callbacks(
        media: &HtmlMediaElement,
        session_id: u64,
    ) -> Result<Vec<DomCallback>, JsValue> {
        let mut retained = Vec::with_capacity(2);
        let play = Closure::<dyn FnMut(Event)>::new(move |_| {
            let action = with_session_mut(session_id, |session| {
                let autoplay_pending = session.autoplay_pending;
                let first_load = !session.load_started;
                session.load_started = true;
                if !autoplay_pending {
                    session.playback_authorized = true;
                    session.warmup_active = false;
                }
                (
                    session.hls().cloned(),
                    first_load,
                    autoplay_pending,
                    session.media.clone(),
                )
            });
            if let Some((hls, first_load, autoplay_pending, media)) = action {
                if !autoplay_pending {
                    media
                        .set_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE, "1")
                        .ok();
                    dispatch_custom_event(
                        &media,
                        HLS_AUTOPLAY_AUTHORIZED_EVENT,
                        &JsValue::UNDEFINED,
                    );
                }
                media
                    .set_attribute("data-weeb3-hls-state", "loading-media")
                    .ok();
                if let Some(hls) = hls {
                    let result = if first_load {
                        hls.start_load_at(-1.0)
                    } else {
                        hls.start_load()
                    };
                    if let Err(error) = result {
                        schedule_hard_restart(session_id, error);
                    }
                }
            }
        });
        register_dom_callback(media, "play", play, &mut retained)?;

        let pause = Closure::<dyn FnMut(Event)>::new(move |_| {
            let action = with_session_mut(session_id, |session| {
                // Chrome may emit `play`/`pause` while rejecting an autoplay
                // promise. Until playback has actually been authorized, that pair
                // is not an explicit user pause and must not stop warm-up or
                // advance the Rust retrieval generation.
                if !session.playback_authorized {
                    return None;
                }
                session.playback_authorized = false;
                session.warmup_active = false;
                Some((session.hls().cloned(), session.media.clone()))
            })
            .flatten();
            if let Some((hls, media)) = action {
                media
                    .remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
                    .ok();
                dispatch_custom_event(&media, HLS_EXPLICIT_PAUSE_EVENT, &JsValue::UNDEFINED);
                if let Some(hls) = hls
                    && let Err(error) = hls.stop_load()
                {
                    stop_with_error(session_id, "Could not pause HLS loading", error);
                }
            }
        });
        register_dom_callback(media, "pause", pause, &mut retained)?;

        Ok(retained)
    }

    fn handle_hls_error(session_id: u64, data: JsValue) {
        let fatal = js_bool_property(&data, "fatal").unwrap_or(false);
        if !fatal {
            let Some(media) = with_session_mut(session_id, |session| session.media.clone()) else {
                return;
            };
            let diagnostic = playback_diagnostic(&media, &data);
            web_sys::console::warn_2(
                &JsValue::from_str("weeb-3 HLS non-fatal event"),
                diagnostic.as_ref(),
            );
            dispatch_custom_event(&media, "weeb3-hls-warning", diagnostic.as_ref());
            return;
        }

        match js_string_property(&data, "type").as_deref() {
            Some(HLS_NETWORK_ERROR) => {
                let manifest_parsed =
                    with_session_mut(session_id, |session| session.manifest_parsed)
                        .unwrap_or(false);
                if !manifest_parsed {
                    schedule_hard_restart(session_id, data);
                    return;
                }

                let next_attempt = with_session_mut(session_id, |session| {
                    if session.network_recovery_attempts >= MAX_NETWORK_RECOVERY_ATTEMPTS {
                        return None;
                    }
                    let attempt = session.network_recovery_attempts;
                    session.network_recovery_attempts =
                        session.network_recovery_attempts.saturating_add(1);
                    Some(attempt)
                })
                .flatten();
                let Some(attempt) = next_attempt else {
                    stop_with_error(session_id, "HLS network recovery limit reached", data);
                    return;
                };
                let delay = 1_000_u64.saturating_mul(1_u64 << u32::from(attempt));
                schedule_network_recovery(session_id, delay.min(30_000));
            }
            Some(HLS_MEDIA_ERROR) => {
                let recovery = with_session_mut(session_id, |session| {
                    if session.media_recovery_attempts == 0 {
                        session.media_recovery_attempts = 1;
                        session.hls().cloned()
                    } else {
                        None
                    }
                })
                .flatten();
                if let Some(hls) = recovery {
                    spawn_local(async move {
                        let _ = JsFuture::from(Promise::resolve(&JsValue::UNDEFINED)).await;
                        if !session_is_current(session_id) {
                            return;
                        }
                        if let Err(error) = hls.recover_media_error() {
                            schedule_hard_restart(session_id, error);
                        }
                    });
                } else {
                    schedule_hard_restart(session_id, data);
                }
            }
            _ => schedule_hard_restart(session_id, data),
        }
    }

    fn schedule_network_recovery(session_id: u64, delay_ms: u64) {
        let scheduled = with_session_mut(session_id, |session| {
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
            sleep(Duration::from_millis(delay_ms)).await;
            let hls = with_session_mut(session_id, |session| {
                session.recovery_pending = false;
                if session.warmup_active || !session.media.paused() {
                    return session.hls().cloned();
                }
                None
            })
            .flatten();
            if let Some(hls) = hls {
                if let Err(error) = hls.start_load_at(-1.0) {
                    schedule_hard_restart(session_id, error);
                }
            }
        });
    }

    fn schedule_hard_restart(session_id: u64, data: JsValue) {
        let restart = with_session_mut(session_id, |session| {
            if session.hard_restart_attempts >= MAX_HARD_RESTART_ATTEMPTS {
                return None;
            }
            if session.recovery_pending {
                return Some(None);
            }
            session.recovery_pending = true;
            Some(Some((
                session.media.clone(),
                session.source.clone(),
                session.hard_restart_attempts.saturating_add(1),
                session.timeline_rebase_attempted,
                session.playback_authorized || session.resume_authorized_playback,
            )))
        })
        .flatten();

        match restart {
            Some(Some((media, source, attempt, timeline_rebase_attempted, resume_authorized))) => {
                spawn_local(async move {
                    sleep(Duration::from_millis(1_000)).await;
                    if !session_is_current(session_id) {
                        return;
                    }
                    if let Err(error) = start_hls_request(
                        media.clone(),
                        source,
                        attempt,
                        timeline_rebase_attempted,
                        resume_authorized,
                    )
                    .await
                    {
                        report_playback_error(
                            &media,
                            &format!(
                                "Could not restart HLS playback: {}",
                                js_error_message(&error)
                            ),
                            &error,
                        );
                    }
                });
            }
            Some(None) => {}
            None => stop_with_error(
                session_id,
                "HLS media remained invalid after two clean restarts",
                data,
            ),
        }
    }

    fn stop_with_error(session_id: u64, message: &str, detail: JsValue) {
        let Some(media) = with_session_mut(session_id, |session| session.media.clone()) else {
            return;
        };
        report_playback_error(&media, message, &detail);

        // The event callback that requested teardown is retained by this session.
        // Let that callback return before dropping its Closure.
        spawn_local(async move {
            let _ = JsFuture::from(Promise::resolve(&JsValue::UNDEFINED)).await;
            if session_is_current(session_id) {
                destroy_current_hls();
            }
        });
    }

    fn maybe_autoplay(session_id: u64) {
        request_programmatic_hls_playback(session_id, true);
    }

    fn resume_authorized_hls_playback(session_id: u64) {
        request_programmatic_hls_playback(session_id, false);
    }

    fn request_programmatic_hls_playback(session_id: u64, require_autoplay: bool) {
        let Some(media) = with_session_mut(session_id, |session| {
            ((!require_autoplay || session.media.autoplay())
                && !session.playback_authorized
                && !session.autoplay_pending)
                .then(|| {
                    // Reuse the pending-play guard for an authorized timeline
                    // resume. It prevents Chrome's provisional play/pause pair
                    // from being mistaken for an explicit user pause.
                    session.autoplay_pending = true;
                    session.media.clone()
                })
        })
        .flatten() else {
            return;
        };
        media
            .set_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE, "1")
            .ok();

        let promise = match media.play() {
            Ok(promise) => promise,
            Err(error) => {
                settle_autoplay(session_id, false);
                report_autoplay_blocked(&media, &error);
                return;
            }
        };
        spawn_local(async move {
            match JsFuture::from(promise).await {
                Ok(_) => {
                    if let Some(media) = settle_autoplay(session_id, true) {
                        dispatch_custom_event(
                            &media,
                            HLS_AUTOPLAY_AUTHORIZED_EVENT,
                            &JsValue::UNDEFINED,
                        );
                    }
                }
                Err(error) => {
                    let Some(media) = settle_autoplay(session_id, false) else {
                        return;
                    };
                    report_autoplay_blocked(&media, &error);
                }
            }
        });
    }

    fn settle_autoplay(session_id: u64, authorized: bool) -> Option<HtmlMediaElement> {
        let media = with_session_mut(session_id, |session| {
            if !session.autoplay_pending {
                return None;
            }
            session.autoplay_pending = false;
            session.resume_authorized_playback = false;
            if authorized {
                session.playback_authorized = true;
                session.warmup_active = false;
            }
            Some(session.media.clone())
        })
        .flatten()?;
        media.remove_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE).ok();
        if authorized {
            media
                .set_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE, "1")
                .ok();
        }
        Some(media)
    }

    fn report_autoplay_blocked(media: &HtmlMediaElement, error: &JsValue) {
        media
            .set_attribute("data-weeb3-hls-state", "autoplay-blocked")
            .ok();
        set_playback_status(
            media,
            "HLS startup media is warming. Autoplay was blocked; press Play to start playback.",
            "autoplay-blocked",
        );
        dispatch_custom_event(media, "weeb3-hls-autoplay-blocked", error);
    }

    fn report_playback_error(media: &HtmlMediaElement, message: &str, detail: &JsValue) {
        let error = Error::new(message);
        let _ = Reflect::set(error.as_ref(), &JsValue::from_str("cause"), detail);
        let error: JsValue = error.into();
        web_sys::console::error_2(&JsValue::from_str("weeb-3 HLS playback error"), &error);
        set_playback_status(media, &format!("HLS playback failed: {message}"), "error");
        dispatch_custom_event(media, "weeb3-hls-error", &error);
    }

    fn set_playback_status(media: &HtmlMediaElement, message: &str, state: &str) {
        let Some(parent) = media.parent_element() else {
            return;
        };
        let Ok(Some(status)) = parent.query_selector(".weeb3-hls-status") else {
            return;
        };
        status.set_text_content(Some(message));
        status.set_attribute("data-state", state).ok();
    }

    fn dispatch_custom_event(media: &HtmlMediaElement, name: &str, detail: &JsValue) {
        let init = CustomEventInit::new();
        init.set_detail(detail);
        if let Ok(event) = CustomEvent::new_with_event_init_dict(name, &init) {
            let _ = media.dispatch_event(&event);
        }
    }

    fn playback_diagnostic(media: &HtmlMediaElement, data: &JsValue) -> Object {
        let diagnostic = Object::new();
        set_property(
            &diagnostic,
            "type",
            js_property(data, "type").unwrap_or(JsValue::UNDEFINED),
        );
        set_property(
            &diagnostic,
            "details",
            js_property(data, "details").unwrap_or(JsValue::UNDEFINED),
        );

        let fragment = js_property(data, "frag");
        set_property(
            &diagnostic,
            "fragmentSequence",
            fragment
                .as_ref()
                .and_then(|fragment| js_property(fragment, "sn"))
                .unwrap_or(JsValue::UNDEFINED),
        );
        set_property(
            &diagnostic,
            "fragmentUrl",
            fragment
                .as_ref()
                .and_then(|fragment| js_property(fragment, "url"))
                .unwrap_or(JsValue::UNDEFINED),
        );
        set_property(
            &diagnostic,
            "currentTime",
            JsValue::from_f64(media.current_time()),
        );

        let buffered = Array::new();
        let ranges = media.buffered();
        for index in 0..ranges.length() {
            let Ok(start) = ranges.start(index) else {
                continue;
            };
            let Ok(end) = ranges.end(index) else {
                continue;
            };
            let range = Array::new();
            range.push(&JsValue::from_f64(start));
            range.push(&JsValue::from_f64(end));
            buffered.push(&range);
        }
        set_property(&diagnostic, "buffered", buffered.into());
        diagnostic
    }

    fn hls_config() -> Object {
        let config = Object::new();
        set_property(&config, "enableWorker", JsValue::TRUE);
        set_property(&config, "autoStartLoad", JsValue::FALSE);
        set_property(&config, "startFragPrefetch", JsValue::FALSE);
        set_property(&config, "progressive", JsValue::FALSE);

        for name in [
            "manifestLoadPolicy",
            "playlistLoadPolicy",
            "fragLoadPolicy",
            "keyLoadPolicy",
        ] {
            set_property(&config, name, swarm_load_policy().into());
        }

        set_property(
            &config,
            "liveSyncDurationCount",
            JsValue::from_f64(HLS_LIVE_SYNC_DURATION_COUNT as f64),
        );
        // A feed-backed archive is indistinguishable from a live playlist until
        // its owner-authenticated ENDLIST update is found. Do not expose the
        // producer's ten-fragment rolling window as a false finite duration.
        // hls.js restores the finite MediaSource duration when ENDLIST arrives.
        set_property(&config, "liveDurationInfinity", JsValue::TRUE);
        // Keep hls.js' infinite maximum-live-latency default. A provisional
        // archived VOD is represented as a reloadable EVENT playlist until its
        // authenticated ENDLIST arrives; a finite maximum would seek that archive
        // from its forced zero start toward the provisional tail. Keep the default
        // 1.0 live-sync playback rate for the same reason: an archive must retain
        // normal timeline speed while its final feed update is found.

        let low_memory = device_memory_gib()
            .filter(|memory| memory.is_finite())
            .is_some_and(|memory| memory <= 2.0);
        set_property(&config, "backBufferLength", JsValue::from_f64(30.0));
        if low_memory {
            set_property(&config, "maxBufferLength", JsValue::from_f64(30.0));
            set_property(&config, "maxMaxBufferLength", JsValue::from_f64(60.0));
            set_property(
                &config,
                "maxBufferSize",
                JsValue::from_f64(32.0 * 1024.0 * 1024.0),
            );
        } else {
            // Unknown memory is common in privacy-oriented browsers and is not
            // evidence of a low-memory device.
            set_property(&config, "maxBufferLength", JsValue::from_f64(90.0));
            set_property(&config, "maxMaxBufferLength", JsValue::from_f64(120.0));
            set_property(
                &config,
                "maxBufferSize",
                JsValue::from_f64(96.0 * 1024.0 * 1024.0),
            );
        }
        set_property(&config, "maxBufferHole", JsValue::from_f64(1.0));
        config
    }

    fn swarm_load_policy() -> Object {
        let retry = Object::new();
        set_property(&retry, "maxNumRetry", JsValue::from_f64(1.0));
        set_property(&retry, "retryDelayMs", JsValue::from_f64(500.0));
        set_property(&retry, "maxRetryDelayMs", JsValue::from_f64(30_000.0));
        set_property(&retry, "backoff", JsValue::from_str("exponential"));

        let defaults = Object::new();
        set_property(
            &defaults,
            "maxTimeToFirstByteMs",
            JsValue::from_f64(SWARM_REQUEST_TIMEOUT_MS + 10_000.0),
        );
        set_property(
            &defaults,
            "maxLoadTimeMs",
            JsValue::from_f64(SWARM_REQUEST_TIMEOUT_MS + 20_000.0),
        );
        set_property(&defaults, "timeoutRetry", JsValue::NULL);
        set_property(&defaults, "errorRetry", retry.into());

        let policy = Object::new();
        set_property(&policy, "default", defaults.into());
        policy
    }

    fn device_memory_gib() -> Option<f64> {
        let navigator = web_sys::window()?.navigator();
        Reflect::get(navigator.as_ref(), &JsValue::from_str("deviceMemory"))
            .ok()?
            .as_f64()
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

    fn js_safe_u64_property(target: &JsValue, name: &str) -> Option<u64> {
        const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
        let value = js_property(target, name)?.as_f64()?;
        (value.is_finite() && value >= 0.0 && value <= MAX_SAFE_INTEGER && value.fract() == 0.0)
            .then_some(value as u64)
    }

    fn js_error_message(error: &JsValue) -> String {
        js_string_property(error, "message")
            .or_else(|| error.as_string())
            .unwrap_or_else(|| "unknown browser error".to_string())
    }
}
#[cfg(target_arch = "wasm32")]
pub(crate) use player::{
    HLS_AUTOPLAY_AUTHORIZED_EVENT, HLS_AUTOPLAY_PENDING_ATTRIBUTE, HLS_EXPLICIT_PAUSE_EVENT,
    HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE, HLS_TIMELINE_REBASE_EVENT, HLS_WARMUP_START_EVENT,
    destroy_current_hls, play_hls,
};

#[cfg(target_arch = "wasm32")]
mod runtime {
    use super::*;
    use super::{
        stream_share_path as build_stream_share_path, stream_share_url as build_stream_share_url,
    };
    use std::{
        cell::RefCell,
        collections::{HashMap, HashSet, VecDeque},
        future::Future,
        rc::Rc,
        time::Duration,
    };

    use async_std::sync::Arc;
    use js_sys::{Function, Promise, Reflect};
    use libp2p::futures::future::{Either, select};
    use libp2p::futures::stream::{self, FuturesUnordered, StreamExt};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::Element;

    use crate::{
        Weeb3,
        bzz_stream::{
            RawFeedPayload, acquire_latest_raw_feed_payload_bounded_from,
            acquire_latest_raw_feed_payload_from,
            acquire_latest_raw_feed_payload_observing_positive, acquire_raw_feed_payload_at_index,
            acquire_raw_feed_payload_at_index_bounded,
        },
        interface::{service_worker_controls_bzz_requests, service_worker_scope_protocol_error},
        mpsc,
        network_profile::{NetworkMode, active_profile},
        normalize_feed_topic, register_retrieve_cancel_token,
        retrieval::{
            retrieve_data_payload, retrieve_data_payload_cancellable, retrieve_data_range_join,
            retrieve_decoded_data_root,
        },
        retrieval_conventions::{
            PendingGenerationRelation, next_nonzero_generation, pending_generation_relation,
        },
        stream::{
            FetchResponse, begin_result_view_request, clear_completed_bzz_media_ranges,
            media_cache_max_bytes, next_media_generation, range_cache_body_bytes,
            release_current_stream_view, replace_stream_result_view,
            result_view_request_is_current, retain_media_element_callback,
        },
        stream_conventions::{
            MEDIA_PREFETCH_BATCH_YIELD_MS, MEDIA_PREFETCH_MAX_PARALLEL,
            MEDIA_STARTUP_RESPONSE_BYTES, MEDIA_STORAGE_WINDOW_BYTES, decode_component,
            if_none_match_matches, if_range_allows_range, media_prefetch_ahead_limit_bytes,
            parse_single_range, plan_media_prefetch_batch, route_markers, streaming_route_base,
            streaming_route_path,
        },
        stream_retrieve_cancel_token,
    };

    impl Weeb3 {
        async fn retrieve_hls_payload(&self, address: String) -> Vec<u8> {
            let progress_id = self
                .start_progress("hls-segment", address.clone(), "retrieve", None, "starting")
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
                format!("{} bytes", bytes.len()),
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
                .start_progress("hls-segment", address.clone(), "retrieve", None, "starting")
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
                format!("{} bytes", bytes.len()),
                ok,
            )
            .await;
            bytes
        }

        async fn publish_hls_generation(&self, stream_key: String, stream_generation: u64) -> bool {
            let Some(cancel) = stream_retrieve_cancel_token(stream_key, stream_generation) else {
                return false;
            };
            register_retrieve_cancel_token(&self.retrieve_cancel_generations, &Some(cancel)).await;
            true
        }

        async fn hls_payload_size(&self, address: String) -> Option<u64> {
            let reference = hex::decode(address).ok()?;
            retrieve_decoded_data_root(&reference, &self.chunk_port.0)
                .await
                .map(|root| root.span)
        }

        async fn retrieve_hls_payload_range(
            &self,
            address: String,
            start: u64,
            end_inclusive: u64,
        ) -> Vec<u8> {
            let progress_id = self
                .start_progress(
                    "hls-segment-range",
                    format!("{} bytes={}-{}", address, start, end_inclusive),
                    "retrieve",
                    None,
                    "starting",
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

            let bytes =
                retrieve_data_range_join(&reference, start, end_inclusive, &self.chunk_port.0)
                    .await;
            let expected = end_inclusive
                .checked_sub(start)
                .and_then(|length| length.checked_add(1))
                .and_then(|length| usize::try_from(length).ok());
            let ok = expected.is_some_and(|expected| bytes.len() == expected);
            self.finish_progress(
                &progress_id,
                if ok { "complete" } else { "failed" },
                format!("{} bytes", bytes.len()),
                ok,
            )
            .await;
            bytes
        }

        async fn latest_hls_feed_payload_bounded_from(
            &self,
            owner: String,
            topic: String,
            initial: RawFeedPayload,
        ) -> Option<RawFeedPayload> {
            acquire_latest_raw_feed_payload_bounded_from(owner, topic, initial, &self.chunk_port.0)
                .await
        }

        async fn latest_hls_feed_payload_from(
            &self,
            owner: String,
            topic: String,
            initial: RawFeedPayload,
        ) -> Option<RawFeedPayload> {
            acquire_latest_raw_feed_payload_from(owner, topic, initial, &self.chunk_port.0).await
        }

        async fn latest_hls_feed_payload_observing_positive(
            &self,
            owner: String,
            topic: String,
            early_payloads: Option<mpsc::Sender<RawFeedPayload>>,
        ) -> Option<RawFeedPayload> {
            let bounded_startup = early_payloads.is_some();
            let progress_id = self
                .start_progress(
                    "feed-frontier",
                    format!("{} topic {}", owner, topic),
                    "retrieve",
                    None,
                    if bounded_startup {
                        "seeking bounded startup candidate from the first accounting-ready peer"
                    } else {
                        "seeking reliable latest update from the first accounting-ready peer"
                    },
                )
                .await;
            let result = acquire_latest_raw_feed_payload_observing_positive(
                owner,
                topic,
                &self.chunk_port.0,
                early_payloads,
            )
            .await;
            match result.as_ref() {
                Some(payload) => {
                    self.finish_progress(
                        &progress_id,
                        "complete",
                        format!(
                            "{} sequence index {}",
                            if bounded_startup {
                                "resolved bounded candidate"
                            } else {
                                "resolved reliable"
                            },
                            payload.index
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
    }

    thread_local! {
        static FEED_ROUTE_CHECK_SEQUENCE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        static FEED_ROUTE_CACHE: RefCell<HashMap<String, FeedRouteState>> =
            RefCell::new(HashMap::new());
        static HLS_ASSET_METADATA_CACHE: RefCell<HashMap<String, HlsAssetMetadataState>> =
            RefCell::new(HashMap::new());
        static HLS_MEDIA_PLANS: RefCell<HlsMediaPlanRegistry> =
            RefCell::new(HlsMediaPlanRegistry::new(HLS_MEDIA_PLAN_MAX_REFERENCES));
        static HLS_PREFETCH_SESSION: RefCell<HlsPrefetchSession> =
            RefCell::new(HlsPrefetchSession::new());
        static HLS_PAYLOAD_SIZES: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
        static HLS_PAYLOAD_SIZE_PROBES: RefCell<HashMap<String, Vec<mpsc::Sender<Option<u64>>>>> =
            RefCell::new(HashMap::new());
        static HLS_PAYLOAD_CACHE: RefCell<HlsPayloadCache> =
            RefCell::new(HlsPayloadCache::new());
        static STREAM_CATALOG_CALLBACKS: RefCell<Vec<StreamCatalogCallback>> =
            RefCell::new(Vec::new());
    }

    const FEED_ROUTE_CACHE_MAX_ENTRIES: usize = 64;
    const FEED_ROUTE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
    const HLS_TERMINAL_CONFIRMATION_MIN_DELAY: Duration = Duration::from_secs(3);
    const HLS_TERMINAL_CONFIRMATION_POLL_INTERVAL: Duration = Duration::from_millis(500);
    const HLS_TERMINAL_CONFIRMATION_MAX_POLLS: usize = 18;
    const HLS_ASSET_METADATA_CACHE_MAX_ENTRIES: usize = 1024;
    const HLS_ASSET_PROBE_BYTES: u64 = 512;
    const HLS_REPRESENTATION_VERSION: &str = "weeb3-hls-v2";
    const HLS_MEDIA_PLAN_MAX_REFERENCES: usize = 4096;
    const HLS_PREFETCH_TRACK_MAX_ENTRIES: usize = 16;
    const HLS_STREAM_KEY: &str = "weeb3:hls-playback";
    const HLS_PAYLOAD_SINGLEFLIGHT_MAX_WAITERS: usize = 64;
    // One foreground plus three speculative bodies fits the four-tree budget.
    const HLS_STARTUP_BODY_MAX_PARALLEL: usize = 3;
    const HLS_ROLLING_EARLY_OVERLAP_SEGMENTS: usize = 1;
    const HLS_PREFETCH_BODY_MAX_PARALLEL: usize = 3;
    const HLS_PREFETCH_PROBE_MAX_PARALLEL: usize = MEDIA_PREFETCH_MAX_PARALLEL;
    const HLS_PREFETCH_MAX_ATTEMPTS: usize = 6;
    const HLS_FOREGROUND_MAX_ATTEMPTS: usize = HLS_PREFETCH_MAX_ATTEMPTS;
    const HLS_EARLY_FEED_PREFIX_ACTIVE_SEGMENTS: usize = 2;
    const HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS: usize = 4;
    const HLS_EARLY_FEED_PREFIX_INDEX: u64 = 3;
    const HLS_EARLY_FEED_PREFIX_STAGGER: Duration = Duration::from_secs(1);
    const HLS_STARTUP_PREFIX_RESULT_GRACE: Duration = Duration::from_secs(1);
    const HLS_EXACT_NEXT_HEAD_START: Duration = Duration::from_secs(1);
    const HLS_NEXT_RESERVE_STAGGER: Duration = Duration::from_secs(1);
    const HLS_SEQUENCE_ZERO_EXTENSION_DELAY: Duration = Duration::from_secs(2);
    const HLS_SEQUENCE_ZERO_EXTENSION_ADMISSION_BUDGET: Duration = Duration::from_secs(8);
    const HLS_EXACT_OVERLAP_ADMISSION_BUDGET: Duration = Duration::from_secs(30);
    const HLS_INITIAL_RESPONSE_BUDGET_MS: f64 = 15_000.0;
    const HLS_PAYLOAD_RETRY_DELAY_MS: u64 = 75;
    const HLS_STARTUP_LOOKAHEAD_BYTES: u64 = 3 * MEDIA_STARTUP_RESPONSE_BYTES;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum HlsOpenIntent {
        Beginning,
        CurrentWindow,
    }

    impl HlsOpenIntent {
        fn requests_sequence_zero(self, index_hint: Option<u64>) -> bool {
            index_hint.is_none() && self == Self::Beginning
        }
    }

    #[derive(Clone)]
    struct FeedRouteSnapshot {
        index: u64,
        body: Arc<[u8]>,
        /// True only when this representation may expose ENDLIST as terminal.
        ///
        /// An explicit-index route is immutable by definition. For the unindexed
        /// route, an ENDLIST-bearing payload remains provisional until the
        /// reliable sequence-feed finder confirms that payload is the current
        /// head.
        finalized: bool,
    }

    struct FeedRouteState {
        snapshot: FeedRouteSnapshot,
        /// Exact decoded bytes authenticated by `snapshot.index`.
        ///
        /// A sequence-zero EVENT presentation may accumulate identity-checked
        /// suffixes from rolling feed updates in `snapshot.body`. Reliable frontier
        /// searches must still resume from the exact owner-authenticated payload.
        source_body: Arc<[u8]>,
        /// Whether `snapshot.body` incorporates the authenticated update at
        /// `snapshot.index`.
        ///
        /// Stateful rolling manifests cannot be spliced into the sequence-zero
        /// EVENT view safely. Discovery may still advance through a continuous
        /// authenticated source chain while retaining the older visible body, but
        /// that held view must never become terminal.
        body_tracks_source: bool,
        /// A terminal source update has passed the mature-peer head check.
        ///
        /// This is separate from visible finality: an unsupported stateful
        /// ENDLIST may be confirmed as the source head while its sequence-zero
        /// presentation remains provisional and keeps probing for a complete
        /// archive.
        source_endlist_confirmed: bool,
        /// A presentation-scoped canonical candidate has already started its
        /// independent reliable stabilization pass.
        ///
        /// This is deliberately separate from `checking_token`: the sequence-zero
        /// exact follower keeps extending startup runway while one far-ahead
        /// canonical seed catches up to ENDLIST. The seed must not be dropped just
        /// because that exact follower currently owns the cache check token.
        canonical_stabilization_started: bool,
        /// Protect the presentation cache entry while the independent canonical
        /// pass is in flight. `canonical_stabilization_started` remains set after
        /// completion so one presentation can never start a second expensive
        /// frontier traversal.
        canonical_stabilization_running: bool,
        checking_token: u64,
        last_touch: f64,
    }

    enum InitialCanonicalFeedResolution {
        Ready(crate::bzz_stream::RawFeedPayload),
        Pending(mpsc::Receiver<Option<crate::bzz_stream::RawFeedPayload>>),
        Unavailable,
    }

    #[derive(Clone)]
    struct HlsAssetMetadata {
        payload_size: u64,
        mime: &'static str,
        is_manifest: bool,
    }

    struct HlsAssetMetadataState {
        metadata: HlsAssetMetadata,
        last_touch: f64,
    }

    struct ResolvedHlsAsset {
        metadata: HlsAssetMetadata,
        prefetched_body: Option<Arc<[u8]>>,
    }

    struct HlsPrefetchTrack {
        schedule_id: u64,
        last_foreground_position: usize,
        running_generation: Option<u64>,
        last_touch: u64,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum HlsPrefetchMode {
        Inactive,
        StartupOnly,
        Sustained,
    }

    struct HlsPrefetchSession {
        generation: u64,
        timeline_epoch: u64,
        schedule_sequence: u64,
        track_touch_sequence: u64,
        mode: HlsPrefetchMode,
        client: Option<Arc<Weeb3>>,
        feed_identity: Option<(String, String)>,
        sequence_zero_start_requested: bool,
        sequence_zero_runway_admitted: bool,
        sequence_zero_extension_claimed: bool,
        sequence_zero_runway_closed: bool,
        presentation_id: u64,
        timeline_rebasing: bool,
        startup_deadline_ms: f64,
        startup_overlap_plans: HashSet<u64>,
        tracks: HashMap<u64, HlsPrefetchTrack>,
    }

    #[derive(Clone, Copy)]
    struct HlsSequenceZeroRunwayTicket {
        generation: u64,
        timeline_epoch: u64,
        presentation_id: u64,
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
                sequence_zero_start_requested: false,
                sequence_zero_runway_admitted: false,
                sequence_zero_extension_claimed: false,
                sequence_zero_runway_closed: false,
                presentation_id: 0,
                timeline_rebasing: false,
                startup_deadline_ms: 0.0,
                startup_overlap_plans: HashSet::new(),
                tracks: HashMap::new(),
            }
        }

        fn advance_generation(&mut self) -> u64 {
            self.generation = next_media_generation();
            for track in self.tracks.values_mut() {
                track.running_generation = None;
            }
            self.generation
        }

        fn advance_timeline(&mut self) -> u64 {
            self.timeline_epoch = next_nonzero_generation(self.timeline_epoch);
            self.timeline_epoch
        }
    }

    #[derive(Clone)]
    struct HlsForegroundContext {
        generation: u64,
        timeline_epoch: u64,
        schedule_id: Option<u64>,
        cursor: Option<HlsMediaCursor>,
    }

    struct HlsPayloadCache {
        order: VecDeque<String>,
        bodies: HashMap<String, Arc<[u8]>>,
        pending: HashMap<String, PendingHlsPayload>,
        next_load_id: u64,
        body_bytes: u64,
        retain_completed: bool,
    }

    impl HlsPayloadCache {
        fn new() -> Self {
            Self {
                order: VecDeque::new(),
                bodies: HashMap::new(),
                pending: HashMap::new(),
                next_load_id: 0,
                body_bytes: 0,
                retain_completed: true,
            }
        }

        fn load_role(
            &mut self,
            reference: &str,
            prefetch: bool,
            generation: u64,
        ) -> HlsPayloadLoadRole {
            let cached = if prefetch {
                self.body(reference)
            } else {
                self.foreground_body(reference)
            };
            if let Some(body) = cached {
                return HlsPayloadLoadRole::Cached(body);
            }

            if let Some(pending) = self.pending.get_mut(reference) {
                match pending_generation_relation(pending.generation, generation) {
                    PendingGenerationRelation::Join => {
                        pending.waiters.retain(|waiter| !waiter.is_closed());
                        if pending.waiters.len() >= HLS_PAYLOAD_SINGLEFLIGHT_MAX_WAITERS {
                            return HlsPayloadLoadRole::Reject(
                                "HLS fragment already has too many waiting requests".to_string(),
                            );
                        }
                        if !prefetch {
                            // The player joined this detached leader. It is now the
                            // foreground tree, so release its speculative lane.
                            pending.speculative = false;
                        }
                        let (sender, receiver) = mpsc::bounded(1);
                        pending.waiters.push(sender);
                        return HlsPayloadLoadRole::Wait(receiver);
                    }
                    PendingGenerationRelation::RejectStale => {
                        return HlsPayloadLoadRole::Reject(
                            "stale HLS fragment generation".to_string(),
                        );
                    }
                    PendingGenerationRelation::Replace => {}
                }
            }
            if let Some(stale) = self.pending.remove(reference) {
                stale.finish(Err(
                    "stale HLS fragment generation was superseded".to_string()
                ));
            }

            let speculative_loads = self
                .pending
                .values()
                // A pause or seek deliberately leaves old peer requests draining
                // for accounting. Those detached generations must not consume
                // admission capacity in the current playback generation.
                .filter(|pending| pending.speculative && pending.generation == generation)
                .count();
            // Foreground loads and seeks always bypass speculative capacity. The
            // HLS-only body cap applies only to lookahead admissions.
            if prefetch && speculative_loads >= HLS_PREFETCH_BODY_MAX_PARALLEL {
                return HlsPayloadLoadRole::AtCapacity;
            }

            let (sender, receiver) = mpsc::bounded(1);
            self.next_load_id = self.next_load_id.wrapping_add(1);
            if self.next_load_id == 0 {
                self.next_load_id = 1;
            }
            let load_id = self.next_load_id;
            self.pending.insert(
                reference.to_string(),
                PendingHlsPayload {
                    generation,
                    load_id,
                    speculative: prefetch,
                    waiters: vec![sender],
                },
            );
            HlsPayloadLoadRole::Lead(receiver, load_id)
        }

        fn finish_load(
            &mut self,
            reference: &str,
            generation: u64,
            load_id: u64,
            result: Result<Arc<[u8]>, String>,
            hot: bool,
        ) {
            let owns_pending = self.pending.get(reference).is_some_and(|pending| {
                pending.generation == generation && pending.load_id == load_id
            });

            // Immutable data completed by a superseded generation is still useful,
            // but insert it cold so it cannot evict the current seek target.
            if self.retain_completed
                && let Ok(body) = &result
            {
                self.remember(reference.to_string(), body.clone(), hot);
            }
            if !owns_pending {
                return;
            }
            if let Some(pending) = self.pending.remove(reference) {
                pending.finish(result);
            }
        }

        fn body(&mut self, reference: &str) -> Option<Arc<[u8]>> {
            let body = self.bodies.get(reference).cloned()?;
            self.order.retain(|key| key != reference);
            self.order.push_back(reference.to_string());
            Some(body)
        }

        fn foreground_body(&mut self, reference: &str) -> Option<Arc<[u8]>> {
            read_forward_cache_entry(&mut self.order, &self.bodies, reference)
        }

        fn body_size(&self, reference: &str) -> Option<u64> {
            self.bodies
                .get(reference)
                .and_then(|body| u64::try_from(body.len()).ok())
        }

        fn contains_body(&self, reference: &str) -> bool {
            self.bodies.contains_key(reference)
        }

        fn contains_body_or_pending(&self, reference: &str, generation: u64) -> bool {
            self.bodies.contains_key(reference)
                || self
                    .pending
                    .get(reference)
                    .is_some_and(|pending| pending.generation == generation)
        }

        fn join_pending(&mut self, reference: &str, generation: u64) -> Option<HlsPayloadLoadRole> {
            if let Some(body) = self.body(reference) {
                return Some(HlsPayloadLoadRole::Cached(body));
            }
            let pending = self.pending.get_mut(reference)?;
            match pending_generation_relation(pending.generation, generation) {
                PendingGenerationRelation::Join => {
                    pending.waiters.retain(|waiter| !waiter.is_closed());
                    if pending.waiters.len() >= HLS_PAYLOAD_SINGLEFLIGHT_MAX_WAITERS {
                        return Some(HlsPayloadLoadRole::Reject(
                            "HLS fragment already has too many waiting requests".to_string(),
                        ));
                    }
                    let (sender, receiver) = mpsc::bounded(1);
                    pending.waiters.push(sender);
                    Some(HlsPayloadLoadRole::Wait(receiver))
                }
                PendingGenerationRelation::RejectStale => Some(HlsPayloadLoadRole::Reject(
                    "stale HLS fragment generation".to_string(),
                )),
                PendingGenerationRelation::Replace => None,
            }
        }

        fn remember(&mut self, reference: String, body: Arc<[u8]>, hot: bool) {
            let body_len = body.len() as u64;
            let max_bytes = hls_payload_cache_capacity_bytes();
            if body_len > max_bytes {
                return;
            }

            if let Some(previous) = self.bodies.remove(&reference) {
                self.body_bytes = self.body_bytes.saturating_sub(previous.len() as u64);
            }
            self.order.retain(|key| key != &reference);
            if hot {
                self.order.push_back(reference.clone());
            } else {
                self.order.push_front(reference.clone());
            }
            self.bodies.insert(reference, body);
            self.body_bytes = self.body_bytes.saturating_add(body_len);

            let max_entries = usize::try_from(
                media_cache_max_bytes()
                    .checked_div(MEDIA_STORAGE_WINDOW_BYTES)
                    .unwrap_or(1)
                    .max(1),
            )
            .unwrap_or(usize::MAX);
            while self.body_bytes > max_bytes || self.bodies.len() > max_entries {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                if let Some(previous) = self.bodies.remove(&oldest) {
                    self.body_bytes = self.body_bytes.saturating_sub(previous.len() as u64);
                }
            }
        }

        fn resume_completed_retention(&mut self) {
            self.retain_completed = true;
        }

        fn suspend_completed_retention(&mut self) {
            self.retain_completed = false;
            self.order.clear();
            self.bodies.clear();
            self.body_bytes = 0;
        }
    }

    struct PendingHlsPayload {
        generation: u64,
        load_id: u64,
        speculative: bool,
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

    struct StreamCatalogCallback {
        target: Element,
        event_name: &'static str,
        callback: Option<Closure<dyn FnMut()>>,
    }

    impl StreamCatalogCallback {
        fn attach(
            target: &Element,
            event_name: &'static str,
            callback: Closure<dyn FnMut()>,
        ) -> Option<Self> {
            if target
                .add_event_listener_with_callback(event_name, callback.as_ref().unchecked_ref())
                .is_err()
            {
                return None;
            }
            Some(Self {
                target: target.clone(),
                event_name,
                callback: Some(callback),
            })
        }
    }

    impl Drop for StreamCatalogCallback {
        fn drop(&mut self) {
            let Some(callback) = self.callback.take() else {
                return;
            };
            if self
                .target
                .remove_event_listener_with_callback(
                    self.event_name,
                    callback.as_ref().unchecked_ref(),
                )
                .is_err()
            {
                // An attached wasm-bindgen Closure must remain alive if the browser
                // refuses to detach it, otherwise a later event would trap.
                std::mem::forget(callback);
            }
        }
    }

    pub(crate) fn hls_payload_cache_body_bytes() -> u64 {
        HLS_PAYLOAD_CACHE.with(|cache| cache.borrow().body_bytes)
    }

    fn hls_payload_cache_capacity_bytes() -> u64 {
        media_cache_max_bytes().saturating_sub(range_cache_body_bytes())
    }

    fn hls_prefetch_ahead_limit_bytes() -> u64 {
        media_prefetch_ahead_limit_bytes(hls_payload_cache_capacity_bytes())
    }

    async fn fetch_hls_bytes_response(
        weeb3: Arc<Weeb3>,
        reference: String,
        method: String,
        mut range: Option<String>,
        if_none_match: Option<String>,
        if_range: Option<String>,
        local_bytes_base: String,
    ) -> FetchResponse {
        let etag = hls_etag(&reference);
        if if_none_match_matches(if_none_match.as_deref(), &etag) {
            return FetchResponse::ok(304, hls_validator_headers(&reference), None);
        }
        if range.is_some() && !if_range_allows_range(if_range.as_deref(), &etag) {
            // RFC range semantics require a complete 200 response when the
            // validator no longer names this immutable representation.
            range = None;
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
            // Rewriting is intentionally bounded. Do not cache an oversized HLS-
            // looking payload as a manifest, otherwise a later HEAD/Range request
            // would full-join it again before the rewrite limit rejects it.
            let looks_like_manifest = is_hls_manifest(&bytes);
            if looks_like_manifest && bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES {
                return FetchResponse::error(413, "HLS manifest exceeds the supported size limit");
            }
            let mime = hls_payload_mime(&bytes);
            remember_hls_asset_metadata(
                &reference,
                HlsAssetMetadata {
                    payload_size,
                    mime,
                    is_manifest: looks_like_manifest,
                },
            );
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

        if method != "HEAD" {
            // EXT-X-BYTERANGE may race a whole-fragment lookahead. Join that exact
            // immutable singleflight and slice its completed Arc instead of
            // dispatching a duplicate selective traversal.
            let _ = wait_for_pending_hls_payload(&reference).await;
        }
        let Some(resolved) = resolve_hls_asset(weeb3.clone(), reference.clone()).await else {
            return FetchResponse::error(503, "weeb-3 did not retrieve resource");
        };

        if resolved.metadata.is_manifest {
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
            let mut headers = hls_bytes_headers(&reference, "application/vnd.apple.mpegurl");

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
            let start_index = match usize::try_from(start) {
                Ok(start) => start,
                Err(_) => return FetchResponse::error(502, "HLS manifest range is too large"),
            };
            let end_index = match usize::try_from(end.saturating_add(1)) {
                Ok(end) => end,
                Err(_) => return FetchResponse::error(502, "HLS manifest range is too large"),
            };
            let Some(selected) = bytes.get(start_index..end_index) else {
                return FetchResponse::error(
                    502,
                    "HLS manifest range is outside its representation",
                );
            };
            let selected = selected.to_vec();
            headers.push(("Content-Length".to_string(), selected.len().to_string()));
            headers.push((
                "Content-Range".to_string(),
                format!("bytes {}-{}/{}", start, end, size),
            ));
            return FetchResponse::ok(206, headers, Some(selected));
        }

        let size = resolved.metadata.payload_size;
        let mut headers = hls_bytes_headers(&reference, resolved.metadata.mime);
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
        if let Some(body) = resolved.prefetched_body {
            let start_index = match usize::try_from(start) {
                Ok(start) => start,
                Err(_) => return FetchResponse::error(502, "HLS range is too large"),
            };
            let end_index = match usize::try_from(end.saturating_add(1)) {
                Ok(end) => end,
                Err(_) => return FetchResponse::error(502, "HLS range is too large"),
            };
            let Some(selected) = body.get(start_index..end_index) else {
                return FetchResponse::error(502, "HLS range is outside its cached representation");
            };
            let selected = selected.to_vec();
            headers.push(("Content-Length".to_string(), selected.len().to_string()));
            headers.push((
                "Content-Range".to_string(),
                format!("bytes {}-{}/{}", start, end, size),
            ));
            return FetchResponse::ok(206, headers, Some(selected));
        }
        let bytes = weeb3
            .retrieve_hls_payload_range(reference, start, end)
            .await;
        let expected_len = end
            .checked_sub(start)
            .and_then(|length| length.checked_add(1))
            .and_then(|length| usize::try_from(length).ok());
        if !expected_len.is_some_and(|expected| bytes.len() == expected) {
            return FetchResponse::error(502, "weeb-3 returned a short HLS byte range");
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
        let references = hls_media_references(manifest);
        let early_overlap_limit =
            if hls_media_sequence(manifest).is_some_and(|sequence| sequence > 0) {
                HLS_ROLLING_EARLY_OVERLAP_SEGMENTS
            } else {
                HLS_STARTUP_BODY_MAX_PARALLEL
            };
        HLS_MEDIA_PLANS.with(|plans| {
            plans
                .borrow_mut()
                .install_with_early_overlap_limit(references, early_overlap_limit)
        });
    }

    fn cached_hls_payload(reference: &str) -> Option<Arc<[u8]>> {
        let reference = reference.to_ascii_lowercase();
        HLS_PAYLOAD_CACHE.with(|cache| cache.borrow_mut().body(&reference))
    }

    fn cached_hls_payload_size(reference: &str) -> Option<u64> {
        let reference = reference.to_ascii_lowercase();
        HLS_PAYLOAD_CACHE
            .with(|cache| cache.borrow().body_size(&reference))
            .or_else(|| HLS_PAYLOAD_SIZES.with(|sizes| sizes.borrow().get(&reference).copied()))
    }

    fn remember_hls_payload_size(reference: &str, size: u64) {
        if size == 0 {
            return;
        }
        HLS_PAYLOAD_SIZES.with(|sizes| {
            let mut sizes = sizes.borrow_mut();
            if !sizes.contains_key(reference) && sizes.len() >= HLS_MEDIA_PLAN_MAX_REFERENCES {
                sizes.clear();
            }
            sizes.insert(reference.to_ascii_lowercase(), size);
        });
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

        let should_spawn = HLS_PAYLOAD_SIZE_PROBES.with(|probes| {
            let mut probes = probes.borrow_mut();
            if let Some(waiters) = probes.get_mut(&reference) {
                waiters.retain(|waiter| !waiter.is_closed());
                if waiters.len() >= HLS_PAYLOAD_SINGLEFLIGHT_MAX_WAITERS {
                    let _ = sender.try_send(None);
                } else {
                    waiters.push(sender);
                }
                false
            } else {
                probes.insert(reference.clone(), vec![sender]);
                true
            }
        });
        if !should_spawn {
            return receiver;
        }

        // The root-size request owns a detached task. A scheduler select may drop
        // only its result receiver; once dispatched, the probe continues through
        // retrieval/accounting settlement and publishes its immutable result to
        // every live waiter (or just the cache when all waiters have gone away).
        spawn_local(async move {
            let size = weeb3
                .hls_payload_size(reference.clone())
                .await
                .filter(|size| *size > 0);
            if let Some(size) = size {
                remember_hls_payload_size(&reference, size);
            }
            let waiters = HLS_PAYLOAD_SIZE_PROBES
                .with(|probes| probes.borrow_mut().remove(&reference).unwrap_or_default());
            for waiter in waiters {
                let _ = waiter.try_send(size);
            }
        });

        receiver
    }

    fn publish_hls_stream_generation(client: Arc<Weeb3>, generation: u64) {
        spawn_local(async move {
            let _ = client
                .publish_hls_generation(HLS_STREAM_KEY.to_string(), generation)
                .await;
        });
    }

    fn begin_hls_prefetch_session(
        client: Arc<Weeb3>,
        normalized_owner: String,
        normalized_topic: String,
        sequence_zero_start_requested: bool,
        presentation_id: u64,
    ) {
        // The outgoing regular-media view has already advanced its generation in
        // `release_current_stream_view`. Reclaim only completed range bodies here;
        // pending/dispatched reads keep their transport and accounting lifecycle,
        // and stale generations are prevented from repopulating this cache.
        clear_completed_bzz_media_ranges();
        HLS_PAYLOAD_CACHE.with(|cache| cache.borrow_mut().resume_completed_retention());
        let generation = HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            session.client = Some(client.clone());
            session.feed_identity = Some((
                normalized_owner.to_ascii_lowercase(),
                normalized_topic.to_ascii_lowercase(),
            ));
            session.sequence_zero_start_requested = sequence_zero_start_requested;
            session.sequence_zero_runway_admitted = false;
            session.sequence_zero_extension_claimed = false;
            session.sequence_zero_runway_closed = false;
            session.presentation_id = presentation_id;
            session.startup_deadline_ms = hls_monotonic_now_ms()
                .map(|now_ms| now_ms + HLS_INITIAL_RESPONSE_BUDGET_MS)
                .unwrap_or(0.0);
            session.startup_overlap_plans.clear();
            session.mode = HlsPrefetchMode::Inactive;
            session.timeline_rebasing = false;
            session.tracks.clear();
            session.advance_timeline();
            session.advance_generation()
        });
        publish_hls_stream_generation(client, generation);
    }

    fn remember_authenticated_hls_startup_prefix(
        best_prefix: &Rc<RefCell<Option<crate::bzz_stream::RawFeedPayload>>>,
        candidate: &crate::bzz_stream::RawFeedPayload,
    ) -> bool {
        if candidate.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES
            || !is_hls_manifest(&candidate.bytes)
            || hls_media_sequence(&candidate.bytes) != Some(0)
            || hls_media_references(&candidate.bytes).len() < HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS
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

    async fn fan_out_authenticated_hls_prefixes(
        early_payloads: mpsc::Receiver<crate::bzz_stream::RawFeedPayload>,
        prefetch_payloads: mpsc::Sender<crate::bzz_stream::RawFeedPayload>,
        best_prefix: Rc<RefCell<Option<crate::bzz_stream::RawFeedPayload>>>,
        prefix_ready: mpsc::Sender<crate::bzz_stream::RawFeedPayload>,
        startup_cache_key: Option<String>,
    ) {
        while let Ok(payload) = early_payloads.recv().await {
            let accepted_prefix = remember_authenticated_hls_startup_prefix(&best_prefix, &payload);
            let may_extend_visible_prefix = startup_cache_key.as_ref().is_some_and(|cache_key| {
                FEED_ROUTE_CACHE.with(|cache| cache.borrow().contains_key(cache_key))
            });
            if may_extend_visible_prefix
                && payload.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                && is_hls_manifest(&payload.bytes)
                && let Some(cache_key) = startup_cache_key.as_deref()
            {
                // The response path is the sole creator of this overlay. Once its
                // sequence-zero prefix is visible, publish later
                // owner-authenticated observations only through the normal
                // monotonic/continuous cache gate. Cache existence is checked
                // before this observation so a ready-channel backlog cannot chain
                // prefix -> rolling window before the selector publishes the
                // initial response.
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
                // Presentation observes complete decoded, owner-authenticated
                // manifest bytes. A full channel means an equal/older prefix is
                // already ready; feed discovery must never wait on this hint.
                let _ = prefix_ready.try_send(payload.clone());
            }
            // Prefix-body prefetch is opportunistic. Never let its bounded
            // consumer delay feed-frontier discovery or presentation selection.
            let _ = prefetch_payloads.try_send(payload);
        }
    }

    async fn prefetch_authenticated_hls_prefix(
        client: Arc<Weeb3>,
        expected_generation: Option<u64>,
        early_payloads: mpsc::Receiver<crate::bzz_stream::RawFeedPayload>,
    ) {
        let Some(expected_generation) = expected_generation else {
            return;
        };
        let mut policy = HlsEarlyPrefixPolicy::new(
            HLS_EARLY_FEED_PREFIX_ACTIVE_SEGMENTS,
            HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
        );
        let mut loads = FuturesUnordered::new();
        let mut last_leader_admission_ms = None::<f64>;
        let mut early_payloads_open = true;
        let minimum_interval_ms =
            u64::try_from(HLS_EARLY_FEED_PREFIX_STAGGER.as_millis()).unwrap_or(u64::MAX);

        loop {
            if !hls_prefix_admission_is_current(&client, expected_generation) {
                let _ = policy.next_admission(false);
                return;
            }

            while let Some(admission) = policy.next_admission(true) {
                let remaining_ms = hls_prefix_stagger_remaining_ms(
                    minimum_interval_ms,
                    last_leader_admission_ms,
                    hls_monotonic_now_ms(),
                );
                if remaining_ms > 0 {
                    async_std::task::sleep(Duration::from_millis(remaining_ms)).await;
                    if !hls_prefix_admission_is_current(&client, expected_generation) {
                        let _ = policy.next_admission(false);
                        return;
                    }
                }

                let reference = admission.reference;
                let role = loop {
                    if !hls_prefix_admission_is_current(&client, expected_generation) {
                        let _ = policy.next_admission(false);
                        return;
                    }
                    match start_hls_payload_load(
                        client.clone(),
                        reference.clone(),
                        true,
                        expected_generation,
                    ) {
                        HlsPayloadLoadRole::AtCapacity => {
                            // Capacity did not dispatch or account a request. Keep
                            // this exact ordered admission and retry it after the
                            // shared bounded body window makes room.
                            async_std::task::sleep(Duration::from_millis(
                                MEDIA_PREFETCH_BATCH_YIELD_MS,
                            ))
                            .await;
                        }
                        HlsPayloadLoadRole::Reject(_) => {
                            policy.reject(&reference);
                            return;
                        }
                        role => break role,
                    }
                };
                if matches!(&role, HlsPayloadLoadRole::Lead(_, _)) {
                    last_leader_admission_ms = hls_monotonic_now_ms();
                }
                // Retain only completion observers. Detached leaders continue to
                // own transport, cache insertion, and accounting settlement.
                loads.push(async move {
                    let result = wait_hls_payload_load(role).await;
                    (reference, result)
                });
            }

            if policy.target_complete() {
                return;
            }

            // Authenticated prefix observations and body completions can arrive in
            // either order. Prefer a ready completion so the serial a->b->c->d
            // refill chain advances immediately, while still accepting a manifest
            // that grows from fewer than four references during the first load.
            let (payload, completion, payloads_closed) =
                match (early_payloads_open, loads.is_empty()) {
                    (true, false) => {
                        let completion = Box::pin(loads.next());
                        let payload = Box::pin(early_payloads.recv());
                        match select(completion, payload).await {
                            Either::Left((completion, _)) => (None, completion, false),
                            Either::Right((Ok(payload), _)) => (Some(payload), None, false),
                            Either::Right((Err(_), _)) => (None, None, true),
                        }
                    }
                    (true, true) => match early_payloads.recv().await {
                        Ok(payload) => (Some(payload), None, false),
                        Err(_) => (None, None, true),
                    },
                    (false, false) => (None, loads.next().await, false),
                    (false, true) => return,
                };

            if payloads_closed {
                early_payloads_open = false;
            }

            if let Some(payload) = payload {
                if payload.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                    && is_hls_manifest(&payload.bytes)
                    && hls_manifest_starts_at_sequence_zero(&payload.bytes)
                {
                    let ordered_prefix = hls_media_references(&payload.bytes)
                        .into_iter()
                        .take(HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS)
                        .collect::<Vec<_>>();
                    if !ordered_prefix.is_empty() {
                        let _ = policy.observe(&ordered_prefix);
                    }
                    // A shorter observation may arrive out of order. An
                    // incompatible sequence-zero prefix is ignored so it cannot
                    // be mixed with bodies from the canonical authenticated view.
                }
            }

            if let Some((reference, result)) = completion {
                let succeeded = result.is_ok();
                policy.complete(&reference, succeeded);
                if !succeeded {
                    // A contiguous prefix with a failed body is not useful. Stop
                    // only future admissions; detached leaders already dispatched
                    // elsewhere continue retrieval and accounting settlement.
                    return;
                }
            }
        }
    }

    fn hls_prefix_generation_for_feed(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
    ) -> Option<u64> {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            (session.generation != 0
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
                && session.feed_identity.as_ref() == Some(&identity))
            .then_some(session.generation)
        })
    }

    fn hls_sequence_zero_start_presentation_for_feed(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
    ) -> Option<u64> {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            (session.generation != 0
                && session.presentation_id != 0
                && session.sequence_zero_start_requested
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
                && session.feed_identity.as_ref() == Some(&identity))
            .then_some(session.presentation_id)
        })
    }

    fn hls_prefix_generation_is_current(client: &Arc<Weeb3>, expected_generation: u64) -> bool {
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            session.generation == expected_generation
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
        })
    }

    fn hls_prefix_admission_is_current(client: &Arc<Weeb3>, expected_generation: u64) -> bool {
        let generation_current = hls_prefix_generation_is_current(client, expected_generation);
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            hls_prefix_admission_window_is_open(
                generation_current,
                session.mode != HlsPrefetchMode::Inactive,
                hls_monotonic_now_ms(),
                session.startup_deadline_ms,
            )
        })
    }

    fn hls_sequence_zero_runway_ticket(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
    ) -> Option<HlsSequenceZeroRunwayTicket> {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            let session_matches = session.generation != 0
                && session.timeline_epoch != 0
                && session.presentation_id != 0
                && session.sequence_zero_start_requested
                && !session.sequence_zero_runway_closed
                && (!session.sequence_zero_runway_admitted
                    || !session.sequence_zero_extension_claimed)
                && !session.timeline_rebasing
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
                && session.feed_identity.as_ref() == Some(&identity);
            hls_prefix_admission_window_is_open(
                session_matches,
                session.mode == HlsPrefetchMode::Sustained,
                hls_monotonic_now_ms(),
                session.startup_deadline_ms,
            )
            .then_some(HlsSequenceZeroRunwayTicket {
                generation: session.generation,
                timeline_epoch: session.timeline_epoch,
                presentation_id: session.presentation_id,
            })
        })
    }

    fn hls_sequence_zero_runway_admission_is_current(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        ticket: HlsSequenceZeroRunwayTicket,
    ) -> bool {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            let ticket_current = session.generation == ticket.generation
                && session.timeline_epoch == ticket.timeline_epoch
                && session.presentation_id == ticket.presentation_id
                && session.sequence_zero_start_requested
                && !session.sequence_zero_runway_closed
                && !session.timeline_rebasing
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
                && session.feed_identity.as_ref() == Some(&identity);
            hls_prefix_admission_window_is_open(
                ticket_current,
                session.mode == HlsPrefetchMode::Sustained,
                hls_monotonic_now_ms(),
                session.startup_deadline_ms,
            )
        })
    }

    fn claim_hls_sequence_zero_extension(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        ticket: HlsSequenceZeroRunwayTicket,
    ) -> bool {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        let now_ms = hls_monotonic_now_ms();
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            let claim_current = session.generation == ticket.generation
                && session.timeline_epoch == ticket.timeline_epoch
                && session.presentation_id == ticket.presentation_id
                && session.sequence_zero_start_requested
                && !session.sequence_zero_runway_closed
                && session.sequence_zero_runway_admitted
                && !session.sequence_zero_extension_claimed
                && !session.timeline_rebasing
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
                && session.feed_identity.as_ref() == Some(&identity)
                && hls_prefix_admission_window_is_open(
                    true,
                    session.mode == HlsPrefetchMode::Sustained,
                    now_ms,
                    session.startup_deadline_ms,
                );
            if claim_current {
                session.sequence_zero_extension_claimed = true;
            }
            claim_current
        })
    }

    fn hls_sequence_zero_extension_admission_is_current(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        ticket: HlsSequenceZeroRunwayTicket,
    ) -> bool {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            let ticket_current = session.generation == ticket.generation
                && session.timeline_epoch == ticket.timeline_epoch
                && session.presentation_id == ticket.presentation_id
                && session.sequence_zero_start_requested
                && !session.sequence_zero_runway_closed
                && session.sequence_zero_runway_admitted
                && session.sequence_zero_extension_claimed
                && !session.timeline_rebasing
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
                && session.feed_identity.as_ref() == Some(&identity);
            hls_prefix_admission_window_is_open(
                ticket_current,
                session.mode == HlsPrefetchMode::Sustained,
                hls_monotonic_now_ms(),
                session.startup_deadline_ms,
            )
        })
    }

    fn prefetch_hls_sequence_zero_runway_segment(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        manifest: &[u8],
        ticket: Option<HlsSequenceZeroRunwayTicket>,
    ) {
        let Some(ticket) = ticket else {
            return;
        };
        if hls_media_sequence(manifest) != Some(0) {
            return;
        }
        let references = hls_media_references(manifest);
        let Some(reference) = references
            .get(HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS)
            .cloned()
        else {
            return;
        };
        let extension_reference = references
            .get(HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS.saturating_add(1))
            .cloned();
        if !hls_sequence_zero_runway_admission_is_current(client, owner, topic, ticket) {
            return;
        }
        let prefix_is_admitted = HLS_PAYLOAD_CACHE.with(|cache| {
            let cache = cache.borrow();
            references[..HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS]
                .iter()
                .all(|reference| cache.contains_body_or_pending(reference, ticket.generation))
        });
        if !prefix_is_admitted {
            return;
        }
        let fifth_already_admitted = HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            session.generation == ticket.generation
                && session.timeline_epoch == ticket.timeline_epoch
                && session.presentation_id == ticket.presentation_id
                && session.sequence_zero_runway_admitted
        });
        if !fifth_already_admitted {
            let role = start_hls_payload_load(client.clone(), reference, true, ticket.generation);
            let admitted = matches!(
                &role,
                HlsPayloadLoadRole::Cached(_)
                    | HlsPayloadLoadRole::Wait(_)
                    | HlsPayloadLoadRole::Lead(_, _)
            );
            if admitted {
                HLS_PREFETCH_SESSION.with(|session| {
                    let mut session = session.borrow_mut();
                    if session.generation == ticket.generation
                        && session.timeline_epoch == ticket.timeline_epoch
                        && session.presentation_id == ticket.presentation_id
                        && session.sequence_zero_start_requested
                        && !session.timeline_rebasing
                    {
                        session.sequence_zero_runway_admitted = true;
                    }
                });
            }
            drop(role);
            if !admitted {
                return;
            }
        }

        let Some(extension_reference) = extension_reference else {
            return;
        };
        if !claim_hls_sequence_zero_extension(client, owner, topic, ticket) {
            return;
        }

        let extension_client = client.clone();
        let extension_owner = owner.to_string();
        let extension_topic = topic.to_string();
        spawn_local(async move {
            async_std::task::sleep(HLS_SEQUENCE_ZERO_EXTENSION_DELAY).await;
            let retry_limit =
                u64::try_from(HLS_SEQUENCE_ZERO_EXTENSION_ADMISSION_BUDGET.as_millis())
                    .unwrap_or(u64::MAX)
                    .checked_div(MEDIA_PREFETCH_BATCH_YIELD_MS.max(1))
                    .unwrap_or(u64::MAX)
                    .max(1);
            let mut capacity_retries = 0_u64;

            loop {
                if !hls_sequence_zero_extension_admission_is_current(
                    &extension_client,
                    &extension_owner,
                    &extension_topic,
                    ticket,
                ) {
                    return;
                }

                let role = start_hls_payload_load(
                    extension_client.clone(),
                    extension_reference.clone(),
                    true,
                    ticket.generation,
                );
                match &role {
                    HlsPayloadLoadRole::AtCapacity if capacity_retries < retry_limit => {
                        capacity_retries = capacity_retries.saturating_add(1);
                        async_std::task::sleep(Duration::from_millis(
                            MEDIA_PREFETCH_BATCH_YIELD_MS,
                        ))
                        .await;
                        continue;
                    }
                    HlsPayloadLoadRole::AtCapacity | HlsPayloadLoadRole::Reject(_) => return,
                    HlsPayloadLoadRole::Cached(_)
                    | HlsPayloadLoadRole::Wait(_)
                    | HlsPayloadLoadRole::Lead(_, _) => {}
                }

                // A leader owns transport, cache insertion, and accounting after
                // this observer is dropped. Later pause/navigation only prevents
                // future admission and never terminates the dispatched request.
                drop(role);
                return;
            }
        });
    }

    fn hls_playback_prefetch_admission_is_current(
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
        plan_id: u64,
    ) -> bool {
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            session.generation == generation
                && session.timeline_epoch == timeline_epoch
                && !session.timeline_rebasing
                && session.mode != HlsPrefetchMode::Inactive
                && session
                    .tracks
                    .get(&plan_id)
                    .is_some_and(|track| track.schedule_id == schedule_id)
                && (session.mode == HlsPrefetchMode::Sustained
                    || hls_monotonic_now_ms()
                        .is_some_and(|now_ms| now_ms < session.startup_deadline_ms))
        })
    }

    fn hls_manifest_starts_at_sequence_zero(bytes: &[u8]) -> bool {
        hls_media_sequence(bytes) == Some(0)
    }

    fn set_hls_prefetch_mode(mode: HlsPrefetchMode, advance_generation: bool) {
        let publish = HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            if mode == HlsPrefetchMode::Inactive && advance_generation {
                // An explicit pause closes future direct-start runway work for
                // this presentation. A later resume may promote ordinary
                // playback lookahead, but it must not resurrect the delayed sixth
                // body captured before the pause.
                session.sequence_zero_runway_closed = true;
            }
            session.mode = mode;
            if !advance_generation {
                return None;
            }
            let generation = session.advance_generation();
            session
                .client
                .as_ref()
                .cloned()
                .map(|client| (client, generation))
        });
        if let Some((client, generation)) = publish {
            publish_hls_stream_generation(client, generation);
        }
    }

    fn activate_hls_prefetch_warmup() {
        let now_ms = hls_monotonic_now_ms();
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            let timeline_rebasing = session.timeline_rebasing;
            let initial_warmup = !timeline_rebasing && session.mode == HlsPrefetchMode::Inactive;
            if timeline_rebasing {
                // Close the short interval in which the retiring hls.js instance
                // can still issue foreground requests from its LEVEL_LOADED
                // callback. Those foreground leaders keep running, but neither
                // their tracks nor their speculative tasks can become current
                // again after the replacement manifest opens this new epoch.
                session.advance_timeline();
                session.tracks.clear();
                session.startup_overlap_plans.clear();
                session.timeline_rebasing = false;
            }
            if (initial_warmup || timeline_rebasing)
                && let Some(now_ms) = now_ms
            {
                // Feed discovery can legitimately spend the original bounded
                // prefix window. Give the first parsed media manifest one fresh,
                // one-shot runway; repeated playlist reloads see StartupOnly and
                // cannot extend speculative retrieval indefinitely.
                session.startup_deadline_ms = now_ms + HLS_INITIAL_RESPONSE_BUDGET_MS;
            }
            // A user-authorized rebase must remain sustained. Downgrading it to
            // StartupOnly makes the replacement wait for media.play() to resolve
            // before its own successors may be admitted, creating a buffer stall.
            if session.mode != HlsPrefetchMode::Sustained {
                session.mode = HlsPrefetchMode::StartupOnly;
            }
        });
    }

    fn retire_hls_prefetch_timeline() {
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            // A backwards archive expansion replaces every media plan owned by the
            // old hls.js instance. Stop only their future lookahead admissions and
            // retries. Keep the immutable-content generation unchanged so an
            // already dispatched sequence-zero prefix remains singleflight-
            // joinable and all old leaders continue through accounting settlement.
            session.timeline_rebasing = true;
            session.advance_timeline();
            session.tracks.clear();
            session.startup_overlap_plans.clear();
        });
    }

    fn retire_hls_prefetch_plan(
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
        plan_id: u64,
    ) {
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            if session.generation != generation || session.timeline_epoch != timeline_epoch {
                return;
            }
            if !session
                .tracks
                .get(&plan_id)
                .is_some_and(|track| track.schedule_id == schedule_id)
            {
                return;
            }
            session.tracks.remove(&plan_id);
            session.startup_overlap_plans.remove(&plan_id);
        });
    }

    fn invalidate_hls_prefetch_session(clear_client: bool) {
        let publish = HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            session.mode = HlsPrefetchMode::Inactive;
            let generation = session.advance_generation();
            session.tracks.clear();
            let client = session.client.as_ref().cloned();
            if clear_client {
                session.client = None;
                session.feed_identity = None;
                session.sequence_zero_start_requested = false;
                session.sequence_zero_runway_admitted = false;
                session.sequence_zero_extension_claimed = false;
                session.sequence_zero_runway_closed = false;
                session.presentation_id = 0;
            }
            client.map(|client| (client, generation))
        });
        if let Some((client, generation)) = publish {
            publish_hls_stream_generation(client, generation);
        }
    }

    fn hls_foreground_context(reference: &str, cached: bool) -> HlsForegroundContext {
        let reference = reference.to_ascii_lowercase();
        let preferred = HLS_PREFETCH_SESSION.with(|session| {
            session
                .borrow()
                .tracks
                .iter()
                .map(|(plan_id, track)| (*plan_id, track.last_foreground_position))
                .collect::<HashMap<_, _>>()
        });
        let selection = HLS_MEDIA_PLANS.with(|plans| plans.borrow().cursor(&reference, &preferred));
        let cursor = selection.as_ref().map(|selection| selection.cursor.clone());
        let superseded_plan_ids = selection
            .as_ref()
            .map(|selection| selection.superseded_plan_ids.as_slice())
            .unwrap_or_default();

        let mut publish = None;
        let (generation, timeline_epoch, schedule_id) = HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            let mut selected_schedule_id = None;
            if session.generation == 0 {
                session.advance_generation();
            }

            if let Some(cursor) = &cursor {
                let transition = session.tracks.get(&cursor.plan_id).map(|track| {
                    hls_foreground_cursor_transition(
                        track.last_foreground_position,
                        cursor.position,
                        cached,
                    )
                });
                let is_seek = transition.is_some_and(|(is_seek, _)| is_seek);
                if is_seek {
                    let generation = session.advance_generation();
                    publish = session
                        .client
                        .as_ref()
                        .cloned()
                        .map(|client| (client, generation));
                }

                if !session.tracks.contains_key(&cursor.plan_id) {
                    session.schedule_sequence = next_nonzero_generation(session.schedule_sequence);
                    let schedule_id = session.schedule_sequence;
                    session.tracks.insert(
                        cursor.plan_id,
                        HlsPrefetchTrack {
                            schedule_id,
                            last_foreground_position: cursor.position,
                            running_generation: None,
                            last_touch: 0,
                        },
                    );
                }
                if let Some(track) = session.tracks.get_mut(&cursor.plan_id) {
                    track.last_foreground_position = transition
                        .map(|(_, next_position)| next_position)
                        .unwrap_or(cursor.position);
                    selected_schedule_id = Some(track.schedule_id);
                }
                session.track_touch_sequence =
                    next_nonzero_generation(session.track_touch_sequence);
                let touch = session.track_touch_sequence;
                if let Some(track) = session.tracks.get_mut(&cursor.plan_id) {
                    track.last_touch = touch;
                }

                let tracks = session
                    .tracks
                    .iter()
                    .map(|(plan_id, track)| HlsTrackRetention {
                        plan_id: *plan_id,
                        last_touch: track.last_touch,
                        running: track.running_generation == Some(session.generation),
                    })
                    .collect::<Vec<_>>();
                for plan_id in hls_track_ids_to_prune(
                    &tracks,
                    cursor.plan_id,
                    superseded_plan_ids,
                    HLS_PREFETCH_TRACK_MAX_ENTRIES,
                ) {
                    session.tracks.remove(&plan_id);
                    session.startup_overlap_plans.remove(&plan_id);
                }
            }
            (
                session.generation,
                session.timeline_epoch,
                selected_schedule_id,
            )
        });
        if let Some((client, generation)) = publish {
            publish_hls_stream_generation(client, generation);
        }

        HlsForegroundContext {
            generation,
            timeline_epoch,
            schedule_id,
            cursor,
        }
    }

    fn hls_generation_current(generation: u64) -> bool {
        HLS_PREFETCH_SESSION.with(|session| session.borrow().generation == generation)
    }

    fn hls_foreground_retry_is_current(generation: u64, timeline_epoch: u64) -> bool {
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            session.generation == generation
                && session.timeline_epoch == timeline_epoch
                && !session.timeline_rebasing
        })
    }

    fn hls_monotonic_now_ms() -> Option<f64> {
        let window = web_sys::window()?;
        let performance = Reflect::get(window.as_ref(), &JsValue::from_str("performance")).ok()?;
        let now = Reflect::get(&performance, &JsValue::from_str("now")).ok()?;
        now.dyn_ref::<Function>()
            .and_then(|now| now.call0(&performance).ok())?
            .as_f64()
            .filter(|now_ms| now_ms.is_finite())
    }

    fn claim_hls_exact_next_overlap(
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
        cursor: &HlsMediaCursor,
    ) -> bool {
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            if session.generation != generation
                || session.timeline_epoch != timeline_epoch
                || session.timeline_rebasing
                || !session
                    .tracks
                    .get(&cursor.plan_id)
                    .is_some_and(|track| track.schedule_id == schedule_id)
                || session.startup_overlap_plans.contains(&cursor.plan_id)
            {
                return false;
            }
            session.startup_overlap_plans.insert(cursor.plan_id)
        })
    }

    fn hls_prefetch_ticket_current(
        plan_id: u64,
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
    ) -> bool {
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            session.mode != HlsPrefetchMode::Inactive
                && session.generation == generation
                && session.timeline_epoch == timeline_epoch
                && !session.timeline_rebasing
                && session.tracks.get(&plan_id).is_some_and(|track| {
                    track.schedule_id == schedule_id && track.running_generation == Some(generation)
                })
        })
    }

    fn hls_sustained_prefetch_ticket_current(
        plan_id: u64,
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
    ) -> bool {
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            session.mode == HlsPrefetchMode::Sustained
                && session.generation == generation
                && session.timeline_epoch == timeline_epoch
                && !session.timeline_rebasing
                && session.tracks.get(&plan_id).is_some_and(|track| {
                    track.schedule_id == schedule_id && track.running_generation == Some(generation)
                })
        })
    }

    fn start_hls_payload_load(
        weeb3: Arc<Weeb3>,
        reference: String,
        prefetch: bool,
        generation: u64,
    ) -> HlsPayloadLoadRole {
        let reference = reference.to_ascii_lowercase();
        let role = HLS_PAYLOAD_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .load_role(&reference, prefetch, generation)
        });

        if let HlsPayloadLoadRole::Lead(_, load_id) = &role {
            let load_id = *load_id;
            let leader_reference = reference;
            spawn_local(async move {
                let bytes = weeb3
                    .retrieve_hls_payload_cancellable(
                        leader_reference.clone(),
                        HLS_STREAM_KEY.to_string(),
                        generation,
                    )
                    .await;
                let result = if bytes.is_empty() {
                    Err(format!(
                        "weeb-3 did not retrieve HLS fragment {}",
                        leader_reference
                    ))
                } else {
                    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    remember_hls_payload_size(&leader_reference, size);
                    Ok(Arc::<[u8]>::from(bytes))
                };
                let hot = hls_generation_current(generation);
                HLS_PAYLOAD_CACHE.with(|cache| {
                    cache.borrow_mut().finish_load(
                        &leader_reference,
                        generation,
                        load_id,
                        result,
                        hot,
                    );
                });
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
        let generation = HLS_PREFETCH_SESSION.with(|session| session.borrow().generation);
        let role = HLS_PAYLOAD_CACHE
            .with(|cache| cache.borrow_mut().join_pending(&reference, generation))?;
        wait_hls_payload_load(role).await.ok()
    }

    fn spawn_hls_prefetch_stages(
        weeb3: Arc<Weeb3>,
        cursor: HlsMediaCursor,
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
        foreground_ready: mpsc::Receiver<bool>,
    ) -> bool {
        let Some(now_ms) = hls_monotonic_now_ms() else {
            return false;
        };
        let should_spawn = HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            if session.mode == HlsPrefetchMode::Inactive
                || session.generation != generation
                || session.timeline_epoch != timeline_epoch
                || session.timeline_rebasing
            {
                return false;
            }
            if session.mode != HlsPrefetchMode::Sustained && now_ms >= session.startup_deadline_ms {
                return false;
            }
            let Some(track) = session.tracks.get_mut(&cursor.plan_id) else {
                return false;
            };
            if track.schedule_id != schedule_id {
                return false;
            }
            if track.running_generation == Some(generation) {
                return false;
            }
            track.running_generation = Some(generation);
            true
        });
        if !should_spawn {
            return false;
        }

        spawn_local(async move {
            let restart_weeb3 = weeb3.clone();
            prefetch_hls_media_stages(
                weeb3,
                cursor.clone(),
                generation,
                timeline_epoch,
                schedule_id,
                foreground_ready,
            )
            .await;
            let restart_position = HLS_PREFETCH_SESSION.with(|session| {
                let mut session = session.borrow_mut();
                let session_is_current = session.mode == HlsPrefetchMode::Sustained
                    && session.generation == generation
                    && session.timeline_epoch == timeline_epoch
                    && !session.timeline_rebasing;
                let Some(track) = session.tracks.get_mut(&cursor.plan_id) else {
                    return None;
                };
                if track.schedule_id != schedule_id || track.running_generation != Some(generation)
                {
                    return None;
                }
                track.running_generation = None;
                (session_is_current
                    && track.last_foreground_position > cursor.position
                    && track.last_foreground_position.saturating_add(1) < cursor.references.len())
                .then_some(track.last_foreground_position)
            });
            if let Some(position) = restart_position {
                let mut restart_cursor = cursor;
                restart_cursor.position = position;
                let (foreground_ready_out, foreground_ready_in) = mpsc::bounded(1);
                let _ = foreground_ready_out.try_send(true);
                spawn_hls_prefetch_stages(
                    restart_weeb3,
                    restart_cursor,
                    generation,
                    timeline_epoch,
                    schedule_id,
                    foreground_ready_in,
                );
            }
        });
        true
    }

    async fn hls_payload_size_for_prefetch(
        weeb3: Arc<Weeb3>,
        reference: String,
        plan_id: u64,
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
    ) -> Option<u64> {
        if let Some(size) = cached_hls_payload_size(&reference) {
            return Some(size);
        }
        if !hls_prefetch_ticket_current(plan_id, generation, timeline_epoch, schedule_id) {
            return None;
        }

        // A size probe retrieves only the immutable root in a detached
        // singleflight. Dropping this waiter never drops the dispatched request.
        let size = start_hls_payload_size_probe(weeb3, reference.clone())
            .recv()
            .await
            .ok()
            .flatten()?;
        hls_prefetch_ticket_current(plan_id, generation, timeline_epoch, schedule_id)
            .then_some(size)
    }

    type HlsPrefetchProbeResult = (usize, Option<u64>);
    type HlsPrefetchBodyResult = (usize, Result<Arc<[u8]>, String>);

    enum HlsPrefetchProgress {
        Probe(HlsPrefetchProbeResult),
        Body(HlsPrefetchBodyResult),
    }

    enum HlsStartupProgress {
        Foreground(bool),
        Work(HlsPrefetchProgress),
    }

    async fn probe_hls_payload_size_at_position(
        weeb3: Arc<Weeb3>,
        reference: String,
        plan_id: u64,
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
        position: usize,
    ) -> HlsPrefetchProbeResult {
        (
            position,
            hls_payload_size_for_prefetch(
                weeb3,
                reference,
                plan_id,
                generation,
                timeline_epoch,
                schedule_id,
            )
            .await,
        )
    }

    async fn next_hls_prefetch_progress<ProbeFuture, BodyFuture>(
        probes: &mut FuturesUnordered<ProbeFuture>,
        bodies: &mut FuturesUnordered<BodyFuture>,
    ) -> Option<HlsPrefetchProgress>
    where
        ProbeFuture: Future<Output = HlsPrefetchProbeResult>,
        BodyFuture: Future<Output = HlsPrefetchBodyResult>,
    {
        match (probes.is_empty(), bodies.is_empty()) {
            (false, false) => {
                let probe = Box::pin(probes.next());
                let body = Box::pin(bodies.next());
                match select(probe, body).await {
                    Either::Left((result, _)) => result.map(HlsPrefetchProgress::Probe),
                    Either::Right((result, _)) => result.map(HlsPrefetchProgress::Body),
                }
            }
            (false, true) => probes.next().await.map(HlsPrefetchProgress::Probe),
            (true, false) => bodies.next().await.map(HlsPrefetchProgress::Body),
            (true, true) => None,
        }
    }

    async fn next_hls_startup_progress<ProbeFuture, BodyFuture>(
        foreground_ready: &mpsc::Receiver<bool>,
        probes: &mut FuturesUnordered<ProbeFuture>,
        bodies: &mut FuturesUnordered<BodyFuture>,
    ) -> HlsStartupProgress
    where
        ProbeFuture: Future<Output = HlsPrefetchProbeResult>,
        BodyFuture: Future<Output = HlsPrefetchBodyResult>,
    {
        if probes.is_empty() && bodies.is_empty() {
            return HlsStartupProgress::Foreground(foreground_ready.recv().await == Ok(true));
        }

        let foreground = Box::pin(foreground_ready.recv());
        let work = Box::pin(next_hls_prefetch_progress(probes, bodies));
        match select(foreground, work).await {
            Either::Left((result, _)) => HlsStartupProgress::Foreground(result == Ok(true)),
            Either::Right((Some(progress), _)) => HlsStartupProgress::Work(progress),
            Either::Right((None, foreground)) => {
                HlsStartupProgress::Foreground(foreground.await == Ok(true))
            }
        }
    }

    async fn wait_hls_prefetch_load_with_retry(
        weeb3: Arc<Weeb3>,
        reference: String,
        role: HlsPayloadLoadRole,
        plan_id: u64,
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
        position: usize,
    ) -> (usize, Result<Arc<[u8]>, String>) {
        let mut result = wait_hls_payload_load(role).await;
        let mut attempts = 1;
        while result.is_err()
            && attempts < HLS_PREFETCH_MAX_ATTEMPTS
            && hls_prefetch_ticket_current(plan_id, generation, timeline_epoch, schedule_id)
        {
            // The previous attempt has reached a terminal result, so a bounded
            // retry cannot ghost or duplicate an unsettled accounting request.
            async_std::task::sleep(Duration::from_millis(
                HLS_PAYLOAD_RETRY_DELAY_MS.saturating_mul(attempts as u64),
            ))
            .await;
            if !hls_prefetch_ticket_current(plan_id, generation, timeline_epoch, schedule_id) {
                break;
            }
            let retry = start_hls_payload_load(weeb3.clone(), reference.clone(), true, generation);
            if matches!(&retry, HlsPayloadLoadRole::AtCapacity) {
                // Capacity is a transient scheduler condition, not a failed media
                // reference. Preserve this ordered retry without consuming one of
                // the post-terminal retrieval attempts.
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
        generation: u64,
        timeline_epoch: u64,
        schedule_id: u64,
        foreground_ready: mpsc::Receiver<bool>,
    ) {
        let ahead_limit_bytes = hls_prefetch_ahead_limit_bytes();
        let startup_target_bytes = HLS_STARTUP_LOOKAHEAD_BYTES.min(ahead_limit_bytes);
        let startup_body_limit = HLS_STARTUP_BODY_MAX_PARALLEL.min(cursor.early_overlap_limit);
        let mut planned_bytes = 0_u64;
        let first_position = cursor.position.saturating_add(1);
        let mut probe_window = HlsOrderedProbeWindow::new(first_position);
        let mut size_probes = FuturesUnordered::new();
        let mut loads = FuturesUnordered::new();
        let mut budget_blocked = false;

        // Root/body leaders are detached; dropping this scheduler drops observers.
        let foreground_succeeded = loop {
            if !hls_prefetch_ticket_current(cursor.plan_id, generation, timeline_epoch, schedule_id)
            {
                break false;
            }
            let mut capacity_blocked = false;

            if !budget_blocked && planned_bytes < startup_target_bytes {
                for position in probe_window
                    .fill_positions(cursor.references.len(), HLS_PREFETCH_PROBE_MAX_PARALLEL)
                {
                    let reference = cursor.references[position].clone();
                    size_probes.push(probe_hls_payload_size_at_position(
                        weeb3.clone(),
                        reference,
                        cursor.plan_id,
                        generation,
                        timeline_epoch,
                        schedule_id,
                        position,
                    ));
                }
            }

            while !budget_blocked
                && loads.len() < startup_body_limit
                && planned_bytes < startup_target_bytes
            {
                let Some((position, size)) = probe_window.next_ready() else {
                    break;
                };
                let Some(size) = size else {
                    let _ = probe_window.commit_ready();
                    return;
                };
                let reference = cursor.references[position].clone();
                let batch = plan_media_prefetch_batch(
                    planned_bytes,
                    startup_target_bytes,
                    ahead_limit_bytes,
                    &[size],
                );
                if batch.unit_count == 0 {
                    budget_blocked = true;
                    break;
                }

                let role =
                    start_hls_payload_load(weeb3.clone(), reference.clone(), true, generation);
                if matches!(&role, HlsPayloadLoadRole::AtCapacity) {
                    capacity_blocked = true;
                    break;
                }
                let _ = probe_window.commit_ready();
                loads.push(wait_hls_prefetch_load_with_retry(
                    weeb3.clone(),
                    reference,
                    role,
                    cursor.plan_id,
                    generation,
                    timeline_epoch,
                    schedule_id,
                    position,
                ));
                planned_bytes = batch.planned_end_bytes;
            }

            if capacity_blocked {
                let foreground = Box::pin(foreground_ready.recv());
                let retry = Box::pin(async_std::task::sleep(Duration::from_millis(
                    MEDIA_PREFETCH_BATCH_YIELD_MS,
                )));
                match select(foreground, retry).await {
                    Either::Left((result, _)) => break result == Ok(true),
                    Either::Right(_) => continue,
                }
            }

            match next_hls_startup_progress(&foreground_ready, &mut size_probes, &mut loads).await {
                HlsStartupProgress::Foreground(result) => break result,
                HlsStartupProgress::Work(HlsPrefetchProgress::Probe((position, size))) => {
                    probe_window.complete(position, size);
                }
                HlsStartupProgress::Work(HlsPrefetchProgress::Body((_position, result))) => {
                    if result.is_err() {
                        // No later position can close this ordered gap. Stop new
                        // admission immediately; detached leaders still cache and
                        // settle without keeping this scheduler generation alive.
                        return;
                    }
                }
            }
        };

        // A false foreground signal or a stale playback ticket closes only future
        // admissions. The detached leaders above retain ownership until their
        // terminal retrieval/accounting result and may still populate the cache.
        if !foreground_succeeded
            || !hls_prefetch_ticket_current(cursor.plan_id, generation, timeline_epoch, schedule_id)
        {
            return;
        }
        if !hls_sustained_prefetch_ticket_current(
            cursor.plan_id,
            generation,
            timeline_epoch,
            schedule_id,
        ) {
            return;
        }

        loop {
            if !hls_sustained_prefetch_ticket_current(
                cursor.plan_id,
                generation,
                timeline_epoch,
                schedule_id,
            ) {
                // Dropping waiters stops future admission and retries only. Every
                // root/body already dispatched is owned by its detached load task
                // and continues through retrieval and accounting settlement.
                return;
            }
            let mut capacity_blocked = false;

            if planned_bytes >= ahead_limit_bytes {
                // The horizon is fully admitted. Observe the terminal results so a
                // transient attempt can use its bounded post-terminal retry.
                while let Some(result) = loads.next().await {
                    if !hls_sustained_prefetch_ticket_current(
                        cursor.plan_id,
                        generation,
                        timeline_epoch,
                        schedule_id,
                    ) {
                        return;
                    }
                    let (_position, Ok(_body)) = result else {
                        return;
                    };
                }
                return;
            }

            if !budget_blocked {
                for position in probe_window
                    .fill_positions(cursor.references.len(), HLS_PREFETCH_PROBE_MAX_PARALLEL)
                {
                    let reference = cursor.references[position].clone();
                    size_probes.push(probe_hls_payload_size_at_position(
                        weeb3.clone(),
                        reference,
                        cursor.plan_id,
                        generation,
                        timeline_epoch,
                        schedule_id,
                        position,
                    ));
                }
            }

            while !budget_blocked
                && loads.len() < HLS_PREFETCH_BODY_MAX_PARALLEL
                && planned_bytes < ahead_limit_bytes
            {
                let Some((position, size)) = probe_window.next_ready() else {
                    break;
                };
                let Some(size) = size else {
                    let _ = probe_window.commit_ready();
                    return;
                };
                let reference = cursor.references[position].clone();
                let batch = plan_media_prefetch_batch(
                    planned_bytes,
                    ahead_limit_bytes,
                    ahead_limit_bytes,
                    &[size],
                );
                if batch.unit_count == 0 {
                    budget_blocked = true;
                    break;
                }
                let role =
                    start_hls_payload_load(weeb3.clone(), reference.clone(), true, generation);
                if matches!(&role, HlsPayloadLoadRole::AtCapacity) {
                    capacity_blocked = true;
                    break;
                }
                let _ = probe_window.commit_ready();
                loads.push(wait_hls_prefetch_load_with_retry(
                    weeb3.clone(),
                    reference,
                    role,
                    cursor.plan_id,
                    generation,
                    timeline_epoch,
                    schedule_id,
                    position,
                ));
                planned_bytes = batch.planned_end_bytes;
            }

            if capacity_blocked {
                // An unrelated discovery body or another ordered track may own the
                // last speculative slot. Do not commit or skip this position, and
                // do not turn local admission pressure into a retrieval retry.
                // Already-dispatched leaders keep draining through accounting.
                async_std::task::sleep(Duration::from_millis(MEDIA_PREFETCH_BATCH_YIELD_MS)).await;
                continue;
            }

            if budget_blocked {
                while let Some((_position, result)) = loads.next().await {
                    if !hls_sustained_prefetch_ticket_current(
                        cursor.plan_id,
                        generation,
                        timeline_epoch,
                        schedule_id,
                    ) {
                        return;
                    }
                    if result.is_err() {
                        return;
                    }
                }
                return;
            }

            if loads.is_empty() && size_probes.is_empty() {
                return;
            }

            // Body and root-probe completions race in one work-conserving loop.
            // Probe results are admitted strictly by manifest position; a slow
            // later body cannot keep a free slot idle when the next size is ready.
            match next_hls_prefetch_progress(&mut size_probes, &mut loads).await {
                Some(HlsPrefetchProgress::Probe((position, size))) => {
                    probe_window.complete(position, size);
                }
                Some(HlsPrefetchProgress::Body((_position, result))) => {
                    if result.is_err() {
                        return;
                    }
                }
                None => return,
            }
        }
    }

    async fn retrieve_hls_payload_for_playback(
        weeb3: Arc<Weeb3>,
        reference: String,
    ) -> Option<Arc<[u8]>> {
        let reference = reference.to_ascii_lowercase();
        let foreground_cached =
            HLS_PAYLOAD_CACHE.with(|cache| cache.borrow().contains_body(&reference));
        let context = hls_foreground_context(&reference, foreground_cached);
        // Register the foreground before the exact-next overlap. Foreground never
        // competes for speculative admission and promotes an exact pending load.
        let foreground =
            start_hls_payload_load(weeb3.clone(), reference.clone(), false, context.generation);
        if let Some(overlap_schedule_id) = context.schedule_id
            && let Some((cursor, exact_successors)) = context.cursor.as_ref().and_then(|cursor| {
                let exact_successors = cursor
                    .references
                    .iter()
                    .skip(cursor.position.saturating_add(1))
                    .take(
                        cursor
                            .early_overlap_limit
                            .min(HLS_STARTUP_BODY_MAX_PARALLEL),
                    )
                    .cloned()
                    .collect::<Vec<_>>();
                (!exact_successors.is_empty()).then_some((cursor, exact_successors))
            })
            && claim_hls_exact_next_overlap(
                context.generation,
                context.timeline_epoch,
                overlap_schedule_id,
                cursor,
            )
        {
            let overlap_weeb3 = weeb3.clone();
            let overlap_generation = context.generation;
            let overlap_timeline_epoch = context.timeline_epoch;
            let overlap_plan_id = cursor.plan_id;
            let overlap_head_start_ms = if foreground_cached {
                0
            } else {
                u64::try_from(HLS_EXACT_NEXT_HEAD_START.as_millis()).unwrap_or(u64::MAX)
            };
            spawn_local(async move {
                let overlap_started_ms = hls_monotonic_now_ms();
                let stagger_ms =
                    u64::try_from(HLS_NEXT_RESERVE_STAGGER.as_millis()).unwrap_or(u64::MAX);
                let capacity_retry_limit =
                    u64::try_from(HLS_EXACT_OVERLAP_ADMISSION_BUDGET.as_millis())
                        .unwrap_or(u64::MAX)
                        .checked_div(MEDIA_PREFETCH_BATCH_YIELD_MS.max(1))
                        .unwrap_or(u64::MAX)
                        .max(1);
                let mut capacity_retries = 0_u64;

                for (successor, reference) in exact_successors.into_iter().enumerate() {
                    let scheduled_offset_ms = overlap_head_start_ms.saturating_add(
                        stagger_ms.saturating_mul(u64::try_from(successor).unwrap_or(u64::MAX)),
                    );
                    let remaining_ms = match (overlap_started_ms, hls_monotonic_now_ms()) {
                        (Some(started_ms), Some(now_ms))
                            if started_ms.is_finite()
                                && now_ms.is_finite()
                                && now_ms >= started_ms =>
                        {
                            let elapsed_ms = now_ms - started_ms;
                            if elapsed_ms >= scheduled_offset_ms as f64 {
                                0
                            } else {
                                (scheduled_offset_ms as f64 - elapsed_ms).ceil() as u64
                            }
                        }
                        _ => scheduled_offset_ms,
                    };
                    if remaining_ms > 0 {
                        async_std::task::sleep(Duration::from_millis(remaining_ms)).await;
                    }

                    loop {
                        if !hls_playback_prefetch_admission_is_current(
                            overlap_generation,
                            overlap_timeline_epoch,
                            overlap_schedule_id,
                            overlap_plan_id,
                        ) {
                            return;
                        }
                        match start_hls_payload_load(
                            overlap_weeb3.clone(),
                            reference.clone(),
                            true,
                            overlap_generation,
                        ) {
                            HlsPayloadLoadRole::AtCapacity => {
                                // Capacity did not dispatch or account this exact
                                // successor. Retain its ordered position until an
                                // old plan drains a shared slot or the foreground
                                // promotes a pending body. The ticket check above
                                // retires this loop on pause, seek, rebase, or a
                                // terminal foreground failure.
                                if capacity_retries >= capacity_retry_limit {
                                    return;
                                }
                                capacity_retries = capacity_retries.saturating_add(1);
                                async_std::task::sleep(Duration::from_millis(
                                    MEDIA_PREFETCH_BATCH_YIELD_MS,
                                ))
                                .await;
                            }
                            HlsPayloadLoadRole::Reject(_) => return,
                            role => {
                                // The detached leader retains transport, cache, and
                                // accounting ownership. A waiter can be dropped
                                // because the foreground will join the same hash.
                                drop(role);
                                break;
                            }
                        }
                    }
                }
            });
        }

        let (foreground_ready_out, foreground_ready_in) = mpsc::bounded(1);
        if let (Some(schedule_id), Some(cursor)) = (context.schedule_id, context.cursor.clone()) {
            // Register the staged runway while the visible body is still in
            // flight. An uncached foreground keeps the same head start as ordered
            // exact-successor startup. Completed cache hits have no transport to
            // protect and may roll lookahead immediately. The result below controls
            // whether bounded warm-up may continue into sustained lookahead.
            if foreground_cached {
                spawn_hls_prefetch_stages(
                    weeb3.clone(),
                    cursor,
                    context.generation,
                    context.timeline_epoch,
                    schedule_id,
                    foreground_ready_in,
                );
            } else {
                let prefetch_weeb3 = weeb3.clone();
                let generation = context.generation;
                let timeline_epoch = context.timeline_epoch;
                spawn_local(async move {
                    async_std::task::sleep(HLS_EXACT_NEXT_HEAD_START).await;
                    spawn_hls_prefetch_stages(
                        prefetch_weeb3,
                        cursor,
                        generation,
                        timeline_epoch,
                        schedule_id,
                        foreground_ready_in,
                    );
                });
            }
        }

        let mut body = wait_hls_payload_load(foreground).await;
        let mut attempts = 1;
        while body.is_err()
            && attempts < HLS_FOREGROUND_MAX_ATTEMPTS
            && hls_foreground_retry_is_current(context.generation, context.timeline_epoch)
        {
            // A foreground request may have promoted a speculative leader that
            // subsequently failed. Retry only after each shared result is terminal
            // so no dispatched/accounting-sensitive request is abandoned. Several
            // whole-fragment attempts are worthwhile because one missing leaf out
            // of hundreds otherwise creates a visible two-second timeline hole.
            async_std::task::sleep(Duration::from_millis(
                HLS_PAYLOAD_RETRY_DELAY_MS.saturating_mul(attempts as u64),
            ))
            .await;
            if !hls_foreground_retry_is_current(context.generation, context.timeline_epoch) {
                break;
            }
            let retry =
                start_hls_payload_load(weeb3.clone(), reference.clone(), false, context.generation);
            body = wait_hls_payload_load(retry).await;
            attempts += 1;
        }
        let foreground_succeeded = body.is_ok();
        if !foreground_succeeded
            && let (Some(schedule_id), Some(cursor)) =
                (context.schedule_id, context.cursor.as_ref())
        {
            retire_hls_prefetch_plan(
                context.generation,
                context.timeline_epoch,
                schedule_id,
                cursor.plan_id,
            );
        }
        let _ = foreground_ready_out.try_send(foreground_succeeded);
        if foreground_succeeded
            && let (Some(schedule_id), Some(cursor)) = (context.schedule_id, context.cursor.clone())
        {
            // Close the race where the previous runner relinquished this track
            // before the in-flight foreground entered the immutable-body cache.
            // If either the old runner or the delayed starter still owns it, the
            // normal running-generation gate makes this a no-op.
            let (completed_out, completed_in) = mpsc::bounded(1);
            let _ = completed_out.try_send(true);
            spawn_hls_prefetch_stages(
                weeb3.clone(),
                cursor,
                context.generation,
                context.timeline_epoch,
                schedule_id,
                completed_in,
            );
        }
        // The foreground body is the player's only path to append media and build
        // its own buffer. Return it as soon as retrieval completes; exact overlap,
        // prefix leaders, and staged lookahead remain detached and continue
        // through their normal accounting lifecycle.
        body.ok()
    }

    fn cached_hls_asset_metadata(reference: &str) -> Option<HlsAssetMetadata> {
        HLS_ASSET_METADATA_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let state = cache.get_mut(&reference.to_ascii_lowercase())?;
            state.last_touch = js_sys::Date::now();
            Some(state.metadata.clone())
        })
    }

    fn remember_hls_asset_metadata(reference: &str, metadata: HlsAssetMetadata) {
        HLS_ASSET_METADATA_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let key = reference.to_ascii_lowercase();
            if !cache.contains_key(&key) && cache.len() >= HLS_ASSET_METADATA_CACHE_MAX_ENTRIES {
                if let Some(oldest) = cache
                    .iter()
                    .min_by(|left, right| left.1.last_touch.total_cmp(&right.1.last_touch))
                    .map(|(key, _)| key.clone())
                {
                    cache.remove(&oldest);
                }
            }
            cache.insert(
                key,
                HlsAssetMetadataState {
                    metadata,
                    last_touch: js_sys::Date::now(),
                },
            );
        });
    }

    async fn resolve_hls_asset(weeb3: Arc<Weeb3>, reference: String) -> Option<ResolvedHlsAsset> {
        if let Some(metadata) = cached_hls_asset_metadata(&reference) {
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
            remember_hls_asset_metadata(&reference, metadata.clone());
            return Some(ResolvedHlsAsset {
                metadata,
                prefetched_body: Some(body),
            });
        }

        let payload_size = weeb3.hls_payload_size(reference.clone()).await?;
        if payload_size == 0 {
            let metadata = HlsAssetMetadata {
                payload_size,
                mime: "application/octet-stream",
                is_manifest: false,
            };
            remember_hls_asset_metadata(&reference, metadata.clone());
            return Some(ResolvedHlsAsset {
                metadata,
                prefetched_body: None,
            });
        }

        let probe_end = payload_size
            .saturating_sub(1)
            .min(HLS_ASSET_PROBE_BYTES.saturating_sub(1));
        let probe = weeb3
            .retrieve_hls_payload_range(reference.clone(), 0, probe_end)
            .await;
        let expected_probe_len = usize::try_from(probe_end.saturating_add(1)).ok()?;
        if probe.len() != expected_probe_len {
            return None;
        }

        let mut prefetched_body = None;
        let is_manifest =
            if payload_size > MAX_STREAM_FEED_PAYLOAD_BYTES as u64 && is_hls_manifest(&probe) {
                // Preserve the classification so the caller can reject it without
                // joining the entire oversized tree for a HEAD or Range probe.
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
        remember_hls_asset_metadata(&reference, metadata.clone());
        Some(ResolvedHlsAsset {
            metadata,
            prefetched_body,
        })
    }

    async fn fetch_feed_response(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        index_hint: Option<u64>,
        method: String,
        local_bytes_base: String,
    ) -> FetchResponse {
        // Capture the active direct, unindexed view generation before the feed
        // lookup. Indexed/archive reads must never spend its startup runway. A
        // pause, navigation, or timeline replacement while that lookup is in
        // flight invalidates the optional fifth/sixth-fragment admissions.
        let runway_ticket = (method != "HEAD" && index_hint.is_none())
            .then(|| hls_sequence_zero_runway_ticket(&weeb3, &owner, &topic))
            .flatten();
        let snapshot = match load_feed_snapshot(
            weeb3.clone(),
            owner.clone(),
            topic.clone(),
            index_hint,
            false,
        )
        .await
        {
            Some(snapshot) => snapshot,
            // A valid owner/topic route can fail because the current routing view
            // did not retrieve its update. That is not authoritative absence:
            // expose a retryable status so hls.js does not turn a transient Swarm
            // miss into a permanent playlist failure.
            None => return FetchResponse::error(503, "weeb-3 did not retrieve feed update"),
        };

        let is_hls = is_hls_manifest(&snapshot.body);
        let body = if is_hls {
            let rewritten = if index_hint.is_none() {
                rewrite_hls_manifest_for_live_reload(
                    &snapshot.body,
                    &local_bytes_base,
                    snapshot.finalized,
                )
            } else {
                rewrite_hls_manifest(&snapshot.body, &local_bytes_base)
            };
            match rewritten {
                Some(body) => body,
                None => return FetchResponse::error(502, "invalid HLS manifest"),
            }
        } else {
            snapshot.body.to_vec()
        };
        if is_hls {
            // Install the immutable order before admitting the one direct-start
            // runway body. General lookahead still waits for hls.js's first real
            // fragment request.
            remember_hls_media_plan(&body);
            prefetch_hls_sequence_zero_runway_segment(&weeb3, &owner, &topic, &body, runway_ticket);
        }

        let headers = vec![
            (
                "Content-Type".to_string(),
                if is_hls {
                    "application/vnd.apple.mpegurl".to_string()
                } else if serde_json::from_slice::<serde_json::Value>(&body).is_ok() {
                    "application/json; charset=utf-8".to_string()
                } else {
                    "application/octet-stream".to_string()
                },
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

        // Manifests and catalog payloads are deliberately no-store. Segment
        // references inside manifests point at immutable /bytes routes and retain
        // their existing retrieval/cache behavior.
        FetchResponse::ok(200, headers, Some(body))
    }

    async fn load_feed_snapshot(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        index_hint: Option<u64>,
        fresh_live: bool,
    ) -> Option<FeedRouteSnapshot> {
        let owner = owner
            .trim_start_matches("0x")
            .trim_start_matches("0X")
            .to_string();
        let topic = normalize_feed_topic(&topic);
        let sequence_zero_presentation_id = (index_hint.is_none() && !fresh_live)
            .then(|| hls_sequence_zero_start_presentation_for_feed(&weeb3, &owner, &topic))
            .flatten();
        let sequence_zero_start_requested = sequence_zero_presentation_id.is_some();
        let canonical_cache_key = feed_cache_key(&owner, &topic, index_hint);
        let cache_key = if let Some(presentation_id) = sequence_zero_presentation_id {
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
            let refresh_head = index_hint.is_none()
                && (fresh_live || cached_feed_should_refresh_head(state.last_touch, now));
            state.last_touch = now;
            Some((state.snapshot.clone(), refresh_head, selected_key))
        });

        if let Some((snapshot, refresh_head, cached_key)) = cached {
            let cached_is_sequence_zero_presentation =
                sequence_zero_start_requested && cached_key == cache_key;
            let cached_late_window = sequence_zero_start_requested
                && !cached_is_sequence_zero_presentation
                && hls_media_sequence(&snapshot.body).is_some_and(|sequence| sequence > 0);
            if index_hint.is_none() && !fresh_live && !cached_late_window {
                let followup_mode = if cached_is_sequence_zero_presentation {
                    FeedFollowupMode::SequenceZeroPresentation
                } else {
                    FeedFollowupMode::Canonical
                };
                schedule_feed_followup(
                    weeb3,
                    cached_key,
                    owner,
                    topic,
                    refresh_head,
                    followup_mode,
                );
                return Some(snapshot);
            }
            if !fresh_live {
                return Some(snapshot);
            }
            // A direct UI/catalog open has no automatic polling consumer. Await a
            // latest-head read so reopening it cannot render an arbitrarily stale
            // cached catalog. The monotonic store below still wins any race.
        }

        // An index supplied by a past-stream catalog is an immutable snapshot,
        // not a lower bound. Never substitute the current feed head if that exact
        // update is unavailable, and keep it isolated from the advancing live
        // cache entry for the same owner/topic.
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
                    Some(index) => {
                        let candidate = weeb3
                            .hls_feed_payload_at_index(owner.clone(), topic.clone(), index)
                            .await;
                        match candidate {
                            Some(candidate)
                                if candidate.index == index
                                    && candidate.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                                    && is_hls_manifest(&candidate.bytes)
                                    && hls_is_finalized(&candidate.bytes) =>
                            {
                                if sequence_zero_start_requested
                                    && hls_media_sequence(&candidate.bytes) != Some(0)
                                {
                                    // A reliably confirmed rolling ENDLIST remains
                                    // a useful canonical/catalog hint, but cannot
                                    // be the presentation shortcut for a new
                                    // start-at-zero share session.
                                    None
                                } else {
                                    Some(candidate)
                                }
                            }
                            _ => {
                                // A catalog index is only a performance hint. An
                                // unavailable, malformed, live, or stale exact SOC
                                // must never change the unindexed route's fallback
                                // semantics.
                                forget_vod_index(network_id, &owner, &topic);
                                None
                            }
                        }
                    }
                    None => None,
                };
                match persisted {
                    Some(loaded) => loaded,
                    None => {
                        let (early_payload_out, early_payload_in) =
                            mpsc::bounded::<crate::bzz_stream::RawFeedPayload>(16);
                        let (prefetch_payload_out, prefetch_payload_in) =
                            mpsc::bounded::<crate::bzz_stream::RawFeedPayload>(16);
                        let (prefix_ready_out, prefix_ready_in) =
                            mpsc::bounded::<crate::bzz_stream::RawFeedPayload>(1);
                        let best_prefix = Rc::new(RefCell::new(None));
                        spawn_local(fan_out_authenticated_hls_prefixes(
                            early_payload_in,
                            prefetch_payload_out,
                            best_prefix.clone(),
                            prefix_ready_out,
                            sequence_zero_start_requested.then(|| cache_key.clone()),
                        ));
                        if sequence_zero_start_requested {
                            let reliable_prefix_out = early_payload_out.clone();
                            let reliable_prefix_client = weeb3.clone();
                            let reliable_prefix_owner = owner.clone();
                            let reliable_prefix_topic = topic.clone();
                            spawn_local(async move {
                                // The bounded frontier already dispatches index
                                // three, but its Bee-compatible one-second result
                                // listener can expire before a sparse cold peer
                                // view answers. Retain one detached reliable
                                // observer for a direct start-at-zero route.
                                // Closing its notification never cancels dispatched
                                // retrieval/accounting work.
                                if let Some(payload) = reliable_prefix_client
                                    .hls_feed_payload_at_index(
                                        reliable_prefix_owner,
                                        reliable_prefix_topic,
                                        HLS_EARLY_FEED_PREFIX_INDEX,
                                    )
                                    .await
                                {
                                    let _ = reliable_prefix_out.try_send(payload);
                                }
                            });
                        }
                        let early_payload_client = weeb3.clone();
                        let expected_generation =
                            hls_prefix_generation_for_feed(&weeb3, &owner, &topic);
                        spawn_local(async move {
                            prefetch_authenticated_hls_prefix(
                                early_payload_client,
                                expected_generation,
                                prefetch_payload_in,
                            )
                            .await;
                        });

                        // Own the canonical finder in a detached task. Selecting
                        // the decoded prefix below drops only a result listener;
                        // the full frontier algorithm and every dispatched,
                        // accounted lookup continue to completion.
                        let (canonical_out, canonical_in) =
                            mpsc::bounded::<Option<crate::bzz_stream::RawFeedPayload>>(1);
                        let canonical_client = weeb3.clone();
                        let canonical_owner = owner.clone();
                        let canonical_topic = topic.clone();
                        spawn_local(async move {
                            let loaded = canonical_client
                                .latest_hls_feed_payload_observing_positive(
                                    canonical_owner,
                                    canonical_topic,
                                    Some(early_payload_out),
                                )
                                .await;
                            let _ = canonical_out.try_send(loaded);
                        });

                        if sequence_zero_start_requested {
                            let prefix_select_in = prefix_ready_in.clone();
                            let canonical_select_in = canonical_in.clone();
                            let prefix_wait = Box::pin(prefix_select_in.recv());
                            let canonical_wait = Box::pin(canonical_select_in.recv());
                            match select(prefix_wait, canonical_wait).await {
                                Either::Left((Ok(prefix), _)) => {
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
                                    canonical_in.recv().await.ok().flatten()?
                                }
                                Either::Right((Ok(Some(canonical)), _)) => {
                                    let mut preferred =
                                        best_prefix.borrow().clone().filter(|prefix| {
                                            prefix.index < canonical.index
                                                && hls_startup_prefix_is_preferred(
                                                    &canonical.bytes,
                                                    &prefix.bytes,
                                                    HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
                                                    true,
                                                )
                                        });
                                    if preferred.is_none()
                                        && hls_media_sequence(&canonical.bytes)
                                            .is_some_and(|sequence| sequence > 0)
                                        && let Ok(Ok(prefix)) = async_std::future::timeout(
                                            HLS_STARTUP_PREFIX_RESULT_GRACE,
                                            prefix_ready_in.recv(),
                                        )
                                        .await
                                        && prefix.index < canonical.index
                                        && hls_startup_prefix_is_preferred(
                                            &canonical.bytes,
                                            &prefix.bytes,
                                            HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
                                            true,
                                        )
                                    {
                                        preferred = Some(prefix);
                                    }
                                    if let Some(prefix) = preferred {
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
                                    authenticated_startup_prefix = best_prefix.borrow().clone();
                                    canonical
                                }
                                Either::Right((Ok(None) | Err(_), _)) => {
                                    let prefix = prefix_ready_in.recv().await.ok()?;
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
                        sequence_zero_start_requested,
                    )
            })
            .unwrap_or_else(|| canonical_loaded.clone());
        // A cold unindexed service fetch may expose any owner-authenticated,
        // decoded HLS candidate immediately. This includes an ENDLIST-bearing
        // historical update: ENDLIST proves that immutable playlist ended, but not
        // that the mutable sequence feed has no higher update. Exact/reliable head
        // verification continues behind the cache so segment retrieval overlaps
        // it instead of waiting serially. Direct UI feed opens retain their
        // fresh-head behavior, while explicit-index routes remain immutable.
        let provisional_hls = index_hint.is_none()
            && !fresh_live
            && canonical_loaded.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
            && is_hls_manifest(&canonical_loaded.bytes);
        let (loaded, head_confirmed) = if index_hint.is_none() && !provisional_hls {
            stabilize_initial_unindexed_hls_payload(
                weeb3.clone(),
                &owner,
                &topic,
                network_id,
                canonical_loaded.clone(),
                |_, _| {},
            )
            .await
        } else {
            (presentation_loaded, false)
        };
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
        // Never seed a fresh start-at-zero presentation with a late rolling
        // window: its high feed index would reject the lower authenticated prefix.
        // Keep that rare initial fallback in the canonical live namespace. Once a
        // session has published sequence zero, its session-scoped overlay may
        // safely advance through continuous rolling windows for active playback.
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
        if index_hint.is_none() && snapshot.finalized {
            // Persist only the monotonic, reliably head-confirmed unindexed cache
            // winner. Opening an arbitrary exact historical ENDLIST must never
            // poison later latest-feed resolution.
            remember_authenticated_endlist_index(network_id, &owner, &topic, snapshot.index);
        }
        if let Some(initial) = stabilization_seed {
            schedule_initial_feed_stabilization(
                weeb3,
                cache_key,
                owner,
                topic,
                snapshot.index,
                initial,
                followup_mode,
            );
        } else if index_hint.is_none() {
            schedule_feed_followup(weeb3, cache_key, owner, topic, false, followup_mode);
        }
        Some(snapshot)
    }

    async fn await_terminal_feed_confirmation_view(
        weeb3: &Weeb3,
        expected_network_id: u64,
    ) -> bool {
        // Playback and provisional manifest polling already proceed from the first
        // priced peer. Only the irreversible ENDLIST decision waits for a wider
        // peer view and a reliable head search.
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
        if !hls_is_finalized(&candidate.bytes) {
            return (candidate, true);
        }
        if !await_terminal_feed_confirmation_view(&weeb3, expected_network_id).await {
            return (candidate, false);
        }

        let Some(confirmed) = weeb3
            .latest_hls_feed_payload_from(owner.to_string(), topic.to_string(), candidate.clone())
            .await
        else {
            return (candidate, false);
        };
        // The reliable lookup may itself wait on several peer exchanges. Do not
        // carry its terminal verdict across a network change or a peer-view
        // collapse that happened while it was in flight.
        if weeb3.get_network_id().await != expected_network_id {
            return (candidate, false);
        }
        let peer_view_is_mature = hls_terminal_peer_view_is_mature(weeb3.get_connections().await);
        if !peer_view_is_mature || weeb3.get_network_id().await != expected_network_id {
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

    async fn stabilize_initial_unindexed_hls_payload<ObserveCandidate>(
        weeb3: Arc<Weeb3>,
        owner: &str,
        topic: &str,
        network_id: u64,
        mut loaded: crate::bzz_stream::RawFeedPayload,
        mut observe_candidate: ObserveCandidate,
    ) -> (crate::bzz_stream::RawFeedPayload, bool)
    where
        ObserveCandidate: FnMut(&crate::bzz_stream::RawFeedPayload, bool),
    {
        if loaded.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES || !is_hls_manifest(&loaded.bytes) {
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
        let mut exact_updates = 0usize;
        let mut bounded_rechecks = 0usize;
        let mut detail = String::new();
        let mut head_confirmed = false;

        'stabilize: loop {
            // ENDLIST is not trusted as terminal here. It is, however, a useful
            // seed for the mature reliable finder below, which can still discover
            // any newer update. Avoid stacking bounded and exact negative scans
            // for a candidate whose only remaining question is whether it is head.
            if hls_is_finalized(&loaded.bytes) {
                break 'stabilize;
            }
            if bounded_rechecks < HLS_INITIAL_BOUNDED_RECHECK_LIMIT {
                weeb3
                    .update_progress(
                        &progress_id,
                        "verify",
                        None,
                        format!(
                            "resuming bounded verification from candidate {} as the priced peer view matures",
                            loaded.index
                        ),
                    )
                    .await;
                if let Some(recheck) = weeb3
                    .latest_hls_feed_payload_bounded_from(
                        owner.to_string(),
                        topic.to_string(),
                        loaded.clone(),
                    )
                    .await
                    && recheck.index > loaded.index
                    && recheck.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                    && is_hls_manifest(&recheck.bytes)
                {
                    loaded = recheck;
                    observe_candidate(&loaded, false);
                }
                bounded_rechecks = bounded_rechecks.saturating_add(1);
                if hls_is_finalized(&loaded.bytes) {
                    break 'stabilize;
                }
            }

            let exact_round_limit = hls_initial_exact_round_limit(bounded_rechecks, exact_updates);
            let mut round_exact_updates = 0usize;
            while exact_updates < HLS_INITIAL_EXACT_CATCHUP_LIMIT
                && round_exact_updates < exact_round_limit
            {
                let Some(next_index) = loaded.index.checked_add(1) else {
                    detail = "sequence index reached u64::MAX".to_string();
                    break 'stabilize;
                };
                let next = if bounded_rechecks < HLS_INITIAL_BOUNDED_RECHECK_LIMIT {
                    // This is only a result-listener deadline around the exact SOC
                    // probe. A request that crossed accounting remains detached
                    // and drains, and an authenticated manifest body is still
                    // decoded without a deadline.
                    weeb3
                        .hls_feed_payload_at_index_bounded(
                            owner.to_string(),
                            topic.to_string(),
                            next_index,
                        )
                        .await
                } else {
                    // Preserve the existing 32-update reliable fallback once both
                    // bounded frontier rechecks have had their chance.
                    weeb3
                        .hls_feed_payload_at_index(owner.to_string(), topic.to_string(), next_index)
                        .await
                };
                let Some(next) = next else {
                    if bounded_rechecks < HLS_INITIAL_BOUNDED_RECHECK_LIMIT {
                        continue 'stabilize;
                    }
                    // A failed exact read is never proof of the head: on a sparse
                    // priced-peer view an existing SOC can be temporarily absent.
                    // Leave detail empty so the reliable seeded finder confirms
                    // both archived and genuinely live feeds below.
                    break 'stabilize;
                };
                if next.index != next_index {
                    detail = format!("rejected non-exact update after {}", loaded.index);
                    break 'stabilize;
                }
                if next.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES || !is_hls_manifest(&next.bytes)
                {
                    detail = format!("latest update {} is not a valid HLS manifest", next.index);
                    break 'stabilize;
                }

                loaded = next;
                observe_candidate(&loaded, false);
                round_exact_updates = round_exact_updates.saturating_add(1);
                exact_updates = exact_updates.saturating_add(1);
                if hls_is_finalized(&loaded.bytes) {
                    break 'stabilize;
                }
            }

            if bounded_rechecks < HLS_INITIAL_BOUNDED_RECHECK_LIMIT {
                continue;
            }
            break;
        }

        if detail.is_empty() {
            weeb3
                .update_progress(
                    &progress_id,
                    "verify",
                    None,
                    format!(
                        "{} exact updates were contiguous; confirming the reliable head",
                        exact_updates
                    ),
                )
                .await;
            // A bounded/exact phase that already reached ENDLIST does not need an
            // unbounded absent-head traversal before terminal confirmation. The
            // confirmation below first waits for the mature eight-peer view and
            // then performs the reliable, network-sandwiched lookup once. Live
            // candidates still need this preliminary finder to discover a newer
            // snapshot.
            let reliable = if hls_is_finalized(&loaded.bytes) {
                Some(loaded.clone())
            } else {
                weeb3
                    .latest_hls_feed_payload_from(
                        owner.to_string(),
                        topic.to_string(),
                        loaded.clone(),
                    )
                    .await
            };
            detail = match reliable {
                Some(reliable)
                    if reliable.index >= loaded.index
                        && reliable.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                        && is_hls_manifest(&reliable.bytes) =>
                {
                    loaded = reliable;
                    // A first-peer negative result is not proof that no higher SOC
                    // exists. Publish the authenticated candidate provisionally,
                    // then require a wider peer view and a repeated head search
                    // before allowing ENDLIST to become terminal.
                    observe_candidate(&loaded, false);
                    let confirmation =
                        confirm_terminal_feed_head(weeb3.clone(), owner, topic, loaded, network_id)
                            .await;
                    loaded = confirmation.0;
                    head_confirmed = confirmation.1;
                    if head_confirmed {
                        // Equality is meaningful only after the mature-peer
                        // confirmation: promote a byte-identical cached ENDLIST
                        // without requiring a nonexistent higher update.
                        observe_candidate(&loaded, true);
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
                _ => format!(
                    "kept authenticated candidate {} after reliable head decoding failed",
                    loaded.index
                ),
            };
        }

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

    fn publish_stabilized_feed_candidate(
        cache_key: &str,
        checking_token: u64,
        candidate: &crate::bzz_stream::RawFeedPayload,
        head_confirmed: bool,
        followup_mode: FeedFollowupMode,
    ) -> bool {
        if candidate.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES
            || !is_hls_manifest(&candidate.bytes)
        {
            return false;
        }

        let candidate_finalized =
            hls_snapshot_is_terminal(hls_is_finalized(&candidate.bytes), false, head_confirmed);
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let Some(state) = cache.get_mut(cache_key) else {
                return false;
            };
            if state.checking_token != checking_token
                || state.snapshot.finalized
                || candidate.index < state.snapshot.index
            {
                return false;
            }

            if candidate.index == state.snapshot.index {
                if !head_confirmed || candidate.bytes.as_slice() != state.source_body.as_ref() {
                    return false;
                }
                state.source_endlist_confirmed = hls_is_finalized(&candidate.bytes);
                if state.body_tracks_source && hls_is_finalized(&state.snapshot.body) {
                    state.snapshot.finalized = candidate_finalized;
                }
                state.last_touch = js_sys::Date::now();
                return true;
            }

            let source_body: Arc<[u8]> = Arc::from(candidate.bytes.clone());
            let Some(update) = feed_route_update_body(
                &state.snapshot.body,
                &state.source_body,
                &source_body,
                followup_mode,
            ) else {
                return false;
            };
            let (body, body_tracks_source) = match update {
                FeedRouteBodyUpdate::Publish(body) => (body, true),
                FeedRouteBodyUpdate::Hold => (state.snapshot.body.clone(), false),
            };
            let finalized =
                candidate_finalized && body_tracks_source && hls_is_finalized(body.as_ref());

            state.source_body = source_body;
            state.body_tracks_source = body_tracks_source;
            state.source_endlist_confirmed = head_confirmed && hls_is_finalized(&candidate.bytes);
            state.snapshot = FeedRouteSnapshot {
                index: candidate.index,
                body,
                finalized,
            };
            state.last_touch = js_sys::Date::now();
            trim_feed_route_cache(&mut cache, cache_key);
            true
        })
    }

    async fn stabilize_claimed_feed_route(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        network_id: u64,
        checking_token: u64,
        initial: crate::bzz_stream::RawFeedPayload,
        followup_mode: FeedFollowupMode,
    ) {
        // This task deliberately outlives the manifest response. The feed probes
        // it dispatches retain their accounting drain even if the view changes
        // while head verification is still running. Each continuous authenticated
        // advance is published immediately so hls.js reloads do not remain pinned
        // to the first presentation snapshot.
        let (_, head_confirmed) = stabilize_initial_unindexed_hls_payload(
            weeb3.clone(),
            &owner,
            &topic,
            network_id,
            initial,
            |candidate, candidate_head_confirmed| {
                let _ = publish_stabilized_feed_candidate(
                    &cache_key,
                    checking_token,
                    candidate,
                    candidate_head_confirmed,
                    followup_mode,
                );
            },
        )
        .await;

        if let Some((cache_finalized, cache_index)) =
            release_feed_route_check(&cache_key, checking_token)
        {
            if cache_finalized {
                // Use the exact token-guarded cache entry. Recomputing a key from
                // the global active profile here could cross networks if the user
                // switched modes while this detached task drained.
                remember_authenticated_endlist_index(network_id, &owner, &topic, cache_index);
            } else {
                // Any v2 catalog hint remains a useful seed, but a reliable
                // live-head result disproves its terminality. Do not let that stale
                // exact update shortcut a future unindexed open.
                if head_confirmed {
                    forget_vod_index(network_id, &owner, &topic);
                }
                // A transient exact miss is not terminal. Keep the lightweight
                // exact-next poller available after reliable stabilization.
                schedule_feed_followup(weeb3, cache_key, owner, topic, false, followup_mode);
            }
        }
    }

    fn claim_sequence_zero_canonical_stabilization(cache_key: &str) -> bool {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let Some(state) = cache.get_mut(cache_key) else {
                return false;
            };
            if state.snapshot.finalized || state.canonical_stabilization_started {
                return false;
            }
            state.canonical_stabilization_started = true;
            state.canonical_stabilization_running = true;
            true
        })
    }

    fn finish_sequence_zero_canonical_stabilization(cache_key: &str) {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let Some(state) = cache.get_mut(cache_key) else {
                return;
            };
            state.canonical_stabilization_running = false;
            trim_feed_route_cache(&mut cache, cache_key);
        });
    }

    async fn stabilize_sequence_zero_canonical_route(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        network_id: u64,
        initial: crate::bzz_stream::RawFeedPayload,
    ) {
        // The exact prefix follower may already own `checking_token`. Do not cancel
        // it or steal its token: its current accounted request continues to drain
        // and it may keep growing useful startup runway. This single independent
        // pass starts from the far-ahead canonical seed and publishes only its
        // stabilized result through the same monotonic continuity gate.
        let (candidate, head_confirmed) = stabilize_initial_unindexed_hls_payload(
            weeb3.clone(),
            &owner,
            &topic,
            network_id,
            initial,
            |_, _| {},
        )
        .await;

        if weeb3.get_network_id().await == network_id
            && active_profile().swarm_network_id == network_id
        {
            let candidate_index = candidate.index;
            let snapshot = FeedRouteSnapshot {
                index: candidate_index,
                finalized: hls_snapshot_is_terminal(
                    hls_is_finalized(&candidate.bytes),
                    false,
                    head_confirmed,
                ),
                body: Arc::from(candidate.bytes),
            };
            let stored = store_feed_snapshot(
                &cache_key,
                snapshot,
                true,
                FeedFollowupMode::SequenceZeroPresentation,
            );
            if stored.finalized && stored.index == candidate_index {
                remember_authenticated_endlist_index(network_id, &owner, &topic, stored.index);
            }
        }

        finish_sequence_zero_canonical_stabilization(&cache_key);
    }

    fn schedule_sequence_zero_canonical_stabilization(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        network_id: u64,
        initial: crate::bzz_stream::RawFeedPayload,
    ) {
        if !claim_sequence_zero_canonical_stabilization(&cache_key) {
            return;
        }
        spawn_local(stabilize_sequence_zero_canonical_route(
            weeb3, cache_key, owner, topic, network_id, initial,
        ));
    }

    fn schedule_initial_feed_stabilization(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        required_cache_index: u64,
        initial: crate::bzz_stream::RawFeedPayload,
        followup_mode: FeedFollowupMode,
    ) {
        let network_id = active_profile().swarm_network_id;
        let Some((_, checking_token)) =
            claim_feed_route_check(&cache_key, Some(required_cache_index))
        else {
            return;
        };

        spawn_local(stabilize_claimed_feed_route(
            weeb3,
            cache_key,
            owner,
            topic,
            network_id,
            checking_token,
            initial,
            followup_mode,
        ));
    }

    fn publish_sequence_zero_startup_snapshot(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        network_id: u64,
        prefix: crate::bzz_stream::RawFeedPayload,
        canonical: InitialCanonicalFeedResolution,
    ) -> FeedRouteSnapshot {
        let snapshot = store_feed_snapshot(
            &cache_key,
            FeedRouteSnapshot {
                index: prefix.index,
                body: Arc::from(prefix.bytes),
                finalized: false,
            },
            true,
            FeedFollowupMode::SequenceZeroPresentation,
        );
        // Start exact contiguous growth immediately. This claim performs useful
        // index-4+ work instead of reserving the route while the independent
        // canonical finder is still pending. Fan-out observations may still
        // advance the cache through the same monotonic/continuity gate.
        schedule_feed_followup(
            weeb3.clone(),
            cache_key.clone(),
            owner.clone(),
            topic.clone(),
            false,
            FeedFollowupMode::SequenceZeroPresentation,
        );

        spawn_local(async move {
            let initial = match canonical {
                InitialCanonicalFeedResolution::Ready(initial) => Some(initial),
                InitialCanonicalFeedResolution::Pending(receiver) => {
                    receiver.recv().await.ok().flatten()
                }
                InitialCanonicalFeedResolution::Unavailable => None,
            };
            if let Some(initial) = initial {
                // The bounded canonical finder can stop on a far-ahead rolling
                // update while the exact prefix follower owns `checking_token`.
                // Keep one independent presentation-scoped stabilization pass so
                // that seed can reach the full sequence-zero ENDLIST instead of
                // being discarded by a one-shot cache-claim race.
                schedule_sequence_zero_canonical_stabilization(
                    weeb3, cache_key, owner, topic, network_id, initial,
                );
                return;
            }

            // The canonical resolver completed without a decoded payload. Its
            // already-dispatched lookups have still drained normally. The exact
            // follower above remains the useful retry path.
            if weeb3.get_network_id().await == network_id {
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

    fn cached_finalized_feed_index(
        owner: &str,
        topic: &str,
        sequence_zero_presentation_id: Option<u64>,
    ) -> Option<u64> {
        let owner = owner.trim_start_matches("0x").trim_start_matches("0X");
        let topic = normalize_feed_topic(topic);
        let mut cache_keys = Vec::with_capacity(2);
        let require_sequence_zero = sequence_zero_presentation_id.is_some();
        if let Some(presentation_id) = sequence_zero_presentation_id {
            cache_keys.push(sequence_zero_feed_cache_key(owner, &topic, presentation_id));
        }
        cache_keys.push(feed_cache_key(owner, &topic, None));
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            for cache_key in cache_keys {
                let Some(state) = cache.get_mut(&cache_key) else {
                    continue;
                };
                if state.snapshot.finalized
                    && (!require_sequence_zero
                        || hls_media_sequence(&state.snapshot.body) == Some(0))
                {
                    state.last_touch = js_sys::Date::now();
                    return Some(state.snapshot.index);
                }
            }
            None
        })
    }

    enum FeedRouteBodyUpdate {
        Publish(Arc<[u8]>),
        Hold,
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
        if !is_hls_manifest(candidate)
            || !hls_manifest_reload_is_continuous(current_source, candidate.as_ref())
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

    fn store_feed_snapshot(
        cache_key: &str,
        snapshot: FeedRouteSnapshot,
        advancing_live_route: bool,
        followup_mode: FeedFollowupMode,
    ) -> FeedRouteSnapshot {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some(existing) = cache.get_mut(cache_key) {
                // Two cold live lookups may complete out of order. Content-addressed
                // pinned entries share an exact index, while a live entry must never
                // move backwards to a slower, older frontier result. A reliably
                // confirmed terminal head cannot move again. At an equal immutable
                // index, only promote a byte-identical provisional snapshot to
                // confirmed finality.
                if existing.snapshot.finalized || existing.snapshot.index > snapshot.index {
                    existing.last_touch = js_sys::Date::now();
                    return existing.snapshot.clone();
                }
                if existing.snapshot.index == snapshot.index {
                    if snapshot.finalized && existing.source_body == snapshot.body {
                        existing.source_endlist_confirmed = true;
                        if existing.body_tracks_source && hls_is_finalized(&existing.snapshot.body)
                        {
                            existing.snapshot.finalized = true;
                        }
                    }
                    existing.last_touch = js_sys::Date::now();
                    return existing.snapshot.clone();
                }
                let source_body = snapshot.body.clone();
                let update = if advancing_live_route {
                    let Some(update) = feed_route_update_body(
                        &existing.snapshot.body,
                        &existing.source_body,
                        &source_body,
                        followup_mode,
                    ) else {
                        existing.last_touch = js_sys::Date::now();
                        return existing.snapshot.clone();
                    };
                    update
                } else {
                    FeedRouteBodyUpdate::Publish(source_body.clone())
                };
                let (body, body_tracks_source) = match update {
                    FeedRouteBodyUpdate::Publish(body) => (body, true),
                    FeedRouteBodyUpdate::Hold => (existing.snapshot.body.clone(), false),
                };
                let finalized =
                    snapshot.finalized && body_tracks_source && hls_is_finalized(body.as_ref());
                existing.source_body = source_body;
                existing.body_tracks_source = body_tracks_source;
                existing.source_endlist_confirmed = snapshot.finalized;
                existing.snapshot = FeedRouteSnapshot {
                    body,
                    finalized,
                    ..snapshot
                };
                existing.last_touch = js_sys::Date::now();
                let stored = existing.snapshot.clone();
                trim_feed_route_cache(&mut cache, cache_key);
                return stored;
            }
            let source_body = snapshot.body.clone();
            let stored = snapshot.clone();
            cache.insert(
                cache_key.to_string(),
                FeedRouteState {
                    snapshot,
                    source_body,
                    body_tracks_source: true,
                    source_endlist_confirmed: stored.finalized,
                    canonical_stabilization_started: false,
                    canonical_stabilization_running: false,
                    checking_token: 0,
                    last_touch: js_sys::Date::now(),
                },
            );
            trim_feed_route_cache(&mut cache, cache_key);
            stored
        })
    }

    fn trim_feed_route_cache(cache: &mut HashMap<String, FeedRouteState>, protected_key: &str) {
        loop {
            let total_bytes = cache
                .values()
                .map(|state| {
                    state
                        .snapshot
                        .body
                        .len()
                        .saturating_add(state.source_body.len())
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
                        && state.checking_token == 0
                        && !state.canonical_stabilization_running
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
        FEED_ROUTE_CHECK_SEQUENCE.with(|sequence| {
            let next = next_nonzero_generation(sequence.get());
            sequence.set(next);
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
            // Entries protected while an authenticated lookup was draining may
            // have temporarily kept the cache over budget. Re-run eviction as soon
            // as that protection is released.
            trim_feed_route_cache(&mut cache, cache_key);
            Some(released)
        })
    }

    async fn refresh_live_feed_head(
        weeb3: Arc<Weeb3>,
        cache_key: &str,
        owner: &str,
        topic: &str,
        checking_token: u64,
        network_id: u64,
        followup_mode: FeedFollowupMode,
    ) -> bool {
        let Some(initial) = FEED_ROUTE_CACHE.with(|cache| {
            let cache = cache.borrow();
            let state = cache.get(cache_key)?;
            if state.checking_token != checking_token {
                return None;
            }
            Some(crate::bzz_stream::RawFeedPayload {
                index: state.snapshot.index,
                bytes: state.source_body.to_vec(),
            })
        }) else {
            return false;
        };
        let latest = if hls_is_finalized(&initial.bytes) {
            initial
        } else {
            let Some(latest) = weeb3
                .latest_hls_feed_payload_from(owner.to_string(), topic.to_string(), initial)
                .await
            else {
                return false;
            };
            latest
        };
        if latest.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES {
            return false;
        }
        let (latest, head_confirmed) =
            confirm_terminal_feed_head(weeb3.clone(), owner, topic, latest, network_id).await;
        if weeb3.get_network_id().await != network_id {
            return false;
        }

        let latest_index = latest.index;
        let latest_is_hls = is_hls_manifest(&latest.bytes);
        let finalized = hls_snapshot_is_terminal(
            latest_is_hls && hls_is_finalized(&latest.bytes),
            false,
            head_confirmed,
        );
        let latest_source: Arc<[u8]> = Arc::from(latest.bytes);
        let accepted = FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let Some((
                current_index,
                current_finalized,
                current_body,
                current_source,
                body_tracks_source,
                source_matches,
            )) = cache.get(cache_key).and_then(|state| {
                (state.checking_token == checking_token).then_some((
                    state.snapshot.index,
                    state.snapshot.finalized,
                    state.snapshot.body.clone(),
                    state.source_body.clone(),
                    state.body_tracks_source,
                    state.source_body.as_ref() == latest_source.as_ref(),
                ))
            })
            else {
                return None;
            };
            if latest_index < current_index {
                return None;
            }
            if latest_index == current_index && !source_matches {
                return None;
            }
            if !current_finalized {
                let (body, body_tracks_source) = if latest_index == current_index {
                    (current_body, body_tracks_source)
                } else {
                    match feed_route_update_body(
                        &current_body,
                        &current_source,
                        &latest_source,
                        followup_mode,
                    )? {
                        FeedRouteBodyUpdate::Publish(body) => (body, true),
                        FeedRouteBodyUpdate::Hold => (current_body, false),
                    }
                };
                let finalized = finalized && body_tracks_source && hls_is_finalized(body.as_ref());
                let Some(state) = cache.get_mut(cache_key) else {
                    return None;
                };
                state.source_body = latest_source.clone();
                state.body_tracks_source = body_tracks_source;
                state.source_endlist_confirmed =
                    latest_is_hls && hls_is_finalized(latest_source.as_ref()) && head_confirmed;
                state.snapshot = FeedRouteSnapshot {
                    index: latest_index,
                    body,
                    finalized,
                };
                state.last_touch = js_sys::Date::now();
                trim_feed_route_cache(&mut cache, cache_key);
            }
            Some(
                cache
                    .get(cache_key)
                    .and_then(|state| state.snapshot.finalized.then_some(state.snapshot.index)),
            )
        });
        if let Some(Some(index)) = accepted {
            remember_authenticated_endlist_index(network_id, owner, topic, index);
        }
        accepted.is_some()
    }

    fn schedule_feed_followup(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        refresh_head: bool,
        followup_mode: FeedFollowupMode,
    ) {
        let network_id = active_profile().swarm_network_id;
        let Some((mut current_index, checking_token)) = claim_feed_route_check(&cache_key, None)
        else {
            return;
        };

        spawn_local(async move {
            if refresh_head
                && refresh_live_feed_head(
                    weeb3.clone(),
                    &cache_key,
                    &owner,
                    &topic,
                    checking_token,
                    network_id,
                    followup_mode,
                )
                .await
            {
                let _ = release_feed_route_check(&cache_key, checking_token);
                return;
            }

            let mut successful_followups = 0usize;
            let mut saw_tentative_endlist = FEED_ROUTE_CACHE.with(|cache| {
                cache.borrow().get(&cache_key).is_some_and(|state| {
                    !state.snapshot.finalized
                        && !state.source_endlist_confirmed
                        && hls_is_finalized(&state.source_body)
                })
            });
            if saw_tentative_endlist {
                let _ = refresh_live_feed_head(
                    weeb3,
                    &cache_key,
                    &owner,
                    &topic,
                    checking_token,
                    network_id,
                    followup_mode,
                )
                .await;
                let _ = release_feed_route_check(&cache_key, checking_token);
                return;
            }
            let exact_followup_limit = feed_followup_batch_limit(followup_mode);
            let exact_indices =
                std::iter::successors(Some(current_index), |index| index.checked_add(1))
                    .skip(1)
                    .take(exact_followup_limit);
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
                // `buffered` overlaps retrieval but yields in input order, so no
                // later feed update can publish across a missing exact index.
                .buffered(feed_followup_max_parallel(followup_mode));
            while let Some((next_index, next)) = exact_followups.next().await {
                let Some(next) = next else {
                    break;
                };
                if next.index != next_index || next.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES {
                    break;
                }

                let next_is_hls = is_hls_manifest(&next.bytes);
                let has_endlist = next_is_hls && hls_is_finalized(&next.bytes);
                let source_body: Arc<[u8]> = Arc::from(next.bytes);
                let accepted = FEED_ROUTE_CACHE.with(|cache| {
                    let mut cache = cache.borrow_mut();
                    let Some(state) = cache.get_mut(&cache_key) else {
                        return false;
                    };
                    if state.checking_token != checking_token
                        || state.snapshot.index != current_index
                        || state.snapshot.finalized
                    {
                        return false;
                    }
                    if !next_is_hls {
                        return false;
                    }
                    let Some(update) = feed_route_update_body(
                        &state.snapshot.body,
                        &state.source_body,
                        &source_body,
                        followup_mode,
                    ) else {
                        return false;
                    };
                    let (body, body_tracks_source) = match update {
                        FeedRouteBodyUpdate::Publish(body) => (body, true),
                        FeedRouteBodyUpdate::Hold => (state.snapshot.body.clone(), false),
                    };
                    state.source_body = source_body.clone();
                    state.body_tracks_source = body_tracks_source;
                    state.source_endlist_confirmed = false;
                    state.snapshot = FeedRouteSnapshot {
                        index: next.index,
                        body,
                        // Exact adjacency proves sequence continuity, not that
                        // this is the feed head. Reliable head confirmation below
                        // is the only path that may expose ENDLIST.
                        finalized: false,
                    };
                    state.last_touch = js_sys::Date::now();
                    trim_feed_route_cache(&mut cache, &cache_key);
                    true
                });
                if !accepted {
                    break;
                }
                saw_tentative_endlist |= has_endlist;
                successful_followups = successful_followups.saturating_add(1);
                current_index = next_index;
                if has_endlist {
                    break;
                }
            }
            // Dropping buffered tail observers closes only their queued admission
            // and future hedges. Any peer request already dispatched keeps its
            // detached terminal accounting lifecycle.
            drop(exact_followups);

            if feed_followup_should_refresh_head(
                followup_mode,
                successful_followups,
                saw_tentative_endlist,
            ) {
                let _ = refresh_live_feed_head(
                    weeb3,
                    &cache_key,
                    &owner,
                    &topic,
                    checking_token,
                    network_id,
                    followup_mode,
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
    ) -> Option<FetchResponse> {
        if let Some(reference) = canonical_hls_bytes_resource(pathname) {
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
                    local_hls_bytes_base(pathname),
                )
                .await,
            );
        }

        let (owner, topic) = canonical_feed_resource(pathname)?;
        let feed_index = web_sys::Url::new(request_url)
            .ok()
            .and_then(|url| url.search_params().get("index"));
        let index_hint = match feed_index {
            Some(index) => match index.parse::<u64>() {
                Ok(index) => Some(index),
                Err(_) => return Some(FetchResponse::error(400, "invalid feed index")),
            },
            None => None,
        };
        Some(
            fetch_feed_response(
                weeb3,
                owner,
                topic,
                index_hint,
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
        let base = streaming_route_base();
        if pathname.starts_with(&format!("{base}/testnet/")) {
            streaming_route_path("testnet/hls/bytes")
        } else if pathname.starts_with(&format!("{base}/mainnet/")) {
            streaming_route_path("mainnet/hls/bytes")
        } else {
            streaming_route_path("hls/bytes")
        }
    }

    pub(crate) async fn open_feed_view(weeb3: Arc<Weeb3>, owner: String, topic: String) {
        let view_generation = begin_result_view_request();
        render_stream_status_for_generation("Resolving Swarm stream feed...", view_generation);
        let owner = owner
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X")
            .to_string();
        let topic = normalize_feed_topic(&topic);
        let Some(snapshot) =
            load_feed_snapshot(weeb3.clone(), owner.clone(), topic.clone(), None, true).await
        else {
            render_stream_status_for_generation(
                "Could not retrieve the Swarm feed.",
                view_generation,
            );
            return;
        };
        if !result_view_request_is_current(view_generation) {
            return;
        }

        if is_hls_manifest(&snapshot.body) {
            open_hls_feed_view_generation(
                weeb3,
                owner,
                topic,
                "video".to_string(),
                None,
                HlsOpenIntent::CurrentWindow,
                view_generation,
            )
            .await;
            return;
        }

        let Some(entries) = parse_stream_catalog(&snapshot.body) else {
            render_stream_status_for_generation(
                "The feed is neither an HLS manifest nor a supported stream catalog.",
                view_generation,
            );
            return;
        };
        render_stream_catalog(weeb3, entries, view_generation);
    }

    pub(crate) async fn open_hls_feed_view(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        media_type: String,
        index_hint: Option<u64>,
    ) {
        let view_generation = begin_result_view_request();
        open_hls_feed_view_generation(
            weeb3,
            owner,
            topic,
            media_type,
            index_hint,
            HlsOpenIntent::Beginning,
            view_generation,
        )
        .await;
    }

    async fn open_hls_feed_view_current_window(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        media_type: String,
        index_hint: Option<u64>,
    ) {
        let view_generation = begin_result_view_request();
        open_hls_feed_view_generation(
            weeb3,
            owner,
            topic,
            media_type,
            index_hint,
            HlsOpenIntent::CurrentWindow,
            view_generation,
        )
        .await;
    }

    #[derive(Clone)]
    struct HlsFeedTarget {
        owner: String,
        topic: String,
        source: String,
        index_hint: Option<u64>,
        sequence_zero_start_requested: bool,
    }

    async fn prepare_hls_feed_target(
        weeb3: &Arc<Weeb3>,
        owner: String,
        topic: String,
        index_hint: Option<u64>,
        intent: HlsOpenIntent,
        view_generation: u64,
    ) -> Result<HlsFeedTarget, String> {
        if !service_worker_controls_bzz_requests(weeb3, "HLS feed and segment requests", || {
            result_view_request_is_current(view_generation)
        })
        .await
        {
            return Err(service_worker_scope_protocol_error(
                "HLS feed and segment requests",
            ));
        }
        if !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }

        let owner = owner
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X")
            .to_string();
        let topic = normalize_feed_topic(&topic);
        if owner.len() != 40 || !owner.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("The stream feed owner is invalid.".to_string());
        }

        Ok(HlsFeedTarget {
            source: canonical_hls_feed_url(&owner, &topic, index_hint),
            owner,
            topic,
            index_hint,
            sequence_zero_start_requested: intent.requests_sequence_zero(index_hint),
        })
    }

    async fn attach_hls_feed_player(
        weeb3: Arc<Weeb3>,
        player: &Element,
        target: &HlsFeedTarget,
        view_generation: u64,
    ) -> Result<&'static str, String> {
        if !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        install_hls_prefetch_lifecycle(
            player,
            weeb3.clone(),
            target.owner.clone(),
            target.topic.clone(),
            target.sequence_zero_start_requested,
            view_generation,
        );
        install_hls_route_canonicalization(
            player,
            &target.owner,
            &target.topic,
            target.index_hint,
            target.sequence_zero_start_requested,
            view_generation,
        );

        weeb3.interface_log(format!(
            "HLS open owner={} topic={}{}",
            target.owner,
            target.topic,
            target
                .index_hint
                .map(|index| format!(" index={}", index))
                .unwrap_or_default()
        ));

        let mode = play_hls(player, &target.source)
            .await
            .map_err(|error| format!("Could not initialize HLS: {}", js_error_message(&error)))?;
        if !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        Ok(mode)
    }

    pub(crate) async fn attach_hls_feed_media(
        weeb3: Arc<Weeb3>,
        player: Element,
        owner: String,
        topic: String,
        index_hint: Option<u64>,
        intent: HlsOpenIntent,
    ) -> Result<&'static str, String> {
        if !player.is_connected() {
            return Err(
                "attach the HLS media element to the document before opening the stream".into(),
            );
        }
        let view_generation = begin_result_view_request();
        release_current_stream_view();
        let target =
            prepare_hls_feed_target(&weeb3, owner, topic, index_hint, intent, view_generation)
                .await?;
        attach_hls_feed_player(weeb3, &player, &target, view_generation).await
    }

    async fn open_hls_feed_view_generation(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        media_type: String,
        index_hint: Option<u64>,
        intent: HlsOpenIntent,
        view_generation: u64,
    ) {
        render_stream_status_for_generation(
            "Preparing reload-free Service Worker routing for HLS...",
            view_generation,
        );
        let target = match prepare_hls_feed_target(
            &weeb3,
            owner,
            topic,
            index_hint,
            intent,
            view_generation,
        )
        .await
        {
            Ok(target) => target,
            Err(error) => {
                if result_view_request_is_current(view_generation) {
                    render_stream_status_for_generation(&error, view_generation);
                }
                return;
            }
        };
        let document = web_sys::window().unwrap().document().unwrap();
        let wrapper = document.create_element("section").unwrap();
        let player = create_hls_player(&media_type);
        let status = document.create_element("div").unwrap();
        let (share_control, share_callback) = create_stream_share_control(
            &target.owner,
            &target.topic,
            target.index_hint,
            "this stream",
            &format!("player-{view_generation}"),
            view_generation,
        );
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
        let _ = wrapper.append_child(&share_control);
        if !replace_stream_result_view(&wrapper, view_generation) {
            return;
        }
        STREAM_CATALOG_CALLBACKS.with(|stored| {
            *stored.borrow_mut() = share_callback.into_iter().collect();
        });

        let mode = match attach_hls_feed_player(weeb3, &player, &target, view_generation).await {
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

    fn render_stream_catalog(
        weeb3: Arc<Weeb3>,
        entries: Vec<StreamCatalogEntry>,
        view_generation: u64,
    ) {
        if !result_view_request_is_current(view_generation) {
            return;
        }
        let vod_hints = entries
            .iter()
            .filter_map(|entry| {
                entry.is_vod().then_some((
                    entry.owner.clone(),
                    normalize_feed_topic(&entry.topic),
                    entry.index?,
                ))
            })
            .collect::<Vec<_>>();
        if !vod_hints.is_empty() {
            remember_catalog_vod_indices(active_profile().swarm_network_id, vod_hints);
        }
        let document = web_sys::window().unwrap().document().unwrap();
        let catalog = document.create_element("section").unwrap();
        catalog.set_class_name("weeb3-stream-catalog");
        let heading = document.create_element("h3").unwrap();
        heading.set_text_content(Some("Streams on Swarm"));
        let explanation = document.create_element("p").unwrap();
        explanation.set_text_content(Some(
            "Choose a stream. Archived manifests use their exact feed index for a fast, deterministic lookup.",
        ));
        let _ = catalog.append_child(&heading);
        let _ = catalog.append_child(&explanation);

        if entries.is_empty() {
            let empty = document.create_element("p").unwrap();
            empty.set_text_content(Some(
                "This stream catalog does not contain any entries yet.",
            ));
            let _ = catalog.append_child(&empty);
            let _ = replace_stream_result_view(&catalog, view_generation);
            return;
        }

        let mut callbacks = Vec::with_capacity(entries.len().min(100).saturating_mul(2));
        for (row_index, entry) in entries.into_iter().take(100).enumerate() {
            let row = document.create_element("div").unwrap();
            row.set_class_name("weeb3-stream-entry");
            let button = document.create_element("button").unwrap();
            let title = if entry.title.trim().is_empty() {
                format!("Stream {}", short_identity(&entry.topic))
            } else {
                entry.title.trim().to_string()
            };
            let state = if entry.is_live() {
                "LIVE"
            } else if entry.is_vod() {
                "VOD"
            } else {
                "STREAM"
            };
            let duration = entry
                .duration
                .filter(|duration| duration.is_finite() && *duration > 0.0)
                .map(format_stream_duration)
                .unwrap_or_default();
            let label = if duration.is_empty() {
                format!("[ {} ] {}", state, title)
            } else {
                format!("[ {} ] {} - {}", state, title, duration)
            };
            button.set_text_content(Some(&label));
            let _ = button.set_attribute("type", "button");
            let _ = button.set_attribute("aria-label", &format!("Play {title}"));
            let _ = row.set_attribute("role", "group");
            let _ = row.set_attribute("aria-label", &format!("{state} stream: {title}"));
            if !entry.description.trim().is_empty() {
                let _ = button.set_attribute("title", entry.description.trim());
            }

            let weeb3 = weeb3.clone();
            let owner = entry.owner.clone();
            let topic = entry.topic.clone();
            let media_type = entry.media_type().to_string();
            // Catalog producers sometimes include their current index on live
            // entries. Treat it as informational; only archived VOD is immutable.
            let index = entry.is_vod().then_some(entry.index).flatten();
            let (share_control, share_callback) = create_stream_share_control(
                &owner,
                &topic,
                index,
                &title,
                &format!("catalog-{view_generation}-{row_index}"),
                view_generation,
            );
            let playback_callback = Closure::<dyn FnMut()>::new(move || {
                let weeb3 = weeb3.clone();
                let owner = owner.clone();
                let topic = topic.clone();
                let media_type = media_type.clone();
                spawn_local(async move {
                    open_hls_feed_view_current_window(weeb3, owner, topic, media_type, index).await;
                });
            });
            if let Some(playback_callback) =
                StreamCatalogCallback::attach(&button, "click", playback_callback)
            {
                callbacks.push(playback_callback);
            }
            if let Some(share_callback) = share_callback {
                callbacks.push(share_callback);
            }
            let _ = row.append_child(&button);
            let _ = row.append_child(&share_control);
            let _ = catalog.append_child(&row);
        }

        if replace_stream_result_view(&catalog, view_generation) {
            STREAM_CATALOG_CALLBACKS.with(|stored| {
                *stored.borrow_mut() = callbacks;
            });
        }
    }

    fn canonical_hls_feed_url(owner: &str, topic: &str, index_hint: Option<u64>) -> String {
        let prefix = match active_profile().mode {
            NetworkMode::Mainnet => streaming_route_path("feeds"),
            NetworkMode::Testnet => streaming_route_path("testnet/feeds"),
        };
        let mut source = format!("{}/{}/{}", prefix, owner, topic);
        if let Some(index) = index_hint {
            source.push_str(&format!("?index={}", index));
        }
        source
    }

    fn stream_share_url(
        owner: &str,
        topic: &str,
        index_hint: Option<u64>,
    ) -> Result<String, String> {
        let network = match active_profile().mode {
            NetworkMode::Mainnet => StreamShareNetwork::Mainnet,
            NetworkMode::Testnet => StreamShareNetwork::Testnet,
        };
        let route = StreamShareRoute::new(
            network,
            owner.trim(),
            normalize_feed_topic(topic),
            index_hint,
        )?;
        let route_base = streaming_route_base();
        match web_sys::window().and_then(|window| window.location().origin().ok()) {
            Some(origin) => build_stream_share_url(&origin, &route_base, &route)
                .or_else(|_| build_stream_share_path(&route_base, &route)),
            None => build_stream_share_path(&route_base, &route),
        }
    }

    fn clipboard_write_text(value: &str) -> Option<Promise> {
        let window = web_sys::window()?;
        let navigator = window.navigator();
        let clipboard = Reflect::get(navigator.as_ref(), &JsValue::from_str("clipboard")).ok()?;
        if clipboard.is_null() || clipboard.is_undefined() {
            return None;
        }
        let write_text = Reflect::get(&clipboard, &JsValue::from_str("writeText"))
            .ok()?
            .dyn_into::<Function>()
            .ok()?;
        write_text
            .call1(&clipboard, &JsValue::from_str(value))
            .ok()?
            .dyn_into::<Promise>()
            .ok()
    }

    fn show_stream_share_fallback(
        status: &Element,
        fallback: &Element,
        share_url: &str,
        view_generation: u64,
    ) {
        if !result_view_request_is_current(view_generation) {
            return;
        }
        status.set_text_content(Some(&format!(
            "Automatic copy was unavailable. Copy this stream link: {share_url}"
        )));
        let _ = fallback.remove_attribute("hidden");
    }

    fn create_stream_share_control(
        owner: &str,
        topic: &str,
        index_hint: Option<u64>,
        stream_label: &str,
        id_suffix: &str,
        view_generation: u64,
    ) -> (Element, Option<StreamCatalogCallback>) {
        let document = web_sys::window().unwrap().document().unwrap();
        let container = document.create_element("div").unwrap();
        container.set_class_name("weeb3-stream-share");

        let button = document.create_element("button").unwrap();
        button.set_class_name("weeb3-stream-share-copy");
        button.set_text_content(Some("Copy stream link"));
        let _ = button.set_attribute("type", "button");
        let _ = button.set_attribute("aria-label", &format!("Copy link for {stream_label}"));

        let status = document.create_element("span").unwrap();
        status.set_class_name("weeb3-stream-share-status");
        let status_id = format!("weeb3-stream-share-status-{id_suffix}");
        status.set_id(&status_id);
        let _ = status.set_attribute("role", "status");
        let _ = status.set_attribute("aria-live", "polite");
        let _ = status.set_attribute("aria-atomic", "true");
        let _ = button.set_attribute("aria-describedby", &status_id);

        let share_url = stream_share_url(owner, topic, index_hint).ok();
        let fallback = document.create_element("a").unwrap();
        fallback.set_class_name("weeb3-stream-share-fallback");
        let fallback_id = format!("weeb3-stream-share-fallback-{id_suffix}");
        fallback.set_id(&fallback_id);
        fallback.set_text_content(Some("Open stream link"));
        let _ = fallback.set_attribute("target", "_blank");
        let _ = fallback.set_attribute("rel", "noopener");
        let _ = fallback.set_attribute("hidden", "");
        let _ = button.set_attribute("aria-controls", &fallback_id);
        if let Some(share_url) = &share_url {
            let _ = fallback.set_attribute("href", share_url);
            let _ = button.set_attribute("data-stream-share-url", share_url);
        } else {
            let _ = button.set_attribute("disabled", "");
            status.set_text_content(Some("A share link could not be created for this stream."));
        }

        let click_button = button.clone();
        let click_status = status.clone();
        let click_fallback = fallback.clone();
        let click_owner = owner.to_string();
        let click_topic = topic.to_string();
        let callback = Closure::<dyn FnMut()>::new(move || {
            if !result_view_request_is_current(view_generation) {
                return;
            }
            let effective_index = index_hint
                .or_else(|| cached_finalized_feed_index(&click_owner, &click_topic, None));
            let Ok(click_share_url) = stream_share_url(&click_owner, &click_topic, effective_index)
            else {
                click_status
                    .set_text_content(Some("A share link could not be created for this stream."));
                return;
            };
            let _ = click_button.set_attribute("data-stream-share-url", &click_share_url);
            let _ = click_fallback.set_attribute("href", &click_share_url);
            click_status.set_text_content(Some("Copying stream link..."));
            let _ = click_fallback.set_attribute("hidden", "");

            let Some(copy_promise) = clipboard_write_text(&click_share_url) else {
                show_stream_share_fallback(
                    &click_status,
                    &click_fallback,
                    &click_share_url,
                    view_generation,
                );
                return;
            };

            let status = click_status.clone();
            let fallback = click_fallback.clone();
            let share_url = click_share_url;
            spawn_local(async move {
                match JsFuture::from(copy_promise).await {
                    Ok(_) if result_view_request_is_current(view_generation) => {
                        status.set_text_content(Some("Stream link copied."));
                        let _ = fallback.set_attribute("hidden", "");
                    }
                    Err(_) => {
                        show_stream_share_fallback(&status, &fallback, &share_url, view_generation);
                    }
                    Ok(_) => {}
                }
            });
        });
        let callback = StreamCatalogCallback::attach(&button, "click", callback);

        let _ = container.append_child(&button);
        let _ = container.append_child(&status);
        let _ = container.append_child(&fallback);
        (container, callback)
    }

    fn create_hls_player(media_type: &str) -> Element {
        let document = web_sys::window().unwrap().document().unwrap();
        let is_audio = media_type.eq_ignore_ascii_case("audio");
        let tag = if is_audio { "audio" } else { "video" };
        let player = document.create_element(tag).unwrap();
        let _ = player.set_attribute("controls", "");
        let _ = player.set_attribute("autoplay", "");
        let _ = player.set_attribute("preload", "metadata");
        let _ = player.set_attribute(
            "aria-label",
            if is_audio {
                "Swarm HLS audio stream"
            } else {
                "Swarm HLS video stream"
            },
        );
        let _ = player.set_attribute("playsinline", "");
        let _ = player.set_attribute("style", "width:90%;max-height:75vh;");
        player
    }

    fn render_stream_status_for_generation(message: &str, view_generation: u64) {
        let document = web_sys::window().unwrap().document().unwrap();
        let status = document.create_element("p").unwrap();
        status.set_text_content(Some(message));
        let _ = replace_stream_result_view(&status, view_generation);
    }

    fn format_stream_duration(seconds: f64) -> String {
        let total = seconds.round().max(0.0) as u64;
        let hours = total / 3600;
        let minutes = (total % 3600) / 60;
        let seconds = total % 60;
        if hours > 0 {
            format!("{}:{:02}:{:02}", hours, minutes, seconds)
        } else {
            format!("{}:{:02}", minutes, seconds)
        }
    }

    fn short_identity(value: &str) -> String {
        value.chars().take(12).collect()
    }

    fn js_error_message(error: &JsValue) -> String {
        Reflect::get(error, &JsValue::from_str("message"))
            .ok()
            .and_then(|message| message.as_string())
            .or_else(|| error.as_string())
            .unwrap_or_else(|| "unknown browser error".to_string())
    }

    fn install_hls_prefetch_lifecycle(
        player: &Element,
        weeb3: Arc<Weeb3>,
        normalized_owner: String,
        normalized_topic: String,
        sequence_zero_start_requested: bool,
        presentation_id: u64,
    ) {
        begin_hls_prefetch_session(
            weeb3,
            normalized_owner,
            normalized_topic,
            sequence_zero_start_requested,
            presentation_id,
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
            set_hls_prefetch_mode(HlsPrefetchMode::Sustained, false);
        });
        retain_media_element_callback(
            player,
            HLS_AUTOPLAY_AUTHORIZED_EVENT,
            autoplay_authorized_callback,
        );

        let explicit_pause_callback = Closure::<dyn FnMut()>::new(move || {
            set_hls_prefetch_mode(HlsPrefetchMode::Inactive, true);
        });
        retain_media_element_callback(player, HLS_EXPLICIT_PAUSE_EVENT, explicit_pause_callback);

        let play_player = player.clone();
        let play_callback = Closure::<dyn FnMut()>::new(move || {
            if play_player.get_attribute("data-weeb3-hls-mode").as_deref() == Some("hls.js") {
                // Rust's HLS callback classifies provisional autoplay separately
                // and emits HLS_AUTOPLAY_AUTHORIZED_EVENT only for real playback.
                return;
            }
            if play_player
                .get_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE)
                .as_deref()
                == Some("1")
            {
                // Await the autoplay promise. Chrome can emit `play` before
                // rejecting it, and that provisional event must not promote the
                // bounded warm-up to sustained retrieval.
                return;
            }
            play_player
                .set_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE, "1")
                .ok();
            set_hls_prefetch_mode(HlsPrefetchMode::Sustained, false);
        });
        retain_media_element_callback(player, "play", play_callback);

        let pause_player = player.clone();
        let pause_callback = Closure::<dyn FnMut()>::new(move || {
            if pause_player.get_attribute("data-weeb3-hls-mode").as_deref() == Some("hls.js") {
                // The HLS player emits HLS_EXPLICIT_PAUSE_EVENT only after playback
                // authorization, so a rejected autoplay pause cannot invalidate
                // the bounded warm-up generation.
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
                // A rejected autoplay can emit `pause` either before or after its
                // promise settles. In neither ordering was playback authorized, so
                // retain StartupOnly and its generation.
                return;
            }
            pause_player
                .remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
                .ok();
            // Close only future admission. A fragment/root request that was
            // already dispatched remains detached and drains through accounting.
            set_hls_prefetch_mode(HlsPrefetchMode::Inactive, true);
        });
        retain_media_element_callback(player, "pause", pause_callback);

        // Do not advance on the media element's generic `seeking` event. hls.js
        // emits it for its normal 0.1-second startup alignment and while stepping
        // across a temporary buffer gap; treating either as a user seek cancels
        // the exact fragment that is filling that gap. A real seek is detected
        // from the ordered manifest cursor in `hls_foreground_context` before its
        // target fragment enters retrieval.
    }

    fn install_hls_route_canonicalization(
        player: &Element,
        owner: &str,
        topic: &str,
        index_hint: Option<u64>,
        sequence_zero_start_requested: bool,
        view_generation: u64,
    ) {
        let owner = owner.to_ascii_lowercase();
        let topic = normalize_feed_topic(topic);
        let callback = Closure::<dyn FnMut()>::new(move || {
            if !result_view_request_is_current(view_generation) {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let route_base = streaming_route_base();
            let Ok(pathname) = window.location().pathname() else {
                return;
            };
            let Ok(current) = parse_stream_share_link(&pathname, &route_base) else {
                // Catalog/API opens must not rewrite an unrelated browser route.
                return;
            };
            let network = match active_profile().mode {
                NetworkMode::Mainnet => StreamShareNetwork::Mainnet,
                NetworkMode::Testnet => StreamShareNetwork::Testnet,
            };
            if current.network != network
                || current.owner != owner
                || normalize_feed_topic(&current.topic) != topic
                || current.index != index_hint
            {
                return;
            }

            // ENDLIST snapshots are immutable, so a proven feed index turns the
            // expensive unindexed UUID route into an exact one for refreshes and
            // address-bar sharing. Live routes retain no index.
            let effective_index = index_hint.or_else(|| {
                cached_finalized_feed_index(
                    &owner,
                    &topic,
                    sequence_zero_start_requested.then_some(view_generation),
                )
            });
            let Ok(canonical_route) =
                StreamShareRoute::new(network, &owner, &topic, effective_index)
            else {
                return;
            };
            let Ok(canonical_path) = build_stream_share_path(&route_base, &canonical_route) else {
                return;
            };
            if canonical_path == pathname {
                return;
            }
            if let Ok(history) = window.history() {
                let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&canonical_path));
            }
        });
        retain_media_element_callback(player, "weeb3-hls-manifest-ready", callback);
    }

    pub(crate) fn release_hls_view() {
        // Closing a view stops only future HLS admissions. Detached retrieval
        // leaders still own every dispatched request through accounting.
        invalidate_hls_prefetch_session(true);
        destroy_current_hls();
        STREAM_CATALOG_CALLBACKS.with(|callbacks| callbacks.borrow_mut().clear());
    }

    pub(crate) fn release_hls_for_bzz_view() {
        release_hls_view();
        HLS_PAYLOAD_CACHE.with(|cache| cache.borrow_mut().suspend_completed_retention());
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use runtime::{
    HlsOpenIntent, attach_hls_feed_media, hls_payload_cache_body_bytes, open_feed_view,
    open_hls_feed_view, release_hls_for_bzz_view, release_hls_view, try_fetch_response,
};
