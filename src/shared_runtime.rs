use std::{
    cell::{Cell, RefCell},
    future::Future,
    rc::Rc,
    time::Duration,
};

use async_lock::Mutex;
use js_sys::{Array, Object, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    Event, File, MessageChannel, MessageEvent, MessagePort, SharedWorker, Url, WorkerOptions,
    WorkerType,
};

use crate::{
    bzz_stream::BzzMetadata,
    erasure_coding::RedundancyLevel,
    events::ProgressRow,
    worker_protocol::{
        array_property, bool_property, bytes_from_js, integer_property, metadata_from_js,
        metadata_to_js, progress_from_js, property, set, set_bool, set_number,
        set_optional_percent, set_string, string_property,
    },
};

pub(crate) const SHARED_WORKER_PROTOCOL: u64 = 5;
const SHARED_WORKER_URL: &str = "/weeb-3/worker.js";
const START_TIMEOUT: Duration = Duration::from_secs(15);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct SharedRuntime {
    _worker: SharedWorker,
    port: MessagePort,
    _message_listener: Closure<dyn FnMut(MessageEvent)>,
    _pagehide_listener: Closure<dyn FnMut(Event)>,
    service_worker_relay_id: u64,
    status: Rc<RuntimeStatus>,
}

struct RuntimeStatus {
    network_id: Cell<u64>,
    configured: Cell<bool>,
}

struct ServiceWorkerRelay {
    next_id: u64,
    ports: Rc<RefCell<Vec<(u64, MessagePort)>>>,
    _listener: Closure<dyn FnMut(MessageEvent)>,
}

thread_local! {
    static SERVICE_WORKER_RELAY: RefCell<Option<ServiceWorkerRelay>> = const { RefCell::new(None) };
}

/// Window-side facade. It never owns a `Weeb3`; all node work is correlated
/// over the one stable SharedWorker port.
pub(crate) struct SharedNodeClient {
    runtime: Result<Rc<SharedRuntime>, String>,
    status: Rc<RuntimeStatus>,
    seen_log_sequence: Cell<u64>,
    transition: Mutex<()>,
}

pub(crate) struct RuntimeSnapshot {
    pub(crate) connections: u64,
    pub(crate) ongoing_connections: u64,
    pub(crate) paused: bool,
    pub(crate) logs: Vec<String>,
    pub(crate) progress: Option<(u64, Vec<ProgressRow>)>,
}

fn create_runtime(
    status: Rc<RuntimeStatus>,
    requested_url: Option<&str>,
) -> Result<SharedRuntime, String> {
    let window =
        web_sys::window().ok_or_else(|| "SharedWorker requires a window client".to_string())?;
    let base = window
        .location()
        .href()
        .map_err(|error| js_error("could not read the page URL", &error))?;
    let url = Url::new_with_base(requested_url.unwrap_or(SHARED_WORKER_URL), &base)
        .map_err(|error| js_error("invalid SharedWorker URL", &error))?;
    url.search_params()
        .set("protocol", &SHARED_WORKER_PROTOCOL.to_string());
    url.search_params()
        .set("build", env!("WEEB3_BUILD_VERSION"));
    let url = url.href();
    let options = WorkerOptions::new();
    options.set_type(WorkerType::Module);
    options.set_name(&format!(
        "weeb3-shared-runtime-v{SHARED_WORKER_PROTOCOL}:{url}"
    ));
    let worker = SharedWorker::new_with_worker_options(&url, &options)
        .map_err(|error| js_error("could not create SharedWorker", &error))?;
    let port = worker.port();
    let listener_status = status.clone();
    let listener = Closure::<dyn FnMut(MessageEvent)>::new(move |event| {
        handle_worker_message(event, &listener_status)
    });
    port.set_onmessage(Some(listener.as_ref().unchecked_ref()));
    port.start();

    let close_port = port.clone();
    let close = Closure::<dyn FnMut(Event)>::new(move |_| {
        let request = request_object("WEEB3_CLIENT_CLOSE", 0);
        let _ = close_port.post_message(&request);
    });
    window
        .add_event_listener_with_callback("pagehide", close.as_ref().unchecked_ref())
        .map_err(|error| js_error("could not install SharedWorker pagehide cleanup", &error))?;
    let service_worker_relay_id = match register_service_worker_relay(&port) {
        Ok(id) => id,
        Err(error) => {
            let _ = window
                .remove_event_listener_with_callback("pagehide", close.as_ref().unchecked_ref());
            return Err(error);
        }
    };

    Ok(SharedRuntime {
        _worker: worker,
        port,
        _message_listener: listener,
        _pagehide_listener: close,
        service_worker_relay_id,
        status,
    })
}

