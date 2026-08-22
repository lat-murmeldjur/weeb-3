use std::collections::{BTreeMap, BTreeSet};

const INTERFACE: &str = include_str!("../src/interface.rs");
const RUNTIME: &str = include_str!("../src/interface_runtime_conventions.rs");
const SERVER: &str = include_str!("../src/main.rs");
const WORKER: &str = include_str!("../static/service.js");
const BUILD: &str = include_str!("../build.rs");

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .and_then(|(_, tail)| tail.split_once(end))
        .map(|(body, _)| body)
        .unwrap_or_else(|| panic!("missing source section between {start:?} and {end:?}"))
}

const MODEL_ACTIVE_LIMIT: usize = 4;
const MODEL_READY_ADMISSION_THRESHOLD: usize = MODEL_ACTIVE_LIMIT - 1;
const MODEL_OUTSTANDING_LIMIT: usize = MODEL_ACTIVE_LIMIT + MODEL_READY_ADMISSION_THRESHOLD - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelResponse {
    Ready,
    Failed,
}

#[derive(Debug)]
struct OrderedFlightModel {
    total_windows: usize,
    managed_prefix_windows: usize,
    emitted_windows: usize,
    next_window: usize,
    admission_open: bool,
    lookahead_admitted: bool,
    known_failure: bool,
    active: BTreeSet<usize>,
    retained: BTreeMap<usize, ModelResponse>,
    dispatched: Vec<usize>,
}

impl OrderedFlightModel {
    fn staged(total_windows: usize, managed_prefix_windows: usize) -> Self {
        Self {
            total_windows,
            managed_prefix_windows: managed_prefix_windows.min(total_windows),
            emitted_windows: 0,
            next_window: 0,
            admission_open: true,
            lookahead_admitted: false,
            known_failure: false,
            active: BTreeSet::new(),
            retained: BTreeMap::new(),
            dispatched: Vec::new(),
        }
    }

    fn admit_first_window(&mut self) {
        assert!(!self.lookahead_admitted);
        assert_eq!(self.next_window, 0);
        self.admit_one();
    }

    fn admit_one(&mut self) {
        assert!(self.admission_open);
        assert!(self.next_window < self.total_windows);
        assert!(self.active.len() < MODEL_ACTIVE_LIMIT);
        assert!(self.active.len() + self.retained.len() < MODEL_OUTSTANDING_LIMIT);
        let window = self.next_window;
        self.next_window += 1;
        assert!(self.active.insert(window));
        self.dispatched.push(window);
        self.assert_bounds();
    }

    fn schedule_more(&mut self) {
        if !self.admission_open || !self.lookahead_admitted || self.known_failure {
            return;
        }
        if self.emitted_windows < self.managed_prefix_windows {
            if self.next_window < self.total_windows && self.active.len() + self.retained.len() < 1
            {
                self.admit_one();
            }
            return;
        }
        while self.next_window < self.total_windows
            && self.active.len() < MODEL_ACTIVE_LIMIT
            && self.retained.len() < MODEL_READY_ADMISSION_THRESHOLD
            && self.active.len() + self.retained.len() < MODEL_OUTSTANDING_LIMIT
        {
            self.admit_one();
        }
    }

    fn settle_success(&mut self, window: usize) {
        assert!(self.active.remove(&window));
        assert_eq!(self.retained.insert(window, ModelResponse::Ready), None);
        if self.emitted_windows >= self.managed_prefix_windows {
            self.schedule_more();
        }
        self.assert_bounds();
    }

    fn settle_failure(&mut self, window: usize) {
        assert!(self.active.remove(&window));
        assert_eq!(self.retained.insert(window, ModelResponse::Failed), None);
        self.known_failure = true;
        self.assert_bounds();
    }

    fn emit_next(&mut self) -> Option<usize> {
        if self.retained.get(&self.emitted_windows) != Some(&ModelResponse::Ready) {
            return None;
        }

        let emitted = self.emitted_windows;
        self.retained.remove(&emitted);
        self.emitted_windows += 1;
        self.lookahead_admitted = true;
        self.schedule_more();
        self.assert_bounds();
        Some(emitted)
    }

    fn close_and_drain_active_snapshot(&mut self) -> Vec<usize> {
        self.admission_open = false;
        self.lookahead_admitted = false;
        self.active.iter().copied().collect()
    }

    fn retained_windows(&self) -> Vec<usize> {
        self.retained.keys().copied().collect()
    }

    fn active_windows(&self) -> Vec<usize> {
        self.active.iter().copied().collect()
    }

