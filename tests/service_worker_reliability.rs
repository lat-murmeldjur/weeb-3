#[path = "support/source.rs"]
pub mod source;
#[path = "support/browser.rs"]
mod browser;

use source::{assert_contains, assert_excludes, assert_first_in_order, assert_in_order, between};

const LIBRARY: &str = include_str!("../src/library.rs");
const RUNTIME: &str = include_str!("../src/interface_runtime_conventions.rs");
const SERVER: &str = include_str!("../src/main.rs");
const WORKER: &str = include_str!("../static/service.js");
const BUILD: &str = include_str!("../build.rs");
const NPM_WORKFLOW: &str = include_str!("../.github/workflows/plain.yml");
const NPM_README: &str = include_str!("../README.npm.md");

fn worker_function(name: &str) -> &'static str {
    between(WORKER, &format!("function {name}("), "\n}")
}

fn runtime_function(name: &str) -> &'static str {
    between(RUNTIME, &format!("fn {name}("), "\n}")
}

fn playback_readiness() -> &'static str {
    runtime_function("wait_for_service_worker_control")
}

fn range_stream_pull() -> &'static str {
    between(worker_function("createRustRangeStream"), "async pull(controller) {", "cancel() {")
}

fn assert_stream_drains(body: &str, terminal: &str) {
    assert_in_order(body, &["admissionOpen = false;", terminal, "await drainScheduledRanges();"]);
}

#[test]
fn native_server_rebuilds_and_revalidates_every_embedded_browser_runtime_asset() {
    let source_version = between(BUILD, "fn source_build_version()", "fn asset_build_version()");
    for asset in ["static/weeb_3.js", "static/weeb_3_bg.wasm", "static/snippets"] {
        assert!(
            !source_version.contains(asset),
            "generated asset {asset} must not feed the version embedded into Wasm"
        );
    }
    assert_contains(BUILD, &[
        "collect_all_files(Path::new(\"static/snippets\"), &mut files);",
        "CARGO_CFG_TARGET_ARCH",
        "cargo:rustc-env=WEEB3_ASSET_VERSION={version}",
        "cargo:rerun-if-changed=static/snippets",
    ]);
    // The native server must embed the freshly generated unified Wasm.
    assert_first_in_order(NPM_WORKFLOW, &["wasm-pack build", "cargo build --verbose"]);
    assert_contains(SERVER, &[
        "use axum::http::header::{",
        "ACCEPT_ENCODING",
        "CACHE_CONTROL",
        "CONTENT_ENCODING",
        "CONTENT_TYPE",
        "ETAG",
        "IF_NONE_MATCH",
        "RANGE",
        "VARY",
        "const EMBEDDED_ASSET_BUILD_VERSION: &str = env!(\"WEEB3_ASSET_VERSION\");",
        "const EMBEDDED_ASSET_ETAG: &str = concat!(\"\\\"\", env!(\"WEEB3_ASSET_VERSION\"), \"\\\"\");",
        "const REVALIDATE_EMBEDDED_ASSET: &str = \"private, no-cache\";",
        "HeaderName::from_static(\"x-weeb3-build-version\")",
        "fn html_response(path: &str)",
    ]);

    let validator = between(SERVER, "fn embedded_asset_is_current(", "fn embedded_asset_response(");
    assert_contains(validator, &[
        ".get_all(IF_NONE_MATCH)",
        "value.strip_prefix(\"W/\").unwrap_or(value)",
        "== EMBEDDED_ASSET_ETAG",
    ]);

    let response = between(SERVER, "fn embedded_asset_response(", "async fn get_static_file(");
    assert_contains(response, &[
        "(CACHE_CONTROL, REVALIDATE_EMBEDDED_ASSET)",
        "(ETAG, EMBEDDED_ASSET_ETAG)",
        "StatusCode::NOT_MODIFIED",
    ]);

    let static_file = between(SERVER, "async fn get_static_file(", "async fn get_static_snippet(");
    assert_contains(static_file, &[
        "matches!(path, \"service.js\" | \"worker.js\")",
        "(CACHE_CONTROL, \"no-store\")",
        "embedded_asset_response(&headers, path, content_type)",
    ]);

    let snippets = between(SERVER, "async fn get_static_snippet(", "async fn get_404(");
    assert!(snippets.contains("embedded_asset_response("));
}

