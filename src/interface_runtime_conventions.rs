use super::*;
use js_sys::{Object, Reflect};
use web_sys::{MessageChannel, MessageEvent, MessagePort};

use crate::worker_protocol::{number_property, set as set_js, string_property};

const SERVICE_WORKER_PROTOCOL: f64 = 10.0;
const SERVICE_WORKER_MARKER: &str = "forwarder-default29";
const SERVICE_WORKER_CONTROL_TOTAL_TIMEOUT_MS: u64 = 30_000;
const SERVICE_WORKER_SETUP_RETRY_MS: f64 = 1_500.0;
const DOWNLOAD_URL_REVOKE_DELAY_MS: i32 = 1_000;
static SERVICE_WORKER_SETUP_LOCK: async_lock::Mutex<()> = async_lock::Mutex::new(());

thread_local! {
    static SERVICE_WORKER_MISSING_VISIBLE: Cell<bool> = const { Cell::new(false) };
    static ACTIVE_RESULT_OBJECT_URL: RefCell<Option<String>> = const { RefCell::new(None) };
    static RESULT_CALLBACKS: RefCell<Vec<Closure<dyn FnMut(Event)>>> = const { RefCell::new(Vec::new()) };
}

pub(super) async fn check_upload_prerequisites(weeb3: InterfaceNode) {
    let progress_id = weeb3
        .start_progress(
            "upload-prereq",
            "wallet and batch",
            "wallet",
            None,
            "connecting wallet",
        )
        .await;

    let (message, phase, detail, ok) = match collect_upload_prerequisites().await {
        Ok(message) => (message, "complete", "prerequisites checked", true),
        Err(error) => (
            format!("Upload prerequisites failed: {error}"),
            "failed",
            "prerequisite check failed",
            false,
        ),
    };
    render_text_result(&message);
    weeb3.interface_log(message);
    weeb3.finish_progress(&progress_id, phase, detail, ok).await;
}

pub(super) async fn collect_upload_prerequisites() -> Result<String, String> {
    let profile = current_network_profile();
    let (batch_depth, validity_days) = read_batch_request_settings()?;
    let payer = connect_wallet_address().await?;
    let inspected = inspect_batch(payer, profile, batch_depth, validity_days).await?;

    let mut lines = vec![
        "Upload prerequisites".to_string(),
        format!(
            "network: {:?}, swarm id {}, wallet chain {}",
            profile.mode, profile.swarm_network_id, profile.wallet_chain_id
        ),
        format!("wallet: 0x{}", hex::encode(payer.as_bytes())),
        format!("wallet chain: {}", inspected.chain_id),
    ];

    let Some(funding) = inspected.funding else {
        lines.push(format!(
            "state: wrong wallet network, expected chain {}",
            profile.wallet_chain_id
        ));
        return Ok(lines.join("\n"));
    };

    if inspected.secure.usable() {
        lines.push(format!(
            "batch: usable, id 0x{}, bucket limit {}, status {}, about {} days remaining",
            hex::encode(&inspected.secure.batch_id),
            inspected.secure.batch_bucket_limit,
            inspected.secure.batch_validity_status,
            funding.remaining_days.unwrap_or_default()
        ));
    } else {
        lines.push(format!(
            "batch: not usable, status {}, id length {}",
            inspected.secure.batch_validity_status,
            inspected.secure.batch_id.len()
        ));
    }

    lines.push(format!(
        "requested batch: depth {}, validity {} days",
        batch_depth, validity_days
    ));
    lines.push(format!(
        "{}: balance {}, required {}, enough {}",
        profile.bzz_symbol,
        funding.token_balance,
        funding.required_bzz,
        funding.token_balance >= funding.required_bzz
    ));
    lines.push(format!(
        "{}: balance {}, nonzero for gas {}",
        profile.base_symbol,
        funding.base_balance,
        !funding.base_balance.is_zero()
    ));

    Ok(lines.join("\n"))
}

pub(super) fn current_network_profile() -> crate::network_profile::NetworkProfile {
    let document = interface_document();
    let network_id = document
        .get_element_by_id("networkIDSettings")
        .and_then(|el| el.dyn_into::<HtmlInputElement>().ok())
        .and_then(|input| input.value().parse::<u64>().ok())
        .unwrap_or(1);

    profile_for_swarm_network_id(network_id)
        .unwrap_or_else(|| profile_for_mode(NetworkMode::Mainnet))
}

pub(super) fn read_batch_request_settings() -> Result<(u8, u64), String> {
    let document = interface_document();
    let validity = document
        .get_element_by_id("batchValidityDays")
        .and_then(|el| el.dyn_into::<HtmlInputElement>().ok())
        .and_then(|input| input.value().parse::<u64>().ok())
        .ok_or_else(|| "failed to read batch validity".to_string())?;

    let batch_depth = document
        .get_element_by_id("batchSize")
        .and_then(|el| el.dyn_into::<HtmlSelectElement>().ok())
        .and_then(|input| input.value().parse::<u8>().ok())
        .map(|size| 17 + size)
        .ok_or_else(|| "failed to read batch size".to_string())?;

    Ok((batch_depth, validity))
}

pub(super) async fn connect_wallet_address() -> Result<Address, String> {
    crate::library::request_wallet_via_shell_connector(current_network_profile().wallet_chain_id)
        .await
        .unwrap_or_else(|| Err("wallet connector is not available".to_string()))
}

pub(super) fn next_network_apply_generation() -> u64 {
    NETWORK_APPLY_GENERATION.with(|generation| {
        let next = generation.get().saturating_add(1);
        generation.set(next);
        next
    })
}

pub(super) fn is_current_network_apply_generation(apply_generation: u64) -> bool {
    NETWORK_APPLY_GENERATION.with(|generation| generation.get() == apply_generation)
}

pub(super) fn install_network_profile_toggle(
    weeb3: InterfaceNode,
) -> Option<Closure<dyn FnMut(Event)>> {
    let document = interface_document();
    update_network_mode_toggle(current_network_profile().mode);

    let button = document
        .get_element_by_id("networkModeToggle")?
        .dyn_into::<HtmlButtonElement>()
        .ok()?;

    let callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        let node = weeb3.clone();
        let mode = match current_network_profile().mode {
            NetworkMode::Testnet => NetworkMode::Mainnet,
            NetworkMode::Mainnet => NetworkMode::Testnet,
        };
        let profile = profile_for_mode(mode);
        set_network_profile_inputs(mode);
        let apply_generation = next_network_apply_generation();
        node.interface_log(format!(
            "Network mode switched to {:?} chain {}",
            profile.mode, profile.wallet_chain_id
        ));
        let network_id = profile.swarm_network_id.to_string();
        spawn_local(async move {
            apply_network_settings_and_connect(&node, apply_generation, network_id).await;
        });
    });

    button.set_onclick(Some(callback.as_ref().unchecked_ref()));
    Some(callback)
}

