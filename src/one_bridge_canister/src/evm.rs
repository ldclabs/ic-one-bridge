use alloy_primitives::{U64, U256, hex::FromHex};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::outcall::{HttpOutcall, LARGE_RESPONSE, Replication, SMALL_RESPONSE, json_rpc_call};

pub use alloy_primitives::{Address, TxHash};

/// The only receipt fields finalization needs.
///
/// RPC providers return logs, bloom filters and gas metadata as well, but
/// deserializing all of that on every poll wastes instructions and heap memory.
/// Serde ignores those fields while retaining the transaction identity,
/// inclusion height and execution status needed by the bridge.
///
/// The quantity fields are [`U64`], whose deserializer accepts both `"0x1"`
/// and a bare JSON number — providers disagree on which they send, and a
/// decode failure here does not fail over (the provider already answered 2xx),
/// so a stricter parse would wedge the receipt poll on that provider forever.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvmReceipt {
    pub transaction_hash: TxHash,
    #[serde(default)]
    block_number: Option<U64>,
    status: U64,
}

impl EvmReceipt {
    pub fn block_number(&self) -> Option<u64> {
        self.block_number.map(|value| value.to::<u64>())
    }

    pub fn succeeded(&self) -> bool {
        self.status == U64::from(1)
    }
}

pub struct EvmClient<T: HttpOutcall> {
    pub providers: Vec<String>,
    pub max_confirmations: u64,
    outcall: T,
}

// https://ethereum.org/zh/developers/docs/apis/json-rpc/
impl<H: HttpOutcall> EvmClient<H> {
    pub fn new(providers: Vec<String>, max_confirmations: u64, outcall: H) -> Self {
        Self {
            providers,
            max_confirmations,
            outcall,
        }
    }

    pub async fn chain_id(&self, now_ms: u64) -> Result<u64, String> {
        let res: String = self
            .call(
                now_ms,
                "eth_chainId",
                &[],
                SMALL_RESPONSE,
                Replication::Single,
            )
            .await?;
        hex_to_u64(&res)
    }

    pub async fn gas_price(&self, now_ms: u64) -> Result<u128, String> {
        let res: String = self
            .call(
                now_ms,
                "eth_gasPrice",
                &[],
                SMALL_RESPONSE,
                Replication::Single,
            )
            .await?;
        hex_to_u128(&res)
    }

    pub async fn max_priority_fee_per_gas(&self, now_ms: u64) -> Result<u128, String> {
        let res: String = self
            .call(
                now_ms,
                "eth_maxPriorityFeePerGas",
                &[],
                SMALL_RESPONSE,
                Replication::Single,
            )
            .await?;
        hex_to_u128(&res)
    }

    pub async fn block_number(&self, now_ms: u64) -> Result<u64, String> {
        let res: String = self
            .call(
                now_ms,
                "eth_blockNumber",
                &[],
                SMALL_RESPONSE,
                Replication::Single,
            )
            .await?;
        hex_to_u64(&res)
    }

    pub async fn get_transaction_count(
        &self,
        now_ms: u64,
        address: &Address,
    ) -> Result<u64, String> {
        let res: String = self
            .call(
                now_ms,
                "eth_getTransactionCount",
                &[address.to_string().into(), "latest".into()],
                SMALL_RESPONSE,
                Replication::Single,
            )
            .await?;
        hex_to_u64(&res)
    }

    pub async fn get_transaction_receipt(
        &self,
        now_ms: u64,
        tx_hash: &TxHash,
    ) -> Result<Option<EvmReceipt>, String> {
        self.call(
            now_ms,
            "eth_getTransactionReceipt",
            &[tx_hash.to_string().into()],
            LARGE_RESPONSE,
            Replication::Single,
        )
        .await
    }

    /// Broadcasts a signed transaction.
    ///
    /// The result is deliberately decoded as an untyped [`Value`]: by the time it
    /// is parsed the transaction is already on its way to the mempool, and no
    /// caller reads it — they all know the hash from the transaction they signed.
    /// Insisting on a string here would turn a provider that answers with a null
    /// or an unexpected shape into a failed broadcast that did happen, which on
    /// the deposit path means a user's tokens move with no task recording it.
    pub async fn send_raw_transaction(
        &self,
        now_ms: u64,
        signed_tx: String,
    ) -> Result<Value, String> {
        self.call(
            now_ms,
            "eth_sendRawTransaction",
            &[signed_tx.into()],
            SMALL_RESPONSE,
            Replication::Single,
        )
        .await
    }

    pub async fn call_contract(
        &self,
        now_ms: u64,
        contract: &Address,
        call_data: String,
    ) -> Result<Vec<u8>, String> {
        let call_object = serde_json::json!({
            "to": contract.to_string(),
            "data": call_data,
        });

        let res: String = self
            .call(
                now_ms,
                "eth_call",
                &[call_object, "latest".into()],
                SMALL_RESPONSE,
                Replication::Single,
            )
            .await?;
        let res = res.strip_prefix("0x").unwrap_or(&res);
        <Vec<u8>>::from_hex(res).map_err(|err| err.to_string())
    }

