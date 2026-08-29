#![cfg(target_arch = "wasm32")]

use crate::PrivateKeySigner;
use crate::{
    decode_resources, encrey,
    erasure_coding::RedundancyLevel,
    erasure_coding::validated_upload_redundancy_number,
    interface::{
        begin_interface_mount, get_service_worker, install_bfcache_restore_guard,
        mount_interface_with_generation, release_result_object_url,
    },
    interface_conventions::render_interface_shell,
    nav::route_network_mode_from_location,
    network_profile::{
        NetworkMode, NetworkProfile, activate_profile, active_profile, initial_bootnodes,
        is_browser_dialable_underlay, profile_for_mode, profile_for_swarm_network_id,
    },
    normalize_feed_topic,
    on_chain::{
        buy_postage_batch_with_payer, chequebook_balance, chunk_count_for_depth,
        compute_initial_balance_per_chunk, deploy_chequebook_with_payer, deposit_to_chequebook,
        get_batch_validity, last_price, postage_contract, token_contract, web3,
    },
    persistence::{
        get_chequebook_address, get_chequebook_signer_key, set_chequebook_address,
        set_chequebook_signer_key,
    },
    secure_vault::{
        secure_batch_state_for_wallet, secure_commit_batch_purchase_and_verify,
        secure_ensure_feed_owner_in_window, secure_open_vault_from_user_action,
        secure_prepare_batch_purchase,
    },
    shared_runtime::SharedNodeClient,
    stream_conventions::{HlsStart, StreamShareRoute},
    strip_hex_prefix,
    worker_protocol::{
        bytes_to_js, progress_to_js as progress_row_to_js, property, set as set_js,
        set_bool as set_js_bool, set_number as set_js_number, set_string as set_js_str,
    },
};
use event_listener::Event;
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    str::FromStr,
    time::Duration,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Element, File, FilePropertyBag, HtmlMediaElement};
use web3::types::{Address, U256};

#[wasm_bindgen(typescript_custom_section)]
const UPLOAD_REDUNDANCY_TYPES: &str = r#"
/** Bee-compatible erasure-coding level used for uploads. */
export type UploadRedundancyLevel = 0 | 1 | 2 | 3 | 4;
export type HlsStart = "beginning" | "live";
"#;

fn resource_to_js(bytes: Vec<u8>, mime: String, path: String) -> Object {
    let obj = Object::new();
    set_js(&obj, "body", bytes_to_js(&bytes).into());
    set_js_str(&obj, "mime", mime);
    set_js_str(&obj, "path", path);
    obj
}

fn make_js_file(bytes: Uint8Array, mime: &str, name: &str) -> File {
    let parts = Array::new();
    parts.push(&bytes);

    let bag = FilePropertyBag::new();
    bag.set_type(mime);
    bag.set_last_modified(js_sys::Date::now());

    File::new_with_u8_array_sequence_and_options(&parts, name, &bag).unwrap()
}

fn make_upload_file(bytes: Uint8Array, mime: String, name: String) -> File {
    let mime = if mime.is_empty() {
        "application/octet-stream"
    } else {
        &mime
    };
    let name = if name.is_empty() { "bytes" } else { &name };
    make_js_file(bytes, mime, name)
}

fn ok_object() -> Object {
    let obj = Object::new();
    set_js_str(&obj, "status", "ok");
    obj
}

fn error_object(error: impl AsRef<str>) -> Object {
    let obj = Object::new();
    set_js_str(&obj, "status", "error");
    set_js_str(&obj, "error", error);
    obj
}

fn u256_string(value: U256) -> JsValue {
    JsValue::from_str(&value.to_string())
}

fn hex_address(address: Address) -> String {
    format!("0x{}", hex::encode(address.as_bytes()))
}

fn normalize_feed_owner(owner: &str) -> Option<String> {
    let owner = owner.trim();
    if owner.is_empty() {
        return None;
    }

    match hex::decode(strip_hex_prefix(owner)) {
        Ok(bytes) if bytes.len() == 20 => Some(format!("0x{}", hex::encode(bytes))),
        _ => None,
    }
}

async fn feed_owner_for_request(owner: &str) -> Result<Option<String>, String> {
    let owner = owner.trim();
    if owner.is_empty() {
        return match secure_ensure_feed_owner_in_window().await {
            Some(owner) if owner.len() == 20 => Ok(Some(format!("0x{}", hex::encode(owner)))),
            Some(owner) => Err(format!("feed owner had invalid length {}", owner.len())),
            None => Err("feed owner unavailable".to_string()),
        };
    }

    normalize_feed_owner(owner)
        .map(Some)
        .ok_or_else(|| "invalid feed owner".to_string())
}

fn feed_status(
    data: &[(Vec<u8>, String, String)],
    connected: u64,
) -> Option<(&'static str, String)> {
    if !data.is_empty()
        && !data
            .iter()
            .all(|(_bytes, mime, path)| mime == "not found" || path == "not found")
    {
        return None;
    }

    let reason = data
        .first()
        .map(|(bytes, _mime, _path)| String::from_utf8_lossy(bytes).trim().to_string())
        .filter(|text| !text.is_empty());

    if let Some(reason) = reason {
        return Some(("error", reason));
    }

    if connected == 0 {
        Some(("network_error", "no connected peers".to_string()))
    } else {
        Some(("not_found", "feed update not found".to_string()))
    }
}