pub(super) fn set_network_profile_inputs(mode: NetworkMode) {
    let profile = profile_for_mode(mode);
    let document = interface_document();
    update_network_mode_toggle(profile.mode);

    if let Some(network_id_input) = document
        .get_element_by_id("networkIDSettings")
        .and_then(|element| element.dyn_into::<HtmlInputElement>().ok())
    {
        network_id_input.set_value(&profile.swarm_network_id.to_string());
    }

    for (index, element_id) in BOOTNODE_INPUT_IDS.iter().enumerate() {
        let Some(input) = document.get_element_by_id(element_id) else {
            continue;
        };
        let Some(input) = input.dyn_ref::<HtmlInputElement>() else {
            continue;
        };
        input.set_value(profile.bootnodes.get(index).copied().unwrap_or_default());
    }

    set_progress_notice(
        "networkModeProgress",
        &format!(
            "Network mode: {:?}. Wallet chain {}, base {}, token {}.",
            profile.mode, profile.wallet_chain_id, profile.base_symbol, profile.bzz_symbol
        ),
    );

    let mainnet_notice = "Mainnet profile loaded. Browser dial skips official TCP bootnodes; enter WSS mainnet underlays to connect from the browser.";
    if mode == NetworkMode::Mainnet
        && profile
            .bootnodes
            .iter()
            .any(|address| !is_browser_dialable_underlay(address))
    {
        set_progress_notice("networkModeWarning", mainnet_notice);
    } else {
        clear_progress_notice("networkModeWarning");
    }
}

pub(super) fn set_progress_notice(id: &str, message: &str) {
    let document = interface_document();
    let Some(actions) = ensure_progress_child(&document, "progressActions", "div") else {
        return;
    };

    let row = match document.get_element_by_id(id) {
        Some(row) => row,
        None => {
            let Ok(row) = document.create_element("div") else {
                return;
            };
            row.set_id(id);
            let _ = actions.prepend_with_node_1(&row);
            row
        }
    };

    row.set_text_content(Some(message));
}

pub(super) fn clear_progress_notice(id: &str) {
    if let Some(row) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(id))
    {
        row.remove();
    }
}

pub(super) fn ensure_progress_child(
    document: &web_sys::Document,
    id: &str,
    tag: &str,
) -> Option<Element> {
    if let Some(existing) = document.get_element_by_id(id) {
        return Some(existing);
    }

    let progress_field = document.get_element_by_id("progressField")?;
    let child = document.create_element(tag).ok()?;
    child.set_id(id);
    let _ = progress_field.append_child(&child);
    Some(child)
}

fn append_result_action(node: &Element) {
    let document = interface_document();
    let Some(parent) = ensure_progress_child(&document, "progressActions", "div") else {
        return;
    };
    let actions = document.get_element_by_id("resultActions").or_else(|| {
        let actions = document.create_element("div").ok()?;
        actions.set_id("resultActions");
        parent.append_child(&actions).ok()?;
        Some(actions)
    });
    let Some(actions) = actions else { return };
    let _ = actions.append_child(node);
}

pub(super) async fn apply_network_settings_and_connect(
    weeb3: &InterfaceNode,
    apply_generation: u64,
    network_id: String,
) {
    if !is_current_network_apply_generation(apply_generation) {
        return;
    }

    if network_id.trim().is_empty() {
        weeb3.interface_log("Network id is empty; not reconnecting");
        return;
    }
    let Ok(expected_network_id) = network_id.parse::<u64>() else {
        return;
    };

    let current_network = weeb3.network_id();
    if expected_network_id != current_network {
        crate::stream::release_current_stream_view();
    }
    if let Some(profile) = profile_for_swarm_network_id(expected_network_id) {
        update_network_mode_toggle(profile.mode);
    }
    connect_all_bootnode_settings(weeb3, apply_generation, expected_network_id).await;
}

pub(super) fn current_network_id_input() -> String {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("networkIDSettings"))
        .and_then(|element| element.dyn_into::<HtmlInputElement>().ok())
        .map(|input| input.value())
        .unwrap_or_else(|| "1".to_string())
}

pub(super) async fn connect_all_bootnode_settings(
    weeb3: &InterfaceNode,
    apply_generation: u64,
    expected_network_id: u64,
) {
    if !is_current_network_apply_generation(apply_generation) {
        return;
    }

    let network_id = expected_network_id.to_string();
    let mut seen = std::collections::HashSet::<String>::new();
    let mut dial_requests = Vec::<(String, bool)>::new();

    let profile = profile_for_swarm_network_id(expected_network_id);
    if let Some(profile) = profile {
        for address in initial_bootnodes(profile) {
            let address = address.to_string();
            if !is_browser_dialable_underlay(&address) {
                weeb3.interface_log(format!(
                    "Skipped non-browser bootnode for network {network_id}: {address}"
                ));
            } else if seen.insert(address.clone()) {
                dial_requests.push((address, true));
            }
        }
    }

    for (index, element_id) in BOOTNODE_INPUT_IDS.iter().enumerate() {
        let address = bootnode_setting(element_id);
        if address.trim().is_empty() {
            continue;
        }
        if profile
            .and_then(|profile| profile.bootnodes.get(index))
            .is_some_and(|default| address == *default)
        {
            continue;
        }
        if !is_browser_dialable_underlay(&address) {
            weeb3.interface_log(format!(
                "Skipped non-browser bootnode for network {}: {}",
                network_id, address
            ));
            continue;
        }
        if seen.insert(address.clone()) {
            dial_requests.push((address, true));
        }
    }

    weeb3.interface_log(format!(
        "Connecting {} configured bootnodes for network {}",
        dial_requests.len(),
        network_id
    ));

    if let Err(error) = weeb3.configure(expected_network_id, dial_requests).await {
        weeb3.interface_log(format!(
            "SharedWorker network configuration failed: {error}"
        ));
        let actual_network_id = weeb3.network_id();
        if let Some(profile) = profile_for_swarm_network_id(actual_network_id) {
            set_network_profile_inputs(profile.mode);
        }
    }
}