    fn assert_bounds(&self) {
        assert!(self.active.len() <= MODEL_ACTIVE_LIMIT);
        assert!(self.active.len() + self.retained.len() <= MODEL_OUTSTANDING_LIMIT);
    }
}

#[test]
fn ordered_flight_model_refills_successes_until_three_ready_results_gate_admission() {
    let mut model = OrderedFlightModel::staged(20, 1);
    model.admit_first_window();
    model.settle_success(0);
    assert_eq!(
        model.next_window, 1,
        "the first window remains exclusive until emitted"
    );
    assert_eq!(model.emit_next(), Some(0));
    assert_eq!(model.active_windows(), vec![1, 2, 3, 4]);

    for settled in [4, 3] {
        let next_before = model.next_window;
        model.settle_success(settled);
        assert_eq!(model.next_window, next_before + 1);
        assert_eq!(model.active.len(), MODEL_ACTIVE_LIMIT);
        assert_eq!(
            model.emit_next(),
            None,
            "window 1 still gates ordered output"
        );
    }

    model.settle_success(2);
    assert_eq!(model.next_window, 7, "three ready responses stop refill");
    assert_eq!(model.active_windows(), vec![1, 5, 6]);
    assert_eq!(model.retained_windows(), vec![2, 3, 4]);
    assert_eq!(model.retained.len(), MODEL_READY_ADMISSION_THRESHOLD);
    assert_eq!(
        model.active.len() + model.retained.len(),
        MODEL_OUTSTANDING_LIMIT
    );
    assert_eq!(
        model.emit_next(),
        None,
        "window 1 still gates ordered output"
    );

    model.settle_success(5);
    assert_eq!(
        model.next_window, 7,
        "already-active settlement does not refill"
    );
    assert_eq!(
        model.retained.len(),
        4,
        "ready results may grow after admission stops"
    );
    assert_eq!(
        model.active.len() + model.retained.len(),
        MODEL_OUTSTANDING_LIMIT
    );

    model.settle_success(1);
    assert_eq!(model.emit_next(), Some(1));
    assert_eq!(
        model.next_window, 7,
        "four retained results still gate refill"
    );
    assert_eq!(model.emit_next(), Some(2));
    assert_eq!(
        model.next_window, 7,
        "three retained results still gate refill"
    );
    assert_eq!(model.emit_next(), Some(3));
    assert_eq!(
        model.next_window, 10,
        "consumption below three refills active pressure"
    );
    assert_eq!(model.active.len(), MODEL_ACTIVE_LIMIT);
    assert_eq!(model.emit_next(), Some(4));
    assert_eq!(model.emit_next(), Some(5));
}

#[test]
fn ordered_flight_model_keeps_every_managed_prefix_window_serial() {
    let mut model = OrderedFlightModel::staged(12, 5);
    model.admit_first_window();
    for window in 0..5 {
        assert_eq!(model.active_windows(), vec![window]);
        assert!(model.retained.is_empty());
        model.settle_success(window);
        assert!(model.active.is_empty());
        assert_eq!(model.retained_windows(), vec![window]);
        assert_eq!(
            model.next_window,
            window + 1,
            "settlement alone must not admit the next prefix range"
        );
        assert_eq!(model.emit_next(), Some(window));
        if window < 4 {
            assert_eq!(model.active_windows(), vec![window + 1]);
        }
    }
    assert_eq!(model.active_windows(), vec![5, 6, 7, 8]);
}

#[test]
fn ordered_flight_model_latches_an_out_of_order_failure_against_later_refills() {
    let mut model = OrderedFlightModel::staged(20, 1);
    model.admit_first_window();
    model.settle_success(0);
    assert_eq!(model.emit_next(), Some(0));
    model.settle_failure(4);
    let next_at_failure = model.next_window;
    model.settle_success(3);
    assert_eq!(model.next_window, next_at_failure);
    assert!(model.known_failure);
    assert_eq!(
        model.emit_next(),
        None,
        "ordered output still waits for window 1"
    );
}

#[test]
fn ordered_flight_model_closes_admission_before_snapshotting_active_dispatches() {
    let mut model = OrderedFlightModel::staged(20, 1);
    model.admit_first_window();
    model.settle_success(0);
    assert_eq!(model.emit_next(), Some(0));
    model.settle_success(4);
    let active_at_close = model.close_and_drain_active_snapshot();
    let next_at_close = model.next_window;

    model.settle_success(5);
    assert_eq!(
        model.next_window, next_at_close,
        "settlement cannot refill after close"
    );
    assert_eq!(active_at_close, vec![1, 2, 3, 5]);
    assert_eq!(model.retained_windows(), vec![4, 5]);
}

