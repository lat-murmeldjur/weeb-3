use anyhow_crates_io::{Context, Result, anyhow};
use headless_chrome::protocol::cdp::Network::{ClearBrowserCache, SetCacheDisabled};
use headless_chrome::protocol::cdp::Page::AddScriptToEvaluateOnNewDocument;
use headless_chrome::{Browser, LaunchOptionsBuilder};
use serde::Serialize;
use serde_json_crates_io::{Value, json};
use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DEFAULT_SERVICE_WORKER_SCOPE: &str = "/weeb-3/";
const SERVICE_WORKER_PROTOCOL: u64 = 5;

const HLS_PROFILE_SCRIPT: &str = r#"
(() => {
    performance.setResourceTimingBufferSize?.(8192);
    const profile = window.__weeb3HlsProfile = {
        marks: { navigation: 0 }, events: [], samples: [], errors: [],
        first_presented_frame: null,
        cold_start: {
            observed_at_ms: performance.now(),
            controller_present_at_document_start:
                navigator.serviceWorker ? Boolean(navigator.serviceWorker.controller) : null,
            registrations: { status: 'pending', count: null, scripts: [] },
            cache_storage: { status: 'pending', count: null, names: [] },
            http_cache_cleared_before_navigation: true,
            http_cache_disabled_before_navigation: true
        }
    };
    const coldStartProbes = [];
    if (!navigator.serviceWorker?.getRegistrations) {
        profile.cold_start.registrations.status = 'unavailable';
    } else {
        try {
            const registrations = navigator.serviceWorker.getRegistrations();
            coldStartProbes.push(registrations.then(value => {
                profile.cold_start.registrations = {
                    status: 'resolved',
                    count: value.length,
                    scripts: value.map(registration =>
                        registration.active?.scriptURL ||
                        registration.waiting?.scriptURL ||
                        registration.installing?.scriptURL || null)
                };
            }, error => {
                profile.cold_start.registrations = {
                    status: 'rejected', count: null, scripts: [],
                    error: String(error?.message || error)
                };
            }));
        } catch (error) {
            profile.cold_start.registrations = {
                status: 'threw', count: null, scripts: [],
                error: String(error?.message || error)
            };
        }
    }
    if (!globalThis.caches?.keys) {
        profile.cold_start.cache_storage.status = 'unavailable';
    } else {
        try {
            const cacheNames = globalThis.caches.keys();
            coldStartProbes.push(cacheNames.then(value => {
                profile.cold_start.cache_storage = {
                    status: 'resolved', count: value.length, names: value
                };
            }, error => {
                profile.cold_start.cache_storage = {
                    status: 'rejected', count: null, names: [],
                    error: String(error?.message || error)
                };
            }));
        } catch (error) {
            profile.cold_start.cache_storage = {
                status: 'threw', count: null, names: [],
                error: String(error?.message || error)
            };
        }
    }
    window.__weeb3ColdStartReady = Promise.allSettled(coldStartProbes);
    const mark = (name, at = performance.now()) =>
        profile.marks[name] ??= at;
    const bufferAhead = media => {
        let forward = 0;
        for (let i = 0; media && i < media.buffered.length; i++) {
            const start = media.buffered.start(i), end = media.buffered.end(i);
            if (media.currentTime >= start - .05 && media.currentTime <= end + .05)
                forward = Math.max(0, end - media.currentTime);
        }
        return forward;
    };
    const snapshot = (event, at = performance.now(), exactMedia = null, extra = {}) => {
        const media = exactMedia || document.querySelector('video');
        const progress = (document.getElementById('progressRows')?.textContent || '')
            .split('\n').filter(Boolean);
        if (media) mark('attach', at);
        const forward = bufferAhead(media);
        const state = media?.getAttribute('data-weeb3-hls-state') ?? null;
        const timeline = media?.getAttribute('data-weeb3-hls-timeline') ?? null;
        if (state === 'manifest-ready') mark('manifest', at);
        if (event === 'playing') mark('first_playing', at);
        const row = {
            event,
            at_ms: at,
            current_time_s: media?.currentTime ?? null,
            duration_s: media?.duration ?? null,
            forward_buffer_s: forward,
            paused: media?.paused ?? null,
            ready_state: media?.readyState ?? null,
            state,
            timeline,
            connections: Number(document.getElementById('connections')?.textContent) || 0,
            ongoing_connections: Number(document.getElementById('ongoing')?.textContent) || 0,
            hls_body_running: progress.filter(line =>
                line.startsWith('hls-segment ') && line.includes('[running]')).length,
            hls_range_running: progress.filter(line =>
                line.startsWith('hls-segment-range ') && line.includes('[running]')).length,
            ...extra
        };
        (event === 'sample' ? profile.samples : profile.events).push(row);
        return row;
    };
    const frameCallbacksArmed = new WeakSet();
    const armFirstPresentedFrame = media => {
        if (!media || frameCallbacksArmed.has(media) ||
            typeof media.requestVideoFrameCallback !== 'function') return;
        frameCallbacksArmed.add(media);
        try {
            media.requestVideoFrameCallback((now, metadata) => {
                if (profile.first_presented_frame) return;
                mark('first_presented_frame', now);
                const row = snapshot('presented-frame', now, media, {
                    media_time_s: Number.isFinite(metadata?.mediaTime)
                        ? metadata.mediaTime : null,
                    presented_frames: Number.isFinite(metadata?.presentedFrames)
                        ? metadata.presentedFrames : null,
                    expected_display_time_ms: Number.isFinite(metadata?.expectedDisplayTime)
                        ? metadata.expectedDisplayTime : null
                });
                profile.first_presented_frame = { ...row };
            });
        } catch (error) {
            profile.errors.push({
                at_ms: performance.now(), type: 'requestVideoFrameCallback',
                message: String(error?.message || error)
            });
        }
    };
    const discoverMedia = () => {
        const media = document.querySelector('video');
        if (!media) return;
        mark('attach');
        armFirstPresentedFrame(media);
    };
    for (const name of [
        'playing', 'waiting', 'stalled', 'seeking', 'seeked', 'durationchange',
        'weeb3-hls-warmup-start', 'weeb3-hls-timeline-rebase'
    ]) {
        document.addEventListener(name, () => {
            discoverMedia();
            if (name === 'weeb3-hls-warmup-start') mark('manifest');
            snapshot(name);
        }, true);
    }
    new MutationObserver(discoverMedia)
        .observe(document, { childList: true, subtree: true });
    discoverMedia();
    const recordError = (type, value) => profile.errors.push({
        at_ms: performance.now(), type, message: String(value)
    });
    addEventListener('error', event =>
        recordError('error', event.error?.stack || event.message));
    addEventListener('unhandledrejection', event =>
        recordError('unhandledrejection', event.reason?.stack || event.reason));
    setInterval(() => snapshot('sample'), 500);
})();
"#;

