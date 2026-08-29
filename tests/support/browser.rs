use std::{env, ffi::OsStr, path::PathBuf, time::Duration};

use anyhow_crates_io::{Result, anyhow};
use headless_chrome::{Browser, LaunchOptionsBuilder};

pub fn launch(timeout: Duration, configurable_headful: bool) -> Result<Browser> {
    let mut builder = LaunchOptionsBuilder::default();
    builder
        .headless(!configurable_headful || !env_bool("WEEB3_HEADFUL", false))
        .ignore_certificate_errors(true)
        .sandbox(!env_bool("WEEB3_CHROME_NO_SANDBOX", true))
        .window_size(Some((1280, 720)))
        .idle_browser_timeout(timeout + Duration::from_secs(30))
        .args(vec![
            OsStr::new("--disable-background-networking"),
            OsStr::new("--disable-cache"),
            OsStr::new("--disable-dev-shm-usage"),
            OsStr::new("--disable-extensions"),
            OsStr::new("--ignore-certificate-errors"),
            OsStr::new("--no-first-run"),
        ]);
    if let Ok(path) = env::var("WEEB3_CHROME")
        && !path.trim().is_empty()
    {
        builder.path(Some(PathBuf::from(path)));
    }
    Browser::new(
        builder
            .build()
            .map_err(|error| anyhow!("could not build Chrome launch options: {error:?}"))?,
    )
    .map_err(|error| anyhow!("could not launch Chrome/Chromium: {error:?}"))
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name).map_or(default, |value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "y" | "on"
        )
    })
}
