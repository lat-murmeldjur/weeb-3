//! Browser-independent peer connection conventions.

use std::{collections::HashMap, hash::Hash};

pub(crate) const CONNECTION_BUILDUP_LIMIT: u64 = 200;
// Populate toward 200 without letting concurrent handshakes starve retrieval.
pub(crate) const CONNECTION_DIAL_CONCURRENCY_LIMIT: u64 = 128;

pub(crate) fn connection_dial_capacity_available(connected: u64, ongoing: u64) -> bool {
    connected.saturating_add(ongoing) < CONNECTION_BUILDUP_LIMIT
        && ongoing < CONNECTION_DIAL_CONCURRENCY_LIMIT
}

/// Retrieval starts with the first accounting-ready peer.
pub(crate) fn retrieval_dispatch_available(priced_peer_count: usize) -> bool {
    priced_peer_count > 0
}

/// Match Bee's accounting disconnect blocklist interval.
///
/// `balance + reserve` conservatively represents the latent debt that the
/// remote Bee may still observe while dispatched retrievals settle.
pub(crate) fn bee_reconnect_delay_seconds(
    balance: u64,
    reserve: u64,
    payment_threshold: u64,
    refresh_rate: u64,
) -> u64 {
    if refresh_rate == 0 {
        return 1;
    }

    // Weeb identifies as a light node. Bee advertises its threshold and
    // refresh rate divided by the protocol's light factor, but computes its
    // disconnect blocklist with the corresponding full values.
    const BEE_LIGHT_ACCOUNTING_FACTOR: u64 = 10;
    let bee_refresh_rate = refresh_rate.saturating_mul(BEE_LIGHT_ACCOUNTING_FACTOR);
    let bee_payment_threshold = payment_threshold.saturating_mul(BEE_LIGHT_ACCOUNTING_FACTOR);

    balance
        .saturating_add(reserve)
        .max(bee_refresh_rate)
        .saturating_add(bee_payment_threshold)
        .checked_div(bee_refresh_rate)
        .unwrap_or(1)
        .max(1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConnectionCounterRelease {
    None,
    Ongoing,
    Connected,
}

/// Choose exactly one counter to release for a terminal peer lifecycle event.
///
/// A pending reservation wins over bootnode/overlay state because an
/// unpriced peer may already have a connected peer file without ever having
/// incremented the promoted connection count.
pub(crate) fn connection_counter_release(
    had_reservation: bool,
    removed_owned_overlay: bool,
    tracked_bootnode: bool,
) -> ConnectionCounterRelease {
    if had_reservation {
        ConnectionCounterRelease::Ongoing
    } else if removed_owned_overlay || tracked_bootnode {
        ConnectionCounterRelease::Connected
    } else {
        ConnectionCounterRelease::None
    }
}

/// Remove an overlay route only when it belongs to the peer being cleaned up.
pub(crate) fn remove_overlay_owner<K, V>(
    overlays: &mut HashMap<K, V>,
    overlay: &K,
    peer: &V,
) -> bool
where
    K: Eq + Hash,
    V: Eq,
{
    if overlays.get(overlay) != Some(peer) {
        return false;
    }
    overlays.remove(overlay);
    true
}

#[cfg(target_arch = "wasm32")]
use std::task::{Context, Poll};

#[cfg(target_arch = "wasm32")]
use libp2p::{
    PeerId,
    core::{Endpoint, Multiaddr, transport::PortUse},
    swarm::{
        ConnectionDenied, ConnectionId, DialError, FromSwarm, NetworkBehaviour, THandler,
        THandlerInEvent, THandlerOutEvent, ToSwarm, behaviour::DialFailure,
    },
};

#[cfg(target_arch = "wasm32")]
pub(crate) use libp2p_stream::{Control as StreamControl, OpenStreamError};

/// Stream behaviour that rejects implicit transport dialing.
#[cfg(target_arch = "wasm32")]
pub(crate) struct StreamBehaviour {
    inner: libp2p_stream::Behaviour,
}

#[cfg(target_arch = "wasm32")]
impl StreamBehaviour {
    pub(crate) fn new() -> Self {
        Self {
            inner: libp2p_stream::Behaviour::new(),
        }
    }

    pub(crate) fn new_control(&self) -> StreamControl {
        self.inner.new_control()
    }
}

#[cfg(target_arch = "wasm32")]
impl NetworkBehaviour for StreamBehaviour {
    type ConnectionHandler = <libp2p_stream::Behaviour as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = <libp2p_stream::Behaviour as NetworkBehaviour>::ToSwarm;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.inner
            .handle_pending_inbound_connection(connection_id, local_addr, remote_addr)
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.inner.handle_established_inbound_connection(
            connection_id,
            peer,
            local_addr,
            remote_addr,
        )
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        self.inner.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.inner.handle_established_outbound_connection(
            connection_id,
            peer,
            addr,
            role_override,
            port_use,
        )
    }

    fn on_swarm_event(&mut self, event: FromSwarm<'_>) {
        self.inner.on_swarm_event(event);
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        self.inner
            .on_connection_handler_event(peer_id, connection_id, event);
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        loop {
            match self.inner.poll(cx) {
                Poll::Ready(ToSwarm::Dial { opts }) => {
                    let peer_id = opts.get_peer_id();
                    let connection_id = opts.connection_id();
                    let error = DialError::NoAddresses;
                    self.inner
                        .on_swarm_event(FromSwarm::DialFailure(DialFailure {
                            peer_id,
                            error: &error,
                            connection_id,
                        }));
                }
                event => return event,
            }
        }
    }
}
