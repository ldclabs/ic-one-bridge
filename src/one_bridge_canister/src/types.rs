use candid::CandidType;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use serde_json::Value;

use crate::{evm::Address, svm::Pubkey};

#[derive(CandidType, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PublicKeyOutput {
    pub public_key: ByteBuf,
    pub chain_code: ByteBuf,
}

impl PublicKeyOutput {
    pub fn to_svm_pubkey(&self) -> Result<Pubkey, String> {
        Pubkey::try_from(self.public_key.as_slice())
            .map_err(|_| "Failed to convert to SVM pubkey".to_string())
    }

    pub fn to_evm_adress(&self) -> Result<Address, String> {
        use k256::elliptic_curve::sec1::ToEncodedPoint;
        let key = k256::PublicKey::from_sec1_bytes(self.public_key.as_slice())
            .map_err(|_| "Failed to convert to EVM address".to_string())?;
        let point = key.to_encoded_point(false);
        Ok(Address::from_raw_public_key(&point.as_bytes()[1..]))
    }
}

#[derive(Debug, Serialize)]
pub struct RPCRequest<'a> {
    pub jsonrpc: &'a str,
    pub method: &'a str,
    pub params: &'a [Value],
    pub id: u64,
}

#[derive(Debug, Deserialize)]
pub struct RPCResponse<T> {
    pub result: Option<T>,
    pub error: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ecdsa::derive_public_key, schnorr::derive_schnorr_public_key};
    use candid::Principal;
    use serde_bytes::ByteBuf;

    /// Every user's EVM and Solana address is derived from the subnet master key
    /// and their principal, and funds sit at those addresses. A dependency bump
    /// that quietly changes the derivation would strand them, so the mapping is
    /// pinned here against the real mainnet master keys.
    fn master(public_key: Vec<u8>) -> PublicKeyOutput {
        PublicKeyOutput {
            public_key: ByteBuf::from(public_key),
            chain_code: ByteBuf::from(vec![0x5au8; 32]),
        }
    }

    #[test]
    fn derived_addresses_are_stable() {
        let user = Principal::from_text("druyg-tyaaa-aaaaq-aactq-cai").unwrap();
        let path = vec![user.as_slice().to_vec()];

        let ecdsa = master(
            ic_secp256k1::PublicKey::mainnet_key(ic_secp256k1::MasterPublicKeyId::EcdsaKey1)
                .serialize_sec1(true),
        );
        let evm = derive_public_key(&ecdsa, path.clone())
            .unwrap()
            .to_evm_adress()
            .unwrap();

        let ed25519 = master(
            ic_ed25519::PublicKey::mainnet_key(ic_ed25519::MasterPublicKeyId::Key1)
                .serialize_raw()
                .to_vec(),
        );
        let svm = derive_schnorr_public_key(&ed25519, path, None)
            .unwrap()
            .to_svm_pubkey()
            .unwrap();

        // verified byte-for-byte against ic-ed25519 0.4 / ic-secp256k1 0.3, the
        // versions these addresses were first derived with
        assert_eq!(
            evm.to_string(),
            "0x01888A81e70d88d51F5cB6787e09Cfc0A8534d7C"
        );
        assert_eq!(
            svm.to_string(),
            "9tELHYX7gorpW39eDs6sCBFJDcwY6t85ht8CL2AsE7xT"
        );
    }
}
