use std::str::FromStr;

use alloy_primitives::{B256, keccak256};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use wasm_bindgen::JsError;
use web3::{
    contract::{Contract, Options},
    ethabi::{Token, encode},
    transports::eip_1193::{Eip1193, Provider},
    types::{Address, H160, H256, TransactionReceipt, U256},
};

use crate::{
    PrivateKeySigner,
    network_profile::{NetworkMode, active_profile},
};

#[derive(Clone, Debug)]
pub struct Cheque {
    pub chequebook: Address,
    pub beneficiary: Address,
    pub cumulative_payout: U256,
}

pub struct ChequeSigner {
    wallet: PrivateKeySigner,
    chain_id: U256,
}

impl ChequeSigner {
    pub fn new(wallet: PrivateKeySigner, chain_id: u64) -> Self {
        Self {
            wallet,
            chain_id: U256::from(chain_id),
        }
    }

    fn domain_separator(&self) -> [u8; 32] {
        let type_hash = keccak256(b"EIP712Domain(string name,string version,uint256 chainId)");
        let name_hash = keccak256(b"Chequebook");
        let version_hash = keccak256(b"1.0");
        let tokens = [
            Token::FixedBytes(type_hash.to_vec()),
            Token::FixedBytes(name_hash.to_vec()),
            Token::FixedBytes(version_hash.to_vec()),
            Token::Uint(self.chain_id),
        ];
        let encoded = encode(&tokens);
        keccak256(encoded).into()
    }

    fn cheque_struct_hash(&self, cheque: &Cheque) -> [u8; 32] {
        let type_hash =
            keccak256(b"Cheque(address chequebook,address beneficiary,uint256 cumulativePayout)");
        let tokens = [
            Token::FixedBytes(type_hash.to_vec()),
            Token::Address(cheque.chequebook),
            Token::Address(cheque.beneficiary),
            Token::Uint(cheque.cumulative_payout),
        ];
        let encoded = encode(&tokens);
        keccak256(encoded).into()
    }

    fn digest(&self, cheque: &Cheque) -> B256 {
        let domain_separator = self.domain_separator();
        let struct_hash = self.cheque_struct_hash(cheque);
        let mut buf = [0u8; 66];
        buf[..2].copy_from_slice(&[0x19, 0x01]);
        buf[2..34].copy_from_slice(&domain_separator);
        buf[34..].copy_from_slice(&struct_hash);
        keccak256(buf)
    }

    pub fn sign(&self, cheque: &Cheque) -> Option<Vec<u8>> {
        self.wallet
            .sign_hash_sync(&self.digest(cheque))
            .ok()
            .map(|signature| signature.as_bytes().to_vec())
    }
}

pub struct ChequebookClient {
    signer: ChequeSigner,
    chequebook: Address,
}

impl ChequebookClient {
    pub fn new(chequebook: Address, wallet: PrivateKeySigner, chain_id: u64) -> Self {
        Self {
            signer: ChequeSigner::new(wallet, chain_id),
            chequebook,
        }
    }

    pub fn prepare_emit_cheque_bytes(
        &self,
        beneficiary: Address,
        cumulative_payout: U256,
    ) -> Option<Vec<u8>> {
        let cheque = Cheque {
            chequebook: self.chequebook,
            beneficiary,
            cumulative_payout,
        };

        let signature = BASE64.encode(self.signer.sign(&cheque)?);
        Some(
            format!(
                r#"{{"Chequebook":"{:#x}","Beneficiary":"{:#x}","CumulativePayout":{},"Signature":"{}"}}"#,
                cheque.chequebook, cheque.beneficiary, cheque.cumulative_payout, signature
            )
            .into_bytes(),
        )
    }
}

const TESTNET_POSTAGE_CONTRACT_ADDR: &str = "cdfdC3752caaA826fE62531E0000C40546eC56A6";
const MAINNET_POSTAGE_CONTRACT_ADDR: &str = "45a1502382541Cd610CC9068e88727426b696293";

const TESTNET_TOKEN_CONTRACT_ADDR: &str = "543dDb01Ba47acB11de34891cD86B675F04840db";
const MAINNET_TOKEN_CONTRACT_ADDR: &str = "dBF3Ea6F5beE45c02255B2c26a16F300502F68da";

const BATCH_CREATED_TOPIC: &str =
    "9b088e2c89b322a3c1d81515e1c88db3d386d022926f0e2d0b9b5813b7413d58";

const TESTNET_PRICE_ORACLE_ADDR: &str = "1814e9b3951Df0CB8e12b2bB99c5594514588936";
const TESTNET_CHEQUEBOOK_FACTORY_ADDR: &str = "0fF044F6bB4F684a5A149B46D7eC03ea659F98A1";

const MAINNET_PRICE_ORACLE_ADDR: &str = "A57A50a831B31c904A770edBCb706E03afCdbd94";
const MAINNET_CHEQUEBOOK_FACTORY_ADDR: &str = "c2d5a532cf69aa9a1378737d8ccdef884b6e7420";