fn network_mode_from_input(mode: &str) -> Option<NetworkMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "mainnet" => Some(NetworkMode::Mainnet),
        "testnet" => Some(NetworkMode::Testnet),
        _ => None,
    }
}

fn network_mode_label(mode: NetworkMode) -> &'static str {
    match mode {
        NetworkMode::Mainnet => "mainnet",
        NetworkMode::Testnet => "testnet",
    }
}

fn network_profile_object(profile: NetworkProfile, current_network_id: u64) -> Object {
    let obj = ok_object();
    set_js_str(&obj, "mode", network_mode_label(profile.mode));
    set_js_number(&obj, "networkId", current_network_id as f64);
    set_js_number(&obj, "swarmNetworkId", profile.swarm_network_id as f64);
    set_js_number(&obj, "walletChainId", profile.wallet_chain_id as f64);
    set_js_str(&obj, "baseSymbol", profile.base_symbol);
    set_js_str(&obj, "bzzSymbol", profile.bzz_symbol);

    let bootnodes = Array::new();
    let browser_bootnodes = Array::new();
    let skipped_bootnodes = Array::new();
    for address in profile.bootnodes {
        bootnodes.push(&JsValue::from_str(address));
        if is_browser_dialable_underlay(address) {
            browser_bootnodes.push(&JsValue::from_str(address));
        } else {
            skipped_bootnodes.push(&JsValue::from_str(address));
        }
    }
    set_js(&obj, "bootnodes", bootnodes.into());
    set_js(&obj, "browserBootnodes", browser_bootnodes.into());
    set_js(&obj, "skippedBootnodes", skipped_bootnodes.into());
    obj
}

struct StartBootstrapNode {
    multiaddr: String,
    usable: bool,
}

struct StartOptions {
    network_id: String,
    bootstrap_nodes: Vec<StartBootstrapNode>,
}

fn profile_bootstrap_nodes(profile: NetworkProfile) -> Vec<StartBootstrapNode> {
    initial_bootnodes(profile)
        .into_iter()
        .map(|address| StartBootstrapNode {
            multiaddr: address.to_string(),
            usable: true,
        })
        .collect()
}

fn js_bool_prop(value: &JsValue, name: &str) -> Option<bool> {
    let prop = property(value, name);
    (!prop.is_null() && !prop.is_undefined())
        .then(|| prop.as_bool())
        .flatten()
}

fn js_string_prop(value: &JsValue, name: &str) -> Option<String> {
    let prop = property(value, name);
    if prop.is_null() || prop.is_undefined() {
        return None;
    }

    if let Some(text) = prop.as_string() {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    prop.as_f64().and_then(|number| {
        if number.is_finite() && number >= 0.0 {
            Some((number as u64).to_string())
        } else {
            None
        }
    })
}

fn parse_start_bootstrap_node(value: JsValue) -> Option<StartBootstrapNode> {
    let multiaddr = js_string_prop(&value, "multiaddr")?;
    let usable = js_bool_prop(&value, "usable").unwrap_or(true);
    Some(StartBootstrapNode { multiaddr, usable })
}

fn parse_start_bootstrap_nodes(value: JsValue) -> Vec<StartBootstrapNode> {
    if !Array::is_array(&value) {
        return vec![];
    }

    Array::from(&value)
        .iter()
        .filter_map(parse_start_bootstrap_node)
        .collect()
}

fn start_options_from_js(options: Option<JsValue>) -> StartOptions {
    let options = options.unwrap_or(JsValue::UNDEFINED);
    let bare_bootnodes = Array::is_array(&options);

    let explicit_network_id = if bare_bootnodes {
        None
    } else {
        js_string_prop(&options, "networkId")
    };

    let mode = if js_bool_prop(&options, "testnet").unwrap_or(false) {
        NetworkMode::Testnet
    } else if js_bool_prop(&options, "mainnet").unwrap_or(false) {
        NetworkMode::Mainnet
    } else if let Some(network_id) = explicit_network_id.as_deref() {
        network_id
            .parse::<u64>()
            .ok()
            .and_then(profile_for_swarm_network_id)
            .map(|profile| profile.mode)
            .unwrap_or(NetworkMode::Mainnet)
    } else {
        route_network_mode_from_location().unwrap_or(NetworkMode::Mainnet)
    };

    let profile = profile_for_mode(mode);
    let network_id = explicit_network_id.unwrap_or_else(|| profile.swarm_network_id.to_string());
    let configured_nodes = if bare_bootnodes {
        parse_start_bootstrap_nodes(options)
    } else {
        parse_start_bootstrap_nodes(property(&options, "bootstrapNodes"))
    };

    let bootstrap_nodes = if configured_nodes.is_empty() {
        profile_bootstrap_nodes(profile)
    } else {
        configured_nodes
    };

    StartOptions {
        network_id,
        bootstrap_nodes,
    }
}

async fn call_promise(
    function: &Function,
    this: &JsValue,
    args: &Array,
) -> Result<JsValue, String> {
    let promise = function
        .apply(this, args)
        .map_err(|e| format!("{e:?}"))?
        .dyn_into::<Promise>()
        .map_err(|_| "wallet call did not return a promise".to_string())?;
    JsFuture::from(promise).await.map_err(|e| format!("{e:?}"))
}

pub(crate) async fn request_wallet_via_shell_connector(
    chain_id: u64,
) -> Option<Result<Address, String>> {
    let window = web_sys::window()?;
    let function = Reflect::get(&window, &JsValue::from_str("weeb3EnsureEip1193"))
        .ok()?
        .dyn_into::<Function>()
        .ok()?;
    let args = Array::new();
    args.push(&JsValue::from_str("64c5f91181ce0a3192a783346a475d23"));
    args.push(&JsValue::from_f64(chain_id as f64));
    let value = match call_promise(&function, &JsValue::NULL, &args).await {
        Ok(value) => value,
        Err(error) => return Some(Err(error)),
    };

    let ok = Reflect::get(&value, &JsValue::from_str("ok"))
        .ok()
        .and_then(|ok| ok.as_bool())
        .unwrap_or(false);
    if !ok {
        let error = Reflect::get(&value, &JsValue::from_str("error"))
            .ok()
            .and_then(|error| error.as_string())
            .unwrap_or_else(|| "wallet connection failed".to_string());
        return Some(Err(error));
    }

    let accounts = Reflect::get(&value, &JsValue::from_str("accounts")).ok()?;
    let first = Array::from(&accounts).get(0).as_string();
    Some(
        first
            .ok_or_else(|| "wallet returned no accounts".to_string())
            .and_then(|address| {
                Address::from_str(&address).map_err(|_| "wallet account is invalid".to_string())
            }),
    )
}

async fn ethereum_request(method: &str, params: Option<Array>) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or_else(|| "window is not available".to_string())?;
    let ethereum = Reflect::get(&window, &JsValue::from_str("ethereum"))
        .map_err(|_| "window.ethereum is not available".to_string())?;
    if ethereum.is_null() || ethereum.is_undefined() {
        return Err("window.ethereum is not available".to_string());
    }

    let request = Reflect::get(&ethereum, &JsValue::from_str("request"))
        .map_err(|_| "ethereum.request is not available".to_string())?
        .dyn_into::<Function>()
        .map_err(|_| "ethereum.request is not callable".to_string())?;
    let payload = Object::new();
    set_js_str(&payload, "method", method);
    if let Some(params) = params {
        set_js(&payload, "params", params.into());
    }

    let args = Array::new();
    args.push(&payload);
    call_promise(&request, &ethereum, &args).await
}

