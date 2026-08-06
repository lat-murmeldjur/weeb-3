use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::Arc,
};

use crate::{retrieval_conventions::next_nonzero_generation, stream_conventions::HlsStart};

const HLS_HEADER: &str = "#EXTM3U";
const HLS_ENDLIST: &str = "#EXT-X-ENDLIST";
const HLS_SERVER_CONTROL: &str = "#EXT-X-SERVER-CONTROL";
pub(crate) const HLS_LIVE_SYNC_SEGMENTS: usize = 8;

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

    pub(crate) fn install_with_early_overlap_limit(
        &mut self,
        mut references: Vec<String>,
        early_overlap_limit: usize,
        retain_tail: bool,
    ) {
        if retain_tail && references.len() > self.max_references {
            references.drain(..references.len() - self.max_references);
        } else {
            references.truncate(self.max_references);
        }
        if references.is_empty() {
            return;
        }

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

        self.next_plan_id = next_nonzero_generation(self.next_plan_id);
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

/// Pruning stops future admission while detached retrieval and accounting settle.
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
        .map(|line| line.trim_end_matches('\r').trim() == HLS_HEADER)
        .unwrap_or(false)
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

pub(crate) fn hls_manifest_reload_is_forward(current: &[u8], candidate: &[u8]) -> bool {
    let Some(current_sequence) = hls_media_sequence(current) else {
        return false;
    };
    let Some(candidate_sequence) = hls_media_sequence(candidate) else {
        return false;
    };
    let Some(current_len) =
        hls_segment_identities(current).and_then(|segments| u64::try_from(segments.len()).ok())
    else {
        return false;
    };
    let Some(candidate_len) =
        hls_segment_identities(candidate).and_then(|segments| u64::try_from(segments.len()).ok())
    else {
        return false;
    };
    let Some(current_end) = current_sequence.checked_add(current_len) else {
        return false;
    };
    let Some(candidate_end) = candidate_sequence.checked_add(candidate_len) else {
        return false;
    };

    candidate_sequence >= current_end && candidate_end > current_end
}

pub(crate) fn hls_live_tail(bytes: &[u8]) -> Option<(usize, f64)> {
    let segments = hls_segment_identities(bytes)?;
    if segments.is_empty() {
        return None;
    }

    let start = segments.len().saturating_sub(HLS_LIVE_SYNC_SEGMENTS);
    let mut duration = 0.0;
    for segment in &segments[start..] {
        let segment_duration = f64::from_bits(segment.duration_bits);
        if !segment_duration.is_finite() || segment_duration <= 0.0 {
            return None;
        }
        duration += segment_duration;
    }
    (duration.is_finite() && duration > 0.0).then_some((start, duration))
}

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
    if !stream_feed_payload_len_is_supported(merged.len()) {
        return None;
    }

    let mut expected = current_segments;
    expected.extend_from_slice(candidate_segments.get(appended_position..)?);
    (hls_media_sequence(&merged) == Some(0)
        && hls_segment_identities(&merged).as_ref() == Some(&expected))
    .then_some(merged)
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

pub(crate) fn append_hls_sequence_zero_archive_suffix(
    archive: &mut Vec<u8>,
    archive_segment_count: &mut u64,
    archive_media_end: &mut usize,
    current_source: &[u8],
    candidate: &[u8],
) -> Option<()> {
    if !hls_manifest_reload_is_continuous(current_source, candidate)
        || !hls_append_only_tags_are_supported(candidate)
        || !hls_has_at_most_one_endlist(candidate)
    {
        return None;
    }

    let candidate_sequence = hls_media_sequence(candidate)?;
    let candidate_segments = hls_segment_identities(candidate)?;
    let candidate_len = u64::try_from(candidate_segments.len()).ok()?;
    let candidate_end = candidate_sequence.checked_add(candidate_len)?;
    if candidate_sequence >= *archive_segment_count {
        return None;
    }
    if candidate_end <= *archive_segment_count {
        return Some(());
    }

    let uri_ends = hls_segment_uri_line_ends(candidate)?;
    if uri_ends.len() != candidate_segments.len() {
        return None;
    }
    let overlap_position = usize::try_from(
        archive_segment_count
            .checked_sub(1)?
            .checked_sub(candidate_sequence)?,
    )
    .ok()?;
    let suffix_start = *uri_ends.get(overlap_position)?;
    let candidate_media_end = *uri_ends.last()?;
    let suffix = candidate.get(suffix_start..)?;
    let insert_newline = !archive.get(..*archive_media_end)?.ends_with(b"\n") && !suffix.is_empty();
    let append_endlist = hls_is_finalized(candidate) && !hls_is_finalized(suffix);
    let endlist_len = if append_endlist {
        usize::from(!suffix.ends_with(b"\n"))
            .checked_add(HLS_ENDLIST.len())?
            .checked_add(1)?
    } else {
        0
    };
    let merged_len = archive_media_end
        .checked_add(usize::from(insert_newline))?
        .checked_add(suffix.len())?
        .checked_add(endlist_len)?;
    if !stream_feed_payload_len_is_supported(merged_len) {
        return None;
    }

    archive.truncate(*archive_media_end);
    if insert_newline {
        archive.push(b'\n');
    }
    let appended_at = archive.len();
    archive.extend_from_slice(suffix);
    *archive_media_end = appended_at.checked_add(candidate_media_end.checked_sub(suffix_start)?)?;
    if append_endlist {
        if !archive.ends_with(b"\n") {
            archive.push(b'\n');
        }
        archive.extend_from_slice(HLS_ENDLIST.as_bytes());
        archive.push(b'\n');
    }
    *archive_segment_count = candidate_end;
    Some(())
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
    let bootstrap = hls_segment_identities(bootstrap_manifest)?
        .into_iter()
        .rev()
        .find(|candidate| {
            !current
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
            "\n#EXTINF:{},\n{}",
            f64::from_bits(segment.duration_bits),
            segment.reference
        ));
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
        if trimmed == HLS_HEADER {
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
        hls_live_tail(bytes).map(|(_, duration)| duration)
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
            rewrote_playlist_type || (!rewrite_vod_as_event && line.trim() == HLS_HEADER);
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
pub(crate) const HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT: usize = 64;
pub(crate) const HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL: usize = 4;
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
    initial_check_token: Option<u64>,
    checking_token: u64,
) -> bool {
    (confirmed_head_index == Some(snapshot_index)
        && last_head_check_ms.is_finite()
        && last_head_check_ms > checked_after_ms)
        || initial_check_token.is_some_and(|token| checking_token != token)
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
    recovered_missing_index: bool,
) -> bool {
    saw_tentative_endlist
        || (mode == FeedFollowupMode::Canonical
            && (successes >= FEED_FOLLOWUP_BATCH_LIMIT || recovered_missing_index))
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
mod player {
    //! Rust owns HLS policy; `hls.js` supplies MSE playback through dynamic import.

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

    use super::{
        HLS_LIVE_SYNC_SEGMENTS, classify_hls_level_transition, hls_timeline_rebase_position,
    };

    const SWARM_REQUEST_TIMEOUT_MS: f64 = 240_000.0;
    const MAX_NETWORK_RECOVERY_ATTEMPTS: u8 = 2;
    const MAX_HARD_RESTART_ATTEMPTS: u8 = 2;
    const HLS_BEGINNING_AUTOPLAY_HEAD_START: Duration = Duration::from_millis(500);
    const HLS_WARMUP_STOP_DELAY: Duration = Duration::from_millis(500);

    const HLS_ERROR_EVENT: &str = "hlsError";
    const HLS_BUFFER_CREATED_EVENT: &str = "hlsBufferCreated";
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
        autoplay_allowed: bool,
        codec_bootstrap_pending: bool,
        codec_bootstrap_required: bool,
        hard_restart_attempts: u8,
        initial_start_position: f64,
        level_snapshots: HashMap<u64, (u64, bool, Option<f64>)>,
        load_started: bool,
        manifest_parsed: bool,
        media_recovery_attempts: u8,
        network_recovery_attempts: u8,
        playback_authorized: bool,
        recovery_pending: bool,
        resume_authorized_playback: bool,
        rebase_origin: Option<(f64, f64)>,
        rebase_position: Option<f64>,
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

        fn restart_position(&self) -> f64 {
            if self.codec_bootstrap_pending {
                return 0.0;
            }
            if !self.playback_authorized {
                return self.initial_start_position;
            }
            let current_position = self.media.current_time();
            if current_position.is_finite() && current_position > 0.0 {
                current_position
            } else {
                self.initial_start_position
            }
        }

        fn dispose(mut self) {
            quarantine_dom_callbacks(&self.media, &mut self.dom_callbacks);

            match &self.mode {
                PlayerMode::Hls(hls) => {
                    // Player destruction cannot cancel requests already dispatched to Rust.
                    destroy_hls_and_quarantine_callbacks(hls, &mut self.hls_callbacks);
                }
                PlayerMode::Native => {
                    // Clearing src stops future native requests, not dispatched Rust work.
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

    pub(crate) async fn play_hls(
        player: &Element,
        source: &str,
        hls_loader: JsFuture,
        initial_start_position: f64,
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
        start_hls_request(
            media,
            source.to_string(),
            Some(hls_loader),
            initial_start_position,
            0,
            false,
            false,
            true,
            None,
        )
        .await
    }

    async fn start_hls_request(
        media: HtmlMediaElement,
        source: String,
        hls_loader: Option<JsFuture>,
        initial_start_position: f64,
        hard_restart_attempts: u8,
        timeline_rebase_attempted: bool,
        resume_authorized_playback: bool,
        autoplay_allowed: bool,
        rebase_origin: Option<(f64, f64)>,
    ) -> Result<&'static str, JsValue> {
        let resume_authorized_playback = resume_authorized_playback
            || media
                .get_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
                .as_deref()
                == Some("1");
        let session_id = begin_player_request();
        media.remove_attribute("data-weeb3-hls-mode").ok();
        media.remove_attribute("data-weeb3-hls-state").ok();
        media.remove_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE).ok();
        media
            .remove_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE)
            .ok();

        let native_supported = supports_native_hls(&media);
        let hls_class = match match hls_loader {
            Some(loader) => loader.await,
            None => JsFuture::from(load_hls()).await,
        } {
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
                initial_start_position,
                autoplay_allowed,
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
        let codec_bootstrap_required = source.contains("&codec-bootstrap=");
        let codec_bootstrap_pending = codec_bootstrap_required && initial_start_position == 0.0;
        if codec_bootstrap_pending {
            let _ = media.pause();
        }
        let config = hls_config(!source.contains("start=live"));
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
                autoplay_allowed,
                codec_bootstrap_pending,
                codec_bootstrap_required,
                hard_restart_attempts,
                initial_start_position,
                level_snapshots: HashMap::new(),
                load_started: false,
                manifest_parsed: false,
                media_recovery_attempts: 0,
                network_recovery_attempts: 0,
                playback_authorized: false,
                recovery_pending: false,
                resume_authorized_playback,
                rebase_origin,
                rebase_position: None,
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
        initial_start_position: f64,
        autoplay_allowed: bool,
        hard_restart_attempts: u8,
        timeline_rebase_attempted: bool,
    ) -> Result<(), JsValue> {
        media.set_attribute("data-weeb3-hls-mode", "native")?;
        let mut dom_callbacks = Vec::with_capacity(1);
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

        CURRENT_SESSION.with(|current| {
            *current.borrow_mut() = Some(HlsSession {
                id: session_id,
                media: media.clone(),
                source: source.clone(),
                mode: PlayerMode::Native,
                dom_callbacks,
                hls_callbacks: Vec::new(),
                autoplay_pending: false,
                autoplay_allowed,
                codec_bootstrap_pending: false,
                codec_bootstrap_required: false,
                hard_restart_attempts,
                initial_start_position,
                level_snapshots: HashMap::new(),
                load_started: true,
                manifest_parsed: true,
                media_recovery_attempts: 0,
                network_recovery_attempts: 0,
                playback_authorized: false,
                recovery_pending: false,
                resume_authorized_playback: false,
                rebase_origin: None,
                rebase_position: None,
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
        let mut retained = Vec::with_capacity(5);

        let error = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |_, data| {
            handle_hls_error(session_id, data);
        });
        register_hls_callback(hls, HLS_ERROR_EVENT, error, &mut retained)?;

        let buffer_created = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |_, data| {
            let video_track =
                js_property(&data, "tracks").and_then(|tracks| js_property(&tracks, "video"));
            let Some(video_track) = video_track else {
                return;
            };
            if let Some(resolution) = js_video_resolution(&video_track) {
                super::set_hls_video_resolution(Some(resolution));
            }
            let seek = with_session_mut(session_id, |session| {
                if !session.codec_bootstrap_pending {
                    return None;
                }
                let hls = session.hls().cloned()?;
                let target = Reflect::get(hls.as_ref(), &JsValue::from_str("liveSyncPosition"))
                    .ok()?
                    .as_f64()
                    .filter(|target| target.is_finite() && *target >= 0.0)?;
                Some((session.media.clone(), session.source.clone(), target))
            })
            .flatten();
            if let Some((media, source, target)) = seek {
                super::runtime::finish_hls_codec_bootstrap(&source);
                let clean_source = source
                    .split("&codec-bootstrap=")
                    .next()
                    .unwrap_or(&source)
                    .to_string();
                media.set_current_time(target);
                let ready = with_session_mut(session_id, |session| {
                    if !session.codec_bootstrap_pending {
                        return false;
                    }
                    session.source = clean_source;
                    session.codec_bootstrap_pending = false;
                    session.codec_bootstrap_required = false;
                    session.initial_start_position = -1.0;
                    true
                })
                .unwrap_or(false);
                if ready {
                    spawn_local(async move {
                        let _ = JsFuture::from(Promise::resolve(&JsValue::UNDEFINED)).await;
                        if session_is_current(session_id) {
                            resume_authorized_hls_playback(session_id);
                            maybe_autoplay(session_id);
                        }
                    });
                }
            }
        });
        register_hls_callback(hls, HLS_BUFFER_CREATED_EVENT, buffer_created, &mut retained)?;

        let fragment_buffered = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |_, _| {
            let action = with_session_mut(session_id, |session| {
                session.network_recovery_attempts = 0;
                session.media_recovery_attempts = 0;
                let stop_warmup = session.warmup_active
                    && session.media.paused()
                    && !session.codec_bootstrap_pending;
                (session.media.clone(), stop_warmup)
            });
            if let Some((media, stop_warmup)) = action {
                media.set_attribute("data-weeb3-hls-state", "buffered").ok();
                set_playback_status(&media, "HLS media buffered through weeb-3.", "buffered");
                if stop_warmup {
                    spawn_local(async move {
                        sleep(HLS_WARMUP_STOP_DELAY).await;
                        let warmup_hls = with_session_mut(session_id, |session| {
                            if !session.warmup_active || !session.media.paused() {
                                return None;
                            }
                            session.warmup_active = false;
                            session.hls().cloned()
                        })
                        .flatten();
                        if let Some(hls) = warmup_hls
                            && let Err(error) = hls.stop_load()
                        {
                            stop_with_error(
                                session_id,
                                "Could not stop bounded HLS warm-up",
                                error,
                            );
                        }
                    });
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
                let rebase_ready =
                    session.rebase_origin.is_none() || session.rebase_position.is_some();
                let startup_hls = if !session.load_started
                    && rebase_ready
                    && (resume_authorized || session.media.paused())
                {
                    let start_position = session
                        .rebase_position
                        .take()
                        .unwrap_or(session.initial_start_position);
                    session.load_started = true;
                    session.warmup_active = true;
                    session
                        .hls()
                        .cloned()
                        .map(|hls| (hls, resume_authorized, start_position))
                } else {
                    None
                };
                let beginning_autoplay_head_start =
                    startup_hls
                        .as_ref()
                        .is_some_and(|(_, resume_authorized, start_position)| {
                            !resume_authorized
                                && *start_position == 0.0
                                && !session.source.contains("start=live")
                        });
                (
                    session.media.clone(),
                    startup_hls,
                    beginning_autoplay_head_start,
                )
            });
            if let Some((media, startup_hls, beginning_autoplay_head_start)) = ready {
                media
                    .set_attribute("data-weeb3-hls-state", "manifest-ready")
                    .ok();
                set_playback_status(
                    &media,
                    "HLS manifest ready through weeb-3. Press Play if autoplay is blocked.",
                    "manifest-ready",
                );
                if let Some((hls, resume_authorized, start_position)) = startup_hls {
                    dispatch_custom_event(&media, HLS_WARMUP_START_EVENT, &JsValue::UNDEFINED);
                    if !session_is_current(session_id) {
                        return;
                    }
                    if let Err(error) = hls.start_load_at(start_position) {
                        schedule_hard_restart(session_id, error);
                        return;
                    }
                    if resume_authorized {
                        resume_authorized_hls_playback(session_id);
                    }
                }
                if beginning_autoplay_head_start {
                    spawn_local(async move {
                        sleep(HLS_BEGINNING_AUTOPLAY_HEAD_START).await;
                        if session_is_current(session_id) {
                            maybe_autoplay(session_id);
                        }
                    });
                } else {
                    maybe_autoplay(session_id);
                }
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
        let edge = js_property(&details, "edge")
            .and_then(|value| value.as_f64())
            .filter(|value| value.is_finite() && *value >= 0.0);

        let rebase_start = with_session_mut(session_id, |session| {
            if session.rebase_position.is_none()
                && let Some((previous_edge, current_time)) = session.rebase_origin
                && let Some(edge) = edge
                && let Some(position) =
                    hls_timeline_rebase_position(previous_edge, current_time, edge)
            {
                session.rebase_origin = None;
                session.rebase_position = Some(position);
            }
            if !session.manifest_parsed || session.load_started {
                return None;
            }
            let position = session.rebase_position.take()?;
            session.load_started = true;
            session.warmup_active = true;
            session.hls().cloned().map(|hls| {
                (
                    session.media.clone(),
                    hls,
                    session.resume_authorized_playback,
                    position,
                )
            })
        })
        .flatten();

        let outcome = with_session_mut(session_id, |session| {
            let previous = session
                .level_snapshots
                .insert(level, (start_sequence, live, edge));
            let transition = classify_hls_level_transition(
                previous.map(|(start_sequence, live, _)| (start_sequence, live)),
                session.timeline_rebase_attempted,
                start_sequence,
            );

            if !transition.rebase {
                return None;
            }

            session.timeline_rebase_attempted = true;
            session.recovery_pending = true;
            let rebase_origin = previous
                .and_then(|(_, _, edge)| edge)
                .map(|edge| (edge, session.media.current_time()));
            Some((
                session.media.clone(),
                session.source.clone(),
                session.hard_restart_attempts,
                session.initial_start_position,
                session.playback_authorized,
                session.autoplay_allowed,
                session.hls().cloned(),
                rebase_origin,
            ))
        });
        if let Some((media, hls, resume_authorized, position)) = rebase_start {
            dispatch_custom_event(&media, HLS_WARMUP_START_EVENT, &JsValue::UNDEFINED);
            if !session_is_current(session_id) {
                return;
            }
            if let Err(error) = hls.start_load_at(position) {
                schedule_hard_restart(session_id, error);
                return;
            }
            if resume_authorized {
                resume_authorized_hls_playback(session_id);
            }
            maybe_autoplay(session_id);
        }
        let Some(restart) = outcome else {
            return;
        };
        let Some((
            media,
            source,
            hard_restart_attempts,
            initial_start_position,
            resume_authorized,
            autoplay_allowed,
            retiring_hls,
            rebase_origin,
        )) = restart
        else {
            return;
        };

        media
            .set_attribute("data-weeb3-hls-state", "rebasing-timeline")
            .ok();
        set_playback_status(
            &media,
            "Complete HLS archive found; rebuilding its finalized timeline.",
            "rebasing-timeline",
        );
        dispatch_custom_event(&media, HLS_TIMELINE_REBASE_EVENT, &JsValue::UNDEFINED);
        // Stop admission before the retiring LEVEL_LOADED callback can dispatch again.
        if let Some(hls) = retiring_hls {
            let _ = hls.stop_load();
        }

        // Defer destruction until its callback returns; dispatched Rust work still settles.
        spawn_local(async move {
            let _ = JsFuture::from(Promise::resolve(&JsValue::UNDEFINED)).await;
            if !session_is_current(session_id) {
                return;
            }
            if let Err(error) = start_hls_request(
                media.clone(),
                source,
                None,
                initial_start_position,
                hard_restart_attempts,
                true,
                resume_authorized,
                autoplay_allowed,
                rebase_origin,
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
            // JS may still retain these wasm closures after a failed destroy.
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
            // Preserve closures for listeners the DOM refused to remove.
            std::mem::forget(quarantined);
        }
    }

    fn register_hls_dom_callbacks(
        media: &HtmlMediaElement,
        session_id: u64,
    ) -> Result<Vec<DomCallback>, JsValue> {
        let mut retained = Vec::with_capacity(3);
        let play = Closure::<dyn FnMut(Event)>::new(move |_| {
            let action = with_session_mut(session_id, |session| {
                let autoplay_pending = session.autoplay_pending;
                let first_load = !session.load_started;
                let resume_load = !first_load && !session.warmup_active;
                let initial_start_position = session.initial_start_position;
                session.load_started = true;
                if !autoplay_pending {
                    session.playback_authorized = true;
                    session.autoplay_allowed = true;
                    session.warmup_active = false;
                }
                (
                    session.hls().cloned(),
                    first_load,
                    resume_load,
                    initial_start_position,
                    autoplay_pending,
                    session.media.clone(),
                )
            });
            if let Some((
                hls,
                first_load,
                resume_load,
                initial_start_position,
                autoplay_pending,
                media,
            )) = action
            {
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
                        Some(hls.start_load_at(initial_start_position))
                    } else if resume_load {
                        Some(hls.start_load())
                    } else {
                        None
                    };
                    if let Some(Err(error)) = result {
                        schedule_hard_restart(session_id, error);
                    }
                }
            }
        });
        register_dom_callback(media, "play", play, &mut retained)?;

        let pause = Closure::<dyn FnMut(Event)>::new(move |_| {
            let action = with_session_mut(session_id, |session| {
                // Chrome may emit play/pause before rejecting autoplay.
                if !session.playback_authorized {
                    return None;
                }
                session.playback_authorized = false;
                session.autoplay_allowed = false;
                session.resume_authorized_playback = false;
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

        let resize = Closure::<dyn FnMut(Event)>::new(move |_| {
            let resolution = with_session_mut(session_id, |session| {
                js_video_resolution(session.media.as_ref())
            })
            .flatten();
            if let Some(resolution) = resolution {
                super::set_hls_video_resolution(Some(resolution));
            }
        });
        register_dom_callback(media, "resize", resize, &mut retained)?;

        Ok(retained)
    }

    fn handle_hls_error(session_id: u64, data: JsValue) {
        let codec_bootstrap = js_string_property(&data, "type").as_deref() == Some(HLS_MEDIA_ERROR)
            && js_string_property(&data, "details").as_deref() == Some("bufferAppendError")
            && js_string_property(&data, "sourceBufferName").as_deref() == Some("video")
            && js_property(&data, "error")
                .is_some_and(|error| js_error_message(&error).contains("video SourceBuffer"))
            && with_session_mut(session_id, |session| {
                if session.codec_bootstrap_pending
                    || !session.source.contains("start=live")
                    || session.hard_restart_attempts >= MAX_HARD_RESTART_ATTEMPTS
                {
                    return false;
                }
                session.codec_bootstrap_pending = true;
                session.codec_bootstrap_required = true;
                session.initial_start_position = 0.0;
                true
            })
            .unwrap_or(false);
        if codec_bootstrap {
            schedule_hard_restart(session_id, data);
            return;
        }

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
            let recovery = with_session_mut(session_id, |session| {
                session.recovery_pending = false;
                if session.warmup_active || !session.media.paused() {
                    let restart_position = session.restart_position();
                    return session.hls().cloned().map(|hls| (hls, restart_position));
                }
                None
            })
            .flatten();
            if let Some((hls, restart_position)) = recovery {
                if let Err(error) = hls.start_load_at(restart_position) {
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
            if session.codec_bootstrap_required {
                session.codec_bootstrap_pending = true;
            }
            let source = if session.codec_bootstrap_required {
                let source = session
                    .source
                    .split("&codec-bootstrap=")
                    .next()
                    .unwrap_or(&session.source);
                format!("{source}&codec-bootstrap={}", session.id)
            } else {
                session.source.clone()
            };
            let codec_bootstrap_restart =
                session.codec_bootstrap_required && session.hard_restart_attempts == 0;
            Some(Some((
                session.media.clone(),
                source,
                session.hard_restart_attempts.saturating_add(1),
                session.timeline_rebase_attempted,
                codec_bootstrap_restart,
            )))
        })
        .flatten();

        match restart {
            Some(Some((
                media,
                source,
                attempt,
                timeline_rebase_attempted,
                codec_bootstrap_restart,
            ))) => {
                spawn_local(async move {
                    if codec_bootstrap_restart {
                        let _ = JsFuture::from(Promise::resolve(&JsValue::UNDEFINED)).await;
                    } else {
                        sleep(Duration::from_millis(1_000)).await;
                    }
                    let Some((initial_start_position, resume_authorized, autoplay_allowed)) =
                        with_session_mut(session_id, |session| {
                            (
                                session.restart_position(),
                                session.playback_authorized || session.resume_authorized_playback,
                                session.autoplay_allowed,
                            )
                        })
                    else {
                        return;
                    };
                    if let Err(error) = start_hls_request(
                        media.clone(),
                        source,
                        None,
                        initial_start_position,
                        attempt,
                        timeline_rebase_attempted,
                        resume_authorized,
                        autoplay_allowed,
                        None,
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

        // Drop the retained callback only after it returns.
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
            ((!require_autoplay || (session.autoplay_allowed && session.media.autoplay()))
                && !session.codec_bootstrap_pending
                && !session.playback_authorized
                && !session.autoplay_pending)
                .then(|| {
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

        spawn_local(async move {
            let promise = match media.play() {
                Ok(promise) => promise,
                Err(error) => {
                    settle_autoplay(session_id, false);
                    report_autoplay_blocked(&media, &error);
                    return;
                }
            };
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

    fn report_autoplay_blocked(media: &HtmlMediaElement, _error: &JsValue) {
        media
            .set_attribute("data-weeb3-hls-state", "autoplay-blocked")
            .ok();
        set_playback_status(
            media,
            "HLS startup media is warming. Autoplay was blocked; press Play to start playback.",
            "autoplay-blocked",
        );
    }

    fn report_playback_error(media: &HtmlMediaElement, message: &str, detail: &JsValue) {
        let error = Error::new(message);
        let _ = Reflect::set(error.as_ref(), &JsValue::from_str("cause"), detail);
        let error: JsValue = error.into();
        web_sys::console::error_2(&JsValue::from_str("weeb-3 HLS playback error"), &error);
        set_playback_status(media, &format!("HLS playback failed: {message}"), "error");
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

    fn hls_config(progressive: bool) -> Object {
        let config = Object::new();
        set_property(&config, "enableWorker", JsValue::TRUE);
        set_property(&config, "autoStartLoad", JsValue::FALSE);
        set_property(&config, "startFragPrefetch", JsValue::FALSE);
        set_property(&config, "progressive", JsValue::from_bool(progressive));

        for name in [
            "manifestLoadPolicy",
            "playlistLoadPolicy",
            "fragLoadPolicy",
            "keyLoadPolicy",
        ] {
            set_property(&config, name, swarm_load_policy().into());
        }

        if !progressive {
            set_property(
                &config,
                "liveSyncDurationCount",
                JsValue::from_f64(HLS_LIVE_SYNC_SEGMENTS as f64),
            );
        }
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

    fn js_video_resolution(target: &JsValue) -> Option<(u32, u32)> {
        for candidate in js_property(target, "metadata")
            .into_iter()
            .chain(std::iter::once(target.clone()))
        {
            for (width_name, height_name) in [("width", "height"), ("videoWidth", "videoHeight")] {
                let Some(width) = js_safe_u64_property(&candidate, width_name)
                    .and_then(|width| u32::try_from(width).ok())
                else {
                    continue;
                };
                let Some(height) = js_safe_u64_property(&candidate, height_name)
                    .and_then(|height| u32::try_from(height).ok())
                else {
                    continue;
                };
                if width > 0 && height > 0 {
                    return Some((width, height));
                }
            }
        }
        None
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
    destroy_current_hls, load_hls, play_hls,
};

#[cfg(target_arch = "wasm32")]
mod runtime {
    use super::*;
    use std::{
        cell::RefCell,
        collections::{HashMap, HashSet, VecDeque},
        future::Future,
        rc::Rc,
        time::Duration,
    };

    use async_std::sync::Arc;
    use js_sys::{Function, Reflect};
    use libp2p::futures::future::{Either, join, select};
    use libp2p::futures::stream::{self, FuturesUnordered, StreamExt};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{Element, HtmlMediaElement};

    use crate::{
        Weeb3,
        bzz_stream::{
            RawFeedPayload, acquire_latest_raw_feed_payload_bounded_from,
            acquire_latest_raw_feed_payload_from, acquire_latest_raw_feed_payload_startup,
            acquire_raw_feed_payload_at_index, acquire_raw_feed_payload_at_index_bounded,
        },
        feed::FEED_FRONTIER_LOOKAHEAD_TIMEOUT,
        interface::{service_worker_controls_bzz_requests, service_worker_scope_protocol_error},
        mpsc,
        network_profile::active_profile,
        normalize_feed_topic, register_retrieve_cancel_token,
        retrieval::{
            retrieve_data_payload, retrieve_data_payload_cancellable,
            retrieve_data_range_join_cancellable, retrieve_decoded_data_root,
        },
        retrieval_conventions::{PendingGenerationRelation, pending_generation_relation},
        stream::{
            FetchResponse, begin_result_view_request, clear_completed_bzz_media_ranges,
            media_cache_max_bytes, next_media_generation, range_cache_body_bytes,
            replace_stream_result_view, result_view_request_is_current,
            retain_media_element_callback,
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
                hls_segment_progress_detail(&address, Some(bytes.len())),
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
                hls_segment_progress_detail(&address, Some(bytes.len())),
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
            stream_generation: Option<u64>,
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

            let cancel = stream_generation.and_then(|generation| {
                stream_retrieve_cancel_token(HLS_STREAM_KEY.to_string(), generation)
            });
            register_retrieve_cancel_token(&self.retrieve_cancel_generations, &cancel).await;
            let cancel_generations = cancel
                .as_ref()
                .map(|_| self.retrieve_cancel_generations.clone());
            let bytes = retrieve_data_range_join_cancellable(
                &reference,
                start,
                end_inclusive,
                &self.chunk_port.0,
                cancel_generations,
                cancel,
            )
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

        async fn latest_hls_feed_payload_from(
            &self,
            owner: String,
            topic: String,
            initial: RawFeedPayload,
        ) -> Option<RawFeedPayload> {
            acquire_latest_raw_feed_payload_from(owner, topic, initial, &self.chunk_port.0).await
        }

        async fn latest_hls_feed_payload_startup(
            &self,
            owner: String,
            topic: String,
            early_payloads: Option<mpsc::Sender<RawFeedPayload>>,
            early_payload_max_index: Option<u64>,
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
            let result = acquire_latest_raw_feed_payload_startup(
                owner,
                topic,
                &self.chunk_port.0,
                early_payloads,
                early_payload_max_index,
            )
            .await;
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
        static HLS_CODEC_BOOTSTRAP_PRESENTATION: RefCell<Option<HlsCodecBootstrapPresentation>> =
            const { RefCell::new(None) };
        static HLS_PAYLOAD_DURATIONS: RefCell<HashMap<String, f64>> = RefCell::new(HashMap::new());
        static HLS_PAYLOAD_SIZES: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
        static HLS_PAYLOAD_SIZE_PROBES: RefCell<HashMap<String, Vec<mpsc::Sender<Option<u64>>>>> =
            RefCell::new(HashMap::new());
        static HLS_PAYLOAD_CACHE: RefCell<HlsPayloadCache> =
            RefCell::new(HlsPayloadCache::new());
    }

    const FEED_ROUTE_CACHE_MAX_ENTRIES: usize = 64;
    const FEED_ROUTE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
    const HLS_TERMINAL_CONFIRMATION_MIN_DELAY: Duration = Duration::from_secs(3);
    const HLS_TERMINAL_CONFIRMATION_POLL_INTERVAL: Duration = Duration::from_millis(500);
    const HLS_TERMINAL_CONFIRMATION_MAX_POLLS: usize = 18;
    const HLS_LIVE_FRONTIER_MAX_WAIT: Duration = Duration::from_secs(15);
    const HLS_LIVE_FRONTIER_CONNECTION_WAIT: Duration = Duration::from_secs(7);
    const HLS_LIVE_FRONTIER_MIN_PRICED_PEERS: u64 = 1;
    const HLS_ASSET_METADATA_CACHE_MAX_ENTRIES: usize = 1024;
    const HLS_ASSET_PROBE_BYTES: u64 = 512;
    const HLS_REPRESENTATION_VERSION: &str = "weeb3-hls-v2";
    const HLS_MEDIA_PLAN_MAX_REFERENCES: usize = 4096;
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
    const HLS_EARLY_FEED_PREFIX_PREFERRED_SEGMENTS: usize = 8;
    const HLS_EARLY_FEED_PREFIX_INDEX: u64 = 7;
    const HLS_STARTUP_PREFIX_RESULT_GRACE: Duration = Duration::from_secs(1);
    const HLS_SEQUENCE_ZERO_CANONICAL_START_GRACE: Duration = Duration::from_secs(1);
    const HLS_EXACT_NEXT_HEAD_START: Duration = Duration::from_secs(1);
    const HLS_NEXT_RESERVE_STAGGER: Duration = Duration::from_secs(1);
    const HLS_SEQUENCE_ZERO_CANONICAL_EXCLUSIVITY: Duration = Duration::from_secs(10);
    const HLS_EXACT_OVERLAP_ADMISSION_BUDGET: Duration = Duration::from_secs(30);
    const HLS_INITIAL_RESPONSE_BUDGET_MS: f64 = 15_000.0;
    const HLS_PAYLOAD_RETRY_DELAY_MS: u64 = 75;
    const HLS_PAYLOAD_SIZE_RETRY_DELAY_MS: u64 = 250;
    const HLS_STARTUP_LOOKAHEAD_BYTES: u64 = 3 * MEDIA_STARTUP_RESPONSE_BYTES;
    const HLS_PROGRESSIVE_SUCCESSOR_PREFIX_BYTES: u64 = MEDIA_STORAGE_WINDOW_BYTES * 3;

    fn hls_segment_progress_detail(reference: &str, size: Option<usize>) -> String {
        let mut detail = size.map_or_else(
            || "starting".to_string(),
            |size| format!("size {:.2} MB", size as f64 / 1_000_000.0),
        );
        if let Some(duration) = HLS_PAYLOAD_DURATIONS.with(|durations| {
            durations
                .borrow()
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
        Continuation,
    }

    struct HlsCodecBootstrapPresentation {
        token: u64,
        complete: bool,
        snapshot: Option<FeedRouteSnapshot>,
    }

    struct FeedRouteState {
        snapshot: FeedRouteSnapshot,
        source_body: Arc<[u8]>,
        body_tracks_source: bool,
        source_endlist_confirmed: bool,
        canonical_stabilization_started: bool,
        canonical_stabilization_running: bool,
        checking_token: u64,
        confirmed_head_index: Option<u64>,
        last_head_check: f64,
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
        progressive_start: Option<(String, Option<String>)>,
        progressive_successor_prefix_ready: bool,
        sequence_zero_runway_closed: bool,
        presentation_id: u64,
        live_start: bool,
        live_history_active: bool,
        timeline_rebasing: bool,
        startup_deadline_ms: f64,
        completed_media_payloads: usize,
        startup_overlap_plans: HashSet<u64>,
        tracks: HashMap<u64, HlsPrefetchTrack>,
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
                progressive_start: None,
                progressive_successor_prefix_ready: false,
                sequence_zero_runway_closed: false,
                presentation_id: 0,
                live_start: false,
                live_history_active: false,
                timeline_rebasing: false,
                startup_deadline_ms: 0.0,
                completed_media_payloads: 0,
                startup_overlap_plans: HashSet::new(),
                tracks: HashMap::new(),
            }
        }

        fn advance_generation(&mut self) -> u64 {
            self.generation = next_media_generation();
            self.progressive_start = None;
            self.progressive_successor_prefix_ready = false;
            self.completed_media_payloads = if self.live_start {
                HLS_SERIAL_PREFETCH_COMPLETIONS.saturating_sub(1)
            } else {
                0
            };
            for track in self.tracks.values_mut() {
                track.running_generation = None;
            }
            self.generation
        }

        fn advance_timeline(&mut self) -> u64 {
            self.timeline_epoch = next_nonzero_generation(self.timeline_epoch);
            self.progressive_start = None;
            self.progressive_successor_prefix_ready = false;
            self.timeline_epoch
        }
    }

    #[derive(Clone)]
    struct HlsForegroundContext {
        generation: u64,
        timeline_epoch: u64,
        schedule_id: Option<u64>,
        cursor: Option<HlsMediaCursor>,
        progressive_successor_prefix_ready: bool,
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
            prefetch_limit: usize,
        ) -> HlsPayloadLoadRole {
            let cached = if prefetch {
                self.body(reference)
            } else {
                read_forward_cache_entry(&mut self.order, &self.bodies, reference)
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

            let active_loads = self
                .pending
                .values()
                .filter(|pending| pending.generation == generation)
                .count();
            if prefetch && active_loads >= prefetch_limit {
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
        ) -> bool {
            let owns_pending = self.pending.get(reference).is_some_and(|pending| {
                pending.generation == generation && pending.load_id == load_id
            });

            if self.retain_completed
                && let Ok(body) = &result
            {
                self.remember(reference.to_string(), body.clone(), hot);
            }
            if !owns_pending {
                return false;
            }
            if let Some(pending) = self.pending.remove(reference) {
                pending.finish(result);
            }
            true
        }

        fn body(&mut self, reference: &str) -> Option<Arc<[u8]>> {
            let body = self.bodies.get(reference).cloned()?;
            self.order.retain(|key| key != reference);
            self.order.push_back(reference.to_string());
            Some(body)
        }

        fn body_size(&self, reference: &str) -> Option<u64> {
            self.bodies
                .get(reference)
                .and_then(|body| u64::try_from(body.len()).ok())
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
        HLS_PAYLOAD_CACHE.with(|cache| cache.borrow().body_bytes)
    }

    fn hls_payload_cache_capacity_bytes() -> u64 {
        media_cache_max_bytes().saturating_sub(range_cache_body_bytes())
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
            range = None;
        }
        let progressive_generation = HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            (session.generation != 0
                && !session.live_start
                && !session.timeline_rebasing
                && session
                    .progressive_start
                    .as_ref()
                    .is_some_and(|start| start.0 == reference))
            .then_some(session.generation)
        });
        let progressive_start = progressive_generation.is_some();
        if method != "HEAD" && range.is_none() && progressive_start {
            let Some(resolved) = resolve_hls_asset(weeb3.clone(), reference.clone()).await else {
                return FetchResponse::error(503, "weeb-3 did not retrieve resource");
            };
            if !resolved.metadata.is_manifest && resolved.prefetched_body.is_none() {
                let mut headers = hls_bytes_headers(&reference, resolved.metadata.mime);
                headers.push((
                    "Content-Length".to_string(),
                    resolved.metadata.payload_size.to_string(),
                ));
                headers.push(("X-Weeb3-Stream-Start".to_string(), "1".to_string()));
                return FetchResponse::stream(200, headers);
            }
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

        if method != "HEAD" && !progressive_start {
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
        let expected_len = end
            .checked_sub(start)
            .and_then(|length| length.checked_add(1))
            .and_then(|length| usize::try_from(length).ok());
        let bytes = if let Some(body) = resolved.prefetched_body {
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
            selected.to_vec()
        } else {
            let mut bytes = Vec::new();
            for attempt in 0..HLS_FOREGROUND_MAX_ATTEMPTS {
                bytes = weeb3
                    .retrieve_hls_payload_range(
                        reference.clone(),
                        start,
                        end,
                        progressive_generation,
                    )
                    .await;
                if expected_len.is_some_and(|expected| bytes.len() == expected) {
                    break;
                }
                if attempt.saturating_add(1) < HLS_FOREGROUND_MAX_ATTEMPTS {
                    async_std::task::sleep(Duration::from_millis(
                        HLS_PAYLOAD_RETRY_DELAY_MS.saturating_mul(attempt.saturating_add(1) as u64),
                    ))
                    .await;
                }
            }
            bytes
        };
        if !expected_len.is_some_and(|expected| bytes.len() == expected) {
            return FetchResponse::error(502, "weeb-3 returned a short HLS byte range");
        }
        if (end.saturating_add(1) >= MEDIA_STORAGE_WINDOW_BYTES.saturating_mul(2)
            || end.saturating_add(1) == size)
            && let Some((successor, generation, timeline_epoch)) =
                take_hls_progressive_successor(&reference)
        {
            let successor_client = weeb3.clone();
            let source_reference = reference.clone();
            spawn_local(async move {
                async_std::task::sleep(Duration::from_millis(MEDIA_PREFETCH_BATCH_YIELD_MS)).await;
                if !hls_progressive_successor_admission_is_current(
                    &source_reference,
                    generation,
                    timeline_epoch,
                ) {
                    return;
                }
                let size =
                    start_hls_payload_size_probe(successor_client.clone(), successor.clone())
                        .recv()
                        .await
                        .ok()
                        .flatten();
                let Some(size) = size.filter(|size| *size > 0) else {
                    return;
                };
                if !hls_progressive_successor_admission_is_current(
                    &source_reference,
                    generation,
                    timeline_epoch,
                ) {
                    return;
                }
                let prefix_end = size
                    .saturating_sub(1)
                    .min(HLS_PROGRESSIVE_SUCCESSOR_PREFIX_BYTES.saturating_sub(1));
                let prefix = successor_client
                    .retrieve_hls_payload_range(successor, 0, prefix_end, Some(generation))
                    .await;
                if u64::try_from(prefix.len()).ok() == Some(prefix_end.saturating_add(1)) {
                    mark_hls_progressive_successor_prefix_ready(
                        &source_reference,
                        generation,
                        timeline_epoch,
                    );
                }
            });
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
        let live_start = HLS_PREFETCH_SESSION.with(|session| session.borrow().live_start);
        let Some(mut segments) = hls_segment_identities(manifest) else {
            return;
        };
        if live_start && segments.len() > HLS_MEDIA_PLAN_MAX_REFERENCES {
            segments.drain(..segments.len() - HLS_MEDIA_PLAN_MAX_REFERENCES);
        } else {
            segments.truncate(HLS_MEDIA_PLAN_MAX_REFERENCES);
        }
        HLS_PAYLOAD_DURATIONS.with(|durations| {
            let mut durations = durations.borrow_mut();
            if durations.len().saturating_add(segments.len()) > HLS_MEDIA_PLAN_MAX_REFERENCES {
                durations.clear();
            }
            for segment in &segments {
                durations.insert(
                    segment.reference.clone(),
                    f64::from_bits(segment.duration_bits),
                );
            }
        });
        let references = segments
            .into_iter()
            .map(|segment| segment.reference)
            .collect();
        let early_overlap_limit =
            if hls_media_sequence(manifest).is_some_and(|sequence| sequence > 0) {
                HLS_ROLLING_EARLY_OVERLAP_SEGMENTS
            } else {
                HLS_EXACT_NEXT_OVERLAP_SEGMENTS
            };
        HLS_MEDIA_PLANS.with(|plans| {
            plans.borrow_mut().install_with_early_overlap_limit(
                references,
                early_overlap_limit,
                live_start,
            )
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

        // Dropping a probe waiter does not cancel its detached accounting owner.
        spawn_local(async move {
            let size = weeb3.hls_payload_size(reference.clone()).await;
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
        presentation_id: u64,
        live_start: bool,
    ) {
        // Reclaim completed ranges; pending/dispatched reads keep their transport.
        clear_completed_bzz_media_ranges();
        HLS_PAYLOAD_CACHE.with(|cache| cache.borrow_mut().resume_completed_retention());
        let generation = HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            session.client = Some(client.clone());
            session.feed_identity = Some((
                normalized_owner.to_ascii_lowercase(),
                normalized_topic.to_ascii_lowercase(),
            ));
            session.sequence_zero_runway_closed = false;
            session.presentation_id = presentation_id;
            session.live_start = live_start;
            session.live_history_active = false;
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
        weeb3: Arc<Weeb3>,
        network_id: u64,
        early_payloads: mpsc::Receiver<crate::bzz_stream::RawFeedPayload>,
        best_prefix: Rc<RefCell<Option<crate::bzz_stream::RawFeedPayload>>>,
        prefix_ready: mpsc::Sender<crate::bzz_stream::RawFeedPayload>,
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
            if accepted_prefix
                && hls_media_references(&payload.bytes).len()
                    >= HLS_EARLY_FEED_PREFIX_PREFERRED_SEGMENTS
            {
                let _ = prefix_ready.try_send(payload.clone());
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

    fn take_hls_progressive_successor(reference: &str) -> Option<(String, u64, u64)> {
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            if session.live_start || session.timeline_rebasing || session.generation == 0 {
                return None;
            }
            let generation = session.generation;
            let timeline_epoch = session.timeline_epoch;
            let start = session.progressive_start.as_mut()?;
            if start.0 != reference {
                return None;
            }
            let successor = start.1.take()?;
            Some((successor, generation, timeline_epoch))
        })
    }

    fn mark_hls_progressive_successor_prefix_ready(
        reference: &str,
        generation: u64,
        timeline_epoch: u64,
    ) {
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            if session.generation == generation
                && session.timeline_epoch == timeline_epoch
                && !session.live_start
                && !session.timeline_rebasing
                && !session.sequence_zero_runway_closed
                && session
                    .progressive_start
                    .as_ref()
                    .is_some_and(|(first, successor)| {
                        successor.is_none() && first.eq_ignore_ascii_case(reference)
                    })
            {
                session.progressive_successor_prefix_ready = true;
            }
        });
    }

    fn hls_progressive_successor_admission_is_current(
        reference: &str,
        generation: u64,
        timeline_epoch: u64,
    ) -> bool {
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            session.generation == generation
                && session.timeline_epoch == timeline_epoch
                && !session.live_start
                && !session.timeline_rebasing
                && !session.sequence_zero_runway_closed
                && session
                    .progressive_start
                    .as_ref()
                    .is_some_and(|start| start.0 == reference)
        })
    }

    fn hls_presentation_for_feed(
        client: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
    ) -> Option<(u64, bool)> {
        let identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
        HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
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

    fn set_hls_prefetch_mode(mode: HlsPrefetchMode) {
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            if mode == HlsPrefetchMode::Inactive {
                session.sequence_zero_runway_closed = true;
            }
            session.mode = mode;
        });
    }

    fn activate_hls_prefetch_warmup() {
        let now_ms = hls_monotonic_now_ms();
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            let timeline_rebasing = session.timeline_rebasing;
            let initial_warmup = !timeline_rebasing && session.mode == HlsPrefetchMode::Inactive;
            if timeline_rebasing {
                // Retire the old timeline before its LEVEL_LOADED callback can schedule work.
                session.advance_timeline();
                session.tracks.clear();
                session.startup_overlap_plans.clear();
                session.timeline_rebasing = false;
            }
            if (initial_warmup || timeline_rebasing)
                && let Some(now_ms) = now_ms
            {
                session.startup_deadline_ms = now_ms + HLS_INITIAL_RESPONSE_BUDGET_MS;
            }
            if session.mode != HlsPrefetchMode::Sustained {
                session.mode = HlsPrefetchMode::StartupOnly;
            }
        });
    }

    fn retire_hls_prefetch_timeline() {
        HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            // Keep immutable work joinable while replacing the archive timeline.
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

    fn invalidate_hls_prefetch_session() {
        let publish = HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            session.mode = HlsPrefetchMode::Inactive;
            let generation = session.advance_generation();
            session.tracks.clear();
            let client = session.client.take();
            session.feed_identity = None;
            session.sequence_zero_runway_closed = false;
            session.presentation_id = 0;
            session.live_history_active = false;
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
        let (generation, timeline_epoch, schedule_id, progressive_successor_prefix_ready) =
            HLS_PREFETCH_SESSION.with(|session| {
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
                        session.schedule_sequence =
                            next_nonzero_generation(session.schedule_sequence);
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
                let progressive_successor_prefix_ready = cursor.as_ref().is_some_and(|cursor| {
                    !session.live_start
                        && !session.timeline_rebasing
                        && session.progressive_successor_prefix_ready
                        && cursor.position == 1
                        && session
                            .progressive_start
                            .as_ref()
                            .is_some_and(|(first, successor)| {
                                successor.is_none()
                                    && cursor.references.first().is_some_and(|reference| {
                                        reference.eq_ignore_ascii_case(first)
                                    })
                            })
                });
                (
                    session.generation,
                    session.timeline_epoch,
                    selected_schedule_id,
                    progressive_successor_prefix_ready,
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
            progressive_successor_prefix_ready,
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
                && session.mode != HlsPrefetchMode::Inactive
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
        let (prefetch_limit, timeline_epoch) = HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            let limit = if session.generation != generation
                || session.completed_media_payloads < HLS_SERIAL_PREFETCH_COMPLETIONS
            {
                1
            } else if session.completed_media_payloads < HLS_TWO_BODY_PREFETCH_COMPLETIONS {
                2
            } else {
                HLS_PREFETCH_BODY_MAX_PARALLEL
            };
            (limit, session.timeline_epoch)
        });
        let role = HLS_PAYLOAD_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .load_role(&reference, prefetch, generation, prefetch_limit)
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
                let media_payload = result
                    .as_ref()
                    .is_ok_and(|body| !is_hls_manifest(body.as_ref()));
                let hot = hls_generation_current(generation);
                let owned = HLS_PAYLOAD_CACHE.with(|cache| {
                    cache.borrow_mut().finish_load(
                        &leader_reference,
                        generation,
                        load_id,
                        result,
                        hot,
                    )
                });
                if owned && hot && media_payload {
                    HLS_PREFETCH_SESSION.with(|session| {
                        let mut session = session.borrow_mut();
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
        for attempt in 0..HLS_PREFETCH_MAX_ATTEMPTS {
            if let Some(size) = cached_hls_payload_size(&reference) {
                return Some(size);
            }
            if !hls_prefetch_ticket_current(plan_id, generation, timeline_epoch, schedule_id) {
                return None;
            }

            let size = start_hls_payload_size_probe(weeb3.clone(), reference.clone())
                .recv()
                .await
                .ok()
                .flatten();
            if !hls_prefetch_ticket_current(plan_id, generation, timeline_epoch, schedule_id) {
                return None;
            }
            if size.is_some() {
                return size;
            }
            if attempt.saturating_add(1) < HLS_PREFETCH_MAX_ATTEMPTS {
                async_std::task::sleep(Duration::from_millis(
                    HLS_PAYLOAD_SIZE_RETRY_DELAY_MS
                        .saturating_mul(attempt.saturating_add(1) as u64),
                ))
                .await;
            }
        }
        None
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
            // Retry only terminal failures so accounting cannot be duplicated.
            async_std::task::sleep(Duration::from_millis(
                HLS_PAYLOAD_RETRY_DELAY_MS.saturating_mul(attempts as u64),
            ))
            .await;
            if !hls_prefetch_ticket_current(plan_id, generation, timeline_epoch, schedule_id) {
                break;
            }
            let retry = start_hls_payload_load(weeb3.clone(), reference.clone(), true, generation);
            if matches!(&retry, HlsPayloadLoadRole::AtCapacity) {
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
        let ahead_limit_bytes =
            media_prefetch_ahead_limit_bytes(hls_payload_cache_capacity_bytes());
        let startup_target_bytes = HLS_STARTUP_LOOKAHEAD_BYTES.min(ahead_limit_bytes);
        let startup_body_limit = HLS_STARTUP_BODY_MAX_PARALLEL.min(cursor.early_overlap_limit);
        let mut planned_bytes = 0_u64;
        let first_position = cursor.position.saturating_add(1);
        let mut probe_window = HlsOrderedProbeWindow::new(first_position);
        let mut size_probes = FuturesUnordered::new();
        let mut loads = FuturesUnordered::new();
        let mut budget_blocked = false;

        // Detached leaders outlive scheduler observers and settle accounting.
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
                        return;
                    }
                }
            }
        };

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
                return;
            }
            let mut capacity_blocked = false;

            if planned_bytes >= ahead_limit_bytes {
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
                let stagger_admission = matches!(&role, HlsPayloadLoadRole::Lead(_, _));
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
                if stagger_admission {
                    async_std::task::sleep(HLS_NEXT_RESERVE_STAGGER).await;
                    if !hls_sustained_prefetch_ticket_current(
                        cursor.plan_id,
                        generation,
                        timeline_epoch,
                        schedule_id,
                    ) {
                        return;
                    }
                }
            }

            if capacity_blocked {
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
            HLS_PAYLOAD_CACHE.with(|cache| cache.borrow().bodies.contains_key(&reference));
        let context = hls_foreground_context(&reference, foreground_cached);
        let prefetch_head_ready = foreground_cached || context.progressive_successor_prefix_ready;
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
                            .min(HLS_EXACT_NEXT_OVERLAP_SEGMENTS),
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
            let overlap_head_start_ms = if prefetch_head_ready {
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
                                // Foreground retrieval will join this detached leader by hash.
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
            if prefetch_head_ready {
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
            // Retry only after the shared accounting-sensitive result is terminal.
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
        // Return foreground bytes immediately while detached lookahead continues.
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
            .retrieve_hls_payload_range(reference.clone(), 0, probe_end, None)
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
        remember_hls_asset_metadata(&reference, metadata.clone());
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
                    complete: false,
                    snapshot: None,
                });
            }
            if presentation
                .as_ref()
                .is_some_and(|current| current.complete)
            {
                HlsCodecBootstrapManifest::Continuation
            } else {
                HlsCodecBootstrapManifest::Bootstrap(token)
            }
        })
    }

    pub(super) fn finish_hls_codec_bootstrap(source: &str) {
        let token = source
            .split_once('?')
            .map(|(_, query)| query)
            .into_iter()
            .flat_map(|query| query.split('&'))
            .find_map(|parameter| parameter.strip_prefix("codec-bootstrap="))
            .and_then(|token| token.parse::<u64>().ok());
        let Some(token) = token else {
            return;
        };
        HLS_CODEC_BOOTSTRAP_PRESENTATION.with(|presentation| {
            if let Some(current) = presentation.borrow_mut().as_mut()
                && current.token == token
            {
                current.complete = true;
            }
        });
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
                        .filter(|current| current.token == token && !current.complete)
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
            )
            .await
            {
                Some(snapshot) => snapshot,
                None => return FetchResponse::error(503, "weeb-3 did not retrieve feed update"),
            },
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
                    let Some(bootstrap_index) = snapshot.index.checked_sub(1) else {
                        return FetchResponse::error(503, "HLS codec bootstrap is not available");
                    };
                    let Some(bootstrap) = load_feed_snapshot(
                        weeb3.clone(),
                        owner.clone(),
                        topic.clone(),
                        Some(bootstrap_index),
                        HlsStart::Beginning,
                        None,
                    )
                    .await
                    else {
                        return FetchResponse::error(503, "HLS codec bootstrap is not available");
                    };
                    prepend_hls_codec_bootstrap(&snapshot.body, &bootstrap.body)
                };
                let Some(presentation) = presentation else {
                    return FetchResponse::error(502, "HLS codec bootstrap is not supported");
                };
                HLS_CODEC_BOOTSTRAP_PRESENTATION.with(|state| {
                    if let Some(current) = state.borrow_mut().as_mut()
                        && current.token == token
                        && !current.complete
                    {
                        current.snapshot = Some(FeedRouteSnapshot {
                            body: Arc::from(presentation.clone()),
                            ..snapshot.clone()
                        });
                    }
                });
                Some(presentation)
            }
            Some(HlsCodecBootstrapManifest::Continuation) => {
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
        let Some(body) = rewritten else {
            return FetchResponse::error(502, "invalid HLS manifest");
        };
        remember_hls_media_plan(&body);
        if sequence_zero_bootstrap
            && let Some(reference) = hls_media_references(&body).into_iter().next()
            && let Some(generation) = hls_prefix_generation_for_feed(&weeb3, &owner, &topic)
        {
            drop(start_hls_payload_load(
                weeb3.clone(),
                reference,
                false,
                generation,
            ));
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
        let Some(generation) = hls_prefix_generation_for_feed(weeb3, owner, topic) else {
            return;
        };
        if !HLS_PREFETCH_SESSION.with(|session| {
            let session = session.borrow();
            session.live_start && session.generation == generation
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
            drop(start_hls_payload_load(
                weeb3.clone(),
                reference.clone(),
                true,
                generation,
            ));
        }
        if start > 0
            && hls_media_sequence(&snapshot.body) == Some(0)
            && let Some(reference) = references.first()
        {
            drop(start_hls_payload_load(
                weeb3.clone(),
                reference.clone(),
                false,
                generation,
            ));
        }
    }

    async fn await_live_frontier_snapshot(
        cache_key: &str,
        checked_after_ms: f64,
        deadline_ms: f64,
        initial_check_token: Option<u64>,
    ) -> Option<FeedRouteSnapshot> {
        loop {
            let state = FEED_ROUTE_CACHE.with(|cache| {
                cache.borrow().get(cache_key).map(|state| {
                    (
                        state.snapshot.clone(),
                        state.confirmed_head_index,
                        state.last_head_check,
                        state.checking_token,
                    )
                })
            });
            let now = js_sys::Date::now();
            if let Some((snapshot, confirmed_head_index, last_head_check, checking_token)) = state {
                if hls_live_frontier_is_ready(
                    snapshot.index,
                    confirmed_head_index,
                    last_head_check,
                    checked_after_ms,
                    initial_check_token,
                    checking_token,
                ) || now >= deadline_ms
                {
                    return Some(snapshot);
                }
            } else if initial_check_token.is_some() || now >= deadline_ms {
                return None;
            }
            async_std::task::sleep(Duration::from_millis(15)).await;
        }
    }

    fn start_beginning_snapshot_runway(
        weeb3: &Arc<Weeb3>,
        owner: &str,
        topic: &str,
        snapshot: &FeedRouteSnapshot,
    ) {
        if hls_media_sequence(&snapshot.body) != Some(0) {
            return;
        }
        let Some(generation) = hls_prefix_generation_for_feed(weeb3, owner, topic) else {
            return;
        };
        let mut references = hls_media_references(&snapshot.body).into_iter();
        let Some(reference) = references.next() else {
            return;
        };
        let successor = references.next();
        let successor_probe = successor.clone();
        let timeline_epoch = HLS_PREFETCH_SESSION.with(|session| {
            let mut session = session.borrow_mut();
            if session.generation != generation
                || session.live_start
                || session.timeline_rebasing
                || session.sequence_zero_runway_closed
            {
                return None;
            }
            session.progressive_start = Some((reference.clone(), successor));
            session.progressive_successor_prefix_ready = false;
            Some(session.timeline_epoch)
        });
        let Some(timeline_epoch) = timeline_epoch else {
            return;
        };

        let warmup_client = weeb3.clone();
        if let Some(successor) = successor_probe {
            drop(start_hls_payload_size_probe(
                warmup_client.clone(),
                successor,
            ));
        }
        let size = start_hls_payload_size_probe(warmup_client.clone(), reference.clone());
        spawn_local(async move {
            let Some(size) = size.recv().await.ok().flatten() else {
                return;
            };
            if !hls_progressive_successor_admission_is_current(
                &reference,
                generation,
                timeline_epoch,
            ) {
                return;
            }
            let prefix_end = size
                .saturating_sub(1)
                .min(MEDIA_STORAGE_WINDOW_BYTES.saturating_sub(1));
            let prefix = warmup_client.retrieve_hls_payload_range(
                reference.clone(),
                0,
                prefix_end,
                Some(generation),
            );
            if size > MEDIA_STORAGE_WINDOW_BYTES {
                let second_end = size.saturating_sub(1).min(
                    MEDIA_STORAGE_WINDOW_BYTES
                        .saturating_mul(2)
                        .saturating_sub(1),
                );
                let second = async {
                    async_std::task::sleep(Duration::ZERO).await;
                    warmup_client
                        .retrieve_hls_payload_range(
                            reference.clone(),
                            MEDIA_STORAGE_WINDOW_BYTES,
                            second_end,
                            Some(generation),
                        )
                        .await
                };
                let _ = join(prefix, second).await;
            } else {
                let _ = prefix.await;
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
            } else if live_history_presentation_id.is_some()
                && cache.contains_key(&canonical_cache_key)
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
                state.canonical_stabilization_running,
                state.last_head_check,
            ))
        });

        if let Some((
            mut snapshot,
            refresh_head,
            cached_key,
            canonical_stabilization_running,
            last_head_check,
        )) = cached
            && !(start == HlsStart::Live
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
                if !canonical_stabilization_running {
                    schedule_feed_followup(
                        weeb3.clone(),
                        cached_key.clone(),
                        owner.clone(),
                        topic.clone(),
                        refresh_head,
                        followup_mode,
                    );
                }
                if start == HlsStart::Live {
                    if refresh_head && wait_for_live_frontier {
                        snapshot = await_live_frontier_snapshot(
                            &cached_key,
                            last_head_check,
                            live_frontier_deadline_ms,
                            None,
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
                        if start == HlsStart::Live {
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
                        weeb3
                            .latest_hls_feed_payload_startup(
                                owner.clone(),
                                topic.clone(),
                                None,
                                None,
                            )
                            .await?
                    }
                    None => {
                        let (early_payload_out, early_payload_in) =
                            mpsc::bounded::<crate::bzz_stream::RawFeedPayload>(16);
                        let (prefix_ready_out, prefix_ready_in) =
                            mpsc::bounded::<crate::bzz_stream::RawFeedPayload>(1);
                        let best_prefix = Rc::new(RefCell::new(None));
                        spawn_local(fan_out_authenticated_hls_prefixes(
                            weeb3.clone(),
                            network_id,
                            early_payload_in,
                            best_prefix.clone(),
                            prefix_ready_out,
                            sequence_zero_start_requested.then(|| cache_key.clone()),
                        ));
                        for index in std::iter::once(HLS_EARLY_FEED_PREFIX_INDEX)
                            .chain(persisted_index.is_some().then_some(0))
                        {
                            let reliable_prefix_out = early_payload_out.clone();
                            let reliable_prefix_client = weeb3.clone();
                            let reliable_prefix_owner = owner.clone();
                            let reliable_prefix_topic = topic.clone();
                            spawn_local(async move {
                                if let Some(payload) = reliable_prefix_client
                                    .hls_feed_payload_at_index(
                                        reliable_prefix_owner,
                                        reliable_prefix_topic,
                                        index,
                                    )
                                    .await
                                {
                                    let _ = reliable_prefix_out.try_send(payload);
                                }
                            });
                        }
                        // Dropping this listener cannot cancel the detached canonical frontier.
                        let (canonical_out, canonical_in) =
                            mpsc::bounded::<Option<crate::bzz_stream::RawFeedPayload>>(1);
                        let canonical_client = weeb3.clone();
                        let canonical_owner = owner.clone();
                        let canonical_topic = topic.clone();
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
                                        .latest_hls_feed_payload_startup(
                                            canonical_owner,
                                            canonical_topic,
                                            early_payloads,
                                            early_payload_max_index,
                                        )
                                        .await
                                }
                            };
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
                                    let canonical_starts_late =
                                        hls_media_sequence(&canonical.bytes)
                                            .is_some_and(|sequence| sequence > 0);
                                    let mut preferred =
                                        best_prefix.borrow().clone().filter(|prefix| {
                                            prefix.index < canonical.index
                                                && hls_media_references(&prefix.bytes).len()
                                                    >= HLS_EARLY_FEED_PREFIX_PREFERRED_SEGMENTS
                                                && hls_startup_prefix_is_preferred(
                                                    &canonical.bytes,
                                                    &prefix.bytes,
                                                    HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
                                                )
                                        });
                                    if preferred.is_none()
                                        && canonical_starts_late
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
                                        )
                                    {
                                        preferred = Some(prefix);
                                    }
                                    if preferred.is_none() {
                                        preferred = best_prefix.borrow().clone().filter(|prefix| {
                                            prefix.index < canonical.index
                                                && hls_startup_prefix_is_preferred(
                                                    &canonical.bytes,
                                                    &prefix.bytes,
                                                    HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS,
                                                )
                                        });
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
                                    if canonical_starts_late {
                                        return None;
                                    }
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
                                        _ => best_prefix.borrow().clone()?,
                                    };
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
                |_, _| {},
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
        let await_live_frontier =
            (start == HlsStart::Live && index_hint.is_none() && stabilization_seed.is_some())
                .then_some(());
        if index_hint.is_none() && snapshot.finalized {
            remember_authenticated_endlist_index(network_id, &owner, &topic, snapshot.index);
        }
        let initial_check_token = if let Some(initial) = stabilization_seed {
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
            Some(()) if initial_check_token.is_some() => await_live_frontier_snapshot(
                &cache_key,
                last_head_check,
                live_frontier_deadline_ms,
                initial_check_token,
            )
            .await
            .unwrap_or(snapshot),
            _ => snapshot,
        };
        if start == HlsStart::Live && index_hint.is_none() {
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

        let Some(confirmed) = weeb3
            .latest_hls_feed_payload_from(owner.to_string(), topic.to_string(), candidate.clone())
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

    async fn stabilize_initial_unindexed_hls_payload<ObserveCandidate>(
        weeb3: Arc<Weeb3>,
        owner: &str,
        topic: &str,
        network_id: u64,
        mut loaded: crate::bzz_stream::RawFeedPayload,
        observe_progress: bool,
        mut observe_candidate: ObserveCandidate,
    ) -> (crate::bzz_stream::RawFeedPayload, bool)
    where
        ObserveCandidate: FnMut(&crate::bzz_stream::RawFeedPayload, bool),
    {
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
            acquire_latest_raw_feed_payload_bounded_from(
                owner.to_string(),
                topic.to_string(),
                loaded.clone(),
                false,
                &weeb3.chunk_port.0,
                None,
            )
            .await
        } else {
            let (observed_out, observed_in) = mpsc::bounded::<crate::bzz_stream::RawFeedPayload>(2);
            let mut search = Box::pin(acquire_latest_raw_feed_payload_bounded_from(
                owner.to_string(),
                topic.to_string(),
                loaded.clone(),
                true,
                &weeb3.chunk_port.0,
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
                                    observe_candidate(&loaded, false);
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
                            observe_candidate(&loaded, false);
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
                observe_candidate(&loaded, false);
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
                    if head_confirmed {
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
                let now = js_sys::Date::now();
                state.confirmed_head_index = Some(candidate.index);
                state.last_head_check = now;
                state.last_touch = now;
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
                FeedRouteBodyUpdate::Publish(body) => {
                    let tracks_source = body.as_ref() == source_body.as_ref();
                    (body, tracks_source)
                }
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
            let now = js_sys::Date::now();
            if head_confirmed {
                state.confirmed_head_index = Some(candidate.index);
                state.last_head_check = now;
            } else {
                state.confirmed_head_index = None;
                state.last_head_check = 0.0;
            }
            state.last_touch = now;
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
        observe_progress: bool,
        followup_mode: FeedFollowupMode,
    ) {
        // Head verification outlives the response and publishes continuous advances.
        let (_, head_confirmed) = stabilize_initial_unindexed_hls_payload(
            weeb3.clone(),
            &owner,
            &topic,
            network_id,
            initial,
            observe_progress,
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

        let network_current = weeb3.get_network_id().await == network_id
            && active_profile().swarm_network_id == network_id;
        if let Some((cache_finalized, cache_index)) =
            release_feed_route_check(&cache_key, checking_token)
            && network_current
        {
            if cache_finalized {
                remember_authenticated_endlist_index(network_id, &owner, &topic, cache_index);
            } else {
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

    fn finish_sequence_zero_canonical_stabilization(cache_key: &str) -> bool {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let Some(state) = cache.get_mut(cache_key) else {
                return false;
            };
            let released = state.canonical_stabilization_running;
            state.canonical_stabilization_running = false;
            trim_feed_route_cache(&mut cache, cache_key);
            released
        })
    }

    async fn stabilize_sequence_zero_canonical_route(
        weeb3: Arc<Weeb3>,
        cache_key: String,
        owner: String,
        topic: String,
        network_id: u64,
        initial: crate::bzz_stream::RawFeedPayload,
    ) {
        // Preserve the exact follower's token while stabilizing the canonical seed.
        let (candidate, head_confirmed) = stabilize_initial_unindexed_hls_payload(
            weeb3.clone(),
            &owner,
            &topic,
            network_id,
            initial,
            false,
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

        spawn_local(stabilize_claimed_feed_route(
            weeb3,
            cache_key,
            owner,
            topic,
            network_id,
            checking_token,
            initial,
            observe_progress,
            followup_mode,
        ));
        Some(checking_token)
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
        let canonical_reserved = claim_sequence_zero_canonical_stabilization(&cache_key);
        if canonical_reserved {
            let fallback_client = weeb3.clone();
            let fallback_cache_key = cache_key.clone();
            let fallback_owner = owner.clone();
            let fallback_topic = topic.clone();
            spawn_local(async move {
                async_std::task::sleep(HLS_SEQUENCE_ZERO_CANONICAL_EXCLUSIVITY).await;
                if fallback_client.get_network_id().await == network_id
                    && finish_sequence_zero_canonical_stabilization(&fallback_cache_key)
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
            if let Some(initial) = initial {
                if canonical_reserved {
                    stabilize_sequence_zero_canonical_route(
                        weeb3, cache_key, owner, topic, network_id, initial,
                    )
                    .await;
                }
                return;
            }

            let follower_released =
                !canonical_reserved || finish_sequence_zero_canonical_stabilization(&cache_key);
            if follower_released && weeb3.get_network_id().await == network_id {
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

    fn start_live_history_accumulator(
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        presentation_id: u64,
        initial: FeedRouteSnapshot,
    ) {
        let network_id = active_profile().swarm_network_id;
        let canonical_cache_key = feed_cache_key(&owner, &topic, None);
        let history_cache_key = sequence_zero_feed_cache_key(&owner, &topic, presentation_id);
        spawn_local(async move {
            let initial_index = initial.index;
            let feed_identity = (owner.to_ascii_lowercase(), topic.to_ascii_lowercase());
            let generation = loop {
                let admission = HLS_PREFETCH_SESSION.with(|session| {
                    let session = session.borrow();
                    if session.presentation_id != presentation_id
                        || !session.live_start
                        || session.live_history_active
                        || session.feed_identity.as_ref() != Some(&feed_identity)
                        || session
                            .client
                            .as_ref()
                            .is_none_or(|active| !Arc::ptr_eq(active, &weeb3))
                    {
                        return None;
                    }
                    Some(
                        session.mode == HlsPrefetchMode::Sustained
                            && session.completed_media_payloads
                                >= HLS_TWO_BODY_PREFETCH_COMPLETIONS,
                    )
                });
                match admission {
                    None => return,
                    Some(true) => {
                        let Some(generation) =
                            hls_prefix_generation_for_feed(&weeb3, &owner, &topic)
                        else {
                            return;
                        };
                        break generation;
                    }
                    Some(false) => {
                        async_std::task::sleep(Duration::from_millis(
                            MEDIA_PREFETCH_BATCH_YIELD_MS,
                        ))
                        .await;
                    }
                }
            };
            if weeb3.get_network_id().await != network_id {
                return;
            }

            let session_is_current = || {
                HLS_PREFETCH_SESSION.with(|session| {
                    let session = session.borrow();
                    session.generation == generation
                        && session.presentation_id == presentation_id
                        && session.live_start
                        && !session.live_history_active
                        && session.feed_identity.as_ref() == Some(&feed_identity)
                        && session
                            .client
                            .as_ref()
                            .is_some_and(|active| Arc::ptr_eq(active, &weeb3))
                })
            };
            let initial_prefix = (hls_media_sequence(&initial.body) == Some(0)).then(|| {
                crate::bzz_stream::RawFeedPayload {
                    index: initial.index,
                    bytes: initial.body.to_vec(),
                }
            });
            let canonical_prefix = initial_prefix.or_else(|| {
                FEED_ROUTE_CACHE.with(|cache| {
                    cache.borrow().get(&canonical_cache_key).and_then(|state| {
                        (hls_media_sequence(&state.snapshot.body) == Some(0)).then(|| {
                            crate::bzz_stream::RawFeedPayload {
                                index: state.snapshot.index,
                                bytes: state.snapshot.body.to_vec(),
                            }
                        })
                    })
                })
            });
            let prefix = if let Some(prefix) = canonical_prefix {
                prefix
            } else {
                loop {
                    if !session_is_current() {
                        return;
                    }
                    let last_prefix_index = HLS_EARLY_FEED_PREFIX_INDEX
                        .saturating_add(HLS_EARLY_FEED_PREFIX_TARGET_SEGMENTS as u64)
                        .min(initial_index);
                    let candidates =
                        stream::iter(0..=last_prefix_index)
                            .map(|index| {
                                let client = weeb3.clone();
                                let owner = owner.clone();
                                let topic = topic.clone();
                                async move {
                                    client.hls_feed_payload_at_index(owner, topic, index).await
                                }
                            })
                            .buffered(feed_followup_max_parallel(
                                FeedFollowupMode::SequenceZeroPresentation,
                            ))
                            .collect::<Vec<_>>()
                            .await;
                    let unavailable = candidates.iter().any(Option::is_none);
                    if let Some(prefix) = candidates.into_iter().rev().flatten().find(|payload| {
                        payload.bytes.len() <= MAX_STREAM_FEED_PAYLOAD_BYTES
                            && hls_media_sequence(&payload.bytes) == Some(0)
                    }) {
                        break prefix;
                    }
                    if !unavailable {
                        return;
                    }
                    async_std::task::sleep(Duration::from_millis(HLS_PAYLOAD_RETRY_DELAY_MS)).await;
                }
            };
            if !is_hls_manifest(&prefix.bytes)
                || !hls_append_only_tags_are_supported(&prefix.bytes)
                || !hls_has_at_most_one_endlist(&prefix.bytes)
            {
                return;
            }
            let mut current_index = prefix.index;
            let mut target_duration = match hls_target_duration(&prefix.bytes) {
                Some(duration) => duration,
                None => return,
            };
            let mut archive_segment_count = match hls_segment_identities(&prefix.bytes)
                .and_then(|segments| u64::try_from(segments.len()).ok())
            {
                Some(count) if count > 0 => count,
                _ => return,
            };
            let archive_uri_ends = match hls_segment_uri_line_ends(&prefix.bytes) {
                Some(ends) if u64::try_from(ends.len()).ok() == Some(archive_segment_count) => ends,
                _ => return,
            };
            let mut archive_media_end = *archive_uri_ends
                .last()
                .expect("a non-empty archive has a final media URI");
            let mut current_source = prefix.bytes.clone();
            let mut archive = prefix.bytes;

            loop {
                if !session_is_current()
                    || weeb3.get_network_id().await != network_id
                    || active_profile().swarm_network_id != network_id
                {
                    return;
                }

                let target_index = FEED_ROUTE_CACHE.with(|cache| {
                    cache
                        .borrow()
                        .get(&canonical_cache_key)
                        .map(|state| state.snapshot.index)
                });
                let Some(target_index) = target_index else {
                    return;
                };
                if current_index >= target_index {
                    let canonical = FEED_ROUTE_CACHE.with(|cache| {
                        cache.borrow().get(&canonical_cache_key).map(|state| {
                            (
                                state.snapshot.index,
                                state.snapshot.finalized,
                                state.confirmed_head_index,
                                state.last_head_check,
                                state.source_body.clone(),
                            )
                        })
                    });
                    if let Some((
                        canonical_index,
                        canonical_finalized,
                        confirmed_head_index,
                        last_head_check,
                        canonical_source,
                    )) = canonical
                        && current_index == canonical_index
                        && confirmed_head_index == Some(canonical_index)
                        && current_source.as_slice() == canonical_source.as_ref()
                    {
                        if raise_hls_target_duration(&mut archive, target_duration).is_none()
                            || archive.len() > MAX_STREAM_FEED_PAYLOAD_BYTES
                            || hls_media_sequence(&archive) != Some(0)
                            || !hls_append_only_tags_are_supported(&archive)
                            || !hls_has_at_most_one_endlist(&archive)
                            || hls_segment_identities(&archive)
                                .and_then(|segments| u64::try_from(segments.len()).ok())
                                != Some(archive_segment_count)
                        {
                            return;
                        }
                        let body: Arc<[u8]> = Arc::from(archive);
                        let body_tracks_source = body.as_ref() == current_source.as_slice();
                        let source_body = if body_tracks_source {
                            body.clone()
                        } else {
                            Arc::from(current_source)
                        };
                        let finalized = canonical_finalized && hls_is_finalized(&body);
                        FEED_ROUTE_CACHE.with(|cache| {
                            let mut cache = cache.borrow_mut();
                            cache.insert(
                                history_cache_key.clone(),
                                FeedRouteState {
                                    snapshot: FeedRouteSnapshot {
                                        index: current_index,
                                        body,
                                        finalized,
                                    },
                                    source_body,
                                    body_tracks_source,
                                    source_endlist_confirmed: canonical_finalized,
                                    canonical_stabilization_started: false,
                                    canonical_stabilization_running: false,
                                    checking_token: 0,
                                    confirmed_head_index: Some(current_index),
                                    last_head_check,
                                    last_touch: js_sys::Date::now(),
                                },
                            );
                            trim_feed_route_cache(&mut cache, &history_cache_key);
                        });
                        HLS_PREFETCH_SESSION.with(|session| {
                            let mut session = session.borrow_mut();
                            if session.generation == generation
                                && session.presentation_id == presentation_id
                                && session.live_start
                            {
                                session.live_history_active = true;
                            }
                        });
                        return;
                    }
                    async_std::task::sleep(Duration::from_millis(MEDIA_PREFETCH_BATCH_YIELD_MS))
                        .await;
                    continue;
                }

                let Some(first_index) = current_index.checked_add(1) else {
                    return;
                };
                let batch_end = current_index
                    .saturating_add(feed_followup_batch_limit(
                        FeedFollowupMode::SequenceZeroPresentation,
                    ) as u64)
                    .min(target_index);
                let batch = stream::iter(first_index..=batch_end)
                    .map(|index| {
                        let client = weeb3.clone();
                        let owner = owner.clone();
                        let topic = topic.clone();
                        async move {
                            (
                                index,
                                client.hls_feed_payload_at_index(owner, topic, index).await,
                            )
                        }
                    })
                    .buffered(feed_followup_max_parallel(
                        FeedFollowupMode::SequenceZeroPresentation,
                    ))
                    .collect::<Vec<_>>()
                    .await;

                let mut unresolved_missing = false;
                for (expected_index, payload) in batch {
                    let Some(payload) = payload else {
                        unresolved_missing = true;
                        continue;
                    };
                    if payload.index != expected_index
                        || payload.bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES
                        || !is_hls_manifest(&payload.bytes)
                        || append_hls_sequence_zero_archive_suffix(
                            &mut archive,
                            &mut archive_segment_count,
                            &mut archive_media_end,
                            &current_source,
                            &payload.bytes,
                        )
                        .is_none()
                    {
                        continue;
                    }
                    let Some(candidate_target_duration) = hls_target_duration(&payload.bytes)
                    else {
                        return;
                    };
                    target_duration = target_duration.max(candidate_target_duration);
                    current_index = expected_index;
                    current_source = payload.bytes;
                    unresolved_missing = false;
                }
                if current_index < batch_end {
                    if !unresolved_missing {
                        return;
                    }
                    async_std::task::sleep(Duration::from_millis(HLS_PAYLOAD_RETRY_DELAY_MS)).await;
                    continue;
                }
                async_std::task::sleep(Duration::from_millis(MEDIA_PREFETCH_BATCH_YIELD_MS)).await;
            }
        });
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
        if !continuous
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

    fn store_feed_snapshot(
        cache_key: &str,
        snapshot: FeedRouteSnapshot,
        advancing_live_route: bool,
        followup_mode: FeedFollowupMode,
    ) -> FeedRouteSnapshot {
        FEED_ROUTE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some(existing) = cache.get_mut(cache_key) {
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
                        existing.confirmed_head_index = Some(existing.snapshot.index);
                        existing.last_head_check = js_sys::Date::now();
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
                    FeedRouteBodyUpdate::Publish(body) => {
                        let tracks_source = body.as_ref() == source_body.as_ref();
                        (body, tracks_source)
                    }
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
                let now = js_sys::Date::now();
                existing.confirmed_head_index = finalized.then_some(existing.snapshot.index);
                existing.last_head_check = if finalized { now } else { 0.0 };
                existing.last_touch = now;
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
                    confirmed_head_index: stored.finalized.then_some(stored.index),
                    last_head_check: if stored.finalized {
                        js_sys::Date::now()
                    } else {
                        0.0
                    },
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
    ) -> Option<(u64, bool)> {
        let Some((initial, force_coarse)) = FEED_ROUTE_CACHE.with(|cache| {
            let cache = cache.borrow();
            let state = cache.get(cache_key)?;
            if state.checking_token != checking_token {
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
            let Some(latest) = acquire_latest_raw_feed_payload_bounded_from(
                owner.to_string(),
                topic.to_string(),
                initial,
                force_coarse,
                &weeb3.chunk_port.0,
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
                        FeedRouteBodyUpdate::Publish(body) => {
                            let tracks_source = body.as_ref() == latest_source.as_ref();
                            (body, tracks_source)
                        }
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
            let state = cache.get_mut(cache_key)?;
            let now = js_sys::Date::now();
            if head_confirmed {
                state.confirmed_head_index = Some(latest_index);
                state.last_head_check = now;
            } else {
                state.confirmed_head_index = None;
                state.last_head_check = 0.0;
            }
            state.last_touch = now;
            Some(
                cache
                    .get(cache_key)
                    .and_then(|state| state.snapshot.finalized.then_some(state.snapshot.index)),
            )
        });
        if let Some(Some(index)) = accepted {
            remember_authenticated_endlist_index(network_id, owner, topic, index);
        }
        if accepted.is_some()
            && let Some(snapshot) = FEED_ROUTE_CACHE.with(|cache| {
                cache
                    .borrow()
                    .get(cache_key)
                    .map(|state| state.snapshot.clone())
            })
        {
            prefetch_live_snapshot_start(&weeb3, owner, topic, &snapshot);
        }
        accepted.map(|_| (latest_index, head_confirmed))
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
            if weeb3.get_network_id().await != network_id
                || active_profile().swarm_network_id != network_id
            {
                let _ = release_feed_route_check(&cache_key, checking_token);
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
                    followup_mode,
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
                        followup_mode,
                    )
                    .await;
                }
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
                .buffered(feed_followup_max_parallel(followup_mode));
            while let Some((next_index, next)) = exact_followups.next().await {
                if weeb3.get_network_id().await != network_id
                    || active_profile().swarm_network_id != network_id
                {
                    break;
                }
                let Some(next) = next else {
                    if !skipped_missing_index {
                        skipped_missing_index = true;
                        continue;
                    }
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
                        FeedRouteBodyUpdate::Publish(body) => {
                            let tracks_source = body.as_ref() == source_body.as_ref();
                            (body, tracks_source)
                        }
                        FeedRouteBodyUpdate::Hold => (state.snapshot.body.clone(), false),
                    };
                    state.source_body = source_body.clone();
                    state.body_tracks_source = body_tracks_source;
                    state.source_endlist_confirmed = false;
                    state.snapshot = FeedRouteSnapshot {
                        index: next.index,
                        body,
                        finalized: false,
                    };
                    state.confirmed_head_index = None;
                    state.last_head_check = 0.0;
                    state.last_touch = js_sys::Date::now();
                    trim_feed_route_cache(&mut cache, &cache_key);
                    true
                });
                if !accepted {
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
                && feed_followup_should_refresh_head(
                    followup_mode,
                    successful_followups,
                    saw_tentative_endlist,
                    recovered_missing_index,
                )
            {
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
        let live_frontier_deadline_ms =
            js_sys::Date::now() + HLS_LIVE_FRONTIER_MAX_WAIT.as_millis() as f64;
        let snapshot_load = Box::pin(async move {
            let snapshot = load_feed_snapshot(
                snapshot_client.clone(),
                snapshot_owner.clone(),
                snapshot_topic.clone(),
                None,
                start,
                Some(live_frontier_deadline_ms),
            )
            .await;
            if start == HlsStart::Beginning
                && let Some(snapshot) = snapshot.as_ref()
            {
                start_beginning_snapshot_runway(
                    &snapshot_client,
                    &snapshot_owner,
                    &snapshot_topic,
                    snapshot,
                );
            }
            snapshot
        });
        let snapshot = match select(worker_ready, snapshot_load).await {
            Either::Left((ready, snapshot_load)) => {
                if !ready {
                    return Err(service_worker_scope_protocol_error(
                        "HLS feed and segment requests",
                    ));
                }
                snapshot_load.await
            }
            Either::Right((snapshot, worker_ready)) => {
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
        if start == HlsStart::Live
            && let Some(snapshot) = snapshot.as_ref()
        {
            let current_snapshot = FEED_ROUTE_CACHE.with(|cache| {
                cache
                    .borrow()
                    .get(&feed_cache_key(&owner, &topic, None))
                    .map(|state| state.snapshot.clone())
            });
            if let Some(current_snapshot) =
                current_snapshot.filter(|current| current.index != snapshot.index)
            {
                prefetch_live_snapshot_start(&weeb3, &owner, &topic, &current_snapshot);
            }
        }
        if !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        let live_history_seed = snapshot
            .as_ref()
            .filter(|snapshot| start == HlsStart::Live && !snapshot.finalized)
            .cloned();

        let initial_start_position = match start {
            HlsStart::Beginning => 0.0,
            HlsStart::Live => -1.0,
        };
        let mode = play_hls(player, &source, hls_loader, initial_start_position)
            .await
            .map_err(|error| format!("Could not initialize HLS: {}", js_error_message(&error)))?;
        if !result_view_request_is_current(view_generation) {
            return Err("HLS open was superseded".to_string());
        }
        if let Some(initial) = live_history_seed {
            start_live_history_accumulator(weeb3, owner, topic, presentation_id, initial);
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
            if play_player.get_attribute("data-weeb3-hls-mode").as_deref() == Some("hls.js") {
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
            if pause_player.get_attribute("data-weeb3-hls-mode").as_deref() == Some("hls.js") {
                // hls.js reports authorized pauses through HLS_EXPLICIT_PAUSE_EVENT.
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
        HLS_PAYLOAD_CACHE.with(|cache| cache.borrow_mut().suspend_completed_retention());
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use runtime::{
    attach_hls_feed_player, hls_payload_cache_body_bytes, open_hls_feed_view,
    release_hls_for_bzz_view, release_hls_view, try_fetch_response,
};
