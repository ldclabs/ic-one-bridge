# `One Bridge Canister`
🌉 A fully on-chain cross-chain token bridge (Internet Computer, Ethereum, BNB Chain and other EVM-compatible chains).

## Demo

ICPanda's PANDA token bridge: https://dpjyw-raaaa-aaaar-qbxlq-cai.raw.icp0.io/?id=53cyg-yyaaa-aaaap-ahpua-cai

## Quick Start

### Local Deployment

Deploy the canister:
```bash
dfx canister create --specified-id dpjyw-raaaa-aaaar-qbxlq-cai one_bridge_canister
# deploy with default settings
dfx deploy one_bridge_canister
```

### Deployment

#### 1. Deploy the canister to subnet `pzp6e`:
```bash
dfx deploy one_bridge_canister --argument "(opt variant {Init =
  record {
    key_name = \"key_1\";
    token_name = \"ICPanda\";
    token_symbol = \"PANDA\";
    token_decimals = 8;
    token_logo = \"https://532er-faaaa-aaaaj-qncpa-cai.icp0.io/f/374?inline&filename=1734188626561.webp\";
    token_ledger = principal \"druyg-tyaaa-aaaaq-aactq-cai\";
    token_bridge_fee = 100_000;
    min_threshold_to_bridge = 1_000_000_000;
    governance_canister = opt principal \"dwv6s-6aaaa-aaaaq-aacta-cai\";
  }
})" --ic --subnet pzp6e-ekpqk-3c5x7-2h6so-njoeq-mt45d-h3h6c-q3mxf-vpeq5-fk5o7-yae
```

#### 2. Check info:
```bash
dfx canister call one_bridge_canister info '()' --ic
dfx canister call one_bridge_canister my_evm_address '()' --ic
```

#### 3. Set EVM providers (e.g. BNB Chain Mainnet):
```bash
# chain_name = "BNB"
# max_confirmations = 11
# providers = vec { "https://bsc.nodereal.io"; "https://bsc-mainnet.nodereal.io/v1/64a9df0874fb4a93b9d0a3849de012d3" }
dfx canister call one_bridge_canister admin_set_evm_providers '("BNB", 11, vec { "https://bsc.nodereal.io"; "https://bsc-mainnet.nodereal.io/v1/64a9df0874fb4a93b9d0a3849de012d3" })' --ic
```

#### 4. Add EVM contract (e.g. BNB Chain PANDA token):
```bash
# chain_name = "BNB"
# chain_id = 56
# contract_address = "0xe74583edAFF618D88463554b84Bc675196b36990" (this is testnet address, replace with mainnet address)
dfx canister call one_bridge_canister admin_add_evm_contract '("BNB", 56, "0xe74583edAFF618D88463554b84Bc675196b36990")' --ic
```

**We can add other EVM chains (Ethereum, Base, Avalanche...) and contracts similarly.**

#### 5. Bridge 1 PANDA from ICP to BNB Chain:
- 5.1. The total supply of PANDA on BNB Chain should be hold by the bridge canister's EVM address at initialization.
- 5.2. Make sure the bridge canister evm address has enough gas (BNB) to pay for the transaction fees on BNB Chain.
- 5.3. The user should approve the canister to spend PANDA on their behalf.

```bash
# from_chain = "ICP"
# to_chain = "BNB"
# amount = 100_000_000 (1 PANDA with 8 decimals)
dfx canister call one_bridge_canister bridge '("ICP", "BNB", 100_000_000)' --ic

# check pending tansfers
dfx canister call one_bridge_canister my_pending_logs '()' --ic

# after some time, check finalized tansfers
dfx canister call one_bridge_canister my_finalized_logs '(null, null)' --ic
```

## API Reference

The canister exposes a comprehensive Candid API. Key endpoints include:

```candid
admin_add_evm_contract : (text, nat64, text) -> (Result);
admin_set_evm_providers : (text, nat64, vec text) -> (Result);
admin_update_evm_gas_price : () -> (Result);
bridge : (text, text, nat) -> (Result_1);
erc20_transfer : (text, text, nat) -> (Result_2);
erc20_transfer_tx : (text, text, nat) -> (Result_2);
info : () -> (Result_3) query;
my_evm_address : () -> (Result_2) query;
my_finalized_logs : (opt nat64, opt nat64) -> (Result_4) query;
my_pending_logs : () -> (Result_4) query;
validate_admin_add_evm_contract : (text, nat64, text) -> (Result_2);
validate_admin_set_evm_providers : (text, nat64, vec text) -> (Result_2);
```

Full Candid API definition: [one_bridge_canister.did](https://github.com/ldclabs/ic-one-bridge/tree/main/src/one_bridge_canister/one_bridge_canister.did)

## License
Copyright © 2024-2025 [LDC Labs](https://github.com/ldclabs).

`ldclabs/ic-one-bridge` is licensed under the MIT License. See [LICENSE](./LICENSE-MIT) for the full license text.