#[test]
fn npm_attach_starts_the_shared_runtime_before_hls() {
    let npm_attach = between(LIBRARY, "pub async fn attach_stream(", "#[wasm_bindgen(js_name = networkState)]");
    assert_first_in_order(npm_attach, &["self.boot_runtime()", "crate::stream_hls::attach_hls_feed_player("]);
}

#[test]
fn npm_release_contains_the_worker_required_by_the_runtime_protocol() {
    assert_contains(NPM_WORKFLOW, &[
        "files[5]=\"service.js\"",
        "'exports[./service.js].default=./service.js'",
        "npm pack ./static",
    ]);
    assert!(NPM_README.contains("Serve the packaged worker at `/weeb-3/service.js`"));
}

#[test]
fn playback_readiness_updates_before_accepting_a_controller() {
    assert_contains(runtime_function("get_service_worker"), &[
        "SERVICE_WORKER_SETUP_LOCK.lock().await",
        "Object::is(worker.as_ref(), controller.as_ref())",
        "get_service_worker_locked(&service0).await",
    ]);
    assert!(playback_readiness().contains("get_service_worker().await.is_some()"));
}

#[test]
fn busy_or_failed_setup_remains_retryable_without_overlap() {
    let readiness = playback_readiness();
    assert_contains(readiness, &[
        "while still_needed()",
        "timeout(SERVICE_WORKER_SETUP_RETRY, changed.recv()).await",
        "\"controllerchange\"",
        "DomListener::new(&container, \"controllerchange\"",
    ]);
    let listener = include_str!("../src/worker_protocol.rs");
    assert!(between(listener, "impl Drop for DomListener", "pub(crate) struct ReplyChannel")
        .contains("remove_event_listener_with_callback"));
    assert_excludes(readiness, &["task::sleep", "Date::now"]);
}

#[test]
fn readiness_requires_a_controlling_protocol_worker() {
    assert_contains(WORKER, &[
        r#"const SERVICE_WORKER_MARKER = "forwarder-default29";"#,
        "const SERVICE_WORKER_PROTOCOL = 10;",
        "event.waitUntil(self.skipWaiting())",
        "event.waitUntil(self.clients.claim())",
        "type: type === \"WEEB3_CLAIM\" ? \"WEEB3_CLAIMED\" : \"WEEB3_PONG\"",
        "protocol: SERVICE_WORKER_PROTOCOL",
        "event.data?.protocol !== SERVICE_WORKER_PROTOCOL",
        "scope: SCOPE_PATH",
    ]);
    assert_eq!(WORKER.matches("marker: SERVICE_WORKER_MARKER").count(), 1);
    assert_contains(RUNTIME, &[
        r#"const SERVICE_WORKER_MARKER: &str = "forwarder-default29";"#,
        "const SERVICE_WORKER_PROTOCOL: f64 = 10.0;",
        "number_property(&data, \"protocol\")",
        "string_property(&data, \"marker\")",
        "marker == SERVICE_WORKER_MARKER",
    ]);
    assert!(!WORKER.contains("source.navigate("));

    let readiness = playback_readiness();
    assert_excludes(readiness, &[
        "registration.active()",
        "registration.waiting()",
        "registration.installing()",
    ]);
}

