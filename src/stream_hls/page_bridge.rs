use std::{cell::RefCell, rc::Rc, time::Duration};

use futures::future::join;
use js_sys::{Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Element, HtmlMediaElement};

use crate::{
    interface::{service_worker_controls_bzz_requests, service_worker_scope_protocol_error},
    shared_runtime::{SharedNodeClient, SharedRuntime, request_object},
    stream::{
        begin_result_view_request, replace_stream_result_view, result_view_request_is_current,
    },
    stream_conventions::HlsStart,
    stream_hls::{HlsStartupPlan, PreparedHlsFeed, player, protocol::plan_from_js},
    worker_protocol::{
        bool_property, integer_property, number_property, set_number, set_string, string_property,
    },
};

const HLS_PREPARE_TIMEOUT: Duration = Duration::from_secs(250);
const HLS_CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

thread_local! {
    static ACTIVE_HLS: RefCell<Option<RemoteHlsSession>> = const { RefCell::new(None) };
    static PENDING_HLS: RefCell<Option<PendingHlsAttempt>> = const { RefCell::new(None) };
}

struct RemoteHlsSession {
    id: u64,
    worker_session: u64,
    runtime: Rc<SharedRuntime>,
    network_id: u64,
    live: bool,
    beginning_history_started: bool,
}

struct PendingHlsAttempt {
    id: u64,
    runtime: Rc<SharedRuntime>,
    network_id: u64,
}

async fn control(
    runtime: &SharedRuntime,
    network_id: u64,
    kind: &str,
    session: u64,
) -> Result<Object, String> {
    let request = request_object(kind, network_id);
    set_number(&request, "session", session as f64);
    runtime.request(&request, HLS_CONTROL_TIMEOUT).await
}

async fn prepare_hls(
    client: Rc<SharedNodeClient>,
    owner: &str,
    topic: &str,
    start: HlsStart,
    view_generation: u64,
) -> Result<PreparedHlsFeed, String> {
    let runtime = client.ensure().await?;
    let network_id = client.network_id();
    if !result_view_request_is_current(view_generation) {
        return Err("HLS open was superseded".to_string());
    }
    if PENDING_HLS.with(|pending| pending.borrow().is_some()) {
        return Err("another HLS preparation is already pending".to_string());
    }
    PENDING_HLS.with(|pending| {
        *pending.borrow_mut() = Some(PendingHlsAttempt {
            id: view_generation,
            runtime: runtime.clone(),
            network_id,
        });
    });
    let request = request_object("WEEB3_HLS_PREPARE", network_id);
    set_number(&request, "attempt", view_generation as f64);
    set_string(&request, "owner", owner);
    set_string(&request, "topic", topic);
    set_string(
        &request,
        "start",
        if start == HlsStart::Live {
            "live"
        } else {
            "beginning"
        },
    );
    let prepared = runtime
        .request(&request, HLS_PREPARE_TIMEOUT)
        .await
        .and_then(parse_prepare_response);
    let (source, plan, worker_session) = match prepared {
        Ok(parsed) => parsed,
        Err(error) => {
            abandon_hls_attempt(view_generation, &runtime, network_id);
            return Err(error);
        }
    };

    let current = result_view_request_is_current(view_generation)
        && PENDING_HLS.with(|pending| {
            pending
                .borrow_mut()
                .take_if(|pending| pending.id == view_generation)
                .is_some()
        });
    if !current {
        notify_abandon(view_generation, &runtime, network_id);
        return Err("HLS preparation was abandoned".to_string());
    }
    ACTIVE_HLS.with(|active| {
        *active.borrow_mut() = Some(RemoteHlsSession {
            id: view_generation,
            worker_session,
            runtime,
            network_id,
            live: start == HlsStart::Live,
            beginning_history_started: false,
        });
    });
    Ok(PreparedHlsFeed { source, plan })
}

