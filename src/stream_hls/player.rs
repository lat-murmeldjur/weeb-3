use event_listener::Listener;
use js_sys::{Array, Function, Object, Promise, Reflect};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Element, Event, HtmlMediaElement, HtmlSelectElement};

use super::{
    HLS_BEGINNING_STARTUP_BUFFER_SECONDS, HLS_LIVE_STARTUP_BUFFER_SECONDS, HlsStart,
    HlsStartupPlan, PreparedHlsFeed,
};
use crate::{
    js_error_message,
    worker_protocol::{
        bool_property as js_bool, integer_property, number_property, set, set_number, set_string,
        string_property as js_string,
    },
};

#[rustfmt::skip]
const HLS_EVENTS: [&str; 7] = ["hlsManifestParsed", "hlsLevelUpdated", "hlsBufferCreated", "hlsFragLoading", "hlsBufferAppended", "hlsFragBuffered", "hlsError"];
#[rustfmt::skip]
const NATIVE_EVENTS: [&str; 6] = ["loadedmetadata", "durationchange", "progress", "canplay", "canplaythrough", "error"];
const MEDIA_LIFECYCLE_EVENTS: [&str; 10] = [
    "play",
    "playing",
    "pause",
    "ended",
    "timeupdate",
    "durationchange",
    "canplay",
    "seeking",
    "seeked",
    "waiting",
];
const MEDIA_SESSION_ACTIONS: [&str; 5] = ["play", "pause", "seekbackward", "seekforward", "seekto"];
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

    #[wasm_bindgen(catch, method, js_name = pauseBuffering)]
    fn pause_buffering(this: &Hls) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_name = resumeBuffering)]
    fn resume_buffering(this: &Hls) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_name = recoverMediaError)]
    fn recover_media_error(this: &Hls) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_name = destroy)]
    fn destroy(this: &Hls) -> Result<(), JsValue>;
}

thread_local! {
    static ACTIVE: RefCell<Option<Player>> = const { RefCell::new(None) };
    static NATIVE: RefCell<Option<NativePlayer>> = const { RefCell::new(None) };
    static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static MEDIA_SESSION: RefCell<Option<(Object, Closure<dyn FnMut(JsValue)>)>> = const { RefCell::new(None) };
    static SCREEN_LOCK: RefCell<Option<Rc<RefCell<Option<JsValue>>>>> = const { RefCell::new(None) };
}

struct Player {
    id: u64,
    hls: Hls,
    hls_class: JsValue,
    source: String,
    initial_source: Option<String>,
    quality: Option<QualityControl>,
    media: HtmlMediaElement,
    callback: Closure<dyn FnMut(JsValue, JsValue)>,
    lifecycle: Closure<dyn FnMut(Event)>,
    plan: HlsStartupPlan,
    restore_autoplay: bool,
    intent: PlaybackIntent,
    live: bool,
    codec_bootstrap_pending: bool,
    codec_recovery: Option<CodecRecovery>,
    initial_live: bool,
    live_lock_pending: bool,
    ready: bool,
    consecutive_media_recoveries: u8,
    hard_restarts: u8,
    decoder_wait: Option<(f64, event_listener::Event)>,
    reload_position: Option<f64>,
    clock_position: Option<f64>,
}

struct QualityControl {
    select: HtmlSelectElement,
    change: Closure<dyn FnMut(Event)>,
}

impl Drop for QualityControl {
    fn drop(&mut self) {
        let _ = self
            .select
            .remove_event_listener_with_callback("change", self.change.as_ref().unchecked_ref());
        self.select.remove();
    }
}

#[derive(Default)]
struct CodecRecovery {
    fragment: Option<(u64, String)>,
    video_ready: bool,
}

impl CodecRecovery {
    fn buffered(&self, fragment: Option<(u64, String)>) -> Option<bool> {
        self.fragment
            .as_ref()
            .filter(|expected| Some(*expected) == fragment.as_ref())
            .map(|_| self.video_ready)
    }
}

struct NativePlayer {
    id: u64,
    media: HtmlMediaElement,
    callback: Closure<dyn FnMut(Event)>,
    lifecycle: Closure<dyn FnMut(Event)>,
    plan: HlsStartupPlan,
    restore_autoplay: bool,
    intent: PlaybackIntent,
    live: bool,
    positioned: bool,
    ready: bool,
    clock_position: Option<f64>,
}

enum Action {
    None,
    Start(Hls, f64),
    PauseBuffering(Hls),
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
}

