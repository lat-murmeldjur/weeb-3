use js_sys::{Array, Function, Object, Promise, Reflect};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Element, Event, HtmlMediaElement};

use super::{HLS_LIVE_EDGE_SEGMENTS, HlsStart, HlsStartupPlan};

#[rustfmt::skip]
const HLS_EVENTS: [&str; 4] = ["hlsManifestParsed", "hlsBufferAppended", "hlsFragBuffered", "hlsError"];
#[rustfmt::skip]
const NATIVE_EVENTS: [&str; 6] = ["loadedmetadata", "durationchange", "progress", "canplay", "canplaythrough", "error"];
const MEDIA_LIFECYCLE_EVENTS: [&str; 3] = ["play", "timeupdate", "durationchange"];
const BUFFER_EPSILON_SECONDS: f64 = 0.15;
const CLOCK_ADVANCE_EPSILON_SECONDS: f64 = 0.01;
const LIVE_RUNWAY_BUFFER: (f64, f64) = (90.0, 120.0);
const MAX_CONSECUTIVE_MEDIA_RECOVERIES: u8 = 1;
const MAX_HARD_RESTARTS: u8 = 2;

#[wasm_bindgen(module = "/static/hls_loader.js")]
extern "C" {
    #[wasm_bindgen(js_name = loadHls)]
    pub(super) fn load_hls() -> Promise;
}

#[wasm_bindgen]
extern "C" {
    #[derive(Clone)]
    type Hls;

    #[wasm_bindgen(catch, method, js_name = on)]
    fn on(this: &Hls, event: &str, callback: &Function) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_name = off)]
    fn off(this: &Hls, event: &str, callback: &Function) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_name = loadSource)]
    fn load_source(this: &Hls, source: &str) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_name = attachMedia)]
    fn attach_media(this: &Hls, media: &HtmlMediaElement) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_name = startLoad)]
    fn start_load_at(this: &Hls, position: f64) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_name = stopLoad)]
    fn stop_load(this: &Hls) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, method, js_name = recoverMediaError)]
    fn recover_media_error(this: &Hls) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_name = destroy)]
    fn destroy(this: &Hls) -> Result<(), JsValue>;
}