#[test]
fn hls_routes_own_their_stream_windows_and_preserve_http_validators() {
    let route_constants = between(WORKER, "const NETWORK_ROUTE_PREFIXES", "const FETCH_TIMEOUT_MS");
    assert_contains(route_constants, &["[\"\", \"mainnet/\", \"testnet/\"]", "[\"hls/bytes\", \"hls-bytes\"]"]);

    let fetch_routes = between(WORKER, "self.addEventListener(\"fetch\"", "function isStableWindowClient(");
    assert_contains(fetch_routes, &[
        "canonicalRawResource(url)",
        "canonicalFeedResource(url)",
        "request.method === \"GET\" || request.method === \"HEAD\"",
    ]);

    let request = worker_function("requestRustFetch");
    assert_contains(request, &[
        "type: \"WEEB3_FETCH_REQUEST\"",
        "method",
        "range",
        "networkId",
        "ifNoneMatch",
        "ifRange",
    ]);

    assert_contains(worker_function("responseBodyStream"), &[
        "const bytes = toUint8Array(body);",
        "controller.enqueue(bytes);",
        "controller.close();",
    ]);

    let forward = worker_function("forwardRequestToRust");
    assert!(!forward.contains("if (hlsResource && response.stream)"));
    assert_contains(forward, &[
        ": responseBodyStream(response.body)",
        "if (response.stream && request.method !== \"HEAD\")",
        "hlsResource ? \"\" : request.headers.get(\"Range\")",
        "request.headers.get(\"If-None-Match\")",
        "hlsResource ? \"\" : request.headers.get(\"If-Range\")",
        "request.method === \"HEAD\" || status === 304",
        "const headers = new Headers(response.headers);",
    ]);

    assert_excludes(WORKER, &[
        "HLS_STREAM_READY_ADMISSION_THRESHOLD",
        "HLS_STREAM_MAX_OUTSTANDING",
        "HLS_REQUEST_FLIGHTS",
        "parseCriticalPrefixWindows",
        "streamToken",
        "X-Weeb3-Stream-Start",
        "X-Weeb3-Stream-Token",
        "X-Weeb3-HLS-Critical-Prefix-Windows",
    ]);
}

#[test]
fn persistent_runtime_port_never_changes_upload_identity_or_replays_fetches() {
    let direct = worker_function("messageRuntimePort");
    assert_in_order(direct, &[
        "timed-out dispatched request is detached",
        "invalidateRuntimePort(runtime)",
        "runtime.port.postMessage(message, [port])",
        "messageClient(runtime.fallback, message, timeoutMs)",
    ]);
    assert!(!direct.contains("requestRuntime("));
    assert_contains(worker_function("messageChannelRequest"), &["closeMessagePort(channel.port1)", "send(channel.port2)"]);

    let close = worker_function("closeMessagePort");
    assert_contains(close, &["port.onmessage = null;", "port.onmessageerror = null;"]);

    assert!(worker_function("bindRuntimePort").contains("candidate.port.addEventListener(\"close\", invalidate)"));

    let routing = worker_function("messageRuntime");
    assert_contains(routing, &["Number(response?.status) === 409", "invalidateRuntimePort(runtime)"]);
    assert!(!routing.contains("messageRuntime("));

    let upload = worker_function("forwardUploadToRust");
    assert_contains(upload, &["requestClient(networkId, clientId, resultingClientId, false)", "type: \"UPLOAD_REQUEST\""]);
    assert!(!upload.contains("requestRuntime("));
}

#[test]
fn network_switch_reprobes_the_stable_window_before_rebinding() {
    let matching = worker_function("windowMatchesNetwork");
    assert_contains(matching, &[
        "const hadCachedNetwork = cachedWindowNetworks.has(client.id)",
        "if (!hadCachedNetwork)",
        "await windowNetworkId(client) === requiredNetworkId",
        "invalidateWindowClient(client)",
    ]);
    assert_eq!(matching.matches("windowNetworkId(client)").count(), 2);

    assert_contains(worker_function("requestClient"), &[
        "windowMatchesNetwork(originating, requiredNetworkId)",
        "candidates.map((client) => windowMatchesNetwork(",
    ]);
}

