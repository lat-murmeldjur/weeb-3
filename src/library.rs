#![cfg(target_arch = "wasm32")]

use crate::{
    Weeb3, decode_resources, encrey,
    erasure_coding::RedundancyLevel,
    interface::{
        begin_interface_mount, get_service_worker, install_service_worker_message_bridge,
        mount_interface_after_service_worker_bridge_install,
    },
    interface_conventions::render_interface_shell,
    nav::route_network_mode_from_location,
    network_profile::{
        NetworkMode, NetworkProfile, activate_profile, active_profile,
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
        secure_ensure_feed_owner, secure_open_vault_from_user_action,
        secure_prepare_batch_purchase,
    },
    stream_conventions::{
        configure_streaming_routes as set_streaming_routes, route_base_controls_path,
    },
    strip_hex_prefix,
    upload_conventions::{
        DEFAULT_UPLOAD_REDUNDANCY_VALUE, UPLOAD_REDUNDANCY_OPTIONS,
        validated_upload_redundancy_number,
    },
};
use alloy::signers::local::PrivateKeySigner;
use async_std::sync::Arc;
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use std::{
    cell::RefCell,
    str::FromStr,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Element, File, FilePropertyBag, HtmlMediaElement};
use web3::types::{Address, U256};

thread_local! {
    static INTERFACE_MOUNT_CONTAINER: RefCell<Option<Element>> = const { RefCell::new(None) };
}

fn reserve_interface_mount_container(container: &Element) -> Result<(), &'static str> {
    INTERFACE_MOUNT_CONTAINER.with(|mounted| {
        let mut mounted = mounted.borrow_mut();
        if let Some(existing) = mounted.as_ref() {
            let same_node = existing.is_same_node(Some(container.unchecked_ref()));
            if existing.is_connected() && !same_node {
                return Err(
                    "weeb-3 supports one connected renderInterface container per page; \
                     rerender the existing container or remove it before mounting another",
                );
            }
        }
        *mounted = Some(container.clone());
        Ok(())
    })
}

#[wasm_bindgen(typescript_custom_section)]
const UPLOAD_REDUNDANCY_TYPES: &'static str = r#"
/** Bee-compatible erasure-coding level used for uploads. */
export type UploadRedundancyLevel = 0 | 1 | 2 | 3 | 4;

/** Display metadata for an upload erasure-coding choice. */
export interface UploadRedundancyOption {
    readonly value: UploadRedundancyLevel;
    readonly label: string;
    readonly description: string;
    readonly isDefault: boolean;
}
"#;

#[wasm_bindgen(typescript_custom_section)]
const HLS_STREAM_TYPES: &'static str = r#"
/** Which presentation an unindexed mutable HLS feed should expose first. */
export type HlsStreamStart = "beginning" | "current-window";

/** Options for attaching the shared weeb-3 HLS pipeline to application-owned media. */
export interface HlsStreamOptions {
    readonly start: HlsStreamStart;
    readonly index?: number | null;
}
"#;

/// Ordered metadata for building a custom upload redundancy selector.
#[wasm_bindgen(
    js_name = uploadRedundancyOptions,
    unchecked_return_type = "UploadRedundancyOption[]"
)]
pub fn upload_redundancy_options() -> Array {
    let options = Array::new();
    for option in UPLOAD_REDUNDANCY_OPTIONS {
        let value = Object::new();
        set_js(
            &value,
            "value",
            JsValue::from_f64(f64::from(option.value())),
        );
        set_js_str(&value, "label", option.label);
        set_js_str(&value, "description", option.description);
        set_js(&value, "isDefault", JsValue::from_bool(option.is_default()));
        options.push(&value);
    }
    options
}

/// Bee's default upload redundancy level (`Medium`).
#[wasm_bindgen(
    js_name = defaultUploadRedundancyLevel,
    unchecked_return_type = "UploadRedundancyLevel"
)]
pub fn default_upload_redundancy_level() -> u8 {
    DEFAULT_UPLOAD_REDUNDANCY_VALUE
}

/// Configure the same-origin Service Worker asset and URL prefix used by HLS,
/// feed, BZZ, and raw-byte browser routes. Call this before `renderInterface`
/// when the npm package is mounted anywhere other than `/weeb-3`.
#[wasm_bindgen(js_name = configureStreamingRoutes)]
pub fn configure_streaming_routes(service_worker_url: String, route_base: String) -> Object {
    let Some(window) = web_sys::window() else {
        return error_object("streaming routes require a browser window");
    };
    let location = window.location();
    let pathname = location.pathname().unwrap_or_default();
    if !route_base_controls_path(&route_base, &pathname) {
        return error_object(
            "streaming route base must contain the current document so its Service Worker can control this client",
        );
    }
    let page_url = match location
        .href()
        .ok()
        .and_then(|href| web_sys::Url::new(&href).ok())
    {
        Some(url) => url,
        None => return error_object("could not resolve the current document URL"),
    };
    let worker_url = match web_sys::Url::new_with_base(&service_worker_url, &page_url.href()) {
        Ok(url)
            if matches!(url.protocol().as_str(), "http:" | "https:")
                && url.origin() == page_url.origin()
                && url.hash().is_empty() =>
        {
            url.href()
        }
        _ => {
            return error_object(
                "service worker URL must be a same-origin HTTP(S) URL without a fragment",
            );
        }
    };
    match set_streaming_routes(&worker_url, &route_base) {
        Ok(()) => ok_object(),
        Err(error) => error_object(error),
    }
}

#[wasm_bindgen(js_name = configure_streaming_routes)]
pub fn configure_streaming_routes_alias(service_worker_url: String, route_base: String) -> Object {
    configure_streaming_routes(service_worker_url, route_base)
}

#[wasm_bindgen]
pub struct BootstrapNode {
    multiaddr: String,
    usable: bool,
}