thread_local! {
    static ACTIVE: RefCell<Option<Player>> = const { RefCell::new(None) };
    static NATIVE: RefCell<Option<NativePlayer>> = const { RefCell::new(None) };
    static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

struct Player {
    id: u64,
    hls: Hls,
    hls_class: JsValue,
    source: String,
    media: HtmlMediaElement,
    callback: Closure<dyn FnMut(JsValue, JsValue)>,
    lifecycle: Closure<dyn FnMut(Event)>,
    plan: HlsStartupPlan,
    restore_autoplay: bool,
    live: bool,
    codec_bootstrap_pending: bool,
    live_runway_locked: bool,
    live_lock_pending: bool,
    ready: bool,
    consecutive_media_recoveries: u8,
    hard_restarts: u8,
    reload_position: Option<f64>,
    clock_started: bool,
    clock_origin: f64,
}

struct NativePlayer {
    id: u64,
    media: HtmlMediaElement,
    callback: Closure<dyn FnMut(Event)>,
    lifecycle: Closure<dyn FnMut(Event)>,
    plan: HlsStartupPlan,
    restore_autoplay: bool,
    live: bool,
    positioned: bool,
    ready: bool,
    clock_started: bool,
    clock_origin: f64,
}

enum Action {
    None,
    Start(Hls, f64),
    Play(HtmlMediaElement, f64),
    Retarget(Hls, HtmlMediaElement, f64),
    RecoverNetwork(Hls, f64),
    ReloadSource(Hls, String),
    RecoverMedia(Hls),
    HardRestart(String),
    ResolveRemoteTail {
        sequence: u64,
        reference: String,
        restart: f64,
        error_type: Option<String>,
        details: String,
    },
}

enum MediaAction {
    None,
    Begin(f64),
    Pause,
    Playing,
}

pub(super) fn play_hls(
    element: &Element,
    source: &str,
    hls_class: Result<JsValue, JsValue>,
    plan: HlsStartupPlan,
    start: HlsStart,
) -> Result<&'static str, JsValue> {
    destroy_current_hls();
    let media = element
        .clone()
        .dyn_into::<HtmlMediaElement>()
        .map_err(|_| JsValue::from_str("HLS requires an HTML media element"))?;
    let id = next_player_id();
    let native_supported = supports_native_hls(&media);
    let hls_class = match hls_class {
        Ok(hls_class) => hls_class,
        Err(_) if native_supported => return play_native(id, media, source, plan, start),
        Err(error) => return Err(error),
    };

    match hls_is_supported(&hls_class) {
        Ok(true) => {}
        Ok(false) | Err(_) if native_supported => {
            return play_native(id, media, source, plan, start);
        }
        Ok(false) => {
            return Err(JsValue::from_str(
                "This browser supports neither hls.js/MSE nor native HLS playback",
            ));
        }
        Err(error) => return Err(error),
    }

    let hls = construct_hls(&hls_class, &hls_config(start))?;
    let callback = Closure::new(move |event: JsValue, data: JsValue| {
        handle_event(id, event.as_string().as_deref().unwrap_or_default(), &data);
    });
    for event in HLS_EVENTS {
        if let Err(error) = hls.on(event, callback.as_ref().unchecked_ref()) {
            remove_hls_events(&hls, &callback);
            return Err(error);
        }
    }

    let lifecycle_media = media.clone();
    let lifecycle = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        handle_media_event(id, &lifecycle_media, &event.type_(), false);
    });
    if let Err(error) = add_media_listeners(&media, &lifecycle) {
        remove_hls_events(&hls, &callback);
        return Err(error);
    }
    if let Err(error) = media.set_attribute("data-weeb3-hls-mode", "hls.js") {
        remove_media_listeners(&media, &lifecycle);
        remove_hls_events(&hls, &callback);
        return Err(error);
    }
    let restore_autoplay = suspend_autoplay(&media);
    set_state(
        &media,
        "loading-manifest",
        "Loading HLS manifest from Swarm...",
    );

    let live = start == HlsStart::Live;
    let codec_bootstrap_pending = plan.codec_bootstrap;
    ACTIVE.with(|active| {
        *active.borrow_mut() = Some(Player {
            id,
            hls: hls.clone(),
            hls_class,
            source: source.to_string(),
            media: media.clone(),
            callback,
            lifecycle,
            clock_origin: plan.play_position,
            plan,
            restore_autoplay,
            live,
            codec_bootstrap_pending,
            live_runway_locked: false,
            live_lock_pending: false,
            ready: false,
            consecutive_media_recoveries: 0,
            hard_restarts: 0,
            reload_position: None,
            clock_started: false,
        });
    });

    if let Err(error) = hls
        .load_source(source)
        .and_then(|_| hls.attach_media(&media))
    {
        destroy_current_hls();
        return Err(error);
    }
    Ok("hls.js")
}

fn next_player_id() -> u64 {
    NEXT_ID.with(|next| {
        let id = next.get().wrapping_add(1).max(1);
        next.set(id);
        id
    })
}

fn play_native(
    id: u64,
    media: HtmlMediaElement,
    source: &str,
    plan: HlsStartupPlan,
    start: HlsStart,
) -> Result<&'static str, JsValue> {
    if !supports_native_hls(&media) {
        return Err(JsValue::from_str(
            "This browser supports neither hls.js/MSE nor native HLS playback",
        ));
    }
    let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        handle_native_event(id, &event.type_());
    });
    for event in NATIVE_EVENTS {
        if let Err(error) =
            media.add_event_listener_with_callback(event, callback.as_ref().unchecked_ref())
        {
            remove_native_events(&media, &callback);
            return Err(error);
        }
    }
    let lifecycle_media = media.clone();
    let lifecycle = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        handle_media_event(id, &lifecycle_media, &event.type_(), true);
    });
    if let Err(error) = add_media_listeners(&media, &lifecycle) {
        remove_native_events(&media, &callback);
        return Err(error);
    }
    if let Err(error) = media.set_attribute("data-weeb3-hls-mode", "native") {
        remove_media_listeners(&media, &lifecycle);
        remove_native_events(&media, &callback);
        return Err(error);
    }
    let restore_autoplay = suspend_autoplay(&media);
    NATIVE.with(|active| {
        *active.borrow_mut() = Some(NativePlayer {
            id,
            media: media.clone(),
            callback,
            lifecycle,
            clock_origin: plan.play_position,
            plan,
            restore_autoplay,
            live: start == HlsStart::Live,
            positioned: false,
            ready: false,
            clock_started: false,
        });
    });
    set_state(
        &media,
        "loading-manifest",
        "Loading native HLS manifest from Swarm...",
    );
    media.set_src(source);
    media.load();
    Ok("native HLS")
}

