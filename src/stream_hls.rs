//! Minimal reader for the append-only HLS feed endpoint.
//!
//! The producer used by weeb-3 publishes one muxed media playlist whose feed
//! payload grows by appending segments.  Keep the implementation deliberately
//! tied to that contract: one playlist, whole segment bodies, a three-segment
//! live runway, and monotonically increasing feed updates.

use crate::stream_conventions::HlsStart;

pub(crate) const HLS_LIVE_SYNC_SEGMENTS: usize = 3;
pub(crate) const MAX_STREAM_FEED_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

const HLS_HEADER: &str = "#EXTM3U";
const HLS_ENDLIST: &str = "#EXT-X-ENDLIST";
const HLS_GAP: &str = "#EXT-X-GAP";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HlsSegment {
    pub(crate) reference: String,
    pub(crate) duration: f64,
    pub(crate) gap: bool,
    pub(crate) discontinuity: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HlsPlaylist {
    pub(crate) sequence: u64,
    pub(crate) target_duration: u64,
    pub(crate) segments: Vec<HlsSegment>,
    pub(crate) finalized: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HlsStartupPlan {
    pub(crate) play_position: f64,
    pub(crate) runway_end: f64,
    pub(crate) duration: f64,
    pub(crate) references: [String; HLS_LIVE_SYNC_SEGMENTS],
}

impl HlsPlaylist {
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_STREAM_FEED_PAYLOAD_BYTES || !is_hls_manifest(bytes) {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        let mut sequence = None;
        let mut target_duration = None;
        for line in text.lines().map(str::trim) {
            if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
                if sequence.replace(value.trim().parse().ok()?).is_some() {
                    return None;
                }
            } else if let Some(value) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
                if target_duration
                    .replace(value.trim().parse::<u64>().ok()?.max(1))
                    .is_some()
                {
                    return None;
                }
            }
        }
        let segments = parse_segment_lines(text)?;
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

    pub(crate) fn startup_plan(&self, start: HlsStart) -> Option<HlsStartupPlan> {
        let playable = self
            .segments
            .iter()
            .enumerate()
            .filter_map(|(index, segment)| (!segment.gap).then_some(index))
            .collect::<Vec<_>>();
        if playable.len() < HLS_LIVE_SYNC_SEGMENTS {
            return None;
        }
        let runway = match start {
            HlsStart::Beginning => &playable[..HLS_LIVE_SYNC_SEGMENTS],
            HlsStart::Live => &playable[playable.len() - HLS_LIVE_SYNC_SEGMENTS..],
        };
        let first = runway[0];
        let last = *runway.last()?;
        let offset = |index: usize| {
            self.segments[..index]
                .iter()
                .map(|segment| segment.duration)
                .sum::<f64>()
        };
        let play_position = offset(first);
        let runway_end = offset(last) + self.segments[last].duration;
        let duration = self.duration();
        if !play_position.is_finite()
            || !runway_end.is_finite()
            || !duration.is_finite()
            || runway_end <= play_position
        {
            return None;
        }
        Some(HlsStartupPlan {
            play_position,
            runway_end,
            duration,
            references: std::array::from_fn(|slot| self.segments[runway[slot]].reference.clone()),
        })
    }

    /// Merge an authenticated tail range from a newer append-only feed body.
    /// At least one segment must overlap, preventing an unrelated body from
    /// being attached to the active timeline.
    pub(crate) fn merge_tail(&mut self, bytes: &[u8]) -> Option<usize> {
        let text = std::str::from_utf8(bytes).ok()?;
        let candidates = parse_segment_lines(text)
            .filter(|segments| !segments.is_empty())
            .or_else(|| {
                text.split_once('\n')
                    .and_then(|(_, complete)| parse_segment_lines(complete))
            })?;
        self.merge_segments(candidates, hls_has_terminal_endlist(text))
    }

    pub(crate) fn merge_playlist(&mut self, candidate: Self) -> Option<usize> {
        let target_duration = candidate.target_duration;
        let appended = self.merge_segments(candidate.segments, candidate.finalized)?;
        self.target_duration = self.target_duration.max(target_duration);
        Some(appended)
    }

    fn merge_segments(&mut self, candidates: Vec<HlsSegment>, finalized: bool) -> Option<usize> {
        let current_tail = &self.segments.last()?.reference;
        let overlap = candidates
            .iter()
            .rposition(|candidate| &candidate.reference == current_tail)?;
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
        let mut output = String::with_capacity(self.segments.len().saturating_mul(112) + 160);
        output.push_str(HLS_HEADER);
        output.push_str("\n#EXT-X-VERSION:3");
        output.push_str(&format!(
            "\n#EXT-X-TARGETDURATION:{}\n#EXT-X-PLAYLIST-TYPE:{}\n#EXT-X-MEDIA-SEQUENCE:{}",
            self.target_duration.max(1),
            if self.finalized { "VOD" } else { "EVENT" },
            self.sequence
        ));
        match start {
            HlsStart::Beginning => output.push_str("\n#EXT-X-START:TIME-OFFSET=0,PRECISE=YES"),
            HlsStart::Live => {
                let tail = self
                    .segments
                    .iter()
                    .rev()
                    .filter(|segment| !segment.gap)
                    .take(HLS_LIVE_SYNC_SEGMENTS)
                    .map(|segment| segment.duration)
                    .sum::<f64>();
                output.push_str(&format!("\n#EXT-X-START:TIME-OFFSET=-{tail:.6},PRECISE=NO"));
            }
        }
        for segment in &self.segments {
            if segment.discontinuity {
                output.push_str("\n#EXT-X-DISCONTINUITY");
            }
            output.push_str(&format!("\n#EXTINF:{:.6},", segment.duration));
            if segment.gap {
                output.push_str("\n#EXT-X-GAP");
            }
            output.push('\n');
            output.push_str(local_bytes_base.trim_end_matches('/'));
            output.push('/');
            output.push_str(&segment.reference);
            output.push_str(if start == HlsStart::Live {
                "?start=live"
            } else {
                "?start=beginning"
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
        .is_some_and(|line| line.trim_end_matches('\r').trim() == HLS_HEADER)
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

fn parse_segment_lines(text: &str) -> Option<Vec<HlsSegment>> {
    let mut segments = Vec::new();
    let mut duration = None;
    let mut gap = false;
    let mut discontinuity = false;
    for original in text.lines() {
        let line = original.trim_end_matches('\r').trim();
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
            if duration.is_none() {
                return None;
            }
            gap = true;
        } else if line == "#EXT-X-DISCONTINUITY" {
            discontinuity = true;
        } else if line.is_empty() || line.starts_with('#') {
        } else if let Some(segment_duration) = duration.take() {
            segments.push(HlsSegment {
                reference: swarm_reference(line)?.to_ascii_lowercase(),
                duration: segment_duration,
                gap,
                discontinuity,
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
        .map(|line| line.trim_end_matches('\r').trim())
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
        if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
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
#[path = "stream_hls/runtime.rs"]
mod runtime;

#[cfg(target_arch = "wasm32")]
pub(crate) use runtime::{
    attach_hls_feed_player, open_hls_feed_view, release_hls_for_bzz_view, release_hls_view,
    try_fetch_response,
};
