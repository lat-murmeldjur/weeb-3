use std::str::FromStr;

use wasm_bindgen::JsError;
use web3::{
    contract::{Contract, Options},
    transports::eip_1193::{Eip1193, Provider},
    types::{Address, H160, H256, TransactionReceipt, U256},
};

use hex;

const POSTAGE_CONTRACT_ADDR: &str = "cdfdC3752caaA826fE62531E0000C40546eC56A6";
const SBZZ_TOKEN_CONTRACT_ADDR: &str = "543dDb01Ba47acB11de34891cD86B675F04840db";
const BATCH_CREATED_TOPIC: &str =
    "9b088e2c89b322a3c1d81515e1c88db3d386d022926f0e2d0b9b5813b7413d58";
const BUCKET_DEPTH: u8 = 16;

pub type Web3Inst = web3::Web3<Eip1193>;
pub type PostageContract = Contract<Eip1193>;
pub type TokenContract = Contract<Eip1193>;

fn ensure_addr(s: &str) -> Result<Address, JsError> {
    Address::from_str(s).map_err(|_| JsError::new("Invalid address constant"))
}

fn provider_from_window() -> Result<Provider, JsError> {
    match Provider::default() {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(JsError::new(
            "No EIP-1193 provider (window.ethereum) available",
        )),
        Err(e) => Err(JsError::new(&format!(
            "Failed to initialize EIP-1193 provider: {e:?}"
        ))),
    }
}

pub fn web3() -> Result<Web3Inst, JsError> {
    let prov = provider_from_window()?;
    Ok(web3::Web3::new(Eip1193::new(prov)))
}

pub async fn request_accounts(w3: &Web3Inst) -> Result<Vec<Address>, JsError> {
    w3.eth()
        .request_accounts()
        .await
        .map_err(|e| JsError::new(&format!("eth_requestAccounts failed: {e}")))
}

pub async fn postage_contract(w3: &Web3Inst) -> Result<PostageContract, JsError> {
    let addr = ensure_addr(POSTAGE_CONTRACT_ADDR)?;
    Contract::from_json(w3.eth(), addr, include_bytes!("./postagestamp.json"))
        .map_err(|e| JsError::new(&format!("Failed to load Postage contract: {e}")))
}

pub async fn token_contract(w3: &Web3Inst) -> Result<TokenContract, JsError> {
    let addr = ensure_addr(SBZZ_TOKEN_CONTRACT_ADDR)?;
    Contract::from_json(w3.eth(), addr, include_bytes!("./sbzz.json"))
        .map_err(|e| JsError::new(&format!("Failed to load SBZZ token contract: {e}")))
}

pub async fn last_price(postage: &PostageContract) -> Result<U256, JsError> {
    postage
        .query("lastPrice", (), None, Options::default(), None)
        .await
        .map_err(|e| JsError::new(&format!("lastPrice() failed: {e}")))
}

pub async fn expired_batches_exist(postage: &PostageContract) -> Result<bool, JsError> {
    postage
        .query("expiredBatchesExist", (), None, Options::default(), None)
        .await
        .map_err(|e| JsError::new(&format!("expiredBatchesExist() failed: {e}")))
}

pub async fn get_batch_validity(batch_id: Vec<u8>) -> U256 {
    let Ok(w3) = web3() else { return U256::from(1) };
    let Ok(contract) = postage_contract(&w3).await else {
        return U256::from(1);
    };

    let id_bytes_32: [u8; 32] = match batch_id.try_into() {
        Ok(x) => x,
        Err(_) => return U256::from(1),
    };

    match contract
        .query(
            "remainingBalance",
            (id_bytes_32,),
            None,
            Options::default(),
            None,
        )
        .await
    {
        Ok(val) => val,
        Err(_) => U256::from(1),
    }
}

#[allow(dead_code)]
pub async fn expire_limited_if_needed(
    postage: &PostageContract,
    from: Address,
) -> Result<(), JsError> {
    loop {
        if !expired_batches_exist(postage).await? {
            break;
        }

        let _tx_hash = postage
            .call(
                "expireLimited",
                (U256::from(5u64),),
                from,
                Options::default(),
            )
            .await
            .map_err(|e| JsError::new(&format!("expireLimited() failed: {e}")))?;
    }
    Ok(())
}

#[allow(dead_code)]
pub async fn approve_token_spend(
    token: &TokenContract,
    spender: Address,
    from: Address,
    amount: U256,
) -> Result<TransactionReceipt, JsError> {
    token
        .call_with_confirmations(
            "approve",
            (spender, amount),
            from,
            Options::default(),
            1usize,
        )
        .await
        .map_err(|e| JsError::new(&format!("approve() failed: {e}")))
}

#[allow(dead_code)]
pub async fn create_postage_batch(
    postage: &PostageContract,
    from: Address,
    owner: Address,
    initial_balance: U256,
    depth: u8,
    bucket_depth: u8,
    nonce: [u8; 32],
    immutable_flag: bool,
) -> Result<TransactionReceipt, JsError> {
    postage
        .call_with_confirmations(
            "createBatch",
            (
                owner,
                initial_balance,
                depth,
                bucket_depth,
                nonce,
                immutable_flag,
            ),
            from,
            Options::default(),
            1usize,
        )
        .await
        .map_err(|e| JsError::new(&format!("createBatch() failed: {e}")))
}

