use std::{
    cell::RefCell,
    io,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

use bytes::{Buf, BytesMut};
use futures::{
    AsyncRead, AsyncWrite, FutureExt,
    future::{BoxFuture, Ready},
};
use js_sys::{Function, Uint8Array};
use libp2p::core::{
    multiaddr::{Multiaddr, Protocol},
    transport::{DialOpts, ListenerId, TransportError, TransportEvent},
};
use send_wrapper::SendWrapper;
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::{Event, MessageEvent, WebSocket};

const MAX_BUFFER: usize = 1024 * 1024;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_name = setTimeout)]
    fn set_timeout(callback: &Function, milliseconds: i32) -> Result<i32, JsValue>;
    #[wasm_bindgen(js_name = clearTimeout)]
    fn clear_timeout(handle: i32);
}

#[derive(Default)]
pub(crate) struct Transport;

impl libp2p::Transport for Transport {
    type Output = Connection;
    type Error = io::Error;
    type ListenerUpgrade = Ready<Result<Connection, io::Error>>;
    type Dial = BoxFuture<'static, Result<Connection, io::Error>>;

    fn listen_on(
        &mut self,
        _: ListenerId,
        address: Multiaddr,
    ) -> Result<(), TransportError<io::Error>> {
        Err(TransportError::MultiaddrNotSupported(address))
    }

    fn remove_listener(&mut self, _: ListenerId) -> bool {
        false
    }

    fn dial(
        &mut self,
        address: Multiaddr,
        options: DialOpts,
    ) -> Result<Self::Dial, TransportError<io::Error>> {
        if options.role.is_listener() {
            return Err(TransportError::MultiaddrNotSupported(address));
        }
        let url = websocket_url(&address).ok_or(TransportError::MultiaddrNotSupported(address))?;
        Ok(async move {
            let socket = WebSocket::new(&url)
                .map_err(|_| io::Error::other(format!("Invalid websocket url: {url}")))?;
            Ok(Connection::new(socket))
        }
        .boxed())
    }