const HLS_PROFILE_RESULT_SCRIPT: &str = r#"
(async () => {
    await window.__weeb3ColdStartReady;
    const ping = worker => new Promise(resolve => {
        if (!worker) return resolve(null);
        const channel = new MessageChannel();
        let settled = false;
        const finish = value => {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            channel.port1.close();
            resolve(value);
        };
        const timer = setTimeout(() => finish({ timeout: true }), 2000);
        channel.port1.onmessage = event => finish(event.data || null);
        channel.port1.start();
        try { worker.postMessage({ type: 'WEEB3_PING' }, [channel.port2]); }
        catch (error) { finish({ error: String(error?.message || error) }); }
    });
    const registration = await navigator.serviceWorker?.getRegistration();
    const controller = navigator.serviceWorker?.controller || null;
    const navigation = performance.getEntriesByType('navigation')[0] || null;
    const media = document.querySelector('video');
    const timing = resource => ({
        name: resource.name,
        initiator_type: resource.initiatorType,
        start_ms: resource.startTime,
        request_start_ms: resource.requestStart,
        response_start_ms: resource.responseStart,
        duration_ms: resource.duration,
        transfer_size: resource.transferSize,
        encoded_body_size: resource.encodedBodySize,
        decoded_body_size: resource.decodedBodySize
    });
    return JSON.stringify({
        href: location.href,
        measured_at_ms: performance.now(),
        profile: window.__weeb3HlsProfile || {
            marks: {}, events: [], samples: [], errors: []
        },
        media: media ? {
            current_time_s: media.currentTime,
            duration_s: media.duration,
            paused: media.paused,
            ready_state: media.readyState,
            network_state: media.networkState,
            state: media.getAttribute('data-weeb3-hls-state'),
            mode: media.getAttribute('data-weeb3-hls-mode'),
            timeline: media.getAttribute('data-weeb3-hls-timeline')
        } : null,
        diagnostics: {
            progress: document.getElementById('progressRows')?.textContent || null,
            result: document.getElementById('resultField')?.textContent || null,
            logs: Array.from(document.querySelectorAll('#logsField > *'))
                .slice(0, 100)
                .map(entry => entry.textContent || '')
        },
        service_worker: {
            controlled: Boolean(controller),
            controller_script: controller?.scriptURL || null,
            controller_state: controller?.state || null,
            scope: registration?.scope || null,
            active_script: registration?.active?.scriptURL || null,
            active_state: registration?.active?.state || null,
            protocol: await ping(controller)
        },
        navigation: navigation ? {
            response_status: typeof navigation.responseStatus === 'number'
                ? navigation.responseStatus : null,
            response_end_ms: navigation.responseEnd,
            dom_content_loaded_ms: navigation.domContentLoadedEventEnd,
            load_event_ms: navigation.loadEventEnd,
            duration_ms: navigation.duration,
            transfer_size: navigation.transferSize,
            encoded_body_size: navigation.encodedBodySize
        } : null,
        resources: performance.getEntriesByType('resource').slice(-8192).map(timing)
    });
})()
"#;

