// #![allow(warnings)]
#![cfg(target_arch = "wasm32")]
use async_std::sync::{Arc, Mutex};

use libp2p::{PeerId, swarm::ConnectionId};

use crate::conventions::{PeerAccounting, get_proximity};
use crate::mpsc;

pub const REFRESH_RATE: u64 = 450000;
pub const PO_PRICE: u64 = 10000;
pub(crate) type RefreshmentInstruction = (PeerId, Arc<Mutex<PeerAccounting>>, ConnectionId);

pub async fn set_payment_threshold(a: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = a.lock().await;
    account.threshold = amount;
    if amount > REFRESH_RATE * 2 {
        account.payment_threshold = REFRESH_RATE * 2;
    }
}

pub async fn reserve(a: &Mutex<PeerAccounting>, amount: u64) -> Option<ConnectionId> {
    let mut account = a.lock().await;
    let connection_id = account.connection_id?;
    let new_reserve = account.reserve.checked_add(amount)?;
    let reserved_balance = account.balance.checked_add(new_reserve)?;

    if reserved_balance > account.threshold {
        return None;
    }

    account.reserve = new_reserve;
    Some(connection_id)
}

pub async fn apply_credit(
    a: &Arc<Mutex<PeerAccounting>>,
    amount: u64,
    chan: &mpsc::Sender<RefreshmentInstruction>,
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

    let instruction = if account.balance >= REFRESH_RATE && !account.refresh_scheduled {
        account.connection_id.map(|connection_id| {
            account.refresh_scheduled = true;
            (account.id, a.clone(), connection_id)
        })
    } else {
        None
    };
    drop(account);

    if let Some(instruction) = instruction {
        let accounting = instruction.1.clone();
        match chan.try_send(instruction) {
            // Pause only the threshold-crossing completion. The receiver is
            // woken first, then the browser can service its WebRTC control IO.
            Ok(()) => async_std::task::sleep(std::time::Duration::ZERO).await,
            Err(_) => accounting.lock().await.refresh_scheduled = false,
        }
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

pub fn price(peer_overlay: &[u8], chunk_address: &[u8]) -> u64 {
    // return uint64(swarm.MaxPO-swarm.Proximity(peer.Bytes(), chunk.Bytes())+1) * pricer.PO_PRICE

    let po = get_proximity(peer_overlay, chunk_address);
    return ((u64::from(crate::conventions::MAX_PO) - u64::from(po) + 1) * PO_PRICE).into();
}
