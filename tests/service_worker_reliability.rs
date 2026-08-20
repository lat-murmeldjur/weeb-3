const INTERFACE: &str = include_str!("../src/interface.rs");
const RUNTIME: &str = include_str!("../src/interface_runtime_conventions.rs");
const WORKER: &str = include_str!("../static/service.js");

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .and_then(|(_, tail)| tail.split_once(end))
        .map(|(body, _)| body)
        .unwrap_or_else(|| panic!("missing source section between {start:?} and {end:?}"))
}

#[test]
fn playback_readiness_never_waits_behind_worker_setup() {
    let nonblocking_setup = between(
        RUNTIME,
        "fn start_service_worker_setup_if_idle()",
        "async fn get_service_worker_locked(",
    );
    assert!(nonblocking_setup.contains("setup_lock.try_lock_arc()"));
    assert!(nonblocking_setup.contains("spawn_local(async move"));
    assert!(nonblocking_setup.contains("get_service_worker_locked(&service0).await"));
    assert!(RUNTIME.contains("fn start_service_worker_setup_if_idle() -> bool"));
    assert!(nonblocking_setup.contains("return false;"));
    assert!(nonblocking_setup.trim_end().ends_with("true\n}"));

    let readiness = between(
        RUNTIME,
        "async fn wait_for_service_worker_control(",
        "pub(crate) async fn service_worker_controls_bzz_requests(",
    );
    assert!(readiness.contains("start_service_worker_setup_if_idle()"));
    assert!(!readiness.contains("get_service_worker().await"));
    assert!(readiness.contains("controlled_service_worker().is_none()"));
    assert!(readiness.contains("service_worker_forwarder_ready_with_timeout(500).await"));
    assert!(!readiness.contains("MAX_FOLLOWUP_PROBES"));
}

#[test]
fn busy_or_failed_setup_remains_retryable_without_overlap() {
    let readiness = between(
        RUNTIME,
        "async fn wait_for_service_worker_control(",
        "pub(crate) async fn service_worker_controls_bzz_requests(",
    );
    assert!(readiness.contains("let mut next_setup_retry_ms: f64 = 0.0;"));
    assert!(readiness.contains("&& start_service_worker_setup_if_idle()"));
    assert!(readiness.contains("next_setup_retry_ms = now + SERVICE_WORKER_SETUP_RETRY_MS;"));
    assert!(readiness.contains("now >= next_setup_retry_ms"));
    assert!(!readiness.contains("let mut setup_started"));
}

#[test]
fn readiness_requires_a_controlling_protocol_worker() {
    assert!(WORKER.contains("event.waitUntil(self.skipWaiting())"));
    assert!(WORKER.contains("event.waitUntil(self.clients.claim())"));
    assert!(WORKER.contains("type: \"WEEB3_PONG\""));
    assert!(WORKER.contains("protocol: SERVICE_WORKER_PROTOCOL"));
    assert!(WORKER.contains("scope: SCOPE_PATH"));

    let readiness = between(
        RUNTIME,
        "async fn wait_for_service_worker_control(",
        "pub(crate) async fn service_worker_controls_bzz_requests(",
    );
    assert!(!readiness.contains("registration.active()"));
    assert!(!readiness.contains("registration.waiting()"));
    assert!(!readiness.contains("registration.installing()"));
}

#[test]
fn staged_range_stream_awaits_its_first_window_then_keeps_four_window_lookahead() {
    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    assert!(WORKER.contains("const HLS_STREAM_LOOKAHEAD_CHUNKS = 4;"));
    assert!(stream.contains(
        "const lookahead = stagedStart ? HLS_STREAM_LOOKAHEAD_CHUNKS : STREAM_LOOKAHEAD_CHUNKS;"
    ));
    assert!(!stream.contains("stagedWindows"));
    let scheduler = between(
        stream,
        "const scheduleMore = () => {",
        "const drainScheduledRanges",
    );
    assert!(scheduler.contains("!schedulingAdmissionOpen || !lookaheadAdmitted"));
    assert!(scheduler.contains("scheduled.size < lookahead"));

    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let staged_gate = pull
        .find("if (lookaheadAdmitted) {")
        .expect("ordinary streams retain immediate bounded lookahead");
    let foreground = pull
        .find("const foreground = admitNextRange();")
        .expect("first foreground range admission");
    let awaited = pull
        .find("const response = await request;")
        .expect("foreground range completion");
    let lookahead = pull
        .find("lookaheadAdmitted = true;")
        .expect("lookahead admission after foreground completion");
    let schedule = pull
        .rfind("scheduleMore();")
        .expect("bounded speculative scheduling");
    assert!(
        staged_gate < foreground
            && foreground < awaited
            && awaited < lookahead
            && lookahead < schedule
    );
    assert!(pull[staged_gate..foreground].contains("scheduleMore();"));
}