#[test]
fn weeb3_hls_profile() -> Result<()> {
    let Some(target_url) = env::var("WEEB3_HLS_PROFILE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        println!("WEEB3_HLS_PROFILE_URL is not set; skipping browser HLS profile");
        return Ok(());
    };
    let profile_seconds = env_u64("WEEB3_HLS_PROFILE_SECONDS", 90)?;
    if profile_seconds == 0 {
        return Err(anyhow!("WEEB3_HLS_PROFILE_SECONDS must be positive"));
    }

    let timeout = Duration::from_secs(profile_seconds.saturating_add(90));
    let browser = launch_fresh_edge(timeout)?;
    let tab = browser
        .new_tab()
        .map_err(|error| anyhow!("failed to open HLS profile tab: {error:?}"))?;
    tab.set_default_timeout(timeout);
    tab.call_method(ClearBrowserCache(None))
        .map_err(|error| anyhow!("failed to clear Edge's HTTP cache: {error:?}"))?;
    tab.call_method(SetCacheDisabled {
        cache_disabled: true,
    })
    .map_err(|error| anyhow!("failed to disable Edge's HTTP cache: {error:?}"))?;
    tab.call_method(AddScriptToEvaluateOnNewDocument {
        source: HLS_PROFILE_SCRIPT.to_string(),
        world_name: None,
        include_command_line_api: None,
        run_immediately: None,
    })
    .map_err(|error| anyhow!("failed to install HLS instrumentation: {error:?}"))?;

    let started = Instant::now();
    tab.navigate_to(&target_url)
        .map_err(|error| anyhow!("failed to navigate to HLS profile URL: {error:?}"))?
        .wait_until_navigated()
        .map_err(|error| anyhow!("HLS profile navigation did not finish: {error:?}"))?;
    let deadline = started + Duration::from_secs(profile_seconds);
    thread::sleep(deadline.saturating_duration_since(Instant::now()));

    let remote = tab
        .evaluate(HLS_PROFILE_RESULT_SCRIPT, true)
        .map_err(|error| anyhow!("failed to read HLS profile: {error:?}"))?;
    let raw = remote
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("Edge did not return HLS profile JSON"))?;
    let browser_metrics: Value =
        serde_json_crates_io::from_str(&raw).context("failed to parse HLS profile JSON")?;

    let first_playing_ms = browser_metrics
        .pointer("/profile/marks/first_playing")
        .and_then(Value::as_f64);
    let first_playing_current_time_s =
        hls_first_event_value(&browser_metrics, "playing", "current_time_s");
    let samples = hls_metric_samples(&browser_metrics);
    let buffer = summarize_hls_buffer(
        &samples,
        first_playing_ms,
        env_u64("WEEB3_HLS_TREND_WINDOW_SECONDS", 15)? as f64 * 1000.0,
    );
    let playback_checkpoints =
        summarize_hls_playback_checkpoints(&browser_metrics, first_playing_ms);
    let waiting_episodes = summarize_hls_waiting_episodes(&browser_metrics, first_playing_ms);
    let duration_changes = summarize_hls_duration_changes(&browser_metrics, first_playing_ms);
    let post_start_waiting = hls_event_count(&browser_metrics, "waiting", first_playing_ms);
    let post_start_stalled = hls_event_count(&browser_metrics, "stalled", first_playing_ms);
    let post_start_seeking = hls_event_count(&browser_metrics, "seeking", first_playing_ms);
    let post_start_seeked = hls_event_count(&browser_metrics, "seeked", first_playing_ms);
    let timeline_rebases = hls_event_count(
        &browser_metrics,
        "weeb3-hls-timeline-rebase",
        first_playing_ms,
    );
    let summary = json!({
        "navigation_ms": 0.0,
        "attach_ms": browser_metrics.pointer("/profile/marks/attach").and_then(Value::as_f64),
        "manifest_ms": browser_metrics.pointer("/profile/marks/manifest").and_then(Value::as_f64),
        "first_playing_ms": first_playing_ms,
        "first_playing_current_time_s": first_playing_current_time_s,
        "first_presented_frame_ms": browser_metrics.pointer("/profile/marks/first_presented_frame").and_then(Value::as_f64),
        "playback_checkpoints": playback_checkpoints,
        "post_start_duration": duration_changes,
        "timeline": browser_metrics.pointer("/media/timeline").and_then(Value::as_str),
        "post_start_waiting_count": post_start_waiting,
        "post_start_waiting_episodes": waiting_episodes,
        "post_start_stalled_count": post_start_stalled,
        "post_start_seeking_count": post_start_seeking,
        "post_start_seeked_count": post_start_seeked,
        "timeline_rebase_count": timeline_rebases,
        "timeline_jump_count": buffer.timeline_jump_count,
        "first_minute": buffer.first_minute,
        "final": buffer.final_sample,
        "early_low_water_s": buffer.early_low_water_s,
        "late_low_water_s": buffer.late_low_water_s,
        "low_water_trend_s": buffer.low_water_trend_s,
        "matched_trend_window_ms": buffer.matched_window_ms
    });
    let report = json!({
        "target_url": target_url,
        "profile_seconds": profile_seconds,
        "summary": summary,
        "browser": browser_metrics
    });

    fs::create_dir_all("target/weeb3-hls-profile")
        .context("failed to create target/weeb3-hls-profile directory")?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let output = format!("target/weeb3-hls-profile/hls-profile-{timestamp}.json");
    fs::write(
        &output,
        serde_json_crates_io::to_string_pretty(&report)
            .context("failed to serialize HLS profile")?,
    )
    .context("failed to write HLS profile")?;
    println!("HLS profile written to {output}");
    println!("HLS profile summary: {summary}");

    let expected_scope = env::var("WEEB3_HLS_EXPECT_SW_SCOPE")
        .ok()
        .filter(|scope| !scope.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE_WORKER_SCOPE.to_string());
    validate_cold_browser_start(&report["browser"])?;
    validate_service_worker(&report["browser"], &expected_scope)?;

    let first_playing_ms =
        first_playing_ms.ok_or_else(|| anyhow!("HLS playback never emitted playing"))?;
    if let Some(limit) = env_optional_u64("WEEB3_HLS_MAX_STARTUP_MS")? {
        if first_playing_ms > limit as f64 {
            return Err(anyhow!(
                "HLS startup {first_playing_ms:.0}ms exceeded {limit}ms"
            ));
        }
    }
    if env_bool("WEEB3_HLS_REQUIRE_NO_STALLS", false)
        && post_start_waiting + post_start_stalled != 0
    {
        return Err(anyhow!(
            "HLS playback waited {post_start_waiting} time(s) and stalled {post_start_stalled} time(s)"
        ));
    }
    if env_bool("WEEB3_HLS_REQUIRE_STABLE_TIMELINE", false) {
        if post_start_seeking != 0 || post_start_seeked != 0 {
            return Err(anyhow!(
                "HLS playback sought {post_start_seeking} time(s) and completed {post_start_seeked} seek(s) after starting"
            ));
        }
        if timeline_rebases != 0 {
            return Err(anyhow!(
                "HLS playback rebased its timeline {timeline_rebases} time(s)"
            ));
        }
        if buffer.timeline_jump_count != 0 {
            return Err(anyhow!(
                "HLS currentTime jumped {} time(s)",
                buffer.timeline_jump_count
            ));
        }
    }
    if env_bool("WEEB3_HLS_REQUIRE_ABSOLUTE_TIMELINE", false)
        && browser_metrics
            .pointer("/media/timeline")
            .and_then(Value::as_str)
            != Some("absolute")
    {
        return Err(anyhow!(
            "HLS playback did not establish an absolute media timeline"
        ));
    }
    if let Some(expected) = env_optional_f64("WEEB3_HLS_EXPECT_FIRST_TIME_SECONDS")? {
        let tolerance = env_optional_f64("WEEB3_HLS_FIRST_TIME_TOLERANCE_SECONDS")?.unwrap_or(1.0);
        if tolerance < 0.0 {
            return Err(anyhow!(
                "WEEB3_HLS_FIRST_TIME_TOLERANCE_SECONDS must be non-negative"
            ));
        }
        let actual = first_playing_current_time_s
            .ok_or_else(|| anyhow!("HLS playback never reported its first playing time"))?;
        if (actual - expected).abs() > tolerance {
            return Err(anyhow!(
                "HLS first playing time {actual:.6}s differed from expected {expected:.6}s by more than {tolerance:.6}s"
            ));
        }
    }
    if let Some(limit) = env_optional_f64("WEEB3_HLS_MAX_DURATION_JUMP_SECONDS")? {
        validate_hls_duration_jump(&duration_changes, limit)?;
    }
    if let Some(minimum) = env_optional_f64("WEEB3_HLS_MIN_LOW_WATER_TREND_SECONDS")? {
        let trend = buffer.low_water_trend_s.ok_or_else(|| {
            anyhow!("HLS profile is too short to calculate matched post-minute windows")
        })?;
        if trend < minimum {
            return Err(anyhow!(
                "HLS low-water trend {trend:.3}s was below {minimum:.3}s"
            ));
        }
    }

    Ok(())
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

    // With no explicit user_data_dir, headless_chrome creates and later removes
    // a new temporary Edge profile for every Browser instance.
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
            anyhow!("Microsoft Edge was not found; set WEEB3_CHROME to the msedge executable")
        })
}