#[wasm_bindgen]
impl BootstrapNode {
    #[wasm_bindgen(constructor)]
    pub fn new(multiaddr: String, usable: bool) -> BootstrapNode {
        BootstrapNode { multiaddr, usable }
    }

    #[wasm_bindgen(getter)]
    pub fn multiaddr(&self) -> String {
        self.multiaddr.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn usable(&self) -> bool {
        self.usable
    }
}

fn resource_to_js(bytes: Vec<u8>, mime: String, path: String) -> Object {
    let obj = Object::new();
    let u8 = Uint8Array::new_with_length(bytes.len() as u32);
    u8.copy_from(&bytes);
    let _ = Reflect::set(&obj, &"body".into(), &u8);
    let _ = Reflect::set(&obj, &"mime".into(), &JsValue::from_str(&mime));
    let _ = Reflect::set(&obj, &"path".into(), &JsValue::from_str(&path));
    obj
}

fn progress_row_to_js(row: crate::events::ProgressRow) -> Object {
    let obj = Object::new();
    set_js_str(&obj, "id", row.id);
    set_js_str(&obj, "kind", row.kind);
    set_js_str(&obj, "subject", row.subject);
    set_js_str(&obj, "phase", row.phase);
    match row.percent {
        Some(percent) => set_js(&obj, "percent", JsValue::from_f64(percent as f64)),
        None => set_js(&obj, "percent", JsValue::NULL),
    }
    set_js_str(&obj, "detail", row.detail);
    set_js(&obj, "done", JsValue::from_bool(row.done));
    set_js(&obj, "ok", JsValue::from_bool(row.ok));
    obj
}

fn bytes_to_js(bytes: &[u8]) -> Uint8Array {
    let out = Uint8Array::new_with_length(bytes.len() as u32);
    out.copy_from(bytes);
    out
}

fn make_js_file(bytes: Vec<u8>, mime: &str, name: &str) -> File {
    let parts = Array::new();
    let u8 = Uint8Array::new_with_length(bytes.len() as u32);
    u8.copy_from(&bytes);
    parts.push(&u8);

    let bag = FilePropertyBag::new();
    bag.set_type(mime);
    bag.set_last_modified(js_sys::Date::now());

    File::new_with_u8_array_sequence_and_options(&parts, name, &bag).unwrap()
}

fn set_js(target: &Object, name: &str, value: JsValue) {
    let _ = Reflect::set(target, &JsValue::from_str(name), &value);
}

fn set_js_str(target: &Object, name: &str, value: impl AsRef<str>) {
    set_js(target, name, JsValue::from_str(value.as_ref()));
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

fn interface_result_view_is_mounted() -> bool {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("resultField"))
        .is_some()
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
        return match secure_ensure_feed_owner().await {
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

fn active_wallet_chain_id() -> u64 {
    active_profile().wallet_chain_id
}

fn network_mode_from_input(mode: &str) -> Option<NetworkMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "mainnet" | "gnosis" | "gnosischain" | "1" => Some(NetworkMode::Mainnet),
        "testnet" | "sepolia" | "10" => Some(NetworkMode::Testnet),
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
    set_js(
        &obj,
        "networkId",
        JsValue::from_f64(current_network_id as f64),
    );
    set_js(
        &obj,
        "swarmNetworkId",
        JsValue::from_f64(profile.swarm_network_id as f64),
    );
    set_js(
        &obj,
        "walletChainId",
        JsValue::from_f64(profile.wallet_chain_id as f64),
    );
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

#[derive(Clone)]
struct StartBootstrapNode {
    multiaddr: String,
    usable: bool,
}

struct StartOptions {
    network_id: String,
    bootstrap_nodes: Vec<StartBootstrapNode>,
}

fn js_prop(value: &JsValue, name: &str) -> JsValue {
    Reflect::get(value, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

fn js_bool_prop(value: &JsValue, names: &[&str]) -> Option<bool> {
    names.iter().find_map(|name| {
        let prop = js_prop(value, name);
        if prop.is_null() || prop.is_undefined() {
            None
        } else {
            prop.as_bool()
        }
    })
}

fn js_string_prop(value: &JsValue, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        let prop = js_prop(value, name);
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
                Some(format!("{}", number as u64))
            } else {
                None
            }
        })
    })
}

fn parse_start_bootstrap_node(value: JsValue) -> Option<StartBootstrapNode> {
    let multiaddr = js_string_prop(&value, &["multiaddr", "address", "underlay"])?;
    let usable = js_bool_prop(&value, &["usable", "usableInProtocols"]).unwrap_or(true);
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
        js_string_prop(&options, &["networkId", "network_id", "swarmNetworkId"])
    };

    let mode = if js_bool_prop(&options, &["testnet"]).unwrap_or(false) {
        NetworkMode::Testnet
    } else if js_bool_prop(&options, &["mainnet"]).unwrap_or(false) {
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
        let nodes = js_prop(&options, "bootstrapNodes");
        if nodes.is_null() || nodes.is_undefined() {
            parse_start_bootstrap_nodes(js_prop(&options, "bootstrap_nodes"))
        } else {
            parse_start_bootstrap_nodes(nodes)
        }
    };

    let bootstrap_nodes = if configured_nodes.is_empty() {
        profile
            .bootnodes
            .iter()
            .map(|address| StartBootstrapNode {
                multiaddr: (*address).to_string(),
                usable: true,
            })
            .collect()
    } else {
        configured_nodes
    };

    StartOptions {
        network_id,
        bootstrap_nodes,
    }
}

struct HlsStreamOptions {
    index: Option<u64>,
    intent: crate::stream_hls::HlsOpenIntent,
}

