use super::*;
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
fn abr_samples_include_retrieval_after_hls_subtracts_ttfb() {
    // Foreground full-body, cached body, and a slow local transport.
    for (first, end, elapsed) in [
        (3000.0, 3001.0, 2000.0),
        (1001.0, 1002.0, 2000.0),
        (2000.0, 4000.0, 500.0),
    ] {
        let data = js_sys::JSON::parse(&format!(
            r#"{{"frag":{{"type":"main","sn":1,"stats":{{"loading":{{"start":1000,"first":{first},"end":{end}}}}}}},"networkDetails":{{}}}}"#
        )).unwrap();
        let response = js_property(&data, "networkDetails").unwrap();
        let get = Closure::<dyn Fn() -> JsValue>::new(move || elapsed.to_string().into());
        set(
            response.unchecked_ref(),
            "getResponseHeader",
            get.as_ref().clone(),
        );
        assert_eq!(account_retrieval_time(&data), Some(()));
        let stats = js_property(&js_property(&data, "frag").unwrap(), "stats").unwrap();
        let loading = js_property(&stats, "loading").unwrap();
        let start = number_property(&loading, "start").unwrap();
        let adjusted_first = number_property(&loading, "first").unwrap();
        let ttfb = first - 1000.0;
        assert_eq!(adjusted_first - start, ttfb);
        assert!(start <= adjusted_first && adjusted_first <= end);
        for estimated_ttfb in [0.0_f64, 1000.0, ttfb] {
            let processing = end + 10.0 - start - ttfb.min(estimated_ttfb);
            assert!(processing >= elapsed && processing >= end - 1000.0);
        }
    }
}

