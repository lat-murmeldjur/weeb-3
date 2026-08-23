use futures::future::join;
use js_sys::{Array, Function, Object, Promise, Reflect};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Element, Event, HtmlMediaElement};

use super::{HLS_LIVE_SYNC_SEGMENTS, HlsStart, HlsStartupPlan};
use crate::stream_conventions::streaming_route_path;

#[rustfmt::skip]
const HLS_EVENTS: [&str; 6] = ["hlsManifestParsed", "hlsFragLoading", "hlsBufferCreated", "hlsBufferAppended", "hlsFragBuffered", "hlsError"];
#[rustfmt::skip]
const NATIVE_EVENTS: [&str; 6] = ["loadedmetadata", "durationchange", "progress", "canplay", "canplaythrough", "error"];
const MEDIA_LIFECYCLE_EVENTS: [&str; 4] = ["play", "seeking", "seeked", "timeupdate"];
const BUFFER_EPSILON_SECONDS: f64 = 0.15;
const SEEK_ALIGNMENT_SECONDS: f64 = 1.0;
const CLOCK_ADVANCE_EPSILON_SECONDS: f64 = 0.01;
const LIVE_RUNWAY_BUFFER: (f64, f64) = (90.0, 120.0);
const MAX_CONSECUTIVE_MEDIA_RECOVERIES: u8 = 1;

#[wasm_bindgen(module = "/static/hls_loader.js")]
extern "C" {
    #[wasm_bindgen(js_name = loadHls)]
    pub(super) fn load_hls() -> Promise;
}

pub(super) fn warm_live_runway(plan: &HlsStartupPlan) {
    let first = startup_request(&plan.references[0], HlsStart::Live);
    let second = startup_request(&plan.references[1], HlsStart::Live);
    spawn_local(async move {
        join(drain_startup_request(first), drain_startup_request(second)).await;
    });
}

pub(super) fn warm_startup_reference(reference: &str, start: HlsStart) {
    spawn_local(drain_startup_request(startup_request(reference, start)));
}

fn startup_request(reference: &str, start: HlsStart) -> Option<Promise> {
    let window = web_sys::window()?;
    let fetch = Reflect::get(&window, &JsValue::from_str("fetch"))
        .ok()?
        .dyn_into::<Function>()
        .ok()?;
    let base = streaming_route_path("hls/bytes");
    let mode = match start {
        HlsStart::Beginning => "beginning",
        HlsStart::Live => "live",
    };
    let url = JsValue::from_str(&format!("{base}/{reference}?start={mode}"));
    fetch.call1(&window, &url).ok()?.dyn_into::<Promise>().ok()
}

async fn drain_startup_request(request: Option<Promise>) {
    if let Some(request) = request
        && let Ok(response) = JsFuture::from(request).await
        && let Ok(body) = Reflect::get(&response, &JsValue::from_str("arrayBuffer"))
            .and_then(|value| value.dyn_into::<Function>())
            .and_then(|array_buffer| array_buffer.call0(&response))
            .and_then(|value| value.dyn_into::<Promise>())
    {
        let _ = JsFuture::from(body).await;
    }
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
    media: HtmlMediaElement,
    callback: Closure<dyn FnMut(JsValue, JsValue)>,
    lifecycle: Closure<dyn FnMut(Event)>,
    plan: HlsStartupPlan,
    restore_autoplay: bool,
    live: bool,
    live_bootstrap_pending: bool,
    beginning_prefetch_ready: bool,
    ready: bool,
    consecutive_media_recoveries: u8,
    clock_started: bool,
    clock_origin: f64,
    seek: Option<SeekGate>,
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
    seek: Option<SeekGate>,
}

#[derive(Clone, Copy)]
struct SeekGate {
    target: f64,
    resume: bool,
}

enum Action {
    None,
    Start(Hls, f64),
    Play(HtmlMediaElement, f64),
    Resume(HtmlMediaElement),
    Retarget(Hls, HtmlMediaElement, f64),
    RecoverNetwork(Hls, f64),
    RecoverMedia(Hls),
    Fail(HtmlMediaElement, String),
}

