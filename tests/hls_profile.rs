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
use headless_chrome::protocol::cdp::Page::AddScriptToEvaluateOnNewDocument;
use headless_chrome::protocol::cdp::types::Event;
use headless_chrome::{Browser, LaunchOptionsBuilder};
use serde::Serialize;
use serde_json_crates_io::{Value, json};
use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DEFAULT_PROFILE_SECONDS: u64 = 75;
const DEFAULT_MAX_STARTUP_MS: u64 = 5_000;
const DEFAULT_MAX_STALL_MS: u64 = 2_500;

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
        hls_open_log: null,
        refreshment_logs: []
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
            if (profile.hls_open_log === null && /HLS open index=.*elapsed=/.test(text)) {
                profile.hls_open_log = text;
            }
            if (/refresh|feed update/i.test(text)) {
                profile.refreshment_logs.push(text);
                if (profile.refreshment_logs.length > 2048) profile.refreshment_logs.shift();
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
            hls_error: document.querySelector('video')?.getAttribute('data-weeb3-hls-error') || null
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
            tail_two_s: durations.slice(-2).reduce((sum, value) => sum + value, 0),
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

#[derive(Clone, Debug, Serialize)]
struct WebSocketBurstSummary {
    total_created: usize,
    first_created_ms: Option<f64>,
    created_within_150_ms_of_first: usize,
    attempt_160_ms: Option<f64>,
    first_to_attempt_160_ms: Option<f64>,
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
    let hls_network = Arc::new(Mutex::new(Vec::<Value>::new()));
    let event_navigation_start = Arc::clone(&navigation_start);
    let event_websocket_times = Arc::clone(&websocket_times);
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
        thread::sleep(Duration::from_secs(profile_seconds));
    }

    let final_playlist = evaluate_playlist(&tab)?;
    let browser_metrics = evaluate_profile(&tab)?;
    let socket_times = websocket_times
        .lock()
        .map_err(|_| anyhow!("WebSocket timing lock was poisoned"))?
        .clone();
    let websocket_burst = summarize_websocket_burst(&socket_times);
    let hls_network = hls_network
        .lock()
        .map_err(|_| anyhow!("HLS network event lock was poisoned"))?
        .clone();
    let playback = summarize_playback(&browser_metrics, profile_seconds as f64);

    let report = json!({
        "target_url": target_url,
        "cold_first_playback_ms": cold_playing,
        "seek_seconds": seek_seconds,
        "requested_playback_seconds": profile_seconds,
        "requirements": {
            "max_startup_ms": max_startup_ms,
            "max_stall_ms": max_stall_ms,
            "minimum_progress_ratio": minimum_progress_ratio,
            "require_buffer_growth": require_buffer_growth
        },
        "summary": {
            "playback": playback,
            "websocket_burst": websocket_burst
        },
        "playlist_at_start": startup_playlist,
        "playlist_at_end": final_playlist,
        "hls_network": hls_network,
        "browser": browser_metrics
    });
    let output = write_report(&report)?;
    println!("HLS profile written to {output}");
    println!(
        "HLS profile summary: {}",
        serde_json_crates_io::to_string_pretty(&report["summary"])
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
    validate_connection_burst(&websocket_burst)?;
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
    serde_json_crates_io::from_str(&raw).context("failed to parse HLS profile JSON")
}

fn evaluate_playlist(tab: &headless_chrome::browser::tab::Tab) -> Result<Value> {
    let remote = tab
        .evaluate(PLAYLIST_SCRIPT, true)
        .map_err(|error| anyhow!("failed to inspect the rendered HLS playlist: {error:?}"))?;
    let raw = remote
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("Edge did not return rendered HLS playlist JSON"))?;
    serde_json_crates_io::from_str(&raw).context("failed to parse rendered HLS playlist JSON")
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
        attempt_160_ms: attempt_160,
        first_to_attempt_160_ms: first
            .zip(attempt_160)
            .map(|(origin, last)| (last - origin).max(0.0)),
    }
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
    if segment_count < 3 {
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
    let observable_growth = summary.observed_after_playing_ms / 1_000.0 + target_duration * 2.0;
    if !beginning && manifest_growth > observable_growth {
        return Err(anyhow!(
            "HLS elapsed duration jumped by {manifest_growth:.3}s after playback began; the startup manifest was stale"
        ));
    }

    if !beginning {
        let tail = object
            .get("tail_two_s")
            .and_then(Value::as_f64)
            .filter(|duration| duration.is_finite() && *duration > 0.0)
            .ok_or_else(|| anyhow!("startup HLS playlist had no two-segment live tail"))?;
        let expected_position = (displayed_duration - tail).max(0.0);
        let actual_position = summary
            .first_media_time_s
            .ok_or_else(|| anyhow!("live playback had no initial media position"))?;
        if (actual_position - expected_position).abs() > 1.0 {
            return Err(anyhow!(
                "live playback did not start two segments behind its displayed edge: position={actual_position:.3}s, expected={expected_position:.3}s"
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
        serde_json_crates_io::to_string_pretty(report)
            .context("failed to serialize HLS profile")?,
    )
    .context("failed to write HLS profile")?;
    Ok(output)
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
                {"at_ms": 2_000.0, "name": "waiting", "error": null}
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
}
