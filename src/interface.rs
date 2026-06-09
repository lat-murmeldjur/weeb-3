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
use js_sys::{Array, Uint8Array};
use tar::{Builder, Header};
use wasm_bindgen::{JsCast, JsError, JsValue, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use web_sys::{
    Blob,
    BlobPropertyBag,
    Element,
    Event,
    HtmlButtonElement,
    HtmlElement,
    HtmlInputElement,
    HtmlSelectElement,
    HtmlSpanElement,
    MessageEvent,
    // Response,
    ServiceWorkerRegistration,
};

use crate::{
    Weeb3,
    bzz_stream::{BzzMetadata, bzz_reference_hex, normalize_bzz_path},
    decode_resources, encrey,
    interface_conventions::{install_interface_conventions, set_bracket_button_label},
    join_all,
    nav::{
        ResourceRoute, clear_path, parse_resource_route, read_routes,
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
        secure_batch_state_for_wallet, secure_commit_batch_purchase,
        secure_open_vault_from_user_action, secure_preload_vault_module,
        secure_prepare_batch_purchase,
    },
};
use alloy::signers::local::PrivateKeySigner;

#[path = "interface_runtime_conventions.rs"]
mod interface_runtime_conventions;
use interface_runtime_conventions::*;
pub(crate) use interface_runtime_conventions::{
    get_service_worker, service_worker_controls_bzz_requests,
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

const DEBUG_INTERFACE_LOGS: bool = false;
const INTERFACE_BUILD_VERSION: &str = env!("WEEB3_BUILD_VERSION");

thread_local! {
    static NETWORK_APPLY_GENERATION: Cell<u64> = Cell::new(0);
}

fn interface_debug(value: &JsValue) {
    if DEBUG_INTERFACE_LOGS {
        web_sys::console::log_1(value);
    }
}

#[wasm_bindgen]
pub async fn interweeb(_st: String) -> Result<(), JsError> {
    //    init_panic_hook();

    clear_path().await;
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
    if start_runtime {
        let weeb30 = weeb3.clone();
        weeb3.interface_log("Node runtime starting".to_string());
        spawn_local(async move {
            weeb30.interface_log("Node runtime booting".to_string());
            weeb30.run("".to_string()).await;
        });
    }

    async_std::task::yield_now().await;

    secure_preload_vault_module();
    install_interface_conventions();
    if let Some(profile) = profile_for_swarm_network_id(weeb3.get_network_id().await) {
        set_network_profile_inputs(profile.mode);
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
    let weeb37 = weeb3.clone();
    let weeb38 = weeb3.clone();
    let weeb39 = weeb3.clone();
    let weeb40 = weeb3.clone();
    let weeb41 = weeb3.clone();

    let initial_network_apply_generation = next_network_apply_generation();
    spawn_local(async move {
        connect_all_bootnode_settings(weeb39, initial_network_apply_generation).await;
    });

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

    let window = web_sys::window().unwrap();

    let host2 = window
        .document()
        .unwrap()
        .location()
        .unwrap()
        .origin()
        .unwrap();

    let interface_async = async move {
        interface_debug(&JsValue::from(format!("host2 {:#?}", host2)));

        let chequebook_state_deploy = chequebook_state.clone();
        let chequebook_state_deposit = chequebook_state.clone();

        // let document = web_sys::window().unwrap().document().unwrap();

        let callback =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                interface_debug(&"oninput callback triggered".into());
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
                        interface_debug(&"oninput callback string".into());

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
                interface_debug(&"transferPauseToggle callback triggered".into());
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
                interface_debug(&"uploadGetBatch callback triggered".into());

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

                interface_debug(&JsValue::from(format!(
                    "Selected batch depth: {}",
                    batch_depth
                )));

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

                    interface_debug(&JsValue::from(format!(
                        "Secure batch owner 0x{} | payer 0x{}",
                        hex::encode(&prepared.owner),
                        hex::encode(payer.as_bytes())
                    )));

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

                    if !secure_commit_batch_purchase(
                        &purchase.batch_id,
                        purchase.bucket_limit,
                        prepared.depth,
                        profile.swarm_network_id,
                    )
                    .await
                    {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Failed to save batch in weeb-3-secure");
                        return;
                    }

                    interface_debug(&JsValue::from(format!(
                        "Approve tx 0x{}, Create tx 0x{}, Batch id 0x{}, depth {}, validity {}d, lastPrice {}",
                        hex::encode(purchase.approve_tx.as_bytes()),
                        hex::encode(purchase.create_tx.as_bytes()),
                        hex::encode(&purchase.batch_id),
                        prepared.depth,
                        prepared.validity_days,
                        purchase.last_price,
                    )));

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
            callback_prereq.forget();
        }

        let callback3 =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                interface_debug(&"oninput file callback".into());

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

                    interface_debug(&JsValue::from(format!(
                        "selected file length {:#?}",
                        file.size()
                    )));

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

                    interface_debug(&JsValue::from(format!("IF Upload Marker 0")));

                    let result = weeb300
                        .post_upload(
                            file,
                            file_enc.checked() && !upload_to_feed.checked(),
                            index_string,
                            upload_to_feed.checked(),
                            feed_topic,
                        )
                        .await;

                    interface_debug(&JsValue::from(format!("IF Upload Marker 1")));

                    let (data, indx) = decode_resources(result);

                    render_result(data, indx).await;

                    interface_debug(&"oninput file callback happened".into());
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

                interface_debug(&"oninput bootnode callback".into());

                let apply_generation = next_network_apply_generation();
                let network_id = current_network_id_input();
                spawn_local(async move {
                    apply_network_settings_and_connect(weeb300, apply_generation, network_id).await;
                });

                interface_debug(&"oninput network settings callback happened".into());
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

                interface_debug(&"oninput reset stamp callback".into());

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

        let service_closure = Closure::wrap(Box::new(move |event: MessageEvent| {
            if let Ok(obj) = event.data().dyn_into::<js_sys::Object>() {
                interface_debug(&JsValue::from(format!(
                    "Attempting to load reference received from service worker {:#?}",
                    obj
                )));
                let ty =
                    js_sys::Reflect::get(&obj, &JsValue::from_str("type")).unwrap_or(JsValue::NULL);

                if crate::streaming_player::handle_service_worker_message(
                    &obj,
                    &event,
                    weeb37.clone(),
                ) {
                    return;
                }

                if ty == JsValue::from_str("RETRIEVE_REQUEST") {
                    let url = js_sys::Reflect::get(&obj, &JsValue::from_str("url"))
                        .unwrap_or(JsValue::NULL);
                    let reference = url.as_string().unwrap_or_default();
                    let weeb300 = weeb37.clone();

                    let ports: Array = event.ports().into();
                    let port = ports.get(0).dyn_into::<web_sys::MessagePort>().ok();

                    wasm_bindgen_futures::spawn_local(async move {
                        interface_debug(&JsValue::from(format!(
                            "Loading /bzz/ reference from service worker {:#?}",
                            reference
                        )));
                        let result = weeb300.acquire(reference).await;
                        let (data, indx) = decode_resources(result);
                        let head_resource = data
                            .iter()
                            .find(|(_, _, path)| *path == indx)
                            .or_else(|| data.get(0));

                        let resp = js_sys::Object::new();
                        js_sys::Reflect::set(&resp, &"ok".into(), &head_resource.is_some().into())
                            .unwrap();
                        js_sys::Reflect::set(&resp, &"type".into(), &"RETRIEVE_RESPONSE".into())
                            .unwrap();
                        js_sys::Reflect::set(&resp, &"indx".into(), &indx.clone().into()).unwrap();

                        if let Some((bytes, mime, path)) = head_resource {
                            interface_debug(&JsValue::from(format!(
                                "service message resource len {}",
                                bytes.len()
                            )));

                            let u8arr = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
                            u8arr.copy_from(&bytes);
                            js_sys::Reflect::set(&resp, &"body".into(), &u8arr).unwrap();

                            js_sys::Reflect::set(&resp, &"mime".into(), &mime.clone().into())
                                .unwrap();
                            js_sys::Reflect::set(&resp, &"path".into(), &path.clone().into())
                                .unwrap();
                        }

                        if let Some(port) = port {
                            port.post_message(&resp).unwrap();
                        }
                    });
                }
                if ty == JsValue::from_str("RETRIEVE_BYTES_REQUEST")
                    || ty == JsValue::from_str("RETRIEVE_CHUNK_REQUEST")
                {
                    let url = js_sys::Reflect::get(&obj, &JsValue::from_str("url"))
                        .unwrap_or(JsValue::NULL);
                    let reference = url.as_string().unwrap_or_default();
                    let retrieve_chunk = ty == JsValue::from_str("RETRIEVE_CHUNK_REQUEST");
                    let weeb300 = weeb37.clone();

                    let ports: Array = event.ports().into();
                    let port = ports.get(0).dyn_into::<web_sys::MessagePort>().ok();

                    wasm_bindgen_futures::spawn_local(async move {
                        let bytes = if retrieve_chunk {
                            weeb300.retrieve_chunk_bytes(reference.clone()).await
                        } else {
                            weeb300.retrieve_bytes(reference.clone()).await
                        };

                        let resp = js_sys::Object::new();
                        js_sys::Reflect::set(&resp, &"ok".into(), &(!bytes.is_empty()).into())
                            .unwrap();
                        js_sys::Reflect::set(
                            &resp,
                            &"type".into(),
                            &if retrieve_chunk {
                                "RETRIEVE_CHUNK_RESPONSE"
                            } else {
                                "RETRIEVE_BYTES_RESPONSE"
                            }
                            .into(),
                        )
                        .unwrap();
                        js_sys::Reflect::set(&resp, &"path".into(), &reference.clone().into())
                            .unwrap();
                        js_sys::Reflect::set(
                            &resp,
                            &"mime".into(),
                            &"application/octet-stream".into(),
                        )
                        .unwrap();

                        let u8arr = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
                        u8arr.copy_from(&bytes);
                        js_sys::Reflect::set(&resp, &"body".into(), &u8arr).unwrap();

                        if let Some(port) = port {
                            port.post_message(&resp).unwrap();
                        }
                    });
                }
                if ty == JsValue::from_str("UPLOAD_REQUEST") {
                    let file: web_sys::File = js_sys::Reflect::get(&obj, &"file".into())
                        .unwrap()
                        .dyn_into()
                        .unwrap();

                    let encryption = js_sys::Reflect::get(&obj, &"encryption".into())
                        .unwrap_or(JsValue::FALSE)
                        .as_bool()
                        .unwrap_or(false);

                    let index_string = js_sys::Reflect::get(&obj, &"indexString".into())
                        .unwrap_or(JsValue::NULL)
                        .as_string()
                        .unwrap_or_default();

                    let add_to_feed = js_sys::Reflect::get(&obj, &"addToFeed".into())
                        .unwrap_or(JsValue::FALSE)
                        .as_bool()
                        .unwrap_or(false);

                    let feed_topic = js_sys::Reflect::get(&obj, &"feedTopic".into())
                        .unwrap_or(JsValue::NULL)
                        .as_string()
                        .unwrap_or_default();

                    let weeb300 = weeb38.clone();
                    let port = event.ports().get(0).dyn_into::<web_sys::MessagePort>().ok();

                    wasm_bindgen_futures::spawn_local(async move {
                        let result = weeb300
                            .post_upload(file, encryption, index_string, add_to_feed, feed_topic)
                            .await;

                        let (data, indx) = decode_resources(result);

                        // send back reference/hash
                        let resp = js_sys::Object::new();
                        js_sys::Reflect::set(&resp, &"ok".into(), &true.into()).unwrap();
                        js_sys::Reflect::set(&resp, &"reference".into(), &indx.clone().into())
                            .unwrap();

                        if let Some(port) = port {
                            port.post_message(&resp).unwrap();
                        }

                        render_result(data, indx).await;
                    });
                }
            }
        }) as Box<dyn FnMut(_)>);

        let _service_listener = match web_sys::window()
            .unwrap()
            .navigator()
            .service_worker()
            .add_event_listener_with_callback("message", service_closure.as_ref().unchecked_ref())
        {
            Ok(aok) => aok,
            Err(err) => {
                interface_debug(&JsValue::from(format!("Service listener error {:#?}", err)));
            }
        };

        if read_initial_routes {
            spawn_local(async move {
                let _ = get_service_worker().await;
                let routes = read_routes().await;
                let mut handles = vec![];
                for route in routes {
                    let handle = async {
                        let weeb300 = weeb36.clone();
                        interface_debug(&JsValue::from(format!(
                            "Loading weeb-3 route from path {:#?}",
                            route
                        )));
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
            #[allow(irrefutable_let_patterns)]
            let logs_current = weeb35.get_current_logs().await;
            for log_message in logs_current.iter() {
                render_log_message(&log_message);
            }

            let ongoing = weeb35.get_ongoing_connections().await;

            if last_ongoing != Some(ongoing) {
                web_sys::window()
                    .unwrap()
                    .document()
                    .unwrap()
                    .get_element_by_id("ongoing")
                    .expect("#ongoing should exist")
                    .dyn_ref::<HtmlSpanElement>()
                    .unwrap()
                    .set_text_content(Some(&ongoing.to_string()));
                last_ongoing = Some(ongoing);
            }

            let connections = weeb35.get_connections().await;

            if last_connections != Some(connections) {
                web_sys::window()
                    .unwrap()
                    .document()
                    .unwrap()
                    .get_element_by_id("connections")
                    .expect("#connections should exist")
                    .dyn_ref::<HtmlSpanElement>()
                    .unwrap()
                    .set_text_content(Some(&connections.to_string()));
                last_connections = Some(connections);
            }

            if let Some((revision, progress_rows)) =
                weeb35.get_progress_snapshot(last_progress_revision).await
            {
                render_progress_rows(progress_rows);
                last_progress_revision = revision;
            }

            async_std::task::sleep(Duration::from_millis(160)).await
        }
    };

    let _fetch_test = async move {
        async_std::task::sleep(Duration::from_millis(6400)).await;

        let host3 = web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .location()
            .unwrap()
            .origin()
            .unwrap();

        let url = format!("{}/weeb-3/bzz", host3);

        let ascii_art = r#"@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@
%@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@@%%@%%@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@%%##+#+#*#%@%@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@%%#==+==+=+*+*#%%%@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@%%#==---------==++**#@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@@@%+==---------===++*#@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@*=-=----------==+*#%@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@@+==----:::::::-=++*#%@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%#@@%=-=====+++=---==+*#%@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%+*@%#---==++*****++++*%@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%#++#*---+##%@@%%*=-=*@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%++=+------=+#%#+::-%@@@#%@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%*====----::----:::=%@@@%%%@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%=====--------::::=%@@@%%%%@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%**===--------:::=+@@@%%%%@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%#====------:+#=*#@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%=====---::::-++#@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%*=====----:-=++#%@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%#+======**#*#%@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@@#+========**#@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@+%%*=+==-==+#%@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@@%**##@@%+---=+*%@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%@%@%#+%@%@@@@@%#*%@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%###*%%@%@@#@@@@@@@@@@@#@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%##*###%####+*%@@*@*@@@@@@@@%@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%%%%%%@%%%%%%#%####%%%@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%%%%%%@%%%%@@@@@%%@%%%%%%%%%%%@@@@@@@@@%@%%#%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%%#*######%@@@@@@@@%%%@@%%@@@@@@@@@@@@@@@%#%%#%%##%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%##**###%%%%@@@@@@@@@%@%%%@@%@@@@@@@@@%@@%%###%%##%##%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%@##**##%%@@@@@@@@@@@@@%%%@@@@@@@@@@@@@%@%@@%#########%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%%%#%%%#%%%@@@@@@@@@@@%%%%%%%@@%@@@@@@@@@%%%@@@%%###%%##%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%#*%%*+*%%%@@@@@@@@@@@@@@%%#%%%%%%%%@@@@@@@@%%%%@@%%#####%#%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%%%#%%+++%@@@@@@@@@@@@@@@@%##@%%%@@@%%%%@%%%@%%%@@@@%#####%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%@###@%#*#%#@@@@@@@@@@@@@@@%+*%%#%%@%%%%@@%@%@@@%%@@@@@@%%###%%#%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%###++%@@#%%@@@@@@@@@@@@@@@@**%%##%%%@@%%%@@@@@@@%%@@@@@@@%%#%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%##*+++#@@%@@@@@@@@@@@@@@@@@*##%%###%#%#%%%@@@@@@@@@@@@@@@@###%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%**++++#%@@@@@@@@@@@@@@@@@%**#%%#####%%#%%%@@@@@@@@%@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%##++=+*#%%%@@@@@@@@@@@@@@%%***#%####%####%%%@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%#*+++*#%%%@@@@@@@@@@@@@@%#*+***#**%#####*#%%@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%*+++++*%@@%@@@@@@@@@@@@%##*+****+********##%@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#*++++*#%@@@@@@@@@@@@@@@%#****##*+********##%%@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#*+++***#@@@@@@@@@@@@@@@%#*%*************##%%%@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#*+++++*%@@@@@@@@@@@@@@@%######*++*******###%%@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#*++=+**#@@@@@@@@@@@@@@@####%##*#########%%%%@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#*++++**#%@@@@@@@@@@@@@%##*##******######%%%@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%*++++*##%%@@@@@@@@@@@@%#*#######%%%%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#*++=+*%%%%@@@@@@@@@@@@%##%%#%**####%%%%%@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#*+++*#%@@@@@@@@@@@@@@@%###***##%%%%@%@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%*+++*#%%@@@%@@@@@@@@@@###########%%%@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%#++==*#@@@@@@@@@@@@@@@#*######%%%%@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%##*+=+#%@@@@@@@@@@@@@@@%########%%%%@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%#*++#%%#%%@@@@@@@@@@@@@%#%#%###%@%%@@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%%#**+**#%@@@@@@@@@@@@@@@@%%%%%%%%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#***+*@@@@@@@@@@@@@@@@@@@%%%%#%%%%%@@@@@@@@@@@@@@@@@@%@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%****#%*#@@@@@@@@@@@@@@@@@@%%%%%%%%@@@@@@@@@@@@@@@@@@@%@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%**#**#***%@@@@@@%@@@@@@@@@%%%%%%%%#%%@@@@@@@@@@@@@@@@%@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%##*#*###%%@@@@@@%%@@@@@@@@%%#%%%%%%%%%@@@@@@@@@@@@@@@@@@%%@@@@@@@@%%%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%*#*#*%%@@@@@@@@@%%%@@@@@@@@@@%%##%%%%%@@@@@@@@@@@@@@%%@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#***##%%%@@@@@@@%%%%@@@@@@@@@%%%%#%%%@@@@@@@@@@@@@@@%%@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%#*****#%#@@@@@@%%%%%@@@@@@@@@%@@@@@@@@@@@@@@@@@@@@@@@%@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%%
%%%%%%%%%%%####**%%@@@@@@@%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%%%%%%%
%%%%%%%%%%#%#*#**#%%%@@@@@%%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@%%%%%%%%%%%%
%%%%%%%%%%%#%#####%%%@@@@@%%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@%%@@@@@@@@@@%@@@@@%##%#%@@@%%%%%%%%%
%%%%%%%%%%%%#*###*#%%@@@@@@%%%%%@@@@@@@@+-+%@@@%%@@@@@@@@@@@@@@@@@@@@@@@@@@%@@%%%#%#%%%@@@@%%%%%%%%%
%%%%%%%%%%%######*#%%@@@@@@%%%%%@@@@@@@%*#%%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@#@+#%######%@@@@@@@%%%%%%%%
%%%%%%%%%%%%*#*##**#%@@@@@@@%%%%%@@@@@@@%%%%%%#**#%%%@@@@@@@@@@@@@@@@@@@@#%#+%#+#%##%@@@@@@@%%%%%%%%
%%%%%%%%%%%%%%#**+*#%%@@@@@@%%%%@@@@@@@@@%%%%%%%%%%%%@@@@@@@@@@@@@@@@@@@%@@%%@@@%%%=*@@##@#@@%%%%%%%
%%%%%%%%%%%%%%#*#**#*%@@@@@@@%%@@@@@@@@@@%%%%###%%%%%@%@@@@@@@@@@@@@@@@@@@@@@@%@@%*#%@@@@@##@@%%%%%%
%%%%%%%%%%%%%%###***##%%@@@@@%@@@@@@@@@@@%%%%%%%%%%%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@*=#@%%%@+%@@%%%%%
%%%%%%%%%%%%%%%#***+*#%@@@@@@@@@@@@@@@@%@@%%#%%%%%%%%@@@@@@@@@@@@@@@@@@@@#%@@@%*@@@@%+=@@%@@@@@%%%%%
%%%%%%%%%%%%%%%#****##@@@@@@@@@@@@@@@@%#****#%%%#%%%%@@@@@@@@@@@@@@@@@@@%%@@@@@@%@@@@@++#@%%@@@@%%%%
%%%%%%%%%%%%%%%####***#%%%@@@@@@@@@@@@%#*******##%%%%@@@@@@@@@@@@@@@@@@@@%%@@@@@@@@@@@@#*@@@*%@@%%%%
%%%%%%%%%%%%%%%%%##***###%%@@@@@@@@@@@%###**###*###%@@@@@@@@@@@@@@@@@@@@%@%@@@@@%%%%%%%@%#@@@@@@%%%%
%%%%%%%%%%%%%%%%%####**#%%@@@@@@@@@@@@%#%#########%@%@@@@@@@@@@@@@@@@@@@###%@%@@%%%%%%%%%%@@@%@@@%%%
%%%%%%%%%%%%%%%%%####***%%%@@@@@@@@@@@@%%%##%###%%%%%@@@@@@@@@@@@@@@@@@@%#@@%@@@%%%%%%%%%@@@@@@@@@%%
%%%%%%%%%%%%%%%%%%###**####@@@@@@@@@@@@@%%%%%%%%%%%@@@@@@@@@@@@@@@@@@@@@**%%@@@@%%%%%%%%%%%#@@@@@@@%
@@@@@@@@%%%%%%%%%%%%%%##**#@@@@@@@@@@@@@@@%%%%%%%@@@@@@@@@@@@@@@@@@@@@@%#%@@%@@@%%%%%%%%%%@@@@@@@%%@
@@@@@@@@@@@@@%%%%%%#####**%@@@@@@@@@@@@@@@@%%%%%@@@@@@@@@@@@@@@@@@@@@@@@#%%@@@@%%%%%%%%%%%@@@@@@#+@@
@@@@@@@@@@@@@@@@@@@%#####%%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@%%@@@@@@@%%%%%%%%%#%@@@@@%@@
@@@@@@@@@@@@@@@@@@@%%%%####@%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@%%@@@@@@@%%%%%%%%%*#%@@@@@@@"#;

        let u8arr = js_sys::Uint8Array::from(ascii_art.as_bytes());

        let blob_parts = js_sys::Array::new();
        blob_parts.push(&u8arr);
        let props = web_sys::BlobPropertyBag::new();
        props.set_type("text/plain");
        let blob =
            web_sys::Blob::new_with_u8_array_sequence_and_options(&blob_parts, &props).unwrap();

        let form = web_sys::FormData::new().unwrap();
        form.append_with_blob_and_filename("file", &blob, "sel.txt")
            .unwrap();

        let opts = web_sys::RequestInit::new();
        opts.set_method("POST");

        let headers = web_sys::Headers::new().unwrap();
        headers.set("swarm-encrypt", "true").unwrap();
        opts.set_headers(&headers);
        opts.set_body(&wasm_bindgen::JsValue::from(form)); // <-- important: JsValue

        let request = web_sys::Request::new_with_str_and_init(&url, &opts).unwrap();
        let window = web_sys::window().unwrap();
        let resp_value = JsFuture::from(window.fetch_with_request(&request)).await;
        web_sys::console::log_1(&JsValue::from(format!("Upload response: {:?}", resp_value)));

        let window = web_sys::window().unwrap();
        let resp_value = JsFuture::from(window.fetch_with_request(&request)).await;

        web_sys::console::log_1(&JsValue::from(format!("Upload response: {:?}", resp_value)));
    };

    interface_async.await;

    #[allow(unreachable_code)]
    Ok(())
}
