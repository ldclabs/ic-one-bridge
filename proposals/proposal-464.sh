#!/usr/bin/env bash

# quill can not support topic field, we should use dfx canister call to send proposal:
dfx canister --network ic call dwv6s-6aaaa-aaaaq-aacta-cai manage_neuron '(
  record {
    subaccount = blob "\84\5a\11\4e\6c\35\0d\a9\24\ea\9c\6b\21\cf\f5\04\e2\02\19\e8\3b\60\a6\2c\96\da\36\ad\41\0e\e0\dd";
    command = opt variant {
      MakeProposal = record {
        title = "Add admin_remove_bridges to remove sub bridges on the one_bridge_canister canister";
        url = "https://internetcomputer.org/docs/current/developer-docs/daos/sns/managing/sns-asset-canister#sns-genericnervoussystemfunctions";
        summary = "Adding a new generic function to remove sub bridges on the one_bridge_canister canister.";
        action = opt variant {
            AddGenericNervousSystemFunction = record {
                id = 1_306 : nat64;
                name = "Remove sub bridges on the one_bridge_canister canister";
                description = opt "Remove sub bridges on the one_bridge_canister canister.";
                function_type = opt variant {
                    GenericNervousSystemFunction = record {
                        topic = opt variant { ApplicationBusinessLogic };
                        validator_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                        target_canister_id = opt principal "dpjyw-raaaa-aaaar-qbxlq-cai";
                        validator_method_name = opt "validate_admin_remove_bridges";
                        target_method_name = opt "admin_remove_bridges";
                    }
                };
            }
        };
      }
    };
  },
)'
