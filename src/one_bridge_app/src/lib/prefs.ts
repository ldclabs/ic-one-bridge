import { page } from '$app/state'
import type { BridgeCanisterAPI } from '$lib/canisters/bridge.svelte'

// the form remembers the last token and chains; a shared link can preset them
// with ?token=&from=&to=
export function formDefault(
  name: 'Token' | 'From' | 'To',
  fallback: string
): string {
  return (
    page.url.searchParams.get(name.toLowerCase()) ||
    localStorage.getItem('default' + name) ||
    fallback
  )
}

export function rememberForm(token: string, from: string, to: string) {
  localStorage.setItem('defaultToken', token)
  localStorage.setItem('defaultFrom', from)
  localStorage.setItem('defaultTo', to)
}

// the bridge of the remembered or linked token, else the main bridge
export function preferredBridge(
  bridges: BridgeCanisterAPI[]
): BridgeCanisterAPI | null {
  const symbol = formDefault('Token', 'PANDA')
  return bridges.find((b) => b.token?.symbol === symbol) ?? bridges[0] ?? null
}