async fn switch_injected_wallet_chain(chain_id: u64) -> Result<(), String> {
    let params = Array::new();
    let chain = Object::new();
    set_js_str(&chain, "chainId", format!("0x{chain_id:x}"));
    params.push(&chain);
    ethereum_request("wallet_switchEthereumChain", Some(params))
        .await
        .map(|_| ())
}

async fn request_wallet_address() -> Result<Address, String> {
    let chain_id = active_profile().wallet_chain_id;
    if let Some(result) = request_wallet_via_shell_connector(chain_id).await {
        return result;
    }

    switch_injected_wallet_chain(chain_id).await?;
    let accounts = ethereum_request("eth_requestAccounts", None).await?;
    let first = Array::from(&accounts).get(0).as_string();
    first
        .ok_or_else(|| "wallet returned no accounts".to_string())
        .and_then(|address| {
            Address::from_str(&address).map_err(|_| "wallet account is invalid".to_string())
        })
}

#[derive(Default)]
struct StartupState {
    configured: Cell<bool>,
    pending: Cell<usize>,
    idle: Event,
    error: RefCell<Option<String>>,
    serial: async_lock::Mutex<()>,
}

impl StartupState {
    fn begin(&self) {
        self.pending.set(self.pending.get().wrapping_add(1));
    }

    fn finish(&self, error: Option<String>) {
        *self.error.borrow_mut() = error;
        let remaining = self.pending.get() - 1;
        self.pending.set(remaining);
        if remaining == 0 {
            self.idle.notify(usize::MAX);
        }
    }

    async fn wait(&self) {
        loop {
            let idle = self.idle.listen();
            if self.pending.get() == 0 {
                return;
            }
            idle.await;
        }
    }
}

#[wasm_bindgen]
pub struct Weeb3No103 {
    inner: Rc<SharedNodeClient>,
    startup: Rc<StartupState>,
}

async fn configure_shared_node(
    inner: &SharedNodeClient,
    network_id: String,
    bootstrap_nodes: Vec<StartBootstrapNode>,
) -> Result<(), String> {
    let Ok(expected_network_id) = network_id.parse::<u64>() else {
        return Err(format!("invalid Swarm network id {network_id}"));
    };
    let mut seen = std::collections::HashSet::new();
    let mut dial_nodes = Vec::new();

    for node in bootstrap_nodes {
        if node.multiaddr.is_empty() || !seen.insert(node.multiaddr.clone()) {
            continue;
        }
        if !is_browser_dialable_underlay(&node.multiaddr) {
            inner.interface_log(format!(
                "Skipped non-browser bootnode for network {}: {}",
                network_id, node.multiaddr
            ));
            continue;
        }
        dial_nodes.push(node);
    }

    inner
        .configure(
            expected_network_id,
            dial_nodes
                .into_iter()
                .map(|node| (node.multiaddr, node.usable))
                .collect(),
        )
        .await
}

#[wasm_bindgen]
impl Weeb3No103 {
    #[wasm_bindgen(constructor)]
    pub fn new(shared_worker_url: Option<String>) -> Weeb3No103 {
        install_bfcache_restore_guard();
        Weeb3No103 {
            inner: Rc::new(SharedNodeClient::new(
                active_profile().swarm_network_id,
                shared_worker_url.as_deref(),
            )),
            startup: Rc::new(StartupState::default()),
        }
    }

