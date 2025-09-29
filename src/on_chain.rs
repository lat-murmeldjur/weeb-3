use std::str::FromStr;
use web3::{
    contract::{Contract, Options},
    transports::eip_1193::{Eip1193, Provider},
    types::{Address, U256},
};

pub async fn get_batch_validity(batch_id: Vec<u8>) -> U256 {
    let provider = Provider::default().unwrap().unwrap();

    let transport = Eip1193::new(provider);
    let web3 = web3::Web3::new(transport);
    let _accounts = match web3.eth().request_accounts().await {
        Ok(aok) => aok,
        _ => return U256::from(1),
    };

    let contract_address = match Address::from_str("cdfdC3752caaA826fE62531E0000C40546eC56A6") {
        Ok(aok) => aok,
        _ => return U256::from(1),
    };

    let contract = match Contract::from_json(
        web3.eth(),
        contract_address,
        include_bytes!("./postagestamp.json"),
    ) {
        Ok(aok) => aok,
        _ => return U256::from(1),
    };

    let id_bytes_32: [u8; 32] = batch_id.try_into().unwrap();

    let result_lp: U256 = contract
        .query(
            "remainingBalance",
            (id_bytes_32,),
            None,
            Options::default(),
            None,
        )
        .await
        .unwrap();

    return result_lp;
}
