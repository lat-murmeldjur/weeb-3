const INTERFACE: &str = include_str!("../src/interface.rs");
const LIBRARY: &str = include_str!("../src/library.rs");
const RUNTIME: &str = include_str!("../src/interface_runtime_conventions.rs");
const SERVER: &str = include_str!("../src/main.rs");
const WORKER: &str = include_str!("../static/service.js");
const BUILD: &str = include_str!("../build.rs");
const NPM_WORKFLOW: &str = include_str!("../.github/workflows/plain.yml");
const NPM_README: &str = include_str!("../README.npm.md");

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .and_then(|(_, tail)| tail.split_once(end))
        .map(|(body, _)| body)
        .unwrap_or_else(|| panic!("missing source section between {start:?} and {end:?}"))
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
fn cold_interface_and_npm_paths_install_the_bridge_before_worker_setup() {
    let interface_mount = between(
        INTERFACE,
        "pub(crate) async fn mount_interface(",
        "pub(crate) async fn mount_interface_after_service_worker_bridge_install(",
    );
    assert!(
        interface_mount
            .find("install_service_worker_message_bridge(weeb3.clone())")
            .unwrap()
            < interface_mount
                .find("mount_interface_after_service_worker_bridge_install(")
                .unwrap()
    );

    let npm_start = between(
        LIBRARY,
        "fn schedule_start(&self, options: StartOptions)",
        "async fn boot_runtime(&self)",
    );
    assert!(
        npm_start
            .find("install_service_worker_message_bridge(self.inner.clone())")
            .unwrap()
            < npm_start.find("get_service_worker().await").unwrap()
    );

    let npm_attach = between(
        LIBRARY,
        "pub async fn attach_stream(",
        "#[wasm_bindgen(js_name = networkState)]",
    );
    assert!(
        npm_attach.find("self.boot_runtime().await").unwrap()
            < npm_attach
                .find("crate::stream_hls::attach_hls_feed_player(")
                .unwrap()
    );
}

#[test]
fn npm_release_contains_the_worker_required_by_the_runtime_protocol() {
    assert!(NPM_WORKFLOW.contains("files[5]=\"service.js\""));
    assert!(NPM_WORKFLOW.contains("static/service.js"));
    assert!(NPM_README.contains("serve the packaged worker at `/weeb-3/service.js`"));
}

#[test]
fn playback_readiness_updates_before_accepting_a_controller() {
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
    let setup = readiness
        .find("let _ = get_service_worker().await")
        .unwrap();
    let ready = readiness
        .find("if service_worker_forwarder_ready().await")
        .unwrap();
    assert!(setup < ready);
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
    assert!(readiness.contains(
        "let mut next_setup_retry_ms = js_sys::Date::now() + SERVICE_WORKER_SETUP_RETRY_MS;"
    ));
    assert!(readiness.contains("&& start_service_worker_setup_if_idle()"));
    assert!(readiness.contains("next_setup_retry_ms = now + SERVICE_WORKER_SETUP_RETRY_MS;"));
    assert!(readiness.contains("now >= next_setup_retry_ms"));
    assert!(!readiness.contains("let mut setup_started"));
}