fn hls_stream_options_from_js(options: &JsValue) -> Result<HlsStreamOptions, &'static str> {
    let start = js_string_prop(options, &["start"])
        .ok_or("HLS stream options require start: \"beginning\" or \"current-window\"")?;
    let intent = match start.trim().to_ascii_lowercase().as_str() {
        "beginning" => crate::stream_hls::HlsOpenIntent::Beginning,
        "current-window" => crate::stream_hls::HlsOpenIntent::CurrentWindow,
        _ => return Err("HLS stream start must be \"beginning\" or \"current-window\""),
    };

    let raw_index = js_prop(options, "index");
    let index = if raw_index.is_null() || raw_index.is_undefined() {
        None
    } else {
        match raw_index.as_f64() {
            Some(index)
                if index.is_finite()
                    && index >= 0.0
                    && index <= 9_007_199_254_740_991.0
                    && (index as u64) as f64 == index =>
            {
                Some(index as u64)
            }
            _ => return Err("HLS feed index must be an exact unsigned integer"),
        }
    };

    Ok(HlsStreamOptions { index, intent })
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

async fn request_wallet_via_shell_connector(chain_id: u64) -> Option<Result<Address, String>> {
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
    let chain_id = active_wallet_chain_id();
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

#[wasm_bindgen]
pub struct Weeb3No103 {
    inner: Arc<Weeb3>,
    startup_configured: Arc<AtomicBool>,
    startup_pending: Arc<AtomicUsize>,
    startup_serial: Arc<async_lock::Mutex<()>>,
}

fn start_weeb3_runtime(inner: Arc<Weeb3>) {
    spawn_local(async move {
        inner.run(String::new()).await;
    });
}

fn schedule_bootnode_dials(
    inner: Arc<Weeb3>,
    network_id: String,
    bootstrap_nodes: Vec<StartBootstrapNode>,
) {
    let Ok(expected_network_id) = network_id.parse::<u64>() else {
        return;
    };
    let mut seen = std::collections::HashSet::new();

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
        let dialer = inner.clone();
        spawn_local(async move {
            let _ = dialer
                .connect_bootnode_for_current_network(
                    node.multiaddr,
                    expected_network_id,
                    node.usable,
                )
                .await;
        });
    }
}

#[wasm_bindgen]
impl Weeb3No103 {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Weeb3No103 {
        Weeb3No103 {
            inner: Arc::new(Weeb3::new("".to_string())),
            startup_configured: Arc::new(AtomicBool::new(false)),
            startup_pending: Arc::new(AtomicUsize::new(0)),
            startup_serial: Arc::new(async_lock::Mutex::new(())),
        }
    }

    fn schedule_start(&self, options: StartOptions) {
        if let Ok(network_id) = options.network_id.parse::<u64>() {
            if let Some(profile) = profile_for_swarm_network_id(network_id) {
                activate_profile(profile);
            }
        }
        install_service_worker_message_bridge(self.inner.clone());
        spawn_local(async {
            let _ = get_service_worker().await;
        });
        self.startup_configured.store(true, Ordering::Release);
        self.startup_pending.fetch_add(1, Ordering::AcqRel);
        let inner = self.inner.clone();
        let pending = self.startup_pending.clone();
        let serial = self.startup_serial.clone();

        spawn_local(async move {
            let _guard = serial.lock().await;
            let network_id = options.network_id;
            if inner.set_network_id(network_id.clone()).await {
                start_weeb3_runtime(inner.clone());
                schedule_bootnode_dials(inner, network_id, options.bootstrap_nodes);
            }
            pending.fetch_sub(1, Ordering::AcqRel);
        });
    }

    async fn boot_runtime(&self) {
        install_service_worker_message_bridge(self.inner.clone());
        if !self.startup_configured.swap(true, Ordering::AcqRel) {
            self.schedule_start(start_options_from_js(None));
        }
        self.await_startup_tasks().await;
        start_weeb3_runtime(self.inner.clone());
        async_std::task::yield_now().await;
    }

    async fn await_startup_tasks(&self) {
        while self.startup_pending.load(Ordering::Acquire) != 0 {
            async_std::task::sleep(Duration::from_millis(10)).await;
        }
    }

    #[wasm_bindgen(js_name = start)]
    pub fn start(&self, options: Option<JsValue>) {
        self.schedule_start(start_options_from_js(options));
    }

    #[wasm_bindgen(js_name = renderInterface)]
    pub fn render_interface(&self, container: Element) -> Object {
        if let Err(error) = reserve_interface_mount_container(&container) {
            return error_object(error);
        }
        let mount_generation = begin_interface_mount();
        // Reserve the new result view before replacing its shell. Older
        // asynchronous opens may finish accounting, but cannot commit into
        // this mount after their retrieval settles.
        let initial_result_generation = crate::stream::begin_result_view_request();
        // Re-rendering an npm mount removes its result DOM directly, bypassing
        // normal result-view replacement. Stop future HLS scheduling first;
        // already-dispatched Rust/accounting work continues to drain.
        crate::stream::release_current_stream_view();
        if let Some(mode) = route_network_mode_from_location() {
            activate_profile(profile_for_mode(mode));
        }
        let startup_already_configured = self.startup_configured.swap(true, Ordering::AcqRel);
        self.startup_pending.fetch_add(1, Ordering::AcqRel);
        install_service_worker_message_bridge(self.inner.clone());
        render_interface_shell(&container);

        let s = self.inner.clone();
        let pending = self.startup_pending.clone();
        let serial = self.startup_serial.clone();
        spawn_local(async move {
            let guard = serial.lock().await;
            // A canonical /stream/... or /testnet/stream/... URL is
            // self-contained. Apply its network before the mounted interface
            // connects and resolves the initial stream, including for
            // configurable npm route bases.
            let network_before_route = s.get_network_id().await;
            let mut route_changed_network = false;
            if let Some(mode) = route_network_mode_from_location() {
                let profile = profile_for_mode(mode);
                route_changed_network = network_before_route != profile.swarm_network_id;
                let _ = s.set_network_id(profile.swarm_network_id.to_string()).await;
            }
            let network_id = s.get_network_id().await;
            let bootstrap_nodes = profile_for_swarm_network_id(network_id)
                .map(|profile| {
                    profile
                        .bootnodes
                        .iter()
                        .map(|address| StartBootstrapNode {
                            multiaddr: (*address).to_string(),
                            usable: true,
                        })
                        .collect()
                })
                .unwrap_or_default();
            start_weeb3_runtime(s.clone());
            // Queue the same profile connection buildup as the bundled mount
            // before releasing npm operations. Do not wait for every dial:
            // retrieval should be able to use the first established peer.
            if !startup_already_configured || route_changed_network {
                schedule_bootnode_dials(s.clone(), network_id.to_string(), bootstrap_nodes);
            }
            pending.fetch_sub(1, Ordering::AcqRel);
            drop(guard);
            let _ = mount_interface_after_service_worker_bridge_install(
                s,
                false,
                true,
                Some(initial_result_generation),
                false,
                mount_generation,
            )
            .await;
        });

        ok_object()
    }