fn register_service_worker_relay(port: &MessagePort) -> Result<u64, String> {
    SERVICE_WORKER_RELAY.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let ports = Rc::new(RefCell::new(Vec::<(u64, MessagePort)>::new()));
            let listener_ports = ports.clone();
            let service_workers = web_sys::window()
                .ok_or_else(|| "SharedWorker requires a window client".to_string())?
                .navigator()
                .service_worker();
            let listener_service_workers = service_workers.clone();
            let listener = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let Some(source) = event.source() else {
                    return;
                };
                let Some(controller) = listener_service_workers.controller() else {
                    return;
                };
                if !Object::is(source.as_ref(), controller.as_ref()) {
                    return;
                }
                let message = event.data();
                if integer_property(&message, "serviceWorkerRelay") != Some(SHARED_WORKER_PROTOCOL)
                {
                    return;
                }
                let Ok(reply) = event.ports().get(0).dyn_into::<MessagePort>() else {
                    return;
                };
                let Some(target) = listener_ports.borrow().last().map(|(_, port)| port.clone())
                else {
                    reply_relay_error(&reply, "weeb-3 SharedWorker port is not available");
                    return;
                };
                let transfer = Array::new();
                transfer.push(reply.as_ref());
                if target
                    .post_message_with_transferable(&message, &transfer)
                    .is_err()
                {
                    reply_relay_error(&reply, "could not relay request to SharedWorker");
                }
            });
            service_workers
                .add_event_listener_with_callback("message", listener.as_ref().unchecked_ref())
                .map_err(|error| {
                    js_error("could not install ServiceWorker relay listener", &error)
                })?;
            *slot = Some(ServiceWorkerRelay {
                next_id: 0,
                ports,
                _listener: listener,
            });
        }

        let relay = slot.as_mut().expect("ServiceWorker relay was initialized");
        relay.next_id += 1;
        relay.ports.borrow_mut().push((relay.next_id, port.clone()));
        Ok(relay.next_id)
    })
}

fn reply_relay_error(reply: &MessagePort, message: &str) {
    let response = Object::new();
    set(&response, "ok", JsValue::FALSE);
    set_number(&response, "status", 503.0);
    set_string(&response, "error", message);
    let _ = reply.post_message(&response);
    reply.close();
}

fn unregister_service_worker_relay(id: u64) {
    SERVICE_WORKER_RELAY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(relay) = slot.as_mut() else {
            return;
        };
        relay
            .ports
            .borrow_mut()
            .retain(|(candidate, _)| *candidate != id);
    });
}

fn handle_worker_message(event: MessageEvent, status: &RuntimeStatus) {
    let data = event.data();
    match string_property(&data, "type").as_deref() {
        Some("WEEB3_NETWORK_CHANGED") => {
            let Some(actual) = integer_property(&data, "networkId") else {
                return;
            };
            status.network_id.set(actual);
            status.configured.set(true);
            if let Some(profile) = crate::network_profile::profile_for_swarm_network_id(actual) {
                crate::network_profile::activate_profile(profile);
            }
            crate::interface::shared_network_changed(actual);
            return;
        }
        Some("WEEB3_VAULT_REQUEST") => {}
        _ => return,
    }
    let Ok(reply) = event.ports().get(0).dyn_into::<MessagePort>() else {
        return;
    };
    let request = property(&data, "request").dyn_into::<Object>().ok();
    spawn_local(async move {
        let response = match request {
            Some(request) => crate::secure_vault::handle_worker_vault_request(&request).await,
            None => {
                let response = Object::new();
                set(&response, "ok", JsValue::FALSE);
                set_string(&response, "error", "worker vault request is not an object");
                response
            }
        };
        let _ = reply.post_message(&response);
        reply.close();
    });
}

