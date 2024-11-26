#![allow(warnings)]
#![cfg(target_arch = "wasm32")]

use anyhow::Result;
use console_error_panic_hook;
use rand::rngs::OsRng;

// use num::bigint::BigInt;

use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZero;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use libp2p::{
    autonat,
    core::{self, Multiaddr, Transport},
    dcutr,
    futures::{join, StreamExt},
    identify, identity,
    identity::{ecdsa, ecdsa::SecretKey},
    noise, ping,
    swarm::{NetworkBehaviour, SwarmEvent},
    websocket_websys, yamux, PeerId, StreamProtocol, Swarm,
};
use libp2p_stream as stream;

use js_sys::Date;
use wasm_bindgen::{prelude::*, JsValue};
use web_sys::{console, HtmlElement, HtmlInputElement, MessageEvent, SharedWorker};

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

const RETRIEVE_ROUND_TIME: f64 = 200.0;
const EVENT_LOOP_INTERRUPTOR: f64 = 200.0;
const PROTO_LOOP_INTERRUPTOR: f64 = 200.0;

#[wasm_bindgen]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub async fn interweeb(_st: String) -> Result<(), JsError> {
    let body = Body::from_current_window()?;
    body.append_p(&format!("Initiating weeb worker:"))?;

    let worker_handle = Rc::new(RefCell::new(
        SharedWorker::new_with_worker_options("./worker.js", &{
            let opts = web_sys::WorkerOptions::new();
            opts.set_type(web_sys::WorkerType::Module);
            opts
        })
        .unwrap(),
    ));

    // Pass the worker to the function which sets up the `oninput` callback.
    body.append_p(&format!("Initializing interface:"))?;
    {
        let worker_handle_2 = &*worker_handle.borrow();
        let port = worker_handle_2.port();
        let _qxy = port.start();
    }

    let document = web_sys::window().unwrap().document().unwrap();

    #[allow(unused_assignments)]
    let mut persistent_callback_handle = get_on_msg_callback();

    let callback =
        wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_msg| {
            console::log_1(&"yyeyyyoninput callback triggered".into());
            let document = web_sys::window().unwrap().document().unwrap();

            let input_field = document
                .get_element_by_id("inputString")
                .expect("#inputString should exist");
            let input_field = input_field
                .dyn_ref::<HtmlInputElement>()
                .expect("#inputString should be a HtmlInputElement");

            // If the value in the field can be parsed to a `i32`, send it to the
            // worker. Otherwise clear the result field.
            match input_field.value().parse::<String>() {
                Ok(text) => {
                    console::log_1(&"yyeyyy oninput callback string".into());
                    // Access worker behind shared handle, following the interior
                    // mutability pattern.
                    let worker_handle_2 = worker_handle.borrow();
                    let _ = worker_handle_2.port().post_message(&text.into());
                    persistent_callback_handle = get_on_msg_callback();

                    // Since the worker returns the message asynchronously, we
                    // attach a callback to be triggered when the worker returns.
                    worker_handle_2
                        .port()
                        .set_onmessage(Some(persistent_callback_handle.as_ref().unchecked_ref()));

                    console::log_1(&"yyeyyy oninput callback happened".into());
                }
                Err(_) => {
                    document
                        .get_element_by_id("resultField")
                        .expect("#resultField should exist")
                        .dyn_ref::<HtmlElement>()
                        .expect("#resultField should be a HtmlElement")
                        .set_inner_text("insxyk");
                }
            }
        });

    document
        .get_element_by_id("inputString")
        .expect("#inputString should exist")
        .dyn_ref::<HtmlInputElement>()
        .expect("#inputString should be a HtmlInputElement")
        .set_oninput(Some(callback.as_ref().unchecked_ref()));

    body.append_p(&format!("Created a new worker from within Wasm"))?;

    loop {
        async_std::task::sleep(Duration::from_secs(60)).await
    }

    Ok(())
}

fn get_on_msg_callback() -> Closure<dyn FnMut(MessageEvent)> {
    Closure::new(move |event: MessageEvent| {
        web_sys::console::log_2(&"Received response: ".into(), &event.data());

        let document = web_sys::window().unwrap().document().unwrap();
        document
            .get_element_by_id("resultField")
            .expect("#resultField should exist")
            .dyn_ref::<HtmlElement>()
            .expect("#resultField should be a HtmlInputElement")
            .set_inner_text(&format!("{:#?}", event));
    })
}

#[wasm_bindgen]
pub struct Sekirei {
    swarm: Mutex<Swarm<Behaviour>>,
    secret_key: Mutex<SecretKey>,
    wings: Mutex<Wings>,
    message_port: (
        mpsc::Sender<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
        mpsc::Receiver<(Vec<u8>, mpsc::Sender<Vec<u8>>)>,
    ),
}

#[wasm_bindgen]
pub struct Wings {
    connected_peers: Mutex<HashMap<PeerId, PeerFile>>,
    overlay_peers: Mutex<HashMap<String, PeerId>>,
    accounting_peers: Mutex<HashMap<PeerId, Mutex<PeerAccounting>>>,
}

