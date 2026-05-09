#![cfg(target_arch = "wasm32")]
use async_lock::Semaphore;
use async_std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen_futures::spawn_local;

pub(crate) use async_std::channel as mpsc;
use rand::rngs::OsRng;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::num::NonZero;
use std::time::Duration;

use tar::Archive;

use alloy::primitives::keccak256;
use web3::types::U256;

use libp2p::{
    PeerId, StreamProtocol, Swarm, autonat,
    core::{self, Multiaddr, Transport},
    dcutr,
    futures::{
        StreamExt,
        future::join_all, //
        join,
    },
    identify, identity,
    identity::{ecdsa, ecdsa::SecretKey},
    noise, ping,
    swarm::{NetworkBehaviour, SwarmEvent},
    websocket_websys, yamux,
};
use libp2p_stream as stream;

use js_sys::Date;
use wasm_bindgen::{JsValue, prelude::*};
use web_sys::File;

mod accounting;
use accounting::*;

mod addresses;
use addresses::{
    UnderlayFormat, beewss_to_dns_transformed, deserialize_underlays, detect_underlay_format,
};

mod bzz_stream;
use bzz_stream::*;

mod conventions;
use conventions::*;

mod handlers;
use handlers::*;

mod interface;

mod library;

mod manifest;

mod manifest_upload;

mod on_chain;
use on_chain::{chequebook_balance, get_price_from_oracle, web3};

mod nav;

mod persistence;
use persistence::{
    get_batch_bucket_limit, get_batch_id, get_batch_owner_key, get_chequebook_address,
    get_chequebook_signer_key, reset_stamp,
};

mod retrieval;
use retrieval::*;

mod streaming_player;

mod upload;
use upload::*;

mod ens;
use ens::*;

static MAINNET: AtomicBool = AtomicBool::new(false);
static TESTNET_OFFICIAL: AtomicBool = AtomicBool::new(true);

pub fn set_mainnet(value: bool) {
    MAINNET.store(value, Ordering::Relaxed);
}

pub fn is_mainnet() -> bool {
    MAINNET.load(Ordering::Relaxed)
}

pub fn set_testnet_official(value: bool) {
    TESTNET_OFFICIAL.store(value, Ordering::Relaxed);
}