fn parse_prepare_response(response: Object) -> Result<(String, HlsStartupPlan, u64), String> {
    if bool_property(&response, "ok") != Some(true) {
        return Err(string_property(&response, "error")
            .unwrap_or_else(|| "SharedWorker HLS preparation failed".to_string()));
    }
    let source = string_property(&response, "source")
        .ok_or_else(|| "SharedWorker HLS preparation omitted source".to_string())?;
    let plan = Reflect::get(&response, &JsValue::from_str("plan"))
        .ok()
        .and_then(|value| plan_from_js(&value))
        .ok_or_else(|| "SharedWorker returned an invalid HLS plan".to_string())?;
    let session = integer_property(&response, "session")
        .ok_or_else(|| "SharedWorker HLS preparation omitted session".to_string())?;
    Ok((source, plan, session))
}

fn active_target(live: bool) -> Option<(u64, Rc<SharedRuntime>, u64, u64)> {
    ACTIVE_HLS.with(|active| {
        let active = active.borrow();
        let active = active.as_ref().filter(|active| active.live == live)?;
        Some((
            active.id,
            active.runtime.clone(),
            active.network_id,
            active.worker_session,
        ))
    })
}

pub(super) async fn lock_live_plan() -> Option<HlsStartupPlan> {
    let (id, runtime, network_id, session) = active_target(true)?;
    let response = control(&runtime, network_id, "WEEB3_HLS_LOCK", session)
        .await
        .ok()?;
    if bool_property(&response, "ok") != Some(true) {
        return None;
    }
    let plan = Reflect::get(&response, &JsValue::from_str("plan"))
        .ok()
        .and_then(|value| plan_from_js(&value))?;
    ACTIVE_HLS.with(|active| {
        active.borrow().as_ref().filter(|active| active.id == id)?;
        Some(plan)
    })
}

pub(super) fn start_beginning_history() -> bool {
    ACTIVE_HLS.with(|active| {
        let mut active = active.borrow_mut();
        let Some(active) = active
            .as_mut()
            .filter(|active| !active.live && !active.beginning_history_started)
        else {
            return false;
        };
        let request = request_object("WEEB3_HLS_BEGINNING_READY", active.network_id);
        set_number(&request, "session", active.worker_session as f64);
        active.beginning_history_started = active.runtime.notify(&request).is_ok();
        active.beginning_history_started
    })
}

fn release_hls() {
    release_hls_matching(None);
}

fn release_hls_for_view(view_generation: u64) {
    release_hls_matching(Some(view_generation));
}

fn release_hls_matching(view_generation: Option<u64>) {
    let active = ACTIVE_HLS.with(|active| {
        active
            .borrow_mut()
            .take_if(|active| view_generation.is_none_or(|id| active.id == id))
    });
    let pending = PENDING_HLS.with(|pending| {
        pending
            .borrow_mut()
            .take_if(|pending| view_generation.is_none_or(|id| pending.id == id))
    });
    if let Some(active) = active {
        let request = request_object("WEEB3_HLS_RELEASE", active.network_id);
        set_number(&request, "session", active.worker_session as f64);
        let _ = active.runtime.notify(&request);
    }
    if let Some(pending) = pending {
        notify_abandon(pending.id, &pending.runtime, pending.network_id);
    }
}

fn abandon_hls_attempt(id: u64, runtime: &SharedRuntime, network_id: u64) {
    let current = PENDING_HLS.with(|pending| {
        pending
            .borrow_mut()
            .take_if(|pending| pending.id == id)
            .is_some()
    });
    if current {
        notify_abandon(id, runtime, network_id);
    }
}

fn notify_abandon(id: u64, runtime: &SharedRuntime, network_id: u64) {
    let request = request_object("WEEB3_HLS_ABANDON", network_id);
    set_number(&request, "attempt", id as f64);
    let _ = runtime.notify(&request);
}

