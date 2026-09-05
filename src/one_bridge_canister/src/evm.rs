use alloy_primitives::{B256, Bytes, U64, U256, b256, hex::FromHex};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::outcall::{
    Agreement, HttpOutcall, LARGE_RESPONSE, RpcCall, SMALL_RESPONSE, as_is, json_rpc_call, lower,
    same, two_provider_verdict,
};

pub use alloy_primitives::{Address, TxHash};

/// `keccak256("Transfer(address,address,uint256)")`, the topic of the ERC-20
/// transfer event.
pub const TRANSFER_EVENT: B256 =
    b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");

/// One event of a receipt, the fields a `Transfer` is recognised by.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvmLog {
    pub address: Address,
    #[serde(default)]
    pub topics: Vec<B256>,
    #[serde(default)]
    pub data: Bytes,
}

/// The receipt fields finalization needs: the transaction identity, its
/// inclusion height and status, and the events that prove what it moved.
///
/// The quantity fields are [`U64`], whose deserializer accepts both `"0x1"`
/// and a bare JSON number — providers disagree on which they send.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvmReceipt {
    pub transaction_hash: TxHash,
    #[serde(default)]
    block_number: Option<U64>,
    status: U64,
    #[serde(default)]
    logs: Vec<EvmLog>,
}

impl EvmReceipt {
    pub fn block_number(&self) -> Option<u64> {
        self.block_number.map(|value| value.to::<u64>())
    }

    pub fn succeeded(&self) -> bool {
        self.status == U64::from(1)
    }

    /// How much the `Transfer` events of `token` in this receipt moved from
    /// `from` to `to`. A successful status only says the call did not revert;
    /// this says what actually arrived.
    pub fn transferred(&self, token: &Address, from: &Address, to: &Address) -> U256 {
        self.logs
            .iter()
            .filter(|log| {
                log.address == *token
                    && log.topics.len() >= 3
                    && log.topics[0] == TRANSFER_EVENT
                    && topic_address(&log.topics[1]) == *from
                    && topic_address(&log.topics[2]) == *to
                    && log.data.len() == 32
            })
            .fold(U256::ZERO, |sum, log| {
                sum.saturating_add(U256::from_be_slice(&log.data))
            })
    }
}

fn topic_address(topic: &B256) -> Address {
    Address::from_slice(&topic[12..])
}

/// Two providers agree on a receipt only when they return the same one. One
/// that has it while the other does not yet, or two different ones during a
/// reorg, count as no receipt at all: the transaction is simply not confirmed
/// yet, and asking again later costs nothing.
pub fn same_or_absent(
    a: Option<EvmReceipt>,
    b: Option<EvmReceipt>,
) -> Result<Option<EvmReceipt>, String> {
    Ok(if a == b { a } else { None })
}

#[derive(Debug, Deserialize)]
struct BlockHeader {
    number: U64,
}

pub struct EvmClient<T: HttpOutcall> {
    pub providers: Vec<String>,
    /// How deep a transaction has to be before it counts as final, or `0` to
    /// rely on the chain's own `finalized` block tag instead.
    pub max_confirmations: u64,
    outcall: T,
}

// https://ethereum.org/en/developers/docs/apis/json-rpc/
impl<H: HttpOutcall> EvmClient<H> {
    pub fn new(providers: Vec<String>, max_confirmations: u64, outcall: H) -> Self {
        Self {
            providers,
            max_confirmations,
            outcall,
        }
    }

    pub async fn chain_id(&self) -> Result<u64, String> {
        self.call(
            "eth_chainId",
            &[],
            SMALL_RESPONSE,
            hex_to_u64,
            Agreement::Two(same),
        )
        .await
    }

    pub async fn gas_price(&self) -> Result<u128, String> {
        self.call(
            "eth_gasPrice",
            &[],
            SMALL_RESPONSE,
            hex_to_u128,
            Agreement::First,
        )
        .await
    }

    pub async fn max_priority_fee_per_gas(&self) -> Result<u128, String> {
        self.call(
            "eth_maxPriorityFeePerGas",
            &[],
            SMALL_RESPONSE,
            hex_to_u128,
            Agreement::First,
        )
        .await
    }

