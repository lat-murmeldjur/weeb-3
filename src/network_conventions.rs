use crate::*;
use async_std::sync::MutexGuard;
use event_listener::Event;
use libp2p::{
    core::{Endpoint, transport::PortUse},
    futures::task::AtomicWaker,
    identity,
    multiaddr::Protocol,
    ping,
    swarm::{
        ConnectionDenied, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent, THandlerOutEvent,
        ToSwarm,
        behaviour::DialFailure,
        dial_opts::{DialOpts, PeerCondition},
    },
};
use std::{
    ops::{Deref, DerefMut},
    task::{Context, Poll},
};

pub(crate) async fn yield_after_swarm_event(events_since_browser_yield: &mut usize) {
    *events_since_browser_yield += 1;
    if *events_since_browser_yield >= 32 {
        *events_since_browser_yield = 0;
        async_std::task::sleep(Duration::ZERO).await;
    } else {
        async_std::task::yield_now().await;
    }
}

impl Weeb3 {
    pub(crate) async fn has_unsettled_accounting(&self) -> bool {
        let accounting_peers = {
            let peers = self.wings.accounting_peers.lock().await;
            peers.values().cloned().collect::<Vec<_>>()
        };
        for accounting_peer in accounting_peers {
            if accounting_peer.lock().await.reserve != 0 {
                return true;
            }
        }

        !self.wings.ongoing_cheques.lock().await.is_empty()
    }

    pub(crate) async fn connection_counts(&self) -> (u64, u64) {
        let population = self.connection_population.lock().await;
        (population.connected, population.ongoing)
    }

    pub(crate) async fn wait_for_connections(&self, minimum: u64, timeout_ms: u64) -> u64 {
        if timeout_ms == 0 {
            return self.get_connections().await;
        }
        let generation = self.current_connection_generation();
        let ready = async {
            loop {
                let changed = {
                    let population = self.connection_population.lock().await;
                    if self.current_connection_generation() != generation {
                        return 0;
                    }
                    if population.connected >= minimum {
                        return population.connected;
                    }
                    population.changed.listen()
                };
                changed.await;
            }
        };
        match async_std::future::timeout(Duration::from_millis(timeout_ms), ready).await {
            Ok(connections) => connections,
            Err(_) => 0,
        }
    }

    pub(crate) fn service_worker_network_id(&self) -> u64 {
        self.service_worker_network_id.load(Ordering::Acquire) as u64
    }

    pub(crate) async fn connect_bootnodes_for_current_network(
        &self,
        nodes: Vec<(String, bool)>,
        expected_network_id: u64,
    ) {
        let (generation, network_id) = self.current_connection_context().await;
        if network_id != expected_network_id {
            return;
        }

        let private_custom_bootnodes = profile_for_swarm_network_id(expected_network_id)
            .is_some_and(|profile| {
                nodes.iter().any(|(address, _)| {
                    !profile.bootnodes.contains(&address.as_str())
                        && is_private_or_local_bootnode(address)
                })
            });
        self.allow_private_gossip
            .store(private_custom_bootnodes, Ordering::Release);

        for (address, usable_in_protocols) in nodes {
            let _ = self
                .bootnode_port
                .0
                .try_send((address, usable_in_protocols, generation));
        }
    }

    pub(super) fn current_connection_generation(&self) -> u64 {
        self.connection_generation.load(Ordering::Acquire)
    }

    pub(super) async fn current_connection_context(&self) -> (u64, u64) {
        loop {
            let before = self.current_connection_generation();
            let network_id = *self.network_id.lock().await;
            let after = self.current_connection_generation();
            if before == after {
                return (after, network_id);
            }
        }
    }