    fn poll(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, io::Error>> {
        Poll::Pending
    }
}

fn websocket_url(address: &Multiaddr) -> Option<String> {
    let mut protocols = address.iter();
    let host = match (protocols.next(), protocols.next()) {
        (Some(Protocol::Ip4(ip)), Some(Protocol::Tcp(port))) => format!("{ip}:{port}"),
        (Some(Protocol::Ip6(ip)), Some(Protocol::Tcp(port))) => format!("[{ip}]:{port}"),
        (
            Some(Protocol::Dns(host) | Protocol::Dns4(host) | Protocol::Dns6(host)),
            Some(Protocol::Tcp(port)),
        ) => format!("{host}:{port}"),
        _ => return None,
    };
    let (scheme, path) = match (protocols.next(), protocols.next()) {
        (Some(Protocol::Tls), Some(Protocol::Ws(path))) | (Some(Protocol::Wss(path)), _) => {
            ("wss", path)
        }
        (Some(Protocol::Ws(path)), _) => ("ws", path),
        _ => return None,
    };
    Some(format!("{scheme}://{host}{path}"))
}

#[derive(Default)]
struct State {
    received: BytesMut,
    failed: bool,
    timer: Option<i32>,
    // Read/open, write/open, and close can have independent waiters.
    wakers: [Option<Waker>; 3],
}

fn wake_all(state: &RefCell<State>) {
    let wakers = std::mem::take(&mut state.borrow_mut().wakers);
    for waker in wakers.into_iter().flatten() {
        waker.wake();
    }
}

fn cancel_timer(state: &RefCell<State>) {
    if let Some(timer) = state.borrow_mut().timer.take() {
        clear_timeout(timer);
    }
}

pub(crate) struct Connection(SendWrapper<Socket>);

struct Socket {
    socket: WebSocket,
    phase: f64,
    state: Rc<RefCell<State>>,
    _on_state: Closure<dyn FnMut(Event)>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    on_write_ready: Closure<dyn FnMut()>,
}

impl Connection {
    fn new(socket: WebSocket) -> Self {
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let state = Rc::new(RefCell::new(State::default()));
        let on_state = Closure::<dyn FnMut(Event)>::new({
            let state = state.clone();
            move |event: Event| {
                if event.type_() != "open" {
                    cancel_timer(&state);
                }
                if event.type_() == "error" {
                    state.borrow_mut().failed = true;
                }
                wake_all(&state);
            }
        });
        socket.set_onopen(Some(on_state.as_ref().unchecked_ref()));
        socket.set_onerror(Some(on_state.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_state.as_ref().unchecked_ref()));
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new({
            let state = state.clone();
            let socket = socket.clone();
            move |event: MessageEvent| {
                let data = Uint8Array::new(&event.data());
                let length = data.length() as usize;
                let mut buffer = state.borrow_mut();
                if buffer.failed || length == 0 {
                    return;
                }
                let start = buffer.received.len();
                if length > MAX_BUFFER - start {
                    buffer.failed = true;
                    drop(buffer);
                    cancel_timer(&state);
                    let _ = socket.close();
                    wake_all(&state);
                    return;
                }
                buffer.received.resize(start + length, 0);
                data.copy_to(&mut buffer.received[start..]);
                let waker = buffer.wakers[0].take();
                drop(buffer);
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
        });
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        let on_write_ready = Closure::new({
            let state = state.clone();
            move || {
                let waker = {
                    let mut state = state.borrow_mut();
                    state.timer = None;
                    state.wakers[1].take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
        });
        Self(SendWrapper::new(Socket {
            socket,
            phase: js_sys::Date::now(),
            state,
            _on_state: on_state,
            _on_message: on_message,
            on_write_ready,
        }))
    }

    fn park(&self, cx: &Context<'_>, waiter: usize) {
        self.0.state.borrow_mut().wakers[waiter] = Some(cx.waker().clone());
    }

    fn check_error(&self) -> io::Result<()> {
        if self.0.state.borrow().failed {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }

    fn wait_to_write(&self, cx: &Context<'_>) -> Poll<io::Result<()>> {
        self.park(cx, 1);
        let mut state = self.0.state.borrow_mut();
        if state.timer.is_none() {
            // Preserve the old interval's deadlines without waking idle sockets.
            let delay =
                (100.0 - (js_sys::Date::now() - self.0.phase).rem_euclid(100.0)).ceil() as i32;
            state.timer = Some(
                set_timeout(self.0.on_write_ready.as_ref().unchecked_ref(), delay)
                    .map_err(|_| io::Error::other("Could not wait for WebSocket output"))?,
            );
        }
        Poll::Pending
    }
}

impl AsyncRead for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if output.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut state = self.0.state.borrow_mut();
        if !state.received.is_empty() {
            let count = output.len().min(state.received.len());
            state.received.copy_to_slice(&mut output[..count]);
            return Poll::Ready(Ok(count));
        }
        drop(state);
        self.check_error()?;
        if self.0.socket.ready_state() == WebSocket::CLOSED {
            return Poll::Ready(Ok(0));
        }
        self.park(cx, 0);
        Poll::Pending
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.check_error()?;
        match self.0.socket.ready_state() {
            WebSocket::CONNECTING => {
                self.park(cx, 1);
                return Poll::Pending;
            }
            WebSocket::OPEN => {}
            _ => return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let count = bytes
            .len()
            .min(MAX_BUFFER.saturating_sub(self.0.socket.buffered_amount() as usize));
        if count == 0 {
            return self.wait_to_write(cx).map_ok(|_| 0);
        }
        self.0
            .socket
            .send_with_u8_array(&bytes[..count])
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        Poll::Ready(Ok(count))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.0.socket.buffered_amount() == 0 {
            cancel_timer(&self.0.state);
            return Poll::Ready(Ok(()));
        }
        self.check_error()?;
        if self.0.socket.ready_state() == WebSocket::CLOSED {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        self.wait_to_write(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        cancel_timer(&self.0.state);
        match self.0.socket.ready_state() {
            WebSocket::CLOSED => return Poll::Ready(Ok(())),
            WebSocket::CLOSING => {}
            _ => {
                self.0
                    .socket
                    .close_with_code_and_reason(1000, "user initiated")
                    .map_err(|_| io::ErrorKind::BrokenPipe)?;
            }
        }
        self.check_error()?;
        self.park(cx, 2);
        Poll::Pending
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.socket.set_onopen(None);
        self.socket.set_onerror(None);
        self.socket.set_onclose(None);
        self.socket.set_onmessage(None);
        cancel_timer(&self.state);
        if matches!(
            self.socket.ready_state(),
            WebSocket::CONNECTING | WebSocket::OPEN
        ) {
            let _ = self
                .socket
                .close_with_code_and_reason(1000, "connection dropped");
        }
    }
}

#[cfg(test)]
#[path = "../tests/support/websocket.rs"]
mod tests;