    fn schedule_start(&self, options: StartOptions) {
        spawn_local(async {
            let _ = get_service_worker().await;
        });
        self.startup.configured.set(true);
        self.startup.begin();
        let inner = self.inner.clone();
        let startup = self.startup.clone();

        spawn_local(async move {
            let _guard = startup.serial.lock().await;
            let network_id = options.network_id;
            let current_network = inner.network_id();
            let network_switch = network_id
                .parse::<u64>()
                .ok()
                .is_some_and(|requested| requested != current_network);
            if network_switch {
                crate::stream::release_current_stream_view();
            }
            match configure_shared_node(&inner, network_id, options.bootstrap_nodes).await {
                Ok(()) => startup.finish(None),
                Err(error) => {
                    inner.interface_log(format!("SharedWorker startup failed: {error}"));
                    startup.finish(Some(error));
                }
            }
        });
    }

    async fn boot_runtime(&self) -> Result<(), String> {
        if !self.startup.configured.replace(true) {
            self.schedule_start(start_options_from_js(None));
        }
        self.startup.wait().await;
        if let Some(error) = self.startup.error.borrow().clone() {
            return Err(error);
        }
        self.inner.ensure().await.map(|_| ())
    }

    #[wasm_bindgen(js_name = start)]
    pub fn start(&self, options: Option<JsValue>) {
        self.schedule_start(start_options_from_js(options));
    }

    #[wasm_bindgen(js_name = renderInterface)]
    pub fn render_interface(&self, container: Element) -> Object {
        let mount_generation = begin_interface_mount();
        let initial_result_generation = crate::stream::begin_result_view_request();
        crate::stream::release_current_stream_view();
        release_result_object_url();
        let startup_already_configured = self.startup.configured.replace(true);
        self.startup.begin();
        render_interface_shell(&container);

        let s = self.inner.clone();
        let startup = self.startup.clone();
        spawn_local(async move {
            let guard = startup.serial.lock().await;
            let network_before_route = s.network_id();
            let route_mode = route_network_mode_from_location();
            let network_id = route_mode
                .map(|mode| profile_for_mode(mode).swarm_network_id)
                .unwrap_or(network_before_route);
            let route_changed_network = network_before_route != network_id;
            let bootstrap_nodes = profile_for_swarm_network_id(network_id)
                .map(profile_bootstrap_nodes)
                .unwrap_or_default();
            let startup_result = if !startup_already_configured || route_changed_network {
                configure_shared_node(&s, network_id.to_string(), bootstrap_nodes).await
            } else {
                s.ensure().await.map(|_| ())
            };
            if let Err(error) = startup_result {
                s.interface_log(format!("SharedWorker startup failed: {error}"));
                startup.finish(Some(error));
                return;
            }
            startup.finish(None);
            drop(guard);
            if let Err(error) = mount_interface_with_generation(
                s,
                Some(initial_result_generation),
                mount_generation,
            )
            .await
            {
                web_sys::console::error_1(&JsValue::from_str(&format!(
                    "weeb-3 interface mount failed: {error:?}"
                )));
            }
        });

        ok_object()
    }

    #[wasm_bindgen(js_name = attachStream)]
    pub async fn attach_stream(
        &self,
        #[wasm_bindgen(unchecked_param_type = "HTMLMediaElement")] media: Element,
        owner: String,
        topic: String,
        #[wasm_bindgen(unchecked_param_type = "HlsStart")] start: String,
    ) -> Result<(), JsValue> {
        if media.dyn_ref::<HtmlMediaElement>().is_none() {
            return Err(JsValue::from_str(
                "attachStream requires an HTML media element",
            ));
        }
        let start = match start.as_str() {
            "beginning" => HlsStart::Beginning,
            "live" => HlsStart::Live,
            _ => return Err(JsValue::from_str("stream start must be beginning or live")),
        };
        let route =
            StreamShareRoute::new(owner, topic).map_err(|error| JsValue::from_str(&error))?;
        let view_generation = crate::stream::begin_result_view_request();
        crate::stream::release_current_stream_view();
        self.boot_runtime()
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        crate::stream_hls::attach_hls_feed_player(
            self.inner.clone(),
            &media,
            route.owner,
            route.topic,
            start,
            view_generation,
        )
        .await
        .map(|_| ())
        .map_err(|error| JsValue::from_str(&error))
    }

    #[wasm_bindgen(js_name = networkState)]
    pub async fn network_state(&self) -> Object {
        if let Err(error) = self.boot_runtime().await {
            return error_object(error);
        }
        let network_id = self.inner.network_id();
        let profile = profile_for_swarm_network_id(network_id).unwrap_or_else(active_profile);
        network_profile_object(profile, network_id)
    }

    #[wasm_bindgen(js_name = openSecureVault)]
    pub fn open_secure_vault(&self) -> Object {
        if self.startup.pending.get() != 0 {
            return error_object(
                "weeb-3 is still starting; wait for networkState before opening the vault",
            );
        }
        if let Some(error) = self.startup.error.borrow().as_ref() {
            return error_object(error);
        }
        if let Some(profile) = profile_for_swarm_network_id(self.inner.network_id()) {
            activate_profile(profile);
        }
        self.inner.claim_vault_broker();
        secure_open_vault_from_user_action();
        ok_object()
    }