#[test]
fn generic_range_stream_keeps_ordered_bounded_lookahead() {
    assert_contains(WORKER, &[
        "const STREAM_WINDOW_BYTES = MIB_BYTES / 2;",
        "const STREAM_LOOKAHEAD_CHUNKS = 8;",
        "const HLS_STREAM_LOOKAHEAD_CHUNKS = 4;",
        "const RANGE_REQUEST_FLIGHTS = new Map();",
    ]);

    let request = worker_function("requestRustRange");
    assert_contains(request, &[
        "range: `bytes=${start}-${end}`",
        "body.byteLength !== expected",
        "RANGE_REQUEST_FLIGHTS.get(key)",
        "RANGE_REQUEST_FLIGHTS.set(key, request)",
        "RANGE_REQUEST_FLIGHTS.delete(key)",
    ]);
    assert_eq!(request.matches("messageRuntime(client,").count(), 1);

    let stream = worker_function("createRustRangeStream");
    assert!(stream.contains("const scheduled = new Map();"));
    let scheduler = between(
        stream,
        "const scheduleMore = () => {",
        "const drainScheduledRanges",
    );
    assert_contains(scheduler, &["scheduled.size < lookahead", "scheduled.set(start, requestRustRange("]);

    let pull = range_stream_pull();
    assert_in_order(pull, &[
        "scheduleMore();",
        "await scheduled.get(start)",
        "scheduled.delete(start)",
        "controller.enqueue(body)",
    ]);

    let forward = worker_function("forwardRequestToRust");
    assert_contains(forward, &[
        "Number.isSafeInteger(size) || size <= 0",
        "STREAM_WINDOW_BYTES,",
        "hlsResource ? HLS_STREAM_LOOKAHEAD_CHUNKS : STREAM_LOOKAHEAD_CHUNKS",
    ]);
    assert_excludes(forward, &["url.searchParams.get(\"startup\")", "beginningHlsResource"]);
}

#[test]
fn hls_stream_admits_four_windows_before_waiting_and_refills_after_emitting() {
    assert_excludes(worker_function("createRustRangeStream"), &["gateFirstWindow", "initialLookahead"]);
    let pull = range_stream_pull();
    let first_schedule = pull.find("scheduleMore();").unwrap();
    let awaited = pull.find("await scheduled.get(start)").unwrap();
    let emitted = pull.find("controller.enqueue(body);").unwrap();
    let refill = pull.rfind("scheduleMore();").unwrap();
    assert!(first_schedule < awaited && awaited < emitted && emitted < refill);
}

#[test]
fn generic_range_stream_cancel_closes_admission_and_drains_dispatched_promises() {
    let stream = worker_function("createRustRangeStream");
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
    assert_in_order(admission, &[
        "const start = schedulePosition;",
        "const end = Math.min(start + windowBytes - 1, size - 1);",
        "schedulePosition = end + 1;",
        "scheduled.set(start, requestRustRange(",
    ]);

    let cancel = between(stream, "cancel() {", "\n    }");
    assert_in_order(
        cancel,
        &["admissionOpen = false;", "return drainScheduledRanges();"],
    );
    assert_excludes(cancel, &["requestRustRange(", ".abort("]);

    let pull = range_stream_pull();
    assert_in_order(pull, &[
        "const response = await scheduled.get(start);",
        "if (!admissionOpen) {",
        "controller.enqueue(body);",
        "scheduleMore();",
    ]);
    let normal_close = between(pull, "if (position >= size) {", "scheduleMore();");
    assert_stream_drains(normal_close, "controller.close();");
}

#[test]
fn generic_range_stream_errors_close_and_drain_before_returning() {
    let failure = between(
        worker_function("createRustRangeStream"),
        "const failStream = async (controller, error) => {",
        "return new ReadableStream(",
    );
    assert_stream_drains(failure, "controller.error(error);");

    let pull = range_stream_pull();
    assert!(pull.matches("await failStream(").count() >= 2);
    assert!(pull.contains("await failStream(controller, error);"));
}

#[test]
fn setup_validates_scope_and_every_registration_state() {
    assert_contains(runtime_function("expected_service_worker_registration"), &[
        "registration.scope() != expected_scope_url",
        "registration.active()",
        "registration.waiting()",
        "registration.installing()",
    ]);

    let setup = runtime_function("get_service_worker_locked");
    assert_in_order(setup, &[
        "expected_service_worker_registration(",
        "registration.update()",
        "claim_service_worker_registration(",
        "register_with_options(",
    ]);
    let expected_url = setup.find("let (expected_worker_url").unwrap();
    assert!(!setup[..expected_url].contains("service_worker_forwarder_ready"));
    assert_excludes(setup, &["or(Some(active))", "return Some(service_worker)", "unregister("]);
    assert_in_order(runtime_function("claim_service_worker_registration"), &[
        "expected_service_worker_registration(",
        "registration.active()",
        "service_worker_protocol_request(&active, \"WEEB3_CLAIM\", \"WEEB3_CLAIMED\", 1_500).await",
        "Some(SERVICE_WORKER_SCOPE.with(Cell::get))",
        "return Ok(None)",
        "service_worker_forwarder_ready_with_timeout(1_500).await",
    ]);
    assert_in_order(runtime_function("service_worker_forwarder_ready_with_timeout"), &[
        "let controller = controlled_service_worker()?",
        "service_worker_protocol_request(&controller",
        "let configured = SERVICE_WORKER_SCOPE.with(Cell::get)",
        "configured == \"/\" && scope == STREAMING_SERVICE_WORKER_SCOPE",
        "controller.script_url() == configured_service_worker_urls()?.0",
        ".pathname().ok()?.starts_with(STREAMING_SERVICE_WORKER_SCOPE)",
        "Object::is(controller.as_ref(), current.as_ref())",
        "Some(controller)",
    ]);
    assert_contains(setup, &["JsValue::from_str(\"updateViaCache\")", "JsValue::from_str(\"none\")"]);
}

