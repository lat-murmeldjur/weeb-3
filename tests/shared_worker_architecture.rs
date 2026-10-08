use std::{env, fs, path::Path, time::Duration};

use anyhow_crates_io::{Context, Result, anyhow};
use serde_json::Value;

#[path = "support/browser.rs"]
mod browser;
#[path = "support/source.rs"]
pub mod source;

use source::{assert_contains, assert_excludes, assert_in_order, between as section, compact};

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
const WALLET_WORKFLOWS: &str = include_str!("../src/wallet_workflows.rs");
const SHARED_WORKER: &str = include_str!("../static/worker.js");
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

fn runtime_function(name: &str) -> &'static str {
    section(INTERFACE_RUNTIME, &format!("fn {name}("), "\n}")
}

fn worker_function(name: &str) -> &'static str {
    section(SHARED_WORKER, &format!("function {name}("), "\n}")
}

fn rust_source_hits(needle: &str) -> Vec<std::path::PathBuf> {
    let mut paths = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    let mut hits = Vec::new();
    while let Some(path) = paths.pop() {
        if path.is_dir() {
            paths.extend(fs::read_dir(path).unwrap().map(|entry| entry.unwrap().path()));
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            let mut source = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            // Test fixtures may construct a node; preserve production code after test modules.
            while let Some(start) = source.find("#[cfg(test)]\nmod ") {
                let end = start + source[start..].find("\n}").expect("test module closing brace") + 2;
                source.replace_range(start..end, "");
            }
            hits.extend(source.match_indices(needle).map(|_| path.clone()));
        }
    }
    hits
}

#[test]
fn batch_purchase_preserves_interface_and_npm_missing_vault_policies() {
    let interface = compact(INTERFACE);
    let library = compact(LIBRARY);
    let workflows = compact(WALLET_WORKFLOWS);

    assert!(interface.contains("MissingSecureBatchState::Error"));
    assert!(library.contains("MissingSecureBatchState::ContinuePurchase"));
    assert!(workflows.contains("Some(state)ifstate.usable()"));
    assert!(workflows.contains(
        "Noneifmatches!(missing,MissingSecureBatchState::Error)=>{returnErr(BatchPurchaseError::CheckSecure);"
    ));
}

fn numeric_constant(source: &str, name: &str) -> u64 {
    let declaration = section(source, &format!("const {name}"), ";");
    let (_, value) = declaration.rsplit_once('=').expect("constant assignment");
    value.trim().parse().unwrap_or_else(|error| panic!("invalid {name}: {error}"))
}

#[test]
fn single_result_download_retains_a_blob_instead_of_wasm_bytes() {
    let render = runtime_function("render_single_result_with_download");
    assert_contains(render, &["let download_blob = blob.clone();", "download_blob.as_ref().and_then(blob_object_url)"]);
    assert_excludes(render, &["entries.clone()", "download_entries"]);

    assert_in_order(
        runtime_function("render_result"),
        &[
            "render_single_result_with_download(&data[selected])",
            "if data.len() > 1",
            "Rc::new(data)",
        ],
    );
}

