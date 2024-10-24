#![cfg(target_arch = "wasm32")]
// #![allow(warnings)]

use alloy::primitives::keccak256;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;

use anyhow::Result;
use byteorder::ByteOrder;
use futures::join;
use prost::Message;
use rand::rngs::OsRng;

use std::io;
use std::io::Cursor;
use std::num::NonZero;
use std::str::FromStr;
use std::time::Duration;

use libp2p::{
    autonat,
    core::{self, Multiaddr, Transport},
    dcutr,
    futures::{AsyncReadExt, AsyncWriteExt, StreamExt},
    identify, identity,
    identity::ecdsa,
    noise, ping,
    swarm::NetworkBehaviour,
    websocket_websys, yamux, PeerId, Stream, StreamProtocol,
};
use libp2p_stream as stream;

use wasm_bindgen::{prelude::*, JsValue};
use web_sys::{Document, HtmlElement};

// mod conventions;
// use conventions::a;

const HANDSHAKE_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/handshake/12.0.0/handshake");
const PRICING_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pricing/1.0.0/pricing");

// const GOSSIP_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/hive/1.1.0/peers");
// const PSEUDOSETTLE_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pseudosettle/1.0.0/pseudosettle");
// const PINGPONG_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pingpong/1.0.0/pingpong");
// const SWAP_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/swap/1.0.0/swap");
// const STATUS_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/status/1.1.1/status");
//
// const PULL_CURSORS_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pullsync/1.4.0/cursors");
// const PULL_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pullsync/1.4.0/pullsync");
// const PUSH_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/pushsync/1.3.0/pushsync");
// const RETRIEVAL_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/retrieval/1.4.0/retrieval");

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

use weeb_3::etiquette_0;
use weeb_3::etiquette_1;
// use weeb_3::etiquette_2;
// use weeb_3::etiquette_3;
use weeb_3::etiquette_4;
// use weeb_3::etiquette_5;
// use weeb_3::etiquette_6;

#[wasm_bindgen]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub async fn run(libp2p_endpoint: String) -> Result<(), JsError> {
    tracing_wasm::set_as_global_default(); // uncomment to turn on tracing
    init_panic_hook();

    let ping_duration = Duration::from_secs(60);

    let body = Body::from_current_window()?;
    body.append_p(&format!("Attempt to establish connection over websocket"))?;

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
                .with_idle_connection_timeout(ping_duration)
                .with_max_negotiating_inbound_streams(NonZero::new(10000_usize).unwrap().into())
                .with_per_connection_event_buffer_size(10000_usize)
                .with_notify_handler_buffer_size(NonZero::new(10000_usize).unwrap().into())
        })
        .build();

    let addr2 = "/ip4/192.168.1.42/tcp/1634/ws/p2p/QmYa9hasbJKBoTpfthcisMPKyGMCidfT1R4VkaRpg14bWP"
        .parse::<Multiaddr>()
        .unwrap();
    swarm.dial(addr2.clone()).unwrap();

    let mut ctrl = swarm.behaviour_mut().stream.new_control();

    let mut incoming_pricing_streams = swarm
        .behaviour_mut()
        .stream
        .new_control()
        .accept(PRICING_PROTOCOL)
        .unwrap();

    let pricing_inbound_handle = async move {
        web_sys::console::log_1(&JsValue::from(format!("Opened Pricing handler 1")));
        while let Some((peer, stream)) = incoming_pricing_streams.next().await {
            web_sys::console::log_1(&JsValue::from(format!("Entered Pricing handler 1")));
            pricing_handler(peer, stream).await.unwrap();
        }
    };

    body.append_p(&format!("establish connection over websocket"))?;

    let addr = libp2p_endpoint.parse::<Multiaddr>()?;

    let conn_handle =
        async { connection_handler(peer_id, &mut ctrl, &addr.clone(), &secret_key).await };

    // swarm.behaviour_mut().identify.push([peer_id]);

    let event_handle = async {
        loop {
            let event = swarm.next().await.unwrap();
            web_sys::console::log_1(&JsValue::from(format!(
                "Current Event Handled {:#?}",
                event
            )));
        }
    };

    join!(event_handle, conn_handle, pricing_inbound_handle);

    web_sys::console::log_1(&JsValue::from(format!("Dropping All handlers")));

    Ok(())
}

async fn connection_handler(
    peer: PeerId,
    control: &mut stream::Control,
    a: &libp2p::core::Multiaddr,
    pk: &ecdsa::SecretKey,
) {
    let mut stream = match control.open_stream(peer, HANDSHAKE_PROTOCOL).await {
        Ok(stream) => stream,
        Err(error @ stream::OpenStreamError::UnsupportedProtocol(_)) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            return;
        }
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!("{} {}", peer, error)));
            return;
        }
    };

    if let Err(e) = ceive(&mut stream, &control, a.clone(), &pk.clone()).await {
        web_sys::console::log_1(&JsValue::from("Handshake protocol failed"));
        web_sys::console::log_1(&JsValue::from(format!("{}", e)));
        return;
    }

    web_sys::console::log_1(&JsValue::from(format!("{} Handshake complete!", peer)));

    web_sys::console::log_1(&JsValue::from(format!("Closing handler 1")));
}

