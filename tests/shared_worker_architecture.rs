use std::{env, fs, path::Path, time::Duration};

use anyhow_crates_io::{Result, anyhow};
use serde_json::Value;

#[path = "support/browser.rs"]
mod browser;
#[path = "support/source.rs"]
pub mod source;

use source::{assert_in_order, between as section, compact};

const CARGO: &str = include_str!("../Cargo.toml");
const BUILD: &str = include_str!("../build.rs");
const SERVER: &str = include_str!("../src/main.rs");
const CORE: &str = include_str!("../src/lib.rs");
const INTERFACE: &str = include_str!("../src/interface.rs");
const INDEX: &str = include_str!("../static/index.html");
const INTERFACE_RUNTIME: &str = include_str!("../src/interface_runtime_conventions.rs");
const LIBRARY: &str = include_str!("../src/library.rs");
const ON_CHAIN: &str = include_str!("../src/on_chain.rs");
const SECURE_VAULT: &str = include_str!("../src/secure_vault.rs");
const SHARED_RUNTIME: &str = include_str!("../src/shared_runtime.rs");
const WORKER_RUNTIME: &str = include_str!("../src/worker_runtime.rs");
const SHARED_WORKER: &str = include_str!("../static/worker.js");
const STATIC_IGNORE: &str = include_str!("../static/.gitignore");
const HAXE_BUILD: &str = include_str!("../Code_One.hx");
const NPM_WORKFLOW: &str = include_str!("../.github/workflows/plain.yml");
const TRANSFER_NODE_OPERATIONS: [&str; 8] = [
    "acquire",
    "retrieveBytes",
    "retrieveChunk",
    "acquireFeed",
    "upload",
    "pushChunk",
    "resolveBzz",
    "acquireRange",
];

fn visit_rust_sources(root: &Path, hits: &mut Vec<std::path::PathBuf>, needle: &str) {
    for entry in fs::read_dir(root).unwrap_or_else(|error| panic!("{}: {error}", root.display())) {
        let entry = entry.unwrap_or_else(|error| panic!("{}: {error}", root.display()));
        let path = entry.path();
        if path.is_dir() {
            visit_rust_sources(&path, hits, needle);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            let source = fs::read_to_string(&path).unwrap();
            hits.extend(source.match_indices(needle).map(|_| path.clone()));
        }
    }
}

fn rust_source_hits(needle: &str) -> Vec<std::path::PathBuf> {
    let mut hits = Vec::new();
    visit_rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut hits,
        needle,
    );
    hits
}

fn numeric_constant(source: &str, name: &str) -> u64 {
    let marker = format!("const {name}");
    source
        .lines()
        .find(|line| line.contains(&marker))
        .and_then(|line| line.rsplit_once('='))
        .map(|(_, value)| value.trim().trim_end_matches(';'))
        .unwrap_or_else(|| panic!("missing value for numeric constant {name}"))
        .parse()
        .unwrap_or_else(|error| panic!("invalid numeric constant {name}: {error}"))
}

#[test]
fn single_result_download_retains_a_blob_instead_of_wasm_bytes() {
    let render = section(
        INTERFACE_RUNTIME,
        "pub(super) fn render_single_result_with_download(",
        "pub(super) fn tar_entries(",
    );
    assert!(render.contains("let download_blob = blob.clone();"));
    assert!(render.contains("download_blob.as_ref().and_then(blob_object_url)"));
    assert!(!render.contains("entries.clone()"));
    assert!(!render.contains("download_entries"));

    let dispatch = section(
        INTERFACE_RUNTIME,
        "pub(super) fn render_result(",
        "fn bootnode_setting(",
    );
    assert_in_order(
        dispatch,
        &[
            "render_single_result_with_download(&data[selected])",
            "if data.len() > 1",
            "Rc::new(data)",
        ],
    );
}

