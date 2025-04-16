#![cfg(target_arch = "wasm32")]

use console_error_panic_hook;
use rand::rngs::OsRng;

use async_std::sync::Arc;

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::num::NonZero;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use tar::Archive;

use libp2p::{
    autonat,
    core::{self, Multiaddr, Transport},
    dcutr,
    futures::{
        future::join_all, //
        join,
        StreamExt,
    },
    identify, identity,
    identity::{ecdsa, ecdsa::SecretKey},
    noise, ping,
    swarm::{NetworkBehaviour, SwarmEvent},
    websocket_websys, yamux, PeerId, StreamProtocol, Swarm,
};
use libp2p_stream as stream;

use js_sys::{Date, Uint8Array};
use wasm_bindgen::{prelude::*, JsValue};
use web_sys::File;

mod accounting;
use accounting::*;

mod conventions;
use conventions::*;

mod handlers;
use handlers::*;

mod interface;

mod manifest;

mod manifest_upload;

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
}

// use crate::weeb_3::etiquette_0;
// use crate::weeb_3::etiquette_1;
use crate::weeb_3::etiquette_2;
// use crate::weeb_3::etiquette_3;
// use crate::weeb_3::etiquette_4;
// use crate::weeb_3::etiquette_5;
// use crate::weeb_3::etiquette_6;

const HANDSHAKE_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/handshake/13.0.0/handshake");
const PRICING_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pricing/1.0.0/pricing");
const GOSSIP_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/hive/1.1.0/peers");
const PSEUDOSETTLE_PROTOCOL: StreamProtocol =
    StreamProtocol::new("/swarm/pseudosettle/1.0.0/pseudosettle");
const RETRIEVAL_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/retrieval/1.4.0/retrieval");
const PUSHSYNC_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pushsync/1.3.1/pushsync");

// const PINGPONG_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pingpong/1.0.0/pingpong");
// const STATUS_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/status/1.1.1/status");
//
// const PULL_CURSORS_PROTOCOL: StreamProtocol = StreamProtocol:: "/swarm/pullsync/1.4.0/cursors");
// const PULL_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pullsync/1.4.0/pullsync");
// const PUSH_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pushsync/1.3.0/pushsync");

const PROTOCOL_ROUND_TIME: f64 = 600.0;
const EVENT_LOOP_INTERRUPTOR: f64 = 600.0;
const PROTO_LOOP_INTERRUPTOR: f64 = 600.0;

#[wasm_bindgen]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub struct Sekirei {
    swarm: Arc<Mutex<Swarm<Behaviour>>>,
    secret_key: Mutex<SecretKey>,
    wings: Mutex<Wings>,
    message_port: (
        mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
        mpsc::Receiver<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    ),
    upload_port: (
        mpsc::Sender<(Vec<(String, String, Vec<u8>)>, bool, mpsc::Sender<Vec<u8>>)>,
        mpsc::Receiver<(Vec<(String, String, Vec<u8>)>, bool, mpsc::Sender<Vec<u8>>)>,
    ),
    bootnode_port: (
        mpsc::Sender<(String, mpsc::Sender<String>)>,
        mpsc::Receiver<(String, mpsc::Sender<String>)>,
    ),
    network_id: Mutex<u64>,
}

#[wasm_bindgen]
pub struct Wings {
    connected_peers: Mutex<HashMap<PeerId, PeerFile>>,
    overlay_peers: Mutex<HashMap<String, PeerId>>,
    accounting_peers: Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
    ongoing_refreshments: Mutex<HashSet<PeerId>>,
}

#[wasm_bindgen]
impl Sekirei {
    pub async fn change_bootnode_address(&self, address: String, _id: String) -> Vec<u8> {
        let parse_id = _id.parse::<u64>();

        match parse_id {
            Ok(parsed_id) => {
                web_sys::console::log_1(&JsValue::from(format!("Parsed network id {}", parsed_id)));
                let mut nid = self.network_id.lock().unwrap();
                *nid = parsed_id;
            }
            _ => {}
        };

        web_sys::console::log_1(&"bootnode change triggered".into());

        let (chan_out, chan_in) = mpsc::channel::<String>();
        let _ = self.bootnode_port.0.send((address, chan_out));

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
    }