    /// The latest block height, the lower of two providers' views.
    pub async fn block_number(&self) -> Result<u64, String> {
        self.call(
            "eth_blockNumber",
            &[],
            SMALL_RESPONSE,
            hex_to_u64,
            Agreement::Two(lower),
        )
        .await
    }

    /// The height of the latest block the chain itself reports as finalized,
    /// the lower of two providers' views. Not every chain supports the tag.
    pub async fn finalized_block_number(&self) -> Result<u64, String> {
        self.call(
            "eth_getBlockByNumber",
            &["finalized".into(), false.into()],
            LARGE_RESPONSE,
            |header: Option<BlockHeader>| {
                header
                    .map(|header| header.number.to::<u64>())
                    .ok_or_else(|| "the chain has no finalized block".to_string())
            },
            Agreement::Two(lower),
        )
        .await
    }

    /// The nonce of the next transaction of `address`, as two providers agree
    /// on it.
    pub async fn get_transaction_count(&self, address: &Address) -> Result<u64, String> {
        self.call(
            "eth_getTransactionCount",
            &[address.to_string().into(), "latest".into()],
            SMALL_RESPONSE,
            hex_to_u64,
            Agreement::Two(same),
        )
        .await
    }

    /// The native balance of `address` in wei, the lower of two providers'
    /// views.
    pub async fn get_balance(&self, address: &Address) -> Result<U256, String> {
        self.call(
            "eth_getBalance",
            &[address.to_string().into(), "latest".into()],
            SMALL_RESPONSE,
            hex_to_u256,
            Agreement::Two(lower),
        )
        .await
    }

    /// The receipt of `tx_hash`, when two providers return the same one.
    pub async fn get_transaction_receipt(
        &self,
        tx_hash: &TxHash,
    ) -> Result<Option<EvmReceipt>, String> {
        self.call(
            "eth_getTransactionReceipt",
            &[tx_hash.to_string().into()],
            LARGE_RESPONSE,
            as_is,
            Agreement::Two(same_or_absent),
        )
        .await
    }

    /// Broadcasts a signed transaction.
    ///
    /// The result is deliberately decoded as an untyped [`Value`]: by the time it
    /// is parsed the transaction is already on its way to the mempool, and no
    /// caller reads it — they all know the hash from the transaction they signed.
    /// Insisting on a string here would turn a provider that answers with a null
    /// or an unexpected shape into a failed broadcast that did happen.
    pub async fn send_raw_transaction(&self, signed_tx: String) -> Result<Value, String> {
        self.call(
            "eth_sendRawTransaction",
            &[signed_tx.into()],
            SMALL_RESPONSE,
            as_is,
            Agreement::First,
        )
        .await
    }

    /// Whether a transaction `sender` signed with `nonce` can no longer be
    /// mined: the sender's nonce has moved past it, yet the transaction has
    /// no receipt.
    ///
    /// Both facts are read from the same provider, one provider at a time:
    /// a provider that has processed the block spending the nonce also has
    /// the receipt, if the transaction is what spent it, so a mixed view —
    /// the nonce from one provider and the receipt from another that lags
    /// behind — is exactly the view that would declare a mined deposit dead.
    /// Two providers have to reach the verdict, see [`two_provider_verdict`].
    pub async fn replaced(
        &self,
        sender: &Address,
        nonce: u64,
        tx_hash: &TxHash,
    ) -> Result<bool, String> {
        two_provider_verdict(&self.providers, "replacement check", |one| async move {
            let current: u64 = json_rpc_call(
                &self.outcall,
                one,
                RpcCall {
                    method: "eth_getTransactionCount",
                    params: &[sender.to_string().into(), "latest".into()],
                    max_response_bytes: SMALL_RESPONSE,
                },
                hex_to_u64,
                Agreement::First,
            )
            .await?;
            if current <= nonce {
                return Ok(false);
            }
            let receipt: Option<EvmReceipt> = json_rpc_call(
                &self.outcall,
                one,
                RpcCall {
                    method: "eth_getTransactionReceipt",
                    params: &[tx_hash.to_string().into()],
                    max_response_bytes: LARGE_RESPONSE,
                },
                as_is,
                Agreement::First,
            )
            .await?;
            Ok(receipt.is_none())
        })
        .await
    }