fn supports_native_hls(media: &HtmlMediaElement) -> bool {
    ["application/vnd.apple.mpegurl", "application/x-mpegURL"]
        .into_iter()
        .any(|mime| matches!(media.can_play_type(mime).as_str(), "probably" | "maybe"))
}

fn add_media_listeners(
    media: &HtmlMediaElement,
    listener: &Closure<dyn FnMut(Event)>,
) -> Result<(), JsValue> {
    for event in MEDIA_LIFECYCLE_EVENTS {
        if let Err(error) =
            media.add_event_listener_with_callback(event, listener.as_ref().unchecked_ref())
        {
            remove_media_listeners(media, listener);
            return Err(error);
        }
    }
    Ok(())
}

fn remove_media_listeners(media: &HtmlMediaElement, listener: &Closure<dyn FnMut(Event)>) {
    for event in MEDIA_LIFECYCLE_EVENTS {
        let _ = media.remove_event_listener_with_callback(event, listener.as_ref().unchecked_ref());
    }
}

fn remove_hls_events(hls: &Hls, callback: &Closure<dyn FnMut(JsValue, JsValue)>) {
    for event in HLS_EVENTS {
        let _ = hls.off(event, callback.as_ref().unchecked_ref());
    }
}

fn remove_native_events(media: &HtmlMediaElement, callback: &Closure<dyn FnMut(Event)>) {
    for event in NATIVE_EVENTS {
        let _ = media.remove_event_listener_with_callback(event, callback.as_ref().unchecked_ref());
    }
}

fn handle_media_event(id: u64, media: &HtmlMediaElement, event: &str, native: bool) {
    let action = if native {
        NATIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(player) = active.as_mut().filter(|player| player.id == id) else {
                return MediaAction::None;
            };
            let ready = player.ready;
            media_action(
                media,
                event,
                ready,
                &mut player.clock_started,
                &mut player.clock_origin,
            )
        })
    } else {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(player) = active.as_mut().filter(|player| player.id == id) else {
                return MediaAction::None;
            };
            if event == "durationchange"
                && !player.ready
                && !player.codec_bootstrap_pending
                && !lock_latest_live_plan(player)
                && let Some(position) =
                    playback_start_position(media, &player.plan, player.live, false)
            {
                player.ready = true;
                player.clock_origin = position;
                return MediaAction::Begin(position);
            }
            let ready = player.ready;
            media_action(
                media,
                event,
                ready,
                &mut player.clock_started,
                &mut player.clock_origin,
            )
        })
    };
    apply_media_action(
        media,
        action,
        if native {
            "Native HLS playback is advancing through weeb-3."
        } else {
            "HLS playback is advancing through weeb-3."
        },
    );
}

fn media_action(
    media: &HtmlMediaElement,
    event: &str,
    ready: bool,
    clock_started: &mut bool,
    clock_origin: &mut f64,
) -> MediaAction {
    if event == "play" && !ready {
        return MediaAction::Pause;
    }
    if event == "timeupdate"
        && ready
        && !*clock_started
        && !media.paused()
        && media.current_time() >= *clock_origin + CLOCK_ADVANCE_EPSILON_SECONDS
    {
        *clock_started = true;
        return MediaAction::Playing;
    }
    MediaAction::None
}

fn apply_media_action(media: &HtmlMediaElement, action: MediaAction, message: &str) {
    match action {
        MediaAction::None => {}
        MediaAction::Begin(position) => begin_playback(media.clone(), position),
        MediaAction::Pause => {
            let _ = media.pause();
        }
        MediaAction::Playing => set_state(media, "playing", message),
    }
}

fn handle_native_event(id: u64, event: &str) {
    let playback = NATIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(player) = active.as_mut().filter(|player| player.id == id) else {
            return None;
        };
        if event == "error" {
            set_state(&player.media, "error", "Native HLS playback failed");
            return None;
        }
        if !player.positioned && player.media.ready_state() > 0 {
            player.media.set_current_time(player.plan.play_position);
            player.positioned = true;
        }
        start_beginning_history_when_safe(&player.media, &player.plan, player.live);
        if player.ready {
            return None;
        }
        if let Some(position) =
            playback_start_position(&player.media, &player.plan, player.live, true)
        {
            player.ready = true;
            player.clock_origin = position;
            Some((player.media.clone(), Some(position)))
        } else {
            set_state(
                &player.media,
                "buffering",
                "Buffering the HLS startup runway...",
            );
            None
        }
    });
    if let Some((media, position)) = playback {
        if let Some(position) = position {
            begin_playback(media, position);
        } else {
            resume_playback(media);
        }
    }
}

