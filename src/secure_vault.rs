use js_sys::{Array, Function, Map, Object, Promise, Reflect, Uint8Array};
use std::{cell::RefCell, rc::Rc, time::Duration};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::network_profile::active_profile;

const VAULT_ORIGIN: &str = "https://weeb-3-secure.github.io";
const VAULT_URL: &str = "https://weeb-3-secure.github.io/vault/";
const VAULT_MODULE_URL: &str = "https://weeb-3-secure.github.io/vault/weeb3_secure_vault.js";
const CLIENT_NAME: &str = "official-weeb-3-shell";
const POPUP_NAME: &str = "weeb3-secure-vault";
const SECURE_CALL_ATTEMPTS: usize = 3;
const RESUME_NOTICE_ID: &str = "secureVaultResumeNotice";
const DEBUG_SECURE_VAULT_LOGS: bool = false;

macro_rules! secure_vault_log {
    ($($arg:tt)*) => {
        if DEBUG_SECURE_VAULT_LOGS {
            web_sys::console::log_1(&JsValue::from(format!($($arg)*)));
        }
    };
}

thread_local! {
    static SECURE_MODULE: RefCell<Option<JsValue>> = RefCell::new(None);
    static SECURE_CLIENT: RefCell<Option<Rc<JsValue>>> = RefCell::new(None);
    static SECURE_WALLET: RefCell<Option<Vec<u8>>> = RefCell::new(None);
    static SECURE_NETWORK_ID: RefCell<Option<u64>> = RefCell::new(None);
    static SECURE_RESUME_WAITERS: RefCell<Vec<Function>> = RefCell::new(Vec::new());
    static SECURE_CLICK_CONNECT_PROMISE: RefCell<Option<Promise>> = RefCell::new(None);
    static SECURE_CLICK_CONNECT_OPTIONS: RefCell<Option<JsValue>> = RefCell::new(None);
    static SECURE_VAULT_WINDOW: RefCell<Option<web_sys::Window>> = RefCell::new(None);
    static SECURE_RESUME_REQUIRED: RefCell<bool> = RefCell::new(false);
}

pub struct SecureBatchState {
    pub has_batch: bool,
    pub batch_id: Vec<u8>,
    pub batch_bucket_limit: u32,
    pub batch_validity_status: String,
}

impl SecureBatchState {
    pub fn usable(&self) -> bool {
        self.has_batch && self.batch_id.len() == 32 && self.batch_validity_status != "expired"
    }
}

pub struct SecurePreparedBatch {
    pub owner: Vec<u8>,
    pub depth: u8,
    pub validity_days: u64,
}

pub struct SecureFeedUpdate {
    pub bucket_full: bool,
    pub soc_chunk: Vec<u8>,
    pub soc_address: Vec<u8>,
    pub stamp: Vec<u8>,
}

pub async fn secure_batch_state_for_wallet(
    wallet: &[u8],
    network_id: u64,
) -> Option<SecureBatchState> {
    let client = secure_client_for_wallet(wallet).await?;
    check_batch_state(client, network_id).await
}

pub async fn secure_ensure_authorized() -> bool {
    secure_client().await.is_some()
}

pub fn secure_preload_vault_module() {
    spawn_local(async {
        if let Err(error) = secure_module().await {
            log_error("secure vault module preload failed", &error);
        }
    });
}

async fn check_batch_state(client: Rc<JsValue>, network_id: u64) -> Option<SecureBatchState> {
    let options = auth_options_for_network(network_id).ok()?;
    let state = match call_secure_client(&client, "checkBatchState", options).await {
        Ok(state) => state,
        Err(e) => {
            log_error("secure checkBatchState failed", &e);
            return None;
        }
    };

    let state = SecureBatchState {
        has_batch: bool_prop(&state, "hasBatch"),
        batch_id: string_prop(&state, "batchIdHex")
            .and_then(|value| hex::decode(strip_0x(&value)).ok())
            .unwrap_or_default(),
        batch_bucket_limit: u32_prop(&state, "batchBucketLimit"),
        batch_validity_status: string_prop(&state, "batchValidityStatus")
            .unwrap_or_else(|| "unknown".to_string()),
    };

    secure_vault_log!(
        "secure batch state: hasBatch={}, batchIdLen={}, bucketLimit={}, validity={}",
        state.has_batch,
        state.batch_id.len(),
        state.batch_bucket_limit,
        state.batch_validity_status
    );

    Some(state)
}

