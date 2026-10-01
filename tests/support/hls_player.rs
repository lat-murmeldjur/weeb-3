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
    for initial_live in [false, true] {
        let fixture = HandoffFixture::new();
        fixture.select.set_selected_index(2);
        with_player(1, |player| player.initial_live = initial_live);
        let error = js_sys::JSON::parse(r#"{"errorAction":{"nextAutoLevel":0}}"#).unwrap();
        set_number(fixture.hls.unchecked_ref(), "manualLevel", -1.0);
        set(fixture.hls.unchecked_ref(), "autoLevelEnabled", JsValue::TRUE);
        release_auto_quality(1, None);
        release_auto_quality(1, Some(&Object::new()));
        assert_eq!(fixture.select.selected_index(), 2);
        set_number(fixture.hls.unchecked_ref(), "manualLevel", 1.0);
        set(fixture.hls.unchecked_ref(), "autoLevelEnabled", JsValue::FALSE);
        release_auto_quality(1, Some(&error));
        assert_eq!(fixture.select.selected_index(), 2);
        assert_eq!(with_player(1, |player| player.quality.as_ref().unwrap().revision), 0);
        set_number(fixture.hls.unchecked_ref(), "manualLevel", -1.0);
        set(fixture.hls.unchecked_ref(), "autoLevelEnabled", JsValue::TRUE);
        release_auto_quality(1, Some(&error));
        assert_eq!(fixture.select.selected_index(), 0);
        assert_eq!(fixture.level(), Some(-1.0));
        assert_eq!(with_player(1, |player| player.quality.as_ref().unwrap().revision), 1);
        assert!(fixture.playback_calls.borrow().is_empty());
    }
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
    level_calls: Rc<RefCell<Vec<(&'static str, f64)>>>,
    buffering_calls: Rc<RefCell<Vec<&'static str>>>,
    forward_flush: Rc<Cell<Option<f64>>>,
    buffer_end: Rc<Cell<f64>>,
    _playback: [Closure<dyn Fn() -> Promise>; 2],
    _buffering: [Closure<dyn Fn()>; 2],
    _level_get: Closure<dyn Fn() -> f64>,
    _level_set: [Closure<dyn Fn(f64)>; 2],
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
        let buffer_end = Rc::new(Cell::new(100.0));
        let range_end = Closure::<dyn Fn(u32) -> f64>::new({
            let buffer_end = buffer_end.clone();
            move |_| buffer_end.get()
        });
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
                "minAutoLevel":0,"maxAutoLevel":1,"bufferingEnabled":true,
                "inFlightFragments":{"main":{"state":"FRAG_LOADING"}},
                "latestLevelDetails":{"fragments":[{"level":1}]},
                "levels":[{"height":360,"uri":"data:text/plain,%23EXTM3U"},{"height":720}]}"#,
        )
        .unwrap()
        .unchecked_into();
        let buffering_calls = Rc::new(RefCell::new(Vec::new()));
        let buffering = [("pauseBuffering", false), ("resumeBuffering", true)].map(|(name, enabled)| {
            let target = hls.clone();
            let calls = buffering_calls.clone();
            let callback = Closure::<dyn Fn()>::new(move || {
                with_player(1, |_| ()); // Public Hls calls must occur outside the ACTIVE borrow.
                calls.borrow_mut().push(name);
                set(target.unchecked_ref(), "bufferingEnabled", enabled.into());
            });
            set(hls.unchecked_ref(), name, callback.as_ref().clone());
            callback
        });
        let requested_level = Rc::new(Cell::new(1.0));
        let level_get = Closure::<dyn Fn() -> f64>::new({
            let requested_level = requested_level.clone();
            move || requested_level.get()
        });
        let level_calls = Rc::new(RefCell::new(Vec::new()));
        let forward_flush = Rc::new(Cell::new(None));
        let level_set = ["loadLevel", "nextLevel"].map(|name| {
            let target = hls.clone();
            let requested_level = requested_level.clone();
            let calls = level_calls.clone();
            let forward_flush = forward_flush.clone();
            let setter = Closure::<dyn Fn(f64)>::new(move |level| {
                if level >= 0.0 {
                    requested_level.set(level);
                }
                calls.borrow_mut().push((name, level));
                set_number(target.unchecked_ref(), "manualLevel", level);
                set(target.unchecked_ref(), "autoLevelEnabled", (level == -1.0).into());
                if name == "nextLevel" && let Some(start) = forward_flush.get() {
                    let data = Object::new();
                    set_number(&data, "startOffset", 0.0);
                    set_number(&data, "endOffset", 1.0);
                    handle_event(1, "hlsBufferFlushing", &data);
                    handle_event(1, "hlsBufferFlushed", &Object::new());
                    set_number(&data, "startOffset", start);
                    set_number(&data, "endOffset", f64::INFINITY);
                    handle_event(1, "hlsBufferFlushing", &data);
                }
            });
            let descriptor = Object::new();
            set(&descriptor, "get", level_get.as_ref().clone());
            set(&descriptor, "set", setter.as_ref().clone());
            Object::define_property(hls.unchecked_ref(), &name.into(), &descriptor);
            setter
        });
        let mut player = player_fixture(media.clone(), hls.clone());
        player.quality = quality_control(1, &media, &hls).unwrap();
        let select = player.quality.as_ref().unwrap().select.clone();
        assert_eq!(select.selected_index(), 2);
        select.set_selected_index(1);
        ACTIVE.with_borrow_mut(|active| *active = Some(player));
        Self {
            media,
            hls,
            select,
            playback_calls,
            level_calls,
            buffering_calls,
            forward_flush,
            buffer_end,
            _playback: playback,
            _buffering: buffering,
            _level_get: level_get,
            _level_set: level_set,
            _range_start: range_start,
            _range_end: range_end,
        }
    }

    async fn prepare(&self, automatic: bool) {
        if automatic {
            self.select.set_selected_index(0);
            set_number(self.hls.unchecked_ref(), "loadLevel", 0.0);
            set(self.hls.unchecked_ref(), "autoLevelEnabled", JsValue::TRUE);
            set_number(self.hls.unchecked_ref(), "manualLevel", -1.0);
        }
        select_quality(1, automatic.then(|| (self.hls.clone(), 0, 0)));
        wait_for_handoff(|| self.waiting()).await;
    }

    fn level(&self) -> Option<f64> {
        // Auto leaves the last loading level intact; assertions track requested mode.
        number_property(self.hls.as_ref(), "manualLevel")
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

#[wasm_bindgen_test(async)]
async fn quality_flush_cancellation_preserves_buffering_ownership() {
    for action in ["forward", "alternate", "no-forward", "already-disabled", "error", "selection", "seek", "pause"] {
        let fixture = HandoffFixture::new();
        if action != "no-forward" { fixture.forward_flush.set(Some(10.0)); }
        if action == "already-disabled" { set(fixture.hls.unchecked_ref(), "bufferingEnabled", JsValue::FALSE); }
        fixture.prepare(false).await;
        handle_event(1, "hlsFragBuffered", &js_sys::JSON::parse(r#"{"frag":{"type":"main","sn":1}}"#).unwrap());
        wait_for_handoff(|| fixture.level() == Some(0.0)).await;
        if !matches!(action, "no-forward" | "already-disabled") {
            assert_eq!(fixture.buffering_calls.borrow().as_slice(), &["pauseBuffering"]);
            assert_eq!(with_player(1, |player| player.quality.as_ref().unwrap().flush_from), Some(10.0));
        }
        let start = Closure::<dyn Fn(f64, bool)>::new(|_, _| {});
        set(fixture.hls.unchecked_ref(), "startLoad", start.as_ref().clone());
        match action {
            "forward" | "alternate" => {
                for (end, enabled) in [(100.0, false), (10.06, false), (10.04, true)] {
                    fixture.buffer_end.set(end);
                    if action == "alternate" {
                        let main = Object::new();
                        set_number(&main, "end", 100.0);
                        set(fixture.hls.unchecked_ref(), "mainForwardBufferInfo", main.clone().into());
                        handle_event(1, "hlsBufferFlushed", &Object::new());
                        assert_eq!(js_bool(fixture.hls.as_ref(), "bufferingEnabled"), Some(false));
                        set_number(&main, "end", end);
                    }
                    handle_event(1, "hlsBufferFlushed", &Object::new());
                    assert_eq!(js_bool(fixture.hls.as_ref(), "bufferingEnabled"), Some(enabled));
                }
                handle_event(1, "hlsBufferFlushed", &Object::new());
            }
            "error" => handle_event(1, "hlsError", &Object::new()),
            "selection" => {
                fixture.select.set_selected_index(0);
                select_quality(1, None);
                wait_for_handoff(|| fixture.level() == Some(-1.0)).await;
            }
            "seek" => seek_hls(1, fixture.hls.clone(), 0.0),
            "pause" => {
                fixture_property(fixture.media.as_ref(), "paused", JsValue::TRUE);
                handle_media_event(1, &fixture.media, "pause", false);
                assert!(with_player(1, |player| player.intent == PlaybackIntent::Paused));
            }
            _ => {}
        }
        assert_eq!(with_player(1, |player| player.quality.as_ref().unwrap().flush_from), None, "{action}");
        let expected: &[&str] = if action == "already-disabled" { &[] } else { &["pauseBuffering", "resumeBuffering"] };
        assert_eq!(fixture.buffering_calls.borrow().as_slice(), expected, "{action}");
        assert_eq!(js_bool(fixture.hls.as_ref(), "bufferingEnabled"), Some(action != "already-disabled"));
        assert!(fixture.playback_calls.borrow().is_empty(), "{action}");
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
        fixture.level_calls.borrow_mut().clear();
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
        let expected: &[(&str, f64)] = if automatic {
            &[("loadLevel", 0.0), ("loadLevel", -1.0)]
        } else {
            &[("nextLevel", 0.0)]
        };
        assert_eq!(fixture.level_calls.borrow().as_slice(), expected);
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
    assert_eq!(number_property(fixture.hls.as_ref(), "manualLevel"), Some(1.0));
    assert_eq!(js_bool(fixture.hls.as_ref(), "autoLevelEnabled"), Some(false));
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

#[wasm_bindgen_test(async)]
async fn quality_api_copies_metadata_and_validates_without_cancelling_handoff() {
    let fixture = HandoffFixture::new();
    fixture.select.set_selected_index(2);
    set_stream_quality(&fixture.media, 0.0).unwrap();
    wait_for_handoff(|| fixture.waiting()).await;
    set_number(fixture.hls.unchecked_ref(), "currentLevel", 1.0);
    let level = array_property(fixture.hls.as_ref(), "levels").unwrap().get(0);
    set_number(level.unchecked_ref(), "width", 640.0);
    set_number(level.unchecked_ref(), "bitrate", 500_000.0);
    let state = stream_quality(&fixture.media).unwrap();
    assert_eq!(number_property(&state, "selectedLevel"), Some(0.0));
    assert_eq!(number_property(&state, "currentLevel"), Some(1.0));
    let copied = array_property(&state, "levels").unwrap().get(0);
    for (name, expected) in [("level", 0.0), ("width", 640.0), ("height", 360.0), ("bitrate", 500_000.0)] {
        assert_eq!(number_property(&copied, name), Some(expected));
    }
    set_number(copied.unchecked_ref(), "height", 999.0);
    assert_eq!(number_property(&level, "height"), Some(360.0));
    let other: HtmlMediaElement = fixture.media.owner_document().unwrap()
        .create_element("video").unwrap().unchecked_into();
    assert!(stream_quality(&other).is_err());
    assert!(set_stream_quality(&other, -1.0).is_err());
    for level in [-2.0, 0.5, 2.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(set_stream_quality(&fixture.media, level).is_err());
    }
    fixture.select.set_disabled(true);
    assert!(set_stream_quality(&fixture.media, -1.0).is_err());
    fixture.select.set_disabled(false);
    set_stream_quality(&fixture.media, 0.0).unwrap();
    async_std::task::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(fixture.select.selected_index(), 1);
    assert_eq!(with_player(1, |player| player.quality.as_ref().unwrap().revision), 1);
    assert!(fixture.waiting());
    set_stream_quality(&fixture.media, -1.0).unwrap();
    wait_for_handoff(|| fixture.level() == Some(-1.0)).await;
    assert!(!fixture.waiting());
    let state = stream_quality(&fixture.media).unwrap();
    assert_eq!(number_property(&state, "selectedLevel"), Some(-1.0));
    assert_eq!(number_property(&state, "currentLevel"), Some(1.0));
    set(fixture.hls.unchecked_ref(), "levels", Array::new().into());
    assert!(set_stream_quality(&fixture.media, -1.0).is_err());
    set(fixture.hls.unchecked_ref(), "levels", Array::of1(&level).into());
    set_number(fixture.hls.unchecked_ref(), "manualLevel", 0.0);
    set_number(fixture.hls.unchecked_ref(), "loadLevel", 0.0);
    with_player(1, |player| player.quality = None);
    let state = stream_quality(&fixture.media).unwrap();
    assert_eq!(number_property(&state, "selectedLevel"), Some(0.0));
    set_stream_quality(&fixture.media, 0.0).unwrap();
    assert_eq!(fixture.level(), Some(0.0));
    set_stream_quality(&fixture.media, -1.0).unwrap();
    assert_eq!(fixture.level(), Some(-1.0));
    with_player(1, |player| player.media = other);
    assert!(stream_quality(&fixture.media).is_err());
    assert!(set_stream_quality(&fixture.media, 0.0).is_err());
    assert!(fixture.playback_calls.borrow().is_empty());
}

#[wasm_bindgen_test(async)]
async fn early_quality_selection_uses_plan_anchor() {
    for (bootstrap, current, expected) in [(false, 0.0, 32.0), (false, 1050.0, 50.0), (true, 1050.0, 4.0)] {
        let fixture = HandoffFixture::new();
        let config = hls_config(HlsStart::Live);
        set_number(&config, "timelineOffset", 1000.0);
        set(fixture.hls.unchecked_ref(), "config", config.clone().into());
        fixture_property(fixture.media.as_ref(), "currentTime", current.into());
        with_player(1, |player| {
            player.ready = false;
            player.initial_live = true;
            player.codec_bootstrap_pending = bootstrap;
            player.plan.play_position = 1032.0;
            player.plan.bootstrap_position = 1004.0;
        });
        let starts = Rc::new(RefCell::new(Vec::new()));
        let start_load = Closure::<dyn Fn(f64, bool)>::new({
            let starts = starts.clone();
            move |position, skip_seek| starts.borrow_mut().push((position, skip_seek))
        });
        set(fixture.hls.unchecked_ref(), "startLoad", start_load.as_ref().clone());
        set_stream_quality(&fixture.media, 1.0).unwrap();
        wait_for_handoff(|| !starts.borrow().is_empty()).await;
        assert_eq!(starts.borrow().as_slice(), &[(expected, true)]);
        assert_eq!(fixture.level(), Some(1.0));
        assert_eq!(fixture.level_calls.borrow().as_slice(), &[("loadLevel", 1.0)]);
        assert!(fixture.playback_calls.borrow().is_empty());
    }
}

#[wasm_bindgen_test(async)]
async fn seeking_outside_selected_rendition_window_prepares_history() {
    let _fetch = FetchOverride::new(Closure::new(|_, _| Promise::resolve(&JsValue::UNDEFINED)));
    // Manual selection and Auto's loading level may have different dated windows.
    for (automatic, first, loaded_first, offset, position, pending, dated, history) in [
        (false, Some(48.0), 0.0, 0.0, 15.0, false, false, true),
        (true, Some(0.0), 48.0, 0.0, 15.0, false, false, true),
        (true, Some(48.0), 0.0, 0.0, 15.0, false, false, false),
        (false, None, 48.0, 0.0, 15.0, false, false, false),
        (false, None, 0.0, 100.0, 15.0, false, false, true),
        (false, Some(48.0), 0.0, 100.0, 60.0, false, false, true),
        (false, Some(48.0), 0.0, 0.0, 48.0, false, false, false),
        (false, Some(48.0), 0.0, 0.0, 90.0, true, false, true),
        (false, Some(48.0), 0.0, 0.0, 500.0, false, true, true),
        (false, Some(48.0), 0.0, 0.0, 318.0, false, true, true),
    ] {
        let fixture = HandoffFixture::new();
        // The advertised duration may outlast the selected finalized rendition.
        fixture_property(fixture.media.as_ref(), "duration", 600.0.into());
        let levels = Array::new();
        for start in [first, Some(loaded_first)] {
            let level = Object::new();
            set_string(&level, "uri", "data:text/plain,%23EXTM3U");
            if let Some(start) = start {
                set(&level, "details", js_sys::JSON::parse(&format!(
                    r#"{{"hasProgramDateTime":{dated},"live":false,"fragments":[{{"start":{start},"end":318}}]}}"#)).unwrap());
            }
            levels.push(&level);
        }
        set(fixture.hls.unchecked_ref(), "loadLevelObj", levels.get(1));
        set(fixture.hls.unchecked_ref(), "levels", levels.into());
        fixture.select.set_selected_index(if automatic { 0 } else { 1 });
        with_player(1, |player| {
            player.plan.timeline_offset = offset;
            player.reload_position = pending.then_some(80.0);
        });
        let stops = Rc::new(Cell::new(0));
        let stop = Closure::<dyn Fn()>::new({ let stops = stops.clone(); move || stops.set(stops.get() + 1) });
        let starts = Rc::new(RefCell::new(Vec::new()));
        let start = Closure::<dyn Fn(f64, bool)>::new({
            let starts = starts.clone(); move |position, _| starts.borrow_mut().push(position)
        });
        set(fixture.hls.unchecked_ref(), "stopLoad", stop.as_ref().clone());
        set(fixture.hls.unchecked_ref(), "startLoad", start.as_ref().clone());
        seek_hls(1, fixture.hls.clone(), position);
        assert_eq!(stops.get(), u32::from(history));
        assert_eq!(*starts.borrow(), if history { vec![] } else { vec![position] });
        assert_eq!(with_player(1, |player| player.ready), !history);
        assert_eq!(fixture.select.disabled(), history);
        assert_eq!(with_player(1, |player| player.history_request), u64::from(history));
        if history { assert_eq!(with_player(1, |player| player.reload_position), Some(position)); }
        assert!(fixture.playback_calls.borrow().is_empty());
        // Retire the fixture before its queued history task can touch the worker.
        drop(fixture);
        async_std::task::sleep(std::time::Duration::from_millis(10)).await;
    }
}

struct FetchOverride {
    original: JsValue,
    _callback: Closure<dyn Fn(JsValue, JsValue) -> Promise>,
}

impl FetchOverride {
    fn new(callback: Closure<dyn Fn(JsValue, JsValue) -> Promise>) -> Self {
        let global = js_sys::global();
        let original = js_property(&global, "fetch").unwrap();
        set(global.unchecked_ref(), "fetch", callback.as_ref().clone());
        Self { original, _callback: callback }
    }
}

impl Drop for FetchOverride {
    fn drop(&mut self) {
        set(js_sys::global().unchecked_ref(), "fetch", self.original.clone());
    }
}
