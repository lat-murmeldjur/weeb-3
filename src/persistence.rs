use crate::JsValue;

use indexed_db_futures::database::Database;
use indexed_db_futures::prelude::*;
use indexed_db_futures::transaction::TransactionMode;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct BucketData {
    id: String,
    value: u32,
}

pub async fn reset_stamp(identifier: &String) {
    let db = match Database::open("weeb_".to_string() + &identifier).await {
        Ok(db0) => db0,
        Err(e) => {
            web_sys::console::log_1(&JsValue::from(format!("error opening database: {}", e)));
            return;
        }
    };
    let _ = db.delete();
}

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
        _ => return None,
    };

    Some(db)
}

pub async fn bump_bucket(stamp_identifier: String, bucket_identifier: String) -> (bool, u32) {
    let mut in_weeb = 0;

    let db = match cat_base(stamp_identifier).await {
        Some(db0) => db0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!("ep0")));
            return (false, in_weeb);
        }
    };

    let transaction = match db
        .transaction("weeb_datastore")
        .with_mode(TransactionMode::Readwrite)
        .build()
    {
        Ok(t0) => t0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!("ep1")));
            return (false, in_weeb);
        }
    };

    let store = match transaction.object_store("weeb_datastore") {
        Ok(s0) => s0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!("ep2")));
            return (false, in_weeb);
        }
    };

    let bucket: BucketData = match store.get(bucket_identifier.clone()).serde().unwrap().await {
        Ok(Some(b)) => b,
        _ => BucketData {
            id: bucket_identifier,
            value: 0,
        },
    };

    in_weeb = bucket.value;

    let b1 = BucketData {
        id: bucket.id,
        value: bucket.value + 1,
    };

    // awaiting individual requests is optional - they still go out
    match store.put(b1.clone()).with_key(b1.id).serde() {
        Ok(_) => {}
        _ => {
            web_sys::console::log_1(&JsValue::from(format!("ep3")));
            return (false, in_weeb);
        }
    };

    return match transaction.commit().await {
        Ok(_) => (true, in_weeb),
        _ => (false, in_weeb),
    };
}

pub async fn cache_chunk(chunk_address: Vec<u8>, chunk_content: Vec<u8>) {
    let db = match cat_base("chunk_cachestore".to_string()).await {
        Some(db0) => db0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!("chunk cache store failed to open")));
            return;
        }
    };

    let transaction = match db
        .transaction("weeb_datastore")
        .with_mode(TransactionMode::Readwrite)
        .build()
    {
        Ok(t0) => t0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!(
                "chunk cache transaction failed to open"
            )));
            return;
        }
    };

    let store = match transaction.object_store("weeb_datastore") {
        Ok(s0) => s0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!(
                "chunk cache datastore failed to open"
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
        _ => {
            web_sys::console::log_1(&JsValue::from(format!("chunk cache store put failed")));
        }
    };

    let _ = transaction.commit().await;

    return;
}

pub async fn retrieve_cached_chunk(chunk_address: Vec<u8>) -> Vec<u8> {
    let db = match cat_base("chunk_cachestore".to_string()).await {
        Some(db0) => db0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!("chunk cache store failed to open")));
            return vec![];
        }
    };

    let transaction = match db
        .transaction("weeb_datastore")
        .with_mode(TransactionMode::Readwrite)
        .build()
    {
        Ok(t0) => t0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!(
                "chunk cache transaction failed to open"
            )));
            return vec![];
        }
    };

    let store = match transaction.object_store("weeb_datastore") {
        Ok(s0) => s0,
        _ => {
            web_sys::console::log_1(&JsValue::from(format!(
                "chunk cache datastore failed to open"
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

    transaction.commit().await;

    return chunk_data;
}
