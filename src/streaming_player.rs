use std::time::Duration;

use async_std::sync::Arc;
use js_sys::{Array, Function, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlElement, MessageEvent};

use crate::{
    Weeb3,
    bzz_stream::{BzzMetadata, bzz_reference_hex, normalize_bzz_path},
    interface::{get_service_worker, service_worker_missing},
};

pub fn handle_service_worker_message(
    obj: &js_sys::Object,
    event: &MessageEvent,
    weeb3: Arc<Weeb3>,
) -> bool {
    let ty = Reflect::get(obj, &JsValue::from_str("type")).unwrap_or(JsValue::NULL);

    if ty == JsValue::from_str("RESOLVE_BZZ_REQUEST") {
        handle_resolve_bzz_message(obj, event, weeb3);
        return true;
    }

    if ty == JsValue::from_str("RETRIEVE_RANGE_REQUEST") {
        handle_retrieve_range_message(obj, event, weeb3);
        return true;
    }

    if ty == JsValue::from_str("PREPARE_BZZ_STREAM_REQUEST") {
        handle_prepare_bzz_stream_message(obj, event, weeb3);
        return true;
    }

    false
}

pub async fn try_render_streaming_player(resource: String, metadata: BzzMetadata) -> bool {
    if !is_streamable_mime(&metadata.mime) {
        return false;
    }

    if !service_worker_controlled().await {
        service_worker_missing();
        return false;
    }

    let Some(src) = canonical_bzz_url(&resource, &metadata) else {
        return false;
    };

    let player = create_streaming_player(&metadata.mime, &src);
    replace_result_view(&player);
    install_playback_notifications(&player, &src);
    install_play_retries(&player);
    start_streaming_player(&player);
    true
}

fn handle_resolve_bzz_message(obj: &js_sys::Object, event: &MessageEvent, weeb3: Arc<Weeb3>) {
    let url = Reflect::get(obj, &JsValue::from_str("url")).unwrap_or(JsValue::NULL);
    let reference = url.as_string().unwrap_or_default();
    let port = message_port(event);

    spawn_local(async move {
        let resp = js_sys::Object::new();

        if let Some(metadata) = weeb3.resolve_bzz(reference).await {
            Reflect::set(&resp, &"ok".into(), &true.into()).unwrap();
            Reflect::set(&resp, &"type".into(), &"RESOLVE_BZZ_RESPONSE".into()).unwrap();
            Reflect::set(
                &resp,
                &"data_reference".into(),
                &hex::encode(metadata.data_reference).into(),
            )
            .unwrap();
            Reflect::set(&resp, &"mime".into(), &metadata.mime.clone().into()).unwrap();
            Reflect::set(
                &resp,
                &"size".into(),
                &JsValue::from_f64(metadata.size as f64),
            )
            .unwrap();
            Reflect::set(&resp, &"etag".into(), &metadata.etag.clone().into()).unwrap();
            Reflect::set(&resp, &"path".into(), &metadata.path.clone().into()).unwrap();
        } else {
            Reflect::set(&resp, &"ok".into(), &false.into()).unwrap();
        }

        if let Some(port) = port {
            let _ = port.post_message(&resp);
        }
    });
}