pub(super) async fn resolve_live_tail_failure(sequence: u64, reference: &str) -> Option<f64> {
    let (_, runtime, network_id, session) = active_target(true)?;
    let request = request_object("WEEB3_HLS_TAIL_FAILURE", network_id);
    set_number(&request, "session", session as f64);
    set_number(&request, "sequence", sequence as f64);
    set_string(&request, "reference", reference);
    let response = runtime.request(&request, HLS_CONTROL_TIMEOUT).await.ok()?;
    if bool_property(&response, "ok") != Some(true) {
        return None;
    }
    number_property(&response, "target").filter(|target| target.is_finite() && *target >= 0.0)
}

pub(crate) async fn attach_hls_feed_player(
    client: Rc<SharedNodeClient>,
    player_element: &Element,
    owner: String,
    topic: String,
    start: HlsStart,
    view_generation: u64,
) -> Result<&'static str, String> {
    let loader = JsFuture::from(player::load_hls());
    let worker_client = client.clone();
    let worker = async {
        service_worker_controls_bzz_requests(
            &worker_client,
            "HLS feed and segment requests",
            || result_view_request_is_current(view_generation),
        )
        .await
    };
    let prepare = prepare_hls(client, &owner, &topic, start, view_generation);
    let (hls_class, (worker_ready, prepared)) = join(loader, join(worker, prepare)).await;
    if !result_view_request_is_current(view_generation) {
        release_hls_for_view(view_generation);
        return Err("HLS open was superseded".to_string());
    }
    let prepared = prepared?;
    if !worker_ready {
        release_hls_view();
        return Err(service_worker_scope_protocol_error(
            "HLS feed and segment requests",
        ));
    }
    player::play_hls(
        player_element,
        &prepared.source,
        hls_class,
        prepared.plan,
        start,
    )
    .map_err(|error| {
        release_hls_view();
        format!("Could not initialize HLS: {}", js_error_message(&error))
    })
}

pub(crate) async fn open_hls_feed_view(
    client: Rc<SharedNodeClient>,
    owner: String,
    topic: String,
    start: HlsStart,
) {
    let view_generation = begin_result_view_request();
    let document = web_sys::window().unwrap().document().unwrap();
    let wrapper = document.create_element("section").unwrap();
    let player = document.create_element("video").unwrap();
    player.set_attribute("controls", "").ok();
    player.set_attribute("autoplay", "").ok();
    player.set_attribute("preload", "auto").ok();
    player.set_attribute("playsinline", "").ok();
    player
        .set_attribute("style", "width:90%;max-height:75vh;")
        .ok();
    player
        .set_attribute("aria-label", "Swarm HLS video stream")
        .ok();
    if let Some(media) = player.dyn_ref::<HtmlMediaElement>() {
        media.set_default_muted(true);
        media.set_muted(true);
    }
    let status = document.create_element("div").unwrap();
    status.set_class_name("weeb3-hls-status");
    status.set_attribute("role", "status").ok();
    status.set_text_content(Some("Discovering the HLS feed edge..."));
    wrapper.append_child(&player).ok();
    wrapper.append_child(&status).ok();
    if !replace_stream_result_view(&wrapper, view_generation) {
        return;
    }
    match attach_hls_feed_player(client, &player, owner, topic, start, view_generation).await {
        Ok(mode) if result_view_request_is_current(view_generation) => {
            status.set_text_content(Some(&format!(
                "HLS player attached with {mode}; buffering through weeb-3."
            )))
        }
        Err(error) if result_view_request_is_current(view_generation) => {
            status.set_text_content(Some(&error));
            status.set_attribute("data-state", "error").ok();
        }
        _ => {}
    }
}

pub(crate) fn release_hls_view() {
    release_hls();
    player::destroy_current_hls();
}

pub(crate) fn release_hls_for_bzz_view(client: &SharedNodeClient) {
    release_hls_view();
    client.clear_hls_cache();
}

fn js_error_message(error: &JsValue) -> String {
    Reflect::get(error, &JsValue::from_str("message"))
        .ok()
        .and_then(|message| message.as_string())
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "unknown browser error".to_string())
}
