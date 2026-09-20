#!/usr/bin/env bash
set -euo pipefail

# Registers SNS function 1314. Executing the registered method requires a
# separate proposal and a bridge version exposing the target and validator.
# Use dfx to include the ApplicationBusinessLogic topic.
dfx canister --network ic call dwv6s-6aaaa-aaaaq-aacta-cai manage_neuron '(
  record {
    subaccount = blob "\84\5a\11\4e\6c\35\0d\a9\24\ea\9c\6b\21\cf\f5\04\e2\02\19\e8\3b\60\a6\2c\96\da\36\ad\41\0e\e0\dd";
    command = opt variant {
      MakeProposal = record {
        title = "Register admin_resolve_operation for the PANDA bridge";
        url = "https://github.com/ldclabs/ic-one-bridge";
        summary = "This proposal registers admin_resolve_operation and validate_admin_resolve_operation on the PANDA bridge. It enables future governance proposals to recognize a finalized transaction or attest non-execution for a durable payment operation after external ledger or chain reconciliation. Each execution must identify the operation, its current revision, the resolution and the supporting evidence. Resolution can update accounting and resume related tasks, so the evidence must establish the actual payment outcome.";
        action = opt variant {
          AddGenericNervousSystemFunction = record {
            id = 1_314 : nat64;
            name = "Reconcile a bridge payment operation";
            description = opt "Reconcile a durable payment operation using its exact revision and external evidence.";
            function_type = opt variant {
              GenericNervousSystemFunction = record {
                topic = opt variant { ApplicationBusinessLogic };
                validator_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                target_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                validator_method_name = opt "validate_admin_resolve_operation";
                target_method_name = opt "admin_resolve_operation";
              }
            };
          }
        };
      }
    };
  },
)'