    pub async fn post_upload(&self, file: File, encryption: bool) -> Vec<u8> {
        web_sys::console::log_1(&JsValue::from(format!("File size {}", file.size())));

        let (chan_out, chan_in) = mpsc::channel::<Vec<u8>>();

        let f_name = file.name();
        let f_type = file.type_();

        web_sys::console::log_1(&JsValue::from(format!("File type {}", f_type)));

        let mut fvec0: Vec<(String, String, Vec<u8>)> = vec![];

        let content_buf = wasm_bindgen_futures::JsFuture::from(file.array_buffer())
            .await
            .unwrap();

        let content_u8a = Uint8Array::new(&content_buf);

        let content: Vec<u8> = content_u8a.to_vec();

        if f_type == "application/x-tar" || f_type == "application/tar" {
            // let tar = GzDecoder::new(file);
            let mut archive = Archive::new(&content[..]);

            for f0 in archive.entries().unwrap() {
                let mut f01 = match f0 {
                    Ok(aok) => aok,
                    _ => continue,
                };

                let f02path = f01.path();

                let f01path = match f02path {
                    Ok(mut aok) => aok.to_mut().clone(),
                    _ => continue,
                };

                let f0path = match f01path.into_os_string().into_string() {
                    Ok(aok) => aok,
                    _ => continue,
                };

                web_sys::console::log_1(&JsValue::from(format!("File size {}", f01.size())));
                web_sys::console::log_1(&JsValue::from(format!("File path {}", f0path)));

                let mime0 = match mime_guess::from_path(&f0path).first_raw() {
                    Some(aok) => aok.to_string(),
                    _ => continue,
                };
                // let mut fc0;
                //copy(&mut f0, &mut fc0).unwrap();

                let mut data0: Vec<u8> = vec![];

                let _ = f01.read_to_end(&mut data0);

                web_sys::console::log_1(&JsValue::from(format!("File size {}", data0.len())));

                fvec0.push((f0path, mime0, data0))
            }
        } else {
            fvec0.push((f_name, f_type, content));
        }

        let _ = self.upload_port.0.send((fvec0, encryption, chan_out));

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
                    "upload result: returned address displayed here: {} {}",
                    hex::encode(&result),
                    hex::encode([0_u8; 4])
                )
                .as_bytes()
                .to_vec(),
                "text/plain".to_string(),
                "... result ...".to_string(),
            )],
            "... result ...".to_string(),
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

        // 3ab408eea4f095bde55c1caeeac8e7fcff49477660f0a28f652f0a6d9c60d05f
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

    pub fn new(_st: String) -> Sekirei {
        // tracing_wasm::set_as_global_default(); // uncomment to turn on tracing
        init_panic_hook();

        let idle_duration = Duration::from_secs(60);

        // let body = Body::from_current_window()?;
        // body.append_p(&format!("Attempt to establish connection over websocket"))?;

        let secret_key_o = ecdsa::SecretKey::generate();
        let secret_key = secret_key_o.clone();
        let keypair: ecdsa::Keypair = secret_key_o.into();

        let swarm = libp2p::SwarmBuilder::with_existing_identity(keypair.clone().into())
            .with_wasm_bindgen()
            .with_other_transport(|key| {
                websocket_websys::Transport::default()
                    .upgrade(core::upgrade::Version::V1)
                    .authenticate(noise::Config::new(&key).unwrap())
                    .multiplex(yamux::Config::default())
                    .boxed()
            })
            .expect("Failed to create WebSocket transport")
            .with_behaviour(|key| Behaviour::new(key.public()))
            .unwrap()
            .with_swarm_config(|_| {
                libp2p::swarm::Config::with_wasm_executor()
                    .with_idle_connection_timeout(idle_duration)
                    .with_max_negotiating_inbound_streams(NonZero::new(10000_usize).unwrap().into())
                    .with_per_connection_event_buffer_size(10000_usize)
                    .with_notify_handler_buffer_size(NonZero::new(10000_usize).unwrap().into())
            })
            .build();

        let connected_peers: Mutex<HashMap<PeerId, PeerFile>> = Mutex::new(HashMap::new());
        let overlay_peers: Mutex<HashMap<String, PeerId>> = Mutex::new(HashMap::new());
        let accounting_peers: Mutex<HashMap<PeerId, Mutex<PeerAccounting>>> =
            Mutex::new(HashMap::new());
        let ongoing_refreshments: Mutex<HashSet<PeerId>> = Mutex::new(HashSet::new());

        let (m_out, m_in) = mpsc::channel::<(Vec<u8>, mpsc::Sender<Vec<u8>>)>();
        let (u_out, u_in) =
            mpsc::channel::<(Vec<(String, String, Vec<u8>)>, bool, mpsc::Sender<Vec<u8>>)>();
        let (b_out, b_in) = mpsc::channel::<(String, mpsc::Sender<String>)>();

        return Sekirei {
            secret_key: Mutex::new(secret_key),
            swarm: Arc::new(Mutex::new(swarm)),
            wings: Mutex::new(Wings {
                connected_peers: connected_peers,
                overlay_peers: overlay_peers,
                accounting_peers: accounting_peers,
                ongoing_refreshments: ongoing_refreshments,
            }),
            message_port: (m_out, m_in),
            upload_port: (u_out, u_in),
            bootnode_port: (b_out, b_in),
            network_id: Mutex::new(10_u64),
        };
    }

    pub async fn run(&self, _st: String) -> () {
        init_panic_hook();

        prt("".to_string(), "".to_string()).await;

        let wings = self.wings.lock().unwrap();

        let (peers_instructions_chan_outgoing, peers_instructions_chan_incoming) = mpsc::channel();
        let (connections_instructions_chan_outgoing, connections_instructions_chan_incoming) =
            mpsc::channel::<etiquette_2::BzzAddress>();

        let (accounting_peer_chan_outgoing, accounting_peer_chan_incoming) = mpsc::channel();

        let (pricing_chan_outgoing, pricing_chan_incoming) = mpsc::channel::<(PeerId, u64)>();

        let (refreshment_instructions_chan_outgoing, refreshment_instructions_chan_incoming) =
            mpsc::channel::<(PeerId, u64)>();

        let (refreshment_chan_outgoing, refreshment_chan_incoming) =
            mpsc::channel::<(PeerId, u64)>();

        let (data_retrieve_chan_outgoing, data_retrieve_chan_incoming) =
            mpsc::channel::<(Vec<u8>, u8, mpsc::Sender<Vec<u8>>)>();

        let (data_upload_chan_outgoing, data_upload_chan_incoming) =
            mpsc::channel::<(Vec<u8>, u8, mpsc::Sender<Vec<u8>>)>();

        let ctrl;
        let mut incoming_pricing_streams;
        let mut incoming_gossip_streams;

        {
            let mut swarm = self.swarm.lock().unwrap();
            ctrl = swarm.behaviour_mut().stream.new_control();

            incoming_pricing_streams = swarm
                .behaviour_mut()
                .stream
                .new_control()
                .accept(PRICING_PROTOCOL)
                .unwrap();

            incoming_gossip_streams = swarm
                .behaviour_mut()
                .stream
                .new_control()
                .accept(GOSSIP_PROTOCOL)
                .unwrap();
        }

        let mut ctrl3 = ctrl.clone();
        let ctrl4 = ctrl.clone();
        let ctrl6 = ctrl.clone();

        let pricing_inbound_handle = async move {
            web_sys::console::log_1(&JsValue::from(format!("Opened Pricing handler 1")));
            while let Some((peer, stream)) = incoming_pricing_streams.next().await {
                web_sys::console::log_1(&JsValue::from(format!("Entered Pricing handler 1")));
                pricing_handler(peer, stream, &pricing_chan_outgoing)
                    .await
                    .unwrap();
            }
        };

        let gossip_inbound_handle = async move {
            web_sys::console::log_1(&JsValue::from(format!("Opened Gossip handler 1")));
            while let Some((peer, stream)) = incoming_gossip_streams.next().await {
                web_sys::console::log_1(&JsValue::from(format!("Entered Gossip handler 1")));
                gossip_handler(peer, stream, &peers_instructions_chan_outgoing)
                    .await
                    .unwrap();
            }
        };

        let swarm_event_handle = async {
            let mut swarm = self.swarm.lock().unwrap();
            loop {
                #[allow(irrefutable_let_patterns)]
                while let paddr0 = peers_instructions_chan_incoming.try_recv() {
                    if !paddr0.is_err() {
                        // web_sys::console::log_1(&JsValue::from(format!(
                        //     "Current Conn Handled {:#?}",
                        //     paddr
                        // )));

                        let paddr = match paddr0 {
                            Ok(aok) => aok,
                            _ => {
                                continue;
                            }
                        };

                        let addr4 = match libp2p::core::Multiaddr::try_from(paddr.clone().underlay)
                        {
                            Ok(aok) => aok,
                            _ => {
                                continue;
                            }
                        };

                        match try_from_multiaddr(&addr4.clone()) {
                            Some(aok) => {
                                let connected_peers_map = wings.connected_peers.lock().unwrap();
                                if connected_peers_map.contains_key(&aok) {
                                    continue;
                                }
                            }
                            _ => {
                                continue;
                            }
                        };

                        let prck = addr4.protocol_stack();
                        let mut ws = false;
                        for p in prck {
                            if p == "ws" {
                                ws = true;
                            }
                        }

                        if !ws {
                            let addr40 = addr4.to_string().replace("/p2p/", "/ws/p2p/");

                            let addr41 = match addr40.parse::<Multiaddr>() {
                                Ok(aok) => aok,
                                _ => {
                                    continue;
                                }
                            };

                            let _ = swarm.dial(addr41.clone());

                            let mut bzzaddr = etiquette_2::BzzAddress::default();
                            bzzaddr.underlay = addr41.to_vec();

                            let _ = connections_instructions_chan_outgoing.send(bzzaddr);
                        } else {
                            let _ = swarm.dial(addr4);
                            let _ = connections_instructions_chan_outgoing.send(paddr);
                        }
                    } else {
                        break;
                    };
                }

                let event = async_std::future::timeout(
                    Duration::from_millis(EVENT_LOOP_INTERRUPTOR as u64),
                    swarm.next(),
                )
                .await;

                if !event.is_err() {
                    // web_sys::console::log_1(&JsValue::from(format!(
                    //     "Current Event Handled {:#?}",
                    //     event
                    // )));
                    match event.unwrap() {
                        Some(SwarmEvent::ConnectionEstablished {
                            // peer_id,
                            // established_in,
                            ..
                        }) => {
                            //
                        }
                        Some(SwarmEvent::ConnectionClosed { peer_id, .. }) => {
                            {
                                let mut connected_peers_map = wings.connected_peers.lock().unwrap();
                                let mut overlay_peers_map = wings.overlay_peers.lock().unwrap();
                                if connected_peers_map.contains_key(&peer_id) {
                                    let ol0 = hex::encode(connected_peers_map.get(&peer_id).unwrap().overlay.clone());
                                    if overlay_peers_map.contains_key(&ol0) {
                                        overlay_peers_map.remove(&ol0);
                                    };
                                    connected_peers_map.remove(&peer_id);
                                };
                            }
                            let mut accounting = wings.accounting_peers.lock().unwrap();
                            if accounting.contains_key(&peer_id) {
                                accounting.remove(&peer_id);
                            };
                        }
                        _ => {}
                    }
                }

                #[allow(irrefutable_let_patterns)]
                while let bootnode_change = self.bootnode_port.1.try_recv() {
                    if !bootnode_change.is_err() {
                        let (baddr, chan) = bootnode_change.unwrap();

                        let addr33 = match baddr.parse::<Multiaddr>() {
                            Ok(aok) => aok,
                            _ => {
                                let _ =
                                    chan.send("parse multiaddress for bootnode failed".to_string());
                                break;
                            }
                        };

                        // let bn_id: PeerId = match try_from_multiaddr(&addr33.clone()) {
                        //     Some(aok) => aok,
                        //     _ => {
                        //         let _ = chan.send("parse peerid for bootnode failed".to_string());
                        //         break;
                        //     }
                        // };

                        let _ = swarm.dial(addr33.clone());

                        let _ = chan.send("dialing bootnode".to_string());

                        let mut bzzaddr = etiquette_2::BzzAddress::default();

                        bzzaddr.underlay = addr33.to_vec();

                        let _ = connections_instructions_chan_outgoing.send(bzzaddr);
                    } else {
                        break;
                    }
                }
            }
        };

        let event_handle = async {
            let mut timelast = Date::now();
            let mut interrupt_last = Date::now();
            loop {
                let k0 = async {
                    #[allow(irrefutable_let_patterns)]
                    while let that = connections_instructions_chan_incoming.try_recv() {
                        if !that.is_err() {
                            let addr3 =
                                libp2p::core::Multiaddr::try_from(that.unwrap().underlay).unwrap();
                            let id = try_from_multiaddr(&addr3);
                            let nid: u64;
                            {
                                let nid0 = self.network_id.lock().unwrap().clone();
                                nid = nid0;
                            }

                            if id.is_some() {
                                connection_handler(
                                    id.expect("not"),
                                    nid,
                                    &mut ctrl3,
                                    &addr3.clone(),
                                    &self.secret_key.lock().unwrap(),
                                    &accounting_peer_chan_outgoing,
                                )
                                .await;
                            }
                        } else {
                            break;
                        }
                    }
                };

                let k1 = async {
                    #[allow(irrefutable_let_patterns)]
                    while let incoming_peer = accounting_peer_chan_incoming.try_recv() {
                        if !incoming_peer.is_err() {
                            // Accounting connect
                            let peer_file: PeerFile = incoming_peer.unwrap();
                            let ol = hex::encode(peer_file.overlay.clone());
                            {
                                let mut accounting = wings.accounting_peers.lock().unwrap();
                                if !accounting.contains_key(&peer_file.peer_id) {
                                    web_sys::console::log_1(&JsValue::from(format!(
                                        "Accounting Connecting Peer {:#?} {:#?}!",
                                        ol, peer_file.peer_id
                                    )));
                                    accounting.insert(
                                        peer_file.peer_id,
                                        Mutex::new(PeerAccounting {
                                            balance: 0,
                                            threshold: 0,
                                            reserve: 0,
                                            refreshment: 0.0,
                                            id: peer_file.peer_id,
                                        }),
                                    );
                                }
                            }
                            {
                                let mut overlay_peers_map = wings.overlay_peers.lock().unwrap();
                                overlay_peers_map.insert(ol, peer_file.peer_id);
                            }
                            {
                                let mut connected_peers_map = wings.connected_peers.lock().unwrap();
                                connected_peers_map.insert(peer_file.peer_id, peer_file);
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
                            let accounting = wings.accounting_peers.lock().unwrap();
                            let accounting_peer = accounting.get(&peer).unwrap();
                            set_payment_threshold(accounting_peer, amount);
                        } else {
                            break;
                        }
                    }
                };

                let k3 = async {
                    let mut refresh_joiner = Vec::new();

                    #[allow(irrefutable_let_patterns)]
                    while let re_out = refreshment_instructions_chan_incoming.try_recv() {
                        if !re_out.is_err() {
                            web_sys::console::log_1(&JsValue::from(format!("Refresh attempt")));
                            let (peer, amount) = re_out.unwrap();
                            {
                                let map = wings.ongoing_refreshments.lock().unwrap();
                                if map.contains(&peer) {
                                    continue;
                                }
                            }
                            #[allow(unused_assignments)]
                            let mut daten = Date::now();
                            let datenow = Date::now();
                            {
                                let accounting = wings.accounting_peers.lock().unwrap();
                                let accounting_peer_lock = accounting.get(&peer).unwrap();
                                let mut accounting_peer = accounting_peer_lock.lock().unwrap();
                                daten = accounting_peer.refreshment;
                                if datenow > accounting_peer.refreshment + 1000.0 {
                                    accounting_peer.refreshment = datenow;
                                }
                            }
                            if datenow > daten + 1000.0 {
                                {
                                    let mut map = wings.ongoing_refreshments.lock().unwrap();
                                    map.insert(peer);
                                }
                                let mut ctrl7 = ctrl4.clone();
                                let rco = refreshment_chan_outgoing.clone();
                                let handle = async move {
                                    refresh_handler(peer, amount, &mut ctrl7, &rco).await;
                                };
                                refresh_joiner.push(handle);
                            }
                        } else {
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
                            {
                                let accounting = wings.accounting_peers.lock().unwrap();
                                let accounting_peer = accounting.get(&peer).unwrap();
                                apply_refreshment(accounting_peer, amount);
                            }
                            let mut map = wings.ongoing_refreshments.lock().unwrap();
                            if map.contains(&peer) {
                                map.remove(&peer);
                            }
                        } else {
                            break;
                        }
                    }
                };

                join!(k0, k1, k2, k3, k4);

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
                        web_sys::console::log_1(&JsValue::from(format!("retrieve triggered")));
                        let (n, chan) = incoming_request.unwrap();
                        let encoded_data =
                            retrieve_resource(&n, &data_retrieve_chan_outgoing).await;

                        chan.send(encoded_data).unwrap();
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
                        web_sys::console::log_1(&JsValue::from(format!("push triggered")));
                        let (file0, enc, chan) = incoming_request.unwrap();
                        let mut res0: Vec<Resource> = vec![];
                        for f in file0 {
                            res0.push(Resource {
                                path0: f.0,
                                mime0: f.1,
                                data: f.2,
                                data_address: vec![],
                            })
                        }

                        let push_reference =
                            upload_resource(res0, enc, "".to_string(), &data_upload_chan_outgoing)
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
                            let mut ctrl9 = ctrl6.clone();
                            web_sys::console::log_1(&JsValue::from(format!("retrieve triggered")));
                            let (n, mode, chan) = incoming_request.unwrap();
                            if mode == 1 {
                                let chunk_data = retrieve_data(
                                    &n,
                                    &mut ctrl9,
                                    &wings.overlay_peers,
                                    &wings.accounting_peers,
                                    &refreshment_instructions_chan_outgoing,
                                )
                                .await;

                                chan.send(chunk_data).unwrap();
                            }
                            if mode == 0 {
                                let chunk_data = retrieve_chunk(
                                    &n,
                                    &mut ctrl9,
                                    &wings.overlay_peers,
                                    &wings.accounting_peers,
                                    &refreshment_instructions_chan_outgoing,
                                )
                                .await;

                                chan.send(chunk_data).unwrap();
                            }
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
                            let mut ctrl9 = ctrl6.clone();
                            web_sys::console::log_1(&JsValue::from(format!("push triggered")));
                            let (n, mode, chan) = incoming_request.unwrap();

                            if mode == 1 {
                                let batch_id =
        hex::decode("c30dd1d557008751a4c49d1d16210bac9331eebce2adf48d2f057887306e6ec0").unwrap();
                                let batch_buckets: Arc<Mutex<HashMap<u32, u32>>> =
                                    Arc::new(Mutex::new(HashMap::new()));
                                let batch_bucket_limit = 1_u32;

                                let data_reference = push_data(
                                    &n,
                                    true,
                                    batch_id,
                                    batch_buckets,
                                    batch_bucket_limit,
                                    &mut ctrl9,
                                    &wings.overlay_peers,
                                    &wings.accounting_peers,
                                    &refreshment_instructions_chan_outgoing,
                                )
                                .await;
                                web_sys::console::log_1(&JsValue::from(format!(
                                    "Writing response to encrypted push data request"
                                )));

                                chan.send(data_reference).unwrap();
                            }
                            if mode == 0 {
                                let batch_id =
        hex::decode("c30dd1d557008751a4c49d1d16210bac9331eebce2adf48d2f057887306e6ec0").unwrap();
                                let batch_buckets: Arc<Mutex<HashMap<u32, u32>>> =
                                    Arc::new(Mutex::new(HashMap::new()));
                                let batch_bucket_limit = 1_u32;

                                let chunk_reference = push_data(
                                    &n,
                                    false,
                                    batch_id,
                                    batch_buckets,
                                    batch_bucket_limit,
                                    &mut ctrl9,
                                    &wings.overlay_peers,
                                    &wings.accounting_peers,
                                    &refreshment_instructions_chan_outgoing,
                                )
                                .await;
                                web_sys::console::log_1(&JsValue::from(format!(
                                    "Writing response to unencrypted push data request"
                                )));

                                chan.send(chunk_reference).unwrap();
                            }
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

        join!(
            event_handle,
            retrieve_handle,
            push_handle,
            retrieve_data_handle,
            push_data_handle,
            swarm_event_handle,
            gossip_inbound_handle,
            pricing_inbound_handle,
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
                    .with_interval(Duration::from_secs(60)), // .with_cache_size(10), //
            ),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(50))),
            stream: stream::Behaviour::new(),
        }
    }
}
