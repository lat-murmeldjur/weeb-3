#![cfg(target_arch = "wasm32")]

use crate::on_chain_conventions::abi_function;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, RequestInit, Response};
use web3::{
    ethabi::{ParamType, StateMutability, Token},
    signing::namehash,
    types::Address,
};

const ENS_REGISTRY_ADDRESS: &str = "0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e";
const DEFAULT_ETHEREUM_RPC_URL: &str = "https://ethereum-rpc.publicnode.com";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = globalThis, js_name = fetch, catch)]
    async fn rpc_fetch(url: &str, init: &RequestInit) -> Result<Response, JsValue>;
}

async fn query(
    rpc_url: &str,
    to: Address,
    method: &str,
    output: ParamType,
    node: &[u8],
) -> Option<Token> {
    let function = abi_function(
        method,
        &[("node", ParamType::FixedBytes(32))],
        &[output],
        StateMutability::View,
    );
    let data = function
        .encode_input(&[Token::FixedBytes(node.to_vec())])
        .ok()?;
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
    function
        .decode_output(&hex::decode(encoded).ok()?)
        .ok()?
        .into_iter()
        .next()
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
        let resolver = query(rpc_url, registry, "resolver", ParamType::Address, &node)
            .await?
            .into_address()?;
        if resolver == Address::zero() {
            return None;
        }
        let content_hash = query(rpc_url, resolver, "contenthash", ParamType::Bytes, &node)
            .await?
            .into_bytes()?;
        (content_hash.len() > 7 && content_hash[0] == 0xe4).then(|| content_hash[7..].to_vec())
    }
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn ens_abis_encode_standard_selectors() {
        for (output, method, selector) in [
            (ParamType::Address, "resolver", "0178b8bf"),
            (ParamType::Bytes, "contenthash", "bc1c58d1"),
        ] {
            let abi = abi_function(
                method,
                &[("node", ParamType::FixedBytes(32))],
                &[output.clone()],
                StateMutability::View,
            );
            let json = format!(
                r#"[{{"inputs":[{{"name":"node","type":"bytes32"}}],"name":"{method}","outputs":[{{"name":"","type":"{output}"}}],"stateMutability":"view","type":"function"}}]"#
            );
            let original = web3::ethabi::Contract::load(json.as_bytes()).unwrap();
            assert_eq!(&abi, original.function(method).unwrap());
            let input = abi.encode_input(&[Token::FixedBytes(vec![1; 32])]).unwrap();
            assert_eq!(hex::encode(input), format!("{selector}{}", "01".repeat(32)));
        }
    }
}