fn validate_service_worker(metrics: &Value, expected_scope: &str) -> Result<()> {
    let service_worker = metrics
        .get("service_worker")
        .ok_or_else(|| anyhow!("HLS profile did not report Service Worker state"))?;
    if service_worker.get("controlled").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!(
            "HLS playback did not finish under a controlling Service Worker"
        ));
    }
    let pong = service_worker
        .get("protocol")
        .ok_or_else(|| anyhow!("controlling Service Worker did not answer WEEB3_PING"))?;
    if pong.get("type").and_then(Value::as_str) != Some("WEEB3_PONG") {
        return Err(anyhow!(
            "controlling Service Worker returned an invalid WEEB3_PING response: {pong}"
        ));
    }
    if pong.get("protocol").and_then(Value::as_u64) != Some(SERVICE_WORKER_PROTOCOL) {
        return Err(anyhow!(
            "controlling Service Worker did not answer protocol {SERVICE_WORKER_PROTOCOL}: {pong}"
        ));
    }
    if pong.get("scope").and_then(Value::as_str) != Some(expected_scope) {
        return Err(anyhow!(
            "controlling Service Worker answered for the wrong scope; expected {expected_scope}: {pong}"
        ));
    }
    Ok(())
}

fn validate_cold_browser_start(metrics: &Value) -> Result<()> {
    let cold_start = metrics
        .pointer("/profile/cold_start")
        .ok_or_else(|| anyhow!("HLS profile did not report its document-start browser state"))?;
    if cold_start
        .get("controller_present_at_document_start")
        .and_then(Value::as_bool)
        != Some(false)
    {
        return Err(anyhow!(
            "HLS profile did not start without a Service Worker controller: {cold_start}"
        ));
    }
    let registrations = cold_start
        .get("registrations")
        .ok_or_else(|| anyhow!("HLS profile did not probe initial Service Worker registrations"))?;
    if registrations.get("status").and_then(Value::as_str) != Some("resolved")
        || registrations.get("count").and_then(Value::as_u64) != Some(0)
    {
        return Err(anyhow!(
            "HLS profile did not start with an empty Service Worker registration set: {registrations}"
        ));
    }
    let cache_storage = cold_start
        .get("cache_storage")
        .ok_or_else(|| anyhow!("HLS profile did not probe initial Cache Storage state"))?;
    if cache_storage.get("status").and_then(Value::as_str) != Some("resolved")
        || cache_storage.get("count").and_then(Value::as_u64) != Some(0)
    {
        return Err(anyhow!(
            "HLS profile did not start with empty Cache Storage: {cache_storage}"
        ));
    }
    for flag in [
        "http_cache_cleared_before_navigation",
        "http_cache_disabled_before_navigation",
    ] {
        if cold_start.get(flag).and_then(Value::as_bool) != Some(true) {
            return Err(anyhow!("HLS profile did not prove {flag}"));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
struct HlsMetricSample {
    at_ms: f64,
    current_time_s: f64,
    duration_s: Option<f64>,
    forward_buffer_s: f64,
    paused: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
struct HlsBufferSummary {
    first_minute: Option<HlsMetricSample>,
    final_sample: Option<HlsMetricSample>,
    early_low_water_s: Option<f64>,
    late_low_water_s: Option<f64>,
    low_water_trend_s: Option<f64>,
    matched_window_ms: Option<f64>,
    timeline_jump_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct HlsPlaybackCheckpoint {
    at_ms: f64,
    current_time_s: Option<f64>,
    media_time_s: Option<f64>,
    duration_s: Option<f64>,
    forward_buffer_s: Option<f64>,
    timeline: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct HlsPlaybackCheckpoints {
    first_playing: Option<HlsPlaybackCheckpoint>,
    first_presented_frame: Option<HlsPlaybackCheckpoint>,
    after_10_seconds: Option<HlsPlaybackCheckpoint>,
    after_30_seconds: Option<HlsPlaybackCheckpoint>,
    after_60_seconds: Option<HlsPlaybackCheckpoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct HlsWaitingEpisode {
    waiting_at_ms: f64,
    playing_at_ms: Option<f64>,
    observed_duration_ms: f64,
    waiting_timeline: Option<String>,
    playing_timeline: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct HlsWaitingSummary {
    episodes: Vec<HlsWaitingEpisode>,
    completed_count: usize,
    open_count: usize,
    total_observed_ms: f64,
    max_observed_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct HlsDurationSummary {
    anchor: Option<String>,
    anchor_at_ms: Option<f64>,
    initial_duration_s: Option<f64>,
    final_duration_s: Option<f64>,
    max_duration_s: Option<f64>,
    largest_positive_jump_s: Option<f64>,
    largest_negative_jump_s: Option<f64>,
    durationchange_event_count: usize,
    observation_count: usize,
    observed_change_count: usize,
}

fn hls_metric_samples(metrics: &Value) -> Vec<HlsMetricSample> {
    metrics
        .pointer("/profile/samples")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|sample| {
            Some(HlsMetricSample {
                at_ms: sample.get("at_ms")?.as_f64()?,
                current_time_s: sample.get("current_time_s")?.as_f64()?,
                duration_s: sample.get("duration_s").and_then(Value::as_f64),
                forward_buffer_s: sample.get("forward_buffer_s")?.as_f64()?,
                paused: sample.get("paused")?.as_bool()?,
            })
        })
        .collect()
}

fn hls_playback_checkpoint(entry: &Value) -> Option<HlsPlaybackCheckpoint> {
    Some(HlsPlaybackCheckpoint {
        at_ms: entry.get("at_ms")?.as_f64()?,
        current_time_s: entry.get("current_time_s").and_then(Value::as_f64),
        media_time_s: entry.get("media_time_s").and_then(Value::as_f64),
        duration_s: entry.get("duration_s").and_then(Value::as_f64),
        forward_buffer_s: entry.get("forward_buffer_s").and_then(Value::as_f64),
        timeline: entry
            .get("timeline")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    })
}

fn hls_first_event_checkpoint(metrics: &Value, event: &str) -> Option<HlsPlaybackCheckpoint> {
    metrics
        .pointer("/profile/events")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("event").and_then(Value::as_str) == Some(event))
        .and_then(hls_playback_checkpoint)
}

fn hls_nearest_sample_checkpoint(
    metrics: &Value,
    first_playing_ms: f64,
    offset_ms: f64,
) -> Option<HlsPlaybackCheckpoint> {
    let samples = metrics.pointer("/profile/samples")?.as_array()?;
    let target_ms = first_playing_ms + offset_ms;
    let last_sample_ms = samples
        .iter()
        .filter_map(|sample| sample.get("at_ms").and_then(Value::as_f64))
        .max_by(f64::total_cmp)?;
    if target_ms > last_sample_ms {
        return None;
    }
    samples
        .iter()
        .filter(|sample| {
            sample
                .get("at_ms")
                .and_then(Value::as_f64)
                .is_some_and(|at_ms| at_ms >= first_playing_ms)
        })
        .min_by(|left, right| {
            let left_distance = left
                .get("at_ms")
                .and_then(Value::as_f64)
                .map(|at_ms| (at_ms - target_ms).abs())
                .unwrap_or(f64::INFINITY);
            let right_distance = right
                .get("at_ms")
                .and_then(Value::as_f64)
                .map(|at_ms| (at_ms - target_ms).abs())
                .unwrap_or(f64::INFINITY);
            left_distance.total_cmp(&right_distance)
        })
        .and_then(hls_playback_checkpoint)
}

fn summarize_hls_playback_checkpoints(
    metrics: &Value,
    first_playing_ms: Option<f64>,
) -> HlsPlaybackCheckpoints {
    let first_presented_frame = metrics
        .pointer("/profile/first_presented_frame")
        .and_then(hls_playback_checkpoint);
    let Some(first_playing_ms) = first_playing_ms.filter(|value| value.is_finite()) else {
        return HlsPlaybackCheckpoints {
            first_presented_frame,
            ..HlsPlaybackCheckpoints::default()
        };
    };
    HlsPlaybackCheckpoints {
        first_playing: hls_first_event_checkpoint(metrics, "playing"),
        first_presented_frame,
        after_10_seconds: hls_nearest_sample_checkpoint(metrics, first_playing_ms, 10_000.0),
        after_30_seconds: hls_nearest_sample_checkpoint(metrics, first_playing_ms, 30_000.0),
        after_60_seconds: hls_nearest_sample_checkpoint(metrics, first_playing_ms, 60_000.0),
    }
}

fn summarize_hls_waiting_episodes(
    metrics: &Value,
    first_playing_ms: Option<f64>,
) -> HlsWaitingSummary {
    let Some(first_playing_ms) = first_playing_ms.filter(|value| value.is_finite()) else {
        return HlsWaitingSummary::default();
    };
    let measured_at_ms = metrics
        .get("measured_at_ms")
        .and_then(Value::as_f64)
        .unwrap_or(first_playing_ms)
        .max(first_playing_ms);
    let mut summary = HlsWaitingSummary::default();
    let mut open: Option<(f64, Option<String>)> = None;
    for event in metrics
        .pointer("/profile/events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(at_ms) = event.get("at_ms").and_then(Value::as_f64) else {
            continue;
        };
        if at_ms < first_playing_ms {
            continue;
        }
        let timeline = event
            .get("timeline")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        match event.get("event").and_then(Value::as_str) {
            Some("waiting") if open.is_none() => open = Some((at_ms, timeline)),
            Some("playing") => {
                if let Some((waiting_at_ms, waiting_timeline)) = open.take() {
                    let observed_duration_ms = (at_ms - waiting_at_ms).max(0.0);
                    summary.episodes.push(HlsWaitingEpisode {
                        waiting_at_ms,
                        playing_at_ms: Some(at_ms),
                        observed_duration_ms,
                        waiting_timeline,
                        playing_timeline: timeline,
                    });
                    summary.completed_count += 1;
                }
            }
            _ => {}
        }
    }
    if let Some((waiting_at_ms, waiting_timeline)) = open {
        summary.episodes.push(HlsWaitingEpisode {
            waiting_at_ms,
            playing_at_ms: None,
            observed_duration_ms: (measured_at_ms - waiting_at_ms).max(0.0),
            waiting_timeline,
            playing_timeline: None,
        });
        summary.open_count = 1;
    }
    summary.total_observed_ms = summary
        .episodes
        .iter()
        .map(|episode| episode.observed_duration_ms)
        .sum();
    summary.max_observed_ms = summary
        .episodes
        .iter()
        .map(|episode| episode.observed_duration_ms)
        .max_by(f64::total_cmp);
    summary
}

fn summarize_hls_duration_changes(
    metrics: &Value,
    first_playing_ms: Option<f64>,
) -> HlsDurationSummary {
    let first_presented_frame = metrics
        .pointer("/profile/first_presented_frame")
        .and_then(hls_playback_checkpoint)
        .filter(|checkpoint| checkpoint.at_ms.is_finite());
    let first_playing = hls_first_event_checkpoint(metrics, "playing")
        .filter(|checkpoint| checkpoint.at_ms.is_finite());
    let fallback_playing = first_playing_ms
        .filter(|value| value.is_finite())
        .map(|at_ms| ("first_playing", at_ms, None));
    let frame_anchor = first_presented_frame.as_ref().map(|checkpoint| {
        (
            "first_presented_frame",
            checkpoint.at_ms,
            checkpoint.duration_s,
        )
    });
    let playing_anchor = first_playing
        .as_ref()
        .map(|checkpoint| ("first_playing", checkpoint.at_ms, checkpoint.duration_s))
        .or(fallback_playing);
    let anchor = match (frame_anchor, playing_anchor) {
        (Some(frame), Some(playing)) if frame.1 <= playing.1 => Some(frame),
        (Some(_), Some(playing)) => Some(playing),
        (Some(frame), None) => Some(frame),
        (None, Some(playing)) => Some(playing),
        (None, None) => None,
    };
    let Some((anchor_name, anchor_at_ms, anchor_duration_s)) = anchor else {
        return HlsDurationSummary::default();
    };

    let mut observations = Vec::new();
    observations.push((anchor_at_ms, anchor_duration_s.unwrap_or(f64::NAN)));
    for entry in metrics
        .pointer("/profile/events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            metrics
                .pointer("/profile/samples")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
    {
        let Some(at_ms) = entry.get("at_ms").and_then(Value::as_f64) else {
            continue;
        };
        if at_ms < anchor_at_ms {
            continue;
        }
        let duration_s = entry
            .get("duration_s")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        observations.push((at_ms, duration_s));
    }
    if let (Some(at_ms), Some(duration_s)) = (
        metrics.get("measured_at_ms").and_then(Value::as_f64),
        metrics.pointer("/media/duration_s").and_then(Value::as_f64),
    ) {
        if at_ms >= anchor_at_ms {
            observations.push((at_ms, duration_s));
        }
    }

    summarize_hls_duration_observations(
        anchor_name,
        anchor_at_ms,
        hls_event_count(metrics, "durationchange", Some(anchor_at_ms)),
        observations,
    )
}

fn summarize_hls_duration_observations(
    anchor: &str,
    anchor_at_ms: f64,
    durationchange_event_count: usize,
    observations: impl IntoIterator<Item = (f64, f64)>,
) -> HlsDurationSummary {
    let mut observations = observations
        .into_iter()
        .filter(|(at_ms, duration_s)| {
            at_ms.is_finite() && duration_s.is_finite() && *duration_s >= 0.0
        })
        .collect::<Vec<_>>();
    observations.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut summary = HlsDurationSummary {
        anchor: Some(anchor.to_string()),
        anchor_at_ms: Some(anchor_at_ms),
        durationchange_event_count,
        observation_count: observations.len(),
        ..HlsDurationSummary::default()
    };
    let Some((_, initial_duration_s)) = observations.first().copied() else {
        return summary;
    };
    summary.initial_duration_s = Some(initial_duration_s);
    summary.final_duration_s = observations.last().map(|(_, duration_s)| *duration_s);
    summary.max_duration_s = observations
        .iter()
        .map(|(_, duration_s)| *duration_s)
        .max_by(f64::total_cmp);
    for pair in observations.windows(2) {
        let jump_s = pair[1].1 - pair[0].1;
        if jump_s > 0.0 {
            summary.largest_positive_jump_s = Some(
                summary
                    .largest_positive_jump_s
                    .map_or(jump_s, |largest| largest.max(jump_s)),
            );
            summary.observed_change_count += 1;
        } else if jump_s < 0.0 {
            summary.largest_negative_jump_s = Some(
                summary
                    .largest_negative_jump_s
                    .map_or(jump_s, |largest| largest.min(jump_s)),
            );
            summary.observed_change_count += 1;
        }
    }
    summary
}

fn validate_hls_duration_jump(summary: &HlsDurationSummary, limit_s: f64) -> Result<()> {
    if !limit_s.is_finite() || limit_s < 0.0 {
        return Err(anyhow!(
            "WEEB3_HLS_MAX_DURATION_JUMP_SECONDS must be finite and non-negative"
        ));
    }
    if summary.initial_duration_s.is_none() {
        return Err(anyhow!(
            "HLS profile did not report a finite media duration after playback started"
        ));
    }
    let largest_jump_s = summary.largest_positive_jump_s.unwrap_or(0.0).max(
        summary
            .largest_negative_jump_s
            .map(|jump| jump.abs())
            .unwrap_or(0.0),
    );
    if largest_jump_s > limit_s {
        return Err(anyhow!(
            "HLS duration jumped by as much as {largest_jump_s:.6}s, exceeding {limit_s:.6}s"
        ));
    }
    Ok(())
}

fn hls_event_count(metrics: &Value, event: &str, after_ms: Option<f64>) -> usize {
    metrics
        .pointer("/profile/events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("event").and_then(Value::as_str) == Some(event))
        .filter(|entry| {
            after_ms.is_none_or(|after| {
                entry
                    .get("at_ms")
                    .and_then(Value::as_f64)
                    .is_some_and(|at| at >= after)
            })
        })
        .count()
}

fn hls_first_event_value(metrics: &Value, event: &str, field: &str) -> Option<f64> {
    metrics
        .pointer("/profile/events")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("event").and_then(Value::as_str) == Some(event))?
        .get(field)?
        .as_f64()
}

fn summarize_hls_buffer(
    samples: &[HlsMetricSample],
    first_playing_ms: Option<f64>,
    requested_window_ms: f64,
) -> HlsBufferSummary {
    let Some(first_playing_ms) = first_playing_ms.filter(|value| value.is_finite()) else {
        return HlsBufferSummary::default();
    };
    let active = &samples[samples.partition_point(|sample| sample.at_ms < first_playing_ms)..];
    let Some(final_sample) = active.last().copied() else {
        return HlsBufferSummary::default();
    };
    let first_minute_at = first_playing_ms + 60_000.0;
    let first_minute = active
        .iter()
        .find(|sample| sample.at_ms >= first_minute_at)
        .copied();
    let available_ms = (final_sample.at_ms - first_minute_at).max(0.0);
    let window_ms = requested_window_ms.max(0.0).min(available_ms / 2.0);
    let low_water = |start: f64, end: f64| {
        active
            .iter()
            .filter(|sample| sample.at_ms >= start && sample.at_ms <= end)
            .map(|sample| sample.forward_buffer_s)
            .min_by(f64::total_cmp)
    };
    let (early, late, trend, matched_window_ms) = if window_ms >= 500.0 {
        let early = low_water(first_minute_at, first_minute_at + window_ms);
        let late = low_water(final_sample.at_ms - window_ms, final_sample.at_ms);
        (
            early,
            late,
            early.zip(late).map(|(early, late)| late - early),
            Some(window_ms),
        )
    } else {
        (None, None, None, None)
    };
    let timeline_jump_count = active
        .windows(2)
        .filter(|pair| {
            if pair[0].paused || pair[1].paused {
                return false;
            }
            let elapsed_s = (pair[1].at_ms - pair[0].at_ms) / 1000.0;
            let media_s = pair[1].current_time_s - pair[0].current_time_s;
            elapsed_s > 0.0 && (media_s < -0.25 || media_s > elapsed_s + 1.0)
        })
        .count();

    HlsBufferSummary {
        first_minute,
        final_sample: Some(final_sample),
        early_low_water_s: early,
        late_low_water_s: late,
        low_water_trend_s: trend,
        matched_window_ms,
        timeline_jump_count,
    }
}

fn contiguous_forward_buffer(current_time_s: f64, ranges: &[(f64, f64)]) -> f64 {
    ranges
        .iter()
        .find(|(start, end)| current_time_s >= start - 0.05 && current_time_s <= end + 0.05)
        .map(|(_, end)| (end - current_time_s).max(0.0))
        .unwrap_or(0.0)
}

fn env_u64(name: &str, default: u64) -> Result<u64> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value
            .parse::<u64>()
            .with_context(|| format!("{name} must be an unsigned integer")),
        _ => Ok(default),
    }
}

fn env_optional_u64(name: &str) -> Result<Option<u64>> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            let parsed = value
                .parse::<u64>()
                .with_context(|| format!("{name} must be an unsigned integer"))?;
            Ok(Some(parsed))
        }
        _ => Ok(None),
    }
}

fn env_optional_f64(name: &str) -> Result<Option<f64>> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            let parsed = value
                .parse::<f64>()
                .with_context(|| format!("{name} must be a number"))?;
            if !parsed.is_finite() {
                return Err(anyhow!("{name} must be finite"));
            }
            Ok(Some(parsed))
        }
        _ => Ok(None),
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "y" | "on"
        ),
        Err(_) => default,
    }
}