    pub(super) fn bump_connection_generation(&self) {
        let _ = self.connection_generation.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |generation| Some(generation.saturating_add(1)),
        );
    }

    pub(super) async fn disconnect_all_peers(&self) {
        let wings = &self.wings;
        let mut peers = wings
            .connection_attempts
            .lock()
            .await
            .keys()
            .copied()
            .collect::<HashSet<_>>();
        wings
            .physical_connections
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        wings
            .handshake_ready_connections
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        *wings
            .canonical_identify_address
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;

        {
            let mut swarm = self.swarm.lock().await;
            peers.extend(swarm.connected_peers().copied());
            for peer in peers {
                let _ = swarm.disconnect_peer_id(peer);
            }
            let external_addresses = swarm.external_addresses().cloned().collect::<Vec<_>>();
            for address in external_addresses {
                swarm.remove_external_address(&address);
            }
        }

        wings.connected_peers.lock().await.clear();
        wings.overlay_peers.lock().await.clear();
        wings.connection_attempts.lock().await.clear();
        wings.connection_cooldowns.lock().await.clear();
        wings.accounting_peers.lock().await.clear();
        wings.bootnodes.lock().await.clear();
        wings.ongoing_cheques.lock().await.clear();
        ACCOUNTING_DRAINED.notify(usize::MAX);
        wings.known_peers.lock().await.clear();
        wings.delayed_peer_retries.lock().await.clear();
        wings.rejected_duplicate_peers.lock().await.clear();

        let mut population = self.connection_population.lock().await;
        population.connected = 0;
        population.ongoing = 0;
        population.changed.notify(usize::MAX);
    }

    pub(super) async fn promote_priced_peer(&self, wings: &Arc<Wings>, peer: PeerId) {
        let connected_peers_guard = wings.connected_peers.lock().await;
        let peer_file = match connected_peers_guard.get(&peer) {
            Some(peer_file) => peer_file,
            None => return,
        };
        if exclusive_physical_connection(&wings.physical_connections, &peer)
            != Some(peer_file.connection_id)
        {
            return;
        }
        let had_reservation =
            remove_connection_attempt(wings, &peer, peer_file.connection_attempt_id).await;
        if !had_reservation {
            return;
        }

        let overlay_hex = hex::encode(&peer_file.overlay);
        let bootnode = wings.bootnodes.lock().await.contains(&peer);

        let (promoted, duplicate_owner) = if !bootnode {
            let mut overlay_peers_map = wings.overlay_peers.lock().await;
            match overlay_peers_map.get(&peer_file.overlay) {
                None => {
                    overlay_peers_map.insert(peer_file.overlay.clone(), peer);
                    (true, None)
                }
                Some(owner) if owner == &peer => (false, None),
                Some(owner) => (false, Some(*owner)),
            }
        } else {
            (true, None)
        };

        complete_connection_reservation(&self.connection_population, promoted).await;

        drop(connected_peers_guard);

        if promoted {
            let kind = if bootnode { "bootnode" } else { "peer" };
            self.interface_log(format!("Connected to {kind} {overlay_hex}"));
        } else if let Some(owner) = duplicate_owner {
            self.interface_log(format!(
                "Rejected duplicate overlay {} peer={} existing_peer={}",
                overlay_hex, peer, owner
            ));
            wings
                .rejected_duplicate_peers
                .lock()
                .await
                .insert(peer, owner);
            wings.connected_peers.lock().await.remove(&peer);
            wings.accounting_peers.lock().await.remove(&peer);
            wings.known_peers.lock().await.remove(&peer);
            wings.delayed_peer_retries.lock().await.remove(&peer);
            let _ = self.swarm.lock().await.disconnect_peer_id(peer);
        }
    }
}

pub(crate) struct StreamBehaviour {
    inner: libp2p_stream::Behaviour,
}

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

