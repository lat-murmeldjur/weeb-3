#![allow(warnings)]
#![cfg(target_arch = "wasm32")]

use anyhow::Result;
use console_error_panic_hook;
use rand::rngs::OsRng;
// use num::bigint::BigInt;

use std::collections::HashMap;
use std::num::NonZero;
use std::str::FromStr;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use libp2p::futures::join;
use libp2p::{
    autonat,
    core::{self, Multiaddr, Transport},
    dcutr,
    futures::StreamExt,
    identify, identity,
    identity::ecdsa,
    noise, ping,
    swarm::{NetworkBehaviour, SwarmEvent},
    websocket_websys, yamux, PeerId, StreamProtocol,
};
use libp2p_stream as stream;

use js_sys::Date;
use wasm_bindgen::{prelude::*, JsValue};

mod conventions;
use conventions::*;

mod retrieval;
use retrieval::*;

mod accounting;
use accounting::*;

mod handlers;
use handlers::*;

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
}

// use crate::weeb_3::etiquette_0;
// use crate::weeb_3::etiquette_1;
use crate::weeb_3::etiquette_2;
// use crate::weeb_3::etiquette_3;
// use crate::weeb_3::etiquette_4;
// use crate::weeb_3::etiquette_5;
// use crate::weeb_3::etiquette_6;

const HANDSHAKE_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/handshake/12.0.0/handshake");
const PRICING_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pricing/1.0.0/pricing");
const GOSSIP_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/hive/1.1.0/peers");
const PSEUDOSETTLE_PROTOCOL: StreamProtocol =
    StreamProtocol::new("/swarm/pseudosettle/1.0.0/pseudosettle");
const RETRIEVAL_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/retrieval/1.4.0/retrieval");

// const PINGPONG_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pingpong/1.0.0/pingpong");
// const STATUS_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/status/1.1.1/status");
//
// const PULL_CURSORS_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pullsync/1.4.0/cursors");
// const PULL_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pullsync/1.4.0/pullsync");
// const PUSH_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pushsync/1.3.0/pushsync");

const RETRIEVE_ROUND_TIME: f64 = 600.0;
const EVENT_LOOP_INTERRUPTOR: f64 = 200.0;
const PROTO_LOOP_INTERRUPTOR: f64 = 50.0;

