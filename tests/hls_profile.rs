#![recursion_limit = "256"]

//! Optional cold-browser smoke profile for the HLS viewer.
//!
//! Set `WEEB3_HLS_PROFILE_URL` to a beginning or live viewer URL to enable it.
//! The test deliberately observes browser-visible behavior only: time to first
//! playback, media-clock progress, buffering, and stalls. It has no dependency
//! on internal retrieval traces, byte ranges, or experimental startup globals.

use anyhow_crates_io::{Context, Result, anyhow};
use headless_chrome::protocol::cdp::Network::{
    ClearBrowserCache, Enable as EnableNetwork, SetCacheDisabled,
};
use headless_chrome::protocol::cdp::types::Event;
use headless_chrome::protocol::cdp::{Page::AddScriptToEvaluateOnNewDocument, Target::GetTargets};
use headless_chrome::{Browser, LaunchOptionsBuilder};
use serde::Serialize;
use serde_json::{Value, json};
use sha3_crates_io::{Digest, Sha3_256};
use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const DEFAULT_PROFILE_SECONDS: u64 = 75;
const DEFAULT_MAX_STARTUP_MS: u64 = 5_000;
// A media clock may appear stationary between two 250 ms samples because of
// timestamp quantization. Anything beyond one sampling interval is observable
// playback interruption, while waiting/stalled/paused events always fail.
const DEFAULT_MAX_STALL_MS: u64 = 250;
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

const PROFILE_SCRIPT: &str = r#"
(() => {
    const sampleCap = 4096;
    const eventCap = 512;
    const profile = window.__weeb3HlsSmokeProfile = {
        schema_version: 1,
        time_origin_ms: performance.timeOrigin,
        installed_at_ms: performance.now(),
        phase_started_ms: 0,
        finished_at_ms: null,
        marks: {
            first_video_ms: null,
            first_play_ms: null,
            first_playing_ms: null,
            first_playing_current_time_s: null,
            first_presented_frame_ms: null,
            first_confirmed_playback_ms: null,
            first_confirmed_playback_current_time_s: null
        },
        samples: [],
        events: [],
        page_errors: [],
        interface_logs: [],
        connection_logs: [],
        shared_worker_attached: false,
        hls_open_log: null,
        refreshment_logs: [],
        lifecycle_logs: []
    };

    const finite = value => Number.isFinite(value) ? value : null;
    const ranges = value => {
        const result = [];
        try {
            for (let index = 0; index < value.length; index++) {
                result.push([value.start(index), value.end(index)]);
            }
        } catch (error) {}
        return result;
    };
    const forwardBuffer = (currentTime, buffered) => {
        for (const [start, end] of buffered) {
            if (start <= currentTime + 0.05 && end >= currentTime) {
                return Math.max(0, end - currentTime);
            }
        }
        return 0;
    };

    const attached = new WeakSet();
    const videoIds = new WeakMap();
    let nextVideoId = 1;
    let activeVideo = null;
    let playingCandidate = null;

    const sample = () => {
        const video = activeVideo || document.querySelector('video');
        if (!video) return;
        const buffered = ranges(video.buffered);
        const seekable = ranges(video.seekable);
        if (
            profile.marks.first_confirmed_playback_ms === null &&
            profile.marks.first_presented_frame_ms !== null &&
            playingCandidate?.video === video &&
            !video.paused &&
            video.currentTime >= playingCandidate.currentTime + 0.1
        ) {
            profile.marks.first_confirmed_playback_ms = performance.now();
            profile.marks.first_confirmed_playback_current_time_s = finite(video.currentTime);
        }
        profile.samples.push({
            at_ms: performance.now(),
            video_id: videoIds.get(video) || null,
            current_time_s: finite(video.currentTime),
            duration_s: finite(video.duration),
            paused: video.paused,
            ended: video.ended,
            ready_state: video.readyState,
            network_state: video.networkState,
            playback_rate: video.playbackRate,
            buffered,
            seekable,
            forward_buffer_s: forwardBuffer(video.currentTime, buffered)
        });
        if (profile.samples.length > sampleCap) profile.samples.shift();
    };
    window.__weeb3TakeHlsSmokeSample = sample;

    window.__weeb3BeginHlsSeekProfile = position => {
        const video = activeVideo || document.querySelector('video');
        if (!video || !Number.isFinite(position) || position < 0) return null;
        profile.phase_started_ms = performance.now();
        profile.samples.length = 0;
        profile.events.length = 0;
        for (const mark of Object.keys(profile.marks)) profile.marks[mark] = null;
        video.currentTime = position;
        playingCandidate = { video, currentTime: position };
        if (typeof video.requestVideoFrameCallback === 'function') {
            video.requestVideoFrameCallback(() => {
                if (profile.marks.first_presented_frame_ms === null) {
                    profile.marks.first_presented_frame_ms = performance.now();
                }
            });
        }
        if (video.paused) video.play().catch(() => {});
        sample();
        return profile.phase_started_ms;
    };

    const recordEvent = (video, name) => {
        if (name === 'playing') activeVideo = video;
        const at = performance.now();
        if (name === 'play' && profile.marks.first_play_ms === null) {
            profile.marks.first_play_ms = at;
        }
        if (name === 'playing' && profile.marks.first_playing_ms === null) {
            profile.marks.first_playing_ms = at;
            profile.marks.first_playing_current_time_s = finite(video.currentTime);
        }
        if (name === 'playing') {
            playingCandidate = { video, currentTime: video.currentTime };
            if (
                profile.marks.first_presented_frame_ms === null &&
                typeof video.requestVideoFrameCallback === 'function'
            ) {
                video.requestVideoFrameCallback(() => {
                    if (profile.marks.first_presented_frame_ms === null) {
                        profile.marks.first_presented_frame_ms = performance.now();
                    }
                });
            }
        }
        profile.events.push({
            at_ms: at,
            name,
            video_id: videoIds.get(video) || null,
            current_time_s: finite(video.currentTime),
            duration_s: finite(video.duration),
            paused: video.paused,
            ready_state: video.readyState,
            network_state: video.networkState,
            error: video.error ? {
                code: video.error.code,
                message: video.error.message || null
            } : null
        });
        if (profile.events.length > eventCap) profile.events.shift();
        sample();
    };

    const attach = video => {
        if (attached.has(video)) return;
        attached.add(video);
        videoIds.set(video, nextVideoId++);
        activeVideo ||= video;
        if (profile.marks.first_video_ms === null) {
            profile.marks.first_video_ms = performance.now();
        }
        for (const name of [
            'loadstart', 'loadedmetadata', 'durationchange', 'canplay', 'play',
            'playing', 'progress', 'waiting', 'stalled', 'seeking', 'seeked',
            'pause', 'ended', 'error', 'abort', 'emptied'
        ]) {
            video.addEventListener(name, () => recordEvent(video, name));
        }
        sample();
    };
    const scan = () => document.querySelectorAll('video').forEach(attach);
    new MutationObserver(scan).observe(document, { childList: true, subtree: true });
    document.addEventListener('DOMContentLoaded', scan, { once: true });
    scan();

    const logged = new WeakSet();
    const captureLogs = () => {
        document.querySelectorAll('#logsField > *').forEach(node => {
            if (logged.has(node)) return;
            const text = (node.textContent || '').trim();
            if (!text) return;
            logged.add(node);
            profile.interface_logs.push(text);
            if (profile.interface_logs.length > 4096) profile.interface_logs.shift();
            if (/Connected to (?:peer|bootnode) /.test(text)) {
                profile.connection_logs.push(text);
                if (profile.connection_logs.length > 512) profile.connection_logs.shift();
            }
            if (text.includes('Attached to the SharedWorker node')) {
                profile.shared_worker_attached = true;
            }
            if (profile.hls_open_log === null && /HLS open index=.*elapsed=/.test(text)) {
                profile.hls_open_log = text;
            }
            if (/refresh|feed update/i.test(text)) {
                profile.refreshment_logs.push(text);
                if (profile.refreshment_logs.length > 2048) profile.refreshment_logs.shift();
            }
            if (
                /(?:Disconnected from|Connection closed|Queued reconnect|Closed unowned|ambiguous|not dispatched)/i.test(text)
            ) {
                profile.lifecycle_logs.push(text);
                if (profile.lifecycle_logs.length > 1024) profile.lifecycle_logs.shift();
            }
        });
    };
    window.__weeb3CaptureHlsLogs = captureLogs;
    new MutationObserver(captureLogs).observe(document, {
        childList: true,
        characterData: true,
        subtree: true
    });
    captureLogs();

    addEventListener('error', event => {
        profile.page_errors.push({
            at_ms: performance.now(),
            type: 'error',
            message: String(event.message || event.error || 'unknown page error')
        });
    });
    addEventListener('unhandledrejection', event => {
        profile.page_errors.push({
            at_ms: performance.now(),
            type: 'unhandledrejection',
            message: String(event.reason?.message || event.reason || 'unknown rejection')
        });
    });

    window.__weeb3HlsSmokeTimer = setInterval(sample, 250);
})()
"#;

