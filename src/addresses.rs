#![cfg(target_arch = "wasm32")]

use libp2p::{Multiaddr, multiaddr::Protocol};
use std::{convert::TryFrom, str::FromStr};

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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UnderlayFormat {
    BeeWss,
    DnsTransformedWss,
    Other,
}

pub(crate) fn detect_underlay_format(address: &Multiaddr) -> UnderlayFormat {
    let mut protocols = address.iter();
    match protocols.next() {
        Some(Protocol::Ip4(_)) => {
            if matches!(
                (
                    protocols.next(),
                    protocols.next(),
                    protocols.next(),
                    protocols.next(),
                    protocols.next(),
                    protocols.next(),
                ),
                (
                    Some(Protocol::Tcp(_)),
                    Some(Protocol::Tls),
                    Some(Protocol::Sni(_)),
                    Some(Protocol::Ws(_)),
                    Some(Protocol::P2p(_)),
                    None,
                )
            ) {
                return UnderlayFormat::BeeWss;
            }
        }
        Some(Protocol::Dns4(_)) => {
            if matches!(
                (
                    protocols.next(),
                    protocols.next(),
                    protocols.next(),
                    protocols.next(),
                    protocols.next(),
                ),
                (
                    Some(Protocol::Tcp(_)),
                    Some(Protocol::Tls),
                    Some(Protocol::Ws(_)),
                    Some(Protocol::P2p(_)),
                    None,
                )
            ) {
                return UnderlayFormat::DnsTransformedWss;
            }
        }
        _ => {}
    }
    UnderlayFormat::Other
}

pub(crate) fn beewss_to_dns_transformed(address: &Multiaddr) -> Multiaddr {
    let mut hostname = None;
    let mut tcp_port = None;
    let mut peer_id = None;

    for protocol in address.iter() {
        match protocol {
            Protocol::Sni(value) => hostname = Some(value.to_string()),
            Protocol::Tcp(value) => tcp_port = Some(value),
            Protocol::P2p(value) => peer_id = Some(value.to_string()),
            _ => {}
        }
    }

    Multiaddr::from_str(&format!(
        "/dns4/{}/tcp/{}/tls/ws/p2p/{}",
        hostname.expect("BeeWss requires SNI"),
        tcp_port.expect("BeeWss requires TCP port"),
        peer_id.expect("BeeWss requires PeerId"),
    ))
    .expect("constructed DNS-transformed WSS multiaddr must be valid")
}
