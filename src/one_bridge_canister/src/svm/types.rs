use serde::Deserialize;
use serde_json::Value;
use std::str::FromStr;

pub use solana_hash::Hash;
pub use solana_pubkey::Pubkey;
pub use solana_transaction::{Message, Signature, Transaction};

/// The subset of a Solana account returned by `getAccountInfo` that contract
/// registration needs. Keeping the parsed payload untyped avoids pulling the
/// validator-side account decoder and its large dependency graph into the
/// canister Wasm.
#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct UiAccount {
    pub data: Value,
    pub owner: String,
}

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

pub fn get_mint_decimals(account: &UiAccount) -> Result<u8, String> {
    let account_type = account
        .data
        .pointer("/parsed/type")
        .and_then(Value::as_str)
        .ok_or_else(|| "account data is not JSON-parsed token data".to_string())?;
    if account_type != "mint" {
        return Err("account is not a token mint account".to_string());
    }

    let decimals = account
        .data
        .pointer("/parsed/info/decimals")
        .and_then(Value::as_u64)
        .ok_or_else(|| "token mint decimals are missing or invalid".to_string())?;
    u8::try_from(decimals).map_err(|_| "token mint decimals exceed u8".to_string())
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

    #[test]
    fn extracts_only_the_mint_fields_from_account_info() {
        let account: UiAccount = serde_json::from_value(json!({
            "owner": "TokenzQdYh...",
            "lamports": 1_461_600,
            "data": {
                "program": "spl-token-2022",
                "parsed": {
                    "type": "mint",
                    "info": { "decimals": 8, "supply": "1000000" }
                },
                "space": 82
            }
        }))
        .unwrap();

        assert_eq!(get_mint_decimals(&account), Ok(8));

        let mut not_a_mint = account;
        not_a_mint.data["parsed"]["type"] = json!("account");
        assert!(get_mint_decimals(&not_a_mint).is_err());
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::svm::system_transfer_instruction;

    /// `solana-transaction` dropped its `bincode` feature, so the crate now enables
    /// `serde` instead. Both give `Transaction` the same short-vec wire encoding,
    /// and this pins that down: a wrong layout here would mean signing and
    /// broadcasting malformed transactions.
    #[test]
    fn transaction_bincode_layout_matches_solana_wire_format() {
        let from = Pubkey::new_from_array([1u8; 32]);
        let to = Pubkey::new_from_array([2u8; 32]);
        let blockhash = Hash::new_from_array([3u8; 32]);

        let ix = system_transfer_instruction(&from, &to, 1_000);
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
