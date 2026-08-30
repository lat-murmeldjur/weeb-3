use wasm_bindgen::JsError;
use web3::{
    contract::Options,
    types::{Address, U256},
};

use crate::{
    network_profile::NetworkProfile,
    on_chain::{
        BatchPurchaseResult, buy_postage_batch_with_payer, chunk_count_for_depth,
        compute_initial_balance_per_chunk, get_batch_validity, last_price, postage_contract,
        token_contract, web3,
    },
    secure_vault::{
        SecureBatchState, SecurePreparedBatch, secure_batch_state_for_wallet,
        secure_commit_batch_purchase_and_verify, secure_prepare_batch_purchase,
    },
};

pub(crate) struct BatchFunding {
    pub last_price: U256,
    pub required_bzz: U256,
    pub token_balance: U256,
    pub base_balance: U256,
    pub remaining_days: Option<U256>,
}

pub(crate) struct BatchPrerequisites {
    pub chain_id: U256,
    pub secure: SecureBatchState,
    pub funding: Option<BatchFunding>,
}

pub(crate) async fn inspect_batch(
    payer: Address,
    profile: NetworkProfile,
    depth: u8,
    validity_days: u64,
) -> Result<BatchPrerequisites, String> {
    let secure = secure_batch_state_for_wallet(payer.as_bytes(), profile.swarm_network_id)
        .await
        .ok_or_else(|| "could not check weeb-3-secure for the connected wallet".to_string())?;
    let w3 = web3().map_err(|error| format!("provider init failed: {error:?}"))?;
    let chain_id = w3
        .eth()
        .chain_id()
        .await
        .map_err(|error| format!("chain id check failed: {error:?}"))?;
    if chain_id != U256::from(profile.wallet_chain_id) {
        return Ok(BatchPrerequisites {
            chain_id,
            secure,
            funding: None,
        });
    }

    let postage =
        postage_contract(&w3).map_err(|error| format!("postage contract failed: {error:?}"))?;
    let token = token_contract(&w3).map_err(|error| format!("token contract failed: {error:?}"))?;
    let last_price = last_price(&postage)
        .await
        .map_err(|error| format!("last price failed: {error:?}"))?;
    let token_balance = token
        .query("balanceOf", (payer,), None, Options::default(), None)
        .await
        .map_err(|error| format!("token balance failed: {error:?}"))?;
    let base_balance = w3
        .eth()
        .balance(payer, None)
        .await
        .map_err(|error| format!("base balance failed: {error:?}"))?;
    let required_bzz =
        compute_initial_balance_per_chunk(last_price, validity_days) * chunk_count_for_depth(depth);
    let remaining_days = if secure.usable() {
        let day_price = last_price * U256::from(7200u64);
        Some(if day_price.is_zero() {
            U256::zero()
        } else {
            get_batch_validity(&secure.batch_id).await / day_price
        })
    } else {
        None
    };

    Ok(BatchPrerequisites {
        chain_id,
        secure,
        funding: Some(BatchFunding {
            last_price,
            required_bzz,
            token_balance,
            base_balance,
            remaining_days,
        }),
    })
}

pub(crate) enum BatchPurchaseOutcome {
    AlreadyReady(SecureBatchState),
    Purchased {
        owner: Address,
        prepared: SecurePreparedBatch,
        purchase: BatchPurchaseResult,
    },
}

pub(crate) enum BatchPurchaseError {
    CheckSecure,
    PrepareSecure,
    OnChain(JsError),
    CommitSecure,
}

#[derive(Clone, Copy)]
pub(crate) enum MissingSecureBatchState {
    Error,
    ContinuePurchase,
}

pub(crate) async fn ensure_batch(
    payer: Address,
    profile: NetworkProfile,
    depth: u8,
    validity_days: u64,
    missing: MissingSecureBatchState,
) -> Result<BatchPurchaseOutcome, BatchPurchaseError> {
    match secure_batch_state_for_wallet(payer.as_bytes(), profile.swarm_network_id).await {
        Some(state) if state.usable() => {
            return Ok(BatchPurchaseOutcome::AlreadyReady(state));
        }
        None if matches!(missing, MissingSecureBatchState::Error) => {
            return Err(BatchPurchaseError::CheckSecure);
        }
        _ => {}
    }

    let prepared = secure_prepare_batch_purchase(depth, validity_days, profile.swarm_network_id)
        .await
        .filter(|prepared| prepared.owner.len() == 20)
        .ok_or(BatchPurchaseError::PrepareSecure)?;
    let owner = Address::from_slice(&prepared.owner);
    let purchase =
        buy_postage_batch_with_payer(prepared.validity_days, prepared.depth, owner, payer)
            .await
            .map_err(BatchPurchaseError::OnChain)?;
    if !secure_commit_batch_purchase_and_verify(
        payer.as_bytes(),
        &purchase.batch_id,
        purchase.bucket_limit,
        prepared.depth,
        profile.swarm_network_id,
    )
    .await
    {
        return Err(BatchPurchaseError::CommitSecure);
    }

    Ok(BatchPurchaseOutcome::Purchased {
        owner,
        prepared,
        purchase,
    })
}