const BUCKET_DEPTH: u8 = 16;

fn select_network_address(mainnet: &'static str, testnet: &'static str) -> &'static str {
    match active_profile().mode {
        NetworkMode::Mainnet => mainnet,
        NetworkMode::Testnet => testnet,
    }
}

fn select_postage_contract_addr() -> &'static str {
    select_network_address(MAINNET_POSTAGE_CONTRACT_ADDR, TESTNET_POSTAGE_CONTRACT_ADDR)
}

fn select_token_contract_addr() -> &'static str {
    select_network_address(MAINNET_TOKEN_CONTRACT_ADDR, TESTNET_TOKEN_CONTRACT_ADDR)
}

fn select_price_oracle_addr() -> &'static str {
    select_network_address(MAINNET_PRICE_ORACLE_ADDR, TESTNET_PRICE_ORACLE_ADDR)
}

fn select_chequebook_factory_addr() -> &'static str {
    select_network_address(
        MAINNET_CHEQUEBOOK_FACTORY_ADDR,
        TESTNET_CHEQUEBOOK_FACTORY_ADDR,
    )
}

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

async fn ensure_wallet_chain(w3: &Web3Inst) -> Result<(), JsError> {
    let profile = active_profile();
    let chain_id = w3
        .eth()
        .chain_id()
        .await
        .map_err(|error| JsError::new(&format!("chain_id failed: {error:?}")))?;
    if chain_id == U256::from(profile.wallet_chain_id) {
        Ok(())
    } else {
        Err(JsError::new(&format!(
            "Wrong network. Please switch wallet to chain {} for {:?}.",
            profile.wallet_chain_id, profile.mode
        )))
    }
}

pub fn postage_contract(w3: &Web3Inst) -> Result<PostageContract, JsError> {
    let addr = ensure_addr(select_postage_contract_addr())?;

    Contract::from_json(w3.eth(), addr, include_bytes!("./postagestamp.json"))
        .map_err(|e| JsError::new(&format!("Failed to load Postage contract: {e}")))
}

pub fn token_contract(w3: &Web3Inst) -> Result<TokenContract, JsError> {
    let addr = ensure_addr(select_token_contract_addr())?;
    Contract::from_json(w3.eth(), addr, include_bytes!("./sbzz.json")).map_err(|e| {
        JsError::new(&format!(
            "Failed to load {} token contract: {e}",
            active_profile().bzz_symbol
        ))
    })
}

pub fn chequebook_factory(w3: &Web3Inst) -> Result<ChequebookFactory, JsError> {
    let addr = ensure_addr(select_chequebook_factory_addr())?;

    Contract::from_json(w3.eth(), addr, include_bytes!("./factory.json"))
        .map_err(|e| JsError::new(&format!("Failed to load chequebook factory contract: {e}")))
}

