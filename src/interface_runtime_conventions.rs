use super::*;

pub(super) async fn check_upload_prerequisites(weeb3: Arc<Weeb3>) {
    let progress_id = weeb3
        .start_progress(
            "upload-prereq",
            "wallet and batch",
            "wallet",
            None,
            "connecting wallet",
        )
        .await;

    match collect_upload_prerequisites().await {
        Ok(message) => {
            render_text_result(&message);
            weeb3.interface_log(message);
            weeb3
                .finish_progress(&progress_id, "complete", "prerequisites checked", true)
                .await;
        }
        Err(error) => {
            let message = format!("Upload prerequisites failed: {}", error);
            render_text_result(&message);
            weeb3.interface_log(message);
            weeb3
                .finish_progress(&progress_id, "failed", "prerequisite check failed", false)
                .await;
        }
    }
}

pub(super) async fn collect_upload_prerequisites() -> Result<String, String> {
    let profile = current_network_profile();
    let (batch_depth, validity_days) = read_batch_request_settings()?;
    let payer = connect_wallet_address().await?;

    let secure_state = secure_batch_state_for_wallet(payer.as_bytes())
        .await
        .ok_or_else(|| "could not check weeb-3-secure for the connected wallet".to_string())?;

    let w3 = crate::on_chain::web3().map_err(|e| format!("provider init failed: {:?}", e))?;
    let chain_id = w3
        .eth()
        .chain_id()
        .await
        .map_err(|e| format!("chain id check failed: {:?}", e))?;

    let mut lines = vec![
        "Upload prerequisites".to_string(),
        format!(
            "network: {:?}, swarm id {}, wallet chain {}",
            profile.mode, profile.swarm_network_id, profile.wallet_chain_id
        ),
        format!("wallet: 0x{}", hex::encode(payer.as_bytes())),
        format!("wallet chain: {}", chain_id),
    ];

    if chain_id != U256::from(profile.wallet_chain_id) {
        lines.push(format!(
            "state: wrong wallet network, expected chain {}",
            profile.wallet_chain_id
        ));
        return Ok(lines.join("\n"));
    }

    let postage = postage_contract(&w3)
        .await
        .map_err(|e| format!("postage contract failed: {:?}", e))?;
    let token = token_contract(&w3)
        .await
        .map_err(|e| format!("token contract failed: {:?}", e))?;
    let lp = last_price(&postage)
        .await
        .map_err(|e| format!("last price failed: {:?}", e))?;

    if secure_state.usable() {
        let remaining = get_batch_validity(secure_state.batch_id.clone()).await;
        let day_price = lp * U256::from(7200u64);
        let days = if day_price.is_zero() {
            U256::from(0)
        } else {
            remaining / day_price
        };
        lines.push(format!(
            "batch: usable, id 0x{}, bucket limit {}, status {}, about {} days remaining",
            hex::encode(&secure_state.batch_id),
            secure_state.batch_bucket_limit,
            secure_state.batch_validity_status,
            days
        ));
    } else {
        lines.push(format!(
            "batch: not usable, status {}, id length {}",
            secure_state.batch_validity_status,
            secure_state.batch_id.len()
        ));
    }

    let initial_per_chunk = compute_initial_balance_per_chunk(lp, validity_days);
    let required_bzz = initial_per_chunk * chunk_count_for_depth(batch_depth);
    let token_balance: U256 = token
        .query("balanceOf", (payer,), None, Options::default(), None)
        .await
        .map_err(|e| format!("{} balance check failed: {:?}", profile.bzz_symbol, e))?;
    let base_balance = w3
        .eth()
        .balance(payer, None)
        .await
        .map_err(|e| format!("{} balance check failed: {:?}", profile.base_symbol, e))?;

    lines.push(format!(
        "requested batch: depth {}, validity {} days",
        batch_depth, validity_days
    ));
    lines.push(format!(
        "{}: balance {}, required {}, enough {}",
        profile.bzz_symbol,
        token_balance,
        required_bzz,
        token_balance >= required_bzz
    ));
    lines.push(format!(
        "{}: balance {}, nonzero for gas {}",
        profile.base_symbol,
        base_balance,
        !base_balance.is_zero()
    ));

    Ok(lines.join("\n"))
}

