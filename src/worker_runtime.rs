use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
};

use async_lock::Mutex;
use async_std::sync::Arc;
use js_sys::{Array, Object};
use wasm_bindgen::{JsCast, JsValue, prelude::*};
use wasm_bindgen_futures::spawn_local;
use web_sys::File;

use crate::{
    Weeb3,
    bzz_stream::BzzMetadata,
    network_profile::{is_browser_dialable_underlay, profile_for_swarm_network_id},
    worker_protocol::{
        array_property, bool_property, bytes_from_js, bytes_to_js, integer_property,
        metadata_from_js, metadata_to_js, number_property, progress_to_js, property, set, set_bool,
        set_number, set_string, string_property,
    },
};

const WORKER_LOG_RING_CAPACITY: usize = 256;

#[derive(Default)]
struct RuntimeState {
    bootnodes: HashMap<String, bool>,
    log_sequence: u64,
    logs: VecDeque<(u64, String)>,
}

#[wasm_bindgen]
pub struct Weeb3WorkerRuntime {
    pub(crate) inner: Arc<Weeb3>,
    configured_network_id: Cell<Option<u64>>,
    state: RefCell<RuntimeState>,
    transition: Mutex<()>,
}

#[wasm_bindgen]
impl Weeb3WorkerRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Weeb3::new()),
            configured_network_id: Cell::new(None),
            state: RefCell::new(RuntimeState::default()),
            transition: Mutex::new(()),
        }
    }

    #[wasm_bindgen(js_name = start)]
    pub async fn start(&self, options: JsValue) -> Object {
        let network_id = match required_network_id(&options) {
            Some(network_id) => network_id,
            None => return error_response(400, "start requires a supported networkId"),
        };

        let _transition = self.transition.lock().await;
        let configured_network_id = self.configured_network_id.get();
        if configured_network_id != Some(network_id) {
            if configured_network_id.is_some() && self.inner.has_unsettled_accounting().await {
                return error_response(
                    409,
                    "cannot switch Swarm network while dispatched accounting is settling",
                );
            }
            crate::stream::begin_result_view_request();
            crate::stream_hls::release_hls_runtime();
            if !self.inner.set_network_id(network_id.to_string()).await {
                return error_response(400, "unsupported weeb-3 networkId");
            }
            self.state.borrow_mut().bootnodes.clear();
            self.configured_network_id.set(Some(network_id));
            crate::network_profile::activate_profile(
                profile_for_swarm_network_id(network_id).expect("validated worker network"),
            );
        }
        if !self.inner.runtime_is_started() {
            let inner = self.inner.clone();
            spawn_local(async move {
                inner.run().await;
            });
            async_std::task::yield_now().await;
        }

        let response = ok_response();
        set_number(&response, "networkId", network_id as f64);
        response
    }

    #[wasm_bindgen(js_name = handleMessage)]
    pub async fn handle_message(&self, message: JsValue) -> Object {
        self.dispatch_message(message)
            .await
            .unwrap_or_else(|response| response)
    }
}

impl Weeb3WorkerRuntime {
    async fn dispatch_message(&self, message: JsValue) -> Result<Object, Object> {
        let message = message
            .dyn_into::<Object>()
            .map_err(|_| error_response(400, "weeb-3 message must be an object"))?;
        let request_type = string_property(&message, "type").unwrap_or_default();

        if let Some(response) =
            crate::stream_hls::worker_bridge::dispatch(self, &request_type, &message).await
        {
            return Ok(response);
        }

        match request_type.as_str() {
            "WEEB3_FETCH_REQUEST" => self.fetch_response(&message).await,
            "WEEB3_NODE_REQUEST" => self.node_response(&message).await,
            "UPLOAD_REQUEST" => self.upload_response(&message).await,
            "WEEB3_RUNTIME_SNAPSHOT" => self.runtime_snapshot_response(&message).await,
            "" => Err(error_response(400, "weeb-3 message type is required")),
            unsupported => Err(error_response(
                400,
                format!("unsupported weeb-3 message type: {unsupported}"),
            )),
        }
    }