#[cfg(test)]
mod metric_tests {
    use super::*;

    fn sample(at_ms: f64, current_time_s: f64, forward_buffer_s: f64) -> HlsMetricSample {
        HlsMetricSample {
            at_ms,
            current_time_s,
            duration_s: Some(600.0),
            forward_buffer_s,
            paused: false,
        }
    }

    #[test]
    fn contiguous_buffer_uses_only_the_range_containing_playback() {
        let ranges = [(0.0, 4.0), (6.0, 10.0)];
        assert_eq!(contiguous_forward_buffer(2.5, &ranges), 1.5);
        assert_eq!(contiguous_forward_buffer(5.0, &ranges), 0.0);
        assert!((contiguous_forward_buffer(6.02, &ranges) - 3.98).abs() < f64::EPSILON * 4.0);
    }

    #[test]
    fn equal_post_minute_windows_report_the_matched_low_water_trend() {
        let samples = [
            sample(60_500.0, 59.5, 4.0),
            sample(61_000.0, 60.0, 5.0),
            sample(70_000.0, 69.0, 6.0),
            sample(80_000.0, 79.0, 7.0),
            sample(91_000.0, 90.0, 8.0),
        ];
        let summary = summarize_hls_buffer(&samples, Some(1_000.0), 15_000.0);
        assert_eq!(summary.first_minute, Some(samples[1]));
        assert_eq!(summary.early_low_water_s, Some(5.0));
        assert_eq!(summary.late_low_water_s, Some(7.0));
        assert_eq!(summary.low_water_trend_s, Some(2.0));
        assert_eq!(summary.matched_window_ms, Some(15_000.0));
    }