enum MediaAction {
    None,
    Pause,
    Resume,
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
    let callback =
        Closure::<dyn FnMut(JsValue, JsValue)>::new(move |event: JsValue, data: JsValue| {
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
    let live_bootstrap_pending = live && plan.codec_bootstrap;
    let clock_origin = plan.play_position;
    ACTIVE.with(|active| {
        *active.borrow_mut() = Some(Player {
            id,
            hls: hls.clone(),
            media: media.clone(),
            callback,
            lifecycle,
            plan,
            restore_autoplay,
            live,
            live_bootstrap_pending,
            beginning_prefetch_ready: false,
            ready: false,
            consecutive_media_recoveries: 0,
            clock_started: false,
            clock_origin,
            seek: None,
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
    let clock_origin = plan.play_position;
    NATIVE.with(|active| {
        *active.borrow_mut() = Some(NativePlayer {
            id,
            media: media.clone(),
            callback,
            lifecycle,
            plan,
            restore_autoplay,
            live: start == HlsStart::Live,
            positioned: false,
            ready: false,
            clock_started: false,
            clock_origin,
            seek: None,
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
            let runway = player.plan.runway_end - player.plan.play_position;
            if event == "seeking" && !player.live {
                super::runtime::start_beginning_history();
            }
            media_action(
                media,
                event,
                ready,
                &mut player.clock_started,
                &mut player.clock_origin,
                &mut player.seek,
                runway,
            )
        })
    } else {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(player) = active.as_mut().filter(|player| player.id == id) else {
                return MediaAction::None;
            };
            let ready = player.ready;
            let runway = player.plan.runway_end - player.plan.play_position;
            if event == "seeking" && !player.live {
                super::runtime::start_beginning_history();
            }
            media_action(
                media,
                event,
                ready,
                &mut player.clock_started,
                &mut player.clock_origin,
                &mut player.seek,
                runway,
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
    seek: &mut Option<SeekGate>,
    runway: f64,
) -> MediaAction {
    if event == "seeking" && ready && (*clock_started || seek.is_some()) {
        let target = media.current_time();
        if target.is_finite() && target >= 0.0 {
            let resume = seek.map_or(!media.paused(), |pending| pending.resume);
            *seek = Some(SeekGate { target, resume });
            *clock_started = false;
            *clock_origin = target;
            set_state(
                media,
                "buffering",
                "Buffering two HLS segments after seek...",
            );
            return MediaAction::Pause;
        }
    }
    if event == "play" {
        if !ready {
            return MediaAction::Pause;
        }
        if let Some(pending) = seek.as_mut() {
            pending.resume = true;
            if settle_seek(media, runway, seek) == Some(true) {
                set_state(
                    media,
                    "starting",
                    "Seek runway buffered; waiting for the media clock...",
                );
                return MediaAction::None;
            }
            return MediaAction::Pause;
        }
    }
    if event == "seeked" && settle_seek(media, runway, seek) == Some(true) {
        return MediaAction::Resume;
    }
    if event == "timeupdate"
        && ready
        && seek.is_none()
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
        MediaAction::Pause => {
            let _ = media.pause();
        }
        MediaAction::Resume => resume_playback(media.clone()),
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
        let _ = start_beginning_history_when_safe(&player.media, &player.plan, player.live);
        if player.ready {
            let runway = player.plan.runway_end - player.plan.play_position;
            return (settle_seek(&player.media, runway, &mut player.seek) == Some(true))
                .then(|| (player.media.clone(), None));
        }
        if playback_ready(&player.media, &player.plan, player.live, true) {
            player.ready = true;
            Some((player.media.clone(), Some(player.plan.play_position)))
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
        if matches!(event, "hlsBufferAppended" | "hlsFragBuffered")
            && start_beginning_history_when_safe(&player.media, &player.plan, player.live)
        {
            player.beginning_prefetch_ready = true;
        }
        match event {
            "hlsManifestParsed" => {
                set_state(
                    &player.media,
                    "manifest-ready",
                    "HLS manifest ready; buffering the playback runway...",
                );
                Action::Start(
                    player.hls.clone(),
                    if player.live_bootstrap_pending {
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
            "hlsBufferCreated" | "hlsFragBuffered"
                if player.live && player.live_bootstrap_pending =>
            {
                let initialized = if event == "hlsBufferCreated" {
                    has_video_track(data)
                } else {
                    is_main_fragment(data)
                };
                if !initialized {
                    return Action::None;
                }
                player.live_bootstrap_pending = false;
                Action::Retarget(
                    player.hls.clone(),
                    player.media.clone(),
                    player.plan.play_position,
                )
            }
            "hlsFragLoading" if !player.live && player.beginning_prefetch_ready => {
                if let Some(reference) = fragment_reference(data) {
                    super::runtime::warm_beginning_successor(&reference);
                }
                Action::None
            }
            "hlsBufferAppended" | "hlsFragBuffered" if !player.ready => {
                if playback_ready(&player.media, &player.plan, player.live, false) {
                    player.ready = true;
                    Action::Play(player.media.clone(), player.plan.play_position)
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
                player.consecutive_media_recoveries = 0;
                let runway = player.plan.runway_end - player.plan.play_position;
                if settle_seek(&player.media, runway, &mut player.seek) == Some(true) {
                    Action::Resume(player.media.clone())
                } else {
                    Action::None
                }
            }
            "hlsError" if js_bool(data, "fatal") == Some(true) => {
                let details =
                    js_string(data, "details").unwrap_or_else(|| "fatal HLS error".to_string());
                player
                    .media
                    .set_attribute("data-weeb3-hls-error", &details)
                    .ok();
                match js_string(data, "type").as_deref() {
                    Some("networkError") => Action::RecoverNetwork(
                        player.hls.clone(),
                        if player.ready {
                            player.media.current_time()
                        } else if player.live_bootstrap_pending {
                            player.plan.bootstrap_position + BUFFER_EPSILON_SECONDS
                        } else {
                            player.plan.play_position
                                + if player.live {
                                    BUFFER_EPSILON_SECONDS
                                } else {
                                    0.0
                                }
                        },
                    ),
                    Some("mediaError")
                        if player.consecutive_media_recoveries
                            < MAX_CONSECUTIVE_MEDIA_RECOVERIES =>
                    {
                        player.consecutive_media_recoveries += 1;
                        Action::RecoverMedia(player.hls.clone())
                    }
                    Some("mediaError") => Action::Fail(player.media.clone(), details),
                    _ => Action::Fail(player.media.clone(), details),
                }
            }
            _ => Action::None,
        }
    });

    match action {
        Action::None => {}
        Action::Start(hls, position) => {
            let _ = hls.start_load_at(position);
        }
        Action::Play(media, position) => {
            begin_playback(media, position);
        }
        Action::Resume(media) => resume_playback(media),
        Action::Retarget(hls, media, position) => {
            spawn_local(async move {
                let _ = hls.stop_load();
                let target = position + BUFFER_EPSILON_SECONDS;
                media.set_current_time(target);
                let _ = hls.start_load_at(target);
            });
        }
        Action::RecoverNetwork(hls, position) => {
            let _ = hls.start_load_at(position);
        }
        Action::RecoverMedia(hls) => {
            let _ = hls.recover_media_error();
        }
        Action::Fail(media, message) => set_state(&media, "error", &message),
    }
}

fn start_beginning_history_when_safe(
    media: &HtmlMediaElement,
    plan: &HlsStartupPlan,
    live: bool,
) -> bool {
    let ready = !live && buffered_covers(media, plan.play_position, plan.runway_end);
    if ready {
        super::runtime::start_beginning_history();
    }
    ready
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

fn settle_seek(media: &HtmlMediaElement, runway: f64, seek: &mut Option<SeekGate>) -> Option<bool> {
    let pending = *seek.as_ref()?;
    let aligned_target = pending.target + SEEK_ALIGNMENT_SECONDS;
    let mut end = aligned_target + runway;
    let duration = media.duration();
    if duration.is_finite() {
        end = end.min(duration);
    }
    if !runway.is_finite()
        || runway <= 0.0
        || (!buffered_covers(media, aligned_target, end)
            && end > aligned_target + BUFFER_EPSILON_SECONDS)
    {
        return None;
    }
    *seek = None;
    if !pending.resume {
        set_state(
            media,
            "ready",
            "Seek runway buffered. Press play to continue.",
        );
    }
    Some(pending.resume)
}

fn playback_ready(
    media: &HtmlMediaElement,
    plan: &HlsStartupPlan,
    live: bool,
    allow_infinite_duration: bool,
) -> bool {
    if !buffered_covers(media, plan.play_position, plan.runway_end) {
        return false;
    }
    let duration = media.duration();
    if !live {
        return duration.is_finite() && duration + BUFFER_EPSILON_SECONDS >= plan.duration;
    }
    (allow_infinite_duration && duration.is_infinite())
        || (duration.is_finite() && duration + BUFFER_EPSILON_SECONDS >= plan.runway_end)
}

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
    set(&config, "startFragPrefetch", JsValue::TRUE);
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
            HLS_LIVE_SYNC_SEGMENTS as f64,
        );
        set_number(&config, "liveSyncOnStallIncrease", 0.0);
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

fn js_property(value: &JsValue, name: &str) -> Option<JsValue> {
    Reflect::get(value, &JsValue::from_str(name)).ok()
}

fn fragment_reference(value: &JsValue) -> Option<String> {
    let fragment = js_property(value, "frag")?;
    let url = js_string(&fragment, "url")?;
    let path = url.split_once('?').map_or(url.as_str(), |(path, _)| path);
    let reference = path.rsplit('/').next()?;
    (matches!(reference.len(), 64 | 128) && reference.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| reference.to_ascii_lowercase())
}

fn has_video_track(value: &JsValue) -> bool {
    let Some(tracks) = js_property(value, "tracks") else {
        return false;
    };
    ["video", "audiovideo"].into_iter().any(|kind| {
        js_property(&tracks, kind).is_some_and(|track| !track.is_null() && !track.is_undefined())
    })
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
