use libp2p::core::Multiaddr;
use std::convert::TryFrom;

pub const UNDERLAY_LIST_PREFIX: u8 = 0x99;

pub fn deserialize_underlays(data: &[u8]) -> Vec<Multiaddr> {
    if data.is_empty() {
        return Vec::new();
    }

    if data[0] == UNDERLAY_LIST_PREFIX {
        return deserialize_list(&data[1..]);
    }

    match Multiaddr::try_from(data.to_vec()) {
        Ok(addr) => vec![addr],
        Err(_) => Vec::new(),
    }
}

fn deserialize_list(data: &[u8]) -> Vec<Multiaddr> {
    let mut addrs = Vec::new();
    let mut i = 0usize;

    while i < data.len() {
        let (addr_len_u64, varint_len) = read_uvarint(&data[i..]);

        if varint_len == 0 {
            break;
        }

        i += varint_len;

        if addr_len_u64 > usize::MAX as u64 {
            break;
        }
        let addr_len = addr_len_u64 as usize;

        let remaining = data.len().saturating_sub(i);
        if remaining < addr_len {
            break;
        }

        let end = i + addr_len;
        let addr_bytes = &data[i..end];

        match Multiaddr::try_from(addr_bytes.to_vec()) {
            Ok(addr) => addrs.push(addr),
            Err(_) => break,
        }

        i = end;
    }

    addrs
}

fn read_uvarint(src: &[u8]) -> (u64, usize) {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;

    for (i, &byte) in src.iter().enumerate() {
        let bits = (byte & 0x7f) as u64;
        value |= bits << shift;

        if byte & 0x80 == 0 {
            return (value, i + 1);
        }

        shift += 7;
        if shift > 63 {
            return (0, 0);
        }
    }

    (0, 0)
}
