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

pub async fn prt(input_address: String) {
    let rpc_url = "https://ethereum-rpc.publicnode.com";
    let mut testaddress = "swarm.eth";
    if input_address.len() > 0 {
        testaddress = &input_address;
    }

    let namehashed = namehash(testaddress);

    let provider = Provider::<Http>::try_from(rpc_url).unwrap();
    let client = Arc::new(provider);

    let reg_address_string = "0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e";
    let reg_address: Address = reg_address_string.parse().unwrap();

    let reg_contract = RegistryContract::new(reg_address, client.clone());

    let res_address = reg_contract
        .resolver(
            //FixedBytes::from_slice(namehashed.as_slice())
            namehashed.clone().try_into().unwrap(),
        )
        .call()
        .await
        .unwrap();

    web_sys::console::log_1(&JsValue::from(format!(
        "Resolver Address {:#?}",
        res_address
    )));

    let res_contract = ResolverContract::new(res_address, client.clone());

    let contenthasd = res_contract
        .contenthash(namehashed.try_into().unwrap())
        .call()
        .await
        .unwrap();

    if contenthasd.len() > 14 {
        web_sys::console::log_1(&JsValue::from(format!(
            "Contenthash Found {}",
            hex::encode(contenthasd[7..].to_vec())
        )));
    };
}