async fn ceive(
    stream: &mut Stream,
    _control: &stream::Control,
    a: libp2p::core::Multiaddr,
    pk: &ecdsa::SecretKey,
) -> io::Result<()> {
    let mut step_0 = etiquette_1::Syn::default();

    step_0.observed_underlay = a.clone().to_vec();

    let mut bufw_0 = Vec::new();

    let step_0_len = step_0.encoded_len();

    bufw_0.reserve(step_0_len + prost::length_delimiter_len(step_0_len));
    step_0.encode_length_delimited(&mut bufw_0).unwrap();

    stream.write_all(&bufw_0).await?;
    stream.flush().await.unwrap();

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let rec_0 =
        etiquette_1::SynAck::decode_length_delimited(&mut Cursor::new(buf_nondiscard_0)).unwrap();

    let underlay = libp2p::core::Multiaddr::try_from(rec_0.syn.unwrap().observed_underlay).unwrap();

    web_sys::console::log_1(&JsValue::from(format!("Got underlay {}!", underlay)));

    let mut step_1 = etiquette_1::Ack::default();

    let signer: PrivateKeySigner = PrivateKeySigner::from_slice(&pk.to_bytes()).unwrap();
    let addrep = signer.address();
    let addre = addrep.to_vec();

    let mut bufidl: [u8; 8] = [0; 8];
    byteorder::LittleEndian::write_u64(&mut bufidl, 10_u64);
    let byteslice = [addre.as_slice(), &bufidl].concat();
    let nonce: [u8; 32] = [0; 32];
    let byteslice2 = [byteslice, (&nonce).to_vec()].concat();
    let overlayp = keccak256(byteslice2);
    let overlay = &overlayp;

    let hsprefix: &[u8] = &"bee-handshake-".to_string().into_bytes();

    let mut bufidb: [u8; 8] = [0; 8];
    byteorder::BigEndian::write_u64(&mut bufidb, 10_u64);
    let byteslice3 = [hsprefix.to_vec(), underlay.to_vec()].concat();
    let byteslice4 = [byteslice3, overlay.to_vec()].concat();
    let byteslice5 = [byteslice4, bufidb.to_vec()].concat();

    let signature = signer.sign_message(&byteslice5).await.unwrap();

    let mut step_1_ad = etiquette_1::BzzAddress::default();

    step_1_ad.overlay = overlay.to_vec();
    step_1_ad.underlay = underlay.to_vec();
    step_1_ad.signature = signature.as_bytes().to_vec();

    step_1.address = Some(step_1_ad);
    step_1.nonce = nonce.to_vec();
    step_1.network_id = 10_u64;
    step_1.full_node = false;
    step_1.welcome_message = "... Ara Ara ...".to_string();

    let mut bufw_1 = Vec::new();

    let step_1_len = step_1.encoded_len();

    bufw_1.reserve(step_1_len + prost::length_delimiter_len(step_1_len));
    step_1.encode_length_delimited(&mut bufw_1).unwrap();
    stream.write_all(&bufw_1).await?;

    stream.flush().await.unwrap();
    stream.close().await.unwrap();

    Ok(())
}

async fn pricing_handler(_peer: PeerId, mut stream: Stream) -> io::Result<()> {
    web_sys::console::log_1(&JsValue::from(format!(
        "Opened Pricing handle 2 for peer !",
    )));

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let empty = etiquette_0::Headers::default();

    let mut buf_empty = Vec::new();

    let empty_len = empty.encoded_len();
    buf_empty.reserve(empty_len + prost::length_delimiter_len(empty_len));
    empty.encode_length_delimited(&mut buf_empty).unwrap();

    stream.write_all(&buf_empty).await?;
    stream.flush().await.unwrap();

    let mut buf_nondiscard_0 = Vec::new();
    let mut buf_discard_0: [u8; 255] = [0; 255];
    loop {
        let n = stream.read(&mut buf_discard_0).await?;
        buf_nondiscard_0.extend_from_slice(&buf_discard_0[..n]);
        if n < 255 {
            break;
        }
    }

    let rec_0 = etiquette_4::AnnouncePaymentThreshold::decode_length_delimited(&mut Cursor::new(
        buf_nondiscard_0,
    ))
    .unwrap();

    web_sys::console::log_1(&JsValue::from(format!(
        "Got AnnouncePaymentThreshold {:#?}!",
        rec_0
    )));

    stream.flush().await.unwrap();
    stream.close().await?;

    Ok(())
}

struct Body {
    body: HtmlElement,
    document: Document,
}

impl Body {
    fn from_current_window() -> Result<Self, JsError> {
        let document = web_sys::window()
            .ok_or(js_error("no global `window` exists"))?
            .document()
            .ok_or(js_error("should have a document on window"))?;
        let body = document
            .body()
            .ok_or(js_error("document should have a body"))?;

        Ok(Self { body, document })
    }

    fn append_p(&self, msg: &str) -> Result<(), JsError> {
        let val = self
            .document
            .create_element("p")
            .map_err(|_| js_error("failed to create <p>"))?;
        val.set_text_content(Some(msg));
        self.body
            .append_child(&val)
            .map_err(|_| js_error("failed to append <p>"))?;

        Ok(())
    }
}

fn js_error(msg: &str) -> JsError {
    io::Error::new(io::ErrorKind::Other, msg).into()
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
                autonat::v2::client::Config::default().with_probe_interval(Duration::from_secs(3)),
            ),
            autonat_s: autonat::v2::server::Behaviour::new(OsRng),
            dcutr: dcutr::Behaviour::new(local_public_key.to_peer_id()),
            stream: stream::Behaviour::new(),
            identify: identify::Behaviour::new(
                identify::Config::new("/weeb".into(), local_public_key.clone())
                    .with_push_listen_addr_updates(true)
                    // .with_cache_size(10000)
                    .with_interval(Duration::from_secs(30)), // .with_cache_size(10), //
            ),
            ping: ping::Behaviour::new(ping::Config::new()),
        }
    }
}