#[test]
fn native_server_rebuilds_and_revalidates_every_embedded_browser_runtime_asset() {
    let source_version = BUILD
        .split("fn source_build_version()")
        .nth(1)
        .and_then(|source| source.split("fn asset_build_version()").next())
        .expect("source build version");
    for asset in ["static/weeb_3.js", "static/weeb_3_bg.wasm"] {
        assert!(
            !source_version.contains(asset),
            "generated asset {asset} must not feed the version embedded into Wasm"
        );
    }
    assert!(!source_version.contains("static/snippets"));
    assert!(BUILD.contains("collect_all_files(Path::new(\"static/snippets\"), &mut files);"));
    assert!(BUILD.contains("CARGO_CFG_TARGET_ARCH"));
    assert!(BUILD.contains("cargo:rustc-env=WEEB3_ASSET_VERSION={asset_version}"));
    assert!(BUILD.contains("cargo:rerun-if-changed=static/snippets"));
    assert!(
        SERVER.contains(
            "use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};"
        )
    );
    assert!(
        SERVER
            .contains("const EMBEDDED_ASSET_BUILD_VERSION: &str = env!(\"WEEB3_ASSET_VERSION\");")
    );
    assert!(SERVER.contains(
        "const EMBEDDED_ASSET_ETAG: &str = concat!(\"\\\"\", env!(\"WEEB3_ASSET_VERSION\"), \"\\\"\");"
    ));
    assert!(SERVER.contains("const REVALIDATE_EMBEDDED_ASSET: &str = \"private, no-cache\";"));
    assert!(SERVER.contains("HeaderName::from_static(\"x-weeb3-build-version\")"));

    for handler in ["get_index", "get_example", "get_404"] {
        let body = between(SERVER, &format!("async fn {handler}"), "\n}");
        assert!(
            body.contains("(CACHE_CONTROL, \"no-store\")"),
            "{handler} must keep document responses out of the HTTP cache"
        );
    }

    let validator = between(
        SERVER,
        "fn embedded_asset_is_current(",
        "fn embedded_asset_response(",
    );
    assert!(validator.contains(".get_all(IF_NONE_MATCH)"));
    assert!(validator.contains("value.strip_prefix(\"W/\").unwrap_or(value)"));
    assert!(validator.contains("== EMBEDDED_ASSET_ETAG"));

    let response = between(
        SERVER,
        "fn embedded_asset_response(",
        "async fn get_static_file(",
    );
    assert!(response.contains("(CACHE_CONTROL, REVALIDATE_EMBEDDED_ASSET)"));
    assert!(response.contains("(ETAG, EMBEDDED_ASSET_ETAG)"));
    assert!(response.contains("StatusCode::NOT_MODIFIED"));

    let static_file = between(
        SERVER,
        "async fn get_static_file(",
        "async fn get_static_snippet(",
    );
    assert!(static_file.contains("if path == \"service.js\""));
    assert!(static_file.contains("(CACHE_CONTROL, \"no-store\")"));
    assert!(static_file.contains("embedded_asset_response(&headers, content, content_type)"));

    let snippets = between(SERVER, "async fn get_static_snippet(", "async fn get_404(");
    assert!(snippets.contains("embedded_asset_response("));
}