impl NetworkBehaviour for StreamBehaviour {
    type ConnectionHandler = <libp2p_stream::Behaviour as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = <libp2p_stream::Behaviour as NetworkBehaviour>::ToSwarm;

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

#[derive(Default)]
pub(crate) struct ConnectionPopulation {
    pub(crate) connected: u64,
    pub(crate) ongoing: u64,
    pub(crate) changed: Event,
}

pub(crate) async fn release_connection_reservation(population: &Arc<Mutex<ConnectionPopulation>>) {
    complete_connection_reservation(population, false).await;
}

pub(crate) async fn release_connected_peer(population: &Arc<Mutex<ConnectionPopulation>>) {
    let mut population = population.lock().await;
    population.connected = population.connected.saturating_sub(1);
    population.changed.notify(usize::MAX);
}

pub(crate) async fn complete_connection_reservation(
    population: &Arc<Mutex<ConnectionPopulation>>,
    connected: bool,
) {
    let mut population = population.lock().await;
    population.ongoing = population.ongoing.saturating_sub(1);
    if connected {
        population.connected = population.connected.saturating_add(1);
    }
    population.changed.notify(usize::MAX);
}

pub(crate) async fn reserve_connection_capacity(
    population: &Arc<Mutex<ConnectionPopulation>>,
    connection_generation: &Arc<AtomicU64>,
    expected_generation: u64,
) -> bool {
    loop {
        let changed = population.lock().await.changed.listen();
        if connection_generation.load(Ordering::Acquire) != expected_generation {
            return false;
        }
        if try_reserve_connection_capacity(population).await {
            return true;
        }
        changed.await;
    }
}

pub(crate) async fn try_reserve_connection_capacity(
    population: &Arc<Mutex<ConnectionPopulation>>,
) -> bool {
    let mut population = population.lock().await;
    if connection_dial_capacity_available(population.connected, population.ongoing) {
        population.ongoing = population.ongoing.saturating_add(1);
        true
    } else {
        false
    }
}

pub(crate) type DelayedPeerRetryMap = Arc<Mutex<HashMap<PeerId, (u64, usize)>>>;
static NEXT_PEER_RETRY_ID: AtomicUsize = AtomicUsize::new(1);

pub(crate) async fn queue_peer_dial_retry(
    address: Multiaddr,
    expected_generation: u64,
    connection_generation: Arc<AtomicU64>,
    peers_instructions: mpsc::Sender<PeerDialInstruction>,
    bootnode: bool,
    delayed_peer_retries: DelayedPeerRetryMap,
) {
    let Some(peer) = try_from_multiaddr(&address) else {
        return;
    };
    let retry_id = NEXT_PEER_RETRY_ID.fetch_add(1, Ordering::Relaxed).max(1);
    delayed_peer_retries
        .lock()
        .await
        .insert(peer, (expected_generation, retry_id));

    spawn_local(async move {
        async_std::task::sleep(Duration::from_millis(failed_peer_retry_delay_ms(&address))).await;

        let mut delayed = delayed_peer_retries.lock().await;
        if delayed.get(&peer) != Some(&(expected_generation, retry_id)) {
            return;
        }
        if connection_generation.load(Ordering::Acquire) != expected_generation {
            delayed.remove(&peer);
            return;
        }
        delayed.remove(&peer);
        drop(delayed);

        let _ = peers_instructions
            .send(PeerDialInstruction {
                underlay: address.to_vec(),
                generation: expected_generation,
                retry: true,
                bootnode,
            })
            .await;
    });
}

pub(crate) fn failed_peer_retry_delay_ms(address: &Multiaddr) -> u64 {
    let address = address.to_string();
    if crate::network_profile::MAINNET_BOOTNODES.contains(&address.as_str()) {
        MAINNET_BOOTNODE_RETRY_DELAY_MS
            .saturating_add(rand::random::<u64>() % MAINNET_BOOTNODE_RETRY_JITTER_MS)
    } else {
        PEER_RETRY_DELAY_MS
    }
}

pub(crate) struct SharedSwarm {
    inner: Mutex<Swarm<Behaviour>>,
    event_waker: AtomicWaker,
}

impl SharedSwarm {
    pub(crate) fn new(swarm: Swarm<Behaviour>) -> Self {
        Self {
            inner: Mutex::new(swarm),
            event_waker: AtomicWaker::new(),
        }
    }

    pub(crate) async fn lock(&self) -> SharedSwarmGuard<'_> {
        SharedSwarmGuard {
            inner: Some(self.inner.lock().await),
            event_waker: &self.event_waker,
        }
    }

    pub(crate) async fn next_event(&self) -> Option<SwarmEvent<BehaviourEvent>> {
        std::future::poll_fn(|cx| {
            self.event_waker.register(cx.waker());
            let Some(mut swarm) = self.inner.try_lock() else {
                return Poll::Pending;
            };
            swarm.poll_next_unpin(cx)
        })
        .await
    }
}