    #[test]
    fn timeline_metric_rejects_only_discontinuous_media_clock_changes() {
        let stable = [sample(1_000.0, 0.0, 5.0), sample(1_500.0, 0.5, 5.0)];
        assert_eq!(
            summarize_hls_buffer(&stable, Some(1_000.0), 15_000.0).timeline_jump_count,
            0
        );
        let jumped = [stable[0], sample(1_500.0, 5.0, 5.0)];
        assert_eq!(
            summarize_hls_buffer(&jumped, Some(1_000.0), 15_000.0).timeline_jump_count,
            1
        );
    }

    #[test]
    fn first_playing_metric_preserves_the_initial_media_clock() {
        let metrics = json!({
            "profile": {
                "events": [
                    { "event": "waiting", "current_time_s": 0.0 },
                    { "event": "playing", "current_time_s": 870.140005 },
                    { "event": "playing", "current_time_s": 871.0 }
                ]
            }
        });
        assert_eq!(
            hls_first_event_value(&metrics, "playing", "current_time_s"),
            Some(870.140005)
        );
    }

    #[test]
    fn playback_checkpoints_preserve_first_frame_timeline_and_nearest_runway_samples() {
        let metrics = json!({
            "profile": {
                "events": [{
                    "event": "playing", "at_ms": 1_000.0,
                    "current_time_s": 870.14, "duration_s": 900.0,
                    "forward_buffer_s": 3.5, "timeline": "absolute"
                }],
                "first_presented_frame": {
                    "event": "presented-frame", "at_ms": 1_120.0,
                    "current_time_s": 870.22, "media_time_s": 870.20,
                    "duration_s": 900.0, "forward_buffer_s": 3.42,
                    "timeline": "absolute"
                },
                "samples": [
                    { "at_ms": 10_800.0, "current_time_s": 880.0,
                      "duration_s": 900.0, "forward_buffer_s": 8.0,
                      "timeline": "absolute" },
                    { "at_ms": 31_200.0, "current_time_s": 900.0,
                      "duration_s": 900.0, "forward_buffer_s": 16.0,
                      "timeline": "absolute" },
                    { "at_ms": 61_100.0, "current_time_s": 930.0,
                      "duration_s": 930.0, "forward_buffer_s": 24.0,
                      "timeline": "absolute" }
                ]
            }
        });
        let summary = summarize_hls_playback_checkpoints(&metrics, Some(1_000.0));
        assert_eq!(
            summary
                .first_playing
                .as_ref()
                .and_then(|checkpoint| checkpoint.forward_buffer_s),
            Some(3.5)
        );
        assert_eq!(
            summary
                .first_presented_frame
                .as_ref()
                .and_then(|checkpoint| checkpoint.media_time_s),
            Some(870.20)
        );
        assert_eq!(
            summary
                .first_presented_frame
                .as_ref()
                .and_then(|checkpoint| checkpoint.timeline.as_deref()),
            Some("absolute")
        );
        assert_eq!(
            summary.after_10_seconds.unwrap().forward_buffer_s,
            Some(8.0)
        );
        assert_eq!(
            summary.after_30_seconds.unwrap().forward_buffer_s,
            Some(16.0)
        );
        assert_eq!(
            summary.after_60_seconds.unwrap().forward_buffer_s,
            Some(24.0)
        );
    }

