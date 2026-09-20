import type { EvmFeeLimits } from '../../declarations/one_bridge_canister/one_bridge_canister.did.js'

export function evmFee(
  gas: bigint,
  gasPrice: bigint,
  priorityFee: bigint,
  limits?: EvmFeeLimits
) {
  const priority = priorityFee + priorityFee / 5n
  const maxFee = gasPrice * 2n + priority
  const amount = gas * maxFee
  const exceeds =
    limits &&
    (maxFee > limits.max_fee_per_gas ||
      priority > limits.max_priority_fee_per_gas ||
      amount > limits.max_transaction_fee)
  return {
    amount,
    warning: exceeds
      ? 'The current gas estimate exceeds the bridge’s fee limits. Wait for lower fees or a governance update; the canister will check again before signing.'
      : undefined
  }
}

export function providerIdentity(url: string): string {
  const host = new URL(url).hostname.toLowerCase().replace(/\.$/, '')
  const families = [
    'alchemy.com',
    'alchemyapi.io',
    'ankr.com',
    'nodereal.io',
    'publicnode.com',
    'bnbchain.org',
    'infura.io',
    'quiknode.pro',
    'quicknode.com',
    'solana.com'
  ]
  const family =
    families.find((f) => host === f || host.endsWith('.' + f)) ?? host
  return family === 'alchemyapi.io'
    ? 'alchemy.com'
    : family === 'quiknode.pro'
      ? 'quicknode.com'
      : family
}
