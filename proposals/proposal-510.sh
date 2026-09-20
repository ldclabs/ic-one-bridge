#!/usr/bin/env bash
set -euo pipefail

# Registers SNS function 1317. Executing the registered method requires a
# separate proposal and a bridge version exposing the target and validator.
# Use dfx to include the ApplicationBusinessLogic topic.
dfx canister --network ic call dwv6s-6aaaa-aaaaq-aacta-cai manage_neuron '(
  record {
    subaccount = blob "\84\5a\11\4e\6c\35\0d\a9\24\ea\9c\6b\21\cf\f5\04\e2\02\19\e8\3b\60\a6\2c\96\da\36\ad\41\0e\e0\dd";
    command = opt variant {
      MakeProposal = record {
        title = "Register admin_recheck_task for the PANDA bridge";
        url = "https://github.com/ldclabs/ic-one-bridge";
        summary = "This proposal registers admin_recheck_task and validate_admin_recheck_task on the PANDA bridge. It enables future governance proposals to recheck a selected pending task after a transient failure, preserving its existing transaction bytes and deduplication keys. The original incoming transaction identifies the task. Durable duplicate-source and financial reconciliation holds remain enforced.";
        action = opt variant {
          AddGenericNervousSystemFunction = record {
            id = 1_317 : nat64;
            name = "Recheck a pending bridge task";
            description = opt "Resume status checks for a pending task while preserving its recorded payment intent.";
            function_type = opt variant {
              GenericNervousSystemFunction = record {
                topic = opt variant { ApplicationBusinessLogic };
                validator_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                target_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                validator_method_name = opt "validate_admin_recheck_task";
                target_method_name = opt "admin_recheck_task";
              }
            };
          }
        };
      }
    };
  },
)'