impl SharedRuntime {
    pub(crate) fn notify(&self, request: &Object) -> Result<(), String> {
        self.port
            .post_message(request)
            .map_err(|error| js_error("could not notify SharedWorker", &error))
    }

    pub(crate) async fn request(
        &self,
        request: &Object,
        timeout: Duration,
    ) -> Result<Object, String> {
        self.request_inner(request, Some(timeout)).await
    }

    async fn request_unbounded(&self, request: &Object) -> Result<Object, String> {
        self.request_inner(request, None).await
    }

    async fn request_inner(
        &self,
        request: &Object,
        timeout: Option<Duration>,
    ) -> Result<Object, String> {
        let channel = MessageChannel::new()
            .map_err(|error| js_error("could not create SharedWorker reply channel", &error))?;
        let reply = channel.port1();
        reply.start();
        let (sender, receiver) = async_std::channel::bounded::<JsValue>(1);
        let callback = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let _ = sender.try_send(event.data());
        });
        reply.set_onmessage(Some(callback.as_ref().unchecked_ref()));
        let transfer = Array::new();
        transfer.push(channel.port2().as_ref());
        if let Err(error) = self.port.post_message_with_transferable(request, &transfer) {
            reply.set_onmessage(None);
            reply.close();
            return Err(js_error("could not post SharedWorker request", &error));
        }
        let response = match timeout {
            Some(timeout) => async_std::future::timeout(timeout, receiver.recv())
                .await
                .map_err(|_| "SharedWorker request timed out".to_string()),
            None => Ok(receiver.recv().await),
        };
        reply.set_onmessage(None);
        reply.close();
        response?
            .map_err(|_| "SharedWorker reply channel closed".to_string())?
            .dyn_into::<Object>()
            .map_err(|_| "SharedWorker returned a non-object response".to_string())
    }

    async fn start(&self, network_id: u64) -> Result<(), String> {
        let request = request_object("WEEB3_WORKER_START", network_id);
        let response = self.request(&request, START_TIMEOUT).await?;
        require_ok(&response, "SharedWorker startup")?;
        let actual = integer_property(&response, "networkId")
            .ok_or_else(|| "SharedWorker startup omitted networkId".to_string())?;
        if actual != network_id {
            return Err(format!(
                "SharedWorker started network {actual}, expected {network_id}"
            ));
        }
        self.status.network_id.set(network_id);
        self.status.configured.set(true);
        if let Some(profile) = crate::network_profile::profile_for_swarm_network_id(network_id) {
            crate::network_profile::activate_profile(profile);
        }
        Ok(())
    }
}

impl Drop for SharedRuntime {
    fn drop(&mut self) {
        unregister_service_worker_relay(self.service_worker_relay_id);
        if let Some(window) = web_sys::window() {
            let _ = window.remove_event_listener_with_callback(
                "pagehide",
                self._pagehide_listener.as_ref().unchecked_ref(),
            );
        }
        let _ = self.notify(&request_object(
            "WEEB3_CLIENT_CLOSE",
            self.status.network_id.get(),
        ));
        self.port.set_onmessage(None);
        self.port.close();
    }
}

impl SharedNodeClient {
    pub(crate) fn new(initial_network_id: u64, shared_worker_url: Option<&str>) -> Self {
        let status = Rc::new(RuntimeStatus {
            network_id: Cell::new(initial_network_id),
            configured: Cell::new(false),
        });
        Self {
            runtime: create_runtime(status.clone(), shared_worker_url).map(Rc::new),
            status,
            seen_log_sequence: Cell::new(0),
            transition: Mutex::new(()),
        }
    }