pub async fn secure_prepare_batch_purchase(
    depth: u8,
    validity_days: u64,
    network_id: u64,
) -> Option<SecurePreparedBatch> {
    let client = secure_client_or_resume("prepareBatchPurchase").await?;
    let options = auth_options_for_network(network_id).ok()?;
    set_prop(&options, "depth", JsValue::from_f64(depth as f64)).ok()?;
    set_prop(
        &options,
        "validityDays",
        JsValue::from_f64(validity_days as f64),
    )
    .ok()?;

    let prepared = match call_secure_client(&client, "prepareBatchPurchase", options).await {
        Ok(prepared) => prepared,
        Err(e) => {
            log_error("secure prepareBatchPurchase failed", &e);
            return None;
        }
    };

    let owner = string_prop(&prepared, "batchOwnerAddressHex")
        .and_then(|value| hex::decode(strip_0x(&value)).ok())?;

    Some(SecurePreparedBatch {
        owner,
        depth: u32_prop(&prepared, "depth") as u8,
        validity_days: u64_prop(&prepared, "validityDays"),
    })
}

pub async fn secure_commit_batch_purchase(
    batch_id: &[u8],
    batch_bucket_limit: u32,
    batch_depth: u8,
    network_id: u64,
) -> bool {
    let Some(client) = secure_client_or_resume("commitBatchPurchase").await else {
        return false;
    };
    let Ok(options) = auth_options_for_network(network_id) else {
        return false;
    };
    set_prop(
        &options,
        "batchIdHex",
        JsValue::from_str(&hex::encode(batch_id)),
    )
    .ok();
    set_prop(
        &options,
        "batchBucketLimit",
        JsValue::from_f64(batch_bucket_limit as f64),
    )
    .ok();
    set_prop(
        &options,
        "batchDepth",
        JsValue::from_f64(batch_depth as f64),
    )
    .ok();

    match call_secure_client(&client, "commitBatchPurchase", options).await {
        Ok(_) => true,
        Err(e) => {
            log_error("secure commitBatchPurchase failed", &e);
            false
        }
    }
}

pub async fn secure_commit_batch_purchase_and_verify(
    wallet: &[u8],
    batch_id: &[u8],
    batch_bucket_limit: u32,
    batch_depth: u8,
    network_id: u64,
) -> bool {
    if !secure_commit_batch_purchase(batch_id, batch_bucket_limit, batch_depth, network_id).await {
        return false;
    }

    let Some(state) = secure_batch_state_for_wallet(wallet, network_id).await else {
        secure_vault_log!("secure commit verification failed: batch state unavailable");
        return false;
    };

    let saved = state.has_batch
        && state.batch_id.as_slice() == batch_id
        && state.batch_bucket_limit == batch_bucket_limit;
    if !saved {
        secure_vault_log!(
            "secure commit verification failed: hasBatch={}, savedBatchIdLen={}, savedBucketLimit={}",
            state.has_batch,
            state.batch_id.len(),
            state.batch_bucket_limit
        );
    }
    saved
}

pub async fn secure_stamp_chunk(chunk_address: Vec<u8>) -> (Vec<u8>, bool) {
    let Some(client) = secure_client_or_resume("stampChunk").await else {
        return (vec![], false);
    };
    let Ok(options) = auth_options_for_network(active_profile().swarm_network_id) else {
        return (vec![], false);
    };
    set_prop(&options, "chunkAddress", bytes_value(&chunk_address)).ok();

    let signed = match call_secure_client(&client, "stampChunk", options).await {
        Ok(signed) => signed,
        Err(e) => {
            log_error("secure stampChunk failed", &e);
            return (vec![], false);
        }
    };

    if bool_prop(&signed, "bucketFull") {
        return (vec![], true);
    }

    let stamp = string_prop(&signed, "stampHex")
        .and_then(|value| hex::decode(strip_0x(&value)).ok())
        .unwrap_or_default();

    (stamp, false)
}

pub async fn secure_reset_stamp() -> bool {
    let Some(client) = secure_client_or_resume("resetStamp").await else {
        return false;
    };
    let Ok(options) = auth_options_for_network(active_profile().swarm_network_id) else {
        return false;
    };

    match call_secure_client(&client, "resetStamp", options).await {
        Ok(_) => true,
        Err(e) => {
            log_error("secure resetStamp failed", &e);
            false
        }
    }
}

