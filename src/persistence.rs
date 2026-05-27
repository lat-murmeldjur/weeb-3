use crate::JsValue;

use indexed_db_futures::database::Database;
use indexed_db_futures::prelude::*;
use indexed_db_futures::transaction::TransactionMode;

async fn cat_base(identifier: String) -> Option<Database> {
    let db = match Database::open("weeb_".to_string() + &identifier)
        .with_version(1u8)
        .with_on_upgrade_needed(|event, db| {
            match (event.old_version(), event.new_version()) {
                (0.0, Some(1.0)) => {
                    let _ = db
                        .create_object_store("weeb_datastore")
                        .with_auto_increment(true)
                        .build();
                }
                _ => {}
            }

            Ok(())
        })
        .await
    {
        Ok(db0) => db0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!("error opening database: {}", e)));
            return None;
        }
    };

    Some(db)
}

#[allow(dead_code)]
pub async fn cache_chunk(chunk_address: &Vec<u8>, chunk_content: &Vec<u8>) {
    let db = match cat_base("chunk_cachestore".to_string()).await {
        Some(db0) => db0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open database for chunk cache"
            )));
            return;
        }
    };

    let transaction = match db
        .transaction("weeb_datastore")
        .with_mode(TransactionMode::Readwrite)
        .build()
    {
        Ok(t0) => t0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open transaction for chunk caching {:#?}",
                e
            )));
            return;
        }
    };

    let store = match transaction.object_store("weeb_datastore") {
        Ok(s0) => s0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open datastore for chunk caching {:#?}",
                e
            )));
            return;
        }
    };

    match store
        .put(chunk_content)
        .with_key(hex::encode(chunk_address))
        .primitive()
    {
        Ok(_) => {}
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed put for chunk caching {:#?}",
                e
            )));
        }
    };

    let _ = match transaction.commit().await {
        Ok(_) => {}
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to commit put for chunk caching {:#?}",
                e
            )));
        }
    };
    return;
}

#[allow(dead_code)]
pub async fn retrieve_cached_chunk(chunk_address: &Vec<u8>) -> Vec<u8> {
    let db = match cat_base("chunk_cachestore".to_string()).await {
        Some(db0) => db0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open database for reading chunk cache"
            )));

            return vec![];
        }
    };

    let transaction = match db
        .transaction("weeb_datastore")
        .with_mode(TransactionMode::Readonly)
        .build()
    {
        Ok(t0) => t0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open transaction for reading chunk cache {:#?}",
                e
            )));
            return vec![];
        }
    };

    let store = match transaction.object_store("weeb_datastore") {
        Ok(s0) => s0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open datastore for reading chunk cache {:#?}",
                e
            )));
            return vec![];
        }
    };

    let chunk_data: Vec<u8> = match store
        .get(hex::encode(chunk_address))
        .primitive()
        .unwrap()
        .await
    {
        Ok(Some(b)) => b,
        _ => vec![],
    };

    let _ = transaction.commit().await;

    return chunk_data;
}

pub async fn get_batch_field(field: String) -> Vec<u8> {
    let db = match cat_base("batchstore_data".to_string()).await {
        Some(db0) => db0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open database for batch metadata {}",
                field
            )));
            return vec![];
        }
    };

    let transaction = match db
        .transaction("weeb_datastore")
        .with_mode(TransactionMode::Readonly)
        .build()
    {
        Ok(t0) => t0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open transaction for batch metadata {} {:#?}",
                field, e
            )));
            return vec![];
        }
    };

    let store = match transaction.object_store("weeb_datastore") {
        Ok(s0) => s0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open datastore for batch metadata {} {:#?}",
                field, e
            )));
            return vec![];
        }
    };

    let key_data0 = match store.get(field).primitive() {
        Ok(aok) => aok,
        _ => return vec![],
    };

    let key_data: Vec<u8> = match key_data0.await {
        Ok(Some(b)) => b,
        _ => vec![],
    };

    let _ = transaction.commit().await;

    return key_data;
}

pub async fn set_batch_field(field: String, value: &Vec<u8>) -> bool {
    let db = match cat_base("batchstore_data".to_string()).await {
        Some(db0) => db0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open database for setting batch metadata {}",
                field
            )));
            return false;
        }
    };

    let transaction = match db
        .transaction("weeb_datastore")
        .with_mode(TransactionMode::Readwrite)
        .build()
    {
        Ok(t0) => t0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open transaction for setting batch metadata {} {:#?}",
                field, e
            )));
            return false;
        }
    };

    let store = match transaction.object_store("weeb_datastore") {
        Ok(s0) => s0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open datastore for setting batch metadata {} {:#?}",
                field, e
            )));
            return false;
        }
    };

    match store.put(value).with_key(field.clone()).primitive() {
        Ok(_) => {}
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to put for setting batch metadata {} {:#?}",
                field, e
            )));
            let _ = transaction.commit().await;
            return false;
        }
    };

    let _ = match transaction.commit().await {
        Ok(_) => {}
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to commit put for setting batch metadata {} {:#?}",
                field, e
            )));
            return false;
        }
    };
    return true;
}

pub async fn get_chequebook_signer_key() -> Vec<u8> {
    return get_batch_field("chequebook_signer_key".to_string()).await;
}

pub async fn set_chequebook_signer_key(key: &Vec<u8>) -> bool {
    return set_batch_field("chequebook_signer_key".to_string(), key).await;
}

pub async fn get_chequebook_address() -> Vec<u8> {
    return get_batch_field("chequebook_address".to_string()).await;
}

pub async fn set_chequebook_address(addr: &Vec<u8>) -> bool {
    return set_batch_field("chequebook_address".to_string(), addr).await;
}

fn chequebook_last_issued_cheque_payout_key(chequebook: &[u8], beneficiary: &[u8]) -> String {
    return format!(
        "swap_chequebook_last_issued_cheque_{}_{}",
        hex::encode(chequebook),
        hex::encode(beneficiary)
    );
}

pub async fn get_chequebook_last_issued_cheque_payout(
    chequebook: &[u8],
    beneficiary: &[u8],
) -> Vec<u8> {
    return get_batch_field(chequebook_last_issued_cheque_payout_key(
        chequebook,
        beneficiary,
    ))
    .await;
}

pub async fn set_chequebook_last_issued_cheque_payout(
    chequebook: &[u8],
    beneficiary: &[u8],
    payout: &Vec<u8>,
) -> bool {
    return set_batch_field(
        chequebook_last_issued_cheque_payout_key(chequebook, beneficiary),
        payout,
    )
    .await;
}
