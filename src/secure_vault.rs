use event_listener::Event;
use js_sys::{Array, Function, Map, Object, Promise, Reflect, Uint8Array};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web3::types::U256;

use crate::{
    feed::{exact_js_feed_index, sequence_feed_id},
    network_profile::active_profile,
    strip_hex_prefix, valid_soc,
    worker_protocol::{
        bytes_to_js, set as set_js, set_bool as set_js_bool, set_number as set_js_number,
        set_string as set_js_string,
    },
};

const VAULT_ORIGIN: &str = "https://weeb-3-secure.github.io";
const VAULT_URL: &str = "https://weeb-3-secure.github.io/vault/";
const VAULT_MODULE_URL: &str = "https://weeb-3-secure.github.io/vault/weeb3_secure_vault.js";
const CLIENT_NAME: &str = "official-weeb-3-shell";
const POPUP_NAME: &str = "weeb3-secure-vault";
const SECURE_CALL_ATTEMPTS: usize = 3;
const RESUME_NOTICE_ID: &str = "secureVaultResumeNotice";

thread_local! {
    static SECURE_MODULE: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    static SECURE_CLIENT: RefCell<Option<Rc<JsValue>>> = const { RefCell::new(None) };
    static SECURE_WALLET: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    static SECURE_NETWORK_ID: Cell<Option<u64>> = const { Cell::new(None) };
    static SECURE_RESUMED: Event = const { Event::new() };
    static SECURE_CLICK_CONNECT_PROMISE: RefCell<Option<Promise>> = const { RefCell::new(None) };
    static SECURE_CLICK_CONNECT_OPTIONS: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    static SECURE_VAULT_WINDOW: RefCell<Option<web_sys::Window>> = const { RefCell::new(None) };
    static SECURE_RESUME_REQUIRED: Cell<bool> = const { Cell::new(false) };
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

async fn worker_vault_call(method: &str, populate: impl FnOnce(&JsValue)) -> Option<JsValue> {
    let request = Object::new();
    set_js_string(&request, "method", method);
    set_js_number(
        &request,
        "networkId",
        active_profile().swarm_network_id as f64,
    );
    populate(request.as_ref());
    let call = Reflect::get(&js_sys::global(), &JsValue::from_str("weeb3VaultCall"))
        .ok()?
        .dyn_into::<Function>()
        .ok()?;
    let promise = call
        .call1(&JsValue::UNDEFINED, request.as_ref())
        .ok()?
        .dyn_into::<Promise>()
        .ok()?;
    let response = JsFuture::from(promise).await.ok()?;
    bool_prop(&response, "ok").then_some(response)
}

pub(crate) async fn worker_cheques_active() -> bool {
    worker_vault_call("chequesActive", |_| {})
        .await
        .is_some_and(|response| bool_prop(&response, "active"))
}

pub(crate) async fn worker_price_oracle() -> Option<(U256, U256)> {
    let response = worker_vault_call("priceOracle", |_| {}).await?;
    let price = exact_u256_prop(&response, "price")?;
    let deduction = exact_u256_prop(&response, "chequeDeduction")?;
    (!price.is_zero()).then_some((price, deduction))
}

pub(crate) async fn handle_worker_vault_request(request: &Object) -> Object {
    let response = Object::new();
    let network_id = u64_prop(request, "networkId");
    let Some(profile) = crate::network_profile::profile_for_swarm_network_id(network_id) else {
        set_js_bool(&response, "ok", false);
        set_js_string(&response, "error", "unsupported secure vault network");
        return response;
    };
    crate::network_profile::activate_profile(profile);
    let method = string_prop(request, "method").unwrap_or_default();
    let result = match method.as_str() {
        "chequesActive" => {
            set_js_bool(&response, "active", crate::cheques_active_in_window().await);
            true
        }
        "priceOracle" => match crate::on_chain::get_price_from_oracle_in_window().await {
            Some((price, deduction)) => {
                set_js(&response, "price", u256_value(price));
                set_js(&response, "chequeDeduction", u256_value(deduction));
                true
            }
            None => false,
        },
        "ensureAuthorized" => {
            set_js_bool(
                &response,
                "authorized",
                secure_ensure_authorized_in_window().await,
            );
            true
        }
        "stampChunk" => {
            let (stamp, bucket_full) = match bytes_array_prop(request, "chunkAddress")
                .filter(|address| address.length() == 32)
            {
                Some(address) => secure_stamp_chunk_in_window(address).await,
                None => (vec![], false),
            };
            let ok = bucket_full || !stamp.is_empty();
            set_js(&response, "stamp", bytes_value(&stamp));
            set_js_bool(&response, "bucketFull", bucket_full);
            ok
        }
        "resetStamp" => secure_reset_stamp_in_window().await,
        "ensureFeedOwner" => match secure_ensure_feed_owner_in_window().await {
            Some(owner) => {
                set_js(&response, "feedOwnerAddress", bytes_value(&owner));
                true
            }
            None => false,
        },
        "createFeedUpdateSocWithStamp" => {
            let topic = string_prop(request, "topic").unwrap_or_default();
            let index = u64_prop(request, "feedIndex");
            match bytes_array_prop(request, "wrappedContent") {
                Some(content) => {
                    match secure_create_feed_update_soc_with_stamp_in_window(topic, index, content)
                        .await
                    {
                        Some(update) => {
                            set_js_bool(&response, "bucketFull", update.bucket_full);
                            set_js(&response, "socChunk", bytes_value(&update.soc_chunk));
                            set_js(&response, "socAddress", bytes_value(&update.soc_address));
                            set_js(&response, "stamp", bytes_value(&update.stamp));
                            true
                        }
                        None => false,
                    }
                }
                None => false,
            }
        }
        _ => false,
    };
    set_js_bool(&response, "ok", result);
    if !result {
        set_js_string(&response, "error", "weeb-3-secure operation failed");
    }
    response
}

pub async fn secure_batch_state_for_wallet(
    wallet: &[u8],
    network_id: u64,
) -> Option<SecureBatchState> {
    let client = secure_client_for_wallet(wallet).await?;
    check_batch_state(client, network_id).await
}

pub async fn secure_ensure_authorized() -> bool {
    worker_vault_call("ensureAuthorized", |_| {})
        .await
        .is_some_and(|response| bool_prop(&response, "authorized"))
}

async fn secure_ensure_authorized_in_window() -> bool {
    secure_client().await.is_some()
}

async fn check_batch_state(client: Rc<JsValue>, network_id: u64) -> Option<SecureBatchState> {
    let options = auth_options_for_network(network_id).ok()?;
    let state = call_secure_client_logged(&client, "checkBatchState", options).await?;

    Some(SecureBatchState {
        has_batch: bool_prop(&state, "hasBatch"),
        batch_id: hex_prop(&state, "batchIdHex"),
        batch_bucket_limit: u32_prop(&state, "batchBucketLimit"),
        batch_validity_status: string_prop(&state, "batchValidityStatus")
            .unwrap_or_else(|| "unknown".to_string()),
    })
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

    let prepared = call_secure_client_logged(&client, "prepareBatchPurchase", options).await?;

    let owner = string_prop(&prepared, "batchOwnerAddressHex")
        .and_then(|value| hex::decode(strip_hex_prefix(&value)).ok())?;

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

    call_secure_client_logged(&client, "commitBatchPurchase", options)
        .await
        .is_some()
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

    secure_batch_state_for_wallet(wallet, network_id)
        .await
        .is_some_and(|state| {
            state.has_batch
                && state.batch_id.as_slice() == batch_id
                && state.batch_bucket_limit == batch_bucket_limit
        })
}

pub async fn secure_stamp_chunk(chunk_address: &[u8]) -> (Vec<u8>, bool) {
    let response = worker_vault_call("stampChunk", |request| {
        set_prop(request, "chunkAddress", bytes_value(chunk_address)).ok();
    })
    .await;
    response.map_or((vec![], false), |response| {
        (
            bytes_prop(&response, "stamp"),
            bool_prop(&response, "bucketFull"),
        )
    })
}

async fn secure_stamp_chunk_in_window(chunk_address: Uint8Array) -> (Vec<u8>, bool) {
    let Some(client) = secure_client_or_resume("stampChunk").await else {
        return (vec![], false);
    };
    let Ok(options) = auth_options_for_network(active_network_id()) else {
        return (vec![], false);
    };
    set_prop(&options, "chunkAddress", chunk_address.into()).ok();

    let Some(signed) = call_secure_client_logged(&client, "stampChunk", options).await else {
        return (vec![], false);
    };

    if bool_prop(&signed, "bucketFull") {
        return (vec![], true);
    }

    let stamp = hex_prop(&signed, "stampHex");

    (stamp, false)
}

pub async fn secure_reset_stamp() -> bool {
    worker_vault_call("resetStamp", |_| {}).await.is_some()
}

async fn secure_reset_stamp_in_window() -> bool {
    let Some(client) = secure_client_or_resume("resetStamp").await else {
        return false;
    };
    let Ok(options) = auth_options_for_network(active_network_id()) else {
        return false;
    };

    call_secure_client_logged(&client, "resetStamp", options)
        .await
        .is_some()
}

pub async fn secure_ensure_feed_owner() -> Option<Vec<u8>> {
    let response = worker_vault_call("ensureFeedOwner", |_| {}).await?;
    let owner = bytes_prop(&response, "feedOwnerAddress");
    (!owner.is_empty()).then_some(owner)
}

pub(crate) async fn secure_ensure_feed_owner_in_window() -> Option<Vec<u8>> {
    let client = secure_client_or_resume("ensureFeedOwner").await?;
    let options = auth_options_for_network(active_network_id()).ok()?;
    let feed_owner = call_secure_client_logged(&client, "ensureFeedOwner", options).await?;

    string_prop(&feed_owner, "feedOwnerAddressHex")
        .and_then(|value| hex::decode(strip_hex_prefix(&value)).ok())
}

pub async fn secure_create_feed_update_soc_with_stamp(
    topic: String,
    feed_index: u64,
    wrapped_content: Vec<u8>,
) -> Option<SecureFeedUpdate> {
    let vault_feed_index = exact_js_feed_index(feed_index)?;
    let response = worker_vault_call("createFeedUpdateSocWithStamp", |request| {
        set_prop(request, "topic", JsValue::from_str(&topic)).ok();
        set_prop(request, "feedIndex", JsValue::from_f64(vault_feed_index)).ok();
        set_prop(request, "wrappedContent", bytes_value(&wrapped_content)).ok();
    })
    .await?;
    Some(SecureFeedUpdate {
        bucket_full: bool_prop(&response, "bucketFull"),
        soc_chunk: bytes_prop(&response, "socChunk"),
        soc_address: bytes_prop(&response, "socAddress"),
        stamp: bytes_prop(&response, "stamp"),
    })
}

async fn secure_create_feed_update_soc_with_stamp_in_window(
    topic: String,
    feed_index: u64,
    wrapped_content: Uint8Array,
) -> Option<SecureFeedUpdate> {
    let topic_bytes = match hex::decode(strip_hex_prefix(&topic)) {
        Ok(topic_bytes) if !topic_bytes.is_empty() => topic_bytes,
        _ => {
            log_error(
                "secure feed update rejected invalid topic",
                &JsValue::from_str("topic must be non-empty hexadecimal bytes"),
            );
            return None;
        }
    };
    let expected_id = sequence_feed_id(&topic_bytes, feed_index, |input| {
        alloy_primitives::keccak256(input).into()
    });
    let Some(vault_feed_index) = exact_js_feed_index(feed_index) else {
        log_error(
            "secure feed update index cannot cross the JavaScript number bridge exactly",
            &JsValue::from_str(&feed_index.to_string()),
        );
        return None;
    };

    let client = secure_client_or_resume("createFeedUpdateSocWithStamp").await?;
    let options = auth_options_for_network(active_network_id()).ok()?;
    set_prop(&options, "topic", JsValue::from_str(&topic)).ok()?;
    // The signer applies Bee's big-endian index encoding.
    set_prop(&options, "feedIndex", JsValue::from_f64(vault_feed_index)).ok()?;
    set_prop(&options, "wrappedContent", wrapped_content.into()).ok()?;

    let signed =
        call_secure_client_logged(&client, "createFeedUpdateSocWithStamp", options).await?;

    if bool_prop(&signed, "bucketFull") {
        return Some(SecureFeedUpdate {
            bucket_full: true,
            soc_chunk: vec![],
            soc_address: vec![],
            stamp: vec![],
        });
    }

    let soc_chunk = hex_prop(&signed, "socChunkHex");
    let soc_address = hex_prop(&signed, "socAddressHex");
    if soc_chunk.get(..32) != Some(expected_id.as_slice()) {
        log_error(
            "secure vault returned a non-canonical feed identifier",
            &JsValue::from_str(&format!("index {feed_index}")),
        );
        return None;
    }
    if soc_address.len() != 32 || !valid_soc(&soc_chunk, &soc_address) {
        log_error(
            "secure vault returned an invalid feed SOC",
            &JsValue::from_str(&format!("index {feed_index}")),
        );
        return None;
    }

    Some(SecureFeedUpdate {
        bucket_full: false,
        soc_chunk,
        soc_address,
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
    SECURE_CLICK_CONNECT_OPTIONS.with(|cell| cell.replace(Some(options.clone())));
    if let Err(error) = preopen_secure_vault_window(&options) {
        log_error("secure vault user-action preopen failed", &error);
        return;
    }

    if let Some(module) = SECURE_MODULE.with(|cell| cell.borrow().clone()) {
        match start_secure_client_connect(&module, options) {
            Ok(promise) => {
                SECURE_CLICK_CONNECT_PROMISE.with(|cell| cell.replace(Some(promise)));
            }
            Err(error) => {
                log_error("secure vault user-action connect failed", &error);
            }
        }
    }
    focus_current_window();
}

fn active_network_id() -> u64 {
    active_profile().swarm_network_id
}

fn secure_network_matches(network_id: u64) -> bool {
    SECURE_NETWORK_ID.with(|cell| cell.get() == Some(network_id))
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

    let client = initial_secure_client().await?;

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

async fn initial_secure_client() -> Option<Rc<JsValue>> {
    if let Some(promise) = SECURE_CLICK_CONNECT_PROMISE.with(RefCell::take) {
        match JsFuture::from(promise).await {
            Ok(client) => return Some(Rc::new(client)),
            Err(error) => log_error("secure vault click-started connect failed", &error),
        }
        return connect_secure_client_logged(
            take_click_connect_options()?,
            "secure vault click-started reconnect failed",
        )
        .await;
    }

    let (options, context) = match take_click_connect_options() {
        Some(options) => (options, "secure vault click-reserved connect failed"),
        None => match connect_options() {
            Ok(options) => (options, "secure vault connect failed"),
            Err(error) => {
                log_error("secure vault options failed", &error);
                return None;
            }
        },
    };
    connect_secure_client_logged(options, context).await
}

async fn connect_secure_client_logged(options: JsValue, context: &str) -> Option<Rc<JsValue>> {
    match connect_secure_client(options).await {
        Ok(client) => Some(Rc::new(client)),
        Err(error) => {
            log_error(context, &error);
            None
        }
    }
}

async fn reconnect_and_authorize_after_auth_error() -> Option<Rc<JsValue>> {
    clear_secure_connection();
    let options = take_click_connect_options().or_else(|| connect_options().ok())?;
    let client =
        connect_secure_client_logged(options, "secure authorizeTempAuth reconnect failed").await?;
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

async fn secure_client_or_resume(context: &str) -> Option<Rc<JsValue>> {
    if secure_vault_window_closed() {
        mark_secure_resume_required();
    }

    if secure_resume_required() {
        return wait_for_user_resume_connection(context).await.ok();
    }

    match secure_client().await {
        Some(client) => Some(client),
        None if secure_resume_required() => wait_for_user_resume_connection(context).await.ok(),
        None => None,
    }
}

async fn authorize_secure_client(client: Rc<JsValue>) -> Result<Rc<JsValue>, JsValue> {
    let network_id = active_network_id();
    let auth = auth_options_for_network(network_id)?;
    let topics = Array::new();
    topics.push(&JsValue::from_str("*"));
    set_prop(&auth, "allowedTopics", topics.into()).ok();
    set_prop(
        &auth,
        "ttlMs",
        JsValue::from_f64(168.0 * 60.0 * 60.0 * 1000.0),
    )
    .ok();

    let grant = call_client(&client, "authorizeTempAuth", auth).await?;

    let wallet = hex_prop(&grant, "walletAddressHex");
    let grant_network_id = u64_prop(&grant, "networkId");
    if grant_network_id != network_id {
        return Err(JsValue::from_str(&format!(
            "secure vault network changed during authorization: expected {}, got {}",
            network_id, grant_network_id
        )));
    }

    if let Some(expected_wallet) = SECURE_WALLET.with(|cell| cell.borrow().clone())
        && !expected_wallet.is_empty()
        && wallet != expected_wallet
    {
        return Err(JsValue::from_str(&format!(
            "secure vault wallet changed during reconnect: expected 0x{}, got 0x{}",
            hex::encode(expected_wallet),
            hex::encode(wallet)
        )));
    }

    SECURE_CLIENT.with(|cell| cell.replace(Some(client.clone())));
    SECURE_WALLET.with(|cell| cell.replace(Some(wallet)));
    SECURE_NETWORK_ID.with(|cell| cell.set(Some(network_id)));
    clear_secure_resume_required();

    Ok(client)
}

async fn secure_client_for_wallet(wallet: &[u8]) -> Option<Rc<JsValue>> {
    let matches_current =
        SECURE_WALLET.with(|cell| cell.borrow().as_deref().map(|current| current == wallet));

    if matches_current == Some(true) {
        return secure_client().await;
    }

    if matches_current == Some(false) {
        clear_secure_client();
    }

    let client = secure_client_or_resume("checkBatchState").await?;
    let matches_new =
        SECURE_WALLET.with(|cell| cell.borrow().as_deref().map(|current| current == wallet));
    if matches_new == Some(true) {
        Some(client)
    } else {
        log_error(
            "secure vault wallet does not match connected wallet",
            &JsValue::from_str(&format!(
                "expected 0x{}, got {}",
                hex::encode(wallet),
                SECURE_WALLET
                    .with(|cell| cell.borrow().as_deref().map(hex::encode))
                    .map(|wallet| format!("0x{wallet}"))
                    .unwrap_or_else(|| "(none)".to_string())
            )),
        );
        clear_secure_client();
        None
    }
}

fn clear_secure_connection() {
    SECURE_CLIENT.with(RefCell::take);
}

fn mark_secure_resume_required() {
    SECURE_RESUME_REQUIRED.with(|cell| cell.set(true));
    clear_secure_connection();
}

fn clear_secure_resume_required() {
    SECURE_RESUME_REQUIRED.with(|cell| cell.set(false));
}

fn secure_resume_required() -> bool {
    SECURE_RESUME_REQUIRED.with(Cell::get)
}

fn clear_secure_connection_if_current(client: &Rc<JsValue>) {
    SECURE_CLIENT.with(|cell| {
        cell.borrow_mut()
            .take_if(|current| Rc::ptr_eq(current, client))
    });
}

fn clear_secure_client() {
    clear_secure_connection();
    SECURE_WALLET.with(RefCell::take);
    SECURE_NETWORK_ID.with(|cell| cell.set(None));
}

async fn connect_secure_client(options: JsValue) -> Result<JsValue, JsValue> {
    let module = secure_module().await?;
    let promise = start_secure_client_connect(&module, options)?;
    JsFuture::from(promise).await
}

fn take_click_connect_options() -> Option<JsValue> {
    SECURE_CLICK_CONNECT_OPTIONS.with(RefCell::take)
}

fn clear_click_connect_options() {
    SECURE_CLICK_CONNECT_OPTIONS.with(RefCell::take);
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
    SECURE_VAULT_WINDOW.with(|cell| cell.replace(Some(vault_window)));
    Ok(())
}

fn start_secure_client_connect(module: &JsValue, options: JsValue) -> Result<Promise, JsValue> {
    let constructor = Reflect::get(module, &JsValue::from_str("Weeb3SecureVaultClient"))?;
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
    focus_current_window();
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
            wake_resume_waiters();
        });
    });

    let callback = callback.into_js_value();
    button
        .add_event_listener_with_callback("click", callback.unchecked_ref())
        .ok();

    if let Some(result) = result_field(&document) {
        result.prepend_with_node_1(&notice).ok();
        return;
    }

    if let Some(body) = document.body() {
        body.prepend_with_node_1(&notice).ok();
    }
}

fn wake_resume_waiters() {
    SECURE_RESUMED.with(|event| event.notify(usize::MAX));
}

fn result_field(document: &web_sys::Document) -> Option<web_sys::HtmlElement> {
    document
        .get_element_by_id("resultField")
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
}

fn remove_element(element: &web_sys::Element) {
    element.remove();
}

fn focus_current_window() {
    if let Some(window) = web_sys::window() {
        window.focus().ok();
    }
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
            .is_some_and(|vault_window| vault_window.closed().unwrap_or(false))
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
            Ok(value) => return Ok(vault_result(value)),
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
                        None => return Err(error),
                    }
                };
            }
        }
    }

    Err(last_error)
}

