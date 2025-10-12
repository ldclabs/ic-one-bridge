# `one_bridge_canister`
🪙 An Internet Computer canister to bridge ERC-20 & ICRC tokens.

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

Deploy the canister to subnet `pzp6e`:
```bash
dfx deploy one_bridge_canister --argument "(opt variant {Init =
  record {
    key_name = \"key_1\";
    token_name = \"ICPanda\";
    token_symbol = \"PANDA\";
    token_decimals = 8;
    token_logo = \"https://532er-faaaa-aaaaj-qncpa-cai.icp0.io/f/374?inline&filename=1734188626561.webp\";
    token_ledger = principal \"druyg-tyaaa-aaaaq-aactq-cai\";
    governance_canister = opt principal \"dwv6s-6aaaa-aaaaq-aacta-cai\";
  }
})" --ic --subnet pzp6e-ekpqk-3c5x7-2h6so-njoeq-mt45d-h3h6c-q3mxf-vpeq5-fk5o7-yae
```

dfx canister call one_bridge_canister info '()' --ic

dfx canister call one_bridge_canister my_evm_address '()' --ic

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