use wasm_bindgen::JsError;
use web3::ethabi::{Contract, Function, Param, ParamType, StateMutability, Token};
use web3::{
    contract::{Contract as Web3Contract, Options, tokens::Tokenize},
    transports::eip_1193::Eip1193,
    types::{Address, TransactionReceipt, U256},
};

pub(crate) fn abi_contract(functions: impl IntoIterator<Item = Function>) -> Contract {
    let mut contract = Contract::default();
    for function in functions {
        contract
            .functions
            .insert(function.name.clone(), vec![function]);
    }
    contract
}

#[allow(deprecated)]
pub(crate) fn abi_function(
    name: &str,
    inputs: &[(&str, ParamType)],
    outputs: &[ParamType],
    state_mutability: StateMutability,
) -> Function {
    let parameter = |name: &str, kind: &ParamType| Param {
        name: name.to_string(),
        kind: kind.clone(),
        internal_type: None,
    };
    Function {
        name: name.to_string(),
        inputs: inputs
            .iter()
            .map(|(name, kind)| parameter(name, kind))
            .collect(),
        outputs: outputs.iter().map(|kind| parameter("", kind)).collect(),
        constant: None,
        state_mutability,
    }
}

struct ContractArgs(Vec<Token>);

impl Tokenize for ContractArgs {
    fn into_tokens(self) -> Vec<Token> {
        self.0
    }
}

pub(crate) async fn confirmed_call(
    contract: &Web3Contract<Eip1193>,
    method: &str,
    parameters: Vec<Token>,
    payer: Address,
    fallback_gas: Option<u64>,
) -> Result<TransactionReceipt, JsError> {
    let mut options = Options::default();
    if let Some(fallback) = fallback_gas {
        let gas = contract
            .estimate_gas(method, parameters.as_slice(), payer, Options::default())
            .await
            .unwrap_or(U256::from(fallback));
        options.gas = Some(add_buffer(gas));
    }
    contract
        .call_with_confirmations(method, ContractArgs(parameters), payer, options, 1usize)
        .await
        .map_err(|e| JsError::new(&format!("{method}() failed: {e}")))
}

pub(crate) fn add_buffer(g: U256) -> U256 {
    g + (g / U256::from(5u8))
}
