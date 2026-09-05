import type { Principal } from '@icp-sdk/core/principal';
import type { ActorMethod } from '@icp-sdk/core/agent';
import type { IDL } from '@icp-sdk/core/candid';

export interface BridgeLog {
  'id' : [] | [bigint],
  'to' : BridgeTarget,
  'fee' : bigint,
  'to_tx' : [] | [BridgeTx],
  'to_addr' : [] | [string],
  'from' : BridgeTarget,
  'user' : Principal,
  'from_tx' : BridgeTx,
  'created_at' : bigint,
  'error' : [] | [string],
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
export type Result_3 = { 'Ok' : bigint } |
  { 'Err' : string };
export type Result_4 = { 'Ok' : string } |
  { 'Err' : string };
export type Result_5 = { 'Ok' : Uint8Array | number[] } |
  { 'Err' : string };
export type Result_6 = { 'Ok' : Array<BridgeLog> } |
  { 'Err' : string };
export type Result_7 = { 'Ok' : StateInfo } |
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
  'sub_bridges' : Array<Principal>,
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
   * the totals, and settling with the user is up to the administrator.
   */
  'admin_close_bridging_task' : ActorMethod<[BridgeTx], Result_1>,
  'admin_collect_fees' : ActorMethod<[Principal, bigint], Result_2>,
  'admin_remove_bridges' : ActorMethod<[Array<Principal>], Result>,
  /**
   * Resets the error circuit breaker and re-arms the finalization timer chain.
   * 
   * Finalization stops scheduling itself once `error_rounds` reaches its limit,
   * which disables bridging until someone intervenes. Use this once the cause of
   * the failures has been dealt with.
   */
  'admin_restart_bridging' : ActorMethod<[], Result_3>,
  /**
   * Drops the outgoing transaction of a stuck bridging task so the next
   * finalization round builds and broadcasts a fresh one.
   * 
   * Only use this after verifying on chain that the recorded outgoing
   * transaction moved no funds (an EVM transaction that reverted, or a Solana
   * transaction whose blockhash expired without landing). Retrying a payout that
   * did go through pays the recipient twice.
   */
  'admin_retry_bridging_task' : ActorMethod<[BridgeTx], Result_1>,
  'admin_set_evm_providers' : ActorMethod<
    [string, bigint, Array<string>],
    Result
  >,
  'admin_set_svm_providers' : ActorMethod<[Array<string>], Result>,
  'bridge' : ActorMethod<[string, string, bigint, [] | [string]], Result_2>,
  'erc20_transfer' : ActorMethod<[string, string, bigint], Result_4>,
  'erc20_transfer_tx' : ActorMethod<[string, string, bigint], Result_4>,
  'evm_address' : ActorMethod<[[] | [Principal]], Result_4>,
  'evm_sign' : ActorMethod<[Uint8Array | number[]], Result_5>,
  'evm_transfer_tx' : ActorMethod<[string, string, bigint], Result_4>,
  'finalized_logs' : ActorMethod<[number, [] | [bigint]], Result_6>,
  'info' : ActorMethod<[], Result_7>,
  'my_bridge_log' : ActorMethod<[BridgeTx], Result_1>,
  'my_finalized_logs' : ActorMethod<[number, [] | [bigint]], Result_6>,
  'my_pending_logs' : ActorMethod<[], Result_6>,
  'pending_logs' : ActorMethod<[], Result_6>,
  'sol_transfer_tx' : ActorMethod<[string, bigint], Result_4>,
  'spl_transfer_tx' : ActorMethod<[string, bigint], Result_4>,
  'svm_address' : ActorMethod<[[] | [Principal]], Result_4>,
  'validate_admin_add_bridges' : ActorMethod<[Array<Principal>], Result_4>,
  'validate_admin_add_evm_contract' : ActorMethod<
    [string, bigint, string],
    Result_4
  >,
  'validate_admin_add_svm_contract' : ActorMethod<[string], Result_4>,
  'validate_admin_close_bridging_task' : ActorMethod<[BridgeTx], Result_4>,
  'validate_admin_collect_fees' : ActorMethod<[Principal, bigint], Result_4>,
  'validate_admin_remove_bridges' : ActorMethod<[Array<Principal>], Result_4>,
  'validate_admin_restart_bridging' : ActorMethod<[], Result_4>,
  'validate_admin_retry_bridging_task' : ActorMethod<[BridgeTx], Result_4>,
  'validate_admin_set_evm_providers' : ActorMethod<
    [string, bigint, Array<string>],
    Result_4
  >,
  'validate_admin_set_svm_providers' : ActorMethod<[Array<string>], Result_4>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
