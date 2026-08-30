use std::borrow::Cow;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;

use tower_http::cors::{Any, CorsLayer};

use axum::extract::OriginalUri;
use axum::extract::Path;
use axum::http::HeaderMap;
use axum::http::HeaderName;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, IF_NONE_MATCH, RANGE,
    VARY,
};
use axum::response::{IntoResponse, Response};
use axum::{Router, http::Method, routing::get};
use axum_server::tls_rustls::RustlsConfig;

const EMBEDDED_ASSET_BUILD_VERSION: &str = env!("WEEB3_ASSET_VERSION");
const EMBEDDED_ASSET_ETAG: &str = concat!("\"", env!("WEEB3_ASSET_VERSION"), "\"");
const WEEB3_BUILD_VERSION_HEADER: HeaderName = HeaderName::from_static("x-weeb3-build-version");
const REVALIDATE_EMBEDDED_ASSET: &str = "private, no-cache";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .unwrap();

    serve(Ipv4Addr::UNSPECIFIED).await;
}

#[derive(rust_embed::RustEmbed)]
#[folder = "static"]
#[include = "404.html"]
#[include = "example.html"]
#[include = "hls-stream-example.html"]
#[include = "index.html"]
#[include = "service.js"]
#[include = "worker.js"]
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
        .route("/weeb-3/worker.js", get(get_static_file))
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

async fn get_index() -> Result<impl IntoResponse, StatusCode> {
    html_response("index.html")
}

async fn get_example(uri: OriginalUri) -> Result<impl IntoResponse, StatusCode> {
    let path = match uri.path() {
        "/example.html" => "example.html",
        "/weeb-3/hls-stream-example.html" => "hls-stream-example.html",
        _ => return Err(StatusCode::NOT_FOUND),
    };
    html_response(path)
}

fn html_response(path: &str) -> Result<Response, StatusCode> {
    let content = StaticFiles::get(path).ok_or(StatusCode::NOT_FOUND)?.data;
    Ok((
        [
            (CONTENT_TYPE, "text/html; charset=utf-8"),
            (CACHE_CONTROL, "no-store"),
            (WEEB3_BUILD_VERSION_HEADER, EMBEDDED_ASSET_BUILD_VERSION),
        ],
        content,
    )
        .into_response())
}

async fn get_stream(
    Path((owner, topic)): Path<(String, String)>,
) -> Result<impl IntoResponse, StatusCode> {
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

fn embedded_asset_is_current(headers: &HeaderMap) -> bool {
    headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|values| {
            values.split(',').map(str::trim).any(|value| {
                value == "*" || value.strip_prefix("W/").unwrap_or(value) == EMBEDDED_ASSET_ETAG
            })
        })
}

fn accepts_content_encoding(headers: &HeaderMap, encoding: &str) -> bool {
    headers
        .get_all(ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|values| values.split(','))
        .any(|value| {
            let mut parameters = value.split(';').map(str::trim);
            let accepted = parameters.next().is_some_and(|candidate| {
                candidate == "*" || candidate.eq_ignore_ascii_case(encoding)
            });
            let quality = parameters
                .find_map(|parameter| parameter.strip_prefix("q="))
                .and_then(|quality| quality.parse::<f32>().ok())
                .unwrap_or(1.0);
            accepted && quality > 0.0
        })
}

fn embedded_asset_response(
    request_headers: &HeaderMap,
    path: &str,
    content_type: &'static str,
) -> Result<Response, StatusCode> {
    let compressed = <StaticFiles as rust_embed::RustEmbed>::compressed(path);
    let identity = if compressed.is_none() {
        Some(StaticFiles::get(path).ok_or(StatusCode::NOT_FOUND)?.data)
    } else {
        None
    };
    let response_headers = [
        (CONTENT_TYPE, content_type),
        (CACHE_CONTROL, REVALIDATE_EMBEDDED_ASSET),
        (ETAG, EMBEDDED_ASSET_ETAG),
        (WEEB3_BUILD_VERSION_HEADER, EMBEDDED_ASSET_BUILD_VERSION),
        (VARY, "Accept-Encoding"),
    ];
    if embedded_asset_is_current(request_headers) {
        return Ok((StatusCode::NOT_MODIFIED, response_headers).into_response());
    }
    if let Some(compressed) = compressed.as_ref()
        && !request_headers.contains_key(RANGE)
        && accepts_content_encoding(request_headers, compressed.content_encoding())
    {
        let content: &'static [u8] = compressed.data.compressed();
        let mut response = (response_headers, Cow::Borrowed(content)).into_response();
        response.headers_mut().insert(
            CONTENT_ENCODING,
            HeaderValue::from_static(compressed.content_encoding()),
        );
        return Ok(response);
    }
    let content = match identity {
        Some(content) => content,
        None => StaticFiles::get(path).ok_or(StatusCode::NOT_FOUND)?.data,
    };
    Ok((response_headers, content).into_response())
}

