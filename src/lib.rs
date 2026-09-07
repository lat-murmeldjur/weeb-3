#![cfg(target_arch = "wasm32")]

use async_lock::Semaphore;
use async_std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use wasm_bindgen_futures::spawn_local;

pub(crate) use async_std::channel as mpsc;
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZero;
use std::time::Duration;

use web3::types::U256;

use js_sys::Date;
use libp2p::{
    PeerId, StreamProtocol, Swarm,
    core::{self, Multiaddr, Transport},
    futures::{
        StreamExt,
        future::{Either, join_all, select},
        join,
    },
    identify,
    identity::ecdsa,
    noise,
    swarm::{ConnectionId, DialError, SwarmEvent},
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

mod network_conventions;
pub(crate) use network_conventions::*;

mod runtime_conventions;
pub(crate) use runtime_conventions::*;

mod signer;
pub(crate) use signer::PrivateKeySigner;

mod accounting;
use accounting::{
    REFRESH_RATE, RefreshmentInstruction, apply_credit, apply_refreshment,
    bee_reconnect_delay_seconds, cancel_reserve, connection_dial_capacity_available, price,
    refreshment_due, reserve, set_payment_threshold,
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

mod on_chain_conventions;

mod nav;

mod network_profile;
use network_profile::{activate_profile, profile_for_swarm_network_id};

mod persistence;
use persistence::{get_chequebook_address, get_chequebook_signer_key};

mod retrieval;
use retrieval::*;

mod retrieval_conventions;
pub(crate) use retrieval_conventions::{
    RetrieveCancelRegistry, RetrieveCancelToken, TransferPause, retrieve_cancel_token_current,
    transfer_pause_enabled, wait_transfer_unpaused, wait_transfer_unpaused_for_admission,
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
const PEER_DIAL_INGEST_BATCH: usize = 256;
const OUTBOUND_CONNECTION_TIMEOUT_MS: u64 = 8_000;
const PRE_HANDSHAKE_CONNECTION_TIMEOUT_MS: u64 = 60_000;
const PEER_RETRY_DELAY_MS: u64 = 500;
const MAINNET_BOOTNODE_RETRY_DELAY_MS: u64 = 30_000;
const MAINNET_BOOTNODE_RETRY_JITTER_MS: u64 = 5_000;
const PUSH_CHUNK_RETRY_DELAY_MS: u64 = 500;
const PUSH_CHUNK_QUEUE_BACKOFF_MS: u64 = 25;
const RANGE_REQUEST_CONCURRENCY: usize = 16;
const RETRIEVE_CHUNK_CONCURRENCY: usize = 256;
const RANGE_REQUEST_QUEUE_CAPACITY: usize = 256;
const LOG_QUEUE_CAPACITY: usize = 256;
const LOG_DRAIN_BATCH: usize = 64;
pub(crate) const LOG_DOM_RETAINED: u32 = 256;

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
    transfer_paused: Arc<TransferPause>,
    retrieve_cancel_registry: RetrieveCancelRegistry,
    connection_generation: Arc<AtomicU64>,
    connection_population: Arc<Mutex<ConnectionPopulation>>,
    progress: Arc<Mutex<ProgressStore>>,
}

impl Weeb3 {
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

            fvec0 = match tar_resources(&content) {
                Ok(resources) => resources,
                Err(_) => {
                    self.finish_progress(&progress_id, "failed", "invalid tar archive", false)
                        .await;
                    return upload_result("upload result: invalid tar archive", "");
                }
            };
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

        upload_result(message, "... result ...")
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
            wings: Arc::new(Wings::default()),
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
            transfer_paused: Arc::new(TransferPause::default()),
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

    pub fn interface_log(&self, log0: String) {
        interface_log_to(&self.log_port.0, self.log_start_ms, log0);
    }

    pub async fn toggle_transfer_pause(&self) -> bool {
        let paused = self.transfer_paused.toggle();
        self.interface_log(if paused {
            "Paused retrieve / push scheduling".to_string()
        } else {
            "Resumed retrieve / push scheduling".to_string()
        });
        paused
    }

    pub fn transfer_paused(&self) -> bool {
        transfer_pause_enabled(&self.transfer_paused)
    }

    pub async fn run(&self) {
        if self.runtime_started.swap(true, Ordering::AcqRel) {
            return;
        }

        self.interface_log("Node runtime handlers starting".to_string());
        let wings = self.wings.clone();
        let local_peer_id = { *self.swarm.lock().await.local_peer_id() };

        let (peers_instructions_chan_outgoing, peers_instructions_chan_incoming) =
            mpsc::bounded::<PeerDialInstruction>(PEER_DIAL_INGEST_BATCH);
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

        let peer_dial_scheduler =
            async {
                let mut queue = VecDeque::<QueuedPeerDial>::new();
                let mut queued_underlays = HashSet::<(PeerId, Multiaddr)>::new();
                let mut queue_generation = self.current_connection_generation();
                let mut pending_instruction = None;

                loop {
                    let current_generation = self.current_connection_generation();
                    if current_generation != queue_generation {
                        queue.clear();
                        queued_underlays.clear();
                        queue_generation = current_generation;
                    }
                    let instruction = match pending_instruction.take() {
                        Some(instruction) => Some(instruction),
                        None if queue.is_empty() => {
                            match peers_instructions_chan_incoming.recv().await {
                                Ok(instruction) => Some(instruction),
                                Err(_) => break,
                            }
                        }
                        None => peers_instructions_chan_incoming.try_recv().ok(),
                    };
                    let public_gossip_only = self.service_worker_network_id() != 0
                        && !self.allow_private_gossip.load(Ordering::Acquire);
                    for instruction in instruction
                        .into_iter()
                        .chain(std::iter::from_fn(|| {
                            peers_instructions_chan_incoming.try_recv().ok()
                        }))
                        .take(PEER_DIAL_INGEST_BATCH)
                    {
                        let current_generation = self.current_connection_generation();
                        if current_generation != queue_generation {
                            queue.clear();
                            queued_underlays.clear();
                            queue_generation = current_generation;
                        }
                        if instruction.generation != queue_generation {
                            continue;
                        }
                        for candidate in peer_dial_candidates(instruction, public_gossip_only) {
                            if candidate.peer != local_peer_id
                                && queued_underlays
                                    .insert((candidate.peer, candidate.dial_addr.clone()))
                            {
                                queue.push_back(candidate);
                            }
                        }
                    }
                    // Listen before inspecting eligibility, without reserving a slot we may release.
                    let capacity_changed = self.connection_population.lock().await.changed.listen();
                    let connected = wings
                        .connected_peers
                        .lock()
                        .await
                        .keys()
                        .copied()
                        .collect::<HashSet<_>>();
                    let rejected = wings.rejected_duplicate_peers.lock().await.clone();
                    let mut unavailable = wings.connection_cooldowns.lock().await.clone();
                    unavailable.extend(wings.connection_attempts.lock().await.keys().copied());
                    unavailable.extend(wings.delayed_peer_retries.lock().await.iter().filter_map(
                        |(peer, retry)| (retry.0 == queue_generation).then_some(*peer),
                    ));
                    let mut next_candidate = None;
                    for _ in 0..queue.len() {
                        let candidate = queue.pop_front().unwrap();
                        if candidate.generation != self.current_connection_generation()
                            || connected.contains(&candidate.peer)
                            || rejected.contains_key(&candidate.peer)
                        {
                            queued_underlays.remove(&(candidate.peer, candidate.dial_addr));
                        } else if unavailable.contains(&candidate.peer) {
                            queue.push_back(candidate);
                        } else {
                            next_candidate = Some(candidate);
                            break;
                        }
                    }
                    let candidate = if next_candidate.is_some()
                        && try_reserve_connection_capacity(&self.connection_population).await
                    {
                        next_candidate.take().unwrap()
                    } else {
                        if let Some(candidate) = next_candidate {
                            queue.push_front(candidate);
                        }
                        // At capacity, keep consuming and deduplicating gossip until a slot opens.
                        match select(
                            Box::pin(capacity_changed),
                            Box::pin(peers_instructions_chan_incoming.recv()),
                        )
                        .await
                        {
                            Either::Left(_) => {}
                            Either::Right((Ok(instruction), _)) => {
                                pending_instruction = Some(instruction);
                            }
                            Either::Right((Err(_), _)) => break,
                        }
                        continue;
                    };
                    queued_underlays.remove(&(candidate.peer, candidate.dial_addr.clone()));
                    if self.current_connection_generation() != candidate.generation {
                        release_connection_reservation(&self.connection_population).await;
                        continue;
                    }
                    let Some((attempt_id, ready_connection)) =
                        try_mark_connection_attempt(&wings, &candidate.peer).await
                    else {
                        release_connection_reservation(&self.connection_population).await;
                        if self.current_connection_generation() == candidate.generation
                            && queued_underlays
                                .insert((candidate.peer, candidate.dial_addr.clone()))
                        {
                            queue.push_back(candidate);
                        }
                        continue;
                    };
                    if self.current_connection_generation() != candidate.generation {
                        if remove_connection_attempt(&wings, &candidate.peer, attempt_id).await {
                            release_connection_reservation(&self.connection_population).await;
                        }
                        continue;
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
                        Ok(false) | Err(_) => {
                            if remove_connection_attempt(&wings, &candidate.peer, attempt_id).await
                            {
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
            while let Some(event) = self.swarm.next_event().await {
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
                    yield_after_swarm_event(&mut events_since_browser_yield).await;
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
                                let was_bootnode = wings.bootnodes.lock().await.contains(&peer_id);
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

                            let accounting_peer =
                                wings.accounting_peers.lock().await.remove(&peer_id);
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
                            ACCOUNTING_DRAINED.notify(usize::MAX);
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

                yield_after_swarm_event(&mut events_since_browser_yield).await;
            }
        };

        let bootnode_change_handle = async {
            while let Ok(first_change) = self.bootnode_port.1.recv().await {
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
                            Ok(false) | Err(_) => {
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
            while let Ok(peer_file) = accounting_peer_chan_incoming.recv().await {
                for peer_file in std::iter::once(peer_file).chain(std::iter::from_fn(|| {
                    accounting_peer_chan_incoming.try_recv().ok()
                })) {
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
                }

                async_std::task::yield_now().await;
            }
        };

        let pricing_event_handle = async {
            while let Ok(pricing) = pricing_chan_incoming.recv().await {
                for pricing in std::iter::once(pricing)
                    .chain(std::iter::from_fn(|| pricing_chan_incoming.try_recv().ok()))
                {
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
            while let Ok((peer, accounting_peer, connection_id)) =
                refreshment_instructions_chan_incoming.recv().await
            {
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
                                drop(cheques);
                                ACCOUNTING_DRAINED.notify(usize::MAX);
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
            while let Ok(cheque_instruction) = cheque_instructions_chan_incoming.recv().await {
                let mut cheque_joiner = Vec::new();

                for cheque_instruction in
                    std::iter::once(cheque_instruction).chain(std::iter::from_fn(|| {
                        cheque_instructions_chan_incoming.try_recv().ok()
                    }))
                {
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
                        let accounting_peer =
                            wings.accounting_peers.lock().await.get(&peer).cloned();
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
                }

                join_all(cheque_joiner).await;
                async_std::task::yield_now().await;
            }
        };

        let cheque_apply_handle = async {
            while let Ok(cheque_result) = cheque_send_chan_incoming.recv().await {
                for cheque_result in
                    std::iter::once(cheque_result).chain(std::iter::from_fn(|| {
                        cheque_send_chan_incoming.try_recv().ok()
                    }))
                {
                    let (peer, ok, cheque_generation) = cheque_result;
                    let current_cheque = wings.ongoing_cheques.lock().await.get(&peer).copied();
                    if let Some((amount, generation)) = current_cheque
                        && generation == cheque_generation
                    {
                        let accounting_peer =
                            wings.accounting_peers.lock().await.get(&peer).cloned();
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
                        drop(cheques);
                        ACCOUNTING_DRAINED.notify(usize::MAX);
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let acquire_range_handle = async {
            let range_sem = Arc::new(Semaphore::new(RANGE_REQUEST_CONCURRENCY));
            while let Ok(incoming_request) = self.range_port.1.recv().await {
                for request in std::iter::once(incoming_request)
                    .chain(std::iter::from_fn(|| self.range_port.1.try_recv().ok()))
                {
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
                        let data = bzz_stream::acquire_resolved_range_cancellable(
                            metadata,
                            start,
                            end_inclusive,
                            &chunk_retrieve_chan,
                            cancel,
                        )
                        .await;
                        let _ = chan.try_send(data);
                    });
                }

                async_std::task::yield_now().await;
            }
        };

        let push_handle = async {
            while let Ok(incoming_request) = self.upload_port.1.recv().await {
                for incoming_request in std::iter::once(incoming_request)
                    .chain(std::iter::from_fn(|| self.upload_port.1.try_recv().ok()))
                {
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
                }

                async_std::task::yield_now().await;
            }
        };

        let push_chunk_handle = async {
            let push_sem = Arc::new(Semaphore::new(PUSH_CHUNK_CONCURRENCY));

            while let Ok(incoming_request) = self.chunk_push_port.1.recv().await {
                for incoming_request in
                    std::iter::once(incoming_request).chain(std::iter::from_fn(|| {
                        self.chunk_push_port.1.try_recv().ok()
                    }))
                {
                    let (d, soc, checkad, stamp, feedback, slot_feedback, progress) =
                        incoming_request;

                    if feedback.is_closed() {
                        let _ = slot_feedback.try_send(true);
                        continue;
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
                        continue;
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
                }

                async_std::task::yield_now().await;
            }
        };

        let retrieve_chunk_handle = async {
            let retrieve_sem = Arc::new(Semaphore::new(RETRIEVE_CHUNK_CONCURRENCY));
            let retrieve_dispatch_yield_every = 128usize;
            let mut retrieve_dispatches_since_browser_yield = 0usize;

            while let Ok(incoming_request) = self.chunk_port.1.recv().await {
                for request in std::iter::once(incoming_request)
                    .chain(std::iter::from_fn(|| self.chunk_port.1.try_recv().ok()))
                {
                    retrieve_dispatches_since_browser_yield += 1;
                    if retrieve_dispatches_since_browser_yield >= retrieve_dispatch_yield_every {
                        retrieve_dispatches_since_browser_yield = 0;
                        async_std::task::sleep(Duration::ZERO).await;
                    }

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
                        continue;
                    }

                    let sem = retrieve_sem.clone();
                    let retrieval_control = retrieval_control.clone();
                    let wings = wings.clone();
                    let refresh_chan = refreshment_instructions_chan_outgoing.clone();
                    let transfer_paused = self.transfer_paused.clone();

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
                }

                async_std::task::yield_now().await;
            }
        };

        let handshake_instruction_handle = async {
            while let Ok(connection_instruction) =
                connections_instructions_chan_incoming.recv().await
            {
                let (mut current_generation, mut network_id) =
                    self.current_connection_context().await;

                for connection_instruction in
                    std::iter::once(connection_instruction).chain(std::iter::from_fn(|| {
                        connections_instructions_chan_incoming.try_recv().ok()
                    }))
                {
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
                        continue;
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
            secure_ensure_feed_owner()
                .await
                .ok_or("feed owner unavailable")
        } else {
            hex::decode(strip_hex_prefix(owner.trim())).map_err(|_| "invalid feed owner")
        };
        let owner_bytes = match owner_bytes.and_then(|owner| {
            (owner.len() == 20)
                .then_some(owner)
                .ok_or("invalid feed owner")
        }) {
            Ok(owner) => owner,
            Err(message) => {
                self.finish_progress(&progress_id, "failed", message, false)
                    .await;
                return failed_feed_result(message);
            }
        };

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
        self.range_port
            .0
            .try_send(BzzRangeRequest {
                metadata,
                start,
                end_inclusive,
                cancel: None,
                chan: chan_out,
            })
            .ok()?;

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

        self.range_port
            .0
            .try_send(BzzRangeRequest {
                metadata,
                start,
                end_inclusive,
                cancel,
                chan: chan_out,
            })
            .ok()?;

        chan_in.recv().await.unwrap_or(None)
    }
}