#[test]
fn replacing_or_remounting_releases_the_previous_result() {
    let release = section(
        INTERFACE_RUNTIME,
        "pub(crate) fn release_result_object_url()",
        "fn clear_result_view()",
    );
    assert!(release.contains("revoke_object_url"));
    let clear = section(
        INTERFACE_RUNTIME,
        "fn clear_result_view()",
        "pub(crate) fn replace_result_view(",
    );
    assert!(clear.contains("release_result_object_url()"));
    assert!(clear.contains("resultActions"));
    assert!(clear.matches("set_inner_html(\"\")").count() >= 2);

    let replace = section(
        INTERFACE_RUNTIME,
        "pub(crate) fn replace_result_view(",
        "fn append_result_view(",
    );
    assert_in_order(
        replace,
        &["clear_result_view()", "append_result_view(node)"],
    );

    let render = section(
        LIBRARY,
        "pub fn render_interface(&self, container: Element) -> Object",
        "let s = self.inner.clone();",
    );
    assert_in_order(
        render,
        &[
            "release_current_stream_view()",
            "release_result_object_url()",
            "render_interface_shell(&container)",
        ],
    );
}

#[test]
fn one_shared_worker_owns_the_node_and_unified_wasm() {
    for source in [INTERFACE, LIBRARY] {
        assert!(!source.contains("Weeb3::new("));
        assert!(!source.contains("Arc<Weeb3>"));
    }
    let owners = rust_source_hits("Weeb3::new(");
    assert_eq!(owners.len(), 1, "unexpected node owners: {owners:?}");
    assert!(owners[0].ends_with("src/worker_runtime.rs"));

    let constructors = rust_source_hits("SharedWorker::new_with_worker_options(");
    assert_eq!(
        constructors.len(),
        1,
        "unexpected worker constructors: {constructors:?}"
    );
    assert!(constructors[0].ends_with("src/shared_runtime.rs"));
    assert_eq!(SHARED_WORKER.matches("new Weeb3WorkerRuntime()").count(), 1);

    assert!(CORE.contains("mod worker_runtime;"));
    assert!(CORE.contains("pub use worker_runtime::Weeb3WorkerRuntime;"));
    assert!(CORE.contains("mod library;"));
    assert!(INDEX.contains("import(\"/weeb-3/weeb_3.js\")"));
    assert!(SHARED_WORKER.starts_with("import init, { Weeb3WorkerRuntime } from \"./weeb_3.js\";"));
    assert!(!CARGO.contains("worker-runtime"));
    assert!(!CARGO.contains("facade ="));

    let facade = compact(SHARED_RUNTIME);
    assert!(facade.contains("constSHARED_WORKER_URL:&str=\"/weeb-3/worker.js\";"));
    assert!(facade.contains("requested_url.unwrap_or(SHARED_WORKER_URL)"));
    assert!(facade.contains(".set(\"build\",env!(\"WEEB3_BUILD_VERSION\"))"));
    assert!(facade.contains("SharedWorker::new_with_worker_options(&url,&options)"));
    assert!(compact(LIBRARY).contains("shared_worker_url.as_deref()"));
    assert!(
        compact(INTERFACE)
            .contains("SharedNodeClient::new(initial_profile.swarm_network_id,None,)")
    );
}

#[test]
fn unified_wasm_and_worker_are_in_every_distribution() {
    assert_eq!(HAXE_BUILD.matches("clientele('wasm-pack'").count(), 1);
    assert_eq!(NPM_WORKFLOW.matches("wasm-pack build").count(), 1);
    assert!(!HAXE_BUILD.contains("--features"));
    assert!(!NPM_WORKFLOW.contains("--features"));
    for asset in ["weeb_3.js", "weeb_3_bg.wasm"] {
        assert!(BUILD.contains(&format!("static/{asset}")));
        assert!(SERVER.contains(&format!("#[include = \"{asset}\"]")));
        assert!(SERVER.contains(&format!("/weeb-3/{asset}")));
    }
    assert!(NPM_WORKFLOW.contains("npm pack ./static"));
    assert!(NPM_WORKFLOW.contains("path: \"*.tgz\""));
    assert!(NPM_WORKFLOW.contains("npm publish ./package/*.tgz"));

    assert!(!SHARED_WORKER.trim().is_empty());
    assert!(
        STATIC_IGNORE
            .lines()
            .any(|line| line.trim() == "!worker.js")
    );
    assert!(CARGO.contains("'SharedWorker'"));
    assert!(CARGO.contains("'WorkerOptions'"));
    assert!(BUILD.matches("static/worker.js").count() >= 2);
    assert!(SERVER.contains("#[include = \"worker.js\"]"));
    assert!(SERVER.contains("/weeb-3/worker.js"));
    assert!(HAXE_BUILD.contains("'worker.js'"));
    assert!(NPM_WORKFLOW.contains("files[6]=\"worker.js\""));
    assert!(NPM_WORKFLOW.contains("'exports[./worker.js].default=./worker.js'"));
}

