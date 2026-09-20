#!/usr/bin/env bash
set -euo pipefail

# Registers SNS function 1318. Executing the registered method requires a
# separate proposal and a bridge version exposing the target and validator.
# Use dfx to include the ApplicationBusinessLogic topic.
dfx canister --network ic call dwv6s-6aaaa-aaaaq-aacta-cai manage_neuron '(
  record {
    subaccount = blob "\84\5a\11\4e\6c\35\0d\a9\24\ea\9c\6b\21\cf\f5\04\e2\02\19\e8\3b\60\a6\2c\96\da\36\ad\41\0e\e0\dd";
    command = opt variant {
      MakeProposal = record {
        title = "Register admin_set_public_providers for the PANDA bridge";
        url = "https://github.com/ldclabs/ic-one-bridge";
        summary = "This proposal registers admin_set_public_providers and validate_admin_set_public_providers on the PANDA bridge. It enables future governance proposals to publish anonymous official browser RPC endpoints for SOL or a configured EVM chain. These public endpoints are separate from the internal providers used for financial evidence. Each execution must provide independent HTTPS endpoints intended for public disclosure and must not contain API credentials.";
        action = opt variant {
          AddGenericNervousSystemFunction = record {
            id = 1_318 : nat64;
            name = "Set public browser RPC endpoints for the bridge";
            description = opt "Publish anonymous browser RPC endpoints separately from the internal financial-evidence providers.";
            function_type = opt variant {
              GenericNervousSystemFunction = record {
                topic = opt variant { ApplicationBusinessLogic };
                validator_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                target_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                validator_method_name = opt "validate_admin_set_public_providers";
                target_method_name = opt "admin_set_public_providers";
              }
            };
          }
        };
      }
    };
  },
)'