fn handle_event(id: u64, event: &str, data: &JsValue) {
    let action = ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(player) = active.as_mut().filter(|player| player.id == id) else {
            return Action::None;
        };
        if matches!(event, "hlsBufferAppended" | "hlsFragBuffered") {
            start_beginning_history_when_safe(&player.media, &player.plan, player.live);
        }
        match event {
            "hlsManifestParsed" => {
                if let Some(position) = player.reload_position.take() {
                    set_state(
                        &player.media,
                        "recovering",
                        "HLS tail fallback loaded; resuming playback...",
                    );
                    player.media.set_current_time(position);
                    return Action::Start(player.hls.clone(), position);
                }
                set_state(
                    &player.media,
                    "manifest-ready",
                    "HLS manifest ready; buffering the playback runway...",
                );
                Action::Start(
                    player.hls.clone(),
                    if player.codec_bootstrap_pending {
                        player.plan.bootstrap_position
                    } else {
                        player.plan.play_position
                    } + if player.live {
                        BUFFER_EPSILON_SECONDS
                    } else {
                        0.0
                    },
                )
            }
            "hlsFragBuffered" if player.codec_bootstrap_pending => {
                if !is_main_fragment(data) {
                    return Action::None;
                }
                player.codec_bootstrap_pending = false;
                if lock_latest_live_plan(player) {
                    return Action::None;
                }
                if let Some(position) =
                    playback_start_position(&player.media, &player.plan, player.live, false)
                {
                    player.ready = true;
                    player.clock_origin = position;
                    Action::Play(player.media.clone(), position)
                } else {
                    Action::Retarget(
                        player.hls.clone(),
                        player.media.clone(),
                        player.plan.play_position,
                    )
                }
            }
            "hlsBufferAppended" | "hlsFragBuffered" if !player.ready => {
                let retarget = lock_latest_live_plan(player);
                if player.live_lock_pending {
                    return Action::None;
                }
                if let Some(position) =
                    playback_start_position(&player.media, &player.plan, player.live, false)
                {
                    player.ready = true;
                    player.clock_origin = position;
                    Action::Play(player.media.clone(), position)
                } else if retarget {
                    Action::Retarget(
                        player.hls.clone(),
                        player.media.clone(),
                        player.plan.play_position,
                    )
                } else {
                    set_state(
                        &player.media,
                        "buffering",
                        "Buffering the HLS startup runway...",
                    );
                    Action::None
                }
            }
            "hlsBufferAppended" | "hlsFragBuffered" => {
                if event == "hlsFragBuffered" {
                    player.consecutive_media_recoveries = 0;
                    player.media.remove_attribute("data-weeb3-hls-error").ok();
                }
                Action::None
            }
            "hlsError" if js_bool(data, "fatal") == Some(true) => {
                let details =
                    js_string(data, "details").unwrap_or_else(|| "fatal HLS error".to_string());
                player
                    .media
                    .set_attribute("data-weeb3-hls-error", &details)
                    .ok();
                if player.reload_position.is_some() {
                    return Action::HardRestart(details);
                }
                let restart = if player.ready {
                    player.media.current_time()
                } else if player.codec_bootstrap_pending {
                    player.plan.bootstrap_position + BUFFER_EPSILON_SECONDS
                } else {
                    player.plan.play_position
                        + if player.live {
                            BUFFER_EPSILON_SECONDS
                        } else {
                            0.0
                        }
                };
                if player.live
                    && matches!(
                        details.as_str(),
                        "fragLoadError" | "fragLoadTimeOut" | "fragParsingError"
                    )
                    && let Some((sequence, reference)) = failed_media_identity(data)
                {
                    Action::ResolveRemoteTail {
                        sequence,
                        reference,
                        restart,
                        error_type: js_string(data, "type"),
                        details,
                    }
                } else {
                    fatal_recovery_action(
                        player,
                        js_string(data, "type").as_deref(),
                        details,
                        restart,
                    )
                }
            }
            _ => Action::None,
        }
    });

    apply_action(id, action);
}