    #[wasm_bindgen(js_name = render_interface)]
    pub fn render_interface_alias(&self, container: Element) -> Object {
        self.render_interface(container)
    }

    /// Open either an HLS manifest feed or a stream-catalog feed in the
    /// interface mounted by `renderInterface`.
    #[wasm_bindgen(js_name = openStreamFeed)]
    pub async fn open_stream_feed(&self, owner: String, topic: String) -> Object {
        if !interface_result_view_is_mounted() {
            return error_object("call renderInterface(container) before opening a stream feed");
        }
        self.boot_runtime().await;
        crate::stream_hls::open_feed_view(self.inner.clone(), owner, topic).await;
        ok_object()
    }

    #[wasm_bindgen(js_name = open_stream_feed)]
    pub async fn open_stream_feed_alias(&self, owner: String, topic: String) -> Object {
        self.open_stream_feed(owner, topic).await
    }

    /// Attach the same Rust-owned HLS pipeline used by the bundled interface to
    /// an application-owned `<video>` or `<audio>` element. One HLS session is
    /// active per Wasm module; attaching another element retires only future
    /// scheduling for the previous session while dispatched accounting work
    /// continues to drain.
    #[wasm_bindgen(js_name = attachHlsStream)]
    pub async fn attach_hls_stream(
        &self,
        media: HtmlMediaElement,
        owner: String,
        topic: String,
        #[wasm_bindgen(unchecked_param_type = "HlsStreamOptions")] options: JsValue,
    ) -> Object {
        let options = match hls_stream_options_from_js(&options) {
            Ok(options) => options,
            Err(error) => return error_object(error),
        };
        install_service_worker_message_bridge(self.inner.clone());
        self.boot_runtime().await;
        let player: Element = media.unchecked_into();
        match crate::stream_hls::attach_hls_feed_media(
            self.inner.clone(),
            player,
            owner,
            topic,
            options.index,
            options.intent,
        )
        .await
        {
            Ok(mode) => {
                let result = ok_object();
                set_js_str(&result, "mode", mode);
                result
            }
            Err(error) => error_object(error),
        }
    }

    #[wasm_bindgen(js_name = attach_hls_stream)]
    pub async fn attach_hls_stream_alias(
        &self,
        media: HtmlMediaElement,
        owner: String,
        topic: String,
        #[wasm_bindgen(unchecked_param_type = "HlsStreamOptions")] options: JsValue,
    ) -> Object {
        self.attach_hls_stream(media, owner, topic, options).await
    }

    /// Stop future work for the active HLS attachment before the application
    /// removes or replaces its media element. Requests already dispatched to
    /// Rust continue through their accounting lifecycle.
    #[wasm_bindgen(js_name = detachHlsStream)]
    pub fn detach_hls_stream(&self) -> Object {
        crate::stream::begin_result_view_request();
        crate::stream::release_current_stream_view();
        ok_object()
    }

    #[wasm_bindgen(js_name = detach_hls_stream)]
    pub fn detach_hls_stream_alias(&self) -> Object {
        self.detach_hls_stream()
    }

    /// Open a known HLS feed directly. Passing the catalog's VOD index avoids
    /// a latest-feed frontier search and retrieves the final manifest SOC in
    /// one lookup.
    #[wasm_bindgen(js_name = playHlsStream)]
    pub async fn play_hls_stream(
        &self,
        owner: String,
        topic: String,
        media_type: String,
        index: Option<f64>,
    ) -> Object {
        if !interface_result_view_is_mounted() {
            return error_object("call renderInterface(container) before playing an HLS stream");
        }
        let index = match index {
            Some(index)
                if index.is_finite()
                    && index >= 0.0
                    && index <= 9_007_199_254_740_991.0
                    && (index as u64) as f64 == index =>
            {
                Some(index as u64)
            }
            Some(_) => return error_object("HLS feed index must be an exact unsigned integer"),
            None => None,
        };
        self.boot_runtime().await;
        crate::stream_hls::open_hls_feed_view(self.inner.clone(), owner, topic, media_type, index)
            .await;
        ok_object()
    }

    #[wasm_bindgen(js_name = play_hls_stream)]
    pub async fn play_hls_stream_alias(
        &self,
        owner: String,
        topic: String,
        media_type: String,
        index: Option<f64>,
    ) -> Object {
        self.play_hls_stream(owner, topic, media_type, index).await
    }

    #[wasm_bindgen(js_name = networkState)]
    pub async fn network_state(&self) -> Object {
        self.await_startup_tasks().await;
        let network_id = self.inner.get_network_id().await;
        let profile = profile_for_swarm_network_id(network_id).unwrap_or_else(active_profile);
        network_profile_object(profile, network_id)
    }

    #[wasm_bindgen(js_name = network_state)]
    pub async fn network_state_alias(&self) -> Object {
        self.network_state().await
    }

