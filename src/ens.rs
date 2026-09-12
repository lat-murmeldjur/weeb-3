#![cfg(target_arch = "wasm32")]

use crate::{
    conventions::namehash,
    on_chain_conventions::{abi_bytes, abi_call, abi_word},
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, RequestInit, Response};
use web3::types::Address;

const ENS_REGISTRY_ADDRESS: &str = "0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e";
const DEFAULT_ETHEREUM_RPC_URL: &str = "https://ethereum-rpc.publicnode.com";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = globalThis, js_name = fetch, catch)]
    async fn rpc_fetch(url: &str, init: &RequestInit) -> Result<Response, JsValue>;
}

async fn query(rpc_url: &str, to: Address, signature: &str, node: &[u8; 32]) -> Option<Vec<u8>> {
    let data = abi_call(signature, &[*node]);
    let headers = Headers::new().ok()?;
    headers.set("Content-Type", "application/json").ok()?;
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_headers(&headers);
    init.set_body(
        &serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "eth_call",
            "params": [{"to": to, "data": format!("0x{}", hex::encode(data))}, "latest"]
        }))
        .ok()?
        .into(),
    );
    let response = rpc_fetch(rpc_url, &init).await.ok()?;
    if !response.ok() {
        return None;
    }
    let body = JsFuture::from(response.array_buffer().ok()?).await.ok()?;
    let bytes = js_sys::Uint8Array::new(&body).to_vec();
    let result = web3::helpers::to_result_from_output(
        web3::helpers::arbitrary_precision_deserialize_workaround(&bytes).ok()?,
    )
    .ok()?;
    let encoded = result.as_str()?.strip_prefix("0x")?;
    hex::decode(encoded).ok()
}

pub(crate) async fn resolve_ens_reference(name: String, rpc_url: &str) -> Vec<u8> {
    async {
        let rpc_url = if rpc_url.is_empty() {
            DEFAULT_ETHEREUM_RPC_URL
        } else {
            rpc_url
        };
        web_sys::Url::new(rpc_url).ok()?;
        let node = namehash(if name.is_empty() { "swarm.eth" } else { &name });
        let registry = ENS_REGISTRY_ADDRESS.parse().ok()?;
        let bytes = query(rpc_url, registry, "resolver(bytes32)", &node).await?;
        let resolver = Address::from_slice(&abi_word(&bytes, 0).ok()?[12..]);
        if resolver == Address::zero() {
            return None;
        }
        let bytes = query(rpc_url, resolver, "contenthash(bytes32)", &node).await?;
        let content_hash = abi_bytes(&bytes)?;
        (content_hash.len() > 7 && content_hash[0] == 0xe4).then(|| content_hash[7..].to_vec())
    }
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn ens_calls_encode_standard_selectors() {
        for (signature, selector) in [
            ("resolver(bytes32)", "0178b8bf"),
            ("contenthash(bytes32)", "bc1c58d1"),
        ] {
            assert_eq!(
                hex::encode(abi_call(signature, &[[1; 32]])),
                format!("{selector}{}", "01".repeat(32))
            );
        }
    }
}
