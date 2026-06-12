// #![allow(warnings)]
#![cfg(target_arch = "wasm32")]
use async_std::sync::Mutex;

use libp2p::PeerId;

use crate::conventions::{PeerAccounting, get_proximity};
use crate::mpsc;

pub const REFRESH_RATE: u64 = 450000;
pub const PO_PRICE: u64 = 10000;

pub async fn set_payment_threshold(a: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = a.lock().await;
    account.threshold = amount;
    if amount > REFRESH_RATE * 2 {
        account.payment_threshold = REFRESH_RATE * 2;
    }
}

pub async fn reserve(a: &Mutex<PeerAccounting>, amount: u64) -> bool {
    let mut account = a.lock().await;
    let Some(new_reserve) = account.reserve.checked_add(amount) else {
        return false;
    };

    let Some(reserved_balance) = account.balance.checked_add(new_reserve) else {
        return false;
    };

    if reserved_balance > account.threshold {
        return false;
    }

    account.reserve = new_reserve;
    true
}

pub async fn apply_credit(
    a: &Mutex<PeerAccounting>,
    amount: u64,
    chan: &mpsc::Sender<(PeerId, u64)>,
) {
    let mut account = a.lock().await;
    let mut debt_increase = amount;
    if account.reserve > amount {
        account.reserve -= amount;
    } else {
        account.reserve = 0;
    }

    if account.surplus_balance > 0 {
        let compensated = account.surplus_balance.min(debt_increase);
        account.surplus_balance -= compensated;
        debt_increase -= compensated;
    }

    if debt_increase > 0 {
        account.balance = account.balance.saturating_add(debt_increase);
    }

    if account.balance >= REFRESH_RATE {
        let _ = chan.try_send((account.id.clone(), account.balance));
    }
}

pub async fn apply_refreshment(
    a: &Mutex<PeerAccounting>,
    amount: u64,
) -> Option<(PeerId, u64, u64)> {
    let mut account = a.lock().await;
    if amount >= account.balance {
        let surplus_growth = amount - account.balance;
        account.balance = 0;
        account.surplus_balance = account.surplus_balance.saturating_add(surplus_growth);
        if surplus_growth > 0 {
            return Some((account.id.clone(), surplus_growth, account.surplus_balance));
        }
    } else {
        account.balance -= amount;
    }

    None
}

pub async fn cancel_reserve(a: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = a.lock().await;
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