    #[wasm_bindgen(js_name = openSecureVault)]
    pub fn open_secure_vault(&self) -> Object {
        secure_open_vault_from_user_action();
        ok_object()
    }

    #[wasm_bindgen(js_name = open_secure_vault)]
    pub fn open_secure_vault_alias(&self) -> Object {
        self.open_secure_vault()
    }

    #[wasm_bindgen(js_name = switchNetwork)]
    pub async fn switch_network(&self, mode: String) -> Object {
        let Some(mode) = network_mode_from_input(&mode) else {
            return error_object("unknown network mode");
        };
        install_service_worker_message_bridge(self.inner.clone());
        spawn_local(async {
            let _ = get_service_worker().await;
        });
        self.startup_pending.fetch_add(1, Ordering::AcqRel);
        let _guard = self.startup_serial.lock().await;
        self.startup_configured.store(true, Ordering::Release);
        let profile = profile_for_mode(mode);
        let network_id = profile.swarm_network_id.to_string();
        if !self.inner.set_network_id(network_id.clone()).await {
            self.startup_pending.fetch_sub(1, Ordering::AcqRel);
            return error_object("network id switch failed");
        }
        start_weeb3_runtime(self.inner.clone());

        let requested_bootnodes = Array::new();
        let skipped_bootnodes = Array::new();
        let mut bootstrap_nodes = Vec::with_capacity(profile.bootnodes.len());
        for address in profile.bootnodes {
            if is_browser_dialable_underlay(address) {
                requested_bootnodes.push(&JsValue::from_str(&address));
                bootstrap_nodes.push(StartBootstrapNode {
                    multiaddr: (*address).to_string(),
                    usable: true,
                });
            } else {
                skipped_bootnodes.push(&JsValue::from_str(&address));
            }
        }
        schedule_bootnode_dials(self.inner.clone(), network_id, bootstrap_nodes);
        self.startup_pending.fetch_sub(1, Ordering::AcqRel);

        let obj = network_profile_object(profile, profile.swarm_network_id);
        set_js(&obj, "requestedBootnodes", requested_bootnodes.into());
        set_js(&obj, "skippedBootnodes", skipped_bootnodes.into());
        obj
    }

    #[wasm_bindgen(js_name = switch_network)]
    pub async fn switch_network_alias(&self, mode: String) -> Object {
        self.switch_network(mode).await
    }

    #[wasm_bindgen(js_name = switchMainnet)]
    pub async fn switch_mainnet(&self) -> Object {
        self.switch_network("mainnet".to_string()).await
    }

    #[wasm_bindgen(js_name = switch_mainnet)]
    pub async fn switch_mainnet_alias(&self) -> Object {
        self.switch_mainnet().await
    }

    #[wasm_bindgen(js_name = switchTestnet)]
    pub async fn switch_testnet(&self) -> Object {
        self.switch_network("testnet".to_string()).await
    }

    #[wasm_bindgen(js_name = switch_testnet)]
    pub async fn switch_testnet_alias(&self) -> Object {
        self.switch_testnet().await
    }

    pub async fn connect(&self) -> Object {
        self.await_startup_tasks().await;
        let network_id = self.inner.get_network_id().await;
        let profile = profile_for_swarm_network_id(network_id).unwrap_or_else(active_profile);
        self.switch_network(network_mode_label(profile.mode).to_string())
            .await
    }

    #[wasm_bindgen(js_name = connectProfile)]
    pub async fn connect_profile(&self, mode: String) -> Object {
        self.switch_network(mode).await
    }

    #[wasm_bindgen(js_name = connect_profile)]
    pub async fn connect_profile_alias(&self, mode: String) -> Object {
        self.connect_profile(mode).await
    }

    #[wasm_bindgen(js_name = retrieve)]
    pub async fn retrieve(&self, address: String) -> Array {
        self.boot_runtime().await;
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
            let file = make_js_file(bytes, &mime, &path);
            let entry = make_entry(&path, &file);
            out.push(&entry);
        }

        for (bytes, mime, path) in data {
            let file = make_js_file(bytes, &mime, &path);
            let entry = make_entry(&path, &file);
            out.push(&entry);
        }