#[test]
fn replacing_or_remounting_releases_the_previous_result() {
    assert!(runtime_function("release_result_object_url").contains("revoke_object_url"));
    let clear = runtime_function("clear_result_view");
    assert_contains(clear, &[
        "release_result_object_url()",
        "resultActions",
        "RESULT_CALLBACKS.with",
        "borrow_mut().clear()",
    ]);
    assert!(clear.matches("set_inner_html(\"\")").count() >= 2);

    assert_in_order(
        runtime_function("replace_result_view"),
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
        assert_excludes(source, &["Weeb3::new(", "Arc<Weeb3>"]);
    }
    for (constructor, owner) in [
        ("Weeb3::new(", "src/worker_runtime.rs"),
        ("SharedWorker::new_with_worker_options(", "src/shared_runtime.rs"),
    ] {
        let hits = rust_source_hits(constructor);
        assert_eq!(hits.len(), 1, "unexpected {constructor} owners: {hits:?}");
        assert!(hits[0].ends_with(owner));
    }
    assert_eq!(SHARED_WORKER.matches("new Weeb3WorkerRuntime()").count(), 1);

    assert_contains(CORE, &[
        "mod worker_runtime;",
        "pub use worker_runtime::Weeb3WorkerRuntime;",
        "mod library;",
    ]);
    assert!(INDEX.contains("import(\"/weeb-3/weeb_3.js\")"));
    assert!(SHARED_WORKER.starts_with("import init, { Weeb3WorkerRuntime } from \"./weeb_3.js\";"));
    assert_excludes(CARGO, &["worker-runtime", "facade ="]);

    let facade = compact(SHARED_RUNTIME);
    assert_contains(&facade, &[
        "constSHARED_WORKER_URL:&str=\"/weeb-3/worker.js\";",
        "requested_url.unwrap_or(SHARED_WORKER_URL)",
        "document.base_uri().ok().flatten()",
        ".set(\"build\",env!(\"WEEB3_BUILD_VERSION\"))",
        "weeb3-shared-runtime-v{SHARED_WORKER_PROTOCOL}:{url}",
        "SharedWorker::new_with_worker_options(&url,&options)",
    ]);
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
    assert_in_order(
        NPM_WORKFLOW,
        &["wasm-pack build", "git restore static/.gitignore"],
    );
    assert!(!HAXE_BUILD.contains("--features"));
    assert!(!NPM_WORKFLOW.contains("--features"));
    for asset in ["weeb_3.js", "weeb_3_bg.wasm", "worker.js"] {
        assert!(BUILD.contains(&format!("static/{asset}")));
        assert!(SERVER.contains(&format!("#[include = \"{asset}\"]")));
        assert!(SERVER.contains(&format!("/weeb-3/{asset}")));
    }
    assert_contains(NPM_WORKFLOW, &[
        "npm pack ./static",
        "path: \"*.tgz\"",
        "npm publish ./package/*.tgz",
    ]);

    assert_contains(CARGO, &["'SharedWorker'", "'WorkerOptions'"]);
    assert!(HAXE_BUILD.contains("'worker.js'"));
    assert_contains(NPM_WORKFLOW, &["files[6]=\"worker.js\"", "'exports[./worker.js].default=./worker.js'"]);
}