pub(super) fn update_network_mode_toggle(mode: NetworkMode) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(button) = document.get_element_by_id("networkModeToggle") else {
        return;
    };
    let Some(button) = button.dyn_ref::<HtmlButtonElement>() else {
        return;
    };

    set_bracket_button_label(
        button.unchecked_ref::<Element>(),
        match mode {
            NetworkMode::Testnet => " Testnet ",
            NetworkMode::Mainnet => " Mainnet ",
        },
    );
}

pub(super) async fn open_resource_input(weeb3: InterfaceNode, input: String) {
    if let Some(route) = parse_networked_resource_route(&input) {
        let profile = profile_for_mode(route.network);
        if weeb3.network_id() != profile.swarm_network_id {
            set_network_profile_inputs(route.network);
            let apply_generation = next_network_apply_generation();
            apply_network_settings_and_connect(
                &weeb3,
                apply_generation,
                profile.swarm_network_id.to_string(),
            )
            .await;
            if weeb3.network_id() != profile.swarm_network_id {
                render_text_result("Could not apply the resource route network");
                return;
            }
        }
        open_resource(weeb3, route.resource).await;
    } else {
        open_bzz_resource(weeb3, input).await;
    }
}

pub(super) async fn open_resource(weeb3: InterfaceNode, route: ResourceRoute) {
    match route {
        ResourceRoute::Bzz(resource) => open_bzz_resource(weeb3, resource).await,
        ResourceRoute::Bytes(reference) => {
            let bytes = weeb3.retrieve_bytes(reference.clone()).await;
            download_raw_bytes(bytes, reference, "bytes");
        }
        ResourceRoute::Chunks(reference) => {
            let bytes = weeb3.retrieve_chunk_bytes(reference.clone()).await;
            download_raw_bytes(bytes, reference, "chunk");
        }
        ResourceRoute::Hls {
            owner,
            topic,
            start,
        } => {
            crate::stream_hls::open_hls_feed_view(weeb3, owner, topic, start).await;
        }
    }
}

pub(super) fn download_raw_bytes(bytes: Uint8Array, filename: String, label: &str) {
    if bytes.length() == 0 {
        render_text_result(&format!("Could not retrieve {} {}", label, filename));
        return;
    }

    let Some(url) = create_blob_part(bytes.as_ref(), "application/octet-stream")
        .as_ref()
        .and_then(blob_object_url)
    else {
        render_text_result(&format!(
            "Could not create download for {} {}",
            label, filename
        ));
        return;
    };
    click_download_url(url, &filename);

    render_text_result(&format!(
        "Started {} download {} ({} bytes)",
        label,
        filename,
        bytes.length()
    ));
}

pub(super) fn click_download_url(url: String, filename: &str) {
    let document = match web_sys::window().and_then(|window| window.document()) {
        Some(document) => document,
        None => {
            revoke_object_url_later(url);
            return;
        }
    };
    let anchor = match document.create_element("a") {
        Ok(anchor) => anchor,
        Err(_) => {
            revoke_object_url_later(url);
            return;
        }
    };
    let _ = anchor.set_attribute("href", &url);
    let _ = anchor.set_attribute("download", filename);
    let _ = anchor.set_attribute("style", "display:none");

    if let Some(body) = document.body() {
        let _ = body.append_child(&anchor);
    }

    if let Some(anchor) = anchor.dyn_ref::<HtmlElement>() {
        anchor.click();
    }

    if let Some(parent) = anchor.parent_node() {
        let _ = parent.remove_child(&anchor);
    }
    revoke_object_url_later(url);
}

fn revoke_object_url_later(url: String) {
    let revoke_url = url.clone();
    let callback = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&revoke_url);
    });
    let scheduled = web_sys::window().is_some_and(|window| {
        window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.unchecked_ref(),
                DOWNLOAD_URL_REVOKE_DELAY_MS,
            )
            .is_ok()
    });
    if !scheduled {
        let _ = web_sys::Url::revoke_object_url(&url);
    }
}

fn create_blob_part(part: &JsValue, mime: &str) -> Option<Blob> {
    let props = BlobPropertyBag::new();
    props.set_type(mime);
    let parts = Array::new();
    parts.push(part);
    Blob::new_with_u8_array_sequence_and_options(&parts, &props).ok()
}

fn create_blob(bytes: &[u8], mime: &str) -> Option<Blob> {
    create_blob_part(Uint8Array::from(bytes).as_ref(), mime)
}

fn blob_object_url(blob: &Blob) -> Option<String> {
    web_sys::Url::create_object_url_with_blob(blob).ok()
}
pub(super) fn blob_url(bytes: &[u8], mime: &str) -> Option<String> {
    blob_object_url(&create_blob(bytes, mime)?)
}

pub(super) fn result_filename(path: &str, fallback: &str) -> String {
    let path = normalize_bzz_path(path);
    if path.is_empty() || path == "not found" || path.starts_with("unknown") {
        fallback.to_string()
    } else {
        path.rsplit('/').next().unwrap_or(fallback).to_string()
    }
}

pub(crate) fn release_result_object_url() {
    ACTIVE_RESULT_OBJECT_URL.with(|active| {
        if let Some(url) = active.borrow_mut().take() {
            let _ = web_sys::Url::revoke_object_url(&url);
        }
    });
}

fn clear_result_view() {
    let document = interface_document();
    if let Some(actions) = document.get_element_by_id("resultActions") {
        actions.set_inner_html("");
    }
    release_result_object_url();
    SERVICE_WORKER_MISSING_VISIBLE.with(|visible| visible.set(false));
    let result = document
        .get_element_by_id("resultField")
        .expect("#resultField should exist");
    result.set_inner_html("");
    RESULT_CALLBACKS.with(|callbacks| callbacks.borrow_mut().clear());
}

pub(crate) fn replace_result_view(node: &Element) {
    clear_result_view();
    append_result_view(node);
}

fn append_result_view(node: &Element) {
    let document = interface_document();
    let result = document
        .get_element_by_id("resultField")
        .expect("#resultField should exist");
    let _ = result.append_child(node);
}

type RenderedEntries = Rc<Vec<(Vec<u8>, String, String)>>;