const RESULT_SCRIPT: &str = r#"
(async () => {
    const profile = window.__weeb3HlsSmokeProfile;
    if (!profile) return null;
    clearInterval(window.__weeb3HlsSmokeTimer);
    window.__weeb3TakeHlsSmokeSample?.();
    window.__weeb3CaptureHlsLogs?.();
    profile.finished_at_ms = performance.now();

    let registrations = [];
    try {
        registrations = await navigator.serviceWorker?.getRegistrations?.() || [];
    } catch (error) {}
    let cacheNames = [];
    try { cacheNames = await caches.keys(); } catch (error) {}

    return JSON.stringify({
        ...profile,
        hls_resources: performance.getEntriesByType('resource')
            .filter(entry => /\/(?:feeds|hls\/bytes)\//.test(new URL(entry.name).pathname))
            .map(entry => ({
                name: entry.name,
                initiator_type: entry.initiatorType,
                start_ms: entry.startTime,
                response_start_ms: entry.responseStart,
                response_end_ms: entry.responseEnd,
                duration_ms: entry.duration,
                transfer_bytes: entry.transferSize,
                encoded_bytes: entry.encodedBodySize,
                decoded_bytes: entry.decodedBodySize
            })),
        document: {
            url: location.href,
            title: document.title,
            visibility_state: document.visibilityState,
            hls_state: document.querySelector('video')?.getAttribute('data-weeb3-hls-state') || null,
            hls_error: document.querySelector('video')?.getAttribute('data-weeb3-hls-error') || null,
            hls_status: document.querySelector('.weeb3-hls-status')?.textContent || null
        },
        service_worker: {
            controlled: Boolean(navigator.serviceWorker?.controller),
            controller_script: navigator.serviceWorker?.controller?.scriptURL || null,
            registrations: registrations.map(item => ({
                scope: item.scope,
                active_script: item.active?.scriptURL || null
            })),
            cache_names: cacheNames
        }
    });
})()
"#;

const PLAYLIST_SCRIPT: &str = r#"
(async () => {
    const entries = performance.getEntriesByType('resource')
        .map(entry => entry.name)
        .filter(name => {
            try { return new URL(name).pathname.includes('/feeds/'); }
            catch (_) { return false; }
        });
    const url = entries.at(-1) || null;
    if (!url) return JSON.stringify(null);
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 3000);
    try {
        const response = await fetch(url, { cache: 'no-store', signal: controller.signal });
        const text = await response.text();
        const durations = text.split(/\r?\n/)
            .filter(line => line.startsWith('#EXTINF:'))
            .map(line => Number(line.substring(8).split(',', 1)[0]))
            .filter(Number.isFinite);
        const references = text.split(/\r?\n/)
            .filter(line => line && !line.startsWith('#'))
            .map(line => line.split(/[?#]/, 1)[0].split('/').at(-1));
        const targetDuration = Number(text.split(/\r?\n/)
            .find(line => line.startsWith('#EXT-X-TARGETDURATION:'))
            ?.substring(22));
        return JSON.stringify({
            url,
            observed_at_ms: performance.now(),
            status: response.status,
            ok: response.ok,
            segment_count: durations.length,
            duration_s: durations.reduce((sum, value) => sum + value, 0),
            tail_one_s: durations.at(-1) || 0,
            tail_two_s: durations.slice(-2).reduce((sum, value) => sum + value, 0),
            tail_three_s: durations.slice(-3).reduce((sum, value) => sum + value, 0),
            tail_durations_s: durations.slice(-5),
            tail_references: references.slice(-5),
            target_duration_s: Number.isFinite(targetDuration) ? targetDuration : null,
            endlist: text.trimEnd().endsWith('#EXT-X-ENDLIST')
        });
    } catch (error) {
        return JSON.stringify({ url, error: String(error?.message || error) });
    } finally {
        clearTimeout(timer);
    }
})()
"#;

const SERVED_ASSET_PROVENANCE_SCRIPT: &str = r#"
(async () => {
    const base = new URL('/weeb-3/', location.origin);
    const hex = bytes => Array.from(new Uint8Array(bytes), byte =>
        byte.toString(16).padStart(2, '0')).join('');
    const assets = [];
    for (const name of [
        'weeb_3.js', 'weeb_3_bg.wasm', 'service.js', 'worker.js'
    ]) {
        const url = new URL(name, base).href;
        try {
            const response = await fetch(url, { cache: 'no-store' });
            const body = await response.arrayBuffer();
            assets.push({
                name,
                url,
                status: response.status,
                ok: response.ok,
                bytes: body.byteLength,
                sha256: hex(await crypto.subtle.digest('SHA-256', body)),
                etag: response.headers.get('etag'),
                last_modified: response.headers.get('last-modified')
            });
        } catch (error) {
            assets.push({ name, url, error: String(error?.message || error) });
        }
    }
    return JSON.stringify({
        user_agent: navigator.userAgent,
        hardware_concurrency: navigator.hardwareConcurrency || null,
        assets
    });
})()
"#;

#[derive(Clone, Debug, Serialize)]
struct WebSocketBurstSummary {
    total_created: usize,
    first_created_ms: Option<f64>,
    created_within_150_ms_of_first: usize,
    created_within_3s: usize,
    created_within_5s: usize,
    attempt_160_ms: Option<f64>,
    first_to_attempt_160_ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
struct UsableConnectionSummary {
    source: &'static str,
    total_unique_ready: usize,
    first_ready_ms: Option<f64>,
    ready_within_3s: usize,
    ready_within_5s: usize,
    ready_40_ms: Option<f64>,
    ready_80_ms: Option<f64>,
    ready_160_ms: Option<f64>,
    ready_200_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ResourcePhase {
    Startup,
    SteadyPlayback,
}

#[derive(Clone, Debug, Serialize)]
struct BrowserResourceSample {
    at_ms: f64,
    interval_ms: f64,
    phase: ResourcePhase,
    process_count: usize,
    resident_bytes: u64,
    private_equivalent_bytes: u64,
    accumulated_cpu_ms: u64,
    cpu_delta_ms: u64,
    processes: Vec<BrowserProcessResourceSample>,
}

#[derive(Clone, Debug, Serialize)]
struct BrowserProcessResourceSample {
    pid: u32,
    parent_pid: Option<u32>,
    process_type: String,
    utility_sub_type: Option<String>,
    resident_bytes: u64,
    private_equivalent_bytes: u64,
    accumulated_cpu_ms: u64,
    cpu_delta_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
struct BrowserResourcePhaseSummary {
    sample_count: usize,
    observed_ms: f64,
    average_resident_bytes: Option<f64>,
    peak_resident_bytes: Option<u64>,
    average_private_equivalent_bytes: Option<f64>,
    peak_private_equivalent_bytes: Option<u64>,
    accumulated_cpu_ms: u64,
    average_cpu_cores: Option<f64>,
    average_cpu_percent_of_machine: Option<f64>,
    maximum_process_count: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
struct BrowserResourceSummary {
    root_pid: u32,
    sample_interval_ms: u64,
    logical_cpu_count: usize,
    memory_scope: &'static str,
    private_equivalent_metric: &'static str,
    startup: BrowserResourcePhaseSummary,
    steady_playback: BrowserResourcePhaseSummary,
    samples: Vec<BrowserResourceSample>,
}

#[derive(Clone, Debug, Serialize)]
struct TransferWindowSummary {
    completed_responses: usize,
    encoded_bytes: u64,
    decoded_bytes: u64,
    observed_ms: f64,
    encoded_mib_per_second: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
struct HlsEfficiencySummary {
    all_hls_responses: TransferWindowSummary,
    steady_playback: TransferWindowSummary,
    cpu_seconds: f64,
    cpu_seconds_per_encoded_mib: Option<f64>,
    buffer_growth_s: Option<f64>,
    media_plus_buffer_growth_s: Option<f64>,
    cpu_seconds_per_media_plus_buffer_second: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
struct GitProvenance {
    worktree: Option<String>,
    head: Option<String>,
    dirty: Option<bool>,
    changed_entries: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct FileProvenance {
    path: String,
    bytes: u64,
    modified_unix_ms: Option<u128>,
    sha3_256: String,
}

#[derive(Clone, Debug, Serialize)]
struct RunProvenance {
    variant: Option<String>,
    harness_git: GitProvenance,
    harness_executable: Option<FileProvenance>,
    server_git: Option<GitProvenance>,
    server_executable: Option<FileProvenance>,
    served_assets: Value,
}

enum ResourceSamplerCommand {
    MarkSteadyPlayback,
    Stop,
}

struct BrowserResourceSampler {
    root_pid: u32,
    commands: Option<mpsc::Sender<ResourceSamplerCommand>>,
    thread: Option<thread::JoinHandle<Vec<BrowserResourceSample>>>,
}

impl BrowserResourceSampler {
    fn start(root_pid: u32) -> Self {
        let (commands, receiver) = mpsc::channel();
        let thread = thread::spawn(move || sample_browser_process_tree(root_pid, receiver));
        Self {
            root_pid,
            commands: Some(commands),
            thread: Some(thread),
        }
    }

    fn mark_steady_playback(&self) -> Result<()> {
        self.commands
            .as_ref()
            .ok_or_else(|| anyhow!("browser resource sampler has already stopped"))?
            .send(ResourceSamplerCommand::MarkSteadyPlayback)
            .map_err(|_| anyhow!("browser resource sampler stopped before playback"))
    }

    fn finish(mut self) -> Result<BrowserResourceSummary> {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(ResourceSamplerCommand::Stop);
        }
        let samples = self
            .thread
            .take()
            .ok_or_else(|| anyhow!("browser resource sampler thread was missing"))?
            .join()
            .map_err(|_| anyhow!("browser resource sampler thread panicked"))?;
        Ok(summarize_browser_resources(self.root_pid, samples))
    }
}

impl Drop for BrowserResourceSampler {
    fn drop(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(ResourceSamplerCommand::Stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn sample_browser_process_tree(
    root_pid: u32,
    receiver: mpsc::Receiver<ResourceSamplerCommand>,
) -> Vec<BrowserResourceSample> {
    let root = Pid::from_u32(root_pid);
    let refresh_kind = ProcessRefreshKind::nothing()
        .with_memory()
        .with_cpu()
        .with_cmd(UpdateKind::OnlyIfNotSet)
        .without_tasks();
    let started = Instant::now();
    let mut system = System::new();
    let mut samples = Vec::new();
    let mut phase = ResourcePhase::Startup;
    let mut previous_at_ms = 0.0;
    let mut previous_cpu = HashMap::<(u32, u64), u64>::new();
    let mut take_sample = |phase| {
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);
        let pids = browser_process_tree(&system, root);
        let at_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let interval_ms = (at_ms - previous_at_ms).max(0.0);
        previous_at_ms = at_ms;
        let mut resident_bytes = 0u64;
        let mut private_equivalent_bytes = 0u64;
        let mut accumulated_cpu_ms = 0u64;
        let mut cpu_delta_ms = 0u64;
        let mut processes = Vec::with_capacity(pids.len());

        for pid in &pids {
            let Some(process) = system.process(*pid) else {
                continue;
            };
            resident_bytes = resident_bytes.saturating_add(process.memory());
            private_equivalent_bytes =
                private_equivalent_bytes.saturating_add(process.virtual_memory());
            let accumulated = process.accumulated_cpu_time();
            accumulated_cpu_ms = accumulated_cpu_ms.saturating_add(accumulated);
            let identity = (pid.as_u32(), process.start_time());
            // A process's accumulated value covers its whole lifetime. Establish a
            // baseline on first observation instead of charging that lifetime to one
            // 500 ms sample; subsequent samples account for every measured delta.
            let process_cpu_delta_ms = previous_cpu
                .insert(identity, accumulated)
                .map_or(0, |previous| accumulated.saturating_sub(previous));
            cpu_delta_ms = cpu_delta_ms.saturating_add(process_cpu_delta_ms);
            let (process_type, utility_sub_type) = edge_process_type(process, *pid == root);
            processes.push(BrowserProcessResourceSample {
                pid: pid.as_u32(),
                parent_pid: process.parent().map(|parent| parent.as_u32()),
                process_type,
                utility_sub_type,
                resident_bytes: process.memory(),
                private_equivalent_bytes: process.virtual_memory(),
                accumulated_cpu_ms: accumulated,
                cpu_delta_ms: process_cpu_delta_ms,
            });
        }
        processes.sort_unstable_by_key(|process| process.pid);

        samples.push(BrowserResourceSample {
            at_ms,
            interval_ms,
            phase,
            process_count: pids.len(),
            resident_bytes,
            private_equivalent_bytes,
            accumulated_cpu_ms,
            cpu_delta_ms,
            processes,
        });
    };

    take_sample(phase);
    loop {
        match receiver.recv_timeout(RESOURCE_SAMPLE_INTERVAL) {
            Ok(ResourceSamplerCommand::MarkSteadyPlayback) => {
                // Close the startup interval before changing phases so startup
                // CPU is not charged to steady playback.
                take_sample(phase);
                phase = ResourcePhase::SteadyPlayback;
            }
            Ok(ResourceSamplerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                take_sample(phase);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => take_sample(phase),
        }
    }
    samples
}

fn edge_process_type(process: &sysinfo::Process, root: bool) -> (String, Option<String>) {
    let flag = |prefix: &str| {
        process.cmd().iter().find_map(|argument| {
            argument
                .to_string_lossy()
                .strip_prefix(prefix)
                .map(str::to_owned)
        })
    };
    let process_type = flag("--type=").unwrap_or_else(|| {
        if root {
            "browser".to_string()
        } else {
            process.name().to_string_lossy().into_owned()
        }
    });
    (process_type, flag("--utility-sub-type="))
}

fn browser_process_tree(system: &System, root: Pid) -> HashSet<Pid> {
    let mut tree = HashSet::from([root]);
    loop {
        let mut changed = false;
        for (pid, process) in system.processes() {
            if process
                .parent()
                .is_some_and(|parent| tree.contains(&parent))
            {
                changed |= tree.insert(*pid);
            }
        }
        if !changed {
            return tree;
        }
    }
}

fn summarize_browser_resources(
    root_pid: u32,
    samples: Vec<BrowserResourceSample>,
) -> BrowserResourceSummary {
    let logical_cpu_count = thread::available_parallelism().map_or(1, usize::from);
    BrowserResourceSummary {
        root_pid,
        sample_interval_ms: RESOURCE_SAMPLE_INTERVAL.as_millis() as u64,
        logical_cpu_count,
        memory_scope: "sum of the Edge root process and all live descendants",
        private_equivalent_metric: if cfg!(target_os = "windows") {
            "Windows process PrivateUsage"
        } else {
            "process virtual memory"
        },
        startup: summarize_resource_phase(&samples, ResourcePhase::Startup, logical_cpu_count),
        steady_playback: summarize_resource_phase(
            &samples,
            ResourcePhase::SteadyPlayback,
            logical_cpu_count,
        ),
        samples,
    }
}

fn summarize_resource_phase(
    samples: &[BrowserResourceSample],
    phase: ResourcePhase,
    logical_cpu_count: usize,
) -> BrowserResourcePhaseSummary {
    let phase_samples: Vec<_> = samples
        .iter()
        .filter(|sample| sample.phase == phase)
        .collect();
    let observed_ms = phase_samples
        .iter()
        .map(|sample| sample.interval_ms)
        .sum::<f64>();
    let accumulated_cpu_ms = phase_samples
        .iter()
        .map(|sample| sample.cpu_delta_ms)
        .sum::<u64>();
    let weighted_average = |value: fn(&BrowserResourceSample) -> u64| {
        if phase_samples.is_empty() {
            return None;
        }
        if observed_ms > 0.0 {
            Some(
                phase_samples
                    .iter()
                    .map(|sample| value(sample) as f64 * sample.interval_ms)
                    .sum::<f64>()
                    / observed_ms,
            )
        } else {
            Some(
                phase_samples
                    .iter()
                    .map(|sample| value(sample) as f64)
                    .sum::<f64>()
                    / phase_samples.len() as f64,
            )
        }
    };
    let average_cpu_cores = (observed_ms > 0.0).then_some(accumulated_cpu_ms as f64 / observed_ms);

    BrowserResourcePhaseSummary {
        sample_count: phase_samples.len(),
        observed_ms,
        average_resident_bytes: weighted_average(|sample| sample.resident_bytes),
        peak_resident_bytes: phase_samples
            .iter()
            .map(|sample| sample.resident_bytes)
            .max(),
        average_private_equivalent_bytes: weighted_average(|sample| {
            sample.private_equivalent_bytes
        }),
        peak_private_equivalent_bytes: phase_samples
            .iter()
            .map(|sample| sample.private_equivalent_bytes)
            .max(),
        accumulated_cpu_ms,
        average_cpu_cores,
        average_cpu_percent_of_machine: average_cpu_cores
            .map(|cores| cores * 100.0 / logical_cpu_count as f64),
        maximum_process_count: phase_samples
            .iter()
            .map(|sample| sample.process_count)
            .max(),
    }
}

fn summarize_hls_efficiency(
    metrics: &Value,
    playback: &PlaybackSummary,
    steady_resources: &BrowserResourcePhaseSummary,
) -> HlsEfficiencySummary {
    let resources: Vec<&Value> = metrics
        .get("hls_resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let all_start_ms = resources
        .iter()
        .filter_map(|resource| number_at(resource, "/start_ms"))
        .reduce(f64::min);
    let all_end_ms = resources
        .iter()
        .filter_map(|resource| number_at(resource, "/response_end_ms"))
        .reduce(f64::max);
    let all_observed_ms = all_start_ms
        .zip(all_end_ms)
        .map_or(0.0, |(start, end)| (end - start).max(0.0));
    let first_playing_ms = number_at(metrics, "/marks/first_confirmed_playback_ms");
    let steady_end_ms = first_playing_ms.map(|start| start + steady_resources.observed_ms);
    let completed_during_steady: Vec<&Value> = resources
        .iter()
        .copied()
        .filter(|resource| {
            number_at(resource, "/response_end_ms").is_some_and(|completed| {
                first_playing_ms.is_some_and(|start| completed >= start)
                    && steady_end_ms.is_some_and(|end| completed <= end)
            })
        })
        .collect();
    let all_hls_responses = summarize_transfer_window(&resources, all_observed_ms);
    let steady_playback =
        summarize_transfer_window(&completed_during_steady, steady_resources.observed_ms);
    let cpu_seconds = steady_resources.accumulated_cpu_ms as f64 / 1_000.0;
    let encoded_mib = steady_playback.encoded_bytes as f64 / (1024.0 * 1024.0);
    let buffer_growth_s = playback
        .start_buffer_s
        .zip(playback.final_buffer_s)
        .map(|(start, end)| end - start);
    let media_plus_buffer_growth_s =
        buffer_growth_s.map(|growth| playback.media_advance_s + growth);

    HlsEfficiencySummary {
        all_hls_responses,
        steady_playback,
        cpu_seconds,
        cpu_seconds_per_encoded_mib: (encoded_mib > 0.0).then_some(cpu_seconds / encoded_mib),
        buffer_growth_s,
        media_plus_buffer_growth_s,
        cpu_seconds_per_media_plus_buffer_second: media_plus_buffer_growth_s
            .filter(|seconds| *seconds > 0.0)
            .map(|seconds| cpu_seconds / seconds),
    }
}

fn summarize_transfer_window(resources: &[&Value], observed_ms: f64) -> TransferWindowSummary {
    let encoded_bytes = resources
        .iter()
        .filter_map(|resource| resource.get("encoded_bytes").and_then(Value::as_u64))
        .sum();
    let decoded_bytes = resources
        .iter()
        .filter_map(|resource| resource.get("decoded_bytes").and_then(Value::as_u64))
        .sum();
    let observed_seconds = observed_ms / 1_000.0;
    let encoded_mib = encoded_bytes as f64 / (1024.0 * 1024.0);
    TransferWindowSummary {
        completed_responses: resources.len(),
        encoded_bytes,
        decoded_bytes,
        observed_ms,
        encoded_mib_per_second: (observed_seconds > 0.0).then_some(encoded_mib / observed_seconds),
    }
}

#[derive(Clone, Debug, Serialize)]
struct PlaybackSummary {
    first_play_event_ms: Option<f64>,
    first_presented_frame_ms: Option<f64>,
    first_playing_ms: Option<f64>,
    first_media_time_s: Option<f64>,
    final_media_time_s: Option<f64>,
    observed_after_playing_ms: f64,
    media_advance_s: f64,
    progress_ratio: f64,
    max_stagnant_ms: f64,
    timeline_regressions: usize,
    timeline_forward_jumps: usize,
    duration_regressions: usize,
    waiting_events: usize,
    stalled_events: usize,
    paused_samples: usize,
    ended_events: usize,
    media_error_events: usize,
    start_buffer_s: Option<f64>,
    final_buffer_s: Option<f64>,
    minimum_buffer_s: Option<f64>,
    maximum_buffer_s: Option<f64>,
    early_low_water_s: Option<f64>,
    minute_low_water_s: Option<f64>,
    late_low_water_s: Option<f64>,
    low_water_trend_s: Option<f64>,
    post_minute_low_water_trend_s: Option<f64>,
    start_duration_s: Option<f64>,
    final_duration_s: Option<f64>,
}

#[test]
fn weeb3_hls_profile() -> Result<()> {
    let Some(target_url) = env::var("WEEB3_HLS_PROFILE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        println!("WEEB3_HLS_PROFILE_URL is not set; skipping browser HLS profile");
        return Ok(());
    };
    let live_target = target_url.contains("/live/") || target_url.contains("start=live");
    let npm_target = target_url.contains("/hls-stream-example.html");

    let profile_seconds = env_u64("WEEB3_HLS_PROFILE_SECONDS", DEFAULT_PROFILE_SECONDS)?;
    if profile_seconds == 0 {
        return Err(anyhow!("WEEB3_HLS_PROFILE_SECONDS must be positive"));
    }
    let max_startup_ms = env_u64("WEEB3_HLS_MAX_STARTUP_MS", DEFAULT_MAX_STARTUP_MS)?;
    let max_stall_ms = env_u64("WEEB3_HLS_MAX_STALL_MS", DEFAULT_MAX_STALL_MS)?;
    let minimum_progress_ratio = env_f64("WEEB3_HLS_MIN_PROGRESS_RATIO", 0.75)?;
    let seek_seconds = env_optional_f64("WEEB3_HLS_SEEK_SECONDS")?;
    let connection_validation =
        env::var("WEEB3_HLS_CONNECTION_VALIDATION").unwrap_or_else(|_| "page_cdp".to_string());
    let shared_worker_connection_logs = match connection_validation.as_str() {
        "page_cdp" => false,
        "shared_worker_logs" => true,
        value => {
            return Err(anyhow!(
                "WEEB3_HLS_CONNECTION_VALIDATION must be page_cdp or shared_worker_logs, not {value}"
            ));
        }
    };
    let minimum_usable_connections = env_u64("WEEB3_HLS_MIN_USABLE_CONNECTIONS", 1)? as usize;
    let max_usable_connection_ms = env_u64("WEEB3_HLS_MAX_USABLE_CONNECTION_MS", 10_000)? as f64;
    if shared_worker_connection_logs && minimum_usable_connections == 0 {
        return Err(anyhow!(
            "WEEB3_HLS_MIN_USABLE_CONNECTIONS must be positive for shared_worker_logs validation"
        ));
    }
    let require_buffer_growth = env_bool(
        "WEEB3_HLS_REQUIRE_BUFFER_GROWTH",
        !live_target && profile_seconds >= 60,
    );
    if !(0.0..=1.0).contains(&minimum_progress_ratio) {
        return Err(anyhow!(
            "WEEB3_HLS_MIN_PROGRESS_RATIO must be between zero and one"
        ));
    }

    let timeout = Duration::from_secs(profile_seconds.saturating_add(45));
    let browser = launch_fresh_edge(timeout)?;
    let browser_root_pid = browser
        .get_process_id()
        .ok_or_else(|| anyhow!("Edge did not expose its root process id"))?;
    let tab = browser
        .new_tab()
        .map_err(|error| anyhow!("failed to open HLS profile tab: {error:?}"))?;
    tab.set_default_timeout(timeout);

    tab.call_method(EnableNetwork {
        max_total_buffer_size: None,
        max_resource_buffer_size: None,
        max_post_data_size: None,
        report_direct_socket_traffic: None,
        enable_durable_messages: None,
    })
    .map_err(|error| anyhow!("failed to enable Edge network events: {error:?}"))?;
    tab.call_method(ClearBrowserCache(None))
        .map_err(|error| anyhow!("failed to clear Edge's HTTP cache: {error:?}"))?;
    tab.call_method(SetCacheDisabled {
        cache_disabled: true,
    })
    .map_err(|error| anyhow!("failed to disable Edge's HTTP cache: {error:?}"))?;
    tab.call_method(AddScriptToEvaluateOnNewDocument {
        source: PROFILE_SCRIPT.to_string(),
        world_name: None,
        include_command_line_api: None,
        run_immediately: None,
    })
    .map_err(|error| anyhow!("failed to install HLS smoke instrumentation: {error:?}"))?;

    // Observe real WebSocket construction rather than merely counting queued
    // bootnode addresses. The profile fails unless all 160 cold-start dials are
    // dispatched within one 150ms burst.
    let navigation_start = Arc::new(Mutex::new(None::<Instant>));
    let websocket_times = Arc::new(Mutex::new(Vec::<f64>::new()));
    let websocket_request_times = Arc::new(Mutex::new(Vec::<f64>::new()));
    let websocket_handshake_times = Arc::new(Mutex::new(Vec::<f64>::new()));
    let hls_network = Arc::new(Mutex::new(Vec::<Value>::new()));
    let event_navigation_start = Arc::clone(&navigation_start);
    let event_websocket_times = Arc::clone(&websocket_times);
    let event_websocket_request_times = Arc::clone(&websocket_request_times);
    let event_websocket_handshake_times = Arc::clone(&websocket_handshake_times);
    let event_hls_network = Arc::clone(&hls_network);
    let _network_listener = tab
        .add_event_listener(Arc::new(move |event: &Event| match event {
            Event::NetworkWebSocketCreated(_)
                if let Ok(started) = event_navigation_start.lock()
                    && let Some(started) = *started
                    && let Ok(mut times) = event_websocket_times.lock() =>
            {
                times.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            Event::NetworkWebSocketWillSendHandshakeRequest(_)
                if let Ok(started) = event_navigation_start.lock()
                    && let Some(started) = *started
                    && let Ok(mut times) = event_websocket_request_times.lock() =>
            {
                times.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            Event::NetworkWebSocketHandshakeResponseReceived(_)
                if let Ok(started) = event_navigation_start.lock()
                    && let Some(started) = *started
                    && let Ok(mut times) = event_websocket_handshake_times.lock() =>
            {
                times.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            Event::NetworkResponseReceived(event)
                if event.params.response.url.contains("/feeds/")
                    || event.params.response.url.contains("/hls/bytes/") =>
            {
                if let Ok(mut events) = event_hls_network.lock() {
                    events.push(json!({
                        "kind": "response",
                        "request_id": event.params.request_id,
                        "url": event.params.response.url,
                        "status": event.params.response.status,
                        "mime_type": event.params.response.mime_type,
                        "from_service_worker": event.params.response.from_service_worker
                    }));
                }
            }
            Event::NetworkLoadingFailed(event) => {
                if let Ok(mut events) = event_hls_network.lock() {
                    events.push(json!({
                        "kind": "failed",
                        "request_id": event.params.request_id,
                        "error": event.params.error_text,
                        "canceled": event.params.canceled
                    }));
                }
            }
            _ => {}
        }))
        .map_err(|error| anyhow!("failed to observe WebSocket creation: {error:?}"))?;

    let resource_sampler = BrowserResourceSampler::start(browser_root_pid);
    *navigation_start
        .lock()
        .map_err(|_| anyhow!("navigation clock lock was poisoned"))? = Some(Instant::now());
    tab.navigate_to(&target_url)
        .map_err(|error| anyhow!("failed to navigate to HLS profile URL: {error:?}"))?
        .wait_until_navigated()
        .map_err(|error| anyhow!("HLS profile navigation did not finish: {error:?}"))?;

    // Give a failed startup enough time to produce useful diagnostics even though
    // the required startup threshold remains five seconds by default.
    let startup_observation_ms = max_startup_ms.saturating_add(10_000);
    let cold_playing = wait_for_first_playing(&tab, Duration::from_millis(startup_observation_ms))?;
    let startup_playlist = evaluate_playlist(&tab)?;
    let playing = if let (Some(_), Some(position)) = (cold_playing, seek_seconds) {
        wait_for_seekable_position(&tab, position, Duration::from_secs(30))?;
        begin_seek_profile(&tab, position)?;
        wait_for_first_playing(&tab, Duration::from_millis(startup_observation_ms))?
    } else {
        cold_playing
    };
    if playing.is_some() {
        resource_sampler.mark_steady_playback()?;
        thread::sleep(Duration::from_secs(profile_seconds));
    }
    let resource_usage = resource_sampler.finish()?;

    let final_playlist = evaluate_playlist(&tab)?;
    let browser_metrics = evaluate_profile(&tab)?;
    let browser_targets = match tab.call_method(GetTargets { filter: None }) {
        Ok(snapshot) => json!({
            "captured_after_playback": true,
            "target_infos": snapshot.target_infos
        }),
        Err(error) => json!({
            "captured_after_playback": true,
            "error": format!("{error:?}")
        }),
    };
    let served_assets = evaluate_served_asset_provenance(&tab)?;
    let socket_times = websocket_times
        .lock()
        .map_err(|_| anyhow!("WebSocket timing lock was poisoned"))?
        .clone();
    let websocket_burst = summarize_websocket_burst(&socket_times);
    let mut websocket_request_times = websocket_request_times
        .lock()
        .map_err(|_| anyhow!("WebSocket request timing lock was poisoned"))?
        .clone();
    websocket_request_times.sort_by(f64::total_cmp);
    let websocket_request_burst = summarize_websocket_burst(&websocket_request_times);
    let mut websocket_handshake_times = websocket_handshake_times
        .lock()
        .map_err(|_| anyhow!("WebSocket handshake timing lock was poisoned"))?
        .clone();
    websocket_handshake_times.sort_by(f64::total_cmp);
    let websocket_handshake_burst = summarize_websocket_burst(&websocket_handshake_times);
    let hls_network = hls_network
        .lock()
        .map_err(|_| anyhow!("HLS network event lock was poisoned"))?
        .clone();
    let playback = summarize_playback(&browser_metrics, profile_seconds as f64);
    let efficiency =
        summarize_hls_efficiency(&browser_metrics, &playback, &resource_usage.steady_playback);
    let usable_connections = summarize_usable_connections(&browser_metrics);
    let provenance = collect_run_provenance(served_assets)?;

    let report = json!({
        "target_url": target_url,
        "cold_first_playback_ms": cold_playing,
        "seek_seconds": seek_seconds,
        "requested_playback_seconds": profile_seconds,
        "requirements": {
            "max_startup_ms": max_startup_ms,
            "max_stall_ms": max_stall_ms,
            "minimum_progress_ratio": minimum_progress_ratio,
            "require_buffer_growth": require_buffer_growth,
            "connection_validation": connection_validation,
            "minimum_usable_connections": minimum_usable_connections,
            "max_usable_connection_ms": max_usable_connection_ms
        },
        "summary": {
            "playback": playback,
            "browser_process_tree": {
                "startup": &resource_usage.startup,
                "steady_playback": &resource_usage.steady_playback
            },
            "hls_efficiency": efficiency,
            "usable_connections": usable_connections,
            "websocket_burst": websocket_burst,
            "websocket_request_burst": websocket_request_burst,
            "websocket_handshake_burst": websocket_handshake_burst
        },
        "playlist_at_start": startup_playlist,
        "playlist_at_end": final_playlist,
        "hls_network": hls_network,
        "browser_process_tree": resource_usage,
        "browser_targets": browser_targets,
        "browser": browser_metrics,
        "provenance": provenance
    });
    let output = write_report(&report)?;
    println!("HLS profile written to {output}");
    println!(
        "HLS profile summary: {}",
        serde_json::to_string_pretty(&report["summary"])
            .context("failed to serialize HLS summary")?
    );

    validate_playback(
        &playback,
        profile_seconds as f64,
        max_startup_ms as f64,
        max_stall_ms as f64,
        minimum_progress_ratio,
        !live_target && seek_seconds.is_none(),
        require_buffer_growth,
    )?;
    if seek_seconds.is_some() && cold_playing.is_none_or(|startup| startup > max_startup_ms as f64)
    {
        return Err(anyhow!(
            "HLS cold playback before the middle seek took {:?}ms; limit is {max_startup_ms}ms",
            cold_playing
        ));
    }
    if let Some(position) = seek_seconds
        && summary_position_error(&playback, position) > 1.15
    {
        return Err(anyhow!(
            "HLS middle playback began at {:?}s instead of requested {position:.3}s",
            playback.first_media_time_s
        ));
    }
    validate_browser_environment(&browser_metrics, !npm_target)?;
    validate_playlist_position(
        &playback,
        &startup_playlist,
        &final_playlist,
        !live_target || seek_seconds.is_some(),
    )?;
    if shared_worker_connection_logs {
        println!(
            "SharedWorker socket construction is outside page-target CDP; explicitly validating timestamped usable-connection logs"
        );
        validate_shared_worker_connections(
            &browser_metrics,
            minimum_usable_connections,
            max_usable_connection_ms,
        )?;
    } else {
        validate_connection_burst(&websocket_burst)?;
    }
    Ok(())
}

fn wait_for_first_playing(
    tab: &headless_chrome::browser::tab::Tab,
    timeout: Duration,
) -> Result<Option<f64>> {
    let deadline = Instant::now() + timeout;
    loop {
        let value = tab
            .evaluate(
                "window.__weeb3HlsSmokeProfile?.marks.first_confirmed_playback_ms ?? null",
                false,
            )
            .map_err(|error| anyhow!("failed to poll HLS playback: {error:?}"))?
            .value;
        if let Some(at_ms) = value.as_ref().and_then(Value::as_f64) {
            return Ok(Some(at_ms));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn begin_seek_profile(tab: &headless_chrome::browser::tab::Tab, position: f64) -> Result<()> {
    let script = format!("window.__weeb3BeginHlsSeekProfile?.({position}) ?? null");
    let started = tab
        .evaluate(&script, false)
        .map_err(|error| anyhow!("failed to begin middle-playback profile: {error:?}"))?
        .value
        .as_ref()
        .and_then(Value::as_f64);
    if started.is_none() {
        return Err(anyhow!("the HLS player could not begin its middle seek"));
    }
    Ok(())
}

fn wait_for_seekable_position(
    tab: &headless_chrome::browser::tab::Tab,
    position: f64,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let expression = format!(
            "(() => {{ const video = document.querySelector('video'); return !!video && Number.isFinite(video.duration) && video.duration >= {position}; }})()"
        );
        if tab
            .evaluate(&expression, false)
            .map_err(|error| anyhow!("failed to inspect HLS seekability: {error:?}"))?
            .value
            .as_ref()
            .and_then(Value::as_bool)
            == Some(true)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "HLS duration never reached middle seek {position:.3}s"
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn evaluate_profile(tab: &headless_chrome::browser::tab::Tab) -> Result<Value> {
    let remote = tab
        .evaluate(RESULT_SCRIPT, true)
        .map_err(|error| anyhow!("failed to read HLS profile: {error:?}"))?;
    let raw = remote
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("Edge did not return HLS profile JSON"))?;
    serde_json::from_str(&raw).context("failed to parse HLS profile JSON")
}

fn evaluate_served_asset_provenance(tab: &headless_chrome::browser::tab::Tab) -> Result<Value> {
    let remote = tab
        .evaluate(SERVED_ASSET_PROVENANCE_SCRIPT, true)
        .map_err(|error| anyhow!("failed to inspect served asset provenance: {error:?}"))?;
    let raw = remote
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("Edge did not return served asset provenance JSON"))?;
    serde_json::from_str(&raw).context("failed to parse served asset provenance JSON")
}

fn evaluate_playlist(tab: &headless_chrome::browser::tab::Tab) -> Result<Value> {
    let remote = tab
        .evaluate(PLAYLIST_SCRIPT, true)
        .map_err(|error| anyhow!("failed to inspect the rendered HLS playlist: {error:?}"))?;
    let raw = remote
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("Edge did not return rendered HLS playlist JSON"))?;
    serde_json::from_str(&raw).context("failed to parse rendered HLS playlist JSON")
}

fn summarize_playback(metrics: &Value, requested_seconds: f64) -> PlaybackSummary {
    let phase_started_ms = number_at(metrics, "/phase_started_ms").unwrap_or(0.0);
    let first_play_event_ms =
        number_at(metrics, "/marks/first_playing_ms").map(|at| (at - phase_started_ms).max(0.0));
    let first_presented_frame_ms = number_at(metrics, "/marks/first_presented_frame_ms")
        .map(|at| (at - phase_started_ms).max(0.0));
    let first_playing_at = number_at(metrics, "/marks/first_confirmed_playback_ms");
    let first_playing_ms = first_playing_at.map(|at| (at - phase_started_ms).max(0.0));
    let first_media_time_s = number_at(metrics, "/marks/first_confirmed_playback_current_time_s");
    let samples: Vec<&Value> = metrics
        .get("samples")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|sample| {
            sample
                .get("at_ms")
                .and_then(Value::as_f64)
                .is_some_and(|at| first_playing_at.is_some_and(|first| at + 0.001 >= first))
        })
        .collect();

    let final_media_time_s = samples
        .iter()
        .rev()
        .find_map(|sample| sample.get("current_time_s").and_then(Value::as_f64));
    let observed_after_playing_ms = samples
        .last()
        .and_then(|sample| sample.get("at_ms").and_then(Value::as_f64))
        .zip(first_playing_at)
        .map_or(0.0, |(last, first)| (last - first).max(0.0));
    let media_advance_s = final_media_time_s
        .zip(first_media_time_s)
        .map_or(0.0, |(last, first)| (last - first).max(0.0));

    let mut last_progress_at = first_playing_at.unwrap_or(0.0);
    let mut last_time = first_media_time_s;
    let mut max_stagnant_ms: f64 = 0.0;
    let mut timeline_regressions = 0;
    let mut timeline_forward_jumps = 0;
    let mut duration_regressions = 0;
    let mut previous_at = first_playing_at;
    let mut previous_duration: Option<f64> = None;
    for sample in &samples {
        let Some(at) = sample.get("at_ms").and_then(Value::as_f64) else {
            continue;
        };
        let Some(current) = sample.get("current_time_s").and_then(Value::as_f64) else {
            continue;
        };
        if let Some(previous) = last_time {
            if current > previous + 0.03 {
                last_progress_at = at;
                let expected = previous_at.map_or(0.0, |previous_at| {
                    ((at - previous_at).max(0.0) / 1_000.0)
                        * sample
                            .get("playback_rate")
                            .and_then(Value::as_f64)
                            .unwrap_or(1.0)
                });
                if current - previous > expected + 0.75 {
                    timeline_forward_jumps += 1;
                }
            } else if current < previous - 0.25 {
                timeline_regressions += 1;
                last_progress_at = at;
            }
        }
        if let Some(duration) = sample.get("duration_s").and_then(Value::as_f64) {
            if previous_duration.is_some_and(|previous| duration < previous - 0.25) {
                duration_regressions += 1;
            }
            previous_duration = Some(duration);
        }
        max_stagnant_ms = max_stagnant_ms.max((at - last_progress_at).max(0.0));
        last_time = Some(current);
        previous_at = Some(at);
    }

    let event_count = |name: &str| {
        metrics
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|event| {
                event.get("name").and_then(Value::as_str) == Some(name)
                    && event
                        .get("ready_state")
                        .and_then(Value::as_u64)
                        .is_none_or(|ready| ready < 3)
                    && event
                        .get("at_ms")
                        .and_then(Value::as_f64)
                        .is_some_and(|at| first_playing_at.is_some_and(|first| at + 0.001 >= first))
            })
            .count()
    };
    let media_error_events = metrics
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|event| event.get("error").is_some_and(|error| !error.is_null()))
        .count();

    let sample_number = |sample: Option<&&Value>, field: &str| {
        sample
            .and_then(|value| value.get(field))
            .and_then(Value::as_f64)
    };
    let start = samples.first();
    let end = samples.last();
    let maximum_buffer_s = samples
        .iter()
        .filter_map(|sample| sample.get("forward_buffer_s").and_then(Value::as_f64))
        .reduce(f64::max);
    let minimum_buffer_s = samples
        .iter()
        .filter_map(|sample| sample.get("forward_buffer_s").and_then(Value::as_f64))
        .reduce(f64::min);
    let early_limit = first_playing_at.map(|first| first + 10_000.0);
    let late_limit = samples
        .last()
        .and_then(|sample| sample.get("at_ms").and_then(Value::as_f64))
        .map(|last| last - 10_000.0);
    let minute_window = first_playing_at.map(|first| (first + 50_000.0, first + 60_000.0));
    let low_water = |late: bool| {
        samples
            .iter()
            .filter(|sample| {
                sample
                    .get("at_ms")
                    .and_then(Value::as_f64)
                    .is_some_and(|at| {
                        if late {
                            late_limit.is_some_and(|limit| at >= limit)
                        } else {
                            early_limit.is_some_and(|limit| at <= limit)
                        }
                    })
            })
            .filter_map(|sample| sample.get("forward_buffer_s").and_then(Value::as_f64))
            .reduce(f64::min)
    };
    let early_low_water_s = low_water(false);
    let late_low_water_s = low_water(true);
    let minute_low_water_s = samples
        .iter()
        .filter(|sample| {
            sample
                .get("at_ms")
                .and_then(Value::as_f64)
                .is_some_and(|at| {
                    minute_window.is_some_and(|(start, end)| at >= start && at <= end)
                })
        })
        .filter_map(|sample| sample.get("forward_buffer_s").and_then(Value::as_f64))
        .reduce(f64::min);

    PlaybackSummary {
        first_play_event_ms,
        first_presented_frame_ms,
        first_playing_ms,
        first_media_time_s,
        final_media_time_s,
        observed_after_playing_ms,
        media_advance_s,
        progress_ratio: if requested_seconds > 0.0 {
            media_advance_s / requested_seconds
        } else {
            0.0
        },
        max_stagnant_ms,
        timeline_regressions,
        timeline_forward_jumps,
        duration_regressions,
        waiting_events: event_count("waiting"),
        stalled_events: event_count("stalled"),
        paused_samples: samples
            .iter()
            .filter(|sample| sample.get("paused").and_then(Value::as_bool) == Some(true))
            .count(),
        ended_events: event_count("ended"),
        media_error_events,
        start_buffer_s: sample_number(start, "forward_buffer_s"),
        final_buffer_s: sample_number(end, "forward_buffer_s"),
        minimum_buffer_s,
        maximum_buffer_s,
        early_low_water_s,
        minute_low_water_s,
        late_low_water_s,
        low_water_trend_s: early_low_water_s
            .zip(late_low_water_s)
            .map(|(early, late)| late - early),
        post_minute_low_water_trend_s: minute_low_water_s
            .zip(late_low_water_s)
            .map(|(minute, late)| late - minute),
        start_duration_s: sample_number(start, "duration_s"),
        final_duration_s: sample_number(end, "duration_s"),
    }
}

fn summarize_websocket_burst(times_ms: &[f64]) -> WebSocketBurstSummary {
    let first = times_ms.first().copied();
    let attempt_160 = times_ms.get(159).copied();
    WebSocketBurstSummary {
        total_created: times_ms.len(),
        first_created_ms: first,
        created_within_150_ms_of_first: first.map_or(0, |origin| {
            times_ms.iter().filter(|&&at| at - origin <= 150.0).count()
        }),
        created_within_3s: times_ms.iter().filter(|&&at| at <= 3_000.0).count(),
        created_within_5s: times_ms.iter().filter(|&&at| at <= 5_000.0).count(),
        attempt_160_ms: attempt_160,
        first_to_attempt_160_ms: first
            .zip(attempt_160)
            .map(|(origin, last)| (last - origin).max(0.0)),
    }
}

fn summarize_usable_connections(metrics: &Value) -> UsableConnectionSummary {
    let times = usable_connection_times(metrics);
    UsableConnectionSummary {
        source: "timestamped Connected-to-peer/bootnode interface logs (unique overlays)",
        total_unique_ready: times.len(),
        first_ready_ms: times.first().copied(),
        ready_within_3s: times.iter().filter(|&&at| at <= 3_000.0).count(),
        ready_within_5s: times.iter().filter(|&&at| at <= 5_000.0).count(),
        ready_40_ms: times.get(39).copied(),
        ready_80_ms: times.get(79).copied(),
        ready_160_ms: times.get(159).copied(),
        ready_200_ms: times.get(199).copied(),
    }
}

fn usable_connection_times(metrics: &Value) -> Vec<f64> {
    let mut ready_by_overlay = HashMap::<String, f64>::new();
    for line in metrics
        .get("connection_logs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let Some((ready_ms, overlay)) = parse_usable_connection_log(line) else {
            continue;
        };
        ready_by_overlay
            .entry(overlay.to_owned())
            .and_modify(|previous| *previous = previous.min(ready_ms))
            .or_insert(ready_ms);
    }
    let mut times: Vec<_> = ready_by_overlay.into_values().collect();
    times.sort_by(f64::total_cmp);
    times
}

fn parse_usable_connection_log(line: &str) -> Option<(f64, &str)> {
    let line = line.strip_prefix("[+")?;
    let (timestamp, message) = line.split_once("ms] ")?;
    let overlay = message
        .strip_prefix("Connected to peer ")
        .or_else(|| message.strip_prefix("Connected to bootnode "))?
        .trim();
    (!overlay.is_empty())
        .then(|| timestamp.parse::<f64>().ok().map(|at| (at, overlay)))
        .flatten()
}

fn validate_playback(
    summary: &PlaybackSummary,
    requested_seconds: f64,
    max_startup_ms: f64,
    max_stall_ms: f64,
    minimum_progress_ratio: f64,
    beginning: bool,
    require_buffer_growth: bool,
) -> Result<()> {
    let startup = summary
        .first_playing_ms
        .ok_or_else(|| anyhow!("HLS playback never emitted playing"))?;
    if startup > max_startup_ms {
        return Err(anyhow!(
            "HLS cold startup {startup:.0}ms exceeded {max_startup_ms:.0}ms"
        ));
    }
    if summary.first_presented_frame_ms.is_none() {
        return Err(anyhow!("HLS playback never presented a video frame"));
    }
    if beginning
        && summary
            .first_media_time_s
            .is_none_or(|position| position > 1.0)
    {
        return Err(anyhow!(
            "HLS beginning playback started at {:.3}s instead of the beginning",
            summary.first_media_time_s.unwrap_or(f64::NAN)
        ));
    }
    let minimum_advance = requested_seconds * minimum_progress_ratio;
    if summary.media_advance_s < minimum_advance {
        return Err(anyhow!(
            "HLS advanced {:.3}s during a {:.3}s observation; expected at least {:.3}s",
            summary.media_advance_s,
            requested_seconds,
            minimum_advance
        ));
    }
    if summary.max_stagnant_ms > max_stall_ms {
        return Err(anyhow!(
            "HLS media clock stopped for {:.0}ms; limit is {:.0}ms",
            summary.max_stagnant_ms,
            max_stall_ms
        ));
    }
    if summary.timeline_regressions != 0 {
        return Err(anyhow!(
            "HLS media clock moved backwards {} time(s)",
            summary.timeline_regressions
        ));
    }
    if summary.timeline_forward_jumps != 0 || summary.duration_regressions != 0 {
        return Err(anyhow!(
            "HLS jumped forward {} time(s) and regressed duration {} time(s)",
            summary.timeline_forward_jumps,
            summary.duration_regressions
        ));
    }
    if summary.waiting_events != 0 || summary.stalled_events != 0 || summary.paused_samples != 0 {
        return Err(anyhow!(
            "HLS emitted {} waiting, {} stalled, and {} paused post-start sample(s)",
            summary.waiting_events,
            summary.stalled_events,
            summary.paused_samples
        ));
    }
    if summary.ended_events != 0 || summary.media_error_events != 0 {
        return Err(anyhow!(
            "HLS emitted {} ended event(s) and {} media error event(s)",
            summary.ended_events,
            summary.media_error_events
        ));
    }
    if require_buffer_growth {
        let trend = summary
            .low_water_trend_s
            .ok_or_else(|| anyhow!("HLS buffer trend could not be measured"))?;
        if trend < -0.25 {
            return Err(anyhow!(
                "HLS late buffer low-water decreased by {:.3}s",
                -trend
            ));
        }
        let post_minute_trend = summary
            .post_minute_low_water_trend_s
            .ok_or_else(|| anyhow!("HLS post-minute buffer trend could not be measured"))?;
        if post_minute_trend < -0.25 {
            return Err(anyhow!(
                "HLS buffer low-water decreased by {:.3}s after one minute",
                -post_minute_trend
            ));
        }
        if summary
            .final_buffer_s
            .zip(summary.start_buffer_s)
            .is_none_or(|(final_buffer, start_buffer)| final_buffer <= start_buffer + 0.5)
        {
            return Err(anyhow!(
                "HLS beginning buffer did not grow: start={:?}, final={:?}",
                summary.start_buffer_s,
                summary.final_buffer_s
            ));
        }
    }
    Ok(())
}

fn validate_playlist_position(
    summary: &PlaybackSummary,
    playlist: &Value,
    final_playlist: &Value,
    beginning: bool,
) -> Result<()> {
    let object = playlist
        .as_object()
        .ok_or_else(|| anyhow!("the browser could not inspect its startup HLS playlist"))?;
    if let Some(error) = object.get("error").and_then(Value::as_str) {
        return Err(anyhow!("startup HLS playlist inspection failed: {error}"));
    }
    if object.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!(
            "startup HLS playlist returned status {:?}",
            object.get("status")
        ));
    }
    let segment_count = object
        .get("segment_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if segment_count < 2 {
        return Err(anyhow!(
            "startup HLS playlist exposed only {segment_count} segment(s)"
        ));
    }
    let playlist_duration = object
        .get("duration_s")
        .and_then(Value::as_f64)
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow!("startup HLS playlist had no finite elapsed duration"))?;
    let displayed_duration = summary
        .start_duration_s
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow!("the player had no finite elapsed duration at playback start"))?;
    let target_duration = object
        .get("target_duration_s")
        .and_then(Value::as_f64)
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .unwrap_or(5.0);

    // The independent fetch happens immediately after the first presented frame.
    // A growing feed may append once while that request is in flight, and MSE may
    // expose container PTS slightly beyond EXTINF. Either clock must remain within
    // one target-duration update of the other.
    let elapsed_tolerance = target_duration + 0.5;
    let elapsed_gap = playlist_duration - displayed_duration;
    if !beginning && elapsed_gap.abs() > elapsed_tolerance {
        return Err(anyhow!(
            "player elapsed duration was not current at startup: displayed={displayed_duration:.3}s, playlist={playlist_duration:.3}s"
        ));
    }

    let final_playlist_duration = final_playlist
        .get("duration_s")
        .and_then(Value::as_f64)
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow!("final HLS playlist had no finite elapsed duration"))?;
    let final_displayed_duration = summary
        .final_duration_s
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow!("the player had no finite elapsed duration at profile end"))?;
    let final_gap = final_playlist_duration - final_displayed_duration;
    if final_gap.abs() > elapsed_tolerance {
        return Err(anyhow!(
            "player elapsed duration was not current at profile end: displayed={final_displayed_duration:.3}s, playlist={final_playlist_duration:.3}s"
        ));
    }
    let manifest_growth = final_playlist_duration - playlist_duration;
    let observable_growth = summary.observed_after_playing_ms / 1_000.0 + elapsed_tolerance + 1.0;
    if !beginning && manifest_growth > observable_growth {
        return Err(anyhow!(
            "HLS elapsed duration jumped by {manifest_growth:.3}s after playback began; the startup manifest was stale"
        ));
    }

    if !beginning {
        let tail_one = object
            .get("tail_one_s")
            .and_then(Value::as_f64)
            .filter(|duration| duration.is_finite() && *duration > 0.0)
            .ok_or_else(|| anyhow!("startup HLS playlist had no live tail segment"))?;
        let tail_two = object
            .get("tail_two_s")
            .and_then(Value::as_f64)
            .filter(|duration| duration.is_finite() && *duration > tail_one)
            .ok_or_else(|| anyhow!("startup HLS playlist had no two-segment live edge"))?;
        let tail_three = object
            .get("tail_three_s")
            .and_then(Value::as_f64)
            .filter(|duration| duration.is_finite() && *duration > tail_two)
            .ok_or_else(|| anyhow!("startup HLS playlist had no three-segment live edge"))?;
        let actual_position = summary
            .first_media_time_s
            .ok_or_else(|| anyhow!("live playback had no initial media position"))?;
        let expected = (displayed_duration - tail_three).max(0.0);
        if (actual_position - expected).abs() > 1.0 {
            return Err(anyhow!(
                "live playback did not start two segments behind its edge: position={actual_position:.3}s, expected={expected:.3}s"
            ));
        }
        let start_buffer = summary
            .start_buffer_s
            .filter(|duration| duration.is_finite() && *duration >= 0.0)
            .ok_or_else(|| anyhow!("live playback had no measurable startup runway"))?;
        let current_segment = tail_three - tail_two;
        if start_buffer + 0.75 < current_segment {
            return Err(anyhow!(
                "live playback started without one buffered segment: buffered={start_buffer:.3}s, segment={current_segment:.3}s"
            ));
        }
    }
    Ok(())
}

fn validate_browser_environment(metrics: &Value, require_interface_log: bool) -> Result<()> {
    if metrics
        .pointer("/service_worker/controlled")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(anyhow!(
            "the cold playback page was not controlled by its service worker"
        ));
    }
    let page_errors = metrics
        .get("page_errors")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if page_errors != 0 {
        return Err(anyhow!(
            "the playback page reported {page_errors} JavaScript error(s)"
        ));
    }
    if let Some(error) = metrics
        .pointer("/document/hls_error")
        .and_then(Value::as_str)
    {
        return Err(anyhow!("hls.js reported fatal playback error {error}"));
    }
    let opened = metrics
        .get("hls_open_log")
        .and_then(Value::as_str)
        .is_some_and(|line| line.contains("HLS open index=") && line.contains("elapsed="))
        || metrics
            .get("interface_logs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|line| line.contains("HLS open index=") && line.contains("elapsed="));
    if require_interface_log && !opened {
        return Err(anyhow!(
            "the playback page did not retain its HLS elapsed-time startup log"
        ));
    }
    Ok(())
}

fn validate_connection_burst(summary: &WebSocketBurstSummary) -> Result<()> {
    if summary.total_created < 160 {
        return Err(anyhow!(
            "CDP observed only {} of the requested 160 WebSocket attempts",
            summary.total_created
        ));
    }
    let span = summary
        .first_to_attempt_160_ms
        .ok_or_else(|| anyhow!("CDP did not time the 160th WebSocket attempt"))?;
    if span > 150.0 {
        return Err(anyhow!(
            "the first 160 WebSocket attempts took {span:.1}ms; limit is 150ms"
        ));
    }
    Ok(())
}

fn validate_shared_worker_connections(
    metrics: &Value,
    minimum_usable_connections: usize,
    max_usable_connection_ms: f64,
) -> Result<()> {
    let shared_worker_attached = metrics
        .get("shared_worker_attached")
        .and_then(Value::as_bool)
        == Some(true);
    if !shared_worker_attached {
        return Err(anyhow!(
            "shared_worker_logs validation was selected, but the interface did not report attaching to its SharedWorker"
        ));
    }
    let times = usable_connection_times(metrics);
    if times.len() < minimum_usable_connections {
        return Err(anyhow!(
            "SharedWorker logs retained only {} unique usable connection(s); required {minimum_usable_connections}",
            times.len()
        ));
    }
    if minimum_usable_connections > 0 {
        let ready_ms = times[minimum_usable_connections - 1];
        if ready_ms > max_usable_connection_ms {
            return Err(anyhow!(
                "SharedWorker usable connection {minimum_usable_connections} became ready at {ready_ms:.0}ms; limit is {max_usable_connection_ms:.0}ms"
            ));
        }
    }
    Ok(())
}

fn number_at(value: &Value, pointer: &str) -> Option<f64> {
    value.pointer(pointer).and_then(Value::as_f64)
}

fn summary_position_error(summary: &PlaybackSummary, requested: f64) -> f64 {
    summary
        .first_media_time_s
        .map_or(f64::INFINITY, |actual| (actual - requested).abs())
}

fn write_report(report: &Value) -> Result<String> {
    fs::create_dir_all("target/weeb3-hls-profile")
        .context("failed to create target/weeb3-hls-profile directory")?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let output = format!("target/weeb3-hls-profile/hls-profile-{timestamp}.json");
    fs::write(
        &output,
        serde_json::to_string_pretty(report).context("failed to serialize HLS profile")?,
    )
    .context("failed to write HLS profile")?;
    Ok(output)
}

fn collect_run_provenance(served_assets: Value) -> Result<RunProvenance> {
    let current_dir = env::current_dir().context("failed to identify the profile worktree")?;
    let harness_executable = env::current_exe()
        .ok()
        .map(|path| file_provenance(&path))
        .transpose()?;
    let server_executable_path = env::var_os("WEEB3_HLS_SERVER_EXECUTABLE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let server_executable = server_executable_path
        .as_deref()
        .map(file_provenance)
        .transpose()?;
    let server_worktree = env::var_os("WEEB3_HLS_SERVER_WORKTREE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            server_executable_path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        });

    Ok(RunProvenance {
        variant: env::var("WEEB3_HLS_VARIANT")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        harness_git: git_provenance(&current_dir),
        harness_executable,
        server_git: server_worktree
            .as_deref()
            .map(git_provenance)
            .filter(|git| git.head.is_some()),
        server_executable,
        served_assets,
    })
}

fn git_provenance(path: &Path) -> GitProvenance {
    let worktree = git_output(path, &["rev-parse", "--show-toplevel"]);
    let head = git_output(path, &["rev-parse", "HEAD"]);
    let status = git_output(path, &["status", "--porcelain=v1", "--untracked-files=all"]);
    let changed_entries = status
        .as_deref()
        .into_iter()
        .flat_map(str::lines)
        .map(ToOwned::to_owned)
        .collect();
    GitProvenance {
        worktree,
        head,
        dirty: status.as_ref().map(|status| !status.is_empty()),
        changed_entries,
    }
}

fn git_output(path: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn file_provenance(path: &Path) -> Result<FileProvenance> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to hash profile artifact {}", path.display()))?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect profile artifact {}", path.display()))?;
    let sha3_256 = Sha3_256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis());
    Ok(FileProvenance {
        path: path.to_string_lossy().into_owned(),
        bytes: metadata.len(),
        modified_unix_ms,
        sha3_256,
    })
}

fn launch_fresh_edge(timeout: Duration) -> Result<Browser> {
    let edge = edge_executable()?;
    let ignore_certificate_errors = env_bool("WEEB3_IGNORE_CERTIFICATE_ERRORS", true);
    let mut args = vec![
        OsStr::new("--disable-background-networking"),
        OsStr::new("--disable-cache"),
        OsStr::new("--disable-dev-shm-usage"),
        OsStr::new("--disable-extensions"),
        OsStr::new("--no-first-run"),
        OsStr::new("--autoplay-policy=no-user-gesture-required"),
    ];
    if ignore_certificate_errors {
        args.push(OsStr::new("--ignore-certificate-errors"));
    }

    let mut builder = LaunchOptionsBuilder::default();
    builder
        .path(Some(edge))
        .headless(!env_bool("WEEB3_HEADFUL", false))
        .ignore_certificate_errors(ignore_certificate_errors)
        .sandbox(!env_bool("WEEB3_CHROME_NO_SANDBOX", true))
        .window_size(Some((1280, 720)))
        .idle_browser_timeout(timeout + Duration::from_secs(30))
        .args(args);
    let options = builder
        .build()
        .map_err(|error| anyhow!("failed to build Edge launch options: {error:?}"))?;
    Browser::new(options).map_err(|error| anyhow!("failed to launch Edge: {error:?}"))
}

fn edge_executable() -> Result<PathBuf> {
    if let Some(path) = env::var_os("WEEB3_CHROME").filter(|path| !path.is_empty()) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(anyhow!(
            "WEEB3_CHROME does not identify an Edge executable: {}",
            path.display()
        ));
    }

    let mut candidates = Vec::new();
    for base in [
        env::var_os("ProgramFiles(x86)"),
        env::var_os("ProgramFiles"),
        env::var_os("LOCALAPPDATA"),
    ]
    .into_iter()
    .flatten()
    {
        candidates.push(
            Path::new(&base)
                .join("Microsoft")
                .join("Edge")
                .join("Application")
                .join("msedge.exe"),
        );
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            anyhow!("Microsoft Edge was not found; set WEEB3_CHROME to the Edge executable")
        })
}

fn env_u64(name: &str, default: u64) -> Result<u64> {
    env::var(name).map_or(Ok(default), |value| {
        value
            .parse::<u64>()
            .with_context(|| format!("{name} must be an unsigned integer"))
    })
}

fn env_f64(name: &str, default: f64) -> Result<f64> {
    env::var(name).map_or(Ok(default), |value| {
        value
            .parse::<f64>()
            .with_context(|| format!("{name} must be a finite number"))
            .and_then(|parsed| {
                if parsed.is_finite() {
                    Ok(parsed)
                } else {
                    Err(anyhow!("{name} must be a finite number"))
                }
            })
    })
}

fn env_optional_f64(name: &str) -> Result<Option<f64>> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .parse::<f64>()
                .with_context(|| format!("{name} must be a non-negative finite number"))
                .and_then(|parsed| {
                    if parsed.is_finite() && parsed >= 0.0 {
                        Ok(parsed)
                    } else {
                        Err(anyhow!("{name} must be a non-negative finite number"))
                    }
                })
        })
        .transpose()
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name).map_or(default, |value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_summary_reports_advance_and_a_real_stagnation() {
        let metrics = json!({
            "marks": {
                "first_playing_ms": 1_000.0,
                "first_playing_current_time_s": 20.0,
                "first_presented_frame_ms": 1_010.0,
                "first_confirmed_playback_ms": 1_000.0,
                "first_confirmed_playback_current_time_s": 20.0
            },
            "samples": [
                {"at_ms": 1_000.0, "current_time_s": 20.0, "forward_buffer_s": 12.0, "duration_s": 50.0, "paused": false, "playback_rate": 1.0},
                {"at_ms": 1_500.0, "current_time_s": 20.5, "forward_buffer_s": 11.5, "duration_s": 50.0, "paused": false, "playback_rate": 1.0},
                {"at_ms": 3_000.0, "current_time_s": 20.5, "forward_buffer_s": 10.0, "duration_s": 54.0, "paused": false, "playback_rate": 1.0},
                {"at_ms": 3_500.0, "current_time_s": 22.5, "forward_buffer_s": 12.0, "duration_s": 54.0, "paused": false, "playback_rate": 1.0}
            ],
            "events": [
                {"at_ms": 2_000.0, "name": "waiting", "ready_state": 2, "error": null},
                {"at_ms": 2_500.0, "name": "waiting", "ready_state": 4, "error": null}
            ]
        });
        let summary = summarize_playback(&metrics, 3.0);
        assert_eq!(summary.media_advance_s, 2.5);
        assert_eq!(summary.max_stagnant_ms, 1_500.0);
        assert_eq!(summary.waiting_events, 1);
        assert_eq!(summary.timeline_forward_jumps, 1);
        assert_eq!(summary.start_duration_s, Some(50.0));
        assert_eq!(summary.final_duration_s, Some(54.0));
    }

    #[test]
    fn websocket_burst_uses_the_first_attempt_as_its_origin() {
        let times: Vec<f64> = (0..160).map(|index| 700.0 + index as f64 * 0.8).collect();
        let summary = summarize_websocket_burst(&times);
        assert_eq!(summary.total_created, 160);
        assert_eq!(summary.created_within_150_ms_of_first, 160);
        assert!((summary.first_to_attempt_160_ms.unwrap() - 127.2).abs() < f64::EPSILON * 512.0);
        validate_connection_burst(&summary).unwrap();
    }

    #[test]
    fn resource_phase_summary_is_time_weighted_and_uses_cpu_time() {
        let samples = vec![
            BrowserResourceSample {
                at_ms: 100.0,
                interval_ms: 100.0,
                phase: ResourcePhase::Startup,
                process_count: 3,
                resident_bytes: 100,
                private_equivalent_bytes: 200,
                accumulated_cpu_ms: 10,
                cpu_delta_ms: 10,
                processes: Vec::new(),
            },
            BrowserResourceSample {
                at_ms: 400.0,
                interval_ms: 300.0,
                phase: ResourcePhase::Startup,
                process_count: 4,
                resident_bytes: 300,
                private_equivalent_bytes: 400,
                accumulated_cpu_ms: 40,
                cpu_delta_ms: 30,
                processes: Vec::new(),
            },
            BrowserResourceSample {
                at_ms: 900.0,
                interval_ms: 500.0,
                phase: ResourcePhase::SteadyPlayback,
                process_count: 4,
                resident_bytes: 500,
                private_equivalent_bytes: 600,
                accumulated_cpu_ms: 50,
                cpu_delta_ms: 10,
                processes: Vec::new(),
            },
        ];
        let summary = summarize_resource_phase(&samples, ResourcePhase::Startup, 8);
        assert_eq!(summary.observed_ms, 400.0);
        assert_eq!(summary.average_resident_bytes, Some(250.0));
        assert_eq!(summary.average_private_equivalent_bytes, Some(350.0));
        assert_eq!(summary.peak_resident_bytes, Some(300));
        assert_eq!(summary.accumulated_cpu_ms, 40);
        assert_eq!(summary.average_cpu_cores, Some(0.1));
        assert_eq!(summary.average_cpu_percent_of_machine, Some(1.25));
        assert_eq!(summary.maximum_process_count, Some(4));
    }

    #[test]
    fn browser_resource_sampler_tracks_its_root_and_both_phases() {
        let sampler = BrowserResourceSampler::start(std::process::id());
        thread::sleep(Duration::from_millis(20));
        sampler.mark_steady_playback().unwrap();
        thread::sleep(Duration::from_millis(20));
        let summary = sampler.finish().unwrap();
        assert_eq!(summary.root_pid, std::process::id());
        assert!(summary.startup.sample_count >= 2);
        assert!(summary.steady_playback.sample_count >= 1);
        assert!(
            summary
                .startup
                .peak_resident_bytes
                .is_some_and(|bytes| bytes > 0)
        );
        assert!(
            summary
                .startup
                .peak_private_equivalent_bytes
                .is_some_and(|bytes| bytes > 0)
        );
    }

    #[test]
    fn usable_connection_summary_deduplicates_overlays_and_sorts_milestones() {
        let metrics = json!({
            "connection_logs": [
                "[+2500ms] Connected to peer overlay-a",
                "[+900ms] Connected to bootnode overlay-b",
                "[+2000ms] Connected to peer overlay-a",
                "[+50ms] unrelated"
            ]
        });
        let summary = summarize_usable_connections(&metrics);
        assert_eq!(summary.total_unique_ready, 2);
        assert_eq!(summary.first_ready_ms, Some(900.0));
        assert_eq!(summary.ready_within_3s, 2);
        assert_eq!(usable_connection_times(&metrics), vec![900.0, 2_000.0]);
    }

    #[test]
    fn shared_worker_connection_validation_is_explicit_and_timed() {
        let metrics = json!({
            "shared_worker_attached": true,
            "connection_logs": [
                "[+1000ms] Connected to peer overlay-a",
                "[+2000ms] Connected to peer overlay-b"
            ]
        });
        validate_shared_worker_connections(&metrics, 2, 2_500.0).unwrap();
        assert!(validate_shared_worker_connections(&metrics, 2, 1_500.0).is_err());
        assert!(
            validate_shared_worker_connections(
                &json!({"shared_worker_attached": false, "connection_logs": []}),
                1,
                5_000.0
            )
            .is_err()
        );
    }

    #[test]
    fn hls_efficiency_normalizes_cpu_by_completed_steady_bytes() {
        let metrics = json!({
            "marks": {
                "first_confirmed_playback_ms": 1_000.0,
                "first_confirmed_playback_current_time_s": 0.0
            },
            "samples": [
                {"at_ms": 1_000.0, "current_time_s": 0.0, "forward_buffer_s": 4.0, "duration_s": 20.0, "paused": false, "playback_rate": 1.0},
                {"at_ms": 2_000.0, "current_time_s": 1.0, "forward_buffer_s": 14.0, "duration_s": 20.0, "paused": false, "playback_rate": 1.0}
            ],
            "events": [],
            "hls_resources": [
                {"start_ms": 100.0, "response_end_ms": 500.0, "encoded_bytes": 1_048_576u64, "decoded_bytes": 1_048_576u64},
                {"start_ms": 600.0, "response_end_ms": 1_100.0, "encoded_bytes": 2_097_152u64, "decoded_bytes": 2_097_152u64},
                {"start_ms": 1_200.0, "response_end_ms": 1_800.0, "encoded_bytes": 3_145_728u64, "decoded_bytes": 3_145_728u64},
                {"start_ms": 2_100.0, "response_end_ms": 2_600.0, "encoded_bytes": 4_194_304u64, "decoded_bytes": 4_194_304u64}
            ]
        });
        let playback = summarize_playback(&metrics, 1.0);
        let steady = BrowserResourcePhaseSummary {
            sample_count: 2,
            observed_ms: 1_000.0,
            average_resident_bytes: None,
            peak_resident_bytes: None,
            average_private_equivalent_bytes: None,
            peak_private_equivalent_bytes: None,
            accumulated_cpu_ms: 10_000,
            average_cpu_cores: Some(10.0),
            average_cpu_percent_of_machine: None,
            maximum_process_count: None,
        };
        let summary = summarize_hls_efficiency(&metrics, &playback, &steady);
        assert_eq!(summary.all_hls_responses.encoded_bytes, 10 * 1_048_576);
        assert_eq!(summary.steady_playback.completed_responses, 2);
        assert_eq!(summary.steady_playback.encoded_bytes, 5 * 1_048_576);
        assert_eq!(summary.steady_playback.encoded_mib_per_second, Some(5.0));
        assert_eq!(summary.cpu_seconds_per_encoded_mib, Some(2.0));
        assert_eq!(summary.buffer_growth_s, Some(10.0));
        assert_eq!(summary.media_plus_buffer_growth_s, Some(11.0));
    }

    #[test]
    fn served_asset_provenance_covers_every_runtime_entrypoint() {
        for asset in ["weeb_3.js", "weeb_3_bg.wasm", "service.js", "worker.js"] {
            assert!(SERVED_ASSET_PROVENANCE_SCRIPT.contains(asset));
        }
        assert!(SERVED_ASSET_PROVENANCE_SCRIPT.contains("SHA-256"));
        assert!(SERVED_ASSET_PROVENANCE_SCRIPT.contains("cache: 'no-store'"));
    }
}