    #[test]
    fn duration_summary_exposes_large_post_frame_live_window_expansion() {
        let metrics = json!({
            "measured_at_ms": 4_000.0,
            "profile": {
                "first_presented_frame": {
                    "event": "presented-frame", "at_ms": 1_000.0,
                    "duration_s": 48.0
                },
                "events": [
                    { "event": "playing", "at_ms": 1_100.0, "duration_s": 48.0 },
                    { "event": "durationchange", "at_ms": 2_000.0,
                      "duration_s": 2_552.0 },
                    { "event": "durationchange", "at_ms": 3_000.0,
                      "duration_s": 2_560.0 }
                ],
                "samples": [
                    { "at_ms": 1_500.0, "duration_s": 48.0 },
                    { "at_ms": 2_500.0, "duration_s": 2_552.0 }
                ]
            },
            "media": { "duration_s": 2_560.0 }
        });
        let summary = summarize_hls_duration_changes(&metrics, Some(1_100.0));
        assert_eq!(summary.anchor.as_deref(), Some("first_presented_frame"));
        assert_eq!(summary.anchor_at_ms, Some(1_000.0));
        assert_eq!(summary.initial_duration_s, Some(48.0));
        assert_eq!(summary.final_duration_s, Some(2_560.0));
        assert_eq!(summary.max_duration_s, Some(2_560.0));
        assert_eq!(summary.largest_positive_jump_s, Some(2_504.0));
        assert_eq!(summary.largest_negative_jump_s, None);
        assert_eq!(summary.durationchange_event_count, 2);
        validate_hls_duration_jump(&summary, 2_504.0).unwrap();
        assert!(validate_hls_duration_jump(&summary, 2_503.0).is_err());
    }