#[test]
fn npm_missing_worker_diagnostics_do_not_require_interface_dom() {
    let missing = runtime_function("service_worker_missing");
    assert_contains(missing, &["web_sys::console::warn_1", "get_element_by_id(\"resultField\")"]);
    assert_in_order(missing, &[
        "get_element_by_id(\"resultField\")",
        "SERVICE_WORKER_MISSING_VISIBLE.with",
    ]);
    assert_excludes(missing, &["expect(", "unwrap()"]);
}

#[test]
fn optional_browser_embedding_preserves_api_paths_under_both_scopes() -> anyhow_crates_io::Result<()> {
    use headless_chrome::protocol::cdp::Page::AddScriptToEvaluateOnNewDocument;
    use headless_chrome::protocol::cdp::{Network::Enable, types::Event};
    use std::{env, sync::{Arc, Mutex}, time::Duration};
    let Ok(origin) = env::var("WEEB3_EMBED_URL") else { return Ok(()) };
    // A deterministic relay isolates browser interception from Swarm availability.
    // Registration and scope configuration still use the served npm Wasm client.
    let relay = r#"
      navigator.serviceWorker.addEventListener('message', event => {
        const message = event.data, port = event.ports[0];
        if (message?.type === 'WEEB3_CLIENT_PING') {
          event.stopImmediatePropagation();
          port.postMessage({ok: true, type: 'WEEB3_CLIENT_PONG', networkId: 1, sharedWorkerProtocol: 5});
        } else if (message?.type === 'WEEB3_RUNTIME_PORT_BIND') {
          event.stopImmediatePropagation();
          port.onmessage = request => {
            const probe = request.data?.url?.endsWith('/chunks/embedded-scope-probe');
            request.ports[0].postMessage({ok: probe, status: probe ? 200 : 404,
              headers: {'Content-Type': 'text/plain'}, body: new TextEncoder().encode('scope probe')});
          };
          port.start();
          port.postMessage({ok: true, type: 'WEEB3_RUNTIME_PORT_READY', networkId: 1, protocol: 5});
        }
      });
    "#;
    let timeout = Duration::from_secs(30);
    let browser = browser::launch(timeout, false)?;
    for (index, (path, scope)) in [
        ("/", "/"),
        ("/embed/7f492a/player", "/"),
        ("/weeb-3/hls-stream-example.html", "/weeb-3/"),
        ("/", "/"),
        ("/weeb-3/hls-stream-example.html", "/"),
    ].into_iter().enumerate() {
        let tab = browser.new_tab()?;
        tab.set_default_timeout(timeout);
        tab.call_method(Enable {
            max_total_buffer_size: Some(0), max_resource_buffer_size: Some(0), max_post_data_size: None,
            report_direct_socket_traffic: None, enable_durable_messages: None,
        })?;
        let assets = Arc::new(Mutex::new(Vec::new()));
        let observed = assets.clone();
        let _listener = tab.add_event_listener(Arc::new(move |event: &Event| {
            if let Event::NetworkResponseReceived(event) = event
                && event.params.response.url.contains("/host-assets/") {
                observed.lock().unwrap().push(event.params.response.from_service_worker);
            }
        }))?;
        tab.call_method(AddScriptToEvaluateOnNewDocument {
            source: relay.into(), world_name: None,
            include_command_line_api: None, run_immediately: None,
        })?;
        tab.navigate_to(&format!("{}{path}", origin.trim_end_matches('/')))?.wait_until_navigated()?;
        let raw = tab.evaluate(&r#"
          (async () => {
            const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
            const {default: init, Weeb3No103} = await import('/weeb-3/weeb_3.js');
            while (document.getElementById('status').textContent.startsWith('Loading')) {
              await delay(50);
            }
            await init();
            const expectedScope = EXPECTED_SCOPE;
            let acceptedInvalidScope = false;
            try { new Weeb3No103(undefined, '/other/'); acceptedInvalidScope = true; } catch {}
            await Promise.race([navigator.serviceWorker.ready,
              new Promise((_, reject) => setTimeout(() => reject(new Error('worker activation timed out')), 20000))]);
            async function waitForScope(scope) {
              let pong;
              for (let attempt = 0; attempt < 100 && pong?.scope !== scope; attempt++) {
                const channel = new MessageChannel();
                const reply = new Promise(resolve => channel.port1.onmessage = event => resolve(event.data));
                navigator.serviceWorker.controller?.postMessage({type: 'WEEB3_PING', protocol: 10}, [channel.port2]);
                pong = await Promise.race([reply, delay(200)]);
                channel.port1.close();
                if (pong?.scope !== scope) await delay(50);
              }
              return pong;
            }
            const narrowPage = location.pathname.startsWith('/weeb-3/');
            if (expectedScope === '/' && narrowPage) {
              if ((await waitForScope('/weeb-3/'))?.scope !== '/weeb-3/') throw new Error('narrow controller missing');
              new Weeb3No103(undefined, '/').start();
            }
            const pong = await waitForScope(narrowPage ? '/weeb-3/' : expectedScope);
            const api = await fetch('/weeb-3/chunks/embedded-scope-probe');
            const unrelated = await fetch('/chunks/embedded-scope-probe');
            const fixtures = [
              ['html', 'text/html', '<p>host</p>'], ['js', 'text/javascript', 'void 0;'],
              ['css', 'text/css', 'body{}'], ['json', 'application/json', '{"host":1}'],
              ['png', 'image/png', atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==')]
            ];
            for (const prefix of ['/host-assets/', '/weeb-3/host-assets/']) {
              for (const [kind, mime, body] of fixtures) {
                const path = `${prefix}probe.${kind}?q=%2Fweeb-3%2F&kind=${kind}`;
                const response = await fetch(path);
                const bytes = new Uint8Array(await response.arrayBuffer());
                if (response.status !== 203 || response.headers.get('Content-Type') !== mime ||
                    response.headers.get('X-Host-Query') !== path || String.fromCharCode(...bytes) !== body) {
                  throw new Error(`host asset changed: ${path}`);
                }
              }
            }
            const scopes = (await navigator.serviceWorker.getRegistrations()).map(registration => new URL(registration.scope).pathname);
            return JSON.stringify({scope: pong?.scope, scopes, acceptedInvalidScope, status: api.status, body: await api.text(), unrelated: await unrelated.text()});
          })()
        "#.replace("EXPECTED_SCOPE", &serde_json::to_string(scope)?), true)?.value.expect("scope probe returned no value");
        let result: serde_json::Value = serde_json::from_str(raw.as_str().expect("scope probe omitted JSON"))?;
        for (field, expected) in serde_json::json!({
            "scope": if index == 4 { "/weeb-3/" } else { scope },
            "acceptedInvalidScope": false, "status": 200, "body": "scope probe",
        }).as_object().unwrap() {
            assert_eq!(&result[field], expected, "{path}: {result}");
        }
        assert_ne!(result["unrelated"], "scope probe", "{path}: {result}");
        let assets = assets.lock().unwrap();
        assert_eq!(assets.len(), 10, "host assets did not reach the server: {path}");
        assert!(assets.iter().all(|intercepted| *intercepted != Some(true)), "host asset intercepted: {path}");
        if index >= 2 {
            let scopes = result["scopes"].as_array().unwrap();
            for scope in ["/", "/weeb-3/"] {
                assert!(scopes.contains(&serde_json::json!(scope)), "{path}: {result}");
            }
        }
    }
    Ok(())
}
