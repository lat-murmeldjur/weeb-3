use crate::{decode_resources, init_panic_hook, Body};
use std::cell::RefCell;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::mpsc;
use std::time::Duration;

use web3::{
    contract::{Contract, Options},
    transports::eip_1193::{Eip1193, Provider},
    types::{Address, U256},
};

use js_sys::{Array, Date, Uint8Array};
use wasm_bindgen::{closure::Closure, prelude::*, JsCast, JsError, JsValue};

use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    console,
    Blob,
    BlobPropertyBag,
    Element,
    HtmlButtonElement,
    HtmlElement,
    HtmlInputElement,
    MessageEvent,
    RequestInit,
    ServiceWorkerRegistration,
    SharedWorker,
    //
};

use alloy::{network::EthereumWallet, primitives::keccak256, signers::local::PrivateKeySigner};

#[wasm_bindgen]
pub async fn interweeb(_st: String) -> Result<(), JsError> {
    init_panic_hook();

    let window = &web_sys::window().unwrap();

    let host2 = window
        .document()
        .unwrap()
        .location()
        .unwrap()
        .origin()
        .unwrap();

    web_sys::console::log_1(&JsValue::from(format!("host2 {:#?}", host2)));

    let body = Body::from_current_window()?;

    let (r_out, r_in) = mpsc::channel::<Vec<u8>>();

    let worker_handle0 = Rc::new(RefCell::new(
        SharedWorker::new_with_worker_options("./worker.js", &{
            let opts = web_sys::WorkerOptions::new();
            opts.set_type(web_sys::WorkerType::Module);
            opts
        })
        .unwrap(),
    ));

    let worker_handle = worker_handle0.clone();
    let worker_handle3 = worker_handle0.clone();

    {
        let worker_handle_2 = &*worker_handle.borrow();
        let port = worker_handle_2.port();
        let _qxy = port.start();
    }

    let document = web_sys::window().unwrap().document().unwrap();

    #[allow(unused_assignments)]
    let mut persistent_callback_handle = get_on_msg_callback(r_out.clone());
    let mut persistent_callback_handle3 = get_on_msg_callback(r_out.clone());

    let r_out0 = r_out.clone();
    let r_out3 = r_out.clone();

    let callback =
        wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
            console::log_1(&"oninput callback triggered".into());
            let document = web_sys::window().unwrap().document().unwrap();

            let input_field = document
                .get_element_by_id("inputString")
                .expect("#inputString should exist");
            let input_field = input_field
                .dyn_ref::<HtmlInputElement>()
                .expect("#inputString should be a HtmlInputElement");

            match input_field.value().parse::<String>() {
                Ok(text) => {
                    console::log_1(&"oninput callback string".into());
                    // Access worker behind shared handle, following the interior
                    // mutability pattern.
                    let worker_handle_2 = worker_handle.borrow();
                    let _ = worker_handle_2.port().post_message(&text.into());
                    persistent_callback_handle = get_on_msg_callback(r_out0.clone());

                    // Since the worker returns the message asynchronously, we
                    // attach a callback to be triggered when the worker returns.
                    worker_handle_2
                        .port()
                        .set_onmessage(Some(persistent_callback_handle.as_ref().unchecked_ref()));

                    console::log_1(&"oninput callback happened".into());
                }
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

    document
        .get_element_by_id("inputString")
        .expect("#inputString should exist")
        .dyn_ref::<HtmlInputElement>()
        .expect("#inputString should be a HtmlInputElement")
        .set_oninput(Some(callback.as_ref().unchecked_ref()));

    let callback2 =
        wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
            console::log_1(&"uploadGetBatch callback triggered".into());

            spawn_local(async {
                {
                    let provider = Provider::default().unwrap().unwrap();

                    let transport = Eip1193::new(provider);
                    let web3 = web3::Web3::new(transport);
                    let accounts = match web3.eth().request_accounts().await {
                        Ok(aok) => aok,
                        _ => return,
                    };

                    for account in &accounts {
                        let balance = web3.eth().balance(*account, None).await.unwrap();
                        web_sys::console::log_1(&JsValue::from(format!(
                            "Balance of {:?}: {}",
                            account, balance
                        )));
                    }

                    let token_contract_address =
                        match Address::from_str("543dDb01Ba47acB11de34891cD86B675F04840db") {
                            Ok(aok) => aok,
                            _ => return,
                        };

                    let contract_address =
                        match Address::from_str("cdfdC3752caaA826fE62531E0000C40546eC56A6") {
                            Ok(aok) => aok,
                            _ => return,
                        };

                    let token_contract = match Contract::from_json(
                        web3.eth(),
                        token_contract_address,
                        include_bytes!("./sbzz.json"),
                    ) {
                        Ok(aok) => aok,
                        _ => return,
                    };

                    let contract = match Contract::from_json(
                        web3.eth(),
                        contract_address,
                        include_bytes!("./postagestamp.json"),
                    ) {
                        Ok(aok) => aok,
                        _ => return,
                    };

                    let stamp_signer_key = keccak256("Key To Be Persisted In Browser Localstore");
                    let stamp_signer: PrivateKeySigner =
                        match PrivateKeySigner::from_bytes(&stamp_signer_key) {
                            Ok(aok) => aok,
                            _ => return,
                        };
                    let _wallet = EthereumWallet::from(stamp_signer.clone());
                    let wallet_address = stamp_signer.address();
                    let wallet_address_bytes: [u8; 20] = *wallet_address.as_ref();

                    web_sys::console::log_1(&JsValue::from(format!(
                        "StampSigner len {:#?}",
                        wallet_address_bytes.len()
                    )));

                    //    function createBatch(
                    //        address _owner,
                    //        uint256 _initialBalancePerChunk,
                    //        uint8 _depth,
                    //        uint8 _bucketDepth,
                    //        bytes32 _nonce,
                    //        bool _immutable
                    //    ) external whenNotPaused returns (bytes32)
                    let result_cc: U256 = contract
                        .query("currentTotalOutPayment", (), None, Options::default(), None)
                        .await
                        .unwrap();

                    web_sys::console::log_1(&JsValue::from(format!(
                        "currentTotalOutPayment (minimum valid balance) {}",
                        result_cc
                    )));

                    // expiredBatchesExist()

                    let mut expire = true;

                    while expire {
                        let expire0 = contract
                            .query("expiredBatchesExist", (), None, Options::default(), None)
                            .await
                            .unwrap();

                        web_sys::console::log_1(&JsValue::from(format!(
                            "expiredBatchesExist {}",
                            expire0
                        )));

                        if expire0 {
                            let tx0 = contract
                                .call(
                                    "expireLimited",
                                    (U256::from(5),),
                                    accounts[0],
                                    Options::default(),
                                )
                                .await
                                .unwrap();

                            web_sys::console::log_1(&JsValue::from(format!(
                                "expirelimited tx0 {}",
                                hex::encode(tx0.as_bytes())
                            )));
                        } else {
                            expire = false;
                        }
                    }

                    let tx00 = token_contract
                        .call_with_confirmations(
                            "approve",
                            (contract_address, U256::from(13107200000000000_u64)),
                            accounts[0],
                            Options::default(),
                            2,
                        )
                        .await
                        .unwrap();

                    web_sys::console::log_1(&JsValue::from(format!(
                        "Approve tx {}",
                        hex::encode(tx00.transaction_hash.as_bytes())
                    )));

                    // 131072000000000

                    let tx = contract
                        .call(
                            "createBatch",
                            (
                                Address::from(wallet_address_bytes),
                                U256::from(100000000000_u64),
                                17_u8,
                                16_u8,
                                [1_u8; 32],
                                false,
                            ),
                            accounts[0],
                            Options::default(),
                        )
                        .await
                        .unwrap();

                    web_sys::console::log_1(&JsValue::from(format!("createBatch tx {}", tx)));

                    // tx 0x538ea062d293a809915336eff3bb5010dc742c0ab6ac12b510992aafb4a68ffb
                    // id 0xb57e46b067d21cede7432900215423e82c97823b733219b7f42e73017562a96d
                    // owner : 0x4d10dDf65bCCB88C458607b4fF64AA808c31C53c
                    // depth : 17
                    // bucketDepth : 16
                    // immutableFlag : False

                    web_sys::console::log_1(&JsValue::from(format!(
                        "createBatch tx 2 {}",
                        hex::encode(tx.as_bytes())
                    )));
                }
            });
        });

    document
        .get_element_by_id("uploadGetBatch")
        .expect("#uploadGetBatch should exist")
        .dyn_ref::<HtmlButtonElement>()
        .expect("#uploadGetBatch should be a HtmlButtonElement")
        .set_onclick(Some(callback2.as_ref().unchecked_ref()));

    let callback3 =
        wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
            console::log_1(&"oninput file callback".into());

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

            web_sys::console::log_1(&JsValue::from(format!(
                "selected file length {:#?}",
                file.size()
            )));

            // let content_u8a = Uint8Array::new(&content_buf);

            let msgobj = js_sys::Object::new();

            let _ = js_sys::Reflect::set(
                &msgobj,
                &JsValue::from_str("type0"),
                &JsValue::from_str("file"),
            );
            let _ = js_sys::Reflect::set(&msgobj, &JsValue::from_str("file0"), &file);

            let _ = js_sys::Reflect::set(
                &msgobj,
                &JsValue::from_str("encryption0"),
                &JsValue::from_bool(false),
            );

            let worker_handle_2 = worker_handle3.borrow();
            let _ = worker_handle_2.port().post_message(&msgobj);

            persistent_callback_handle3 = get_on_msg_callback(r_out3.clone());

            // Since the worker returns the message asynchronously, we
            // attach a callback to be triggered when the worker returns.
            worker_handle_2
                .port()
                .set_onmessage(Some(persistent_callback_handle3.as_ref().unchecked_ref()));

            console::log_1(&"oninput file callback happened".into());
        });
    document
        .get_element_by_id("uploadFile")
        .expect("#uploadFile should exist")
        .dyn_ref::<HtmlButtonElement>()
        .expect("#uploadFile should be a HtmlButtonElement")
        .set_onclick(Some(callback3.as_ref().unchecked_ref()));

    body.append_p(&format!("Created a new worker from within Wasm"))?;

    loop {
        #[allow(irrefutable_let_patterns)]
        while let data0 = r_in.try_recv() {
            if !data0.is_err() {
                let (data, indx) = decode_resources(data0.unwrap());
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

                    let blob =
                        Blob::new_with_u8_array_sequence_and_options(&bytes, &props).unwrap();

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
                                    continue;
                                }
                            }
                        }
                        _ => {
                            service_worker_missing();
                            continue;
                        }
                    };

                    let service_worker0 = registration1.active();

                    let service_worker1 = match service_worker0 {
                        Some(service_worker) => service_worker,
                        _ => {
                            service_worker_missing();
                            continue;
                        }
                    };

                    for (data3, mime3, path3) in data {
                        let opts = RequestInit::new();

                        opts.set_method("GET");

                        let req_headers = web_sys::Headers::new().unwrap();
                        let _ = req_headers
                            .append("Referrer-Policy", "strict-origin-when-cross-origin");
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
                //
            } else {
                break;
            }
        }
        async_std::task::sleep(Duration::from_millis(600)).await
    }

    #[allow(unreachable_code)]
    Ok(())
}

fn get_on_msg_callback(r_out: mpsc::Sender<Vec<u8>>) -> Closure<dyn FnMut(MessageEvent)> {
    let r_out2 = r_out.clone();
    Closure::new(move |event: MessageEvent| {
        let _ = r_out2.send(Uint8Array::new(&event.data()).to_vec());
    })
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
