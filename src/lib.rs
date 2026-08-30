#![cfg(target_arch = "wasm32")]

use async_lock::Semaphore;
use async_std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use wasm_bindgen_futures::spawn_local;

pub(crate) use async_std::channel as mpsc;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::num::NonZero;
use std::ops::{Deref, DerefMut};
use std::task::{Context, Poll};
use std::time::Duration;

use rand::seq::SliceRandom;
use tar::Archive;

use web3::types::U256;

use js_sys::Date;
use libp2p::{
    PeerId, StreamProtocol, Swarm,
    core::{self, Endpoint, Multiaddr, Transport, transport::PortUse},
    futures::{StreamExt, future::join_all, join, task::AtomicWaker},
    identify, identity,
    identity::ecdsa,
    multiaddr::Protocol,
    noise, ping,
    swarm::{
        ConnectionDenied, ConnectionId, DialError, FromSwarm, NetworkBehaviour, SwarmEvent,
        THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
        behaviour::DialFailure,
        dial_opts::{DialOpts, PeerCondition},
    },
    websocket_websys, yamux,
};
pub(crate) use libp2p_stream::Control as StreamControl;
use wasm_bindgen::JsValue;
use web_sys::File;

mod manifest;
use manifest::acquire_bzz_collection;

mod bzz_stream;
use bzz_stream::*;

mod conventions;
pub(crate) use conventions::*;

mod signer;
pub(crate) use signer::PrivateKeySigner;

mod accounting;
use accounting::{
    REFRESH_RATE, RefreshmentInstruction, apply_credit, apply_refreshment,
    bee_reconnect_delay_seconds, cancel_reserve, connection_dial_capacity_available,
    connection_population_deficit, price, refreshment_due, reserve, set_payment_threshold,
};

mod addresses;
use addresses::{browser_dial_address, deserialize_underlays, is_publicly_dialable_underlay};

mod erasure_coding;

mod feed;

mod handlers;
use handlers::*;

mod stream_hls;

mod shared_runtime;

mod worker_protocol;

mod worker_runtime;
pub use worker_runtime::Weeb3WorkerRuntime;

mod interface;

mod interface_conventions;

mod library;

mod manifest_upload;

mod stream_conventions;

mod on_chain;
use on_chain::{chequebook_balance, get_price_from_oracle, web3};

mod nav;

mod network_profile;
use network_profile::{activate_profile, profile_for_swarm_network_id};

mod persistence;
use persistence::{get_chequebook_address, get_chequebook_signer_key};

mod retrieval;
use retrieval::*;

mod retrieval_conventions;
pub(crate) use retrieval_conventions::{
    RetrieveCancelRegistry, RetrieveCancelToken, retrieve_cancel_token_current,
};

mod secure_vault;
use secure_vault::{secure_ensure_authorized, secure_ensure_feed_owner, secure_reset_stamp};

mod wallet_workflows;

mod stream;

mod upload;
use upload::*;

mod ens;
use ens::resolve_ens_reference;

mod events;
use events::{ProgressRow, ProgressStore};

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

fn spawn_upload_progress_listener(
    progress_store: Arc<Mutex<ProgressStore>>,
    progress_id: String,
    progress_in: mpsc::Receiver<UploadProgressDelta>,
) {
    spawn_local(async move {
        let mut chunks_total = 0u64;
        let mut chunks_done = 0u64;
        let mut last_render = 0.0;

        while let Ok(delta) = progress_in.recv().await {
            chunks_total = chunks_total.saturating_add(delta.chunks_total_delta);
            chunks_done = chunks_done.saturating_add(delta.chunks_done_delta);

            if chunks_total > 0 {
                chunks_done = chunks_done.min(chunks_total);
            }

            let complete = chunks_total > 0 && chunks_done >= chunks_total;
            let now = Date::now();
            if !complete && now - last_render < 250.0 && !chunks_done.is_multiple_of(64) {
                continue;
            }

            let percent = if chunks_total > 0 {
                Some(((chunks_done.saturating_mul(100)) / chunks_total).min(100) as u8)
            } else {
                None
            };
            let detail = if chunks_total > 0 {
                format!("{} of {} chunks pushed", chunks_done, chunks_total)
            } else {
                "waiting for chunk plan".to_string()
            };

            progress_store
                .lock()
                .await
                .update(&progress_id, "push", percent, detail);
            last_render = now;
        }
    });
}

pub mod weeb_3 {
    pub mod etiquette_0 {
        include!(concat!(env!("OUT_DIR"), "/weeb_3.etiquette_0.rs"));
    }
    pub mod etiquette_1 {
        include!(concat!(env!("OUT_DIR"), "/weeb_3.etiquette_1.rs"));
    }
    pub mod etiquette_2 {
        include!(concat!(env!("OUT_DIR"), "/weeb_3.etiquette_2.rs"));
    }
    pub mod etiquette_4 {
        include!(concat!(env!("OUT_DIR"), "/weeb_3.etiquette_4.rs"));
    }
    pub mod etiquette_5 {
        include!(concat!(env!("OUT_DIR"), "/weeb_3.etiquette_5.rs"));
    }
    pub mod etiquette_6 {
        include!(concat!(env!("OUT_DIR"), "/weeb_3.etiquette_6.rs"));
    }
    pub mod etiquette_7 {
        include!(concat!(env!("OUT_DIR"), "/weeb_3.etiquette_7.rs"));
    }
    pub mod etiquette_8 {
        include!(concat!(env!("OUT_DIR"), "/weeb_3.etiquette_8.rs"));
    }
}

const HANDSHAKE_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/handshake/15.0.0/handshake");
const PRICING_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pricing/1.0.0/pricing");
const GOSSIP_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/hive/2.0.0/peers");
const PSEUDOSETTLE_PROTOCOL: StreamProtocol =
    StreamProtocol::new("/swarm/pseudosettle/1.0.0/pseudosettle");
const RETRIEVAL_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/retrieval/1.4.0/retrieval");
const PUSHSYNC_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pushsync/1.3.1/pushsync");
const SWAP_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/swap/1.0.0/swap");

const PROTOCOL_ROUND_TIME: f64 = 160.0;
const PUSH_CHUNK_CONFIRMATION_PEERS: usize = 6;
const RETRIEVE_CHECK_CONFIRMATION_PEERS: usize = 6;
const PUSH_CHUNK_CONCURRENCY: usize = 256;
const HANDSHAKE_PROTOCOL_TIMEOUT_MS: u64 = 20000;
const PRICING_CONNECT_TIMEOUT_MS: u64 = 20000;
const CONNECTION_CAPACITY_WAIT_MS: u64 = 25;
const PEER_DIAL_INGEST_BATCH: usize = 256;
const MAX_QUEUED_PEER_DIALS: usize = 4_096;
const FRESH_PEER_DIALS_PER_RETRY: usize = 128;
const OUTBOUND_CONNECTION_TIMEOUT_MS: u64 = 8_000;
const PRE_HANDSHAKE_CONNECTION_TIMEOUT_MS: u64 = 60_000;
const PEER_POPULATION_RESCAN_MS: u64 = 2_000;
const SWARM_EVENTS_PER_BROWSER_YIELD: usize = 32;
const PEER_RETRY_DELAY_MS: u64 = 500;
const MAINNET_BOOTNODE_RETRY_DELAY_MS: u64 = 30_000;
const MAINNET_BOOTNODE_RETRY_JITTER_MS: u64 = 5_000;
const PUSH_CHUNK_RETRY_DELAY_MS: u64 = 500;
const PUSH_CHUNK_QUEUE_BACKOFF_MS: u64 = 25;
const RETRIEVE_QUEUE_HOT_LOOP_GUARD_MS: u64 = 25;
const RANGE_REQUEST_CONCURRENCY: usize = 16;
const RETRIEVE_CHUNK_CONCURRENCY: usize = 2_048;
const RANGE_REQUEST_QUEUE_CAPACITY: usize = 256;
const LOG_QUEUE_CAPACITY: usize = 256;
const LOG_DRAIN_BATCH: usize = 64;
pub(crate) const LOG_DOM_RETAINED: u32 = 256;

#[derive(Default)]
struct ConnectionPopulation {
    connected: u64,
    ongoing: u64,
}

async fn release_connection_reservation(population: &Arc<Mutex<ConnectionPopulation>>) {
    let mut population = population.lock().await;
    population.ongoing = population.ongoing.saturating_sub(1);
}

async fn release_connected_peer(population: &Arc<Mutex<ConnectionPopulation>>) {
    let mut population = population.lock().await;
    population.connected = population.connected.saturating_sub(1);
}

async fn complete_connection_reservation(
    population: &Arc<Mutex<ConnectionPopulation>>,
    connected: bool,
) {
    let mut population = population.lock().await;
    population.ongoing = population.ongoing.saturating_sub(1);
    if connected {
        population.connected = population.connected.saturating_add(1);
    }
}

async fn reserve_connection_capacity(
    population: &Arc<Mutex<ConnectionPopulation>>,
    connection_generation: &Arc<AtomicU64>,
    expected_generation: u64,
) -> bool {
    loop {
        if connection_generation.load(Ordering::Acquire) != expected_generation {
            return false;
        }
        if try_reserve_connection_capacity(population).await {
            return true;
        }
        async_std::task::sleep(Duration::from_millis(CONNECTION_CAPACITY_WAIT_MS)).await;
    }
}

async fn try_reserve_connection_capacity(population: &Arc<Mutex<ConnectionPopulation>>) -> bool {
    let mut population = population.lock().await;
    if connection_dial_capacity_available(population.connected, population.ongoing) {
        population.ongoing = population.ongoing.saturating_add(1);
        true
    } else {
        false
    }
}

async fn current_connection_population_deficit(
    population: &Arc<Mutex<ConnectionPopulation>>,
) -> usize {
    let population = population.lock().await;
    connection_population_deficit(population.connected, population.ongoing) as usize
}

type DelayedPeerRetryMap = Arc<Mutex<HashMap<PeerId, (u64, usize)>>>;
static NEXT_PEER_RETRY_ID: AtomicUsize = AtomicUsize::new(1);

async fn queue_peer_dial_retry(
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
        drop(delayed);

        let _ = peers_instructions
            .send(PeerDialInstruction {
                underlay: address.to_vec(),
                generation: expected_generation,
                retry: true,
                bootnode,
            })
            .await;
        let mut delayed = delayed_peer_retries.lock().await;
        if delayed.get(&peer) == Some(&(expected_generation, retry_id)) {
            delayed.remove(&peer);
        }
    });
}

fn failed_peer_retry_delay_ms(address: &Multiaddr) -> u64 {
    let address = address.to_string();
    if crate::network_profile::MAINNET_BOOTNODES.contains(&address.as_str()) {
        MAINNET_BOOTNODE_RETRY_DELAY_MS
            .saturating_add(rand::random::<u64>() % MAINNET_BOOTNODE_RETRY_JITTER_MS)
    } else {
        PEER_RETRY_DELAY_MS
    }
}

pub(crate) fn interface_log_to(log_port: &mpsc::Sender<String>, log_start_ms: f64, log0: String) {
    if log_port.is_full() {
        return;
    }
    let elapsed_ms = (Date::now() - log_start_ms).max(0.0).round() as u64;
    let log = format!("[+{}ms] {}", elapsed_ms, log0);
    let _ = log_port.try_send(log);
}

pub(crate) async fn cheques_active_in_window() -> bool {
    if get_chequebook_signer_key().await.is_empty() {
        return false;
    }

    let chequebook = get_chequebook_address().await;
    if chequebook.len() != 20 {
        return false;
    }

    let w3 = match web3() {
        Ok(w3) => w3,
        Err(_) => return false,
    };

    chequebook_balance(&w3, web3::types::Address::from_slice(&chequebook))
        .await
        .is_ok_and(|balance| !balance.is_zero())
}

struct SharedSwarm {
    inner: Mutex<Swarm<Behaviour>>,
    event_waker: AtomicWaker,
}

impl SharedSwarm {
    fn new(swarm: Swarm<Behaviour>) -> Self {
        Self {
            inner: Mutex::new(swarm),
            event_waker: AtomicWaker::new(),
        }
    }