async fn get_static_file(headers: HeaderMap, uri: OriginalUri) -> Result<Response, StatusCode> {
    let (path, content_type) = match uri.path() {
        "/weeb-3/weeb_3.js" => ("weeb_3.js", "text/javascript"),
        "/weeb-3/weeb_3_bg.wasm" => ("weeb_3_bg.wasm", "application/wasm"),
        "/weeb-3/service.js" => ("service.js", "text/javascript"),
        "/weeb-3/worker.js" => ("worker.js", "text/javascript"),
        _ => return Err(StatusCode::NOT_FOUND),
    };
    if matches!(path, "service.js" | "worker.js") {
        let content = StaticFiles::get(path).ok_or(StatusCode::NOT_FOUND)?.data;
        return Ok((
            [
                (CONTENT_TYPE, content_type),
                (CACHE_CONTROL, "no-store"),
                (WEEB3_BUILD_VERSION_HEADER, EMBEDDED_ASSET_BUILD_VERSION),
            ],
            content,
        )
            .into_response());
    }
    embedded_asset_response(&headers, path, content_type)
}

async fn get_static_snippet(
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Response, StatusCode> {
    if !path.ends_with(".js")
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains('\\'))
    {
        return Err(StatusCode::NOT_FOUND);
    }

    let embedded_path = format!("snippets/{path}");
    embedded_asset_response(&headers, &embedded_path, "text/javascript")
}

async fn get_404() -> Result<impl IntoResponse, StatusCode> {
    html_response("404.html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_documents_are_not_cached_and_expose_the_build_version() {
        for path in ["index.html", "example.html", "404.html"] {
            let response = html_response(path).unwrap().into_response();
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
            assert_eq!(
                response.headers()[WEEB3_BUILD_VERSION_HEADER],
                EMBEDDED_ASSET_BUILD_VERSION
            );
        }
        assert_eq!(
            html_response("missing.html").err(),
            Some(StatusCode::NOT_FOUND)
        );
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn embedded_assets_negotiate_compression_without_breaking_cache_or_range_requests() {
        let path = "worker.js";
        let encoding = StaticFiles::compressed(path).unwrap().content_encoding();
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, encoding.parse().unwrap());

        let compressed = embedded_asset_response(&headers, path, "text/javascript").unwrap();
        assert_eq!(compressed.status(), StatusCode::OK);
        assert_eq!(compressed.headers()[CONTENT_ENCODING], encoding);
        assert_eq!(compressed.headers()[CONTENT_TYPE], "text/javascript");
        assert_eq!(compressed.headers()[ETAG], EMBEDDED_ASSET_ETAG);
        assert_eq!(compressed.headers()[VARY], "Accept-Encoding");

        headers.insert(RANGE, "bytes=0-15".parse().unwrap());
        let ranged = embedded_asset_response(&headers, path, "text/javascript").unwrap();
        assert_eq!(ranged.status(), StatusCode::OK);
        assert!(!ranged.headers().contains_key(CONTENT_ENCODING));

        headers.remove(RANGE);
        headers.insert(IF_NONE_MATCH, EMBEDDED_ASSET_ETAG.parse().unwrap());
        let current = embedded_asset_response(&headers, path, "text/javascript").unwrap();
        assert_eq!(current.status(), StatusCode::NOT_MODIFIED);
        assert!(!current.headers().contains_key(CONTENT_ENCODING));

        assert_eq!(
            embedded_asset_response(&headers, "missing.js", "text/javascript").unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_embedded_assets_use_the_identity_fallback() {
        let mut headers = HeaderMap::new();
        let path = "worker.js";

        let response = embedded_asset_response(&headers, path, "text/javascript").unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(CONTENT_ENCODING));

        headers.insert(IF_NONE_MATCH, EMBEDDED_ASSET_ETAG.parse().unwrap());
        let current = embedded_asset_response(&headers, path, "text/javascript").unwrap();
        assert_eq!(current.status(), StatusCode::NOT_MODIFIED);

        assert_eq!(
            embedded_asset_response(&headers, "missing.js", "text/javascript").unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn zero_quality_disables_embedded_compression() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, "deflate;q=0, gzip".parse().unwrap());
        assert!(!accepts_content_encoding(&headers, "deflate"));
    }
}
