#![cfg(target_arch = "wasm32")]

use alloy::primitives::keccak256;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;

use byteorder::ByteOrder;

use anyhow::Result;
use futures::join;
use libp2p::identity::ecdsa;
use libp2p::{
    autonat,
    core::Multiaddr,
    futures::{AsyncReadExt, AsyncWriteExt, StreamExt},
    identify, identity,
    swarm::NetworkBehaviour,
    PeerId, Stream, StreamProtocol,
};
use libp2p_stream as stream;
use libp2p_webrtc_websys as webrtc_websys;
use prost::Message;
use rand::rngs::OsRng;
use std::io;
use std::io::Cursor;
use std::str::FromStr;
use std::time::Duration;
use wasm_bindgen::{prelude::*, JsValue};
use web_sys::{Document, HtmlElement};

// mod conventions;
// use conventions::a;

const HANDSHAKE_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/handshake/12.0.0/handshake");

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

// use weeb_3::etiquette_0;
use weeb_3::etiquette_1;
// use weeb_3::etiquette_2;
// use weeb_3::etiquette_3;
// use weeb_3::etiquette_4;
// use weeb_3::etiquette_5;
// use weeb_3::etiquette_6;

#[wasm_bindgen]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub async fn run(libp2p_endpoint: String) -> Result<(), JsError> {
    // tracing_wasm::set_as_global_default(); // uncomment to turn on tracing
    init_panic_hook();
    let ping_duration = Duration::from_secs(60);

    let body = Body::from_current_window()?;
    body.append_p(&format!("Attempt to establish connection over webrtc"))?;

    let peer_id =
        libp2p::PeerId::from_str("QmVne42GS4QKBg48bHrmotcC8TjqmMyg2ehkCbstUT5tSN").unwrap();

    let secret_key_o = ecdsa::SecretKey::generate();
    let secret_key = secret_key_o.clone();
    let keypair: ecdsa::Keypair = secret_key_o.into(); // libp2p::identity::Keypair::generate_ecdsa();

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair.clone().into())
        .with_wasm_bindgen()
        .with_other_transport(|key| {
            webrtc_websys::Transport::new(webrtc_websys::Config::new(&key))
        })?
        .with_behaviour(|key| Behaviour::new(key.public()))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(ping_duration))
        .build();

    let addr = libp2p_endpoint.parse::<Multiaddr>()?;

    swarm.dial(addr.clone()).unwrap();

    let ctrl = swarm.behaviour().stream.new_control();

    body.append_p(&format!("establish connection over webrtc"))?;

    let conn_handle = async { connection_handler(peer_id, ctrl, &addr.clone(), &secret_key).await };

    let event_handle = async {
        loop {
            let event = swarm.next().await.expect("never terminates");
            web_sys::console::log_1(&JsValue::from(format!("{:#?}", event)))
        }
    };

    join!(conn_handle, event_handle);

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

async fn connection_handler(
    peer: PeerId,
    mut control: stream::Control,
    a: &libp2p::core::Multiaddr,
    pk: &ecdsa::SecretKey,
) {
    let stream = match control.open_stream(peer, HANDSHAKE_PROTOCOL).await {
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

    if let Err(e) = ceive(stream, a.clone(), &pk.clone()).await {
        web_sys::console::log_1(&JsValue::from("Handshake protocol failed"));
        web_sys::console::log_1(&JsValue::from(format!("{}", e)));
        return;
    }

    web_sys::console::log_1(&JsValue::from(format!("{} Handshake complete!", peer)));
}

async fn ceive(
    mut stream: Stream,
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

    let mut bufw_1 = Vec::new();

    let step_1_len = step_1.encoded_len();

    bufw_1.reserve(step_1_len + prost::length_delimiter_len(step_1_len));
    step_1.encode_length_delimited(&mut bufw_1).unwrap();
    stream.write_all(&bufw_1).await?;
    stream.flush().await.unwrap();

    stream.close().await?;

    Ok(())
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    autonat: autonat::v2::client::Behaviour,
    autonat_s: autonat::v2::server::Behaviour,
    identify: identify::Behaviour,
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
            identify: identify::Behaviour::new(identify::Config::new(
                "/_.../6.3.3".into(),
                local_public_key.clone(),
            )),
            stream: stream::Behaviour::new(),
        }
    }
}
