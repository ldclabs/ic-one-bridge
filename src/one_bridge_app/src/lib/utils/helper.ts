import { Principal } from '@dfinity/principal'

export function shortAddress(id: string, long: boolean = false): string {
  if (long) {
    return id.length > 25 ? id.slice(0, 11) + '...' + id.slice(-11) : id
  }
  return id.length > 15 ? id.slice(0, 6) + '...' + id.slice(-6) : id
}

export function validateAddress(chain: string, address: string): boolean {
  switch (chain) {
    case 'ICP':
      try {
        Principal.fromText(address)
        return true
      } catch (_) {}
      return false
    default:
      return /^0x[a-fA-F0-9]{40}$/.test(address)
  }
}