fn apply_action(id: u64, action: Action) {
    match action {
        Action::None => {}
        Action::Start(hls, position) | Action::RecoverNetwork(hls, position) => {
            let result = hls.start_load_at(position);
            finish_hls_action(id, &hls, "HLS loading failed", result);
        }
        Action::Play(media, position) => begin_playback(media, position),
        Action::Retarget(hls, media, position) => {
            spawn_local(async move {
                if !is_current_hls(id, &hls) {
                    return;
                }
                let target = position + BUFFER_EPSILON_SECONDS;
                let result = hls.stop_load().and_then(|_| {
                    media.set_current_time(target);
                    hls.start_load_at(target)
                });
                finish_hls_action(id, &hls, "HLS retarget failed", result);
            });
        }
        Action::ReloadSource(hls, source) => {
            let result = hls.stop_load().and_then(|_| hls.load_source(&source));
            finish_hls_action(id, &hls, "HLS source reload failed", result);
        }
        Action::RecoverMedia(hls) => {
            let result = hls.recover_media_error();
            finish_hls_action(id, &hls, "HLS media recovery failed", result);
        }
        Action::ResolveRemoteTail {
            sequence,
            reference,
            restart,
            error_type,
            details,
        } => resolve_remote_tail_failure(id, sequence, reference, restart, error_type, details),
        Action::HardRestart(message) => hard_restart(id, message),
    }
}

fn fatal_recovery_action(
    player: &mut Player,
    error_type: Option<&str>,
    details: String,
    restart: f64,
) -> Action {
    match error_type {
        Some("networkError") => Action::RecoverNetwork(player.hls.clone(), restart),
        Some("mediaError")
            if player.consecutive_media_recoveries < MAX_CONSECUTIVE_MEDIA_RECOVERIES =>
        {
            player.consecutive_media_recoveries += 1;
            Action::RecoverMedia(player.hls.clone())
        }
        _ => Action::HardRestart(details),
    }
}

fn resolve_remote_tail_failure(
    id: u64,
    sequence: u64,
    reference: String,
    restart: f64,
    error_type: Option<String>,
    details: String,
) {
    spawn_local(async move {
        let target = super::page_bridge::resolve_live_tail_failure(sequence, &reference).await;
        let action = ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(player) = active.as_mut().filter(|player| player.id == id) else {
                return Action::None;
            };
            if let Some(target) = target {
                player.consecutive_media_recoveries = 0;
                player.reload_position = Some(target);
                Action::ReloadSource(player.hls.clone(), player.source.clone())
            } else {
                fatal_recovery_action(player, error_type.as_deref(), details, restart)
            }
        });
        apply_action(id, action);
    });
}

fn finish_hls_action(id: u64, hls: &Hls, context: &str, result: Result<(), JsValue>) {
    let Err(error) = result else { return };
    if is_current_hls(id, hls) {
        hard_restart(id, format!("{context}: {}", js_error_message(&error)));
    }
}

fn is_current_hls(id: u64, hls: &Hls) -> bool {
    ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .is_some_and(|player| player.id == id && Object::is(player.hls.as_ref(), hls.as_ref()))
    })
}

