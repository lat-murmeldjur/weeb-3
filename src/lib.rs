#![cfg(target_arch = "wasm32")]

use async_std::sync::{Arc, Mutex};

use rand::rngs::OsRng;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::num::NonZero;
use std::sync::mpsc;
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

mod conventions;
use conventions::*;

mod handlers;
use handlers::*;

mod interface;

mod library;

mod manifest;

mod manifest_upload;

mod on_chain;
use on_chain::get_price_from_oracle;

mod nav;

mod persistence;
use persistence::{
    get_batch_bucket_limit,
    get_batch_id,
    get_batch_owner_key,
    // get_chequebook_signer_key,
    reset_stamp,
};

mod retrieval;
use retrieval::*;

mod upload;
use upload::*;

mod ens;
use ens::*;

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

const PROTOCOL_ROUND_TIME: f64 = 300.0;
const EVENT_LOOP_INTERRUPTOR: f64 = 50.0;
const PROTO_LOOP_INTERRUPTOR: f64 = 50.0;

//
// pub fn init_panic_hook() {
//     console_error_panic_hook::set_once();
// }

#[wasm_bindgen]
pub struct Sekirei {
    swarm: Arc<Mutex<Swarm<Behaviour>>>,
    secret_key: Mutex<SecretKey>,
    wings: Mutex<Arc<Wings>>,
    log_port: (mpsc::Sender<String>, mpsc::Receiver<String>),
    message_port: (
        mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
        mpsc::Receiver<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
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
        mpsc::Sender<(String, mpsc::Sender<String>, bool)>,
        mpsc::Receiver<(String, mpsc::Sender<String>, bool)>,
    ),
    network_id: Mutex<u64>,
    ongoing_connections: Mutex<u64>,
    connections: Mutex<u64>,
}

type PeerAddrMap = Arc<Mutex<HashMap<PeerId, Multiaddr>>>;

#[wasm_bindgen]
pub struct Wings {
    connected_peers: Arc<Mutex<HashMap<PeerId, PeerFile>>>,
    overlay_peers: Arc<Mutex<HashMap<String, PeerId>>>,
    bootnodes: Arc<Mutex<HashSet<String>>>,
    accounting_peers: Arc<Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>>,
    ongoing_refreshments: Arc<Mutex<HashSet<PeerId>>>,
    ongoing_cheques: Arc<Mutex<HashMap<PeerId, u64>>>,
    swap_beneficiaries: Arc<Mutex<HashMap<PeerId, (web3::types::Address, bool)>>>,
    connection_attempts: Arc<Mutex<HashSet<PeerId>>>,
    self_ephemerals: PeerAddrMap,
}