pub(super) fn render_single_result_with_download((bytes, mime, path): &(Vec<u8>, String, String)) {
    let document = interface_document();
    let wrapper = match document.create_element("div") {
        Ok(wrapper) => wrapper,
        Err(_) => return,
    };
    let button = match document.create_element("button") {
        Ok(button) => button,
        Err(_) => return,
    };

    let filename = result_filename(path, "download");
    set_bracket_button_label(&button, &format!("Download {}", filename));
    let blob = create_blob(bytes, mime);
    let download_blob = blob.clone();
    let callback = Closure::<dyn FnMut(Event)>::new(move |_event| {
        if let Some(url) = download_blob.as_ref().and_then(blob_object_url) {
            click_download_url(url, &filename);
        }
    });
    let _ = button.add_event_listener_with_callback("click", callback.as_ref().unchecked_ref());
    append_result_action(&button);
    RESULT_CALLBACKS.with(|callbacks| callbacks.borrow_mut().push(callback));

    let display_mime = mime.split(';').next().unwrap_or("").trim();
    if display_mime.starts_with("text/") {
        let text = String::from_utf8_lossy(bytes);
        if let Ok(display) = document.create_element("div") {
            display.set_text_content(Some(&text));
            let _ = wrapper.append_child(&display);
        }
    } else if let Some(url) = blob.as_ref().and_then(blob_object_url) {
        let display = create_element_wmt(mime, &url);
        let _ = wrapper.append_child(&display);
        ACTIVE_RESULT_OBJECT_URL.with(|active| *active.borrow_mut() = Some(url));
    } else if let Ok(error) = document.create_element("div") {
        error.set_text_content(Some("Could not create display blob"));
        let _ = wrapper.append_child(&error);
    }

    append_result_view(&wrapper);
}

pub(super) fn tar_entries(entries: &[(Vec<u8>, String, String)]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut builder = Builder::new(&mut out);
        for (bytes, _mime, path) in entries.iter() {
            let name = normalize_bzz_path(path);
            if name.is_empty() {
                continue;
            }
            let mut header = Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, name, bytes.as_slice())
                .ok()?;
        }
        builder.finish().ok()?;
    }
    Some(out)
}

pub(super) fn render_collection_download_button(entries: RenderedEntries, index: &str) {
    let document = interface_document();
    let button = match document.create_element("button") {
        Ok(button) => button,
        Err(_) => return,
    };
    let filename = format!("{}.tar", result_filename(index, "collection"));
    set_bracket_button_label(&button, &format!("Download {}", filename));
    let callback = Closure::<dyn FnMut(Event)>::new(move |_event| {
        if let Some(bytes) = tar_entries(&entries)
            && let Some(url) = blob_url(&bytes, "application/x-tar")
        {
            click_download_url(url, &filename);
        }
    });
    let _ = button.add_event_listener_with_callback("click", callback.as_ref().unchecked_ref());
    append_result_action(&button);
    RESULT_CALLBACKS.with(|callbacks| callbacks.borrow_mut().push(callback));
}

async fn bzz_view_request_is_current(
    weeb3: &InterfaceNode,
    progress_id: &str,
    view_generation: u64,
) -> bool {
    if crate::stream::result_view_request_is_current(view_generation) {
        return true;
    }

    weeb3
        .finish_progress(
            progress_id,
            "superseded",
            "a newer resource selection owns the result view",
            true,
        )
        .await;
    false
}

pub(super) async fn open_bzz_resource(weeb3: InterfaceNode, resource: String) {
    let view_generation = crate::stream::begin_result_view_request();
    let stream_files = stream_files_when_available();
    let progress_id = weeb3
        .start_progress("bzz", resource.clone(), "resolve", None, "resolving")
        .await;
    if !bzz_view_request_is_current(&weeb3, &progress_id, view_generation).await {
        return;
    }

    if let Some(metadata) = weeb3.resolve_bzz(resource.clone()).await {
        if !bzz_view_request_is_current(&weeb3, &progress_id, view_generation).await {
            return;
        }
        if stream_files {
            weeb3
                .update_progress(&progress_id, "stream", None, "checking stream support")
                .await;
            if !bzz_view_request_is_current(&weeb3, &progress_id, view_generation).await {
                return;
            }
            let streamed = crate::stream::try_render_streaming_player(
                weeb3.clone(),
                resource.clone(),
                metadata.clone(),
                view_generation,
            )
            .await;
            if !bzz_view_request_is_current(&weeb3, &progress_id, view_generation).await {
                return;
            }
            if streamed {
                weeb3
                    .finish_progress(&progress_id, "streaming", "stream player started", true)
                    .await;
                return;
            }
        }

        weeb3
            .update_progress(
                &progress_id,
                "retrieve",
                Some(0),
                format!("{} bytes", metadata.size),
            )
            .await;
        if !bzz_view_request_is_current(&weeb3, &progress_id, view_generation).await {
            return;
        }
        let rendered =
            render_resolved_asset(weeb3.clone(), &resource, metadata, view_generation).await;
        if !bzz_view_request_is_current(&weeb3, &progress_id, view_generation).await {
            return;
        }
        if rendered {
            weeb3
                .finish_progress(&progress_id, "complete", "displayed selected asset", true)
                .await;
            return;
        }
    }

    weeb3
        .update_progress(&progress_id, "retrieve", None, "legacy retrieve fallback")
        .await;
    if !bzz_view_request_is_current(&weeb3, &progress_id, view_generation).await {
        return;
    }
    let result = weeb3.acquire(resource).await;
    if !bzz_view_request_is_current(&weeb3, &progress_id, view_generation).await {
        return;
    }
    let (data, indx) = decode_resources(result);
    let ok = !data.is_empty();
    render_result(data, indx);
    weeb3
        .finish_progress(
            &progress_id,
            if ok { "complete" } else { "failed" },
            if ok {
                "displayed retrieved resource"
            } else {
                "resource not found"
            },
            ok,
        )
        .await;
}