    async fn eth_call<T>(
        &self,
        contract: &Address,
        call_data: Vec<u8>,
        interpret: fn(Vec<u8>) -> Result<T, String>,
        agreement: Agreement<T>,
    ) -> Result<T, String> {
        let call_object = serde_json::json!({
            "to": contract.to_string(),
            "data": Bytes::from(call_data).to_string(),
        });
        self.call(
            "eth_call",
            &[call_object, "latest".into()],
            SMALL_RESPONSE,
            move |res: String| {
                let res = res.strip_prefix("0x").unwrap_or(&res);
                let bytes = <Vec<u8>>::from_hex(res).map_err(|err| err.to_string())?;
                interpret(bytes)
            },
            agreement,
        )
        .await
    }

    pub async fn erc20_decimals(&self, contract: &Address) -> Result<u8, String> {
        self.eth_call(
            contract,
            DECIMALS_SELECTOR.to_vec(),
            |bytes| {
                let value = decode_abi_uint(&bytes)?;
                u8::try_from(value).map_err(|_| "decimals overflow u8".to_string())
            },
            Agreement::Two(same),
        )
        .await
    }

    /// The token balance of `owner`, the lower of two providers' views.
    pub async fn erc20_balance_of(
        &self,
        contract: &Address,
        owner: &Address,
    ) -> Result<U256, String> {
        self.eth_call(
            contract,
            encode_erc20_balance_of(owner),
            |bytes| decode_abi_uint(&bytes),
            Agreement::Two(lower),
        )
        .await
    }

    async fn call<R: DeserializeOwned, T>(
        &self,
        method: &str,
        params: &[Value],
        max_response_bytes: u64,
        interpret: impl Fn(R) -> Result<T, String>,
        agreement: Agreement<T>,
    ) -> Result<T, String> {
        json_rpc_call(
            &self.outcall,
            &self.providers,
            RpcCall {
                method,
                params,
                max_response_bytes,
            },
            interpret,
            agreement,
        )
        .await
    }
}

const TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb]; // keccak256("transfer(address,uint256)")[:4]
const BALANCE_OF_SELECTOR: [u8; 4] = [0x70, 0xa0, 0x82, 0x31]; // keccak256("balanceOf(address)")[:4]
const DECIMALS_SELECTOR: [u8; 4] = [0x31, 0x3c, 0xe5, 0x67]; // keccak256("decimals()")[:4]

pub fn encode_erc20_transfer(to: &Address, value: u128) -> Vec<u8> {
    let mut call_data = Vec::with_capacity(4 + 32 + 32);
    call_data.extend_from_slice(&TRANSFER_SELECTOR);
    call_data.extend_from_slice(&padded_address(to));
    call_data.extend_from_slice(&U256::from(value).to_be_bytes::<32>());
    call_data
}

fn encode_erc20_balance_of(owner: &Address) -> Vec<u8> {
    let mut call_data = Vec::with_capacity(4 + 32);
    call_data.extend_from_slice(&BALANCE_OF_SELECTOR);
    call_data.extend_from_slice(&padded_address(owner));
    call_data
}

fn padded_address(address: &Address) -> [u8; 32] {
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(address.as_slice());
    padded
}

fn hex_to_u64(s: String) -> Result<u64, String> {
    let s = s.strip_prefix("0x").unwrap_or(&s);
    u64::from_str_radix(s, 16).map_err(|err| err.to_string())
}

fn hex_to_u128(s: String) -> Result<u128, String> {
    let s = s.strip_prefix("0x").unwrap_or(&s);
    u128::from_str_radix(s, 16).map_err(|err| err.to_string())
}