#[test]
fn secure_vault_stays_lazy_and_window_brokered() {
    assert!(!INTERFACE.contains("secure_preload_vault_module"));
    assert!(!SECURE_VAULT.contains("secure_preload_vault_module"));
    let authorization = section(
        SECURE_VAULT,
        "pub async fn secure_ensure_authorized()",
        "async fn check_batch_state(",
    );
    assert!(authorization.contains("secure_client().await"));
    let user_action = section(
        SECURE_VAULT,
        "pub fn secure_open_vault_from_user_action()",
        "fn active_network_id()",
    );
    assert!(user_action.contains("preopen_secure_vault_window(&options)"));

    let handler = section(
        SECURE_VAULT,
        "pub(crate) async fn handle_worker_vault_request(",
        "pub async fn secure_batch_state_for_wallet(",
    );
    for (broker_call, window_call) in [
        (
            "worker_vault_call(\"ensureAuthorized\"",
            "secure_ensure_authorized_in_window().await",
        ),
        (
            "worker_vault_call(\"stampChunk\"",
            "secure_stamp_chunk_in_window(",
        ),
        (
            "worker_vault_call(\"resetStamp\"",
            "secure_reset_stamp_in_window().await",
        ),
        (
            "worker_vault_call(\"ensureFeedOwner\"",
            "secure_ensure_feed_owner_in_window().await",
        ),
        (
            "worker_vault_call(\"createFeedUpdateSocWithStamp\"",
            "secure_create_feed_update_soc_with_stamp_in_window(",
        ),
    ] {
        assert!(SECURE_VAULT.contains(broker_call));
        assert!(handler.contains(window_call));
    }
    assert!(!CORE.contains("WORKER_CONTEXT"));
    assert!(!CORE.contains("window_context()"));
    assert_eq!(
        LIBRARY
            .matches("secure_ensure_feed_owner_in_window().await")
            .count(),
        2
    );

    let oracle = compact(ON_CHAIN);
    assert!(oracle.contains(
        "pubasyncfnget_price_from_oracle()->Option<(U256,U256)>{crate::secure_vault::worker_price_oracle().await}"
    ));
    let vault = compact(SECURE_VAULT);
    assert!(vault.contains("worker_vault_call(\"priceOracle\",|_|{})"));
    assert!(vault.contains("(bytes.len()==32).then(||U256::from_big_endian(&bytes))"));

    let gate = section(
        SHARED_WORKER,
        "function requiresVault(message)",
        "function dispatchForClient(message",
    );
    assert!(gate.contains("message?.type === \"UPLOAD_REQUEST\""));
    assert!(gate.contains("message.op === \"upload\""));
    assert!(gate.contains("message.op === \"resetStamp\""));
    let queue = section(
        SHARED_WORKER,
        "function dispatchForClient(message",
        "async function acquirePlaybackLock()",
    );
    assert!(queue.contains("const flight = vaultQueue.then(run, run)"));
    assert!(queue.contains("vaultQueue = flight.then"));
}

