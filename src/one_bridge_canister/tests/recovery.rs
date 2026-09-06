//! Run with `make integration-test`. All ledgers and HTTPS responses are local
//! fixtures; no real chain transactions or external RPC calls are made.
use alloy_consensus::{Transaction as _, TxEnvelope};
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::{Address, B256, U256, hex, keccak256};
use candid::{CandidType, Deserialize, Principal, decode_one, encode_args, encode_one};
use pocket_ic::{
    PocketIc, PocketIcBuilder,
    common::rest::{CanisterHttpReply, CanisterHttpResponse, MockCanisterHttpResponse},
};
use serde_bytes::ByteBuf;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

#[derive(Clone, Debug, CandidType, Deserialize, PartialEq, Eq)]
enum Tx {
    Icp(bool, u64),
    Evm(bool, ByteBuf),
    Sol(bool, ByteBuf),
}
#[derive(CandidType)]
enum Args {
    Init(Init),
}
#[derive(CandidType)]
struct Init {
    key_name: String,
    token_name: String,
    token_symbol: String,
    token_decimals: u8,
    token_logo: String,
    token_ledger: Principal,
    token_bridge_fee: u128,
    min_threshold_to_bridge: u128,
    governance_canister: Option<Principal>,
    erc20_gas_limit: Option<u64>,
}
#[derive(Debug, CandidType, Deserialize)]
struct Info {
    runtime: Option<RuntimeInfo>,
    evm_address: String,
    total_bridge_count: u64,
    total_bridged_tokens: u128,
    finalize_bridging_round: (u64, bool),
}
#[derive(Debug, CandidType, Deserialize)]
struct RuntimeInfo {
    keys_ready: (bool, bool),
    ledger_verified: bool,
    pending_count: u64,
    icp_transfer_fees: u128,
}
impl std::ops::Deref for Info {
    type Target = RuntimeInfo;
    fn deref(&self) -> &RuntimeInfo {
        self.runtime.as_ref().expect("new canister runtime")
    }
}

#[derive(Debug, CandidType, Deserialize)]
struct Log {
    runtime: Option<LogRuntime>,
    fee: u128,
    from_tx: Tx,
    to_tx: Option<Tx>,
    stuck: bool,
}
#[derive(Debug, CandidType, Deserialize)]
struct LogRuntime {
    task_id: u64,
}
impl std::ops::Deref for Log {
    type Target = LogRuntime;
    fn deref(&self) -> &LogRuntime {
        self.runtime.as_ref().expect("new log runtime")
    }
}

#[derive(Debug, CandidType, Deserialize)]
struct Stats {
    incoming: u64,
    outgoing: u64,
    last_amount: u128,
    last_to: Option<Principal>,
}
#[derive(Debug, CandidType, Deserialize)]
enum Phase {
    Recorded,
    Planning,
    Prepared,
    Submitted,
    Signed,
    Completed(Tx),
    Rejected(String),
    NeedsReview(String),
}
#[derive(Debug, CandidType, Deserialize)]
struct Operation {
    id: u64,
    revision: u64,
    phase: Phase,
}

fn wasm(name: &str) -> Vec<u8> {
    let root = std::env::var_os("BRIDGE_TEST_WASM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-wasm")
        });
    std::fs::read(root.join(name))
        .unwrap_or_else(|e| panic!("build test Wasm with make integration-test: {name}: {e}"))
}
fn query<T: for<'de> Deserialize<'de> + CandidType>(
    pic: &PocketIc,
    id: Principal,
    owner: Principal,
    method: &str,
    args: Vec<u8>,
) -> T {
    decode_one(&pic.query_call(id, owner, method, args).unwrap()).unwrap()
}
fn info(pic: &PocketIc, id: Principal) -> Info {
    query::<Result<Info, String>>(
        pic,
        id,
        Principal::anonymous(),
        "info",
        encode_args(()).unwrap(),
    )
    .unwrap()
}
fn stats(pic: &PocketIc, id: Principal) -> Stats {
    query(
        pic,
        id,
        Principal::anonymous(),
        "stats",
        encode_args(()).unwrap(),
    )
}

