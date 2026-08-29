use std::cell::RefCell;

use js_sys::Object;
use wasm_bindgen::JsValue;

use crate::{
    stream_conventions::HlsStart,
    stream_hls::{
        HlsStartupPlan, HlsTailFailure, clear_hls_runtime_cache, install_live_tail_fallback,
        live_tail_failure_identity, lock_live_startup_plan, prepare_hls_feed, protocol::plan_to_js,
        release_hls_runtime, start_beginning_history,
    },
    worker_protocol::{integer_property, set, set_number, string_property},
    worker_runtime::{Weeb3WorkerRuntime, error_response, ok_response},
};

thread_local! {
    static REMOTE_TAIL_FAILURE: RefCell<HlsTailFailure> = Default::default();
    static REMOTE_HLS_PREPARE_KEY: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub(crate) async fn dispatch(
    runtime: &Weeb3WorkerRuntime,
    kind: &str,
    message: &Object,
) -> Option<Object> {
    if !kind.starts_with("WEEB3_HLS_") {
        return None;
    }
    if let Err(response) = runtime.request_network(message).await {
        return Some(response);
    }
    Some(match kind {
        "WEEB3_HLS_PREPARE" => prepare_hls_response(runtime, message).await,
        "WEEB3_HLS_CANCEL_PREPARE" => cancel_hls_prepare_response(message),
        "WEEB3_HLS_LOCK"
        | "WEEB3_HLS_BEGINNING_READY"
        | "WEEB3_HLS_RELEASE"
        | "WEEB3_HLS_TAIL_FAILURE" => hls_control_response(kind, message),
        "WEEB3_HLS_CLEAR_CACHE" => {
            clear_hls_runtime_cache();
            ok_response()
        }
        _ => return None,
    })
}

async fn prepare_hls_response(runtime: &Weeb3WorkerRuntime, message: &Object) -> Object {
    let Some(owner) = string_property(message, "owner") else {
        return error_response(400, "HLS preparation requires owner");
    };
    let Some(topic) = string_property(message, "topic") else {
        return error_response(400, "HLS preparation requires topic");
    };
    let start = match string_property(message, "start").as_deref() {
        Some("beginning") => HlsStart::Beginning,
        Some("live") => HlsStart::Live,
        _ => return error_response(400, "HLS start must be beginning or live"),
    };
    let Some(prepare_key) = string_property(message, "prepareKey") else {
        return error_response(400, "HLS preparation requires prepareKey");
    };
    let view_generation = crate::stream::begin_result_view_request();
    release_remote_hls_runtime();
    REMOTE_HLS_PREPARE_KEY.with(|current| *current.borrow_mut() = Some(prepare_key.clone()));
    let prepared =
        prepare_hls_feed(runtime.inner.clone(), owner, topic, start, view_generation).await;
    if prepared.is_err() {
        REMOTE_HLS_PREPARE_KEY.with(|current| {
            let mut current = current.borrow_mut();
            if current.as_deref() == Some(&prepare_key) {
                *current = None;
            }
        });
    }
    match prepared {
        Ok(prepared) => {
            let response = ok_response();
            set(&response, "source", JsValue::from_str(&prepared.source));
            set(&response, "plan", plan_to_js(&prepared.plan).into());
            set_number(&response, "session", view_generation as f64);
            response
        }
        Err(error) => error_response(502, error),
    }
}

fn hls_control_response(kind: &str, message: &Object) -> Object {
    let Some(session) = integer_property(message, "session") else {
        return error_response(400, "HLS control requires session");
    };
    if !crate::stream::result_view_request_is_current(session) {
        return error_response(409, "HLS session is no longer active");
    }
    match kind {
        "WEEB3_HLS_LOCK" => hls_plan_response(lock_live_startup_plan()),
        "WEEB3_HLS_BEGINNING_READY" => {
            start_beginning_history();
            ok_response()
        }
        "WEEB3_HLS_RELEASE" => {
            REMOTE_HLS_PREPARE_KEY.with(|current| *current.borrow_mut() = None);
            release_remote_hls_runtime();
            ok_response()
        }
        "WEEB3_HLS_TAIL_FAILURE" => hls_tail_failure_response(message),
        _ => unreachable!(),
    }
}

fn cancel_hls_prepare_response(message: &Object) -> Object {
    let Some(prepare_key) = string_property(message, "prepareKey") else {
        return error_response(400, "HLS cancellation requires prepareKey");
    };
    let current = REMOTE_HLS_PREPARE_KEY
        .with(|value| value.borrow().as_deref() == Some(prepare_key.as_str()));
    if current {
        crate::stream::begin_result_view_request();
        release_remote_hls_runtime();
    }
    ok_response()
}

fn hls_plan_response(plan: Option<HlsStartupPlan>) -> Object {
    let response = ok_response();
    set(
        &response,
        "plan",
        plan.as_ref()
            .map_or(JsValue::NULL, |plan| plan_to_js(plan).into()),
    );
    response
}

fn release_remote_hls_runtime() {
    release_hls_runtime();
    REMOTE_HLS_PREPARE_KEY.with(|current| *current.borrow_mut() = None);
    REMOTE_TAIL_FAILURE.with(|failure| failure.borrow_mut().clear());
}

fn hls_tail_failure_response(message: &Object) -> Object {
    let Some(sequence) = integer_property(message, "sequence") else {
        return error_response(400, "tail failure requires sequence");
    };
    let Some(reference) = string_property(message, "reference") else {
        return error_response(400, "tail failure requires reference");
    };
    let target = live_tail_failure_identity(sequence, &reference).and_then(
        |(snapshot, sequence, reference)| {
            REMOTE_TAIL_FAILURE
                .with(|failure| failure.borrow_mut().record(snapshot, sequence, &reference))
                .then(|| install_live_tail_fallback(snapshot, sequence, &reference))
                .flatten()
        },
    );
    if target.is_some() {
        REMOTE_TAIL_FAILURE.with(|failure| failure.borrow_mut().clear());
    }
    let response = ok_response();
    set(
        &response,
        "target",
        target.map(JsValue::from_f64).unwrap_or(JsValue::NULL),
    );
    response
}
