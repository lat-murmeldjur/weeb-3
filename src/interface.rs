use crate::{decode_resource, Body};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use js_sys::{Date, Uint8Array};
use wasm_bindgen::{closure::Closure, prelude::wasm_bindgen, JsCast, JsError, JsValue};
use web_sys::{console, HtmlElement, HtmlInputElement, MessageEvent, SharedWorker};

#[wasm_bindgen]
pub async fn interweeb(_st: String) -> Result<(), JsError> {
    let body = Body::from_current_window()?;
    // body.append_p(&format!("Initiating weeb worker:"))?;

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
    let mut persistent_callback_handle = get_on_msg_callback();

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
                    persistent_callback_handle = get_on_msg_callback();

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
        async_std::task::sleep(Duration::from_secs(60)).await
    }

    Ok(())
}

fn get_on_msg_callback() -> Closure<dyn FnMut(MessageEvent)> {
    Closure::new(move |event: MessageEvent| {
        let (data, string) = decode_resource(Uint8Array::new(&event.data()).to_vec());

        web_sys::console::log_2(&"Received data: ".into(), &JsValue::from(data));
        web_sys::console::log_2(&"Received string: ".into(), &JsValue::from(&string));

        let document = web_sys::window().unwrap().document().unwrap();
        document
            .get_element_by_id("resultField")
            .expect("#resultField should exist")
            .dyn_ref::<HtmlElement>()
            .expect("#resultField should be a HtmlInputElement")
            .set_inner_text(&format!("{:#?}", string));
    })
}