#[derive(Clone)]
struct RemoteTx {
    chain: u64,
    from: Address,
    to: Address,
    amount: U256,
    nonce: u64,
}
struct Rpc {
    bridge: Address,
    user: Address,
    txs: BTreeMap<String, RemoteTx>,
    nonces: BTreeMap<(u64, Address), u64>,
    reorg: bool,
    calls: usize,
    sol_messages: BTreeMap<String, Vec<u8>>,
}
impl Default for Rpc {
    fn default() -> Self {
        Self {
            bridge: Address::ZERO,
            user: Address::ZERO,
            txs: BTreeMap::new(),
            nonces: BTreeMap::new(),
            reorg: false,
            calls: 0,
            sol_messages: BTreeMap::new(),
        }
    }
}
impl Rpc {
    fn answer(&mut self, url: &str, body: &[u8]) -> Value {
        self.calls += 1;
        let request: Value = serde_json::from_slice(body).unwrap();
        let p = &request["params"];
        let chain = if url.contains("bnb") { 56 } else { 1 };
        let block = B256::from([42; 32]);
        if url.contains("sol-") {
            use base64::{Engine, engine::general_purpose::STANDARD};
            let result = match request["method"].as_str().unwrap() {
                "getAccountInfo" => {
                    json!({"context":{"slot":1000},"value":{"owner":"TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA","data":{"parsed":{"type":"mint","info":{"decimals":8,"isInitialized":true}}}}})
                }
                "getLatestBlockhash" => {
                    json!({"context":{"slot":1000},"value":{"blockhash":solana_hash::Hash::new_from_array([55;32]).to_string(),"lastValidBlockHeight":2000}})
                }
                "isBlockhashValid" => json!({"context":{"slot":1000},"value":true}),
                "sendTransaction" => {
                    let raw = STANDARD.decode(p[0].as_str().unwrap()).unwrap();
                    let tx: solana_transaction::Transaction = bincode::deserialize(&raw).unwrap();
                    let message = bincode::serialize(&tx.message).unwrap();
                    ic_ed25519::PublicKey::deserialize_raw(tx.message.account_keys[0].as_ref())
                        .unwrap()
                        .verify_signature(&message, tx.signatures[0].as_ref())
                        .unwrap();
                    let memo = tx.message.instructions.last().unwrap();
                    assert_eq!(
                        tx.message.account_keys[memo.program_id_index as usize].to_string(),
                        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"
                    );
                    assert!(String::from_utf8_lossy(&memo.data).starts_with("1bridge:"));
                    let signature = tx.signatures[0].to_string();
                    if !self.sol_messages.contains_key(&signature) {
                        assert!(!self.sol_messages.values().any(|old| old == &message));
                    }
                    self.sol_messages.insert(signature.clone(), message);
                    json!(signature)
                }
                "getSignatureStatuses" => {
                    json!({"context":{"slot":1100},"value":p[0].as_array().unwrap().iter().map(|sig| if self.sol_messages.contains_key(sig.as_str().unwrap()) {json!({"slot":1000,"confirmations":null,"confirmationStatus":"finalized","err":null})} else {Value::Null}).collect::<Vec<_>>()})
                }
                method => panic!("unexpected Solana RPC method {method}"),
            };
            return json!({"jsonrpc":"2.0","id":1,"result":result});
        }
        let result = match request["method"].as_str().unwrap() {
            "eth_chainId" => json!(format!("0x{chain:x}")),
            "eth_gasPrice" => json!("0x3b9aca00"),
            "eth_maxPriorityFeePerGas" => json!("0x5f5e100"),
            "eth_blockNumber" => json!("0x6e"),
            "eth_getBlockByNumber" => {
                let number = if p[0] == "finalized" {
                    110
                } else {
                    u64::from_str_radix(p[0].as_str().unwrap().trim_start_matches("0x"), 16)
                        .unwrap()
                };
                json!({"number":format!("0x{number:x}"),"hash":if self.reorg {B256::from([43;32])} else {block}})
            }
            "eth_getTransactionCount" => {
                let address = p[0].as_str().unwrap().parse::<Address>().unwrap();
                json!(format!(
                    "0x{:x}",
                    self.nonces.get(&(chain, address)).copied().unwrap_or(0)
                ))
            }
            "eth_getBalance" => json!("0xffffffffffffffffffffffffffffffff"),
            "eth_call" => {
                let data = p[0]["data"].as_str().unwrap();
                json!(format!(
                    "0x{:064x}",
                    if data.starts_with("0x313ce567") {
                        U256::from(8)
                    } else {
                        U256::from(1_000_000_000_000_000_000u64)
                    }
                ))
            }
            "eth_sendRawTransaction" => {
                let bytes = hex::decode(p[0].as_str().unwrap()).unwrap();
                let hash = keccak256(&bytes).to_string();
                if !self.txs.contains_key(&hash) {
                    let tx = TxEnvelope::decode_2718(&mut bytes.as_slice()).unwrap();
                    let data = tx.input();
                    assert_eq!(&data[..4], &[0xa9, 0x05, 0x9c, 0xbb]);
                    let to = Address::from_slice(&data[16..36]);
                    let from = if to == self.bridge {
                        self.user
                    } else {
                        self.bridge
                    };
                    assert_eq!(
                        tx.nonce(),
                        self.nonces.get(&(chain, from)).copied().unwrap_or(0)
                    );
                    self.nonces.insert((chain, from), tx.nonce() + 1);
                    self.txs.insert(
                        hash.clone(),
                        RemoteTx {
                            chain,
                            from,
                            to,
                            amount: U256::from_be_slice(&data[36..68]),
                            nonce: tx.nonce(),
                        },
                    );
                }
                json!(hash)
            }
            "eth_getTransactionReceipt" => {
                let hash = p[0].as_str().unwrap();
                if let Some(tx) = self.txs.get(hash).filter(|tx| tx.chain == chain) {
                    let _ = tx.nonce;
                    json!({"transactionHash":hash,"blockHash":block,"blockNumber":"0x64","status":"0x1","logs":[{
                        "address":Address::from([17;20]),"topics":[
                            "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
                            format!("0x{:0>64}",hex::encode(tx.from)),format!("0x{:0>64}",hex::encode(tx.to))],
                        "data":format!("0x{:064x}",tx.amount)}]})
                } else {
                    Value::Null
                }
            }
            method => panic!("unexpected external method {method}"),
        };
        json!({"jsonrpc":"2.0","id":1,"result":result})
    }
    fn respond(&mut self, pic: &PocketIc) {
        for request in pic.get_canister_http() {
            assert!(request.max_response_bytes.is_some());
            let body = serde_json::to_vec(&self.answer(&request.url, &request.body)).unwrap();
            pic.mock_canister_http_response(MockCanisterHttpResponse {
                subnet_id: request.subnet_id,
                request_id: request.request_id,
                response: CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
                    status: 200,
                    headers: vec![],
                    body,
                }),
                additional_responses: vec![],
            });
        }
    }
}
fn update(
    pic: &PocketIc,
    rpc: &mut Rpc,
    id: Principal,
    owner: Principal,
    method: &str,
    args: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let call = pic
        .submit_call(id, owner, method, args)
        .map_err(|e| format!("{e:?}"))?;
    for _ in 0..400 {
        rpc.respond(pic);
        pic.tick();
        if let Some(result) = pic.ingress_status(call.clone()) {
            return result.map_err(|e| format!("{e:?}"));
        }
    }
    panic!("{method} did not resolve");
}
fn bridge_result(bytes: Vec<u8>) -> Result<Tx, String> {
    decode_one(&bytes).unwrap()
}
fn pump(pic: &PocketIc, rpc: &mut Rpc, rounds: usize) {
    for _ in 0..rounds {
        pic.advance_time(Duration::from_secs(1));
        pic.tick();
        rpc.respond(pic);
    }
}
fn ready(pic: &PocketIc, rpc: &mut Rpc, bridge: Principal) {
    for _ in 0..100 {
        if info(pic, bridge).keys_ready == (true, true) && info(pic, bridge).ledger_verified {
            return;
        }
        pump(pic, rpc, 1);
    }
    panic!("initialization failed: {:?}", info(pic, bridge));
}
fn deploy(pic: &PocketIc, module: Vec<u8>, args: Vec<u8>) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 100_000_000_000_000);
    pic.install_canister(id, module, args, None);
    id
}

