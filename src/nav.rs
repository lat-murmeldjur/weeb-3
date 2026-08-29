#[cfg(target_arch = "wasm32")]
use web_sys::window;

use crate::{
    network_profile::NetworkMode,
    stream_conventions::{HlsStart, STREAMING_ROUTE_BASE, parse_stream_share_link},
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResourceRoute {
    Bzz(String),
    Bytes(String),
    Chunks(String),
    Hls {
        owner: String,
        topic: String,
        start: HlsStart,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NetworkedResourceRoute {
    pub network: NetworkMode,
    pub resource: ResourceRoute,
}

fn strip_query(input: &str) -> &str {
    let query = input.find('?').unwrap_or(input.len());
    &input[..query]
}

fn trim_route_prefix(input: &str) -> String {
    let route = strip_query(input).trim();
    let normalized;
    let route = if route.contains('\\') {
        normalized = route.replace('\\', "/");
        normalized.as_str()
    } else {
        route
    };

    let route = if let Some(hash_path) = route.find("#/") {
        &route[hash_path + 2..]
    } else if let Some(route) = route.strip_prefix('#') {
        route
    } else if let Some(hash_path) = route.find('#') {
        &route[..hash_path]
    } else {
        route
    };

    let route = if let Some(scheme) = route.find("://") {
        let after_scheme = scheme + 3;
        if let Some(path_start) = route[after_scheme..].find('/') {
            &route[after_scheme + path_start..]
        } else {
            route
        }
    } else {
        route
    };

    let route = route.trim_start_matches('/');
    let route_base = STREAMING_ROUTE_BASE.trim_matches('/');
    let route = if route == route_base {
        ""
    } else {
        route
            .strip_prefix(route_base)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(route)
    };
    route.trim_start_matches('/').to_string()
}

fn network_mode_segment(segment: &str) -> Option<NetworkMode> {
    let segment = segment.trim();
    if segment.eq_ignore_ascii_case("mainnet") {
        Some(NetworkMode::Mainnet)
    } else if segment.eq_ignore_ascii_case("testnet") {
        Some(NetworkMode::Testnet)
    } else {
        None
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

#[cfg(target_arch = "wasm32")]
pub(crate) fn clear_hash_route() {
    let Some(window) = window() else {
        return;
    };
    let location = window.location();
    let Ok(hash) = location.hash() else {
        return;
    };
    let Some(route) = hash.strip_prefix("#/") else {
        return;
    };
    let path = format!("{STREAMING_ROUTE_BASE}/{route}");
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&path));
    }
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
                start: route.start,
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
pub fn read_route() -> Option<ResourceRoute> {
    let window = window()?;
    let location = window.location();
    parse_resource_route(&location.pathname().ok()?)
}
