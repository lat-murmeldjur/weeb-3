use std::collections::HashMap;
use std::str::FromStr;

use wasm_bindgen::JsError;
use web3::{
    contract::{Contract, Options},
    transports::eip_1193::{Eip1193, Provider},
    types::{Address, H160, H256, TransactionReceipt, U256},
};

use num::BigUint;

use base64;
use ethers::abi::{Token, encode};
use ethers::signers::LocalWallet;
use ethers::types::{Address as EthAddress, H256 as EthH256, Signature, U256 as EthU256};
use ethers::utils::keccak256;
use hex;
use prost::Message;
use serde::Serialize;

#[derive(Clone, Debug)]
pub struct Cheque {
    pub chequebook: EthAddress,
    pub beneficiary: EthAddress,
    pub cumulative_payout: EthU256,
}

#[derive(Clone, Debug)]
pub struct SignedCheque {
    pub cheque: Cheque,
    pub signature: Vec<u8>,
}

#[derive(Serialize)]
struct SignedChequeJson {
    #[serde(rename = "Chequebook")]
    chequebook: String,
    #[serde(rename = "Beneficiary")]
    beneficiary: String,
    #[serde(rename = "CumulativePayout")]
    cumulative_payout: u128,
    #[serde(rename = "Signature")]
    signature: String,
}

