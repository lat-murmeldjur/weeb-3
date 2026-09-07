use web3::ethabi::{Contract, Function, Param, ParamType, StateMutability};

pub(crate) fn abi_contract(functions: impl IntoIterator<Item = Function>) -> Contract {
    let mut contract = Contract::default();
    for function in functions {
        contract.functions.insert(function.name.clone(), vec![function]);
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
