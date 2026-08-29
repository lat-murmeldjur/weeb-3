use crate::JsValue;

use indexed_db_futures::database::Database;
use indexed_db_futures::prelude::*;
use indexed_db_futures::transaction::TransactionMode;
use std::fmt::Debug;

const DATASTORE: &str = "weeb_datastore";
const BATCH_DATABASE: &str = "weeb_batchstore_data";

fn log_failure<T, E: Debug>(result: Result<T, E>, action: &str, field: &str) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to {action} batch metadata {field}: {error:?}"
            )));
            None
        }
    }
}

async fn open_batch_database() -> Option<Database> {
    match Database::open(BATCH_DATABASE)
        .with_version(1u8)
        .with_on_upgrade_needed(|event, db| {
            if event.old_version() == 0.0 && event.new_version() == Some(1.0) {
                let _ = db
                    .create_object_store(DATASTORE)
                    .with_auto_increment(true)
                    .build();
            }
            Ok(())
        })
        .await
    {
        Ok(database) => Some(database),
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to open batch database: {error}"
            )));
            None
        }
    }
}

pub async fn get_batch_field(field: &str) -> Vec<u8> {
    let Some(db) = open_batch_database().await else {
        return vec![];
    };

    let Some(transaction) = log_failure(
        db.transaction(DATASTORE)
            .with_mode(TransactionMode::Readonly)
            .build(),
        "open a read transaction for",
        field,
    ) else {
        return vec![];
    };
    let Some(store) = log_failure(
        transaction.object_store(DATASTORE),
        "open the datastore for",
        field,
    ) else {
        return vec![];
    };
    let Ok(request) = store.get(field).primitive() else {
        return vec![];
    };
    let key_data = request.await.ok().flatten().unwrap_or_default();

    let _ = transaction.commit().await;
    key_data
}

pub async fn set_batch_field(field: &str, value: &[u8]) -> bool {
    let Some(db) = open_batch_database().await else {
        return false;
    };

    let Some(transaction) = log_failure(
        db.transaction(DATASTORE)
            .with_mode(TransactionMode::Readwrite)
            .build(),
        "open a write transaction for",
        field,
    ) else {
        return false;
    };
    let Some(store) = log_failure(
        transaction.object_store(DATASTORE),
        "open the datastore for",
        field,
    ) else {
        return false;
    };
    if log_failure(store.put(value).with_key(field).primitive(), "write", field).is_none() {
        let _ = transaction.commit().await;
        return false;
    }

    match transaction.commit().await {
        Ok(_) => true,
        Err(error) => {
            web_sys::console::log_1(&JsValue::from(format!(
                "Failed to commit batch metadata {field}: {error:?}"
            )));
            false
        }
    }
}

pub async fn get_chequebook_signer_key() -> Vec<u8> {
    get_batch_field("chequebook_signer_key").await
}

pub async fn set_chequebook_signer_key(key: &[u8]) -> bool {
    set_batch_field("chequebook_signer_key", key).await
}

pub async fn get_chequebook_address() -> Vec<u8> {
    get_batch_field("chequebook_address").await
}

pub async fn set_chequebook_address(addr: &[u8]) -> bool {
    set_batch_field("chequebook_address", addr).await
}

fn chequebook_last_issued_cheque_payout_key(chequebook: &[u8], beneficiary: &[u8]) -> String {
    format!(
        "swap_chequebook_last_issued_cheque_{}_{}",
        hex::encode(chequebook),
        hex::encode(beneficiary)
    )
}

pub async fn get_chequebook_last_issued_cheque_payout(
    chequebook: &[u8],
    beneficiary: &[u8],
) -> Vec<u8> {
    let key = chequebook_last_issued_cheque_payout_key(chequebook, beneficiary);
    get_batch_field(&key).await
}

pub async fn set_chequebook_last_issued_cheque_payout(
    chequebook: &[u8],
    beneficiary: &[u8],
    payout: &[u8],
) -> bool {
    let key = chequebook_last_issued_cheque_payout_key(chequebook, beneficiary);
    set_batch_field(&key, payout).await
}
