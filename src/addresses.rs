#![cfg(target_arch = "wasm32")]

use libp2p::{Multiaddr, multiaddr::Protocol};
use std::{convert::TryFrom, net::Ipv4Addr};

const UNDERLAY_LIST_PREFIX: u8 = 0x99;
const MAX_UNDERLAYS_PER_PEER: usize = 20;
const MAX_UNDERLAY_BYTES: usize = 2048;

pub(crate) fn deserialize_underlays(data: &[u8]) -> Vec<Multiaddr> {
    if data.is_empty() || data.len() > MAX_UNDERLAY_BYTES {
        return Vec::new();
    }
    if data[0] == UNDERLAY_LIST_PREFIX {
        return deserialize_underlay_list(&data[1..]);
    }
    Multiaddr::try_from(data.to_vec()).map_or_else(|_| Vec::new(), |address| vec![address])
}

fn deserialize_underlay_list(data: &[u8]) -> Vec<Multiaddr> {
    let mut addresses = Vec::new();
    let mut offset = 0usize;

    while offset < data.len() {
        let (address_len, varint_len) = read_underlay_uvarint(&data[offset..]);
        if addresses.len() >= MAX_UNDERLAYS_PER_PEER
            || varint_len == 0
            || address_len > usize::MAX as u64
        {
            return Vec::new();
        }
        offset += varint_len;
        let address_len = address_len as usize;
        if data.len().saturating_sub(offset) < address_len {
            return Vec::new();
        }
        let end = offset + address_len;
        match Multiaddr::try_from(data[offset..end].to_vec()) {
            Ok(address) => addresses.push(address),
            Err(_) => return Vec::new(),
        }
        offset = end;
    }

    addresses
}

fn read_underlay_uvarint(src: &[u8]) -> (u64, usize) {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (index, &byte) in src.iter().enumerate() {
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return (value, index + 1);
        }
        shift += 7;
        if shift > 63 {
            return (0, 0);
        }
    }
    (0, 0)
}

pub(crate) fn browser_dial_address(address: Multiaddr) -> Result<Multiaddr, Multiaddr> {
    let mut protocols = address.iter();
    match (
        protocols.next(),
        protocols.next(),
        protocols.next(),
        protocols.next(),
        protocols.next(),
        protocols.next(),
        protocols.next(),
    ) {
        (
            Some(Protocol::Ip4(_)),
            Some(Protocol::Tcp(tcp_port)),
            Some(Protocol::Tls),
            Some(Protocol::Sni(hostname)),
            Some(Protocol::Ws(_)),
            Some(Protocol::P2p(peer_id)),
            None,
        ) => Ok([
            Protocol::Dns4(hostname),
            Protocol::Tcp(tcp_port),
            Protocol::Tls,
            Protocol::Ws("/".into()),
            Protocol::P2p(peer_id),
        ]
        .into_iter()
        .collect()),
        (
            Some(Protocol::Dns4(_)),
            Some(Protocol::Tcp(_)),
            Some(Protocol::Tls),
            Some(Protocol::Ws(_)),
            Some(Protocol::P2p(_)),
            None,
            None,
        ) => Ok(address),
        _ => Err(address),
    }
}

pub(crate) fn is_publicly_dialable_underlay(address: &Multiaddr) -> bool {
    let mut protocols = address.iter();
    match protocols.next() {
        Some(Protocol::Ip4(address)) if is_public_ipv4(address) => protocols
            .find_map(|protocol| match protocol {
                Protocol::Sni(hostname) => Some(hostname),
                _ => None,
            })
            .is_some_and(|hostname| {
                libp2p_direct_ipv4(&hostname).map_or_else(
                    || is_public_dns_name(&hostname),
                    |embedded| embedded == address,
                )
            }),
        Some(Protocol::Dns4(hostname)) => libp2p_direct_ipv4(&hostname)
            .map(is_public_ipv4)
            .unwrap_or_else(|| is_public_dns_name(&hostname)),
        _ => false,
    }
}

fn is_public_dns_name(hostname: &str) -> bool {
    let hostname = hostname.trim_end_matches('.');
    if let Ok(address) = hostname.parse::<Ipv4Addr>() {
        return is_public_ipv4(address);
    }
    let ends_with = |suffix: &str| {
        let bytes = hostname.as_bytes();
        bytes.len() >= suffix.len()
            && bytes[bytes.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
    };
    !hostname.is_empty()
        && hostname.contains('.')
        && !hostname.eq_ignore_ascii_case("localhost")
        && !ends_with(".localhost")
        && !ends_with(".local")
        && !ends_with(".internal")
        && !ends_with(".home.arpa")
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    !(first == 0
        || address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_broadcast()
        || address.is_documentation()
        || (first == 100 && (64..=127).contains(&second))
        || (first == 192 && second == 0 && third == 0)
        || (first == 192 && second == 88 && third == 99)
        || (first == 198 && (18..=19).contains(&second))
        || first >= 240)
}

fn libp2p_direct_ipv4(hostname: &str) -> Option<Ipv4Addr> {
    let mut octets = hostname
        .strip_suffix(".libp2p.direct")?
        .split('.')
        .next()?
        .split('-');
    let address = Ipv4Addr::new(
        octets.next()?.parse().ok()?,
        octets.next()?.parse().ok()?,
        octets.next()?.parse().ok()?,
        octets.next()?.parse().ok()?,
    );
    octets.next().is_none().then_some(address)
}