fn handle_retrieve_range_message(obj: &js_sys::Object, event: &MessageEvent, weeb3: Arc<Weeb3>) {
    let url = Reflect::get(obj, &JsValue::from_str("url")).unwrap_or(JsValue::NULL);
    let reference = url.as_string().unwrap_or_default();
    let start = Reflect::get(obj, &JsValue::from_str("start"))
        .unwrap_or(JsValue::from_f64(0.0))
        .as_f64()
        .unwrap_or(0.0)
        .max(0.0) as u64;
    let end_inclusive = Reflect::get(obj, &JsValue::from_str("end"))
        .unwrap_or(JsValue::from_f64(0.0))
        .as_f64()
        .unwrap_or(0.0)
        .max(0.0) as u64;
    let stream_key = js_string_property(obj, "stream_key").unwrap_or_default();
    let stream_generation = Reflect::get(obj, &JsValue::from_str("stream_generation"))
        .unwrap_or(JsValue::from_f64(0.0))
        .as_f64()
        .unwrap_or(0.0)
        .max(0.0) as u64;
    let resolved_metadata = metadata_from_range_message(obj);
    let port = message_port(event);

    spawn_local(async move {
        let resp = js_sys::Object::new();

        let range_result = if let Some(metadata) = resolved_metadata {
            if !stream_key.is_empty() && stream_generation > 0 {
                weeb3
                    .acquire_resolved_stream_range(
                        metadata,
                        start,
                        end_inclusive,
                        stream_key,
                        stream_generation,
                    )
                    .await
            } else {
                weeb3
                    .acquire_resolved_range(metadata, start, end_inclusive)
                    .await
            }
        } else {
            weeb3.acquire_range(reference, start, end_inclusive).await
        };

        if let Some((bytes, metadata)) = range_result {
            Reflect::set(&resp, &"ok".into(), &true.into()).unwrap();
            Reflect::set(&resp, &"type".into(), &"RETRIEVE_RANGE_RESPONSE".into()).unwrap();

            let u8arr = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
            u8arr.copy_from(&bytes);
            Reflect::set(&resp, &"body".into(), &u8arr).unwrap();
            Reflect::set(&resp, &"mime".into(), &metadata.mime.clone().into()).unwrap();
            Reflect::set(
                &resp,
                &"size".into(),
                &JsValue::from_f64(metadata.size as f64),
            )
            .unwrap();
            Reflect::set(&resp, &"etag".into(), &metadata.etag.clone().into()).unwrap();
            Reflect::set(&resp, &"path".into(), &metadata.path.clone().into()).unwrap();
        } else {
            Reflect::set(&resp, &"ok".into(), &false.into()).unwrap();
            Reflect::set(
                &resp,
                &"error".into(),
                &format!("failed to retrieve range {}-{}", start, end_inclusive).into(),
            )
            .unwrap();
        }

        if let Some(port) = port {
            let _ = port.post_message(&resp);
        }
    });
}

fn handle_prepare_bzz_stream_message(
    obj: &js_sys::Object,
    event: &MessageEvent,
    weeb3: Arc<Weeb3>,
) {
    let metadata = metadata_from_range_message(obj);
    let port = message_port(event);

    spawn_local(async move {
        let resp = js_sys::Object::new();
        let prepared = if let Some(metadata) = metadata {
            weeb3.prepare_bzz_stream(metadata).await
        } else {
            false
        };

        Reflect::set(&resp, &"ok".into(), &prepared.into()).unwrap();
        Reflect::set(&resp, &"type".into(), &"PREPARE_BZZ_STREAM_RESPONSE".into()).unwrap();

        if let Some(port) = port {
            let _ = port.post_message(&resp);
        }
    });
}

fn metadata_from_range_message(obj: &js_sys::Object) -> Option<BzzMetadata> {
    let data_reference = js_string_property(obj, "data_reference")
        .and_then(|reference| hex::decode(reference).ok())?;
    if data_reference.len() != 32 && data_reference.len() != 64 {
        return None;
    }

    let size = Reflect::get(obj, &JsValue::from_str("size"))
        .ok()
        .and_then(|size| size.as_f64())
        .filter(|size| *size >= 0.0)? as u64;

    Some(BzzMetadata {
        data_reference,
        mime: js_string_property(obj, "mime").unwrap_or_else(|| "application/octet-stream".into()),
        size,
        etag: js_string_property(obj, "etag").unwrap_or_default(),
        path: js_string_property(obj, "path").unwrap_or_default(),
        target_count: 1,
    })
}

fn js_string_property(obj: &js_sys::Object, name: &str) -> Option<String> {
    Reflect::get(obj, &JsValue::from_str(name))
        .ok()
        .and_then(|value| value.as_string())
}

fn message_port(event: &MessageEvent) -> Option<web_sys::MessagePort> {
    let ports: Array = event.ports().into();
    ports.get(0).dyn_into::<web_sys::MessagePort>().ok()
}

fn is_streamable_mime(mime: &str) -> bool {
    mime.starts_with("video/") || mime.starts_with("audio/")
}

fn canonical_bzz_url(resource: &str, metadata: &BzzMetadata) -> Option<String> {
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

    if path.is_empty() || path.starts_with("unknown") || path == "not found" {
        Some(format!("/weeb-3/bzz/{}", reference))
    } else {
        Some(format!("/weeb-3/bzz/{}/{}", reference, path))
    }
}