pub async fn secure_ensure_feed_owner() -> Option<Vec<u8>> {
    let client = secure_client_or_resume("ensureFeedOwner").await?;
    let options = auth_options_for_network(active_network_id()).ok()?;
    let feed_owner = match call_secure_client(&client, "ensureFeedOwner", options).await {
        Ok(feed_owner) => feed_owner,
        Err(e) => {
            log_error("secure ensureFeedOwner failed", &e);
            return None;
        }
    };

    string_prop(&feed_owner, "feedOwnerAddressHex")
        .and_then(|value| hex::decode(strip_0x(&value)).ok())
}

pub async fn secure_create_feed_update_soc_with_stamp(
    topic: String,
    feed_index: u64,
    wrapped_content: Vec<u8>,
) -> Option<SecureFeedUpdate> {
    let client = secure_client_or_resume("createFeedUpdateSocWithStamp").await?;
    let options = auth_options_for_network(active_profile().swarm_network_id).ok()?;
    set_prop(&options, "topic", JsValue::from_str(&topic)).ok()?;
    set_prop(&options, "feedIndex", JsValue::from_f64(feed_index as f64)).ok()?;
    set_prop(&options, "wrappedContent", bytes_value(&wrapped_content)).ok()?;

    let signed = match call_secure_client(&client, "createFeedUpdateSocWithStamp", options).await {
        Ok(signed) => signed,
        Err(e) => {
            log_error("secure createFeedUpdateSocWithStamp failed", &e);
            return None;
        }
    };

    if bool_prop(&signed, "bucketFull") {
        return Some(SecureFeedUpdate {
            bucket_full: true,
            soc_chunk: vec![],
            soc_address: vec![],
            stamp: vec![],
        });
    }

    Some(SecureFeedUpdate {
        bucket_full: false,
        soc_chunk: hex_prop(&signed, "socChunkHex"),
        soc_address: hex_prop(&signed, "socAddressHex"),
        stamp: hex_prop(&signed, "stampHex"),
    })
}

pub fn secure_open_vault_from_user_action() {
    if SECURE_CLIENT.with(|cell| cell.borrow().is_some()) {
        if !secure_vault_window_closed() {
            return;
        }
        clear_secure_connection();
    }

    let Ok(options) = connect_options_with_popup_name(&fresh_popup_name()) else {
        return;
    };
    SECURE_CLICK_CONNECT_OPTIONS.with(|cell| {
        *cell.borrow_mut() = Some(options.clone());
    });
    if let Err(error) = preopen_secure_vault_window(&options) {
        log_error("secure vault user-action preopen failed", &error);
        return;
    }

    match SECURE_MODULE.with(|cell| cell.borrow().clone()) {
        Some(module) => match start_secure_client_connect(&module, options) {
            Ok(promise) => {
                SECURE_CLICK_CONNECT_PROMISE.with(|cell| {
                    *cell.borrow_mut() = Some(promise);
                });
                focus_current_window_soon();
            }
            Err(error) => {
                log_error("secure vault user-action connect failed", &error);
                focus_current_window_soon();
            }
        },
        None => {
            focus_current_window_soon();
        }
    }
}

fn active_network_id() -> u64 {
    active_profile().swarm_network_id
}

fn secure_network_matches(network_id: u64) -> bool {
    SECURE_NETWORK_ID.with(|cell| {
        cell.borrow()
            .map(|current| current == network_id)
            .unwrap_or(false)
    })
}

async fn secure_client() -> Option<Rc<JsValue>> {
    if let Some(client) = SECURE_CLIENT.with(|cell| cell.borrow().clone()) {
        if secure_network_matches(active_network_id()) {
            return Some(client);
        }
        match authorize_secure_client(client).await {
            Ok(client) => return Some(client),
            Err(error) => {
                log_error("secure authorizeTempAuth for network switch failed", &error);
                clear_secure_connection();
            }
        }
    }

    let client = match SECURE_CLICK_CONNECT_PROMISE.with(|cell| cell.borrow_mut().take()) {
        Some(promise) => match JsFuture::from(promise).await {
            Ok(client) => Rc::new(client),
            Err(e) => {
                log_error("secure vault click-started connect failed", &e);
                match take_click_connect_options() {
                    Some(options) => match connect_secure_client(options).await {
                        Ok(client) => Rc::new(client),
                        Err(error) => {
                            log_error("secure vault click-started reconnect failed", &error);
                            return None;
                        }
                    },
                    None => return None,
                }
            }
        },
        None => match take_click_connect_options() {
            Some(options) => match connect_secure_client(options).await {
                Ok(client) => Rc::new(client),
                Err(e) => {
                    log_error("secure vault click-reserved connect failed", &e);
                    return None;
                }
            },
            None => match connect_options() {
                Ok(options) => match connect_secure_client(options).await {
                    Ok(client) => Rc::new(client),
                    Err(e) => {
                        log_error("secure vault connect failed", &e);
                        return None;
                    }
                },
                Err(e) => {
                    log_error("secure vault options failed", &e);
                    return None;
                }
            },
        },
    };

    match authorize_secure_client(client).await {
        Ok(client) => {
            clear_click_connect_options();
            Some(client)
        }
        Err(e) => {
            if reconnectable_error(&e) {
                log_error("secure authorizeTempAuth reconnecting", &e);
                if let Some(client) = reconnect_and_authorize_after_auth_error().await {
                    return Some(client);
                }
            } else {
                clear_click_connect_options();
            }
            log_error("secure authorizeTempAuth failed", &e);
            None
        }
    }
}