#[wasm_bindgen]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub async fn run(_argument: String) -> Result<(), JsError> {
    tracing_wasm::set_as_global_default(); // uncomment to turn on tracing
    init_panic_hook();

    let idle_duration = Duration::from_secs(60);

    // let body = Body::from_current_window()?;
    // body.append_p(&format!("Attempt to establish connection over websocket"))?;

    let peer_id =
        libp2p::PeerId::from_str("QmYa9hasbJKBoTpfthcisMPKyGMCidfT1R4VkaRpg14bWP").unwrap();

    let secret_key_o = ecdsa::SecretKey::generate();
    let secret_key = secret_key_o.clone();
    let keypair: ecdsa::Keypair = secret_key_o.into();

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair.clone().into())
        .with_wasm_bindgen()
        .with_other_transport(|key| {
            websocket_websys::Transport::default()
                .upgrade(core::upgrade::Version::V1)
                .authenticate(noise::Config::new(&key).unwrap())
                .multiplex(yamux::Config::default())
                .boxed()
        })
        .expect("Failed to create WebSocket transport")
        .with_behaviour(|key| Behaviour::new(key.public()))?
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

    let (peers_instructions_chan_outgoing, peers_instructions_chan_incoming) = mpsc::channel();
    let (connections_instructions_chan_outgoing, connections_instructions_chan_incoming) =
        mpsc::channel::<etiquette_2::BzzAddress>();

    let (accounting_peer_chan_outgoing, accounting_peer_chan_incoming) = mpsc::channel();

    let (pricing_chan_outgoing, pricing_chan_incoming) = mpsc::channel::<(PeerId, u64)>();

    let (refreshment_instructions_chan_outgoing, refreshment_instructions_chan_incoming) =
        mpsc::channel::<(PeerId, u64)>();

    let (refreshment_chan_outgoing, refreshment_chan_incoming) = mpsc::channel::<(PeerId, u64)>();

    let addr = "/ip4/192.168.1.42/tcp/11634/ws/p2p/QmYa9hasbJKBoTpfthcisMPKyGMCidfT1R4VkaRpg14bWP"
        .parse::<Multiaddr>()
        .unwrap();

    let addr2 = addr.clone();
    swarm.dial(addr.clone()).unwrap();

    let mut ctrl = swarm.behaviour_mut().stream.new_control();
    let mut ctrl3 = swarm.behaviour_mut().stream.new_control();
    let mut ctrl4 = swarm.behaviour_mut().stream.new_control();
    let mut ctrl5 = swarm.behaviour_mut().stream.new_control();

    let mut incoming_pricing_streams = swarm
        .behaviour_mut()
        .stream
        .new_control()
        .accept(PRICING_PROTOCOL)
        .unwrap();

    let mut incoming_gossip_streams = swarm
        .behaviour_mut()
        .stream
        .new_control()
        .accept(GOSSIP_PROTOCOL)
        .unwrap();

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

    let conn_handle = async {
        connection_handler(
            peer_id,
            &mut ctrl,
            &addr2.clone(),
            &secret_key,
            &accounting_peer_chan_outgoing,
        )
        .await;
    };

    let swarm_event_handle = async {
        loop {
            while let paddr = peers_instructions_chan_incoming.try_recv() {
                web_sys::console::log_1(&JsValue::from(format!(
                    "Current Conn Handled {:#?}",
                    paddr
                )));
                if !paddr.is_err() {
                    let addr4 =
                        libp2p::core::Multiaddr::try_from(paddr.clone().unwrap().underlay).unwrap();
                    swarm.dial(addr4).unwrap();
                    let _ = connections_instructions_chan_outgoing.send(paddr.unwrap());
                } else {
                    break;
                };
            }

            let event = swarm.next().await.unwrap();
            match event {
                    SwarmEvent::ConnectionEstablished {
                        // peer_id,
                        // established_in,
                        ..
                    } => {
                        //
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        {
                            let mut connected_peers_map = connected_peers.lock().unwrap();
                            let mut overlay_peers_map = overlay_peers.lock().unwrap();
                            if connected_peers_map.contains_key(&peer_id) {
                                let ol0 = hex::encode(connected_peers_map.get(&peer_id).unwrap().overlay.clone());
                                if overlay_peers_map.contains_key(&ol0) {
                                    overlay_peers_map.remove(&ol0);
                                };
                                connected_peers_map.remove(&peer_id);
                            };
                        }
                        let mut accounting = accounting_peers.lock().unwrap();
                        if accounting.contains_key(&peer_id) {
                            accounting.remove(&peer_id);
                        };
                    }
                    _ => {}
                }
            web_sys::console::log_1(&JsValue::from(format!(
                "Current Event Handled {:#?}",
                event
            )));
        }
    };

    let event_handle = async {
        let mut timelast = Date::now();
        let mut interrupt_last = Date::now();
        loop {
            let timenow = Date::now();
            let seg = timenow - interrupt_last;
            if seg < EVENT_LOOP_INTERRUPTOR {
                web_sys::console::log_1(&JsValue::from(format!(
                    "Ease event handle loop for {}",
                    EVENT_LOOP_INTERRUPTOR - seg
                )));
                async_std::task::sleep(Duration::from_millis(
                    (EVENT_LOOP_INTERRUPTOR - seg) as u64,
                ))
                .await;
            }
            let timenow = Date::now();
            interrupt_last = timenow;

            // only used for retr prototype progress
            let timelast_r = timelast;

            if timelast + EVENT_LOOP_INTERRUPTOR < timenow {
                timelast = timenow
            }
            //

            let k0 = async {
                web_sys::console::log_1(&JsValue::from(format!("Got frih")));
                while let that = connections_instructions_chan_incoming.try_recv() {
                    if !that.is_err() {
                        web_sys::console::log_1(&JsValue::from(format!("Got frih2")));
                        let addr3 =
                            libp2p::core::Multiaddr::try_from(that.unwrap().underlay).unwrap();
                        let id = try_from_multiaddr(&addr3);
                        web_sys::console::log_1(&JsValue::from(format!("Got Id {:#?}", id)));
                        if id.is_some() {
                            connection_handler(
                                id.expect("not"),
                                &mut ctrl3,
                                &addr3.clone(),
                                &secret_key,
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
                while let incoming_peer = accounting_peer_chan_incoming.try_recv() {
                    if !incoming_peer.is_err() {
                        // Accounting connect
                        let peer_file: PeerFile = incoming_peer.unwrap();
                        let ol = hex::encode(peer_file.overlay.clone());
                        {
                            let mut accounting = accounting_peers.lock().unwrap();
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
                            let mut overlay_peers_map = overlay_peers.lock().unwrap();
                            overlay_peers_map.insert(ol, peer_file.peer_id);
                        }
                        {
                            let mut connected_peers_map = connected_peers.lock().unwrap();
                            connected_peers_map.insert(peer_file.peer_id, peer_file);
                        }
                    } else {
                        break;
                    }
                }
            };

            let k3 = async {
                while let pt_in = pricing_chan_incoming.try_recv() {
                    if !pt_in.is_err() {
                        let (peer, amount) = pt_in.unwrap();
                        let accounting = accounting_peers.lock().unwrap();
                        let accounting_peer = accounting.get(&peer).unwrap();
                        set_payment_threshold(accounting_peer, amount);
                    } else {
                        break;
                    }
                }
            };

            let k4 = async {
                while let re_out = refreshment_instructions_chan_incoming.try_recv() {
                    if !re_out.is_err() {
                        web_sys::console::log_1(&JsValue::from(format!("Refresh attempt")));
                        let (peer, amount) = re_out.unwrap();
                        let mut daten = Date::now();
                        let datenow = Date::now();
                        {
                            let accounting = accounting_peers.lock().unwrap();
                            let accounting_peer_lock = accounting.get(&peer).unwrap();
                            let mut accounting_peer = accounting_peer_lock.lock().unwrap();
                            daten = accounting_peer.refreshment;
                            if datenow > accounting_peer.refreshment + 1000.0 {
                                accounting_peer.refreshment = datenow;
                            }
                        }
                        if datenow > daten + 1000.0 {
                            refresh_handler(peer, amount, &mut ctrl4, &refreshment_chan_outgoing)
                                .await;
                        }
                    } else {
                        break;
                    }
                }
            };

            let k5 = async {
                while let re_in = refreshment_chan_incoming.try_recv() {
                    if !re_in.is_err() {
                        let (peer, amount) = re_in.unwrap();
                        let accounting = accounting_peers.lock().unwrap();
                        let accounting_peer = accounting.get(&peer).unwrap();
                        apply_refreshment(accounting_peer, amount);
                    } else {
                        break;
                    }
                }
            };

            join!(k0, k1, k3, k4, k5);
        }
    };

    let k2_handle = async {
        let mut timelast = Date::now();
        let mut interrupt_last = Date::now();

        loop {
            let timenow = Date::now();
            let seg = timenow - timelast;
            if seg < PROTO_LOOP_INTERRUPTOR {
                web_sys::console::log_1(&JsValue::from(format!(
                    "Ease proto event handle loop for {}",
                    PROTO_LOOP_INTERRUPTOR - seg
                )));
                async_std::task::sleep(Duration::from_millis(
                    (PROTO_LOOP_INTERRUPTOR - seg) as u64,
                ))
                .await;
            }
            let timenow = Date::now();
            interrupt_last = timenow;

            web_sys::console::log_1(&JsValue::from(format!(
                "Time Elapsed Since Last Attempt {}",
                timenow - timelast
            )));
            timelast = timenow;
            web_sys::console::log_1(&JsValue::from(format!(
                "Retrieve Chunk Attempt {:#?}",
                timenow
            )));
            retrieve_chunk(
                hex::decode("3ab408eea4f095bde55c1caeeac8e7fcff49477660f0a28f652f0a6d9c60d05f")
                    .unwrap(),
                &mut ctrl5,
                &overlay_peers,
                &accounting_peers,
                &refreshment_instructions_chan_outgoing,
            )
            .await;
        }
    };

    join!(
        conn_handle,
        event_handle,
        k2_handle,
        swarm_event_handle,
        gossip_inbound_handle,
        pricing_inbound_handle,
    );

    web_sys::console::log_1(&JsValue::from(format!("Dropping All handlers")));

    Ok(())
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
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(16))),
            stream: stream::Behaviour::new(),
        }
    }
}