    pub(crate) async fn request_network(&self, message: &Object) -> Result<u64, Object> {
        let network_id = required_network_id(message)
            .ok_or_else(|| error_response(400, "weeb-3 request requires a supported networkId"))?;
        if self.configured_network_id.get() != Some(network_id)
            || !self.inner.runtime_is_started()
            || self.inner.service_worker_network_id() != network_id
        {
            return Err(error_response(409, "request targets another Swarm network"));
        }
        Ok(network_id)
    }

    async fn node_response(&self, message: &Object) -> Result<Object, Object> {
        let network_id = self.request_network(message).await?;
        let op = string_property(message, "op")
            .ok_or_else(|| error_response(400, "node request requires op"))?;

        match op.as_str() {
            "connectBootnodes" => self.connect_bootnodes_response(message, network_id).await,
            "connections" => {
                let response = ok_response();
                set_number(
                    &response,
                    "connections",
                    self.inner.get_connections().await as f64,
                );
                Ok(response)
            }
            "log" => {
                let value = string_property(message, "message")
                    .ok_or_else(|| error_response(400, "log requires message"))?;
                self.inner.interface_log(value);
                Ok(ok_response())
            }
            "toggleTransferPause" => {
                let response = ok_response();
                set_bool(
                    &response,
                    "paused",
                    self.inner.toggle_transfer_pause().await,
                );
                Ok(response)
            }
            "progressStart" => {
                let kind = string_property(message, "kind")
                    .ok_or_else(|| error_response(400, "progressStart requires kind"))?;
                let subject = string_property(message, "subject")
                    .ok_or_else(|| error_response(400, "progressStart requires subject"))?;
                let phase = string_property(message, "phase")
                    .ok_or_else(|| error_response(400, "progressStart requires phase"))?;
                let response = ok_response();
                set_string(
                    &response,
                    "id",
                    self.inner
                        .start_progress(
                            kind,
                            subject,
                            phase,
                            percent_property(message),
                            string_property(message, "detail").unwrap_or_default(),
                        )
                        .await,
                );
                Ok(response)
            }
            "progressUpdate" => {
                let id = string_property(message, "id")
                    .ok_or_else(|| error_response(400, "progressUpdate requires id"))?;
                let phase = string_property(message, "phase")
                    .ok_or_else(|| error_response(400, "progressUpdate requires phase"))?;
                self.inner
                    .update_progress(
                        &id,
                        phase,
                        percent_property(message),
                        string_property(message, "detail").unwrap_or_default(),
                    )
                    .await;
                Ok(ok_response())
            }
            "progressFinish" => {
                let id = string_property(message, "id")
                    .ok_or_else(|| error_response(400, "progressFinish requires id"))?;
                let phase = string_property(message, "phase")
                    .ok_or_else(|| error_response(400, "progressFinish requires phase"))?;
                let ok = bool_property(message, "ok")
                    .ok_or_else(|| error_response(400, "progressFinish requires ok"))?;
                self.inner
                    .finish_progress(
                        &id,
                        phase,
                        string_property(message, "detail").unwrap_or_default(),
                        ok,
                    )
                    .await;
                Ok(ok_response())
            }
            op @ ("acquire" | "retrieveBytes" | "retrieveChunk") => {
                let address = string_property(message, "address")
                    .ok_or_else(|| error_response(400, format!("{op} requires address")))?;
                Ok(bytes_response(match op {
                    "acquire" => self.inner.acquire(address).await,
                    "retrieveBytes" => self.inner.retrieve_bytes(address).await,
                    "retrieveChunk" => self.inner.retrieve_chunk_bytes(address).await,
                    _ => unreachable!(),
                }))
            }
            "acquireFeed" => {
                let owner = string_property(message, "owner")
                    .ok_or_else(|| error_response(400, "acquireFeed requires owner"))?;
                let topic = string_property(message, "topic")
                    .ok_or_else(|| error_response(400, "acquireFeed requires topic"))?;
                Ok(bytes_response(
                    self.inner.acquire_feed_envelope(owner, topic).await,
                ))
            }
            "upload" => self.upload_response(message).await,
            "pushChunk" => self.push_chunk_response(message).await,
            "resetStamp" => Ok(bytes_response(self.inner.reset_stamp().await)),
            "resolveBzz" => {
                let resource = string_property(message, "resource")
                    .ok_or_else(|| error_response(400, "resolveBzz requires resource"))?;
                self.inner
                    .resolve_bzz(resource)
                    .await
                    .map(metadata_response)
                    .ok_or_else(|| error_response(404, "BZZ resource was not resolved"))
            }
            "acquireRange" => self.acquire_range_response(message).await,
            _ => Err(error_response(400, format!("unsupported node op: {op}"))),
        }
    }