#[test]
fn bfcache_restore_requests_one_full_reload_and_normal_pageshow_does_not() {
    let persisted = between(
        INTERFACE,
        "fn pageshow_event_is_persisted(",
        "fn install_bfcache_restore_guard(",
    );
    assert!(persisted.contains("Reflect::get(event.as_ref()"));
    assert!(persisted.contains("JsValue::from_str(\"persisted\")"));
    assert!(persisted.contains(".as_bool()"));

    let guard = between(
        INTERFACE,
        "fn install_bfcache_restore_guard()",
        "pub(crate) fn install_service_worker_message_bridge(",
    );
    assert!(guard.contains("if listener.borrow().is_some()"));
    assert!(guard.contains("let mut reload_requested = false;"));
    assert!(guard.contains("add_event_listener_with_callback(\"pageshow\""));
    let condition = guard
        .find("if reload_requested || !pageshow_event_is_persisted(&event)")
        .expect("normal pageshow and duplicate restore events must not reload");
    let latch = guard
        .find("reload_requested = true;")
        .expect("persisted restore must latch before reloading");
    let reload = guard
        .find("window.location().reload()")
        .expect("persisted restore must perform a full document reload");
    assert!(condition < latch && latch < reload);

    let bridge = between(
        INTERFACE,
        "pub(crate) fn install_service_worker_message_bridge(",
        "#[wasm_bindgen]\npub async fn interweeb",
    );
    assert!(
        bridge
            .trim_start()
            .starts_with("weeb3: Arc<Weeb3>) {\n    install_bfcache_restore_guard();")
    );
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
    assert!(WORKER.contains("const SERVICE_WORKER_PROTOCOL = 8;"));
    assert!(RUNTIME.contains("const SERVICE_WORKER_PROTOCOL: f64 = 8.0;"));
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
fn staged_range_stream_separates_active_requests_from_settled_ordered_responses() {
    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    assert!(WORKER.contains("const HLS_STREAM_LOOKAHEAD_CHUNKS = 4;"));
    assert!(
        stream.contains(
            "const HLS_STREAM_READY_ADMISSION_THRESHOLD = HLS_STREAM_LOOKAHEAD_CHUNKS - 1;"
        ) || WORKER.contains(
            "const HLS_STREAM_READY_ADMISSION_THRESHOLD = HLS_STREAM_LOOKAHEAD_CHUNKS - 1;"
        )
    );
    assert!(
        WORKER.contains("HLS_STREAM_LOOKAHEAD_CHUNKS + HLS_STREAM_READY_ADMISSION_THRESHOLD - 1;")
    );
    assert!(stream.contains("const activeRequests = new Map();"));
    assert!(stream.contains("const retainedResponses = new Map();"));

    let admission = between(
        stream,
        "const admitStagedRange = () => {",
        "const scheduleMore = () => {",
    );
    let settled = admission
        .find("activeRequests.delete(start);")
        .expect("settlement must release active request pressure");
    let retained = admission
        .find("retainedResponses.set(start, result);")
        .expect("settled result must remain available for ordered output");
    let successful = admission
        .find("schedulingAdmissionOpen && position >= managedPrefixEnd")
        .expect("settlement refill starts only beyond the managed prefix");
    let refill = admission
        .find("scheduleMore();")
        .expect("successful settlement must replenish immediately");
    assert!(settled < retained && retained < successful && successful < refill);
    assert!(admission.contains("activeRequests.set(start, request);"));
    assert!(admission.matches("knownRangeFailure = true;").count() >= 2);

    let scheduler = between(
        stream,
        "const scheduleMore = () => {",
        "const drainScheduledRanges",
    );
    assert!(
        scheduler.contains("!schedulingAdmissionOpen || !lookaheadAdmitted || knownRangeFailure")
    );
    assert!(scheduler.contains("activeRequests.size < HLS_STREAM_LOOKAHEAD_CHUNKS"));
    assert!(scheduler.contains("retainedResponses.size < HLS_STREAM_READY_ADMISSION_THRESHOLD"));
    assert!(
        scheduler
            .contains("activeRequests.size + retainedResponses.size < HLS_STREAM_MAX_OUTSTANDING")
    );

    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let staged_gate = pull
        .find("if (lookaheadAdmitted) {")
        .expect("ordinary streams retain immediate bounded lookahead");
    let foreground = pull
        .find("const foreground = admitStagedRange();")
        .expect("first foreground range admission");
    let awaited = pull
        .find("result = await request;")
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
    let ordered_lookup = pull
        .find("retainedResponses.get(start)")
        .expect("output must await the exact next range");
    let ordered_delete = pull
        .find("retainedResponses.delete(start)")
        .expect("only the exact next settled result is consumed");
    let ordered_enqueue = pull
        .find("controller.enqueue(body);")
        .expect("ordered response emission");
    assert!(
        ordered_lookup < awaited && awaited < ordered_delete && ordered_delete < ordered_enqueue
    );
}

#[test]
fn managed_hls_prefix_keeps_every_critical_window_strictly_serial() {
    let parser = between(
        WORKER,
        "function parseCriticalPrefixWindows(",
        "function createRustRangeStream(",
    );
    assert!(parser.contains("/^[1-9][0-9]*$/"));
    assert!(parser.contains("Number.isSafeInteger(size)"));
    assert!(parser.contains("Math.ceil(size / STREAM_STORAGE_WINDOW_BYTES)"));
    assert!(parser.contains("Math.min(requested, total)"));

    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    assert!(stream.contains("criticalPrefixWindows = null"));
    assert!(stream.contains("const managedPrefixEnd = stagedStart"));
    assert!(
        stream.contains("const HLS_STREAM_LOOKAHEAD_CHUNKS = 4;")
            || WORKER.contains("const HLS_STREAM_LOOKAHEAD_CHUNKS = 4;")
    );

    let scheduler = between(
        stream,
        "const scheduleMore = () => {",
        "const drainScheduledRanges",
    );
    let prefix = between(scheduler, "if (position < managedPrefixEnd) {", "while (");
    assert!(prefix.contains("activeRequests.size + retainedResponses.size < 1"));
    assert!(prefix.contains("admitStagedRange();"));
    assert!(prefix.contains("return;"));

    let admission = between(
        stream,
        "const admitStagedRange = () => {",
        "const scheduleMore = () => {",
    );
    assert!(admission.contains("position >= managedPrefixEnd"));
    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let position = pull.find("position = start + body.byteLength;").unwrap();
    let emitted = pull.find("controller.enqueue(body);").unwrap();
    let refill = pull.rfind("scheduleMore();").unwrap();
    assert!(position < emitted && emitted < refill);

    let forward = between(
        WORKER,
        "async function forwardRequestToRust(",
        "function parseUploadRedundancyHeader(",
    );
    assert!(forward.contains("const criticalPrefixWindows = stagedStart"));
    assert!(forward.contains("headers.get(\"X-Weeb3-HLS-Critical-Prefix-Windows\")"));
    assert!(forward.contains("criticalPrefixWindows"));
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
    assert!(stream.contains("const scheduled = new Map();"));
    let scheduler = between(
        stream,
        "const scheduleMore = () => {",
        "const drainScheduledRanges",
    );
    let ordinary = between(
        scheduler,
        "if (!stagedStart) {",
        "if (position < managedPrefixEnd) {",
    );
    assert!(ordinary.contains("scheduled.size < lookahead"));
    assert!(ordinary.contains("admitOrdinaryRange()"));
    assert!(ordinary.contains("return;"));
    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let schedule = pull
        .find("if (lookaheadAdmitted) {")
        .expect("ordinary lookahead gate");
    let await_response = pull
        .find("response = await request;")
        .expect("range completion");
    assert!(schedule < await_response);
    assert!(pull[schedule..await_response].contains("scheduleMore();"));
    assert!(pull.contains("scheduled.get(start)"));
    assert!(pull.contains("scheduled.delete(start);"));
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
    assert!(stream.contains("streamToken = \"\""));
    assert!(stream.contains("criticalPrefixWindows = null"));
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
        .find("Array.from(activeRequests.values())")
        .expect("every active staged request must enter the drain snapshot");
    assert!(drain.contains("Array.from(scheduled.values())"));
    let settled = drain
        .find("Promise.allSettled(dispatched)")
        .expect("already-dispatched range promises must drain");
    let clear = drain
        .find("activeRequests.clear();")
        .expect("settled range bookkeeping cleanup");
    assert!(retained < settled && settled < clear);
    assert!(drain.contains("retainedResponses.clear();"));
    assert!(!drain[..settled].contains("retainedResponses.values()"));

    let close_admission = between(
        stream,
        "const closeAdmission = () => {",
        "const failStream = async",
    );
    assert!(close_admission.contains("schedulingAdmissionOpen = false;"));
    assert!(close_admission.contains("lookaheadAdmitted = false;"));

    let cancel = between(stream, "cancel() {", "\n    }");
    let close = cancel
        .find("closeAdmission();")
        .expect("future range scheduling must close first");
    let drain = cancel
        .find("return drainScheduledRanges();")
        .expect("cancel must retain the drain promise");
    assert!(close < drain);
    assert!(!cancel.contains("activeRequests.clear()"));
    assert!(!cancel.contains("requestRustRange("));
    assert!(!cancel.contains("AbortController"));
    assert!(!cancel.contains(".abort("));

    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let normal_close = between(pull, "if (position >= size) {", "if (lookaheadAdmitted) {");
    let close_admission = normal_close.find("closeAdmission();").unwrap();
    let close_controller = normal_close.find("controller.close();").unwrap();
    let drain = normal_close.find("await drainScheduledRanges();").unwrap();
    assert!(close_admission < close_controller && close_controller < drain);
}

#[test]
fn range_stream_errors_close_new_admission_and_drain_every_dispatched_request() {
    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    let failure = between(
        stream,
        "const failStream = async (controller, error) => {",
        "return new ReadableStream(",
    );
    let close = failure.find("closeAdmission();").unwrap();
    let error = failure.find("controller.error(error);").unwrap();
    let drain = failure.find("await drainScheduledRanges();").unwrap();
    assert!(close < error && error < drain);

    let pull = between(stream, "async pull(controller) {", "cancel() {");
    assert!(pull.matches("await failStream(").count() >= 2);
    assert!(pull.contains("await failStream(controller, error);"));
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