pub(super) async fn render_resolved_asset(
    weeb3: InterfaceNode,
    resource: &str,
    metadata: BzzMetadata,
    view_generation: u64,
) -> bool {
    if !crate::stream::result_view_request_is_current(view_generation) {
        return true;
    }

    if should_render_canonical_bzz_frame(&metadata) {
        let service_worker_ready =
            service_worker_controls_bzz_requests(&weeb3, "bzz frame requests", || {
                crate::stream::result_view_request_is_current(view_generation)
            })
            .await;
        if !crate::stream::result_view_request_is_current(view_generation) {
            return true;
        }
        if !service_worker_ready {
            service_worker_missing();
            return true;
        }
        if let Some(url) = canonical_bzz_url(resource, &metadata.path, Some("index.html")) {
            let index_html = preload_canonical_bzz_frame(&weeb3, resource, &metadata).await;
            if !crate::stream::result_view_request_is_current(view_generation) {
                return true;
            }
            let Some(index_html) = index_html else {
                render_text_result("Could not retrieve website index");
                return true;
            };
            render_canonical_bzz_frame(weeb3.clone(), resource, &url, &metadata, &index_html);
            return true;
        }
    }

    if metadata.size == 0 {
        let path = metadata.path;
        render_result(vec![(vec![], metadata.mime, path.clone())], path);
        return true;
    }

    let end = metadata.size - 1;
    if let Some((bytes, metadata)) = weeb3.acquire_resolved_range(metadata, 0, end).await {
        if !crate::stream::result_view_request_is_current(view_generation) {
            return true;
        }
        let path = metadata.path;
        render_result(vec![(bytes, metadata.mime, path.clone())], path);
        return true;
    }

    if !crate::stream::result_view_request_is_current(view_generation) {
        return true;
    }
    false
}

async fn preload_canonical_bzz_frame(
    weeb3: &InterfaceNode,
    resource: &str,
    metadata: &BzzMetadata,
) -> Option<Vec<u8>> {
    if metadata.size == 0 {
        weeb3.interface_log(format!(
            "website index unavailable for {}; resolved target is empty",
            resource
        ));
        return None;
    }

    let progress_id = weeb3
        .start_progress(
            "bzz",
            resource.to_string(),
            "index",
            Some(0),
            "retrieving website index",
        )
        .await;
    weeb3.interface_log(format!(
        "website index retrieval started for {}; path {}, {} bytes",
        resource, metadata.path, metadata.size
    ));

    let retrieved = weeb3
        .acquire_resolved_range(metadata.clone(), 0, metadata.size - 1)
        .await;

    match retrieved {
        Some((bytes, _)) if bytes.len() == metadata.size as usize => {
            weeb3
                .finish_progress(&progress_id, "complete", "website index retrieved", true)
                .await;
            weeb3.interface_log(format!(
                "website index retrieved for {}; rendering iframe",
                resource
            ));
            Some(bytes)
        }
        Some((bytes, _)) => {
            weeb3.interface_log(format!(
                "website index retrieval failed for {}; received {} of {} bytes",
                resource,
                bytes.len(),
                metadata.size
            ));
            weeb3
                .finish_progress(&progress_id, "failed", "short website index", false)
                .await;
            None
        }
        None => {
            weeb3.interface_log(format!("website index retrieval failed for {}", resource));
            weeb3
                .finish_progress(&progress_id, "failed", "website index not retrieved", false)
                .await;
            None
        }
    }
}

pub(super) fn should_render_canonical_bzz_frame(metadata: &BzzMetadata) -> bool {
    let mime = metadata.mime.split(';').next().unwrap_or("").trim();
    mime == "text/html" || mime == "application/xhtml+xml"
}

pub(super) async fn download_bzz_resource(
    weeb3: InterfaceNode,
    resource: String,
    fallback_filename: String,
) {
    let progress_id = weeb3
        .start_progress(
            "download",
            resource.clone(),
            "retrieve",
            Some(0),
            "preparing download",
        )
        .await;
    let result = weeb3.acquire(resource).await;
    let (entries, _index) = decode_resources(result);

    if entries.is_empty() {
        weeb3
            .finish_progress(&progress_id, "failed", "resource not found", false)
            .await;
        render_text_result("Could not prepare download");
        return;
    }

    if entries.len() > 1 {
        weeb3
            .update_progress(
                &progress_id,
                "pack",
                Some(80),
                format!("{} files", entries.len()),
            )
            .await;
        if let Some(bytes) = tar_entries(&entries)
            && let Some(url) = blob_url(&bytes, "application/x-tar")
        {
            click_download_url(url, &fallback_filename);
            weeb3
                .finish_progress(
                    &progress_id,
                    "complete",
                    format!("{} bytes", bytes.len()),
                    true,
                )
                .await;
            return;
        }

        weeb3
            .finish_progress(&progress_id, "failed", "tar creation failed", false)
            .await;
        render_text_result("Could not create collection download");
        return;
    }

    let (bytes, mime, path) = entries.into_iter().next().unwrap();
    let filename = result_filename(&path, &fallback_filename);
    match blob_url(&bytes, &mime) {
        Some(url) => {
            click_download_url(url, &filename);
            weeb3
                .finish_progress(
                    &progress_id,
                    "complete",
                    format!("{} bytes", bytes.len()),
                    true,
                )
                .await;
        }
        None => {
            weeb3
                .finish_progress(&progress_id, "failed", "blob creation failed", false)
                .await;
            render_text_result("Could not create file download");
        }
    }
}

pub(super) fn render_canonical_bzz_frame(
    weeb3: InterfaceNode,
    resource: &str,
    url: &str,
    metadata: &BzzMetadata,
    index_html: &[u8],
) {
    let document = interface_document();

    let wrapper = match document.create_element("div") {
        Ok(wrapper) => wrapper,
        Err(_) => return,
    };

    let download = match document.create_element("button") {
        Ok(download) => download,
        Err(_) => return,
    };
    let filename = if metadata.path.is_empty() {
        "index.html"
    } else {
        metadata.path.as_str()
    };
    let download_filename = if metadata.target_count > 1 {
        format!(
            "{}.tar",
            bzz_reference_hex(resource).unwrap_or_else(|| result_filename(filename, "collection"))
        )
    } else {
        result_filename(filename, "download")
    };
    set_bracket_button_label(&download, &format!("Download {}", download_filename));
    let frame_url = url.to_string();
    let resource = resource.to_string();
    let callback = Closure::<dyn FnMut(Event)>::new(move |_event| {
        let node = weeb3.clone();
        let resource = resource.clone();
        let filename = download_filename.clone();
        spawn_local(async move {
            download_bzz_resource(node, resource, filename).await;
        });
    });
    let _ = download.add_event_listener_with_callback("click", callback.as_ref().unchecked_ref());
    let frame = match document.create_element("iframe") {
        Ok(frame) => frame,
        Err(_) => return,
    };
    let _ = frame.set_attribute("srcdoc", &srcdoc_with_base(index_html, &frame_url));
    let _ = frame.set_attribute("data-src", &frame_url);
    let _ = frame.set_attribute("width", "100%");
    let _ = frame.set_attribute("height", "640");
    let _ = frame.set_attribute("loading", "eager");
    let _ = frame.set_attribute("referrerpolicy", "same-origin");

    let _ = wrapper.append_child(&frame);
    replace_result_view(&wrapper);
    append_result_action(&download);
    RESULT_CALLBACKS.with(|callbacks| callbacks.borrow_mut().push(callback));
}

