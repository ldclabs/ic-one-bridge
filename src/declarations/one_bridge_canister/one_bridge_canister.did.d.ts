import type { Principal } from '@icp-sdk/core/principal';
import type { ActorMethod } from '@icp-sdk/core/agent';
import type { IDL } from '@icp-sdk/core/candid';

export interface BridgeLog {
  'id' : [] | [bigint],
  'to' : BridgeTarget,
  'fee' : bigint,
  /**
   * When the payout was first attempted, in ms, or 0. The ledger dedup key
   * of an ICP payout is built from it, so every attempt shares it.
   */
  'payout_started_at' : bigint,
  'to_tx' : [] | [BridgeTx],
  'from_meta' : [] | [TxMeta],
  'to_addr' : [] | [string],
  'to_meta' : [] | [TxMeta],
  'from' : BridgeTarget,
  'user' : Principal,
  'from_tx' : BridgeTx,
  'created_at' : bigint,
  'error' : [] | [string],
  /**
   * The error is the task's own and will not clear by itself: an
   * administrator has to retry or close the task. It blocks nothing else
   * meanwhile.
   */
  'stuck' : boolean,
  'icp_amount' : bigint,
  'finalized_at' : bigint,
}
export type BridgeTarget = { 'Evm' : string } |
  { 'Icp' : null } |
  { 'Sol' : null };
export type BridgeTx = { 'Evm' : [boolean, Uint8Array | number[]] } |
  { 'Icp' : [boolean, bigint] } |
  { 'Sol' : [boolean, Uint8Array | number[]] };
export type CanisterArgs = { 'Upgrade' : UpgradeArgs } |
  { 'Init' : InitArgs };
export interface InitArgs {
  'min_threshold_to_bridge' : bigint,
  'token_symbol' : string,
  'governance_canister' : [] | [Principal],
  /**
   * Gas limit of the token's ERC-20 `transfer`; omitted, a limit that fits
   * a plain OpenZeppelin token is used.
   */
  'erc20_gas_limit' : [] | [bigint],
  'token_bridge_fee' : bigint,
  'key_name' : string,
  'token_decimals' : number,
  'token_ledger' : Principal,
  'token_logo' : string,
  'token_name' : string,
}
export type Result = { 'Ok' : null } |
  { 'Err' : string };
export type Result_1 = { 'Ok' : BridgeLog } |
  { 'Err' : string };
export type Result_2 = { 'Ok' : BridgeTx } |
  { 'Err' : string };
export type Result_3 = { 'Ok' : [string, string] } |
  { 'Err' : string };
export type Result_4 = { 'Ok' : bigint } |
  { 'Err' : string };
export type Result_5 = { 'Ok' : string } |
  { 'Err' : string };
export type Result_6 = { 'Ok' : Uint8Array | number[] } |
  { 'Err' : string };
export type Result_7 = { 'Ok' : Array<BridgeLog> } |
  { 'Err' : string };
export type Result_8 = { 'Ok' : StateInfo } |
  { 'Err' : string };
export interface StateInfo {
  'total_withdrawn_fees' : bigint,
  'error_rounds' : bigint,
  'evm_address' : string,
  'evm_latest_gas' : Array<[string, [bigint, bigint, bigint]]>,
  'svm_address' : string,
  'finalize_bridging_round' : [bigint, boolean],
  'total_collected_fees' : bigint,
  'min_threshold_to_bridge' : bigint,
  'token_symbol' : string,
  'governance_canister' : [] | [Principal],
  'icp_address' : Principal,
  'total_bridge_count' : bigint,
  'evm_token_contracts' : Array<[string, [string, number, bigint]]>,
  'erc20_gas_limit' : bigint,
  'svm_providers' : Array<string>,
  'svm_token_address' : [string, number, string],
  'token_bridge_fee' : bigint,
  'key_name' : string,
  'total_bridged_tokens' : bigint,
  'evm_providers' : Array<[string, [bigint, Array<string>]]>,
  'token_decimals' : number,
  'token_ledger' : Principal,
  'token_logo' : string,
  'token_name' : string,
  'icp_collected_fees' : bigint,
  'sub_bridges' : Array<Principal>,
}
/**
 * The point past which a transaction can never be included any more.
 */
export type TxDeadline = {
    /**
     * EVM: the nonce it spends. Once the sender's nonce has moved past it
     * without it being mined, another transaction took its place.
     */
    'Nonce' : bigint
  } |
  {
    /**
     * Solana: the last block height its blockhash is valid at.
     */
    'BlockHeight' : bigint
  };
/**
 * What a round needs to know about a transaction besides its hash: how to
 * tell that it is dead, and how to broadcast it again while it is not.
 */
