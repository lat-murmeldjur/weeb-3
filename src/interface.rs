use crate::{decode_resource, Body};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use js_sys::{Array, Date, Uint8Array};
use wasm_bindgen::{closure::Closure, prelude::wasm_bindgen, JsCast, JsError, JsValue};
use web_sys::{
    console, Blob, BlobPropertyBag, Element, HtmlElement, HtmlInputElement, MessageEvent,
    SharedWorker, Url,
};

#[wasm_bindgen]
pub async fn interweeb(_st: String) -> Result<(), JsError> {
    let body = Body::from_current_window()?;
    // body.append_p(&format!("Initiating weeb worker:"))?;

    let (r_out, r_in) = mpsc::channel::<Vec<u8>>();

    let worker_handle = Rc::new(RefCell::new(
        SharedWorker::new_with_worker_options("./worker.js", &{
            let opts = web_sys::WorkerOptions::new();
            opts.set_type(web_sys::WorkerType::Module);
            opts
        })
        .unwrap(),
    ));

    // Pass the worker to the function which sets up the `oninput` callback.
    // body.append_p(&format!("Initializing interface:"))?;
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
        while let data0 = r_in.try_recv() {
            if !data0.is_err() {
                let (data, string) = decode_resource(data0.unwrap());
                web_sys::console::log_1(&JsValue::from(format!("data length {:#?}", data.len())));

                let mut props = BlobPropertyBag::new();
                props.set_type(&string);

                let data2: Uint8Array = JsValue::from(data).into();
                let bytes = Array::new();
                bytes.push(&data2);

                let blob = Blob::new_with_u8_array_sequence_and_options(&bytes, &props).unwrap();

                let blob_url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();

                let new_element = create_element_wmt(blob.type_(), blob_url);

                // web_sys::console::log_2(&"Received data: ".into(), &JsValue::from(data));
                // web_sys::console::log_2(&"Received string: ".into(), &JsValue::from(&string));

                let document = web_sys::window().unwrap().document().unwrap();

                let r = document
                    .get_element_by_id("resultField")
                    .expect("#resultField should exist")
                    .dyn_ref::<HtmlElement>()
                    .unwrap()
                    .append_child(&new_element)
                    .unwrap();

                //
            } else {
                break;
            }
        }
        async_std::task::sleep(Duration::from_millis(600)).await
    }

    Ok(())
}

fn get_on_msg_callback(r_out: mpsc::Sender<Vec<u8>>) -> Closure<dyn FnMut(MessageEvent)> {
    let r_out2 = r_out.clone();
    Closure::new(move |event: MessageEvent| {
        r_out2.send(Uint8Array::new(&event.data()).to_vec());

        //        let (data, string) = decode_resource(Uint8Array::new(&event.data()).to_vec());
        //        let mut props = BlobPropertyBag::new();
        //
        //        props.set_type(&string);
        //        let blob = Blob::new_with_buffer_source_sequence_and_options(
        //            &Array::from_iter([Uint8Array::from(data.as_slice()).buffer()]),
        //            &props,
        //        )
        //        .unwrap();
        //        let blob_url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();
        //        let new_element = create_element_wmt(blob, blob_url);
        //
        //        // web_sys::console::log_2(&"Received data: ".into(), &JsValue::from(data));
        //        // web_sys::console::log_2(&"Received string: ".into(), &JsValue::from(&string));
        //
        //        let document = web_sys::window().unwrap().document().unwrap();
        //
        //        let r = document
        //            .get_element_by_id("resultField")
        //            .expect("#resultField should exist")
        //            .dyn_ref::<HtmlElement>()
        //            .unwrap()
        //            .append_child(&new_element)
        //            .unwrap();
    })
}

fn create_element_wmt(tmype: String, blob_url: String) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();
    if tmype == "image/jpg" {
        let i = document.create_element("img").unwrap();
        i.set_attribute("src", &blob_url);
        return i;
    }

    let e = document.create_element("div").unwrap();
    e.set_inner_html("type not implemented");
    e
}