    #[test]
    fn duration_summary_ignores_non_finite_values_and_reports_rolling_changes() {
        let summary = summarize_hls_duration_observations(
            "first_playing",
            1_000.0,
            0,
            [
                (1_000.0, f64::NAN),
                (1_100.0, f64::INFINITY),
                (1_200.0, 48.0),
                (1_300.0, 56.0),
                (1_400.0, 64.0),
                (1_500.0, 60.0),
                (1_600.0, -1.0),
            ],
        );
        assert_eq!(summary.initial_duration_s, Some(48.0));
        assert_eq!(summary.final_duration_s, Some(60.0));
        assert_eq!(summary.max_duration_s, Some(64.0));
        assert_eq!(summary.largest_positive_jump_s, Some(8.0));
        assert_eq!(summary.largest_negative_jump_s, Some(-4.0));
        assert_eq!(summary.observation_count, 4);
        assert_eq!(summary.observed_change_count, 3);
        validate_hls_duration_jump(&summary, 8.0).unwrap();
        assert!(validate_hls_duration_jump(&summary, 7.999).is_err());
    }

    #[test]
    fn duration_summary_keeps_null_duration_events_counted_but_not_numeric() {
        let metrics = json!({
            "measured_at_ms": 2_000.0,
            "profile": {
                "events": [
                    { "event": "playing", "at_ms": 1_000.0, "duration_s": null },
                    { "event": "durationchange", "at_ms": 1_500.0,
                      "duration_s": null }
                ],
                "samples": [
                    { "at_ms": 1_750.0, "duration_s": "NaN" }
                ]
            },
            "media": { "duration_s": null }
        });
        let summary = summarize_hls_duration_changes(&metrics, Some(1_000.0));
        assert_eq!(summary.anchor.as_deref(), Some("first_playing"));
        assert_eq!(summary.initial_duration_s, None);
        assert_eq!(summary.final_duration_s, None);
        assert_eq!(summary.max_duration_s, None);
        assert_eq!(summary.durationchange_event_count, 1);
        assert_eq!(summary.observation_count, 0);
    }

    #[test]
    fn waiting_summary_pairs_post_start_episodes_and_accounts_for_an_open_wait() {
        let metrics = json!({
            "measured_at_ms": 4_500.0,
            "profile": {
                "events": [
                    { "event": "waiting", "at_ms": 500.0, "timeline": "relative" },
                    { "event": "playing", "at_ms": 1_000.0, "timeline": "absolute" },
                    { "event": "waiting", "at_ms": 2_000.0, "timeline": "absolute" },
                    { "event": "waiting", "at_ms": 2_100.0, "timeline": "relative" },
                    { "event": "playing", "at_ms": 2_600.0, "timeline": "absolute" },
                    { "event": "waiting", "at_ms": 4_000.0, "timeline": "relative" }
                ]
            }
        });
        let summary = summarize_hls_waiting_episodes(&metrics, Some(1_000.0));
        assert_eq!(summary.completed_count, 1);
        assert_eq!(summary.open_count, 1);
        assert_eq!(summary.total_observed_ms, 1_100.0);
        assert_eq!(summary.max_observed_ms, Some(600.0));
        assert_eq!(summary.episodes.len(), 2);
        assert_eq!(summary.episodes[0].waiting_at_ms, 2_000.0);
        assert_eq!(summary.episodes[0].playing_at_ms, Some(2_600.0));
        assert_eq!(
            summary.episodes[0].waiting_timeline.as_deref(),
            Some("absolute")
        );
        assert_eq!(summary.episodes[1].playing_at_ms, None);
        assert_eq!(summary.episodes[1].observed_duration_ms, 500.0);
    }

    #[test]
    fn service_worker_gate_requires_controller_protocol_and_scope() {
        let valid = json!({
            "service_worker": {
                "controlled": true,
                "protocol": {
                    "type": "WEEB3_PONG",
                    "protocol": 5,
                    "scope": "/weeb-3/"
                }
            }
        });
        validate_service_worker(&valid, "/weeb-3/").unwrap();

        let no_controller = json!({
            "service_worker": {
                "controlled": false,
                "protocol": null
            }
        });
        assert!(validate_service_worker(&no_controller, "/weeb-3/").is_err());

        let old_protocol = json!({
            "service_worker": {
                "controlled": true,
                "protocol": {
                    "type": "WEEB3_PONG",
                    "protocol": 4,
                    "scope": "/weeb-3/"
                }
            }
        });
        assert!(validate_service_worker(&old_protocol, "/weeb-3/").is_err());
    }

    #[test]
    fn cold_start_gate_requires_empty_worker_and_cache_state() {
        let valid = json!({
            "profile": {
                "cold_start": {
                    "controller_present_at_document_start": false,
                    "registrations": {
                        "status": "resolved", "count": 0, "scripts": []
                    },
                    "cache_storage": {
                        "status": "resolved", "count": 0, "names": []
                    },
                    "http_cache_cleared_before_navigation": true,
                    "http_cache_disabled_before_navigation": true
                }
            }
        });
        validate_cold_browser_start(&valid).unwrap();

        let controlled = json!({
            "profile": {
                "cold_start": {
                    "controller_present_at_document_start": true,
                    "registrations": {
                        "status": "resolved", "count": 1, "scripts": ["service.js"]
                    },
                    "cache_storage": {
                        "status": "resolved", "count": 0, "names": []
                    },
                    "http_cache_cleared_before_navigation": true,
                    "http_cache_disabled_before_navigation": true
                }
            }
        });
        assert!(validate_cold_browser_start(&controlled).is_err());

        let populated_cache = json!({
            "profile": {
                "cold_start": {
                    "controller_present_at_document_start": false,
                    "registrations": {
                        "status": "resolved", "count": 0, "scripts": []
                    },
                    "cache_storage": {
                        "status": "resolved", "count": 1, "names": ["stale"]
                    },
                    "http_cache_cleared_before_navigation": true,
                    "http_cache_disabled_before_navigation": true
                }
            }
        });
        assert!(validate_cold_browser_start(&populated_cache).is_err());
    }
}
