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
    let source_version = crate::source::between(
        BUILD,
        "fn source_build_version()",
        "fn asset_build_version()",
    );
    for asset in ["static/weeb_3.js", "static/weeb_3_bg.wasm"] {
        assert!(
            !source_version.contains(asset),
            "generated asset {asset} must not feed the version embedded into Wasm"
        );
    }
    assert!(!source_version.contains("static/snippets"));
    crate::source::assert_contains(BUILD, &[
        "collect_all_files(Path::new(\"static/snippets\"), &mut files);",
        "CARGO_CFG_TARGET_ARCH",
        "cargo:rustc-env=WEEB3_ASSET_VERSION={version}",
        "cargo:rerun-if-changed=static/snippets",
    ]);
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
    crate::source::assert_contains(SERVER, &[
        "const EMBEDDED_ASSET_ETAG: &str = concat!(\"\\\"\", env!(\"WEEB3_ASSET_VERSION\"), \"\\\"\");",
        "const REVALIDATE_EMBEDDED_ASSET: &str = \"private, no-cache\";",
        "HeaderName::from_static(\"x-weeb3-build-version\")",
        "fn html_response(path: &str)",
    ]);

    let validator = between(
        SERVER,
        "fn embedded_asset_is_current(",
        "fn embedded_asset_response(",
    );
    crate::source::assert_contains(validator, &[
        ".get_all(IF_NONE_MATCH)",
        "value.strip_prefix(\"W/\").unwrap_or(value)",
        "== EMBEDDED_ASSET_ETAG",
    ]);

    let response = between(
        SERVER,
        "fn embedded_asset_response(",
        "async fn get_static_file(",
    );
    crate::source::assert_contains(response, &[
        "(CACHE_CONTROL, REVALIDATE_EMBEDDED_ASSET)",
        "(ETAG, EMBEDDED_ASSET_ETAG)",
        "StatusCode::NOT_MODIFIED",
    ]);

    let static_file = between(
        SERVER,
        "async fn get_static_file(",
        "async fn get_static_snippet(",
    );
    crate::source::assert_contains(static_file, &[
        "matches!(path, \"service.js\" | \"worker.js\")",
        "(CACHE_CONTROL, \"no-store\")",
        "embedded_asset_response(&headers, path, content_type)",
    ]);

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
    crate::source::assert_contains(NPM_WORKFLOW, &[
        "files[5]=\"service.js\"",
        "'exports[./service.js].default=./service.js'",
        "npm pack ./static",
    ]);
    assert!(NPM_README.contains("serve the packaged worker at `/weeb-3/service.js`"));
}

#[test]
fn playback_readiness_updates_before_accepting_a_controller() {
    let setup = between(
        RUNTIME,
        "pub async fn get_service_worker()",
        "async fn get_service_worker_locked(",
    );
    crate::source::assert_contains(setup, &[
        "SERVICE_WORKER_SETUP_LOCK.lock().await",
        "Object::is(worker.as_ref(), controller.as_ref())",
        "get_service_worker_locked(&service0).await",
    ]);
    assert!(playback_readiness().contains("get_service_worker().await.is_some()"));
}

#[test]
fn busy_or_failed_setup_remains_retryable_without_overlap() {
    let readiness = playback_readiness();
    crate::source::assert_contains(readiness, &[
        "while still_needed()",
        "timeout(SERVICE_WORKER_SETUP_RETRY, changed.recv()).await",
        "\"controllerchange\"",
        "DomListener::new(&container, \"controllerchange\"",
    ]);
    let listener = include_str!("../src/worker_protocol.rs");
    assert!(between(listener, "impl Drop for DomListener", "pub(crate) struct ReplyChannel")
        .contains("remove_event_listener_with_callback"));
    assert!(!readiness.contains("task::sleep"));
    assert!(!readiness.contains("Date::now"));
}