fn srcdoc_with_base(bytes: &[u8], canonical_url: &str) -> String {
    let html = String::from_utf8_lossy(bytes);
    let base = format!(r#"<base href="{}">"#, srcdoc_base_url(canonical_url));

    if let Some(head_end) = find_ascii_case_insensitive(&html, "<head>") {
        let insert = head_end + "<head>".len();
        format!("{}{}{}", &html[..insert], base, &html[insert..])
    } else if let Some(head_end) = find_ascii_case_insensitive(&html, "<head ") {
        match html[head_end..].find('>') {
            Some(offset) => {
                let insert = head_end + offset + 1;
                format!("{}{}{}", &html[..insert], base, &html[insert..])
            }
            None => format!("{}{}", base, html),
        }
    } else {
        format!("{}{}", base, html)
    }
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|candidate| candidate.eq_ignore_ascii_case(needle.as_bytes()))
}

fn srcdoc_base_url(canonical_url: &str) -> &str {
    match canonical_url.rfind('/') {
        Some(index) => &canonical_url[..=index],
        None => canonical_url,
    }
}

pub(super) fn stream_files_when_available() -> bool {
    let document = interface_document();
    document
        .get_element_by_id("streamFilesWhenAvailable")
        .and_then(|setting| setting.dyn_into::<HtmlInputElement>().ok())
        .is_none_or(|setting| setting.checked())
}

pub(super) fn update_transfer_pause_button(paused: bool) {
    let document = interface_document();
    let button = match document.get_element_by_id("transferPauseToggle") {
        Some(button) => button,
        None => return,
    };

    let button = button
        .dyn_ref::<HtmlButtonElement>()
        .expect("#transferPauseToggle should be a HtmlButtonElement");

    let label = if paused {
        " Resume retrieve / push "
    } else {
        " Pause retrieve / push "
    };
    set_bracket_button_label(button.unchecked_ref::<Element>(), label);
}

pub(super) fn create_element_wmt(mime: &str, blob_url: &str) -> Element {
    let document = interface_document();
    if mime == "undefined" {
        let element = document.create_element("div").unwrap();
        element.set_text_content(Some("Not found"));
        return element;
    }

    let element = document.create_element("embed").unwrap();
    let _ = element.set_attribute("src", blob_url);
    let _ = element.set_attribute("type", mime);

    element
}

pub(crate) fn service_worker_missing() {
    let message = "The weeb-3 Service Worker is unavailable or did not become ready. Browser-routed Swarm resources require the configured /weeb-3/ scope in a secure context with a trusted HTTPS certificate.";
    web_sys::console::warn_1(&JsValue::from_str(message));

    // npm consumers do not necessarily mount the built-in interface. A Service
    // Worker diagnostic must not become a Wasm panic when #resultField is absent.
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(result_field) = document
        .get_element_by_id("resultField")
        .and_then(|element| element.dyn_into::<HtmlElement>().ok())
    else {
        return;
    };
    if SERVICE_WORKER_MISSING_VISIBLE.with(Cell::get) {
        return;
    }
    let Ok(error) = document.create_element("div") else {
        return;
    };
    error.set_text_content(Some(message));
    if result_field.prepend_with_node_1(&error).is_ok() {
        SERVICE_WORKER_MISSING_VISIBLE.with(|visible| visible.set(true));
    }
}

pub(super) fn render_text_result(message: &str) {
    let document = interface_document();
    let Ok(result) = document.create_element("div") else {
        return;
    };
    result.set_text_content(Some(message));
    replace_result_view(&result);
}

pub(super) fn render_log_messages(messages: &[String]) {
    if messages.is_empty() {
        return;
    }
    let document = interface_document();
    let fragment = document.create_document_fragment();
    for message in messages.iter().rev() {
        let element = document.create_element("div").unwrap();
        element.set_text_content(Some(message));
        let _ = fragment.append_child(&element);
    }
    let logs = document
        .get_element_by_id("logsField")
        .expect("#logsField should exist");
    let _ = logs.prepend_with_node_1(&fragment);
    while logs.child_element_count() > crate::LOG_DOM_RETAINED {
        let Some(oldest) = logs.last_element_child() else {
            break;
        };
        oldest.remove();
    }
}

pub(super) fn render_progress_rows(rows: Vec<crate::events::ProgressRow>) {
    let document = interface_document();
    let Some(progress_rows) = ensure_progress_child(&document, "progressRows", "pre") else {
        return;
    };

    let lines: Vec<String> = rows
        .into_iter()
        .map(|row| {
            let percent = row
                .percent
                .map(|percent| format!("{}%", percent))
                .unwrap_or_else(|| "...".to_string());
            let status = if row.done {
                if row.ok || row.phase.starts_with("complete") {
                    "done"
                } else {
                    "failed"
                }
            } else {
                "running"
            };

            format!(
                "{} {} [{}] {} {} {}",
                row.kind, row.subject, status, row.phase, percent, row.detail
            )
        })
        .collect();

    progress_rows.set_text_content(Some(&lines.join("\n")));
}

pub(super) fn render_result(data: Vec<(Vec<u8>, String, String)>, indx: String) {
    clear_result_view();
    if data.is_empty() {
        let new_element = create_element_wmt("undefined", "");
        append_result_view(&new_element);
    } else {
        let selected = data
            .iter()
            .position(|(_, _, path)| *path == indx)
            .unwrap_or(0);
        render_single_result_with_download(&data[selected]);
        if data.len() > 1 {
            render_collection_download_button(Rc::new(data), &indx);
        }
    }
}

fn bootnode_setting(id: &str) -> String {
    let document = interface_document();
    let bootnode_element = document
        .get_element_by_id(id)
        .unwrap_or_else(|| panic!("#{id} should exist"));
    let bootnode_input = bootnode_element
        .dyn_ref::<HtmlInputElement>()
        .unwrap_or_else(|| panic!("#{id} should be a HtmlInputElement"));
    bootnode_input.value()
}

pub(super) fn service_worker_container() -> Option<web_sys::ServiceWorkerContainer> {
    let window = web_sys::window()?;
    let is_secure_context =
        js_sys::Reflect::get(window.as_ref(), &JsValue::from_str("isSecureContext"))
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
    if !is_secure_context {
        return None;
    }

    let navigator = window.navigator();
    let service_worker =
        js_sys::Reflect::get(navigator.as_ref(), &JsValue::from_str("serviceWorker")).ok()?;
    if service_worker.is_null() || service_worker.is_undefined() {
        return None;
    }

    service_worker
        .dyn_into::<web_sys::ServiceWorkerContainer>()
        .ok()
}