    async fn connect_bootnodes_response(
        &self,
        message: &Object,
        network_id: u64,
    ) -> Result<Object, Object> {
        let values = array_property(message, "nodes")
            .ok_or_else(|| error_response(400, "connectBootnodes requires nodes array"))?;
        let nodes = {
            let mut nodes = Vec::new();
            let mut state = self.state.borrow_mut();
            for value in values.iter() {
                let node = value
                    .dyn_into::<Object>()
                    .map_err(|_| error_response(400, "bootnode must be an object"))?;
                let address = string_property(&node, "address")
                    .ok_or_else(|| error_response(400, "bootnode requires address"))?;
                if !is_browser_dialable_underlay(&address) {
                    return Err(error_response(
                        400,
                        "bootnode address is not browser-dialable",
                    ));
                }
                let usable = bool_property(&node, "usable").unwrap_or(true);
                if state.bootnodes.get(&address) == Some(&usable) {
                    continue;
                }
                state.bootnodes.insert(address.clone(), usable);
                if state.bootnodes.len() > 512 {
                    state.bootnodes.clear();
                    state.bootnodes.insert(address.clone(), usable);
                }
                nodes.push((address, usable));
            }
            nodes
        };
        if !nodes.is_empty() {
            self.inner
                .connect_bootnodes_for_current_network(nodes, network_id)
                .await;
        }
        Ok(ok_response())
    }

    async fn upload_response(&self, message: &Object) -> Result<Object, Object> {
        self.request_network(message).await?;
        let file = property(message, "file")
            .dyn_into::<File>()
            .map_err(|_| error_response(400, "upload requires File"))?;
        let encryption = bool_property(message, "encryption")
            .ok_or_else(|| error_response(400, "upload requires encryption"))?;
        let redundancy = number_property(message, "redundancyLevel")
            .and_then(crate::erasure_coding::validated_upload_redundancy_number)
            .ok_or_else(|| {
                error_response(400, "upload requires redundancyLevel from 0 through 4")
            })?;
        let add_to_feed = bool_property(message, "addToFeed").unwrap_or(false);
        let result = self
            .inner
            .post_upload_with_redundancy(
                file,
                encryption,
                redundancy,
                string_property(message, "indexString").unwrap_or_default(),
                add_to_feed,
                string_property(message, "feedTopic").unwrap_or_default(),
            )
            .await;
        if string_property(message, "type").as_deref() == Some("UPLOAD_REQUEST") {
            let (resources, reference) = crate::decode_resources(result);
            if reference.is_empty() {
                let error = resources
                    .first()
                    .and_then(|(body, _, _)| std::str::from_utf8(body).ok())
                    .filter(|message| !message.trim().is_empty())
                    .unwrap_or("upload failed before returning a reference");
                return Err(error_response(500, error));
            }
            let response = ok_response();
            set_string(&response, "reference", reference);
            return Ok(response);
        }
        Ok(bytes_response(result))
    }

    async fn push_chunk_response(&self, message: &Object) -> Result<Object, Object> {
        let data = bytes_from_js(message, "data")
            .ok_or_else(|| error_response(400, "pushChunk requires data"))?;
        let address = bytes_from_js(message, "chunkAddress")
            .ok_or_else(|| error_response(400, "pushChunk requires chunkAddress"))?;
        let stamp = bytes_from_js(message, "stamp")
            .ok_or_else(|| error_response(400, "pushChunk requires stamp"))?;
        let soc = bool_property(message, "soc")
            .ok_or_else(|| error_response(400, "pushChunk requires soc"))?;
        Ok(bytes_response(
            self.inner.post_push_chunk(data, soc, address, stamp).await,
        ))
    }

