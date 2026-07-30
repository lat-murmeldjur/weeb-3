use std::{
    cell::{Cell, RefCell},
    io::Cursor,
    rc::Rc,
    str::FromStr,
    time::Duration,
};

use web3::{
    contract::Options,
    types::{Address, U256},
};

use async_std::sync::Arc;
use js_sys::{Array, Object, Reflect, Uint8Array};
use tar::{Builder, Header};
use wasm_bindgen::{JsCast, JsError, JsValue, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use web_sys::{
    Blob, BlobPropertyBag, Element, Event, HtmlButtonElement, HtmlElement, HtmlInputElement,
    HtmlSelectElement, HtmlSpanElement, MessageChannel, MessageEvent, MessagePort,
    RegistrationOptions, ServiceWorkerRegistration,
};

use crate::{
    Weeb3,
    bzz_stream::{BzzMetadata, bzz_reference_hex, normalize_bzz_path},
    decode_resources, encrey,
    erasure_coding::{upload_redundancy_from_number, upload_redundancy_from_select},
    interface_conventions::{install_interface_conventions, set_bracket_button_label},
    join_all,
    nav::{
        ResourceRoute, clear_hash_route, parse_networked_resource_route, read_routes,
        route_network_mode_from_location,
    },
    network_profile::{
        NetworkMode, is_browser_dialable_underlay, profile_for_mode, profile_for_swarm_network_id,
    },
    on_chain::{
        buy_postage_batch_with_payer, chequebook_balance, chunk_count_for_depth,
        compute_initial_balance_per_chunk, deploy_chequebook_with_payer, deposit_to_chequebook,
        get_batch_validity, last_price, postage_contract, token_contract,
    },
    persistence::{
        get_chequebook_address, get_chequebook_signer_key, set_chequebook_address,
        set_chequebook_signer_key,
    },
    secure_vault::{
        secure_batch_state_for_wallet, secure_commit_batch_purchase_and_verify,
        secure_open_vault_from_user_action, secure_preload_vault_module,
        secure_prepare_batch_purchase,
    },
    stream_conventions::{STREAMING_SERVICE_WORKER_SCOPE, STREAMING_SERVICE_WORKER_URL},
};
use alloy::signers::local::PrivateKeySigner;

#[path = "interface_runtime_conventions.rs"]
mod interface_runtime_conventions;
use interface_runtime_conventions::*;
pub(crate) use interface_runtime_conventions::{
    get_service_worker, service_worker_controls_bzz_requests, service_worker_scope_protocol_error,
};

const BOOTNODE_INPUT_IDS: [&str; 8] = [
    "bootNodeMASettings",
    "bootNodeMASettings0",
    "bootNodeMASettings1",
    "bootNodeMASettings2",
    "bootNodeMASettings3",
    "bootNodeMASettings4",
    "bootNodeMASettings5",
    "bootNodeMASettings6",
];

const INTERFACE_BUILD_VERSION: &str = env!("WEEB3_BUILD_VERSION");

thread_local! {
    static NETWORK_APPLY_GENERATION: Cell<u64> = Cell::new(0);
    static INTERFACE_MOUNT_GENERATION: Cell<u64> = Cell::new(0);
    static SERVICE_WORKER_BRIDGE_CLIENT: RefCell<Option<Arc<Weeb3>>> =
        const { RefCell::new(None) };
    static SERVICE_WORKER_BRIDGE_LISTENER: RefCell<Option<Closure<dyn FnMut(MessageEvent)>>> =
        const { RefCell::new(None) };
}

pub(crate) fn begin_interface_mount() -> u64 {
    INTERFACE_MOUNT_GENERATION.with(|generation| {
        let next = generation.get().wrapping_add(1);
        let next = if next == 0 { 1 } else { next };
        generation.set(next);
        next
    })
}

fn interface_mount_is_current(expected: u64) -> bool {
    INTERFACE_MOUNT_GENERATION.with(|generation| generation.get() == expected)
}

fn interface_mount_can_poll(expected: u64) -> bool {
    if !interface_mount_is_current(expected) {
        return false;
    }

    web_sys::window()
        .and_then(|window| window.document())
        .is_some_and(|document| {
            ["logsField", "ongoing", "connections"]
                .into_iter()
                .all(|id| document.get_element_by_id(id).is_some())
        })
}

fn is_weeb3_service_worker_request_type(request_type: &str) -> bool {
    matches!(
        request_type,
        "WEEB3_CLIENT_PING" | "WEEB3_FETCH_REQUEST" | "UPLOAD_REQUEST"
    )
}

fn service_worker_required_network_id(obj: &Object) -> Option<u64> {
    let value = Reflect::get(obj, &JsValue::from_str("networkId"))
        .ok()?
        .as_f64()?;
    if !value.is_finite() || value.fract() != 0.0 {
        return None;
    }
    let network_id = value as u64;
    matches!(network_id, 1 | 10).then_some(network_id)
}

fn post_service_worker_bridge_error(
    event: &MessageEvent,
    status: u16,
    error: &str,
    network_mismatch: bool,
) {
    let Some(port) = event.ports().get(0).dyn_into::<MessagePort>().ok() else {
        return;
    };
    let response = Object::new();
    let _ = Reflect::set(&response, &"ok".into(), &JsValue::FALSE);
    let _ = Reflect::set(
        &response,
        &"status".into(),
        &JsValue::from_f64(f64::from(status)),
    );
    let _ = Reflect::set(&response, &"error".into(), &JsValue::from_str(error));
    let _ = Reflect::set(
        &response,
        &"networkMismatch".into(),
        &JsValue::from_bool(network_mismatch),
    );
    let _ = port.post_message(&response);
}

fn handle_service_worker_bridge_event(event: MessageEvent) {
    let Ok(obj) = event.data().dyn_into::<Object>() else {
        return;
    };
    let request_type = Reflect::get(&obj, &JsValue::from_str("type"))
        .ok()
        .and_then(|value| value.as_string());
    let Some(request_type) = request_type else {
        return;
    };
    if !is_weeb3_service_worker_request_type(&request_type) {
        return;
    }

    // Prevent another listener from redispatching accounting-sensitive work.
    event.stop_immediate_propagation();

    let bridge_client =
        SERVICE_WORKER_BRIDGE_CLIENT.with(|client| client.borrow().as_ref().cloned());

    if request_type == "WEEB3_CLIENT_PING" {
        let Some(weeb3) = bridge_client else {
            post_service_worker_bridge_error(&event, 503, "weeb-3 runtime is not mounted", false);
            return;
        };
        let response = Object::new();
        let _ = Reflect::set(&response, &"ok".into(), &JsValue::TRUE);
        let _ = Reflect::set(&response, &"type".into(), &"WEEB3_CLIENT_PONG".into());
        let _ = Reflect::set(
            &response,
            &"networkId".into(),
            &JsValue::from_f64(weeb3.service_worker_network_id() as f64),
        );
        if let Some(port) = event.ports().get(0).dyn_into::<MessagePort>().ok() {
            let _ = port.post_message(&response);
        }
        return;
    }

    let Some(weeb3) = bridge_client else {
        post_service_worker_bridge_error(&event, 503, "weeb-3 runtime is not mounted", false);
        return;
    };

    let Some(required_network_id) = service_worker_required_network_id(&obj) else {
        post_service_worker_bridge_error(
            &event,
            400,
            "weeb-3 request is missing a supported networkId",
            false,
        );
        return;
    };
    let active_network_id = weeb3.service_worker_network_id();
    if active_network_id != required_network_id {
        post_service_worker_bridge_error(
            &event,
            409,
            &format!(
                "weeb-3 runtime network mismatch: required {}, active {}",
                required_network_id, active_network_id
            ),
            true,
        );
        return;
    }

    if crate::stream::handle_service_worker_message(&obj, &event, weeb3.clone()) {
        return;
    }

    match request_type.as_str() {
        "UPLOAD_REQUEST" => {
            let file: web_sys::File = Reflect::get(&obj, &"file".into())
                .unwrap()
                .dyn_into()
                .unwrap();
            let encryption = Reflect::get(&obj, &"encryption".into())
                .unwrap_or(JsValue::FALSE)
                .as_bool()
                .unwrap_or(false);
            let redundancy_level = upload_redundancy_from_number(
                Reflect::get(&obj, &"redundancyLevel".into())
                    .ok()
                    .and_then(|value| value.as_f64()),
            )
            .as_u8();
            let index_string = Reflect::get(&obj, &"indexString".into())
                .unwrap_or(JsValue::NULL)
                .as_string()
                .unwrap_or_default();
            let add_to_feed = Reflect::get(&obj, &"addToFeed".into())
                .unwrap_or(JsValue::FALSE)
                .as_bool()
                .unwrap_or(false);
            let feed_topic = Reflect::get(&obj, &"feedTopic".into())
                .unwrap_or(JsValue::NULL)
                .as_string()
                .unwrap_or_default();
            let port = event.ports().get(0).dyn_into::<web_sys::MessagePort>().ok();

            spawn_local(async move {
                let result = weeb3
                    .post_upload_with_redundancy(
                        file,
                        encryption,
                        f64::from(redundancy_level),
                        index_string,
                        add_to_feed,
                        feed_topic,
                    )
                    .await;
                let (data, indx) = decode_resources(result);
                let upload_ok = !indx.is_empty();
                let upload_error = data
                    .first()
                    .and_then(|(bytes, _, _)| String::from_utf8(bytes.clone()).ok())
                    .filter(|message| !message.trim().is_empty())
                    .unwrap_or_else(|| "Upload failed before returning a reference".into());

                let resp = Object::new();
                Reflect::set(&resp, &"ok".into(), &upload_ok.into()).unwrap();
                if upload_ok {
                    Reflect::set(&resp, &"reference".into(), &indx.clone().into()).unwrap();
                } else {
                    Reflect::set(&resp, &"status".into(), &500.into()).unwrap();
                    Reflect::set(&resp, &"error".into(), &upload_error.into()).unwrap();
                }

                if let Some(port) = port {
                    let _ = port.post_message(&resp);
                }

                render_result(data, indx).await;
            });
        }
        _ => {}
    }
}

pub(crate) fn install_service_worker_message_bridge(weeb3: Arc<Weeb3>) {
    SERVICE_WORKER_BRIDGE_CLIENT.with(|client| {
        *client.borrow_mut() = Some(weeb3);
    });

    SERVICE_WORKER_BRIDGE_LISTENER.with(|listener| {
        if listener.borrow().is_some() {
            return;
        }

        let closure = Closure::<dyn FnMut(MessageEvent)>::new(handle_service_worker_bridge_event);
        let service_worker = web_sys::window().unwrap().navigator().service_worker();
        if service_worker
            .add_event_listener_with_callback("message", closure.as_ref().unchecked_ref())
            .is_err()
        {
            return;
        }

        *listener.borrow_mut() = Some(closure);
    });
}

#[wasm_bindgen]
pub async fn interweeb(_st: String) -> Result<(), JsError> {
    clear_hash_route();
    let initial_mode = route_network_mode_from_location().unwrap_or(NetworkMode::Mainnet);
    let initial_profile = profile_for_mode(initial_mode);
    set_network_profile_inputs(initial_mode);

    let weeb3 = Arc::new(Weeb3::new("".to_string()));
    let _ = weeb3
        .set_network_id(initial_profile.swarm_network_id.to_string())
        .await;
    weeb3.interface_log(format!(
        "Node created for {:?} network {}",
        initial_profile.mode, initial_profile.swarm_network_id
    ));
    mount_interface(weeb3, true, true).await
}

pub(crate) async fn mount_interface(
    weeb3: Arc<Weeb3>,
    start_runtime: bool,
    read_initial_routes: bool,
) -> Result<(), JsError> {
    let mount_generation = begin_interface_mount();
    install_service_worker_message_bridge(weeb3.clone());
    mount_interface_after_service_worker_bridge_install(
        weeb3,
        start_runtime,
        read_initial_routes,
        None,
        true,
        mount_generation,
    )
    .await
}

pub(crate) async fn mount_interface_after_service_worker_bridge_install(
    weeb3: Arc<Weeb3>,
    start_runtime: bool,
    read_initial_routes: bool,
    initial_result_generation: Option<u64>,
    schedule_initial_connections: bool,
    mount_generation: u64,
) -> Result<(), JsError> {
    if !interface_mount_is_current(mount_generation) {
        return Ok(());
    }

    if start_runtime {
        let weeb30 = weeb3.clone();
        weeb3.interface_log("Node runtime starting".to_string());
        spawn_local(async move {
            weeb30.interface_log("Node runtime booting".to_string());
            weeb30.run("".to_string()).await;
        });
    }

    async_std::task::yield_now().await;
    if !interface_mount_is_current(mount_generation) {
        return Ok(());
    }

    secure_preload_vault_module();
    install_interface_conventions();
    if let Some(profile) = profile_for_swarm_network_id(weeb3.get_network_id().await) {
        if !interface_mount_is_current(mount_generation) {
            return Ok(());
        }
        set_network_profile_inputs(profile.mode);
    }
    if !interface_mount_is_current(mount_generation) {
        return Ok(());
    }
    weeb3.interface_log(format!(
        "Interface mounted, version {}",
        INTERFACE_BUILD_VERSION
    ));

    let weeb31 = weeb3.clone();
    let weeb32 = weeb3.clone();
    let weeb33 = weeb3.clone();
    let weeb34 = weeb3.clone();
    let weeb35 = weeb3.clone();
    let weeb36 = weeb3.clone();
    let weeb39 = weeb3.clone();
    let weeb40 = weeb3.clone();
    let weeb41 = weeb3.clone();

    if schedule_initial_connections {
        let initial_network_apply_generation = next_network_apply_generation();
        spawn_local(async move {
            connect_all_bootnode_settings(weeb39, initial_network_apply_generation).await;
        });
    }

    spawn_local(async {
        let _ = get_service_worker().await;
    });

    let chequebook_state = Rc::new(RefCell::new(None::<Address>));

    let chequebook_state_init = chequebook_state.clone();
    spawn_local(async move {
        let stored_chequebook_signer_key = get_chequebook_signer_key().await;
        let stored_chequebook_address = get_chequebook_address().await;

        if !stored_chequebook_signer_key.is_empty() && stored_chequebook_address.len() == 20 {
            if let Ok(address) = Address::from_str(&hex::encode(stored_chequebook_address)) {
                *chequebook_state_init.borrow_mut() = Some(address);
            }
        }
    });

    let interface_async = async move {
        let chequebook_state_deploy = chequebook_state.clone();
        let chequebook_state_deposit = chequebook_state.clone();
        let mut retained_message_callbacks =
            Vec::<Closure<dyn FnMut(web_sys::MessageEvent)>>::new();

        let callback =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                let weeb300 = weeb31.clone();
                let document = web_sys::window().unwrap().document().unwrap();

                let input_field = document
                    .get_element_by_id("inputString")
                    .expect("#inputString should exist");
                let input_field = input_field
                    .dyn_ref::<HtmlInputElement>()
                    .expect("#inputString should be a HtmlInputElement");

                match input_field.value().parse::<String>() {
                    Ok(text) => spawn_local(async move {
                        open_resource_input(weeb300, text).await;
                    }),
                    Err(_) => {
                        document
                            .get_element_by_id("resultField")
                            .expect("#resultField should exist")
                            .dyn_ref::<HtmlElement>()
                            .expect("#resultField should be a HtmlElement")
                            .set_inner_text("insxyk");
                    }
                }
            });

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("inputString")
            .expect("#inputString should exist")
            .dyn_ref::<HtmlInputElement>()
            .expect("#inputString should be a HtmlInputElement")
            .set_oninput(Some(callback.as_ref().unchecked_ref()));

        update_transfer_pause_button(weeb40.transfer_paused());

        let callback_pause =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                let weeb300 = weeb40.clone();

                spawn_local(async move {
                    let paused = weeb300.toggle_transfer_pause().await;
                    update_transfer_pause_button(paused);
                });
            });

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("transferPauseToggle")
            .expect("#transferPauseToggle should exist")
            .dyn_ref::<HtmlButtonElement>()
            .expect("#transferPauseToggle should be a HtmlButtonElement")
            .set_onclick(Some(callback_pause.as_ref().unchecked_ref()));

        let callback2 = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |_msg| {
                let document = web_sys::window().unwrap().document().unwrap();

                let validity_el = document
                    .get_element_by_id("batchValidityDays")
                    .expect("#batchValidityDays should exist");
                let validity_input: HtmlInputElement = validity_el
                    .dyn_into::<HtmlInputElement>()
                    .expect("#batchValidityDays should be a HtmlInputElement");

                let validity = match validity_input.value().parse::<u64>() {
                    Ok(v) => v,
                    _ => {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Failed to read batch validity");
                        return;
                    }
                };

                let size_el = document
                    .get_element_by_id("batchSize")
                    .expect("#batchSize should exist");
                let size_input: HtmlSelectElement = size_el
                    .dyn_into::<HtmlSelectElement>()
                    .expect("#batchSize should be a HtmlSelectElement");

                let batch_depth = match size_input.value().parse::<u8>() {
                    Ok(size0) => 17 + size0,
                    _ => {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Failed to read batch size");
                        return;
                    }
                };

                spawn_local(async move {
                    let payer = match connect_wallet_address().await {
                        Ok(payer) => payer,
                        Err(error) => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd
                                .alert_with_message(&format!("Wallet connect failed: {}", error));
                            return;
                        }
                    };

                    let profile = current_network_profile();

                    let secure_state = match secure_batch_state_for_wallet(
                        payer.as_bytes(),
                        profile.swarm_network_id,
                    )
                    .await
                    {
                        Some(state) => state,
                        None => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd.alert_with_message(
                                "Could not check weeb-3-secure for the connected wallet",
                            );
                            return;
                        }
                    };

                    if secure_state.usable() {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Already have a secure batch for uploads");
                        return;
                    }

                    if let Ok(w3) = crate::on_chain::web3() {
                        if let Ok(cid) = w3.eth().chain_id().await {
                            if cid != U256::from(profile.wallet_chain_id) {
                                let wnd = web_sys::window().unwrap();
                                let _ = wnd.alert_with_message(&format!(
                                    "Wallet is not on {:?} chain ({}). Please switch in your wallet and try again.",
                                    profile.mode, profile.wallet_chain_id
                                ));
                                return;
                            }
                        }
                    }

                    let prepared = match secure_prepare_batch_purchase(
                        batch_depth,
                        validity,
                        profile.swarm_network_id,
                    )
                    .await
                    {
                        Some(prepared) if prepared.owner.len() == 20 => prepared,
                        _ => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd.alert_with_message("Failed to prepare secure batch owner");
                            return;
                        }
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
                        Ok(p) => p,
                        Err(e) => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd.alert_with_message(&format!(
                                "Batch purchase failed: {:?}. Ensure wallet is on {:?} and has {} + {}.",
                                e, profile.mode, profile.bzz_symbol, profile.base_symbol
                            ));
                            return;
                        }
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
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd
                            .alert_with_message("Failed to save or verify batch in weeb-3-secure");
                        return;
                    }

                    let wnd = web_sys::window().unwrap();
                    let _ = wnd.alert_with_message(&format!(
                        "Storage batch ready.\nBatch ID: 0x{}\nDepth: {}\nStorage slots per bucket: {}",
                        hex::encode(&purchase.batch_id),
                        prepared.depth,
                        purchase.bucket_limit
                    ));
                });
            },
        );

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("uploadGetBatch")
            .expect("#uploadGetBatch should exist")
            .dyn_ref::<HtmlButtonElement>()
            .expect("#uploadGetBatch should be a HtmlButtonElement")
            .set_onclick(Some(callback2.as_ref().unchecked_ref()));

        if let Some(button) = web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("uploadPrereqCheck")
            .and_then(|button| button.dyn_into::<HtmlButtonElement>().ok())
        {
            let callback_prereq =
                wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new({
                    let weeb41 = weeb41.clone();
                    move |_msg| {
                        let weeb300 = weeb41.clone();
                        spawn_local(async move {
                            check_upload_prerequisites(weeb300).await;
                        });
                    }
                });

            button.set_onclick(Some(callback_prereq.as_ref().unchecked_ref()));
            retained_message_callbacks.push(callback_prereq);
        }

        let callback3 =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                let weeb300 = weeb32.clone();

                let document = web_sys::window().unwrap().document().unwrap();

                let file_input = document
                    .get_element_by_id("uploadFileSelect")
                    .expect("#uploadFileSelect should exist");

                let file_input = file_input
                    .dyn_ref::<HtmlInputElement>()
                    .expect("#uploadFileSelect should be a HtmlInputElement");

                let file0 = match file_input.files() {
                    Some(aok) => aok,
                    _ => return,
                };

                let file = match file0.item(0) {
                    Some(aok) => aok,
                    _ => return,
                };
                secure_open_vault_from_user_action();
                spawn_local(async move {
                    let file_enc = document
                        .get_element_by_id("uploadFileEncrypt")
                        .expect("#uploadFileEncrypt should exist");

                    let file_enc = file_enc
                        .dyn_ref::<HtmlInputElement>()
                        .expect("#uploadFileEncrypt should be a HtmlInputElement");

                    let upload_to_feed = document
                        .get_element_by_id("uploadAddToFeed")
                        .expect("#uploadAddToFeed should exist");

                    let upload_to_feed = upload_to_feed
                        .dyn_ref::<HtmlInputElement>()
                        .expect("#uploadAddToFeed should be a HtmlInputElement");

                    let mut feed_topic = "".to_string();

                    if upload_to_feed.checked() {
                        let topic_field = document
                            .get_element_by_id("feedTopicString")
                            .expect("#feedTopicString should exist");
                        let topic_field = topic_field
                            .dyn_ref::<HtmlInputElement>()
                            .expect("#feedTopicString should be a HtmlInputElement");

                        match topic_field.value().parse::<String>() {
                            Ok(text) => {
                                feed_topic = text;
                            }
                            Err(_) => {}
                        }
                    }

                    let index_input = document
                        .get_element_by_id("indexString")
                        .expect("#indexString should exist");

                    let index_input = index_input
                        .dyn_ref::<HtmlInputElement>()
                        .expect("#indexString should be a HtmlInputElement");

                    let index_string = match index_input.value().parse::<String>() {
                        Ok(text) => text,
                        Err(_) => "".to_string(),
                    };

                    let redundancy_value = document
                        .get_element_by_id("uploadRedundancyLevel")
                        .and_then(|element| element.dyn_into::<HtmlSelectElement>().ok())
                        .map(|select| select.value());
                    let redundancy_level =
                        upload_redundancy_from_select(redundancy_value.as_deref()).as_u8();

                    let result = weeb300
                        .post_upload_with_redundancy(
                            file,
                            file_enc.checked() && !upload_to_feed.checked(),
                            f64::from(redundancy_level),
                            index_string,
                            upload_to_feed.checked(),
                            feed_topic,
                        )
                        .await;

                    let (data, indx) = decode_resources(result);

                    render_result(data, indx).await;
                })
            });

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("uploadFile")
            .expect("#uploadFile should exist")
            .dyn_ref::<HtmlButtonElement>()
            .expect("#uploadFile should be a HtmlButtonElement")
            .set_onclick(Some(callback3.as_ref().unchecked_ref()));

        let weeb_network_toggle = weeb33.clone();
        let callback4 =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                let weeb300 = weeb33.clone();

                let apply_generation = next_network_apply_generation();
                let network_id = current_network_id_input();
                spawn_local(async move {
                    apply_network_settings_and_connect(weeb300, apply_generation, network_id).await;
                });
            });

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("networkSet")
            .expect("#networkSet should exist")
            .dyn_ref::<HtmlButtonElement>()
            .expect("#networkSet should be a HtmlButtonElement")
            .set_onclick(Some(callback4.as_ref().unchecked_ref()));

        install_network_profile_toggle(weeb_network_toggle);

        let callback5 =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                let weeb300 = weeb34.clone();

                let window = web_sys::window().unwrap();

                if window
                .confirm_with_message(
                    "This will enable overwriting previously uploaded content with new content.",
                )
                .unwrap_or(false)
            {
                spawn_local(async move {
                    let result = weeb300.reset_stamp().await;

                    let (data, indx) = decode_resources(result);

                    render_result(data, indx).await;
                })
            }
            });

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("uploadResetStamp")
            .expect("#uploadResetStamp should exist")
            .dyn_ref::<HtmlButtonElement>()
            .expect("#uploadResetStamp should be a HtmlButtonElement")
            .set_onclick(Some(callback5.as_ref().unchecked_ref()));

        let callback6 = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |_msg| {
                let state = chequebook_state_deploy.clone();
                spawn_local(async move {
                    let payer = match connect_wallet_address().await {
                        Ok(payer) => payer,
                        Err(error) => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd
                                .alert_with_message(&format!("Wallet connect failed: {}", error));
                            return;
                        }
                    };

                    let stored_chequebook_signer_key = get_chequebook_signer_key().await;
                    let stored_chequebook_address = get_chequebook_address().await;

                    if !stored_chequebook_signer_key.is_empty()
                        && stored_chequebook_address.len() == 20
                    {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message(&format!(
                            "Already have a chequebook deployed at address {}",
                            hex::encode(stored_chequebook_address)
                        ));
                        return;
                    }

                    let profile = current_network_profile();

                    if let Ok(w3) = crate::on_chain::web3() {
                        if let Ok(cid) = w3.eth().chain_id().await {
                            if cid != U256::from(profile.wallet_chain_id) {
                                let wnd = web_sys::window().unwrap();
                                let _ = wnd.alert_with_message(&format!(
                                    "Wallet is not on {:?} chain ({}). Please switch in your wallet and try again.",
                                    profile.mode, profile.wallet_chain_id
                                ));
                                return;
                            }
                        }
                    }

                    let cheque_signer_key = encrey();
                    let cheque_signer = match PrivateKeySigner::from_slice(&cheque_signer_key) {
                        Ok(s) => s,
                        Err(_) => {
                            let wnd = web_sys::window().unwrap();
                            let _ =
                                wnd.alert_with_message("Failed to create chequebook signer key");
                            return;
                        }
                    };
                    let issuer_h160_bytes: [u8; 20] = *cheque_signer.address().as_ref();
                    let issuer = Address::from(issuer_h160_bytes);

                    let deployment = match deploy_chequebook_with_payer(issuer, payer).await {
                        Ok(d) => d,
                        Err(e) => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd.alert_with_message(&format!(
                                "Chequebook deployment failed: {:?}",
                                e
                            ));
                            return;
                        }
                    };

                    if !set_chequebook_signer_key(&cheque_signer_key).await {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message(
                            "Chequebook deployed, but failed to save signer key locally.",
                        );
                    }

                    if !set_chequebook_address(&deployment.chequebook.as_bytes().to_vec()).await {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message(
                            "Chequebook deployed, but failed to save address locally.",
                        );
                    }

                    *state.borrow_mut() = Some(deployment.chequebook);

                    let wnd = web_sys::window().unwrap();
                    let _ = wnd.alert_with_message(&format!(
                        "Chequebook deployed at 0x{}.\nIssuer: 0x{}\nDeployment tx: 0x{}",
                        hex::encode(deployment.chequebook.as_bytes()),
                        hex::encode(issuer_h160_bytes),
                        hex::encode(deployment.tx.as_bytes())
                    ));
                });
            },
        );

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("deployChequebook")
            .expect("#deployChequebook should exist")
            .dyn_ref::<HtmlButtonElement>()
            .expect("#deployChequebook should be a HtmlButtonElement")
            .set_onclick(Some(callback6.as_ref().unchecked_ref()));

        let callback7 = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |_msg| {
                let state = chequebook_state_deposit.clone();

                let document = web_sys::window().unwrap().document().unwrap();
                let amount_el = document
                    .get_element_by_id("depositAmount")
                    .expect("#depositAmount should exist");
                let amount_input: HtmlInputElement = amount_el
                    .dyn_into::<HtmlInputElement>()
                    .expect("#depositAmount should be a HtmlInputElement");

                let amount_raw = amount_input.value();
                let amount = match U256::from_dec_str(amount_raw.trim()) {
                    Ok(v) => v,
                    Err(_) => {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Failed to read deposit amount");
                        return;
                    }
                };

                if amount == U256::from(0u8) {
                    let wnd = web_sys::window().unwrap();
                    let _ = wnd.alert_with_message("Deposit amount must be greater than zero");
                    return;
                }

                let chequebook = *state.borrow();

                spawn_local(async move {
                    let chequebook = match chequebook {
                        Some(addr) => addr,
                        None => {
                            let stored_chequebook_signer_key = get_chequebook_signer_key().await;
                            let stored_chequebook_address = get_chequebook_address().await;

                            if !stored_chequebook_signer_key.is_empty()
                                && stored_chequebook_address.len() == 20
                            {
                                match Address::from_str(&hex::encode(stored_chequebook_address)) {
                                    Ok(addr) => {
                                        *state.borrow_mut() = Some(addr);
                                        addr
                                    }
                                    Err(_) => {
                                        let wnd = web_sys::window().unwrap();
                                        let _ = wnd.alert_with_message(
                                            "Stored chequebook address is invalid.",
                                        );
                                        return;
                                    }
                                }
                            } else {
                                let wnd = web_sys::window().unwrap();
                                let _ = wnd.alert_with_message(
                                    "Deploy a chequebook first before depositing.",
                                );
                                return;
                            }
                        }
                    };

                    let payer = match connect_wallet_address().await {
                        Ok(payer) => payer,
                        Err(error) => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd
                                .alert_with_message(&format!("Wallet connect failed: {}", error));
                            return;
                        }
                    };

                    let w3 = match crate::on_chain::web3() {
                        Ok(w) => w,
                        Err(e) => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd
                                .alert_with_message(&format!("Failed to initialize web3: {:?}", e));
                            return;
                        }
                    };

                    let profile = current_network_profile();

                    if let Ok(cid) = w3.eth().chain_id().await {
                        if cid != U256::from(profile.wallet_chain_id) {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd.alert_with_message(&format!(
                                "Wallet is not on {:?} chain ({}). Please switch in your wallet and try again.",
                                profile.mode, profile.wallet_chain_id
                            ));
                            return;
                        }
                    }

                    let token = match token_contract(&w3).await {
                        Ok(t) => t,
                        Err(e) => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd.alert_with_message(&format!(
                                "Failed to load token contract: {:?}",
                                e
                            ));
                            return;
                        }
                    };

                    let receipt =
                        match deposit_to_chequebook(&token, chequebook, payer, amount).await {
                            Ok(r) => r,
                            Err(e) => {
                                let wnd = web_sys::window().unwrap();
                                let _ = wnd.alert_with_message(&format!("Deposit failed: {:?}", e));
                                return;
                            }
                        };

                    let mut balance_note = String::new();
                    if let Ok(balance) = chequebook_balance(&w3, chequebook).await {
                        balance_note = format!("\nNew balance: {}", balance);
                    }

                    let wnd = web_sys::window().unwrap();
                    let _ = wnd.alert_with_message(&format!(
                        "Deposit submitted.\nTx: 0x{}{}",
                        hex::encode(receipt.transaction_hash.as_bytes()),
                        balance_note
                    ));
                });
            },
        );

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("depositCash")
            .expect("#depositCash should exist")
            .dyn_ref::<HtmlButtonElement>()
            .expect("#depositCash should be a HtmlButtonElement")
            .set_onclick(Some(callback7.as_ref().unchecked_ref()));

        if read_initial_routes {
            spawn_local(async move {
                if initial_result_generation.is_some_and(|generation| {
                    !crate::stream::result_view_request_is_current(generation)
                }) {
                    return;
                }
                let routes = read_routes().await;
                let mut handles = vec![];
                for route in routes {
                    let handle = async {
                        if initial_result_generation.is_some_and(|generation| {
                            !crate::stream::result_view_request_is_current(generation)
                        }) {
                            return;
                        }
                        let weeb300 = weeb36.clone();
                        open_resource(weeb300, route).await;
                    };
                    handles.push(handle);
                }
                let _ = join_all(handles).await;
            });
        }

        let mut last_progress_revision = 0u64;
        let mut last_ongoing = None::<u64>;
        let mut last_connections = None::<u64>;
        loop {
            let _retained_callback_count = retained_message_callbacks.len();
            if !interface_mount_can_poll(mount_generation) {
                break;
            }

            #[allow(irrefutable_let_patterns)]
            let logs_current = weeb35.get_current_logs().await;
            if !interface_mount_can_poll(mount_generation) {
                break;
            }
            for log_message in logs_current.iter() {
                render_log_message(&log_message);
            }

            let ongoing = weeb35.get_ongoing_connections().await;
            if !interface_mount_can_poll(mount_generation) {
                break;
            }

            if last_ongoing != Some(ongoing) {
                let Some(ongoing_element) = web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.get_element_by_id("ongoing"))
                    .and_then(|element| element.dyn_into::<HtmlSpanElement>().ok())
                else {
                    break;
                };
                ongoing_element.set_text_content(Some(&ongoing.to_string()));
                last_ongoing = Some(ongoing);
            }

            let connections = weeb35.get_connections().await;
            if !interface_mount_can_poll(mount_generation) {
                break;
            }

            if last_connections != Some(connections) {
                let Some(connections_element) = web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.get_element_by_id("connections"))
                    .and_then(|element| element.dyn_into::<HtmlSpanElement>().ok())
                else {
                    break;
                };
                connections_element.set_text_content(Some(&connections.to_string()));
                last_connections = Some(connections);
            }

            let progress_snapshot = weeb35.get_progress_snapshot(last_progress_revision).await;
            if !interface_mount_can_poll(mount_generation) {
                break;
            }
            if let Some((revision, progress_rows)) = progress_snapshot {
                render_progress_rows(progress_rows);
                last_progress_revision = revision;
            }

            async_std::task::sleep(Duration::from_millis(160)).await
        }
    };

    interface_async.await;

    Ok(())
}