pub fn chequebook_contract(w3: &Web3Inst, addr: Address) -> Result<ChequebookContract, JsError> {
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

pub async fn get_batch_validity(batch_id: &[u8]) -> U256 {
    let Ok(w3) = web3() else {
        return U256::zero();
    };
    let Ok(contract) = postage_contract(&w3) else {
        return U256::zero();
    };
    let Ok(batch_id): Result<[u8; 32], _> = batch_id.try_into() else {
        return U256::zero();
    };

    contract
        .query(
            "remainingBalance",
            (batch_id,),
            None,
            Options::default(),
            None,
        )
        .await
        .unwrap_or_default()
}

pub fn parse_batch_id_from_receipt(receipt: &TransactionReceipt) -> Option<Vec<u8>> {
    let topic = H256::from_slice(&hex::decode(BATCH_CREATED_TOPIC).ok()?);
    let contract = H160::from_slice(&hex::decode(select_postage_contract_addr()).ok()?);

    for log in receipt.logs.iter() {
        if log.topics.first() == Some(&topic)
            && log.address == contract
            && let Some(batch_topic) = log.topics.get(1)
        {
            return Some(batch_topic.as_bytes().to_vec());
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
    let web3 = web3()?;
    ensure_wallet_chain(&web3).await?;

    let postage = postage_contract(&web3)?;
    let token = token_contract(&web3)?;

    let current_price = last_price(&postage).await?;
    let initial_per_chunk = compute_initial_balance_per_chunk(current_price, validity_days);
    let approval = total_approve_amount(initial_per_chunk, depth);

    let bzz_balance: U256 = token
        .query("balanceOf", (payer,), None, Options::default(), None)
        .await
        .map_err(|e| JsError::new(&format!("balanceOf() failed: {e}")))?;
    if bzz_balance < approval {
        return Err(JsError::new(&format!(
            "Insufficient {}. Need {}, have {}. Reduce depth/validity or top up.",
            active_profile().bzz_symbol,
            approval,
            bzz_balance
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
    let spender = ensure_addr(select_postage_contract_addr())?;

    let approve_gas = token
        .estimate_gas("approve", (spender, approval), payer, Options::default())
        .await
        .unwrap_or(U256::from(100_000u64));
    approve_opts.gas = Some(add_buffer(approve_gas));
    let approve_receipt = token
        .call_with_confirmations("approve", (spender, approval), payer, approve_opts, 1usize)
        .await
        .map_err(|e| JsError::new(&format!("approve() failed: {e}")))?;

    let mut create_batch_options = Options::default();
    let nonce_rand: [u8; 32] = crate::random_encryption_key()
        .try_into()
        .map_err(|_| JsError::new("nonce gen"))?;
    let create_batch_gas = postage
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
    create_batch_options.gas = Some(add_buffer(create_batch_gas));
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
            create_batch_options,
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
        last_price: current_price,
        bucket_limit: buckets_for_depth(depth),
    })
}

#[derive(Debug, Clone)]
pub struct BatchPurchaseResult {
    pub approve_tx: H256,
    pub create_tx: H256,
    pub batch_id: Vec<u8>,
    pub last_price: U256,
    pub bucket_limit: u32,
}

pub fn parse_chequebook_address_from_receipt(receipt: &TransactionReceipt) -> Option<Address> {
    let topic_bytes = keccak256(b"SimpleSwapDeployed(address)");
    let topic = H256::from_slice(topic_bytes.as_slice());
    let addr_str = select_chequebook_factory_addr();
    let factory = H160::from_slice(&hex::decode(addr_str).ok()?);

    for log in receipt.logs.iter() {
        if log.address == factory && log.topics.first() == Some(&topic) {
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
    let contract = chequebook_contract(w3, chequebook_addr)?;
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
    let web3 = web3()?;
    ensure_wallet_chain(&web3).await?;

    let factory = chequebook_factory(&web3)?;

    let salt: [u8; 32] = crate::random_encryption_key()
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

fn price_oracle_contract(w3: &Web3Inst) -> Result<PriceOracleContract, JsError> {
    let addr = ensure_addr(select_price_oracle_addr())?;

    Contract::from_json(w3.eth(), addr, include_bytes!("./priceoracle.json"))
        .map_err(|e| JsError::new(&format!("Failed to load PriceOracle contract: {e}")))
}

pub async fn get_price_from_oracle() -> Option<(U256, U256)> {
    crate::secure_vault::worker_price_oracle().await
}

pub(crate) async fn get_price_from_oracle_in_window() -> Option<(U256, U256)> {
    let web3 = web3().ok()?;
    let oracle = price_oracle_contract(&web3).ok()?;
    let (price, deduction) = oracle
        .query::<(U256, U256), _, _, _>("getPrice", (), None, Options::default(), None)
        .await
        .ok()?;
    (!price.is_zero()).then_some((price, deduction))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Signature;
    use wasm_bindgen_test::wasm_bindgen_test;

    const PRIVATE_KEY: [u8; 32] = [1; 32];
    const EXPECTED_SIGNATURE: &str = "59914b4bd53a81a73a6e28ddff40ee0457b99d824cce48ee2c68eeacd7df6dfd42589fcee1a12a0799217b1cc797c67d9ff0f9be1386fcc2b2c3e17474318e121c";

    fn fixture() -> (PrivateKeySigner, Cheque) {
        let wallet = PrivateKeySigner::from_slice(&PRIVATE_KEY).expect("fixture private key");
        let cheque = Cheque {
            chequebook: Address::from_slice(&[0x11; 20]),
            beneficiary: Address::from_slice(&[0x22; 20]),
            cumulative_payout: U256::from_dec_str("12345678901234567890").expect("fixture payout"),
        };
        (wallet, cheque)
    }

    #[wasm_bindgen_test]
    fn cheque_signature_is_the_stable_ethers_compatible_65_byte_value() {
        let (wallet, cheque) = fixture();
        let signer = ChequeSigner::new(wallet.clone(), 100);
        let signature = signer.sign(&cheque).expect("sign cheque");

        assert_eq!(signature.len(), 65);
        assert!(matches!(signature[64], 27 | 28));
        assert_eq!(hex::encode(&signature), EXPECTED_SIGNATURE);

        let signature = Signature::from_raw(&signature).expect("parse signature");
        assert_eq!(
            signature
                .recover_address_from_prehash(&signer.digest(&cheque))
                .expect("recover signer"),
            wallet.address()
        );
    }

    #[wasm_bindgen_test]
    fn cheque_json_keeps_the_exact_bee_wire_shape() {
        let (wallet, cheque) = fixture();
        let expected_signature = BASE64.encode(hex::decode(EXPECTED_SIGNATURE).unwrap());
        let client = ChequebookClient::new(cheque.chequebook, wallet, 100);
        let encoded = client
            .prepare_emit_cheque_bytes(cheque.beneficiary, cheque.cumulative_payout)
            .expect("encode cheque");
        assert_eq!(
            String::from_utf8(encoded).unwrap(),
            format!(
                r#"{{"Chequebook":"0x1111111111111111111111111111111111111111","Beneficiary":"0x2222222222222222222222222222222222222222","CumulativePayout":12345678901234567890,"Signature":"{expected_signature}"}}"#
            )
        );
    }
}
