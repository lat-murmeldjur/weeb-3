pub async fn stamp_chunk_var(
    stamp_signer_key: Vec<u8>,
    batch_id: Vec<u8>,
    batch_bucket_limit: u32,
    data0: Vec<u8>,
    mut encrey0: Vec<u8>,
    span_length: usize,
    encryption: bool,
) -> (Vec<u8>, Vec<u8>) {
    let mut cha = content_address(&data0);
    let mut cstamp0: Vec<u8> = vec![];
    let mut bucket_full: bool;

    for _ in 0..BATCH_BUCKET_TRIALS {
        (cstamp0, bucket_full) = stamp_chunk(
            batch_owner.clone(),
            batch_id.clone(),
            batch_bucket_limit,
            cha.clone(),
        )
        .await;

        if !bucket_full {
            break;
        } else {
            match encryption {
                true => {
                    encrey0 = encrey();
                    data0 = encrypt(span_length, &data[0], &encrey0);
                    cha = content_address(&data0);
                }
                false => {
                    soc = true;
                    (data0, cha) = make_soc(
                        &[span_length.to_le_bytes().to_vec(), data[0].clone()].concat(),
                        encrey(),
                        encrey(),
                    )
                    .await;
                }
            }
        }
    }
}
