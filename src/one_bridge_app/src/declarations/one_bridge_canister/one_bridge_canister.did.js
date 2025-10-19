export const idlFactory = ({ IDL }) => {
  const UpgradeArgs = IDL.Record({
    'min_threshold_to_bridge' : IDL.Opt(IDL.Nat),
    'token_symbol' : IDL.Opt(IDL.Text),
    'governance_canister' : IDL.Opt(IDL.Principal),
    'token_bridge_fee' : IDL.Opt(IDL.Nat),
    'token_ledger' : IDL.Opt(IDL.Principal),
    'token_logo' : IDL.Opt(IDL.Text),
    'token_name' : IDL.Opt(IDL.Text),
  });
  const InitArgs = IDL.Record({
    'min_threshold_to_bridge' : IDL.Nat,
    'token_symbol' : IDL.Text,
    'governance_canister' : IDL.Opt(IDL.Principal),
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
  });
  const Result_1 = IDL.Variant({ 'Ok' : BridgeTx, 'Err' : IDL.Text });
  const Result_2 = IDL.Variant({ 'Ok' : IDL.Text, 'Err' : IDL.Text });
  const BridgeTarget = IDL.Variant({ 'Evm' : IDL.Text, 'Icp' : IDL.Null });
  const BridgeLog = IDL.Record({
    'id' : IDL.Opt(IDL.Nat64),
    'to' : BridgeTarget,
    'fee' : IDL.Nat,
    'to_tx' : IDL.Opt(BridgeTx),
    'to_addr' : IDL.Opt(IDL.Text),
    'from' : BridgeTarget,
    'user' : IDL.Principal,
    'from_tx' : BridgeTx,
    'created_at' : IDL.Nat64,
    'error' : IDL.Opt(IDL.Text),
    'icp_amount' : IDL.Nat,
    'finalized_at' : IDL.Nat64,
  });
  const Result_3 = IDL.Variant({ 'Ok' : IDL.Vec(BridgeLog), 'Err' : IDL.Text });
  const StateInfo = IDL.Record({
    'total_withdrawn_fees' : IDL.Nat,
    'evm_address' : IDL.Text,
    'evm_latest_gas' : IDL.Vec(
      IDL.Tuple(IDL.Text, IDL.Tuple(IDL.Nat64, IDL.Nat, IDL.Nat))
    ),
    'finalize_bridging_round' : IDL.Tuple(IDL.Nat64, IDL.Bool),
    'total_collected_fees' : IDL.Nat,
    'min_threshold_to_bridge' : IDL.Nat,
    'token_symbol' : IDL.Text,
    'icp_address' : IDL.Principal,
    'total_bridge_count' : IDL.Nat64,
    'evm_token_contracts' : IDL.Vec(
      IDL.Tuple(IDL.Text, IDL.Tuple(IDL.Text, IDL.Nat8, IDL.Nat64))
    ),
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
  });
  const Result_4 = IDL.Variant({ 'Ok' : StateInfo, 'Err' : IDL.Text });
  const Result_5 = IDL.Variant({ 'Ok' : BridgeLog, 'Err' : IDL.Text });
  return IDL.Service({
    'admin_add_evm_contract' : IDL.Func(
        [IDL.Text, IDL.Nat64, IDL.Text],
        [Result],
        [],
      ),
    'admin_collect_fees' : IDL.Func([IDL.Principal, IDL.Nat], [Result_1], []),
    'admin_set_evm_providers' : IDL.Func(
        [IDL.Text, IDL.Nat64, IDL.Vec(IDL.Text)],
        [Result],
        [],
      ),
    'bridge' : IDL.Func(
        [IDL.Text, IDL.Text, IDL.Nat, IDL.Opt(IDL.Text)],
        [Result_1],
        [],
      ),
    'erc20_transfer' : IDL.Func([IDL.Text, IDL.Text, IDL.Nat], [Result_2], []),
    'erc20_transfer_tx' : IDL.Func(
        [IDL.Text, IDL.Text, IDL.Nat],
        [Result_2],
        [],
      ),
    'evm_transfer_tx' : IDL.Func([IDL.Text, IDL.Text, IDL.Nat], [Result_2], []),
    'finalized_logs' : IDL.Func(
        [IDL.Opt(IDL.Nat64), IDL.Opt(IDL.Nat64)],
        [Result_3],
        ['query'],
      ),
    'info' : IDL.Func([], [Result_4], ['query']),
    'my_bridge_log' : IDL.Func([BridgeTx], [Result_5], ['query']),
    'my_evm_address' : IDL.Func([], [Result_2], ['query']),
    'my_finalized_logs' : IDL.Func(
        [IDL.Opt(IDL.Nat64), IDL.Opt(IDL.Nat64)],
        [Result_3],
        ['query'],
      ),
    'my_pending_logs' : IDL.Func([], [Result_3], ['query']),
    'pending_logs' : IDL.Func([], [Result_3], ['query']),
    'validate_admin_add_evm_contract' : IDL.Func(
        [IDL.Text, IDL.Nat64, IDL.Text],
        [Result_2],
        [],
      ),
    'validate_admin_collect_fees' : IDL.Func(
        [IDL.Principal, IDL.Nat],
        [Result_2],
        [],
      ),
    'validate_admin_set_evm_providers' : IDL.Func(
        [IDL.Text, IDL.Nat64, IDL.Vec(IDL.Text)],
        [Result_2],
        [],
      ),
  });
};
export const init = ({ IDL }) => {
  const UpgradeArgs = IDL.Record({
    'min_threshold_to_bridge' : IDL.Opt(IDL.Nat),
    'token_symbol' : IDL.Opt(IDL.Text),
    'governance_canister' : IDL.Opt(IDL.Principal),
    'token_bridge_fee' : IDL.Opt(IDL.Nat),
    'token_ledger' : IDL.Opt(IDL.Principal),
    'token_logo' : IDL.Opt(IDL.Text),
    'token_name' : IDL.Opt(IDL.Text),
  });
  const InitArgs = IDL.Record({
    'min_threshold_to_bridge' : IDL.Nat,
    'token_symbol' : IDL.Text,
    'governance_canister' : IDL.Opt(IDL.Principal),
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