pub(crate) struct SharedSwarmGuard<'a> {
    inner: Option<MutexGuard<'a, Swarm<Behaviour>>>,
    event_waker: &'a AtomicWaker,
}

impl Deref for SharedSwarmGuard<'_> {
    type Target = Swarm<Behaviour>;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().unwrap()
    }
}

impl DerefMut for SharedSwarmGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().unwrap()
    }
}

impl Drop for SharedSwarmGuard<'_> {
    fn drop(&mut self) {
        drop(self.inner.take());
        self.event_waker.wake();
    }
}

pub(crate) type OverlayPeerMap = Arc<Mutex<HashMap<Vec<u8>, PeerId>>>;
pub(crate) type PeerAccountingMap = Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>;
pub(crate) type PhysicalConnectionMap =
    Arc<std::sync::Mutex<HashMap<PeerId, HashSet<ConnectionId>>>>;

pub(crate) fn record_physical_connection_established(
    connections: &PhysicalConnectionMap,
    peer: &PeerId,
    connection_id: ConnectionId,
) {
    let mut connections = connections
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    connections.entry(*peer).or_default().insert(connection_id);
}

pub(crate) fn record_physical_connection_closed(
    connections: &PhysicalConnectionMap,
    peer: &PeerId,
    connection_id: ConnectionId,
) {
    let mut connections = connections
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let remove_peer = connections.get_mut(peer).is_some_and(|peer_connections| {
        peer_connections.remove(&connection_id);
        peer_connections.is_empty()
    });
    if remove_peer {
        connections.remove(peer);
    }
}

pub(crate) fn exclusive_physical_connection(
    connections: &PhysicalConnectionMap,
    peer: &PeerId,
) -> Option<ConnectionId> {
    let connections = connections
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let peer_connections = connections.get(peer)?;
    (peer_connections.len() == 1).then(|| *peer_connections.iter().next().unwrap())
}

pub(crate) struct TransportConnectionSession {
    peer: PeerId,
    connection_id: ConnectionId,
    physical_connections: PhysicalConnectionMap,
}

impl TransportConnectionSession {
    pub(crate) fn capture(
        peer: PeerId,
        connection_id: ConnectionId,
        physical_connections: PhysicalConnectionMap,
    ) -> Option<Self> {
        let session = Self {
            peer,
            connection_id,
            physical_connections,
        };
        session.is_current().then_some(session)
    }

    pub(crate) fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    pub(crate) fn is_current(&self) -> bool {
        exclusive_physical_connection(&self.physical_connections, &self.peer)
            == Some(self.connection_id)
    }
}

pub(crate) type OutboundProtocolSession = TransportConnectionSession;

pub(crate) type ConnectionAttemptId = usize;

pub(crate) struct ConnectionAttempt {
    pub(crate) id: ConnectionAttemptId,
    pub(crate) physical_connection_id: Option<ConnectionId>,
    pub(crate) identify_failed: bool,
    pub(crate) handshake_ready: mpsc::Sender<ConnectionId>,
}

pub(crate) struct KnownPeer {
    pub(crate) underlay: Multiaddr,
    pub(crate) generation: u64,
}

pub(crate) struct PeerDialInstruction {
    pub(crate) underlay: Vec<u8>,
    pub(crate) generation: u64,
    pub(crate) retry: bool,
    pub(crate) bootnode: bool,
}
pub(crate) type ConnectionInstruction = (
    Multiaddr,
    bool,
    u64,
    ConnectionAttemptId,
    mpsc::Receiver<ConnectionId>,
);

static NEXT_CONNECTION_ATTEMPT_ID: AtomicUsize = AtomicUsize::new(1);

pub(crate) struct QueuedPeerDial {
    pub(crate) peer: PeerId,
    pub(crate) dial_addr: Multiaddr,
    pub(crate) generation: u64,
    pub(crate) bootnode: bool,
}

