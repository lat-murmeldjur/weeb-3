use js_sys::{Array, Function, Map, Object, Promise, Reflect, Uint8Array};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

const VAULT_ORIGIN: &str = "https://weeb-3-secure.github.io";
const VAULT_URL: &str = "https://weeb-3-secure.github.io/vault/";
const VAULT_MODULE_URL: &str = "https://weeb-3-secure.github.io/vault/weeb3_secure_vault.js";
const CLIENT_NAME: &str = "official-weeb-3-shell";
const POPUP_NAME: &str = "weeb3-secure-vault";
const SECURE_CALL_ATTEMPTS: usize = 3;

thread_local! {
    static SECURE_MODULE: RefCell<Option<JsValue>> = RefCell::new(None);
    static SECURE_CLIENT: RefCell<Option<Rc<JsValue>>> = RefCell::new(None);
    static SECURE_WALLET: RefCell<Option<Vec<u8>>> = RefCell::new(None);
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

pub async fn secure_batch_state() -> Option<SecureBatchState> {
    let client = secure_client().await?;
    check_batch_state(client).await
}

pub async fn secure_batch_state_for_wallet(wallet: &[u8]) -> Option<SecureBatchState> {
    let client = secure_client_for_wallet(wallet).await?;
    check_batch_state(client).await
}

async fn check_batch_state(client: Rc<JsValue>) -> Option<SecureBatchState> {
    let options = auth_options().ok()?;
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

    web_sys::console::log_1(&JsValue::from(format!(
        "secure batch state: hasBatch={}, batchIdLen={}, bucketLimit={}, validity={}",
        state.has_batch,
        state.batch_id.len(),
        state.batch_bucket_limit,
        state.batch_validity_status
    )));

    Some(state)
}

pub async fn secure_prepare_batch_purchase(
    depth: u8,
    validity_days: u64,
) -> Option<SecurePreparedBatch> {
    let client = secure_client().await?;
    let options = auth_options().ok()?;
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
) -> bool {
    let Some(client) = secure_client().await else {
        return false;
    };
    let Ok(options) = auth_options() else {
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

pub async fn secure_stamp_chunk(chunk_address: Vec<u8>) -> (Vec<u8>, bool) {
    let Some(client) = secure_client().await else {
        return (vec![], false);
    };
    let Ok(options) = auth_options() else {
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
    let Some(client) = secure_client().await else {
        return false;
    };
    let Ok(options) = auth_options() else {
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
    let client = secure_client().await?;
    let options = auth_options().ok()?;
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
    let client = secure_client().await?;
    let options = auth_options().ok()?;
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

async fn secure_client() -> Option<Rc<JsValue>> {
    if let Some(client) = SECURE_CLIENT.with(|cell| cell.borrow().clone()) {
        return Some(client);
    }

    let options = match connect_options() {
        Ok(options) => options,
        Err(e) => {
            log_error("secure vault options failed", &e);
            return None;
        }
    };

    let client = match connect_secure_client(options).await {
        Ok(client) => Rc::new(client),
        Err(e) => {
            log_error("secure vault connect failed", &e);
            return None;
        }
    };

    let auth = match auth_options() {
        Ok(auth) => auth,
        Err(e) => {
            log_error("secure vault auth options failed", &e);
            return None;
        }
    };
    let topics = Array::new();
    topics.push(&JsValue::from_str("*"));
    set_prop(&auth, "allowedTopics", topics.into()).ok();
    set_prop(
        &auth,
        "ttlMs",
        JsValue::from_f64(6.0 * 60.0 * 60.0 * 1000.0),
    )
    .ok();

    let grant = match call_client(&client, "authorizeTempAuth", auth).await {
        Ok(grant) => grant,
        Err(e) => {
            log_error("secure authorizeTempAuth failed", &e);
            return None;
        }
    };

    let wallet = string_prop(&grant, "walletAddressHex")
        .and_then(|value| hex::decode(strip_0x(&value)).ok())
        .unwrap_or_default();

    SECURE_CLIENT.with(|cell| {
        *cell.borrow_mut() = Some(client.clone());
    });
    SECURE_WALLET.with(|cell| {
        *cell.borrow_mut() = Some(wallet);
    });

    Some(client)
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

    let client = secure_client().await?;
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

fn clear_secure_client() {
    SECURE_CLIENT.with(|cell| {
        *cell.borrow_mut() = None;
    });
    SECURE_WALLET.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

async fn connect_secure_client(options: JsValue) -> Result<JsValue, JsValue> {
    let module = secure_module().await?;
    let constructor = Reflect::get(&module, &JsValue::from_str("Weeb3SecureVaultClient"))?;
    let connect =
        Reflect::get(&constructor, &JsValue::from_str("connect"))?.dyn_into::<Function>()?;
    let promise = connect
        .call1(&constructor, &options)?
        .dyn_into::<Promise>()?;
    JsFuture::from(promise).await
}

async fn call_client(client: &JsValue, method: &str, options: JsValue) -> Result<JsValue, JsValue> {
    let method = Reflect::get(client, &JsValue::from_str(method))?.dyn_into::<Function>()?;
    let promise = method.call1(client, &options)?.dyn_into::<Promise>()?;
    JsFuture::from(promise).await
}

async fn call_secure_client(
    client: &Rc<JsValue>,
    method: &str,
    options: JsValue,
) -> Result<JsValue, JsValue> {
    let mut active_client = client.clone();
    let mut last_error = JsValue::from_str("secure vault call failed");

    for attempt in 0..SECURE_CALL_ATTEMPTS {
        match call_client(&active_client, method, options.clone()).await {
            Ok(value) => return Ok(plain_vault_result(value)),
            Err(error) => {
                last_error = error.clone();
                if !reconnectable_error(&error) {
                    return Err(error);
                }

                log_error(
                    &format!("secure {method} reconnect attempt {}", attempt + 1),
                    &error,
                );
                clear_secure_client();
                sleep_ms(250 * (attempt as i32 + 1)).await;
                active_client = secure_client()
                    .await
                    .ok_or_else(|| JsValue::from_str("secure vault reconnect failed"))?;
            }
        }
    }

    Err(last_error)
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
    let origin = current_origin()?;
    let options = Object::new();
    let vault_url = format!(
        "{VAULT_URL}?allow={}",
        js_sys::encode_uri_component(&origin)
    );
    set_prop(&options, "vaultUrl", JsValue::from_str(&vault_url))?;
    set_prop(&options, "targetOrigin", JsValue::from_str(VAULT_ORIGIN))?;
    set_prop(
        &options,
        "clientName",
        JsValue::from_str(&format!("{CLIENT_NAME}:{origin}")),
    )?;
    set_prop(&options, "popupName", JsValue::from_str(POPUP_NAME))?;
    Ok(options.into())
}

fn auth_options() -> Result<JsValue, JsValue> {
    let options = Object::new();
    set_prop(&options, "appId", JsValue::from_str(&current_origin()?))?;
    Ok(options.into())
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
        || text.contains("vault request timed out")
        || text.contains("vault response channel closed")
        || text.contains("vault session was reconnected")
        || text.contains("stale request")
        || text.contains("vault reconnect")
        || text.contains("closed")
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