#[wasm_bindgen]
impl Sekirei {
    pub async fn acquire(&self, address: String) -> Vec<u8> {
        let (chan_out, chan_in) = mpsc::channel::<Vec<u8>>();
        let valaddr_0 = hex::decode(address);
        let valaddr = match valaddr_0 {
            Ok(hex) => hex,
            _ => return vec![],
        };

        let _ = self.message_port.0.send((valaddr, chan_out));

        // 3ab408eea4f095bde55c1caeeac8e7fcff49477660f0a28f652f0a6d9c60d05f
        let k0 = async {
            let mut timelast = Date::now();
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

        web_sys::console::log_1(&JsValue::from(format!("sydh {:#?}", _st)));

        let idle_duration = Duration::from_secs(60);

        // let body = Body::from_current_window()?;
        // body.append_p(&format!("Attempt to establish connection over websocket"))?;

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

        let addr =
            "/ip4/192.168.100.251/tcp/11634/ws/p2p/QmYa9hasbJKBoTpfthcisMPKyGMCidfT1R4VkaRpg14bWP"
                .parse::<Multiaddr>()
                .unwrap();

        swarm.dial(addr.clone()).unwrap();

        let connected_peers: Mutex<HashMap<PeerId, PeerFile>> = Mutex::new(HashMap::new());
        let overlay_peers: Mutex<HashMap<String, PeerId>> = Mutex::new(HashMap::new());
        let accounting_peers: Mutex<HashMap<PeerId, Mutex<PeerAccounting>>> =
            Mutex::new(HashMap::new());

        let (m_out, m_in) = mpsc::channel::<(Vec<u8>, mpsc::Sender<Vec<u8>>)>();

        return Sekirei {
            secret_key: Mutex::new(secret_key),
            swarm: Mutex::new(swarm),
            wings: Mutex::new(Wings {
                connected_peers: connected_peers,
                overlay_peers: overlay_peers,
                accounting_peers: accounting_peers,
            }),
            message_port: (m_out, m_in),
        };
    }

    pub async fn run(&self, _st: String) -> () {
        init_panic_hook();

        let wings = self.wings.lock().unwrap();

        let peer_id =
            libp2p::PeerId::from_str("QmYa9hasbJKBoTpfthcisMPKyGMCidfT1R4VkaRpg14bWP").unwrap();

        let addr2 =
            "/ip4/192.168.100.251/tcp/11634/ws/p2p/QmYa9hasbJKBoTpfthcisMPKyGMCidfT1R4VkaRpg14bWP"
                .parse::<Multiaddr>()
                .unwrap();

        let (peers_instructions_chan_outgoing, peers_instructions_chan_incoming) = mpsc::channel();
        let (connections_instructions_chan_outgoing, connections_instructions_chan_incoming) =
            mpsc::channel::<etiquette_2::BzzAddress>();

        let (accounting_peer_chan_outgoing, accounting_peer_chan_incoming) = mpsc::channel();

        let (pricing_chan_outgoing, pricing_chan_incoming) = mpsc::channel::<(PeerId, u64)>();

        let (refreshment_instructions_chan_outgoing, refreshment_instructions_chan_incoming) =
            mpsc::channel::<(PeerId, u64)>();

        let (refreshment_chan_outgoing, refreshment_chan_incoming) =
            mpsc::channel::<(PeerId, u64)>();

        let mut swarm = self.swarm.lock().unwrap();
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
                &self.secret_key.lock().unwrap(),
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
                            libp2p::core::Multiaddr::try_from(paddr.clone().unwrap().underlay)
                                .unwrap();
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
                let k0 = async {
                    while let that = connections_instructions_chan_incoming.try_recv() {
                        if !that.is_err() {
                            let addr3 =
                                libp2p::core::Multiaddr::try_from(that.unwrap().underlay).unwrap();
                            let id = try_from_multiaddr(&addr3);
                            if id.is_some() {
                                connection_handler(
                                    id.expect("not"),
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
                    while let re_out = refreshment_instructions_chan_incoming.try_recv() {
                        if !re_out.is_err() {
                            web_sys::console::log_1(&JsValue::from(format!("Refresh attempt")));
                            let (peer, amount) = re_out.unwrap();
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
                                refresh_handler(
                                    peer,
                                    amount,
                                    &mut ctrl4,
                                    &refreshment_chan_outgoing,
                                )
                                .await;
                            }
                        } else {
                            break;
                        }
                    }
                };

                let k4 = async {
                    while let re_in = refreshment_chan_incoming.try_recv() {
                        if !re_in.is_err() {
                            let (peer, amount) = re_in.unwrap();
                            let accounting = wings.accounting_peers.lock().unwrap();
                            let accounting_peer = accounting.get(&peer).unwrap();
                            apply_refreshment(accounting_peer, amount);
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
                while let incoming_request = self.message_port.1.try_recv() {
                    if !incoming_request.is_err() {
                        web_sys::console::log_1(&JsValue::from(format!("retrieve triggered")));
                        let (n, chan) = incoming_request.unwrap();
                        let chunk_data = retrieve_chunk(
                            &n,
                            &mut ctrl5,
                            &wings.overlay_peers,
                            &wings.accounting_peers,
                            &refreshment_instructions_chan_outgoing,
                        )
                        .await;
                        web_sys::console::log_1(&JsValue::from(format!(
                            "Writing response to interface request"
                        )));

                        chan.send(chunk_data).unwrap();
                    } else {
                        break;
                    }
                }

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

        join!(
            conn_handle,
            event_handle,
            retrieve_handle,
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
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(16))),
            stream: stream::Behaviour::new(),
        }
    }
}