#[test]
fn secure_vault_stays_lazy_and_window_brokered() {
    assert!(!INTERFACE.contains("secure_preload_vault_module"));
    assert!(!SECURE_VAULT.contains("secure_preload_vault_module"));
    let authorization = section(
        SECURE_VAULT,
        "pub async fn secure_ensure_authorized()",
        "pub async fn secure_prepare_batch_purchase(",
    );
    assert!(authorization.contains("worker_vault_call("));
    let user_action = section(
        SECURE_VAULT,
        "pub fn secure_open_vault_from_user_action()",
        "fn active_network_id()",
    );
    assert_in_order(
        user_action,
        &[
            "preopen_secure_vault_window(&options)",
            "SECURE_CLICK_CONNECT_OPTIONS.with",
        ],
    );

    let handler = section(
        SECURE_VAULT,
        "pub(crate) async fn handle_worker_vault_request(",
        "pub async fn secure_batch_state_for_wallet(",
    );
    for (operation, window_call) in [
        ("ensureAuthorized", "secure_client().await.is_some()"),
        ("stampChunk", "secure_stamp_chunk_in_window("),
        ("resetStamp", "secure_reset_stamp_in_window().await"),
        ("ensureFeedOwner", "secure_ensure_feed_owner_in_window().await"),
        ("createFeedUpdateSocWithStamp", "secure_create_feed_update_soc_with_stamp_in_window("),
    ] {
        assert!(SECURE_VAULT.contains(&format!("worker_vault_call(\"{operation}\"")));
        assert!(handler.contains(window_call));
    }
    assert_excludes(CORE, &["WORKER_CONTEXT", "window_context()"]);
    assert!(SECURE_VAULT.contains("SECURE_RESUME_CALLBACK"));
    assert!(!SECURE_VAULT.contains("let callback = callback.into_js_value()"));
    assert_eq!(
        LIBRARY
            .matches("secure_ensure_feed_owner_in_window().await")
            .count(),
        1
    );
    assert!(LIBRARY.contains("None => feed_owner_for_request(\"\").await"));

    let oracle = compact(ON_CHAIN);
    assert!(oracle.contains(
        "pubasyncfnget_price_from_oracle()->Option<(U256,U256)>{crate::secure_vault::worker_price_oracle().await}"
    ));
    let vault = compact(SECURE_VAULT);
    assert_contains(&vault, &[
        "worker_vault_call(\"priceOracle\",Object::new())",
        "bytes_array_prop(value,name).filter(|value|value.length()==32)?",
        "value.copy_to(&mutbytes)",
        "Some(U256::from_big_endian(&bytes))",
    ]);

    assert_contains(worker_function("requiresVault"), &[
        "message?.type === \"UPLOAD_REQUEST\"",
        "message.op === \"upload\"",
        "message.op === \"resetStamp\"",
    ]);
    assert_contains(worker_function("dispatchForClient"), &["const flight = vaultQueue.then(run, run)", "vaultQueue = flight.then"]);
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
    assert_contains(worker_function("carriesTransferAccounting"), &[
        "WEEB3_FETCH_REQUEST",
        "UPLOAD_REQUEST",
        "WEEB3_NODE_REQUEST",
    ]);

    assert_in_order(
        worker_function("dispatchRuntimeMessage"),
        &[
            "activeTransferOperations += 1",
            "return await runtime.handleMessage(message)",
            "finally",
            "activeTransferOperations -= 1",
        ],
    );
    let start = worker_function("performStart");
    assert_in_order(
        start,
        &[
            "activeTransferOperations > 0",
            "cannot switch Swarm network while dispatched transfers are settling",
            "closeServiceWorkerPort(true)",
        ],
    );

    let accounting = section(
        include_str!("../src/network_conventions.rs"),
        "pub(crate) async fn has_unsettled_accounting(",
        "pub(crate) async fn connection_counts(",
    );
    assert_contains(accounting, &["accounting_peer.borrow().reserve != 0", "ongoing_cheques.lock().await.is_empty()"]);
    let rust_start = section(
        WORKER_RUNTIME,
        "pub async fn start(&self, options: JsValue)",
        "#[wasm_bindgen(js_name = handleMessage)]",
    );
    assert_in_order(
        rust_start,
        &[
            "self.transition.lock().await",
            "if configured_network_id != Some(network_id)",
            "self.inner.has_unsettled_accounting().await",
            "self.inner.set_network_id(",
        ],
    );

    assert!(!SHARED_RUNTIME.contains("NODE_OPERATION_TIMEOUT"));
    let request = section(
        SHARED_RUNTIME,
        "pub(crate) async fn request(",
        "async fn start(&self, network_id: u64)",
    );
    assert!(request.contains("None => Ok((&mut channel.receiver).await)"));
    let dispatch = section(
        SHARED_RUNTIME,
        "fn node_operation<'a>(",
        "pub(crate) async fn runtime_snapshot(",
    );
    for operation in TRANSFER_NODE_OPERATIONS {
        assert!(dispatch.contains(&format!("\"{operation}\"")));
    }
    assert_in_order(
        dispatch,
        &[
            "impl Future<Output = Result<Object, String>> + 'a",
            "let request = node_request_object(op, self.network_id());",
            "populate(&request);",
            "self.send_node_operation(op, request)",
            "async fn send_node_operation(&self, op: &str, request: Object)",
            "let runtime = self.ensure().await?;",
            "let timeout = if matches!",
            ") {\n            None\n        } else {",
            "if op == \"connections\"",
            "integer_property(&request, \"waitMs\").unwrap_or(0)",
            "Some(CONTROL_TIMEOUT + Duration::from_millis(wait))",
            "runtime.request(&request, timeout).await?",
            "require_ok(&response, op)?;",
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
    assert!(poll.contains("Duration::from_millis(600)"));
    assert_excludes(poll, &["get_connections(", "get_current_logs("]);
    assert_eq!(INTERFACE.matches("runtime_snapshot(").count(), 1);
    assert_eq!(LIBRARY.matches("runtime_snapshot(0, false, false)").count(), 1);

    assert_contains(SHARED_RUNTIME, &["seen_log_sequence: Cell<u64>", "self.seen_log_sequence.set(log_sequence)"]);
    let snapshot = section(
        WORKER_RUNTIME,
        "async fn runtime_snapshot_response(",
        "fn required_network_id(",
    );
    assert_contains(snapshot, &[
        "*sequence > seen_logs",
        "state.logs.push_back((sequence, log))",
        "state.logs.pop_front()",
    ]);
    assert!(!snapshot.contains("state.logs.clear()"));
    assert_contains(WORKER_RUNTIME, &["state: RefCell<RuntimeState>", "const WORKER_LOG_RING_CAPACITY: usize = 256;"]);

    assert_contains(CORE, &[
        "const LOG_QUEUE_CAPACITY: usize = 256;",
        "const LOG_DRAIN_BATCH: usize = 64;",
        "pub(crate) const LOG_DOM_RETAINED: u32 = 256;",
        "mpsc::bounded::<String>(LOG_QUEUE_CAPACITY)",
    ]);
    let log_drain = section(
        CORE,
        "pub fn get_current_logs(&self)",
        "pub async fn get_connections(&self)",
    );
    assert!(log_drain.contains("drain_ready(None, &self.log_port.1).take(LOG_DRAIN_BATCH)"));
    assert!(!log_drain.contains("REFRESHMENT"));
    let progress = section(
        SHARED_RUNTIME,
        "pub(crate) async fn get_progress_snapshot(",
        "pub(crate) async fn toggle_transfer_pause(",
    );
    assert!(progress.contains("runtime_snapshot(seen_revision, false, true)"));
}

#[test]
fn web_lock_and_client_state_follow_window_lifecycle() {
    assert_contains(SHARED_WORKER, &["const clients = new Map();", "const vaultDisconnects = new Map();"]);
    assert!(!SHARED_WORKER.contains("runtimeMap"));

    assert_in_order(
        worker_function("removeClient"),
        &[
            "clients.delete(id)",
            "vaultDisconnects.delete(id)",
            "activeHlsLease?.clientId === id",
            "releasePlaybackLock()",
            "await cancelHlsLease(lease)",
            "if (clients.size === 0) closeServiceWorkerPort(true)",
        ],
    );
    let connect = section(SHARED_WORKER, "self.addEventListener(\"connect\"", "\n});");
    assert_contains(connect, &[
        "clients.set(client.id, client)",
        "if (!clients.has(client.id)) clients.set(client.id, client)",
        "port.addEventListener(\"close\"",
        "WEEB3_CLIENT_CLOSE",
    ]);

    let pagehide = section(SHARED_RUNTIME, "fn create_runtime(", "\n}");
    assert_contains(pagehide, &["DomListener::new(&window, \"pagehide\"", "request_object(\"WEEB3_CLIENT_CLOSE\", 0)"]);
    let restore = section(
        INTERFACE,
        "fn pageshow_event_is_persisted(event: &Event) -> bool",
        "pub(crate) fn begin_interface_mount()",
    );
    assert!(restore.contains("reload_requested || !pageshow_event_is_persisted(&event)"));
    assert!(restore.contains("window.location().reload()"));

    let lock = section(
        SHARED_WORKER,
        "function acquirePlaybackLock()",
        "async function removeClient(client)",
    );
    assert_contains(lock, &[
        "self.navigator.locks.request(\"weeb3-active-playback\"",
        "mode: \"exclusive\"",
        "ifAvailable: true",
        "await held",
        "function releasePlaybackLock()",
    ]);
    assert_contains(SHARED_WORKER, &["void acquirePlaybackLock()", "releasePlaybackLock();"]);
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
    let network_id = env::var("WEEB3_SHARED_WORKER_NETWORK_ID").unwrap_or_default();
    let network_id = if network_id.trim().is_empty() { 1 } else {
        network_id.parse().context("invalid WEEB3_SHARED_WORKER_NETWORK_ID")?
    };
    let worker_url = if raw_url.contains("protocol=") {
        raw_url
    } else if raw_url.contains('?') {
        format!("{raw_url}&protocol={protocol}")
    } else {
        format!("{raw_url}?protocol={protocol}")
    };
    let worker_name = format!("weeb3-shared-runtime-v{protocol}:{worker_url}");
    let timeout = Duration::from_secs(30);
    let browser = browser::launch(timeout, false)?;
    let tabs = [browser.new_tab()?, browser.new_tab()?];
    let mut runtime_ids = Vec::new();
    for (index, tab) in tabs.iter().enumerate() {
        tab.set_default_timeout(timeout);
        tab.navigate_to(&worker_url)
            .with_context(|| format!("tab {index} navigation to {worker_url}"))?
            .wait_until_navigated()
            .context("SharedWorker navigation did not finish")?;
        let result = run_worker_smoke(tab, &worker_url, &worker_name, protocol, network_id)?;
        for operation in ["started", "ping"] {
            assert_eq!(result[operation]["ok"], true, "tab {index} {operation}: {result}");
        }
        assert_eq!(result["ping"]["networkId"], network_id);
        runtime_ids.push(result["ping"]["runtimeId"].as_str()
            .ok_or_else(|| anyhow!("tab {index} ping omitted runtimeId: {result}"))?.to_owned());
    }
    assert_eq!(runtime_ids[0], runtime_ids[1], "tabs reached different SharedWorkers");
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
          const call = (type) => new Promise((resolve, reject) => {
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
            worker.port.postMessage({type, protocol: __PROTOCOL__, networkId: __NETWORK_ID__}, [channel.port2]);
          });
          return JSON.stringify({ started: await call("WEEB3_WORKER_START"), ping: await call("WEEB3_CLIENT_PING") });
        })()
    "#
    .replace("__WORKER_URL__", &serde_json::to_string(worker_url)?)
    .replace("__WORKER_NAME__", &serde_json::to_string(worker_name)?)
    .replace("__PROTOCOL__", &protocol.to_string())
    .replace("__NETWORK_ID__", &network_id.to_string());
    let value = tab.evaluate(&script, true).context("SharedWorker smoke script failed")?
        .value.context("SharedWorker smoke did not return a value")?;
    Ok(serde_json::from_str(value.as_str().context("SharedWorker smoke omitted JSON")?)?)
}