pub fn is_testnet_official() -> bool {
    TESTNET_OFFICIAL.load(Ordering::Relaxed)
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
    pub mod etiquette_3 {
        include!(concat!(env!("OUT_DIR"), "/weeb_3.etiquette_3.rs"));
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

// use crate::weeb_3::etiquette_0;
// use crate::weeb_3::etiquette_1;
use crate::weeb_3::etiquette_2;
// use crate::weeb_3::etiquette_3;
// use crate::weeb_3::etiquette_4;
// use crate::weeb_3::etiquette_5;
// use crate::weeb_3::etiquette_6;
// use crate::weeb_3::etiquette_8;

const HANDSHAKE_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/handshake/14.0.0/handshake");
const PRICING_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pricing/1.0.0/pricing");
const GOSSIP_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/hive/1.1.0/peers");
const PSEUDOSETTLE_PROTOCOL: StreamProtocol =
    StreamProtocol::new("/swarm/pseudosettle/1.0.0/pseudosettle");
const RETRIEVAL_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/retrieval/1.4.0/retrieval");
const PUSHSYNC_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pushsync/1.3.1/pushsync");
const SWAP_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/swap/1.0.0/swap");

// const PINGPONG_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pingpong/1.0.0/pingpong");
// const STATUS_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/status/1.1.1/status");
//
// const PULL_CURSORS_PROTOCOL: StreamProtocol = StreamProtocol:: "/swarm/pullsync/1.4.0/cursors");
// const PULL_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pullsync/1.4.0/pullsync");
// const PUSH_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pushsync/1.3.0/pushsync");

const PROTOCOL_ROUND_TIME: f64 = 160.0;
const STARTUP_QUEUE_POLL_MS: u64 = 25;
const PUSH_CHUNK_CONFIRMATION_PEERS: usize = 2;
const RETRIEVE_CHECK_CONFIRMATION_PEERS: usize = 2;
const PUSH_CHUNK_CONCURRENCY: usize = 1280;
const HANDSHAKE_RETRY_DELAY_MS: u64 = 500;
const PUSH_CHUNK_RETRY_DELAY_MS: u64 = 500;
const PUSH_CHUNK_QUEUE_BACKOFF_MS: u64 = 25;

pub fn timed_log(message: impl AsRef<str>) {
    web_sys::console::log_1(&JsValue::from(message.as_ref()));
}

pub(crate) fn interface_log_to(log_port: &mpsc::Sender<String>, log_start_ms: f64, log0: String) {
    let elapsed_ms = (Date::now() - log_start_ms).max(0.0).round() as u64;
    let log = format!("[+{}ms] {}", elapsed_ms, log0);
    let _ = log_port.try_send(log.clone());
    web_sys::console::log_1(&JsValue::from(log));
}

async fn cheques_active() -> bool {
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

    match chequebook_balance(&w3, web3::types::Address::from_slice(&chequebook)).await {
        Ok(balance) => !balance.is_zero(),
        Err(_) => false,
    }
}

//
// pub fn init_panic_hook() {
//     console_error_panic_hook::set_once();
// }

#[wasm_bindgen]
pub struct Weeb3 {
    swarm: Arc<Mutex<Swarm<Behaviour>>>,
    secret_key: Arc<Mutex<SecretKey>>,
    wings: Mutex<Arc<Wings>>,
    log_port: (mpsc::Sender<String>, mpsc::Receiver<String>),
    log_start_ms: f64,
    message_port: (ChunkRetrieveSender, ChunkRetrieveReceiver),
    resolve_port: (
        mpsc::Sender<BzzResolveRequest>,
        mpsc::Receiver<BzzResolveRequest>,
    ),
    range_port: (
        mpsc::Sender<BzzRangeRequest>,
        mpsc::Receiver<BzzRangeRequest>,
    ),
    chunk_push_port: (
        mpsc::Sender<(Vec<u8>, bool, Vec<u8>, Vec<u8>, mpsc::Sender<bool>)>,
        mpsc::Receiver<(Vec<u8>, bool, Vec<u8>, Vec<u8>, mpsc::Sender<bool>)>,
    ),
    upload_port: (
        mpsc::Sender<(
            Vec<Resource>,
            bool,
            String,
            bool,
            String,
            mpsc::Sender<Vec<u8>>,
        )>,
        mpsc::Receiver<(
            Vec<Resource>,
            bool,
            String,
            bool,
            String,
            mpsc::Sender<Vec<u8>>,
        )>,
    ),
    bootnode_port: (
        mpsc::Sender<(String, mpsc::Sender<String>, bool, u64)>,
        mpsc::Receiver<(String, mpsc::Sender<String>, bool, u64)>,
    ),
    network_id: Mutex<u64>,
    transfer_paused: Arc<AtomicBool>,
    retrieve_cancel_generations: RetrieveGenerationMap,
    connection_generation: Arc<Mutex<u64>>,
    ongoing_connections: Arc<Mutex<u64>>,
    connections: Arc<Mutex<u64>>,
}

type PeerAddrMap = Arc<Mutex<HashMap<PeerId, Multiaddr>>>;
type RetrieveGenerationMap = Arc<Mutex<HashMap<String, u64>>>;
type ChunkRetrieveSender = mpsc::Sender<ChunkRetrieveRequest>;
type ChunkRetrieveReceiver = mpsc::Receiver<ChunkRetrieveRequest>;

#[derive(Clone, Debug)]
pub(crate) struct RetrieveCancelToken {
    pub stream_key: String,
    pub generation: u64,
}

#[derive(Clone)]
pub(crate) struct ChunkRetrieveRequest {
    pub address: Vec<u8>,
    pub chan: mpsc::Sender<Vec<u8>>,
    pub cancel: Option<RetrieveCancelToken>,
}

pub(crate) fn chunk_retrieve_request(
    address: Vec<u8>,
    chan: mpsc::Sender<Vec<u8>>,
) -> ChunkRetrieveRequest {
    ChunkRetrieveRequest {
        address,
        chan,
        cancel: None,
    }
}

pub(crate) fn cancellable_chunk_retrieve_request(
    address: Vec<u8>,
    chan: mpsc::Sender<Vec<u8>>,
    cancel: Option<RetrieveCancelToken>,
) -> ChunkRetrieveRequest {
    ChunkRetrieveRequest {
        address,
        chan,
        cancel,
    }
}

pub(crate) async fn register_retrieve_cancel_token(
    generations: &RetrieveGenerationMap,
    cancel: &Option<RetrieveCancelToken>,
) {
    let Some(cancel) = cancel else {
        return;
    };

    let mut generations = generations.lock().await;
    let entry = generations.entry(cancel.stream_key.clone()).or_insert(0);
    if *entry < cancel.generation {
        *entry = cancel.generation;
    }
}

pub(crate) async fn retrieve_cancel_token_current(
    generations: &RetrieveGenerationMap,
    cancel: &Option<RetrieveCancelToken>,
) -> bool {
    let Some(cancel) = cancel else {
        return true;
    };

    let generations = generations.lock().await;
    generations
        .get(&cancel.stream_key)
        .map(|generation| *generation <= cancel.generation)
        .unwrap_or(true)
}

pub(crate) fn transfer_pause_enabled(paused: &Arc<AtomicBool>) -> bool {
    paused.load(Ordering::Relaxed)
}

pub(crate) async fn wait_transfer_unpaused(paused: &Arc<AtomicBool>) {
    while transfer_pause_enabled(paused) {
        async_std::task::sleep(Duration::from_millis(100)).await;
    }
}

type BzzResolveRequest = (String, mpsc::Sender<Option<BzzMetadata>>);
enum BzzRangeRequest {
    Resource {
        resource: String,
        start: u64,
        end_inclusive: u64,
        cancel: Option<RetrieveCancelToken>,
        chan: mpsc::Sender<Option<(Vec<u8>, BzzMetadata)>>,
    },
    Resolved {
        metadata: BzzMetadata,
        start: u64,
        end_inclusive: u64,
        cancel: Option<RetrieveCancelToken>,
        chan: mpsc::Sender<Option<(Vec<u8>, BzzMetadata)>>,
    },
    Prepare {
        metadata: BzzMetadata,
        chan: mpsc::Sender<bool>,
    },
}

#[wasm_bindgen]
pub struct Wings {
    connected_peers: Arc<Mutex<HashMap<PeerId, PeerFile>>>,
    overlay_peers: Arc<Mutex<HashMap<String, PeerId>>>,
    bootnodes: Arc<Mutex<HashSet<String>>>,
    accounting_peers: Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>>,
    ongoing_refreshments: Arc<Mutex<HashMap<PeerId, u64>>>,
    ongoing_cheques: Arc<Mutex<HashMap<PeerId, u64>>>,
    swap_beneficiaries: Arc<Mutex<HashMap<PeerId, (web3::types::Address, bool)>>>,
    connection_attempts: Arc<Mutex<HashSet<PeerId>>>,
    known_peer_underlays: PeerAddrMap,
    self_ephemerals: PeerAddrMap,
    self_ephemeral_waiters: Arc<Mutex<HashMap<PeerId, Vec<mpsc::Sender<Multiaddr>>>>>,
}

#[wasm_bindgen]
impl Weeb3 {
    pub async fn change_bootnode_address(
        &self,
        address: String,
        _id: String,
        usable_in_protocols: bool,
    ) -> Vec<u8> {
        let parse_id = _id.parse::<u64>();
        let mut network_changed = false;

        match parse_id {
            Ok(parsed_id) => {
                web_sys::console::log_1(&JsValue::from(format!("Parsed network id {}", parsed_id)));
                let mut nid = self.network_id.lock().await;

                if *nid != parsed_id {
                    if parsed_id == 1 {
                        set_mainnet(true);
                        set_testnet_official(false);
                    }

                    if parsed_id == 10 {
                        set_testnet_official(true);
                        set_mainnet(false);
                    }

                    *nid = parsed_id;
                    network_changed = true;
                }
            }
            _ => {}
        };

        if network_changed {
            self.bump_connection_generation().await;
            self.disconnect_all_peers().await;
        }

        let generation = self.current_connection_generation().await;

        web_sys::console::log_1(&"bootnode change triggered".into());

        let (chan_out, chan_in) = mpsc::unbounded::<String>();
        let _ = self
            .bootnode_port
            .0
            .try_send((address, chan_out, usable_in_protocols, generation));

        let result = chan_in.recv().await.unwrap_or_default();

        if !usable_in_protocols {
            return encode_resources(
                vec![(
                    format!("Bootnode connect status: {}", result)
                        .as_bytes()
                        .to_vec(),
                    "text/plain".to_string(),
                    "... result ...".to_string(),
                )],
                "... result ...".to_string(),
            );
        } else {
            return encode_resources(
                vec![(
                    format!("Bootstrap connect status: {}", result)
                        .as_bytes()
                        .to_vec(),
                    "text/plain".to_string(),
                    "... result ...".to_string(),
                )],
                "... result ...".to_string(),
            );
        }
    }

    async fn current_connection_generation(&self) -> u64 {
        let generation = self.connection_generation.lock().await;
        *generation
    }

    async fn bump_connection_generation(&self) -> u64 {
        let mut generation = self.connection_generation.lock().await;
        *generation = generation.saturating_add(1);
        web_sys::console::log_1(&JsValue::from(format!(
            "Connection generation bumped to {}",
            *generation
        )));
        *generation
    }

    async fn disconnect_all_peers(&self) {
        {
            let mut swarm = self.swarm.lock_arc().await;
            let peers: Vec<_> = swarm.connected_peers().cloned().collect();
            for peer in peers {
                let _ = swarm.disconnect_peer_id(peer);
            }
        }

        let wings = self.wings.lock().await;

        wings.connected_peers.lock().await.clear();
        wings.overlay_peers.lock().await.clear();
        wings.connection_attempts.lock().await.clear();
        wings.accounting_peers.lock().await.clear();
        wings.bootnodes.lock().await.clear();
        wings.ongoing_refreshments.lock().await.clear();
        wings.ongoing_cheques.lock().await.clear();
        wings.swap_beneficiaries.lock().await.clear();
        wings.known_peer_underlays.lock().await.clear();
        wings.self_ephemerals.lock().await.clear();
        wings.self_ephemeral_waiters.lock().await.clear();

        {
            let mut ongoing = self.ongoing_connections.lock().await;
            *ongoing = 0;
        }

        {
            let mut connected = self.connections.lock().await;
            *connected = 0;
        }

        web_sys::console::log_1(&JsValue::from("All peers disconnected"));
    }

    async fn promote_priced_peer(&self, wings: &Arc<Wings>, peer: PeerId) {
        let peer_file = {
            let connected_peers_map = wings.connected_peers.lock().await;
            match connected_peers_map.get(&peer) {
                Some(peer_file) => peer_file.clone(),
                None => return,
            }
        };

        let ol = hex::encode(peer_file.overlay.clone());
        let bootnode = {
            let bootnodes_set = wings.bootnodes.lock().await;
            bootnodes_set.contains(&peer.to_string())
        };

        let promoted = if !bootnode {
            let mut overlay_peers_map = wings.overlay_peers.lock().await;
            if overlay_peers_map.contains_key(&ol) {
                false
            } else {
                overlay_peers_map.insert(ol.clone(), peer);
                true
            }
        } else {
            true
        };

        if promoted {
            if bootnode {
                self.interface_log(format!("Connected to bootnode {}", &ol));
            } else {
                self.interface_log(format!("Connected to peer {}", &ol));
            }

            {
                let mut connections = self.connections.lock().await;
                *connections = *connections + 1
            }
            {
                let mut ongoing = self.ongoing_connections.lock().await;
                if *ongoing > 0 {
                    *ongoing = *ongoing - 1
                }
            }
        }
    }

    pub async fn post_upload(
        &self,
        file: File,
        encryption: bool,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Vec<u8> {
        let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();

        let f_name = file.name();
        let f_type0 = file.type_();
        let f_type: String = match f_type0.starts_with("text/") {
            true => f_type0 + "; charset=utf-8",
            false => f_type0,
        };

        let mut fvec0: Vec<Resource> = vec![];

        let mut index_document = "".to_string();

        if f_type == "application/x-tar" || f_type == "application/tar" {
            // let tar = GzDecoder::new(file);

            index_document = match index_string.len() == 0 {
                true => "index.html".to_string(),
                false => index_string,
            };

            let content0: Vec<u8> = read_file(file)
                .await
                .into_iter() // Take ownership of the outer vector and its inner vectors
                .flat_map(|inner_vec| inner_vec.into_iter()) // Flatten by iterating over each inner_vec
                .collect();

            let mut archive = Archive::new(&content0[..]);

            for f0 in archive.entries().unwrap() {
                let mut f01 = match f0 {
                    Ok(aok) => aok,
                    _ => continue,
                };

                let entry_header0 = f01.header();
                let entry_type_file0 = entry_header0.entry_type().is_file();

                if entry_type_file0 {
                    let f02path = f01.path();

                    let f01path = match f02path {
                        Ok(mut aok) => aok.to_mut().clone(),
                        _ => continue,
                    };

                    let fname0 = match f01path.file_name() {
                        Some(aok) => match aok.to_os_string().into_string() {
                            Ok(aok0) => aok0,
                            _ => continue,
                        },
                        _ => continue,
                    };

                    let f0path = match f01path.into_os_string().into_string() {
                        Ok(aok) => aok.strip_prefix("./").unwrap_or(&aok).to_string(),
                        _ => continue,
                    };

                    let mime0 = match mime_guess::from_path(&f0path).first_raw() {
                        Some(aok) => match aok.to_string().starts_with("text/") {
                            true => aok.to_string() + "; charset=utf-8",
                            false => aok.to_string(),
                        },
                        _ => continue,
                    };

                    let mut data0: Vec<u8> = vec![];

                    let _ = f01.read_to_end(&mut data0);

                    fvec0.push(Resource {
                        path0: f0path,
                        filename0: fname0,
                        mime0: mime0,
                        data: vec![data0],
                        data_address: vec![],
                    })
                }
            }
        } else {
            fvec0.push(Resource {
                path0: f_name.clone(),
                filename0: f_name,
                mime0: f_type,
                data: read_file(file).await,
                data_address: vec![],
            });
        }

        let topic_safe = match hex::decode(&feed_topic) {
            Ok(_aok) => feed_topic,
            _ => hex::encode(keccak256(feed_topic)),
        };

        let _ = self.upload_port.0.try_send((
            fvec0,
            encryption,
            index_document,
            add_to_feed,
            topic_safe,
            chan_out,
        ));

        let result = chan_in.recv().await.unwrap_or_default();

        return encode_resources(
            vec![(
                format!(
                    "upload result: returned address displayed here: {}",
                    hex::encode(&result)
                )
                .as_bytes()
                .to_vec(),
                "text/plain".to_string(),
                "... result ...".to_string(),
            )],
            hex::encode(&result).to_string(),
        );
    }

    pub async fn post_push_chunk(
        &self,
        d: Vec<u8>,
        soc: bool,
        chunk_address: Vec<u8>,
        stamp: Vec<u8>,
    ) -> Vec<u8> {
        let (chan_out, chan_in) = mpsc::unbounded::<bool>();

        let chunk_address0 = chunk_address.clone();

        let _ = self
            .chunk_push_port
            .0
            .try_send((d, soc, chunk_address, stamp, chan_out));

        let result = chan_in.recv().await.unwrap_or(false);
        if result {
            let result_data = vec![(
                format!("Upload result: success").as_bytes().to_vec(),
                "text/plain".to_string(),
                "Upload result".to_string(),
            )];
            let result_hex = hex::encode(&chunk_address0);

            return encode_resources(result_data, result_hex);
        } else {
            let result_data = vec![(
                format!("Upload result: failure").as_bytes().to_vec(),
                "text/plain".to_string(),
                "... result ...".to_string(),
            )];
            let result_hex = hex::encode(&chunk_address0);

            return encode_resources(result_data, result_hex);
        }
    }

    pub async fn acquire(&self, address: String) -> Vec<u8> {
        if let Some(resource) = parse_bzz_resource(&address) {
            if !resource.path.is_empty() {
                if let Some(metadata) = self.resolve_bzz(address.clone()).await {
                    if metadata.size == 0 {
                        return encode_resources(
                            vec![(vec![], metadata.mime, metadata.path.clone())],
                            metadata.path,
                        );
                    }

                    if let Some((bytes, metadata)) = self
                        .acquire_resolved_range(metadata.clone(), 0, metadata.size - 1)
                        .await
                    {
                        return encode_resources(
                            vec![(bytes, metadata.mime, metadata.path.clone())],
                            metadata.path,
                        );
                    }
                }
            }
        }

        let (chan_out, chan_in) = mpsc::unbounded::<Vec<u8>>();
        let valaddr_0 = hex::decode(&address);
        let valaddr = match valaddr_0 {
            Ok(hex) => hex,
            _ => prt(address, "".to_string()).await,
        };

        let _ = self
            .message_port
            .0
            .try_send(chunk_retrieve_request(valaddr, chan_out));

        return chan_in.recv().await.unwrap_or_default();
    }

    pub async fn reset_stamp(&self) -> Vec<u8> {
        let batch_id = get_batch_id().await;

        reset_stamp(&hex::encode(&batch_id).to_string()).await;

        return encode_resources(
            vec![(
                format!("Stamp reset and ready to be reused. Uploads after this point will overwrite uploads from before this point.")
                    .as_bytes()
                    .to_vec(),
                "text/plain".to_string(),
                "... result ...".to_string(),
            )],
            "... result ...".to_string(),
        );
    }

    pub fn new(_st: String) -> Weeb3 {
        // tracing_wasm::set_as_global_default(); // uncomment to turn on tracing
        // init_panic_hook();

        // let body = Body::from_current_window()?;
        // body.append_p(&format!("Attempt to establish connection over websocket"))?;

        let secret_key_o = ecdsa::SecretKey::generate();
        let secret_key = secret_key_o.clone();
        let keypair: ecdsa::Keypair = secret_key_o.into();

        let swarm = libp2p::SwarmBuilder::with_existing_identity(keypair.clone().into())
            .with_wasm_bindgen()
            .with_other_transport(|_key| {
                let mut yamux_config = yamux::Config::default();
                yamux_config.set_max_num_streams(4096);

                websocket_websys::Transport::default()
                    .upgrade(core::upgrade::Version::V1)
                    .authenticate(noise::Config::new(&keypair.clone().into()).unwrap())
                    .multiplex(yamux_config)
                    .boxed()
            })
            .expect("Failed to create WebSocket transport")
            .with_behaviour(|key| Behaviour::new(key.public()))
            .unwrap()
            .with_swarm_config(|_| {
                libp2p::swarm::Config::with_wasm_executor()
                    .with_idle_connection_timeout(Duration::from_secs(36000000))
                    .with_dial_concurrency_factor(NonZero::new(32_u8).unwrap())
                    .with_substream_upgrade_protocol_override(core::upgrade::Version::V1Lazy)
                    .with_max_negotiating_inbound_streams(NonZero::new(10000_usize).unwrap().into())
                    .with_per_connection_event_buffer_size(10000_usize)
                    .with_notify_handler_buffer_size(NonZero::new(10000_usize).unwrap().into())
            })
            .build();

        let connected_peers: Arc<Mutex<HashMap<PeerId, PeerFile>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let overlay_peers: Arc<Mutex<HashMap<String, PeerId>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let bootnodes: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let accounting_peers: Arc<Mutex<HashMap<PeerId, Arc<Mutex<PeerAccounting>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let ongoing_refreshments: Arc<Mutex<HashMap<PeerId, u64>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let connection_attempts: Arc<Mutex<HashSet<PeerId>>> = Arc::new(Mutex::new(HashSet::new()));
        let known_peer_underlays: PeerAddrMap = Arc::new(Mutex::new(HashMap::new()));
        let ongoing_cheques: Arc<Mutex<HashMap<PeerId, u64>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let swap_beneficiaries: Arc<Mutex<HashMap<PeerId, (web3::types::Address, bool)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let self_ephemerals: PeerAddrMap = Arc::new(Mutex::new(HashMap::new()));
        let self_ephemeral_waiters: Arc<Mutex<HashMap<PeerId, Vec<mpsc::Sender<Multiaddr>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (m_out, m_in) = mpsc::unbounded::<ChunkRetrieveRequest>();
        let (resolve_out, resolve_in) = mpsc::unbounded::<BzzResolveRequest>();
        let (range_out, range_in) = mpsc::unbounded::<BzzRangeRequest>();

        let (log_port_out, log_port_in) = mpsc::unbounded::<String>();

        let (u_out, u_in) = mpsc::unbounded::<(
            Vec<Resource>,
            bool,
            String,
            bool,
            String,
            mpsc::Sender<Vec<u8>>,
        )>();
        let (b_out, b_in) = mpsc::unbounded::<(String, mpsc::Sender<String>, bool, u64)>();
        let (chunk_push_port_out, chunk_push_port_in) =
            mpsc::unbounded::<(Vec<u8>, bool, Vec<u8>, Vec<u8>, mpsc::Sender<bool>)>();

        return Weeb3 {
            secret_key: Arc::new(Mutex::new(secret_key)),
            swarm: Arc::new(Mutex::new(swarm)),
            wings: Mutex::new(Arc::new(Wings {
                connected_peers: connected_peers,
                overlay_peers: overlay_peers,
                bootnodes: bootnodes,
                accounting_peers: accounting_peers,
                ongoing_refreshments: ongoing_refreshments,
                ongoing_cheques: ongoing_cheques,
                swap_beneficiaries: swap_beneficiaries,
                connection_attempts: connection_attempts,
                known_peer_underlays: known_peer_underlays,
                self_ephemerals: self_ephemerals,
                self_ephemeral_waiters: self_ephemeral_waiters,
            })),
            log_port: (log_port_out, log_port_in),
            log_start_ms: Date::now(),
            message_port: (m_out, m_in),
            resolve_port: (resolve_out, resolve_in),
            range_port: (range_out, range_in),
            upload_port: (u_out, u_in),
            chunk_push_port: (chunk_push_port_out, chunk_push_port_in),
            bootnode_port: (b_out, b_in),
            network_id: Mutex::new(10_u64),
            transfer_paused: Arc::new(AtomicBool::new(false)),
            retrieve_cancel_generations: Arc::new(Mutex::new(HashMap::new())),
            connection_generation: Arc::new(Mutex::new(0_u64)),
            ongoing_connections: Arc::new(Mutex::new(0_u64)),
            connections: Arc::new(Mutex::new(0_u64)),
        };
    }

    pub async fn get_current_logs(&self) -> Vec<String> {
        let mut logs: Vec<String> = vec![];

        #[allow(irrefutable_let_patterns)]
        while let data0 = self.log_port.1.try_recv() {
            if !data0.is_err() {
                let log_message = data0.unwrap();
                logs.push(log_message);
            } else {
                break;
            }
        }

        return logs;
    }

    pub async fn get_ongoing_connections(&self) -> u64 {
        let ongoing = self.ongoing_connections.lock().await;
        return ongoing.clone();
    }

    pub async fn get_connections(&self) -> u64 {
        let connected = self.connections.lock().await;
        return connected.clone();
    }

    pub fn interface_log(&self, log0: String) {
        interface_log_to(&self.log_port.0, self.log_start_ms, log0);
    }

    pub async fn toggle_transfer_pause(&self) -> bool {
        let paused = !self.transfer_paused.load(Ordering::Relaxed);
        self.transfer_paused.store(paused, Ordering::Relaxed);
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

    pub async fn run(&self, _st: String) -> () {
        // init_panic_hook();
        let baddr = "/ip4/49.12.172.37/tcp/32532/tls/sni/49-12-172-37.k2k4r8pr3m3aug5nudg2y039qfj2gxw6wnlx0e0ghzxufcn38soyp9z4.libp2p.direct/ws/p2p/QmfSx1ujzboapD5h2CiqTJqUy46FeTDwXBszB3XUCfKEEj".to_string();
        let addr30 = match baddr.parse::<Multiaddr>() {
            Ok(aok) => aok,
            _ => return,
        };

        web_sys::console::log_1(&JsValue::from(format!(
            "F0: {:#?}",
            detect_underlay_format(&addr30)
        )));

        web_sys::console::log_1(&JsValue::from(format!(
            "F1: {:#?}",
            beewss_to_dns_transformed(&addr30)
        )));

        web_sys::console::log_1(&JsValue::from(format!(
            "F2: {:#?}",
            detect_underlay_format(&beewss_to_dns_transformed(&addr30))
        )));

        let wings = self.wings.lock().await;

        let (peers_instructions_chan_outgoing, peers_instructions_chan_incoming) =
            mpsc::unbounded();
        let (connections_instructions_chan_outgoing, connections_instructions_chan_incoming) =
            mpsc::unbounded::<(etiquette_2::BzzAddress, bool, u64)>();

        let (accounting_peer_chan_outgoing, accounting_peer_chan_incoming) =
            mpsc::unbounded::<PeerFile>();

        let (pricing_chan_outgoing, pricing_chan_incoming) = mpsc::unbounded::<(PeerId, u64)>();

        let (refreshment_instructions_chan_outgoing, refreshment_instructions_chan_incoming) =
            mpsc::unbounded::<(PeerId, u64)>();

        let (refreshment_chan_outgoing, refreshment_chan_incoming) =
            mpsc::unbounded::<(PeerId, u64, u64)>();

        let (data_retrieve_chan_outgoing, data_retrieve_chan_incoming) =
            mpsc::unbounded::<(Vec<u8>, mpsc::Sender<Vec<u8>>)>();

        let (chunk_retrieve_chan_outgoing, chunk_retrieve_chan_incoming) =
            mpsc::unbounded::<ChunkRetrieveRequest>();

        let (data_upload_chan_outgoing, data_upload_chan_incoming) =
            mpsc::unbounded::<(Vec<Vec<u8>>, u8, Vec<u8>, Vec<u8>, mpsc::Sender<Vec<u8>>)>();

        let (chunk_upload_chan_outgoing, chunk_upload_chan_incoming) =
            mpsc::unbounded::<(Vec<u8>, bool, Vec<u8>, Vec<u8>, mpsc::Sender<bool>)>();

        let (cheque_instructions_chan_outgoing, cheque_instructions_chan_incoming) =
            mpsc::unbounded::<(PeerId, u64)>();
        let (cheque_send_chan_outgoing, cheque_send_chan_incoming) =
            mpsc::unbounded::<(PeerId, bool)>();

        let mut ctrl0;
        let mut ctrl1;
        let ctrl3;
        let ctrl4;
        let ctrl5;
        let ctrl6;
        let ctrl8;
        let mut incoming_pricing_streams;
        let mut incoming_gossip_streams;

        let swarm0 = self.swarm.clone();

        {
            let mut swarm = swarm0.lock().await;
            ctrl0 = swarm.behaviour_mut().stream.new_control();
            ctrl1 = swarm.behaviour_mut().stream.new_control();
            ctrl3 = swarm.behaviour_mut().stream.new_control();
            ctrl4 = swarm.behaviour_mut().stream.new_control();
            ctrl5 = swarm.behaviour_mut().stream.new_control();
            ctrl6 = swarm.behaviour_mut().stream.new_control();
            ctrl8 = swarm.behaviour_mut().stream.new_control();
        }

        incoming_pricing_streams = ctrl0.accept(PRICING_PROTOCOL).unwrap();
        incoming_gossip_streams = ctrl1.accept(GOSSIP_PROTOCOL).unwrap();

        let pricing_inbound_handle = async move {
            while let Some((peer, stream)) = incoming_pricing_streams.next().await {
                let pricing_chan_outgoing = pricing_chan_outgoing.clone();
                spawn_local(async move {
                    pricing_handler(peer, stream, &pricing_chan_outgoing).await;
                });
                async_std::task::yield_now().await;
            }
        };

        let gossip_peers_instructions_chan_outgoing = peers_instructions_chan_outgoing.clone();
        let gossip_inbound_handle = async move {
            while let Some((peer, stream)) = incoming_gossip_streams.next().await {
                let peers_instructions_chan_outgoing =
                    gossip_peers_instructions_chan_outgoing.clone();
                spawn_local(async move {
                    gossip_handler(peer, stream, &peers_instructions_chan_outgoing).await;
                });
                async_std::task::yield_now().await;
            }
        };

        let swarm_event_handle_0 = async {
            loop {
                let mut paddr = match peers_instructions_chan_incoming.recv().await {
                    Ok(paddr) => paddr,
                    Err(_) => break,
                };

                loop {
                    let paddr_current = paddr.clone();
                    let wings = wings.clone();
                    let swarm = self.swarm.clone();
                    let connections_instructions_chan_outgoing =
                        connections_instructions_chan_outgoing.clone();
                    let connection_generation = self.connection_generation.clone();

                    spawn_local(async move {
                        let und_addrs = deserialize_underlays(&paddr_current.clone().underlay);

                        for addr3 in und_addrs.iter() {
                            web_sys::console::log_1(&JsValue::from(format!(
                                "Current Conn Handled {:#?}",
                                addr3.to_string()
                            )));

                            let dial_addr = match detect_underlay_format(&addr3) {
                                UnderlayFormat::BeeWss => beewss_to_dns_transformed(&addr3),
                                UnderlayFormat::DNSTransformedWss => addr3.clone(),
                                UnderlayFormat::Other => continue,
                            };

                            {
                                let pid: PeerId = match try_from_multiaddr(&addr3.clone()) {
                                    Some(aok) => {
                                        let connected_peers_map =
                                            wings.connected_peers.lock().await;
                                        if connected_peers_map.contains_key(&aok) {
                                            continue;
                                        }
                                        let mut connection_attempts_map =
                                            wings.connection_attempts.lock().await;
                                        if connection_attempts_map.contains(&aok) {
                                            continue;
                                        } else {
                                            connection_attempts_map.insert(aok);
                                        }
                                        aok
                                    }
                                    _ => {
                                        continue;
                                    }
                                };

                                let mut paddr5 = paddr_current.clone();
                                {
                                    let mut known = wings.known_peer_underlays.lock().await;
                                    known.insert(pid.clone(), dial_addr.clone());
                                }
                                let dial_result = {
                                    let mut swarm = swarm.lock_arc().await;
                                    web_sys::console::log_1(&JsValue::from(format!("dial 0",)));
                                    swarm.dial(dial_addr.clone())
                                };

                                if let Err(error) = dial_result {
                                    timed_log(format!(
                                        "Dial failed immediately peer={} address={} error={:?}",
                                        pid, dial_addr, error
                                    ));
                                    let mut connection_attempts_map =
                                        wings.connection_attempts.lock().await;
                                    connection_attempts_map.remove(&pid);
                                    continue;
                                }

                                {
                                    paddr5.underlay = dial_addr.to_vec();
                                }

                                let _ = connections_instructions_chan_outgoing.try_send((
                                    paddr5.clone(),
                                    false,
                                    *connection_generation.lock().await,
                                ));
                            }
                        }

                        let addr4 =
                            match libp2p::core::Multiaddr::try_from(paddr_current.clone().underlay)
                            {
                                Ok(aok) => aok,
                                _ => {
                                    return;
                                }
                            };

                        let dial_addr = match detect_underlay_format(&addr4) {
                            UnderlayFormat::BeeWss => beewss_to_dns_transformed(&addr4),
                            UnderlayFormat::DNSTransformedWss => addr4.clone(),
                            UnderlayFormat::Other => return,
                        };

                        {
                            let pid: PeerId = match try_from_multiaddr(&addr4.clone()) {
                                Some(aok) => {
                                    let connected_peers_map = wings.connected_peers.lock().await;
                                    if connected_peers_map.contains_key(&aok) {
                                        return;
                                    }
                                    let mut connection_attempts_map =
                                        wings.connection_attempts.lock().await;
                                    if connection_attempts_map.contains(&aok) {
                                        return;
                                    } else {
                                        connection_attempts_map.insert(aok);
                                    }
                                    aok
                                }
                                _ => {
                                    return;
                                }
                            };

                            let mut paddr5 = paddr_current.clone();
                            {
                                let mut known = wings.known_peer_underlays.lock().await;
                                known.insert(pid.clone(), dial_addr.clone());
                            }
                            let dial_result = {
                                let mut swarm = swarm.lock_arc().await;
                                web_sys::console::log_1(&JsValue::from(format!("dial 1",)));
                                swarm.dial(dial_addr.clone())
                            };

                            if let Err(error) = dial_result {
                                timed_log(format!(
                                    "Dial failed immediately peer={} address={} error={:?}",
                                    pid, dial_addr, error
                                ));
                                let mut connection_attempts_map =
                                    wings.connection_attempts.lock().await;
                                connection_attempts_map.remove(&pid);
                                return;
                            }

                            {
                                paddr5.underlay = dial_addr.to_vec();
                            }

                            let _ = connections_instructions_chan_outgoing.try_send((
                                paddr5.clone(),
                                false,
                                *connection_generation.lock().await,
                            ));
                        }
                    });

                    match peers_instructions_chan_incoming.try_recv() {
                        Ok(next) => paddr = next,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let swarm_event_handle_1 = async {
            loop {
                let event = {
                    let mut swarm = self.swarm.lock_arc().await;
                    async_std::future::timeout(
                        Duration::from_millis(STARTUP_QUEUE_POLL_MS),
                        swarm.next(),
                    )
                    .await
                };

                let event = match event {
                    Ok(Some(event)) => event,
                    Ok(None) | Err(_) => {
                        async_std::task::yield_now().await;
                        continue;
                    }
                };

                let wings = wings.clone();
                let swarm = self.swarm.clone();
                let peers_instructions_chan_outgoing = peers_instructions_chan_outgoing.clone();
                let connection_generation = self.connection_generation.clone();
                let connections = self.connections.clone();
                let ongoing_connections = self.ongoing_connections.clone();
                let log_port = self.log_port.0.clone();
                let log_start_ms = self.log_start_ms;

                spawn_local(async move {
                    let interface_log = |log0: String| {
                        interface_log_to(&log_port, log_start_ms, log0);
                    };

                    match event {
                        SwarmEvent::Behaviour(out_event) => {
                            if let BehaviourEvent::Identify(identify_event) = out_event {
                                if let identify::Event::Received { peer_id, info, .. } =
                                    identify_event
                                {
                                    let observed_addr: Multiaddr = info.observed_addr.clone();

                                    {
                                        let mut swarm = swarm.lock_arc().await;
                                        swarm.add_external_address(observed_addr.clone());
                                        swarm
                                            .behaviour_mut()
                                            .identify
                                            .push(std::iter::once(peer_id.clone()));
                                    }

                                    {
                                        let mut map = wings.self_ephemerals.lock().await;
                                        map.insert(peer_id.clone(), observed_addr.clone());
                                    }

                                    let waiters = {
                                        let mut waiters = wings.self_ephemeral_waiters.lock().await;
                                        waiters.remove(&peer_id).unwrap_or_default()
                                    };
                                    for waiter in waiters {
                                        let _ = waiter.try_send(observed_addr.clone());
                                    }

                                    interface_log(format!(
                                        "Observed address {} for peer {} stored/overwritten",
                                        observed_addr, peer_id
                                    ));
                                }
                            }
                        }
                        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                            let retry_address = match &error {
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

                            let peer_to_clear = peer_id.or_else(|| {
                                retry_address
                                    .as_ref()
                                    .and_then(|address| try_from_multiaddr(address))
                            });

                            let mut should_retry = false;

                            if let Some(peer_id) = peer_to_clear {
                                let had_attempt = {
                                    let mut connection_attempts_map =
                                        wings.connection_attempts.lock().await;
                                    connection_attempts_map.remove(&peer_id)
                                };

                                should_retry = had_attempt;

                                if had_attempt {
                                    let mut ongoing = ongoing_connections.lock().await;
                                    if *ongoing > 0 {
                                        *ongoing -= 1;
                                    }
                                }
                            }

                            if should_retry {
                                if let Some(address) = retry_address {
                                    let retry_generation = *connection_generation.lock().await;
                                    async_std::task::sleep(Duration::from_millis(
                                        HANDSHAKE_RETRY_DELAY_MS,
                                    ))
                                    .await;
                                    if *connection_generation.lock().await == retry_generation {
                                        let mut bzzaddr = etiquette_2::BzzAddress::default();
                                        bzzaddr.underlay = address.to_vec();
                                        let _ = peers_instructions_chan_outgoing.try_send(bzzaddr);
                                    }
                                }
                            } else if let Some(address) = retry_address {
                                timed_log(format!(
                                    "Skipping retry for untracked outgoing dial error address={}",
                                    address
                                ));
                            }
                        }
                        SwarmEvent::ConnectionClosed {
                            peer_id, endpoint, ..
                        } => {
                            let removed_overlay = {
                                let mut connected_peers_map = wings.connected_peers.lock().await;
                                connected_peers_map
                                    .remove(&peer_id)
                                    .map(|peer_file| hex::encode(peer_file.overlay))
                            };
                            let was_tracked_peer = removed_overlay.is_some();

                            if let Some(ol0) = removed_overlay {
                                let promoted_peer = {
                                    let mut overlay_peers_map = wings.overlay_peers.lock().await;
                                    overlay_peers_map.remove(&ol0).is_some()
                                };

                                if promoted_peer {
                                    let mut connections = connections.lock().await;
                                    if *connections > 0 {
                                        *connections -= 1;
                                    }
                                } else {
                                    let mut ongoing = ongoing_connections.lock().await;
                                    if *ongoing > 0 {
                                        *ongoing -= 1;
                                    }
                                }

                                interface_log(format!(
                                    "Disconnected from peer {} {:#?}",
                                    &ol0, endpoint
                                ));
                            }

                            let had_attempt = {
                                let mut connection_attempts_map =
                                    wings.connection_attempts.lock().await;
                                connection_attempts_map.remove(&peer_id)
                            };

                            {
                                let mut map = wings.self_ephemerals.lock().await;
                                map.remove(&peer_id);
                            }

                            let retry_address = match &endpoint {
                                libp2p::core::ConnectedPoint::Dialer { address, .. } => {
                                    Some(address.clone())
                                }
                                _ => {
                                    let known = wings.known_peer_underlays.lock().await;
                                    known.get(&peer_id).cloned()
                                }
                            };

                            if let Some(address) = retry_address {
                                if was_tracked_peer || had_attempt {
                                    let retry_generation = *connection_generation.lock().await;
                                    async_std::task::sleep(Duration::from_millis(
                                        HANDSHAKE_RETRY_DELAY_MS,
                                    ))
                                    .await;
                                    if *connection_generation.lock().await != retry_generation {
                                        let mut accounting = wings.accounting_peers.lock().await;
                                        accounting.remove(&peer_id);
                                        return;
                                    }
                                    let mut bzzaddr = etiquette_2::BzzAddress::default();
                                    bzzaddr.underlay = address.to_vec();
                                    let _ = peers_instructions_chan_outgoing.try_send(bzzaddr);
                                    interface_log(format!(
                                        "Queued reconnect for peer {} {}",
                                        peer_id, address
                                    ));
                                } else {
                                    timed_log(format!(
                                        "Skipping retry for untracked closed peer={} address={}",
                                        peer_id, address
                                    ));
                                }
                            } else {
                                timed_log(format!(
                                    "No known reconnect address for closed peer={}",
                                    peer_id
                                ));
                            }

                            let mut accounting = wings.accounting_peers.lock().await;
                            accounting.remove(&peer_id);
                        }
                        _ => {}
                    }
                });

                async_std::task::yield_now().await;
            }
        };

        let swarm_event_handle_2 = async {
            loop {
                let mut bootnode_change = match self.bootnode_port.1.recv().await {
                    Ok(bootnode_change) => bootnode_change,
                    Err(_) => break,
                };

                loop {
                    let (baddr, chan, usable, request_generation) = bootnode_change;
                    let swarm = self.swarm.clone();
                    let wings = wings.clone();
                    let connections_instructions_chan_outgoing =
                        connections_instructions_chan_outgoing.clone();
                    let connection_generation = self.connection_generation.clone();

                    spawn_local(async move {
                        if *connection_generation.lock().await != request_generation {
                            let _ = chan.try_send("stale bootnode connect skipped".to_string());
                            return;
                        }

                        let addr33 = match baddr.parse::<Multiaddr>() {
                            Ok(aok) => aok,
                            _ => {
                                let _ = chan
                                    .try_send("parse multiaddress for bootnode failed".to_string());
                                return;
                            }
                        };

                        // let bn_id: PeerId = match try_from_multiaddr(&addr33.clone()) {
                        //     Some(aok) => aok,
                        //     _ => {
                        //         let _ = chan.try_send("parse peerid for bootnode failed".to_string());
                        //         break;
                        //     }
                        // };
                        let pid: PeerId = match try_from_multiaddr(&addr33.clone()) {
                            Some(aok) => {
                                let mut connection_attempts_map =
                                    wings.connection_attempts.lock().await;
                                if connection_attempts_map.remove(&aok) {
                                    timed_log(format!(
                                        "Refreshing bootnode dial for peer {} already marked as attempting",
                                        aok
                                    ));
                                }
                                connection_attempts_map.insert(aok);
                                aok
                            }
                            _ => {
                                let _ =
                                    chan.try_send("parse peerid for bootnode failed".to_string());
                                return;
                            }
                        };
                        if *connection_generation.lock().await != request_generation {
                            let mut connection_attempts_map =
                                wings.connection_attempts.lock().await;
                            connection_attempts_map.remove(&pid);
                            let _ = chan.try_send("stale bootnode connect skipped".to_string());
                            return;
                        }
                        let dial_addr = if detect_underlay_format(&addr33) == UnderlayFormat::BeeWss
                        {
                            beewss_to_dns_transformed(&addr33)
                        } else {
                            addr33.clone()
                        };
                        {
                            let mut known = wings.known_peer_underlays.lock().await;
                            known.insert(pid.clone(), dial_addr.clone());
                        }
                        let dial_result = {
                            let mut swarm = swarm.lock_arc().await;
                            web_sys::console::log_1(&JsValue::from(format!(
                                "dial 2 :: {:#?}",
                                addr33
                            )));
                            swarm.dial(dial_addr.clone())
                        };

                        if let Err(error) = dial_result {
                            timed_log(format!(
                                "Bootnode dial failed immediately peer={} address={} error={:?}",
                                pid, dial_addr, error
                            ));
                            let mut connection_attempts_map =
                                wings.connection_attempts.lock().await;
                            connection_attempts_map.remove(&pid);
                            let _ = chan.try_send(format!("bootnode dial failed: {:?}", error));
                            return;
                        }

                        let _ = chan.try_send("dialing bootnode".to_string());

                        let mut bzzaddr = etiquette_2::BzzAddress::default();
                        bzzaddr.underlay = dial_addr.to_vec();

                        let _ = connections_instructions_chan_outgoing.try_send((
                            bzzaddr,
                            !usable,
                            request_generation,
                        ));
                    });

                    match self.bootnode_port.1.try_recv() {
                        Ok(change) => bootnode_change = change,
                        Err(_) => break,
                    }
                }

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
                    // Accounting connect
                    let peer = peer_file.peer_id.clone();
                    let accounting_peer = {
                        let mut accounting = wings.accounting_peers.lock().await;
                        if let Some(accounting_peer_lock) =
                            accounting.get(&peer_file.peer_id).cloned()
                        {
                            Some(accounting_peer_lock)
                        } else {
                            accounting.insert(
                                peer_file.peer_id.clone(),
                                Arc::new(Mutex::new(PeerAccounting {
                                    balance: 0,
                                    surplus_balance: 0,
                                    threshold: 0,
                                    payment_threshold: 0,
                                    reserve: 0,
                                    refreshment: 0.0,
                                    id: peer_file.peer_id.clone(),
                                })),
                            );
                            None
                        }
                    };

                    let threshold_ready = if let Some(accounting_peer_lock) = accounting_peer {
                        let accounting_peer = accounting_peer_lock.lock().await;
                        accounting_peer.threshold > 0
                    } else {
                        false
                    };

                    {
                        let mut connected_peers_map = wings.connected_peers.lock().await;
                        connected_peers_map.insert(peer_file.peer_id.clone(), peer_file.clone());
                    }
                    {
                        let mut swap_beneficiaries_map = wings.swap_beneficiaries.lock().await;

                        swap_beneficiaries_map
                            .insert(peer_file.peer_id, (peer_file.beneficiary, false));
                    }

                    if threshold_ready {
                        self.promote_priced_peer(&wings, peer).await;
                    } else {
                        let mut ongoing = self.ongoing_connections.lock().await;
                        *ongoing = *ongoing + 1;
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
                    let (peer, amount) = pricing;
                    let accounting_peer_lock = {
                        let mut accounting = wings.accounting_peers.lock().await;
                        if let Some(accounting_peer_lock) = accounting.get(&peer).cloned() {
                            accounting_peer_lock
                        } else {
                            let accounting_peer_lock = Arc::new(Mutex::new(PeerAccounting {
                                balance: 0,
                                surplus_balance: 0,
                                threshold: 0,
                                payment_threshold: 0,
                                reserve: 0,
                                refreshment: 0.0,
                                id: peer.clone(),
                            }));
                            accounting.insert(peer.clone(), accounting_peer_lock.clone());
                            accounting_peer_lock
                        }
                    };

                    set_payment_threshold(&accounting_peer_lock, amount).await;

                    self.promote_priced_peer(&wings, peer).await;

                    match pricing_chan_incoming.try_recv() {
                        Ok(next) => pricing = next,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let cheques_active_cache = Arc::new(Mutex::new(None::<bool>));
        let refreshment_generations = Arc::new(Mutex::new(0_u64));

        let refreshment_instruction_handle = async {
            loop {
                let (peer, _amount) = match refreshment_instructions_chan_incoming.recv().await {
                    Ok(instruction) => instruction,
                    Err(_) => break,
                };

                let wings0 = wings.clone();
                let ctrl7 = ctrl4.clone();
                let refresh_chan = refreshment_chan_outgoing.clone();
                let cheque_chan = cheque_instructions_chan_outgoing.clone();
                let cheques_active_cache = cheques_active_cache.clone();
                let refreshment_generations = refreshment_generations.clone();

                spawn_local(async move {
                    let cheques_are_active = {
                        let mut cache = cheques_active_cache.lock().await;
                        match *cache {
                            Some(active) => active,
                            None => {
                                let active = cheques_active().await;
                                *cache = Some(active);
                                active
                            }
                        }
                    };

                    let accounting_peer = {
                        let accounting = wings0.accounting_peers.lock().await;
                        accounting.get(&peer).cloned()
                    };

                    let Some(accounting_peer_lock) = accounting_peer else {
                        return;
                    };

                    let (balance, last_refreshment) = {
                        let accounting_peer = accounting_peer_lock.lock().await;
                        (accounting_peer.balance, accounting_peer.refreshment)
                    };

                    if balance <= REFRESH_RATE {
                        return;
                    }

                    {
                        let mut map = wings0.ongoing_refreshments.lock().await;
                        if map.contains_key(&peer) {
                            return;
                        }
                        let refresh_generation = {
                            let mut generations = refreshment_generations.lock().await;
                            *generations = generations.wrapping_add(1);
                            *generations
                        };
                        map.insert(peer, refresh_generation);
                    }

                    if cheques_are_active {
                        let cheque_amt = balance - REFRESH_RATE;
                        if cheque_amt > 0 {
                            let should_issue = {
                                let mut map = wings0.ongoing_cheques.lock().await;
                                if map.contains_key(&peer) {
                                    false
                                } else {
                                    map.insert(peer, cheque_amt);
                                    true
                                }
                            };

                            if should_issue {
                                spawn_local(async move {
                                    let _ = cheque_chan.try_send((peer, cheque_amt));
                                });
                            }
                        }
                    }

                    let now = Date::now();
                    let elapsed = now - last_refreshment;
                    if elapsed < 1000.0 {
                        async_std::task::sleep(Duration::from_millis((1000.0 - elapsed) as u64))
                            .await;
                    }

                    let (refresh_done_out, refresh_done_in) = mpsc::unbounded::<()>();
                    let (refresh_result_out, refresh_result_in) =
                        mpsc::unbounded::<(PeerId, u64)>();
                    let refresh_chan0 = refresh_chan.clone();
                    let refresh_peer = peer.clone();
                    let refresh_generation = {
                        let map = wings0.ongoing_refreshments.lock().await;
                        map.get(&peer).copied()
                    };
                    let Some(refresh_generation) = refresh_generation else {
                        return;
                    };
                    spawn_local(async move {
                        refresh_handler(
                            refresh_peer,
                            REFRESH_RATE * 100,
                            ctrl7,
                            &refresh_result_out,
                        )
                        .await;
                        while let Ok((peer, amount)) = refresh_result_in.try_recv() {
                            let _ = refresh_chan0.try_send((peer, amount, refresh_generation));
                        }
                        let _ = refresh_done_out.try_send(());
                    });

                    if async_std::future::timeout(Duration::from_secs(15), refresh_done_in.recv())
                        .await
                        .is_err()
                    {
                        let mut map = wings0.ongoing_refreshments.lock().await;
                        if map.get(&peer).copied() == Some(refresh_generation) {
                            map.remove(&peer);
                        }
                    }
                });
            }
        };

        let refreshment_apply_handle = async {
            loop {
                let mut refreshment = match refreshment_chan_incoming.recv().await {
                    Ok(refreshment) => refreshment,
                    Err(_) => break,
                };

                loop {
                    let (peer, amount, refresh_generation) = refreshment;

                    if amount > 0 {
                        self.interface_log(format!("Applied refreshment {}", amount));
                        let accounting_peer = {
                            let accounting = wings.accounting_peers.lock().await;
                            accounting.get(&peer).cloned()
                        };
                        if let Some(accounting_peer_lock) = accounting_peer {
                            if let Some((peer, surplus_growth, surplus_balance)) =
                                apply_refreshment(&accounting_peer_lock, amount).await
                            {
                                self.interface_log(format!(
                                    "Surplus balance increased for peer {} by {} to {}",
                                    peer, surplus_growth, surplus_balance
                                ));
                            }
                            let mut accounting_peer = accounting_peer_lock.lock().await;
                            accounting_peer.refreshment = Date::now();
                        }
                    } else {
                        self.interface_log(format!("Refreshment attempt cleared {}", amount));
                    }
                    let mut map = wings.ongoing_refreshments.lock().await;
                    if map.get(&peer).copied() == Some(refresh_generation) {
                        map.remove(&peer);
                    }

                    match refreshment_chan_incoming.try_recv() {
                        Ok(next) => refreshment = next,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let swap_price = Arc::new(Mutex::new(U256::from(0)));
        let swap_deduction = Arc::new(Mutex::new(U256::from(0)));

        let cheque_instruction_handle = async {
            loop {
                let mut cheque_instruction = match cheque_instructions_chan_incoming.recv().await {
                    Ok(instruction) => instruction,
                    Err(_) => break,
                };
                let mut cheque_joiner = Vec::new();

                loop {
                    let swap_price_0 = swap_price.clone();
                    let swap_deduction_0 = swap_deduction.clone();
                    let set_price = {
                        let price = swap_price_0.lock().await;
                        price.is_zero()
                    };

                    if set_price {
                        let (oracle_price, cheque_deduction) = get_price_from_oracle().await;

                        let mut price = swap_price_0.lock().await;
                        if price.is_zero() {
                            *price = oracle_price;
                        }

                        let mut deduction = swap_deduction_0.lock().await;
                        if deduction.is_zero() {
                            *deduction = cheque_deduction;
                        }
                    }

                    let (peer, amount) = cheque_instruction;
                    let ctrl_swap = ctrl5.clone();
                    let cheque_chan = cheque_send_chan_outgoing.clone();
                    let peers_for_cheque = wings.swap_beneficiaries.clone();
                    let handle = async move {
                        let price: U256 = {
                            let current_price = swap_price_0.lock().await;
                            *current_price
                        };

                        let deduction: U256 = {
                            let current_deduction = swap_deduction_0.lock().await;
                            *current_deduction
                        };

                        issue_handler(
                            peer,
                            amount,
                            ctrl_swap,
                            &cheque_chan,
                            peers_for_cheque,
                            price,
                            deduction,
                        )
                        .await;
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
                    let (peer, ok) = cheque_result;
                    let amt_opt = {
                        let mut map = wings.ongoing_cheques.lock().await;
                        map.remove(&peer)
                    };
                    if ok {
                        if let Some(amount) = amt_opt {
                            let accounting_peer = {
                                let accounting = wings.accounting_peers.lock().await;
                                accounting.get(&peer).cloned()
                            };
                            if let Some(accounting_peer_lock) = accounting_peer {
                                if let Some((peer, surplus_growth, surplus_balance)) =
                                    apply_refreshment(&accounting_peer_lock, amount).await
                                {
                                    self.interface_log(format!(
                                        "Surplus balance increased for peer {} by {} to {}",
                                        peer, surplus_growth, surplus_balance
                                    ));
                                }
                            }
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

        let retrieve_handle = async {
            loop {
                let mut incoming_request = match self.message_port.1.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };
                let mut dispatched = 0usize;

                loop {
                    let request = incoming_request;
                    let n = request.address;
                    let chan = request.chan;
                    let data_retrieve_chan = data_retrieve_chan_outgoing.clone();
                    let chunk_retrieve_chan = chunk_retrieve_chan_outgoing.clone();

                    dispatched += 1;

                    spawn_local(async move {
                        let _ = chan.try_send(
                            retrieve_resource(&n, &data_retrieve_chan, &chunk_retrieve_chan).await,
                        );
                    });

                    match self.message_port.1.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                if dispatched > 0 {
                    async_std::task::yield_now().await;
                }
            }
        };

        let resolve_bzz_handle = async {
            loop {
                let mut incoming_request = match self.resolve_port.1.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };
                let mut dispatched = 0usize;

                loop {
                    let (resource, chan) = incoming_request;
                    let data_retrieve_chan = data_retrieve_chan_outgoing.clone();
                    let chunk_retrieve_chan = chunk_retrieve_chan_outgoing.clone();

                    dispatched += 1;

                    spawn_local(async move {
                        let resolved = bzz_stream::resolve_bzz(
                            &resource,
                            &data_retrieve_chan,
                            &chunk_retrieve_chan,
                        )
                        .await;
                        let _ = chan.try_send(resolved);
                    });

                    match self.resolve_port.1.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                if dispatched > 0 {
                    async_std::task::yield_now().await;
                }
            }
        };

        let acquire_range_handle = async {
            loop {
                let mut incoming_request = match self.range_port.1.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };
                let mut dispatched = 0usize;

                loop {
                    let request = incoming_request;
                    let data_retrieve_chan = data_retrieve_chan_outgoing.clone();
                    let chunk_retrieve_chan = chunk_retrieve_chan_outgoing.clone();
                    let retrieve_cancel_generations = self.retrieve_cancel_generations.clone();

                    dispatched += 1;

                    spawn_local(async move {
                        match request {
                            BzzRangeRequest::Resource {
                                resource,
                                start,
                                end_inclusive,
                                cancel,
                                chan,
                            } => {
                                register_retrieve_cancel_token(
                                    &retrieve_cancel_generations,
                                    &cancel,
                                )
                                .await;
                                let data = if cancel.is_some() {
                                    bzz_stream::acquire_range_cancellable(
                                        &resource,
                                        start,
                                        end_inclusive,
                                        &data_retrieve_chan,
                                        &chunk_retrieve_chan,
                                        cancel,
                                        retrieve_cancel_generations,
                                    )
                                    .await
                                } else {
                                    bzz_stream::acquire_range(
                                        &resource,
                                        start,
                                        end_inclusive,
                                        &data_retrieve_chan,
                                        &chunk_retrieve_chan,
                                    )
                                    .await
                                };
                                let _ = chan.try_send(data);
                            }
                            BzzRangeRequest::Resolved {
                                metadata,
                                start,
                                end_inclusive,
                                cancel,
                                chan,
                            } => {
                                register_retrieve_cancel_token(
                                    &retrieve_cancel_generations,
                                    &cancel,
                                )
                                .await;
                                let data = if cancel.is_some() {
                                    bzz_stream::acquire_resolved_range_cancellable(
                                        metadata,
                                        start,
                                        end_inclusive,
                                        &chunk_retrieve_chan,
                                        cancel,
                                        Some(retrieve_cancel_generations),
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
                            }
                            BzzRangeRequest::Prepare { metadata, chan } => {
                                let prepared =
                                    bzz_stream::prepare_bzz_stream(metadata, &chunk_retrieve_chan)
                                        .await;
                                let _ = chan.try_send(prepared);
                            }
                        }
                    });

                    match self.range_port.1.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                if dispatched > 0 {
                    async_std::task::yield_now().await;
                }
            }
        };

        let push_handle = async {
            loop {
                let mut incoming_request = match self.upload_port.1.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };

                loop {
                    let (file0, enc, index, feed, topic, chan) = incoming_request;

                    let batch_owner = get_batch_owner_key().await;
                    let batch_id = get_batch_id().await;

                    if batch_owner.len() == 0 {
                        self.interface_log("No batch found for uploads".to_string());

                        let _ = chan.try_send(vec![]);
                    } else if batch_id.len() == 0 {
                        self.interface_log("No batchId found for uploads".to_string());

                        let _ = chan.try_send(vec![]);
                    } else {
                        let push_reference = upload_resource(
                            file0,
                            enc,
                            index,
                            "404.html".to_string(),
                            feed,
                            topic,
                            batch_owner.clone(),
                            batch_id.clone(),
                            &data_upload_chan_outgoing.clone(),
                            &chunk_upload_chan_outgoing.clone(),
                            &chunk_retrieve_chan_outgoing.clone(),
                        )
                        .await;
                        web_sys::console::log_1(&JsValue::from(format!(
                            "Writing response to interface push request"
                        )));

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

        let push_chunk_port_handle = async {
            loop {
                let mut incoming = match self.chunk_push_port.1.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };

                loop {
                    let (d, soc, chunk_address, stamp, feedback) = incoming;

                    let _ = chunk_upload_chan_outgoing.try_send((
                        d,
                        soc,
                        chunk_address,
                        stamp,
                        feedback,
                    ));

                    match self.chunk_push_port.1.try_recv() {
                        Ok(request) => incoming = request,
                        Err(_) => break,
                    }
                }

                async_std::task::yield_now().await;
            }
        };

        let retrieve_data_handle = async {
            loop {
                let mut incoming_request = match data_retrieve_chan_incoming.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };
                let mut dispatched = 0usize;

                loop {
                    let (n, chan) = incoming_request;
                    let chunk_retrieve_chan_outgoing = chunk_retrieve_chan_outgoing.clone();

                    dispatched += 1;

                    spawn_local(async move {
                        let retrieved_data =
                            retrieve_data(&n, &chunk_retrieve_chan_outgoing.clone()).await;
                        let _ = chan.try_send(retrieved_data);
                    });

                    match data_retrieve_chan_incoming.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                if dispatched > 0 {
                    async_std::task::yield_now().await;
                }
            }
        };

        let push_data_handle = async {
            loop {
                let mut incoming_request = match data_upload_chan_incoming.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };
                let mut request_joiner = Vec::new();

                loop {
                    let chunk_upload_chan = chunk_upload_chan_outgoing.clone();
                    let handle = async move {
                        let (n, mode, batch_owner, batch_id, chan) = incoming_request;

                        let encrypted_data = match mode {
                            0 => false,
                            _ => true,
                        };

                        let batch_bucket_limit = get_batch_bucket_limit().await;

                        let data_reference = push_data(
                            n,
                            encrypted_data,
                            batch_owner,
                            batch_id,
                            batch_bucket_limit,
                            &chunk_upload_chan,
                        )
                        .await;

                        let _ = chan.try_send(data_reference);
                    };
                    request_joiner.push(handle);

                    match data_upload_chan_incoming.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                join_all(request_joiner).await;
                async_std::task::yield_now().await;
            }
        };

        let push_chunk_handle = async {
            let push_sem = Arc::new(Semaphore::new(PUSH_CHUNK_CONCURRENCY));

            loop {
                let mut incoming_request = match chunk_upload_chan_incoming.recv().await {
                    Ok(request) => request,
                    Err(_) => break,
                };

                loop {
                    wait_transfer_unpaused(&self.transfer_paused).await;

                    let Some(permit) = push_sem.try_acquire_arc() else {
                        async_std::task::sleep(Duration::from_millis(PUSH_CHUNK_QUEUE_BACKOFF_MS))
                            .await;
                        let _ = chunk_upload_chan_outgoing.try_send(incoming_request);
                        break;
                    };

                    let (d, soc, checkad, stamp, feedback) = incoming_request;

                    let ctrl8 = ctrl8.clone();
                    let overlay_peers = wings.overlay_peers.clone();
                    let accounting_peers = wings.accounting_peers.clone();
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
                                soc.clone(),
                                checkad.clone(),
                                stamp.clone(),
                                ctrl8.clone(),
                                &overlay_peers,
                                &accounting_peers,
                                &refreshment,
                                Some(transfer_paused.clone()),
                            )
                            .await
                        };

                        let chunk = if address.len() > 0 {
                            wait_transfer_unpaused(&transfer_paused).await;
                            retrieve_check_chunk(
                                &checkad,
                                ctrl8.clone(),
                                &overlay_peers,
                                &accounting_peers,
                                &refreshment,
                                Some(transfer_paused.clone()),
                            )
                            .await
                        } else {
                            vec![]
                        };

                        if chunk.len() == 0 {
                            if address.len() > 0 {
                                interface_log_to(
                                    &log_port,
                                    log_start_ms,
                                    format!(
                                        "Retrieve check failed for chunk {}",
                                        hex::encode(&checkad)
                                    ),
                                );
                            }
                            async_std::task::sleep(Duration::from_millis(
                                PUSH_CHUNK_RETRY_DELAY_MS,
                            ))
                            .await;
                            let _ = chunk_upload_chan_outgoing.try_send((
                                d.clone(),
                                soc.clone(),
                                checkad.clone(),
                                stamp.clone(),
                                feedback.clone(),
                            ));
                        } else {
                            let _ = feedback.try_send(true);
                        }
                    });

                    match chunk_upload_chan_incoming.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                // if dispatched > 0 {
                //     self.interface_log(format!("Making {} pushsync requests", dispatched));
                // }

                async_std::task::yield_now().await;
            }
        };

        let retrieve_chunk_handle = async {
            let retrieve_sem = Arc::new(Semaphore::new(576));
            let retrieve_dispatch_yield_every = 128usize;

            loop {
                let mut incoming_request = match chunk_retrieve_chan_incoming.recv().await {
                    Ok(request) => request,
                    Err(_) => continue,
                };
                let mut dispatched = 0usize;

                loop {
                    let request = incoming_request;
                    let n = request.address;
                    let chan = request.chan;
                    let cancel = request.cancel;
                    wait_transfer_unpaused(&self.transfer_paused).await;

                    if !retrieve_cancel_token_current(&self.retrieve_cancel_generations, &cancel)
                        .await
                    {
                        let _ = chan.try_send(vec![]);
                        match chunk_retrieve_chan_incoming.try_recv() {
                            Ok(request) => {
                                incoming_request = request;
                                continue;
                            }
                            Err(_) => break,
                        }
                    }

                    let sem = retrieve_sem.clone();
                    let ctrl9 = ctrl6.clone();
                    let overlay_peers = wings.overlay_peers.clone();
                    let accounting_peers = wings.accounting_peers.clone();
                    let refresh_chan = refreshment_instructions_chan_outgoing.clone();
                    let log_port = self.log_port.0.clone();
                    let log_start_ms = self.log_start_ms;
                    let retrieve_cancel_generations = self.retrieve_cancel_generations.clone();
                    let transfer_paused = self.transfer_paused.clone();

                    dispatched += 1;

                    spawn_local(async move {
                        let started = Date::now();
                        wait_transfer_unpaused(&transfer_paused).await;
                        let _permit = sem.acquire().await;
                        wait_transfer_unpaused(&transfer_paused).await;

                        if !retrieve_cancel_token_current(&retrieve_cancel_generations, &cancel)
                            .await
                        {
                            let _ = chan.try_send(vec![]);
                            return;
                        }

                        let chunk_data = retrieve_chunk(
                            &n,
                            ctrl9,
                            &overlay_peers,
                            &accounting_peers,
                            &refresh_chan,
                            Some(retrieve_cancel_generations),
                            cancel,
                            Some(transfer_paused),
                        )
                        .await;

                        interface_log_to(
                            &log_port,
                            log_start_ms,
                            format!(
                                "Retrieved chunk {} len {} in {}ms",
                                hex::encode(&n),
                                chunk_data.len(),
                                (Date::now() - started).max(0.0).round() as u64,
                            ),
                        );
                        let _ = chan.try_send(chunk_data);
                    });

                    if dispatched % retrieve_dispatch_yield_every == 0 {
                        async_std::task::yield_now().await;
                    }

                    match chunk_retrieve_chan_incoming.try_recv() {
                        Ok(request) => incoming_request = request,
                        Err(_) => break,
                    }
                }

                if dispatched > 0 {
                    self.interface_log(format!(
                        "Dispatched ({}) chunk retrieval requests",
                        dispatched
                    ));
                }
                async_std::task::yield_now().await;
            }
        };

        let hive_joiner = async {
            loop {
                let mut that = match connections_instructions_chan_incoming.recv().await {
                    Ok(instruction) => instruction,
                    Err(_) => break,
                };

                loop {
                    let (bzzaddr0, bootn, instruction_generation) = that;
                    let current_generation = self.current_connection_generation().await;
                    if instruction_generation != current_generation {
                        timed_log(format!(
                            "Skipping stale connection instruction generation={} current={}",
                            instruction_generation, current_generation
                        ));
                        match connections_instructions_chan_incoming.try_recv() {
                            Ok(instruction) => {
                                that = instruction;
                                continue;
                            }
                            Err(_) => break,
                        }
                    }

                    let ctrl3 = ctrl3.clone();
                    let accounting_peer_chan_outgoing = accounting_peer_chan_outgoing.clone();
                    let connections_instructions_chan_outgoing =
                        connections_instructions_chan_outgoing.clone();
                    let connection_generation = self.connection_generation.clone();
                    let secret_key = self.secret_key.clone();
                    let nid: u64;
                    {
                        let nid0 = self.network_id.lock().await.clone();
                        nid = nid0.clone();
                    }

                    let wings = wings.clone();

                    spawn_local(async move {
                        let handshake_started = Date::now();
                        let retry_bzzaddr = bzzaddr0.clone();
                        let addr3 = match libp2p::core::Multiaddr::try_from(bzzaddr0.underlay) {
                            Ok(addr) => addr,
                            Err(_) => {
                                timed_log("Handshake dispatch skipped invalid underlay");
                                return;
                            }
                        };

                        let id = match try_from_multiaddr(&addr3) {
                            Some(peer_id) => peer_id,
                            None => {
                                timed_log("Handshake dispatch skipped invalid peer id");
                                return;
                            }
                        };

                        timed_log(format!(
                            "Entering Handshake Joiner peer={} bootnode={}",
                            id, bootn,
                        ));

                        if bootn {
                            let mut bootnodes_set = wings.bootnodes.lock().await;
                            bootnodes_set.insert(id.to_string());
                        }

                        let self_ephemeral: Multiaddr = {
                            let existing = {
                                let map = wings.self_ephemerals.lock().await;
                                map.get(&id).cloned()
                            };
                            match existing {
                                Some(addr) => addr,
                                None => {
                                    let (waiter_out, waiter_in) = mpsc::unbounded::<Multiaddr>();
                                    {
                                        let mut waiters = wings.self_ephemeral_waiters.lock().await;
                                        waiters.entry(id).or_default().push(waiter_out);
                                    }

                                    let existing = {
                                        let map = wings.self_ephemerals.lock().await;
                                        map.get(&id).cloned()
                                    };
                                    if let Some(addr) = existing {
                                        let mut waiters = wings.self_ephemeral_waiters.lock().await;
                                        waiters.remove(&id);
                                        addr
                                    } else {
                                        match waiter_in.recv().await {
                                            Ok(addr) => addr,
                                            Err(_) => return,
                                        }
                                    }
                                }
                            }
                        };

                        if *connection_generation.lock().await != instruction_generation {
                            let mut connection_attempts_map =
                                wings.connection_attempts.lock().await;
                            connection_attempts_map.remove(&id);
                            timed_log(format!(
                                "Handshake skipped stale generation peer={} generation={}",
                                id, instruction_generation
                            ));
                            return;
                        }

                        let success = connection_handler(
                            id,
                            nid,
                            self_ephemeral,
                            ctrl3,
                            &addr3,
                            &secret_key,
                            &accounting_peer_chan_outgoing,
                        )
                        .await;

                        let elapsed = Date::now() - handshake_started;
                        timed_log(format!(
                            "Handshake Joiner finished peer={} success={} elapsed_ms={}",
                            id, success, elapsed
                        ));

                        if !success {
                            {
                                let mut connection_attempts_map =
                                    wings.connection_attempts.lock().await;
                                connection_attempts_map.remove(&id);
                            }

                            let already_connected = {
                                let connected_peers_map = wings.connected_peers.lock().await;
                                connected_peers_map.contains_key(&id)
                            };

                            if !already_connected {
                                async_std::task::sleep(Duration::from_millis(
                                    HANDSHAKE_RETRY_DELAY_MS,
                                ))
                                .await;
                                if *connection_generation.lock().await == instruction_generation {
                                    let _ = connections_instructions_chan_outgoing.try_send((
                                        retry_bzzaddr,
                                        bootn,
                                        instruction_generation,
                                    ));
                                    timed_log(format!(
                                        "Handshake retry queued for peer={} bootnode={} generation={}",
                                        id, bootn, instruction_generation
                                    ));
                                } else {
                                    timed_log(format!(
                                        "Handshake retry skipped stale generation peer={} generation={}",
                                        id, instruction_generation
                                    ));
                                }
                            }
                        }
                    });

                    match connections_instructions_chan_incoming.try_recv() {
                        Ok(instruction) => that = instruction,
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
            refreshment_apply_handle,
            cheque_instruction_handle,
            cheque_apply_handle,
            retrieve_handle,
            resolve_bzz_handle,
            acquire_range_handle,
            retrieve_data_handle,
            retrieve_chunk_handle,
            push_handle,
            push_data_handle,
            push_chunk_handle,
            push_chunk_port_handle,
            swarm_event_handle_0,
            swarm_event_handle_1,
            swarm_event_handle_2,
            gossip_inbound_handle,
            pricing_inbound_handle,
            hive_joiner,
        );

        web_sys::console::log_1(&JsValue::from(format!("Dropping All handlers")));

        ()
    }
}

impl Weeb3 {
    pub async fn resolve_bzz(&self, resource: String) -> Option<BzzMetadata> {
        let (chan_out, chan_in) = mpsc::unbounded::<Option<BzzMetadata>>();
        let _ = self.resolve_port.0.try_send((resource, chan_out));

        chan_in.recv().await.unwrap_or(None)
    }

    pub async fn acquire_range(
        &self,
        resource: String,
        start: u64,
        end_inclusive: u64,
    ) -> Option<(Vec<u8>, BzzMetadata)> {
        let (chan_out, chan_in) = mpsc::unbounded::<Option<(Vec<u8>, BzzMetadata)>>();
        let _ = self.range_port.0.try_send(BzzRangeRequest::Resource {
            resource,
            start,
            end_inclusive,
            cancel: None,
            chan: chan_out,
        });

        chan_in.recv().await.unwrap_or(None)
    }

    pub async fn acquire_resolved_range(
        &self,
        metadata: BzzMetadata,
        start: u64,
        end_inclusive: u64,
    ) -> Option<(Vec<u8>, BzzMetadata)> {
        let (chan_out, chan_in) = mpsc::unbounded::<Option<(Vec<u8>, BzzMetadata)>>();
        let _ = self.range_port.0.try_send(BzzRangeRequest::Resolved {
            metadata,
            start,
            end_inclusive,
            cancel: None,
            chan: chan_out,
        });

        chan_in.recv().await.unwrap_or(None)
    }

    pub async fn acquire_resolved_stream_range(
        &self,
        metadata: BzzMetadata,
        start: u64,
        end_inclusive: u64,
        stream_key: String,
        stream_generation: u64,
    ) -> Option<(Vec<u8>, BzzMetadata)> {
        let (chan_out, chan_in) = mpsc::unbounded::<Option<(Vec<u8>, BzzMetadata)>>();
        let cancel = if stream_key.is_empty() || stream_generation == 0 {
            None
        } else {
            Some(RetrieveCancelToken {
                stream_key,
                generation: stream_generation,
            })
        };

        let _ = self.range_port.0.try_send(BzzRangeRequest::Resolved {
            metadata,
            start,
            end_inclusive,
            cancel,
            chan: chan_out,
        });

        chan_in.recv().await.unwrap_or(None)
    }

    pub async fn prepare_bzz_stream(&self, metadata: BzzMetadata) -> bool {
        let (chan_out, chan_in) = mpsc::unbounded::<bool>();
        let _ = self.range_port.0.try_send(BzzRangeRequest::Prepare {
            metadata,
            chan: chan_out,
        });

        chan_in.recv().await.unwrap_or(false)
    }
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    autonat: autonat::v2::client::Behaviour,
    autonat_s: autonat::v2::server::Behaviour,
    dcutr: dcutr::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    stream: stream::Behaviour,
}

impl Behaviour {
    fn new(local_public_key: identity::PublicKey) -> Self {
        Self {
            autonat: autonat::v2::client::Behaviour::new(
                OsRng,
                autonat::v2::client::Config::default().with_probe_interval(Duration::from_secs(60)),
            ),
            autonat_s: autonat::v2::server::Behaviour::new(OsRng),
            dcutr: dcutr::Behaviour::new(local_public_key.to_peer_id()),
            identify: identify::Behaviour::new(
                identify::Config::new("/weeb-3".into(), local_public_key.clone())
                    .with_push_listen_addr_updates(true)
                    .with_interval(Duration::from_secs(30)), // .with_cache_size(10), //
            ),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(15))),
            stream: stream::Behaviour::new(),
        }
    }
}
