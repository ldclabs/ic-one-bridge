`bnb-block-34500000.json` is the unmodified JSON-RPC response captured on
2026-09-10 from `https://bsc-dataseed.bnbchain.org` using
`eth_getBlockByNumber("0x20e6da0", false)`. NodeReal returned the same block
identity and response size during the review.

The response contains 807 transaction hashes and is 60,575 bytes long. Its
SHA-256 is `4c7408bf70d21da9f3bd4dbcf21d420f105864877eb50189df4a64aae68f95e6`.
Unit tests reuse it to check compact-header reads, bounded compatibility
fallbacks and responses beyond the full-block limit. No live RPC is required
when running the tests.

Protocol references:

- [BNB header and finality RPCs](https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/bsc-api-list/)
- [Geth header RPC](https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-eth)
