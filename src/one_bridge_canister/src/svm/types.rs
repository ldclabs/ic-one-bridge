use serde::Deserialize;
use serde_json::Value;
use std::str::FromStr;

pub use solana_account_decoder_client_types::{UiAccount, UiAccountData, token::TokenAccountType};
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

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::svm::instruction;

    /// `solana-transaction` dropped its `bincode` feature, so the crate now enables
    /// `serde` instead. Both give `Transaction` the same short-vec wire encoding,
    /// and this pins that down: a wrong layout here would mean signing and
    /// broadcasting malformed transactions.
    #[test]
    fn transaction_bincode_layout_matches_solana_wire_format() {
        let from = Pubkey::new_from_array([1u8; 32]);
        let to = Pubkey::new_from_array([2u8; 32]);
        let blockhash = Hash::new_from_array([3u8; 32]);

        let ix = instruction::transfer(&from, &to, 1_000);
        let message = Message::new_with_blockhash(&[ix], Some(&from), &blockhash);
        let tx = Transaction {
            message,
            signatures: vec![Signature::from([7u8; 64])],
        };

        let bytes = bincode::serialize(&tx).unwrap();

        // compact-u16 signature count, then the signature itself
        assert_eq!(bytes[0], 1);
        assert_eq!(&bytes[1..65], &[7u8; 64]);

        // message header: 1 required signature, 0 readonly signed, 1 readonly
        // unsigned (the system program), then a compact-u16 account count
        assert_eq!(&bytes[65..68], &[1, 0, 1]);
        assert_eq!(bytes[68], 3);
        assert_eq!(&bytes[69..101], &[1u8; 32]); // payer
        assert_eq!(&bytes[101..133], &[2u8; 32]); // recipient
        assert_eq!(&bytes[133..165], &[0u8; 32]); // system program
        assert_eq!(&bytes[165..197], &[3u8; 32]); // recent blockhash

        // one instruction: program index 2, accounts [0, 1], 12 bytes of data
        // holding SystemInstruction::Transfer (2) and 1000 lamports little-endian
        assert_eq!(&bytes[197..203], &[1, 2, 2, 0, 1, 12]);
        assert_eq!(&bytes[203..215], &[2, 0, 0, 0, 232, 3, 0, 0, 0, 0, 0, 0]);
        assert_eq!(bytes.len(), 215);
    }
}
