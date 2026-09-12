use std::{future::Future, time::Duration};
use wasm_bindgen::JsError;
use web3::{
    Transport,
    api::{Eth, Namespace},
    helpers::CallFuture,
    transports::eip_1193::Eip1193,
    types::{Address, Bytes, TransactionReceipt, TransactionRequest, U256},
};

use crate::conventions::keccak256;

pub struct ChainContract {
    pub(crate) eth: Eth<Eip1193>,
    address: Address,
}

impl ChainContract {
    pub(crate) fn new(eth: Eth<Eip1193>, address: Address) -> Self {
        Self { eth, address }
    }

    pub(crate) fn query(
        &self,
        signature: &str,
        parameters: &[[u8; 32]],
    ) -> impl Future<Output = Result<Vec<u8>, String>> + '_ {
        let response = CallFuture::<Bytes, _>::new(self.eth.transport().execute(
            "eth_call",
            vec![
                serde_json::json!({"to": self.address, "data": Bytes(abi_call(signature, parameters))}),
                "latest".into(),
            ],
        ));
        async move { Ok(response.await.map_err(|e| format!("Api error: {e}"))?.0) }
    }

    pub(crate) async fn uint(
        &self,
        signature: &str,
        parameters: &[[u8; 32]],
    ) -> Result<U256, String> {
        abi_uint(&self.query(signature, parameters).await?, 0)
    }

    pub(crate) async fn transaction(
        &self,
        signature: &str,
        parameters: &[[u8; 32]],
        payer: Address,
        fallback_gas: Option<u64>,
    ) -> TransactionRequest {
        let data = Bytes(abi_call(signature, parameters));
        let gas = if let Some(fallback) = fallback_gas {
            let estimate = CallFuture::<U256, _>::new(self.eth.transport().execute(
                "eth_estimateGas",
                vec![serde_json::json!({"from": payer, "to": self.address, "data": &data})],
            ))
            .await
            .unwrap_or(fallback.into());
            Some(add_buffer(estimate))
        } else {
            None
        };
        TransactionRequest {
            from: payer,
            to: Some(self.address),
            gas,
            data: Some(data),
            ..Default::default()
        }
    }
}

pub(crate) fn abi_call(signature: &str, parameters: &[[u8; 32]]) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + parameters.len() * 32);
    data.extend_from_slice(&keccak256(signature)[..4]);
    data.extend_from_slice(parameters.as_flattened());
    data
}

pub(crate) fn address_word(address: Address) -> [u8; 32] {
    uint_word(U256::from_big_endian(address.as_bytes()))
}

pub(crate) fn uint_word(value: U256) -> [u8; 32] {
    let mut word = [0; 32];
    value.to_big_endian(&mut word);
    word
}

pub(crate) fn abi_word(bytes: &[u8], offset: usize) -> Result<&[u8], String> {
    if bytes.is_empty() {
        return Err(
            "Abi error: Invalid name: please ensure the contract and method you're calling exist! failed to decode empty bytes. if you're using jsonrpc this is likely due to jsonrpc returning `0x` in case contract or method don't exist".into(),
        );
    }
    bytes
        .get(offset..)
        .and_then(|tail| tail.get(..32))
        .ok_or_else(|| "Abi error: Invalid data".into())
}

pub(crate) fn abi_uint(bytes: &[u8], offset: usize) -> Result<U256, String> {
    Ok(U256::from_big_endian(abi_word(bytes, offset)?))
}

pub(crate) fn abi_bool(bytes: &[u8]) -> Result<bool, String> {
    let word = abi_word(bytes, 0)?;
    word[..31]
        .iter()
        .all(|byte| *byte == 0)
        .then_some(word[31] == 1)
        .ok_or_else(|| "Abi error: Invalid data".into())
}

pub(crate) fn abi_bytes(bytes: &[u8]) -> Option<&[u8]> {
    let position = |offset| {
        let value = abi_uint(bytes, offset).ok()?;
        (value <= u32::MAX.into()).then(|| value.low_u32() as usize)
    };
    let offset = position(0)?;
    let length = position(offset)?;
    let start = offset.checked_add(32)?;
    bytes.get(start..start.checked_add(length)?)
}

pub(crate) async fn confirmed_call(
    contract: &ChainContract,
    signature: &str,
    parameters: &[[u8; 32]],
    payer: Address,
    fallback_gas: Option<u64>,
) -> Result<TransactionReceipt, JsError> {
    let transaction = contract
        .transaction(signature, parameters, payer, fallback_gas)
        .await;
    web3::confirm::send_transaction_with_confirmation(
        contract.eth.transport().clone(),
        transaction,
        Duration::from_secs(1),
        1,
    )
    .await
    .map_err(|error| {
        let method = signature.split('(').next().unwrap();
        JsError::new(&format!("{method}() failed: {error}"))
    })
}

pub(crate) fn add_buffer(gas: U256) -> U256 {
    gas + (gas / U256::from(5u8))
}