pub(super) fn current_network_profile() -> crate::network_profile::NetworkProfile {
    let document = web_sys::window().unwrap().document().unwrap();
    let network_id = document
        .get_element_by_id("networkIDSettings")
        .and_then(|el| el.dyn_into::<HtmlInputElement>().ok())
        .and_then(|input| input.value().parse::<u64>().ok())
        .unwrap_or(10);

    profile_for_swarm_network_id(network_id)
        .unwrap_or_else(|| profile_for_mode(NetworkMode::Testnet))
}

pub(super) fn read_batch_request_settings() -> Result<(u8, u64), String> {
    let document = web_sys::window().unwrap().document().unwrap();
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
    let window = web_sys::window().ok_or_else(|| "window is not available".to_string())?;
    let func = js_sys::Reflect::get(&window, &JsValue::from_str("weeb3EnsureEip1193"))
        .map_err(|_| "wallet connector is not available".to_string())?;
    let func = func
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "wallet connector is not callable".to_string())?;
    let project_id = JsValue::from_str("64c5f91181ce0a3192a783346a475d23");
    let profile = current_network_profile();
    let args = js_sys::Array::new();
    args.push(&project_id);
    args.push(&JsValue::from_f64(profile.wallet_chain_id as f64));
    let promise = func
        .apply(&JsValue::NULL, &args)
        .map_err(|_| "wallet connector call failed".to_string())?
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| "wallet connector did not return a promise".to_string())?;
    let obj = JsFuture::from(promise)
        .await
        .map_err(|_| "wallet connection failed".to_string())?;

    let ok = js_sys::Reflect::get(&obj, &"ok".into())
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !ok {
        let error = js_sys::Reflect::get(&obj, &"error".into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "wallet connection failed".to_string());
        return Err(error);
    }

    let accounts = js_sys::Reflect::get(&obj, &"accounts".into())
        .map_err(|_| "wallet account list missing".to_string())?;
    let first = js_sys::Array::from(&accounts).get(0);
    let address = first
        .as_string()
        .ok_or_else(|| "connected wallet did not return an account".to_string())?;

    Address::from_str(&address).map_err(|_| "connected account is not an address".to_string())
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

pub(super) fn install_network_profile_toggle(weeb3: Arc<Weeb3>) {
    let document = web_sys::window().unwrap().document().unwrap();
    update_network_mode_toggle(current_network_profile().mode);

    let Some(button) = document.get_element_by_id("networkModeToggle") else {
        return;
    };

    let callback =
        wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
            let weeb300 = weeb3.clone();
            let mode = match current_network_profile().mode {
                NetworkMode::Testnet => NetworkMode::Mainnet,
                NetworkMode::Mainnet => NetworkMode::Testnet,
            };
            let profile = profile_for_mode(mode);
            set_network_profile_inputs(mode);
            let apply_generation = next_network_apply_generation();
            let network_id = profile.swarm_network_id.to_string();
            weeb300.interface_log(format!(
                "Network mode switched to {:?} chain {}",
                profile.mode, profile.wallet_chain_id
            ));
            spawn_local(async move {
                apply_network_settings_and_connect(weeb300, apply_generation, network_id).await;
            });
        });

    if let Some(button) = button.dyn_ref::<HtmlButtonElement>() {
        button.set_onclick(Some(callback.as_ref().unchecked_ref()));
        callback.forget();
    }
}