fn replace_result_view(new_element: &Element) {
    let document = web_sys::window().unwrap().document().unwrap();
    let result = document
        .get_element_by_id("resultField")
        .expect("#resultField should exist")
        .dyn_into::<HtmlElement>()
        .expect("#resultField should be a HtmlElement");

    result.set_inner_html("");
    let _ = result.append_child(new_element);
}

fn create_streaming_player(mime: &str, src: &str) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();
    let is_video = mime.starts_with("video/");
    let tag = if is_video { "video" } else { "audio" };
    let player = document.create_element(tag).unwrap();

    let _ = player.set_attribute("controls", "");
    let _ = player.set_attribute("autoplay", "");
    let _ = player.set_attribute("preload", "metadata");
    if is_video {
        let _ = player.set_attribute("playsinline", "");
    }
    let _ = Reflect::set(
        player.as_ref(),
        &JsValue::from_str("muted"),
        &JsValue::FALSE,
    );
    let _ = Reflect::set(
        player.as_ref(),
        &JsValue::from_str("defaultMuted"),
        &JsValue::FALSE,
    );
    let _ = Reflect::set(
        player.as_ref(),
        &JsValue::from_str("volume"),
        &JsValue::from_f64(1.0),
    );
    let _ = Reflect::set(
        player.as_ref(),
        &JsValue::from_str("autoplay"),
        &JsValue::TRUE,
    );
    let _ = player.set_attribute("src", src);
    let _ = player.set_attribute("style", "width:90%;max-height:75vh;");

    player
}

fn start_streaming_player(player: &Element) {
    let Ok(play) = Reflect::get(player.as_ref(), &JsValue::from_str("play")) else {
        return;
    };
    let Some(play) = play.dyn_ref::<Function>() else {
        return;
    };

    let _ = play.call0(player.as_ref());
}

fn install_playback_notifications(player: &Element, src: &str) {
    let src = src.to_string();
    let callback = Closure::<dyn FnMut()>::new(move || {
        notify_media_playing(&src);
    });

    let _ = player.add_event_listener_with_callback("playing", callback.as_ref().unchecked_ref());
    callback.forget();
}

fn install_play_retries(player: &Element) {
    for event_name in ["loadedmetadata", "loadeddata", "canplay"] {
        let player_for_callback = player.clone();
        let event_target = player.clone();
        let callback = Closure::<dyn FnMut()>::new(move || {
            start_streaming_player(&player_for_callback);
        });

        let _ = event_target
            .add_event_listener_with_callback(event_name, callback.as_ref().unchecked_ref());
        callback.forget();
    }
}

fn notify_media_playing(src: &str) {
    let service0 = web_sys::window().unwrap().navigator().service_worker();
    let Ok(controller) = Reflect::get(service0.as_ref(), &JsValue::from_str("controller")) else {
        return;
    };
    if controller.is_null() || controller.is_undefined() {
        return;
    }

    let message = Object::new();
    let _ = Reflect::set(
        &message,
        &JsValue::from_str("type"),
        &JsValue::from_str("BZZ_MEDIA_PLAYING"),
    );
    let _ = Reflect::set(&message, &JsValue::from_str("url"), &JsValue::from_str(src));

    let Ok(post_message) = Reflect::get(&controller, &JsValue::from_str("postMessage")) else {
        return;
    };
    let Some(post_message) = post_message.dyn_ref::<Function>() else {
        return;
    };

    let _ = post_message.call1(&controller, message.as_ref());
}

fn service_worker_has_controller() -> bool {
    let service0 = web_sys::window().unwrap().navigator().service_worker();
    Reflect::get(service0.as_ref(), &JsValue::from_str("controller"))
        .map(|controller| !controller.is_null() && !controller.is_undefined())
        .unwrap_or(false)
}

async fn service_worker_controlled() -> bool {
    if get_service_worker().await.is_none() {
        return false;
    }

    for _ in 0..50 {
        if service_worker_has_controller() {
            return true;
        }
        async_std::task::sleep(Duration::from_millis(100)).await;
    }

    false
}