    pub async fn erc20_decimals(&self, now_ms: u64, contract: &Address) -> Result<u8, String> {
        let res = self
            .call_contract(now_ms, contract, "0x313ce567".to_string())
            .await?;
        let v = decode_abi_uint(&res)?;
        u8::try_from(v).map_err(|_| "decimals overflow u8".to_string())
    }

    pub async fn call<T: DeserializeOwned>(
        &self,
        now_ms: u64,
        method: &str,
        params: &[Value],
        max_response_bytes: u64,
        replication: Replication,
    ) -> Result<T, String> {
        json_rpc_call(
            &self.outcall,
            &self.providers,
            now_ms,
            method,
            params,
            max_response_bytes,
            replication,
        )
        .await
    }
}

pub fn encode_erc20_transfer(to: &Address, value: u128) -> Vec<u8> {
    const TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb]; // keccak256("transfer(address,uint256)")[:4]

    let mut call_data = Vec::with_capacity(4 + 32 + 32);
    call_data.extend_from_slice(&TRANSFER_SELECTOR);

    let mut padded_to = [0u8; 32];
    padded_to[12..].copy_from_slice(to.as_slice());
    call_data.extend_from_slice(&padded_to);

    let value_bytes = U256::from(value).to_be_bytes::<32>();
    call_data.extend_from_slice(&value_bytes);

    call_data
}

fn hex_to_u64(s: &str) -> Result<u64, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    u64::from_str_radix(s, 16).map_err(|err| err.to_string())
}

fn hex_to_u128(s: &str) -> Result<u128, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    u128::from_str_radix(s, 16).map_err(|err| err.to_string())
}