pub(super) fn set_network_profile_inputs(mode: NetworkMode) {
    let profile = profile_for_mode(mode);
    let document = web_sys::window().unwrap().document().unwrap();
    update_network_mode_toggle(profile.mode);

    if let Some(network_id_input) = document.get_element_by_id("networkIDSettings") {
        if let Some(network_id_input) = network_id_input.dyn_ref::<HtmlInputElement>() {
            network_id_input.set_value(&profile.swarm_network_id.to_string());
        }
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
    let document = web_sys::window().unwrap().document().unwrap();
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

pub(super) fn prepend_progress_node(node: &Element) {
    let document = web_sys::window().unwrap().document().unwrap();
    let Some(actions) = ensure_progress_child(&document, "progressActions", "div") else {
        return;
    };
    let _ = actions.prepend_with_node_1(node);
}

pub(super) async fn apply_network_settings_and_connect(
    weeb3: Arc<Weeb3>,
    apply_generation: u64,
    network_id: String,
) {
    if !is_current_network_apply_generation(apply_generation) {
        return;
    }

    if network_id.trim().is_empty() {
        weeb3.interface_log("Network id is empty; not reconnecting".to_string());
        return;
    }

    if !weeb3.set_network_id(network_id.clone()).await {
        weeb3.interface_log(format!("Network id switch failed: {}", network_id));
        return;
    }

    if !is_current_network_apply_generation(apply_generation) {
        return;
    }

    update_network_mode_toggle(current_network_profile().mode);
    connect_all_bootnode_settings(weeb3, apply_generation).await;
}

pub(super) fn current_network_id_input() -> String {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("networkIDSettings"))
        .and_then(|element| element.dyn_into::<HtmlInputElement>().ok())
        .map(|input| input.value())
        .unwrap_or_else(|| "10".to_string())
}

pub(super) async fn connect_all_bootnode_settings(weeb3: Arc<Weeb3>, apply_generation: u64) {
    if !is_current_network_apply_generation(apply_generation) {
        return;
    }

    let futures = BOOTNODE_INPUT_IDS.into_iter().map(|element_id| {
        let weeb300 = weeb3.clone();
        async move {
            connect_bootnode_setting(weeb300, element_id, apply_generation).await;
        }
    });

    join_all(futures).await;
}

pub(super) async fn connect_bootnode_setting(
    weeb3: Arc<Weeb3>,
    element_id: &'static str,
    apply_generation: u64,
) {
    if !is_current_network_apply_generation(apply_generation) {
        return;
    }

    let (bna, nid) = parsebootconnect(element_id.to_string());
    if !is_current_network_apply_generation(apply_generation) {
        return;
    }

    if bna.trim().is_empty() {
        return;
    }
    if !is_browser_dialable_underlay(&bna) {
        weeb3.interface_log(format!(
            "Skipped non-browser bootnode for network {}: {}",
            nid, bna
        ));
        return;
    }
    if !is_current_network_apply_generation(apply_generation) {
        return;
    }
    let _ = weeb3.change_bootnode_address(bna, nid, true).await;
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

pub(super) async fn open_resource_input(weeb3: Arc<Weeb3>, input: String) {
    if let Some(route) = parse_resource_route(&input) {
        open_resource(weeb3, route).await;
    } else {
        open_bzz_resource(weeb3, input).await;
    }
}

pub(super) async fn open_resource(weeb3: Arc<Weeb3>, route: ResourceRoute) {
    match route {
        ResourceRoute::Bzz(resource) => open_bzz_resource(weeb3, resource).await,
        ResourceRoute::Bytes(reference) => {
            let bytes = weeb3.retrieve_bytes(reference.clone()).await;
            download_raw_bytes(bytes, reference, "bytes").await;
        }
        ResourceRoute::Chunks(reference) => {
            let bytes = weeb3.retrieve_chunk_bytes(reference.clone()).await;
            download_raw_bytes(bytes, reference, "chunk").await;
        }
    }
}

pub(super) async fn download_raw_bytes(bytes: Vec<u8>, filename: String, label: &str) {
    if bytes.is_empty() {
        render_text_result(&format!("Could not retrieve {} {}", label, filename));
        return;
    }

    let props = BlobPropertyBag::new();
    props.set_type("application/octet-stream");

    let data: Uint8Array = JsValue::from(bytes.clone()).into();
    let parts = Array::new();
    parts.push(&data);

    let blob = match Blob::new_with_u8_array_sequence_and_options(&parts, &props) {
        Ok(blob) => blob,
        Err(_) => {
            render_text_result(&format!(
                "Could not create download for {} {}",
                label, filename
            ));
            return;
        }
    };

    let blob_url = match web_sys::Url::create_object_url_with_blob(&blob) {
        Ok(url) => url,
        Err(_) => {
            render_text_result(&format!(
                "Could not create download URL for {} {}",
                label, filename
            ));
            return;
        }
    };

    let document = web_sys::window().unwrap().document().unwrap();
    let anchor = match document.create_element("a") {
        Ok(anchor) => anchor,
        Err(_) => return,
    };
    let _ = anchor.set_attribute("href", &blob_url);
    let _ = anchor.set_attribute("download", &filename);
    let _ = anchor.set_attribute("style", "display:none");

    if let Some(body) = document.body() {
        let _ = body.append_child(&anchor);
    }

    if let Some(anchor) = anchor.dyn_ref::<HtmlElement>() {
        anchor.click();
    }

    render_text_result(&format!(
        "Started {} download {} ({} bytes)",
        label,
        filename,
        bytes.len()
    ));
}

pub(super) fn click_download_url(url: &str, filename: &str) {
    let document = match web_sys::window().and_then(|window| window.document()) {
        Some(document) => document,
        None => return,
    };
    let anchor = match document.create_element("a") {
        Ok(anchor) => anchor,
        Err(_) => return,
    };
    let _ = anchor.set_attribute("href", url);
    let _ = anchor.set_attribute("download", filename);
    let _ = anchor.set_attribute("style", "display:none");

    if let Some(body) = document.body() {
        let _ = body.append_child(&anchor);
    }

    if let Some(anchor) = anchor.dyn_ref::<HtmlElement>() {
        anchor.click();
    }
}

pub(super) fn blob_url(bytes: &[u8], mime: &str) -> Option<String> {
    let props = BlobPropertyBag::new();
    props.set_type(mime);

    let data: Uint8Array = JsValue::from(bytes.to_vec()).into();
    let parts = Array::new();
    parts.push(&data);

    let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &props).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

pub(super) fn result_filename(path: &str, fallback: &str) -> String {
    let path = normalize_bzz_path(path);
    if path.is_empty() || path == "not found" || path.starts_with("unknown") {
        fallback.to_string()
    } else {
        path.rsplit('/').next().unwrap_or(fallback).to_string()
    }
}

pub(super) fn prepend_result_node(node: &Element) {
    let document = web_sys::window().unwrap().document().unwrap();
    let _ = document
        .get_element_by_id("resultField")
        .expect("#resultField should exist")
        .dyn_ref::<HtmlElement>()
        .unwrap()
        .prepend_with_node_1(node);
}

pub(super) fn render_single_result_with_download(bytes: Vec<u8>, mime: String, path: String) {
    let document = web_sys::window().unwrap().document().unwrap();
    let wrapper = match document.create_element("div") {
        Ok(wrapper) => wrapper,
        Err(_) => return,
    };
    let button = match document.create_element("button") {
        Ok(button) => button,
        Err(_) => return,
    };

    let filename = result_filename(&path, "download");
    set_bracket_button_label(&button, &format!("Download {}", filename));
    let download_bytes = bytes.clone();
    let download_mime = mime.clone();
    let download_filename = filename.clone();
    let callback = wasm_bindgen::closure::Closure::<dyn FnMut(Event)>::new(move |_event| {
        if let Some(url) = blob_url(&download_bytes, &download_mime) {
            click_download_url(&url, &download_filename);
        }
    });
    let _ = button.add_event_listener_with_callback("click", callback.as_ref().unchecked_ref());
    callback.forget();
    prepend_progress_node(&button);

    let display_mime = mime.split(';').next().unwrap_or("").trim();
    if display_mime.starts_with("text/") {
        let text = String::from_utf8_lossy(&bytes);
        if let Ok(display) = document.create_element("div") {
            display.set_text_content(Some(&text));
            let _ = wrapper.append_child(&display);
        }
    } else if let Some(url) = blob_url(&bytes, &mime) {
        let display = create_element_wmt(mime, url);
        let _ = wrapper.append_child(&display);
    } else if let Ok(error) = document.create_element("div") {
        error.set_text_content(Some("Could not create display blob"));
        let _ = wrapper.append_child(&error);
    }

    prepend_result_node(&wrapper);
}

pub(super) fn tar_entries(entries: &[(Vec<u8>, String, String)]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let cursor = Cursor::new(&mut out);
        let mut builder = Builder::new(cursor);
        for (bytes, _mime, path) in entries.iter() {
            let name = normalize_bzz_path(&path);
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

pub(super) fn render_collection_download_button(
    entries: Rc<Vec<(Vec<u8>, String, String)>>,
    index: &str,
) {
    let document = web_sys::window().unwrap().document().unwrap();
    let button = match document.create_element("button") {
        Ok(button) => button,
        Err(_) => return,
    };
    let filename = format!("{}.tar", result_filename(index, "collection"));
    set_bracket_button_label(&button, &format!("Download {}", filename));
    let callback = wasm_bindgen::closure::Closure::<dyn FnMut(Event)>::new(move |_event| {
        if let Some(bytes) = tar_entries(&entries) {
            if let Some(url) = blob_url(&bytes, "application/x-tar") {
                click_download_url(&url, &filename);
            }
        }
    });
    let _ = button.add_event_listener_with_callback("click", callback.as_ref().unchecked_ref());
    callback.forget();
    prepend_progress_node(&button);
}

pub(super) async fn open_bzz_resource(weeb3: Arc<Weeb3>, resource: String) {
    let stream_files = stream_files_when_available();
    let progress_id = weeb3
        .start_progress("bzz", resource.clone(), "resolve", None, "resolving")
        .await;

    if let Some(metadata) = weeb3.resolve_bzz(resource.clone()).await {
        if stream_files {
            weeb3
                .update_progress(&progress_id, "stream", None, "checking stream support")
                .await;
            if crate::streaming_player::try_render_streaming_player(
                resource.clone(),
                metadata.clone(),
            )
            .await
            {
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
        if render_resolved_asset(weeb3.clone(), &resource, metadata).await {
            weeb3
                .finish_progress(&progress_id, "complete", "displayed selected asset", true)
                .await;
            return;
        }
    }

    weeb3
        .update_progress(&progress_id, "retrieve", None, "legacy retrieve fallback")
        .await;
    let result = weeb3.acquire(resource).await;
    let (data, indx) = decode_resources(result);
    let ok = !data.is_empty();
    render_result(data, indx).await;
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
    weeb3: Arc<Weeb3>,
    resource: &str,
    metadata: BzzMetadata,
) -> bool {
    if should_render_canonical_bzz_frame(&metadata) {
        if !service_worker_controlled_for_bzz_frame(&weeb3).await {
            service_worker_missing();
            return true;
        }
        if let Some(url) = canonical_bzz_url(resource, &metadata) {
            let Some(index_html) =
                preload_canonical_bzz_frame(&weeb3, resource, &url, &metadata).await
            else {
                render_text_result("Could not retrieve website index");
                return true;
            };
            render_canonical_bzz_frame(weeb3.clone(), resource, &url, &metadata, &index_html);
            return true;
        }
    }

    if metadata.size == 0 {
        render_result(
            vec![(vec![], metadata.mime.clone(), metadata.path.clone())],
            metadata.path,
        )
        .await;
        return true;
    }

    if let Some((bytes, metadata)) = weeb3
        .acquire_resolved_range(metadata.clone(), 0, metadata.size - 1)
        .await
    {
        render_result(
            vec![(bytes, metadata.mime.clone(), metadata.path.clone())],
            metadata.path,
        )
        .await;
        return true;
    }

    false
}

async fn preload_canonical_bzz_frame(
    weeb3: &Arc<Weeb3>,
    resource: &str,
    url: &str,
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
            crate::streaming_player::warm_bzz_fetch_cache(
                resource,
                metadata.clone(),
                bytes.clone(),
            );
            if let Some(canonical_resource) = url.strip_prefix("/weeb-3/bzz/") {
                crate::streaming_player::warm_bzz_fetch_cache(
                    canonical_resource,
                    metadata.clone(),
                    bytes.clone(),
                );
            }
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

pub(super) fn canonical_bzz_url(resource: &str, metadata: &BzzMetadata) -> Option<String> {
    let reference = bzz_reference_hex(resource)?;
    let requested_path = resource
        .split_once(&reference)
        .map(|(_, tail)| normalize_bzz_path(tail))
        .unwrap_or_default();
    let resolved_path = normalize_bzz_path(&metadata.path);
    let path = if !requested_path.is_empty()
        && (resolved_path.is_empty() || requested_path == resolved_path)
    {
        requested_path
    } else {
        resolved_path
    };
    let path = if path.is_empty() && should_render_canonical_bzz_frame(metadata) {
        "index.html".to_string()
    } else {
        path
    };

    if path.is_empty() || path.starts_with("unknown") || path == "not found" {
        Some(format!("/weeb-3/bzz/{}", reference))
    } else {
        Some(format!("/weeb-3/bzz/{}/{}", reference, path))
    }
}

pub(super) async fn download_bzz_resource(
    weeb3: Arc<Weeb3>,
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
        match tar_entries(&entries) {
            Some(bytes) => {
                if let Some(url) = blob_url(&bytes, "application/x-tar") {
                    click_download_url(&url, &fallback_filename);
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
            }
            None => {}
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
            click_download_url(&url, &filename);
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
    weeb3: Arc<Weeb3>,
    resource: &str,
    url: &str,
    metadata: &BzzMetadata,
    index_html: &[u8],
) {
    let document = web_sys::window().unwrap().document().unwrap();

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
    let callback = wasm_bindgen::closure::Closure::<dyn FnMut(Event)>::new(move |_event| {
        let weeb300 = weeb3.clone();
        let resource = resource.clone();
        let filename = download_filename.clone();
        spawn_local(async move {
            download_bzz_resource(weeb300, resource, filename).await;
        });
    });
    let _ = download.add_event_listener_with_callback("click", callback.as_ref().unchecked_ref());
    callback.forget();
    prepend_progress_node(&download);

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

    let _ = document
        .get_element_by_id("resultField")
        .expect("#resultField should exist")
        .dyn_ref::<HtmlElement>()
        .unwrap()
        .prepend_with_node_1(&wrapper);
}

fn srcdoc_with_base(bytes: &[u8], canonical_url: &str) -> String {
    let html = String::from_utf8_lossy(bytes);
    let base = format!(r#"<base href="{}">"#, srcdoc_base_url(canonical_url));
    let lower = html.to_ascii_lowercase();

    if let Some(head_end) = lower.find("<head>") {
        let insert = head_end + "<head>".len();
        format!("{}{}{}", &html[..insert], base, &html[insert..])
    } else if let Some(head_end) = lower.find("<head ") {
        match lower[head_end..].find('>') {
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

fn srcdoc_base_url(canonical_url: &str) -> String {
    match canonical_url.rfind('/') {
        Some(index) => canonical_url[..=index].to_string(),
        None => canonical_url.to_string(),
    }
}

pub(super) fn stream_files_when_available() -> bool {
    let document = web_sys::window().unwrap().document().unwrap();

    let stream_setting = match document.get_element_by_id("streamFilesWhenAvailable") {
        Some(stream_setting) => stream_setting,
        None => return true,
    };

    match stream_setting.dyn_ref::<HtmlInputElement>() {
        Some(stream_setting) => stream_setting.checked(),
        None => true,
    }
}

pub(super) fn update_transfer_pause_button(paused: bool) {
    let document = web_sys::window().unwrap().document().unwrap();
    let button = match document.get_element_by_id("transferPauseToggle") {
        Some(button) => button,
        None => return,
    };

    let button = button
        .dyn_ref::<HtmlButtonElement>()
        .expect("#transferPauseToggle should be a HtmlButtonElement");

    if paused {
        set_bracket_button_label(
            button.unchecked_ref::<Element>(),
            " Resume retrieve / push ",
        );
    } else {
        set_bracket_button_label(button.unchecked_ref::<Element>(), " Pause retrieve / push ");
    }
}

pub(super) fn create_element_wmt(tmype: String, blob_url: String) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();
    if tmype == "undefined" {
        let e = document.create_element("div").unwrap();
        e.set_inner_html("Not found");
        return e;
    }

    let i = document.create_element("embed").unwrap();
    let _ = i.set_attribute("src", &blob_url);
    let _ = i.set_attribute("type", &tmype);
    // let _ = i.set_attribute("allow", "fullscreen");

    return i;
}

pub(crate) fn service_worker_missing() {
    let document = web_sys::window().unwrap().document().unwrap();
    let errod = document.create_element("div").unwrap();
    errod.set_inner_html("Service worker required and not found. Loading websites from swarm requires accessing weeb-3 via https through secure certificate.");
    let _r = document
        .get_element_by_id("resultField")
        .expect("#resultField should exist")
        .dyn_ref::<HtmlElement>()
        .unwrap()
        .prepend_with_node_1(&errod)
        .unwrap();
}

pub(super) fn render_text_result(message: &str) {
    let document = web_sys::window().unwrap().document().unwrap();
    let result = match document.create_element("div") {
        Ok(result) => result,
        Err(_) => return,
    };
    result.set_text_content(Some(message));
    let _ = document
        .get_element_by_id("resultField")
        .expect("#resultField should exist")
        .dyn_ref::<HtmlElement>()
        .unwrap()
        .prepend_with_node_1(&result);
}

pub(super) fn render_log_message(log: &String) {
    let document = web_sys::window().unwrap().document().unwrap();
    let log_message_div = document.create_element("div").unwrap();
    log_message_div.set_text_content(Some(log));
    let _r = document
        .get_element_by_id("logsField")
        .expect("#logsField should exist")
        .dyn_ref::<HtmlElement>()
        .unwrap()
        .prepend_with_node_1(&log_message_div)
        .unwrap();
}

pub(super) fn render_progress_rows(rows: Vec<crate::events::ProgressRow>) {
    let document = web_sys::window().unwrap().document().unwrap();
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

pub(super) async fn render_result(data: Vec<(Vec<u8>, String, String)>, indx: String) {
    interface_debug(&JsValue::from(format!(
        "data array length {:#?}",
        data.len()
    )));

    if data.len() == 0 {
        let new_element = create_element_wmt("undefined".to_string(), "".to_string());
        prepend_result_node(&new_element);
    } else if data.len() == 1 {
        interface_debug(&JsValue::from(format!(
            "data length {:#?}",
            data[0].0.len()
        )));
        let (bytes, mime, path) = data.into_iter().next().unwrap();
        render_single_result_with_download(bytes, mime, path);
    } else {
        let selected = data
            .iter()
            .find(|(_, _, path)| *path == indx)
            .or_else(|| data.get(0))
            .cloned();
        if let Some((bytes, mime, path)) = selected {
            render_single_result_with_download(bytes, mime, path);
        }
        render_collection_download_button(Rc::new(data), &indx);
    }
}

pub fn parsebootconnect(boot_node_masettings_id: String) -> (String, String) {
    let document = web_sys::window().unwrap().document().unwrap();

    let bootnode_input = document
        .get_element_by_id(&boot_node_masettings_id)
        .expect(&format!("#{} should exist", boot_node_masettings_id));

    let bootnode_input = bootnode_input
        .dyn_ref::<HtmlInputElement>()
        .expect(&format!(
            "#{} should be a HtmlInputElement",
            boot_node_masettings_id
        ));

    interface_debug(&"g0 bootnode change triggered".into());
    match bootnode_input.value().parse::<String>() {
        Ok(bootnode_address) => {
            interface_debug(&"g1 bootnode change triggered".into());

            let network_id_input = document
                .get_element_by_id("networkIDSettings")
                .expect("#networkIDSettings should exist");

            let network_id_input = network_id_input
                .dyn_ref::<HtmlInputElement>()
                .expect("#networkIDSettings should be a HtmlInputElement");

            match network_id_input.value().parse::<String>() {
                Ok(network_id) => return (bootnode_address, network_id),
                _ => return (bootnode_address, "10".to_string()),
            };
        }
        _ => {}
    };
    return ("".to_string(), "".to_string());
}

pub async fn get_service_worker() -> Option<web_sys::ServiceWorker> {
    let service0 = web_sys::window().unwrap().navigator().service_worker();

    match JsFuture::from(service0.register("/weeb-3/service.js")).await {
        Ok(registration) => {
            let _ = JsFuture::from(
                registration
                    .unchecked_into::<ServiceWorkerRegistration>()
                    .update()
                    .unwrap(),
            )
            .await;
            let _ = JsFuture::from(service0.ready().unwrap()).await;
        }
        Err(err) => {
            web_sys::console::warn_1(&err);
        }
    }

    let registration0 = JsFuture::from(service0.get_registration()).await;

    let registration1: ServiceWorkerRegistration = match registration0 {
        Ok(registration) => {
            let reg = registration.dyn_into();
            match reg {
                Ok(reg) => reg,
                _ => {
                    service_worker_missing();
                    return None;
                }
            }
        }
        _ => {
            service_worker_missing();
            return None;
        }
    };

    let service_worker0 = registration1.active();

    let _service_worker1 = match service_worker0 {
        Some(service_worker) => {
            return Some(service_worker);
        }
        _ => {
            service_worker_missing();
            return None;
        }
    };
}

fn service_worker_has_controller() -> bool {
    let service0 = web_sys::window().unwrap().navigator().service_worker();
    js_sys::Reflect::get(service0.as_ref(), &JsValue::from_str("controller"))
        .map(|controller| !controller.is_null() && !controller.is_undefined())
        .unwrap_or(false)
}

async fn service_worker_controlled_for_bzz_frame(weeb3: &Arc<Weeb3>) -> bool {
    if service_worker_has_controller() {
        return true;
    }

    weeb3.interface_log("service worker activating for bzz frame".to_string());
    if get_service_worker().await.is_none() {
        weeb3.interface_log("service worker unavailable for bzz frame".to_string());
        return false;
    }

    for _ in 0..80 {
        if service_worker_has_controller() {
            weeb3.interface_log("service worker controls bzz frame requests".to_string());
            return true;
        }
        async_std::task::sleep(Duration::from_millis(100)).await;
    }

    weeb3.interface_log("service worker did not control bzz frame requests".to_string());
    false
}
