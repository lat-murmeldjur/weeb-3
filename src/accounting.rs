#![allow(warnings)]
#![cfg(target_arch = "wasm32")]

use std::collections::HashMap;
use std::io;
use std::sync::mpsc;
use std::sync::Mutex;

use libp2p::PeerId;

use js_sys::Date;

use crate::conventions::{get_proximity, PeerAccounting};

const refreshRate: u64 = 450000;

pub fn set_payment_threshold(a: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = a.lock().unwrap();
    account.threshold = amount;
}

pub fn reserve(a: &Mutex<PeerAccounting>, amount: u64, chan: &mpsc::Sender<(PeerId, u64)>) -> bool {
    let mut account = a.lock().unwrap();
    if account.balance > refreshRate && account.refreshment + 1000.0 < Date::now() {
        // start refreshing
        account.refreshment = Date::now();
        chan.send((account.id.clone(), account.threshold));
    }
    if account.reserve + account.balance + amount < account.threshold {
        account.reserve += amount;
        return true;
    }
    return false;
}

pub fn apply_credit(a: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = a.lock().unwrap();
    account.balance += amount;
    if account.reserve > amount {
        account.reserve -= amount;
        return;
    }
    account.reserve = 0;
}

pub fn apply_refreshment(a: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = a.lock().unwrap();
    if account.balance > amount {
        account.balance -= amount;
        return;
    }
    account.balance = 0;
}

pub fn cancel_reserve(a: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = a.lock().unwrap();
    if account.reserve > amount {
        account.reserve -= amount;
        return;
    }
    account.reserve = 0;
}

pub fn price(peer_overlay: String, chunk_address: &Vec<u8>) -> u64 {
    // return uint64(swarm.MaxPO-swarm.Proximity(peer.Bytes(), chunk.Bytes())+1) * pricer.poPrice

    let po = get_proximity(&peer_overlay.as_bytes().to_vec(), &chunk_address);
    return ((u64::from(crate::conventions::max_po) - u64::from(po)) * 10000).into();
}
