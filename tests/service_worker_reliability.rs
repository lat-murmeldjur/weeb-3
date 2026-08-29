#[path = "support/source.rs"]
pub mod source;

use source::{assert_in_order, between};

const LIBRARY: &str = include_str!("../src/library.rs");
const RUNTIME: &str = include_str!("../src/interface_runtime_conventions.rs");
const SERVER: &str = include_str!("../src/main.rs");
const WORKER: &str = include_str!("../static/service.js");
const BUILD: &str = include_str!("../build.rs");
const NPM_WORKFLOW: &str = include_str!("../.github/workflows/plain.yml");
const NPM_README: &str = include_str!("../README.npm.md");

fn playback_readiness() -> &'static str {
    between(
        RUNTIME,
        "async fn wait_for_service_worker_control(",
        "pub(crate) async fn service_worker_controls_bzz_requests(",
    )
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
    assert!(BUILD.contains("cargo:rustc-env=WEEB3_ASSET_VERSION={version}"));
    assert!(BUILD.contains("cargo:rerun-if-changed=static/snippets"));
    assert!(
        NPM_WORKFLOW.find("wasm-pack build").unwrap()
            < NPM_WORKFLOW.find("cargo build --verbose").unwrap(),
        "the native server must embed the freshly generated unified Wasm"
    );
    assert!(SERVER.contains("use axum::http::header::{"));
    for header in [
        "ACCEPT_ENCODING",
        "CACHE_CONTROL",
        "CONTENT_ENCODING",
        "CONTENT_TYPE",
        "ETAG",
        "IF_NONE_MATCH",
        "RANGE",
        "VARY",
    ] {
        assert!(SERVER.contains(header), "missing HTTP header {header}");
    }
    assert!(
        SERVER
            .contains("const EMBEDDED_ASSET_BUILD_VERSION: &str = env!(\"WEEB3_ASSET_VERSION\");")
    );
    assert!(SERVER.contains(
        "const EMBEDDED_ASSET_ETAG: &str = concat!(\"\\\"\", env!(\"WEEB3_ASSET_VERSION\"), \"\\\"\");"
    ));
    assert!(SERVER.contains("const REVALIDATE_EMBEDDED_ASSET: &str = \"private, no-cache\";"));
    assert!(SERVER.contains("HeaderName::from_static(\"x-weeb3-build-version\")"));
    assert!(SERVER.contains("fn html_response(path: &str)"));

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
    assert!(static_file.contains("matches!(path, \"service.js\" | \"worker.js\")"));
    assert!(static_file.contains("(CACHE_CONTROL, \"no-store\")"));
    assert!(static_file.contains("embedded_asset_response(&headers, path, content_type)"));

    let snippets = between(SERVER, "async fn get_static_snippet(", "async fn get_404(");
    assert!(snippets.contains("embedded_asset_response("));
}

#[test]
fn npm_attach_starts_the_shared_runtime_before_hls() {
    let npm_attach = between(
        LIBRARY,
        "pub async fn attach_stream(",
        "#[wasm_bindgen(js_name = networkState)]",
    );
    assert!(
        npm_attach.find("self.boot_runtime()").unwrap()
            < npm_attach
                .find("crate::stream_hls::attach_hls_feed_player(")
                .unwrap()
    );
}

#[test]
fn npm_release_contains_the_worker_required_by_the_runtime_protocol() {
    assert!(NPM_WORKFLOW.contains("files[5]=\"service.js\""));
    assert!(NPM_WORKFLOW.contains("'exports[./service.js].default=./service.js'"));
    assert!(NPM_WORKFLOW.contains("npm pack ./static"));
    assert!(NPM_README.contains("serve the packaged worker at `/weeb-3/service.js`"));
}

