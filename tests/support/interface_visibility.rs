use super::*;
use futures::FutureExt;
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
fn hidden_interface_wait_wakes_on_visibility_and_remount_and_cleans_up() {
    let document = web_sys::window().unwrap().document().unwrap();
    let visibility = DomListener::new(&document, "visibilitychange", |_| {
        INTERFACE_CHANGED.with(|changed| changed.notify(usize::MAX));
    }).unwrap();
    let change_visibility = || {
        document.dispatch_event(&Event::new("visibilitychange").unwrap()).unwrap();
    };

    let changed = INTERFACE_CHANGED.with(event_listener::Event::listen);
    change_visibility();
    assert!(changed.now_or_never().is_some());

    let previous = begin_interface_mount();
    let changed = INTERFACE_CHANGED.with(event_listener::Event::listen);
    let current = begin_interface_mount();
    assert!(!interface_mount_is_current(previous));
    assert!(interface_mount_is_current(current));
    assert!(changed.now_or_never().is_some());

    drop(visibility);
    let changed = INTERFACE_CHANGED.with(event_listener::Event::listen);
    change_visibility();
    assert!(changed.now_or_never().is_none());
}

#[wasm_bindgen_test]
fn dom_listeners_keep_targets_and_events_independent() {
    let first = web_sys::EventTarget::new().unwrap();
    let second = web_sys::EventTarget::new().unwrap();
    let received = Rc::new(Cell::new(0));
    let listener = DomListener::new(&first, "controllerchange", {
        let received = received.clone();
        move |_| received.set(received.get() + 1)
    }).unwrap();
    second.dispatch_event(&Event::new("controllerchange").unwrap()).unwrap();
    first.dispatch_event(&Event::new("visibilitychange").unwrap()).unwrap();
    assert_eq!(received.get(), 0);
    first.dispatch_event(&Event::new("controllerchange").unwrap()).unwrap();
    assert_eq!(received.get(), 1);
    drop(listener);
    first.dispatch_event(&Event::new("controllerchange").unwrap()).unwrap();
    assert_eq!(received.get(), 1);
}