pub(crate) fn peer_dial_candidates(
    instruction: PeerDialInstruction,
    public_network: bool,
) -> impl Iterator<Item = QueuedPeerDial> {
    deserialize_underlays(&instruction.underlay)
        .into_iter()
        .filter_map(move |source_addr| {
            let peer = try_from_multiaddr(&source_addr)?;
            if public_network
                && !instruction.retry
                && !instruction.bootnode
                && !is_publicly_dialable_underlay(&source_addr)
            {
                return None;
            }
            let dial_addr = browser_dial_address(source_addr).ok()?;
            Some(QueuedPeerDial {
                peer,
                dial_addr,
                generation: instruction.generation,
                bootnode: instruction.bootnode,
            })
        })
}

pub(crate) fn is_private_or_local_bootnode(address: &str) -> bool {
    let Ok(address) = address.parse::<Multiaddr>() else {
        return false;
    };
    match address.iter().next() {
        Some(Protocol::Ip4(address)) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_unspecified()
        }
        Some(Protocol::Dns4(_)) => !is_publicly_dialable_underlay(&address),
        _ => false,
    }
}

#[derive(Default)]
pub(crate) struct Wings {
    pub(crate) connected_peers: Mutex<HashMap<PeerId, PeerFile>>,
    pub(crate) overlay_peers: OverlayPeerMap,
    pub(crate) bootnodes: Mutex<HashSet<PeerId>>,
    pub(crate) accounting_peers: PeerAccountingMap,
    pub(crate) ongoing_cheques: Mutex<HashMap<PeerId, (u64, u64)>>,
    pub(crate) connection_attempts: Mutex<HashMap<PeerId, ConnectionAttempt>>,
    pub(crate) connection_cooldowns: Mutex<HashSet<PeerId>>,
    pub(crate) physical_connections: PhysicalConnectionMap,
    pub(crate) handshake_ready_connections: std::sync::Mutex<HashSet<(PeerId, ConnectionId)>>,
    pub(crate) canonical_identify_address: std::sync::Mutex<Option<Multiaddr>>,
    pub(crate) known_peers: Mutex<HashMap<PeerId, KnownPeer>>,
    pub(crate) delayed_peer_retries: DelayedPeerRetryMap,
    pub(crate) rejected_duplicate_peers: Mutex<HashMap<PeerId, PeerId>>,
}

pub(crate) async fn get_or_create_accounting_peer(
    wings: &Wings,
    peer: PeerId,
) -> Arc<Mutex<PeerAccounting>> {
    wings
        .accounting_peers
        .lock()
        .await
        .entry(peer)
        .or_insert_with(|| {
            Arc::new(Mutex::new(PeerAccounting {
                balance: 0,
                surplus_balance: 0,
                threshold: 0,
                reserve: 0,
                refreshment: 0.0,
                refresh_scheduled: false,
                id: peer,
                connection_id: None,
            }))
        })
        .clone()
}

pub(crate) async fn try_mark_connection_attempt(
    wings: &Arc<Wings>,
    peer: &PeerId,
) -> Option<(ConnectionAttemptId, mpsc::Receiver<ConnectionId>)> {
    let connected_peers = wings.connected_peers.lock().await;
    if connected_peers.contains_key(peer) {
        return None;
    }

    let connection_cooldowns = wings.connection_cooldowns.lock().await;
    if connection_cooldowns.contains(peer) {
        return None;
    }

    let delayed_peer_retries = wings.delayed_peer_retries.lock().await;
    if delayed_peer_retries.contains_key(peer) {
        return None;
    }

    let mut connection_attempts = wings.connection_attempts.lock().await;
    if connection_attempts.contains_key(peer) {
        return None;
    }
    let attempt_id = NEXT_CONNECTION_ATTEMPT_ID
        .fetch_add(1, Ordering::Relaxed)
        .max(1);
    let (handshake_ready, ready_connection) = mpsc::bounded(1);
    connection_attempts.insert(
        *peer,
        ConnectionAttempt {
            id: attempt_id,
            physical_connection_id: None,
            identify_failed: false,
            handshake_ready,
        },
    );
    Some((attempt_id, ready_connection))
}