async fn service_worker_registration(
    service0: &web_sys::ServiceWorkerContainer,
) -> Option<ServiceWorkerRegistration> {
    let registration = async_std::future::timeout(
        Duration::from_secs(10),
        JsFuture::from(service0.get_registration()),
    )
    .await
    .ok()?
    .ok()?;
    registration.dyn_into::<ServiceWorkerRegistration>().ok()
}

fn configured_service_worker_urls() -> Option<(String, String)> {
    let window = web_sys::window()?;
    let page_url = window.location().href().ok()?;
    let absolute = |path| {
        web_sys::Url::new_with_base(path, &page_url)
            .ok()
            .map(|url| url.href())
    };
    Some((
        absolute(STREAMING_SERVICE_WORKER_URL)?,
        absolute(STREAMING_SERVICE_WORKER_SCOPE)?,
    ))
}

fn expected_service_worker_registration(
    registration: &ServiceWorkerRegistration,
    expected_worker_url: &str,
    expected_scope_url: &str,
) -> bool {
    if registration.scope() != expected_scope_url {
        warn_about_worker_conflict(
            &format!("scope {}", registration.scope()),
            expected_scope_url,
        );
        return false;
    }

    for worker in [
        registration.active(),
        registration.waiting(),
        registration.installing(),
    ]
    .into_iter()
    .flatten()
    {
        if worker.script_url() != expected_worker_url {
            warn_about_worker_conflict(&worker.script_url(), expected_scope_url);
            return false;
        }
    }
    true
}

async fn claim_service_worker_registration(
    registration: &ServiceWorkerRegistration,
    expected_worker_url: &str,
    expected_scope_url: &str,
) -> Result<Option<web_sys::ServiceWorker>, ()> {
    if !expected_service_worker_registration(registration, expected_worker_url, expected_scope_url)
    {
        return Err(());
    }
    let Some(active) = registration.active() else {
        return Ok(None);
    };
    Ok(claim_exact_service_worker(&active).await)
}

fn warn_about_worker_conflict(worker: &str, expected_scope_url: &str) {
    web_sys::console::warn_1(&JsValue::from_str(&format!(
        "weeb-3 left the existing Service Worker in place ({worker}); integrate the weeb-3 \
         forwarding protocol or remove the conflicting registration before using scope \
         {expected_scope_url}"
    )));
}

pub async fn get_service_worker() -> Option<web_sys::ServiceWorker> {
    let Some(service0) = service_worker_container() else {
        service_worker_missing();
        return None;
    };
    let _setup_guard = SERVICE_WORKER_SETUP_LOCK.lock().await;
    get_service_worker_locked(&service0).await
}

fn start_service_worker_setup_if_idle() -> bool {
    let Some(service0) = service_worker_container() else {
        service_worker_missing();
        return false;
    };
    let Some(setup_guard) = SERVICE_WORKER_SETUP_LOCK.try_lock() else {
        // A setup attempt already owns registration/update. Readiness keeps
        // polling the exact implementation and retries after the lock is free.
        return false;
    };
    spawn_local(async move {
        let _setup_guard = setup_guard;
        let _ = get_service_worker_locked(&service0).await;
    });
    true
}

async fn get_service_worker_locked(
    service0: &web_sys::ServiceWorkerContainer,
) -> Option<web_sys::ServiceWorker> {
    let (expected_worker_url, expected_scope_url) = configured_service_worker_urls()?;
    // Never replace an unrelated host application's worker. It may opt into
    // the exact forwarding implementation without using weeb-3's script URL.
    if let Some(controller) = controlled_service_worker()
        && controller.script_url() != expected_worker_url
    {
        if service_worker_forwarder_ready_with_timeout(1_500).await {
            return Some(controller);
        }
        warn_about_worker_conflict(&controller.script_url(), &expected_scope_url);
        return None;
    }
    if let Some(registration) = service_worker_registration(service0).await {
        if !expected_service_worker_registration(
            &registration,
            &expected_worker_url,
            &expected_scope_url,
        ) {
            return None;
        }
        // Check the no-cache worker script before accepting a same-URL
        // controller. Protocol alone cannot distinguish an older forwarder.
        if let Ok(update) = registration.update() {
            let _ =
                async_std::future::timeout(Duration::from_secs(10), JsFuture::from(update)).await;
        }
        if let Some(controller) = claim_service_worker_registration(
            &registration,
            &expected_worker_url,
            &expected_scope_url,
        )
        .await
        .ok()?
        {
            return Some(controller);
        }
    }

    let registration_options = RegistrationOptions::new();
    registration_options.set_scope(STREAMING_SERVICE_WORKER_SCOPE);
    let _ = Reflect::set(
        registration_options.as_ref(),
        &JsValue::from_str("updateViaCache"),
        &JsValue::from_str("none"),
    );
    match async_std::future::timeout(
        Duration::from_secs(10),
        JsFuture::from(
            service0.register_with_options(STREAMING_SERVICE_WORKER_URL, &registration_options),
        ),
    )
    .await
    {
        Ok(Ok(registration)) => {
            if let Ok(registration) = registration.dyn_into::<ServiceWorkerRegistration>()
                && let Some(controller) = claim_service_worker_registration(
                    &registration,
                    &expected_worker_url,
                    &expected_scope_url,
                )
                .await
                .ok()?
            {
                return Some(controller);
            }

            if let Ok(ready) = service0.ready()
                && let Ok(Ok(registration)) =
                    async_std::future::timeout(Duration::from_secs(10), JsFuture::from(ready)).await
                && let Ok(registration) = registration.dyn_into::<ServiceWorkerRegistration>()
                && let Some(controller) = claim_service_worker_registration(
                    &registration,
                    &expected_worker_url,
                    &expected_scope_url,
                )
                .await
                .ok()?
            {
                return Some(controller);
            }
        }
        Ok(Err(err)) => {
            web_sys::console::warn_1(&err);
        }
        Err(_) => web_sys::console::warn_1(&JsValue::from_str(
            "timed out while registering the weeb-3 Service Worker",
        )),
    }

    let registration = service_worker_registration(service0).await?;
    claim_service_worker_registration(&registration, &expected_worker_url, &expected_scope_url)
        .await
        .ok()
        .flatten()
}

