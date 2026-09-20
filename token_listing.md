# Apply for Token Listing

One Bridge is a cross-chain token bridge project based on a "lock/release" mechanism, running entirely on the Internet Computer (ICP) blockchain.

Each token asset is managed by a dedicated ICP smart contract (canister). This canister already enables seamless multi-chain token transfers between ICP, Ethereum, BNB Chain, and other EVM-compatible networks, and Solana.

## Application Requirements

1. The project team must deploy the One Bridge canister on ICP and the corresponding token contracts on each target chain (e.g., an ERC-20 contract for EVM-compatible chains).
2. The contract code must be an official stable release, and the ERC-20 contract must have its source code verified.
3. The controllers of the One Bridge canister and the Token Ledger canister on ICP must be an SNS DAO, the NNS, or a black hole address to ensure the contract's security and decentralization.
4. The project team must provide sufficient liquidity to support cross-chain transfers.
5. The project team is responsible for covering the transaction fees (Gas Fees) on all respective chains.

## Application Process

1. The project team completes the deployment and configuration of the smart contracts.
2. Submit the token information and contract addresses via a Github issue or on X (@ICPandaDAO) for review by our team (the ICPanda Team).
3. Upon approval, the project team must provide sufficient liquidity and funds for gas fees.
4. We will submit a proposal to add the token to the official One Bridge list for public review and voting.
5. Once the proposal is passed, the token will be automatically listed on the One Bridge cross-chain bridge.

## Deployment Guidelines

TODO


Supported assets must have fixed-unit transfer semantics. Solana mint extensions such as transfer
fees and hooks are rejected for bridging; the ledger decimals and minting account are checked at
initialization. The bridge's ledger account must cover payouts and transfer fees, and its external
addresses need native gas reserves. The project must configure independent official core RPC
providers and publish separate anonymous browser RPC
endpoints when the internal providers require credentials. Governance must register any newly used
admin methods and their matching validation functions before invoking them through SNS proposals.
