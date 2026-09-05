import { Principal } from '@icp-sdk/core/principal'
import { isAddress } from '@solana/kit'

/**
 * Everything the app knows about one chain the bridge can reach.
 *
 * This is the only place chain-specific knowledge lives: explorer links, the
 * native token's decimals, and how an address on that chain is spelled.
 * Listing a new chain means adding one entry to `CHAINS` — no other module
 * switches on a chain name.
 */
export type Chain = {
  // the name the canister uses: 'ICP', 'SOL', or the EVM chain's own name
  readonly name: string
  readonly fullName: string
  readonly logo: string
  readonly nativeDecimals: number
  // explorer page for the token on this chain, given its identifier there
  // (ledger canister on ICP, mint on Solana, contract on EVM)
  readonly tokenUrl: (token: string) => string
  // explorer page for one transfer. ICP has no per-transaction page, so it
  // falls back to the token's page and ignores the hash
  readonly txUrl: (hash: string, token: string) => string
  readonly isValidAddress: (address: string) => boolean
}

const isEvmAddress = (address: string) => /^0x[a-fA-F0-9]{40}$/.test(address)

const CHAINS: Record<string, Chain> = {
  ICP: {
    name: 'ICP',
    fullName: 'Internet Computer',
    logo: '/_assets/icp.webp',
    nativeDecimals: 8,
    tokenUrl: (token) =>
      `https://dashboard.internetcomputer.org/canister/${token}`,
    txUrl: (_hash, token) => `https://www.icexplorer.io/token/details/${token}`,
    isValidAddress: (address) => {
      try {
        Principal.fromText(address)
        return true
      } catch {
        return false
      }
    }
  },
  SOL: {
    name: 'SOL',
    fullName: 'Solana',
    logo: '/_assets/sol.webp',
    nativeDecimals: 9,
    tokenUrl: (token) => `https://solscan.io/token/${token}`,
    txUrl: (hash) => `https://solscan.io/tx/${hash}`,
    isValidAddress: isAddress
  },
  BNB: {
    name: 'BNB',
    fullName: 'BNB Chain',
    logo: '/_assets/bnb.png',
    nativeDecimals: 18,
    tokenUrl: (token) => `https://bscscan.com/token/${token}`,
    txUrl: (hash) => `https://bscscan.com/tx/${hash}`,
    isValidAddress: isEvmAddress
  }
}

/**
 * The chain the canister named, or a bare EVM entry when the bridge lists a
 * chain this build has never heard of.
 *
 * Degrading is deliberate: the canister decides which chains are live, and a
 * newly listed one must not take the chain selector down with it. Such a chain
 * bridges normally; it just shows its raw name and carries no explorer links.
 */
export function getChain(name: string): Chain {
  return (
    CHAINS[name] ?? {
      name,
      fullName: name,
      logo: '',
      nativeDecimals: 18,
      tokenUrl: () => '',
      txUrl: () => '',
      isValidAddress: isEvmAddress
    }
  )
}