#[test]
fn network_switches_preserve_dispatched_transfer_accounting() {
    let operations = section(
        SHARED_WORKER,
        "const TRANSFER_NODE_OPERATIONS = new Set([",
        "function errorResponse(",
    );
    for operation in TRANSFER_NODE_OPERATIONS {
        assert!(operations.contains(&format!("\"{operation}\"")));
    }
    let classifier = section(
        SHARED_WORKER,
        "function carriesTransferAccounting(message)",
        "async function dispatchRuntimeMessage(message)",
    );
    for request_type in [
        "WEEB3_FETCH_REQUEST",
        "UPLOAD_REQUEST",
        "WEEB3_NODE_REQUEST",
    ] {
        assert!(classifier.contains(request_type));
    }

    let tracked = section(
        SHARED_WORKER,
        "async function dispatchRuntimeMessage(message)",
        "function dispatchForClient(message, client)",
    );
    assert_in_order(
        tracked,
        &[
            "activeTransferOperations += 1",
            "return await runtime.handleMessage(message)",
            "finally",
            "activeTransferOperations -= 1",
        ],
    );
    let start = section(
        SHARED_WORKER,
        "async function performStart(message, client)",
        "async function startRuntime(message, client)",
    );
    assert_in_order(
        start,
        &[
            "activeTransferOperations > 0",
            "cannot switch Swarm network while dispatched transfers are settling",
            "closeServiceWorkerPort(true)",
        ],
    );

    let accounting = section(
        CORE,
        "pub(crate) async fn has_unsettled_accounting(",
        "pub async fn set_network_id(",
    );
    assert!(accounting.contains("accounting_peer.lock().await.reserve != 0"));
    assert!(accounting.contains("ongoing_cheques.lock().await.is_empty()"));
    let rust_start = section(
        WORKER_RUNTIME,
        "pub async fn start(&self, options: JsValue)",
        "#[wasm_bindgen(js_name = handleMessage)]",
    );
    assert_in_order(
        rust_start,
        &[
            "if configured_network_id != Some(network_id)",
            "self.inner.has_unsettled_accounting().await",
            "self.inner.set_network_id(",
        ],
    );

    assert!(!SHARED_RUNTIME.contains("NODE_OPERATION_TIMEOUT"));
    let request = section(
        SHARED_RUNTIME,
        "async fn request_unbounded(&self, request: &Object)",
        "async fn start(&self, network_id: u64)",
    );
    assert!(request.contains("self.request_inner(request, None).await"));
    assert!(request.contains("None => Ok(receiver.recv().await)"));
    let dispatch = section(
        SHARED_RUNTIME,
        "async fn send_node_operation(&self, op: &str, request: Object)",
        "pub(crate) async fn runtime_snapshot(",
    );
    for operation in TRANSFER_NODE_OPERATIONS {
        assert!(dispatch.contains(&format!("\"{operation}\"")));
    }
    assert_in_order(
        dispatch,
        &[
            "let transfer_bearing = matches!",
            "runtime.request_unbounded(&request)",
        ],
    );
}

#[test]
fn log_polling_is_combined_bounded_and_per_client() {
    let poll = section(
        INTERFACE,
        "let mut last_progress_revision = 0u64;",
        "\n    Ok(())\n}",
    );
    assert_eq!(poll.matches("runtime_snapshot(").count(), 1);
    assert!(poll.contains("Duration::from_millis(160)"));
    assert!(!poll.contains("get_connections("));
    assert!(!poll.contains("get_current_logs("));
    assert_eq!(
        INTERFACE.matches("runtime_snapshot(").count()
            + LIBRARY.matches("runtime_snapshot(").count(),
        1
    );

    assert!(SHARED_RUNTIME.contains("seen_log_sequence: Cell<u64>"));
    assert!(SHARED_RUNTIME.contains("self.seen_log_sequence.set(log_sequence)"));
    let snapshot = section(
        WORKER_RUNTIME,
        "async fn runtime_snapshot_response(",
        "fn required_network_id(",
    );
    assert!(snapshot.contains("*sequence > seen_logs"));
    assert!(snapshot.contains("state.logs.push_back((sequence, log))"));
    assert!(snapshot.contains("state.logs.pop_front()"));
    assert!(!snapshot.contains("state.logs.clear()"));
    assert!(WORKER_RUNTIME.contains("state: RefCell<RuntimeState>"));
    assert!(WORKER_RUNTIME.contains("const WORKER_LOG_RING_CAPACITY: usize = 256;"));

    assert!(CORE.contains("const LOG_QUEUE_CAPACITY: usize = 256;"));
    assert!(CORE.contains("const LOG_DRAIN_BATCH: usize = 64;"));
    assert!(CORE.contains("pub(crate) const LOG_DOM_RETAINED: u32 = 256;"));
    assert!(CORE.contains("mpsc::bounded::<String>(LOG_QUEUE_CAPACITY)"));
    let progress = section(
        SHARED_RUNTIME,
        "pub(crate) async fn get_progress_snapshot(",
        "pub(crate) async fn toggle_transfer_pause(",
    );
    assert!(progress.contains("runtime_snapshot_options(seen_revision, false, true)"));
}

