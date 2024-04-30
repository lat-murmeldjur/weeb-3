#![allow(warnings)] // not today, erosion
#![cfg(target_arch = "wasm32")]

//use libp2p::core::multiaddr::Protocol;
use libp2p::{
    autonat,
    core::Multiaddr,
    futures::{AsyncReadExt, AsyncWriteExt, StreamExt},
    identify, identity,
    multiaddr::Protocol,
    noise, ping,
    swarm::{NetworkBehaviour, SwarmEvent},
    yamux, PeerId, Stream, StreamProtocol,
};
use libp2p_stream as stream;
use libp2p_webrtc_websys as webrtc_websys;

use anyhow::{Context, Result};
use prost::Message;
use rand::RngCore;
use std::io;
use std::io::Cursor;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::thread;
use std::time::Duration;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;
use wasm_bindgen::prelude::*;
use web_sys::{Document, HtmlElement};

use secp256k1::hashes::{sha256, Hash};
use secp256k1::rand::rngs::OsRng;
use secp256k1::{Message as secMess, Secp256k1};

mod conventions;
use conventions::a;

const HANDSHAKE_PROTOCOL: StreamProtocol = StreamProtocol::new("/swarm/handshake/11.0.0/handshake");

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
use weeb_3::etiquette_2;
use weeb_3::etiquette_3;
use weeb_3::etiquette_4;
use weeb_3::etiquette_5;
use weeb_3::etiquette_6;

#[wasm_bindgen]
pub async fn run(libp2p_endpoint: String) -> Result<(), JsError> {
    tracing_wasm::set_as_global_default();

    let ping_duration = Duration::from_secs(30);

    let body = Body::from_current_window()?;
    body.append_p(&format!(
        "Let's ping the rust-libp2p server over WebRTC for {:?}:",
        ping_duration
    ))?;

    let peer_id =
        libp2p::PeerId::from_str("QmbtmtkRmmozBdTqyz4L8XFBpvAA72kxCRMMz4D7uaVwDG").unwrap();

    let keypair = libp2p::identity::Keypair::generate_ecdsa();

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair.clone())
        .with_wasm_bindgen()
        .with_other_transport(|key| {
            webrtc_websys::Transport::new(webrtc_websys::Config::new(&key))
        })?
        .with_behaviour(|key| Behaviour::new(key.public()))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(ping_duration))
        .build();

    let addr = libp2p_endpoint.parse::<Multiaddr>()?;

    swarm
        .behaviour_mut()
        .auto_nat
        .add_server(peer_id, Some(addr.clone()));

    swarm.dial(addr.clone())?;

    connection_handler(
        peer_id,
        swarm.behaviour().stream.new_control(),
        &addr.clone(),
        &keypair,
    );

    loop {
        let event = swarm.next().await.expect("never terminates");

        match event {
            libp2p::swarm::SwarmEvent::NewListenAddr { address, .. } => {
                let listen_address = address.with_p2p(*swarm.local_peer_id()).unwrap();
                tracing::info!(%listen_address);
            }
            event => {}
        }
    }

    Ok(())
}

/// Convenience wrapper around the current document body
struct Body {
    body: HtmlElement,
    document: Document,
}