#[wasm_bindgen_test]
fn failed_manual_quality_shows_auto_without_cancelling_pending_selection() {
    let document = web_sys::window().unwrap().document().unwrap();
    let media: HtmlMediaElement = document.create_element("video").unwrap().unchecked_into();
    let hls: Hls = js_sys::JSON::parse(
        r#"{"autoLevelEnabled":true,"startLevel":1,"levels":[{"height":360},{"height":720}]}"#,
    )
    .unwrap()
    .unchecked_into();
    let quality = quality_control(1, &media, &hls).unwrap();
    let select = quality.as_ref().unwrap().select.clone();
    let mut player = player_fixture(media, hls.clone());
    player.quality = quality;
    ACTIVE.with_borrow_mut(|active| *active = Some(player));
    let error = js_sys::JSON::parse(r#"{"errorAction":{"nextAutoLevel":0}}"#).unwrap();
    release_auto_quality(1, None);
    release_auto_quality(1, Some(&Object::new()));
    assert_eq!(select.selected_index(), 2);
    assert_eq!(
        with_player(1, |player| player.quality.as_ref().unwrap().revision),
        0
    );
    set(hls.unchecked_ref(), "autoLevelEnabled", JsValue::FALSE);
    release_auto_quality(1, Some(&error));
    assert_eq!(select.selected_index(), 2);
    set(hls.unchecked_ref(), "autoLevelEnabled", JsValue::TRUE);
    release_auto_quality(1, Some(&error));
    assert_eq!(select.selected_index(), 0);
    assert_eq!(
        with_player(1, |player| player.quality.as_ref().unwrap().revision),
        1
    );
    ACTIVE.with_borrow_mut(|active| *active = None);
}

fn player_fixture(media: HtmlMediaElement, hls: Hls) -> Player {
    Player {
        id: 1,
        hls,
        hls_class: JsValue::NULL,
        source: String::new(),
        initial_source: None,
        quality: None,
        media,
        callback: Closure::new(|_, _| {}),
        xhr_setup: Closure::new(|_, _| Ok(())),
        lifecycle: Closure::new(|_| {}),
        plan: HlsStartupPlan {
            timeline_offset: 0.0,
            bootstrap_position: 0.0,
            codec_bootstrap: false,
            play_position: 0.0,
            runway_end: 2.0,
            duration: 100.0,
        },
        restore_autoplay: false,
        intent: PlaybackIntent::Play,
        live: false,
        codec_bootstrap_pending: false,
        codec_recovery: None,
        initial_live: false,
        ready: true,
        consecutive_media_recoveries: 0,
        hard_restarts: 0,
        decoder_wait: None,
        reload_position: None,
        history_request: 0,
        clock_position: None,
    }
}

fn fixture_property(target: &JsValue, name: &str, value: JsValue) {
    let descriptor = Object::new();
    set(&descriptor, "value", value);
    set(&descriptor, "writable", JsValue::TRUE);
    set(&descriptor, "configurable", JsValue::TRUE);
    Object::define_property(target.unchecked_ref(), &name.into(), &descriptor);
}

#[wasm_bindgen_test(async)]
async fn hidden_decoder_stall_recovers_without_resuming_paused_playback() {
    let document = web_sys::window().unwrap().document().unwrap();
    let previous_hidden =
        Object::get_own_property_descriptor(document.unchecked_ref(), &"hidden".into());
    let media: HtmlMediaElement = document.create_element("video").unwrap().unchecked_into();
    for (name, value) in [
        ("paused", JsValue::FALSE),
        ("seeking", JsValue::FALSE),
        ("ended", JsValue::FALSE),
        ("readyState", 2.into()),
    ] {
        fixture_property(media.as_ref(), name, value);
    }

    let seeks = Rc::new(Cell::new(0));
    let last_seek = Rc::new(Cell::new(0.0));
    let position = Closure::<dyn Fn() -> f64>::new(|| 12.0);
    let seek = Closure::<dyn Fn(f64)>::new({
        let seeks = seeks.clone();
        let last_seek = last_seek.clone();
        move |value| {
            seeks.set(seeks.get() + 1);
            last_seek.set(value);
        }
    });
    let descriptor = Object::new();
    set(&descriptor, "get", position.as_ref().clone());
    set(&descriptor, "set", seek.as_ref().clone());
    Object::define_property(media.unchecked_ref(), &"currentTime".into(), &descriptor);

    let buffer_end = Rc::new(Cell::new(13.0));
    let start = Closure::<dyn Fn(u32) -> f64>::new(|_| 0.0);
    let end = Closure::<dyn Fn(u32) -> f64>::new({
        let buffer_end = buffer_end.clone();
        move |_| buffer_end.get()
    });
    let ranges = Object::new();
    set(&ranges, "length", 1.into());
    set(&ranges, "start", start.as_ref().clone());
    set(&ranges, "end", end.as_ref().clone());
    fixture_property(media.as_ref(), "buffered", ranges.into());
    let playback_calls = Rc::new(Cell::new(0));
    let playback = Closure::<dyn Fn() -> Promise>::new({
        let playback_calls = playback_calls.clone();
        move || {
            playback_calls.set(playback_calls.get() + 1);
            Promise::resolve(&JsValue::UNDEFINED)
        }
    });
    for name in ["play", "pause"] {
        fixture_property(media.as_ref(), name, playback.as_ref().clone());
    }
    fixture_property(document.as_ref(), "hidden", JsValue::TRUE);
    ACTIVE.with_borrow_mut(|active| {
        *active = Some(player_fixture(
            media.clone(),
            Object::new().unchecked_into(),
        ));
    });

    handle_media_event(1, &media, "waiting", false);
    let pending_without_buffer = with_player(1, |player| {
        player.decoder_wait.as_ref().unwrap().1.total_listeners()
    });
    buffer_end.set(15.0);
    handle_event(1, "hlsBufferAppended", &Object::new());
    let pending_with_buffer = with_player(1, |player| {
        player.decoder_wait.as_ref().unwrap().1.total_listeners()
    });
    // Hiding or showing during confirmation replaces, rather than duplicates, the wait.
    handle_media_event(1, &media, "visibilitychange", false);
    async_std::task::sleep(std::time::Duration::from_millis(1100)).await;
    let recovered = seeks.get();

    handle_media_event(1, &media, "waiting", false);
    fixture_property(media.as_ref(), "paused", JsValue::TRUE);
    handle_media_event(1, &media, "pause", false);
    let pause_cancelled = with_player(1, |player| {
        player.intent == PlaybackIntent::Paused && player.decoder_wait.is_none()
    });
    // A native/system pause is safe even before its queued pause event is handled.
    fixture_property(media.as_ref(), "paused", JsValue::FALSE);
    with_player(1, |player| player.intent = PlaybackIntent::Play);
    handle_media_event(1, &media, "waiting", false);
    fixture_property(media.as_ref(), "paused", JsValue::TRUE);
    async_std::task::sleep(std::time::Duration::from_millis(1100)).await;

    ACTIVE.with_borrow_mut(|active| {
        if let Some(mut player) = active.take() {
            cancel_decoder_recovery(&mut player);
        }
    });
    if previous_hidden.is_undefined() {
        Reflect::delete_property(document.unchecked_ref(), &"hidden".into()).unwrap();
    } else {
        Object::define_property(
            document.unchecked_ref(),
            &"hidden".into(),
            previous_hidden.unchecked_ref(),
        );
    }
    assert_eq!(pending_without_buffer, 0);
    assert_eq!(pending_with_buffer, 1);
    assert_eq!(recovered, 1);
    assert_eq!(last_seek.get(), 12.0);
    assert!(pause_cancelled);
    assert_eq!(seeks.get(), 1);
    assert_eq!(playback_calls.get(), 0);
}

#[wasm_bindgen_test]
fn rendition_coverage_uses_current_time_and_preserves_undated_playlists() {
    let hls: Hls = Object::new().unchecked_into();
    let response = Object::new();
    set(&response, "ok", JsValue::TRUE);
    let headers = Object::new();
    set(&response, "headers", headers.clone().into());
    for (start, end, position, expected) in [
        (None, None, 200_000.0, true),
        (Some("100000"), Some("130000"), 100_000.0, true),
        (Some("100000"), Some("130000"), 120_000.0, true),
        (Some("100000"), Some("130000"), 130_000.0, false),
        (Some("100000"), Some("130000"), 200_000.0, false),
        (Some("100000"), Some("300000"), 200_000.0, true),
        (Some("100000"), None, 200_000.0, false),
        (Some("100000"), Some("invalid"), 200_000.0, false),
    ] {
        let get = Closure::<dyn Fn(String) -> JsValue>::new(move |name: String| {
            (if name.ends_with("Start") { start } else { end })
                .map_or(JsValue::NULL, JsValue::from_str)
        });
        set(&headers, "get", get.as_ref().clone());
        set(
            hls.unchecked_ref(),
            "playingDate",
            js_sys::Date::new(&position.into()).into(),
        );
        assert_eq!(quality_response_covers(&hls, &response), expected);
    }
    set(&response, "ok", JsValue::FALSE);
    set(hls.unchecked_ref(), "playingDate", JsValue::NULL);
    assert!(!quality_response_covers(&hls, &response));
}

struct HandoffFixture {
    media: HtmlMediaElement,
    hls: Hls,
    select: HtmlSelectElement,
    playback_calls: Rc<RefCell<Vec<&'static str>>>,
    _playback: [Closure<dyn Fn() -> Promise>; 2],
    _range_start: Closure<dyn Fn(u32) -> f64>,
    _range_end: Closure<dyn Fn(u32) -> f64>,
}

impl HandoffFixture {
    fn new() -> Self {
        let document = web_sys::window().unwrap().document().unwrap();
        let media: HtmlMediaElement = document.create_element("video").unwrap().unchecked_into();
        for (name, value) in [
            ("paused", JsValue::FALSE),
            ("seeking", JsValue::FALSE),
            ("ended", JsValue::FALSE),
            ("readyState", 4.into()),
            ("duration", 100.into()),
        ] {
            fixture_property(media.as_ref(), name, value);
        }
        let range_start = Closure::<dyn Fn(u32) -> f64>::new(|_| 0.0);
        let range_end = Closure::<dyn Fn(u32) -> f64>::new(|_| 100.0);
        let ranges = Object::new();
        set(&ranges, "length", 1.into());
        set(&ranges, "start", range_start.as_ref().clone());
        set(&ranges, "end", range_end.as_ref().clone());
        fixture_property(media.as_ref(), "buffered", ranges.into());
        let playback_calls = Rc::new(RefCell::new(Vec::new()));
        let playback = ["play", "pause"].map(|name| {
            let calls = playback_calls.clone();
            let callback = Closure::<dyn Fn() -> Promise>::new(move || {
                calls.borrow_mut().push(name);
                Promise::resolve(&JsValue::UNDEFINED)
            });
            fixture_property(media.as_ref(), name, callback.as_ref().clone());
            callback
        });
        let hls: Hls = js_sys::JSON::parse(
            r#"{"startLevel":1,"loadLevel":1,"manualLevel":1,"autoLevelEnabled":false,
                "minAutoLevel":0,"maxAutoLevel":1,
                "inFlightFragments":{"main":{"state":"FRAG_LOADING"}},
                "latestLevelDetails":{"fragments":[{"level":1}]},
                "levels":[{"height":360,"uri":"data:text/plain,%23EXTM3U"},{"height":720}]}"#,
        )
        .unwrap()
        .unchecked_into();
        let mut player = player_fixture(media.clone(), hls.clone());
        player.quality = quality_control(1, &media, &hls).unwrap();
        let select = player.quality.as_ref().unwrap().select.clone();
        select.set_selected_index(1);
        ACTIVE.with_borrow_mut(|active| *active = Some(player));
        Self {
            media,
            hls,
            select,
            playback_calls,
            _playback: playback,
            _range_start: range_start,
            _range_end: range_end,
        }
    }

    async fn prepare(&self, automatic: bool) {
        if automatic {
            self.select.set_selected_index(0);
            set(self.hls.unchecked_ref(), "autoLevelEnabled", JsValue::TRUE);
            set_number(self.hls.unchecked_ref(), "manualLevel", -1.0);
            set_number(self.hls.unchecked_ref(), "loadLevel", 0.0);
        }
        select_quality(1, automatic.then(|| (self.hls.clone(), 0, 0)));
        wait_for_handoff(|| self.waiting()).await;
    }

    fn level(&self) -> Option<f64> {
        number_property(self.hls.as_ref(), "loadLevel")
    }

    fn waiting(&self) -> bool {
        with_player(1, |player| {
            player.quality.as_ref().unwrap().waiting.is_some()
        })
    }
}

