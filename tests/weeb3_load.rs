use anyhow_crates_io::{Context, Result, anyhow};
use headless_chrome::{Browser, LaunchOptionsBuilder};
use serde_json_crates_io::{Value, json};
use std::{
    env,
    ffi::OsStr,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

const DEFAULT_TARGET_URL: &str = "https://192.168.100.148:8080/weeb-3/bzz/296e6d5d416e538cefcfeaae6898c483a8becfe3c94964bf0b979cb1f537f7a2ef8fd00fedd11253510f6624dd543513562e57e307740f41482a675ac1484c8e";

#[test]
fn weeb3_loads_in_browser() -> Result<()> {
    let target_url = env::var("WEEB3_URL").unwrap_or_else(|_| DEFAULT_TARGET_URL.to_string());

    let runs = env_u64("WEEB3_RUNS", 3)? as u32;
    assert!(runs > 0, "WEEB3_RUNS must be greater than 0");

    let timeout = Duration::from_millis(env_u64("WEEB3_TIMEOUT_MS", 60_000)?);
    let max_load_ms = env_optional_u64("WEEB3_MAX_LOAD_MS")?;

    let ready_selector = env::var("WEEB3_READY_SELECTOR")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let mut results: Vec<Value> = Vec::new();

    for run in 1..=runs {
        let browser = launch_browser(timeout)?;
        let tab = browser
            .new_tab()
            .map_err(|err| anyhow!("failed to open a new Chrome tab: {err:?}"))?;

        tab.set_default_timeout(timeout);

        let started = Instant::now();

        tab.navigate_to(&target_url)
            .map_err(|err| anyhow!("failed to navigate to {target_url}: {err:?}"))?
            .wait_until_navigated()
            .map_err(|err| anyhow!("Chrome did not finish navigation: {err:?}"))?;

        wait_for_load_event_end(&tab, timeout)?;

        let wall_load_ms = elapsed_ms(started);

        let ready_ms = if let Some(selector) = &ready_selector {
            tab.wait_until_visible_with_custom_timeout(selector, timeout)
                .map_err(|err| {
                    anyhow!("ready selector did not become visible: {selector}: {err:?}")
                })?;

            Some(elapsed_ms(started))
        } else {
            None
        };

        let metrics = read_browser_metrics(&tab)?;

        let href = metrics["href"].as_str().unwrap_or_default();
        assert!(
            !href.starts_with("chrome-error://"),
            "Chrome opened an error page instead of the target page: {href}"
        );

        let ready_state = metrics["ready_state"].as_str().unwrap_or_default();
        assert_eq!(
            ready_state, "complete",
            "document.readyState was not complete"
        );

        if let Some(status) = metrics
            .pointer("/navigation/response_status")
            .and_then(Value::as_u64)
        {
            assert!(
                (200..400).contains(&status),
                "navigation returned HTTP status {status}"
            );
        }

        let load_event_ms = metrics
            .pointer("/navigation/load_event_ms")
            .and_then(Value::as_u64);

        let dom_content_loaded_ms = metrics
            .pointer("/navigation/dom_content_loaded_ms")
            .and_then(Value::as_u64);

        let wasm_count = metrics["wasm_resources"]
            .as_array()
            .map(|items| items.len())
            .unwrap_or(0);

        println!(
            "weeb-3 load run {run}: wall_load_ms={wall_load_ms}, \
             load_event_ms={load_event_ms:?}, \
             dom_content_loaded_ms={dom_content_loaded_ms:?}, \
             wasm_count={wasm_count}, \
             ready_ms={ready_ms:?}"
        );

        results.push(json!({
            "run": run,
            "wall_load_ms": wall_load_ms,
            "ready_ms": ready_ms,
            "metrics": metrics
        }));

        drop(browser);
    }

    let median_wall_load_ms = median(
        results
            .iter()
            .filter_map(|result| result["wall_load_ms"].as_u64())
            .collect(),
    );

    let summary = json!({
        "target_url": target_url,
        "runs": runs,
        "median_wall_load_ms": median_wall_load_ms,
        "max_load_ms": max_load_ms,
        "ready_selector": ready_selector,
        "results": results
    });

    fs::create_dir_all("target/weeb3-load-test")
        .context("failed to create target/weeb3-load-test directory")?;

    let summary_json = serde_json_crates_io::to_string_pretty(&summary)
        .context("failed to serialize load test summary")?;

    fs::write(
        "target/weeb3-load-test/weeb3-load-results.json",
        summary_json,
    )
    .context("failed to write target/weeb3-load-test/weeb3-load-results.json")?;

    println!("weeb-3 median wall load: {median_wall_load_ms} ms over {runs} run(s)");
    println!("results written to target/weeb3-load-test/weeb3-load-results.json");

    if let Some(limit) = max_load_ms {
        assert!(
            median_wall_load_ms <= limit,
            "median wall load {median_wall_load_ms} ms exceeded WEEB3_MAX_LOAD_MS={limit} ms"
        );
    }

    Ok(())
}

fn launch_browser(timeout: Duration) -> Result<Browser> {
    let args = vec![
        OsStr::new("--disable-background-networking"),
        OsStr::new("--disable-cache"),
        OsStr::new("--disable-dev-shm-usage"),
        OsStr::new("--disable-extensions"),
        OsStr::new("--ignore-certificate-errors"),
        OsStr::new("--no-first-run"),
    ];

    let mut builder = LaunchOptionsBuilder::default();

    builder
        .headless(!env_bool("WEEB3_HEADFUL", false))
        .ignore_certificate_errors(true)
        .sandbox(!env_bool("WEEB3_CHROME_NO_SANDBOX", true))
        .window_size(Some((1280, 720)))
        .idle_browser_timeout(timeout + Duration::from_secs(30))
        .args(args);

    if let Ok(path) = env::var("WEEB3_CHROME") {
        if !path.trim().is_empty() {
            builder.path(Some(PathBuf::from(path)));
        }
    }

    let options = builder
        .build()
        .map_err(|err| anyhow!("failed to build Chrome launch options: {err:?}"))?;

    Browser::new(options).map_err(|err| {
        anyhow!(
            "failed to launch Chrome/Chromium. \
             Install Chrome/Chromium, set CHROME, or set WEEB3_CHROME=/path/to/chrome. \
             Error: {err:?}"
        )
    })
}

fn wait_for_load_event_end(tab: &headless_chrome::Tab, timeout: Duration) -> Result<()> {
    let script = format!(
        r#"
        new Promise((resolve, reject) => {{
            const deadline = performance.now() + {};
            const check = () => {{
                const nav = performance.getEntriesByType('navigation')[0];

                if (nav && nav.loadEventEnd > 0) {{
                    return resolve(true);
                }}

                if (performance.now() > deadline) {{
                    return reject(new Error('timed out waiting for loadEventEnd'));
                }}

                setTimeout(check, 50);
            }};

            check();
        }})
        "#,
        timeout.as_millis()
    );

    tab.evaluate(&script, true)
        .map_err(|err| anyhow!("load event did not finish before timeout: {err:?}"))?;

    Ok(())
}

fn read_browser_metrics(tab: &headless_chrome::Tab) -> Result<Value> {
    let remote = tab
        .evaluate(
            r#"
            JSON.stringify((() => {
                const round = n => Number.isFinite(n) ? Math.round(n) : null;

                const responseStatus = entry => {
                    if (entry && typeof entry.responseStatus === 'number') {
                        return entry.responseStatus;
                    }

                    return null;
                };

                const nav = performance.getEntriesByType('navigation')[0] || null;
                const resources = performance.getEntriesByType('resource') || [];

                return {
                    href: location.href,
                    title: document.title,
                    ready_state: document.readyState,

                    navigation: nav ? {
                        response_status: responseStatus(nav),
                        response_end_ms: round(nav.responseEnd - nav.startTime),
                        dom_content_loaded_ms: round(
                            nav.domContentLoadedEventEnd - nav.startTime
                        ),
                        load_event_ms: round(nav.loadEventEnd - nav.startTime),
                        duration_ms: round(nav.duration),
                        transfer_size: nav.transferSize || 0,
                        encoded_body_size: nav.encodedBodySize || 0,
                        decoded_body_size: nav.decodedBodySize || 0
                    } : null,

                    resource_count: resources.length,

                    wasm_resources: resources
                        .filter(r => r.name.toLowerCase().includes('.wasm'))
                        .map(r => ({
                            name: r.name,
                            start_ms: round(r.startTime),
                            duration_ms: round(r.duration),
                            response_status: responseStatus(r),
                            transfer_size: r.transferSize || 0,
                            encoded_body_size: r.encodedBodySize || 0,
                            decoded_body_size: r.decodedBodySize || 0
                        }))
                };
            })())
            "#,
            false,
        )
        .map_err(|err| anyhow!("failed to evaluate browser performance metrics: {err:?}"))?;

    let raw = remote
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("Chrome did not return metrics JSON"))?;

    serde_json_crates_io::from_str(&raw).context("failed to parse browser metrics JSON")
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn median(mut values: Vec<u64>) -> u64 {
    assert!(
        !values.is_empty(),
        "cannot calculate median of empty values"
    );

    values.sort_unstable();
    values[values.len() / 2]
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

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "y" | "on"
        ),
        Err(_) => default,
    }
}
