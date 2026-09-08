//! Minimal append-only HLS feed reader with a duration-based live startup runway.

use std::fmt::Write;

use crate::stream_conventions::HlsStart;

pub(crate) const HLS_LIVE_STARTUP_BUFFER_SECONDS: f64 = 8.0;
pub(crate) const HLS_LIVE_EDGE_SEGMENTS: usize = 3;
pub(crate) const HLS_LIVE_BODY_RUNWAY_SEGMENTS: usize = 4;
pub(crate) const MAX_STREAM_FEED_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

const HLS_HEADER: &str = "#EXTM3U";
const HLS_ENDLIST: &str = "#EXT-X-ENDLIST";
const HLS_GAP: &str = "#EXT-X-GAP";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HlsSegment {
    pub(crate) reference: String,
    pub(crate) duration: f64,
    pub(crate) gap: bool,
    pub(crate) discontinuity_sequence: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HlsPlaylist {
    pub(crate) sequence: u64,
    pub(crate) discontinuity_sequence: u64,
    pub(crate) target_duration: u64,
    pub(crate) segments: Vec<HlsSegment>,
    pub(crate) finalized: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HlsStartupPlan {
    pub(crate) bootstrap_position: f64,
    pub(crate) codec_bootstrap: bool,
    pub(crate) play_position: f64,
    pub(crate) runway_end: f64,
    pub(crate) duration: f64,
}

pub(crate) struct PreparedHlsFeed {
    pub(crate) source: String,
    pub(crate) plan: HlsStartupPlan,
}

#[derive(Default)]
pub(crate) struct HlsTailFailure {
    key: Option<(u64, u64, String)>,
}

impl HlsTailFailure {
    pub(crate) fn record(&mut self, snapshot: u64, sequence: u64, reference: &str) -> bool {
        let matches =
            self.key
                .as_ref()
                .is_some_and(|(current_snapshot, current_sequence, current)| {
                    (*current_snapshot, *current_sequence, current.as_str())
                        == (snapshot, sequence, reference)
                });
        if !matches {
            self.key = Some((snapshot, sequence, reference.to_string()));
        }
        matches
    }

    pub(crate) fn clear(&mut self) {
        self.key = None;
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

impl HlsPlaylist {
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES || !is_hls_manifest(bytes) {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        let mut sequence = None;
        let mut discontinuity_sequence = None;
        let mut target_duration = None;
        for line in text.lines().map(str::trim) {
            for (prefix, field) in [
                ("#EXT-X-MEDIA-SEQUENCE:", &mut sequence),
                (
                    "#EXT-X-DISCONTINUITY-SEQUENCE:",
                    &mut discontinuity_sequence,
                ),
                ("#EXT-X-TARGETDURATION:", &mut target_duration),
            ] {
                if let Some(value) = line.strip_prefix(prefix)
                    && field.replace(value.trim().parse::<u64>().ok()?).is_some()
                {
                    return None;
                }
            }
        }
        let discontinuity_sequence = discontinuity_sequence.unwrap_or(0);
        let segments = parse_segment_lines(text, discontinuity_sequence)?;
        if segments.is_empty() {
            return None;
        }
        let measured_target = segments
            .iter()
            .map(|segment| segment.duration.round() as u64)
            .max()
            .unwrap_or(1)
            .max(1);
        Some(Self {
            sequence: sequence.unwrap_or(0),
            discontinuity_sequence,
            target_duration: target_duration
                .unwrap_or(measured_target)
                .max(measured_target),
            segments,
            finalized: hls_has_terminal_endlist(text),
        })
    }

    pub(crate) fn duration(&self) -> f64 {
        self.segments.iter().map(|segment| segment.duration).sum()
    }

    fn end_sequence(&self) -> Option<u64> {
        self.sequence
            .checked_add(self.segments.len().try_into().ok()?)
    }

    pub(crate) fn startup_plan(&self, start: HlsStart) -> Option<HlsStartupPlan> {
        if start == HlsStart::Live {
            let (position, segment) = self
                .segments
                .iter()
                .enumerate()
                .rfind(|(_, segment)| !segment.gap)?;
            let sequence = self.sequence.checked_add(u64::try_from(position).ok()?)?;
            return self
                .anchored_startup_plan(sequence, &segment.reference)
                .map(|(plan, _)| plan);
        }
        let (first, segment) = self
            .segments
            .iter()
            .enumerate()
            .find(|(_, segment)| !segment.gap)?;
        let play_position = self.segments[..first]
            .iter()
            .map(|segment| segment.duration)
            .sum::<f64>();
        let runway_end = play_position + segment.duration;
        let duration = self.duration();
        if !play_position.is_finite()
            || !runway_end.is_finite()
            || !duration.is_finite()
            || runway_end <= play_position
        {
            return None;
        }
        Some(HlsStartupPlan {
            bootstrap_position: play_position,
            codec_bootstrap: false,
            play_position,
            runway_end,
            duration,
        })
    }

    pub(crate) fn anchored_startup_plan(
        &self,
        anchor_sequence: u64,
        anchor_reference: &str,
    ) -> Option<(HlsStartupPlan, usize)> {
        let anchor = usize::try_from(anchor_sequence.checked_sub(self.sequence)?).ok()?;
        let segment = self.segments.get(anchor)?;
        if segment.gap || segment.reference != anchor_reference {
            return None;
        }
        let mut first = anchor;
        let mut last = anchor;
        let mut seconds = segment.duration;
        while seconds < HLS_LIVE_STARTUP_BUFFER_SECONDS
            && let Some(next) = self.segments.get(last + 1).filter(|segment| !segment.gap)
        {
            seconds += next.duration;
            last += 1;
        }
        while seconds < HLS_LIVE_STARTUP_BUFFER_SECONDS && first > 0 {
            let previous = &self.segments[first - 1];
            if previous.gap {
                break;
            }
            seconds += previous.duration;
            first -= 1;
        }
        if !seconds.is_finite() || seconds < HLS_LIVE_STARTUP_BUFFER_SECONDS {
            return None;
        }
        let mut plan = self.startup_plan(HlsStart::Beginning)?;
        plan.play_position = self.segments[..first]
            .iter()
            .map(|segment| segment.duration)
            .sum();
        plan.runway_end = self.segments[..=last]
            .iter()
            .map(|segment| segment.duration)
            .sum();
        plan.codec_bootstrap = self.sequence == 0 && plan.play_position > plan.bootstrap_position;
        (plan.runway_end.is_finite() && plan.runway_end > plan.play_position)
            .then_some((plan, first))
    }

    pub(crate) fn merge_tail(&mut self, bytes: &[u8]) -> Option<usize> {
        let text = std::str::from_utf8(bytes).ok()?;
        let candidates = parse_segment_lines(text, 0)
            .filter(|segments| !segments.is_empty())
            .or_else(|| {
                text.split_once('\n')
                    .and_then(|(_, complete)| parse_segment_lines(complete, 0))
            })?;
        self.merge_segments(candidates, hls_has_terminal_endlist(text))
    }

    pub(crate) fn merge_playlist(&mut self, candidate: Self) -> Option<usize> {
        let (appended, first) = self.merge_extension(&candidate)?;
        if candidate.sequence < self.sequence {
            self.sequence = candidate.sequence;
            self.discontinuity_sequence = candidate.discontinuity_sequence;
            self.segments = candidate.segments;
        } else if appended != 0 {
            self.segments
                .extend(candidate.segments.into_iter().skip(first));
        }
        self.finalized = candidate.finalized;
        self.target_duration = self.target_duration.max(candidate.target_duration);
        Some(appended)
    }

    fn merge_extension(&self, candidate: &Self) -> Option<(usize, usize)> {
        let current_end = self.end_sequence()?;
        let candidate_end = candidate.end_sequence()?;
        if candidate.sequence > current_end || candidate_end < current_end {
            return None;
        }
        let overlap_end = current_end.min(candidate_end);
        for sequence in self.sequence.max(candidate.sequence)..overlap_end {
            let current = usize::try_from(sequence.checked_sub(self.sequence)?).ok()?;
            let incoming = usize::try_from(sequence.checked_sub(candidate.sequence)?).ok()?;
            if !self
                .segments
                .get(current)?
                .same_media(candidate.segments.get(incoming)?)
            {
                return None;
            }
        }
        let appended = usize::try_from(candidate_end.saturating_sub(current_end)).ok()?;
        let first = if appended == 0 {
            0
        } else {
            let first = usize::try_from(current_end.checked_sub(candidate.sequence)?).ok()?;
            let previous = self.segments.last()?.discontinuity_sequence;
            let next = candidate.segments.get(first)?.discontinuity_sequence;
            if next < previous || next > previous.checked_add(1)? {
                return None;
            }
            first
        };
        Some((appended, first))
    }

    pub(crate) fn joins(&self, candidate: &Self) -> bool {
        self.merge_extension(candidate).is_some()
    }

    pub(crate) fn mark_gap(&mut self, sequence: u64, reference: &str) -> bool {
        let Some(position) = sequence
            .checked_sub(self.sequence)
            .and_then(|position| usize::try_from(position).ok())
        else {
            return false;
        };
        let Some(segment) = self.segments.get_mut(position) else {
            return false;
        };
        if segment.reference != reference {
            return false;
        }
        segment.gap = true;
        true
    }

    pub(crate) fn reconstruct(
        mut snapshots: Vec<(u64, Self)>,
        head_index: u64,
        head: Self,
    ) -> Option<Self> {
        snapshots.retain(|(index, _)| *index < head_index);
        snapshots.sort_by_key(|(index, _)| *index);
        let expected_end = head.end_sequence()?;
        snapshots.push((head_index, head));
        let mut snapshots = snapshots.into_iter();
        let (_, mut archive) = snapshots.next()?;
        if archive.sequence != 0 {
            return None;
        }
        for (_, snapshot) in snapshots {
            archive.merge_playlist(snapshot)?;
        }
        let archive_end = archive.end_sequence()?;
        (archive_end == expected_end).then_some(archive)
    }

    fn merge_segments(
        &mut self,
        mut candidates: Vec<HlsSegment>,
        finalized: bool,
    ) -> Option<usize> {
        let current_tail = self.segments.last()?;
        let overlap = candidates
            .iter()
            .rposition(|candidate| candidate.same_payload(current_tail))?;
        let offset = current_tail
            .discontinuity_sequence
            .checked_sub(candidates[overlap].discontinuity_sequence)?;
        for candidate in &mut candidates[overlap..] {
            candidate.discontinuity_sequence =
                candidate.discontinuity_sequence.checked_add(offset)?;
        }
        if !candidates[overlap].same_media(current_tail) {
            return None;
        }
        let appended = candidates.len().saturating_sub(overlap + 1);
        for candidate in candidates.into_iter().skip(overlap + 1) {
            self.target_duration = self
                .target_duration
                .max(candidate.duration.round() as u64)
                .max(1);
            self.segments.push(candidate);
        }
        self.finalized = finalized;
        Some(appended)
    }

    pub(crate) fn render(&self, local_bytes_base: &str, start: HlsStart) -> Vec<u8> {
        self.render_with_plan(local_bytes_base, start, self.startup_plan(start).as_ref())
    }

    pub(crate) fn render_with_plan(
        &self,
        local_bytes_base: &str,
        start: HlsStart,
        plan: Option<&HlsStartupPlan>,
    ) -> Vec<u8> {
        let mut output = String::with_capacity(self.segments.len().saturating_mul(112) + 160);
        output.push_str(HLS_HEADER);
        output.push_str(if self.segments.iter().any(|segment| segment.gap) {
            "\n#EXT-X-VERSION:8"
        } else {
            "\n#EXT-X-VERSION:3"
        });
        let _ = write!(
            output,
            "\n#EXT-X-TARGETDURATION:{}\n#EXT-X-PLAYLIST-TYPE:{}\n#EXT-X-MEDIA-SEQUENCE:{}",
            self.target_duration.max(1),
            if self.finalized { "VOD" } else { "EVENT" },
            self.sequence
        );
        if self.discontinuity_sequence != 0 {
            let _ = write!(
                output,
                "\n#EXT-X-DISCONTINUITY-SEQUENCE:{}",
                self.discontinuity_sequence
            );
        }
        match start {
            HlsStart::Beginning => output.push_str("\n#EXT-X-START:TIME-OFFSET=0,PRECISE=YES"),
            HlsStart::Live => {
                let position = plan.map_or(0.0, |plan| plan.play_position);
                let _ = write!(
                    output,
                    "\n#EXT-X-START:TIME-OFFSET={position:.6},PRECISE=NO"
                );
            }
        }
        let mut discontinuity_sequence = self.discontinuity_sequence;
        let mut beginning_startup = start == HlsStart::Beginning;
        let local_bytes_base = local_bytes_base.trim_end_matches('/');
        for segment in &self.segments {
            let discontinuity = segment.discontinuity_sequence > discontinuity_sequence;
            if discontinuity {
                output.push_str("\n#EXT-X-DISCONTINUITY");
            }
            discontinuity_sequence = segment.discontinuity_sequence;
            let _ = write!(output, "\n#EXTINF:{:.6},", segment.duration);
            if segment.gap {
                output.push_str("\n#EXT-X-GAP");
            }
            output.push('\n');
            output.push_str(local_bytes_base);
            output.push('/');
            output.push_str(&segment.reference);
            let startup = beginning_startup && !segment.gap;
            beginning_startup &= !startup;
            output.push_str(match (start, startup) {
                (HlsStart::Live, _) => "?start=live",
                (HlsStart::Beginning, true) => "?start=beginning&startup=1",
                (HlsStart::Beginning, false) => "?start=beginning",
            });
        }
        if self.finalized {
            output.push_str("\n#EXT-X-ENDLIST");
        }
        output.push('\n');
        output.into_bytes()
    }
}

pub(crate) fn is_hls_manifest(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .map(|text| text.strip_prefix('\u{feff}').unwrap_or(text).trim_start())
        .and_then(|text| text.lines().next())
        .is_some_and(|line| line.trim() == HLS_HEADER)
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

impl HlsSegment {
    fn same_payload(&self, candidate: &Self) -> bool {
        self.reference == candidate.reference
            && self.duration.to_bits() == candidate.duration.to_bits()
            && self.gap == candidate.gap
    }

    fn same_media(&self, candidate: &Self) -> bool {
        self.same_payload(candidate)
            && self.discontinuity_sequence == candidate.discontinuity_sequence
    }
}

fn parse_segment_lines(text: &str, mut discontinuity_sequence: u64) -> Option<Vec<HlsSegment>> {
    let mut segments = Vec::new();
    let mut duration = None;
    let mut gap = false;
    let mut discontinuity = false;
    for original in text.lines() {
        let line = original.trim();
        if let Some(value) = line.strip_prefix("#EXTINF:") {
            if duration.is_some() {
                return None;
            }
            let value = value.split(',').next()?.trim().parse::<f64>().ok()?;
            if !value.is_finite() || value <= 0.0 {
                return None;
            }
            duration = Some(value);
        } else if line == HLS_GAP {
            duration?;
            gap = true;
        } else if line == "#EXT-X-DISCONTINUITY" {
            if discontinuity {
                return None;
            }
            discontinuity = true;
            discontinuity_sequence = discontinuity_sequence.checked_add(1)?;
        } else if line.is_empty() || line.starts_with('#') {
        } else if let Some(segment_duration) = duration.take() {
            segments.push(HlsSegment {
                reference: swarm_reference(line)?.to_ascii_lowercase(),
                duration: segment_duration,
                gap,
                discontinuity_sequence,
            });
            gap = false;
            discontinuity = false;
        }
    }
    if duration.is_some() || gap {
        return None;
    }
    Some(segments)
}

fn hls_has_terminal_endlist(text: &str) -> bool {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        == Some(HLS_ENDLIST)
}

fn swarm_reference(uri: &str) -> Option<&str> {
    if uri != uri.trim() {
        return None;
    }
    let candidate = uri.split(['?', '#']).next()?.trim_end_matches('/');
    if is_hex_reference(candidate) {
        return Some(candidate);
    }
    let path = if let Some((scheme, remainder)) = candidate.split_once("://") {
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return None;
        }
        let slash = remainder.find('/')?;
        &remainder[slash..]
    } else {
        candidate
    };
    let reference = path.rsplit('/').next()?;
    is_hex_reference(reference).then_some(reference)
}

fn is_hex_reference(value: &str) -> bool {
    matches!(value.len(), 64 | 128) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(target_arch = "wasm32")]
#[path = "stream_hls/player.rs"]
mod player;

#[cfg(target_arch = "wasm32")]
#[path = "stream_hls/page_bridge.rs"]
mod page_bridge;

#[cfg(target_arch = "wasm32")]
#[path = "stream_hls/protocol.rs"]
mod protocol;

#[cfg(target_arch = "wasm32")]
#[path = "stream_hls/runtime.rs"]
mod runtime;

#[cfg(target_arch = "wasm32")]
#[path = "stream_hls/worker_bridge.rs"]
pub(crate) mod worker_bridge;

#[cfg(target_arch = "wasm32")]
pub(crate) use page_bridge::{
    attach_hls_feed_player, open_hls_feed_view, release_hls_for_bzz_view, release_hls_view,
};

#[cfg(target_arch = "wasm32")]
pub(crate) use runtime::{
    clear_hls_runtime_cache, install_live_tail_fallback, live_tail_failure_identity,
    lock_live_startup_plan, prepare_hls_feed, release_hls_runtime, start_beginning_history,
    try_fetch_response,
};