pub(crate) async fn mark_handshake_ready_connection(
    wings: &Arc<Wings>,
    peer: PeerId,
    connection_id: ConnectionId,
) {
    let attempts = wings.connection_attempts.lock().await;
    let Some(attempt) = attempts.get(&peer).filter(|attempt| {
        attempt.physical_connection_id == Some(connection_id) && !attempt.identify_failed
    }) else {
        return;
    };
    let physical = wings
        .physical_connections
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(connections) = physical
        .get(&peer)
        .filter(|connections| connections.contains(&connection_id))
    else {
        return;
    };
    let exclusive_connection = connections.len() == 1;
    let mut ready = wings
        .handshake_ready_connections
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    ready.insert((peer, connection_id));
    drop(ready);
    drop(physical);
    if exclusive_connection {
        let _ = attempt.handshake_ready.try_send(connection_id);
    }
}

pub(crate) async fn close_failed_identify_connection(
    wings: &Arc<Wings>,
    swarm: &Arc<SharedSwarm>,
    peer: &PeerId,
    connection_id: ConnectionId,
) -> bool {
    {
        let mut attempts = wings.connection_attempts.lock().await;
        let Some(attempt) = attempts.get_mut(peer).filter(|attempt| {
            attempt.physical_connection_id == Some(connection_id) && !attempt.identify_failed
        }) else {
            return false;
        };
        if !wings
            .physical_connections
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(peer)
            .is_some_and(|connections| connections.contains(&connection_id))
            || wings
                .handshake_ready_connections
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(&(*peer, connection_id))
        {
            return false;
        }
        attempt.identify_failed = true;
    }
    swarm.lock().await.close_connection(connection_id)
}

pub(crate) async fn remove_connection_attempt(
    wings: &Arc<Wings>,
    peer: &PeerId,
    expected_attempt_id: ConnectionAttemptId,
) -> bool {
    let mut attempts = wings.connection_attempts.lock().await;
    if attempts.get(peer).map(|attempt| attempt.id) != Some(expected_attempt_id) {
        return false;
    }
    attempts.remove(peer).is_some()
}

pub(crate) async fn remove_connection_attempt_for_connection(
    wings: &Arc<Wings>,
    peer: &PeerId,
    connection_id: ConnectionId,
) -> bool {
    let mut attempts = wings.connection_attempts.lock().await;
    if attempts
        .get(peer)
        .and_then(|attempt| attempt.physical_connection_id)
        != Some(connection_id)
    {
        return false;
    }
    attempts.remove(peer).is_some()
}

pub(crate) async fn connection_attempt_is_current(
    wings: &Arc<Wings>,
    peer: &PeerId,
    expected_attempt_id: ConnectionAttemptId,
) -> bool {
    let attempts = wings.connection_attempts.lock().await;
    attempts.get(peer).map(|attempt| attempt.id) == Some(expected_attempt_id)
}

pub(crate) async fn start_owned_connection_attempt(
    swarm: &Arc<SharedSwarm>,
    wings: &Arc<Wings>,
    peer: &PeerId,
    dial_addr: &Multiaddr,
    attempt_id: ConnectionAttemptId,
) -> Result<bool, libp2p::swarm::DialError> {
    if swarm.lock().await.is_connected(peer) {
        let physical_connection_id =
            exclusive_physical_connection(&wings.physical_connections, peer);
        let attempt_owned = {
            let mut attempts = wings.connection_attempts.lock().await;
            match attempts.get_mut(peer) {
                Some(attempt) if attempt.id == attempt_id && physical_connection_id.is_some() => {
                    attempt.physical_connection_id = physical_connection_id;
                    true
                }
                _ => false,
            }
        };
        return Ok(attempt_owned);
    }

    let options = DialOpts::peer_id(*peer)
        .condition(PeerCondition::DisconnectedAndNotDialing)
        .addresses(vec![dial_addr.clone()])
        .build();
    let connection_id = options.connection_id();
    let attempt_owned = {
        let mut attempts = wings.connection_attempts.lock().await;
        match attempts.get_mut(peer) {
            Some(attempt) if attempt.id == attempt_id => {
                attempt.physical_connection_id = Some(connection_id);
                true
            }
            _ => false,
        }
    };
    if attempt_owned {
        swarm.lock().await.dial(options)?;
    }
    Ok(attempt_owned)
}