#[test]
fn readiness_requires_a_controlling_protocol_worker() {
    assert!(WORKER.contains(r#"const SERVICE_WORKER_MARKER = "forwarder-default28";"#));
    assert!(WORKER.contains("const SERVICE_WORKER_PROTOCOL = 10;"));
    assert!(RUNTIME.contains(r#"const SERVICE_WORKER_MARKER: &str = "forwarder-default28";"#));
    assert!(RUNTIME.contains("const SERVICE_WORKER_PROTOCOL: f64 = 10.0;"));
    assert!(WORKER.contains("event.waitUntil(self.skipWaiting())"));
    assert!(WORKER.contains("event.waitUntil(self.clients.claim())"));
    assert!(WORKER.contains("type: \"WEEB3_PONG\""));
    assert!(WORKER.contains("protocol: SERVICE_WORKER_PROTOCOL"));
    assert_eq!(WORKER.matches("marker: SERVICE_WORKER_MARKER").count(), 2);
    assert!(RUNTIME.contains("JsValue::from_str(\"protocol\")"));
    assert!(RUNTIME.contains("JsValue::from_str(\"marker\")"));
    assert!(RUNTIME.contains("marker == expected_marker"));
    assert!(WORKER.contains("event.data?.protocol !== SERVICE_WORKER_PROTOCOL"));
    assert!(!WORKER.contains("source.navigate("));
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
fn hls_routes_own_their_stream_windows_and_preserve_http_validators() {
    let route_constants = between(
        WORKER,
        "const NETWORK_ROUTE_PREFIXES",
        "const FETCH_TIMEOUT_MS",
    );
    assert!(route_constants.contains("[\"\", \"mainnet/\", \"testnet/\"]"));
    assert!(route_constants.contains("[\"hls/bytes\", \"hls-bytes\"]"));

    let fetch_routes = between(
        WORKER,
        "self.addEventListener(\"fetch\"",
        "function clientInScope(",
    );
    assert!(fetch_routes.contains("canonicalRawResource(url)"));
    assert!(fetch_routes.contains("canonicalFeedResource(url)"));
    assert!(
        fetch_routes
            .matches("request.method === \"GET\" || request.method === \"HEAD\"")
            .count()
            >= 3
    );

    let request = between(
        WORKER,
        "function requestRustFetch(",
        "function toUint8Array(",
    );
    for field in [
        "type: \"WEEB3_FETCH_REQUEST\"",
        "method",
        "range",
        "networkId",
        "ifNoneMatch",
        "ifRange",
    ] {
        assert!(request.contains(field), "missing request field {field}");
    }

    let forward = between(
        WORKER,
        "async function forwardRequestToRust(",
        "function parseUploadRedundancyHeader(",
    );
    assert!(!forward.contains("if (hlsResource && response.stream)"));
    assert!(forward.contains("if (response.stream && request.method !== \"HEAD\")"));
    assert!(forward.contains("hlsResource ? \"\" : request.headers.get(\"Range\")"));
    assert!(forward.contains("request.headers.get(\"If-None-Match\")"));
    assert!(forward.contains("hlsResource ? \"\" : request.headers.get(\"If-Range\")"));
    assert!(forward.contains("request.method === \"HEAD\" || status === 304"));
    assert!(forward.contains("const headers = responseHeaders(response.headers);"));

    for removed in [
        "HLS_STREAM_READY_ADMISSION_THRESHOLD",
        "HLS_STREAM_MAX_OUTSTANDING",
        "HLS_REQUEST_FLIGHTS",
        "parseCriticalPrefixWindows",
        "streamToken",
        "X-Weeb3-Stream-Start",
        "X-Weeb3-Stream-Token",
        "X-Weeb3-HLS-Critical-Prefix-Windows",
    ] {
        assert!(
            !WORKER.contains(removed),
            "obsolete HLS protocol remains: {removed}"
        );
    }
}

#[test]
fn generic_range_stream_keeps_ordered_bounded_lookahead() {
    assert!(WORKER.contains("const STREAM_STORAGE_WINDOW_BYTES = MIB_BYTES / 2;"));
    assert!(WORKER.contains("const STREAM_LOOKAHEAD_CHUNKS = 8;"));
    assert!(WORKER.contains("const HLS_STREAM_WINDOW_BYTES = MIB_BYTES / 2;"));
    assert!(WORKER.contains("const HLS_STREAM_INITIAL_LOOKAHEAD_CHUNKS = 1;"));
    assert!(WORKER.contains("const HLS_STREAM_LOOKAHEAD_CHUNKS = 4;"));
    assert!(WORKER.contains("const HLS_LIVE_STREAM_WINDOW_BYTES = MIB_BYTES / 2;"));
    assert!(WORKER.contains("const HLS_LIVE_STREAM_LOOKAHEAD_CHUNKS = 4;"));
    assert!(WORKER.contains("const RANGE_REQUEST_FLIGHTS = new Map();"));

    let request = between(
        WORKER,
        "function requestRustRange(",
        "function createRustRangeStream(",
    );
    assert!(request.contains("range: `bytes=${start}-${end}`"));
    assert!(request.contains("body.byteLength !== expected"));
    assert!(request.contains("RANGE_REQUEST_FLIGHTS.get(key)"));
    assert!(request.contains("RANGE_REQUEST_FLIGHTS.set(key, request)"));
    assert!(request.contains("RANGE_REQUEST_FLIGHTS.get(key) === request"));
    assert!(request.contains("RANGE_REQUEST_FLIGHTS.delete(key)"));

    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    assert!(stream.contains("const scheduled = new Map();"));
    let scheduler = between(
        stream,
        "const scheduleMore = () => {",
        "const drainScheduledRanges",
    );
    assert!(
        scheduler
            .contains("position < initialLookahead * windowBytes ? initialLookahead : lookahead")
    );
    assert!(scheduler.contains("scheduled.size < limit"));
    assert!(scheduler.contains("admitRange()"));

    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let schedule = pull.find("scheduleMore();").expect("bounded lookahead");
    let lookup = pull
        .find("scheduled.get(start)")
        .expect("ordered range lookup");
    let awaited = pull.find("await pending").expect("range completion");
    let removed = pull
        .find("scheduled.delete(start)")
        .expect("consumed range cleanup");
    let emitted = pull
        .find("controller.enqueue(body)")
        .expect("ordered body emission");
    assert!(schedule < lookup && lookup < awaited && awaited < removed && removed < emitted);

    let forward = between(
        WORKER,
        "async function forwardRequestToRust(",
        "function parseUploadRedundancyHeader(",
    );
    assert!(forward.contains("Number.isSafeInteger(size) || size <= 0"));
    assert!(forward.contains("url.searchParams.get(\"start\") === \"live\""));
    assert!(forward.contains("? HLS_LIVE_STREAM_WINDOW_BYTES"));
    assert!(forward.contains(": hlsResource ? HLS_STREAM_WINDOW_BYTES"));
    assert!(forward.contains("? HLS_LIVE_STREAM_LOOKAHEAD_CHUNKS"));
    assert!(forward.contains(": hlsResource ? HLS_STREAM_LOOKAHEAD_CHUNKS"));
    assert!(forward.contains("? HLS_STREAM_INITIAL_LOOKAHEAD_CHUNKS"));
    assert!(forward.contains("const initialLookahead = hlsResource"));
    assert!(!forward.contains("url.searchParams.get(\"startup\")"));
    assert!(!forward.contains("beginningHlsResource"));
}

#[test]
fn hls_stream_stages_one_window_before_admitting_four_window_lookahead() {
    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    assert!(!stream.contains("gateFirstWindow"));
    assert!(
        stream.contains("position < initialLookahead * windowBytes ? initialLookahead : lookahead")
    );
    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let first_schedule = pull.find("scheduleMore();").unwrap();
    let awaited = pull.find("await pending").unwrap();
    let emitted = pull.find("controller.enqueue(body);").unwrap();
    let refill = pull.rfind("scheduleMore();").unwrap();
    assert!(first_schedule < awaited && awaited < emitted && emitted < refill);
}

#[test]
fn generic_range_stream_cancel_closes_admission_and_drains_dispatched_promises() {
    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    let drain = between(
        stream,
        "const drainScheduledRanges = () => {",
        "const closeAdmission = () => {",
    );
    assert!(drain.contains("Promise.allSettled(Array.from(scheduled.values()))"));
    assert!(drain.contains("scheduled.clear();"));

    let bounds = between(
        stream,
        "const nextRangeBounds = () => {",
        "const admitRange = () => {",
    );
    assert!(bounds.contains("if (!admissionOpen || schedulePosition >= size)"));

    let admission = between(
        stream,
        "const admitRange = () => {",
        "const scheduleMore = () => {",
    );
    let dispatch = admission.find("requestRustRange(").unwrap();
    let retain = admission.find("scheduled.set(start, request)").unwrap();
    assert!(dispatch < retain);

    let close_admission = between(
        stream,
        "const closeAdmission = () => {",
        "const failStream = async",
    );
    assert!(close_admission.contains("admissionOpen = false;"));

    let cancel = between(stream, "cancel() {", "\n    }");
    let close = cancel
        .find("closeAdmission();")
        .expect("future scheduling must close first");
    let drain = cancel
        .find("return drainScheduledRanges();")
        .expect("cancel must return the drain promise");
    assert!(close < drain);
    assert!(!cancel.contains("requestRustRange("));
    assert!(!cancel.contains(".abort("));

    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let awaited = pull.find("const response = await pending;").unwrap();
    let cancelled = pull.find("if (!admissionOpen) {").unwrap();
    let emitted = pull.find("controller.enqueue(body);").unwrap();
    let refill = pull.rfind("scheduleMore();").unwrap();
    assert!(awaited < cancelled && cancelled < emitted && emitted < refill);
    let normal_close = between(pull, "if (position >= size) {", "scheduleMore();");
    let close_admission = normal_close.find("closeAdmission();").unwrap();
    let close_controller = normal_close.find("controller.close();").unwrap();
    let drain = normal_close.find("await drainScheduledRanges();").unwrap();
    assert!(close_admission < close_controller && close_controller < drain);
}

#[test]
fn generic_range_stream_errors_close_and_drain_before_returning() {
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
    let update = setup
        .find("registration.update()")
        .expect("an existing expected registration must be updated");
    let acceptance = setup[update..]
        .find("claim_exact_service_worker(&active).await")
        .map(|offset| update + offset)
        .expect("the updated implementation must answer readiness");
    assert!(validation < update && update < acceptance && acceptance < registration);
    let expected_url = setup.find("let expected_worker_url").unwrap();
    assert!(!setup[..expected_url].contains("service_worker_forwarder_ready"));
    assert!(!setup.contains("or(Some(active))"));
    assert!(!setup.contains("return Some(service_worker)"));
    let exact_claim = between(
        RUNTIME,
        "async fn claim_exact_service_worker(",
        "struct ServiceWorkerProtocolPort",
    );
    assert!(exact_claim.contains("request_service_worker_claim(worker).await"));
    assert!(exact_claim.contains("service_worker_forwarder_ready().await"));
    assert!(exact_claim.contains("controlled_service_worker()"));
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
