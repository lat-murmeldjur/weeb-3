// #![allow(warnings)]
#![cfg(target_arch = "wasm32")]
use std::sync::Mutex;
use std::sync::mpsc;

use libp2p::PeerId;

use js_sys::Date;

use crate::conventions::{PeerAccounting, get_proximity};

pub const REFRESH_RATE: u64 = 4500000;
pub const PO_PRICE: u64 = 10000;

pub fn set_payment_threshold(a: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = a.lock().unwrap();
    account.threshold = amount;
    if amount > REFRESH_RATE * 2 {
        account.payment_threshold = amount - REFRESH_RATE;
    }
}

pub fn reserve(a: &Mutex<PeerAccounting>, amount: u64, chan: &mpsc::Sender<(PeerId, u64)>) -> bool {
    let mut account = a.lock().unwrap();
    if account.balance >= account.payment_threshold && account.refreshment + 1000.0 < Date::now() {
        // start refreshing
        let _ = chan.send((account.id.clone(), account.threshold));
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

pub fn price(peer_overlay: &String, chunk_address: &Vec<u8>) -> u64 {
    // return uint64(swarm.MaxPO-swarm.Proximity(peer.Bytes(), chunk.Bytes())+1) * pricer.PO_PRICE

    let po = get_proximity(&hex::decode(peer_overlay).unwrap(), &chunk_address);
    return ((u64::from(crate::conventions::MAX_PO) - u64::from(po) + 1) * PO_PRICE).into();
}
