use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{retrieval_conventions::next_nonzero_generation, stream_conventions::HlsStart};

const HLS_HEADER: &str = "#EXTM3U";
const HLS_ENDLIST: &str = "#EXT-X-ENDLIST";
const HLS_SERVER_CONTROL: &str = "#EXT-X-SERVER-CONTROL";
pub(crate) const HLS_LIVE_SYNC_SEGMENTS: usize = 8;
pub(crate) const HLS_SPARSE_HISTORY_STRIDE: u64 = 10;
pub(crate) const HLS_SPARSE_HISTORY_MAX_SEGMENTS: usize = 32_768;
pub(crate) const HLS_SPARSE_HISTORY_MAX_PROBES: usize = 4_096;
pub(crate) const HLS_SPARSE_HISTORY_MAX_REPAIRS: usize = 4_096;
pub(crate) const HLS_SPARSE_HISTORY_MAX_CANDIDATES: usize =
    HLS_SPARSE_HISTORY_MAX_PROBES + HLS_SPARSE_HISTORY_MAX_REPAIRS;
pub(crate) const HLS_SPARSE_HISTORY_MAX_WINDOW_BYTES: usize = 64 * 1024;
pub(crate) const HLS_SPARSE_HISTORY_MAX_AGGREGATE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const HLS_SPARSE_HISTORY_MAX_PARALLEL: usize = 64;
const HLS_SPARSE_HISTORY_CAPACITY_REFRESH_MS: f64 = 100.0;

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

    fn resize(&mut self, max_references: usize) {
        let max_references = max_references.max(1);
        if self.max_references != max_references {
            self.max_references = max_references;
            self.cursor_count = 0;
            self.cursors.clear();
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
        if references.is_empty()
            || self.cursors.get(&references[0]).is_some_and(|candidates| {
                candidates.iter().any(|cursor| {
                    cursor.position == 0
                        && cursor.plan.references.as_ref() == references
                        && cursor.plan.early_overlap_limit == early_overlap_limit
                })
            })
        {
            return;
        }
        if self.cursor_count.saturating_add(references.len()) > self.max_references {
            self.cursors.clear();
            self.cursor_count = 0;
        }

        self.next_plan_id = next_nonzero_generation(self.next_plan_id);
        let plan = Arc::new(HlsMediaPlan {
            id: self.next_plan_id,
            references: references.into(),
            early_overlap_limit,
        });
        for (position, reference) in plan.references.iter().enumerate() {
            self.cursors
                .entry(reference.clone())
                .or_default()
                .push(HlsMediaCursor {
                    plan: plan.clone(),
                    position,
                });
            self.cursor_count += 1;
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
        return Some(candidate.to_vec());
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
    if !stream_feed_payload_len_is_supported(merged.len()) {
        return None;
    }

    let mut expected = current_segments;
    expected.extend_from_slice(candidate_segments.get(appended_position..)?);
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
    let suffix_start = if adjacent {
        candidate_media_start?
    } else {
        let overlap_position = usize::try_from(
            archive_segment_count
                .checked_sub(1)?
                .checked_sub(candidate_sequence)?,
        )
        .ok()?;
        *uri_ends.get(overlap_position)?
    };
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
        Some(Self {
            body: source.to_vec(),
            segments: timeline.segments.clone(),
            media_end: *timeline.uri_ends.last()?,
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

struct HlsTimeline {
    sequence: u64,
    segments: Vec<HlsSegmentIdentity>,
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
        let mut uri_ends = Vec::new();
        let mut expects_media_uri = false;
        let mut duration_bits = None::<u64>;
        let mut byte_range = None::<(u64, Option<u64>)>;
        let mut media_start = None;
        let mut previous_range_end = None::<(String, u64)>;
        let mut discontinuity_counter = 0_u64;
        let mut saw_discontinuity_sequence = false;
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
                });
                uri_ends.push(offset.checked_add(original_line.len())?);
                expects_media_uri = false;
            } else {
                return None;
            }
            offset = offset.checked_add(original_line.len())?;
        }
        if expects_media_uri {
            return None;
        }
        Some(Self {
            sequence: sequence.unwrap_or(0),
            segments,
            uri_ends,
            media_start,
        })
    }

    fn end(&self) -> Option<u64> {
        self.sequence
            .checked_add(u64::try_from(self.segments.len()).ok()?)
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
        let start = self.segments.len().saturating_sub(HLS_LIVE_SYNC_SEGMENTS);
        let duration = self.segments[start..]
            .iter()
            .try_fold(0.0, |duration, segment| {
                let segment_duration = f64::from_bits(segment.duration_bits);
                (segment_duration.is_finite() && segment_duration > 0.0)
                    .then_some(duration + segment_duration)
            })?;
        (duration.is_finite() && duration > 0.0).then_some((start, duration))
    }
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
    let mut bootstrap_segments = hls_segment_identities(bootstrap_manifest)?;
    if hls_media_sequence(bootstrap_manifest) != Some(0) {
        bootstrap_segments.reverse();
    }
    let bootstrap = bootstrap_segments.into_iter().find(|candidate| {
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

    use std::{cell::RefCell, collections::HashMap, time::Duration};

    use async_std::task::sleep;
    use js_sys::{Array, Error, Function, Object, Promise, Reflect};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{CustomEvent, CustomEventInit, Element, Event, HtmlMediaElement};

    use super::{
        HLS_LIVE_SYNC_SEGMENTS, classify_hls_level_transition, hls_timeline_rebase_position,
        js_error_message,
    };

    const SWARM_REQUEST_TIMEOUT_MS: f64 = 240_000.0;
    const MAX_NETWORK_RECOVERY_ATTEMPTS: u8 = 2;
    const MAX_HARD_RESTART_ATTEMPTS: u8 = 2;
    const HLS_BEGINNING_AUTOPLAY_HEAD_START: Duration = Duration::from_millis(500);
    const HLS_WARMUP_STOP_DELAY: Duration = Duration::from_millis(500);

    const HLS_CALLBACK_EVENTS: [&str; 5] = [
        "hlsError",
        "hlsBufferCreated",
        "hlsFragBuffered",
        "hlsLevelLoaded",
        "hlsManifestParsed",
    ];
    const HLS_DOM_EVENTS: [&str; 3] = ["play", "pause", "resize"];
    const NATIVE_DOM_EVENTS: [&str; 2] = ["error", "loadedmetadata"];
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

    struct Session {
        media: HtmlMediaElement,
        source: String,
        backend: Backend,
        hls_listener: Option<HlsListener>,
        dom_listener: Option<DomListener>,
        codec_required: bool,
        codec_pending: bool,
        load: LoadPhase,
        manifest_parsed: bool,
        playback_authorized: bool,
        autoplay_allowed: bool,
        autoplay_pending: bool,
        resume: bool,
        recovery_pending: bool,
        hard_restarts: u8,
        network_recoveries: u8,
        media_recoveries: u8,
        timeline_rebased: bool,
        initial_position: f64,
        rebase_position: Option<f64>,
        level_snapshots: HashMap<u64, (u64, bool, Option<f64>)>,
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
        hard_attempts: u8,
        timeline_rebased: bool,
        resume: bool,
        autoplay_allowed: bool,
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

    enum Autoplay {
        Policy,
        Resume,
    }

    impl Session {
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
            if let Some(position) = self.rebase_position {
                return position;
            }
            if !self.playback_authorized {
                return self.initial_position;
            }
            let current = self.media.current_time();
            if current.is_finite() && current > 0.0 {
                current
            } else {
                self.initial_position
            }
        }

        fn dispose(mut self) {
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
        launch(Launch {
            media,
            source: source.to_string(),
            loader: Some(hls_loader),
            initial_position: initial_start_position,
            rebase_position: None,
            hard_attempts: 0,
            timeline_rebased: false,
            resume: false,
            autoplay_allowed: true,
        })
        .await
    }

    async fn launch(mut request: Launch) -> Result<&'static str, JsValue> {
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
            media.set_src(&source);
            media.load();
            autoplay(epoch, Autoplay::Policy);
            if resume {
                autoplay(epoch, Autoplay::Resume);
            }
            return Ok("native");
        }

        request
            .media
            .set_attribute("data-weeb3-hls-mode", "hls.js")?;
        request
            .media
            .set_attribute("data-weeb3-hls-state", "loading-manifest")?;
        let codec = (
            request.source.contains("&codec-bootstrap="),
            request.source.contains("&codec-bootstrap=") && request.initial_position == 0.0,
        );
        if codec.1 {
            let _ = request.media.pause();
        }
        let config = hls_config(!request.source.contains("start=live"));
        let hls = construct_hls(
            hls_class
                .as_ref()
                .expect("an MSE-capable load must retain the hls.js class"),
            &config,
        )?;
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
        Session {
            media: request.media.clone(),
            source: request.source.clone(),
            backend,
            hls_listener: listeners.0,
            dom_listener: listeners.1,
            codec_required: codec.0,
            codec_pending: codec.1,
            load,
            manifest_parsed,
            playback_authorized: false,
            autoplay_allowed: request.autoplay_allowed,
            autoplay_pending: false,
            resume,
            recovery_pending: false,
            hard_restarts: request.hard_attempts,
            network_recoveries: 0,
            media_recoveries: 0,
            timeline_rebased: request.timeline_rebased,
            initial_position: request.initial_position,
            rebase_position,
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
                "hlsFragBuffered" => fragment_buffered(epoch),
                "hlsLevelLoaded" => level_loaded(epoch, &data),
                "hlsManifestParsed" => manifest_parsed(epoch),
                _ => {}
            },
            EventKind::Dom(name) => dom_event(epoch, &name),
        }
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
        let bootstrap = with_session(epoch, |session| {
            session.codec_pending.then(|| {
                session
                    .hls()
                    .cloned()
                    .map(|hls| (session.media.clone(), session.source.clone(), hls))
            })
        })
        .flatten()
        .flatten();
        let Some((media, source, hls)) = bootstrap else {
            return;
        };
        let target = Reflect::get(hls.as_ref(), &JsValue::from_str("liveSyncPosition"))
            .ok()
            .and_then(|value| value.as_f64())
            .filter(|target| target.is_finite() && *target >= 0.0);
        let Some(target) = target else { return };
        if !is_current(epoch) {
            return;
        }
        super::runtime::finish_hls_codec_bootstrap(&source);
        let clean = source
            .split("&codec-bootstrap=")
            .next()
            .unwrap_or(&source)
            .to_string();
        media.set_current_time(target);
        let ready = with_session(epoch, |session| {
            if !session.codec_pending {
                return false;
            }
            session.source = clean;
            session.codec_required = false;
            session.codec_pending = false;
            session.initial_position = -1.0;
            true
        })
        .unwrap_or(false);
        if ready {
            spawn_local(async move {
                Wait::Microtask.wait().await;
                if is_current(epoch) {
                    autoplay(epoch, Autoplay::Resume);
                    autoplay(epoch, Autoplay::Policy);
                }
            });
        }
    }

    fn fragment_buffered(epoch: u64) {
        let action = with_session(epoch, |session| {
            session.network_recoveries = 0;
            session.media_recoveries = 0;
            let stop = matches!(session.load, LoadPhase::Warmup)
                && session.media.paused()
                && !session.codec_pending;
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
                    if !matches!(session.load, LoadPhase::Warmup) || !session.media.paused() {
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
                && (session.rebase_position.is_some() || resume || session.media.paused());
            let position = session.rebase_position.unwrap_or(session.initial_position);
            let load = start.then(|| {
                session.load = LoadPhase::Warmup;
                session.hls().cloned().map(|hls| (hls, resume, position))
            });
            let head_start =
                start && !resume && position == 0.0 && !session.source.contains("start=live");
            (session.media.clone(), load.flatten(), head_start)
        });
        let Some((media, load, head_start)) = action else {
            return;
        };
        set_state(
            &media,
            "manifest-ready",
            "HLS manifest ready through weeb-3. Press Play if autoplay is blocked.",
        );
        if let Some((hls, resume, position)) = load {
            emit(&media, HLS_WARMUP_START_EVENT);
            if !is_current(epoch) || !start_at(epoch, &hls, position) {
                return;
            }
            if resume {
                autoplay(epoch, Autoplay::Resume);
            }
        }
        if head_start {
            spawn_local(async move {
                sleep(HLS_BEGINNING_AUTOPLAY_HEAD_START).await;
                if is_current(epoch) {
                    autoplay(epoch, Autoplay::Policy);
                }
            });
        } else {
            autoplay(epoch, Autoplay::Policy);
        }
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
        let action = with_session(epoch, |session| {
            let previous = session.level_snapshots.insert(level, (start, live, edge));
            if !classify_hls_level_transition(
                previous.map(|(start, live, _)| (start, live)),
                session.timeline_rebased,
                start,
            )
            .rebase
            {
                return None;
            }
            session.timeline_rebased = true;
            session.recovery_pending = true;
            let rebase = previous
                .and_then(|(_, _, edge)| edge)
                .zip(edge)
                .and_then(|(old, new)| {
                    hls_timeline_rebase_position(old, session.media.current_time(), new)
                });
            Some((
                session.media.clone(),
                Launch {
                    media: session.media.clone(),
                    source: session.source.clone(),
                    loader: None,
                    initial_position: session.initial_position,
                    rebase_position: rebase,
                    hard_attempts: session.hard_restarts,
                    timeline_rebased: true,
                    resume: session.playback_authorized,
                    autoplay_allowed: session.autoplay_allowed,
                },
                session.hls().cloned(),
            ))
        })
        .flatten();
        let Some((media, request, retiring)) = action else {
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
                    let pending = session.autoplay_pending;
                    let first = matches!(session.load, LoadPhase::Cold);
                    let resume = !first && !matches!(session.load, LoadPhase::Warmup);
                    let position = session.rebase_position.unwrap_or(session.initial_position);
                    if !pending || first {
                        session.load = LoadPhase::Started;
                    }
                    if !pending {
                        session.playback_authorized = true;
                        session.autoplay_allowed = true;
                    }
                    let load = session.hls().cloned().and_then(|hls| {
                        (first || resume).then_some((hls, first.then_some(position)))
                    });
                    (session.media.clone(), load, !pending)
                });
                let Some((media, load, user)) = action else {
                    return;
                };
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
                    if !session.playback_authorized {
                        return None;
                    }
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

    fn handle_error(epoch: u64, data: JsValue) {
        let codec_bootstrap = js_string_property(&data, "type").as_deref() == Some("mediaError")
            && js_string_property(&data, "details").as_deref() == Some("bufferAppendError")
            && js_string_property(&data, "sourceBufferName").as_deref() == Some("video")
            && js_property(&data, "error")
                .is_some_and(|error| js_error_message(&error).contains("video SourceBuffer"))
            && with_session(epoch, |session| {
                if session.codec_pending
                    || !session.source.contains("start=live")
                    || session.hard_restarts >= MAX_HARD_RESTART_ATTEMPTS
                {
                    return false;
                }
                session.codec_required = true;
                session.codec_pending = true;
                session.initial_position = 0.0;
                true
            })
            .unwrap_or(false);
        if codec_bootstrap {
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
            session.recovery_pending = true;
            if session.codec_required {
                session.codec_pending = true;
            }
            let source = if session.codec_required {
                let source = session
                    .source
                    .split("&codec-bootstrap=")
                    .next()
                    .unwrap_or(&session.source);
                format!("{source}&codec-bootstrap={epoch}")
            } else {
                session.source.clone()
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
                        hard_attempts: attempt,
                        timeline_rebased,
                        resume: session.playback_authorized || session.resume,
                        autoplay_allowed: session.autoplay_allowed,
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

    fn autoplay(epoch: u64, intent: Autoplay) {
        let media = with_session(epoch, |session| {
            let allowed = matches!(intent, Autoplay::Resume)
                || (session.autoplay_allowed && session.media.autoplay());
            (allowed
                && !session.codec_pending
                && !session.playback_authorized
                && !session.autoplay_pending)
                .then(|| {
                    session.autoplay_pending = true;
                    session.media.clone()
                })
        })
        .flatten();
        let Some(media) = media else { return };
        media
            .set_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE, "1")
            .ok();
        spawn_local(async move {
            let result = match media.play() {
                Ok(promise) => JsFuture::from(promise).await.map(|_| ()),
                Err(error) => Err(error),
            };
            let authorized = result.is_ok();
            let settled = with_session(epoch, |session| {
                if !session.autoplay_pending {
                    return None;
                }
                session.autoplay_pending = false;
                session.resume = false;
                if authorized {
                    session.playback_authorized = true;
                    if matches!(session.load, LoadPhase::Warmup) {
                        session.load = LoadPhase::Started;
                    }
                }
                Some(session.media.clone())
            })
            .flatten();
            let Some(media) = settled else { return };
            media.remove_attribute(HLS_AUTOPLAY_PENDING_ATTRIBUTE).ok();
            if authorized {
                media
                    .set_attribute(HLS_PLAYBACK_AUTHORIZED_ATTRIBUTE, "1")
                    .ok();
                emit(&media, HLS_AUTOPLAY_AUTHORIZED_EVENT);
            } else {
                set_state(
                    &media,
                    "autoplay-blocked",
                    "HLS startup media is warming. Autoplay was blocked; press Play to start playback.",
                );
            }
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
    use super::*;
    use std::{
        cell::RefCell,
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
    use libp2p::futures::stream::{self, FuturesUnordered, StreamExt};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{Element, HtmlMediaElement};

    use crate::{
        Weeb3,
        bzz_stream::{
            DeferredRawFeedPayload, RawFeedPayload, RetainedRawFeedPayloadProbe,
            StartupRawFeedPayload, acquire_deferred_raw_feed_payload,
            acquire_latest_raw_feed_payload_bounded_from, acquire_latest_raw_feed_payload_from,
            acquire_latest_raw_feed_payload_startup,
            acquire_latest_raw_feed_payload_startup_observing_deferred,
            acquire_raw_feed_payload_at_index, acquire_raw_feed_payload_at_index_bounded,
            acquire_raw_feed_payload_at_index_retained_status,
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
    }

    thread_local! {
        static FEED_ROUTE_CACHE: RefCell<FeedRegistry> = RefCell::new(FeedRegistry::new());
        static HLS_PLAYBACK: RefCell<HlsPlaybackState> = RefCell::new(HlsPlaybackState::new());
        static HLS_CODEC_BOOTSTRAP_PRESENTATION: RefCell<Option<HlsCodecBootstrapPresentation>> =
            const { RefCell::new(None) };
        static HLS_ASSET_CACHE: RefCell<HlsAssetCache> = RefCell::new(HlsAssetCache::new());
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
    const HLS_STARTUP_LOOKAHEAD_BYTES: u64 = 2 * MEDIA_STARTUP_RESPONSE_BYTES;
    const HLS_PROGRESSIVE_SUCCESSOR_PREFIX_BYTES: u64 = MEDIA_STORAGE_WINDOW_BYTES * 3;

    fn hls_segment_progress_detail(reference: &str, size: Option<usize>) -> String {
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
        checking_token: u64,
        confirmed_head_index: Option<u64>,
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

    #[derive(Clone, Copy, Eq, PartialEq)]
    struct PlaybackStamp {
        generation: u64,
        timeline_epoch: u64,
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    struct PrefetchTicket {
        stamp: PlaybackStamp,
        plan_id: u64,
        schedule_id: u64,
    }

    struct HlsPrefetchTrack {
        schedule_id: u64,
        last_foreground_position: usize,
        running: Option<PrefetchTicket>,
        last_touch: u64,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum HlsPrefetchMode {
        Inactive,
        StartupOnly,
        Sustained,
    }

    struct BeginningRunway {
        first: String,
        successor: Option<String>,
        successor_prefix_started: bool,
        successor_prefix_ready: bool,
    }

    struct HlsPrefetchSession {
        generation: u64,
        timeline_epoch: u64,
        schedule_sequence: u64,
        track_touch_sequence: u64,
        mode: HlsPrefetchMode,
        client: Option<Arc<Weeb3>>,
        feed_identity: Option<(String, String)>,
        runway: Option<BeginningRunway>,
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
                runway: None,
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

        fn stamp(&self) -> PlaybackStamp {
            PlaybackStamp {
                generation: self.generation,
                timeline_epoch: self.timeline_epoch,
            }
        }

        fn advance_generation(&mut self) -> u64 {
            self.generation = next_media_generation();
            self.runway = None;
            self.startup_overlap_plans.clear();
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
            self.timeline_epoch = next_nonzero_generation(self.timeline_epoch);
            self.runway = None;
            self.timeline_epoch
        }

        fn ticket_current(&self, ticket: PrefetchTicket, sustained: bool) -> bool {
            (!sustained || self.mode == HlsPrefetchMode::Sustained)
                && self.mode != HlsPrefetchMode::Inactive
                && !self.timeline_rebasing
                && self.stamp() == ticket.stamp
                && self.tracks.get(&ticket.plan_id).is_some_and(|track| {
                    track.schedule_id == ticket.schedule_id && track.running == Some(ticket)
                })
        }

        fn progressive_current(&self, reference: &str, stamp: PlaybackStamp) -> bool {
            self.stamp() == stamp
                && !self.live_start
                && !self.timeline_rebasing
                && !self.sequence_zero_runway_closed
                && self.runway.as_ref().is_some_and(|runway| {
                    runway.first == reference
                        || runway
                            .successor
                            .as_ref()
                            .is_some_and(|successor| successor == reference)
                })
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
            }
        }
    }

    #[derive(Clone)]
    struct HlsForegroundContext {
        stamp: PlaybackStamp,
        ticket: Option<PrefetchTicket>,
        cursor: Option<HlsMediaCursor>,
        progressive_successor_prefix_ready: bool,
        seek_successor: Option<String>,
    }

    #[derive(Default)]
    struct HlsAssetCache {
        metadata: HashMap<String, (HlsAssetMetadata, f64)>,
        sizes: HashMap<String, u64>,
        body_order: VecDeque<String>,
        bodies: HashMap<String, Arc<[u8]>>,
        body_pending: HashMap<String, PendingHlsPayload>,
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
            self.body_order.retain(|key| key != reference);
            if foreground {
                self.body_order.push_front(reference.to_string());
            } else {
                self.body_order.push_back(reference.to_string());
            }
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
            let owns_pending = self.body_pending.get(reference).is_some_and(|pending| {
                pending.generation == generation && pending.load_id == load_id
            });
            if let Ok(body) = &result {
                self.remember_size(reference, u64::try_from(body.len()).unwrap_or(u64::MAX));
                if self.retain_completed {
                    self.remember_body(reference.to_string(), body.clone(), hot);
                }
            }
            if owns_pending && let Some(pending) = self.body_pending.remove(reference) {
                pending.finish(result);
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
            self.body_order.retain(|key| key != &reference);
            if hot {
                self.body_order.push_back(reference.clone());
            } else {
                self.body_order.push_front(reference.clone());
            }
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
        let progressive_stamp = HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            let stamp = session.stamp();
            (stamp.generation != 0 && session.progressive_current(&reference, stamp))
                .then_some(stamp)
        });
        let progressive_start = progressive_stamp.is_some();
        if method != "HEAD" && range.is_none() && progressive_start {
            let Some(resolved) = resolve_hls_asset(weeb3.clone(), reference.clone()).await else {
                return FetchResponse::error(503, "weeb-3 did not retrieve resource");
            };
            if !resolved.metadata.is_manifest && resolved.prefetched_body.is_none() {
                hls_foreground_context(&reference, false);
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

        if method != "HEAD" && !progressive_start {
            let _ = wait_for_pending_hls_payload(&reference).await;
        }
        let Some(resolved) = resolve_hls_asset(weeb3.clone(), reference.clone()).await else {
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
                Err(_) => return FetchResponse::error(502, too_large),
            };
            let end_index = match usize::try_from(end.saturating_add(1)) {
                Ok(end) => end,
                Err(_) => return FetchResponse::error(502, too_large),
            };
            let body = match &body {
                Either::Left(body) => body.get(start_index..end_index),
                Either::Right(body) => body.get(start_index..end_index),
            };
            let Some(selected) = body else {
                return FetchResponse::error(502, outside);
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
                        progressive_stamp.map(|stamp| stamp.generation),
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
        if !is_manifest && !expected_len.is_some_and(|expected| bytes.len() == expected) {
            return FetchResponse::error(502, "weeb-3 returned a short HLS byte range");
        }
        if !is_manifest
            && (end.saturating_add(1) >= MEDIA_STORAGE_WINDOW_BYTES.saturating_mul(2)
                || end.saturating_add(1) == size)
            && let Some((successor, stamp)) = take_hls_progressive_successor(&reference)
        {
            let successor_client = weeb3.clone();
            let source_reference = reference.clone();
            spawn_local(async move {
                async_std::task::sleep(Duration::from_millis(MEDIA_PREFETCH_BATCH_YIELD_MS)).await;
                if !hls_progressive_successor_admission_is_current(&source_reference, stamp) {
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
                if !hls_progressive_successor_admission_is_current(&source_reference, stamp) {
                    return;
                }
                let prefix_end = size
                    .saturating_sub(1)
                    .min(HLS_PROGRESSIVE_SUCCESSOR_PREFIX_BYTES.saturating_sub(1));
                let prefix = successor_client
                    .retrieve_hls_payload_range(successor, 0, prefix_end, Some(stamp.generation))
                    .await;
                if u64::try_from(prefix.len()).ok() == Some(prefix_end.saturating_add(1)) {
                    mark_hls_progressive_successor_prefix_ready(&source_reference, stamp);
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
        let Some(mut segments) = hls_segment_identities(manifest) else {
            return;
        };
        HLS_PLAYBACK.with(|playback| {
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
            playback
                .plans
                .install_with_early_overlap_limit(references, overlap, live_start);
        });
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
        HLS_ASSET_CACHE.with(|cache| cache.borrow_mut().retain_completed = true);
        let generation = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            session.client = Some(client.clone());
            session.feed_identity = Some((owner.to_ascii_lowercase(), topic.to_ascii_lowercase()));
            session.sequence_zero_runway_closed = false;
            session.presentation_id = presentation_id;
            session.live_start = live_start;
            session.live_history_active = false;
            session.startup_deadline_ms = hls_monotonic_now_ms()
                .map(|now| now + HLS_INITIAL_RESPONSE_BUDGET_MS)
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
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            (session.generation != 0
                && session
                    .client
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, client))
                && session.feed_identity.as_ref() == Some(&identity))
            .then_some(session.generation)
        })
    }

    fn take_hls_progressive_successor(reference: &str) -> Option<(String, PlaybackStamp)> {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            let stamp = session.stamp();
            if stamp.generation == 0
                || session.live_start
                || session.timeline_rebasing
                || !session.progressive_current(reference, stamp)
            {
                return None;
            }
            let runway = session.runway.as_mut()?;
            if runway.first != reference || runway.successor_prefix_started {
                return None;
            }
            let successor = runway.successor.clone()?;
            runway.successor_prefix_started = true;
            Some((successor, stamp))
        })
    }

    fn mark_hls_progressive_successor_prefix_ready(reference: &str, stamp: PlaybackStamp) {
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            if session.progressive_current(reference, stamp)
                && let Some(runway) = session.runway.as_mut()
                && runway.first == reference
                && runway.successor_prefix_started
            {
                runway.successor_prefix_ready = true;
            }
        });
    }

    fn hls_progressive_successor_admission_is_current(
        reference: &str,
        stamp: PlaybackStamp,
    ) -> bool {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            session.progressive_current(reference, stamp)
                && session
                    .runway
                    .as_ref()
                    .is_some_and(|runway| runway.first == reference)
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
            if mode == HlsPrefetchMode::Inactive {
                playback.session.sequence_zero_runway_closed = true;
            }
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
        HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            if session.stamp() == ticket.stamp
                && session
                    .tracks
                    .get(&ticket.plan_id)
                    .is_some_and(|track| track.schedule_id == ticket.schedule_id)
            {
                session.tracks.remove(&ticket.plan_id);
                session.startup_overlap_plans.remove(&ticket.plan_id);
            }
        });
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
            client.map(|client| (client, generation))
        });
        if let Some((client, generation)) = publish {
            publish_hls_stream_generation(client, generation);
        }
    }

    fn hls_foreground_context(reference: &str, cached: bool) -> HlsForegroundContext {
        let reference = reference.to_ascii_lowercase();
        let mut publish = None;
        let (stamp, ticket, cursor, progressive_ready, seek_successor) =
            HLS_PLAYBACK.with(|playback| {
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
                let session = &mut playback.session;
                if session.generation == 0 {
                    session.advance_generation();
                }

                let mut seek_successor = None;
                let mut schedule_id = None;
                if let Some(cursor) = &cursor {
                    let transition = session.tracks.get(&cursor.plan.id).map(|track| {
                        if cached && cursor.position < track.last_foreground_position {
                            (false, track.last_foreground_position)
                        } else {
                            (
                                track.last_foreground_position.abs_diff(cursor.position) > 1,
                                cursor.position,
                            )
                        }
                    });
                    if transition.is_some_and(|transition| transition.0) {
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
                        session.schedule_sequence =
                            next_nonzero_generation(session.schedule_sequence);
                        session.tracks.insert(
                            cursor.plan.id,
                            HlsPrefetchTrack {
                                schedule_id: session.schedule_sequence,
                                last_foreground_position: cursor.position,
                                running: None,
                                last_touch: 0,
                            },
                        );
                    }
                    session.track_touch_sequence =
                        next_nonzero_generation(session.track_touch_sequence);
                    let touch = session.track_touch_sequence;
                    if let Some(track) = session.tracks.get_mut(&cursor.plan.id) {
                        track.last_foreground_position = transition
                            .map(|transition| transition.1)
                            .unwrap_or(cursor.position);
                        track.last_touch = touch;
                        schedule_id = Some(track.schedule_id);
                    }

                    session.prune_tracks(cursor.plan.id, superseded);
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
                let progressive_ready =
                    cursor.as_ref().is_some_and(|cursor| {
                        !session.live_start
                            && !session.timeline_rebasing
                            && cursor.position == 1
                            && session.runway.as_ref().is_some_and(|runway| {
                                runway.successor_prefix_started
                                    && runway.successor_prefix_ready
                                    && runway
                                        .successor
                                        .as_ref()
                                        .is_some_and(|successor| successor == &reference)
                                    && cursor.plan.references.first().is_some_and(|first| {
                                        first.eq_ignore_ascii_case(&runway.first)
                                    })
                            })
                    });
                (stamp, ticket, cursor, progressive_ready, seek_successor)
            });
        if let Some((client, generation)) = publish {
            publish_hls_stream_generation(client, generation);
        }
        HlsForegroundContext {
            stamp,
            ticket,
            cursor,
            progressive_successor_prefix_ready: progressive_ready,
            seek_successor,
        }
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
        let head_ready = cached || context.progressive_successor_prefix_ready;
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
                false,
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
        if !HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
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
        let stamp = HLS_PLAYBACK.with(|playback| {
            let mut playback = playback.borrow_mut();
            let session = &mut playback.session;
            if session.generation != generation
                || session.live_start
                || session.timeline_rebasing
                || session.sequence_zero_runway_closed
            {
                return None;
            }
            session.runway = Some(BeginningRunway {
                first: reference.clone(),
                successor,
                successor_prefix_started: false,
                successor_prefix_ready: false,
            });
            Some(session.stamp())
        });
        let Some(stamp) = stamp else {
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
            if !hls_progressive_successor_admission_is_current(&reference, stamp) {
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

        spawn_local(stabilize_claimed_feed_route(
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
        let canonical_token =
            claim_feed_route_check(&cache_key, Some(snapshot.index)).map(|(_, token)| token);
        if let Some(token) = canonical_token {
            let fallback_client = weeb3.clone();
            let fallback_cache_key = cache_key.clone();
            let fallback_owner = owner.clone();
            let fallback_topic = topic.clone();
            spawn_local(async move {
                async_std::task::sleep(HLS_SEQUENCE_ZERO_CANONICAL_EXCLUSIVITY).await;
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

    struct LiveHistoryCollector {
        weeb3: Arc<Weeb3>,
        owner: String,
        topic: String,
        presentation_id: u64,
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
                presentation_id,
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
                capacity_parallelism: 0,
                capacity_priced_peers: 0,
                capacity_checked_at: 0.0,
                highest_authenticated_positive_index: initial.index,
            })
        }

        fn current(&self) -> bool {
            live_history_session_is_current(
                &self.weeb3,
                &self.owner,
                &self.topic,
                self.presentation_id,
            ) && active_profile().swarm_network_id == self.network_id
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
                self.states
                    .insert(index, LiveHistoryProbeState::Unavailable);
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
                        self.states
                            .insert(index, LiveHistoryProbeState::Unavailable);
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
                let payload = client.hls_deferred_feed_payload(deferred).await;
                (index, payload)
            }));
            Ok(())
        }

        fn adopt_inline_direct(&mut self, payload: RawFeedPayload) -> Result<(), String> {
            if self.direct_in_flight.is_some() || self.direct_candidate.is_some() {
                return Ok(());
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
            let valid = hls_is_finalized(&candidate.bytes)
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
                        .insert(candidate.index, LiveHistoryProbeState::Unavailable);
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
                    Some(LiveHistoryProbeState::Found | LiveHistoryProbeState::Unavailable)
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
            candidate: &[u8],
        ) -> Result<Option<Vec<u8>>, String> {
            let Some(timeline) = hls_complete_history_timeline(candidate) else {
                return Ok(None);
            };
            if timeline.sequence != 0 || hls_is_finalized(candidate) {
                return Ok(None);
            }
            if !self
                .windows
                .values()
                .all(|window| hls_sequence_zero_timeline_covers_head(window, &timeline))
            {
                return Err(
                    "The authenticated Live checkpoint contradicts the pinned timeline."
                        .to_string(),
                );
            }
            Ok(hls_sequence_zero_sparse_tail_from_timeline(
                candidate, &timeline,
            ))
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
                        self.start_deferred_direct(deferred, true)?;
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
                        self.states
                            .insert(index, LiveHistoryProbeState::Unavailable);
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
                    self.states
                        .insert(index, LiveHistoryProbeState::Unavailable);
                    return Ok(());
                }
                self.retained_bytes = total;
                self.direct_reserved_bytes = payload.bytes.len();
                self.direct_required = false;
                self.direct_index = Some(payload.index);
                self.states
                    .insert(index, LiveHistoryProbeState::Unavailable);
                self.direct_candidate = Some(payload);
                return Ok(());
            }
            if finalized_sequence_zero {
                self.states
                    .insert(index, LiveHistoryProbeState::Unavailable);
                return Ok(());
            }
            if hls_is_long_sequence_zero_checkpoint(&payload.bytes)
                && self
                    .windows
                    .last_key_value()
                    .is_some_and(|(head_index, _)| index <= *head_index)
            {
                self.states
                    .insert(index, LiveHistoryProbeState::Unavailable);
                return Ok(());
            }
            if let Some(sparse_tail) =
                self.verified_sequence_zero_checkpoint_tail(&payload.bytes)?
            {
                self.remember_window(RawFeedPayload {
                    index,
                    bytes: sparse_tail,
                })?;
                self.states.insert(index, LiveHistoryProbeState::Found);
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
                self.states
                    .insert(index, LiveHistoryProbeState::Unavailable);
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
                self.capacity_parallelism =
                    hls_sparse_history_parallelism(priced_peers, available_slots);
                self.capacity_checked_at = now;
            }
            let parallelism = self.capacity_parallelism;
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
                    let result = client
                        .hls_feed_payload_at_index_retained_status(owner, topic, index)
                        .await;
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
                    hls_direct_archive_disposition(
                        index,
                        self.windows
                            .last_key_value()
                            .map_or(index, |(head_index, _)| *head_index),
                        self.highest_authenticated_positive_index,
                        &payload.bytes,
                    )
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
                        self.states
                            .insert(index, LiveHistoryProbeState::Unavailable);
                        self.direct_candidate = Some(candidate);
                    }
                    Some(HlsDirectArchiveDisposition::Stale)
                    | Some(HlsDirectArchiveDisposition::Nonterminal)
                    | Some(HlsDirectArchiveDisposition::Unsupported) => {
                        self.drop_direct_work();
                    }
                    Some(HlsDirectArchiveDisposition::SequenceZeroCheckpoint) => {
                        let checkpoint = payload.expect("the disposition requires a payload");
                        let sparse_tail =
                            match self.verified_sequence_zero_checkpoint_tail(&checkpoint.bytes) {
                                Ok(sparse_tail) => sparse_tail,
                                Err(error) => {
                                    self.drop_direct_work();
                                    return Err(error);
                                }
                            };
                        self.drop_direct_work();
                        if let Some(sparse_tail) = sparse_tail {
                            self.remember_window(RawFeedPayload {
                                index,
                                bytes: sparse_tail,
                            })?;
                            self.states.insert(index, LiveHistoryProbeState::Found);
                        }
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
            {
                return None;
            }
            let now = js_sys::Date::now();
            state.confirmed_head_index = Some(head.index);
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
                collector.start_deferred_direct(observed_deferred, true)?;
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
                let (advancing, task) = match candidate.admission {
                    FeedCandidateAdmission::Seed { advancing } => (advancing, None),
                    FeedCandidateAdmission::Task {
                        token,
                        expected_index,
                        require_confirmed_same_index,
                    } => {
                        if existing.checking_token != token
                            || expected_index.is_some_and(|index| {
                                existing.snapshot.index != index || candidate.index <= index
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
                    let Some(update) = feed_route_update_body(
                        &existing.snapshot.body,
                        &existing.source_body,
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
                    body_tracks_source: true,
                    source_endlist_confirmed: candidate.head_confirmed,
                    checking_token: 0,
                    confirmed_head_index: candidate.head_confirmed.then_some(candidate.index),
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

    fn active_live_history_feed_cache_key() -> Option<String> {
        HLS_PLAYBACK.with(|playback| {
            let playback = playback.borrow();
            let session = &playback.session;
            let (owner, topic) = session.feed_identity.as_ref()?;
            (session.live_start && session.live_history_active)
                .then(|| sequence_zero_feed_cache_key(owner, topic, session.presentation_id))
        })
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
            let admission_client = weeb3.clone();
            let admission_cache_key = cache_key.to_string();
            let admission_owner = owner.to_string();
            let admission_topic = topic.to_string();
            let admission_deadline =
                js_sys::Date::now() + HLS_FEED_WAVE_CREDIT_WAIT.as_millis() as f64;
            let Some(latest) = acquire_latest_raw_feed_payload_bounded_from(
                owner.to_string(),
                topic.to_string(),
                initial,
                force_coarse,
                &weeb3.chunk_port.0,
                move |probe_count| {
                    let still_current = followup_mode != FeedFollowupMode::SequenceZeroPresentation
                        || sequence_zero_followup_is_current(
                            &admission_client,
                            &admission_cache_key,
                            &admission_owner,
                            &admission_topic,
                        );
                    let credit_client = admission_client.clone();
                    let current_client = admission_client.clone();
                    let current_cache_key = admission_cache_key.clone();
                    let current_owner = admission_owner.clone();
                    let current_topic = admission_topic.clone();
                    async move {
                        if !still_current
                            || !await_feed_probe_wave_credit(
                                credit_client,
                                network_id,
                                probe_count,
                                admission_deadline,
                            )
                            .await
                        {
                            return false;
                        }
                        followup_mode != FeedFollowupMode::SequenceZeroPresentation
                            || sequence_zero_followup_is_current(
                                &current_client,
                                &current_cache_key,
                                &current_owner,
                                &current_topic,
                            )
                    }
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
            || (followup_mode == FeedFollowupMode::SequenceZeroPresentation
                && !sequence_zero_followup_is_current(&weeb3, cache_key, owner, topic))
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
                mode: followup_mode,
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
        let network_id = active_profile().swarm_network_id;
        let Some((mut current_index, checking_token)) = claim_feed_route_check(&cache_key, None)
        else {
            return;
        };

        spawn_local(async move {
            if weeb3.get_network_id().await != network_id
                || active_profile().swarm_network_id != network_id
                || (followup_mode == FeedFollowupMode::SequenceZeroPresentation
                    && !sequence_zero_followup_is_current(&weeb3, &cache_key, &owner, &topic))
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
            let mut consecutive_sequence_zero_missing = 0usize;
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
            let exact_followup_limit = match followup_mode {
                FeedFollowupMode::Canonical => FEED_FOLLOWUP_BATCH_LIMIT,
                FeedFollowupMode::SequenceZeroPresentation => {
                    HLS_SEQUENCE_ZERO_PRESENTATION_BATCH_LIMIT
                }
            };
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
                .buffered(match followup_mode {
                    FeedFollowupMode::Canonical => 1,
                    FeedFollowupMode::SequenceZeroPresentation => {
                        HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL
                    }
                });
            while let Some((next_index, next)) = exact_followups.next().await {
                if weeb3.get_network_id().await != network_id
                    || active_profile().swarm_network_id != network_id
                    || (followup_mode == FeedFollowupMode::SequenceZeroPresentation
                        && !sequence_zero_followup_is_current(&weeb3, &cache_key, &owner, &topic))
                {
                    break;
                }
                let Some(next) = next else {
                    let can_skip = match followup_mode {
                        FeedFollowupMode::Canonical => !skipped_missing_index,
                        FeedFollowupMode::SequenceZeroPresentation => {
                            consecutive_sequence_zero_missing
                                < HLS_SEQUENCE_ZERO_FOLLOWUP_MAX_PARALLEL
                        }
                    };
                    if !can_skip {
                        break;
                    }
                    skipped_missing_index = true;
                    consecutive_sequence_zero_missing =
                        consecutive_sequence_zero_missing.saturating_add(1);
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
                        mode: followup_mode,
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
                if followup_mode == FeedFollowupMode::SequenceZeroPresentation {
                    consecutive_sequence_zero_missing = 0;
                }
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
                || (followup_mode == FeedFollowupMode::SequenceZeroPresentation
                    && !sequence_zero_followup_is_current(&weeb3, &cache_key, &owner, &topic))
            {
                let _ = release_feed_route_check(&cache_key, checking_token);
                return;
            }

            if !refresh_head
                && (saw_tentative_endlist
                    || (followup_mode == FeedFollowupMode::SequenceZeroPresentation
                        && successful_followups == 0
                        && skipped_missing_index)
                    || (followup_mode == FeedFollowupMode::Canonical
                        && (successful_followups >= FEED_FOLLOWUP_BATCH_LIMIT
                            || recovered_missing_index)))
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
                    }
                    Ok(snapshot)
                }
                (HlsStart::Live, Some(snapshot)) => prepare_live_history(
                    snapshot_client,
                    snapshot_owner,
                    snapshot_topic,
                    presentation_id,
                    snapshot,
                    startup_deferred_in.and_then(|input| input.try_recv().ok()),
                )
                .await
                .map(Some),
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
        HLS_ASSET_CACHE.with(|cache| cache.borrow_mut().clear_completed_bodies());
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use runtime::{
    attach_hls_feed_player, hls_payload_cache_body_bytes, open_hls_feed_view,
    release_hls_for_bzz_view, release_hls_view, try_fetch_response,
};
