use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use web3::types::{Address, U256};

use js_sys::{Array, Reflect, Uint8Array};
use tar::{Builder, Header};
use wasm_bindgen::{JsCast, JsError, JsValue, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use web_sys::{
    Blob, BlobPropertyBag, Element, Event, HtmlButtonElement, HtmlElement, HtmlInputElement,
    HtmlSelectElement, HtmlSpanElement, RegistrationOptions, ServiceWorkerRegistration,
};

use crate::PrivateKeySigner;
use crate::{
    bzz_stream::{BzzMetadata, bzz_reference_hex, canonical_bzz_url, normalize_bzz_path},
    decode_resources,
    erasure_coding::upload_redundancy_from_select,
    interface_conventions::{install_interface_conventions, set_bracket_button_label},
    nav::{
        ResourceRoute, clear_hash_route, parse_networked_resource_route, read_route,
        route_network_mode_from_location,
    },
    network_profile::{
        NetworkMode, NetworkProfile, initial_bootnodes, is_browser_dialable_underlay,
        profile_for_mode, profile_for_swarm_network_id,
    },
    on_chain::{
        Web3Inst, chequebook_balance, deploy_chequebook_with_payer, deposit_to_chequebook,
        token_contract,
    },
    persistence::{
        get_chequebook_address, get_chequebook_signer_key, set_chequebook_address,
        set_chequebook_signer_key,
    },
    random_encryption_key,
    secure_vault::secure_open_vault_from_user_action,
    shared_runtime::SharedNodeClient,
    stream_conventions::{STREAMING_SERVICE_WORKER_SCOPE, STREAMING_SERVICE_WORKER_URL},
    wallet_workflows::{
        BatchPurchaseError, BatchPurchaseOutcome, MissingSecureBatchState, ensure_batch,
        inspect_batch,
    },
};

#[path = "interface_runtime_conventions.rs"]
mod interface_runtime_conventions;
use interface_runtime_conventions::*;
pub(crate) use interface_runtime_conventions::{
    get_service_worker, release_result_object_url, replace_result_view,
    service_worker_controls_bzz_requests, service_worker_scope_protocol_error,
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
pub(crate) type InterfaceNode = Rc<SharedNodeClient>;

fn required_element<T: JsCast>(document: &web_sys::Document, id: &str) -> T {
    document
        .get_element_by_id(id)
        .unwrap_or_else(|| panic!("#{id} should exist"))
        .dyn_into::<T>()
        .unwrap_or_else(|_| panic!("#{id} has an unexpected element type"))
}

fn interface_document() -> web_sys::Document {
    web_sys::window().unwrap().document().unwrap()
}

fn alert(message: &str) {
    let _ = web_sys::window().unwrap().alert_with_message(message);
}

async fn wallet_chain_matches(w3: &Web3Inst, profile: NetworkProfile) -> bool {
    if w3
        .eth()
        .chain_id()
        .await
        .is_ok_and(|chain_id| chain_id != U256::from(profile.wallet_chain_id))
    {
        alert(&format!(
            "Wallet is not on {:?} chain ({}). Please switch in your wallet and try again.",
            profile.mode, profile.wallet_chain_id
        ));
        false
    } else {
        true
    }
}

async fn stored_chequebook_address() -> Option<Address> {
    let signer_key = get_chequebook_signer_key().await;
    let address = get_chequebook_address().await;
    (!signer_key.is_empty() && address.len() == 20).then(|| Address::from_slice(&address))
}

pub(crate) fn shared_network_changed(network_id: u64) {
    if let Some(profile) = profile_for_swarm_network_id(network_id) {
        set_network_profile_inputs(profile.mode);
    } else if let Some(input) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("networkIDSettings"))
        .and_then(|element| element.dyn_into::<HtmlInputElement>().ok())
    {
        input.set_value(&network_id.to_string());
    }
}

thread_local! {
    static NETWORK_APPLY_GENERATION: Cell<u64> = const { Cell::new(0) };
    static INTERFACE_MOUNT_GENERATION: Cell<u64> = const { Cell::new(0) };
    static BFCACHE_PAGESHOW_LISTENER: RefCell<Option<Closure<dyn FnMut(Event)>>> =
        const { RefCell::new(None) };
}

fn pageshow_event_is_persisted(event: &Event) -> bool {
    Reflect::get(event.as_ref(), &JsValue::from_str("persisted"))
        .ok()
        .and_then(|persisted| persisted.as_bool())
        .unwrap_or(false)
}

pub(crate) fn install_bfcache_restore_guard() {
    BFCACHE_PAGESHOW_LISTENER.with(|listener| {
        if listener.borrow().is_some() {
            return;
        }

        let Some(window) = web_sys::window() else {
            return;
        };
        let mut reload_requested = false;
        let callback = Closure::<dyn FnMut(Event)>::new(move |event| {
            if reload_requested || !pageshow_event_is_persisted(&event) {
                return;
            }
            reload_requested = true;
            if let Some(window) = web_sys::window() {
                let _ = window.location().reload();
            }
        });
        if window
            .add_event_listener_with_callback("pageshow", callback.as_ref().unchecked_ref())
            .is_ok()
        {
            *listener.borrow_mut() = Some(callback);
        }
    });
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

#[wasm_bindgen]
pub async fn interweeb(_st: String) -> Result<(), JsError> {
    install_bfcache_restore_guard();
    clear_hash_route();
    let initial_mode = route_network_mode_from_location().unwrap_or(NetworkMode::Mainnet);
    let initial_profile = profile_for_mode(initial_mode);
    set_network_profile_inputs(initial_mode);

    // Overlap service-worker installation/update with SharedWorker WASM startup and
    // the initial connection burst.
    spawn_local(async {
        let _ = get_service_worker().await;
    });
    let weeb3 = Rc::new(SharedNodeClient::new(
        initial_profile.swarm_network_id,
        None,
    ));
    let initial_apply = next_network_apply_generation();
    connect_all_bootnode_settings(&weeb3, initial_apply, initial_profile.swarm_network_id).await;
    weeb3.interface_log(format!(
        "Attached to the SharedWorker node for {:?} network {}",
        initial_profile.mode, initial_profile.swarm_network_id
    ));
    mount_interface(weeb3).await
}

pub(crate) async fn mount_interface(weeb3: InterfaceNode) -> Result<(), JsError> {
    let mount_generation = begin_interface_mount();
    mount_interface_with_generation(weeb3, None, mount_generation).await
}

pub(crate) async fn mount_interface_with_generation(
    weeb3: InterfaceNode,
    initial_result_generation: Option<u64>,
    mount_generation: u64,
) -> Result<(), JsError> {
    if !interface_mount_is_current(mount_generation) {
        return Ok(());
    }

    if let Err(error) = weeb3.ensure().await {
        return Err(JsError::new(&error));
    }

    async_std::task::yield_now().await;
    if !interface_mount_is_current(mount_generation) {
        return Ok(());
    }

    install_interface_conventions();
    if let Some(profile) = profile_for_swarm_network_id(weeb3.network_id()) {
        set_network_profile_inputs(profile.mode);
    }
    weeb3.interface_log(format!(
        "Interface mounted, version {}",
        INTERFACE_BUILD_VERSION
    ));
    weeb3.claim_vault_broker();

    let input_node = weeb3.clone();
    let upload_node = weeb3.clone();
    let network_node = weeb3.clone();
    let reset_node = weeb3.clone();
    let snapshot_node = weeb3.clone();
    let route_node = weeb3.clone();
    let pause_node = weeb3.clone();
    let prerequisites_node = weeb3.clone();

    let chequebook_state = Rc::new(Cell::new(None::<Address>));
    let chequebook_state_deploy = chequebook_state.clone();
    let chequebook_state_deposit = chequebook_state.clone();
    let document = interface_document();

    let resource_input_callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let node = input_node.clone();
        let document = interface_document();
        let input_field = required_element::<HtmlInputElement>(&document, "inputString");

        let text = input_field.value();
        spawn_local(async move {
            open_resource_input(node, text).await;
        });
    });

    required_element::<HtmlInputElement>(&document, "inputString")
        .set_oninput(Some(resource_input_callback.as_ref().unchecked_ref()));

    let pause_callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let node = pause_node.clone();

        spawn_local(async move {
            let paused = node.toggle_transfer_pause().await;
            update_transfer_pause_button(paused);
        });
    });

    required_element::<HtmlButtonElement>(&document, "transferPauseToggle")
        .set_onclick(Some(pause_callback.as_ref().unchecked_ref()));

    let batch_purchase_callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let (batch_depth, validity) = match read_batch_request_settings() {
            Ok(settings) => settings,
            Err(error) => {
                alert(&error);
                return;
            }
        };

        spawn_local(async move {
            let payer = match connect_wallet_address().await {
                Ok(payer) => payer,
                Err(error) => {
                    alert(&format!("Wallet connect failed: {error}"));
                    return;
                }
            };

            let profile = current_network_profile();

            let (prepared, purchase) = match ensure_batch(
                payer,
                profile,
                batch_depth,
                validity,
                MissingSecureBatchState::Error,
            )
            .await
            {
                Ok(BatchPurchaseOutcome::AlreadyReady(_)) => {
                    alert("Already have a secure batch for uploads");
                    return;
                }
                Ok(BatchPurchaseOutcome::Purchased {
                    prepared, purchase, ..
                }) => (prepared, purchase),
                Err(BatchPurchaseError::CheckSecure) => {
                    alert("Could not check weeb-3-secure for the connected wallet");
                    return;
                }
                Err(BatchPurchaseError::PrepareSecure) => {
                    alert("Failed to prepare secure batch owner");
                    return;
                }
                Err(BatchPurchaseError::OnChain(error)) => {
                    alert(&format!(
                        "Batch purchase failed: {:?}. Ensure wallet is on {:?} and has {} + {}.",
                        error, profile.mode, profile.bzz_symbol, profile.base_symbol
                    ));
                    return;
                }
                Err(BatchPurchaseError::CommitSecure) => {
                    alert("Failed to save or verify batch in weeb-3-secure");
                    return;
                }
            };

            alert(&format!(
                "Storage batch ready.\nBatch ID: 0x{}\nDepth: {}\nStorage slots per bucket: {}",
                hex::encode(&purchase.batch_id),
                prepared.depth,
                purchase.bucket_limit
            ));
        });
    });

    required_element::<HtmlButtonElement>(&document, "uploadGetBatch")
        .set_onclick(Some(batch_purchase_callback.as_ref().unchecked_ref()));

    let _retained_message_callback = if let Some(button) = document
        .get_element_by_id("uploadPrereqCheck")
        .and_then(|button| button.dyn_into::<HtmlButtonElement>().ok())
    {
        let callback_prereq = Closure::<dyn FnMut(Event)>::new({
            let prerequisites_node = prerequisites_node.clone();
            move |_| {
                let node = prerequisites_node.clone();
                spawn_local(async move {
                    check_upload_prerequisites(node).await;
                });
            }
        });

        button.set_onclick(Some(callback_prereq.as_ref().unchecked_ref()));
        Some(callback_prereq)
    } else {
        None
    };

    let upload_callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let node = upload_node.clone();

        let document = interface_document();
        let file_input = required_element::<HtmlInputElement>(&document, "uploadFileSelect");

        let Some(file) = file_input.files().and_then(|files| files.item(0)) else {
            return;
        };
        file_input.set_value("");
        secure_open_vault_from_user_action();
        spawn_local(async move {
            let file_enc = required_element::<HtmlInputElement>(&document, "uploadFileEncrypt");
            let upload_to_feed = required_element::<HtmlInputElement>(&document, "uploadAddToFeed");
            let upload_to_feed = upload_to_feed.checked();

            let feed_topic = if upload_to_feed {
                let topic_field =
                    required_element::<HtmlInputElement>(&document, "feedTopicString");
                topic_field.value()
            } else {
                String::new()
            };

            let index_input = required_element::<HtmlInputElement>(&document, "indexString");
            let index_string = index_input.value();

            let redundancy_value = document
                .get_element_by_id("uploadRedundancyLevel")
                .and_then(|element| element.dyn_into::<HtmlSelectElement>().ok())
                .map(|select| select.value());
            let redundancy_level = upload_redundancy_from_select(redundancy_value.as_deref());

            let result = node
                .post_upload_with_redundancy(
                    file,
                    file_enc.checked() && !upload_to_feed,
                    redundancy_level,
                    index_string,
                    upload_to_feed,
                    feed_topic,
                )
                .await;

            let (data, indx) = decode_resources(result);

            render_result(data, indx);
        })
    });

    required_element::<HtmlButtonElement>(&document, "uploadFile")
        .set_onclick(Some(upload_callback.as_ref().unchecked_ref()));

    let network_toggle_node = network_node.clone();
    let network_apply_callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let node = network_node.clone();
        let network_id = current_network_id_input();
        let apply_generation = next_network_apply_generation();
        spawn_local(async move {
            apply_network_settings_and_connect(&node, apply_generation, network_id).await;
        });
    });

    required_element::<HtmlButtonElement>(&document, "networkSet")
        .set_onclick(Some(network_apply_callback.as_ref().unchecked_ref()));

    let _network_profile_toggle_callback = install_network_profile_toggle(network_toggle_node);

    let stamp_reset_callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let node = reset_node.clone();

        let window = web_sys::window().unwrap();

        if window
            .confirm_with_message(
                "This will enable overwriting previously uploaded content with new content.",
            )
            .unwrap_or(false)
        {
            spawn_local(async move {
                let result = node.reset_stamp().await;

                let (data, indx) = decode_resources(result);

                render_result(data, indx);
            })
        }
    });

    required_element::<HtmlButtonElement>(&document, "uploadResetStamp")
        .set_onclick(Some(stamp_reset_callback.as_ref().unchecked_ref()));

    let deploy_chequebook_callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let state = chequebook_state_deploy.clone();
        spawn_local(async move {
            let payer = match connect_wallet_address().await {
                Ok(payer) => payer,
                Err(error) => {
                    alert(&format!("Wallet connect failed: {error}"));
                    return;
                }
            };

            if let Some(address) = stored_chequebook_address().await {
                alert(&format!(
                    "Already have a chequebook deployed at address 0x{}",
                    hex::encode(address.as_bytes())
                ));
                return;
            }

            let cheque_signer_key = random_encryption_key();
            let cheque_signer = match PrivateKeySigner::from_slice(&cheque_signer_key) {
                Ok(s) => s,
                Err(_) => {
                    alert("Failed to create chequebook signer key");
                    return;
                }
            };
            let issuer_h160_bytes: [u8; 20] = *cheque_signer.address().as_ref();
            let issuer = Address::from(issuer_h160_bytes);

            let deployment = match deploy_chequebook_with_payer(issuer, payer).await {
                Ok(d) => d,
                Err(e) => {
                    alert(&format!("Chequebook deployment failed: {e:?}"));
                    return;
                }
            };

            if !set_chequebook_signer_key(&cheque_signer_key).await {
                alert("Chequebook deployed, but failed to save signer key locally.");
            }

            if !set_chequebook_address(deployment.chequebook.as_bytes()).await {
                alert("Chequebook deployed, but failed to save address locally.");
            }

            state.set(Some(deployment.chequebook));

            alert(&format!(
                "Chequebook deployed at 0x{}.\nIssuer: 0x{}\nDeployment tx: 0x{}",
                hex::encode(deployment.chequebook.as_bytes()),
                hex::encode(issuer_h160_bytes),
                hex::encode(deployment.tx.as_bytes())
            ));
        });
    });

    required_element::<HtmlButtonElement>(&document, "deployChequebook")
        .set_onclick(Some(deploy_chequebook_callback.as_ref().unchecked_ref()));

    let deposit_callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let state = chequebook_state_deposit.clone();

        let document = interface_document();
        let amount_input = required_element::<HtmlInputElement>(&document, "depositAmount");

        let amount_raw = amount_input.value();
        let amount = match U256::from_dec_str(amount_raw.trim()) {
            Ok(v) => v,
            Err(_) => {
                alert("Failed to read deposit amount");
                return;
            }
        };

        if amount == U256::from(0u8) {
            alert("Deposit amount must be greater than zero");
            return;
        }

        let chequebook = state.get();

        spawn_local(async move {
            let chequebook = match chequebook {
                Some(addr) => addr,
                None => match stored_chequebook_address().await {
                    Some(address) => {
                        state.set(Some(address));
                        address
                    }
                    None => {
                        alert("Deploy a chequebook first before depositing.");
                        return;
                    }
                },
            };

            let payer = match connect_wallet_address().await {
                Ok(payer) => payer,
                Err(error) => {
                    alert(&format!("Wallet connect failed: {error}"));
                    return;
                }
            };

            let w3 = match crate::on_chain::web3() {
                Ok(w) => w,
                Err(e) => {
                    alert(&format!("Failed to initialize web3: {e:?}"));
                    return;
                }
            };

            let profile = current_network_profile();

            if !wallet_chain_matches(&w3, profile).await {
                return;
            }

            let token = match token_contract(&w3) {
                Ok(t) => t,
                Err(e) => {
                    alert(&format!("Failed to load token contract: {e:?}"));
                    return;
                }
            };

            let receipt = match deposit_to_chequebook(&token, chequebook, payer, amount).await {
                Ok(r) => r,
                Err(e) => {
                    alert(&format!("Deposit failed: {e:?}"));
                    return;
                }
            };

            let mut balance_note = String::new();
            if let Ok(balance) = chequebook_balance(&w3, chequebook).await {
                balance_note = format!("\nNew balance: {}", balance);
            }

            alert(&format!(
                "Deposit submitted.\nTx: 0x{}{}",
                hex::encode(receipt.transaction_hash.as_bytes()),
                balance_note
            ));
        });
    });

    required_element::<HtmlButtonElement>(&document, "depositCash")
        .set_onclick(Some(deposit_callback.as_ref().unchecked_ref()));

    spawn_local(async move {
        if initial_result_generation
            .is_some_and(|generation| !crate::stream::result_view_request_is_current(generation))
        {
            return;
        }
        let Some(route) = read_route() else {
            return;
        };
        open_resource(route_node, route).await;
    });

    let ongoing_element = required_element::<HtmlSpanElement>(&document, "ongoing");
    let connections_element = required_element::<HtmlSpanElement>(&document, "connections");
    let mut last_progress_revision = 0u64;
    let mut last_ongoing = None::<u64>;
    let mut last_connections = None::<u64>;
    let mut last_paused = None::<bool>;
    loop {
        if !interface_mount_can_poll(mount_generation) {
            break;
        }
        if document.hidden() {
            async_std::task::sleep(Duration::from_millis(160)).await;
            continue;
        }

        let Some(snapshot) = snapshot_node.runtime_snapshot(last_progress_revision).await else {
            async_std::task::sleep(Duration::from_millis(160)).await;
            continue;
        };
        if !interface_mount_can_poll(mount_generation) {
            break;
        }
        render_log_messages(&snapshot.logs);
        if last_paused != Some(snapshot.paused) {
            update_transfer_pause_button(snapshot.paused);
            last_paused = Some(snapshot.paused);
        }

        let ongoing = snapshot.ongoing_connections;
        if last_ongoing != Some(ongoing) {
            ongoing_element.set_text_content(Some(&ongoing.to_string()));
            last_ongoing = Some(ongoing);
        }

        let connections = snapshot.connections;
        if last_connections != Some(connections) {
            connections_element.set_text_content(Some(&connections.to_string()));
            last_connections = Some(connections);
        }

        if let Some((revision, progress_rows)) = snapshot.progress {
            render_progress_rows(progress_rows);
            last_progress_revision = revision;
        }

        async_std::task::sleep(Duration::from_millis(160)).await
    }

    Ok(())
}