    #[wasm_bindgen(js_name = switchNetwork)]
    pub async fn switch_network(&self, mode: String) -> Object {
        let Some(mode) = network_mode_from_input(&mode) else {
            return error_object("unknown network mode");
        };
        spawn_local(async {
            let _ = get_service_worker().await;
        });
        self.startup.begin();
        let _guard = self.startup.serial.lock().await;
        self.startup.configured.set(true);
        let profile = profile_for_mode(mode);
        let network_id = profile.swarm_network_id.to_string();
        if self.inner.network_id() != profile.swarm_network_id {
            crate::stream::release_current_stream_view();
        }
        let requested_bootnodes = Array::new();
        let skipped_bootnodes = Array::new();
        let mut bootstrap_nodes = Vec::new();
        for node in profile_bootstrap_nodes(profile) {
            if is_browser_dialable_underlay(&node.multiaddr) {
                requested_bootnodes.push(&JsValue::from_str(&node.multiaddr));
                bootstrap_nodes.push(node);
            } else {
                skipped_bootnodes.push(&JsValue::from_str(&node.multiaddr));
            }
        }
        if let Err(error) =
            configure_shared_node(&self.inner, network_id.clone(), bootstrap_nodes).await
        {
            self.startup.finish(Some(error.clone()));
            return error_object(format!("network id switch failed: {error}"));
        }

        self.startup.finish(None);

        let obj = network_profile_object(profile, profile.swarm_network_id);
        set_js(&obj, "requestedBootnodes", requested_bootnodes.into());
        set_js(&obj, "skippedBootnodes", skipped_bootnodes.into());
        obj
    }

    #[wasm_bindgen(js_name = retrieve)]
    pub async fn retrieve(&self, address: String) -> Result<Array, JsValue> {
        self.boot_runtime()
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        let raw = self.inner.acquire(address).await;
        let (mut data, indx) = decode_resources(raw);

        let out = Array::new();

        fn make_entry(path: &str, file: &JsValue) -> JsValue {
            let obj = Object::new();
            Reflect::set(&obj, &JsValue::from("path"), &JsValue::from(path)).expect("set path");
            Reflect::set(&obj, &JsValue::from("file"), file).expect("set file");
            obj.into()
        }

        if let Some(pos) = data.iter().position(|(_, _, p)| *p == indx) {
            let (bytes, mime, path) = data.remove(pos);
            let file = make_js_file(bytes_to_js(&bytes), &mime, &path);
            let entry = make_entry(&path, &file);
            out.push(&entry);
        }

        for (bytes, mime, path) in data {
            let file = make_js_file(bytes_to_js(&bytes), &mime, &path);
            let entry = make_entry(&path, &file);
            out.push(&entry);
        }

        Ok(out)
    }

    #[wasm_bindgen(js_name = retrieveBytes)]
    pub async fn retrieve_bytes(&self, address: String) -> Result<Uint8Array, JsValue> {
        self.boot_runtime()
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        Ok(self.inner.retrieve_bytes(address).await)
    }

    #[wasm_bindgen(js_name = retrieveChunk)]
    pub async fn retrieve_chunk(&self, address: String) -> Result<Uint8Array, JsValue> {
        self.boot_runtime()
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        Ok(self.inner.retrieve_chunk_bytes(address).await)
    }

    #[wasm_bindgen(js_name = ready)]
    pub async fn ready(&self, min_connections: u32, timeout_ms: u32) -> Result<bool, JsValue> {
        self.boot_runtime()
            .await
            .map_err(|error| JsValue::from_str(&error))?;

        let min_connections = min_connections.max(1) as u64;
        let started = js_sys::Date::now();
        loop {
            if self.inner.get_connections().await >= min_connections {
                return Ok(true);
            }

            if timeout_ms == 0 || js_sys::Date::now() - started >= timeout_ms as f64 {
                return Ok(false);
            }

            async_std::task::sleep(Duration::from_millis(160)).await;
        }
    }

    pub async fn logs(&self) -> Array {
        let out = Array::new();
        for log in self.inner.get_current_logs().await {
            out.push(&JsValue::from_str(&log));
        }
        out
    }

    #[wasm_bindgen(js_name = progressSnapshot)]
    pub async fn progress_snapshot(&self, seen_revision: u32) -> Object {
        let obj = ok_object();
        let rows = Array::new();

        match self.inner.get_progress_snapshot(seen_revision as u64).await {
            Some((revision, snapshot)) => {
                set_js_bool(&obj, "changed", true);
                set_js_number(&obj, "revision", revision as f64);
                for row in snapshot {
                    rows.push(&progress_row_to_js(row));
                }
            }
            None => {
                set_js_bool(&obj, "changed", false);
                set_js_number(&obj, "revision", seen_revision as f64);
            }
        }

        set_js(&obj, "rows", rows.into());
        obj
    }

    #[wasm_bindgen(js_name = postPushChunk)]
    pub async fn post_push_chunk_js(
        &self,
        data: Uint8Array,
        soc: bool,
        chunk_address: Uint8Array,
        stamp: Uint8Array,
    ) -> Result<String, JsValue> {
        self.boot_runtime()
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        let raw = self
            .inner
            .post_push_chunk(data, soc, chunk_address, stamp)
            .await;

        let (resources, _) = decode_resources(raw);

        Ok(if let Some((bytes, _, _)) = resources.into_iter().next() {
            String::from_utf8(bytes).unwrap_or_else(|_| "Invalid UTF-8 result".to_string())
        } else {
            "No upload result returned".to_string()
        })
    }