fn hard_restart(id: u64, message: String) {
    type Launch = (Hls, Hls, String, HtmlMediaElement);
    type Failure = (HtmlMediaElement, String, bool);
    let replacement: Option<Result<Launch, Failure>> = ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let player = active.as_mut().filter(|player| player.id == id)?;
        if player.hard_restarts >= MAX_HARD_RESTARTS {
            return Some(Err((player.media.clone(), message, true)));
        }
        player.hard_restarts += 1;
        let runway = playback_runway(&player.plan);
        let mut position = player
            .reload_position
            .or_else(|| player.ready.then(|| player.media.current_time()))
            .unwrap_or(player.plan.play_position);
        if !position.is_finite() || position < 0.0 {
            position = player.plan.play_position;
        }
        player.plan.play_position = position;
        player.plan.runway_end = if player.live {
            position + runway
        } else {
            (position + runway).min(player.plan.duration)
        };
        let start = if player.live {
            HlsStart::Live
        } else {
            HlsStart::Beginning
        };
        let hls = match construct_hls(&player.hls_class, &hls_config(start)) {
            Ok(hls) => hls,
            Err(error) => {
                return Some(Err((
                    player.media.clone(),
                    js_error_message(&error),
                    player.hard_restarts >= MAX_HARD_RESTARTS,
                )));
            }
        };
        for event in HLS_EVENTS {
            if let Err(error) = hls.on(event, player.callback.as_ref().unchecked_ref()) {
                remove_hls_events(&hls, &player.callback);
                let _ = hls.destroy();
                return Some(Err((
                    player.media.clone(),
                    js_error_message(&error),
                    player.hard_restarts >= MAX_HARD_RESTARTS,
                )));
            }
        }
        remove_hls_events(&player.hls, &player.callback);
        let retired = std::mem::replace(&mut player.hls, hls.clone());
        player.reload_position = None;
        player.ready = false;
        player.codec_bootstrap_pending = player.plan.codec_bootstrap;
        player.consecutive_media_recoveries = 0;
        player.clock_started = false;
        player.clock_origin = position;
        player.media.remove_attribute("data-weeb3-hls-error").ok();
        Some(Ok((
            hls,
            retired,
            player.source.clone(),
            player.media.clone(),
        )))
    });
    match replacement {
        Some(Ok((hls, retired, source, media))) => {
            let _ = media.pause();
            let _ = retired.destroy();
            set_state(&media, "recovering", "Rebuilding HLS playback...");
            if let Err(error) = hls
                .load_source(&source)
                .and_then(|_| hls.attach_media(&media))
            {
                hard_restart(id, js_error_message(&error));
            }
        }
        Some(Err((_, error, false))) => hard_restart(id, error),
        Some(Err((media, error, true))) => {
            super::page_bridge::release_hls_view();
            set_state(&media, "error", &format!("HLS recovery failed: {error}"));
        }
        None => {}
    }
}

fn start_beginning_history_when_safe(media: &HtmlMediaElement, plan: &HlsStartupPlan, live: bool) {
    if !live && buffered_covers(media, plan.play_position, plan.runway_end) {
        let _ = super::page_bridge::start_beginning_history();
    }
}

fn begin_playback(media: HtmlMediaElement, position: f64) {
    let current = media.current_time();
    if !current.is_finite()
        || (current - position).abs() > BUFFER_EPSILON_SECONDS + CLOCK_ADVANCE_EPSILON_SECONDS
    {
        media.set_current_time(position);
    }
    resume_playback(media);
}

fn resume_playback(media: HtmlMediaElement) {
    set_state(
        &media,
        "starting",
        "HLS startup runway buffered; waiting for the media clock...",
    );
    match media.play() {
        Ok(promise) => {
            spawn_local(async move {
                if JsFuture::from(promise).await.is_err() {
                    set_state(
                        &media,
                        "ready",
                        "HLS media is buffered. Press play if autoplay is blocked.",
                    );
                }
            });
        }
        Err(_) => set_state(
            &media,
            "ready",
            "HLS media is buffered. Press play if autoplay is blocked.",
        ),
    }
}

fn playback_start_position(
    media: &HtmlMediaElement,
    plan: &HlsStartupPlan,
    live: bool,
    allow_infinite_duration: bool,
) -> Option<f64> {
    let duration = media.duration();
    let duration_ready = if live {
        (allow_infinite_duration && duration.is_infinite())
            || (duration.is_finite() && duration + BUFFER_EPSILON_SECONDS >= plan.runway_end)
    } else {
        duration.is_finite() && duration + BUFFER_EPSILON_SECONDS >= plan.duration
    };
    if !duration_ready {
        return None;
    }
    let buffer_end = plan.runway_end;
    if buffered_covers(media, plan.play_position, buffer_end) {
        return Some(plan.play_position);
    }
    if !live {
        return None;
    }
    let runway = plan.runway_end - plan.play_position;
    if !runway.is_finite() || runway <= 0.0 {
        return None;
    }
    let ranges = media.buffered();
    (0..ranges.length()).rev().find_map(|index| {
        ranges
            .start(index)
            .ok()
            .zip(ranges.end(index).ok())
            .and_then(|(range_start, range_end)| {
                let candidate = range_start.max(plan.play_position);
                (range_end > plan.play_position + BUFFER_EPSILON_SECONDS
                    && range_end + BUFFER_EPSILON_SECONDS >= candidate + runway)
                    .then_some(candidate)
            })
    })
}

