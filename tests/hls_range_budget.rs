const STREAM: &str = include_str!("../src/stream.rs");

fn source_section(start: &str, end: &str) -> &'static str {
    let start = STREAM
        .find(start)
        .unwrap_or_else(|| panic!("missing stream source marker: {start}"));
    let tail = &STREAM[start..];
    let end = tail
        .find(end)
        .unwrap_or_else(|| panic!("missing stream source marker: {end}"));
    &tail[..end]
}

#[test]
fn hls_range_has_one_bounded_cache_read_without_an_outer_retry() {
    let adapter = source_section(
        "pub(crate) async fn read_cached_hls_range(",
        "pub(crate) fn hls_aligned_range_cached(",
    );

    assert_eq!(adapter.matches("read_cached_range(").count(), 1);
    assert!(adapter.contains("Duration::from_millis(STREAM_RANGE_REQUEST_TIMEOUT_MS)"));
    assert!(!adapter.contains("read_cached_range_with_retry"));
    assert!(!adapter.contains("STREAM_RANGE_RETRY_COUNT"));
}

#[test]
fn ordinary_media_keeps_its_existing_retry_policy() {
    let ordinary = source_section(
        "async fn read_cached_range_with_retry(",
        "async fn read_cached_range(",
    );

    assert_eq!(ordinary.matches("read_cached_range(").count(), 1);
    assert!(ordinary.contains("generation,\n            None,"));
    assert!(ordinary.contains("STREAM_RANGE_RETRY_COUNT"));
    assert!(ordinary.contains("RANGE_RETRY_DELAY_MS"));
}

#[test]
fn scheduler_cache_probe_accepts_only_exact_storage_windows() {
    let probe = source_section(
        "pub(crate) fn hls_aligned_range_cached(",
        "pub(crate) fn hls_range_body_fully_cached(",
    );

    assert!(probe.contains("range_storage_window_for_start(start, metadata.size) != (start, end)"));
    assert!(probe.contains("cache.borrow().ranges.contains_key(&key)"));
}