#[test]
fn playback_readiness_updates_before_accepting_a_controller() {
    let nonblocking_setup = between(
        RUNTIME,
        "fn start_service_worker_setup_if_idle()",
        "async fn get_service_worker_locked(",
    );
    assert!(nonblocking_setup.contains("SERVICE_WORKER_SETUP_LOCK.try_lock()"));
    assert!(nonblocking_setup.contains("spawn_local(async move"));
    assert!(nonblocking_setup.contains("get_service_worker_locked(&service0).await"));
    assert!(RUNTIME.contains("fn start_service_worker_setup_if_idle() -> bool"));
    assert!(nonblocking_setup.contains("return false;"));
    assert!(nonblocking_setup.trim_end().ends_with("true\n}"));

    let readiness = playback_readiness();
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
    let readiness = playback_readiness();
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
    assert!(WORKER.contains(r#"const SERVICE_WORKER_MARKER = "forwarder-default29";"#));
    assert!(WORKER.contains("const SERVICE_WORKER_PROTOCOL = 10;"));
    assert!(RUNTIME.contains(r#"const SERVICE_WORKER_MARKER: &str = "forwarder-default29";"#));
    assert!(RUNTIME.contains("const SERVICE_WORKER_PROTOCOL: f64 = 10.0;"));
    assert!(WORKER.contains("event.waitUntil(self.skipWaiting())"));
    assert!(WORKER.contains("event.waitUntil(self.clients.claim())"));
    assert!(WORKER.contains("type: \"WEEB3_PONG\""));
    assert!(WORKER.contains("protocol: SERVICE_WORKER_PROTOCOL"));
    assert_eq!(WORKER.matches("marker: SERVICE_WORKER_MARKER").count(), 2);
    assert!(RUNTIME.contains("number_property(&data, \"protocol\")"));
    assert!(RUNTIME.contains("string_property(&data, \"marker\")"));
    assert!(RUNTIME.contains("marker == expected_marker"));
    assert!(WORKER.contains("event.data?.protocol !== SERVICE_WORKER_PROTOCOL"));
    assert!(!WORKER.contains("source.navigate("));
    assert!(WORKER.contains("scope: SCOPE_PATH"));

    let readiness = playback_readiness();
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
        "function isStableWindowClient(",
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

    let one_shot = between(
        WORKER,
        "function responseBodyStream(",
        "function responseHeaders(",
    );
    assert!(one_shot.contains("const bytes = toUint8Array(body);"));
    assert!(one_shot.contains("controller.enqueue(bytes);"));
    assert!(one_shot.contains("controller.close();"));

    let forward = between(
        WORKER,
        "async function forwardRequestToRust(",
        "function parseUploadRedundancyHeader(",
    );
    assert!(!forward.contains("if (hlsResource && response.stream)"));
    assert!(forward.contains(": responseBodyStream(response.body)"));
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
fn persistent_runtime_port_never_changes_upload_identity_or_replays_fetches() {
    let direct = between(
        WORKER,
        "function messageRuntimePort(",
        "function messageRuntime(",
    );
    assert_in_order(
        direct,
        &[
            "timed-out dispatched request is detached",
            "invalidateRuntimePort(runtime)",
            "runtime.port.postMessage(message, [port])",
            "messageClient(runtime.fallback, message, timeoutMs)",
        ],
    );
    assert!(!direct.contains("requestRuntime("));
    let channel_request = between(
        WORKER,
        "function messageChannelRequest(",
        "function messageClient(",
    );
    assert!(channel_request.contains("closeMessagePort(channel.port1)"));
    assert!(channel_request.contains("send(channel.port2)"));

    let close = between(
        WORKER,
        "function closeMessagePort(",
        "function errorResult(",
    );
    assert!(close.contains("port.onmessage = null;"));
    assert!(close.contains("port.onmessageerror = null;"));

    let binding = between(
        WORKER,
        "function bindRuntimePort(",
        "async function requestRuntime(",
    );
    assert!(binding.contains("candidate.port.addEventListener(\"close\", invalidate)"));

    let routing = between(
        WORKER,
        "function messageRuntime(runtime, message",
        "function requestRustFetch(",
    );
    assert!(routing.contains("Number(response?.status) === 409"));
    assert!(routing.contains("invalidateRuntimePort(runtime)"));
    assert!(!routing.contains("messageRuntime("));

    let upload = between(WORKER, "async function forwardUploadToRust(", "\n}");
    assert!(upload.contains("requestClient(networkId, clientId, resultingClientId, false)"));
    assert!(!upload.contains("requestRuntime("));
    assert!(upload.contains("type: \"UPLOAD_REQUEST\""));
}

#[test]
fn network_switch_reprobes_the_stable_window_before_rebinding() {
    let matching = between(
        WORKER,
        "async function windowMatchesNetwork(",
        "async function originatingWindow(",
    );
    assert!(matching.contains("const hadCachedNetwork = cachedWindowNetworks.has(client.id)"));
    assert!(matching.contains("if (!hadCachedNetwork)"));
    assert!(matching.contains("await windowNetworkId(client) === requiredNetworkId"));
    assert!(matching.contains("invalidateWindowClient(client)"));
    assert_eq!(matching.matches("windowNetworkId(client)").count(), 2);

    let selection = between(
        WORKER,
        "async function requestClient(",
        "function bindRuntimePort(",
    );
    assert!(selection.contains("windowMatchesNetwork(originating, requiredNetworkId)"));
    assert!(selection.contains("candidates.map((client) => windowMatchesNetwork("));
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
    assert_eq!(request.matches("messageRuntime(client,").count(), 1);

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
    assert_in_order(
        pull,
        &[
            "scheduleMore();",
            "scheduled.get(start)",
            "await pending",
            "scheduled.delete(start)",
            "controller.enqueue(body)",
        ],
    );

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
    assert_in_order(
        admission,
        &["requestRustRange(", "scheduled.set(start, request)"],
    );

    let close_admission = between(
        stream,
        "const closeAdmission = () => {",
        "const failStream = async",
    );
    assert!(close_admission.contains("admissionOpen = false;"));

    let cancel = between(stream, "cancel() {", "\n    }");
    assert_in_order(
        cancel,
        &["closeAdmission();", "return drainScheduledRanges();"],
    );
    assert!(!cancel.contains("requestRustRange("));
    assert!(!cancel.contains(".abort("));

    let pull = between(stream, "async pull(controller) {", "cancel() {");
    assert_in_order(
        pull,
        &[
            "const response = await pending;",
            "if (!admissionOpen) {",
            "controller.enqueue(body);",
            "scheduleMore();",
        ],
    );
    let normal_close = between(pull, "if (position >= size) {", "scheduleMore();");
    assert_in_order(
        normal_close,
        &[
            "closeAdmission();",
            "controller.close();",
            "await drainScheduledRanges();",
        ],
    );
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
    assert_in_order(
        failure,
        &[
            "closeAdmission();",
            "controller.error(error);",
            "await drainScheduledRanges();",
        ],
    );

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
    assert_in_order(
        setup,
        &[
            "expected_service_worker_registration(",
            "registration.update()",
            "claim_service_worker_registration(",
            "register_with_options(",
        ],
    );
    let expected_url = setup.find("let (expected_worker_url").unwrap();
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
fn npm_missing_worker_diagnostics_do_not_require_interface_dom() {
    let missing = between(
        RUNTIME,
        "pub(crate) fn service_worker_missing()",
        "pub(super) fn render_text_result(",
    );
    assert!(missing.contains("web_sys::console::warn_1"));
    assert!(missing.contains("get_element_by_id(\"resultField\")"));
    assert_in_order(
        missing,
        &[
            "get_element_by_id(\"resultField\")",
            "SERVICE_WORKER_MISSING_VISIBLE.with",
        ],
    );
    assert!(!missing.contains("expect("));
    assert!(!missing.contains("unwrap()"));
}