#[test]
fn readiness_requires_a_controlling_protocol_worker() {
    assert!(WORKER.contains(r#"const SERVICE_WORKER_MARKER = "forwarder-default29";"#));
    assert!(WORKER.contains("const SERVICE_WORKER_PROTOCOL = 10;"));
    assert!(RUNTIME.contains(r#"const SERVICE_WORKER_MARKER: &str = "forwarder-default29";"#));
    assert!(RUNTIME.contains("const SERVICE_WORKER_PROTOCOL: f64 = 10.0;"));
    crate::source::assert_contains(WORKER, &[
        "event.waitUntil(self.skipWaiting())",
        "event.waitUntil(self.clients.claim())",
        "type: type === \"WEEB3_CLAIM\" ? \"WEEB3_CLAIMED\" : \"WEEB3_PONG\"",
        "protocol: SERVICE_WORKER_PROTOCOL",
    ]);
    assert_eq!(WORKER.matches("marker: SERVICE_WORKER_MARKER").count(), 1);
    crate::source::assert_contains(RUNTIME, &[
        "number_property(&data, \"protocol\")",
        "string_property(&data, \"marker\")",
        "marker == SERVICE_WORKER_MARKER",
    ]);
    assert!(WORKER.contains("event.data?.protocol !== SERVICE_WORKER_PROTOCOL"));
    assert!(!WORKER.contains("source.navigate("));
    assert!(WORKER.contains("scope: SCOPE_PATH"));

    let readiness = playback_readiness();
    crate::source::assert_excludes(readiness, &[
        "registration.active()",
        "registration.waiting()",
        "registration.installing()",
    ]);
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
    crate::source::assert_contains(fetch_routes, &[
        "canonicalRawResource(url)",
        "canonicalFeedResource(url)",
        "request.method === \"GET\" || request.method === \"HEAD\"",
    ]);

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
        "function requestRustRange(",
    );
    crate::source::assert_contains(one_shot, &[
        "const bytes = toUint8Array(body);",
        "controller.enqueue(bytes);",
        "controller.close();",
    ]);

    let forward = between(
        WORKER,
        "async function forwardRequestToRust(",
        "function parseUploadRedundancyHeader(",
    );
    assert!(!forward.contains("if (hlsResource && response.stream)"));
    crate::source::assert_contains(forward, &[
        ": responseBodyStream(response.body)",
        "if (response.stream && request.method !== \"HEAD\")",
        "hlsResource ? \"\" : request.headers.get(\"Range\")",
        "request.headers.get(\"If-None-Match\")",
        "hlsResource ? \"\" : request.headers.get(\"If-Range\")",
        "request.method === \"HEAD\" || status === 304",
        "const headers = new Headers(response.headers);",
    ]);

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
    crate::source::assert_contains(matching, &[
        "const hadCachedNetwork = cachedWindowNetworks.has(client.id)",
        "if (!hadCachedNetwork)",
        "await windowNetworkId(client) === requiredNetworkId",
        "invalidateWindowClient(client)",
    ]);
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
    crate::source::assert_contains(WORKER, &[
        "const STREAM_WINDOW_BYTES = MIB_BYTES / 2;",
        "const STREAM_LOOKAHEAD_CHUNKS = 8;",
        "const HLS_STREAM_LOOKAHEAD_CHUNKS = 4;",
        "const RANGE_REQUEST_FLIGHTS = new Map();",
    ]);

    let request = between(
        WORKER,
        "function requestRustRange(",
        "function createRustRangeStream(",
    );
    crate::source::assert_contains(request, &[
        "range: `bytes=${start}-${end}`",
        "body.byteLength !== expected",
        "RANGE_REQUEST_FLIGHTS.get(key)",
        "RANGE_REQUEST_FLIGHTS.set(key, request)",
        "RANGE_REQUEST_FLIGHTS.delete(key)",
    ]);
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
    assert!(scheduler.contains("scheduled.size < lookahead"));
    assert!(scheduler.contains("scheduled.set(start, requestRustRange("));

    let pull = between(stream, "async pull(controller) {", "cancel() {");
    assert_in_order(
        pull,
        &[
            "scheduleMore();",
            "await scheduled.get(start)",
            "scheduled.delete(start)",
            "controller.enqueue(body)",
        ],
    );

    let forward = between(
        WORKER,
        "async function forwardRequestToRust(",
        "function parseUploadRedundancyHeader(",
    );
    crate::source::assert_contains(forward, &[
        "Number.isSafeInteger(size) || size <= 0",
        "STREAM_WINDOW_BYTES,",
        "hlsResource ? HLS_STREAM_LOOKAHEAD_CHUNKS : STREAM_LOOKAHEAD_CHUNKS",
    ]);
    assert!(!forward.contains("url.searchParams.get(\"startup\")"));
    assert!(!forward.contains("beginningHlsResource"));
}

#[test]
fn hls_stream_admits_four_windows_before_waiting_and_refills_after_emitting() {
    let stream = between(
        WORKER,
        "function createRustRangeStream(",
        "async function forwardRequestToRust(",
    );
    assert!(!stream.contains("gateFirstWindow"));
    assert!(!stream.contains("initialLookahead"));
    let pull = between(stream, "async pull(controller) {", "cancel() {");
    let first_schedule = pull.find("scheduleMore();").unwrap();
    let awaited = pull.find("await scheduled.get(start)").unwrap();
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
        "const failStream = async",
    );
    assert_in_order(drain, &[
        "Array.from(scheduled.values(), request => request.then(() => {}))",
        "scheduled.clear();",
        "Promise.allSettled(pending)",
    ]);

    let admission = between(
        stream,
        "const scheduleMore = () => {",
        "const drainScheduledRanges",
    );
    assert!(admission.contains("while (admissionOpen && schedulePosition < size"));
    assert_in_order(
        admission,
        &[
            "const start = schedulePosition;",
            "const end = Math.min(start + windowBytes - 1, size - 1);",
            "schedulePosition = end + 1;",
            "scheduled.set(start, requestRustRange(",
        ],
    );

    let cancel = between(stream, "cancel() {", "\n    }");
    assert_in_order(
        cancel,
        &["admissionOpen = false;", "return drainScheduledRanges();"],
    );
    assert!(!cancel.contains("requestRustRange("));
    assert!(!cancel.contains(".abort("));

    let pull = between(stream, "async pull(controller) {", "cancel() {");
    assert_in_order(
        pull,
        &[
            "const response = await scheduled.get(start);",
            "if (!admissionOpen) {",
            "controller.enqueue(body);",
            "scheduleMore();",
        ],
    );
    let normal_close = between(pull, "if (position >= size) {", "scheduleMore();");
    assert_in_order(
        normal_close,
        &[
            "admissionOpen = false;",
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
            "admissionOpen = false;",
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
    crate::source::assert_contains(validation, &[
        "registration.scope() != expected_scope_url",
        "registration.active()",
        "registration.waiting()",
        "registration.installing()",
    ]);

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
        "async fn claim_service_worker_registration(",
        "fn warn_about_worker_conflict(",
    );
    assert_in_order(exact_claim, &[
        "expected_service_worker_registration(",
        "registration.active()",
        "service_worker_protocol_request(&active, \"WEEB3_CLAIM\", \"WEEB3_CLAIMED\", 1_500).await",
        "return Ok(None)",
        "service_worker_forwarder_ready_with_timeout(1_500).await",
    ]);
    let ready = between(
        RUNTIME,
        "async fn service_worker_forwarder_ready_with_timeout(",
        "async fn service_worker_protocol_request(",
    );
    assert_in_order(
        ready,
        &[
            "let controller = controlled_service_worker()?",
            "service_worker_protocol_request(&controller",
            "Object::is(controller.as_ref(), current.as_ref())",
            "Some(controller)",
        ],
    );
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
