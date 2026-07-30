#![cfg(target_arch = "wasm32")]

use alloy::primitives::keccak256;
use ethers::{
    contract::abigen,
    providers::{Http, Provider},
    types::Address,
};
use std::sync::Arc;

const ENS_REGISTRY_ADDRESS: &str = "0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e";
const DEFAULT_ETHEREUM_RPC_URL: &str = "https://ethereum-rpc.publicnode.com";

abigen!(
    RegistryContract,
    r#"[function resolver(bytes32 node) external view returns (address)]"#
);
abigen!(
    ResolverContract,
    r#"[function contenthash(bytes32 node) external view returns (bytes)]"#
);

fn namehash(name: &str) -> [u8; 32] {
    let mut hash = [0u8; 32];
    for label in name.rsplit('.') {
        let mut node = Vec::with_capacity(64);
        node.extend_from_slice(&hash);
        node.extend_from_slice(keccak256(label.as_bytes()).as_slice());
        hash = keccak256(node).into();
    }
    hash
}

pub(crate) async fn resolve_ens_reference(name: String, rpc_url: &str) -> Vec<u8> {
    let provider = match Provider::<Http>::try_from(if rpc_url.is_empty() {
        DEFAULT_ETHEREUM_RPC_URL
    } else {
        rpc_url
    }) {
        Ok(provider) => Arc::new(provider),
        Err(_) => return vec![],
    };
    let registry_address: Address = match ENS_REGISTRY_ADDRESS.parse() {
        Ok(address) => address,
        Err(_) => return vec![],
    };
    let node = namehash(if name.is_empty() { "swarm.eth" } else { &name });
    let resolver_address = match RegistryContract::new(registry_address, provider.clone())
        .resolver(node)
        .call()
        .await
    {
        Ok(address) => address,
        Err(_) => return vec![],
    };
    let content_hash = match ResolverContract::new(resolver_address, provider)
        .contenthash(node)
        .call()
        .await
    {
        Ok(content_hash) => content_hash,
        Err(_) => return vec![],
    };

    if content_hash.len() > 7 && content_hash[0] == 0xe4 {
        content_hash[7..].to_vec()
    } else {
        vec![]
    }
}