async fn reconnect_and_authorize_after_auth_error() -> Option<Rc<JsValue>> {
    clear_secure_connection();
    let options = take_click_connect_options().or_else(|| connect_options().ok())?;
    let client = match connect_secure_client(options).await {
        Ok(client) => Rc::new(client),
        Err(error) => {
            log_error("secure authorizeTempAuth reconnect failed", &error);
            return None;
        }
    };
    match authorize_secure_client(client).await {
        Ok(client) => {
            clear_click_connect_options();
            Some(client)
        }
        Err(error) => {
            log_error("secure authorizeTempAuth retry failed", &error);
            None
        }
    }
}

async fn secure_client_or_resume(_context: &str) -> Option<Rc<JsValue>> {
    if secure_vault_window_closed() {
        mark_secure_resume_required();
    }

    if secure_resume_required() {
        return wait_for_user_resume_connection(_context).await.ok();
    }

    match secure_client().await {
        Some(client) => Some(client),
        None if secure_resume_required() => wait_for_user_resume_connection(_context).await.ok(),
        None => None,
    }
}

async fn authorize_secure_client(client: Rc<JsValue>) -> Result<Rc<JsValue>, JsValue> {
    let network_id = active_network_id();
    let auth = match auth_options_for_network(network_id) {
        Ok(auth) => auth,
        Err(e) => {
            return Err(e);
        }
    };
    let topics = Array::new();
    topics.push(&JsValue::from_str("*"));
    set_prop(&auth, "allowedTopics", topics.into()).ok();
    set_prop(
        &auth,
        "ttlMs",
        JsValue::from_f64(168.0 * 60.0 * 60.0 * 1000.0),
    )
    .ok();

    let grant = match call_client(&client, "authorizeTempAuth", auth).await {
        Ok(grant) => grant,
        Err(e) => return Err(e),
    };

    let wallet = string_prop(&grant, "walletAddressHex")
        .and_then(|value| hex::decode(strip_0x(&value)).ok())
        .unwrap_or_default();
    let grant_network_id = u64_prop(&grant, "networkId");
    if grant_network_id != network_id {
        return Err(JsValue::from_str(&format!(
            "secure vault network changed during authorization: expected {}, got {}",
            network_id, grant_network_id
        )));
    }

    if let Some(expected_wallet) = SECURE_WALLET.with(|cell| cell.borrow().clone()) {
        if !expected_wallet.is_empty() && wallet != expected_wallet {
            return Err(JsValue::from_str(&format!(
                "secure vault wallet changed during reconnect: expected 0x{}, got 0x{}",
                hex::encode(expected_wallet),
                hex::encode(wallet)
            )));
        }
    }

    SECURE_CLIENT.with(|cell| {
        *cell.borrow_mut() = Some(client.clone());
    });
    SECURE_WALLET.with(|cell| {
        *cell.borrow_mut() = Some(wallet);
    });
    SECURE_NETWORK_ID.with(|cell| {
        *cell.borrow_mut() = Some(network_id);
    });
    clear_secure_resume_required();

    Ok(client)
}

async fn secure_client_for_wallet(wallet: &[u8]) -> Option<Rc<JsValue>> {
    let wallet = wallet.to_vec();
    let matches_current = SECURE_WALLET.with(|cell| cell.borrow().as_ref().map(|w| w == &wallet));

    if matches_current == Some(true) {
        return secure_client().await;
    }

    if matches_current == Some(false) {
        clear_secure_client();
    }

    let client = secure_client_or_resume("checkBatchState").await?;
    let matches_new = SECURE_WALLET.with(|cell| cell.borrow().as_ref().map(|w| w == &wallet));
    if matches_new == Some(true) {
        Some(client)
    } else {
        log_error(
            "secure vault wallet does not match connected wallet",
            &JsValue::from_str(&format!(
                "expected 0x{}, got {}",
                hex::encode(wallet),
                SECURE_WALLET
                    .with(|cell| cell.borrow().clone())
                    .map(hex::encode)
                    .map(|wallet| format!("0x{wallet}"))
                    .unwrap_or_else(|| "(none)".to_string())
            )),
        );
        clear_secure_client();
        None
    }
}