pub(crate) async fn current_accounting_protocol_session(
    wings: &Arc<Wings>,
    peer: &PeerId,
    accounting_peer: &Arc<Mutex<PeerAccounting>>,
    connection_id: ConnectionId,
) -> Option<OutboundProtocolSession> {
    let connected_peers = wings.connected_peers.lock().await;
    if !connected_peers
        .get(peer)
        .is_some_and(|peer_file| peer_file.connection_id == connection_id)
    {
        return None;
    }
    let owns_account = wings
        .accounting_peers
        .lock()
        .await
        .get(peer)
        .is_some_and(|current| Arc::ptr_eq(current, accounting_peer));
    if !owns_account {
        return None;
    }
    if accounting_peer.lock().await.connection_id != Some(connection_id) {
        return None;
    }
    OutboundProtocolSession::capture(*peer, connection_id, wings.physical_connections.clone())
}

pub(crate) async fn claim_current_cheque(
    wings: &Arc<Wings>,
    peer: PeerId,
    accounting_peer: &Arc<Mutex<PeerAccounting>>,
    connection_id: ConnectionId,
    amount: u64,
    generation: u64,
) -> bool {
    let connected_peers = wings.connected_peers.lock().await;
    if !connected_peers
        .get(&peer)
        .is_some_and(|peer_file| peer_file.connection_id == connection_id)
    {
        return false;
    }
    let owns_account = wings
        .accounting_peers
        .lock()
        .await
        .get(&peer)
        .is_some_and(|current| Arc::ptr_eq(current, accounting_peer));
    if !owns_account || accounting_peer.lock().await.connection_id != Some(connection_id) {
        return false;
    }

    let mut cheques = wings.ongoing_cheques.lock().await;
    if cheques.contains_key(&peer)
        || exclusive_physical_connection(&wings.physical_connections, &peer) != Some(connection_id)
    {
        return false;
    }
    cheques.insert(peer, (amount, generation));
    true
}

// Only accounting-session close waits subscribe; retrieval selectors do not.
pub(crate) static ACCOUNTING_DRAINED: Event = Event::new();

pub(crate) async fn quiesce_drain_and_close_accounting_session(
    wings: &Arc<Wings>,
    swarm: &Arc<SharedSwarm>,
    peer: PeerId,
    accounting_peer: &Arc<Mutex<PeerAccounting>>,
    connection_id: ConnectionId,
) {
    // Reserved requests must settle before the accounting connection closes.
    let pending_cheque = {
        let connected_peers = wings.connected_peers.lock().await;
        let owns_connection = connected_peers
            .get(&peer)
            .is_some_and(|peer_file| peer_file.connection_id == connection_id);
        let owns_account = {
            let accounting = wings.accounting_peers.lock().await;
            accounting
                .get(&peer)
                .is_some_and(|current| Arc::ptr_eq(current, accounting_peer))
        };
        let mut account = accounting_peer.lock().await;
        if owns_connection && owns_account && account.connection_id == Some(connection_id) {
            account.connection_id = None;
            drop(account);
            wings.ongoing_cheques.lock().await.get(&peer).copied()
        } else {
            None
        }
    };

    loop {
        let changed = ACCOUNTING_DRAINED.listen();
        let reserve_drained = accounting_peer.lock().await.reserve == 0;
        let cheque_drained = match pending_cheque {
            Some(claim) => {
                let cheques = wings.ongoing_cheques.lock().await;
                cheques.get(&peer).copied() != Some(claim)
            }
            None => true,
        };
        if reserve_drained && cheque_drained {
            break;
        }
        changed.await;
    }

    let mut swarm = swarm.lock().await;
    let _ = swarm.close_connection(connection_id);
}

#[derive(NetworkBehaviour)]
pub(crate) struct Behaviour {
    pub(crate) identify: identify::Behaviour,
    pub(crate) ping: ping::Behaviour,
    pub(crate) stream: StreamBehaviour,
}

impl Behaviour {
    pub(crate) fn new(local_public_key: identity::PublicKey) -> Self {
        Self {
            identify: identify::Behaviour::new(
                identify::Config::new("/weeb-3".into(), local_public_key)
                    .with_interval(Duration::from_secs(3600)),
            ),
            ping: ping::Behaviour::default(),
            stream: StreamBehaviour::new(),
        }
    }
}