impl Body {
    fn from_current_window() -> Result<Self, JsError> {
        // Use `web_sys`'s global `window` function to get a handle on the global
        // window object.
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

/// A very simple, `async fn`-based connection handler for our custom echo protocol.
async fn connection_handler(
    peer: PeerId,
    mut control: stream::Control,
    a: &libp2p::core::Multiaddr,
    k: &libp2p::identity::Keypair,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await; // Wait a second between echos.

        let stream = match control.open_stream(peer, HANDSHAKE_PROTOCOL).await {
            Ok(stream) => stream,
            Err(error @ stream::OpenStreamError::UnsupportedProtocol(_)) => {
                tracing::info!("casette 1");

                tracing::info!(%peer, %error);
                return;
            }
            Err(error) => {
                // Other errors may be temporary.
                // In production, something like an exponential backoff / circuit-breaker may be more appropriate.
                tracing::info!("casette 2");

                tracing::debug!(%peer, %error);
                continue;
            }
        };

        if let Err(e) = ceive(stream, a.clone(), k.clone()).await {
            tracing::info!("casette 3");
            tracing::warn!(%peer, "Echo protocol failed: {e}");
            continue;
        }

        tracing::info!(%peer, "Echo complete!")
    }
}

async fn echo(mut stream: Stream) -> io::Result<usize> {
    let mut total = 0;

    let mut buf = [0u8; 100];

    loop {
        let read = stream.read(&mut buf).await?;
        if read == 0 {
            return Ok(total);
        }

        total += read;
        stream.write_all(&buf[..read]).await?;
    }
}

async fn ceive(
    mut stream: Stream,
    a: libp2p::core::Multiaddr,
    k: libp2p::identity::Keypair,
) -> io::Result<()> {
    let empty = etiquette_0::Headers::default();

    let mut bufw = Vec::new();
    bufw.reserve(empty.encoded_len());
    // Unwrap is safe, since we have reserved sufficient capacity in the vector.
    empty.encode(&mut bufw).unwrap();

    stream.write_all(&bufw).await?;

    let mut buf = vec![];
    stream.read_exact(&mut buf).await?;

    let mut step_0 = etiquette_1::Syn::default();

    step_0.observed_underlay = a.clone().to_vec();

    let mut bufw_0 = Vec::new();
    bufw_0.reserve(step_0.encoded_len());

    stream.write_all(&bufw_0).await?;

    let mut buf_nondiscard_0 = vec![];
    stream.read_exact(&mut buf_nondiscard_0).await?;

    let rec_0 = etiquette_1::Syn::decode(&mut Cursor::new(buf_nondiscard_0)).unwrap();

    let underlay = libp2p::core::Multiaddr::try_from(rec_0.observed_underlay).unwrap();

    let mut step_1 = etiquette_1::Ack::default();

    // go //    networkIDBytes := make([]byte, 8)
    // go //    binary.BigEndian.PutUint64(networkIDBytes, networkID)

    let bID = 10_u64.to_be_bytes();

    // go //    signData := append([]byte("bee-handshake-"), underlay...)
    // go //    signData = append(signData, overlay...)

    let hsprefix: &[u8] = &"bee-handshake-".to_string().into_bytes();

    let part1: &[u8] = &underlay.to_vec();

    let part2: &[u8] = &underlay.to_vec();

    let x19prefix = "\x19Ethereum Signed Message:".to_string();

    // go // msg := &pb.Ack{
    // go //         Address: &pb.BzzAddress{
    // go //             Underlay:  advertisableUnderlayBytes,
    // go //             Overlay:   bzzAddress.Overlay.Bytes(),
    // go //             Signature: bzzAddress.Signature,
    // go //         },
    // go //         NetworkID:      s.networkID,
    // go //         FullNode:       s.fullNode,
    // go //         Nonce:          s.nonce,
    // go //         WelcomeMessage: welcomeMessage,
    // go //     }

    step_1.welcome_message = "...Ara Ara... ^^".to_string();

    let mut step_1_ad = etiquette_1::BzzAddress::default();

    let mut bufw_1 = Vec::new();
    bufw_1.reserve(step_1.encoded_len());

    stream.write_all(&bufw_1).await?;

    stream.close().await?;

    Ok(())
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    identify: identify::Behaviour,
    auto_nat: autonat::Behaviour,
    stream: stream::Behaviour,
}

impl Behaviour {
    fn new(local_public_key: identity::PublicKey) -> Self {
        Self {
            identify: identify::Behaviour::new(identify::Config::new(
                "/_.../6.3.3".into(),
                local_public_key.clone(),
            )),
            auto_nat: autonat::Behaviour::new(
                local_public_key.to_peer_id(),
                autonat::Config {
                    retry_interval: Duration::from_secs(2),
                    refresh_interval: Duration::from_secs(5),
                    use_connected: true,
                    boot_delay: Duration::from_secs(1),
                    throttle_server_period: Duration::ZERO,
                    only_global_ips: false,
                    ..Default::default()
                },
            ),
            stream: stream::Behaviour::new(),
        }
    }
}