    pub async fn upload(
        &self,
        file: File,
        encryption: bool,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        self.upload_with_redundancy(
            file,
            encryption,
            f64::from(RedundancyLevel::DEFAULT_UPLOAD.as_u8()),
            index_string,
            add_to_feed,
            feed_topic,
        )
        .await
    }

    #[wasm_bindgen(js_name = uploadWithRedundancy)]
    pub async fn upload_with_redundancy(
        &self,
        file: File,
        encryption: bool,
        #[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: f64,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        let Some(redundancy_level) = validated_upload_redundancy_number(redundancy_level) else {
            return error_object("redundancy level must be an integer between 0 and 4");
        };
        let redundancy_level = f64::from(redundancy_level.as_u8());

        if let Err(error) = self.boot_runtime().await {
            return error_object(error);
        }
        let feed_topic = if add_to_feed {
            normalize_feed_topic(&feed_topic)
        } else {
            feed_topic
        };
        let raw = self
            .inner
            .post_upload_with_redundancy(
                file,
                encryption,
                redundancy_level,
                index_string,
                add_to_feed,
                feed_topic.clone(),
            )
            .await;

        let (data, indx) = decode_resources(raw);
        let obj = if indx.is_empty() {
            error_object("upload failed")
        } else {
            ok_object()
        };

        set_js_str(&obj, "reference", &indx);
        set_js_number(&obj, "redundancyLevel", redundancy_level);
        if add_to_feed {
            set_js_str(&obj, "feedTopic", &feed_topic);
            set_js_str(&obj, "feedReference", &indx);
            match secure_ensure_feed_owner_in_window().await {
                Some(owner) if owner.len() == 20 => {
                    set_js_str(&obj, "feedOwner", format!("0x{}", hex::encode(owner)));
                }
                Some(owner) => {
                    set_js_str(
                        &obj,
                        "feedOwnerError",
                        format!("feed owner had invalid length {}", owner.len()),
                    );
                }
                None => set_js_str(&obj, "feedOwnerError", "feed owner unavailable"),
            }
        }