#[test]
fn ordinary_range_streams_keep_their_immediate_bounded_lookahead() {
    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    assert!(WORKER.contains("const STREAM_LOOKAHEAD_CHUNKS = 8;"));
    assert!(stream.contains(
        "const lookahead = stagedStart ? HLS_STREAM_LOOKAHEAD_CHUNKS : STREAM_LOOKAHEAD_CHUNKS;"
    ));
    assert!(stream.contains("let lookaheadAdmitted = !stagedStart;"));
    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let schedule = pull
        .find("if (lookaheadAdmitted) {")
        .expect("ordinary lookahead gate");
    let await_response = pull
        .find("const response = await request;")
        .expect("range completion");
    assert!(schedule < await_response);
    assert!(pull[schedule..await_response].contains("scheduleMore();"));
}

#[test]
fn progressive_range_streams_echo_the_outer_playback_token_to_every_rust_range() {
    let request = between(
        WORKER,
        "function requestRustRange(",
        "function createRustRangeStream(",
    );
    assert!(request.contains("streamToken = \"\""));
    assert!(request.contains("streamToken\n"));

    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    assert!(stream.contains("stagedStart, streamToken = \"\""));
    assert!(stream.contains("requestRustRange(clients, url, start, end, networkId, streamToken)"));

    let forward = between(
        WORKER,
        "async function forwardRequestToRust(",
        "function parseUploadRedundancyHeader(",
    );
    assert!(forward.contains("headers.get(\"X-Weeb3-Stream-Token\")"));
    assert!(forward.contains("stagedStart,\n        streamToken"));
}

#[test]
fn range_stream_cancel_closes_admission_and_drains_dispatched_promises() {
    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    let drain = between(
        stream,
        "const drainScheduledRanges = () => {",
        "return new ReadableStream(",
    );
    let retained = drain
        .find("Array.from(scheduled.values())")
        .expect("already-dispatched range promises must be retained");
    let settled = drain
        .find("Promise.allSettled(dispatched)")
        .expect("already-dispatched range promises must drain");
    let clear = drain
        .find("scheduled.clear();")
        .expect("settled range bookkeeping cleanup");
    assert!(retained < settled && settled < clear);

    let cancel = between(stream, "cancel() {", "\n    }");
    let close = cancel
        .find("schedulingAdmissionOpen = false;")
        .expect("future range scheduling must close first");
    let drain = cancel
        .find("return drainScheduledRanges();")
        .expect("cancel must retain the drain promise");
    assert!(close < drain);
    assert!(!cancel.contains("scheduled.clear()"));
    assert!(!cancel.contains("requestRustRange("));
    assert!(!cancel.contains("AbortController"));
    assert!(!cancel.contains(".abort("));
}

#[test]
fn setup_validates_scope_and_every_registration_state() {
    let validation = between(
        RUNTIME,
        "fn expected_service_worker_registration(",
        "fn warn_about_worker_conflict(",
    );
    assert!(validation.contains("registration.scope() != expected_scope_url"));
    assert!(validation.contains("registration.active()"));
    assert!(validation.contains("registration.waiting()"));
    assert!(validation.contains("registration.installing()"));

    let setup = between(
        RUNTIME,
        "async fn get_service_worker_locked(",
        "fn controlled_service_worker()",
    );
    let validation = setup
        .find("expected_service_worker_registration(")
        .expect("registration must be validated");
    let registration = setup
        .find("register_with_options(")
        .expect("worker must be registered");
    assert!(validation < registration);
    assert!(setup.contains("JsValue::from_str(\"updateViaCache\")"));
    assert!(setup.contains("JsValue::from_str(\"none\")"));
}

#[test]
fn npm_bridge_and_missing_worker_diagnostics_do_not_require_interface_dom() {
    let bridge = between(
        INTERFACE,
        "pub(crate) fn install_service_worker_message_bridge(",
        "#[wasm_bindgen]\npub async fn interweeb",
    );
    assert!(bridge.contains("let Some(service_worker) = service_worker_container()"));
    assert!(!bridge.contains("navigator().service_worker()"));

    let missing = between(
        RUNTIME,
        "pub(crate) fn service_worker_missing()",
        "pub(super) fn render_text_result(",
    );
    assert!(missing.contains("web_sys::console::warn_1"));
    assert!(missing.contains("get_element_by_id(\"resultField\")"));
    let result_field = missing.find("get_element_by_id(\"resultField\")").unwrap();
    let visible_latch = missing.find("SERVICE_WORKER_MISSING_VISIBLE.with").unwrap();
    assert!(result_field < visible_latch);
    assert!(!missing.contains("expect("));
    assert!(!missing.contains("unwrap()"));
}