fn decode_abi_uint(bytes: &[u8]) -> Result<U256, String> {
    if bytes.len() != 32 {
        return Err("abi uint result must be 32 bytes".to_string());
    }
    Ok(U256::from_be_slice(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcall::tests::{MockHttpOutcall, success_response};

    #[test]
    fn test_encode_erc20_transfer() {
        let addr = Address::from_hex("0x00112233445566778899aabbccddeeff00112233").unwrap();
        let encoded = encode_erc20_transfer(&addr, 12345);

        let mut expected = vec![0xa9, 0x05, 0x9c, 0xbb];
        expected.extend(vec![0u8; 12]);
        expected.extend_from_slice(addr.as_ref());
        expected.extend(U256::from(12345u128).to_be_bytes::<32>());

        assert_eq!(encoded, expected);
    }

    #[test]
    fn test_hex_to_u64_and_u128() {
        assert_eq!(hex_to_u64("0x2a").unwrap(), 42);
        assert_eq!(hex_to_u128("0xff").unwrap(), 255);
        assert!(hex_to_u64("g1").is_err());
        assert!(hex_to_u128("xyz").is_err());
    }

    #[test]
    fn test_decode_abi_uint() {
        let value = U256::from(999u64).to_be_bytes::<32>();
        assert_eq!(decode_abi_uint(&value).unwrap(), U256::from(999u64));
        assert!(decode_abi_uint(&value[..31]).is_err());
    }

    #[test]
    fn test_chain_id_uses_mock_outcall() {
        let mock = MockHttpOutcall::new(vec![success_response(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x2a"
        }))]);

        let client = EvmClient::new(vec!["https://rpc.one".to_string()], 5, mock.clone());
        let value = futures::executor::block_on(client.chain_id(1_000)).unwrap();

        assert_eq!(value, 42);
        assert_eq!(mock.urls(), vec!["https://rpc.one".to_string()]);
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE)]);
    }

    #[test]
    fn test_http_request_fallbacks_between_providers() {
        let mock = MockHttpOutcall::new(vec![
            Err("network down".to_string()),
            success_response(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": "0xa"
            })),
        ]);

        let client = EvmClient::new(
            vec!["https://first".to_string(), "https://second".to_string()],
            5,
            mock.clone(),
        );

        let block = futures::executor::block_on(client.block_number(2_000)).unwrap();

        assert_eq!(block, 10);
        assert_eq!(
            mock.urls(),
            vec!["https://first".to_string(), "https://second".to_string()]
        );
    }

    #[test]
    fn test_call_handles_error_payload() {
        let error_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32000, "message": "execution reverted"}
        });
        let mock = MockHttpOutcall::new(vec![success_response(error_body)]);
        let client = EvmClient::new(vec!["https://rpc".to_string()], 5, mock);

        let result: Result<u64, _> = futures::executor::block_on(client.call(
            1,
            "method",
            &[],
            SMALL_RESPONSE,
            Replication::Single,
        ));
        assert!(result.unwrap_err().contains("execution reverted"));
    }

    #[test]
    fn test_get_transaction_receipt() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": null
        });
        let mock = MockHttpOutcall::new(vec![success_response(body)]);
        let client = EvmClient::new(vec!["https://rpc".to_string()], 5, mock);

        let tx_hash = TxHash::from([0u8; 32]);
        let result = futures::executor::block_on(client.get_transaction_receipt(1000, &tx_hash));
        assert!(result.unwrap().is_none());

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "type": "0x2",
                "from": "0x9ac6b9ffbb4269fc51cf0ef7bcd322cefb3e5e14",
                "to": "0xe74583edaff618d88463554b84bc675196b36990",
                "status": "0x1",
                "cumulativeGasUsed": "0x3e45f",
                "logsBloom": "0x0000000000008000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000210800000000000000000000000000000000000000000000000000100000000000000000000004000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000800000000000000000000000000020000000000000000000000000000000000000000800000000000000",
                "logs": [
                    {
                        "address": "0xe74583edaff618d88463554b84bc675196b36990",
                        "topics": [
                            "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
                            "0x0000000000000000000000009ac6b9ffbb4269fc51cf0ef7bcd322cefb3e5e14",
                            "0x0000000000000000000000009792cc010fe26155c676d0cc0057a3c66564fbcd"
                        ],
                        "data": "0x0000000000000000000000000000000000000000000000000de0b6b3a7640000",
                        "blockNumber": "0x418f472",
                        "transactionHash": "0xbbded599a5f088cb82d9b439043ff691857ebff4f480225d5d563aed4ef11aaa",
                        "transactionIndex": "0x3",
                        "blockHash": "0xc2259f320a755bb1f21ab3cd3590f6838a48c8167268088dcf648acee2362b15",
                        "logIndex": "0x5",
                        "removed": false
                    }
                ],
                "transactionHash": "0xbbded599a5f088cb82d9b439043ff691857ebff4f480225d5d563aed4ef11aaa",
                "contractAddress": null,
                "gasUsed": "0xcbdb",
                "blockHash": "0xc2259f320a755bb1f21ab3cd3590f6838a48c8167268088dcf648acee2362b15",
                "blockNumber": "0x418f472",
                "transactionIndex": "0x3",
                "effectiveGasPrice": "0x7270e00"
            }
        });
        let payload_len = serde_json::to_vec(&body).unwrap().len();
        let mock = MockHttpOutcall::new(vec![success_response(body)]);
        let client = EvmClient::new(vec!["https://rpc".to_string()], 5, mock.clone());

        let tx_hash =
            TxHash::from_hex("0xbbded599a5f088cb82d9b439043ff691857ebff4f480225d5d563aed4ef11aaa")
                .unwrap();
        let result =
            futures::executor::block_on(client.get_transaction_receipt(1000, &tx_hash)).unwrap();
        let receipt = result.unwrap();
        assert_eq!(receipt.block_number(), Some(0x418f472));
        assert!(receipt.succeeded());

        // a real receipt, bloom filter and logs included, has to fit in the
        // response the outcall reserved for it
        assert!(payload_len < LARGE_RESPONSE as usize);
        assert_eq!(mock.max_response_bytes(), vec![Some(LARGE_RESPONSE)]);
    }

    #[test]
    fn minimal_receipt_preserves_revert_status() {
        let receipt: EvmReceipt = serde_json::from_value(serde_json::json!({
            "transactionHash": "0xbbded599a5f088cb82d9b439043ff691857ebff4f480225d5d563aed4ef11aaa",
            "blockNumber": "0x2a",
            "status": "0x0",
            "logs": [{"data": "ignored"}],
            "logsBloom": "0x00"
        }))
        .unwrap();

        assert_eq!(receipt.block_number(), Some(42));
        assert!(!receipt.succeeded());
    }

    #[test]
    fn receipt_accepts_numeric_quantities() {
        // Some providers serialize receipt quantities as bare JSON numbers
        // rather than hex strings; both forms must decode, since a decode
        // failure after a 2xx does not fail over to another provider.
        let receipt: EvmReceipt = serde_json::from_value(serde_json::json!({
            "transactionHash": "0xbbded599a5f088cb82d9b439043ff691857ebff4f480225d5d563aed4ef11aaa",
            "blockNumber": 68_680_818,
            "status": 1
        }))
        .unwrap();

        assert_eq!(receipt.block_number(), Some(68_680_818));
        assert!(receipt.succeeded());

        let receipt: EvmReceipt = serde_json::from_value(serde_json::json!({
            "transactionHash": "0xbbded599a5f088cb82d9b439043ff691857ebff4f480225d5d563aed4ef11aaa",
            "status": "0x1"
        }))
        .unwrap();

        assert_eq!(receipt.block_number(), None);
        assert!(receipt.succeeded());
    }
}
