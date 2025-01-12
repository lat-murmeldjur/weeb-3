use crate::{decode_resources, init_panic_hook, Body};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use js_sys::{Array, Date, Uint8Array};
use wasm_bindgen::{closure::Closure, prelude::*, JsCast, JsError, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    console,
    Blob,
    BlobPropertyBag,
    Element,
    HtmlElement,
    HtmlInputElement,
    MessageEvent,
    Request,
    RequestInit,
    Response,
    // ResponseInit,
    ServiceWorker,
    ServiceWorkerRegistration,
    SharedWorker,
};

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

    let service2 = web_sys::window().unwrap().navigator().service_worker();

    match JsFuture::from(service2.register("./service.js")).await {
        Ok(registration) => {
            let _ = JsFuture::from(
                registration
                    .unchecked_into::<ServiceWorkerRegistration>()
                    .update()
                    .unwrap(),
            )
            .await;
            let _ = JsFuture::from(service2.ready().unwrap()).await;
        }
        Err(err) => {
            console::warn_1(&err);
        }
    }

    let registration: ServiceWorkerRegistration = JsFuture::from(service2.get_registration())
        .await
        .unwrap()
        .unchecked_into();
    let service_worker = registration.active().unwrap();
    service_worker
        .post_message(&JsValue::from(format!("from host2 {:#?}", host2)))
        .unwrap();

    let body = Body::from_current_window()?;

    let (r_out, r_in) = mpsc::channel::<Vec<u8>>();

    let worker_handle = Rc::new(RefCell::new(
        SharedWorker::new_with_worker_options("./worker.js", &{
            let opts = web_sys::WorkerOptions::new();
            opts.set_type(web_sys::WorkerType::Module);
            opts
        })
        .unwrap(),
    ));

    {
        let worker_handle_2 = &*worker_handle.borrow();
        let port = worker_handle_2.port();
        let _qxy = port.start();
    }

    let document = web_sys::window().unwrap().document().unwrap();

    #[allow(unused_assignments)]
    let mut persistent_callback_handle = get_on_msg_callback(r_out.clone());

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

            // If the value in the field can be parsed to a `i32`, send it to the
            // worker. Otherwise clear the result field.
            match input_field.value().parse::<String>() {
                Ok(text) => {
                    console::log_1(&"oninput callback string".into());
                    // Access worker behind shared handle, following the interior
                    // mutability pattern.
                    let worker_handle_2 = worker_handle.borrow();
                    let _ = worker_handle_2.port().post_message(&text.into());
                    persistent_callback_handle = get_on_msg_callback(r_out.clone());

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

    body.append_p(&format!("Created a new worker from within Wasm"))?;

    loop {
        #[allow(irrefutable_let_patterns)]
        while let data0 = r_in.try_recv() {
            if !data0.is_err() {
                let data = decode_resources(data0.unwrap());
                web_sys::console::log_1(&JsValue::from(format!(
                    "data array length {:#?}",
                    data.len()
                )));

                if data.len() == 1 {
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

                    // web_sys::console::log_2(&"Received data: ".into(), &JsValue::from(data));
                    // web_sys::console::log_2(&"Received string: ".into(), &JsValue::from(&string));

                    let document = web_sys::window().unwrap().document().unwrap();

                    let _r = document
                        .get_element_by_id("resultField")
                        .expect("#resultField should exist")
                        .dyn_ref::<HtmlElement>()
                        .unwrap()
                        .append_child(&new_element)
                        .unwrap();
                } else {
                    let date3 = Date::now().to_string();

                    for (data3, mime3, path3) in data {
                        let opts = RequestInit::new();

                        opts.set_method("GET");

                        let req_headers = web_sys::Headers::new().unwrap();
                        let _ = req_headers
                            .append("Referrer-Policy", "strict-origin-when-cross-origin");
                        // let _ = req_headers.append("access-control-allow-origin", "*");
                        // let _ = req_headers.append("Cache-Control", "public, max-age=6000000");
                        opts.set_headers(&req_headers);

                        // let dlen = data3.len().to_string();

                        let sep = "/".to_string();
                        let mut path03 = host2.clone();
                        path03.push_str(&sep);
                        path03.push_str(&date3);
                        path03.push_str(&sep);
                        path03.push_str(&path3);

                        let props = BlobPropertyBag::new();
                        props.set_type(&mime3);
                        let data2: Uint8Array = JsValue::from(data3).into();
                        let bytes = Array::new();
                        bytes.push(&data2);

                        let mut msgobj = js_sys::Object::new();

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

                        let _ = service_worker.post_message(&JsValue::from(msgobj));

                        let mut got = false;

                        while !got {
                            async_std::task::sleep(Duration::from_millis(600)).await;

                            let opts = RequestInit::new();
                            opts.set_method("GET");
                            let request = Request::new_with_str_and_init(&path03, &opts).unwrap();
                            let window = web_sys::window().unwrap();
                            let resp_value = JsFuture::from(window.fetch_with_request(&request))
                                .await
                                .unwrap();
                            assert!(resp_value.is_instance_of::<Response>());

                            let resp: Response = resp_value.dyn_into().unwrap();

                            web_sys::console::log_1(&JsValue::from(format!(
                                "got {:#?}",
                                resp.status()
                            )));

                            if resp.status() == 200 {
                                got = true;
                            }
                        }
                    }

                    let sep = "/".to_string();
                    let mut path00 = host2.clone();
                    path00.push_str(&sep);
                    path00.push_str(&date3);
                    path00.push_str(&sep);
                    path00.push_str(&"index.html".to_string());

                    let new_element = create_ielement_wmt(path00);

                    let document = web_sys::window().unwrap().document().unwrap();

                    let _r = document
                        .get_element_by_id("resultField")
                        .expect("#resultField should exist")
                        .dyn_ref::<HtmlElement>()
                        .unwrap()
                        .append_child(&new_element)
                        .unwrap();

                    //                    let mut path03 = host2.clone();
                    //                    path03.push_str(&date3);
                    //                    path03.push_str("/index.html");
                    //
                    //                    let new_element = create_element_wmt("text/html".to_string(), path03);
                    //
                    //                    let document = web_sys::window().unwrap().document().unwrap();
                    //
                    //                    let _r = document
                    //                        .get_element_by_id("resultField")
                    //                        .expect("#resultField should exist")
                    //                        .dyn_ref::<HtmlElement>()
                    //                        .unwrap()
                    //                        .append_child(&new_element)
                    //                        .unwrap();
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

fn create_ielement_wmt(blob_url: String) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();

    let i = document.create_element("iframe").unwrap();
    let _ = i.set_attribute("src", &blob_url);
    return i;
}
