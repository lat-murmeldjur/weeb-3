use crate::JsValue;
use std::sync::Arc;

use alloy::primitives::keccak256;
use ethers::{
    contract::abigen,
    providers::{Http, Provider},
    types::Address,
};

abigen!(
    RegistryContract,
    r#"[
        function resolver(bytes32 node) external view returns (address)
    ]"# // event_derives(serde::Deserialize, serde::Serialize)
);
abigen!(
    ResolverContract,
    r#"[
        function contenthash(bytes32 node) external view returns (bytes)
    ]"# // event_derives(serde::Deserialize, serde::Serialize)
);

fn namehash(name: &str) -> Vec<u8> {
    if name.is_empty() {
        return vec![0u8; 32];
    }
    let mut hash = vec![0u8; 32];
    for label in name.rsplit('.') {
        hash.append(&mut keccak256(label.as_bytes()).to_vec());
        hash = keccak256(hash.as_slice()).to_vec();
    }
    hash
}

pub async fn prt(input_address: String, inherit_rpc_url: String) -> Vec<u8> {
    let mut rpc_url = "https://ethereum-rpc.publicnode.com";
    let mut testaddress = "swarm.eth";
    if input_address.len() > 0 {
        testaddress = &input_address;
    }

    if inherit_rpc_url.len() > 0 {
        rpc_url = &inherit_rpc_url;
    }

    let namehashed = namehash(testaddress);

    let provider = match Provider::<Http>::try_from(rpc_url) {
        Ok(aok) => aok,
        _ => return vec![],
    };

    let client = Arc::new(provider);

    let reg_address_string = "0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e";
    let reg_address: Address = match reg_address_string.parse() {
        Ok(aok) => aok,
        _ => return vec![],
    };

    let reg_contract = RegistryContract::new(reg_address, client.clone());

    let namehashed32: [u8; 32] = match namehashed.clone().try_into() {
        Ok(aok) => aok,
        _ => return vec![],
    };

    let res_address = match reg_contract.resolver(namehashed32).call().await {
        Ok(aok) => aok,
        _ => return vec![],
    };

    web_sys::console::log_1(&JsValue::from(format!(
        "Resolver Address {:#?}",
        res_address
    )));

    let res_contract = ResolverContract::new(res_address, client.clone());

    let contenthasd = match res_contract.contenthash(namehashed32).call().await {
        Ok(aok) => aok,
        _ => return vec![],
    };

    if contenthasd.len() > 7 {
        web_sys::console::log_1(&JsValue::from(format!(
            "Contenthash Found {}",
            hex::encode(contenthasd[..].to_vec())
        )));
        if hex::encode(&[contenthasd[0]]) == "e4" {
            web_sys::console::log_1(&JsValue::from(format!(
                "Swarm Hash Found {}",
                hex::encode(contenthasd[7..].to_vec())
            )));
            return contenthasd[7..].to_vec();
        }
    };

    return vec![];
}