fn hex_to_u256(s: String) -> Result<U256, String> {
    let s = s.strip_prefix("0x").unwrap_or(&s);
    U256::from_str_radix(s, 16).map_err(|err| err.to_string())
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
    use crate::outcall::tests::{MockHttpOutcall, result, success_response};

    fn client(mock: &MockHttpOutcall, providers: usize) -> EvmClient<MockHttpOutcall> {
        EvmClient::new(
            (0..providers).map(|i| format!("https://rpc{i}")).collect(),
            5,
            mock.clone(),
        )
    }

    #[test]
    fn test_encode_erc20_transfer() {
        let addr = Address::from_hex("0x00112233445566778899aabbccddeeff00112233").unwrap();
        let encoded = encode_erc20_transfer(&addr, 12345);

        let mut expected = vec![0xa9, 0x05, 0x9c, 0xbb];
        expected.extend(vec![0u8; 12]);
        expected.extend_from_slice(addr.as_ref());
        expected.extend(U256::from(12345u128).to_be_bytes::<32>());

        assert_eq!(encoded, expected);

        let balance_of = encode_erc20_balance_of(&addr);
        assert_eq!(&balance_of[..4], &BALANCE_OF_SELECTOR);
        assert_eq!(&balance_of[16..], addr.as_slice());
    }

    #[test]
    fn test_hex_conversions() {
        assert_eq!(hex_to_u64("0x2a".into()).unwrap(), 42);
        assert_eq!(hex_to_u128("0xff".into()).unwrap(), 255);
        assert_eq!(hex_to_u256("0x10".into()).unwrap(), U256::from(16));
        assert!(hex_to_u64("g1".into()).is_err());
        assert!(hex_to_u128("xyz".into()).is_err());
    }

    #[test]
    fn test_decode_abi_uint() {
        let value = U256::from(999u64).to_be_bytes::<32>();
        assert_eq!(decode_abi_uint(&value).unwrap(), U256::from(999u64));
        assert!(decode_abi_uint(&value[..31]).is_err());
    }

    #[test]
    fn chain_id_needs_two_agreeing_providers() {
        let mock = MockHttpOutcall::new(vec![result("0x2a".into()), result("0x2a".into())]);
        let value = futures::executor::block_on(client(&mock, 2).chain_id()).unwrap();
        assert_eq!(value, 42);
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE); 2]);

        let mock = MockHttpOutcall::new(vec![result("0x2a".into()), result("0x38".into())]);
        assert!(futures::executor::block_on(client(&mock, 2).chain_id()).is_err());
    }

    #[test]
    fn heights_and_balances_take_the_lower_view() {
        let mock = MockHttpOutcall::new(vec![result("0xa".into()), result("0x9".into())]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).block_number()),
            Ok(9)
        );

        let mock = MockHttpOutcall::new(vec![result("0x64".into()), result("0x65".into())]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).get_balance(&Address::ZERO)),
            Ok(U256::from(100))
        );

        let mock = MockHttpOutcall::new(vec![
            result(serde_json::json!({"number": "0x10", "hash": "0x00"})),
            result(serde_json::json!({"number": "0xf"})),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).finalized_block_number()),
            Ok(15)
        );
        assert_eq!(mock.max_response_bytes(), vec![Some(LARGE_RESPONSE); 2]);
    }

    #[test]
    fn gas_and_broadcasts_take_the_first_answer() {
        let mock = MockHttpOutcall::new(vec![Err("down".into()), result("0x3b9aca00".into())]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).gas_price()),
            Ok(1_000_000_000)
        );
        assert_eq!(mock.urls().len(), 2);

        let mock = MockHttpOutcall::new(vec![result(Value::Null)]);
        assert!(
            futures::executor::block_on(client(&mock, 2).send_raw_transaction("0x00".into()))
                .is_ok()
        );
        assert_eq!(mock.urls().len(), 1);
    }

    #[test]
    fn test_call_handles_error_payload() {
        let error_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32000, "message": "execution reverted"}
        });
        let mock = MockHttpOutcall::new(vec![success_response(error_body)]);

        let result = futures::executor::block_on(client(&mock, 2).block_number());
        assert!(result.unwrap_err().contains("execution reverted"));
    }

    fn receipt_json(status: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "0x2",
            "from": "0x9ac6b9ffbb4269fc51cf0ef7bcd322cefb3e5e14",
            "to": "0xe74583edaff618d88463554b84bc675196b36990",
            "status": status,
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
        })
    }

    #[test]
    fn a_receipt_is_used_only_when_two_providers_return_the_same_one() {
        let tx_hash =
            TxHash::from_hex("0xbbded599a5f088cb82d9b439043ff691857ebff4f480225d5d563aed4ef11aaa")
                .unwrap();

        // one provider lags: not confirmed yet
        let mock = MockHttpOutcall::new(vec![result(receipt_json("0x1")), result(Value::Null)]);
        let receipt =
            futures::executor::block_on(client(&mock, 2).get_transaction_receipt(&tx_hash))
                .unwrap();
        assert!(receipt.is_none());

        let mock = MockHttpOutcall::new(vec![
            result(receipt_json("0x1")),
            result(receipt_json("0x1")),
        ]);
        let payload_len = serde_json::to_vec(&receipt_json("0x1")).unwrap().len();
        let receipt =
            futures::executor::block_on(client(&mock, 2).get_transaction_receipt(&tx_hash))
                .unwrap()
                .unwrap();
        assert_eq!(receipt.block_number(), Some(0x418f472));
        assert!(receipt.succeeded());

        // a real receipt, bloom filter and logs included, has to fit in the
        // response the outcall reserved for it
        assert!(payload_len < LARGE_RESPONSE as usize);
        assert_eq!(mock.max_response_bytes(), vec![Some(LARGE_RESPONSE); 2]);

        // the events say what the transfer delivered, and to whom
        let token = Address::from_hex("0xe74583edaff618d88463554b84bc675196b36990").unwrap();
        let from = Address::from_hex("0x9ac6b9ffbb4269fc51cf0ef7bcd322cefb3e5e14").unwrap();
        let to = Address::from_hex("0x9792cc010fe26155c676d0cc0057a3c66564fbcd").unwrap();
        assert_eq!(
            receipt.transferred(&token, &from, &to),
            U256::from(1_000_000_000_000_000_000u128)
        );
        assert_eq!(receipt.transferred(&token, &to, &from), U256::ZERO);
        assert_eq!(receipt.transferred(&Address::ZERO, &from, &to), U256::ZERO);
    }

    #[test]
    fn a_replacement_verdict_needs_two_providers_each_seeing_nonce_and_receipt() {
        let sender = Address::from([1; 20]);
        let tx_hash = TxHash::from([2; 32]);

        // both providers: nonce moved past 4, no receipt
        let mock = MockHttpOutcall::new(vec![
            result("0x5".into()),
            result(Value::Null),
            result("0x5".into()),
            result(Value::Null),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).replaced(&sender, 4, &tx_hash)),
            Ok(true)
        );
        assert_eq!(
            mock.urls(),
            vec![
                "https://rpc0",
                "https://rpc0",
                "https://rpc1",
                "https://rpc1"
            ]
        );
        assert_eq!(
            mock.methods(),
            vec![
                "eth_getTransactionCount",
                "eth_getTransactionReceipt",
                "eth_getTransactionCount",
                "eth_getTransactionReceipt"
            ]
        );

        // the second provider lags: its nonce has not moved, so no verdict
        let mock = MockHttpOutcall::new(vec![
            result("0x5".into()),
            result(Value::Null),
            result("0x4".into()),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).replaced(&sender, 4, &tx_hash)),
            Ok(false)
        );
        assert_eq!(mock.urls().len(), 3);

        // a provider that has the receipt: the nonce was spent by this very
        // transaction, whatever another provider says
        let mock = MockHttpOutcall::new(vec![
            result("0x5".into()),
            result(receipt_json("0x1")),
            result("0x5".into()),
            result(Value::Null),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).replaced(&sender, 4, &tx_hash)),
            Ok(false)
        );

        // one provider down: one verdict is not enough
        let mock = MockHttpOutcall::new(vec![
            Err("down".into()),
            result("0x5".into()),
            result(Value::Null),
        ]);
        assert!(
            futures::executor::block_on(client(&mock, 2).replaced(&sender, 4, &tx_hash)).is_err()
        );
    }

    #[test]
    fn minimal_receipt_preserves_revert_status() {
        let receipt: EvmReceipt = serde_json::from_value(serde_json::json!({
            "transactionHash": "0xbbded599a5f088cb82d9b439043ff691857ebff4f480225d5d563aed4ef11aaa",
            "blockNumber": "0x2a",
            "status": "0x0",
            "logs": [],
            "logsBloom": "0x00"
        }))
        .unwrap();

        assert_eq!(receipt.block_number(), Some(42));
        assert!(!receipt.succeeded());
    }

    #[test]
    fn receipt_accepts_numeric_quantities() {
        // Some providers serialize receipt quantities as bare JSON numbers
        // rather than hex strings; both forms must decode.
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