#[test]
fn web_lock_and_client_state_follow_window_lifecycle() {
    assert_eq!(SHARED_WORKER.matches("new Weeb3WorkerRuntime()").count(), 1);
    assert!(SHARED_WORKER.contains("const clients = new Map();"));
    assert!(SHARED_WORKER.contains("const vaultDisconnects = new Map();"));
    assert!(!SHARED_WORKER.contains("runtimeMap"));

    let remove = section(
        SHARED_WORKER,
        "async function removeClient(client)",
        "async function dispatch(message",
    );
    assert_in_order(
        remove,
        &[
            "clients.delete(id)",
            "vaultDisconnects.delete(id)",
            "activeHlsLease?.clientId === id",
            "releasePlaybackLock()",
            "await cancelHlsLease(lease)",
        ],
    );
    let connect = section(SHARED_WORKER, "self.addEventListener(\"connect\"", "\n});");
    assert!(connect.contains("clients.set(client.id, client)"));
    assert!(connect.contains("if (!clients.has(client.id)) clients.set(client.id, client)"));
    assert!(connect.contains("port.addEventListener(\"close\""));
    assert!(connect.contains("WEEB3_CLIENT_CLOSE"));

    let pagehide = section(
        SHARED_RUNTIME,
        "fn create_runtime(",
        "fn register_service_worker_relay(",
    );
    assert!(pagehide.contains("add_event_listener_with_callback(\"pagehide\""));
    assert!(pagehide.contains("request_object(\"WEEB3_CLIENT_CLOSE\", 0)"));
    let restore = section(
        INTERFACE,
        "fn pageshow_event_is_persisted(event: &Event) -> bool",
        "pub(crate) fn begin_interface_mount()",
    );
    assert!(restore.contains("reload_requested || !pageshow_event_is_persisted(&event)"));
    assert!(restore.contains("window.location().reload()"));

    let lock = section(
        SHARED_WORKER,
        "async function acquirePlaybackLock()",
        "async function removeClient(client)",
    );
    assert!(lock.contains("self.navigator.locks.request(\"weeb3-active-playback\""));
    assert!(lock.contains("mode: \"exclusive\""));
    assert!(lock.contains("ifAvailable: true"));
    assert!(lock.contains("await held"));
    assert!(lock.contains("function releasePlaybackLock()"));
    assert!(SHARED_WORKER.contains("void acquirePlaybackLock()"));
    assert!(SHARED_WORKER.contains("releasePlaybackLock();"));
}

