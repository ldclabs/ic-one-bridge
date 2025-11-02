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