async fn call_secure_client_logged(
    client: &Rc<JsValue>,
    method: &str,
    options: JsValue,
) -> Option<JsValue> {
    match call_secure_client(client, method, options).await {
        Ok(value) => Some(value),
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!("secure {method} failed: {error:?}")));
            None
        }
    }
}

async fn wait_for_user_resume_connection(context: &str) -> Result<Rc<JsValue>, JsValue> {
    ensure_resume_connection_prompt(context);
    SECURE_RESUMED.with(Event::listen).await;
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

    SECURE_MODULE.with(|cell| cell.replace(Some(module.clone())));

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
    Reflect::get(value, &key)
        .ok()
        .and_then(present_js_value)
        .or_else(|| {
            value
                .dyn_ref::<Map>()
                .and_then(|map| present_js_value(map.get(&key)))
        })
}

fn present_js_value(value: JsValue) -> Option<JsValue> {
    (!value.is_null() && !value.is_undefined()).then_some(value)
}

fn vault_result(value: JsValue) -> JsValue {
    js_value_field(&value, "result").unwrap_or(value)
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
    number_prop(value, name) as u32
}

fn u64_prop(value: &JsValue, name: &str) -> u64 {
    number_prop(value, name) as u64
}

fn number_prop(value: &JsValue, name: &str) -> f64 {
    js_value_field(value, name)
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_string().and_then(|text| text.parse::<f64>().ok()))
        })
        .unwrap_or(0.0)
}

fn hex_prop(value: &JsValue, name: &str) -> Vec<u8> {
    string_prop(value, name)
        .and_then(|value| hex::decode(strip_hex_prefix(&value)).ok())
        .unwrap_or_default()
}

fn bytes_prop(value: &JsValue, name: &str) -> Vec<u8> {
    bytes_array_prop(value, name)
        .map(|value| value.to_vec())
        .unwrap_or_default()
}

fn bytes_array_prop(value: &JsValue, name: &str) -> Option<Uint8Array> {
    js_value_field(value, name)?.dyn_into().ok()
}

fn exact_u256_prop(value: &JsValue, name: &str) -> Option<U256> {
    let bytes = bytes_prop(value, name);
    (bytes.len() == 32).then(|| U256::from_big_endian(&bytes))
}

fn bytes_value(bytes: &[u8]) -> JsValue {
    bytes_to_js(bytes).into()
}

fn u256_value(value: U256) -> JsValue {
    let mut bytes = [0; 32];
    value.to_big_endian(&mut bytes);
    bytes_value(&bytes)
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