    async fn lock(&self) -> SharedSwarmGuard<'_> {
        SharedSwarmGuard {
            inner: Some(self.inner.lock().await),
            event_waker: &self.event_waker,
        }
    }

    async fn next_event(&self) -> Option<SwarmEvent<BehaviourEvent>> {
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

struct SharedSwarmGuard<'a> {
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

type AsyncPort<T> = (mpsc::Sender<T>, mpsc::Receiver<T>);
type UploadRequest = (
    Vec<Resource>,
    bool,
    erasure_coding::RedundancyLevel,
    String,
    bool,
    String,
    Option<UploadProgressSender>,
    mpsc::Sender<Vec<u8>>,
);
type BootnodeChange = (String, bool, u64);

pub(crate) struct Weeb3 {
    swarm: Arc<SharedSwarm>,
    handshake_signer: Arc<PrivateKeySigner>,
    wings: Arc<Wings>,
    log_port: AsyncPort<String>,
    log_start_ms: f64,
    chunk_port: (ChunkRetrieveSender, ChunkRetrieveReceiver),
    range_port: AsyncPort<BzzRangeRequest>,
    chunk_push_port: AsyncPort<ChunkUploadRequest>,
    upload_port: AsyncPort<UploadRequest>,
    bootnode_port: AsyncPort<BootnodeChange>,
    network_id: Mutex<u64>,
    service_worker_network_id: AtomicUsize,
    runtime_started: AtomicBool,
    allow_private_gossip: AtomicBool,
    transfer_paused: Arc<AtomicBool>,
    retrieve_cancel_registry: RetrieveCancelRegistry,
    connection_generation: Arc<AtomicU64>,
    connection_population: Arc<Mutex<ConnectionPopulation>>,
    progress: Arc<Mutex<ProgressStore>>,
}

pub(crate) type OverlayPeerMap = Arc<Mutex<HashMap<Vec<u8>, PeerId>>>;
pub(crate) type PeerAccountingMap = Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>;
pub(crate) type PhysicalConnectionMap =
    Arc<std::sync::Mutex<HashMap<PeerId, HashSet<ConnectionId>>>>;

fn record_physical_connection_established(
    connections: &PhysicalConnectionMap,
    peer: &PeerId,
    connection_id: ConnectionId,
) {
    let mut connections = connections
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    connections.entry(*peer).or_default().insert(connection_id);
}

fn record_physical_connection_closed(
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

fn exclusive_physical_connection(
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

#[derive(Clone)]
struct ChunkRetrieveSender {
    runtime_scope: usize,
    sender: mpsc::Sender<ChunkRetrieveRequest>,
}

impl ChunkRetrieveSender {
    fn runtime_scope(&self) -> usize {
        self.runtime_scope
    }

    pub(crate) fn try_send(
        &self,
        request: ChunkRetrieveRequest,
    ) -> Result<(), mpsc::TrySendError<ChunkRetrieveRequest>> {
        self.sender.try_send(request)
    }
}

type ChunkRetrieveReceiver = mpsc::Receiver<ChunkRetrieveRequest>;

static NEXT_CHUNK_RETRIEVE_RUNTIME_SCOPE: AtomicUsize = AtomicUsize::new(1);

fn chunk_retrieve_channel() -> (ChunkRetrieveSender, ChunkRetrieveReceiver) {
    let (sender, receiver) = mpsc::unbounded::<ChunkRetrieveRequest>();
    let runtime_scope = NEXT_CHUNK_RETRIEVE_RUNTIME_SCOPE.fetch_add(1, Ordering::Relaxed);
    (
        ChunkRetrieveSender {
            runtime_scope,
            sender,
        },
        receiver,
    )
}
type ConnectionAttemptId = usize;

struct ConnectionAttempt {
    id: ConnectionAttemptId,
    physical_connection_id: Option<ConnectionId>,
    identify_failed: bool,
    handshake_ready: mpsc::Sender<ConnectionId>,
}

struct KnownPeer {
    underlay: Multiaddr,
    generation: u64,
}

pub(crate) struct PeerDialInstruction {
    pub(crate) underlay: Vec<u8>,
    pub(crate) generation: u64,
    pub(crate) retry: bool,
    pub(crate) bootnode: bool,
}
type ConnectionInstruction = (
    Multiaddr,
    bool,
    u64,
    ConnectionAttemptId,
    mpsc::Receiver<ConnectionId>,
);

static NEXT_CONNECTION_ATTEMPT_ID: AtomicUsize = AtomicUsize::new(1);

fn next_connection_attempt_id() -> ConnectionAttemptId {
    NEXT_CONNECTION_ATTEMPT_ID
        .fetch_add(1, Ordering::Relaxed)
        .max(1)
}

struct QueuedPeerDial {
    peer: PeerId,
    dial_addr: Multiaddr,
    generation: u64,
    retry: bool,
    bootnode: bool,
}

fn peer_dial_candidates(
    instruction: PeerDialInstruction,
    public_network: bool,
) -> impl Iterator<Item = QueuedPeerDial> {
    let PeerDialInstruction {
        underlay,
        generation,
        retry,
        bootnode,
    } = instruction;
    deserialize_underlays(&underlay)
        .into_iter()
        .filter_map(move |source_addr| {
            let peer = try_from_multiaddr(&source_addr)?;
            if public_network && !retry && !bootnode && !is_publicly_dialable_underlay(&source_addr)
            {
                return None;
            }
            let dial_addr = browser_dial_address(source_addr).ok()?;
            Some(QueuedPeerDial {
                peer,
                dial_addr,
                generation,
                retry,
                bootnode,
            })
        })
}

fn is_private_or_local_bootnode(address: &str) -> bool {
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

pub(crate) struct ChunkRetrieveRequest {
    pub address: Vec<u8>,
    pub chan: mpsc::Sender<Vec<u8>>,
    pub cancel: Option<RetrieveCancelToken>,
    pub admission: Option<retrieval_conventions::RetrieveAdmission>,
    pub hedge_demand: Option<retrieval_conventions::SharedRetrieveHedgeDemand>,
}

pub(crate) fn chunk_retrieve_request(
    address: Vec<u8>,
    chan: mpsc::Sender<Vec<u8>>,
) -> ChunkRetrieveRequest {
    ChunkRetrieveRequest {
        address,
        chan,
        cancel: None,
        admission: None,
        hedge_demand: None,
    }
}

pub(crate) fn transfer_pause_enabled(paused: &Arc<AtomicBool>) -> bool {
    paused.load(Ordering::Relaxed)
}

pub(crate) async fn wait_transfer_unpaused(paused: &Arc<AtomicBool>) {
    while transfer_pause_enabled(paused) {
        async_std::task::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_transfer_unpaused_for_admission(
    paused: &Arc<AtomicBool>,
    admission: &Option<retrieval_conventions::RetrieveAdmission>,
) -> bool {
    while transfer_pause_enabled(paused) {
        if let Some(admission) = admission {
            if !admission.is_open()
                || async_std::future::timeout(Duration::from_millis(100), admission.wait_closed())
                    .await
                    .is_ok()
            {
                return false;
            }
        } else {
            async_std::task::sleep(Duration::from_millis(100)).await;
        }
    }

    retrieval_conventions::retrieve_admission_current(true, admission)
}

struct BzzRangeRequest {
    metadata: BzzMetadata,
    start: u64,
    end_inclusive: u64,
    cancel: Option<RetrieveCancelToken>,
    chan: mpsc::Sender<Option<(Vec<u8>, BzzMetadata)>>,
}

pub(crate) struct Wings {
    connected_peers: Mutex<HashMap<PeerId, PeerFile>>,
    overlay_peers: OverlayPeerMap,
    bootnodes: Mutex<HashSet<PeerId>>,
    accounting_peers: PeerAccountingMap,
    ongoing_cheques: Mutex<HashMap<PeerId, (u64, u64)>>,
    connection_attempts: Mutex<HashMap<PeerId, ConnectionAttempt>>,
    connection_cooldowns: Mutex<HashSet<PeerId>>,
    physical_connections: PhysicalConnectionMap,
    handshake_ready_connections: std::sync::Mutex<HashSet<(PeerId, ConnectionId)>>,
    canonical_identify_address: std::sync::Mutex<Option<Multiaddr>>,
    known_peers: Mutex<HashMap<PeerId, KnownPeer>>,
    delayed_peer_retries: DelayedPeerRetryMap,
    rejected_duplicate_peers: Mutex<HashMap<PeerId, PeerId>>,
}

async fn get_or_create_accounting_peer(wings: &Wings, peer: PeerId) -> Arc<Mutex<PeerAccounting>> {
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

async fn try_mark_connection_attempt(
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

    let mut connection_attempts = wings.connection_attempts.lock().await;
    if connection_attempts.contains_key(peer) {
        None
    } else {
        let attempt_id = next_connection_attempt_id();
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
}

async fn mark_handshake_ready_connection(
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

async fn close_failed_identify_connection(
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

async fn remove_connection_attempt(
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

async fn remove_connection_attempt_for_connection(
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

async fn connection_attempt_is_current(
    wings: &Arc<Wings>,
    peer: &PeerId,
    expected_attempt_id: ConnectionAttemptId,
) -> bool {
    let attempts = wings.connection_attempts.lock().await;
    attempts.get(peer).map(|attempt| attempt.id) == Some(expected_attempt_id)
}

async fn start_owned_connection_attempt(
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

async fn current_accounting_protocol_session(
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
    let current_accounting_peer = {
        let accounting = wings.accounting_peers.lock().await;
        accounting.get(peer).cloned()
    };
    if !current_accounting_peer
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, accounting_peer))
    {
        return None;
    }
    if accounting_peer.lock().await.connection_id != Some(connection_id) {
        return None;
    }
    OutboundProtocolSession::capture(*peer, connection_id, wings.physical_connections.clone())
}

async fn claim_current_cheque(
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
    let current_accounting_peer = {
        let accounting = wings.accounting_peers.lock().await;
        accounting.get(&peer).cloned()
    };
    if !current_accounting_peer
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, accounting_peer))
        || accounting_peer.lock().await.connection_id != Some(connection_id)
    {
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

async fn quiesce_drain_and_close_accounting_session(
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
        async_std::task::sleep(Duration::from_millis(25)).await;
    }

    let mut swarm = swarm.lock().await;
    let _ = swarm.close_connection(connection_id);
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

    pub async fn set_network_id(&self, id: String) -> bool {
        let Ok(parsed_id) = id.parse::<u64>() else {
            return false;
        };
        let profile = profile_for_swarm_network_id(parsed_id);

        let mut network_changed = false;
        {
            let mut network_id = self.network_id.lock().await;
            if let Some(profile) = profile {
                activate_profile(profile);
            }
            if *network_id != parsed_id {
                self.bump_connection_generation();
                *network_id = parsed_id;
                self.service_worker_network_id.store(
                    profile
                        .map(|profile| profile.swarm_network_id as usize)
                        .unwrap_or_default(),
                    Ordering::Release,
                );
                network_changed = true;
            }
        }

        if network_changed {
            self.allow_private_gossip.store(false, Ordering::Release);
            self.disconnect_all_peers().await;
        }

        true
    }

    fn current_connection_generation(&self) -> u64 {
        self.connection_generation.load(Ordering::Acquire)
    }

    async fn current_connection_context(&self) -> (u64, u64) {
        loop {
            let before = self.current_connection_generation();
            let network_id = *self.network_id.lock().await;
            let after = self.current_connection_generation();
            if before == after {
                return (after, network_id);
            }
        }
    }

    fn bump_connection_generation(&self) {
        let _ = self.connection_generation.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |generation| Some(generation.saturating_add(1)),
        );
    }

    async fn disconnect_all_peers(&self) {
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
        wings.known_peers.lock().await.clear();
        wings.delayed_peer_retries.lock().await.clear();
        wings.rejected_duplicate_peers.lock().await.clear();

        *self.connection_population.lock().await = ConnectionPopulation::default();
    }

    async fn promote_priced_peer(&self, wings: &Arc<Wings>, peer: PeerId) {
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
        let bootnode = {
            let bootnodes_set = wings.bootnodes.lock().await;
            bootnodes_set.contains(&peer)
        };

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
            if bootnode {
                self.interface_log(format!("Connected to bootnode {}", overlay_hex));
            } else {
                self.interface_log(format!("Connected to peer {}", overlay_hex));
            }
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
            {
                let mut connected = wings.connected_peers.lock().await;
                connected.remove(&peer);
            }
            {
                let mut accounting = wings.accounting_peers.lock().await;
                accounting.remove(&peer);
            }
            wings.known_peers.lock().await.remove(&peer);
            wings.delayed_peer_retries.lock().await.remove(&peer);
            {
                let mut swarm = self.swarm.lock().await;
                let _ = swarm.disconnect_peer_id(peer);
            }
        }
    }

    pub async fn post_upload_with_redundancy(
        &self,
        file: File,
        encryption: bool,
        redundancy_level: erasure_coding::RedundancyLevel,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Vec<u8> {
        let (chan_out, chan_in) = mpsc::bounded::<Vec<u8>>(1);
        let (progress_out, progress_in) = mpsc::unbounded::<UploadProgressDelta>();

        let f_size = file.size();
        let f_name = file.name();
        let progress_id = self
            .start_progress("upload", f_name.clone(), "read", Some(0), "reading input")
            .await;
        spawn_upload_progress_listener(self.progress.clone(), progress_id.clone(), progress_in);
        let f_type0 = file.type_();
        let f_type = if f_type0.starts_with("text/") {
            f_type0 + "; charset=utf-8"
        } else {
            f_type0
        };

        let mut fvec0 = Vec::new();

        let mut index_document = "".to_string();

        if f_type == "application/x-tar" || f_type == "application/tar" {
            index_document = match index_string.is_empty() {
                true => "index.html".to_string(),
                false => index_string,
            };

            let content = read_file(file).await;
            if content.is_empty() && f_size > 0.0 {
                self.finish_progress(&progress_id, "failed", "file read failed", false)
                    .await;
                return upload_result("upload result: failed to read file", "");
            }

            self.update_progress(&progress_id, "parse", Some(20), "reading tar archive")
                .await;

            let mut archive = Archive::new(&content[..]);

            let entries = match archive.entries() {
                Ok(entries) => entries,
                Err(_) => {
                    self.finish_progress(&progress_id, "failed", "invalid tar archive", false)
                        .await;
                    return upload_result("upload result: invalid tar archive", "");
                }
            };

            for f0 in entries {
                let mut f01 = match f0 {
                    Ok(aok) => aok,
                    _ => continue,
                };

                if f01.header().entry_type().is_file() {
                    let f01path = match f01.path() {
                        Ok(path) => path.into_owned(),
                        Err(_) => continue,
                    };

                    let fname0 = match f01path.file_name().and_then(|name| name.to_str()) {
                        Some(name) => name.to_string(),
                        None => continue,
                    };

                    let f0path = match f01path.into_os_string().into_string() {
                        Ok(aok) => aok.strip_prefix("./").unwrap_or(&aok).to_string(),
                        _ => continue,
                    };

                    let mime = match mime_guess::from_path(&f0path).first_raw() {
                        Some(mime) if mime.starts_with("text/") => {
                            format!("{mime}; charset=utf-8")
                        }
                        Some(mime) => mime.to_string(),
                        None => continue,
                    };

                    let mut data0 = Vec::new();

                    if f01.read_to_end(&mut data0).is_err() {
                        continue;
                    }

                    fvec0.push(Resource {
                        path: f0path,
                        filename: fname0,
                        mime,
                        data: ResourceData::Parts(vec![data0]),
                    })
                }
            }
        } else {
            fvec0.push(Resource {
                path: f_name.clone(),
                filename: f_name,
                mime: f_type,
                data: ResourceData::BrowserFile(file),
            });
        }

        if fvec0.is_empty() {
            self.finish_progress(&progress_id, "failed", "no uploadable files", false)
                .await;
            return upload_result("upload result: no uploadable files", "");
        }

        let topic_safe = normalize_feed_topic(&feed_topic);

        self.update_progress(&progress_id, "push", None, "upload queued")
            .await;

        if self
            .upload_port
            .0
            .try_send((
                fvec0,
                encryption,
                redundancy_level,
                index_document,
                add_to_feed,
                topic_safe,
                Some(progress_out),
                chan_out,
            ))
            .is_err()
        {
            self.finish_progress(&progress_id, "failed", "upload queue unavailable", false)
                .await;
            return upload_result("upload result: upload queue unavailable", "");
        }

        let result = chan_in.recv().await.unwrap_or_default();

        if result.is_empty() {
            self.finish_progress(&progress_id, "failed", "upload failed", false)
                .await;
            return upload_result("upload result: failure", "");
        }

        let reference_hex = hex::encode(&result);
        self.finish_progress(
            &progress_id,
            "complete",
            format!("reference {}", reference_hex),
            true,
        )
        .await;

        upload_result(
            &format!(
                "upload result: returned address displayed here: {}",
                reference_hex
            ),
            &reference_hex,
        )
    }

    pub async fn post_push_chunk(
        &self,
        d: Vec<u8>,
        soc: bool,
        chunk_address: Vec<u8>,
        stamp: Vec<u8>,
    ) -> Vec<u8> {
        let (chan_out, chan_in) = mpsc::bounded::<bool>(1);
        let (slot_chan_out, _slot_chan_in) = mpsc::bounded::<bool>(1);

        let result_reference = hex::encode(&chunk_address);

        let _ = self.chunk_push_port.0.try_send((
            d,
            soc,
            chunk_address,
            stamp,
            chan_out,
            slot_chan_out,
            None,
        ));

        let result = chan_in.recv().await.unwrap_or(false);
        let (message, filename) = if result {
            ("Upload result: success", "Upload result")
        } else {
            ("Upload result: failure", "... result ...")
        };
        encode_resources(
            vec![(
                message.as_bytes().to_vec(),
                "text/plain".to_string(),
                filename.to_string(),
            )],
            result_reference,
        )
    }

    pub async fn acquire(&self, address: String) -> Vec<u8> {
        if let Some(resource) = parse_bzz_resource(&address) {
            if resource.path.is_empty() {
                return acquire_bzz_collection(resource.reference, &self.chunk_port.0).await;
            }
            if let Some(metadata) = self.resolve_bzz(address.clone()).await {
                if metadata.size == 0 {
                    return encode_resources(
                        vec![(vec![], metadata.mime, metadata.path.clone())],
                        metadata.path,
                    );
                }

                let end_inclusive = metadata.size - 1;
                if let Some((bytes, metadata)) = self
                    .acquire_resolved_range(metadata, 0, end_inclusive)
                    .await
                {
                    return encode_resources(
                        vec![(bytes, metadata.mime, metadata.path.clone())],
                        metadata.path,
                    );
                }
            }
        }

        let valaddr = match hex::decode(&address) {
            Ok(hex) => hex,
            _ => resolve_ens_reference(address, "").await,
        };

        acquire_bzz_collection(valaddr, &self.chunk_port.0).await
    }

    pub async fn retrieve_bytes(&self, address: String) -> Vec<u8> {
        let progress_id = self
            .start_progress("bytes", address.clone(), "retrieve", None, "starting")
            .await;
        let valaddr = match hex::decode(&address) {
            Ok(hex) => hex,
            Err(_) => {
                self.finish_progress(&progress_id, "failed", "invalid reference", false)
                    .await;
                return vec![];
            }
        };

        let bytes = retrieve_data(&valaddr, &self.chunk_port.0).await;
        let ok = !bytes.is_empty();
        self.finish_progress(
            &progress_id,
            if ok { "complete" } else { "failed" },
            format!("{} bytes", bytes.len()),
            ok,
        )
        .await;
        bytes
    }

    pub async fn retrieve_chunk_bytes(&self, address: String) -> Vec<u8> {
        let progress_id = self
            .start_progress("chunk", address.clone(), "retrieve", None, "starting")
            .await;
        let valaddr = match hex::decode(&address) {
            Ok(hex) => hex,
            Err(_) => {
                self.finish_progress(&progress_id, "failed", "invalid reference", false)
                    .await;
                return vec![];
            }
        };

        let (chan_out, chan_in) = mpsc::bounded::<Vec<u8>>(1);
        let _ = self
            .chunk_port
            .0
            .try_send(chunk_retrieve_request(valaddr, chan_out));

        let bytes = chan_in.recv().await.unwrap_or_default();
        let ok = !bytes.is_empty();
        self.finish_progress(
            &progress_id,
            if ok { "complete" } else { "failed" },
            format!("{} bytes", bytes.len()),
            ok,
        )
        .await;
        bytes
    }

    pub async fn reset_stamp(&self) -> Vec<u8> {
        let reset = secure_reset_stamp().await;
        let message = if reset {
            "Stamp reset and ready to be reused. Uploads after this point will overwrite uploads from before this point."
        } else {
            "Secure stamp reset failed. Open the weeb-3-secure vault and try again."
        };

        encode_resources(
            vec![(
                message.as_bytes().to_vec(),
                "text/plain".to_string(),
                "... result ...".to_string(),
            )],
            "... result ...".to_string(),
        )
    }

    pub fn new() -> Weeb3 {
        let secret_key = ecdsa::SecretKey::generate();
        let handshake_signer =
            PrivateKeySigner::from_slice(&secret_key.to_bytes()).expect("valid handshake key");
        let keypair: ecdsa::Keypair = secret_key.into();

        let swarm = libp2p::SwarmBuilder::with_existing_identity(keypair.into())
            .with_wasm_bindgen()
            .with_other_transport(|key| {
                websocket_websys::Transport::default()
                    .upgrade(core::upgrade::Version::V1Lazy)
                    .authenticate(noise::Config::new(key).unwrap())
                    .multiplex(yamux::Config::default())
                    .outbound_timeout(Duration::from_millis(OUTBOUND_CONNECTION_TIMEOUT_MS))
                    .boxed()
            })
            .expect("Failed to create WebSocket transport")
            .with_behaviour(|key| Behaviour::new(key.public()))
            .unwrap()
            .with_swarm_config(|_| {
                libp2p::swarm::Config::with_wasm_executor()
                    .with_idle_connection_timeout(Duration::from_secs(36000000))
                    .with_substream_upgrade_protocol_override(core::upgrade::Version::V1Lazy)
                    .with_max_negotiating_inbound_streams(10_000)
                    .with_per_connection_event_buffer_size(10_000)
                    .with_notify_handler_buffer_size(NonZero::new(10_000).unwrap())
            })
            .build();

        Weeb3 {
            handshake_signer: Arc::new(handshake_signer),
            swarm: Arc::new(SharedSwarm::new(swarm)),
            wings: Arc::new(Wings {
                connected_peers: Mutex::new(HashMap::new()),
                overlay_peers: Arc::new(Mutex::new(HashMap::new())),
                bootnodes: Mutex::new(HashSet::new()),
                accounting_peers: Arc::new(Mutex::new(HashMap::new())),
                ongoing_cheques: Mutex::new(HashMap::new()),
                connection_attempts: Mutex::new(HashMap::new()),
                connection_cooldowns: Mutex::new(HashSet::new()),
                physical_connections: Arc::new(std::sync::Mutex::new(HashMap::new())),
                handshake_ready_connections: std::sync::Mutex::new(HashSet::new()),
                canonical_identify_address: std::sync::Mutex::new(None),
                known_peers: Mutex::new(HashMap::new()),
                delayed_peer_retries: Arc::new(Mutex::new(HashMap::new())),
                rejected_duplicate_peers: Mutex::new(HashMap::new()),
            }),
            log_port: mpsc::bounded::<String>(LOG_QUEUE_CAPACITY),
            log_start_ms: Date::now(),
            chunk_port: chunk_retrieve_channel(),
            range_port: mpsc::bounded(RANGE_REQUEST_QUEUE_CAPACITY),
            upload_port: mpsc::unbounded(),
            chunk_push_port: mpsc::unbounded(),
            bootnode_port: mpsc::unbounded(),
            network_id: Mutex::new(1),
            service_worker_network_id: AtomicUsize::new(1),
            runtime_started: AtomicBool::new(false),
            allow_private_gossip: AtomicBool::new(false),
            transfer_paused: Arc::new(AtomicBool::new(false)),
            retrieve_cancel_registry: RetrieveCancelRegistry::default(),
            connection_generation: Arc::new(AtomicU64::new(0)),
            connection_population: Arc::new(Mutex::new(ConnectionPopulation::default())),
            progress: Arc::new(Mutex::new(ProgressStore::default())),
        }
    }

    pub fn get_current_logs(&self) -> Vec<String> {
        let mut logs = Vec::with_capacity(self.log_port.1.len().min(LOG_DRAIN_BATCH));

        for _ in 0..LOG_DRAIN_BATCH {
            match self.log_port.1.try_recv() {
                Ok(log_message) => logs.push(log_message),
                Err(_) => break,
            }
        }

        logs
    }

    pub async fn get_connections(&self) -> u64 {
        self.connection_population.lock().await.connected
    }

    pub(crate) async fn connection_counts(&self) -> (u64, u64) {
        let population = self.connection_population.lock().await;
        (population.connected, population.ongoing)
    }

    pub(crate) fn service_worker_network_id(&self) -> u64 {
        self.service_worker_network_id.load(Ordering::Acquire) as u64
    }

    pub(crate) fn runtime_is_started(&self) -> bool {
        self.runtime_started.load(Ordering::Acquire)
    }

    pub fn interface_log(&self, log0: String) {
        interface_log_to(&self.log_port.0, self.log_start_ms, log0);
    }

    pub(crate) async fn start_progress(
        &self,
        kind: impl Into<String>,
        subject: impl Into<String>,
        phase: impl Into<String>,
        percent: Option<u8>,
        detail: impl Into<String>,
    ) -> String {
        self.progress
            .lock()
            .await
            .start(kind, subject, phase, percent, detail)
    }

    pub(crate) async fn update_progress(
        &self,
        id: &str,
        phase: impl Into<String>,
        percent: Option<u8>,
        detail: impl Into<String>,
    ) {
        self.progress
            .lock()
            .await
            .update(id, phase, percent, detail);
    }

    pub(crate) async fn finish_progress(
        &self,
        id: &str,
        phase: impl Into<String>,
        detail: impl Into<String>,
        ok: bool,
    ) {
        self.progress.lock().await.finish(id, phase, detail, ok);
    }

    pub(crate) async fn get_progress_snapshot(
        &self,
        seen_revision: u64,
    ) -> Option<(u64, Vec<ProgressRow>)> {
        self.progress
            .lock()
            .await
            .snapshot_if_changed(seen_revision)
    }

    pub async fn toggle_transfer_pause(&self) -> bool {
        let paused = !self.transfer_paused.fetch_xor(true, Ordering::Relaxed);
        self.interface_log(if paused {
            "Paused retrieve / push scheduling".to_string()
        } else {
            "Resumed retrieve / push scheduling".to_string()
        });
        paused
    }

    pub fn transfer_paused(&self) -> bool {
        self.transfer_paused.load(Ordering::Relaxed)
    }

    pub async fn run(&self) {
        if self.runtime_started.swap(true, Ordering::AcqRel) {
            return;
        }

        self.interface_log("Node runtime handlers starting".to_string());
        let wings = self.wings.clone();
        let local_peer_id = { *self.swarm.lock().await.local_peer_id() };

        let (peers_instructions_chan_outgoing, peers_instructions_chan_incoming) =
            mpsc::bounded::<PeerDialInstruction>(MAX_QUEUED_PEER_DIALS);
        let (connections_instructions_chan_outgoing, connections_instructions_chan_incoming) =
            mpsc::unbounded::<ConnectionInstruction>();

        let (accounting_peer_chan_outgoing, accounting_peer_chan_incoming) =
            mpsc::unbounded::<PeerFile>();

        let (pricing_chan_outgoing, pricing_chan_incoming) =
            mpsc::unbounded::<(PeerId, u64, TransportConnectionSession)>();

        let (refreshment_instructions_chan_outgoing, refreshment_instructions_chan_incoming) =
            mpsc::unbounded::<RefreshmentInstruction>();

        let chunk_retrieve_chan_outgoing = self.chunk_port.0.clone();

        let chunk_upload_chan_outgoing = self.chunk_push_port.0.clone();

        let (cheque_instructions_chan_outgoing, cheque_instructions_chan_incoming) =
            mpsc::unbounded::<(PeerId, u64, u64)>();
        let (cheque_send_chan_outgoing, cheque_send_chan_incoming) =
            mpsc::unbounded::<(PeerId, bool, u64)>();

        let (
            mut pricing_control,
            mut gossip_control,
            handshake_control,
            refreshment_control,
            cheque_control,
            retrieval_control,
            upload_control,
        ) = {
            let mut swarm = self.swarm.lock().await;
            let stream = &mut swarm.behaviour_mut().stream;
            (
                stream.new_control(),
                stream.new_control(),
                stream.new_control(),
                stream.new_control(),
                stream.new_control(),
                stream.new_control(),
                stream.new_control(),
            )
        };

        let mut incoming_pricing_streams = pricing_control.accept(PRICING_PROTOCOL).unwrap();
        let mut incoming_gossip_streams = gossip_control.accept(GOSSIP_PROTOCOL).unwrap();
        self.interface_log("Node protocol listeners ready".to_string());

        let pricing_physical_connections = wings.physical_connections.clone();
        let pricing_inbound_handle = async move {
            while let Some((peer, stream)) = incoming_pricing_streams.next().await {
                let Some(connection_id) =
                    exclusive_physical_connection(&pricing_physical_connections, &peer)
                else {
                    continue;
                };
                let Some(pricing_session) = TransportConnectionSession::capture(
                    peer,
                    connection_id,
                    pricing_physical_connections.clone(),
                ) else {
                    continue;
                };
                let pricing_chan_outgoing = pricing_chan_outgoing.clone();
                spawn_local(async move {
                    pricing_handler(peer, stream, pricing_session, &pricing_chan_outgoing).await;
                });
                async_std::task::yield_now().await;
            }
        };

        let gossip_peers_instructions_chan_outgoing = peers_instructions_chan_outgoing.clone();
        let gossip_connection_generation = self.connection_generation.clone();
        let gossip_inbound_handle = async move {
            while let Some((_, stream)) = incoming_gossip_streams.next().await {
                let peers_instructions_chan_outgoing =
                    gossip_peers_instructions_chan_outgoing.clone();
                let instruction_generation = gossip_connection_generation.load(Ordering::Acquire);
                spawn_local(async move {
                    gossip_handler(
                        stream,
                        &peers_instructions_chan_outgoing,
                        instruction_generation,
                    )
                    .await;
                });
                async_std::task::yield_now().await;
            }
        };

        let peer_dial_scheduler = async {
            let mut new_peers = VecDeque::<QueuedPeerDial>::new();
            let mut retries = VecDeque::<QueuedPeerDial>::new();
            let mut queued_underlays = HashSet::<(PeerId, Multiaddr)>::new();
            let mut fresh_dials_since_retry = 0usize;
            let mut queue_generation = self.current_connection_generation();
            let mut last_population_rescan_ms = Date::now();

            loop {
                let (mut instruction, population_rescan_due) = if new_peers.is_empty()
                    && retries.is_empty()
                {
                    match async_std::future::timeout(
                        Duration::from_millis(PEER_POPULATION_RESCAN_MS),
                        peers_instructions_chan_incoming.recv(),
                    )
                    .await
                    {
                        Ok(Ok(instruction)) => (
                            Some(instruction),
                            Date::now() - last_population_rescan_ms
                                >= PEER_POPULATION_RESCAN_MS as f64,
                        ),
                        Ok(Err(_)) => break,
                        Err(_) => (None, true),
                    }
                } else {
                    (
                        peers_instructions_chan_incoming.try_recv().ok(),
                        Date::now() - last_population_rescan_ms >= PEER_POPULATION_RESCAN_MS as f64,
                    )
                };
                let current_generation = self.current_connection_generation();
                if current_generation != queue_generation {
                    new_peers.clear();
                    retries.clear();
                    queued_underlays.clear();
                    fresh_dials_since_retry = 0;
                    queue_generation = current_generation;
                }
                let mut candidates = Vec::new();
                let public_gossip_only = self.service_worker_network_id() != 0
                    && !self.allow_private_gossip.load(Ordering::Acquire);
                for _ in 0..PEER_DIAL_INGEST_BATCH {
                    let Some(next_instruction) = instruction.take() else {
                        break;
                    };
                    if next_instruction.generation != queue_generation {
                        let current_generation = self.current_connection_generation();
                        if current_generation != queue_generation {
                            new_peers.clear();
                            retries.clear();
                            queued_underlays.clear();
                            candidates.clear();
                            fresh_dials_since_retry = 0;
                            queue_generation = current_generation;
                        }
                    }
                    if next_instruction.generation != queue_generation {
                        instruction = peers_instructions_chan_incoming.try_recv().ok();
                        continue;
                    }
                    candidates.extend(
                        peer_dial_candidates(next_instruction, public_gossip_only)
                            .filter(|candidate| candidate.peer != local_peer_id),
                    );
                    instruction = peers_instructions_chan_incoming.try_recv().ok();
                }
                if population_rescan_due {
                    last_population_rescan_ms = Date::now();
                    let deficit =
                        current_connection_population_deficit(&self.connection_population).await;
                    if deficit > 0 {
                        let connected = wings
                            .connected_peers
                            .lock()
                            .await
                            .keys()
                            .copied()
                            .collect::<HashSet<_>>();
                        let attempts = wings
                            .connection_attempts
                            .lock()
                            .await
                            .keys()
                            .copied()
                            .collect::<HashSet<_>>();
                        let cooldowns = wings.connection_cooldowns.lock().await.clone();
                        let bootnodes = wings.bootnodes.lock().await.clone();
                        let delayed = wings.delayed_peer_retries.lock().await.clone();
                        let rejected = wings.rejected_duplicate_peers.lock().await.clone();
                        let known = wings.known_peers.lock().await;
                        let mut eligible = known
                            .iter()
                            .filter(|(peer, known)| {
                                known.generation == queue_generation
                                    && !connected.contains(*peer)
                                    && !attempts.contains(*peer)
                                    && !cooldowns.contains(*peer)
                                    && !delayed.get(*peer).is_some_and(|(generation, _)| {
                                        *generation == queue_generation
                                    })
                                    && !rejected.contains_key(*peer)
                            })
                            .map(|(peer, known)| QueuedPeerDial {
                                peer: *peer,
                                dial_addr: known.underlay.clone(),
                                generation: queue_generation,
                                retry: true,
                                bootnode: bootnodes.contains(peer),
                            })
                            .collect::<Vec<_>>();
                        eligible.shuffle(&mut rand::thread_rng());
                        eligible.truncate(deficit);
                        candidates.extend(eligible);
                    }
                }
                {
                    let rejected = wings.rejected_duplicate_peers.lock().await.clone();
                    let mut known_peers = wings.known_peers.lock().await;
                    for candidate in candidates {
                        if rejected.contains_key(&candidate.peer) {
                            continue;
                        }
                        let candidate_key = (candidate.peer, candidate.dial_addr.clone());
                        if queued_underlays.contains(&candidate_key) {
                            continue;
                        }
                        let exact_known_address = known_peers
                            .get(&candidate.peer)
                            .is_some_and(|known| known.underlay == candidate.dial_addr);
                        if !candidate.retry && exact_known_address {
                            continue;
                        }
                        if queued_underlays.len() >= MAX_QUEUED_PEER_DIALS {
                            let Some(displaced) = retries.pop_back() else {
                                if candidate.retry
                                    && known_peers
                                        .get(&candidate.peer)
                                        .is_some_and(|known| known.underlay == candidate.dial_addr)
                                {
                                    known_peers.remove(&candidate.peer);
                                }
                                continue;
                            };
                            queued_underlays.remove(&(displaced.peer, displaced.dial_addr.clone()));
                            if known_peers
                                .get(&displaced.peer)
                                .is_some_and(|known| known.underlay == displaced.dial_addr)
                            {
                                known_peers.remove(&displaced.peer);
                            }
                        }
                        queued_underlays.insert(candidate_key);
                        if candidate.retry {
                            retries.push_back(candidate);
                        } else {
                            new_peers.push_back(candidate);
                        }
                    }
                }
                let current_generation = self.current_connection_generation();
                if current_generation != queue_generation {
                    new_peers.clear();
                    retries.clear();
                    queued_underlays.clear();
                    fresh_dials_since_retry = 0;
                    queue_generation = current_generation;
                    continue;
                }

                if new_peers.is_empty() && retries.is_empty() {
                    async_std::task::yield_now().await;
                    continue;
                }
                if !try_reserve_connection_capacity(&self.connection_population).await {
                    async_std::task::sleep(Duration::from_millis(CONNECTION_CAPACITY_WAIT_MS))
                        .await;
                    continue;
                }
                let connected_peers = {
                    let connected = wings.connected_peers.lock().await;
                    connected.keys().copied().collect::<HashSet<_>>()
                };
                let rejected_peers = wings.rejected_duplicate_peers.lock().await.clone();
                let mut unavailable_peers = HashSet::new();
                unavailable_peers.extend(wings.connection_cooldowns.lock().await.iter().copied());
                unavailable_peers.extend(wings.connection_attempts.lock().await.keys().copied());
                unavailable_peers.extend(
                    wings
                        .delayed_peer_retries
                        .lock()
                        .await
                        .iter()
                        .filter_map(|(peer, retry)| (retry.0 == queue_generation).then_some(*peer)),
                );

                let mut take_eligible = |queue: &mut VecDeque<QueuedPeerDial>| {
                    for _ in 0..queue.len() {
                        let candidate = queue.pop_front().unwrap();
                        if candidate.generation != queue_generation
                            || connected_peers.contains(&candidate.peer)
                            || rejected_peers.contains_key(&candidate.peer)
                        {
                            queued_underlays.remove(&(candidate.peer, candidate.dial_addr.clone()));
                        } else if unavailable_peers.contains(&candidate.peer) {
                            queue.push_back(candidate);
                        } else {
                            return Some(candidate);
                        }
                    }
                    None
                };
                let next_candidate = if fresh_dials_since_retry >= FRESH_PEER_DIALS_PER_RETRY {
                    take_eligible(&mut retries).or_else(|| take_eligible(&mut new_peers))
                } else {
                    take_eligible(&mut new_peers).or_else(|| take_eligible(&mut retries))
                };
                let Some(candidate) = next_candidate else {
                    release_connection_reservation(&self.connection_population).await;
                    async_std::task::sleep(Duration::from_millis(CONNECTION_CAPACITY_WAIT_MS))
                        .await;
                    continue;
                };
                queued_underlays.remove(&(candidate.peer, candidate.dial_addr.clone()));

                if self.connection_generation.load(Ordering::Acquire) != candidate.generation {
                    release_connection_reservation(&self.connection_population).await;
                    continue;
                }

                let Some((attempt_id, ready_connection)) =
                    try_mark_connection_attempt(&wings, &candidate.peer).await
                else {
                    release_connection_reservation(&self.connection_population).await;
                    let generation_current =
                        self.connection_generation.load(Ordering::Acquire) == candidate.generation;
                    let connected = wings
                        .connected_peers
                        .lock()
                        .await
                        .contains_key(&candidate.peer);
                    let candidate_key = (candidate.peer, candidate.dial_addr.clone());
                    if generation_current && !connected && queued_underlays.insert(candidate_key) {
                        if candidate.retry {
                            retries.push_back(candidate);
                        } else {
                            new_peers.push_back(candidate);
                        }
                    }
                    continue;
                };
                if self.connection_generation.load(Ordering::Acquire) != candidate.generation {
                    if remove_connection_attempt(&wings, &candidate.peer, attempt_id).await {
                        release_connection_reservation(&self.connection_population).await;
                    }
                    continue;
                }
                if candidate.retry {
                    fresh_dials_since_retry = 0;
                } else {
                    fresh_dials_since_retry = fresh_dials_since_retry.saturating_add(1);
                }

                wings.known_peers.lock().await.insert(
                    candidate.peer,
                    KnownPeer {
                        underlay: candidate.dial_addr.clone(),
                        generation: candidate.generation,
                    },
                );
                if candidate.bootnode {
                    wings.bootnodes.lock().await.insert(candidate.peer);
                }
                match start_owned_connection_attempt(
                    &self.swarm,
                    &wings,
                    &candidate.peer,
                    &candidate.dial_addr,
                    attempt_id,
                )
                .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        if remove_connection_attempt(&wings, &candidate.peer, attempt_id).await {
                            release_connection_reservation(&self.connection_population).await;
                        }
                        continue;
                    }
                    Err(_) => {
                        if remove_connection_attempt(&wings, &candidate.peer, attempt_id).await {
                            queue_peer_dial_retry(
                                candidate.dial_addr.clone(),
                                candidate.generation,
                                self.connection_generation.clone(),
                                peers_instructions_chan_outgoing.clone(),
                                candidate.bootnode,
                                wings.delayed_peer_retries.clone(),
                            )
                            .await;
                            release_connection_reservation(&self.connection_population).await;
                        }
                        continue;
                    }
                }

                if connections_instructions_chan_outgoing
                    .try_send((
                        candidate.dial_addr,
                        candidate.bootnode,
                        candidate.generation,
                        attempt_id,
                        ready_connection,
                    ))
                    .is_err()
                    && remove_connection_attempt(&wings, &candidate.peer, attempt_id).await
                {
                    release_connection_reservation(&self.connection_population).await;
                }
                async_std::task::yield_now().await;
            }
        };

        let swarm_event_loop = async {
            let mut events_since_browser_yield = 0usize;
            loop {
                let Some(event) = self.swarm.next_event().await else {
                    break;
                };

                match &event {
                    SwarmEvent::ConnectionEstablished {
                        peer_id,
                        connection_id,
                        ..
                    } => {
                        record_physical_connection_established(
                            &wings.physical_connections,
                            peer_id,
                            *connection_id,
                        );
                    }
                    SwarmEvent::ConnectionClosed {
                        peer_id,
                        connection_id,
                        ..
                    } => {
                        record_physical_connection_closed(
                            &wings.physical_connections,
                            peer_id,
                            *connection_id,
                        );
                        wings
                            .handshake_ready_connections
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .remove(&(*peer_id, *connection_id));
                    }
                    _ => {}
                }

                if !matches!(
                    &event,
                    SwarmEvent::ConnectionEstablished { .. }
                        | SwarmEvent::Behaviour(BehaviourEvent::Identify(_))
                        | SwarmEvent::OutgoingConnectionError { .. }
                        | SwarmEvent::ConnectionClosed { .. }
                ) {
                    events_since_browser_yield += 1;
                    if events_since_browser_yield >= SWARM_EVENTS_PER_BROWSER_YIELD {
                        events_since_browser_yield = 0;
                        async_std::task::sleep(Duration::ZERO).await;
                    } else {
                        async_std::task::yield_now().await;
                    }
                    continue;
                }

                let wings = wings.clone();
                let swarm = self.swarm.clone();
                let peers_instructions_chan_outgoing = peers_instructions_chan_outgoing.clone();
                let connection_generation = self.connection_generation.clone();
                let connection_population = self.connection_population.clone();
                let log_port = self.log_port.0.clone();
                let log_start_ms = self.log_start_ms;
                let identify_local_peer_id = local_peer_id;

                spawn_local(async move {
                    let interface_log = |log0: String| {
                        interface_log_to(&log_port, log_start_ms, log0);
                    };

                    match event {
                        SwarmEvent::ConnectionEstablished {
                            peer_id,
                            connection_id,
                            ..
                        } => {
                            let expected_peer_connection = {
                                let connected = wings.connected_peers.lock().await;
                                connected.get(&peer_id).map(|peer| peer.connection_id)
                            };
                            let expected_attempt_connection = {
                                let attempts = wings.connection_attempts.lock().await;
                                attempts
                                    .get(&peer_id)
                                    .and_then(|attempt| attempt.physical_connection_id)
                            };
                            let cooling_down =
                                wings.connection_cooldowns.lock().await.contains(&peer_id);
                            let lifecycle_known = expected_peer_connection.is_some()
                                || expected_attempt_connection.is_some()
                                || cooling_down;
                            let connection_owned = expected_peer_connection == Some(connection_id)
                                || expected_attempt_connection == Some(connection_id);

                            if lifecycle_known && !connection_owned {
                                let closed = {
                                    let mut swarm = swarm.lock().await;
                                    swarm.close_connection(connection_id)
                                };
                                interface_log(format!(
                                    "Closed unowned physical connection peer={} connection_id={:?} expected_peer={:?} expected_attempt={:?} cooldown={} closed={}",
                                    peer_id,
                                    connection_id,
                                    expected_peer_connection,
                                    expected_attempt_connection,
                                    cooling_down,
                                    closed
                                ));
                            }
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Identify(identify_event)) => {
                            match identify_event {
                                identify::Event::Received {
                                    peer_id,
                                    connection_id,
                                    info,
                                } => {
                                    let physical_session_current = wings
                                        .physical_connections
                                        .lock()
                                        .unwrap_or_else(|error| error.into_inner())
                                        .get(&peer_id)
                                        .is_some_and(|connections| {
                                            connections.contains(&connection_id)
                                        });
                                    if !physical_session_current {
                                        return;
                                    }
                                    if wings
                                        .handshake_ready_connections
                                        .lock()
                                        .unwrap_or_else(|error| error.into_inner())
                                        .contains(&(peer_id, connection_id))
                                    {
                                        return;
                                    }
                                    if info.observed_addr.is_empty()
                                        || try_from_multiaddr(&info.observed_addr)
                                            .is_some_and(|peer| peer != identify_local_peer_id)
                                    {
                                        let _ = close_failed_identify_connection(
                                            &wings,
                                            &swarm,
                                            &peer_id,
                                            connection_id,
                                        )
                                        .await;
                                        return;
                                    }
                                    let observed_addr = info.observed_addr;
                                    let mut swarm = swarm.lock().await;
                                    let canonical = {
                                        let mut canonical = wings
                                            .canonical_identify_address
                                            .lock()
                                            .unwrap_or_else(|error| error.into_inner());
                                        if canonical.is_none() {
                                            *canonical = Some(observed_addr.clone());
                                            Some(observed_addr)
                                        } else {
                                            None
                                        }
                                    };
                                    if let Some(canonical) = canonical {
                                        swarm.add_external_address(canonical);
                                    }
                                    swarm
                                        .behaviour_mut()
                                        .identify
                                        .push(std::iter::once(peer_id));
                                    drop(swarm);
                                    mark_handshake_ready_connection(&wings, peer_id, connection_id)
                                        .await;
                                }
                                identify::Event::Error {
                                    peer_id,
                                    connection_id,
                                    ..
                                } => {
                                    let _ = close_failed_identify_connection(
                                        &wings,
                                        &swarm,
                                        &peer_id,
                                        connection_id,
                                    )
                                    .await;
                                }
                                _ => {}
                            }
                        }
                        SwarmEvent::OutgoingConnectionError {
                            peer_id,
                            connection_id,
                            error,
                        } => {
                            let retryable = !matches!(
                                &error,
                                libp2p::swarm::DialError::LocalPeerId { .. }
                                    | libp2p::swarm::DialError::WrongPeerId { .. }
                            );
                            let mut retry_address = match &error {
                                libp2p::swarm::DialError::LocalPeerId { address } => {
                                    Some(address.clone())
                                }
                                libp2p::swarm::DialError::WrongPeerId { address, .. } => {
                                    Some(address.clone())
                                }
                                libp2p::swarm::DialError::Transport(errors) => {
                                    errors.first().map(|(address, _)| address.clone())
                                }
                                _ => None,
                            };

                            if retry_address.is_none()
                                && let Some(peer_id) = &peer_id
                            {
                                retry_address = wings
                                    .known_peers
                                    .lock()
                                    .await
                                    .get(peer_id)
                                    .map(|known| known.underlay.clone());
                            }

                            let peer_to_clear = peer_id
                                .or_else(|| retry_address.as_ref().and_then(try_from_multiaddr));

                            let retry_generation = connection_generation.load(Ordering::Acquire);
                            let Some(peer_id) = peer_to_clear else {
                                return;
                            };
                            if !remove_connection_attempt_for_connection(
                                &wings,
                                &peer_id,
                                connection_id,
                            )
                            .await
                            {
                                return;
                            }

                            let peer_generation = wings
                                .known_peers
                                .lock()
                                .await
                                .get(&peer_id)
                                .map(|known| known.generation);
                            if retryable
                                && peer_generation == Some(retry_generation)
                                && let Some(address) = retry_address
                            {
                                let bootnode = wings.bootnodes.lock().await.contains(&peer_id);
                                queue_peer_dial_retry(
                                    address,
                                    retry_generation,
                                    connection_generation,
                                    peers_instructions_chan_outgoing,
                                    bootnode,
                                    wings.delayed_peer_retries.clone(),
                                )
                                .await;
                            } else if !retryable && peer_generation == Some(retry_generation) {
                                wings.known_peers.lock().await.remove(&peer_id);
                            }
                            release_connection_reservation(&connection_population).await;
                        }
                        SwarmEvent::ConnectionClosed {
                            peer_id,
                            connection_id,
                            endpoint,
                            num_established: _,
                            cause,
                        } => {
                            let mut connected_peers = wings.connected_peers.lock().await;
                            let expected_peer_connection = connected_peers
                                .get(&peer_id)
                                .map(|peer_file| peer_file.connection_id);
                            let expected_attempt_connection = if expected_peer_connection.is_none()
                            {
                                let attempts = wings.connection_attempts.lock().await;
                                attempts
                                    .get(&peer_id)
                                    .and_then(|attempt| attempt.physical_connection_id)
                            } else {
                                None
                            };
                            let close_owns_lifecycle = expected_peer_connection
                                .map(|expected| expected == connection_id)
                                .unwrap_or_else(|| {
                                    expected_attempt_connection == Some(connection_id)
                                });
                            if !close_owns_lifecycle {
                                drop(connected_peers);
                                if let Some(remaining_connection_id) = exclusive_physical_connection(
                                    &wings.physical_connections,
                                    &peer_id,
                                ) && wings
                                    .handshake_ready_connections
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .contains(&(peer_id, remaining_connection_id))
                                {
                                    mark_handshake_ready_connection(
                                        &wings,
                                        peer_id,
                                        remaining_connection_id,
                                    )
                                    .await;
                                }
                                return;
                            }

                            let removed_peer_file = connected_peers.remove(&peer_id);
                            wings
                                .rejected_duplicate_peers
                                .lock()
                                .await
                                .retain(|_, owner| owner != &peer_id);
                            let was_tracked_peer = removed_peer_file.is_some();
                            let mut removed_owned_overlay = false;
                            let mut tracked_bootnode = false;

                            if let Some(peer_file) = removed_peer_file.as_ref() {
                                let overlay = &peer_file.overlay;
                                let was_bootnode = {
                                    let bootnodes_set = wings.bootnodes.lock().await;
                                    bootnodes_set.contains(&peer_id)
                                };
                                tracked_bootnode = was_bootnode;
                                removed_owned_overlay = {
                                    let mut overlay_peers_map = wings.overlay_peers.lock().await;
                                    if overlay_peers_map.get(overlay) == Some(&peer_id) {
                                        overlay_peers_map.remove(overlay);
                                        true
                                    } else {
                                        false
                                    }
                                };
                            }

                            let had_attempt = if let Some(peer_file) = removed_peer_file.as_ref() {
                                remove_connection_attempt(
                                    &wings,
                                    &peer_id,
                                    peer_file.connection_attempt_id,
                                )
                                .await
                            } else {
                                remove_connection_attempt_for_connection(
                                    &wings,
                                    &peer_id,
                                    connection_id,
                                )
                                .await
                            };
                            if !was_tracked_peer && !had_attempt {
                                drop(connected_peers);
                                return;
                            }

                            let accounting_peer = {
                                let mut accounting = wings.accounting_peers.lock().await;
                                accounting.remove(&peer_id)
                            };
                            let (balance, reserve, announced_threshold) =
                                if let Some(accounting_peer) = accounting_peer {
                                    let accounting_peer = accounting_peer.lock().await;
                                    (
                                        accounting_peer.balance,
                                        accounting_peer.reserve,
                                        accounting_peer.threshold,
                                    )
                                } else {
                                    (0, 0, 0)
                                };
                            let reconnect_delay_seconds = bee_reconnect_delay_seconds(
                                balance,
                                reserve,
                                announced_threshold.max(REFRESH_RATE.saturating_mul(3)),
                                REFRESH_RATE,
                            );
                            let known_peer = wings.known_peers.lock().await.remove(&peer_id);
                            let retry_address = match &endpoint {
                                libp2p::core::ConnectedPoint::Dialer { address, .. } => {
                                    Some(address.clone())
                                }
                                _ => known_peer.as_ref().map(|known| known.underlay.clone()),
                            };
                            let reconnect_delay_ms = if was_tracked_peer {
                                reconnect_delay_seconds.saturating_mul(1000)
                            } else {
                                retry_address
                                    .as_ref()
                                    .map(failed_peer_retry_delay_ms)
                                    .unwrap_or(PEER_RETRY_DELAY_MS)
                            };

                            let _ = wings.ongoing_cheques.lock().await.remove(&peer_id);
                            let retry_generation = connection_generation.load(Ordering::Acquire);
                            let peer_generation = known_peer.map(|known| known.generation);
                            let retry_is_current = peer_generation == Some(retry_generation);
                            if retry_is_current {
                                wings.connection_cooldowns.lock().await.insert(peer_id);
                            }
                            let counter_release = if had_attempt {
                                release_connection_reservation(&connection_population).await;
                                "Ongoing"
                            } else if removed_owned_overlay || tracked_bootnode {
                                release_connected_peer(&connection_population).await;
                                "Connected"
                            } else {
                                "None"
                            };
                            drop(connected_peers);

                            let (connected_count, ongoing_count) = {
                                let population = connection_population.lock().await;
                                (population.connected, population.ongoing)
                            };
                            interface_log(format!(
                                "Disconnected from peer {} endpoint={:?} reason={:?} release={} connected={} ongoing={}",
                                peer_id,
                                endpoint,
                                cause,
                                counter_release,
                                connected_count,
                                ongoing_count
                            ));

                            let retry_bootnode = wings.bootnodes.lock().await.contains(&peer_id);

                            if retry_is_current {
                                async_std::task::sleep(Duration::from_millis(reconnect_delay_ms))
                                    .await;
                                if connection_generation.load(Ordering::Acquire) == retry_generation
                                {
                                    wings.connection_cooldowns.lock().await.remove(&peer_id);
                                    if let Some(address) = retry_address
                                        && peers_instructions_chan_outgoing
                                            .send(PeerDialInstruction {
                                                underlay: address.to_vec(),
                                                generation: retry_generation,
                                                retry: true,
                                                bootnode: retry_bootnode,
                                            })
                                            .await
                                            .is_ok()
                                    {
                                        interface_log(format!(
                                            "Queued reconnect for peer {} {} after {}ms backoff",
                                            peer_id, address, reconnect_delay_ms
                                        ));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                });

                events_since_browser_yield += 1;
                if events_since_browser_yield >= SWARM_EVENTS_PER_BROWSER_YIELD {
                    events_since_browser_yield = 0;
                    async_std::task::sleep(Duration::ZERO).await;
                } else {
                    async_std::task::yield_now().await;
                }
            }
        };

        let bootnode_change_handle = async {
            loop {
                let first_change = match self.bootnode_port.1.recv().await {
                    Ok(bootnode_change) => bootnode_change,
                    Err(_) => break,
                };
                let mut bootnode_changes = vec![first_change];
                while let Ok(change) = self.bootnode_port.1.try_recv() {
                    bootnode_changes.push(change);
                }

                let swarm = self.swarm.clone();
                let wings = wings.clone();
                let connections_instructions_chan_outgoing =
                    connections_instructions_chan_outgoing.clone();
                let peers_instructions_chan_outgoing = peers_instructions_chan_outgoing.clone();
                let connection_generation = self.connection_generation.clone();
                let connection_population = self.connection_population.clone();

                spawn_local(async move {
                    for (baddr, usable, request_generation) in bootnode_changes {
                        if connection_generation.load(Ordering::Acquire) != request_generation {
                            continue;
                        }

                        let addr33 = match baddr.parse::<Multiaddr>() {
                            Ok(aok) => aok,
                            _ => {
                                continue;
                            }
                        };

                        let pid: PeerId = match try_from_multiaddr(&addr33) {
                            Some(aok) => aok,
                            _ => {
                                continue;
                            }
                        };

                        let dial_addr =
                            browser_dial_address(addr33).unwrap_or_else(std::convert::identity);
                        if !reserve_connection_capacity(
                            &connection_population,
                            &connection_generation,
                            request_generation,
                        )
                        .await
                        {
                            continue;
                        }

                        let Some((attempt_id, ready_connection)) =
                            try_mark_connection_attempt(&wings, &pid).await
                        else {
                            release_connection_reservation(&connection_population).await;
                            continue;
                        };

                        if connection_generation.load(Ordering::Acquire) != request_generation {
                            if remove_connection_attempt(&wings, &pid, attempt_id).await {
                                release_connection_reservation(&connection_population).await;
                            }
                            continue;
                        }
                        wings.known_peers.lock().await.insert(
                            pid,
                            KnownPeer {
                                underlay: dial_addr.clone(),
                                generation: request_generation,
                            },
                        );
                        if !usable {
                            wings.bootnodes.lock().await.insert(pid);
                        }
                        match start_owned_connection_attempt(
                            &swarm, &wings, &pid, &dial_addr, attempt_id,
                        )
                        .await
                        {
                            Ok(true) => {}
                            Ok(false) => {
                                if remove_connection_attempt(&wings, &pid, attempt_id).await {
                                    release_connection_reservation(&connection_population).await;
                                }
                                continue;
                            }
                            Err(_) => {
                                let released =
                                    remove_connection_attempt(&wings, &pid, attempt_id).await;
                                wings.known_peers.lock().await.remove(&pid);
                                if released {
                                    queue_peer_dial_retry(
                                        dial_addr,
                                        request_generation,
                                        connection_generation.clone(),
                                        peers_instructions_chan_outgoing.clone(),
                                        !usable,
                                        wings.delayed_peer_retries.clone(),
                                    )
                                    .await;
                                    release_connection_reservation(&connection_population).await;
                                }
                                continue;
                            }
                        }

                        if connections_instructions_chan_outgoing
                            .try_send((
                                dial_addr,
                                !usable,
                                request_generation,
                                attempt_id,
                                ready_connection,
                            ))
                            .is_err()
                            && remove_connection_attempt(&wings, &pid, attempt_id).await
                        {
                            release_connection_reservation(&connection_population).await;
                        }
                    }
                });

                async_std::task::yield_now().await;
            }
        };

        let accounting_event_handle = async {
            loop {
                let mut peer_file = match accounting_peer_chan_incoming.recv().await {
                    Ok(peer_file) => peer_file,
                    Err(_) => break,
                };

                loop {
                    let peer = peer_file.peer_id;
                    let connection_attempt_id = peer_file.connection_attempt_id;
                    let mut connected_peers = wings.connected_peers.lock().await;
                    let physical_session_current =
                        exclusive_physical_connection(&wings.physical_connections, &peer)
                            == Some(peer_file.connection_id);
                    let owns_attempt = {
                        let attempts = wings.connection_attempts.lock().await;
                        attempts.get(&peer).is_some_and(|attempt| {
                            attempt.id == connection_attempt_id
                                && attempt.physical_connection_id == Some(peer_file.connection_id)
                        })
                    };
                    if !physical_session_current || !owns_attempt {
                        if owns_attempt {
                            let mut swarm = self.swarm.lock().await;
                            let _ = swarm.disconnect_peer_id(peer);
                        }
                        break;
                    }

                    let newly_connected = match connected_peers.get(&peer) {
                        Some(existing)
                            if existing.connection_attempt_id != connection_attempt_id =>
                        {
                            break;
                        }
                        Some(_) => false,
                        None => true,
                    };

                    let accounting_peer_lock = {
                        // Pricing can arrive before handshake publication; both paths share this entry.
                        get_or_create_accounting_peer(&wings, peer).await
                    };
                    let threshold_ready = {
                        let mut accounting_peer = accounting_peer_lock.lock().await;
                        accounting_peer.connection_id = Some(peer_file.connection_id);
                        accounting_peer.threshold > 0
                    };

                    connected_peers.insert(peer, peer_file);
                    drop(connected_peers);

                    if threshold_ready {
                        self.promote_priced_peer(&wings, peer).await;
                    } else if newly_connected {
                        let wings_for_timeout = wings.clone();
                        let accounting_peer_for_timeout = accounting_peer_lock.clone();
                        let connection_generation = self.connection_generation.clone();
                        let swarm = self.swarm.clone();

                        spawn_local(async move {
                            let retry_generation = connection_generation.load(Ordering::Acquire);
                            async_std::task::sleep(Duration::from_millis(
                                PRICING_CONNECT_TIMEOUT_MS,
                            ))
                            .await;

                            if connection_generation.load(Ordering::Acquire) != retry_generation {
                                return;
                            }

                            let connected_peers = wings_for_timeout.connected_peers.lock().await;
                            let owns_peer_file =
                                connected_peers.get(&peer).is_some_and(|peer_file| {
                                    peer_file.connection_attempt_id == connection_attempt_id
                                });
                            if !owns_peer_file {
                                return;
                            }

                            let current_accounting_peer = {
                                let accounting = wings_for_timeout.accounting_peers.lock().await;
                                accounting.get(&peer).cloned()
                            };

                            let Some(current_accounting_peer) = current_accounting_peer else {
                                return;
                            };

                            if !Arc::ptr_eq(&accounting_peer_for_timeout, &current_accounting_peer)
                            {
                                return;
                            }

                            if current_accounting_peer.lock().await.threshold > 0 {
                                return;
                            }

                            if !connection_attempt_is_current(
                                &wings_for_timeout,
                                &peer,
                                connection_attempt_id,
                            )
                            .await
                            {
                                return;
                            }

                            {
                                let mut swarm = swarm.lock().await;
                                let _ = swarm.disconnect_peer_id(peer);
                            }
                        });
                    }

                    match accounting_peer_chan_incoming.try_recv() {
                        Ok(next) => peer_file = next,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let pricing_event_handle = async {
            loop {
                let mut pricing = match pricing_chan_incoming.recv().await {
                    Ok(pricing) => pricing,
                    Err(_) => break,
                };

                loop {
                    let (peer, amount, pricing_session) = pricing;
                    let connected_peers = wings.connected_peers.lock().await;
                    let expected_connection = if let Some(peer_file) = connected_peers.get(&peer) {
                        Some(peer_file.connection_id)
                    } else {
                        let attempts = wings.connection_attempts.lock().await;
                        attempts
                            .get(&peer)
                            .and_then(|attempt| attempt.physical_connection_id)
                    };
                    let physical_session_current = pricing_session.is_current()
                        && expected_connection == Some(pricing_session.connection_id());
                    if physical_session_current {
                        let accounting_peer_lock =
                            get_or_create_accounting_peer(&wings, peer).await;
                        set_payment_threshold(&accounting_peer_lock, amount).await;
                    }
                    drop(connected_peers);
                    if physical_session_current {
                        self.promote_priced_peer(&wings, peer).await;
                    }

                    match pricing_chan_incoming.try_recv() {
                        Ok(next) => pricing = next,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let cheques_active_cache = Arc::new(AtomicBool::new(false));
        let cheque_generations = Arc::new(AtomicU64::new(0));
        {
            let cheques_active_cache = cheques_active_cache.clone();
            spawn_local(async move {
                let active = secure_vault::worker_cheques_active().await;
                cheques_active_cache.store(active, Ordering::Relaxed);
            });
        }

        let refreshment_swarm = self.swarm.clone();
        let refreshment_log_port = self.log_port.0.clone();
        let refreshment_log_start_ms = self.log_start_ms;
        let refreshment_instruction_handle = async {
            let mut refresh_dispatches = 0usize;
            loop {
                let (peer, accounting_peer, connection_id) =
                    match refreshment_instructions_chan_incoming.recv().await {
                        Ok(instruction) => instruction,
                        Err(_) => break,
                    };

                let refreshment_wings = wings.clone();
                let refreshment_control = refreshment_control.clone();
                let cheque_chan = cheque_instructions_chan_outgoing.clone();
                let cheques_active_cache = cheques_active_cache.clone();
                let cheque_generations = cheque_generations.clone();
                let refreshment_swarm = refreshment_swarm.clone();
                let refreshment_log_port = refreshment_log_port.clone();

                spawn_local(async move {
                    let interface_log = |message: String| {
                        interface_log_to(&refreshment_log_port, refreshment_log_start_ms, message);
                    };
                    loop {
                        let (balance, last_refreshment, payment_threshold) = {
                            let mut account = accounting_peer.lock().await;
                            if !refreshment_due(
                                account.balance,
                                account.refreshment,
                                account.threshold,
                            ) {
                                account.refresh_scheduled = false;
                                return;
                            }
                            (account.balance, account.refreshment, account.threshold)
                        };

                        if current_accounting_protocol_session(
                            &refreshment_wings,
                            &peer,
                            &accounting_peer,
                            connection_id,
                        )
                        .await
                        .is_none()
                        {
                            accounting_peer.lock().await.refresh_scheduled = false;
                            return;
                        }

                        let pending_cheque_amount = {
                            let cheques = refreshment_wings.ongoing_cheques.lock().await;
                            cheques.get(&peer).map_or(0, |(amount, _)| *amount)
                        };
                        if !refreshment_due(
                            balance.saturating_sub(pending_cheque_amount),
                            last_refreshment,
                            payment_threshold,
                        ) {
                            async_std::task::sleep(Duration::from_secs(1)).await;
                            continue;
                        }

                        let elapsed = Date::now() - last_refreshment;
                        let delay_ms = if !elapsed.is_finite() || elapsed < 0.0 {
                            1000
                        } else if elapsed < 1000.0 {
                            (1000.0 - elapsed).ceil() as u64
                        } else {
                            0
                        };
                        if delay_ms > 0 {
                            async_std::task::sleep(Duration::from_millis(delay_ms)).await;
                        }

                        let Some(protocol_session) = current_accounting_protocol_session(
                            &refreshment_wings,
                            &peer,
                            &accounting_peer,
                            connection_id,
                        )
                        .await
                        else {
                            accounting_peer.lock().await.refresh_scheduled = false;
                            return;
                        };

                        // Dispatched settlement always runs to completion.
                        let attempted_amount = {
                            let mut account = accounting_peer.lock().await;
                            if !refreshment_due(
                                account.balance,
                                account.refreshment,
                                account.threshold,
                            ) {
                                account.refresh_scheduled = false;
                                return;
                            }
                            // Include in-flight completions in the advertised allowance.
                            account.threshold
                        };
                        if attempted_amount == 0 {
                            accounting_peer.lock().await.refresh_scheduled = false;
                            return;
                        }
                        let outcome = refresh_handler(
                            peer,
                            attempted_amount,
                            refreshment_control.clone(),
                            protocol_session,
                        )
                        .await;

                        let amount = match outcome {
                            RefreshmentOutcome::NotDispatched => {
                                interface_log(format!("Refreshment attempt cleared {}", 0));
                                async_std::task::sleep(Duration::from_secs(1)).await;
                                continue;
                            }
                            RefreshmentOutcome::Acknowledged(0) => {
                                let mut account = accounting_peer.lock().await;
                                account.refreshment = Date::now();
                                account.balance = 0;
                                account.refresh_scheduled = false;
                                interface_log(format!("Refreshment attempt cleared {}", 0));
                                return;
                            }
                            RefreshmentOutcome::Acknowledged(amount) => {
                                accounting_peer.lock().await.refreshment = Date::now();
                                amount
                            }
                            RefreshmentOutcome::AmbiguousAfterPayment => {
                                interface_log(format!("Refreshment attempt cleared {}", 0));
                                quiesce_drain_and_close_accounting_session(
                                    &refreshment_wings,
                                    &refreshment_swarm,
                                    peer,
                                    &accounting_peer,
                                    connection_id,
                                )
                                .await;
                                return;
                            }
                        };

                        interface_log(format!("Applied refreshment {}", amount));
                        if let Some((peer, surplus_growth, surplus_balance)) =
                            apply_refreshment(&accounting_peer, amount).await
                        {
                            interface_log(format!(
                                "Surplus balance increased for peer {} by {} to {}",
                                peer, surplus_growth, surplus_balance
                            ));
                        }

                        let balance = accounting_peer.lock().await.balance;
                        if cheques_active_cache.load(Ordering::Relaxed) && balance > REFRESH_RATE {
                            let cheque_amt = balance - REFRESH_RATE;
                            let cheque_generation = cheque_generations
                                .fetch_add(1, Ordering::Relaxed)
                                .wrapping_add(1);
                            if claim_current_cheque(
                                &refreshment_wings,
                                peer,
                                &accounting_peer,
                                connection_id,
                                cheque_amt,
                                cheque_generation,
                            )
                            .await
                                && cheque_chan
                                    .try_send((peer, cheque_amt, cheque_generation))
                                    .is_err()
                            {
                                let mut cheques = refreshment_wings.ongoing_cheques.lock().await;
                                if cheques.get(&peer).copied()
                                    == Some((cheque_amt, cheque_generation))
                                {
                                    cheques.remove(&peer);
                                }
                            }
                        }
                    }
                });
                refresh_dispatches += 1;
                if refresh_dispatches % 8 == 0 {
                    async_std::task::sleep(Duration::ZERO).await;
                } else {
                    async_std::task::yield_now().await;
                }
            }
        };

        let swap_price = std::cell::Cell::new(U256::from(0));
        let swap_deduction = std::cell::Cell::new(U256::from(0));

        let cheque_instruction_handle = async {
            loop {
                let mut cheque_instruction = match cheque_instructions_chan_incoming.recv().await {
                    Ok(instruction) => instruction,
                    Err(_) => break,
                };
                let mut cheque_joiner = Vec::new();

                loop {
                    let swap_price_0 = &swap_price;
                    let swap_deduction_0 = &swap_deduction;
                    let set_price = swap_price_0.get().is_zero();

                    if set_price
                        && let Some((oracle_price, cheque_deduction)) =
                            get_price_from_oracle().await
                    {
                        if swap_price_0.get().is_zero() {
                            swap_price_0.set(oracle_price);
                        }

                        if swap_deduction_0.get().is_zero() {
                            swap_deduction_0.set(cheque_deduction);
                        }
                    }

                    let (peer, amount, cheque_generation) = cheque_instruction;
                    let cheque_control = cheque_control.clone();
                    let cheque_chan = &cheque_send_chan_outgoing;
                    let wings = &wings;
                    let handle = async move {
                        let price = swap_price_0.get();
                        let deduction = swap_deduction_0.get();

                        if price.is_zero() {
                            let _ = cheque_chan.try_send((peer, false, cheque_generation));
                            return;
                        }

                        let still_current = {
                            let map = wings.ongoing_cheques.lock().await;
                            map.get(&peer).copied() == Some((amount, cheque_generation))
                        };
                        if !still_current {
                            return;
                        }
                        let accounting_peer = {
                            let accounting = wings.accounting_peers.lock().await;
                            accounting.get(&peer).cloned()
                        };
                        let Some(accounting_peer) = accounting_peer else {
                            let _ = cheque_chan.try_send((peer, false, cheque_generation));
                            return;
                        };
                        let connection_id = accounting_peer.lock().await.connection_id;
                        let Some(protocol_session) = connection_id.and_then(|connection_id| {
                            OutboundProtocolSession::capture(
                                peer,
                                connection_id,
                                wings.physical_connections.clone(),
                            )
                        }) else {
                            let _ = cheque_chan.try_send((peer, false, cheque_generation));
                            return;
                        };
                        let beneficiary = {
                            let connected_peers = wings.connected_peers.lock().await;
                            connected_peers
                                .get(&peer)
                                .filter(|peer_file| {
                                    peer_file.connection_id == protocol_session.connection_id()
                                })
                                .map(|peer_file| peer_file.beneficiary)
                        };
                        let Some(beneficiary) = beneficiary else {
                            let _ = cheque_chan.try_send((peer, false, cheque_generation));
                            return;
                        };
                        let still_current = {
                            let map = wings.ongoing_cheques.lock().await;
                            map.get(&peer).copied() == Some((amount, cheque_generation))
                        };
                        if !still_current {
                            return;
                        }

                        let ok = issue_handler(
                            peer,
                            amount,
                            cheque_control,
                            protocol_session,
                            beneficiary,
                            price,
                            deduction,
                        )
                        .await;
                        let _ = cheque_chan.try_send((peer, ok, cheque_generation));
                    };
                    cheque_joiner.push(handle);

                    match cheque_instructions_chan_incoming.try_recv() {
                        Ok(next) => cheque_instruction = next,
                        Err(_) => break,
                    }
                }

                join_all(cheque_joiner).await;
                async_std::task::yield_now().await;
            }
        };

        let cheque_apply_handle = async {
            loop {
                let mut cheque_result = match cheque_send_chan_incoming.recv().await {
                    Ok(result) => result,
                    Err(_) => break,
                };

                loop {
                    let (peer, ok, cheque_generation) = cheque_result;
                    let current_cheque = {
                        let cheques = wings.ongoing_cheques.lock().await;
                        cheques.get(&peer).copied()
                    };
                    if let Some((amount, generation)) = current_cheque
                        && generation == cheque_generation
                    {
                        let accounting_peer = {
                            let accounting = wings.accounting_peers.lock().await;
                            accounting.get(&peer).cloned()
                        };
                        let still_current = {
                            let cheques = wings.ongoing_cheques.lock().await;
                            cheques.get(&peer).copied() == Some((amount, cheque_generation))
                        };
                        if still_current
                            && ok
                            && let Some(accounting_peer) = accounting_peer
                        {
                            let _ = apply_refreshment(&accounting_peer, amount).await;
                        }
                        let mut cheques = wings.ongoing_cheques.lock().await;
                        if cheques.get(&peer).copied() == Some((amount, cheque_generation)) {
                            cheques.remove(&peer);
                        }
                    }

                    match cheque_send_chan_incoming.try_recv() {
                        Ok(next) => cheque_result = next,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let acquire_range_handle = async {
            let range_sem = Arc::new(Semaphore::new(RANGE_REQUEST_CONCURRENCY));
            loop {
                let mut incoming_request = match self.range_port.1.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };
                loop {
                    let request = incoming_request;
                    let chunk_retrieve_chan = chunk_retrieve_chan_outgoing.clone();
                    let range_permit = range_sem.acquire_arc().await;

                    spawn_local(async move {
                        // Closing admission never cancels dispatched accounting work.
                        let _range_permit = range_permit;
                        let BzzRangeRequest {
                            metadata,
                            start,
                            end_inclusive,
                            cancel,
                            chan,
                        } = request;
                        let data = if cancel.is_some() {
                            bzz_stream::acquire_resolved_range_cancellable(
                                metadata,
                                start,
                                end_inclusive,
                                &chunk_retrieve_chan,
                                cancel,
                            )
                            .await
                        } else {
                            bzz_stream::acquire_resolved_range(
                                metadata,
                                start,
                                end_inclusive,
                                &chunk_retrieve_chan,
                            )
                            .await
                        };
                        let _ = chan.try_send(data);
                    });

                    match self.range_port.1.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let push_handle = async {
            loop {
                let mut incoming_request = match self.upload_port.1.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };

                loop {
                    let (file0, enc, redundancy_level, index, feed, topic, progress, chan) =
                        incoming_request;

                    if !secure_ensure_authorized().await {
                        self.interface_log(
                            "Could not authorize weeb-3-secure for upload signing".to_string(),
                        );
                        let _ = chan.try_send(vec![]);
                    } else {
                        let push_reference = upload_resource(
                            file0,
                            enc,
                            redundancy_level,
                            index,
                            "404.html".to_string(),
                            feed,
                            topic,
                            &chunk_upload_chan_outgoing,
                            &chunk_retrieve_chan_outgoing,
                            progress,
                        )
                        .await;
                        let _ = chan.try_send(push_reference);
                    }

                    match self.upload_port.1.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let push_chunk_handle = async {
            let push_sem = Arc::new(Semaphore::new(PUSH_CHUNK_CONCURRENCY));

            loop {
                let mut incoming_request = match self.chunk_push_port.1.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };
                loop {
                    let (d, soc, checkad, stamp, feedback, slot_feedback, progress) =
                        incoming_request;

                    if feedback.is_closed() {
                        let _ = slot_feedback.try_send(true);
                        match self.chunk_push_port.1.try_recv() {
                            Ok(request) => {
                                incoming_request = request;
                                continue;
                            }
                            Err(_) => break,
                        }
                    }

                    wait_transfer_unpaused(&self.transfer_paused).await;

                    let Some(permit) = push_sem.try_acquire_arc() else {
                        async_std::task::sleep(Duration::from_millis(PUSH_CHUNK_QUEUE_BACKOFF_MS))
                            .await;
                        if !feedback.is_closed() {
                            let _ = chunk_upload_chan_outgoing.try_send((
                                d,
                                soc,
                                checkad,
                                stamp,
                                feedback,
                                slot_feedback,
                                progress,
                            ));
                        } else {
                            let _ = slot_feedback.try_send(true);
                        }
                        break;
                    };

                    if feedback.is_closed() {
                        let _ = slot_feedback.try_send(true);
                        drop(permit);
                        match self.chunk_push_port.1.try_recv() {
                            Ok(request) => {
                                incoming_request = request;
                                continue;
                            }
                            Err(_) => break,
                        }
                    }

                    let upload_control = upload_control.clone();
                    let wings = wings.clone();
                    let refreshment = refreshment_instructions_chan_outgoing.clone();
                    let chunk_upload_chan_outgoing = chunk_upload_chan_outgoing.clone();
                    let log_port = self.log_port.0.clone();
                    let log_start_ms = self.log_start_ms;
                    let transfer_paused = self.transfer_paused.clone();
                    spawn_local(async move {
                        wait_transfer_unpaused(&transfer_paused).await;
                        let address = {
                            let _permit = permit;
                            push_chunk(
                                d.clone(),
                                soc,
                                checkad.clone(),
                                stamp.clone(),
                                upload_control.clone(),
                                &wings.overlay_peers,
                                &wings.accounting_peers,
                                &wings.physical_connections,
                                &refreshment,
                                Some(transfer_paused.clone()),
                            )
                            .await
                        };
                        let _ = slot_feedback.try_send(true);

                        let chunk = if !address.is_empty() {
                            wait_transfer_unpaused(&transfer_paused).await;
                            retrieve_check_chunk(
                                &checkad,
                                upload_control.clone(),
                                &wings.overlay_peers,
                                &wings.accounting_peers,
                                &wings.physical_connections,
                                &refreshment,
                                Some(transfer_paused.clone()),
                            )
                            .await
                        } else {
                            vec![]
                        };

                        if chunk.is_empty() {
                            if !address.is_empty() {
                                interface_log_to(
                                    &log_port,
                                    log_start_ms,
                                    format!(
                                        "Retrieve check failed for chunk {}",
                                        hex::encode(&checkad)
                                    ),
                                );
                            }
                            if !feedback.is_closed() {
                                async_std::task::sleep(Duration::from_millis(
                                    PUSH_CHUNK_RETRY_DELAY_MS,
                                ))
                                .await;
                                if !feedback.is_closed() {
                                    let _ = chunk_upload_chan_outgoing.try_send((
                                        d.clone(),
                                        soc,
                                        checkad.clone(),
                                        stamp.clone(),
                                        feedback.clone(),
                                        slot_feedback.clone(),
                                        progress.clone(),
                                    ));
                                }
                            }
                        } else {
                            report_upload_progress(&progress, 0, 1);
                            let _ = feedback.try_send(true);
                        }
                    });

                    match self.chunk_push_port.1.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let retrieve_chunk_handle = async {
            let retrieve_sem = Arc::new(Semaphore::new(RETRIEVE_CHUNK_CONCURRENCY));
            let retrieve_dispatch_yield_every = 128usize;
            let mut retrieve_dispatches_since_browser_yield = 0usize;

            loop {
                let mut incoming_request = match self.chunk_port.1.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };

                loop {
                    let request = incoming_request;
                    let n = request.address;
                    let chan = request.chan;
                    let cancel = request.cancel;
                    let admission = request.admission;
                    let hedge_demand = request.hedge_demand;
                    let admission_open =
                        wait_transfer_unpaused_for_admission(&self.transfer_paused, &admission)
                            .await;

                    let stream_generation_current = retrieve_cancel_token_current(&cancel);
                    if !admission_open
                        || !retrieval_conventions::retrieve_admission_current(
                            stream_generation_current,
                            &admission,
                        )
                    {
                        let _ = chan.try_send(vec![]);
                        match self.chunk_port.1.try_recv() {
                            Ok(request) => {
                                incoming_request = request;
                                async_std::task::sleep(Duration::from_millis(
                                    RETRIEVE_QUEUE_HOT_LOOP_GUARD_MS,
                                ))
                                .await;
                                continue;
                            }
                            Err(_) => break,
                        }
                    }

                    let sem = retrieve_sem.clone();
                    let retrieval_control = retrieval_control.clone();
                    let wings = wings.clone();
                    let refresh_chan = refreshment_instructions_chan_outgoing.clone();
                    let transfer_paused = self.transfer_paused.clone();

                    retrieve_dispatches_since_browser_yield += 1;

                    spawn_local(async move {
                        let chunk_data = async {
                            if !wait_transfer_unpaused_for_admission(&transfer_paused, &admission)
                                .await
                            {
                                return vec![];
                            }

                            let Some(_permit) = retrieval_conventions::acquire_retrieve_permit(
                                &sem,
                                admission.as_ref(),
                            )
                            .await
                            else {
                                return vec![];
                            };

                            if !wait_transfer_unpaused_for_admission(&transfer_paused, &admission)
                                .await
                            {
                                return vec![];
                            }

                            let stream_generation_current = retrieve_cancel_token_current(&cancel);
                            if !retrieval_conventions::retrieve_admission_current(
                                stream_generation_current,
                                &admission,
                            ) {
                                return vec![];
                            }

                            retrieve_chunk(
                                &n,
                                retrieval_control,
                                &wings.overlay_peers,
                                &wings.accounting_peers,
                                &wings.physical_connections,
                                &refresh_chan,
                                cancel,
                                admission,
                                hedge_demand,
                                Some(transfer_paused),
                            )
                            .await
                        }
                        .await;

                        let _ = chan.try_send(chunk_data);
                    });

                    if retrieve_dispatches_since_browser_yield >= retrieve_dispatch_yield_every {
                        retrieve_dispatches_since_browser_yield = 0;
                        async_std::task::sleep(Duration::ZERO).await;
                    }

                    match self.chunk_port.1.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let handshake_instruction_handle = async {
            loop {
                let mut connection_instruction =
                    match connections_instructions_chan_incoming.recv().await {
                        Ok(instruction) => instruction,
                        Err(_) => break,
                    };
                let (mut current_generation, mut network_id) =
                    self.current_connection_context().await;

                loop {
                    let (
                        underlay_address,
                        bootnode,
                        instruction_generation,
                        connection_attempt_id,
                        ready_connection,
                    ) = connection_instruction;
                    if instruction_generation != current_generation {
                        (current_generation, network_id) = self.current_connection_context().await;
                    }
                    if instruction_generation != current_generation {
                        match connections_instructions_chan_incoming.try_recv() {
                            Ok(instruction) => {
                                connection_instruction = instruction;
                                continue;
                            }
                            Err(_) => break,
                        }
                    }

                    let handshake_control = handshake_control.clone();
                    let accounting_peer_chan_outgoing = accounting_peer_chan_outgoing.clone();
                    let peers_instructions_chan_outgoing = peers_instructions_chan_outgoing.clone();
                    let connection_generation = self.connection_generation.clone();
                    let connection_population = self.connection_population.clone();
                    let handshake_signer = self.handshake_signer.clone();
                    let swarm = self.swarm.clone();
                    let wings = wings.clone();

                    spawn_local(async move {
                        let id = match try_from_multiaddr(&underlay_address) {
                            Some(peer_id) => peer_id,
                            None => return,
                        };

                        if bootnode {
                            wings.bootnodes.lock().await.insert(id);
                        }

                        if connection_generation.load(Ordering::Acquire) != instruction_generation {
                            let connected_peers = wings.connected_peers.lock().await;
                            let had_attempt =
                                remove_connection_attempt(&wings, &id, connection_attempt_id).await;
                            if had_attempt {
                                release_connection_reservation(&connection_population).await;
                            }
                            drop(connected_peers);
                            return;
                        }
                        let Some(physical_connection_id) = ({
                            let attempts = wings.connection_attempts.lock().await;
                            attempts
                                .get(&id)
                                .filter(|attempt| attempt.id == connection_attempt_id)
                                .and_then(|attempt| attempt.physical_connection_id)
                        }) else {
                            return;
                        };
                        let handshake_ready = async_std::future::timeout(
                            Duration::from_millis(PRE_HANDSHAKE_CONNECTION_TIMEOUT_MS),
                            async {
                                if connection_generation.load(Ordering::Acquire)
                                    != instruction_generation
                                    || !connection_attempt_is_current(
                                        &wings,
                                        &id,
                                        connection_attempt_id,
                                    )
                                    .await
                                {
                                    return false;
                                }
                                let mut handshake_ready = wings
                                    .handshake_ready_connections
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .contains(&(id, physical_connection_id));
                                loop {
                                    if !handshake_ready
                                        && !matches!(
                                            ready_connection.recv().await,
                                            Ok(connection_id)
                                                if connection_id == physical_connection_id
                                        )
                                    {
                                        return false;
                                    }
                                    if connection_generation.load(Ordering::Acquire)
                                        != instruction_generation
                                        || !connection_attempt_is_current(
                                            &wings,
                                            &id,
                                            connection_attempt_id,
                                        )
                                        .await
                                    {
                                        return false;
                                    }
                                    if exclusive_physical_connection(
                                        &wings.physical_connections,
                                        &id,
                                    ) == Some(physical_connection_id)
                                    {
                                        return true;
                                    }
                                    handshake_ready = false;
                                }
                            },
                        )
                        .await
                        .unwrap_or(false);
                        let success = if handshake_ready {
                            async_std::future::timeout(
                                Duration::from_millis(HANDSHAKE_PROTOCOL_TIMEOUT_MS),
                                connection_handler(
                                    id,
                                    local_peer_id,
                                    connection_attempt_id,
                                    physical_connection_id,
                                    wings.physical_connections.clone(),
                                    network_id,
                                    handshake_control,
                                    &underlay_address,
                                    &handshake_signer,
                                    &accounting_peer_chan_outgoing,
                                ),
                            )
                            .await
                            .unwrap_or(false)
                        } else {
                            false
                        };

                        if !success {
                            let connected_peers = wings.connected_peers.lock().await;
                            if !connection_attempt_is_current(&wings, &id, connection_attempt_id)
                                .await
                            {
                                drop(connected_peers);
                                return;
                            }
                            let pending_dial_aborted = {
                                let mut swarm = swarm.lock().await;
                                swarm.disconnect_peer_id(id).is_err()
                            };
                            let pending_dial_released = pending_dial_aborted
                                && remove_connection_attempt(&wings, &id, connection_attempt_id)
                                    .await;
                            drop(connected_peers);
                            if pending_dial_released {
                                queue_peer_dial_retry(
                                    underlay_address,
                                    instruction_generation,
                                    connection_generation,
                                    peers_instructions_chan_outgoing,
                                    bootnode,
                                    wings.delayed_peer_retries.clone(),
                                )
                                .await;
                                release_connection_reservation(&connection_population).await;
                            }
                        }
                    });

                    match connections_instructions_chan_incoming.try_recv() {
                        Ok(instruction) => connection_instruction = instruction,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        join!(
            accounting_event_handle,
            pricing_event_handle,
            refreshment_instruction_handle,
            cheque_instruction_handle,
            cheque_apply_handle,
            acquire_range_handle,
            retrieve_chunk_handle,
            push_handle,
            push_chunk_handle,
            peer_dial_scheduler,
            swarm_event_loop,
            bootnode_change_handle,
            gossip_inbound_handle,
            pricing_inbound_handle,
            handshake_instruction_handle,
        );
    }
}

impl Weeb3 {
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

    pub(crate) async fn acquire_feed_envelope(&self, owner: String, topic: String) -> Vec<u8> {
        let failed_feed_result = |message: &str| {
            encode_resources(
                vec![(
                    message.as_bytes().to_vec(),
                    "not found".to_string(),
                    "not found".to_string(),
                )],
                "not found".to_string(),
            )
        };
        let progress_id = self
            .start_progress(
                "feed",
                format!(
                    "{} topic {}",
                    if owner.trim().is_empty() {
                        "current-wallet"
                    } else {
                        owner.trim()
                    },
                    topic.trim()
                ),
                "resolve",
                None,
                "seeking latest feed update",
            )
            .await;
        let owner_bytes = if owner.trim().is_empty() {
            match secure_ensure_feed_owner().await {
                Some(owner) => owner,
                None => {
                    self.finish_progress(&progress_id, "failed", "feed owner unavailable", false)
                        .await;
                    return failed_feed_result("feed owner unavailable");
                }
            }
        } else {
            match hex::decode(strip_hex_prefix(owner.trim())) {
                Ok(owner) => owner,
                Err(_) => {
                    self.finish_progress(&progress_id, "failed", "invalid feed owner", false)
                        .await;
                    return failed_feed_result("invalid feed owner");
                }
            }
        };

        if owner_bytes.len() != 20 {
            self.finish_progress(&progress_id, "failed", "invalid feed owner", false)
                .await;
            return failed_feed_result("invalid feed owner");
        }

        let topic_safe = normalize_feed_topic(&topic);

        match acquire_latest_feed(hex::encode(owner_bytes), topic_safe, &self.chunk_port.0).await {
            Some((bytes, metadata)) => {
                self.finish_progress(
                    &progress_id,
                    "complete",
                    format!("{} bytes", bytes.len()),
                    true,
                )
                .await;
                encode_resources(
                    vec![(bytes, metadata.mime, metadata.path.clone())],
                    metadata.path,
                )
            }
            None => {
                self.finish_progress(&progress_id, "failed", "feed update not found", false)
                    .await;
                failed_feed_result("")
            }
        }
    }

    pub async fn resolve_bzz(&self, resource: String) -> Option<BzzMetadata> {
        bzz_stream::resolve_bzz(&resource, &self.chunk_port.0).await
    }

    pub async fn acquire_resolved_range(
        &self,
        metadata: BzzMetadata,
        start: u64,
        end_inclusive: u64,
    ) -> Option<(Vec<u8>, BzzMetadata)> {
        let (chan_out, chan_in) = mpsc::bounded::<Option<(Vec<u8>, BzzMetadata)>>(1);
        if self
            .range_port
            .0
            .try_send(BzzRangeRequest {
                metadata,
                start,
                end_inclusive,
                cancel: None,
                chan: chan_out,
            })
            .is_err()
        {
            return None;
        }

        chan_in.recv().await.unwrap_or(None)
    }

    pub(crate) async fn acquire_resolved_stream_range(
        &self,
        metadata: BzzMetadata,
        start: u64,
        end_inclusive: u64,
        stream_key: String,
        stream_generation: u64,
    ) -> Option<(Vec<u8>, BzzMetadata)> {
        let (chan_out, chan_in) = mpsc::bounded::<Option<(Vec<u8>, BzzMetadata)>>(1);
        // Superseded ranges stop admission without cancelling dispatched work.
        let cancel = self
            .retrieve_cancel_registry
            .register(stream_key, stream_generation)
            .await;

        if self
            .range_port
            .0
            .try_send(BzzRangeRequest {
                metadata,
                start,
                end_inclusive,
                cancel,
                chan: chan_out,
            })
            .is_err()
        {
            return None;
        }

        chan_in.recv().await.unwrap_or(None)
    }
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    stream: StreamBehaviour,
}

impl Behaviour {
    fn new(local_public_key: identity::PublicKey) -> Self {
        Self {
            identify: identify::Behaviour::new(identify::Config::new(
                "/weeb-3".into(),
                local_public_key,
            )),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(15))),
            stream: StreamBehaviour::new(),
        }
    }
}