fn lock_latest_live_plan(player: &mut Player) -> bool {
    if !player.live
        || !buffered_covers(
            &player.media,
            player.plan.play_position,
            player.plan.runway_end,
        )
    {
        return false;
    }
    if player.live_lock_pending {
        return true;
    }
    let was_locked = player.live_runway_locked;
    player.live_lock_pending = true;
    let id = player.id;
    spawn_local(async move {
        let plan = super::page_bridge::lock_live_plan().await;
        let action = ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let player = active.as_mut().filter(|player| player.id == id)?;
            player.live_lock_pending = false;
            let changed = if let Some(plan) = plan {
                let changed = !was_locked
                    && plan.play_position > player.plan.play_position + BUFFER_EPSILON_SECONDS;
                if !was_locked
                    || (plan.play_position > player.plan.play_position + BUFFER_EPSILON_SECONDS
                        && buffered_covers(&player.media, plan.play_position, plan.runway_end))
                {
                    player.plan = plan;
                }
                player.live_runway_locked = true;
                changed
            } else {
                false
            };
            if let Some(position) =
                playback_start_position(&player.media, &player.plan, true, false)
            {
                player.ready = true;
                player.clock_origin = position;
                Some(Action::Play(player.media.clone(), position))
            } else if changed {
                Some(Action::Retarget(
                    player.hls.clone(),
                    player.media.clone(),
                    player.plan.play_position,
                ))
            } else {
                set_state(
                    &player.media,
                    "buffering",
                    "Buffering the HLS startup runway...",
                );
                Some(Action::None)
            }
        });
        if let Some(action) = action {
            apply_action(id, action);
        }
    });
    true
}

#[rustfmt::skip]
fn playback_runway(plan: &HlsStartupPlan) -> f64 { plan.runway_end - plan.play_position }

fn buffered_covers(media: &HtmlMediaElement, start: f64, end: f64) -> bool {
    if !start.is_finite() || !end.is_finite() || end <= start {
        return false;
    }
    let ranges = media.buffered();
    (0..ranges.length()).any(|index| {
        ranges
            .start(index)
            .ok()
            .zip(ranges.end(index).ok())
            .is_some_and(|(range_start, range_end)| {
                range_start <= start + BUFFER_EPSILON_SECONDS
                    && range_end + BUFFER_EPSILON_SECONDS >= end
            })
    })
}

fn suspend_autoplay(media: &HtmlMediaElement) -> bool {
    let name = JsValue::from_str("autoplay");
    let requested = Reflect::get(media.as_ref(), &name)
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or_else(|| media.has_attribute("autoplay"));
    let _ = Reflect::set(media.as_ref(), &name, &JsValue::FALSE);
    media.remove_attribute("autoplay").ok();
    requested
}

fn restore_autoplay(media: &HtmlMediaElement, restore: bool) {
    if restore {
        media.set_attribute("autoplay", "").ok();
        let autoplay = JsValue::from_str("autoplay");
        let _ = Reflect::set(media.as_ref(), &autoplay, &JsValue::TRUE);
    }
}

pub(super) fn destroy_current_hls() {
    if let Some(player) = ACTIVE.with(|active| active.borrow_mut().take()) {
        remove_hls_events(&player.hls, &player.callback);
        remove_media_listeners(&player.media, &player.lifecycle);
        let _ = player.hls.destroy();
        let _ = player.media.pause();
        player.media.remove_attribute("data-weeb3-hls-mode").ok();
        player.media.remove_attribute("data-weeb3-hls-state").ok();
        player.media.remove_attribute("data-weeb3-hls-error").ok();
        restore_autoplay(&player.media, player.restore_autoplay);
    }
    if let Some(player) = NATIVE.with(|active| active.borrow_mut().take()) {
        remove_native_events(&player.media, &player.callback);
        remove_media_listeners(&player.media, &player.lifecycle);
        let _ = player.media.pause();
        player.media.remove_attribute("src").ok();
        player.media.load();
        player.media.remove_attribute("data-weeb3-hls-mode").ok();
        player.media.remove_attribute("data-weeb3-hls-state").ok();
        restore_autoplay(&player.media, player.restore_autoplay);
    }
}

fn hls_is_supported(hls_class: &JsValue) -> Result<bool, JsValue> {
    let function = Reflect::get(hls_class, &JsValue::from_str("isSupported"))?
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str("hls.js does not expose isSupported()"))?;
    function
        .call0(hls_class)?
        .as_bool()
        .ok_or_else(|| JsValue::from_str("hls.js isSupported() returned a non-boolean"))
}

