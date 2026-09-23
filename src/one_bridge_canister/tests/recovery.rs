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
    icp_collected_fees: u128,
    finalize_bridging_round: (u64, bool),
}
#[derive(Debug, CandidType, Deserialize)]
struct RuntimeInfo {
    keys_ready: (bool, bool),
    ledger_verified: bool,
    pending_count: u64,
    migration_remaining: u64,
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
    metadata_calls: u64,
    fee_calls: u64,
}
#[derive(Debug, CandidType, Deserialize)]
enum Phase {
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
    compact_headers: bool,
    block_transactions: usize,
    header_calls: usize,
    full_block_calls: usize,
    hold_sol_status: bool,
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
            compact_headers: true,
            block_transactions: 0,
            header_calls: 0,
            full_block_calls: 0,
            hold_sol_status: false,
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
            "eth_getHeaderByNumber" | "eth_getBlockByNumber" => {
                let compact = request["method"] == "eth_getHeaderByNumber";
                if compact {
                    self.header_calls += 1;
                    assert_eq!(p.as_array().unwrap().len(), 1);
                    if !self.compact_headers {
                        return json!({"jsonrpc":"2.0","id":1,"error":{
                            "code":-32601,"message":"method not found"
                        }});
                    }
                } else {
                    self.full_block_calls += 1;
                    assert_eq!(p[1], false);
                }
                let number = if p[0] == "finalized" {
                    110
                } else {
                    u64::from_str_radix(p[0].as_str().unwrap().trim_start_matches("0x"), 16)
                        .unwrap()
                };
                let mut value = json!({"number":format!("0x{number:x}"),"hash":if self.reorg {B256::from([43;32])} else {block}});
                if !compact {
                    value["transactions"] = json!(
                        (0..self.block_transactions)
                            .map(|i| format!("0x{i:064x}"))
                            .collect::<Vec<_>>()
                    );
                }
                value
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
            let rpc_request: Value = serde_json::from_slice(&request.body).unwrap();
            if self.hold_sol_status && rpc_request["method"] == "getSignatureStatuses" {
                continue;
            }
            let body = serde_json::to_vec(&self.answer(&request.url, &request.body)).unwrap();
            if body.len() as u64 > request.max_response_bytes.unwrap() {
                pic.mock_canister_http_response(MockCanisterHttpResponse {
                    subnet_id: request.subnet_id,
                    request_id: request.request_id,
                    response: CanisterHttpResponse::CanisterHttpReject(
                        pocket_ic::common::rest::CanisterHttpReject {
                            reject_code: 2,
                            message: "HTTP response exceeded max_response_bytes".into(),
                        },
                    ),
                    additional_responses: vec![],
                });
                continue;
            }
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
        let state = info(pic, bridge);
        if state.keys_ready == (true, true)
            && state.ledger_verified
            && state.migration_remaining == 0
        {
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

fn test_network() -> PocketIc {
    PocketIcBuilder::new()
        .with_application_subnet()
        .with_test_threshold_keys_subnet()
        .with_nns_subnet()
        .with_initial_time(pocket_ic::Time::from_nanos_since_unix_epoch(
            1_800_000_000_000_000_000,
        ))
        .build()
}

fn register_evm_chain(
    pic: &PocketIc,
    rpc: &mut Rpc,
    bridge: Principal,
    admin: Principal,
    chain: &str,
    chain_id: u64,
) {
    let providers = vec![
        format!("https://{}-a.example", chain.to_lowercase()),
        format!("https://{}-b.example", chain.to_lowercase()),
    ];
    for (method, args) in [
        (
            "admin_set_evm_providers",
            encode_args((chain, 3u64, providers)).unwrap(),
        ),
        (
            "admin_add_evm_contract",
            encode_args((chain, chain_id, Address::from([17; 20]).to_string())).unwrap(),
        ),
    ] {
        decode_one::<Result<(), String>>(&update(pic, rpc, bridge, admin, method, args).unwrap())
            .unwrap()
            .unwrap();
    }
}

fn setup_bridge(hooks: bool) -> (PocketIc, Rpc, Principal, Principal, Principal, Principal) {
    let pic = test_network();
    let user = Principal::self_authenticating([7; 32]);
    let admin = Principal::self_authenticating([9; 32]);
    let ledger = deploy(&pic, wasm("ledger.wasm"), encode_args(()).unwrap());
    let bridge = deploy(
        &pic,
        wasm(if hooks {
            "bridge-hooks.wasm"
        } else {
            "bridge.wasm"
        }),
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
    register_evm_chain(&pic, &mut rpc, bridge, admin, "ETH", 1);
    (pic, rpc, bridge, ledger, user, admin)
}

fn set_ledger_mode(pic: &PocketIc, rpc: &mut Rpc, ledger: Principal, mode: u8) {
    update(
        pic,
        rpc,
        ledger,
        Principal::anonymous(),
        "set_mode",
        encode_one(mode).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "build fixture and test Wasm with make integration-test"]
fn initialization_retries_temporary_outages_and_stops_on_configuration_mismatch() {
    let (pic, mut rpc, bridge, ledger, user, _) = setup_bridge(false);
    set_ledger_mode(&pic, &mut rpc, ledger, 7);
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
    pump(&pic, &mut rpc, 30);
    assert_eq!(info(&pic, bridge).pending_count, 1);
    assert_eq!(stats(&pic, ledger).outgoing, 0);
    set_ledger_mode(&pic, &mut rpc, ledger, 3);
    pic.upgrade_canister(
        bridge,
        wasm("bridge.wasm"),
        encode_one(None::<u8>).unwrap(),
        None,
    )
    .unwrap();
    pump(&pic, &mut rpc, 10);
    assert!(!info(&pic, bridge).ledger_verified);
    set_ledger_mode(&pic, &mut rpc, ledger, 0);
    // No restart, init or resume ingress: the initialization timer recovers
    // readiness and the outstanding ledger payment by itself.
    ready(&pic, &mut rpc, bridge);
    pump(&pic, &mut rpc, 60);
    assert_eq!(stats(&pic, ledger).outgoing, 1);
    assert_eq!(info(&pic, bridge).pending_count, 0);

    set_ledger_mode(&pic, &mut rpc, ledger, 4);
    pic.upgrade_canister(
        bridge,
        wasm("bridge.wasm"),
        encode_one(None::<u8>).unwrap(),
        None,
    )
    .unwrap();
    pump(&pic, &mut rpc, 20);
    assert!(!info(&pic, bridge).ledger_verified);
    let calls = stats(&pic, ledger).metadata_calls;
    pump(&pic, &mut rpc, 60);
    assert_eq!(stats(&pic, ledger).metadata_calls, calls);
    assert_eq!(stats(&pic, ledger).outgoing, 1);
}

#[test]
#[ignore = "build fixture and test Wasm with make integration-test"]
fn slow_solana_status_does_not_delay_an_independent_icp_payout() {
    let (pic, mut rpc, bridge, ledger, user, admin) = setup_bridge(false);
    for (method, args) in [
        (
            "admin_set_svm_providers",
            encode_one(vec!["https://sol-a.example", "https://sol-b.example"]).unwrap(),
        ),
        (
            "admin_add_svm_contract",
            encode_one(solana_pubkey::Pubkey::new_from_array([44; 32]).to_string()).unwrap(),
        ),
    ] {
        decode_one::<Result<(), String>>(
            &update(&pic, &mut rpc, bridge, admin, method, args).unwrap(),
        )
        .unwrap()
        .unwrap();
    }
    bridge_result(
        update(
            &pic,
            &mut rpc,
            bridge,
            user,
            "bridge",
            encode_args((
                "ICP",
                "SOL",
                100u128,
                Some(solana_pubkey::Pubkey::new_from_array([33; 32]).to_string()),
            ))
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    // Finish the initial broadcast at fixed time, before the next poll is due.
    for _ in 0..60 {
        rpc.respond(&pic);
        pic.tick();
    }
    assert_eq!(rpc.sol_messages.len(), 1);
    assert!(!info(&pic, bridge).finalize_bridging_round.1);
    rpc.hold_sol_status = true;
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
    pump(&pic, &mut rpc, 30);
    assert!(pic.get_canister_http().iter().any(|request| {
        serde_json::from_slice::<Value>(&request.body).unwrap()["method"] == "getSignatureStatuses"
    }));
    assert_eq!(stats(&pic, ledger).outgoing, 1);
    assert_eq!(info(&pic, bridge).pending_count, 1);
    rpc.hold_sol_status = false;
    pump(&pic, &mut rpc, 40);
    assert_eq!(info(&pic, bridge).pending_count, 0);
}

#[test]
#[ignore = "build fixture and test Wasm with make integration-test"]
fn reconciled_external_deposit_is_paid_without_owner_resume() {
    let (pic, mut rpc, bridge, ledger, user, admin) = setup_bridge(true);
    update(
        &pic,
        &mut rpc,
        bridge,
        Principal::anonymous(),
        "test_trap_after_signature",
        encode_one(true).unwrap(),
    )
    .unwrap();
    assert!(
        update(
            &pic,
            &mut rpc,
            bridge,
            user,
            "bridge",
            encode_args(("ETH", "ICP", 100u128, None::<String>)).unwrap()
        )
        .is_err()
    );
    assert_eq!(info(&pic, bridge).pending_count, 0);
    let entries = query::<Result<Vec<Operation>, String>>(
        &pic,
        bridge,
        user,
        "my_operations",
        encode_args((10u32, None::<u64>)).unwrap(),
    )
    .unwrap();
    let entry = entries
        .iter()
        .find(|entry| matches!(entry.phase, Phase::Submitted))
        .unwrap();
    #[derive(CandidType)]
    enum Resolution {
        Completed(Tx),
    }
    // Governance supplies external evidence for the uncertain signature intent.
    decode_one::<Result<(), String>>(
        &update(
            &pic,
            &mut rpc,
            bridge,
            admin,
            "admin_resolve_operation",
            encode_args((
                entry.id,
                entry.revision,
                Resolution::Completed(Tx::Evm(true, vec![95; 32].into())),
                "fixture: controller verified the finalized external transfer",
            ))
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
    .unwrap();
    pump(&pic, &mut rpc, 60);
    assert_eq!(stats(&pic, ledger).outgoing, 1);
    assert_eq!(stats(&pic, ledger).last_amount, 99);
    assert_eq!(info(&pic, bridge).pending_count, 0);
    assert_eq!(info(&pic, bridge).total_bridge_count, 1);
    pic.upgrade_canister(
        bridge,
        wasm("bridge.wasm"),
        encode_one(None::<u8>).unwrap(),
        None,
    )
    .unwrap();
    ready(&pic, &mut rpc, bridge);
    pump(&pic, &mut rpc, 30);
    assert_eq!(stats(&pic, ledger).outgoing, 1);
}

#[test]
#[ignore = "build fixture and test Wasm with make integration-test"]
fn ledger_rejections_and_temporary_failures_do_not_duplicate_deposits() {
    let (pic, mut rpc, bridge, ledger, _, _) = setup_bridge(false);
    for mode in 5..=8 {
        let user = Principal::self_authenticating([mode; 32]);
        let before = stats(&pic, ledger).incoming;
        set_ledger_mode(&pic, &mut rpc, ledger, mode);
        assert!(
            bridge_result(
                update(
                    &pic,
                    &mut rpc,
                    bridge,
                    user,
                    "bridge",
                    encode_args(("ICP", "ETH", 100u128, None::<String>)).unwrap()
                )
                .unwrap()
            )
            .is_err()
        );
        assert_eq!(stats(&pic, ledger).incoming, before);
        let entries = query::<Result<Vec<Operation>, String>>(
            &pic,
            bridge,
            user,
            "my_operations",
            encode_args((10u32, None::<u64>)).unwrap(),
        )
        .unwrap();
        let entry = &entries[0];
        match mode {
            5 | 6 => assert!(matches!(entry.phase, Phase::Rejected(_))),
            7 => assert!(matches!(entry.phase, Phase::Prepared)),
            _ => assert!(matches!(entry.phase, Phase::NeedsReview(_))),
        }
        set_ledger_mode(&pic, &mut rpc, ledger, 0);
        pump(&pic, &mut rpc, 60);
        assert_eq!(stats(&pic, ledger).incoming, before + u64::from(mode == 7));
        if mode == 7 {
            bridge_result(
                update(
                    &pic,
                    &mut rpc,
                    bridge,
                    user,
                    "resume_deposit",
                    encode_one(entry.id).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(stats(&pic, ledger).incoming, before + 1);
        }
    }
    // Successful EVM payouts do not query ledger fees.
    assert_eq!(stats(&pic, ledger).fee_calls, 0);
}

#[test]
#[ignore = "build fixture and test Wasm with make integration-test"]
fn icp_payouts_need_no_fee_funding_before_or_after_upgrade() {
    for legacy in [false, true] {
        let pic = test_network();
        let user = Principal::self_authenticating([7; 32]);
        let admin = Principal::self_authenticating([9; 32]);
        let ledger = deploy(&pic, wasm("ledger.wasm"), encode_args(()).unwrap());
        let bridge = if legacy {
            let id = deploy(
                &pic,
                wasm("legacy.wasm"),
                encode_args((ledger, Some(3u8))).unwrap(),
            );
            pic.upgrade_canister(
                id,
                wasm("bridge.wasm"),
                encode_one(None::<u8>).unwrap(),
                None,
            )
            .unwrap();
            id
        } else {
            deploy(
                &pic,
                wasm("bridge.wasm"),
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
            )
        };
        let mut rpc = Rpc::default();
        ready(&pic, &mut rpc, bridge);
        if !legacy {
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
            register_evm_chain(&pic, &mut rpc, bridge, admin, "BNB", 56);
            bridge_result(
                update(
                    &pic,
                    &mut rpc,
                    bridge,
                    user,
                    "bridge",
                    encode_args(("BNB", "ICP", 100u128, None::<String>)).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        }
        pump(&pic, &mut rpc, 60);
        // The ledger pays its fee directly. No ICP deposit or sponsorship was
        // needed, even when the bridge fee is smaller than the ledger fee.
        assert_eq!(stats(&pic, ledger).incoming, 0);
        assert_eq!(stats(&pic, ledger).outgoing, 1);
        assert_eq!(
            stats(&pic, ledger).last_amount,
            if legacy { 100 } else { 99 }
        );
        assert_eq!(stats(&pic, ledger).last_to, Some(user));
        assert_eq!(info(&pic, bridge).icp_collected_fees, 0);
        assert_eq!(info(&pic, bridge).pending_count, 0);
        pic.upgrade_canister(
            bridge,
            wasm("bridge.wasm"),
            encode_one(None::<u8>).unwrap(),
            None,
        )
        .unwrap();
        ready(&pic, &mut rpc, bridge);
        pump(&pic, &mut rpc, 30);
        assert_eq!(stats(&pic, ledger).outgoing, 1);
        assert_eq!(info(&pic, bridge).pending_count, 0);
    }
}

#[test]
#[ignore = "build fixture and test Wasm with make integration-test"]
fn busy_evm_blocks_settle_deposits_and_payouts_without_duplicate_payments() {
    // The compact path must also work when a full block exceeds the IC limit;
    // the compatibility path covers providers lacking the compact method.
    for (compact_headers, block_transactions) in [(true, 40_000), (false, 807)] {
        let pic = test_network();
        let user = Principal::self_authenticating([7; 32]);
        let admin = Principal::self_authenticating([9; 32]);
        let ledger = deploy(&pic, wasm("ledger.wasm"), encode_args(()).unwrap());
        let bridge = deploy(
            &pic,
            wasm("bridge.wasm"),
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
        let mut rpc = Rpc {
            compact_headers,
            block_transactions,
            reorg: true,
            ..Default::default()
        };
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
        register_evm_chain(&pic, &mut rpc, bridge, admin, "BNB", 56);
        let deposit_args = encode_args((
            "ICP",
            "BNB",
            100u128,
            Some(Address::from([119; 20]).to_string()),
            ByteBuf::from(b"busy-block".to_vec()),
        ))
        .unwrap();
        let source = bridge_result(
            update(
                &pic,
                &mut rpc,
                bridge,
                user,
                "bridge_with_id",
                deposit_args.clone(),
            )
            .unwrap(),
        )
        .unwrap();
        pump(&pic, &mut rpc, 80);
        assert_eq!(info(&pic, bridge).pending_count, 1);
        assert_eq!(info(&pic, bridge).total_bridge_count, 0);
        // A normal unconfirmed task may still be rechecked by its owner. Drain
        // the current round at fixed time, then restore a canonical header.
        for _ in 0..30 {
            pic.tick();
            rpc.respond(&pic);
        }
        rpc.reorg = false;
        decode_one::<Result<(), String>>(
            &update(
                &pic,
                &mut rpc,
                bridge,
                user,
                "recheck_task",
                encode_one(source.clone()).unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
        .unwrap();
        pump(&pic, &mut rpc, 80);
        assert_eq!(info(&pic, bridge).pending_count, 0);
        assert_eq!(info(&pic, bridge).total_bridge_count, 1);
        assert_eq!(rpc.txs.len(), 1);
        assert_eq!(
            bridge_result(
                update(&pic, &mut rpc, bridge, user, "bridge_with_id", deposit_args).unwrap()
            )
            .unwrap(),
            source
        );
        assert_eq!(stats(&pic, ledger).incoming, 1);
        bridge_result(
            update(
                &pic,
                &mut rpc,
                bridge,
                user,
                "bridge",
                encode_args(("BNB", "ICP", 100u128, None::<String>)).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        pump(&pic, &mut rpc, 80);
        assert_eq!(stats(&pic, ledger).outgoing, 1);
        assert_eq!(stats(&pic, ledger).last_amount, 99);
        assert_eq!(info(&pic, bridge).pending_count, 0);
        assert_eq!(info(&pic, bridge).total_bridge_count, 2);
        assert_eq!(rpc.txs.len(), 2);
        assert!(rpc.header_calls > 0);
        assert_eq!(rpc.full_block_calls > 0, !compact_headers);
    }
}

#[test]
#[ignore = "build fixture and test Wasm with make integration-test"]
fn migrated_duplicates_cannot_be_rechecked_or_retried_even_after_another_upgrade() {
    for (scenario, archive_count) in [(1u8, 1u64), (2, 150)] {
        let pic = test_network();
        let user = Principal::self_authenticating([7; 32]);
        let ledger = deploy(&pic, wasm("ledger.wasm"), encode_args(()).unwrap());
        let bridge = deploy(
            &pic,
            wasm("legacy.wasm"),
            encode_args((ledger, Some(scenario))).unwrap(),
        );
        let mut rpc = Rpc::default();
        for upgrade in 0..2 {
            pic.upgrade_canister(
                bridge,
                wasm("bridge.wasm"),
                encode_one(None::<u8>).unwrap(),
                None,
            )
            .unwrap();
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
            if upgrade == 0 {
                register_evm_chain(&pic, &mut rpc, bridge, Principal::anonymous(), "ETH", 1);
            }
            let logs: Result<Vec<Log>, String> = query(
                &pic,
                bridge,
                user,
                "my_pending_logs",
                encode_args(()).unwrap(),
            );
            let logs = logs.unwrap();
            assert_eq!(logs.len(), 1);
            assert!(logs[0].stuck);
            let source = logs[0].from_tx.clone();
            for (caller, method) in [
                (user, "recheck_task"),
                (Principal::anonymous(), "admin_recheck_task"),
            ] {
                let result: Result<(), String> = decode_one(
                    &update(
                        &pic,
                        &mut rpc,
                        bridge,
                        caller,
                        method,
                        encode_one(source.clone()).unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert!(result.unwrap_err().contains("archive"));
            }
            #[derive(CandidType)]
            enum Target {
                Icp,
            }
            let result: Result<Log, String> = decode_one(
                &update(
                    &pic,
                    &mut rpc,
                    bridge,
                    Principal::anonymous(),
                    "admin_retry_bridging_task",
                    encode_args((source, Some(Target::Icp), None::<String>)).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(result.unwrap_err().contains("archive"));
            pump(&pic, &mut rpc, 50);
            assert!(rpc.txs.is_empty());
            assert_eq!(stats(&pic, ledger).outgoing, 0);
            assert_eq!(info(&pic, bridge).total_bridge_count, archive_count);
        }
        // Explicit external settlement still has a safe administrative closure.
        let logs: Result<Vec<Log>, String> = query(
            &pic,
            bridge,
            user,
            "my_pending_logs",
            encode_args(()).unwrap(),
        );
        let source = logs.unwrap().remove(0).from_tx;
        decode_one::<Result<Log, String>>(
            &update(
                &pic,
                &mut rpc,
                bridge,
                Principal::anonymous(),
                "admin_close_bridging_task",
                encode_args((source, Some(true))).unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(info(&pic, bridge).pending_count, 0);
        assert!(rpc.txs.is_empty());
    }
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

    // A request ID introduced while adopting an unresolved legacy bridge call
    // must remain attached after the recovery succeeds. Otherwise a lost reply
    // to the adoption call would make the next retry debit the ledger again.
    let adopter = Principal::self_authenticating([8; 32]);
    let before = stats(&pic, ledger).incoming;
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
                adopter,
                "bridge",
                encode_args(("ICP", "ETH", 100u128, Some(destination.clone()))).unwrap(),
            )
            .unwrap()
        )
        .is_err()
    );
    assert_eq!(stats(&pic, ledger).incoming, before + 1);
    let adopted_args = encode_args((
        "ICP",
        "ETH",
        100u128,
        Some(destination.clone()),
        ByteBuf::from(b"adopt-unkeyed".to_vec()),
    ))
    .unwrap();
    let adopted = bridge_result(
        update(
            &pic,
            &mut rpc,
            bridge,
            adopter,
            "bridge_with_id",
            adopted_args.clone(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        adopted,
        bridge_result(
            update(
                &pic,
                &mut rpc,
                bridge,
                adopter,
                "bridge_with_id",
                adopted_args,
            )
            .unwrap()
        )
        .unwrap()
    );
    assert_eq!(stats(&pic, ledger).incoming, before + 1);
    pump(&pic, &mut rpc, 50);
    assert_eq!(info(&pic, bridge).total_bridge_count, 8);

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
