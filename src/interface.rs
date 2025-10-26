use std::time::Duration;

use web3::types::{Address, U256};

use async_std::sync::Arc;
use js_sys::{Array, Date, Uint8Array};
use wasm_bindgen::{JsCast, JsError, JsValue, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use web_sys::{
    Blob,
    BlobPropertyBag,
    Element,
    HtmlButtonElement,
    HtmlElement,
    HtmlInputElement,
    HtmlSelectElement,
    HtmlSpanElement,
    MessageEvent,
    RequestInit,
    ServiceWorkerRegistration,
    //
    console,
};

use crate::{
    Sekirei, decode_resources, encrey, join, join_all,
    nav::{clear_path, read_path},
    on_chain::{buy_postage_batch, get_batch_validity},
    persistence::{
        get_batch_bucket_limit, get_batch_id, get_batch_owner_key, set_batch_bucket_limit,
        set_batch_id, set_batch_owner_key,
    },
};
use alloy::{network::EthereumWallet, signers::local::PrivateKeySigner};

#[wasm_bindgen]
pub async fn interweeb(_st: String) -> Result<(), JsError> {
    //    init_panic_hook();

    clear_path().await;

    let stored_batch_id = get_batch_id().await;
    if stored_batch_id.len() == 32 {
        let validity = get_batch_validity(stored_batch_id).await;
        web_sys::console::log_1(&JsValue::from(format!(
            "Found batch with validity {:#?}",
            validity
        )));
    }

    let _service_worker = match get_service_worker().await {
        Some(service_worker) => Some(service_worker),
        None => None,
    };

    let sekirei = Arc::new(Sekirei::new("".to_string()));

    let sekirei0 = sekirei.clone();

    let sekirei_async = async move {
        sekirei0.run("".to_string()).await;
    };

    let sekirei1 = sekirei.clone();
    let sekirei2 = sekirei.clone();
    let sekirei3 = sekirei.clone();
    let sekirei4 = sekirei.clone();
    let sekirei5 = sekirei.clone();
    let sekirei6 = sekirei.clone();
    let sekirei7 = sekirei.clone();
    let sekirei8 = sekirei.clone();
    let sekirei9 = sekirei.clone();

    let path_load_init = async {
        let references = read_path().await;
        let mut handles = vec![];
        for reference in references {
            let handle = async {
                let sekirei00 = sekirei6.clone();
                web_sys::console::log_1(&JsValue::from(format!(
                    "Loading /bzz/ reference from path {:#?}",
                    reference
                )));
                let result = sekirei00.acquire(reference).await;
                let (data, indx) = decode_resources(result);
                render_result(data, indx).await;
            };
            handles.push(handle);
        }
        let _ = join_all(handles).await;
    };

    let window = web_sys::window().unwrap();

    let host2 = window
        .document()
        .unwrap()
        .location()
        .unwrap()
        .origin()
        .unwrap();

    let interface_async = async move {
        web_sys::console::log_1(&JsValue::from(format!("host2 {:#?}", host2)));

        // let document = web_sys::window().unwrap().document().unwrap();

        let callback =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                console::log_1(&"oninput callback triggered".into());
                let sekirei00 = sekirei1.clone();
                let document = web_sys::window().unwrap().document().unwrap();

                let input_field = document
                    .get_element_by_id("inputString")
                    .expect("#inputString should exist");
                let input_field = input_field
                    .dyn_ref::<HtmlInputElement>()
                    .expect("#inputString should be a HtmlInputElement");

                match input_field.value().parse::<String>() {
                    Ok(text) => spawn_local(async move {
                        console::log_1(&"oninput callback string".into());

                        let result = sekirei00.acquire(text).await;

                        let (data, indx) = decode_resources(result);

                        render_result(data, indx).await;
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
        let callback2 = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |_msg| {
                console::log_1(&"uploadGetBatch callback triggered".into());

                let document = web_sys::window().unwrap().document().unwrap();

                let validity_input = document
                    .get_element_by_id("batchValidityDays")
                    .expect("#batchValidityDays should exist");
                let validity_input = validity_input
                    .dyn_ref::<HtmlInputElement>()
                    .expect("#batchValidityDays should be a HtmlInputElement");

                let validity = match validity_input.value().parse::<u64>() {
                    Ok(v) => v,
                    _ => {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Failed to read batch validity");
                        return;
                    }
                };

                let size_input = document
                    .get_element_by_id("batchSize")
                    .expect("#batchSize should exist");
                let size_input = size_input
                    .dyn_ref::<HtmlSelectElement>()
                    .expect("#batchSize should be a HtmlSelectElement");

                let batch_depth = match size_input.value().parse::<u8>() {
                    Ok(size0) => 17 + size0,
                    _ => {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Failed to read batch size");
                        return;
                    }
                };

                web_sys::console::log_1(&JsValue::from(format!(
                    "Selected batch depth: {}",
                    batch_depth
                )));

                spawn_local(async move {
                    {
                        let window = web_sys::window().unwrap();
                        if let Ok(func) =
                            js_sys::Reflect::get(&window, &JsValue::from_str("weeb3EnsureEip1193"))
                        {
                            if let Ok(f) = func.dyn_into::<js_sys::Function>() {
                                let project_id =
                                    JsValue::from_str("64c5f91181ce0a3192a783346a475d23");
                                if let Ok(promise_val) = f.call1(&JsValue::NULL, &project_id) {
                                    if let Ok(promise) = promise_val.dyn_into::<js_sys::Promise>() {
                                        let res = JsFuture::from(promise).await;
                                        if let Ok(obj) = res {
                                            let ok = js_sys::Reflect::get(&obj, &"ok".into())
                                                .ok()
                                                .and_then(|v| v.as_bool())
                                                .unwrap_or(false);
                                            if !ok {
                                                let err =
                                                    js_sys::Reflect::get(&obj, &"error".into())
                                                        .ok()
                                                        .and_then(|v| v.as_string())
                                                        .unwrap_or_else(|| {
                                                            "WalletConnect failed".into()
                                                        });
                                                let wnd = web_sys::window().unwrap();
                                                let _ = wnd.alert_with_message(&format!(
                                                    "Wallet connect failed: {}",
                                                    err
                                                ));
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if web3::transports::eip_1193::Provider::default()
                            .ok()
                            .flatten()
                            .is_none()
                        {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd.alert_with_message(
            "No Ethereum provider. Try again, or open this page in the MetaMask in‑app browser."
        );
                            return;
                        }
                    }

                    let stored_stamp_signer_key = get_batch_owner_key().await;
                    let stored_batch_id = get_batch_id().await;
                    let bucket_limit = get_batch_bucket_limit().await;

                    if !stored_stamp_signer_key.is_empty()
                        && !stored_batch_id.is_empty()
                        && bucket_limit > 0
                    {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Already have a batch for uploads");
                        return;
                    }

                    let stamp_signer_key = encrey();
                    let stamp_signer: PrivateKeySigner =
                        match PrivateKeySigner::from_slice(&stamp_signer_key) {
                            Ok(s) => s,
                            Err(_) => {
                                let wnd = web_sys::window().unwrap();
                                let _ = wnd
                                    .alert_with_message("Failed to create local stamp signer key");
                                return;
                            }
                        };

                    let _wallet = EthereumWallet::from(stamp_signer.clone());

                    let owner_h160_bytes: [u8; 20] = *stamp_signer.address().as_ref();
                    let owner = Address::from(owner_h160_bytes);

                    web_sys::console::log_1(&JsValue::from(format!(
                        "StampSigner addr 0x{}",
                        hex::encode(owner_h160_bytes)
                    )));

                    let purchase = match buy_postage_batch(validity, batch_depth, owner).await {
                        Ok(p) => p,
                        Err(e) => {
                            let wnd = web_sys::window().unwrap();
                            let _ = wnd.alert_with_message(&format!(
                                    "Batch purchase failed: {:?}. \
                                     Ensure your wallet is connected, on Sepolia, and has sufficient SBZZ + Sepolia ETH.",
                                    e
                                ));
                            return;
                        }
                    };

                    if !set_batch_owner_key(&stamp_signer_key).await {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Failed to save batch owner key");
                        return;
                    }

                    if !set_batch_id(&purchase.batch_id).await {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Failed to save batch id");
                        return;
                    }

                    if !set_batch_bucket_limit(purchase.bucket_limit).await {
                        let wnd = web_sys::window().unwrap();
                        let _ = wnd.alert_with_message("Failed to save batch bucket depth");
                        return;
                    }

                    web_sys::console::log_1(&JsValue::from(format!(
                        "Approve tx 0x{}, Create tx 0x{}, Batch id 0x{}, depth {}, validity {}d, lastPrice {}",
                        hex::encode(purchase.approve_tx.as_bytes()),
                        hex::encode(purchase.create_tx.as_bytes()),
                        hex::encode(&purchase.batch_id),
                        batch_depth,
                        validity,
                        purchase.last_price,
                    )));

                    let wnd = web_sys::window().unwrap();
                    let _ = wnd.alert_with_message(&format!(
                        "Storage batch ready.\nBatch ID: 0x{}\nDepth: {}\nStorage slots per bucket: {}",
                        hex::encode(&purchase.batch_id),
                        batch_depth,
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

        let callback3 = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |_msg| {
                console::log_1(&"oninput file callback".into());

                let sekirei00 = sekirei2.clone();

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
                spawn_local(async move {
                    let stored_stamp_signer_key = get_batch_owner_key().await;
                    let stored_batch_id = get_batch_id().await;
                    let bucket_limit = get_batch_bucket_limit().await;

                    if stored_stamp_signer_key.len() == 0
                        || stored_batch_id.len() == 0
                        || bucket_limit == 0
                    {
                        let window = web_sys::window().unwrap();
                        let _ = window.alert_with_message("Require a postage batch for uploads. To get one, connect metamask wallet with sepolia Eth / sepolia Bzz with the 'Create Storage on Swarm for Uploads' button");
                        return;
                    }

                    let validity = get_batch_validity(stored_batch_id).await;

                    if validity == U256::from(0) {
                        let window = web_sys::window().unwrap();
                        let _ = window.alert_with_message(
                            "Postage batch validity reached zero. Getting a new batch is required.",
                        );

                        set_batch_owner_key(&vec![]).await;
                        set_batch_id(&vec![]).await;
                        set_batch_bucket_limit(0).await;
                    }

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

                    web_sys::console::log_1(&JsValue::from(format!(
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

                    web_sys::console::log_1(&JsValue::from(format!("IF Upload Marker 0")));

                    let result = sekirei00
                        .post_upload(
                            file,
                            file_enc.checked() && !upload_to_feed.checked(),
                            index_string,
                            upload_to_feed.checked(),
                            feed_topic,
                        )
                        .await;

                    web_sys::console::log_1(&JsValue::from(format!("IF Upload Marker 1")));

                    let (data, indx) = decode_resources(result);

                    render_result(data, indx).await;

                    console::log_1(&"oninput file callback happened".into());
                })
            },
        );

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("uploadFile")
            .expect("#uploadFile should exist")
            .dyn_ref::<HtmlButtonElement>()
            .expect("#uploadFile should be a HtmlButtonElement")
            .set_onclick(Some(callback3.as_ref().unchecked_ref()));

        let callback4 =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                let sekirei00 = sekirei3.clone();

                console::log_1(&"oninput bootnode callback".into());

                spawn_local(async move {
                    let (bna, nid) = parsebootconnect();
                    let result = sekirei00.change_bootnode_address(bna, nid).await;

                    let (data, indx) = decode_resources(result);

                    render_result(data, indx).await;
                });

                console::log_1(&"oninput network settings callback happened".into());
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

        let callback5 =
            wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
                let sekirei00 = sekirei4.clone();

                console::log_1(&"oninput reset stamp callback".into());

                let window = web_sys::window().unwrap();

                if window
                .confirm_with_message(
                    "This will enable overwriting previously uploaded content with new content.",
                )
                .unwrap_or(false)
            {
                spawn_local(async move {
                    let result = sekirei00.reset_stamp().await;

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

        let service_closure = Closure::wrap(Box::new(move |event: MessageEvent| {
            if let Ok(obj) = event.data().dyn_into::<js_sys::Object>() {
                web_sys::console::log_1(&JsValue::from(format!(
                    "Attempting to load reference received from service worker {:#?}",
                    obj
                )));
                let ty =
                    js_sys::Reflect::get(&obj, &JsValue::from_str("type")).unwrap_or(JsValue::NULL);
                if ty == JsValue::from_str("RETRIEVE_REQUEST") {
                    let url = js_sys::Reflect::get(&obj, &JsValue::from_str("url"))
                        .unwrap_or(JsValue::NULL);
                    let reference = url.as_string().unwrap_or_default();
                    let sekirei00 = sekirei7.clone();

                    let ports: Array = event.ports().into();
                    let port = ports.get(0).dyn_into::<web_sys::MessagePort>().ok();

                    wasm_bindgen_futures::spawn_local(async move {
                        web_sys::console::log_1(&JsValue::from(format!(
                            "Loading /bzz/ reference from service worker {:#?}",
                            reference
                        )));
                        let result = sekirei00.acquire(reference).await;
                        let (data, indx) = decode_resources(result);
                        render_result(data.clone(), indx.clone()).await;

                        let resp = js_sys::Object::new();
                        js_sys::Reflect::set(&resp, &"ok".into(), &true.into()).unwrap();
                        js_sys::Reflect::set(&resp, &"type".into(), &"RETRIEVE_RESPONSE".into())
                            .unwrap();
                        js_sys::Reflect::set(&resp, &"indx".into(), &indx.clone().into()).unwrap();

                        let head_resource = data
                            .iter()
                            .find(|(_, _, path)| *path == indx)
                            .or_else(|| data.get(0));

                        if let Some((bytes, mime, path)) = head_resource {
                            web_sys::console::log_1(&JsValue::from(format!(
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

                    let sekirei00 = sekirei8.clone();
                    let port = event.ports().get(0).dyn_into::<web_sys::MessagePort>().ok();

                    wasm_bindgen_futures::spawn_local(async move {
                        let result = sekirei00
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
                web_sys::console::log_1(&JsValue::from(format!(
                    "Service listener error {:#?}",
                    err
                )));
            }
        };

        loop {
            #[allow(irrefutable_let_patterns)]
            let logs_current = sekirei5.get_current_logs().await;
            for log_message in logs_current.iter() {
                render_log_message(&log_message);
            }

            let ongoing = sekirei5.get_ongoing_connections().await;

            let _ = web_sys::window()
                .unwrap()
                .document()
                .unwrap()
                .get_element_by_id("ongoing")
                .expect("#ongoing should exist")
                .dyn_ref::<HtmlSpanElement>()
                .unwrap()
                .set_inner_html(&ongoing.to_string());

            let connections = sekirei5.get_connections().await;

            let _ = web_sys::window()
                .unwrap()
                .document()
                .unwrap()
                .get_element_by_id("connections")
                .expect("#connections should exist")
                .dyn_ref::<HtmlSpanElement>()
                .unwrap()
                .set_inner_html(&connections.to_string());

            async_std::task::sleep(Duration::from_millis(600)).await
        }
    };

    let fetch_test = async move {
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

    let initial_connect_handle = async {
        async_std::task::sleep(Duration::from_millis(600)).await;
        let (bna, nid) = parsebootconnect();
        sekirei9.change_bootnode_address(bna, nid).await;
    };

    join!(
        sekirei_async,
        interface_async,
        path_load_init,
        fetch_test,
        initial_connect_handle
    );

    #[allow(unreachable_code)]
    Ok(())
}

fn create_element_wmt(tmype: String, blob_url: String) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();
    if tmype == "undefined" {
        let e = document.create_element("div").unwrap();
        e.set_inner_html("Not found");
        return e;
    }

    let i = document.create_element("embed").unwrap();
    let _ = i.set_attribute("src", &blob_url);
    let _ = i.set_attribute("type", &tmype);
    return i;
}

fn create_ielement(indx: String) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();

    let i = document.create_element("iframe").unwrap();
    let _ = i.set_attribute("src", &indx);
    let _ = i.set_attribute("width", "90%");
    let _ = i.set_attribute("height", "90%");
    let _ = i.set_attribute("sandbox", "allow-scripts");
    return i;
}

fn service_worker_missing() {
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

fn render_log_message(log: &String) {
    let document = web_sys::window().unwrap().document().unwrap();
    let log_message_div = document.create_element("div").unwrap();
    log_message_div.set_inner_html(&log);
    let _r = document
        .get_element_by_id("logsField")
        .expect("#logsField should exist")
        .dyn_ref::<HtmlElement>()
        .unwrap()
        .prepend_with_node_1(&log_message_div)
        .unwrap();
}

async fn render_result(data: Vec<(Vec<u8>, String, String)>, indx: String) {
    let host2 = web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .location()
        .unwrap()
        .origin()
        .unwrap();

    web_sys::console::log_1(&JsValue::from(format!(
        "data array length {:#?}",
        data.len()
    )));

    if data.len() == 0 {
        let new_element = create_element_wmt("undefined".to_string(), "".to_string());

        let document = web_sys::window().unwrap().document().unwrap();

        let _r = document
            .get_element_by_id("resultField")
            .expect("#resultField should exist")
            .dyn_ref::<HtmlElement>()
            .unwrap()
            .prepend_with_node_1(&new_element)
            .unwrap();
    } else if data.len() == 1 {
        web_sys::console::log_1(&JsValue::from(format!(
            "data length {:#?}",
            data[0].0.len()
        )));

        let props = BlobPropertyBag::new();
        props.set_type(&data[0].1);

        let data2: Uint8Array = JsValue::from(data[0].0.clone()).into();
        let bytes = Array::new();
        bytes.push(&data2);

        let blob = Blob::new_with_u8_array_sequence_and_options(&bytes, &props).unwrap();

        let blob_url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();

        let new_element = create_element_wmt(blob.type_(), blob_url);

        let document = web_sys::window().unwrap().document().unwrap();

        let _r = document
            .get_element_by_id("resultField")
            .expect("#resultField should exist")
            .dyn_ref::<HtmlElement>()
            .unwrap()
            .prepend_with_node_1(&new_element)
            .unwrap();
    } else {
        let date3 = Date::now().to_string();

        let service_worker1 = match get_service_worker().await {
            Some(service_worker) => service_worker,
            None => {
                return;
            }
        };

        for (data3, mime3, path3) in data {
            let opts = RequestInit::new();

            opts.set_method("GET");

            let req_headers = web_sys::Headers::new().unwrap();
            let _ = req_headers.append("Referrer-Policy", "strict-origin-when-cross-origin");
            opts.set_headers(&req_headers);

            let sep = "/".to_string();
            let mut path03 = host2.clone();
            path03.push_str(&sep);
            path03.push_str(&"weeb-3".to_string());
            path03.push_str(&sep);
            path03.push_str(&date3);
            path03.push_str(&sep);
            path03.push_str(&path3);

            let props = BlobPropertyBag::new();
            props.set_type(&mime3);
            let data2: Uint8Array = JsValue::from(data3).into();
            let bytes = Array::new();
            bytes.push(&data2);

            let msgobj = js_sys::Object::new();

            let _ = js_sys::Reflect::set(&msgobj, &JsValue::from_str("data0"), &bytes);
            let _ = js_sys::Reflect::set(
                &msgobj,
                &JsValue::from_str("mime0"),
                &JsValue::from_str(&mime3),
            );
            let _ = js_sys::Reflect::set(
                &msgobj,
                &JsValue::from_str("path0"),
                &JsValue::from_str(&path03),
            );

            let _ = service_worker1.post_message(&JsValue::from(msgobj));
        }

        let sep = "/".to_string();
        let mut path00 = host2.clone();
        path00.push_str(&sep);
        path00.push_str(&"weeb-3".to_string());
        path00.push_str(&sep);
        path00.push_str(&date3);
        path00.push_str(&sep);
        path00.push_str(&indx);

        async_std::task::sleep(Duration::from_millis(600)).await;

        let new_element = create_ielement(path00);

        let document = web_sys::window().unwrap().document().unwrap();

        let _r = document
            .get_element_by_id("resultField")
            .expect("#resultField should exist")
            .dyn_ref::<HtmlElement>()
            .unwrap()
            .prepend_with_node_1(&new_element)
            .unwrap();
    }
}

pub fn parsebootconnect() -> (String, String) {
    let document = web_sys::window().unwrap().document().unwrap();

    let bootnode_input = document
        .get_element_by_id("bootNodeMASettings")
        .expect("#bootNodeMASettings should exist");

    let bootnode_input = bootnode_input
        .dyn_ref::<HtmlInputElement>()
        .expect("#bootNodeMASettings should be a HtmlInputElement");

    web_sys::console::log_1(&"g0 bootnode change triggered".into());
    match bootnode_input.value().parse::<String>() {
        Ok(bootnode_address) => {
            web_sys::console::log_1(&"g1 bootnode change triggered".into());

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

    match JsFuture::from(service0.register("./service.js")).await {
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
            console::warn_1(&err);
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