fn controlled_service_worker() -> Option<web_sys::ServiceWorker> {
    service_worker_container()?.controller()
}

async fn service_worker_forwarder_ready() -> bool {
    for _ in 0..3 {
        if service_worker_forwarder_ready_with_timeout(500).await {
            return true;
        }
    }
    false
}

async fn service_worker_forwarder_ready_with_timeout(timeout_ms: u64) -> bool {
    let Some(controller) = controlled_service_worker() else {
        return false;
    };
    service_worker_protocol_request(&controller, "WEEB3_PING", "WEEB3_PONG", timeout_ms).await
}

async fn request_service_worker_claim(worker: &web_sys::ServiceWorker) -> bool {
    service_worker_protocol_request(worker, "WEEB3_CLAIM", "WEEB3_CLAIMED", 1_500).await
}

async fn claim_exact_service_worker(
    worker: &web_sys::ServiceWorker,
) -> Option<web_sys::ServiceWorker> {
    if !request_service_worker_claim(worker).await || !service_worker_forwarder_ready().await {
        return None;
    }
    controlled_service_worker()
}

struct ServiceWorkerProtocolPort {
    port: MessagePort,
    _callback: Closure<dyn FnMut(MessageEvent)>,
}

impl Drop for ServiceWorkerProtocolPort {
    fn drop(&mut self) {
        self.port.set_onmessage(None);
        self.port.close();
    }
}

async fn service_worker_protocol_request(
    worker: &web_sys::ServiceWorker,
    request_type: &str,
    response_type: &str,
    timeout_ms: u64,
) -> bool {
    let Ok(channel) = MessageChannel::new() else {
        return false;
    };
    let (sender, receiver) = async_std::channel::bounded::<bool>(1);
    let expected_scope = STREAMING_SERVICE_WORKER_SCOPE;
    let expected_marker = SERVICE_WORKER_MARKER;
    let expected_response_type = response_type.to_string();
    let callback = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let data = event.data();
        let matches = string_property(&data, "type")
            .is_some_and(|value| value == expected_response_type)
            && number_property(&data, "protocol") == Some(SERVICE_WORKER_PROTOCOL)
            && string_property(&data, "scope").is_some_and(|scope| scope == expected_scope)
            && string_property(&data, "marker").is_some_and(|marker| marker == expected_marker);
        let _ = sender.try_send(matches);
    });
    let protocol_port = ServiceWorkerProtocolPort {
        port: channel.port1(),
        _callback: callback,
    };
    protocol_port
        .port
        .set_onmessage(Some(protocol_port._callback.as_ref().unchecked_ref()));
    protocol_port.port.start();

    let ping = Object::new();
    set_js(&ping, "type", JsValue::from_str(request_type));
    set_js(
        &ping,
        "protocol",
        JsValue::from_f64(SERVICE_WORKER_PROTOCOL),
    );
    let transfer = Array::new();
    transfer.push(&channel.port2());
    if worker
        .post_message_with_transferable(&ping, &transfer)
        .is_err()
    {
        return false;
    }

    async_std::future::timeout(Duration::from_millis(timeout_ms), receiver.recv())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false)
}

pub(crate) fn service_worker_scope_protocol_error(purpose: &str) -> String {
    format!(
        "Service Worker protocol {} implementation {} did not become ready for {}: configured worker {} did not \
         claim scope {} and answer WEEB3_PING within {} ms.",
        SERVICE_WORKER_PROTOCOL as u8,
        SERVICE_WORKER_MARKER,
        purpose,
        STREAMING_SERVICE_WORKER_URL,
        STREAMING_SERVICE_WORKER_SCOPE,
        SERVICE_WORKER_CONTROL_TOTAL_TIMEOUT_MS,
    )
}

async fn wait_for_service_worker_control(
    weeb3: &InterfaceNode,
    purpose: &str,
    still_needed: &impl Fn() -> bool,
) -> bool {
    if service_worker_container().is_none() {
        weeb3.interface_log(format!("service worker unavailable for {}", purpose));
        return false;
    }

    weeb3.interface_log(format!("service worker activating for {}", purpose));
    // Serialize behind any startup registration so a stale same-URL controller
    // cannot satisfy readiness before its registration has been updated.
    let _ = get_service_worker().await;
    if !still_needed() {
        return false;
    }
    if service_worker_forwarder_ready().await {
        weeb3.interface_log(format!("service worker controls {}", purpose));
        return true;
    }

    let mut next_setup_retry_ms = js_sys::Date::now() + SERVICE_WORKER_SETUP_RETRY_MS;
    let mut activation_retry_logged = false;
    loop {
        if !still_needed() {
            return false;
        }

        let now = js_sys::Date::now();
        if (!next_setup_retry_ms.is_finite()
            || next_setup_retry_ms <= 0.0
            || now < next_setup_retry_ms - SERVICE_WORKER_CONTROL_TOTAL_TIMEOUT_MS as f64
            || now >= next_setup_retry_ms)
            && start_service_worker_setup_if_idle()
        {
            // The setup lock prevents overlap. Retrying after it is released lets a transient
            // registration/update failure recover even when a stale same-URL controller exists.
            next_setup_retry_ms = now + SERVICE_WORKER_SETUP_RETRY_MS;
        }
        if controlled_service_worker().is_none() {
            if !activation_retry_logged {
                activation_retry_logged = true;
                weeb3.interface_log(format!(
                    "service worker still activating for {}; retrying without a reload",
                    purpose
                ));
            }
            async_std::task::sleep(Duration::from_millis(100)).await;
            continue;
        }

        if service_worker_forwarder_ready_with_timeout(500).await {
            weeb3.interface_log(format!("service worker controls {}", purpose));
            return true;
        }
        async_std::task::sleep(Duration::from_millis(100)).await;
    }
}

pub(crate) async fn service_worker_controls_bzz_requests(
    weeb3: &InterfaceNode,
    purpose: &str,
    still_needed: impl Fn() -> bool,
) -> bool {
    if !still_needed() {
        return false;
    }

    let ready = async_std::future::timeout(
        Duration::from_millis(SERVICE_WORKER_CONTROL_TOTAL_TIMEOUT_MS),
        wait_for_service_worker_control(weeb3, purpose, &still_needed),
    )
    .await
    .unwrap_or(false);
    if ready || !still_needed() {
        return ready;
    }
    if service_worker_container().is_none() {
        return false;
    }

    weeb3.interface_log(service_worker_scope_protocol_error(purpose));
    false
}