fn clear_secure_connection() {
    SECURE_CLIENT.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

fn mark_secure_resume_required() {
    SECURE_RESUME_REQUIRED.with(|cell| {
        *cell.borrow_mut() = true;
    });
    clear_secure_connection();
}

fn clear_secure_resume_required() {
    SECURE_RESUME_REQUIRED.with(|cell| {
        *cell.borrow_mut() = false;
    });
}

fn secure_resume_required() -> bool {
    SECURE_RESUME_REQUIRED.with(|cell| *cell.borrow())
}

fn clear_secure_connection_if_current(client: &Rc<JsValue>) {
    SECURE_CLIENT.with(|cell| {
        let should_clear = cell
            .borrow()
            .as_ref()
            .map(|current| Rc::ptr_eq(current, client))
            .unwrap_or(false);
        if should_clear {
            *cell.borrow_mut() = None;
        }
    });
}

fn clear_secure_client() {
    clear_secure_connection();
    SECURE_WALLET.with(|cell| {
        *cell.borrow_mut() = None;
    });
    SECURE_NETWORK_ID.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

async fn connect_secure_client(options: JsValue) -> Result<JsValue, JsValue> {
    let module = secure_module().await?;
    let promise = start_secure_client_connect(&module, options)?;
    JsFuture::from(promise).await
}

fn take_click_connect_options() -> Option<JsValue> {
    SECURE_CLICK_CONNECT_OPTIONS.with(|cell| cell.borrow_mut().take())
}

fn clear_click_connect_options() {
    SECURE_CLICK_CONNECT_OPTIONS.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

fn preopen_secure_vault_window(options: &JsValue) -> Result<(), JsValue> {
    let vault_url = string_prop(options, "vaultUrl")
        .ok_or_else(|| JsValue::from_str("secure vaultUrl missing"))?;
    let popup_name = string_prop(options, "popupName")
        .ok_or_else(|| JsValue::from_str("secure popupName missing"))?;
    let vault_window = web_sys::window()
        .ok_or_else(|| JsValue::from_str("window missing"))?
        .open_with_url_and_target_and_features(
            &vault_url,
            &popup_name,
            "popup,width=580,height=780",
        )?
        .ok_or_else(|| JsValue::from_str("Could not open weeb-3-secure popup"))?;
    SECURE_VAULT_WINDOW.with(|cell| {
        *cell.borrow_mut() = Some(vault_window);
    });
    Ok(())
}

fn start_secure_client_connect(module: &JsValue, options: JsValue) -> Result<Promise, JsValue> {
    let constructor = Reflect::get(&module, &JsValue::from_str("Weeb3SecureVaultClient"))?;
    let connect =
        Reflect::get(&constructor, &JsValue::from_str("connect"))?.dyn_into::<Function>()?;
    connect.call1(&constructor, &options)?.dyn_into::<Promise>()
}

fn begin_secure_client_connect_from_click() -> Result<Promise, JsValue> {
    let module = SECURE_MODULE
        .with(|cell| cell.borrow().clone())
        .ok_or_else(|| JsValue::from_str("secure vault module is not loaded"))?;
    let options = connect_options_with_popup_name(&fresh_popup_name())?;
    preopen_secure_vault_window(&options)?;
    let promise = start_secure_client_connect(&module, options)?;
    focus_current_window_soon();
    Ok(promise)
}

fn ensure_resume_connection_prompt(context: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };

    if document.get_element_by_id(RESUME_NOTICE_ID).is_some() {
        return;
    }

    let Ok(notice) = document.create_element("div") else {
        return;
    };
    notice.set_id(RESUME_NOTICE_ID);
    notice.set_class_name("secure-vault-resume");

    let Ok(message) = document.create_element("span") else {
        return;
    };
    message.set_text_content(Some(&format!(
        "weeb-3-secure connection paused during {context}. "
    )));
    notice.append_child(&message).ok();

    let Ok(button) = document.create_element("button") else {
        return;
    };
    button.set_text_content(Some("Resume weeb-3-secure connection"));
    notice.append_child(&button).ok();

    let button_for_click = button.clone();
    let notice_for_click = notice.clone();
    let callback = Closure::<dyn FnMut(JsValue)>::new(move |_event| {
        button_for_click.set_text_content(Some("Opening weeb-3-secure..."));
        button_for_click.set_attribute("disabled", "true").ok();

        let promise = match begin_secure_client_connect_from_click() {
            Ok(promise) => promise,
            Err(error) => {
                log_error("secure resume connection failed to start", &error);
                button_for_click.set_text_content(Some("Resume weeb-3-secure connection"));
                button_for_click.remove_attribute("disabled").ok();
                return;
            }
        };

        let button_for_retry = button_for_click.clone();
        let notice_for_success = notice_for_click.clone();
        wasm_bindgen_futures::spawn_local(async move {
            clear_secure_connection();
            let client = match JsFuture::from(promise).await {
                Ok(client) => Rc::new(client),
                Err(error) => {
                    log_error("secure resume connection failed", &error);
                    button_for_retry.set_text_content(Some("Resume weeb-3-secure connection"));
                    button_for_retry.remove_attribute("disabled").ok();
                    return;
                }
            };

            if let Err(error) = authorize_secure_client(client).await {
                log_error("secure resume authorization failed", &error);
                button_for_retry.set_text_content(Some("Resume weeb-3-secure connection"));
                button_for_retry.remove_attribute("disabled").ok();
                return;
            }

            remove_element(&notice_for_success);
            resolve_resume_waiters();
        });
    });

    button
        .add_event_listener_with_callback("click", callback.as_ref().unchecked_ref())
        .ok();
    callback.forget();

    if let Some(result) = result_field(&document) {
        result.prepend_with_node_1(&notice).ok();
        return;
    }

    if let Some(body) = document.body() {
        if let Ok(result) = document.create_element("div") {
            result.set_id("resultField");
            body.prepend_with_node_1(&result).ok();
            if let Some(result) = result.dyn_ref::<web_sys::HtmlElement>() {
                result.prepend_with_node_1(&notice).ok();
                return;
            }
        }
    }

    if let Some(logs) = document.get_element_by_id("logsField") {
        if let Some(logs) = logs.dyn_ref::<web_sys::HtmlElement>() {
            logs.prepend_with_node_1(&notice).ok();
            return;
        }
    }

    if let Some(body) = document.body() {
        body.prepend_with_node_1(&notice).ok();
    }
}

fn resolve_resume_waiters() {
    let waiters = SECURE_RESUME_WAITERS.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    for resolve in waiters {
        resolve.call0(&JsValue::NULL).ok();
    }
}

fn result_field(document: &web_sys::Document) -> Option<web_sys::HtmlElement> {
    document
        .get_element_by_id("resultField")
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
}

fn remove_element(element: &web_sys::Element) {
    if let Some(parent) = element.parent_node() {
        parent.remove_child(element).ok();
    }
}

fn focus_current_window() {
    if let Some(window) = web_sys::window() {
        window.focus().ok();
    }
}

fn focus_current_window_soon() {
    focus_current_window();
    spawn_local(async {
        sleep_ms(0).await;
        focus_current_window();
        sleep_ms(100).await;
        focus_current_window();
    });
}

async fn call_client(client: &JsValue, method: &str, options: JsValue) -> Result<JsValue, JsValue> {
    if secure_vault_window_closed() {
        mark_secure_resume_required();
        return Err(vault_window_closed_js_error());
    }

    let method = Reflect::get(client, &JsValue::from_str(method))?.dyn_into::<Function>()?;
    let promise = method.call1(client, &options)?.dyn_into::<Promise>()?;
    await_secure_promise_or_vault_closed(promise).await
}

async fn await_secure_promise_or_vault_closed(promise: Promise) -> Result<JsValue, JsValue> {
    let mut future = std::pin::pin!(JsFuture::from(promise));

    loop {
        match async_std::future::timeout(Duration::from_millis(250), future.as_mut()).await {
            Ok(result) => return result,
            Err(_) if secure_vault_window_closed() => {
                mark_secure_resume_required();
                return Err(vault_window_closed_js_error());
            }
            Err(_) => {}
        }
    }
}

fn secure_vault_window_closed() -> bool {
    SECURE_VAULT_WINDOW.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|vault_window| {
                Reflect::get(vault_window.as_ref(), &JsValue::from_str("closed"))
                    .ok()
                    .and_then(|closed| closed.as_bool())
            })
            .unwrap_or(false)
    })
}