fn construct_hls(hls_class: &JsValue, config: &Object) -> Result<Hls, JsValue> {
    let constructor = hls_class
        .dyn_ref::<Function>()
        .ok_or_else(|| JsValue::from_str("hls.js did not export a constructor"))?;
    let arguments = Array::new();
    arguments.push(config.as_ref());
    Reflect::construct(constructor, &arguments).map(JsCast::unchecked_into)
}

fn hls_config(start: HlsStart) -> Object {
    let config = Object::new();
    set(&config, "enableWorker", JsValue::TRUE);
    set(&config, "autoStartLoad", JsValue::FALSE);
    set(&config, "startFragPrefetch", JsValue::FALSE);
    set(&config, "progressive", JsValue::TRUE);
    let (buffer, maximum) = if start == HlsStart::Live {
        LIVE_RUNWAY_BUFFER
    } else {
        (180.0, 600.0)
    };
    for (name, value) in [
        ("maxBufferLength", buffer),
        ("maxMaxBufferLength", maximum),
        ("maxBufferSize", 128.0 * 1024.0 * 1024.0),
        ("backBufferLength", 15.0),
        ("maxBufferHole", 0.5),
    ] {
        set_number(&config, name, value);
    }
    if start == HlsStart::Live {
        set_number(
            &config,
            "liveSyncDurationCount",
            HLS_LIVE_EDGE_SEGMENTS as f64,
        );
    }
    let retry = Object::new();
    for (name, value) in [
        ("maxNumRetry", 2.0),
        ("retryDelayMs", 250.0),
        ("maxRetryDelayMs", 2_000.0),
    ] {
        set_number(&retry, name, value);
    }
    let defaults = Object::new();
    set_number(&defaults, "maxTimeToFirstByteMs", 240_000.0);
    set_number(&defaults, "maxLoadTimeMs", 250_000.0);
    set(&defaults, "timeoutRetry", JsValue::NULL);
    set(&defaults, "errorRetry", retry.into());
    let policy = Object::new();
    set(&policy, "default", defaults.into());
    for name in ["manifestLoadPolicy", "playlistLoadPolicy", "fragLoadPolicy"] {
        set(&config, name, policy.clone().into());
    }
    config
}

fn set(target: &Object, name: &str, value: JsValue) {
    let _ = Reflect::set(target.as_ref(), &JsValue::from_str(name), &value);
}

fn set_number(target: &Object, name: &str, value: f64) {
    set(target, name, JsValue::from_f64(value));
}

fn js_string(value: &JsValue, name: &str) -> Option<String> {
    js_property(value, name).and_then(|value| value.as_string())
}

fn js_bool(value: &JsValue, name: &str) -> Option<bool> {
    js_property(value, name).and_then(|value| value.as_bool())
}

fn js_error_message(error: &JsValue) -> String {
    js_string(error, "message")
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "unknown browser error".to_string())
}

fn failed_media_identity(data: &JsValue) -> Option<(u64, String)> {
    let fragment = js_property(data, "frag")?;
    let sequence = js_property(&fragment, "sn")?.as_f64()?;
    if !sequence.is_finite()
        || sequence < 0.0
        || sequence.fract() != 0.0
        || sequence > u64::MAX as f64
    {
        return None;
    }
    ["url", "relurl"].into_iter().find_map(|name| {
        let url = js_string(&fragment, name)?;
        let reference = url
            .split('?')
            .next()?
            .rsplit('/')
            .next()?
            .to_ascii_lowercase();
        super::is_hex_reference(&reference).then_some((sequence as u64, reference))
    })
}

fn js_property(value: &JsValue, name: &str) -> Option<JsValue> {
    Reflect::get(value, &JsValue::from_str(name)).ok()
}

fn is_main_fragment(value: &JsValue) -> bool {
    js_property(value, "frag")
        .and_then(|fragment| js_string(&fragment, "type"))
        .is_some_and(|kind| kind == "main")
}

fn set_state(media: &HtmlMediaElement, state: &str, message: &str) {
    media.set_attribute("data-weeb3-hls-state", state).ok();
    let Some(parent) = media.parent_element() else {
        return;
    };
    let Ok(Some(status)) = parent.query_selector(".weeb3-hls-status") else {
        return;
    };
    status.set_text_content(Some(message));
    status.set_attribute("data-state", state).ok();
}