        out
    }

    #[wasm_bindgen(js_name = retrieveBytes)]
    pub async fn retrieve_bytes(&self, address: String) -> Uint8Array {
        self.boot_runtime().await;
        let bytes = self.inner.retrieve_bytes(address).await;
        let out = Uint8Array::new_with_length(bytes.len() as u32);
        out.copy_from(&bytes);
        out
    }

    #[wasm_bindgen(js_name = retrieve_bytes)]
    pub async fn retrieve_bytes_alias(&self, address: String) -> Uint8Array {
        self.retrieve_bytes(address).await
    }

    #[wasm_bindgen(js_name = retrieveChunk)]
    pub async fn retrieve_chunk(&self, address: String) -> Uint8Array {
        self.boot_runtime().await;
        let bytes = self.inner.retrieve_chunk_bytes(address).await;
        let out = Uint8Array::new_with_length(bytes.len() as u32);
        out.copy_from(&bytes);
        out
    }

    #[wasm_bindgen(js_name = retrieve_chunk)]
    pub async fn retrieve_chunk_alias(&self, address: String) -> Uint8Array {
        self.retrieve_chunk(address).await
    }

    #[wasm_bindgen(js_name = ready)]
    pub async fn ready(&self, min_connections: u32, timeout_ms: u32) -> bool {
        self.boot_runtime().await;

        let min_connections = min_connections.max(1) as u64;
        let started = js_sys::Date::now();
        loop {
            if self.inner.get_connections().await >= min_connections {
                return true;
            }

            if timeout_ms == 0 || js_sys::Date::now() - started >= timeout_ms as f64 {
                return false;
            }

            async_std::task::sleep(Duration::from_millis(160)).await;
        }
    }

    #[wasm_bindgen(js_name = readyState)]
    pub async fn ready_state(&self, min_connections: u32, timeout_ms: u32) -> Object {
        self.boot_runtime().await;

        let min_connections = min_connections.max(1) as u64;
        let started = js_sys::Date::now();
        let mut connections;
        loop {
            connections = self.inner.get_connections().await;
            if connections >= min_connections {
                break;
            }

            if timeout_ms == 0 || js_sys::Date::now() - started >= timeout_ms as f64 {
                break;
            }

            async_std::task::sleep(Duration::from_millis(160)).await;
        }

        let connecting = self.inner.get_ongoing_connections().await;
        let network_id = self.inner.get_network_id().await;
        let profile = profile_for_swarm_network_id(network_id).unwrap_or_else(active_profile);
        let ready = connections >= min_connections;
        let obj = ok_object();

        set_js(&obj, "ready", JsValue::from_bool(ready));
        set_js(&obj, "connections", JsValue::from_f64(connections as f64));
        set_js(&obj, "connecting", JsValue::from_f64(connecting as f64));
        set_js(
            &obj,
            "minConnections",
            JsValue::from_f64(min_connections as f64),
        );
        set_js(&obj, "networkId", JsValue::from_f64(network_id as f64));
        set_js_str(&obj, "network", network_mode_label(profile.mode));
        set_js_str(&obj, "mode", network_mode_label(profile.mode));
        set_js(
            &obj,
            "walletChainId",
            JsValue::from_f64(profile.wallet_chain_id as f64),
        );
        set_js_str(
            &obj,
            "reason",
            if ready {
                "ready"
            } else if connecting > 0 {
                "connecting"
            } else {
                "insufficient_connections"
            },
        );
        obj
    }

    #[wasm_bindgen(js_name = ready_state)]
    pub async fn ready_state_alias(&self, min_connections: u32, timeout_ms: u32) -> Object {
        self.ready_state(min_connections, timeout_ms).await
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
                set_js(&obj, "changed", JsValue::from_bool(true));
                set_js(&obj, "revision", JsValue::from_f64(revision as f64));
                for row in snapshot {
                    rows.push(&progress_row_to_js(row));
                }
            }
            None => {
                set_js(&obj, "changed", JsValue::from_bool(false));
                set_js(&obj, "revision", JsValue::from_f64(seen_revision as f64));
            }
        }

        set_js(&obj, "rows", rows.into());
        obj
    }

    #[wasm_bindgen(js_name = progress_snapshot)]
    pub async fn progress_snapshot_alias(&self, seen_revision: u32) -> Object {
        self.progress_snapshot(seen_revision).await
    }

    #[wasm_bindgen(js_name = postPushChunk)]
    pub async fn post_push_chunk_js(
        &self,
        data: Vec<u8>,
        soc: bool,
        chunk_address: Vec<u8>,
        stamp: Vec<u8>,
    ) -> String {
        self.boot_runtime().await;
        let raw = self
            .inner
            .post_push_chunk(data, soc, chunk_address, stamp)
            .await;

        let (resources, indx) = decode_resources(raw);

        if let Some((bytes, _mime, _path)) = resources.into_iter().find(|(_, _, p)| *p == indx) {
            String::from_utf8(bytes).unwrap_or_else(|_| "Invalid UTF-8 result".to_string())
        } else {
            "No upload result returned".to_string()
        }
    }

    #[wasm_bindgen(js_name = post_push_chunk)]
    pub async fn post_push_chunk_alias(
        &self,
        data: Vec<u8>,
        soc: bool,
        chunk_address: Vec<u8>,
        stamp: Vec<u8>,
    ) -> String {
        self.post_push_chunk_js(data, soc, chunk_address, stamp)
            .await
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

        self.boot_runtime().await;
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

        let _ = Reflect::set(&obj, &"reference".into(), &JsValue::from_str(&indx));
        set_js(&obj, "redundancyLevel", JsValue::from_f64(redundancy_level));
        if add_to_feed {
            set_js_str(&obj, "feedTopic", &feed_topic);
            set_js_str(&obj, "feedReference", &indx);
            match secure_ensure_feed_owner().await {
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
        let _ = Reflect::set(&obj, &"resources".into(), &resources);
        obj
    }

    #[wasm_bindgen(js_name = post_upload)]
    pub async fn post_upload_alias(
        &self,
        file: File,
        encryption: bool,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        self.upload(file, encryption, index_string, add_to_feed, feed_topic)
            .await
    }

    #[wasm_bindgen(js_name = postUploadWithRedundancy)]
    pub async fn post_upload_with_redundancy(
        &self,
        file: File,
        encryption: bool,
        #[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: f64,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        self.upload_with_redundancy(
            file,
            encryption,
            redundancy_level,
            index_string,
            add_to_feed,
            feed_topic,
        )
        .await
    }

    #[wasm_bindgen(js_name = post_upload_with_redundancy)]
    pub async fn post_upload_with_redundancy_alias(
        &self,
        file: File,
        encryption: bool,
        #[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: f64,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        self.post_upload_with_redundancy(
            file,
            encryption,
            redundancy_level,
            index_string,
            add_to_feed,
            feed_topic,
        )
        .await
    }

    #[wasm_bindgen(js_name = postUploadBytes)]
    pub async fn post_upload_bytes(
        &self,
        bytes: Vec<u8>,
        mime: String,
        filename: String,
        encryption: bool,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        let mime = if mime.is_empty() {
            "application/octet-stream".to_string()
        } else {
            mime
        };
        let filename = if filename.is_empty() {
            "bytes".to_string()
        } else {
            filename
        };
        let file = make_js_file(bytes, &mime, &filename);
        self.upload(file, encryption, String::new(), add_to_feed, feed_topic)
            .await
    }

    #[wasm_bindgen(js_name = postUploadBytesWithRedundancy)]
    pub async fn post_upload_bytes_with_redundancy(
        &self,
        bytes: Vec<u8>,
        mime: String,
        filename: String,
        encryption: bool,
        #[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: f64,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        let mime = if mime.is_empty() {
            "application/octet-stream".to_string()
        } else {
            mime
        };
        let filename = if filename.is_empty() {
            "bytes".to_string()
        } else {
            filename
        };
        let file = make_js_file(bytes, &mime, &filename);
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

    #[wasm_bindgen(js_name = post_upload_bytes_with_redundancy)]
    pub async fn post_upload_bytes_with_redundancy_alias(
        &self,
        bytes: Vec<u8>,
        mime: String,
        filename: String,
        encryption: bool,
        #[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: f64,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        self.post_upload_bytes_with_redundancy(
            bytes,
            mime,
            filename,
            encryption,
            redundancy_level,
            add_to_feed,
            feed_topic,
        )
        .await
    }

    #[wasm_bindgen(js_name = post_upload_bytes)]
    pub async fn post_upload_bytes_alias(
        &self,
        bytes: Vec<u8>,
        mime: String,
        filename: String,
        encryption: bool,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Object {
        self.post_upload_bytes(bytes, mime, filename, encryption, add_to_feed, feed_topic)
            .await
    }

    #[wasm_bindgen(js_name = postFeedBytes)]
    pub async fn post_feed_bytes(
        &self,
        topic: String,
        bytes: Vec<u8>,
        mime: String,
        filename: String,
        encryption: bool,
    ) -> Object {
        self.post_upload_bytes(bytes, mime, filename, encryption, true, topic)
            .await
    }

    #[wasm_bindgen(js_name = post_feed_bytes)]
    pub async fn post_feed_bytes_alias(
        &self,
        topic: String,
        bytes: Vec<u8>,
        mime: String,
        filename: String,
        encryption: bool,
    ) -> Object {
        self.post_feed_bytes(topic, bytes, mime, filename, encryption)
            .await
    }

    #[wasm_bindgen(js_name = feedTopic)]
    pub fn feed_topic(&self, topic: String) -> Object {
        let obj = ok_object();
        set_js_str(&obj, "topic", &topic);
        set_js_str(&obj, "feedTopic", normalize_feed_topic(&topic));
        obj
    }

    #[wasm_bindgen(js_name = feed_topic)]
    pub fn feed_topic_alias(&self, topic: String) -> Object {
        self.feed_topic(topic)
    }

    #[wasm_bindgen(js_name = feedIdentity)]
    pub async fn feed_identity(&self, topic: String) -> Object {
        self.boot_runtime().await;
        let feed_topic = normalize_feed_topic(&topic);
        let obj = match secure_ensure_feed_owner().await {
            Some(owner) if owner.len() == 20 => {
                let obj = ok_object();
                let owner = format!("0x{}", hex::encode(owner));
                set_js_str(&obj, "owner", &owner);
                set_js_str(&obj, "feedOwner", owner);
                obj
            }
            Some(owner) => error_object(format!("feed owner had invalid length {}", owner.len())),
            None => error_object("feed owner unavailable"),
        };

        set_js_str(&obj, "topic", &topic);
        set_js_str(&obj, "feedTopic", feed_topic);
        obj
    }

    #[wasm_bindgen(js_name = feed_identity)]
    pub async fn feed_identity_alias(&self, topic: String) -> Object {
        self.feed_identity(topic).await
    }

    #[wasm_bindgen(js_name = acquireFeed)]
    pub async fn acquire_feed(&self, owner: String, topic: String) -> Object {
        self.boot_runtime().await;
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
                set_js(&obj, "resources", Array::new().into());
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
        let resources = Array::new();

        if let Some((status, reason)) = feed_status(&data, self.inner.get_connections().await) {
            set_js_str(&obj, "status", status);
            set_js_str(&obj, "reason", reason);
        } else {
            set_js_str(&obj, "status", "ok");
            for (bytes, mime, path) in data {
                resources.push(&resource_to_js(bytes, mime, path));
            }
        }

        let _ = Reflect::set(&obj, &"owner".into(), &JsValue::from_str(&owner));
        let _ = Reflect::set(&obj, &"topic".into(), &JsValue::from_str(&topic));
        set_js_str(&obj, "feedTopic", &feed_topic);
        if let Some(owner) = feed_owner {
            set_js_str(&obj, "feedOwner", owner);
        }
        let _ = Reflect::set(&obj, &"index".into(), &JsValue::from_str(&indx));
        let _ = Reflect::set(&obj, &"resources".into(), &resources);
        obj
    }

    #[wasm_bindgen(js_name = acquire_feed)]
    pub async fn acquire_feed_alias(&self, owner: String, topic: String) -> Object {
        self.acquire_feed(owner, topic).await
    }

    #[wasm_bindgen(js_name = acquireFeedBytes)]
    pub async fn acquire_feed_bytes(&self, owner: String, topic: String) -> Object {
        self.boot_runtime().await;
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

    #[wasm_bindgen(js_name = acquire_feed_bytes)]
    pub async fn acquire_feed_bytes_alias(&self, owner: String, topic: String) -> Object {
        self.acquire_feed_bytes(owner, topic).await
    }

    #[wasm_bindgen(js_name = feedOwner)]
    pub async fn feed_owner(&self) -> Object {
        self.boot_runtime().await;
        match secure_ensure_feed_owner().await {
            Some(owner) if owner.len() == 20 => {
                let obj = ok_object();
                set_js_str(&obj, "owner", format!("0x{}", hex::encode(owner)));
                obj
            }
            Some(owner) => error_object(format!("feed owner had invalid length {}", owner.len())),
            None => error_object("feed owner unavailable"),
        }
    }

    #[wasm_bindgen(js_name = feed_owner)]
    pub async fn feed_owner_alias(&self) -> Object {
        self.feed_owner().await
    }

    #[wasm_bindgen(js_name = batchState)]
    pub async fn batch_state(&self, depth: u8, validity_days: u32) -> Object {
        self.boot_runtime().await;
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
        set_js(
            &obj,
            "swarmNetworkId",
            JsValue::from_f64(profile.swarm_network_id as f64),
        );
        set_js(
            &obj,
            "walletChainId",
            JsValue::from_f64(profile.wallet_chain_id as f64),
        );
        set_js_str(&obj, "wallet", hex_address(payer));
        set_js_str(&obj, "chainId", chain_id.to_string());
        set_js(&obj, "hasBatch", JsValue::from_bool(secure_state.has_batch));
        set_js(
            &obj,
            "usableBatch",
            JsValue::from_bool(secure_state.usable()),
        );
        set_js_str(&obj, "batchId", hex::encode(&secure_state.batch_id));
        set_js(
            &obj,
            "batchBucketLimit",
            JsValue::from_f64(secure_state.batch_bucket_limit as f64),
        );
        set_js_str(
            &obj,
            "batchValidityStatus",
            &secure_state.batch_validity_status,
        );
        set_js(&obj, "depth", JsValue::from_f64(depth as f64));
        set_js(
            &obj,
            "validityDays",
            JsValue::from_f64(validity_days as f64),
        );

        if chain_id != U256::from(profile.wallet_chain_id) {
            set_js_str(&obj, "status", "wrong_network");
            return obj;
        }

        let postage = match postage_contract(&w3).await {
            Ok(postage) => postage,
            Err(error) => return error_object(format!("postage contract failed: {error:?}")),
        };
        let token = match token_contract(&w3).await {
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
            let remaining = get_batch_validity(secure_state.batch_id.clone()).await;
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
        set_js(
            &obj,
            "hasFunds",
            JsValue::from_bool(token_balance >= required),
        );
        obj
    }

    #[wasm_bindgen(js_name = batch_state)]
    pub async fn batch_state_alias(&self, depth: u8, validity_days: u32) -> Object {
        self.batch_state(depth, validity_days).await
    }

    #[wasm_bindgen(js_name = uploadPrerequisites)]
    pub async fn upload_prerequisites(&self, depth: u8, validity_days: u32) -> Object {
        self.batch_state(depth, validity_days).await
    }

    #[wasm_bindgen(js_name = upload_prerequisites)]
    pub async fn upload_prerequisites_alias(&self, depth: u8, validity_days: u32) -> Object {
        self.upload_prerequisites(depth, validity_days).await
    }

    #[wasm_bindgen(js_name = buyBatch)]
    pub async fn buy_batch(&self, depth: u8, validity_days: u32) -> Object {
        self.boot_runtime().await;
        let profile = active_profile();
        let payer = match request_wallet_address().await {
            Ok(payer) => payer,
            Err(error) => return error_object(error),
        };
        if let Some(state) =
            secure_batch_state_for_wallet(payer.as_bytes(), profile.swarm_network_id).await
        {
            if state.usable() {
                let obj = ok_object();
                set_js_str(&obj, "status", "already_ready");
                set_js_str(&obj, "batchId", hex::encode(&state.batch_id));
                set_js(
                    &obj,
                    "batchBucketLimit",
                    JsValue::from_f64(state.batch_bucket_limit as f64),
                );
                return obj;
            }
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
        set_js(&obj, "depth", JsValue::from_f64(prepared.depth as f64));
        set_js(
            &obj,
            "validityDays",
            JsValue::from_f64(prepared.validity_days as f64),
        );
        set_js(
            &obj,
            "batchBucketLimit",
            JsValue::from_f64(purchase.bucket_limit as f64),
        );
        set_js_str(
            &obj,
            "approveTx",
            hex::encode(purchase.approve_tx.as_bytes()),
        );
        set_js_str(&obj, "createTx", hex::encode(purchase.create_tx.as_bytes()));
        set_js(&obj, "lastPrice", u256_string(purchase.last_price));
        obj
    }

    #[wasm_bindgen(js_name = buy_batch)]
    pub async fn buy_batch_alias(&self, depth: u8, validity_days: u32) -> Object {
        self.buy_batch(depth, validity_days).await
    }

    #[wasm_bindgen(js_name = deployChequebook)]
    pub async fn deploy_chequebook(&self) -> Object {
        self.boot_runtime().await;
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
        if !set_chequebook_address(&deployment.chequebook.as_bytes().to_vec()).await {
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

    #[wasm_bindgen(js_name = deploy_chequebook)]
    pub async fn deploy_chequebook_alias(&self) -> Object {
        self.deploy_chequebook().await
    }

    #[wasm_bindgen(js_name = depositChequebook)]
    pub async fn deposit_chequebook(&self, amount: String) -> Object {
        self.boot_runtime().await;
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
        let token = match token_contract(&w3).await {
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

    #[wasm_bindgen(js_name = deposit_chequebook)]
    pub async fn deposit_chequebook_alias(&self, amount: String) -> Object {
        self.deposit_chequebook(amount).await
    }

    #[wasm_bindgen(js_name = resetStamp)]
    pub async fn reset_stamp(&self) -> Object {
        self.boot_runtime().await;
        let raw = self.inner.reset_stamp().await;
        let (data, _indx) = decode_resources(raw);
        let obj = Object::new();
        if let Some((bytes, mime, path)) = data.get(0) {
            let _ = Reflect::set(
                &obj,
                &"message".into(),
                &JsValue::from_str(&String::from_utf8_lossy(bytes)),
            );
            let _ = Reflect::set(&obj, &"mime".into(), &JsValue::from_str(mime));
            let _ = Reflect::set(&obj, &"path".into(), &JsValue::from_str(path));
        }
        obj
    }

    #[wasm_bindgen(js_name = reset_stamp)]
    pub async fn reset_stamp_alias(&self) -> Object {
        self.reset_stamp().await
    }
}
