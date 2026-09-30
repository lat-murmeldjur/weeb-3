use super::*;
use futures::{
    AsyncReadExt, AsyncWriteExt,
    task::{ArcWake, waker},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen(inline_js = r#"
export function fakeSocket() {
    return {readyState: 1, bufferedAmount: 0, sent: [], closes: 0,
        send(bytes) { this.sent.push(bytes.slice()); this.bufferedAmount += bytes.length; },
        close() { this.closes++; this.readyState = 2; }};
}
export function incoming(socket, bytes) { socket.onmessage(new MessageEvent('message', {data: bytes.slice().buffer})); }
export function stateEvent(socket, state, event) { socket.readyState = state; socket['on' + event](new Event(event)); }
export function echoUrl() { return globalThis.WEEB3_TRANSPORT_ECHO_URL || ''; }
"#)]
extern "C" {
    #[wasm_bindgen(js_name = fakeSocket)]
    fn fake_socket() -> WebSocket;
    fn incoming(socket: &WebSocket, bytes: &[u8]);
    #[wasm_bindgen(js_name = stateEvent)]
    fn state_event(socket: &WebSocket, state: u16, event: &str);
    #[wasm_bindgen(js_name = echoUrl)]
    fn echo_url() -> String;
}

#[derive(Default)]
struct Wakes(AtomicUsize);
impl ArcWake for Wakes {
    fn wake_by_ref(this: &Arc<Self>) {
        this.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn amount(socket: &WebSocket, value: usize) {
    js_sys::Reflect::set(socket, &"bufferedAmount".into(), &(value as f64).into()).unwrap();
}

#[wasm_bindgen_test]
fn websocket_urls_preserve_browser_dial_variants() {
    for (address, expected) in [
        ("/ip4/127.0.0.1/tcp/80/ws", Some("ws://127.0.0.1:80/")),
        ("/ip6/::1/tcp/443/tls/ws", Some("wss://[::1]:443/")),
        (
            "/dns4/example.com/tcp/443/wss",
            Some("wss://example.com:443/"),
        ),
        (
            "/dns6/example.com/tcp/443/tls/ws",
            Some("wss://example.com:443/"),
        ),
        ("/dns/example.com/tcp/80/ws", Some("ws://example.com:80/")),
        ("/ip4/127.0.0.1/tcp/80/tls/wss", None),
        ("/ip4/127.0.0.1/tcp/80", None),
        ("/dnsaddr/example.com/tcp/80/ws", None),
    ] {
        assert_eq!(
            websocket_url(&address.parse().unwrap()).as_deref(),
            expected
        );
    }
}

#[wasm_bindgen_test]
fn websocket_reads_partial_messages_and_drains_before_eof() {
    let socket = fake_socket();
    let mut connection = Connection::new(socket.clone());
    let wakes = Arc::new(Wakes::default());
    let task = waker(wakes.clone());
    let mut cx = Context::from_waker(&task);
    let mut output = [0; 2];
    assert!(
        Pin::new(&mut connection)
            .poll_read(&mut cx, &mut output)
            .is_pending()
    );
    incoming(&socket, &[1, 2, 3]);
    incoming(&socket, &[]);
    incoming(&socket, &[4, 5]);
    assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
    state_event(&socket, WebSocket::CLOSED, "close");
    for (count, bytes, buffered) in [(2, [1, 2], 3), (2, [3, 4], 1), (1, [5, 4], 0)] {
        assert!(
            matches!(Pin::new(&mut connection).poll_read(&mut cx, &mut output), Poll::Ready(Ok(n)) if n == count)
        );
        assert_eq!(output, bytes);
        assert_eq!(connection.0.state.borrow().received.len(), buffered);
    }
    assert!(connection.0.state.borrow().received.is_empty());
    assert!(matches!(
        Pin::new(&mut connection).poll_read(&mut cx, &mut output),
        Poll::Ready(Ok(0))
    ));
    assert!(matches!(
        Pin::new(&mut connection).poll_read(&mut cx, &mut []),
        Poll::Ready(Ok(0))
    ));
    drop(connection);
    for event in ["onopen", "onerror", "onclose", "onmessage"] {
        let handler = js_sys::Reflect::get(&socket, &event.into()).unwrap();
        assert!(handler.is_null() || handler.is_undefined());
    }
}

#[wasm_bindgen_test(async)]
async fn websocket_backpressure_has_one_timer_and_keeps_latest_waiter() {
    let socket = fake_socket();
    let mut connection = Connection::new(socket.clone());
    let old = Arc::new(Wakes::default());
    let old_task = waker(old.clone());
    let mut cx = Context::from_waker(&old_task);
    assert!(connection.0.state.borrow().timer.is_none());
    amount(&socket, MAX_BUFFER - 2);
    assert!(matches!(
        Pin::new(&mut connection).poll_write(&mut cx, &[1, 2, 3]),
        Poll::Ready(Ok(2))
    ));
    assert!(
        Pin::new(&mut connection)
            .poll_write(&mut cx, &[3])
            .is_pending()
    );
    let timer = connection.0.state.borrow().timer.unwrap();
    for _ in 0..20 {
        assert!(
            Pin::new(&mut connection)
                .poll_write(&mut cx, &[3])
                .is_pending()
        );
        assert_eq!(connection.0.state.borrow().timer, Some(timer));
    }
    let latest = Arc::new(Wakes::default());
    let task = waker(latest.clone());
    let mut latest_cx = Context::from_waker(&task);
    assert!(
        Pin::new(&mut connection)
            .poll_write(&mut latest_cx, &[3])
            .is_pending()
    );
    amount(&socket, MAX_BUFFER - 1);
    async_std::task::sleep(std::time::Duration::from_millis(130)).await;
    assert_eq!(old.0.load(Ordering::SeqCst), 0);
    assert_eq!(latest.0.load(Ordering::SeqCst), 1);
    assert!(connection.0.state.borrow().timer.is_none());
    assert!(matches!(
        Pin::new(&mut connection).poll_write(&mut cx, &[3]),
        Poll::Ready(Ok(1))
    ));
    assert!(Pin::new(&mut connection).poll_flush(&mut cx).is_pending());
    let timer = connection.0.state.borrow().timer;
    amount(&socket, 1);
    assert!(matches!(
        Pin::new(&mut connection).poll_write(&mut cx, &[4]),
        Poll::Ready(Ok(1))
    ));
    assert_eq!(connection.0.state.borrow().timer, timer);
    amount(&socket, 0);
    assert!(matches!(
        Pin::new(&mut connection).poll_flush(&mut cx),
        Poll::Ready(Ok(()))
    ));
    assert!(connection.0.state.borrow().timer.is_none());
    amount(&socket, 1);
    assert!(Pin::new(&mut connection).poll_flush(&mut cx).is_pending());
    let state = connection.0.state.clone();
    drop(connection);
    assert!(state.borrow().timer.is_none());
    assert_eq!(socket.ready_state(), WebSocket::CLOSING);
}

#[wasm_bindgen_test]
fn websocket_open_error_close_and_overflow_wake_all_waiters() {
    for event in ["open", "error", "close", "overflow"] {
        let socket = fake_socket();
        let mut connection = Connection::new(socket.clone());
        let counters: Vec<_> = (0..3).map(|_| Arc::new(Wakes::default())).collect();
        for (index, counter) in counters.iter().enumerate() {
            connection.0.state.borrow_mut().wakers[index] = Some(waker(counter.clone()));
        }
        if event == "overflow" {
            incoming(&socket, &vec![7; MAX_BUFFER + 1]);
            assert_eq!(connection.0.state.borrow().received.len(), 0);
            assert_eq!(socket.ready_state(), WebSocket::CLOSING);
        } else {
            state_event(
                &socket,
                if event == "open" {
                    WebSocket::OPEN
                } else {
                    WebSocket::CLOSED
                },
                event,
            );
        }
        assert!(
            counters
                .iter()
                .all(|counter| counter.0.load(Ordering::SeqCst) == 1)
        );
        let task = futures::task::noop_waker();
        let mut cx = Context::from_waker(&task);
        if matches!(event, "error" | "overflow") {
            assert!(
                matches!(Pin::new(&mut connection).poll_read(&mut cx, &mut [0]), Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::BrokenPipe)
            );
        }
    }
}

#[wasm_bindgen_test(async)]
async fn websocket_write_waits_keep_the_original_interval_phase() {
    for elapsed in (3..=88).step_by(5) {
        let socket = fake_socket();
        let mut connection = Connection::new(socket.clone());
        connection.0.phase = js_sys::Date::now() - elapsed as f64;
        amount(&socket, 1);
        let wakes = Arc::new(Wakes::default());
        let task = waker(wakes.clone());
        let mut cx = Context::from_waker(&task);
        assert!(Pin::new(&mut connection).poll_flush(&mut cx).is_pending());
        async_std::task::sleep(std::time::Duration::from_millis(100 - elapsed + 20)).await;
        assert_eq!(
            wakes.0.load(Ordering::SeqCst),
            1,
            "missed original deadline at phase {elapsed}"
        );
        assert!(connection.0.state.borrow().timer.is_none());
    }
}

#[wasm_bindgen_test(async)]
async fn websocket_real_echo_preserves_fragmented_binary_data() {
    let url = echo_url();
    if url.is_empty() {
        return;
    }
    let mut connection = Connection::new(WebSocket::new(&url).unwrap());
    let sent: Vec<u8> = (0..256 * 1024).map(|index| (index % 251) as u8).collect();
    connection.write_all(&sent).await.unwrap();
    connection.flush().await.unwrap();
    let mut received = vec![0; sent.len()];
    connection.read_exact(&mut received).await.unwrap();
    assert_eq!(sent, received);
    connection.close().await.unwrap();
}