impl SignedChequeJson {
    fn from_cheque(cheque: &Cheque, signature: &[u8]) -> Self {
        let chequebook = format!("{:#x}", cheque.chequebook);
        let beneficiary = format!("{:#x}", cheque.beneficiary);

        let cumulative_payout = cheque.cumulative_payout.as_u128();

        let signature = base64::encode(signature);
        Self {
            chequebook,
            beneficiary,
            cumulative_payout,
            signature,
        }
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct EmitCheque {
    #[prost(bytes = "vec", tag = "1")]
    pub cheque: Vec<u8>,
}

pub struct ChequeSigner {
    wallet: LocalWallet,
    chain_id: EthU256,
}

impl ChequeSigner {
    pub fn new(wallet: LocalWallet, chain_id: u64) -> Self {
        Self {
            wallet,
            chain_id: EthU256::from(chain_id),
        }
    }

    fn domain_separator(&self) -> [u8; 32] {
        let type_hash = keccak256(b"EIP712Domain(string name,string version,uint256 chainId)");
        let name_hash = keccak256(b"Chequebook");
        let version_hash = keccak256(b"1.0");
        let tokens = vec![
            Token::FixedBytes(type_hash.to_vec()),
            Token::FixedBytes(name_hash.to_vec()),
            Token::FixedBytes(version_hash.to_vec()),
            Token::Uint(self.chain_id),
        ];
        let encoded = encode(&tokens);
        keccak256(encoded)
    }

    fn cheque_struct_hash(&self, cheque: &Cheque) -> [u8; 32] {
        let type_hash =
            keccak256(b"Cheque(address chequebook,address beneficiary,uint256 cumulativePayout)");
        let tokens = vec![
            Token::FixedBytes(type_hash.to_vec()),
            Token::Address(cheque.chequebook),
            Token::Address(cheque.beneficiary),
            Token::Uint(cheque.cumulative_payout),
        ];
        let encoded = encode(&tokens);
        keccak256(encoded)
    }

    pub fn sign(&self, cheque: &Cheque) -> Option<Vec<u8>> {
        let domain_separator = self.domain_separator();
        let struct_hash = self.cheque_struct_hash(cheque);
        let mut buf = Vec::with_capacity(2 + 32 + 32);
        buf.push(0x19);
        buf.push(0x01);
        buf.extend_from_slice(&domain_separator);
        buf.extend_from_slice(&struct_hash);
        let digest_bytes = keccak256(&buf);
        let digest = EthH256::from(digest_bytes);
        let sig: Signature = self.wallet.sign_hash(digest).ok()?;
        Some(sig.to_vec())
    }
}

pub struct ChequebookClient {
    signer: ChequeSigner,
    chequebook: EthAddress,
    last_payouts: HashMap<EthAddress, EthU256>,
}

impl ChequebookClient {
    pub fn new(chequebook: EthAddress, wallet: LocalWallet, chain_id: u64) -> Self {
        Self {
            signer: ChequeSigner::new(wallet, chain_id),
            chequebook,
            last_payouts: HashMap::new(),
        }
    }

    pub fn cumulative_payout_for(&self, beneficiary: &EthAddress) -> EthU256 {
        self.last_payouts
            .get(beneficiary)
            .cloned()
            .unwrap_or_else(EthU256::zero)
    }

    pub fn prepare_emit_cheque_bytes(
        &mut self,
        beneficiary: EthAddress,
        amount: EthU256,
    ) -> Option<Vec<u8>> {
        let last = self.cumulative_payout_for(&beneficiary);
        let cumulative = last.checked_add(amount)?;

        let cheque = Cheque {
            chequebook: self.chequebook,
            beneficiary,
            cumulative_payout: cumulative,
        };

        let signature = self.signer.sign(&cheque)?;
        self.last_payouts.insert(beneficiary, cumulative);

        let json = SignedChequeJson::from_cheque(&cheque, &signature);
        serde_json::to_vec(&json).ok()
    }
}

const POSTAGE_CONTRACT_ADDR: &str = "cdfdC3752caaA826fE62531E0000C40546eC56A6";
const SBZZ_TOKEN_CONTRACT_ADDR: &str = "543dDb01Ba47acB11de34891cD86B675F04840db";
const BATCH_CREATED_TOPIC: &str =
    "9b088e2c89b322a3c1d81515e1c88db3d386d022926f0e2d0b9b5813b7413d58";
const PRICE_ORACLE_ADDR: &str = "1814e9b3951Df0CB8e12b2bB99c5594514588936";
const BUCKET_DEPTH: u8 = 16;
const CHEQUEBOOK_FACTORY_ADDR: &str = "0fF044F6bB4F684a5A149B46D7eC03ea659F98A1";

pub type Web3Inst = web3::Web3<Eip1193>;
pub type PostageContract = Contract<Eip1193>;
pub type TokenContract = Contract<Eip1193>;
pub type ChequebookFactory = Contract<Eip1193>;
pub type ChequebookContract = Contract<Eip1193>;
pub type PriceOracleContract = Contract<Eip1193>;

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

#[allow(dead_code)]
pub async fn connected_accounts(w3: &Web3Inst) -> Result<Vec<Address>, JsError> {
    let accs = w3
        .eth()
        .accounts()
        .await
        .map_err(|e| JsError::new(&format!("eth_accounts failed: {e:?}")))?;
    if accs.is_empty() {
        return Err(JsError::new(
            "No wallet account available. Connect the wallet first.",
        ));
    }
    Ok(accs)
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

pub async fn chequebook_factory(w3: &Web3Inst) -> Result<ChequebookFactory, JsError> {
    let addr = ensure_addr(CHEQUEBOOK_FACTORY_ADDR)?;
    Contract::from_json(w3.eth(), addr, include_bytes!("./factory.json"))
        .map_err(|e| JsError::new(&format!("Failed to load chequebook factory contract: {e}")))
}

pub async fn chequebook_contract(
    w3: &Web3Inst,
    addr: Address,
) -> Result<ChequebookContract, JsError> {
    Contract::from_json(w3.eth(), addr, include_bytes!("./simple_swap.json"))
        .map_err(|e| JsError::new(&format!("Failed to load chequebook contract: {e}")))
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
    let Ok(w3) = web3() else { return U256::from(0) };
    let Ok(contract) = postage_contract(&w3).await else {
        return U256::from(0);
    };

    let id_bytes_32: [u8; 32] = match batch_id.try_into() {
        Ok(x) => x,
        Err(_) => return U256::from(0),
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
        Err(_) => U256::from(0),
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

pub async fn buy_postage_batch_with_payer(
    validity_days: u64,
    depth: u8,
    owner: Address,
    payer: Address,
) -> Result<BatchPurchaseResult, JsError> {
    let w3 = web3()?;

    {
        let cid = w3
            .eth()
            .chain_id()
            .await
            .map_err(|e| JsError::new(&format!("chain_id failed: {e:?}")))?;
        if cid != U256::from(11155111u64) {
            return Err(JsError::new(
                "Wrong network. Please switch to Sepolia (11155111).",
            ));
        }
    }

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
            "Insufficient SBZZ. Need {}, have {}. Reduce depth/validity or top up.",
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
        let _ = postage
            .call("expireLimited", (U256::from(5u64),), payer, exp_opts)
            .await
            .map_err(|e| JsError::new(&format!("expireLimited() failed: {e}")))?;
    }

    let mut approve_opts = Options::default();
    let spender = Address::from_slice(&hex::decode(POSTAGE_CONTRACT_ADDR).unwrap());
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

    let mut cb_opts = Options::default();
    let nonce_rand: [u8; 32] = crate::encrey()
        .try_into()
        .map_err(|_| JsError::new("nonce gen"))?;
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

pub fn parse_chequebook_address_from_receipt(receipt: &TransactionReceipt) -> Option<Address> {
    let topic_bytes = keccak256(b"SimpleSwapDeployed(address)");
    let topic = H256::from_slice(&topic_bytes);
    let factory = H160::from_slice(&hex::decode(CHEQUEBOOK_FACTORY_ADDR).ok()?);
    for log in receipt.logs.iter() {
        if log.address == factory && log.topics.get(0) == Some(&topic) {
            let data = log.data.0.as_slice();
            if data.len() >= 32 {
                let addr_bytes = &data[12..32];
                return Some(Address::from_slice(addr_bytes));
            }
        }
    }
    None
}

pub async fn chequebook_balance(w3: &Web3Inst, chequebook_addr: Address) -> Result<U256, JsError> {
    let contract = chequebook_contract(w3, chequebook_addr).await?;
    contract
        .query("balance", (), None, Options::default(), None)
        .await
        .map_err(|e| JsError::new(&format!("balance() failed: {e}")))
}

pub async fn deposit_to_chequebook(
    token: &TokenContract,
    chequebook: Address,
    from: Address,
    amount: U256,
) -> Result<TransactionReceipt, JsError> {
    token
        .call_with_confirmations(
            "transfer",
            (chequebook, amount),
            from,
            Options::default(),
            1usize,
        )
        .await
        .map_err(|e| JsError::new(&format!("transfer() failed: {e}")))
}

#[derive(Debug, Clone)]
pub struct ChequebookDeploymentResult {
    pub tx: H256,
    pub chequebook: Address,
}

pub async fn deploy_chequebook_with_payer(
    issuer: Address,
    payer: Address,
) -> Result<ChequebookDeploymentResult, JsError> {
    let w3 = web3()?;

    {
        let cid = w3
            .eth()
            .chain_id()
            .await
            .map_err(|e| JsError::new(&format!("chain_id failed: {e:?}")))?;
        if cid != U256::from(11155111u64) {
            return Err(JsError::new(
                "Wrong network. Please switch to Sepolia (11155111).",
            ));
        }
    }

    let factory = chequebook_factory(&w3).await?;

    let salt: [u8; 32] = crate::encrey()
        .try_into()
        .map_err(|_| JsError::new("nonce gen"))?;

    let mut opts = Options::default();
    let gas_est = factory
        .estimate_gas(
            "deploySimpleSwap",
            (issuer, U256::from(0u64), salt),
            payer,
            Options::default(),
        )
        .await
        .unwrap_or(U256::from(175_000u64));
    opts.gas = Some(add_buffer(gas_est));

    let receipt = factory
        .call_with_confirmations(
            "deploySimpleSwap",
            (issuer, U256::from(0u64), salt),
            payer,
            opts,
            1usize,
        )
        .await
        .map_err(|e| JsError::new(&format!("deploySimpleSwap() failed: {e}")))?;

    let chequebook = parse_chequebook_address_from_receipt(&receipt)
        .ok_or_else(|| JsError::new("SimpleSwapDeployed event not found in receipt"))?;

    Ok(ChequebookDeploymentResult {
        tx: receipt.transaction_hash,
        chequebook,
    })
}

fn add_buffer(g: U256) -> U256 {
    g + (g / U256::from(5u8))
}

async fn price_oracle_contract(w3: &Web3Inst) -> Result<PriceOracleContract, JsError> {
    let addr = ensure_addr(PRICE_ORACLE_ADDR)?;

    Contract::from_json(w3.eth(), addr, include_bytes!("./priceoracle.json"))
        .map_err(|e| JsError::new(&format!("Failed to load PriceOracle contract: {e}")))
}

pub async fn get_price_from_oracle() -> (U256, U256) {
    let w3 = match web3() {
        Ok(w3) => w3,
        Err(_) => return (U256::from(0), U256::from(0)),
    };

    let oracle = match price_oracle_contract(&w3).await {
        Ok(oracle) => oracle,
        Err(_) => return (U256::from(0), U256::from(0)),
    };

    match oracle
        .query::<(U256, U256), _, _, _>("getPrice", (), None, Options::default(), None)
        .await
    {
        Ok((price, cheque_value_deduction)) => (price, cheque_value_deduction),
        Err(_) => (U256::from(0), U256::from(0)),
    }
}
