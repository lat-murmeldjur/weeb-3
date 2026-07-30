use anyhow::Result;

use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;

use tower_http::cors::{Any, CorsLayer};

use axum::extract::OriginalUri;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::{Html, IntoResponse};
use axum::{Router, http::Method, routing::get};
use axum_server::tls_rustls::RustlsConfig;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .unwrap();

    serve(Ipv4Addr::UNSPECIFIED).await;
    Ok(())
}

#[derive(rust_embed::RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/static"]
#[include = "404.html"]
#[include = "example.html"]
#[include = "hls-stream-example.html"]
#[include = "index.html"]
#[include = "service.js"]
#[include = "snippets/**"]
#[include = "weeb_3.js"]
#[include = "weeb_3_bg.wasm"]
struct StaticFiles;

pub(crate) async fn serve(listen_addr: Ipv4Addr) {
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
        .route("/example.html", get(get_example))
        .route("/weeb-3/hls-stream-example.html", get(get_example))
        .route("/weeb-3/stream/{owner}/{topic}", get(get_stream))
        .route("/weeb-3/live/stream/{owner}/{topic}", get(get_stream))
        .route("/weeb-3/weeb_3.js", get(get_static_file))
        .route("/weeb-3/weeb_3_bg.wasm", get(get_static_file))
        .route("/weeb-3/service.js", get(get_static_file))
        .route("/weeb-3/snippets/{*path}", get(get_static_snippet))
        .route("/{*wildcard}", get(get_404))
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

async fn get_example(uri: OriginalUri) -> Result<Html<String>, StatusCode> {
    let path = match uri.path() {
        "/example.html" => "example.html",
        "/weeb-3/hls-stream-example.html" => "hls-stream-example.html",
        _ => return Err(StatusCode::NOT_FOUND),
    };
    let content = StaticFiles::get(path).ok_or(StatusCode::NOT_FOUND)?.data;

    let html = std::str::from_utf8(&content)
        .expect("embedded HTML example to be valid utf8")
        .to_string();

    Ok(Html(html))
}

async fn get_stream(
    Path((owner, topic)): Path<(String, String)>,
) -> Result<Html<String>, StatusCode> {
    if owner.len() != 40
        || !owner.bytes().all(|byte| byte.is_ascii_hexdigit())
        || topic.is_empty()
        || topic.len() > 256
        || topic.chars().any(char::is_control)
        || matches!(topic.as_str(), "." | "..")
    {
        return Err(StatusCode::NOT_FOUND);
    }
    get_index().await
}

async fn get_static_file(uri: OriginalUri) -> Result<impl IntoResponse, StatusCode> {
    let (path, content_type) = match uri.path() {
        "/weeb-3/weeb_3.js" => ("weeb_3.js", "text/javascript"),
        "/weeb-3/weeb_3_bg.wasm" => ("weeb_3_bg.wasm", "application/wasm"),
        "/weeb-3/service.js" => ("service.js", "text/javascript"),
        _ => return Err(StatusCode::NOT_FOUND),
    };
    let content = StaticFiles::get(path).ok_or(StatusCode::NOT_FOUND)?.data;
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

    Ok(([(CONTENT_TYPE, "text/javascript")], content))
}

async fn get_404() -> Result<Html<String>, StatusCode> {
    let content = StaticFiles::get("404.html")
        .ok_or(StatusCode::NOT_FOUND)?
        .data;

    let html = std::str::from_utf8(&content)
        .expect("404.html to be valid utf8")
        .to_string();

    Ok(Html(html))
}