pub fn parse_batch_id_from_receipt(receipt: &TransactionReceipt) -> Option<Vec<u8>> {
    let topic = H256::from_slice(&hex::decode(BATCH_CREATED_TOPIC).ok()?);
    let contract = H160::from_slice(&hex::decode(POSTAGE_CONTRACT_ADDR).ok()?);

    for log in receipt.logs.iter() {
        if log.topics.get(0) == Some(&topic) && log.address == contract {
            if let Some(batch_topic) = log.topics.get(1) {
                return Some(batch_topic.as_bytes().to_vec());
            }
        }
    }
    None
}

pub fn compute_initial_balance_per_chunk(last_price: U256, validity_days: u64) -> U256 {
    last_price * U256::from(7200u64) * U256::from(validity_days)
}

pub fn chunk_count_for_depth(depth: u8) -> U256 {
    U256::from(1u64) << depth
}

pub fn total_approve_amount(initial_per_chunk: U256, depth: u8) -> U256 {
    initial_per_chunk * chunk_count_for_depth(depth)
}

pub fn buckets_for_depth(depth: u8) -> u32 {
    if depth < BUCKET_DEPTH {
        0
    } else {
        1u32 << (depth as u32 - BUCKET_DEPTH as u32)
    }
}

#[derive(Debug, Clone)]
pub struct BatchPurchaseResult {
    pub approve_tx: H256,
    pub create_tx: H256,
    pub batch_id: Vec<u8>,
    pub last_price: U256,
    #[allow(dead_code)]
    pub initial_balance_per_chunk: U256,
    #[allow(dead_code)]
    pub approve_amount: U256,
    pub bucket_limit: u32,
}

fn add_buffer(g: U256) -> U256 {
    g + (g / U256::from(5u8))
}

pub async fn buy_postage_batch(
    validity_days: u64,
    depth: u8,
    owner: Address,
) -> Result<BatchPurchaseResult, JsError> {
    let w3 = web3()?;
    let accounts = request_accounts(&w3).await?;
    let payer = *accounts
        .first()
        .ok_or_else(|| JsError::new("No accounts returned by provider"))?;

    let postage = postage_contract(&w3).await?;
    let token = token_contract(&w3).await?;

    let lp = last_price(&postage).await?;
    let initial_per_chunk = compute_initial_balance_per_chunk(lp, validity_days);
    let approve_amt = total_approve_amount(initial_per_chunk, depth);

    let sbzz_balance: U256 = token
        .query("balanceOf", (payer,), None, Options::default(), None)
        .await
        .map_err(|e| JsError::new(&format!("balanceOf() failed: {e}")))?;

    if sbzz_balance < approve_amt {
        return Err(JsError::new(&format!(
            "Insufficient SBZZ balance. Need {}, have {}. \
             Reduce batch size or validity days, or top up SBZZ.",
            approve_amt, sbzz_balance
        )));
    }

    while expired_batches_exist(&postage).await? {
        let mut exp_opts = Options::default();
        let gas_est = postage
            .estimate_gas(
                "expireLimited",
                (U256::from(5u64),),
                payer,
                Options::default(),
            )
            .await
            .unwrap_or(U256::from(200_000u64));
        exp_opts.gas = Some(add_buffer(gas_est));

        let _tx = postage
            .call("expireLimited", (U256::from(5u64),), payer, exp_opts)
            .await
            .map_err(|e| JsError::new(&format!("expireLimited() failed: {e}")))?;
    }

    let spender = ensure_addr(POSTAGE_CONTRACT_ADDR)?;
    let mut approve_opts = Options::default();
    let approve_gas = token
        .estimate_gas("approve", (spender, approve_amt), payer, Options::default())
        .await
        .unwrap_or(U256::from(100_000u64));
    approve_opts.gas = Some(add_buffer(approve_gas));

    let approve_receipt = token
        .call_with_confirmations(
            "approve",
            (spender, approve_amt),
            payer,
            approve_opts,
            1usize,
        )
        .await
        .map_err(|e| JsError::new(&format!("approve() failed: {e}")))?;

    let nonce_rand: [u8; 32] = crate::encrey()
        .try_into()
        .map_err(|_| JsError::new("nonce gen"))?;
    let mut cb_opts = Options::default();
    let cb_gas = postage
        .estimate_gas(
            "createBatch",
            (
                owner,
                initial_per_chunk,
                depth,
                BUCKET_DEPTH,
                nonce_rand,
                false,
            ),
            payer,
            Options::default(),
        )
        .await
        .unwrap_or(U256::from(1_500_000u64));
    cb_opts.gas = Some(add_buffer(cb_gas));

    let create_receipt = postage
        .call_with_confirmations(
            "createBatch",
            (
                owner,
                initial_per_chunk,
                depth,
                BUCKET_DEPTH,
                nonce_rand,
                false,
            ),
            payer,
            cb_opts,
            1usize,
        )
        .await
        .map_err(|e| JsError::new(&format!("createBatch() failed: {e}")))?;

    let batch_id = parse_batch_id_from_receipt(&create_receipt)
        .ok_or_else(|| JsError::new("BatchCreated event not found in receipt"))?;

    Ok(BatchPurchaseResult {
        approve_tx: approve_receipt.transaction_hash,
        create_tx: create_receipt.transaction_hash,
        batch_id,
        last_price: lp,
        initial_balance_per_chunk: initial_per_chunk,
        approve_amount: approve_amt,
        bucket_limit: buckets_for_depth(depth),
    })
}
