import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface BridgeLog {
  'id' : [] | [bigint],
  'to' : BridgeTarget,
  'fee' : bigint,
  'to_tx' : [] | [BridgeTx],
  'from' : BridgeTarget,
  'user' : Principal,
  'from_tx' : BridgeTx,
  'created_at' : bigint,
  'error' : [] | [string],
  'icp_amount' : bigint,
  'finalized_at' : bigint,
}
export type BridgeTarget = { 'Evm' : string } |
  { 'Icp' : null };
export type BridgeTx = { 'Evm' : [boolean, Uint8Array | number[]] } |
  { 'Icp' : [boolean, bigint] };
export type CanisterArgs = { 'Upgrade' : UpgradeArgs } |
  { 'Init' : InitArgs };
export interface InitArgs {
  'min_threshold_to_bridge' : bigint,
  'token_symbol' : string,
  'governance_canister' : [] | [Principal],
  'token_bridge_fee' : bigint,
  'key_name' : string,
  'token_decimals' : number,
  'token_ledger' : Principal,
  'token_logo' : string,
  'token_name' : string,
}
export type Result = { 'Ok' : null } |
  { 'Err' : string };
export type Result_1 = { 'Ok' : BridgeTx } |
  { 'Err' : string };
export type Result_2 = { 'Ok' : string } |
  { 'Err' : string };
export type Result_3 = { 'Ok' : Array<BridgeLog> } |
  { 'Err' : string };
export type Result_4 = { 'Ok' : StateInfo } |
  { 'Err' : string };
export type Result_5 = { 'Ok' : BridgeLog } |
  { 'Err' : string };
export interface StateInfo {
  'total_withdrawn_fees' : bigint,
  'evm_address' : string,
  'evm_latest_gas' : Array<[string, [bigint, bigint, bigint]]>,
  'finalize_bridging_round' : [bigint, boolean],
  'total_collected_fees' : bigint,
  'min_threshold_to_bridge' : bigint,
  'token_symbol' : string,
  'icp_address' : Principal,
  'total_bridge_count' : bigint,
  'evm_token_contracts' : Array<[string, [string, number, bigint]]>,
  'token_bridge_fee' : bigint,
  'key_name' : string,
  'total_bridged_tokens' : bigint,
  'evm_providers' : Array<[string, [bigint, Array<string>]]>,
  'token_decimals' : number,
  'token_ledger' : Principal,
  'token_logo' : string,
  'token_name' : string,
}
export interface UpgradeArgs {
  'min_threshold_to_bridge' : [] | [bigint],
  'token_symbol' : [] | [string],
  'governance_canister' : [] | [Principal],
  'token_bridge_fee' : [] | [bigint],
  'token_ledger' : [] | [Principal],
  'token_logo' : [] | [string],
  'token_name' : [] | [string],
}
export interface _SERVICE {
  'admin_add_evm_contract' : ActorMethod<[string, bigint, string], Result>,
  'admin_collect_fees' : ActorMethod<[Principal, bigint], Result_1>,
  'admin_set_evm_providers' : ActorMethod<
    [string, bigint, Array<string>],
    Result
  >,
  'bridge' : ActorMethod<[string, string, bigint], Result_1>,
  'erc20_transfer' : ActorMethod<[string, string, bigint], Result_2>,
  'erc20_transfer_tx' : ActorMethod<[string, string, bigint], Result_2>,
  'evm_transfer_tx' : ActorMethod<[string, string, bigint], Result_2>,
  'finalized_logs' : ActorMethod<[[] | [bigint], [] | [bigint]], Result_3>,
  'info' : ActorMethod<[], Result_4>,
  'my_bridge_log' : ActorMethod<[BridgeTx], Result_5>,
  'my_evm_address' : ActorMethod<[], Result_2>,
  'my_finalized_logs' : ActorMethod<[[] | [bigint], [] | [bigint]], Result_3>,
  'my_pending_logs' : ActorMethod<[], Result_3>,
  'pending_logs' : ActorMethod<[], Result_3>,
  'validate_admin_add_evm_contract' : ActorMethod<
    [string, bigint, string],
    Result_2
  >,
  'validate_admin_collect_fees' : ActorMethod<[Principal, bigint], Result_2>,
  'validate_admin_set_evm_providers' : ActorMethod<
    [string, bigint, Array<string>],
    Result_2
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
