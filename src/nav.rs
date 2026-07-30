#[cfg(target_arch = "wasm32")]
use web_sys::window;

use crate::{
    network_profile::NetworkMode,
    stream_conventions::{STREAMING_ROUTE_BASE, parse_stream_share_link},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResourceRoute {
    Bzz(String),
    Bytes(String),
    Chunks(String),
    Hls { owner: String, topic: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NetworkedResourceRoute {
    pub network: NetworkMode,
    pub resource: ResourceRoute,
}

fn strip_query(input: &str) -> &str {
    let query = input.find('?').unwrap_or(input.len());
    &input[..query]
}

fn trim_route_prefix(input: &str) -> String {
    let mut route = strip_query(input).trim().replace('\\', "/");

    if let Some(hash_path) = route.find("#/") {
        route = route[hash_path + 2..].to_string();
    } else if route.starts_with('#') {
        route = route[1..].to_string();
    } else if let Some(hash_path) = route.find('#') {
        route.truncate(hash_path);
    }

    if let Some(scheme) = route.find("://") {
        let after_scheme = scheme + 3;
        if let Some(path_start) = route[after_scheme..].find('/') {
            route = route[after_scheme + path_start..].to_string();
        }
    }

    let route_base = STREAMING_ROUTE_BASE;
    let relative_base = route_base.trim_start_matches('/');
    for prefix in [
        format!("{route_base}/"),
        format!("{relative_base}/"),
        format!("{route_base}/#/"),
        format!("{relative_base}/#/"),
    ] {
        if let Some(rest) = route.strip_prefix(&prefix) {
            route = rest.to_string();
            break;
        }
    }
    if route == route_base || route == relative_base {
        route.clear();
    }

    route.trim_start_matches('/').to_string()
}

fn network_mode_segment(segment: &str) -> Option<NetworkMode> {
    match segment.trim().to_ascii_lowercase().as_str() {
        "mainnet" => Some(NetworkMode::Mainnet),
        "testnet" => Some(NetworkMode::Testnet),
        _ => None,
    }
}

fn split_transport_network(route: &str) -> (NetworkMode, &str) {
    let route = route.trim_start_matches('/');
    let mut parts = route.splitn(2, '/');
    let head = parts.next().unwrap_or_default();
    let tail = parts.next().unwrap_or_default();

    match head {
        "mainnet" => (NetworkMode::Mainnet, tail.trim_start_matches('/')),
        "testnet" => (NetworkMode::Testnet, tail.trim_start_matches('/')),
        _ => (NetworkMode::Mainnet, route),
    }
}

pub(crate) fn route_network_mode_from_path(pathname: &str) -> Option<NetworkMode> {
    if let Some(route) = parse_networked_resource_route(pathname) {
        return Some(route.network);
    }

    let route = trim_route_prefix(pathname);
    if route == "bzz" {
        return Some(NetworkMode::Mainnet);
    }
    if !route.contains('/') {
        return network_mode_segment(&route);
    }

    let (network, route) = split_transport_network(&route);
    if route == "bzz" || is_hls_bytes_route(route) {
        Some(network)
    } else {
        None
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn route_network_mode_from_location() -> Option<NetworkMode> {
    let window = window()?;
    let location = window.location();
    let pathname = location.pathname().ok()?;
    route_network_mode_from_path(&pathname)
}

fn is_reference_hex(reference: &str) -> bool {
    (reference.len() == 64 || reference.len() == 128)
        && reference.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
}

fn is_hls_bytes_route(route: &str) -> bool {
    let Some(reference) = route.strip_prefix("hls/bytes/") else {
        return false;
    };
    is_reference_hex(reference.trim_matches('/'))
}

fn parse_resource_route_body(route: &str) -> Option<ResourceRoute> {
    let mut parts = route.splitn(2, '/');
    let head = parts.next().unwrap_or_default();
    let tail = parts.next().unwrap_or_default();

    match head {
        "bzz" => {
            let resource = tail.trim_start_matches('/');
            if resource
                .split('/')
                .next()
                .map(is_reference_hex)
                .unwrap_or(false)
            {
                Some(ResourceRoute::Bzz(resource.to_string()))
            } else {
                None
            }
        }
        "bytes" => {
            let reference = tail.trim_matches('/');
            if is_reference_hex(reference) {
                Some(ResourceRoute::Bytes(reference.to_string()))
            } else {
                None
            }
        }
        "chunks" => {
            let reference = tail.trim_matches('/');
            if is_reference_hex(reference) {
                Some(ResourceRoute::Chunks(reference.to_string()))
            } else {
                None
            }
        }
        reference if is_reference_hex(reference) => Some(ResourceRoute::Bzz(route.to_string())),
        _ => None,
    }
}

pub(crate) fn parse_networked_resource_route(input: &str) -> Option<NetworkedResourceRoute> {
    let route = trim_route_prefix(input);
    if let Ok(route) = parse_stream_share_link(&route) {
        return Some(NetworkedResourceRoute {
            network: NetworkMode::Mainnet,
            resource: ResourceRoute::Hls {
                owner: route.owner,
                topic: route.topic,
            },
        });
    }

    let (network, route) = split_transport_network(&route);
    let resource = parse_resource_route_body(route)?;
    Some(NetworkedResourceRoute { network, resource })
}

pub(crate) fn parse_resource_route(input: &str) -> Option<ResourceRoute> {
    parse_networked_resource_route(input).map(|route| route.resource)
}

#[cfg(target_arch = "wasm32")]
pub async fn read_routes() -> Vec<ResourceRoute> {
    let window = match window() {
        Some(w) => w,
        None => return vec![],
    };
    let location = window.location();
    let pathname = match location.pathname() {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    parse_resource_route(&pathname).into_iter().collect()
}