#[test]
#[ignore = "build fixture and test Wasm with make integration-test"]
fn payment_recovery_traps_watchdog_upgrade_and_certification() {
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .with_test_threshold_keys_subnet()
        .with_nns_subnet()
        .with_initial_time(pocket_ic::Time::from_nanos_since_unix_epoch(
            1_800_000_000_000_000_000,
        ))
        .build();
    let user = Principal::self_authenticating([7; 32]);
    let admin = Principal::self_authenticating([9; 32]);
    let ledger = deploy(&pic, wasm("ledger.wasm"), encode_args(()).unwrap());
    let bridge = deploy(
        &pic,
        wasm("bridge-hooks.wasm"),
        encode_one(Some(Args::Init(Init {
            key_name: "test_key_1".into(),
            token_name: "Fixture".into(),
            token_symbol: "TEST".into(),
            token_decimals: 8,
            token_logo: "".into(),
            token_ledger: ledger,
            token_bridge_fee: 1,
            min_threshold_to_bridge: 2,
            governance_canister: Some(admin),
            erc20_gas_limit: None,
        })))
        .unwrap(),
    );
    let mut rpc = Rpc::default();
    ready(&pic, &mut rpc, bridge);
    rpc.bridge = info(&pic, bridge).evm_address.parse().unwrap();
    rpc.user = query::<Result<String, String>>(
        &pic,
        bridge,
        user,
        "evm_address",
        encode_one(None::<Principal>).unwrap(),
    )
    .unwrap()
    .parse()
    .unwrap();
    let destination = Address::from([119; 20]).to_string();
    for (chain, id) in [("ETH", 1u64), ("BNB", 56)] {
        let providers = vec![
            format!("https://{}-a.example", chain.to_lowercase()),
            format!("https://{}-b.example", chain.to_lowercase()),
        ];
        let result: Result<(), String> = decode_one(
            &update(
                &pic,
                &mut rpc,
                bridge,
                admin,
                "admin_set_evm_providers",
                encode_args((chain, 0u64, providers)).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        result.unwrap();
        let result: Result<(), String> = decode_one(
            &update(
                &pic,
                &mut rpc,
                bridge,
                admin,
                "admin_add_evm_contract",
                encode_args((chain, id, Address::from([17; 20]).to_string())).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        result.unwrap();
    }
    let deposit_args = |key: u8| {
        encode_args((
            "ICP",
            "ETH",
            100u128,
            Some(destination.clone()),
            ByteBuf::from(vec![key]),
        ))
        .unwrap()
    };
    update(
        &pic,
        &mut rpc,
        ledger,
        Principal::anonymous(),
        "set_mode",
        encode_one(1u8).unwrap(),
    )
    .unwrap();
    assert!(
        bridge_result(
            update(
                &pic,
                &mut rpc,
                bridge,
                user,
                "bridge_with_id",
                deposit_args(1)
            )
            .unwrap()
        )
        .is_err()
    );
    assert_eq!(stats(&pic, ledger).incoming, 1);
    let operations: Result<Vec<Operation>, String> = query(
        &pic,
        bridge,
        user,
        "my_operations",
        encode_args((10u32, None::<u64>)).unwrap(),
    );
    assert!(
        operations
            .unwrap()
            .iter()
            .any(|o| matches!(o.phase, Phase::Submitted))
    );
    pic.upgrade_canister(
        bridge,
        wasm("bridge-hooks.wasm"),
        encode_one(None::<u8>).unwrap(),
        None,
    )
    .unwrap();
    ready(&pic, &mut rpc, bridge);
    let first = bridge_result(
        update(
            &pic,
            &mut rpc,
            bridge,
            user,
            "bridge_with_id",
            deposit_args(1),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        first,
        bridge_result(
            update(
                &pic,
                &mut rpc,
                bridge,
                user,
                "bridge_with_id",
                deposit_args(1)
            )
            .unwrap()
        )
        .unwrap()
    );
    assert_eq!(stats(&pic, ledger).incoming, 1);
    pump(&pic, &mut rpc, 50);
    assert_eq!(info(&pic, bridge).pending_count, 0);

    update(
        &pic,
        &mut rpc,
        bridge,
        Principal::anonymous(),
        "test_trap_after_ledger",
        encode_one(true).unwrap(),
    )
    .unwrap();
    assert!(
        update(
            &pic,
            &mut rpc,
            bridge,
            user,
            "bridge_with_id",
            deposit_args(2)
        )
        .is_err()
    );
    assert_eq!(stats(&pic, ledger).incoming, 2);
    update(
        &pic,
        &mut rpc,
        bridge,
        Principal::anonymous(),
        "test_trap_after_ledger",
        encode_one(false).unwrap(),
    )
    .unwrap();
    bridge_result(
        update(
            &pic,
            &mut rpc,
            bridge,
            user,
            "bridge_with_id",
            deposit_args(2),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stats(&pic, ledger).incoming, 2);
    pump(&pic, &mut rpc, 50);

    // A stalled old ledger callback survives the watchdog. The newer retry gets
    // Duplicate; a healthy task also progresses, and the late old reply is inert.
    bridge_result(
        update(
            &pic,
            &mut rpc,
            bridge,
            admin,
            "fund_ledger_fees",
            encode_one(100u128).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    // Trap in a timer callback as well as in the original ingress callback.
    update(
        &pic,
        &mut rpc,
        bridge,
        Principal::anonymous(),
        "test_trap_after_ledger",
        encode_one(true).unwrap(),
    )
    .unwrap();
    bridge_result(
        update(
            &pic,
            &mut rpc,
            bridge,
            user,
            "bridge",
            encode_args(("ETH", "ICP", 100u128, None::<String>)).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    for _ in 0..60 {
        pump(&pic, &mut rpc, 1);
        if stats(&pic, ledger).outgoing == 1 && !info(&pic, bridge).finalize_bridging_round.1 {
            break;
        }
    }
    assert_eq!(stats(&pic, ledger).outgoing, 1);
    assert_eq!(info(&pic, bridge).pending_count, 1);
    update(
        &pic,
        &mut rpc,
        bridge,
        Principal::anonymous(),
        "test_trap_after_ledger",
        encode_one(false).unwrap(),
    )
    .unwrap();
    pump(&pic, &mut rpc, 40);
    assert_eq!(stats(&pic, ledger).outgoing, 1);
    assert_eq!(info(&pic, bridge).pending_count, 0);

    update(
        &pic,
        &mut rpc,
        bridge,
        Principal::anonymous(),
        "test_unbounded_ledger",
        encode_one(true).unwrap(),
    )
    .unwrap();
    update(
        &pic,
        &mut rpc,
        ledger,
        Principal::anonymous(),
        "set_mode",
        encode_one(2u8).unwrap(),
    )
    .unwrap();
    bridge_result(
        update(
            &pic,
            &mut rpc,
            bridge,
            user,
            "bridge",
            encode_args(("ETH", "ICP", 100u128, None::<String>)).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    for _ in 0..60 {
        pump(&pic, &mut rpc, 1);
        if stats(&pic, ledger).outgoing == 2 {
            break;
        }
    }
    assert_eq!(stats(&pic, ledger).outgoing, 2);
    assert!(info(&pic, bridge).finalize_bridging_round.1);
    bridge_result(
        update(
            &pic,
            &mut rpc,
            bridge,
            user,
            "bridge",
            encode_args(("ICP", "BNB", 100u128, Some(destination.clone()))).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    pic.advance_time(Duration::from_secs(11 * 60));
    pump(&pic, &mut rpc, 60);
    assert_eq!(stats(&pic, ledger).outgoing, 2);
    assert_eq!(stats(&pic, ledger).last_amount, 99);
    assert_eq!(stats(&pic, ledger).last_to, Some(user));
    assert_eq!(info(&pic, bridge).pending_count, 0);
    assert_eq!(info(&pic, bridge).total_bridge_count, 5);
    update(
        &pic,
        &mut rpc,
        ledger,
        Principal::anonymous(),
        "set_mode",
        encode_one(0u8).unwrap(),
    )
    .unwrap();
    pump(&pic, &mut rpc, 20);
    assert_eq!(info(&pic, bridge).total_bridge_count, 5);
    assert_eq!(info(&pic, bridge).total_bridged_tokens, 500);
    assert_eq!(info(&pic, bridge).icp_transfer_fees, 20);

    // Distinct equal-sized Solana payouts sharing one recent blockhash must
    // remain distinct messages. Verify the real management-canister signature.
    let public = vec!["https://sol-a.example", "https://sol-b.example"];
    let configured: Result<(), String> = decode_one(
        &update(
            &pic,
            &mut rpc,
            bridge,
            admin,
            "admin_set_svm_providers",
            encode_one(public).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    configured.unwrap();
    let mint = solana_pubkey::Pubkey::new_from_array([44; 32]).to_string();
    let configured: Result<(), String> = decode_one(
        &update(
            &pic,
            &mut rpc,
            bridge,
            admin,
            "admin_add_svm_contract",
            encode_one(mint).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    configured.unwrap();
    let recipient = solana_pubkey::Pubkey::new_from_array([33; 32]).to_string();
    for _ in 0..2 {
        bridge_result(
            update(
                &pic,
                &mut rpc,
                bridge,
                user,
                "bridge",
                encode_args(("ICP", "SOL", 100u128, Some(recipient.clone()))).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    pump(&pic, &mut rpc, 60);
    assert_eq!(rpc.sol_messages.len(), 2);
    assert_eq!(info(&pic, bridge).pending_count, 0);
    assert_eq!(info(&pic, bridge).total_bridge_count, 7);

    #[derive(CandidType)]
    struct Limits {
        requests_per_hour: u32,
        requests_per_user_hour: u32,
        max_active_requests: u32,
        max_pending: u32,
        max_pending_per_user: u32,
        min_cycles_reserve: u128,
    }
    let limits = Limits {
        requests_per_hour: 120,
        requests_per_user_hour: 1,
        max_active_requests: 16,
        max_pending: 512,
        max_pending_per_user: 32,
        min_cycles_reserve: 2_000_000_000_000,
    };
    let result: Result<(), String> = decode_one(
        &update(
            &pic,
            &mut rpc,
            bridge,
            admin,
            "admin_set_resource_limits",
            encode_one(limits).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    result.unwrap();
    let calls = rpc.calls;
    assert!(
        update(
            &pic,
            &mut rpc,
            bridge,
            user,
            "erc20_transfer_tx",
            encode_args(("ETH", destination.clone(), 100u128)).unwrap()
        )
        .is_err()
    );
    assert_eq!(rpc.calls, calls);

    update(
        &pic,
        &mut rpc,
        bridge,
        Principal::anonymous(),
        "test_unbounded_ledger",
        encode_one(false).unwrap(),
    )
    .unwrap();
    verify_configuration_certificate(&pic, bridge);
    // Production exports must not contain the controller fault-injection hook.
    assert!(
        !wasm("bridge.wasm")
            .windows(b"test_trap_after_ledger".len())
            .any(|w| w == b"test_trap_after_ledger")
    );

    let legacy = deploy(&pic, wasm("legacy.wasm"), encode_one(ledger).unwrap());
    pic.upgrade_canister(
        legacy,
        wasm("bridge.wasm"),
        encode_one(None::<u8>).unwrap(),
        None,
    )
    .unwrap();
    ready(&pic, &mut rpc, legacy);
    let logs: Result<Vec<Log>, String> = query(
        &pic,
        legacy,
        user,
        "my_pending_logs",
        encode_args(()).unwrap(),
    );
    let logs = logs.unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].fee, 0);
    assert_ne!(logs[0].task_id, 0);
    let closed: Result<Log, String> = decode_one(
        &update(
            &pic,
            &mut rpc,
            legacy,
            Principal::anonymous(),
            "admin_close_bridging_task",
            encode_args((logs[0].from_tx.clone(), Some(true))).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    closed.unwrap();
    pic.upgrade_canister(legacy, wasm("legacy.wasm"), encode_args(()).unwrap(), None)
        .unwrap();
    pic.upgrade_canister(
        legacy,
        wasm("bridge.wasm"),
        encode_one(None::<u8>).unwrap(),
        None,
    )
    .unwrap();
    ready(&pic, &mut rpc, legacy);
    let history: Result<Vec<Log>, String> = query(
        &pic,
        legacy,
        user,
        "my_finalized_logs",
        encode_args((100u32, None::<u64>)).unwrap(),
    );
    assert_eq!(history.unwrap().len(), 3);
}

fn verify_configuration_certificate(pic: &PocketIc, canister: Principal) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use ic_certificate_verification::VerifyCertificate;
    use ic_certification::{Certificate, HashTree, LookupResult};
    use ic_http_certification::{
        DefaultCelBuilder, DefaultResponseCertification, HttpCertification, HttpCertificationPath,
        HttpCertificationTree, HttpCertificationTreeEntry, HttpRequest, HttpResponse,
    };
    #[derive(CandidType, Deserialize)]
    struct Response {
        status_code: u16,
        headers: Vec<(String, String)>,
        body: ByteBuf,
    }
    let full = DefaultCelBuilder::full_certification()
        .with_request_headers(vec![])
        .with_request_query_parameters(vec![])
        .with_response_certification(DefaultResponseCertification::certified_response_headers(
            vec![
                "content-type",
                "content-length",
                "cache-control",
                "x-content-type-options",
            ],
        ))
        .build();
    let error = DefaultCelBuilder::response_only_certification()
        .with_response_certification(DefaultResponseCertification::certified_response_headers(
            vec!["content-type", "allow"],
        ))
        .build();
    let mut expected = HttpCertificationTree::default();
    expected.insert(&HttpCertificationTreeEntry::new(
        HttpCertificationPath::wildcard(""),
        HttpCertification::skip(),
    ));
    let mut certified_root = None;
    for path in ["/config", "/config.cbor"] {
        for method in ["GET", "HEAD", "POST"] {
            let request = HttpRequest::builder()
                .with_url(path)
                .with_method(method.parse().unwrap())
                .build();
            let reply: Response = query(
                pic,
                canister,
                Principal::anonymous(),
                "http_request",
                encode_one(&request).unwrap(),
            );
            let header = reply
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("ic-certificate"))
                .unwrap()
                .1
                .clone();
            let part = |name: &str| {
                STANDARD
                    .decode(
                        header
                            .split(&format!("{name}=:"))
                            .nth(1)
                            .unwrap()
                            .split(':')
                            .next()
                            .unwrap(),
                    )
                    .unwrap()
            };
            let cert: Certificate = ic_auth_types::cbor_from_slice(&part("certificate")).unwrap();
            cert.verify(
                canister.as_slice(),
                &pic.root_key().unwrap(),
                &u128::from(pic.get_time().as_nanos_since_unix_epoch()),
                &60_000_000_000,
            )
            .unwrap();
            let root = match cert.tree.lookup_path([
                b"canister".as_slice(),
                canister.as_slice(),
                b"certified_data".as_slice(),
            ]) {
                LookupResult::Found(value) => value.to_vec(),
                _ => panic!("missing certified data"),
            };
            let witness: HashTree = ic_auth_types::cbor_from_slice(&part("tree")).unwrap();
            assert_eq!(witness.digest().to_vec(), root);
            certified_root = Some(root);
            if method == "HEAD" {
                assert!(reply.body.is_empty());
            }
            let response = HttpResponse::builder()
                .with_status_code(reply.status_code.try_into().unwrap())
                .with_headers(reply.headers)
                .with_body(reply.body.into_vec())
                .build();
            let cert = if method == "POST" {
                assert_eq!(response.status_code().as_u16(), 405);
                HttpCertification::response_only(&error, &response, None).unwrap()
            } else {
                HttpCertification::full(&full, &request, &response, None).unwrap()
            };
            expected.insert(&HttpCertificationTreeEntry::new(
                HttpCertificationPath::exact(path),
                cert,
            ));
        }
    }
    assert_eq!(expected.root_hash().to_vec(), certified_root.unwrap());
}
