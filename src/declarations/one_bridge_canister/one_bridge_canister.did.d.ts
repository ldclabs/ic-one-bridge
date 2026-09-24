import type { Principal } from '@icp-sdk/core/principal';
import type { ActorMethod } from '@icp-sdk/core/agent';
import type { IDL } from '@icp-sdk/core/candid';

export interface Account {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
export interface BridgeLog {
  'id' : [] | [bigint],
  'to' : BridgeTarget,
  'fee' : bigint,
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
  'stuck' : boolean,
  'icp_amount' : bigint,
  'runtime' : [] | [LogRuntime],
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
export interface DepositPlan {
  'to' : BridgeTarget,
  'fee' : bigint,
  'to_addr' : [] | [string],
  'from' : BridgeTarget,
  'user' : Principal,
  'ledger' : Principal,
  'amount' : bigint,
}
export interface EvmFeeLimits {
  'max_priority_fee_per_gas' : bigint,
  'max_hourly_fee' : bigint,
  'max_fee_per_gas' : bigint,
  'max_transaction_fee' : bigint,
}
export interface InitArgs {
  'resource_limits' : [] | [ResourceLimits],
  'min_threshold_to_bridge' : bigint,
  'token_symbol' : string,
  'governance_canister' : [] | [Principal],
  'erc20_gas_limit' : [] | [bigint],
  'token_bridge_fee' : bigint,
  'key_name' : string,
  'token_decimals' : number,
  'token_ledger' : Principal,
  'token_logo' : string,
  'token_name' : string,
}
export interface LogRuntime {
  'next_poll_at' : bigint,
  'task_id' : bigint,
  'poll_attempts' : number,
  'payout_attempt' : [] | [bigint],
  'payout_resolution' : [] | [PayoutResolution],
  'ledger' : [] | [Principal],
  'payout_mined' : boolean,
  'error_chain' : [] | [BridgeTarget],
}
export interface OperationInfo {
  'id' : bigint,
  'owner' : Principal,
  'request' : [] | [RequestInfo],
  'kind' : string,
  'signed_tx' : [] | [BridgeTx],
  'deposit' : [] | [DepositPlan],
  'created_at' : bigint,
  'error' : [] | [string],
  'related_task' : [] | [BridgeLog],
  'phase' : Phase,
  'revision' : bigint,
  'reconciliation' : [] | [Reconciliation],
}
export type PayoutResolution = { 'Failed' : null } |
  { 'Completed' : null } |
  { 'Expired' : null } |
  { 'Incomplete' : null };
export type Phase = { 'Prepared' : null } |
  { 'Rejected' : string } |
  { 'NeedsReview' : string } |
  { 'Submitted' : null } |
  { 'Planning' : null } |
  { 'Signed' : null } |
  { 'Completed' : BridgeTx };
export interface Reconciliation {
  'controller' : Principal,
  'resolution' : Resolution,
  'evidence' : string,
}
export type RequestInfo = { 'LegacyPayout' : BridgeLog } |
  { 'Transfer' : [Principal, TransferArg] } |
  {
    'Signature' : {
      'validity' : [] | [SolValidity],
      'scheme' : string,
      'deadline' : TxDeadline,
      'sender' : Principal,
      'message_hash' : Uint8Array | number[],
    }
  } |
  { 'TransferFrom' : [Principal, TransferFromArgs] };
export type Resolution = { 'NotExecuted' : null } |
  { 'Completed' : BridgeTx };
export interface ResourceLimits {
  'max_pending' : number,
  'requests_per_user_hour' : number,
  'max_pending_per_user' : number,
  'min_cycles_reserve' : bigint,
  'requests_per_hour' : number,
  'max_active_requests' : number,
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
export type Result_9 = { 'Ok' : Array<OperationInfo> } |
  { 'Err' : string };
export interface SolValidity {
  'blockhash' : string,
  'last_valid_block_height' : bigint,
  'context_slot' : bigint,
}
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
  'runtime' : [] | [StateRuntimeInfo],
  'icp_collected_fees' : bigint,
  'sub_bridges' : Array<Principal>,
}
export interface StateRuntimeInfo {
  'resource_limits' : ResourceLimits,
  'pending_count' : bigint,
  'ledger_verified' : boolean,
  'keys_ready' : [boolean, boolean],
  'migration_remaining' : bigint,
  'evm_provider_hosts' : Array<[string, Array<string>]>,
  'svm_provider_hosts' : Array<string>,
  'available_icp_fees' : bigint,
  'evm_fee_limits' : Array<[string, EvmFeeLimits]>,
  'unresolved_operations' : bigint,
  'svm_mint_verified' : boolean,
}
export interface TransferArg {
  'to' : Account,
  'fee' : [] | [bigint],
  'memo' : [] | [Uint8Array | number[]],
  'from_subaccount' : [] | [Uint8Array | number[]],
  'created_at_time' : [] | [bigint],
  'amount' : bigint,
}
export interface TransferFromArgs {
  'to' : Account,
  'fee' : [] | [bigint],
  'spender_subaccount' : [] | [Uint8Array | number[]],
  'from' : Account,
  'memo' : [] | [Uint8Array | number[]],
  'created_at_time' : [] | [bigint],
  'amount' : bigint,
}
export type TxDeadline = { 'Nonce' : bigint } |
  { 'BlockHeight' : bigint };
export interface TxMeta {
  'raw' : [] | [Uint8Array | number[]],
  'svm_validity' : [] | [SolValidity],
  'deadline' : TxDeadline,
}
export interface UpgradeArgs {
  'resource_limits' : [] | [ResourceLimits],
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
  'admin_close_bridging_task' : ActorMethod<
    [BridgeTx, [] | [boolean]],
    Result_1
  >,
  'admin_collect_fees' : ActorMethod<[Principal, bigint], Result_2>,
  'admin_init_public_keys' : ActorMethod<[], Result_3>,
  'admin_operations' : ActorMethod<
    [Principal, number, [] | [bigint]],
    Array<OperationInfo>
  >,
  'admin_recheck_task' : ActorMethod<[BridgeTx], Result>,
  'admin_remove_bridges' : ActorMethod<[Array<Principal>], Result>,
  'admin_resolve_legacy_payout' : ActorMethod<
    [BridgeTx, bigint, Resolution, string],
    Result
  >,
  'admin_resolve_operation' : ActorMethod<
    [bigint, bigint, Resolution, string],
    Result
  >,
  'admin_restart_bridging' : ActorMethod<[], Result_4>,
  'admin_retry_bridging_task' : ActorMethod<
    [BridgeTx, [] | [BridgeTarget], [] | [string]],
    Result_1
  >,
  'admin_set_evm_fee_limits' : ActorMethod<[string, EvmFeeLimits], Result>,
  'admin_set_evm_providers' : ActorMethod<
    [string, bigint, Array<string>],
    Result
  >,
  'admin_set_public_providers' : ActorMethod<[string, Array<string>], Result>,
  'admin_set_resource_limits' : ActorMethod<[ResourceLimits], Result>,
  'admin_set_svm_providers' : ActorMethod<[Array<string>], Result>,
  'bridge' : ActorMethod<[string, string, bigint, [] | [string]], Result_2>,
  'bridge_with_id' : ActorMethod<
    [string, string, bigint, [] | [string], Uint8Array | number[]],
    Result_2
  >,
  'cancel_operation' : ActorMethod<[bigint], Result>,
  'erc20_transfer' : ActorMethod<[string, string, bigint], Result_5>,
  'erc20_transfer_tx' : ActorMethod<[string, string, bigint], Result_5>,
  'evm_address' : ActorMethod<[[] | [Principal]], Result_5>,
  'evm_sign' : ActorMethod<[Uint8Array | number[]], Result_6>,
  'evm_transfer_tx' : ActorMethod<[string, string, bigint], Result_5>,
  'finalized_logs' : ActorMethod<[number, [] | [bigint]], Result_7>,
  'info' : ActorMethod<[], Result_8>,
  'my_bridge_log' : ActorMethod<[BridgeTx], Result_1>,
  'my_finalized_logs' : ActorMethod<[number, [] | [bigint]], Result_7>,
  'my_operations' : ActorMethod<[number, [] | [bigint]], Result_9>,
  'my_pending_logs' : ActorMethod<[], Result_7>,
  'my_pending_logs_page' : ActorMethod<[number, [] | [bigint]], Result_7>,
  'pending_logs' : ActorMethod<[], Result_7>,
  'pending_logs_page' : ActorMethod<[number, [] | [bigint]], Result_7>,
  'recheck_task' : ActorMethod<[BridgeTx], Result>,
  'resume_deposit' : ActorMethod<[bigint], Result_2>,
  'resume_operation' : ActorMethod<[bigint], Result_2>,
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
  'validate_admin_recheck_task' : ActorMethod<[BridgeTx], Result_5>,
  'validate_admin_remove_bridges' : ActorMethod<[Array<Principal>], Result_5>,
  'validate_admin_resolve_legacy_payout' : ActorMethod<
    [BridgeTx, bigint, Resolution, string],
    Result_5
  >,
  'validate_admin_resolve_operation' : ActorMethod<
    [bigint, bigint, Resolution, string],
    Result_5
  >,
  'validate_admin_restart_bridging' : ActorMethod<[], Result_5>,
  'validate_admin_retry_bridging_task' : ActorMethod<
    [BridgeTx, [] | [BridgeTarget], [] | [string]],
    Result_5
  >,
  'validate_admin_set_evm_fee_limits' : ActorMethod<
    [string, EvmFeeLimits],
    Result_5
  >,
  'validate_admin_set_evm_providers' : ActorMethod<
    [string, bigint, Array<string>],
    Result_5
  >,
  'validate_admin_set_public_providers' : ActorMethod<
    [string, Array<string>],
    Result_5
  >,
  'validate_admin_set_resource_limits' : ActorMethod<
    [ResourceLimits],
    Result_5
  >,
  'validate_admin_set_svm_providers' : ActorMethod<[Array<string>], Result_5>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
