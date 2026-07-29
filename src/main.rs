use anyhow::Result;
use rand::thread_rng;

use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::time::Duration;

use tower_http::cors::{Any, CorsLayer};

use axum::extract::Path;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Router, http::Method, routing::get};
use axum_server::tls_rustls::RustlsConfig;

use libp2p::futures::StreamExt;
use libp2p::{
    core::Transport,
    core::muxing::StreamMuxerBox,
    multiaddr::{Multiaddr, Protocol},
    ping,
    swarm::SwarmEvent,
};
use libp2p_webrtc as webrtc;

#[allow(dead_code)]
mod stream_conventions;
use stream_conventions::{is_direct_share_route, is_document_navigation};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    //rustls::crypto::aws_lc_rs::default_provider()
    //    .install_default()
    //    .unwrap();

    rustls::crypto::ring::default_provider()
        .install_default()
        .unwrap();

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_other_transport(|id_keys| {
            Ok(webrtc::tokio::Transport::new(
                id_keys.clone(),
                webrtc::tokio::Certificate::generate(&mut thread_rng())?,
            )
            .map(|(peer_id, conn), _| (peer_id, StreamMuxerBox::new(conn))))
        })?
        .with_behaviour(|_| ping::Behaviour::default())?
        .with_swarm_config(|cfg| {
            cfg.with_idle_connection_timeout(
                Duration::from_secs(u64::MAX), // Allows us to observe the pings.
            )
        })
        .build();

    let address_webrtc = Multiaddr::from(Ipv4Addr::UNSPECIFIED)
        .with(Protocol::Udp(0))
        .with(Protocol::WebRTCDirect);

    swarm.listen_on(address_webrtc.clone())?;

    let address = loop {
        if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
            if address
                .iter()
                .any(|e| e == Protocol::Ip4(Ipv4Addr::LOCALHOST))
            {
                continue;
            }

            break address;
        }
    };

    // Serve .wasm, .js and server multiaddress over HTTP on this address.
    tokio::spawn(serve(address));

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    Ok(())
}

#[derive(rust_embed::RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/static"]
struct StaticFiles;

pub(crate) async fn serve(libp2p_transport: Multiaddr) {
    let Some(Protocol::Ip4(listen_addr)) = libp2p_transport.iter().next() else {
        panic!("Expected 1st protocol to be IP4")
    };

    let config = RustlsConfig::from_pem_file("static/cert.pem", "static/key.pem")
        .await
        .unwrap();

    let server = Router::new()
        .route("/weeb-3/", get(get_index))
        .route("/weeb-3/index.html", get(get_index))
        .route("/weeb-3/mainnet", get(get_index))
        .route("/weeb-3/mainnet/", get(get_index))
        .route("/weeb-3/testnet", get(get_index))
        .route("/weeb-3/testnet/", get(get_index))
        .route("/example.html", get(redirect_example))
        .route("/weeb-3/example.html", get(get_example))
        .route("/hls-stream-example.html", get(redirect_hls_stream_example))
        .route(
            "/weeb-3/hls-stream-example.html",
            get(get_hls_stream_example),
        )
        .route("/weeb-3/weeb_3.js", get(get_static_file_weeb_3_js))
        .route(
            "/weeb-3/weeb_3_bg.wasm",
            get(get_static_file_weeb_3_bg_wasm),
        )
        .route("/weeb-3/worker.js", get(get_static_file_worker_js))
        .route("/weeb-3/service.js", get(get_static_file_service_js))
        .route("/weeb-3/snippets/{*path}", get(get_static_snippet))
        .route("/{*wildcard}", get(get_fallback))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET]),
        );

    let socket = SocketAddr::new(IpAddr::V4(listen_addr), 8080);

    axum_server::bind_rustls(socket, config)
        .serve(server.into_make_service())
        .await
        .unwrap();
}

async fn get_index() -> Result<Html<String>, StatusCode> {
    let content = StaticFiles::get("index.html")
        .ok_or(StatusCode::NOT_FOUND)?
        .data;

    let html = std::str::from_utf8(&content)
        .expect("index.html to be valid utf8")
        .to_string();

    Ok(Html(html))
}