    async fn acquire_range_response(&self, message: &Object) -> Result<Object, Object> {
        let metadata = metadata_from_js(&property(message, "metadata"))
            .ok_or_else(|| error_response(400, "acquireRange requires valid metadata"))?;
        let start = integer_property(message, "start")
            .ok_or_else(|| error_response(400, "acquireRange requires start"))?;
        let end = integer_property(message, "end")
            .ok_or_else(|| error_response(400, "acquireRange requires end"))?;
        if start > end || end >= metadata.size {
            return Err(error_response(
                416,
                "requested range is outside the resource",
            ));
        }
        self.inner
            .acquire_resolved_range(metadata, start, end)
            .await
            .map(|(body, metadata)| {
                let response = bytes_response(body);
                set(&response, "metadata", metadata_to_js(&metadata).into());
                response
            })
            .ok_or_else(|| error_response(502, "range retrieval failed"))
    }

    async fn fetch_response(&self, message: &Object) -> Result<Object, Object> {
        self.request_network(message).await?;
        crate::stream::service_worker_message_response(message, self.inner.clone())
            .await
            .ok_or_else(|| error_response(400, "unsupported service-worker request"))
    }

    async fn runtime_snapshot_response(&self, message: &Object) -> Result<Object, Object> {
        self.request_network(message).await?;
        let seen_logs = integer_property(message, "seenLogSequence").unwrap_or(0);
        let seen_progress = integer_property(message, "seenProgressRevision").unwrap_or(0);
        let include_logs = bool_property(message, "includeLogs").unwrap_or(true);
        let include_progress = bool_property(message, "includeProgress").unwrap_or(true);
        let fresh_logs = self.inner.get_current_logs();
        let log_snapshot = {
            let mut state = self.state.borrow_mut();
            for log in fresh_logs {
                state.log_sequence += 1;
                let sequence = state.log_sequence;
                state.logs.push_back((sequence, log));
                if state.logs.len() > WORKER_LOG_RING_CAPACITY {
                    state.logs.pop_front();
                }
            }
            include_logs.then(|| {
                let logs = (seen_logs < state.log_sequence).then(|| {
                    let logs = Array::new();
                    for (_, log) in state
                        .logs
                        .iter()
                        .filter(|(sequence, _)| *sequence > seen_logs)
                    {
                        logs.push(&JsValue::from_str(log));
                    }
                    logs
                });
                (state.log_sequence, logs)
            })
        };

        let (connections, ongoing_connections) = self.inner.connection_counts().await;
        let response = ok_response();
        set_number(&response, "connections", connections as f64);
        set_number(&response, "ongoingConnections", ongoing_connections as f64);
        set_bool(&response, "paused", self.inner.transfer_paused());
        if let Some((log_sequence, logs)) = log_snapshot {
            set_number(&response, "logSequence", log_sequence as f64);
            if let Some(logs) = logs {
                set(&response, "logs", logs.into());
            }
        }

        if include_progress {
            if let Some((revision, rows)) = self.inner.get_progress_snapshot(seen_progress).await {
                set(&response, "progressChanged", JsValue::TRUE);
                set_number(&response, "progressRevision", revision as f64);
                let values = Array::new();
                for row in rows {
                    values.push(&progress_to_js(row));
                }
                set(&response, "progressRows", values.into());
            } else {
                set(&response, "progressChanged", JsValue::FALSE);
            }
        }
        Ok(response)
    }
}

fn required_network_id(value: &JsValue) -> Option<u64> {
    let network_id = integer_property(value, "networkId")?;
    profile_for_swarm_network_id(network_id).map(|profile| profile.swarm_network_id)
}

fn percent_property(value: &JsValue) -> Option<u8> {
    integer_property(value, "percent").and_then(|value| u8::try_from(value).ok())
}

fn bytes_response(body: Vec<u8>) -> Object {
    let response = ok_response();
    set(&response, "body", bytes_to_js(&body).into());
    response
}

fn metadata_response(metadata: BzzMetadata) -> Object {
    let response = metadata_to_js(&metadata);
    set(&response, "ok", JsValue::TRUE);
    response
}

pub(crate) fn ok_response() -> Object {
    let response = Object::new();
    set(&response, "ok", JsValue::TRUE);
    response
}

pub(crate) fn error_response(status: u16, error: impl AsRef<str>) -> Object {
    let response = Object::new();
    set(&response, "ok", JsValue::FALSE);
    set_number(&response, "status", f64::from(status));
    set_string(&response, "error", error);
    response
}
