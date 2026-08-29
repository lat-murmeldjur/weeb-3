use alloy_primitives::{Address, B256, Signature, eip191_hash_message};
use k256::ecdsa::{
    RecoveryId, Signature as K256Signature, SigningKey, signature::hazmat::PrehashSigner,
};

#[derive(Clone)]
pub(crate) struct PrivateKeySigner {
    key: SigningKey,
    address: Address,
}

impl PrivateKeySigner {
    pub(crate) fn from_slice(bytes: &[u8]) -> Result<Self, k256::ecdsa::Error> {
        let key = SigningKey::from_slice(bytes)?;
        let public_key = key.verifying_key().to_encoded_point(false);
        let address = Address::from_raw_public_key(&public_key.as_bytes()[1..]);
        Ok(Self { key, address })
    }

    pub(crate) fn address(&self) -> Address {
        self.address
    }

    pub(crate) fn sign_message(&self, message: &[u8]) -> Result<Signature, k256::ecdsa::Error> {
        self.sign_hash_sync(&eip191_hash_message(message))
    }

    pub(crate) fn sign_hash_sync(&self, hash: &B256) -> Result<Signature, k256::ecdsa::Error> {
        let signature: (K256Signature, RecoveryId) = self.key.sign_prehash(hash.as_slice())?;
        Ok(signature.into())
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn eip191_message_signature_is_stable() {
        let signer = PrivateKeySigner::from_slice(&[1; 32]).expect("fixture private key");
        let message = b"weeb-3 handshake signing fixture";
        let signature = signer.sign_message(message).expect("sign fixture");

        assert_eq!(
            hex::encode(signature.as_bytes()),
            "e681a6ac3223272ccd5a24cc94799dda749e9983c9153ee226bd3a682993ed42460e63138750648a287c0a04624721a5cbfa141f7d507e58ad4e9cc294c73e061c"
        );
        assert_eq!(
            signature
                .recover_address_from_msg(message)
                .expect("recover fixture signer"),
            signer.address()
        );
        assert_eq!(
            hex::encode(signer.address()),
            "1a642f0e3c3af545e7acbd38b07251b3990914f1"
        );
    }
}
