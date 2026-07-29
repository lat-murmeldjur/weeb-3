#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;
#[cfg(target_arch = "wasm32")]
use web_sys::window;

use crate::{
    network_profile::NetworkMode,
    stream_conventions::streaming_route_base,
    stream_hls::{StreamShareNetwork, parse_stream_share_link},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResourceRoute {
    Bzz(String),
    Bytes(String),
    Chunks(String),
    Feed {
        owner: String,
        topic: String,
    },
    Hls {
        media_type: String,
        owner: String,
        topic: String,
        index: Option<u64>,
    },
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

    let route_base = streaming_route_base();
    if !route_base.is_empty() {
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
    }

    route.trim_start_matches('/').to_string()
}

fn network_mode_segment(segment: &str) -> Option<NetworkMode> {
    match segment.trim().to_ascii_lowercase().as_str() {
        "mainnet" | "gnosis" | "gnosischain" | "1" => Some(NetworkMode::Mainnet),
        "testnet" | "sepolia" | "10" => Some(NetworkMode::Testnet),
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
    let route = route.strip_prefix("read/").unwrap_or(route);
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

fn is_feed_owner(owner: &str) -> bool {
    let owner = owner
        .strip_prefix("0x")
        .or_else(|| owner.strip_prefix("0X"))
        .unwrap_or(owner);
    owner.len() == 40 && owner.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
}

fn feed_identity(tail: &str) -> Option<(String, String)> {
    let mut parts = tail.trim_matches('/').split('/');
    let owner = parts.next()?.trim();
    let topic = parts.next()?.trim();
    if parts.next().is_some() || !is_feed_owner(owner) || topic.is_empty() || topic.len() > 256 {
        return None;
    }
    Some((owner.to_string(), topic.to_string()))
}

fn is_hls_bytes_route(route: &str) -> bool {
    let Some(reference) = route.strip_prefix("hls/bytes/") else {
        return false;
    };
    is_reference_hex(reference.trim_matches('/'))
}

fn parse_resource_route_body(route: &str) -> Option<ResourceRoute> {
    let route = route.strip_prefix("read/").unwrap_or(route);
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
        "chunk" | "chunks" => {
            let reference = tail.trim_matches('/');
            if is_reference_hex(reference) {
                Some(ResourceRoute::Chunks(reference.to_string()))
            } else {
                None
            }
        }
        "feed" | "feeds" => {
            let (owner, topic) = feed_identity(tail)?;
            Some(ResourceRoute::Feed { owner, topic })
        }
        reference if is_reference_hex(reference) => Some(ResourceRoute::Bzz(route.to_string())),
        _ => None,
    }
}

pub(crate) fn parse_networked_resource_route(input: &str) -> Option<NetworkedResourceRoute> {
    if let Ok(route) = parse_stream_share_link(input, &streaming_route_base()) {
        let network = match route.network {
            StreamShareNetwork::Mainnet => NetworkMode::Mainnet,
            StreamShareNetwork::Testnet => NetworkMode::Testnet,
        };
        return Some(NetworkedResourceRoute {
            network,
            resource: ResourceRoute::Hls {
                media_type: "video".to_string(),
                owner: route.owner,
                topic: route.topic,
                index: route.index,
            },
        });
    }

    let route = trim_route_prefix(input);
    let (network, route) = split_transport_network(&route);
    let resource = parse_resource_route_body(route)?;
    Some(NetworkedResourceRoute { network, resource })
}

pub(crate) fn parse_resource_route(input: &str) -> Option<ResourceRoute> {
    parse_networked_resource_route(input).map(|route| route.resource)
}

fn parse_routes_from_path(pathname: &str) -> Vec<ResourceRoute> {
    let path = pathname.replace('\\', "/");
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let mut routes = Vec::new();
    let mut index = 0usize;

    while index < segments.len() {
        let segment = segments[index];
        if matches!(
            segment,
            "bzz" | "bytes" | "chunk" | "chunks" | "feed" | "feeds"
        ) {
            if let Some(reference) = segments.get(index + 1) {
                let path_tail = if index + 2 < segments.len() {
                    format!("{}/{}", reference, segments[index + 2..].join("/"))
                } else {
                    (*reference).to_string()
                };

                if let Some(route) = parse_resource_route(&format!("{}/{}", segment, path_tail)) {
                    routes.push(route);
                }

                break;
            }
        }

        index += 1;
    }

    routes
}

#[cfg(target_arch = "wasm32")]
pub async fn clear_path() {
    let window = window().unwrap();
    let location = window.location();
    let _ = match location.href() {
        Ok(href0) => {
            if let Some(hash0) = href0.find("/#/") {
                let p0 = &href0[..hash0];
                let p1 = &href0[hash0 + 3..];

                let new_url = format!("{}/{}", p0, p1);
                match window.history() {
                    Ok(history) => {
                        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                    }
                    _ => {}
                };
            }
        }
        _ => {}
    };
    let _ = match location.href() {
        Ok(href0) => {
            if let Some(hash0) = href0.find("/#") {
                let p0 = &href0[..hash0];
                let p1 = &href0[hash0 + 2..];

                let new_url = format!("{}/{}", p0, p1);
                match window.history() {
                    Ok(history) => {
                        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                    }
                    _ => {}
                };
            }
        }
        _ => {}
    };
    let _ = match location.href() {
        Ok(href0) => {
            if let Some(hash0) = href0.find("#/") {
                let p0 = &href0[..hash0];
                let p1 = &href0[hash0 + 2..];

                let new_url = format!("{}/{}", p0, p1);
                match window.history() {
                    Ok(history) => {
                        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                    }
                    _ => {}
                };
            }
        }
        _ => {}
    };
    let _ = match location.href() {
        Ok(href0) => {
            if let Some(hash0) = href0.find("#") {
                let p0 = &href0[..hash0];
                let p1 = &href0[hash0 + 1..];

                let new_url = format!("{}/{}", p0, p1);
                match window.history() {
                    Ok(history) => {
                        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                    }
                    _ => {}
                };
            }
        }
        _ => {}
    };
    loop {
        let _ = match location.pathname() {
            Ok(path0) => {
                if let Some(slashslash) = path0.find("//") {
                    let p0 = &path0[..slashslash];
                    let p1 = &path0[slashslash + 2..];

                    let origin = location.origin().unwrap();
                    let new_url = format!("{}{}/{}", origin, p0, p1);
                    match window.history() {
                        Ok(history) => {
                            let _ =
                                history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                        }
                        _ => {
                            break;
                        }
                    };
                } else {
                    break;
                }
            }
            _ => {
                break;
            }
        };
    }
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

    // Canonical stream share routes must retain their configured mount and
    // explicit network until the strict codec has decoded the topic. The
    // scanner below is intentionally only a legacy/fallback path search.
    if let Some(route) = parse_resource_route(&pathname) {
        return vec![route];
    }

    let parsed_routes = parse_routes_from_path(&pathname);
    if !parsed_routes.is_empty() {
        return parsed_routes;
    }

    let mut references: Vec<String> = vec![];
    let mut current = vec![];
    let mut entered_bzz = false;

    for part in pathname.split('/') {
        if part == "bzz" {
            if entered_bzz && !current.is_empty() {
                references.push(current.join("/"));
                current = vec![];
            }
            entered_bzz = true;
        } else if entered_bzz && !part.is_empty() {
            current.push(part.to_string());
        }
    }

    if entered_bzz && !current.is_empty() {
        references.push(current.join("/"));
    }

    references.into_iter().map(ResourceRoute::Bzz).collect()
}
