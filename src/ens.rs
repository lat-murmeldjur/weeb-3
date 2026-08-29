#![cfg(target_arch = "wasm32")]

use std::str::FromStr;

use web3::{
    Web3,
    contract::{Contract, Options},
    signing::namehash,
    transports::Http,
    types::Address,
};

const ENS_REGISTRY_ADDRESS: &str = "0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e";
const DEFAULT_ETHEREUM_RPC_URL: &str = "https://ethereum-rpc.publicnode.com";
const REGISTRY_ABI: &[u8] =
    br#"[{"inputs":[{"name":"node","type":"bytes32"}],"name":"resolver","outputs":[{"type":"address"}],"stateMutability":"view","type":"function"}]"#;
const RESOLVER_ABI: &[u8] =
    br#"[{"inputs":[{"name":"node","type":"bytes32"}],"name":"contenthash","outputs":[{"type":"bytes"}],"stateMutability":"view","type":"function"}]"#;

pub(crate) async fn resolve_ens_reference(name: String, rpc_url: &str) -> Vec<u8> {
    let rpc_url = if rpc_url.is_empty() {
        DEFAULT_ETHEREUM_RPC_URL
    } else {
        rpc_url
    };
    let Ok(transport) = Http::new(rpc_url) else {
        return vec![];
    };
    let eth = Web3::new(transport).eth();
    let Ok(registry_address) = Address::from_str(ENS_REGISTRY_ADDRESS) else {
        return vec![];
    };
    let Ok(registry) = Contract::from_json(eth.clone(), registry_address, REGISTRY_ABI) else {
        return vec![];
    };
    let node = namehash(if name.is_empty() { "swarm.eth" } else { &name });
    let Ok(resolver_address) = registry
        .query("resolver", node, None, Options::default(), None)
        .await
    else {
        return vec![];
    };
    if resolver_address == Address::zero() {
        return vec![];
    }
    let Ok(resolver) = Contract::from_json(eth, resolver_address, RESOLVER_ABI) else {
        return vec![];
    };
    let Ok(content_hash): Result<Vec<u8>, _> = resolver
        .query("contenthash", node, None, Options::default(), None)
        .await
    else {
        return vec![];
    };

    if content_hash.len() > 7 && content_hash[0] == 0xe4 {
        content_hash[7..].to_vec()
    } else {
        vec![]
    }
}
