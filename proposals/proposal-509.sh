#!/usr/bin/env bash
set -euo pipefail

# Registers SNS function 1315. Executing the registered method requires a
# separate proposal and a bridge version exposing the target and validator.
# Use dfx to include the ApplicationBusinessLogic topic.
dfx canister --network ic call dwv6s-6aaaa-aaaaq-aacta-cai manage_neuron '(
  record {
    subaccount = blob "\84\5a\11\4e\6c\35\0d\a9\24\ea\9c\6b\21\cf\f5\04\e2\02\19\e8\3b\60\a6\2c\96\da\36\ad\41\0e\e0\dd";
    command = opt variant {
      MakeProposal = record {
        title = "Register admin_resolve_legacy_payout for the PANDA bridge";
        url = "https://github.com/ldclabs/ic-one-bridge";
        summary = "This proposal registers admin_resolve_legacy_payout and validate_admin_resolve_legacy_payout on the PANDA bridge. It enables future governance proposals to attach an auditable operation record and resolve a legacy payout after external reconciliation. Each execution must identify the incoming transaction, the current task ID, the payout outcome and the supporting evidence. A completed outcome must match the recorded outgoing transaction when one is known.";
        action = opt variant {
          AddGenericNervousSystemFunction = record {
            id = 1_315 : nat64;
            name = "Reconcile a legacy bridge payout";
            description = opt "Record an externally verified outcome for a legacy payout without a complete payment journal.";
            function_type = opt variant {
              GenericNervousSystemFunction = record {
                topic = opt variant { ApplicationBusinessLogic };
                validator_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                target_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                validator_method_name = opt "validate_admin_resolve_legacy_payout";
                target_method_name = opt "admin_resolve_legacy_payout";
              }
            };
          }
        };
      }
    };
  },
)'