        let resources = Array::new();
        for (bytes, mime, path) in data {
            resources.push(&resource_to_js(bytes, mime, path));
        }
        set_js(&obj, "resources", resources.into());
        obj
    }

    #[wasm_bindgen(js_name = postUploadBytes)]
    pub async fn post_upload_bytes(
        &self,
        bytes: Uint8Array,
        mime: String,
        filename: String,
        encryption: bool,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        let file = make_upload_file(bytes, mime, filename);
        self.upload(file, encryption, String::new(), add_to_feed, feed_topic)
            .await
    }

    #[wasm_bindgen(js_name = postUploadBytesWithRedundancy)]
    pub async fn post_upload_bytes_with_redundancy(
        &self,
        bytes: Uint8Array,
        mime: String,
        filename: String,
        encryption: bool,
        #[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: f64,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        let file = make_upload_file(bytes, mime, filename);
        self.upload_with_redundancy(
            file,
            encryption,
            redundancy_level,
            String::new(),
            add_to_feed,
            feed_topic,
        )
        .await
    }

    #[wasm_bindgen(js_name = postFeedBytes)]
    pub async fn post_feed_bytes(
        &self,
        topic: String,
        bytes: Uint8Array,
        mime: String,
        filename: String,
        encryption: bool,
    ) -> Object {
        self.post_upload_bytes(bytes, mime, filename, encryption, true, topic)
            .await
    }

    #[wasm_bindgen(js_name = acquireFeedBytes)]
    pub async fn acquire_feed_bytes(&self, owner: String, topic: String) -> Object {
        if let Err(error) = self.boot_runtime().await {
            return error_object(error);
        }
        let feed_topic = normalize_feed_topic(&topic);
        let feed_owner = match feed_owner_for_request(&owner).await {
            Ok(feed_owner) => feed_owner,
            Err(reason) => {
                let obj = Object::new();
                set_js_str(&obj, "status", "error");
                set_js_str(&obj, "reason", reason);
                set_js_str(&obj, "owner", &owner);
                set_js_str(&obj, "topic", &topic);
                set_js_str(&obj, "feedTopic", &feed_topic);
                set_js(&obj, "body", bytes_to_js(&[]).into());
                return obj;
            }
        };
        let owner_for_read = feed_owner.clone().unwrap_or_else(|| owner.clone());
        let raw = self
            .inner
            .acquire_feed_envelope(owner_for_read, topic.clone())
            .await;
        let (data, indx) = decode_resources(raw);
        let obj = Object::new();

        set_js_str(&obj, "owner", &owner);
        set_js_str(&obj, "topic", &topic);
        set_js_str(&obj, "feedTopic", &feed_topic);
        if let Some(owner) = feed_owner {
            set_js_str(&obj, "feedOwner", owner);
        }
        set_js_str(&obj, "index", &indx);

        if let Some((status, reason)) = feed_status(&data, self.inner.get_connections().await) {
            set_js_str(&obj, "status", status);
            set_js_str(&obj, "reason", reason);
            set_js(&obj, "body", bytes_to_js(&[]).into());
            return obj;
        }

        if let Some((bytes, mime, path)) = data.into_iter().next() {
            set_js_str(&obj, "status", "ok");
            set_js(&obj, "body", bytes_to_js(&bytes).into());
            set_js_str(&obj, "mime", mime);
            set_js_str(&obj, "path", path);
        } else {
            set_js_str(&obj, "status", "not_found");
            set_js_str(&obj, "reason", "feed update not found");
            set_js(&obj, "body", bytes_to_js(&[]).into());
        }

        obj
    }

    #[wasm_bindgen(js_name = batchState)]
    pub async fn batch_state(&self, depth: u8, validity_days: u32) -> Object {
        if let Err(error) = self.boot_runtime().await {
            return error_object(error);
        }
        let profile = active_profile();
        let payer = match request_wallet_address().await {
            Ok(payer) => payer,
            Err(error) => return error_object(error),
        };
        let secure_state =
            match secure_batch_state_for_wallet(payer.as_bytes(), profile.swarm_network_id).await {
                Some(state) => state,
                None => return error_object("could not check weeb-3-secure batch state"),
            };
        let w3 = match web3() {
            Ok(w3) => w3,
            Err(error) => return error_object(format!("provider init failed: {error:?}")),
        };
        let chain_id = match w3.eth().chain_id().await {
            Ok(chain_id) => chain_id,
            Err(error) => return error_object(format!("chain id check failed: {error:?}")),
        };

        let obj = ok_object();
        set_js_str(&obj, "network", network_mode_label(profile.mode));
        set_js_str(&obj, "mode", network_mode_label(profile.mode));
        set_js_number(&obj, "swarmNetworkId", profile.swarm_network_id as f64);
        set_js_number(&obj, "walletChainId", profile.wallet_chain_id as f64);
        set_js_str(&obj, "wallet", hex_address(payer));
        set_js_str(&obj, "chainId", chain_id.to_string());
        set_js_bool(&obj, "hasBatch", secure_state.has_batch);
        set_js_bool(&obj, "usableBatch", secure_state.usable());
        set_js_str(&obj, "batchId", hex::encode(&secure_state.batch_id));
        set_js_number(
            &obj,
            "batchBucketLimit",
            secure_state.batch_bucket_limit as f64,
        );
        set_js_str(
            &obj,
            "batchValidityStatus",
            &secure_state.batch_validity_status,
        );
        set_js_number(&obj, "depth", depth as f64);
        set_js_number(&obj, "validityDays", validity_days as f64);

        if chain_id != U256::from(profile.wallet_chain_id) {
            set_js_str(&obj, "status", "wrong_network");
            return obj;
        }

        let postage = match postage_contract(&w3) {
            Ok(postage) => postage,
            Err(error) => return error_object(format!("postage contract failed: {error:?}")),
        };
        let token = match token_contract(&w3) {
            Ok(token) => token,
            Err(error) => return error_object(format!("token contract failed: {error:?}")),
        };
        let last_price = match last_price(&postage).await {
            Ok(last_price) => last_price,
            Err(error) => return error_object(format!("last price failed: {error:?}")),
        };
        let token_balance: U256 = match token
            .query(
                "balanceOf",
                (payer,),
                None,
                web3::contract::Options::default(),
                None,
            )
            .await
        {
            Ok(balance) => balance,
            Err(error) => return error_object(format!("token balance failed: {error:?}")),
        };
        let base_balance = match w3.eth().balance(payer, None).await {
            Ok(balance) => balance,
            Err(error) => return error_object(format!("base balance failed: {error:?}")),
        };
        let required = compute_initial_balance_per_chunk(last_price, validity_days as u64)
            * chunk_count_for_depth(depth);

        if secure_state.usable() {
            let remaining = get_batch_validity(&secure_state.batch_id).await;
            let day_price = last_price * U256::from(7200u64);
            let days = if day_price.is_zero() {
                U256::from(0)
            } else {
                remaining / day_price
            };
            set_js_str(&obj, "batchRemainingDays", days.to_string());
        }

        set_js_str(&obj, "bzzSymbol", profile.bzz_symbol);
        set_js_str(&obj, "baseSymbol", profile.base_symbol);
        set_js(&obj, "lastPrice", u256_string(last_price));
        set_js(&obj, "requiredBzz", u256_string(required));
        set_js(&obj, "tokenBalance", u256_string(token_balance));
        set_js(&obj, "baseBalance", u256_string(base_balance));
        set_js_bool(&obj, "hasFunds", token_balance >= required);
        obj
    }

    #[wasm_bindgen(js_name = buyBatch)]
    pub async fn buy_batch(&self, depth: u8, validity_days: u32) -> Object {
        if let Err(error) = self.boot_runtime().await {
            return error_object(error);
        }
        let profile = active_profile();
        let payer = match request_wallet_address().await {
            Ok(payer) => payer,
            Err(error) => return error_object(error),
        };
        if let Some(state) =
            secure_batch_state_for_wallet(payer.as_bytes(), profile.swarm_network_id).await
            && state.usable()
        {
            let obj = ok_object();
            set_js_str(&obj, "status", "already_ready");
            set_js_str(&obj, "batchId", hex::encode(&state.batch_id));
            set_js_number(&obj, "batchBucketLimit", state.batch_bucket_limit as f64);
            return obj;
        }

        let prepared = match secure_prepare_batch_purchase(
            depth,
            validity_days as u64,
            profile.swarm_network_id,
        )
        .await
        {
            Some(prepared) if prepared.owner.len() == 20 => prepared,
            _ => return error_object("failed to prepare secure batch owner"),
        };
        let owner = Address::from_slice(&prepared.owner);
        let purchase = match buy_postage_batch_with_payer(
            prepared.validity_days,
            prepared.depth,
            owner,
            payer,
        )
        .await
        {
            Ok(purchase) => purchase,
            Err(error) => return error_object(format!("batch purchase failed: {error:?}")),
        };

        if !secure_commit_batch_purchase_and_verify(
            payer.as_bytes(),
            &purchase.batch_id,
            purchase.bucket_limit,
            prepared.depth,
            profile.swarm_network_id,
        )
        .await
        {
            return error_object("failed to save or verify batch in weeb-3-secure");
        }

        let obj = ok_object();
        set_js_str(&obj, "wallet", hex_address(payer));
        set_js_str(&obj, "owner", hex_address(owner));
        set_js_str(&obj, "batchId", hex::encode(&purchase.batch_id));
        set_js_number(&obj, "depth", prepared.depth as f64);
        set_js_number(&obj, "validityDays", prepared.validity_days as f64);
        set_js_number(&obj, "batchBucketLimit", purchase.bucket_limit as f64);
        set_js_str(
            &obj,
            "approveTx",
            hex::encode(purchase.approve_tx.as_bytes()),
        );
        set_js_str(&obj, "createTx", hex::encode(purchase.create_tx.as_bytes()));
        set_js(&obj, "lastPrice", u256_string(purchase.last_price));
        obj
    }

    #[wasm_bindgen(js_name = deployChequebook)]
    pub async fn deploy_chequebook(&self) -> Object {
        if let Err(error) = self.boot_runtime().await {
            return error_object(error);
        }
        let stored_key = get_chequebook_signer_key().await;
        let stored_address = get_chequebook_address().await;
        if !stored_key.is_empty() && stored_address.len() == 20 {
            let obj = ok_object();
            set_js_str(&obj, "status", "already_ready");
            set_js_str(&obj, "chequebook", hex::encode(stored_address));
            return obj;
        }

        let payer = match request_wallet_address().await {
            Ok(payer) => payer,
            Err(error) => return error_object(error),
        };
        let cheque_signer_key = encrey();
        let cheque_signer = match PrivateKeySigner::from_slice(&cheque_signer_key) {
            Ok(signer) => signer,
            Err(_) => return error_object("failed to create chequebook signer key"),
        };
        let issuer_bytes: [u8; 20] = *cheque_signer.address().as_ref();
        let issuer = Address::from(issuer_bytes);
        let deployment = match deploy_chequebook_with_payer(issuer, payer).await {
            Ok(deployment) => deployment,
            Err(error) => return error_object(format!("chequebook deployment failed: {error:?}")),
        };

        if !set_chequebook_signer_key(&cheque_signer_key).await {
            return error_object("chequebook deployed, but signer key could not be saved");
        }
        if !set_chequebook_address(deployment.chequebook.as_bytes()).await {
            return error_object("chequebook deployed, but address could not be saved");
        }

        let obj = ok_object();
        set_js_str(&obj, "payer", hex_address(payer));
        set_js_str(&obj, "issuer", hex_address(issuer));
        set_js_str(
            &obj,
            "chequebook",
            hex::encode(deployment.chequebook.as_bytes()),
        );
        set_js_str(&obj, "tx", hex::encode(deployment.tx.as_bytes()));
        obj
    }

    #[wasm_bindgen(js_name = depositChequebook)]
    pub async fn deposit_chequebook(&self, amount: String) -> Object {
        if let Err(error) = self.boot_runtime().await {
            return error_object(error);
        }
        let amount = match U256::from_dec_str(amount.trim()) {
            Ok(amount) => amount,
            Err(_) => return error_object("amount must be a base-unit integer string"),
        };
        let stored_address = get_chequebook_address().await;
        if stored_address.len() != 20 {
            return error_object("deploy a chequebook before depositing");
        }
        let chequebook = Address::from_slice(&stored_address);
        let payer = match request_wallet_address().await {
            Ok(payer) => payer,
            Err(error) => return error_object(error),
        };
        let w3 = match web3() {
            Ok(w3) => w3,
            Err(error) => return error_object(format!("provider init failed: {error:?}")),
        };
        let token = match token_contract(&w3) {
            Ok(token) => token,
            Err(error) => return error_object(format!("token contract failed: {error:?}")),
        };
        let receipt = match deposit_to_chequebook(&token, chequebook, payer, amount).await {
            Ok(receipt) => receipt,
            Err(error) => return error_object(format!("deposit failed: {error:?}")),
        };

        let obj = ok_object();
        set_js_str(&obj, "payer", hex_address(payer));
        set_js_str(&obj, "chequebook", hex::encode(stored_address));
        set_js_str(&obj, "tx", hex::encode(receipt.transaction_hash.as_bytes()));
        if let Ok(balance) = chequebook_balance(&w3, chequebook).await {
            set_js(&obj, "balance", u256_string(balance));
        }
        obj
    }

    #[wasm_bindgen(js_name = resetStamp)]
    pub async fn reset_stamp(&self) -> Object {
        if let Err(error) = self.boot_runtime().await {
            return error_object(error);
        }
        let raw = self.inner.reset_stamp().await;
        let (data, _) = decode_resources(raw);
        let obj = Object::new();
        if let Some((bytes, mime, path)) = data.first() {
            set_js_str(&obj, "message", String::from_utf8_lossy(bytes));
            set_js_str(&obj, "mime", mime);
            set_js_str(&obj, "path", path);
        }
        obj
    }
}
