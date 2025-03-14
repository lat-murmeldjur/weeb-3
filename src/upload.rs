use alloy::primitives::keccak256;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;

use js_sys::Date;

pub async fn stamp_chunk(
    //
    // stamp_signer: Signer,
    batch_id: Vec<u8>,
    // batch_buckets: HashMap<u32, u32>
    chunk_address: Vec<u8>,
    //
) -> Vec<u8> {
    let stamp_signer_key = keccak256("Key To Be Persisted In Browser Localstore");
    let stamp_signer: PrivateKeySigner = match PrivateKeySigner::from_bytes(&stamp_signer_key) {
        Ok(aok) => aok,
        _ => return vec![],
    };

    let bucket = u32::from_be_bytes(chunk_address[..4].try_into().unwrap()) >> (32 - 16);
    let index = 0_u32;
    let index_bytes = [bucket.to_be_bytes(), index.to_be_bytes()].concat();

    let timestamp: u64 = (Date::now() as u64) * 1000000;
    let timestamp_bytes = timestamp.to_be_bytes();

    let to_sign_digest = keccak256(
        [
            chunk_address.clone(),
            batch_id.clone(),
            index_bytes.clone(),
            timestamp_bytes.to_vec(),
        ]
        .concat(),
    );

    let signature = stamp_signer
        .sign_message(to_sign_digest.as_slice())
        .await
        .unwrap()
        .as_bytes()
        .to_vec();

    let stamp = [batch_id, index_bytes, timestamp_bytes.to_vec(), signature].concat();

    stamp
    //    if n := copy(buf, s.batchID); n != 32 {
    //        return nil, ErrInvalidBatchID
    //    }
    //    if n := copy(buf[32:40], s.index); n != 8 {
    //        return nil, ErrInvalidBatchIndex
    //    }
    //    if n := copy(buf[40:48], s.timestamp); n != 8 {
    //        return nil, ErrInvalidBatchTimestamp
    //    }
    //    if n := copy(buf[48:], s.sig); n != 65 {
    //        return nil, ErrInvalidBatchSignature
    //    }
}