#[wasm_bindgen]
impl Sekirei {
    pub async fn change_bootnode_address(
        &self,
        address: String,
        _id: String,
        usable_in_protocols: bool,
    ) -> Vec<u8> {
        let parse_id = _id.parse::<u64>();

        match parse_id {
            Ok(parsed_id) => {
                web_sys::console::log_1(&JsValue::from(format!("Parsed network id {}", parsed_id)));
                let mut nid = self.network_id.lock().await;
                *nid = parsed_id;
            }
            _ => {}
        };

        web_sys::console::log_1(&"bootnode change triggered".into());

        let (chan_out, chan_in) = mpsc::channel::<String>();
        let _ = self
            .bootnode_port
            .0
            .send((address, chan_out, usable_in_protocols));

        let k0 = async {
            let mut timelast: f64;
            #[allow(irrefutable_let_patterns)]
            while let that = chan_in.try_recv() {
                let timenow = Date::now();
                timelast = timenow;
                if !that.is_err() {
                    return that.unwrap();
                }

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < EVENT_LOOP_INTERRUPTOR {
                    async_std::task::sleep(Duration::from_millis(
                        (EVENT_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                };
            }

            return "".to_string();
        };

        let result = k0.await;

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

    pub async fn post_upload(
        &self,
        file: File,
        encryption: bool,
        index_string: String,
        add_to_feed: bool,
        feed_topic: String,
    ) -> Vec<u8> {
        let (chan_out, chan_in) = mpsc::channel::<Vec<u8>>();

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

        let _ = self.upload_port.0.send((
            fvec0,
            encryption,
            index_document,
            add_to_feed,
            topic_safe,
            chan_out,
        ));

        let k0 = async {
            let mut timelast: f64;
            #[allow(irrefutable_let_patterns)]
            while let that = chan_in.try_recv() {
                let timenow = Date::now();
                timelast = timenow;
                if !that.is_err() {
                    return that.unwrap();
                }

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < EVENT_LOOP_INTERRUPTOR {
                    async_std::task::sleep(Duration::from_millis(
                        (EVENT_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                };
            }

            return vec![];
        };

        let result = k0.await;

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

    pub async fn acquire(&self, address: String) -> Vec<u8> {
        let (chan_out, chan_in) = mpsc::channel::<Vec<u8>>();
        let valaddr_0 = hex::decode(&address);
        let valaddr = match valaddr_0 {
            Ok(hex) => hex,
            _ => prt(address, "".to_string()).await,
        };

        let _ = self.message_port.0.send((valaddr, chan_out));

        let k0 = async {
            let mut timelast: f64;
            #[allow(irrefutable_let_patterns)]
            while let that = chan_in.try_recv() {
                let timenow = Date::now();
                timelast = timenow;
                if !that.is_err() {
                    return that.unwrap();
                }

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < EVENT_LOOP_INTERRUPTOR {
                    //                web_sys::console::log_1(&JsValue::from(format!(
                    //                    "Ease event handle loop for {}",
                    //                    EVENT_LOOP_INTERRUPTOR - seg
                    //                )));
                    async_std::task::sleep(Duration::from_millis(
                        (EVENT_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                };
            }

            return vec![];
        };

        let result = k0.await;

        return result;
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

    pub fn new(_st: String) -> Sekirei {
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
                websocket_websys::Transport::default()
                    .upgrade(core::upgrade::Version::V1)
                    .authenticate(noise::Config::new(&keypair.clone().into()).unwrap())
                    .multiplex(yamux::Config::default())
                    .boxed()
            })
            .expect("Failed to create WebSocket transport")
            .with_behaviour(|key| Behaviour::new(key.public()))
            .unwrap()
            .with_swarm_config(|_| {
                libp2p::swarm::Config::with_wasm_executor()
                    .with_idle_connection_timeout(Duration::from_secs(36000000))
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
        let accounting_peers: Arc<Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let ongoing_refreshments: Arc<Mutex<HashSet<PeerId>>> =
            Arc::new(Mutex::new(HashSet::new()));
        let connection_attempts: Arc<Mutex<HashSet<PeerId>>> = Arc::new(Mutex::new(HashSet::new()));
        let ongoing_cheques: Arc<Mutex<HashMap<PeerId, u64>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let swap_beneficiaries: Arc<Mutex<HashMap<PeerId, (web3::types::Address, bool)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let self_ephemerals: PeerAddrMap = Arc::new(Mutex::new(HashMap::new()));

        let (m_out, m_in) = mpsc::channel::<(Vec<u8>, mpsc::Sender<Vec<u8>>)>();

        let (log_port_out, log_port_in) = mpsc::channel::<String>();

        let (u_out, u_in) = mpsc::channel::<(
            Vec<Resource>,
            bool,
            String,
            bool,
            String,
            mpsc::Sender<Vec<u8>>,
        )>();
        let (b_out, b_in) = mpsc::channel::<(String, mpsc::Sender<String>, bool)>();

        return Sekirei {
            secret_key: Mutex::new(secret_key),
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
                self_ephemerals: self_ephemerals,
            })),
            log_port: (log_port_out, log_port_in),
            message_port: (m_out, m_in),
            upload_port: (u_out, u_in),
            bootnode_port: (b_out, b_in),
            network_id: Mutex::new(10_u64),
            ongoing_connections: Mutex::new(0_u64),
            connections: Mutex::new(0_u64),
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
        return *ongoing;
    }

    pub async fn get_connections(&self) -> u64 {
        let connected = self.connections.lock().await;
        return *connected;
    }

    pub fn interface_log(&self, log0: String) {
        let _ = self.log_port.0.send(log0.to_string());
        web_sys::console::log_1(&JsValue::from(log0));
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

        let (peers_instructions_chan_outgoing, peers_instructions_chan_incoming) = mpsc::channel();
        let (connections_instructions_chan_outgoing, connections_instructions_chan_incoming) =
            mpsc::channel::<(etiquette_2::BzzAddress, bool, u64)>();

        let (accounting_peer_chan_outgoing, accounting_peer_chan_incoming) = mpsc::channel();

        let (pricing_chan_outgoing, pricing_chan_incoming) = mpsc::channel::<(PeerId, u64)>();

        let (refreshment_instructions_chan_outgoing, refreshment_instructions_chan_incoming) =
            mpsc::channel::<(PeerId, u64)>();

        let (refreshment_chan_outgoing, refreshment_chan_incoming) =
            mpsc::channel::<(PeerId, u64)>();

        let (data_retrieve_chan_outgoing, data_retrieve_chan_incoming) =
            mpsc::channel::<(Vec<u8>, mpsc::Sender<Vec<u8>>)>();

        let (chunk_retrieve_chan_outgoing, chunk_retrieve_chan_incoming) =
            mpsc::channel::<(Vec<u8>, mpsc::Sender<Vec<u8>>)>();

        let (data_upload_chan_outgoing, data_upload_chan_incoming) =
            mpsc::channel::<(Vec<Vec<u8>>, u8, Vec<u8>, Vec<u8>, mpsc::Sender<Vec<u8>>)>();

        let (chunk_upload_chan_outgoing, chunk_upload_chan_incoming) =
            mpsc::channel::<(Vec<u8>, bool, Vec<u8>, Vec<u8>, mpsc::Sender<bool>)>();

        let (cheque_instructions_chan_outgoing, cheque_instructions_chan_incoming) =
            mpsc::channel::<(PeerId, u64)>();
        let (cheque_send_chan_outgoing, cheque_send_chan_incoming) =
            mpsc::channel::<(PeerId, bool)>();

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
                pricing_handler(peer, stream, &pricing_chan_outgoing.clone()).await;
            }
        };

        let gossip_inbound_handle = async move {
            while let Some((peer, stream)) = incoming_gossip_streams.next().await {
                gossip_handler(peer, stream, &peers_instructions_chan_outgoing.clone()).await;
            }
        };

        let swarm_event_handle_0 = async {
            loop {
                let mut dial_joiner = Vec::new();

                #[allow(irrefutable_let_patterns)]
                while let paddr0 = peers_instructions_chan_incoming.try_recv() {
                    if !paddr0.is_err() {
                        let handle = async {
                            // web_sys::console::log_1(&JsValue::from(format!(
                            //     "Current Conn Handled {:#?}",
                            //     paddr
                            // )));

                            let paddr = match paddr0 {
                                Ok(aok) => aok,
                                _ => {
                                    return;
                                }
                            };

                            let und_addrs = deserialize_underlays(&paddr.clone().underlay);

                            for addr3 in und_addrs.iter() {
                                web_sys::console::log_1(&JsValue::from(format!(
                                    "Current Conn Handled {:#?}",
                                    addr3.to_string()
                                )));

                                if detect_underlay_format(&addr3) == UnderlayFormat::BeeWss {
                                    let _pid: PeerId = match try_from_multiaddr(&addr3.clone()) {
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

                                    let mut paddr5 = paddr.clone();
                                    {
                                        let mut swarm = self.swarm.lock_arc().await;
                                        web_sys::console::log_1(&JsValue::from(format!("dial 0",)));
                                        let addr30 = beewss_to_dns_transformed(&addr3);
                                        let _ = swarm.dial(addr30.clone());
                                        paddr5.underlay = addr30.to_vec();
                                    }

                                    let _ = connections_instructions_chan_outgoing.send((
                                        paddr5.clone(),
                                        false,
                                        Date::now() as u64,
                                    ));
                                }
                            }

                            let addr4 =
                                match libp2p::core::Multiaddr::try_from(paddr.clone().underlay) {
                                    Ok(aok) => aok,
                                    _ => {
                                        return;
                                    }
                                };

                            if detect_underlay_format(&addr4) == UnderlayFormat::BeeWss {
                                let _pid: PeerId = match try_from_multiaddr(&addr4.clone()) {
                                    Some(aok) => {
                                        let connected_peers_map =
                                            wings.connected_peers.lock().await;
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

                                let mut paddr5 = paddr.clone();
                                {
                                    let mut swarm = self.swarm.lock_arc().await;

                                    web_sys::console::log_1(&JsValue::from(format!("dial 1",)));
                                    let addr40 = beewss_to_dns_transformed(&addr4);
                                    let _ = swarm.dial(addr40.clone());
                                    paddr5.underlay = addr40.to_vec();
                                }

                                let _ = connections_instructions_chan_outgoing.send((
                                    paddr5.clone(),
                                    false,
                                    Date::now() as u64,
                                ));
                            }
                        };
                        dial_joiner.push(handle);
                    } else {
                        break;
                    };
                }

                join_all(dial_joiner).await;
                async_std::task::sleep(Duration::from_millis(300)).await;
            }
        };

        let swarm_event_handle_1 = async {
            loop {
                {
                    let mut swarm = self.swarm.lock_arc().await;

                    #[allow(irrefutable_let_patterns)]
                    while let event = async_std::future::timeout(
                        Duration::from_millis(EVENT_LOOP_INTERRUPTOR as u64),
                        swarm.next(),
                    )
                    .await
                    {
                        // self.interface_log(format!("Current Event Handled {:#?}", event));
                        // web_sys::console::log_1(&JsValue::from(format!(
                        //     "Current Event Handled {:#?}",
                        //     event
                        // )));

                        if !event.is_err() {
                            match event.unwrap() {
                                Some(SwarmEvent::Behaviour(out_event)) => {
                                    if let BehaviourEvent::Identify(identify_event) = out_event {
                                        if let identify::Event::Received { peer_id, info, .. } = identify_event {
                                            let observed_addr: Multiaddr = info.observed_addr.clone();
                                            let mut map = wings.self_ephemerals.lock().await;
                                            map.insert(peer_id.clone(), observed_addr.clone());
                                            self.interface_log(format!(
                                                "Observed address {} for peer {} stored/overwritten",
                                                observed_addr, peer_id
                                            ));
                                        }
                                    }
                                }
                                Some(SwarmEvent::ConnectionEstablished {
                                    // peer_id,
                                    // established_in,
                                    ..
                                }) => {
                                    //
                                }
                                Some(SwarmEvent::ConnectionClosed { peer_id, .. }) => {
                                    {
                                        let mut connected_peers_map = wings.connected_peers.lock().await;
                                        let mut overlay_peers_map = wings.overlay_peers.lock().await;
                                        if connected_peers_map.contains_key(&peer_id) {
                                            let ol0 = hex::encode(connected_peers_map.get(&peer_id).unwrap().overlay.clone());
                                            if overlay_peers_map.contains_key(&ol0) {
                                                overlay_peers_map.remove(&ol0);
                                                {
                                                   let mut connections = self.connections.lock().await;
                                                    if *connections > 0 {
                                                        *connections = *connections - 1
                                                    }
                                                }
                                            } else {
                                                {
                                                   let mut ongoing = self.ongoing_connections.lock().await;
                                                    if *ongoing > 0 {
                                                        *ongoing = *ongoing - 1
                                                    }
                                                }
                                            };
                                            connected_peers_map.remove(&peer_id);

                                            self.interface_log(format!("Disconnected from peer {}", &ol0));
                                        };
                                        let mut connection_attempts_map = wings.connection_attempts.lock().await;
                                        if connection_attempts_map.contains(&peer_id) {
                                            connection_attempts_map.remove(&peer_id);
                                        }
                                    }
                                    let mut accounting = wings.accounting_peers.lock().await;
                                    if accounting.contains_key(&peer_id) {
                                        accounting.remove(&peer_id);
                                    };
                                }
                                _ => {}
                            }
                        } else {
                            break;
                        }
                    }
                }
                async_std::task::sleep(Duration::from_millis(300)).await;
            }
        };

        let swarm_event_handle_2 = async {
            loop {
                let mut bootnode_dial_joiner = Vec::new();

                #[allow(irrefutable_let_patterns)]
                while let bootnode_change = self.bootnode_port.1.try_recv() {
                    if !bootnode_change.is_err() {
                        let handle = async {
                            let (baddr, chan, usable) = bootnode_change.unwrap();

                            let addr33 = match baddr.parse::<Multiaddr>() {
                                Ok(aok) => aok,
                                _ => {
                                    let _ = chan
                                        .send("parse multiaddress for bootnode failed".to_string());
                                    return;
                                }
                            };

                            // let bn_id: PeerId = match try_from_multiaddr(&addr33.clone()) {
                            //     Some(aok) => aok,
                            //     _ => {
                            //         let _ = chan.send("parse peerid for bootnode failed".to_string());
                            //         break;
                            //     }
                            // };
                            let _pid: PeerId = match try_from_multiaddr(&addr33.clone()) {
                                Some(aok) => {
                                    let mut connection_attempts_map =
                                        wings.connection_attempts.lock().await;
                                    if connection_attempts_map.contains(&aok) {
                                        return;
                                    } else {
                                        connection_attempts_map.insert(aok);
                                    }
                                    aok
                                }
                                _ => return,
                            };
                            {
                                let mut swarm = self.swarm.lock_arc().await;
                                web_sys::console::log_1(&JsValue::from(format!(
                                    "dial 2 :: {:#?}",
                                    addr33
                                )));
                                if detect_underlay_format(&addr33) == UnderlayFormat::BeeWss {
                                    let addr30 = beewss_to_dns_transformed(&addr33);
                                    let _ = swarm.dial(addr30.clone());

                                    let _ = chan.send("dialing bootnode".to_string());

                                    let mut bzzaddr = etiquette_2::BzzAddress::default();

                                    bzzaddr.underlay = addr30.to_vec();

                                    let _ = connections_instructions_chan_outgoing.send((
                                        bzzaddr,
                                        !usable,
                                        Date::now() as u64,
                                    ));
                                } else {
                                    let _ = swarm.dial(addr33.clone());

                                    let _ = chan.send("dialing bootnode".to_string());

                                    let mut bzzaddr = etiquette_2::BzzAddress::default();

                                    bzzaddr.underlay = addr33.to_vec();

                                    let _ = connections_instructions_chan_outgoing.send((
                                        bzzaddr,
                                        !usable,
                                        Date::now() as u64,
                                    ));
                                }
                            }
                        };
                        bootnode_dial_joiner.push(handle);
                    } else {
                        break;
                    }
                }
                join_all(bootnode_dial_joiner).await;
                async_std::task::sleep(Duration::from_millis(300)).await;
            }
        };

        let event_handle = async {
            let mut timelast = Date::now();
            let mut interrupt_last = Date::now();
            loop {
                let k1 = async {
                    #[allow(irrefutable_let_patterns)]
                    while let incoming_peer = accounting_peer_chan_incoming.try_recv() {
                        if !incoming_peer.is_err() {
                            // Accounting connect
                            let peer_file: PeerFile = incoming_peer.unwrap();
                            // let ol = hex::encode(peer_file.overlay.clone());
                            {
                                let mut accounting = wings.accounting_peers.lock().await;
                                if !accounting.contains_key(&peer_file.peer_id) {
                                    accounting.insert(
                                        peer_file.peer_id,
                                        Mutex::new(PeerAccounting {
                                            balance: 0,
                                            threshold: 0,
                                            payment_threshold: 0,
                                            reserve: 0,
                                            refreshment: 0.0,
                                            id: peer_file.peer_id,
                                        }),
                                    );
                                }
                            }

                            {
                                let mut connected_peers_map = wings.connected_peers.lock().await;
                                connected_peers_map
                                    .insert(peer_file.peer_id.clone(), peer_file.clone());
                            }
                            {
                                let mut swap_beneficiaries_map =
                                    wings.swap_beneficiaries.lock().await;

                                swap_beneficiaries_map
                                    .insert(peer_file.peer_id, (peer_file.beneficiary, false));
                            }
                            {
                                let mut ongoing = self.ongoing_connections.lock().await;
                                *ongoing = *ongoing + 1;
                            }
                        } else {
                            break;
                        }
                    }
                };

                let k2 = async {
                    #[allow(irrefutable_let_patterns)]
                    while let pt_in = pricing_chan_incoming.try_recv() {
                        if !pt_in.is_err() {
                            let (peer, amount) = pt_in.unwrap();
                            let accounting = wings.accounting_peers.lock().await;
                            let accounting_peer_lock = match accounting.get(&peer) {
                                Some(aok) => aok,
                                _ => continue,
                            };
                            set_payment_threshold(accounting_peer_lock, amount).await;
                            {
                                let bootnodes_set = wings.bootnodes.lock().await;
                                let ol: String;
                                {
                                    let connected_peers_map = wings.connected_peers.lock().await;
                                    ol = hex::encode(
                                        connected_peers_map.get(&peer).unwrap().overlay.clone(),
                                    );
                                }
                                if !bootnodes_set.contains(&peer.to_string()) {
                                    let mut overlay_peers_map = wings.overlay_peers.lock().await;
                                    if !overlay_peers_map.contains_key(&ol.to_string()) {
                                        self.interface_log(format!("Connected to peer {}", &ol));
                                        overlay_peers_map.insert(ol, peer);

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
                                } else {
                                    self.interface_log(format!("Connected to bootnode {}", &ol));

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
                        } else {
                            let _ = pt_in;
                            break;
                        }
                    }
                };

                let k3 = async {
                    let mut refresh_joiner = Vec::new();

                    #[allow(irrefutable_let_patterns)]
                    while let re_out = refreshment_instructions_chan_incoming.try_recv() {
                        if !re_out.is_err() {
                            let (peer, amount) = re_out.unwrap();
                            {
                                let map = wings.ongoing_refreshments.lock().await;
                                if map.contains(&peer) {
                                    continue;
                                }
                            }
                            #[allow(unused_assignments)]
                            let mut daten = Date::now();
                            let datenow = daten;
                            let mut cheque_amt: u64 = 0;
                            {
                                let accounting = wings.accounting_peers.lock().await;
                                let accounting_peer_lock = match accounting.get(&peer) {
                                    Some(aok) => aok,
                                    _ => continue,
                                };
                                let mut accounting_peer = accounting_peer_lock.lock().await;
                                daten = accounting_peer.refreshment;
                                if datenow > accounting_peer.refreshment + 1000.0 {
                                    accounting_peer.refreshment = datenow;
                                } else {
                                    if accounting_peer.balance > REFRESH_RATE {
                                        cheque_amt = accounting_peer.balance - REFRESH_RATE;
                                    }
                                }
                            }
                            if datenow > daten + 1000.0 {
                                {
                                    let mut map = wings.ongoing_refreshments.lock().await;
                                    map.insert(peer);
                                }
                                let ctrl7 = ctrl4.clone();
                                let rco = refreshment_chan_outgoing.clone();
                                let handle = async move {
                                    refresh_handler(peer, amount, ctrl7, &rco).await;
                                };
                                refresh_joiner.push(handle);
                            } else if cheque_amt > 0 {
                                let mut map = wings.ongoing_cheques.lock().await;
                                if !map.contains_key(&peer) {
                                    map.insert(peer, cheque_amt);
                                    let _ =
                                        cheque_instructions_chan_outgoing.send((peer, cheque_amt));
                                }
                            }
                        } else {
                            let _ = re_out;
                            break;
                        }
                    }

                    join_all(refresh_joiner).await;
                };

                let k4 = async {
                    #[allow(irrefutable_let_patterns)]
                    while let re_in = refreshment_chan_incoming.try_recv() {
                        if !re_in.is_err() {
                            let (peer, amount) = re_in.unwrap();
                            if amount > 0 {
                                let accounting = wings.accounting_peers.lock().await;
                                let accounting_peer_lock = match accounting.get(&peer) {
                                    Some(aok) => aok,
                                    _ => continue,
                                };
                                apply_refreshment(accounting_peer_lock, amount).await;
                            }
                            let mut map = wings.ongoing_refreshments.lock().await;
                            if map.contains(&peer) {
                                map.remove(&peer);
                            }
                        } else {
                            let _ = re_in;
                            break;
                        }
                    }
                };

                let swap_price = Arc::new(Mutex::new(U256::from(0)));
                let swap_deduction = Arc::new(Mutex::new(U256::from(0)));

                let k5 = async {
                    let mut cheque_joiner = Vec::new();

                    #[allow(irrefutable_let_patterns)]
                    while let ch_out = cheque_instructions_chan_incoming.try_recv() {
                        let swap_price_0 = swap_price.clone();
                        let swap_deduction_0 = swap_deduction.clone();
                        if !ch_out.is_err() {
                            let set_price = {
                                let price = swap_price_0.lock().await;
                                price.is_zero()
                            };

                            if set_price {
                                let (oracle_price, cheque_deduction) =
                                    get_price_from_oracle().await;

                                let mut price = swap_price_0.lock().await;
                                if price.is_zero() {
                                    *price = oracle_price;
                                }

                                let mut deduction = swap_deduction_0.lock().await;
                                if deduction.is_zero() {
                                    *deduction = cheque_deduction;
                                }
                            }

                            let (peer, amount) = ch_out.unwrap();
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
                        } else {
                            let _ = ch_out;
                            break;
                        }
                    }

                    join_all(cheque_joiner).await;
                };

                let k6 = async {
                    #[allow(irrefutable_let_patterns)]
                    while let ch_in = cheque_send_chan_incoming.try_recv() {
                        if !ch_in.is_err() {
                            let (peer, ok) = ch_in.unwrap();
                            let amt_opt = {
                                let mut map = wings.ongoing_cheques.lock().await;
                                map.remove(&peer)
                            };
                            if ok {
                                if let Some(amount) = amt_opt {
                                    let accounting = wings.accounting_peers.lock().await;
                                    let accounting_peer_lock = match accounting.get(&peer) {
                                        Some(aok) => aok,
                                        _ => continue,
                                    };
                                    apply_refreshment(accounting_peer_lock, amount).await;
                                }
                            }
                        } else {
                            let _ = ch_in;
                            break;
                        }
                    }
                };

                join!(k1, k2, k3, k4, k5, k6);

                let timenow = Date::now();
                let seg = timenow - interrupt_last;
                if seg < EVENT_LOOP_INTERRUPTOR {
                    //                web_sys::console::log_1(&JsValue::from(format!(
                    //                    "Ease event handle loop for {}",
                    //                    EVENT_LOOP_INTERRUPTOR - seg
                    //                )));
                    async_std::task::sleep(Duration::from_millis(
                        (EVENT_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                }
                let timenow = Date::now();
                interrupt_last = timenow;

                if timelast + EVENT_LOOP_INTERRUPTOR < timenow {
                    timelast = timenow
                }
                //
            }
        };

        let retrieve_handle = async {
            let mut timelast = Date::now();
            loop {
                #[allow(irrefutable_let_patterns)]
                while let incoming_request = self.message_port.1.try_recv() {
                    if !incoming_request.is_err() {
                        let (n, chan) = incoming_request.unwrap();

                        let _ = chan.send(
                            retrieve_resource(
                                &n,
                                &data_retrieve_chan_outgoing.clone(),
                                &chunk_retrieve_chan_outgoing.clone(),
                            )
                            .await,
                        );
                    } else {
                        break;
                    }
                }

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < PROTO_LOOP_INTERRUPTOR {
                    async_std::task::sleep(Duration::from_millis(
                        (PROTO_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                }
                timelast = Date::now();
            }
        };

        let push_handle = async {
            let mut timelast = Date::now();
            loop {
                #[allow(irrefutable_let_patterns)]
                while let incoming_request = self.upload_port.1.try_recv() {
                    if !incoming_request.is_err() {
                        let (file0, enc, index, feed, topic, chan) = incoming_request.unwrap();

                        let batch_owner = get_batch_owner_key().await;
                        let batch_id = get_batch_id().await;

                        if batch_owner.len() == 0 {
                            self.interface_log("No batch found for uploads".to_string());

                            chan.send(vec![]).unwrap();
                            continue;
                        }

                        if batch_id.len() == 0 {
                            self.interface_log("No batchId found for uploads".to_string());

                            chan.send(vec![]).unwrap();
                            continue;
                        }

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

                        chan.send(push_reference).unwrap();
                    } else {
                        break;
                    }
                }

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < PROTO_LOOP_INTERRUPTOR {
                    async_std::task::sleep(Duration::from_millis(
                        (PROTO_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                }
                timelast = Date::now();
            }
        };

        let retrieve_data_handle = async {
            let mut timelast = Date::now();
            loop {
                let mut request_joiner = Vec::new();

                #[allow(irrefutable_let_patterns)]
                while let incoming_request = data_retrieve_chan_incoming.try_recv() {
                    if !incoming_request.is_err() {
                        let handle = async {
                            let (n, chan) = incoming_request.unwrap();
                            let retrieved_data =
                                retrieve_data(&n, &chunk_retrieve_chan_outgoing.clone()).await;
                            chan.send(retrieved_data).unwrap();
                        };
                        request_joiner.push(handle);
                    } else {
                        break;
                    }
                }

                join_all(request_joiner).await;

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < PROTO_LOOP_INTERRUPTOR {
                    // web_sys::console::log_1(&JsValue::from(format!(
                    //     "Ease retrieve handle loop for {}",
                    //     PROTO_LOOP_INTERRUPTOR - seg
                    // )));
                    async_std::task::sleep(Duration::from_millis(
                        (PROTO_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                }
                timelast = Date::now();
            }
        };

        let push_data_handle = async {
            let mut timelast = Date::now();
            loop {
                let mut request_joiner = Vec::new();

                #[allow(irrefutable_let_patterns)]
                while let incoming_request = data_upload_chan_incoming.try_recv() {
                    if !incoming_request.is_err() {
                        let handle = async {
                            let (n, mode, batch_owner, batch_id, chan) = incoming_request.unwrap();

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
                                &chunk_upload_chan_outgoing.clone(),
                            )
                            .await;

                            chan.send(data_reference).unwrap();
                        };
                        request_joiner.push(handle);
                    } else {
                        break;
                    }
                }

                join_all(request_joiner).await;

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < PROTO_LOOP_INTERRUPTOR {
                    async_std::task::sleep(Duration::from_millis(
                        (PROTO_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                }
                timelast = Date::now();
            }
        };

        let push_chunk_handle = async {
            let mut timelast = Date::now();
            let mut connections = false;
            loop {
                if !connections {
                    {
                        let overlay_peers_map = wings.overlay_peers.lock().await;
                        if overlay_peers_map.len() > 0 {
                            connections = true;
                        }
                    }
                    async_std::task::sleep(Duration::from_millis(PROTO_LOOP_INTERRUPTOR as u64))
                        .await;
                    continue;
                }

                let mut request_joiner = vec![];

                #[allow(irrefutable_let_patterns)]
                for _i in 0..1024 {
                    let incoming_request = chunk_upload_chan_incoming.try_recv();
                    if !incoming_request.is_err() {
                        let handle = async {
                            let (d, soc, checkad, stamp, feedback) = incoming_request.unwrap();

                            let address = push_chunk(
                                d.clone(),
                                soc.clone(),
                                checkad.clone(),
                                stamp.clone(),
                                ctrl8.clone(),
                                &wings.overlay_peers,
                                &wings.accounting_peers,
                                &refreshment_instructions_chan_outgoing,
                            )
                            .await;

                            let chunk = retrieve_chunk(
                                &checkad,
                                ctrl8.clone(),
                                &wings.overlay_peers.clone(),
                                &wings.accounting_peers.clone(),
                                &refreshment_instructions_chan_outgoing.clone(),
                            )
                            .await;

                            if chunk.len() == 0 {
                                chunk_upload_chan_outgoing.send((
                                    d.clone(),
                                    soc.clone(),
                                    checkad.clone(),
                                    stamp.clone(),
                                    feedback.clone(),
                                ));

                                self.interface_log(format!("reuploading chunk Y0N"));
                            } else {
                                let _ = feedback.send(true);
                            }
                        };
                        request_joiner.push(handle);
                    } else {
                        break;
                    }
                }

                if request_joiner.len() > 0 {
                    self.interface_log(format!(
                        "Making {} pushsync requests",
                        request_joiner.len()
                    ));
                }

                let _ = join_all(request_joiner).await;

                // while let Some(()) = request_joiner.next().await {
                //     web_sys::console::log_1(&JsValue::from(format!("push chunk completed")));
                // }

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < PROTO_LOOP_INTERRUPTOR {
                    async_std::task::sleep(Duration::from_millis(
                        (PROTO_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                }
                timelast = Date::now();
            }
        };

        let retrieve_chunk_handle = async {
            let mut timelast = Date::now();
            let mut connections = false;
            loop {
                let mut request_joiner = Vec::new();

                if !connections {
                    {
                        let overlay_peers_map = wings.overlay_peers.lock().await;
                        if overlay_peers_map.len() > 7 {
                            connections = true;
                        }
                    }
                    async_std::task::sleep(Duration::from_millis(PROTO_LOOP_INTERRUPTOR as u64))
                        .await;
                    continue;
                }

                #[allow(irrefutable_let_patterns)]
                for _i in 0..4096 {
                    let incoming_request = chunk_retrieve_chan_incoming.try_recv();
                    if !incoming_request.is_err() {
                        let handle = async {
                            let (n, chan) = incoming_request.unwrap();

                            let ctrl9 = ctrl6.clone();

                            let chunk_data = retrieve_chunk(
                                &n,
                                ctrl9,
                                &wings.overlay_peers.clone(),
                                &wings.accounting_peers.clone(),
                                &refreshment_instructions_chan_outgoing.clone(),
                            )
                            .await;

                            chan.send(chunk_data).unwrap();
                        };
                        request_joiner.push(handle);
                    } else {
                        break;
                    }
                }

                if request_joiner.len() > 0 {
                    self.interface_log(format!(
                        "Making ({}) chunk retrieval requests",
                        request_joiner.len()
                    ));
                }

                join_all(request_joiner).await;

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < PROTO_LOOP_INTERRUPTOR {
                    async_std::task::sleep(Duration::from_millis(
                        (PROTO_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                }
                timelast = Date::now();
            }
        };

        let hive_joiner = async {
            let mut timelast = Date::now();
            loop {
                let mut handshake_joiner = Vec::new();

                #[allow(irrefutable_let_patterns)]
                while let that = connections_instructions_chan_incoming.try_recv() {
                    if !that.is_err() {
                        let handle = async {
                            web_sys::console::log_1(&JsValue::from(format!(
                                "Entering Handshake Joiner"
                            )));
                            let (bzzaddr0, bootn, _dialat) = that.unwrap();

                            let addr3 =
                                libp2p::core::Multiaddr::try_from(bzzaddr0.underlay).unwrap();
                            let id = match try_from_multiaddr(&addr3) {
                                Some(aok) => aok,
                                _ => return,
                            };

                            let nid: u64;
                            {
                                let nid0 = self.network_id.lock().await.clone();
                                nid = nid0.clone();
                            }

                            if bootn {
                                let mut bootnodes_set = wings.bootnodes.lock().await;
                                bootnodes_set.insert(id.to_string());
                            }

                            // if dialat + 1000 > now {
                            //     // 6400
                            //     async_std::task::sleep(Duration::from_millis(1000 + dialat - now))
                            //         .await;
                            // }

                            let self_ephemeral: Multiaddr = loop {
                                {
                                    let map = wings.self_ephemerals.lock().await;
                                    if let Some(addr) = map.get(&id) {
                                        break addr.clone(); // found it, break the loop with the cloned address
                                    }
                                }
                                async_std::task::sleep(Duration::from_millis(100)).await;
                            };

                            connection_handler(
                                id,
                                nid,
                                self_ephemeral,
                                ctrl3.clone(),
                                &addr3.clone(),
                                &(*self.secret_key.lock().await),
                                &accounting_peer_chan_outgoing.clone(),
                            )
                            .await;
                        };
                        handshake_joiner.push(handle);
                    } else {
                        break;
                    }
                }
                join_all(handshake_joiner).await;

                let timenow = Date::now();
                let seg = timenow - timelast;
                if seg < PROTO_LOOP_INTERRUPTOR {
                    async_std::task::sleep(Duration::from_millis(
                        (PROTO_LOOP_INTERRUPTOR - seg) as u64,
                    ))
                    .await;
                }
                timelast = Date::now();
            }
        };

        join!(
            event_handle,
            retrieve_handle,
            retrieve_data_handle,
            retrieve_chunk_handle,
            push_handle,
            push_data_handle,
            push_chunk_handle,
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
