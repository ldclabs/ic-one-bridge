use serde::Deserialize;
use serde_json::Value;
use std::str::FromStr;

pub use solana_account_decoder_client_types::{
    UiAccount, UiAccountData,
    token::{TokenAccountType, UiTokenAmount},
};
pub use solana_program::{hash::Hash, pubkey::Pubkey};
pub use solana_transaction::{Message, Signature, Transaction};

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LatestBlockhash {
    pub blockhash: String,
    pub last_valid_block_height: u64,
}

impl LatestBlockhash {
    pub fn to_hash(&self) -> Result<Hash, String> {
        Hash::from_str(&self.blockhash).map_err(|e| format!("Failed to parse blockhash: {}", e))
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SignatureStatus {
    pub slot: u64,
    pub confirmations: Option<u64>,
    // processed、confirmed 或 finalized
    pub confirmation_status: Option<String>,
    pub err: Option<Value>,
}

impl SignatureStatus {
    /// A transaction only counts as finalized when it also succeeded: a failed
    /// transaction still reaches the `finalized` commitment level, but it moved
    /// no funds and must never be treated as a completed transfer.
    pub fn is_finalized(&self) -> bool {
        !self.is_error()
            && self
                .confirmation_status
                .as_deref()
                .map(|s| s == "finalized")
                .unwrap_or(false)
    }

    pub fn is_error(&self) -> bool {
        self.err.is_some()
    }
}

pub fn get_token_account(val: UiAccount) -> Result<TokenAccountType, String> {
    match val.data {
        UiAccountData::Json(parsed_account) => {
            let account: TokenAccountType = serde_json::from_value(parsed_account.parsed)
                .map_err(|err| format!("failed to parse TokenAccountType: {}", err))?;
            Ok(account)
        }
        _ => Err("UiAccount data is not in JSON format".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status(confirmation_status: &str, err: Option<Value>) -> SignatureStatus {
        SignatureStatus {
            slot: 1,
            confirmations: None,
            confirmation_status: Some(confirmation_status.to_string()),
            err,
        }
    }

    #[test]
    fn finalized_but_failed_transaction_is_not_finalized() {
        assert!(status("finalized", None).is_finalized());
        assert!(!status("confirmed", None).is_finalized());

        let failed = status(
            "finalized",
            Some(json!({"InstructionError": [0, "Custom"]})),
        );
        assert!(failed.is_error());
        assert!(!failed.is_finalized());
    }
}
