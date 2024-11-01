#![allow(warnings)]
#![cfg(target_arch = "wasm32")]

use std::collections::HashMap;
use std::io;
use std::sync::mpsc;
use std::sync::Mutex;

use libp2p::PeerId;

use js_sys::Date;

use crate::conventions::PeerAccounting;

const refreshRate: u64 = 1000000;

pub fn reserve(a: Mutex<PeerAccounting>, amount: u64, chan: &mpsc::Sender<(PeerId, u64)>) -> bool {
    let mut account = a.lock().unwrap();
    if account.balance > refreshRate && account.refreshment + 1000.0 < Date::now() {
        // start refreshing
        chan.send((account.id.clone(), account.threshold));
    }
    if account.reserve + account.balance + amount < account.threshold {
        account.reserve += amount;
        return true;
    }
    return false;
}

pub fn apply_credit(a: Mutex<PeerAccounting>, amount: u64) {
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

pub fn cancel_reserve(a: Mutex<PeerAccounting>, amount: u64) {
    let mut account = a.lock().unwrap();
    if account.reserve > amount {
        account.reserve -= amount;
        return;
    }
    account.reserve = 0;
}