fn vault_window_closed_js_error() -> JsValue {
    JsValue::from_str("vault window closed")
}

async fn call_secure_client(
    client: &Rc<JsValue>,
    method: &str,
    options: JsValue,
) -> Result<JsValue, JsValue> {
    let mut active_client = client.clone();
    let mut last_error = JsValue::from_str("secure vault call failed");
    let mut attempt = 0usize;

    while attempt < SECURE_CALL_ATTEMPTS {
        match call_client(&active_client, method, options.clone()).await {
            Ok(value) => return Ok(plain_vault_result(value)),
            Err(error) => {
                last_error = error.clone();
                if vault_window_closed_error(&error) {
                    if secure_vault_window_closed() {
                        mark_secure_resume_required();
                    } else {
                        clear_secure_connection_if_current(&active_client);
                    }
                    active_client = if secure_resume_required() {
                        wait_for_user_resume_connection(method).await?
                    } else {
                        match SECURE_CLIENT.with(|cell| cell.borrow().clone()) {
                            Some(client) => client,
                            None => wait_for_user_resume_connection(method).await?,
                        }
                    };
                    continue;
                }

                if !reconnectable_error(&error) {
                    return Err(error);
                }

                attempt += 1;
                log_error(
                    &format!("secure {method} reconnect attempt {attempt}"),
                    &error,
                );
                clear_secure_connection_if_current(&active_client);
                sleep_ms(250 * attempt as i32).await;
                active_client = if secure_resume_required() {
                    wait_for_user_resume_connection(method).await?
                } else {
                    match secure_client().await {
                        Some(client) => client,
                        None if vault_window_closed_error(&error) => {
                            wait_for_user_resume_connection(method).await?
                        }
                        None => return Err(error),
                    }
                };
            }
        }
    }

    Err(last_error)
}

