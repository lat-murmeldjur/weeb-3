use wasm_bindgen::JsValue;
use web_sys::window;

use crate::network_profile::NetworkMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResourceRoute {
    Bzz(String),
    Bytes(String),
    Chunks(String),
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

    for prefix in ["/weeb-3/#/", "/weeb-3/", "weeb-3/#/", "weeb-3/"] {
        if let Some(rest) = route.strip_prefix(prefix) {
            route = rest.to_string();
            break;
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

fn trim_network_mode_prefix(route: &str) -> &str {
    let route = route.trim_start_matches('/');
    let mut parts = route.splitn(2, '/');
    let head = parts.next().unwrap_or_default();
    let tail = parts.next().unwrap_or_default();

    if network_mode_segment(head).is_some() {
        tail.trim_start_matches('/')
    } else {
        route
    }
}

pub(crate) fn route_network_mode_from_path(pathname: &str) -> Option<NetworkMode> {
    let route = trim_route_prefix(pathname);
    let head = route.split('/').find(|part| !part.is_empty())?;
    network_mode_segment(head)
}

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

pub(crate) fn parse_resource_route(input: &str) -> Option<ResourceRoute> {
    let route = trim_route_prefix(input);
    let route = trim_network_mode_prefix(&route).to_string();
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
        reference if is_reference_hex(reference) => Some(ResourceRoute::Bzz(route)),
        _ => None,
    }
}

fn parse_routes_from_path(pathname: &str) -> Vec<ResourceRoute> {
    let path = pathname.replace('\\', "/");
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let mut routes = Vec::new();
    let mut index = 0usize;

    while index < segments.len() {
        let segment = segments[index];
        if matches!(segment, "bzz" | "bytes" | "chunk" | "chunks") {
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