pub(super) fn play_hls(
    element: &Element,
    prepared: PreparedHlsFeed,
    hls_class: Result<JsValue, JsValue>,
    start: HlsStart,
) -> Result<&'static str, JsValue> {
    let PreparedHlsFeed {
        source,
        plan,
        initial_source,
    } = prepared;
    destroy_current_hls();
    let media = element
        .clone()
        .dyn_into::<HtmlMediaElement>()
        .map_err(|_| "HLS requires an HTML media element")?;
    let id = next_player_id();
    let native_supported = supports_native_hls(&media);
    let hls_class = match hls_class {
        Ok(hls_class) => hls_class,
        Err(_) if native_supported => return play_native(id, media, &source, plan, start),
        Err(error) => return Err(error),
    };

    match hls_is_supported(&hls_class) {
        Ok(true) => {}
        Ok(false) | Err(_) if native_supported => {
            return play_native(id, media, &source, plan, start);
        }
        Ok(false) => {
            return Err("This browser supports neither hls.js/MSE nor native HLS playback".into());
        }
        Err(error) => return Err(error),
    }

    let config = hls_config(start);
    // Incremental TS appends can silently leave video gaps after a live seek.
    if start == HlsStart::Live || plan.codec_bootstrap {
        fragment_loader(&hls_class, &config)?;
    }
    let hls = construct_hls(&hls_class, &config)?;
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
    set_state(&media, "loading-manifest", "Loading stream...");

    let live = start == HlsStart::Live;
    ACTIVE.with(|active| {
        *active.borrow_mut() = Some(Player {
            id,
            hls: hls.clone(),
            hls_class,
            source: source.clone(),
            initial_source,
            quality: None,
            media: media.clone(),
            callback,
            lifecycle,
            restore_autoplay,
            intent: PlaybackIntent::Play,
            live,
            codec_bootstrap_pending: plan.codec_bootstrap,
            codec_recovery: None,
            plan,
            initial_live: live,
            live_lock_pending: false,
            ready: false,
            consecutive_media_recoveries: 0,
            hard_restarts: 0,
            decoder_wait: None,
            reload_position: None,
            clock_position: None,
        });
    });

    let _ = install_media_session(&media);
    if let Err(error) = hls
        .load_source(&source)
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
            plan,
            restore_autoplay,
            intent: PlaybackIntent::Play,
            live: start == HlsStart::Live,
            positioned: false,
            ready: false,
            clock_position: None,
        });
    });
    set_state(&media, "loading-manifest", "Loading stream...");
    let _ = install_media_session(&media);
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
    if let Some(document) = web_sys::window().and_then(|window| window.document()) {
        document.add_event_listener_with_callback(
            "visibilitychange",
            listener.as_ref().unchecked_ref(),
        )?;
    }
    Ok(())
}

fn remove_media_listeners(media: &HtmlMediaElement, listener: &Closure<dyn FnMut(Event)>) {
    if let Some(document) = web_sys::window().and_then(|window| window.document()) {
        let _ = document.remove_event_listener_with_callback(
            "visibilitychange",
            listener.as_ref().unchecked_ref(),
        );
    }
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
    if matches!(event, "playing" | "pause" | "ended" | "visibilitychange") {
        let _ = update_media_session(media);
    }
    let mut seek = None;
    let action = if native {
        NATIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(player) = active.as_mut().filter(|player| player.id == id) else {
                return MediaAction::None;
            };
            media_action(
                media,
                event,
                player.ready,
                &mut player.intent,
                &mut player.clock_position,
            )
        })
    } else {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(player) = active.as_mut().filter(|player| player.id == id) else {
                return MediaAction::None;
            };
            if matches!(
                event,
                "play" | "playing" | "pause" | "ended" | "seeking" | "seeked" | "visibilitychange"
            ) || event == "waiting"
                && player.decoder_wait.as_ref().is_some_and(|(position, _)| {
                    (media.current_time() - position).abs() >= CLOCK_ADVANCE_EPSILON_SECONDS
                })
            {
                let waiting = cancel_decoder_recovery(player);
                if event == "visibilitychange" && waiting {
                    player.decoder_wait =
                        Some((media.current_time(), event_listener::Event::new()));
                }
            }
            if event == "waiting" && player.ready && media.ready_state() == 2 {
                player
                    .decoder_wait
                    .get_or_insert_with(|| (media.current_time(), event_listener::Event::new()));
            }
            schedule_decoder_recovery(player);
            if event == "seeking"
                && !player.codec_bootstrap_pending
                && (player.reload_position.is_some()
                    || if player.ready {
                        !position_buffered(media)
                    } else {
                        let position = media.current_time();
                        player.live
                            && player.initial_live
                            && position.is_finite()
                            && position >= 0.0
                            && (position - player.plan.play_position).abs()
                                > BUFFER_EPSILON_SECONDS + CLOCK_ADVANCE_EPSILON_SECONDS
                    })
            {
                seek = Some(player.hls.clone());
                let _ = player_fragment_loader(player);
                let finalized = consumed_playlist_finalized(&player.hls).unwrap_or(false);
                if let Some(plan) = seek_buffer_plan(media, &player.plan, finalized, player.live) {
                    player.plan = plan;
                    if !player.ready {
                        player.reload_position = Some(media.current_time());
                    }
                    player.live_lock_pending = false;
                    player.clock_position = None;
                }
            }
            if matches!(event, "durationchange" | "seeked" | "canplay")
                && !player.ready
                && !player.codec_bootstrap_pending
                && !lock_latest_live_plan(player)
                && let Some(position) = finish_buffering(player)
            {
                return MediaAction::Begin(position);
            }
            media_action(
                media,
                event,
                player.ready,
                &mut player.intent,
                &mut player.clock_position,
            )
        })
    };
    apply_media_action(media, action);
    if let Some(hls) = seek {
        let result = hls.start_load_at(media.current_time());
        finish_hls_action(id, &hls, "seeking", result);
    }
}

fn media_action(
    media: &HtmlMediaElement,
    event: &str,
    ready: bool,
    intent: &mut PlaybackIntent,
    clock_position: &mut Option<f64>,
) -> MediaAction {
    if event == "play" && media.paused() {
        return MediaAction::None;
    }
    if intent.event(event, ready) && media.paused() {
        return MediaAction::Begin(media.current_time());
    }
    if matches!(event, "seeking" | "waiting") {
        *clock_position = None;
        set_state(media, "buffering", "Buffering playback...");
    } else if *intent == PlaybackIntent::Paused {
        *clock_position = None;
        if matches!(event, "pause" | "ended") {
            set_state(media, "paused", "Paused");
        }
    } else if event == "play" {
        set_state(media, "starting", "Starting playback...");
    } else if matches!(event, "playing" | "seeked") && !media.seeking() {
        // A seek queues timeupdate before seeked; observe the settled position.
        *clock_position = Some(media.current_time());
    }
    if event == "timeupdate"
        && ready
        && clock_position.is_some_and(|position| {
            !media.paused()
                && !media.seeking()
                && media.current_time() >= position + CLOCK_ADVANCE_EPSILON_SECONDS
        })
    {
        *clock_position = None;
        set_state(media, "playing", "Playing");
    }
    MediaAction::None
}