async fn get_example() -> Result<Html<String>, StatusCode> {
    let content = StaticFiles::get("example.html")
        .ok_or(StatusCode::NOT_FOUND)?
        .data;

    let html = std::str::from_utf8(&content)
        .expect("example.html to be valid utf8")
        .to_string();

    Ok(Html(html))
}

async fn get_hls_stream_example() -> Result<Html<String>, StatusCode> {
    let content = StaticFiles::get("hls-stream-example.html")
        .ok_or(StatusCode::NOT_FOUND)?
        .data;

    let html = std::str::from_utf8(&content)
        .expect("hls-stream-example.html to be valid utf8")
        .to_string();

    Ok(Html(html))
}

async fn redirect_example() -> Redirect {
    Redirect::temporary("/weeb-3/example.html")
}

async fn redirect_hls_stream_example() -> Redirect {
    Redirect::temporary("/weeb-3/hls-stream-example.html")
}

async fn get_static_file_weeb_3_js() -> Result<impl IntoResponse, StatusCode> {
    let content = StaticFiles::get("weeb_3.js")
        .ok_or(StatusCode::NOT_FOUND)?
        .data;
    let content_type = mime_guess::from_path("weeb_3.js")
        .first_or_octet_stream()
        .to_string();

    Ok(([(CONTENT_TYPE, content_type)], content))
}

async fn get_static_file_weeb_3_bg_wasm() -> Result<impl IntoResponse, StatusCode> {
    let content = StaticFiles::get("weeb_3_bg.wasm")
        .ok_or(StatusCode::NOT_FOUND)?
        .data;
    let content_type = mime_guess::from_path("weeb_3_bg.wasm")
        .first_or_octet_stream()
        .to_string();

    Ok(([(CONTENT_TYPE, content_type)], content))
}

async fn get_static_file_worker_js() -> Result<impl IntoResponse, StatusCode> {
    let content = StaticFiles::get("worker.js")
        .ok_or(StatusCode::NOT_FOUND)?
        .data;
    let content_type = mime_guess::from_path("worker.js")
        .first_or_octet_stream()
        .to_string();

    Ok(([(CONTENT_TYPE, content_type)], content))
}

async fn get_static_file_service_js() -> Result<impl IntoResponse, StatusCode> {
    let content = StaticFiles::get("service.js")
        .ok_or(StatusCode::NOT_FOUND)?
        .data;
    let content_type = mime_guess::from_path("service.js")
        .first_or_octet_stream()
        .to_string();

    Ok(([(CONTENT_TYPE, content_type)], content))
}

async fn get_static_snippet(Path(path): Path<String>) -> Result<impl IntoResponse, StatusCode> {
    if !path.ends_with(".js")
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains('\\'))
    {
        return Err(StatusCode::NOT_FOUND);
    }

    let embedded_path = format!("snippets/{}", path);
    let content = StaticFiles::get(&embedded_path)
        .ok_or(StatusCode::NOT_FOUND)?
        .data;
    let content_type = mime_guess::from_path(&embedded_path)
        .first_or_octet_stream()
        .to_string();

    Ok(([(CONTENT_TYPE, content_type)], content))
}

async fn get_fallback(uri: Uri, headers: HeaderMap) -> Result<Response, StatusCode> {
    let fetch_mode = headers
        .get("sec-fetch-mode")
        .and_then(|value| value.to_str().ok());
    let fetch_destination = headers
        .get("sec-fetch-dest")
        .and_then(|value| value.to_str().ok());
    let accept = headers.get(ACCEPT).and_then(|value| value.to_str().ok());

    if is_direct_share_route(uri.path())
        && is_document_navigation("GET", fetch_mode, fetch_destination, accept)
    {
        Ok(get_index().await?.into_response())
    } else {
        get_404().await
    }
}

async fn get_404() -> Result<Response, StatusCode> {
    let content = StaticFiles::get("404.html")
        .ok_or(StatusCode::NOT_FOUND)?
        .data;

    let html = std::str::from_utf8(&content)
        .expect("404.html to be valid utf8")
        .to_string();

    Ok((StatusCode::NOT_FOUND, Html(html)).into_response())
}