async fn wait_for_user_resume_connection(context: &str) -> Result<Rc<JsValue>, JsValue> {
    ensure_resume_connection_prompt(context);
    let promise = Promise::new(&mut |resolve, _reject| {
        SECURE_RESUME_WAITERS.with(|cell| {
            cell.borrow_mut().push(resolve.clone());
        });
    });
    JsFuture::from(promise).await?;
    SECURE_CLIENT
        .with(|cell| cell.borrow().clone())
        .ok_or_else(|| JsValue::from_str("secure vault resume did not set a client"))
}

async fn secure_module() -> Result<JsValue, JsValue> {
    if let Some(module) = SECURE_MODULE.with(|cell| cell.borrow().clone()) {
        return Ok(module);
    }

    let import = Function::new_with_args("url", "return import(url);");
    let promise = import
        .call1(&JsValue::NULL, &JsValue::from_str(VAULT_MODULE_URL))?
        .dyn_into::<Promise>()?;
    let module = JsFuture::from(promise).await?;
    let init = Reflect::get(&module, &JsValue::from_str("default"))?.dyn_into::<Function>()?;
    if let Ok(promise) = init.call0(&JsValue::NULL)?.dyn_into::<Promise>() {
        JsFuture::from(promise).await?;
    }

    SECURE_MODULE.with(|cell| {
        *cell.borrow_mut() = Some(module.clone());
    });

    Ok(module)
}

async fn sleep_ms(ms: i32) {
    let promise = Promise::new(&mut |resolve, _reject| {
        if let Some(window) = web_sys::window() {
            window
                .set_timeout_with_callback_and_timeout_and_arguments_0(resolve.unchecked_ref(), ms)
                .ok();
        }
    });
    JsFuture::from(promise).await.ok();
}

fn connect_options() -> Result<JsValue, JsValue> {
    connect_options_with_popup_name(POPUP_NAME)
}

fn connect_options_with_popup_name(popup_name: &str) -> Result<JsValue, JsValue> {
    let origin = current_origin()?;
    let options = Object::new();
    let vault_url = format!(
        "{VAULT_URL}?allow={}&connect={}",
        js_sys::encode_uri_component(&origin),
        js_sys::Date::now() as u64
    );
    set_prop(&options, "vaultUrl", JsValue::from_str(&vault_url))?;
    set_prop(&options, "targetOrigin", JsValue::from_str(VAULT_ORIGIN))?;
    set_prop(
        &options,
        "clientName",
        JsValue::from_str(&format!("{CLIENT_NAME}:{origin}")),
    )?;
    set_prop(&options, "popupName", JsValue::from_str(popup_name))?;
    Ok(options.into())
}

