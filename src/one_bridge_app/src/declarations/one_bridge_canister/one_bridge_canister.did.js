export const idlFactory = ({ IDL }) => {
  const ResourceLimits = IDL.Record({
    'max_pending' : IDL.Nat32,
    'requests_per_user_hour' : IDL.Nat32,
    'max_pending_per_user' : IDL.Nat32,
    'min_cycles_reserve' : IDL.Nat,
    'requests_per_hour' : IDL.Nat32,
    'max_active_requests' : IDL.Nat32,
  });
  const UpgradeArgs = IDL.Record({
    'resource_limits' : IDL.Opt(ResourceLimits),
    'min_threshold_to_bridge' : IDL.Opt(IDL.Nat),
    'token_symbol' : IDL.Opt(IDL.Text),
    'governance_canister' : IDL.Opt(IDL.Principal),
    'erc20_gas_limit' : IDL.Opt(IDL.Nat64),
    'token_bridge_fee' : IDL.Opt(IDL.Nat),
    'token_ledger' : IDL.Opt(IDL.Principal),
    'token_logo' : IDL.Opt(IDL.Text),
    'token_name' : IDL.Opt(IDL.Text),
  });
  const InitArgs = IDL.Record({
    'resource_limits' : IDL.Opt(ResourceLimits),
    'min_threshold_to_bridge' : IDL.Nat,
    'token_symbol' : IDL.Text,
    'governance_canister' : IDL.Opt(IDL.Principal),
    'erc20_gas_limit' : IDL.Opt(IDL.Nat64),
    'token_bridge_fee' : IDL.Nat,
    'key_name' : IDL.Text,
    'token_decimals' : IDL.Nat8,
    'token_ledger' : IDL.Principal,
    'token_logo' : IDL.Text,
    'token_name' : IDL.Text,
  });
  const CanisterArgs = IDL.Variant({
    'Upgrade' : UpgradeArgs,
    'Init' : InitArgs,
  });
  const Result = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : IDL.Text });
  const BridgeTx = IDL.Variant({
    'Evm' : IDL.Tuple(IDL.Bool, IDL.Vec(IDL.Nat8)),
    'Icp' : IDL.Tuple(IDL.Bool, IDL.Nat64),
    'Sol' : IDL.Tuple(IDL.Bool, IDL.Vec(IDL.Nat8)),
  });
  const BridgeTarget = IDL.Variant({
    'Evm' : IDL.Text,
    'Icp' : IDL.Null,
    'Sol' : IDL.Null,
  });
  const SolValidity = IDL.Record({
    'blockhash' : IDL.Text,
    'last_valid_block_height' : IDL.Nat64,
    'context_slot' : IDL.Nat64,
  });
  const TxDeadline = IDL.Variant({
    'Nonce' : IDL.Nat64,
    'BlockHeight' : IDL.Nat64,
  });
  const TxMeta = IDL.Record({
    'raw' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'svm_validity' : IDL.Opt(SolValidity),
    'deadline' : TxDeadline,
  });
  const PayoutResolution = IDL.Variant({
    'Failed' : IDL.Null,
    'Completed' : IDL.Null,
    'Expired' : IDL.Null,
    'Incomplete' : IDL.Null,
  });
  const LogRuntime = IDL.Record({
    'next_poll_at' : IDL.Nat64,
    'task_id' : IDL.Nat64,
    'poll_attempts' : IDL.Nat32,
    'payout_attempt' : IDL.Opt(IDL.Nat64),
    'payout_resolution' : IDL.Opt(PayoutResolution),
    'ledger' : IDL.Opt(IDL.Principal),
    'error_chain' : IDL.Opt(BridgeTarget),
  });
  const BridgeLog = IDL.Record({
    'id' : IDL.Opt(IDL.Nat64),
    'to' : BridgeTarget,
    'fee' : IDL.Nat,
    'payout_started_at' : IDL.Nat64,
    'to_tx' : IDL.Opt(BridgeTx),
    'from_meta' : IDL.Opt(TxMeta),
    'to_addr' : IDL.Opt(IDL.Text),
    'to_meta' : IDL.Opt(TxMeta),
    'from' : BridgeTarget,
    'user' : IDL.Principal,
    'from_tx' : BridgeTx,
    'created_at' : IDL.Nat64,
    'error' : IDL.Opt(IDL.Text),
    'stuck' : IDL.Bool,
    'icp_amount' : IDL.Nat,
    'runtime' : IDL.Opt(LogRuntime),
    'finalized_at' : IDL.Nat64,
  });
  const Result_1 = IDL.Variant({ 'Ok' : BridgeLog, 'Err' : IDL.Text });
  const Result_2 = IDL.Variant({ 'Ok' : BridgeTx, 'Err' : IDL.Text });
  const Result_3 = IDL.Variant({
    'Ok' : IDL.Tuple(IDL.Text, IDL.Text),
    'Err' : IDL.Text,
  });
  const Account = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const TransferArg = IDL.Record({
    'to' : Account,
    'fee' : IDL.Opt(IDL.Nat),
    'memo' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'from_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'created_at_time' : IDL.Opt(IDL.Nat64),
    'amount' : IDL.Nat,
  });
  const TransferFromArgs = IDL.Record({
    'to' : Account,
    'fee' : IDL.Opt(IDL.Nat),
    'spender_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'from' : Account,
    'memo' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'created_at_time' : IDL.Opt(IDL.Nat64),
    'amount' : IDL.Nat,
  });
  const RequestInfo = IDL.Variant({
    'LegacyPayout' : BridgeLog,
    'Transfer' : IDL.Tuple(IDL.Principal, TransferArg),
    'Signature' : IDL.Record({
      'validity' : IDL.Opt(SolValidity),
      'scheme' : IDL.Text,
      'deadline' : TxDeadline,
      'sender' : IDL.Principal,
      'message_hash' : IDL.Vec(IDL.Nat8),
    }),
    'TransferFrom' : IDL.Tuple(IDL.Principal, TransferFromArgs),
  });
  const DepositPlan = IDL.Record({
    'to' : BridgeTarget,
    'fee' : IDL.Nat,
    'to_addr' : IDL.Opt(IDL.Text),
    'from' : BridgeTarget,
    'user' : IDL.Principal,
    'ledger' : IDL.Principal,
    'amount' : IDL.Nat,
  });
  const Phase = IDL.Variant({
    'Prepared' : IDL.Null,
    'Rejected' : IDL.Text,
    'NeedsReview' : IDL.Text,
    'Submitted' : IDL.Null,
    'Planning' : IDL.Null,
    'Signed' : IDL.Null,
    'Completed' : BridgeTx,
  });
  const Resolution = IDL.Variant({
    'NotExecuted' : IDL.Null,
    'Completed' : BridgeTx,
  });
  const Reconciliation = IDL.Record({
    'controller' : IDL.Principal,
    'resolution' : Resolution,
    'evidence' : IDL.Text,
  });
  const OperationInfo = IDL.Record({
    'id' : IDL.Nat64,
    'owner' : IDL.Principal,
    'request' : IDL.Opt(RequestInfo),
    'kind' : IDL.Text,
    'signed_tx' : IDL.Opt(BridgeTx),
    'deposit' : IDL.Opt(DepositPlan),
    'created_at' : IDL.Nat64,
    'error' : IDL.Opt(IDL.Text),
    'related_task' : IDL.Opt(BridgeLog),
    'phase' : Phase,
    'revision' : IDL.Nat64,
    'reconciliation' : IDL.Opt(Reconciliation),
  });
  const Result_4 = IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : IDL.Text });
  const EvmFeeLimits = IDL.Record({
    'max_priority_fee_per_gas' : IDL.Nat,
    'max_hourly_fee' : IDL.Nat,
    'max_fee_per_gas' : IDL.Nat,
    'max_transaction_fee' : IDL.Nat,
  });
  const Result_5 = IDL.Variant({ 'Ok' : IDL.Text, 'Err' : IDL.Text });
  const Result_6 = IDL.Variant({ 'Ok' : IDL.Vec(IDL.Nat8), 'Err' : IDL.Text });
  const Result_7 = IDL.Variant({ 'Ok' : IDL.Vec(BridgeLog), 'Err' : IDL.Text });
  const StateRuntimeInfo = IDL.Record({
    'resource_limits' : ResourceLimits,
    'pending_count' : IDL.Nat64,
    'ledger_verified' : IDL.Bool,
    'keys_ready' : IDL.Tuple(IDL.Bool, IDL.Bool),
    'migration_remaining' : IDL.Nat64,
    'evm_provider_hosts' : IDL.Vec(IDL.Tuple(IDL.Text, IDL.Vec(IDL.Text))),
    'svm_provider_hosts' : IDL.Vec(IDL.Text),
    'available_icp_fees' : IDL.Nat,
    'evm_fee_limits' : IDL.Vec(IDL.Tuple(IDL.Text, EvmFeeLimits)),
    'unresolved_operations' : IDL.Nat64,
    'svm_mint_verified' : IDL.Bool,
  });
  const StateInfo = IDL.Record({
    'total_withdrawn_fees' : IDL.Nat,
    'error_rounds' : IDL.Nat64,
    'evm_address' : IDL.Text,
    'evm_latest_gas' : IDL.Vec(
      IDL.Tuple(IDL.Text, IDL.Tuple(IDL.Nat64, IDL.Nat, IDL.Nat))
    ),
    'svm_address' : IDL.Text,
    'finalize_bridging_round' : IDL.Tuple(IDL.Nat64, IDL.Bool),
    'total_collected_fees' : IDL.Nat,
    'min_threshold_to_bridge' : IDL.Nat,
    'token_symbol' : IDL.Text,
    'governance_canister' : IDL.Opt(IDL.Principal),
    'icp_address' : IDL.Principal,
    'total_bridge_count' : IDL.Nat64,
    'evm_token_contracts' : IDL.Vec(
      IDL.Tuple(IDL.Text, IDL.Tuple(IDL.Text, IDL.Nat8, IDL.Nat64))
    ),
    'erc20_gas_limit' : IDL.Nat64,
    'svm_providers' : IDL.Vec(IDL.Text),
    'svm_token_address' : IDL.Tuple(IDL.Text, IDL.Nat8, IDL.Text),
    'token_bridge_fee' : IDL.Nat,
    'key_name' : IDL.Text,
    'total_bridged_tokens' : IDL.Nat,
    'evm_providers' : IDL.Vec(
      IDL.Tuple(IDL.Text, IDL.Tuple(IDL.Nat64, IDL.Vec(IDL.Text)))
    ),
    'token_decimals' : IDL.Nat8,
    'token_ledger' : IDL.Principal,
    'token_logo' : IDL.Text,
    'token_name' : IDL.Text,
    'runtime' : IDL.Opt(StateRuntimeInfo),
    'icp_collected_fees' : IDL.Nat,
    'sub_bridges' : IDL.Vec(IDL.Principal),
  });
  const Result_8 = IDL.Variant({ 'Ok' : StateInfo, 'Err' : IDL.Text });
  const Result_9 = IDL.Variant({
    'Ok' : IDL.Vec(OperationInfo),
    'Err' : IDL.Text,
  });
  return IDL.Service({
    'admin_add_bridges' : IDL.Func([IDL.Vec(IDL.Principal)], [Result], []),
    'admin_add_evm_contract' : IDL.Func(
        [IDL.Text, IDL.Nat64, IDL.Text],
        [Result],
        [],
      ),
    'admin_add_svm_contract' : IDL.Func([IDL.Text], [Result], []),
    'admin_close_bridging_task' : IDL.Func(
        [BridgeTx, IDL.Opt(IDL.Bool)],
        [Result_1],
        [],
      ),
    'admin_collect_fees' : IDL.Func([IDL.Principal, IDL.Nat], [Result_2], []),
    'admin_init_public_keys' : IDL.Func([], [Result_3], []),
    'admin_operations' : IDL.Func(
        [IDL.Principal, IDL.Nat32, IDL.Opt(IDL.Nat64)],
        [IDL.Vec(OperationInfo)],
        ['query'],
      ),
    'admin_recheck_task' : IDL.Func([BridgeTx], [Result], []),
    'admin_remove_bridges' : IDL.Func([IDL.Vec(IDL.Principal)], [Result], []),
    'admin_resolve_legacy_payout' : IDL.Func(
        [BridgeTx, IDL.Nat64, Resolution, IDL.Text],
        [Result],
        [],
      ),
    'admin_resolve_operation' : IDL.Func(
        [IDL.Nat64, IDL.Nat64, Resolution, IDL.Text],
        [Result],
        [],
      ),
    'admin_restart_bridging' : IDL.Func([], [Result_4], []),
    'admin_retry_bridging_task' : IDL.Func(
        [BridgeTx, IDL.Opt(BridgeTarget), IDL.Opt(IDL.Text)],
        [Result_1],
        [],
      ),
    'admin_set_evm_fee_limits' : IDL.Func(
        [IDL.Text, EvmFeeLimits],
        [Result],
        [],
      ),
    'admin_set_evm_providers' : IDL.Func(
        [IDL.Text, IDL.Nat64, IDL.Vec(IDL.Text)],
        [Result],
        [],
      ),
    'admin_set_public_providers' : IDL.Func(
        [IDL.Text, IDL.Vec(IDL.Text)],
        [Result],
        [],
      ),
    'admin_set_resource_limits' : IDL.Func([ResourceLimits], [Result], []),
    'admin_set_svm_providers' : IDL.Func([IDL.Vec(IDL.Text)], [Result], []),
    'bridge' : IDL.Func(
        [IDL.Text, IDL.Text, IDL.Nat, IDL.Opt(IDL.Text)],
        [Result_2],
        [],
      ),
    'bridge_with_id' : IDL.Func(
        [IDL.Text, IDL.Text, IDL.Nat, IDL.Opt(IDL.Text), IDL.Vec(IDL.Nat8)],
        [Result_2],
        [],
      ),
    'cancel_operation' : IDL.Func([IDL.Nat64], [Result], []),
    'erc20_transfer' : IDL.Func([IDL.Text, IDL.Text, IDL.Nat], [Result_5], []),
    'erc20_transfer_tx' : IDL.Func(
        [IDL.Text, IDL.Text, IDL.Nat],
        [Result_5],
        [],
      ),
    'evm_address' : IDL.Func([IDL.Opt(IDL.Principal)], [Result_5], ['query']),
    'evm_sign' : IDL.Func([IDL.Vec(IDL.Nat8)], [Result_6], []),
    'evm_transfer_tx' : IDL.Func([IDL.Text, IDL.Text, IDL.Nat], [Result_5], []),
    'finalized_logs' : IDL.Func(
        [IDL.Nat32, IDL.Opt(IDL.Nat64)],
        [Result_7],
        ['query'],
      ),
    'info' : IDL.Func([], [Result_8], ['query']),
    'my_bridge_log' : IDL.Func([BridgeTx], [Result_1], ['query']),
    'my_finalized_logs' : IDL.Func(
        [IDL.Nat32, IDL.Opt(IDL.Nat64)],
        [Result_7],
        ['query'],
      ),
    'my_operations' : IDL.Func(
        [IDL.Nat32, IDL.Opt(IDL.Nat64)],
        [Result_9],
        ['query'],
      ),
    'my_pending_logs' : IDL.Func([], [Result_7], ['query']),
    'my_pending_logs_page' : IDL.Func(
        [IDL.Nat32, IDL.Opt(IDL.Nat64)],
        [Result_7],
        ['query'],
      ),
    'pending_logs' : IDL.Func([], [Result_7], ['query']),
    'pending_logs_page' : IDL.Func(
        [IDL.Nat32, IDL.Opt(IDL.Nat64)],
        [Result_7],
        ['query'],
      ),
    'recheck_task' : IDL.Func([BridgeTx], [Result], []),
    'resume_deposit' : IDL.Func([IDL.Nat64], [Result_2], []),
    'resume_operation' : IDL.Func([IDL.Nat64], [Result_2], []),
    'sol_transfer_tx' : IDL.Func([IDL.Text, IDL.Nat64], [Result_5], []),
    'spl_transfer_tx' : IDL.Func([IDL.Text, IDL.Nat], [Result_5], []),
    'svm_address' : IDL.Func([IDL.Opt(IDL.Principal)], [Result_5], ['query']),
    'validate_admin_add_bridges' : IDL.Func(
        [IDL.Vec(IDL.Principal)],
        [Result_5],
        [],
      ),
    'validate_admin_add_evm_contract' : IDL.Func(
        [IDL.Text, IDL.Nat64, IDL.Text],
        [Result_5],
        [],
      ),
    'validate_admin_add_svm_contract' : IDL.Func([IDL.Text], [Result_5], []),
    'validate_admin_close_bridging_task' : IDL.Func(
        [BridgeTx, IDL.Opt(IDL.Bool)],
        [Result_5],
        [],
      ),
    'validate_admin_collect_fees' : IDL.Func(
        [IDL.Principal, IDL.Nat],
        [Result_5],
        [],
      ),
    'validate_admin_init_public_keys' : IDL.Func([], [Result_5], []),
    'validate_admin_recheck_task' : IDL.Func([BridgeTx], [Result_5], []),
    'validate_admin_remove_bridges' : IDL.Func(
        [IDL.Vec(IDL.Principal)],
        [Result_5],
        [],
      ),
    'validate_admin_resolve_legacy_payout' : IDL.Func(
        [BridgeTx, IDL.Nat64, Resolution, IDL.Text],
        [Result_5],
        [],
      ),
    'validate_admin_resolve_operation' : IDL.Func(
        [IDL.Nat64, IDL.Nat64, Resolution, IDL.Text],
        [Result_5],
        [],
      ),
    'validate_admin_restart_bridging' : IDL.Func([], [Result_5], []),
    'validate_admin_retry_bridging_task' : IDL.Func(
        [BridgeTx, IDL.Opt(BridgeTarget), IDL.Opt(IDL.Text)],
        [Result_5],
        [],
      ),
    'validate_admin_set_evm_fee_limits' : IDL.Func(
        [IDL.Text, EvmFeeLimits],
        [Result_5],
        [],
      ),
    'validate_admin_set_evm_providers' : IDL.Func(
        [IDL.Text, IDL.Nat64, IDL.Vec(IDL.Text)],
        [Result_5],
        [],
      ),
    'validate_admin_set_public_providers' : IDL.Func(
        [IDL.Text, IDL.Vec(IDL.Text)],
        [Result_5],
        [],
      ),
    'validate_admin_set_resource_limits' : IDL.Func(
        [ResourceLimits],
        [Result_5],
        [],
      ),
    'validate_admin_set_svm_providers' : IDL.Func(
        [IDL.Vec(IDL.Text)],
        [Result_5],
        [],
      ),
  });
};
export const init = ({ IDL }) => {
  const ResourceLimits = IDL.Record({
    'max_pending' : IDL.Nat32,
    'requests_per_user_hour' : IDL.Nat32,
    'max_pending_per_user' : IDL.Nat32,
    'min_cycles_reserve' : IDL.Nat,
    'requests_per_hour' : IDL.Nat32,
    'max_active_requests' : IDL.Nat32,
  });
  const UpgradeArgs = IDL.Record({
    'resource_limits' : IDL.Opt(ResourceLimits),
    'min_threshold_to_bridge' : IDL.Opt(IDL.Nat),
    'token_symbol' : IDL.Opt(IDL.Text),
    'governance_canister' : IDL.Opt(IDL.Principal),
    'erc20_gas_limit' : IDL.Opt(IDL.Nat64),
    'token_bridge_fee' : IDL.Opt(IDL.Nat),
    'token_ledger' : IDL.Opt(IDL.Principal),
    'token_logo' : IDL.Opt(IDL.Text),
    'token_name' : IDL.Opt(IDL.Text),
  });
  const InitArgs = IDL.Record({
    'resource_limits' : IDL.Opt(ResourceLimits),
    'min_threshold_to_bridge' : IDL.Nat,
    'token_symbol' : IDL.Text,
    'governance_canister' : IDL.Opt(IDL.Principal),
    'erc20_gas_limit' : IDL.Opt(IDL.Nat64),
    'token_bridge_fee' : IDL.Nat,
    'key_name' : IDL.Text,
    'token_decimals' : IDL.Nat8,
    'token_ledger' : IDL.Principal,
    'token_logo' : IDL.Text,
    'token_name' : IDL.Text,
  });
  const CanisterArgs = IDL.Variant({
    'Upgrade' : UpgradeArgs,
    'Init' : InitArgs,
  });
  return [IDL.Opt(CanisterArgs)];
};