fn apply_media_action(media: &HtmlMediaElement, action: MediaAction) {
    match action {
        MediaAction::None => {}
        MediaAction::Begin(position) => begin_playback(media.clone(), position),
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
        let position = if player.live {
            initial_live_position(&player.media, &player.plan, true)
        } else {
            playback_start_position(&player.media, &player.plan, false, true)
        };
        if let Some(position) = position {
            player.ready = true;
            Some((player.media.clone(), position))
        } else {
            set_state(&player.media, "buffering", "Buffering playback...");
            None
        }
    });
    if let Some((media, position)) = playback {
        begin_playback(media, position);
    }
}

fn handle_event(id: u64, event: &str, data: &JsValue) {
    let mut resume_bootstrap = None;
    let action = ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(player) = active.as_mut().filter(|player| player.id == id) else {
            return Action::None;
        };
        if matches!(event, "hlsBufferAppended" | "hlsFragBuffered") {
            start_beginning_history_when_safe(&player.media, &player.plan, player.live);
        }
        if event == "hlsBufferAppended" {
            schedule_decoder_recovery(player);
        }
        if event == "hlsError"
            && missing_video_source_buffer(data)
            && !player.codec_bootstrap_pending
            && player.codec_recovery.is_none()
            && player.hard_restarts < MAX_HARD_RESTARTS
        {
            player.codec_recovery = Some(CodecRecovery::default());
            player.plan.codec_bootstrap = true;
            return Action::HardRestart("HLS video codec initialization failed".into());
        }
        if event == "hlsError" && player.codec_bootstrap_pending {
            resume_bootstrap = Some(player.hls.clone());
        }
        match event {
            "hlsFragLoading" if player.codec_bootstrap_pending => {
                if let Some(codec) = player.codec_recovery.as_mut()
                    && codec.fragment.is_none()
                    && is_main_fragment(data)
                {
                    codec.fragment = failed_media_identity(data);
                }
                Action::None
            }
            "hlsBufferCreated" if player.codec_bootstrap_pending => {
                if let Some(codec) = player.codec_recovery.as_mut() {
                    codec.video_ready |= js_property(data, "tracks")
                        .and_then(|tracks| js_property(&tracks, "video"))
                        .is_some_and(|video| !video.is_null() && !video.is_undefined());
                }
                Action::None
            }
            "hlsManifestParsed" => {
                if let Some(source) = &player.initial_source
                    && let Err(error) = pin_initial_level(&player.hls, &player.media, source)
                {
                    return Action::HardRestart(js_error_message(&error));
                }
                if player.quality.is_none() {
                    match quality_control(id, &player.media, &player.hls) {
                        Ok(quality) => player.quality = quality,
                        Err(error) => return Action::HardRestart(js_error_message(&error)),
                    }
                }
                if player.codec_bootstrap_pending
                    && level_details(&player.hls).is_some()
                    && let Err(error) = codec_fragment_url(&player.hls, true)
                {
                    return Action::HardRestart(js_error_message(&error));
                }
                if let Some(position) = player.reload_position.take() {
                    set_state(&player.media, "recovering", "Resuming playback...");
                    player.media.set_current_time(position);
                    return Action::Start(player.hls.clone(), position);
                }
                set_state(&player.media, "manifest-ready", "Buffering playback...");
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
            // hls.js selects the next fragment before our FragBuffered callback.
            "hlsBufferAppended" if player.codec_bootstrap_pending && is_main_fragment(data) => {
                Action::PauseBuffering(player.hls.clone())
            }
            "hlsFragBuffered" if player.codec_bootstrap_pending => {
                if !is_main_fragment(data) {
                    return Action::None;
                }
                if let Some(codec) = &player.codec_recovery {
                    match codec.buffered(failed_media_identity(data)) {
                        None => return Action::None,
                        Some(false) => {
                            return Action::HardRestart(
                                "HLS codec segment has no video buffer".into(),
                            );
                        }
                        Some(true) => {}
                    }
                }
                if let Err(error) = codec_fragment_url(&player.hls, false) {
                    return Action::HardRestart(js_error_message(&error));
                }
                player.codec_bootstrap_pending = false;
                resume_bootstrap = Some(player.hls.clone());
                if lock_latest_live_plan(player) {
                    return Action::None;
                }
                if let Some(position) = finish_buffering(player) {
                    Action::Play(player.media.clone(), position)
                } else {
                    Action::Retarget(
                        player.hls.clone(),
                        player.media.clone(),
                        player.plan.play_position,
                    )
                }
            }
            "hlsLevelUpdated" if !player.ready => {
                if player.codec_bootstrap_pending
                    && let Err(error) = codec_fragment_url(&player.hls, true)
                {
                    return Action::HardRestart(js_error_message(&error));
                }
                if !player.initial_live {
                    return Action::None;
                }
                let Some(_) = consumed_playlist_finalized(&player.hls) else {
                    return Action::HardRestart("The live presentation was unavailable".into());
                };
                if !player.codec_bootstrap_pending
                    && !lock_latest_live_plan(player)
                    && let Some(position) = finish_buffering(player)
                {
                    Action::Play(player.media.clone(), position)
                } else {
                    Action::None
                }
            }
            "hlsBufferAppended" | "hlsFragBuffered" if !player.ready => {
                lock_latest_live_plan(player);
                if player.live_lock_pending {
                    return Action::None;
                }
                if let Some(position) = finish_buffering(player) {
                    Action::Play(player.media.clone(), position)
                } else {
                    set_state(&player.media, "buffering", "Buffering playback...");
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

    if let Some(hls) = resume_bootstrap {
        let result = hls.resume_buffering();
        if result.is_err() {
            finish_hls_action(id, &hls, "HLS buffering resume failed", result);
            return;
        }
    }
    apply_action(id, action);
}

fn apply_action(id: u64, action: Action) {
    if !matches!(action, Action::None | Action::PauseBuffering(..)) {
        ACTIVE.with(|active| {
            if let Some(player) = active
                .borrow_mut()
                .as_mut()
                .filter(|player| player.id == id)
            {
                cancel_decoder_recovery(player);
            }
        });
    }
    let manifest = matches!(action, Action::Start(..));
    match action {
        Action::None => {}
        Action::Start(hls, position) | Action::RecoverNetwork(hls, position) => {
            let result = hls.start_load_at(position);
            if manifest && result.is_ok() && is_current_hls(id, &hls) {
                let pin = ACTIVE.with(|active| {
                    active
                        .borrow()
                        .as_ref()
                        .is_some_and(|player| player.id == id && player.initial_source.is_some())
                });
                if pin && let Some(level) = number_property(hls.as_ref(), "startLevel") {
                    set_number(hls.unchecked_ref(), "loadLevel", level);
                }
                // Earlier reset makes startLoad request the playlist immediately.
                if let Some(details) = level_details(&hls)
                    && js_bool(&details, "live") == Some(true)
                {
                    set_number(details.unchecked_ref(), "requestScheduled", -1.0);
                }
            }
            finish_hls_action(id, &hls, "HLS loading failed", result);
        }
        Action::Play(media, position) => begin_playback(media, position),
        Action::PauseBuffering(hls) => {
            let result = hls.pause_buffering();
            finish_hls_action(id, &hls, "HLS buffering pause failed", result);
        }
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
    let Some(hls) = ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .filter(|player| player.id == id)
            .map(|player| player.hls.clone())
    }) else {
        return;
    };
    spawn_local(async move {
        let target = super::page_bridge::resolve_live_tail_failure(sequence, &reference).await;
        if !is_current_hls(id, &hls) {
            return;
        }
        let action = ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(player) = active.as_mut().filter(|player| player.id == id) else {
                return Action::None;
            };
            if let Some(target) = target {
                player.consecutive_media_recoveries = 0;
                player.reload_position = Some(target);
                player.initial_live = false;
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
        cancel_decoder_recovery(player);
        if player.hard_restarts >= MAX_HARD_RESTARTS {
            return Some(Err((player.media.clone(), message, true)));
        }
        player.hard_restarts += 1;
        let mut position = player
            .reload_position
            .or_else(|| player.ready.then(|| player.media.current_time()))
            .unwrap_or(player.plan.play_position);
        if !position.is_finite() || position < 0.0 {
            position = player.plan.play_position;
        }
        let Some(plan) =
            recovery_plan(&player.plan, position, player.media.duration(), player.live)
        else {
            return Some(Err((player.media.clone(), message, true)));
        };
        player.plan = plan;
        if player.initial_source.is_some() {
            player.initial_source = loaded_level(&player.hls)
                .and_then(|level| js_string(&level, "uri"))
                .or_else(|| player.initial_source.take());
        }
        let start = if player.live {
            HlsStart::Live
        } else {
            HlsStart::Beginning
        };
        let config = hls_config(start);
        if (player.live || player.codec_recovery.is_some())
            && let Err(error) = fragment_loader(&player.hls_class, &config)
        {
            return Some(Err((player.media.clone(), js_error_message(&error), true)));
        }
        let hls = match construct_hls(&player.hls_class, &config) {
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
        player.live_lock_pending = false;
        player.reload_position = None;
        player.ready = false;
        player.codec_bootstrap_pending = player.plan.codec_bootstrap;
        if let Some(codec) = player.codec_recovery.as_mut() {
            *codec = CodecRecovery::default();
        }
        player.consecutive_media_recoveries = 0;
        player.clock_position = None;
        player.intent.pause_internally(player.media.paused());
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
            let _ = retired.destroy();
            set_state(&media, "recovering", "Recovering playback...");
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
    if playback_intent(&media, None) {
        resume_playback(media);
    }
}

fn resume_playback(media: HtmlMediaElement) {
    set_state(&media, "starting", "Starting playback...");
    let position = media.current_time();
    media.set_hidden(false);
    let result = media.play();
    spawn_local(async move {
        let result = match result {
            Ok(promise) => JsFuture::from(promise).await,
            Err(error) => Err(error),
        };
        if let Err(error) = result
            && js_string(&error, "name").as_deref() != Some("AbortError")
            && media.paused()
            && !media.seeking()
            && media.current_time() == position
            && media.get_attribute("data-weeb3-hls-state").as_deref() == Some("starting")
            && playback_intent(&media, None)
        {
            set_state(
                &media,
                "ready",
                "Playback is ready. Press play to continue.",
            );
        }
    });
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
        if !live {
            let current = media.current_time();
            if !current.is_finite()
                || (current - plan.play_position).abs()
                    > BUFFER_EPSILON_SECONDS + CLOCK_ADVANCE_EPSILON_SECONDS
            {
                media.set_current_time(plan.play_position);
                return None;
            }
            if media.ready_state() < 3 {
                return None;
            }
        }
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

fn recovery_plan(
    plan: &HlsStartupPlan,
    position: f64,
    duration: f64,
    live: bool,
) -> Option<HlsStartupPlan> {
    let runway = playback_runway(plan);
    let duration = if !live && duration.is_finite() {
        plan.duration.max(duration)
    } else {
        plan.duration
    };
    let end = position + runway;
    let end = if live { end } else { end.min(duration) };
    (position.is_finite()
        && position >= 0.0
        && runway.is_finite()
        && runway > 0.0
        && end.is_finite()
        && end > position)
        .then(|| HlsStartupPlan {
            play_position: position,
            runway_end: end,
            duration,
            ..plan.clone()
        })
}

fn fragment_loader(hls_class: &JsValue, config: &Object) -> Result<(), JsValue> {
    let loader = js_property(hls_class, "DefaultConfig")
        .and_then(|defaults| js_property(&defaults, "loader"))
        .filter(JsValue::is_function)
        .ok_or("HLS complete-fragment loader is unavailable")?;
    Reflect::set(config, &JsValue::from_str("fLoader"), &loader)?
        .then_some(())
        .ok_or_else(|| "HLS fragment loader could not be changed".into())
}

fn loaded_level(hls: &Hls) -> Option<JsValue> {
    let levels = js_property(hls.as_ref(), "levels")?;
    let levels = Array::from(&levels);
    let index = integer_property(hls.as_ref(), "loadLevel")
        .and_then(|index| u32::try_from(index).ok())
        .or_else(|| (levels.length() == 1).then_some(0))?;
    (index < levels.length()).then(|| levels.get(index))
}

fn level_details(hls: &Hls) -> Option<JsValue> {
    js_property(&loaded_level(hls)?, "details")
        .filter(|details| !details.is_undefined() && !details.is_null())
}

fn pin_initial_level(hls: &Hls, media: &HtmlMediaElement, source: &str) -> Result<(), JsValue> {
    let source =
        web_sys::Url::new_with_base(source, &media.base_uri()?.unwrap_or_default())?.href();
    let levels = js_property(hls.as_ref(), "levels").ok_or("HLS levels are unavailable")?;
    let index = Array::from(&levels)
        .iter()
        .position(|level| js_string(&level, "uri").as_ref() == Some(&source))
        .ok_or("The prepared HLS rendition is unavailable")?;
    set_number(hls.unchecked_ref(), "startLevel", index as f64);
    Ok(())
}

fn quality_control(
    id: u64,
    media: &HtmlMediaElement,
    hls: &Hls,
) -> Result<Option<QualityControl>, JsValue> {
    let levels =
        Array::from(&js_property(hls.as_ref(), "levels").ok_or("HLS levels are unavailable")?);
    if levels.length() < 2 {
        return Ok(None);
    }
    let document = media
        .owner_document()
        .ok_or("The player document is unavailable")?;
    let select: HtmlSelectElement = document.create_element("select")?.unchecked_into();
    select.set_attribute("aria-label", "Video quality")?;
    select.set_attribute("data-weeb3-hls-quality", "")?;
    for (value, label) in std::iter::once((-1, "Auto".to_string())).chain(
        levels.iter().enumerate().map(|(index, level)| {
            let height = number_property(&level, "height").unwrap_or_default();
            let bitrate = number_property(&level, "bitrate").unwrap_or_default() / 1_000_000.0;
            let label = if height > 0.0 {
                format!("{height:.0}p ({bitrate:.1} Mbps)")
            } else {
                js_string(&level, "name")
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("{bitrate:.1} Mbps"))
            };
            (index as i32, label)
        }),
    ) {
        let option = document.create_element("option")?;
        option.set_attribute("value", &value.to_string())?;
        option.set_text_content(Some(&label));
        select.append_child(&option)?;
    }
    select.set_value("-1");
    select.set_disabled(true);
    let selected = select.clone();
    let change = Closure::new(move |_: Event| {
        let hls = ACTIVE.with(|active| {
            active
                .borrow()
                .as_ref()
                .filter(|player| player.id == id && player.ready)
                .map(|player| player.hls.clone())
        });
        if let Some(hls) = hls
            && let Ok(level) = selected.value().parse::<i32>()
        {
            set_number(hls.unchecked_ref(), "loadLevel", f64::from(level));
        }
    });
    select.add_event_listener_with_callback("change", change.as_ref().unchecked_ref())?;
    if let Some(parent) = media.parent_node() {
        parent.insert_before(&select, media.next_sibling().as_ref())?;
    }
    Ok(Some(QualityControl { select, change }))
}

fn consumed_playlist_finalized(hls: &Hls) -> Option<bool> {
    let details = level_details(hls)?;
    let finalized = !js_bool(&details, "live")?;
    (integer_property(&details, "startSN")? == 0).then_some(finalized)
}

fn codec_fragment_url(hls: &Hls, bootstrap: bool) -> Result<(), JsValue> {
    let fragment = level_details(hls)
        .and_then(|details| js_property(&details, "fragments"))
        .and_then(|fragments| {
            Array::from(&fragments)
                .iter()
                .find(|fragment| js_bool(fragment, "gap") != Some(true))
        })
        .ok_or("HLS codec fragment is unavailable")?;
    let url = js_string(&fragment, "url").ok_or("HLS codec URL is unavailable")?;
    let url = web_sys::Url::new(&url)?;
    if bootstrap {
        url.search_params().set("bootstrap", "1");
    } else {
        url.search_params().delete("bootstrap");
    }
    Reflect::set(
        &fragment,
        &JsValue::from_str("url"),
        &JsValue::from_str(&url.href()),
    )?
    .then_some(())
    .ok_or_else(|| "HLS codec URL could not be changed".into())
}

fn player_fragment_loader(player: &Player) -> Result<(), JsValue> {
    let config =
        js_property(player.hls.as_ref(), "config").ok_or("HLS configuration is unavailable")?;
    fragment_loader(&player.hls_class, &config.unchecked_into())
}

fn initial_live_position(
    media: &HtmlMediaElement,
    plan: &HlsStartupPlan,
    native: bool,
) -> Option<f64> {
    let duration = media.duration();
    let duration = if native && duration.is_infinite() {
        let seekable = media.seekable();
        seekable
            .length()
            .checked_sub(1)
            .and_then(|last| seekable.end(last).ok())?
    } else {
        duration
    };
    if !duration.is_finite()
        || duration + BUFFER_EPSILON_SECONDS < plan.duration
        || !buffered_covers(media, plan.play_position, plan.runway_end)
    {
        return None;
    }
    Some(plan.play_position)
}

fn player_start_position(player: &Player) -> Option<f64> {
    if player.media.seeking() {
        return None;
    }
    if player.reload_position.is_some()
        || (player.live && player.initial_live && player.hard_restarts == 0)
    {
        initial_live_position(&player.media, &player.plan, false)
    } else {
        playback_start_position(&player.media, &player.plan, player.live, false)
    }
}

fn finish_buffering(player: &mut Player) -> Option<f64> {
    let position = player_start_position(player)?;
    if player.initial_source.is_some() {
        let level = player.quality.as_ref().map_or(-1, |quality| {
            quality.select.set_disabled(false);
            quality.select.value().parse::<i32>().unwrap_or(-1)
        });
        set_number(player.hls.unchecked_ref(), "loadLevel", f64::from(level));
    }
    player.ready = true;
    player.initial_live = false;
    player.reload_position = None;
    Some(position)
}

fn lock_latest_live_plan(player: &mut Player) -> bool {
    if !player.live
        || player.reload_position.is_some()
        || (player.initial_live && player.hard_restarts == 0)
    {
        return false;
    }
    if player.live_lock_pending {
        return true;
    }
    if !buffered_covers(
        &player.media,
        player.plan.play_position,
        player.plan.runway_end,
    ) {
        return false;
    }
    player.live_lock_pending = true;
    let initial = player.initial_live;
    let id = player.id;
    let hls = player.hls.clone();
    spawn_local(async move {
        let plan = super::page_bridge::lock_live_plan().await;
        if !is_current_hls(id, &hls) {
            return;
        }
        let action = ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let player = active.as_mut().filter(|player| player.id == id)?;
            player.live_lock_pending = false;
            if player.ready || player.reload_position.is_some() || player.initial_live != initial {
                return None;
            }
            if let Some(plan) = plan
                && plan.play_position > player.plan.play_position + BUFFER_EPSILON_SECONDS
                && buffered_covers(&player.media, plan.play_position, plan.runway_end)
            {
                player.plan = plan;
            }
            finish_buffering(player).map(|position| Action::Play(player.media.clone(), position))
        });
        if let Some(action) = action {
            apply_action(id, action);
        }
    });
    true
}

#[derive(Clone, Copy, PartialEq)]
enum PlaybackIntent {
    Play,
    Paused,
    InternalPause,
}

impl PlaybackIntent {
    fn pause_internally(&mut self, paused: bool) -> bool {
        if paused {
            return false;
        }
        if *self != Self::Paused {
            *self = Self::InternalPause;
        }
        true
    }

    fn event(&mut self, event: &str, ready: bool) -> bool {
        match event {
            "pause" if *self == Self::InternalPause => {
                *self = Self::Play;
                ready
            }
            "pause" | "ended" => {
                *self = Self::Paused;
                false
            }
            "play" => {
                if *self != Self::InternalPause {
                    *self = Self::Play;
                }
                false
            }
            _ => false,
        }
    }
}

fn seek_buffer_plan(
    media: &HtmlMediaElement,
    plan: &HlsStartupPlan,
    finalized: bool,
    live: bool,
) -> Option<HlsStartupPlan> {
    let position = media.current_time();
    let duration = if media.duration().is_finite() {
        plan.duration.max(media.duration())
    } else {
        plan.duration
    };
    let end = position
        + if live {
            HLS_LIVE_STARTUP_BUFFER_SECONDS
        } else {
            HLS_BEGINNING_STARTUP_BUFFER_SECONDS
        };
    let end = if finalized { end.min(duration) } else { end };
    (position.is_finite() && position >= 0.0 && end > position).then(|| HlsStartupPlan {
        play_position: position,
        runway_end: end,
        duration,
        ..plan.clone()
    })
}

fn cancel_decoder_recovery(player: &mut Player) -> bool {
    if let Some((_, changed)) = player.decoder_wait.take() {
        changed.notify(usize::MAX);
        return true;
    }
    false
}

// Confirm a stalled decoder only after two seconds of media arrive; normal playback cancels the wait.
fn decoder_recovery_allowed(player: &Player, position: f64) -> bool {
    player.ready
        && player.intent == PlaybackIntent::Play
        && !player.media.paused()
        && !player.media.seeking()
        && !player.media.ended()
        && player.media.ready_state() == 2
        && position.is_finite()
        && (player.media.current_time() - position).abs() < CLOCK_ADVANCE_EPSILON_SECONDS
        && web_sys::window()
            .and_then(|window| window.document())
            .is_some_and(|document| !document.hidden())
        && {
            let ranges = player.media.buffered();
            (0..ranges.length()).any(|index| matches!((ranges.start(index), ranges.end(index)),
                (Ok(start), Ok(end)) if start <= position && end >= position + 2.0))
        }
}

fn schedule_decoder_recovery(player: &mut Player) {
    let Some((position, changed)) = &player.decoder_wait else {
        return;
    };
    let position = *position;
    if changed.total_listeners() != 0 || !decoder_recovery_allowed(player, position) {
        return;
    }
    let mut listener = changed.listen();
    let id = player.id;
    spawn_local(async move {
        if async_std::future::timeout(std::time::Duration::from_secs(1), &mut listener)
            .await
            .is_ok()
        {
            return;
        }
        let recovery = ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let player = active.as_mut().filter(|player| player.id == id)?;
            if !listener.listens_to(&player.decoder_wait.as_ref()?.1) {
                return None;
            }
            player.decoder_wait = None;
            decoder_recovery_allowed(player, position).then(|| player.media.clone())
        });
        if let Some(media) = recovery {
            media.set_current_time(media.current_time());
        }
    });
}

fn position_buffered(media: &HtmlMediaElement) -> bool {
    let ranges = media.buffered();
    let position = media.current_time();
    (0..ranges.length()).any(|index| {
        matches!((ranges.start(index), ranges.end(index)),
            (Ok(start), Ok(end)) if (start..end).contains(&position))
    })
}

fn playback_intent(media: &HtmlMediaElement, requested: Option<bool>) -> bool {
    match requested {
        Some(true) => set_state(media, "starting", "Starting playback..."),
        Some(false) => set_state(media, "paused", "Paused"),
        None => {}
    }
    let update = |intent: &mut PlaybackIntent| {
        match requested {
            Some(false) => *intent = PlaybackIntent::Paused,
            Some(true) if *intent != PlaybackIntent::InternalPause => {
                *intent = PlaybackIntent::Play
            }
            _ => {}
        }
        *intent == PlaybackIntent::Play
    };
    ACTIVE
        .with(|active| {
            active
                .borrow_mut()
                .as_mut()
                .filter(|player| player.media == *media)
                .map(|player| update(&mut player.intent) && player.ready)
        })
        .or_else(|| {
            NATIVE.with(|active| {
                active
                    .borrow_mut()
                    .as_mut()
                    .filter(|player| player.media == *media)
                    .map(|player| update(&mut player.intent) && player.ready)
            })
        })
        .unwrap_or(false)
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
    let requested = media.autoplay();
    media.set_autoplay(false);
    requested
}

fn restore_autoplay(media: &HtmlMediaElement, restore: bool) {
    if restore {
        media.set_autoplay(true);
    }
}

pub(super) fn destroy_current_hls() {
    release_screen_lock();
    if let Some((session, _callback)) = MEDIA_SESSION.with(|session| session.borrow_mut().take()) {
        if let Some(handler) = js_function(&session, "setActionHandler") {
            for action in MEDIA_SESSION_ACTIONS {
                let _ = handler.call2(&session, &JsValue::from_str(action), &JsValue::NULL);
            }
        }
        set_string(&session, "playbackState", "none");
        set(&session, "metadata", JsValue::NULL);
    }
    if let Some(mut player) = ACTIVE.with(|active| active.borrow_mut().take()) {
        cancel_decoder_recovery(&mut player);
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

fn install_media_session(media: &HtmlMediaElement) -> Option<()> {
    let window = web_sys::window()?;
    let session = js_property(&window.navigator(), "mediaSession").filter(JsValue::is_object)?;
    let handler = js_function(&session, "setActionHandler")?;
    let target = media.clone();
    let callback =
        Closure::new(
            move |details: JsValue| match js_string(&details, "action").as_deref() {
                Some("play") => {
                    if playback_intent(&target, Some(true)) {
                        resume_playback(target.clone());
                    }
                }
                Some("pause") => {
                    playback_intent(&target, Some(false));
                    let _ = target.pause();
                }
                action => {
                    let offset = js_property(&details, "seekOffset")
                        .and_then(|value| value.as_f64())
                        .unwrap_or(10.0);
                    let position = match action {
                        Some("seekbackward") => Some(target.current_time() - offset),
                        Some("seekforward") => Some(target.current_time() + offset),
                        Some("seekto") => {
                            js_property(&details, "seekTime").and_then(|value| value.as_f64())
                        }
                        _ => None,
                    };
                    if let Some(position) = position.filter(|position| position.is_finite()) {
                        target.set_current_time(position.max(0.0).min(target.duration().max(0.0)));
                    }
                }
            },
        );
    for action in MEDIA_SESSION_ACTIONS {
        let _ = handler.call2(&session, &JsValue::from_str(action), callback.as_ref());
    }
    let session: Object = session.unchecked_into();
    if let Some(constructor) = js_function(&window, "MediaMetadata") {
        let metadata = Object::new();
        set_string(
            &metadata,
            "title",
            media
                .get_attribute("aria-label")
                .unwrap_or_else(|| "Swarm HLS stream".to_string()),
        );
        set_string(&metadata, "artist", "weeb-3");
        if let Ok(metadata) = Reflect::construct(&constructor, &Array::of1(&metadata)) {
            set(&session, "metadata", metadata);
        }
    }
    MEDIA_SESSION.with(|active| *active.borrow_mut() = Some((session, callback)));
    Some(())
}

fn update_media_session(media: &HtmlMediaElement) -> Option<()> {
    let playing = !media.paused() && !media.ended();
    MEDIA_SESSION.with(|session| {
        if let Some((session, _)) = session.borrow().as_ref() {
            set_string(
                session,
                "playbackState",
                if playing { "playing" } else { "paused" },
            );
        }
    });
    let window = web_sys::window()?;
    let visible = window
        .document()
        .is_some_and(|document| js_bool(&document, "hidden") == Some(false));
    if !playing || !visible {
        release_screen_lock();
        return None;
    }
    let held = SCREEN_LOCK.with(|lock| {
        lock.borrow().as_ref().is_some_and(|lock| {
            lock.borrow()
                .as_ref()
                .is_none_or(|sentinel| js_bool(sentinel, "released") != Some(true))
        })
    });
    if held {
        return None;
    }
    let wake_lock = js_property(&window.navigator(), "wakeLock")?;
    let request = js_function(&wake_lock, "request")?;
    let promise = request
        .call1(&wake_lock, &JsValue::from_str("screen"))
        .ok()?
        .dyn_into::<Promise>()
        .ok()?;
    let pending = Rc::new(RefCell::new(None));
    SCREEN_LOCK.with(|lock| *lock.borrow_mut() = Some(pending.clone()));
    let media = media.clone();
    spawn_local(async move {
        let sentinel = JsFuture::from(promise).await.ok();
        let current = SCREEN_LOCK.with(|lock| {
            lock.borrow()
                .as_ref()
                .is_some_and(|lock| Rc::ptr_eq(lock, &pending))
        });
        if current && !media.paused() && !media.ended() {
            *pending.borrow_mut() = sentinel;
            if pending.borrow().is_none() {
                release_screen_lock()
            }
        } else if let Some(sentinel) = sentinel {
            release_wake_sentinel(sentinel);
        }
    });
    Some(())
}

fn release_screen_lock() {
    if let Some(lock) = SCREEN_LOCK.with(|lock| lock.borrow_mut().take())
        && let Some(sentinel) = lock.borrow_mut().take()
    {
        release_wake_sentinel(sentinel);
    }
}

fn release_wake_sentinel(sentinel: JsValue) {
    if let Some(release) = js_function(&sentinel, "release")
        && let Some(promise) = release
            .call0(&sentinel)
            .ok()
            .and_then(|value| value.dyn_into::<Promise>().ok())
    {
        spawn_local(async move {
            let _ = JsFuture::from(promise).await;
        });
    }
}

fn hls_is_supported(hls_class: &JsValue) -> Result<bool, JsValue> {
    let function = Reflect::get(hls_class, &JsValue::from_str("isSupported"))?
        .dyn_into::<Function>()
        .map_err(|_| "hls.js does not expose isSupported()")?;
    function
        .call0(hls_class)?
        .as_bool()
        .ok_or_else(|| "hls.js isSupported() returned a non-boolean".into())
}

fn construct_hls(hls_class: &JsValue, config: &Object) -> Result<Hls, JsValue> {
    let constructor = hls_class
        .dyn_ref::<Function>()
        .ok_or("hls.js did not export a constructor")?;
    Reflect::construct(constructor, &Array::of1(config)).map(JsCast::unchecked_into)
}

fn hls_config(start: HlsStart) -> Object {
    let config = Object::new();
    set(&config, "progressive", JsValue::TRUE);
    set(&config, "enableWorker", JsValue::TRUE);
    set(&config, "autoStartLoad", JsValue::FALSE);
    set(&config, "startFragPrefetch", JsValue::FALSE);
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
        set_number(&config, "liveSyncDuration", HLS_LIVE_STARTUP_BUFFER_SECONDS);
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

fn missing_video_source_buffer(data: &JsValue) -> bool {
    if js_bool(data, "fatal") != Some(true)
        || js_string(data, "type").as_deref() != Some("mediaError")
        || js_string(data, "details").as_deref() != Some("bufferAppendError")
        || js_string(data, "sourceBufferName").as_deref() != Some("video")
    {
        return false;
    }
    let Some(error) = js_property(data, "error") else {
        return false;
    };
    js_string(&error, "name").as_deref() == Some("HlsJsTrackRemovedError")
        && js_string(&error, "message").as_deref()
            == Some("Attempting to append to the video SourceBuffer, but it does not exist")
        && is_main_fragment(data)
        && failed_media_identity(data).is_some()
        && js_property(data, "frag")
            .and_then(|fragment| number_property(&fragment, "start"))
            .is_some_and(|start| start >= 0.0)
}

fn failed_media_identity(data: &JsValue) -> Option<(u64, String)> {
    let fragment = js_property(data, "frag")?;
    let sequence = integer_property(&fragment, "sn")?;
    ["url", "relurl"].into_iter().find_map(|name| {
        let url = js_string(&fragment, name)?;
        let reference = url.split('?').next()?.rsplit('/').next()?;
        super::is_hex_reference(reference).then(|| (sequence, reference.to_ascii_lowercase()))
    })
}

fn js_function(value: &JsValue, name: &str) -> Option<Function> {
    js_property(value, name)?.dyn_into::<Function>().ok()
}

fn js_property(value: &JsValue, name: &str) -> Option<JsValue> {
    Reflect::get(value, &JsValue::from_str(name)).ok()
}

fn is_main_fragment(value: &JsValue) -> bool {
    js_property(value, "frag").is_some_and(|fragment| {
        js_string(&fragment, "type").as_deref() == Some("main")
            && integer_property(&fragment, "sn").is_some()
    })
}

pub(super) fn set_state(media: &HtmlMediaElement, state: &str, message: &str) {
    if media.get_attribute("data-weeb3-hls-state").as_deref() == Some("paused")
        && !matches!(state, "starting" | "playing" | "error")
    {
        return;
    }
    media.set_attribute("data-weeb3-hls-state", state).ok();
    let busy = !matches!(state, "playing" | "ready" | "paused" | "error");
    media.set_hidden(busy && (media.hidden() || state == "loading-manifest"));
    let Some(parent) = media.parent_element() else {
        return;
    };
    let Ok(Some(status)) = parent.query_selector(".weeb3-hls-status") else {
        return;
    };
    status.set_text_content(Some(message));
    status.set_attribute("data-state", state).ok();
    status.toggle_attribute_with_force("hidden", busy).ok();
    status
        .set_attribute("aria-busy", if busy { "true" } else { "false" })
        .ok();
}