fn fresh_popup_name() -> String {
    format!("{}-resume-{}", POPUP_NAME, js_sys::Date::now() as u64)
}

fn auth_options() -> Result<JsValue, JsValue> {
    let options = Object::new();
    set_prop(&options, "appId", JsValue::from_str(&current_origin()?))?;
    Ok(options.into())
}

fn auth_options_for_network(network_id: u64) -> Result<JsValue, JsValue> {
    let options = auth_options()?;
    set_prop(&options, "networkId", JsValue::from_f64(network_id as f64))?;
    Ok(options)
}

fn current_origin() -> Result<String, JsValue> {
    web_sys::window()
        .ok_or_else(|| JsValue::from_str("window missing"))?
        .location()
        .origin()
}

fn set_prop(target: &JsValue, name: &str, value: JsValue) -> Result<bool, JsValue> {
    Reflect::set(target, &JsValue::from_str(name), &value)
}

fn js_value_field(value: &JsValue, name: &str) -> Option<JsValue> {
    let key = JsValue::from_str(name);
    if let Ok(field) = Reflect::get(value, &key) {
        if !field.is_null() && !field.is_undefined() {
            return Some(field);
        }
    }

    value.dyn_ref::<Map>().and_then(|map| {
        let field = map.get(&key);
        if field.is_null() || field.is_undefined() {
            None
        } else {
            Some(field)
        }
    })
}

fn to_plain_vault_value(value: &JsValue) -> JsValue {
    if let Some(map) = value.dyn_ref::<Map>() {
        let out = Object::new();
        let entries = Array::from(&map.entries());
        for i in 0..entries.length() {
            let entry = Array::from(&entries.get(i));
            if entry.length() < 2 {
                continue;
            }
            let key = entry.get(0);
            let value = to_plain_vault_value(&entry.get(1));
            let _ = Reflect::set(&out, &key, &value);
        }
        return out.into();
    }

    value.clone()
}

fn plain_vault_result(value: JsValue) -> JsValue {
    let plain = to_plain_vault_value(&value);
    if let Some(result) = js_value_field(&plain, "result") {
        return to_plain_vault_value(&result);
    }
    plain
}

fn string_prop(value: &JsValue, name: &str) -> Option<String> {
    js_value_field(value, name).and_then(|v| v.as_string())
}

fn bool_prop(value: &JsValue, name: &str) -> bool {
    js_value_field(value, name)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn u32_prop(value: &JsValue, name: &str) -> u32 {
    js_value_field(value, name)
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_string().and_then(|text| text.parse::<f64>().ok()))
        })
        .unwrap_or(0.0) as u32
}

fn u64_prop(value: &JsValue, name: &str) -> u64 {
    js_value_field(value, name)
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_string().and_then(|text| text.parse::<f64>().ok()))
        })
        .unwrap_or(0.0) as u64
}

fn hex_prop(value: &JsValue, name: &str) -> Vec<u8> {
    string_prop(value, name)
        .and_then(|value| hex::decode(strip_0x(&value)).ok())
        .unwrap_or_default()
}

fn strip_0x(value: &str) -> &str {
    value.strip_prefix("0x").unwrap_or(value)
}

fn bytes_value(bytes: &[u8]) -> JsValue {
    let value = Uint8Array::new_with_length(bytes.len() as u32);
    value.copy_from(bytes);
    value.into()
}

fn reconnectable_error(error: &JsValue) -> bool {
    let text = js_error_text(error).to_ascii_lowercase();
    text.contains("popup")
        || text.contains("user gesture")
        || text.contains("vault not ready")
        || text.contains("did not become ready")
        || text.contains("request stalled")
        || text.contains("vault request timed out")
        || text.contains("vault response channel closed")
        || text.contains("vault session was reconnected")
        || text.contains("stale request")
        || text.contains("vault reconnect")
        || text.contains("closed")
}

fn vault_window_closed_error(error: &JsValue) -> bool {
    let text = js_error_text(error).to_ascii_lowercase();
    text.contains("vault window closed")
        || text.contains("weeb-3-secure window closed")
        || text.contains("weeb-3-secure popup closed")
        || text.contains("popup window closed")
}

fn js_error_text(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| string_prop(value, "message"))
        .unwrap_or_else(|| format!("{value:?}"))
}

fn log_error(context: &str, error: &JsValue) {
    web_sys::console::log_1(&JsValue::from(format!("{context}: {error:?}")));
}