impl Drop for HandoffFixture {
    fn drop(&mut self) {
        ACTIVE.with_borrow_mut(|active| {
            if let Some(mut player) = active.take() {
                cancel_decoder_recovery(&mut player);
            }
        });
    }
}

async fn wait_for_handoff(condition: impl Fn() -> bool) {
    for _ in 0..100 {
        if condition() {
            return;
        }
        async_std::task::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        condition(),
        "quality handoff did not finish within one second"
    );
}

#[wasm_bindgen_test(async)]
async fn prepared_quality_waits_for_one_main_request_and_preserves_auto() {
    for automatic in [false, true] {
        let fixture = HandoffFixture::new();
        fixture.prepare(automatic).await;
        assert_eq!(fixture.level(), Some(1.0));
        let audio = js_sys::JSON::parse(r#"{"frag":{"type":"audio","sn":1}}"#).unwrap();
        handle_event(1, "hlsFragLoaded", &audio);
        handle_event(1, "hlsFragBuffered", &audio);
        assert!(fixture.waiting());
        let main = js_sys::JSON::parse(r#"{"frag":{"type":"main","sn":1}}"#).unwrap();
        handle_event(1, "hlsFragLoaded", &main);
        assert!(fixture.waiting());
        handle_event(1, "hlsFragBuffered", &main);
        // hls.js may start its next request before the waiting Rust future resumes.
        handle_event(1, "hlsFragLoading", &main);
        let target = if automatic { -1.0 } else { 0.0 };
        wait_for_handoff(|| fixture.level() == Some(target)).await;
        assert!(!fixture.waiting());
        if automatic {
            assert_eq!(
                number_property(fixture.hls.as_ref(), "nextAutoLevel"),
                Some(0.0)
            );
        }
        assert!(fixture.playback_calls.borrow().is_empty());
    }
}

#[wasm_bindgen_test(async)]
async fn pending_quality_releases_on_media_interruptions_errors_and_abort() {
    for event in [
        "waiting",
        "pause",
        "ended",
        "seeking",
        "hlsError",
        "hlsFragLoadEmergencyAborted",
        "hlsFragLoading",
    ] {
        let fixture = HandoffFixture::new();
        fixture.prepare(false).await;
        if event.starts_with("hls") {
            let main = js_sys::JSON::parse(r#"{"frag":{"type":"main","sn":2}}"#).unwrap();
            handle_event(1, event, &main);
        } else {
            if event == "pause" {
                fixture_property(fixture.media.as_ref(), "paused", JsValue::TRUE);
            } else if matches!(event, "ended" | "seeking") {
                fixture_property(fixture.media.as_ref(), event, JsValue::TRUE);
            } else if event == "waiting" {
                fixture_property(fixture.media.as_ref(), "readyState", 2.into());
            }
            handle_media_event(1, &fixture.media, event, false);
        }
        wait_for_handoff(|| fixture.level() == Some(0.0)).await;
        assert!(!fixture.waiting(), "{event}");
        assert!(fixture.playback_calls.borrow().is_empty(), "{event}");
        if event == "pause" {
            assert!(with_player(1, |player| player.intent == PlaybackIntent::Paused));
            assert!(fixture.media.paused());
        }
    }
}

#[wasm_bindgen_test(async)]
async fn starving_auto_releases_the_hold_and_cancels_prepared_handoff() {
    let fixture = HandoffFixture::new();
    fixture.prepare(true).await;
    // Represent hls.js's manual state after the production discovery hold.
    set(
        fixture.hls.unchecked_ref(),
        "autoLevelEnabled",
        JsValue::FALSE,
    );
    fixture_property(fixture.media.as_ref(), "readyState", 2.into());
    handle_media_event(1, &fixture.media, "waiting", false);
    assert!(!fixture.waiting());
    async_std::task::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(fixture.level(), Some(-1.0));
    assert_eq!(number_property(fixture.hls.as_ref(), "nextAutoLevel"), None);
    assert!(fixture.playback_calls.borrow().is_empty());
    assert!(with_player(1, |player| player.intent == PlaybackIntent::Play));
}

#[wasm_bindgen_test(async)]
async fn quality_supersession_failed_restart_and_destruction_cancel_old_handoff() {
    for action in ["selection", "restart", "destruction"] {
        let fixture = HandoffFixture::new();
        fixture.prepare(false).await;
        match action {
            "selection" => {
                fixture.select.set_selected_index(0);
                select_quality(1, None);
                wait_for_handoff(|| fixture.level() == Some(-1.0)).await;
            }
            "restart" => {
                with_player(1, |player| player.hard_restarts = MAX_HARD_RESTARTS);
                hard_restart(1, "test exhausted restart budget".into());
            }
            _ => ACTIVE.with_borrow_mut(|active| *active = None),
        }
        async_std::task::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!fixture.waiting(), "{action}");
        let expected = if action == "selection" { -1.0 } else { 1.0 };
        assert_eq!(fixture.level(), Some(expected), "{action}");
        // Exhausting recovery destroys the player and pauses its retired media.
        let calls: &[&str] = if action == "restart" { &["pause"] } else { &[] };
        assert_eq!(fixture.playback_calls.borrow().as_slice(), calls, "{action}");
    }
    let fixture = HandoffFixture::new();
    let (sender, mut receiver) = oneshot::channel();
    with_player(1, |player| {
        player.quality.as_mut().unwrap().waiting = Some(sender)
    });
    assert_eq!(receiver.try_recv().unwrap(), None);
    drop(fixture);
    assert!(
        receiver.try_recv().is_err(),
        "destroying the player must wake the receiver"
    );
}

#[wasm_bindgen_test(async)]
async fn quality_selection_does_not_wait_when_playback_cannot_use_the_current_load() {
    for (property, value) in [
        ("paused", JsValue::TRUE),
        ("seeking", JsValue::TRUE),
        ("ended", JsValue::TRUE),
        ("readyState", 2.into()),
    ] {
        let fixture = HandoffFixture::new();
        fixture_property(fixture.media.as_ref(), property, value);
        select_quality(1, None);
        wait_for_handoff(|| fixture.level() == Some(0.0)).await;
        assert!(!fixture.waiting(), "{property}");
        assert!(fixture.playback_calls.borrow().is_empty(), "{property}");
    }
}