#[test]
fn optional_two_tab_shared_worker_smoke() -> Result<()> {
    let Some(raw_url) = env::var("WEEB3_SHARED_WORKER_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        println!("WEEB3_SHARED_WORKER_URL is not set; skipping SharedWorker browser smoke");
        return Ok(());
    };
    if !raw_url.contains("worker.js") {
        return Err(anyhow!(
            "WEEB3_SHARED_WORKER_URL must identify the served worker.js"
        ));
    }

    let protocol = numeric_constant(SHARED_RUNTIME, "SHARED_WORKER_PROTOCOL");
    let network_id = env::var("WEEB3_SHARED_WORKER_NETWORK_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value.parse::<u64>().map_err(|error| {
                anyhow!("invalid WEEB3_SHARED_WORKER_NETWORK_ID {value:?}: {error}")
            })
        })
        .transpose()?
        .unwrap_or(1);
    let worker_url = if raw_url.contains("protocol=") {
        raw_url
    } else if raw_url.contains('?') {
        format!("{raw_url}&protocol={protocol}")
    } else {
        format!("{raw_url}?protocol={protocol}")
    };
    let worker_name = format!("weeb3-shared-runtime-v{protocol}");
    let timeout = Duration::from_secs(30);
    let browser = browser::launch(timeout, false)?;
    let first = browser
        .new_tab()
        .map_err(|error| anyhow!("could not open first SharedWorker smoke tab: {error:?}"))?;
    let second = browser
        .new_tab()
        .map_err(|error| anyhow!("could not open second SharedWorker smoke tab: {error:?}"))?;
    for tab in [&first, &second] {
        tab.set_default_timeout(timeout);
        tab.navigate_to(&worker_url)
            .map_err(|error| anyhow!("could not navigate to {worker_url}: {error:?}"))?
            .wait_until_navigated()
            .map_err(|error| anyhow!("navigation to {worker_url} did not finish: {error:?}"))?;
    }

    let first_result = run_worker_smoke(&first, &worker_url, &worker_name, protocol, network_id)?;
    let second_result = run_worker_smoke(&second, &worker_url, &worker_name, protocol, network_id)?;
    for (name, result) in [("first", &first_result), ("second", &second_result)] {
        assert_eq!(
            result.pointer("/started/ok"),
            Some(&Value::Bool(true)),
            "{name} tab failed to start the worker: {result}"
        );
        assert_eq!(
            result.pointer("/ping/ok"),
            Some(&Value::Bool(true)),
            "{name} tab failed to ping the worker: {result}"
        );
        assert_eq!(
            result.pointer("/ping/networkId").and_then(Value::as_u64),
            Some(network_id)
        );
    }
    let first_runtime = first_result
        .pointer("/ping/runtimeId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("first tab ping omitted runtimeId: {first_result}"))?;
    let second_runtime = second_result
        .pointer("/ping/runtimeId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("second tab ping omitted runtimeId: {second_result}"))?;
    assert_eq!(
        first_runtime, second_runtime,
        "two tabs reached different node-owning SharedWorkers"
    );
    Ok(())
}

fn run_worker_smoke(
    tab: &headless_chrome::Tab,
    worker_url: &str,
    worker_name: &str,
    protocol: u64,
    network_id: u64,
) -> Result<Value> {
    let script = r#"
        (async () => {
          const worker = new SharedWorker(__WORKER_URL__, {
            type: "module",
            name: __WORKER_NAME__
          });
          globalThis.__weeb3SharedWorkerSmoke = worker;
          worker.port.start();
          const call = (message) => new Promise((resolve, reject) => {
            const channel = new MessageChannel();
            const timer = setTimeout(() => {
              channel.port1.close();
              reject(new Error("SharedWorker smoke request timed out"));
            }, 20000);
            channel.port1.onmessage = (event) => {
              clearTimeout(timer);
              channel.port1.close();
              resolve(event.data);
            };
            worker.port.postMessage(message, [channel.port2]);
          });
          const started = await call({
            type: "WEEB3_WORKER_START",
            protocol: __PROTOCOL__,
            networkId: __NETWORK_ID__
          });
          const ping = await call({
            type: "WEEB3_CLIENT_PING",
            protocol: __PROTOCOL__,
            networkId: __NETWORK_ID__
          });
          return JSON.stringify({ started, ping });
        })()
    "#
    .replace("__WORKER_URL__", &serde_json::to_string(worker_url)?)
    .replace("__WORKER_NAME__", &serde_json::to_string(worker_name)?)
    .replace("__PROTOCOL__", &protocol.to_string())
    .replace("__NETWORK_ID__", &network_id.to_string());
    let remote = tab
        .evaluate(&script, true)
        .map_err(|error| anyhow!("SharedWorker smoke script failed: {error:?}"))?;
    let raw = remote
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("SharedWorker smoke did not return JSON"))?;
    serde_json::from_str(&raw).map_err(|error| anyhow!("invalid SharedWorker smoke JSON: {error}"))
}