    pub(crate) async fn ensure(&self) -> Result<Rc<SharedRuntime>, String> {
        let runtime = self.runtime.clone()?;
        if self.status.configured.get()
            && let Some(_guard) = self.transition.try_lock()
        {
            return Ok(runtime);
        }
        let _guard = self.transition.lock().await;
        if !self.status.configured.get() {
            runtime.start(self.network_id()).await?;
        }
        Ok(runtime)
    }

    pub(crate) async fn configure(
        &self,
        network_id: u64,
        bootstrap_nodes: Vec<(String, bool)>,
    ) -> Result<(), String> {
        let _guard = self.transition.lock().await;
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        runtime.start(network_id).await?;
        if !bootstrap_nodes.is_empty() {
            let request = node_request_object("connectBootnodes", network_id);
            set(&request, "nodes", bootnodes_to_js(&bootstrap_nodes).into());
            let result = runtime
                .request(&request, CONTROL_TIMEOUT)
                .await
                .and_then(|response| require_ok(&response, "connect bootnodes"));
            if let Err(error) = result {
                self.interface_log(format!(
                    "SharedWorker network {network_id} started; bootnode configuration was best-effort: {error}"
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn network_id(&self) -> u64 {
        self.status.network_id.get()
    }

    pub(crate) fn interface_log(&self, message: impl AsRef<str>) {
        let message = message.as_ref();
        if let Ok(runtime) = self.runtime.as_ref()
            && self.status.configured.get()
        {
            let request = node_request_object("log", self.network_id());
            set_string(&request, "message", message);
            let _ = runtime.notify(&request);
            return;
        }
        web_sys::console::log_1(&JsValue::from_str(message));
    }

    pub(crate) fn claim_vault_broker(&self) {
        let Ok(runtime) = self.runtime.as_ref() else {
            return;
        };
        let request = request_object("WEEB3_VAULT_BROKER_CLAIM", self.network_id());
        let _ = runtime.notify(&request);
    }

    pub(crate) fn clear_hls_cache(&self) {
        let Ok(runtime) = self.runtime.as_ref() else {
            return;
        };
        let request = request_object("WEEB3_HLS_CLEAR_CACHE", self.network_id());
        let _ = runtime.notify(&request);
    }

    fn node_operation<'a>(
        &'a self,
        op: &'a str,
        populate: impl FnOnce(&Object),
    ) -> impl Future<Output = Result<Object, String>> + 'a {
        let request = node_request_object(op, self.network_id());
        populate(&request);
        self.send_node_operation(op, request)
    }

    async fn send_node_operation(&self, op: &str, request: Object) -> Result<Object, String> {
        let runtime = self.ensure().await?;
        let transfer_bearing = matches!(
            op,
            "acquire"
                | "retrieveBytes"
                | "retrieveChunk"
                | "acquireFeed"
                | "upload"
                | "pushChunk"
                | "resolveBzz"
                | "acquireRange"
        );
        let response = if transfer_bearing {
            runtime.request_unbounded(&request).await?
        } else {
            runtime.request(&request, CONTROL_TIMEOUT).await?
        };
        require_ok(&response, op)?;
        Ok(response)
    }

    pub(crate) async fn runtime_snapshot(
        &self,
        seen_progress_revision: u64,
    ) -> Option<RuntimeSnapshot> {
        self.runtime_snapshot_options(seen_progress_revision, true, true)
            .await
    }

    async fn runtime_snapshot_options(
        &self,
        seen_progress_revision: u64,
        include_logs: bool,
        include_progress: bool,
    ) -> Option<RuntimeSnapshot> {
        let runtime = self.ensure().await.ok()?;
        let request = request_object("WEEB3_RUNTIME_SNAPSHOT", self.network_id());
        set_bool(&request, "includeLogs", include_logs);
        set_bool(&request, "includeProgress", include_progress);
        set_number(
            &request,
            "seenLogSequence",
            self.seen_log_sequence.get() as f64,
        );
        set_number(
            &request,
            "seenProgressRevision",
            seen_progress_revision as f64,
        );
        let response = runtime.request(&request, CONTROL_TIMEOUT).await.ok()?;
        require_ok(&response, "SharedWorker snapshot").ok()?;
        let logs = if include_logs {
            let log_sequence = integer_property(&response, "logSequence")?;
            self.seen_log_sequence.set(log_sequence);
            if let Some(values) = array_property(&response, "logs") {
                values
                    .iter()
                    .filter_map(|value| value.as_string())
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let progress =
            if include_progress && bool_property(&response, "progressChanged") == Some(true) {
                let revision = integer_property(&response, "progressRevision")?;
                let rows = array_property(&response, "progressRows")?
                    .iter()
                    .filter_map(|value| progress_from_js(&value))
                    .collect();
                Some((revision, rows))
            } else {
                None
            };
        Some(RuntimeSnapshot {
            connections: integer_property(&response, "connections")?,
            ongoing_connections: integer_property(&response, "ongoingConnections")?,
            paused: bool_property(&response, "paused")?,
            logs,
            progress,
        })
    }

    pub(crate) async fn get_connections(&self) -> u64 {
        self.node_operation("connections", |_| {})
            .await
            .ok()
            .and_then(|response| integer_property(&response, "connections"))
            .unwrap_or_default()
    }

    pub(crate) async fn get_current_logs(&self) -> Vec<String> {
        self.runtime_snapshot_options(0, true, false)
            .await
            .map(|snapshot| snapshot.logs)
            .unwrap_or_default()
    }

    pub(crate) async fn get_progress_snapshot(
        &self,
        seen_revision: u64,
    ) -> Option<(u64, Vec<ProgressRow>)> {
        self.runtime_snapshot_options(seen_revision, false, true)
            .await?
            .progress
    }

    pub(crate) async fn toggle_transfer_pause(&self) -> bool {
        self.node_operation("toggleTransferPause", |_| {})
            .await
            .ok()
            .and_then(|response| bool_property(&response, "paused"))
            .unwrap_or_default()
    }

    pub(crate) async fn start_progress(
        &self,
        kind: impl AsRef<str>,
        subject: impl AsRef<str>,
        phase: impl AsRef<str>,
        percent: Option<u8>,
        detail: impl AsRef<str>,
    ) -> String {
        self.node_operation("progressStart", |request| {
            set_string(request, "kind", kind);
            set_string(request, "subject", subject);
            set_string(request, "phase", phase);
            set_optional_percent(request, percent);
            set_string(request, "detail", detail);
        })
        .await
        .ok()
        .and_then(|response| string_property(&response, "id"))
        .unwrap_or_default()
    }

    pub(crate) async fn update_progress(
        &self,
        id: &str,
        phase: impl AsRef<str>,
        percent: Option<u8>,
        detail: impl AsRef<str>,
    ) {
        let _ = self
            .node_operation("progressUpdate", |request| {
                set_string(request, "id", id);
                set_string(request, "phase", phase);
                set_optional_percent(request, percent);
                set_string(request, "detail", detail);
            })
            .await;
    }

    pub(crate) async fn finish_progress(
        &self,
        id: &str,
        phase: impl AsRef<str>,
        detail: impl AsRef<str>,
        ok: bool,
    ) {
        let _ = self
            .node_operation("progressFinish", |request| {
                set_string(request, "id", id);
                set_string(request, "phase", phase);
                set_string(request, "detail", detail);
                set_bool(request, "ok", ok);
            })
            .await;
    }

    pub(crate) async fn acquire(&self, address: String) -> Vec<u8> {
        self.bytes_operation("acquire", |request| set_string(request, "address", address))
            .await
    }

    pub(crate) async fn retrieve_bytes(&self, address: String) -> Uint8Array {
        self.typed_bytes_operation("retrieveBytes", |request| {
            set_string(request, "address", address)
        })
        .await
    }

    pub(crate) async fn retrieve_chunk_bytes(&self, address: String) -> Uint8Array {
        self.typed_bytes_operation("retrieveChunk", |request| {
            set_string(request, "address", address)
        })
        .await
    }

    pub(crate) async fn acquire_feed_envelope(&self, owner: String, topic: String) -> Vec<u8> {
        self.bytes_operation("acquireFeed", |request| {
            set_string(request, "owner", owner);
            set_string(request, "topic", topic);
        })
        .await
    }

    pub(crate) async fn resolve_bzz(&self, resource: String) -> Option<BzzMetadata> {
        let response = self
            .node_operation("resolveBzz", |request| {
                set_string(request, "resource", resource)
            })
            .await
            .ok()?;
        metadata_from_js(&response)
    }

    pub(crate) async fn acquire_resolved_range(
        &self,
        metadata: BzzMetadata,
        start: u64,
        end: u64,
    ) -> Option<(Vec<u8>, BzzMetadata)> {
        let response = self
            .node_operation("acquireRange", |request| {
                set(request, "metadata", metadata_to_js(&metadata).into());
                set_number(request, "start", start as f64);
                set_number(request, "end", end as f64);
            })
            .await
            .ok()?;
        let body = bytes_from_js(&response, "body")?;
        let metadata = metadata_from_js(&property(&response, "metadata")).unwrap_or(metadata);
        Some((body, metadata))
    }

    pub(crate) async fn post_upload_with_redundancy(
        &self,
        file: File,
        encryption: bool,
        redundancy_level: RedundancyLevel,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Vec<u8> {
        self.bytes_operation("upload", |request| {
            set(request, "file", file.into());
            set_bool(request, "encryption", encryption);
            set_number(
                request,
                "redundancyLevel",
                f64::from(redundancy_level.as_u8()),
            );
            set_string(request, "indexString", index_string);
            set_bool(request, "addToFeed", add_to_feed);
            set_string(request, "feedTopic", feed_topic);
        })
        .await
    }

    pub(crate) async fn post_push_chunk(
        &self,
        data: Uint8Array,
        soc: bool,
        chunk_address: Uint8Array,
        stamp: Uint8Array,
    ) -> Vec<u8> {
        self.bytes_operation("pushChunk", |request| {
            set(request, "data", data.into());
            set_bool(request, "soc", soc);
            set(request, "chunkAddress", chunk_address.into());
            set(request, "stamp", stamp.into());
        })
        .await
    }

    pub(crate) async fn reset_stamp(&self) -> Vec<u8> {
        self.bytes_operation("resetStamp", |_| {}).await
    }

    async fn bytes_operation(&self, op: &str, populate: impl FnOnce(&Object)) -> Vec<u8> {
        let Ok(response) = self.node_operation(op, populate).await else {
            return Vec::new();
        };
        bytes_from_js(&response, "body").unwrap_or_default()
    }

    async fn typed_bytes_operation(&self, op: &str, populate: impl FnOnce(&Object)) -> Uint8Array {
        self.node_operation(op, populate)
            .await
            .ok()
            .and_then(|response| property(&response, "body").dyn_into().ok())
            .unwrap_or_default()
    }
}

fn node_request_object(op: &str, network_id: u64) -> Object {
    let request = request_object("WEEB3_NODE_REQUEST", network_id);
    set_string(&request, "op", op);
    request
}

fn bootnodes_to_js(nodes: &[(String, bool)]) -> Array {
    let values = Array::new();
    for (address, usable) in nodes {
        let node = Object::new();
        set_string(&node, "address", address);
        set_bool(&node, "usable", *usable);
        values.push(&node);
    }
    values
}

pub(crate) fn request_object(kind: &str, network_id: u64) -> Object {
    let request = Object::new();
    set_string(&request, "type", kind);
    set_number(&request, "protocol", SHARED_WORKER_PROTOCOL as f64);
    set_number(&request, "networkId", network_id as f64);
    request
}

fn require_ok(response: &Object, context: &str) -> Result<(), String> {
    if bool_property(response, "ok") == Some(true) {
        return Ok(());
    }
    Err(string_property(response, "error").unwrap_or_else(|| format!("{context} failed")))
}

fn js_error(context: &str, error: &JsValue) -> String {
    let message = string_property(error, "message")
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "unknown browser error".to_string());
    format!("{context}: {message}")
}
