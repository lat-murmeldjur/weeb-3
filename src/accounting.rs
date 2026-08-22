pub(crate) const CONNECTION_BUILDUP_LIMIT: u64 = 200;
pub(crate) const REFRESH_RATE: u64 = 450000;
const PO_PRICE: u64 = 10000;
pub(crate) const MAX_CHUNK_PRICE: u64 = 32 * PO_PRICE;

pub(crate) fn refreshment_due(balance: u64, last_refreshment: f64, payment_threshold: u64) -> bool {
    let target = if last_refreshment == 0.0 {
        REFRESH_RATE.saturating_mul(2)
    } else {
        REFRESH_RATE
    };
    balance
        >= if payment_threshold == 0 {
            target
        } else {
            target.min(payment_threshold)
        }
}

pub(crate) fn connection_dial_capacity_available(connected: u64, ongoing: u64) -> bool {
    connection_population_deficit(connected, ongoing) > 0
}

pub(crate) fn connection_population_deficit(connected: u64, ongoing: u64) -> u64 {
    CONNECTION_BUILDUP_LIMIT.saturating_sub(connected.saturating_add(ongoing))
}

pub(crate) fn bee_reconnect_delay_seconds(
    balance: u64,
    reserve: u64,
    payment_threshold: u64,
    refresh_rate: u64,
) -> u64 {
    if refresh_rate == 0 {
        return 1;
    }

    const BEE_LIGHT_ACCOUNTING_FACTOR: u64 = 10;
    let bee_refresh_rate = refresh_rate.saturating_mul(BEE_LIGHT_ACCOUNTING_FACTOR);
    let bee_payment_threshold = payment_threshold.saturating_mul(BEE_LIGHT_ACCOUNTING_FACTOR);

    balance
        .saturating_add(reserve)
        .max(bee_refresh_rate)
        .saturating_add(bee_payment_threshold)
        .checked_div(bee_refresh_rate)
        .unwrap_or(1)
        .max(1)
}

#[cfg(target_arch = "wasm32")]
use crate::{
    conventions::{PeerAccounting, get_proximity},
    mpsc,
};
#[cfg(target_arch = "wasm32")]
use async_std::sync::{Arc, Mutex};
#[cfg(target_arch = "wasm32")]
use libp2p::{PeerId, swarm::ConnectionId};

#[cfg(target_arch = "wasm32")]
pub(crate) type RefreshmentInstruction = (PeerId, Arc<Mutex<PeerAccounting>>, ConnectionId);

#[cfg(target_arch = "wasm32")]
pub(crate) async fn set_payment_threshold(accounting: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = accounting.lock().await;
    account.threshold = amount;
    if amount > REFRESH_RATE * 2 {
        account.payment_threshold = REFRESH_RATE * 2;
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn reserve(
    accounting: &Mutex<PeerAccounting>,
    amount: u64,
) -> Option<ConnectionId> {
    let mut account = accounting.lock().await;
    let connection_id = account.connection_id?;
    let new_reserve = account.reserve.checked_add(amount)?;
    if account.balance.checked_add(new_reserve)? > account.threshold {
        return None;
    }
    account.reserve = new_reserve;
    Some(connection_id)
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn apply_credit(
    accounting: &Arc<Mutex<PeerAccounting>>,
    amount: u64,
    refreshments: &mpsc::Sender<RefreshmentInstruction>,
) {
    let mut account = accounting.lock().await;
    let mut debt_increase = amount;
    account.reserve = account.reserve.saturating_sub(amount);

    let compensated = account.surplus_balance.min(debt_increase);
    account.surplus_balance -= compensated;
    debt_increase -= compensated;
    account.balance = account.balance.saturating_add(debt_increase);

    let instruction = if refreshment_due(account.balance, account.refreshment, account.threshold)
        && !account.refresh_scheduled
    {
        account.connection_id.map(|connection_id| {
            account.refresh_scheduled = true;
            (account.id, accounting.clone(), connection_id)
        })
    } else {
        None
    };
    drop(account);

    if let Some(instruction) = instruction {
        let accounting = instruction.1.clone();
        match refreshments.try_send(instruction) {
            Ok(()) => async_std::task::sleep(std::time::Duration::ZERO).await,
            Err(_) => accounting.lock().await.refresh_scheduled = false,
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn apply_refreshment(
    accounting: &Mutex<PeerAccounting>,
    amount: u64,
) -> Option<(PeerId, u64, u64)> {
    let mut account = accounting.lock().await;
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

#[cfg(target_arch = "wasm32")]
pub(crate) async fn cancel_reserve(accounting: &Mutex<PeerAccounting>, amount: u64) {
    let mut account = accounting.lock().await;
    account.reserve = account.reserve.saturating_sub(amount);
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn price(peer_overlay: &[u8], chunk_address: &[u8]) -> u64 {
    (u64::from(crate::conventions::MAX_PO)
        - u64::from(get_proximity(peer_overlay, chunk_address).min(crate::conventions::MAX_PO))
        + 1)
        * PO_PRICE
}