export interface TxMeta {
  /**
   * The signed transaction, kept while it is unconfirmed.
   */
  'raw' : [] | [Uint8Array | number[]],
  'deadline' : TxDeadline,
}
export interface UpgradeArgs {
  'min_threshold_to_bridge' : [] | [bigint],
  'token_symbol' : [] | [string],
  'governance_canister' : [] | [Principal],
  'erc20_gas_limit' : [] | [bigint],
  'token_bridge_fee' : [] | [bigint],
  'token_ledger' : [] | [Principal],
  'token_logo' : [] | [string],
  'token_name' : [] | [string],
}
export interface _SERVICE {
  'admin_add_bridges' : ActorMethod<[Array<Principal>], Result>,
  'admin_add_evm_contract' : ActorMethod<[string, bigint, string], Result>,
  'admin_add_svm_contract' : ActorMethod<[string], Result>,
  /**
   * Removes a stuck bridging task from the pending queue and archives it with its
   * error preserved, unblocking the chains it references.
   * 
   * The task is recorded as not bridged: the amount and the fee are left out of
   * the totals, and settling with the user is up to the administrator — prefer
   * `admin_retry_bridging_task` with a refund target. A task whose payout is
   * broadcast but not confirmed is refused unless `force` is set.
   */
  'admin_close_bridging_task' : ActorMethod<
    [BridgeTx, [] | [boolean]],
    Result_1
  >,
  /**
   * Withdraws collected fees. Only the fees that sit on the ICP ledger — those
   * of tasks deposited on ICP — can be taken from it; a task deposited on
   * another chain left its fee there.
   */
  'admin_collect_fees' : ActorMethod<[Principal, bigint], Result_2>,
  /**
   * Fetches whichever of the subnet master keys the canister is still
   * missing, and returns the bridge's EVM and Solana addresses. Bridging that
   * needs a missing key is refused until it is there.
   */
  'admin_init_public_keys' : ActorMethod<[], Result_3>,
  'admin_remove_bridges' : ActorMethod<[Array<Principal>], Result>,
  /**
   * Resets the error circuit breaker and re-arms the finalization timer chain.
   * 
   * Once `error_rounds` reaches its limit, new tasks are refused and the rounds
   * slow to an hourly cooldown that lifts the pause by itself after a clean
   * round. Use this to lift it right away once the cause has been dealt with.
   */
  'admin_restart_bridging' : ActorMethod<[], Result_4>,
  /**
   * Drops the outgoing transaction and the error of a stuck bridging task so
   * the next finalization round pays it out afresh, optionally somewhere else.
   * 
   * `to` and `to_addr` replace the task's target: a corrected address, the
   * user's own address on the same chain (`to_addr = null`), or the chain the
   * deposit came from — a refund. They are vetted like a `bridge()` call's.
   * 
   * Only use this after verifying on chain that the recorded outgoing
   * transaction moved no funds (an EVM transaction that reverted, or a Solana
   * transaction whose blockhash expired without landing). Retrying a payout that
   * did go through pays the recipient twice.
   */
  'admin_retry_bridging_task' : ActorMethod<
    [BridgeTx, [] | [BridgeTarget], [] | [string]],
    Result_1
  >,
  /**
   * Sets the RPC providers of an EVM chain and when a transaction on it counts
   * as final: after `max_confirmations` blocks, or, when it is `0`, once the
   * chain's own `finalized` block tag has passed it. The tag is the safer
   * choice on every chain that supports it.
   */
  'admin_set_evm_providers' : ActorMethod<
    [string, bigint, Array<string>],
    Result
  >,
  'admin_set_svm_providers' : ActorMethod<[Array<string>], Result>,
  'bridge' : ActorMethod<[string, string, bigint, [] | [string]], Result_2>,
  'erc20_transfer' : ActorMethod<[string, string, bigint], Result_5>,
  'erc20_transfer_tx' : ActorMethod<[string, string, bigint], Result_5>,
  'evm_address' : ActorMethod<[[] | [Principal]], Result_5>,
  'evm_sign' : ActorMethod<[Uint8Array | number[]], Result_6>,
  'evm_transfer_tx' : ActorMethod<[string, string, bigint], Result_5>,
  'finalized_logs' : ActorMethod<[number, [] | [bigint]], Result_7>,
  'info' : ActorMethod<[], Result_8>,
  'my_bridge_log' : ActorMethod<[BridgeTx], Result_1>,
  'my_finalized_logs' : ActorMethod<[number, [] | [bigint]], Result_7>,
  'my_pending_logs' : ActorMethod<[], Result_7>,
  /**
   * The oldest pending tasks, at most `PENDING_LOGS_LIMIT` of them.
   */
  'pending_logs' : ActorMethod<[], Result_7>,
  'sol_transfer_tx' : ActorMethod<[string, bigint], Result_5>,
  'spl_transfer_tx' : ActorMethod<[string, bigint], Result_5>,
  'svm_address' : ActorMethod<[[] | [Principal]], Result_5>,
  'validate_admin_add_bridges' : ActorMethod<[Array<Principal>], Result_5>,
  'validate_admin_add_evm_contract' : ActorMethod<
    [string, bigint, string],
    Result_5
  >,
  'validate_admin_add_svm_contract' : ActorMethod<[string], Result_5>,
  'validate_admin_close_bridging_task' : ActorMethod<
    [BridgeTx, [] | [boolean]],
    Result_5
  >,
  'validate_admin_collect_fees' : ActorMethod<[Principal, bigint], Result_5>,
  'validate_admin_init_public_keys' : ActorMethod<[], Result_5>,
  'validate_admin_remove_bridges' : ActorMethod<[Array<Principal>], Result_5>,
  'validate_admin_restart_bridging' : ActorMethod<[], Result_5>,
  'validate_admin_retry_bridging_task' : ActorMethod<
    [BridgeTx, [] | [BridgeTarget], [] | [string]],
    Result_5
  >,
  'validate_admin_set_evm_providers' : ActorMethod<
    [string, bigint, Array<string>],
    Result_5
  >,
  'validate_admin_set_svm_providers' : ActorMethod<[Array<string>], Result_5>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
