const locale = new Intl.Locale(globalThis?.navigator?.language || 'en')

export interface TokenInfo {
  name: string
  symbol: string
  decimals: number
  fee: bigint
  logo: string // base64 encoded
  canisterId: string
}

/**
 * Formats and parses the amounts of one token.
 *
 * Amounts are bigint ulps everywhere else in the app — 10^decimals per whole
 * token — and this is the only place that turns them into text or back, so
 * rounding and the viewer's locale are decided once.
 */
export class TokenDisplay {
  readonly #decimals: number
  readonly #one: number
  readonly #formatter: Intl.NumberFormat

  constructor(decimals: number) {
    this.#decimals = decimals
    this.#one = Number(10n ** BigInt(decimals))
    this.#formatter = new Intl.NumberFormat(locale, {
      minimumFractionDigits: 1,
      maximumFractionDigits: decimals,
      // never round up: a displayed balance must not exceed the real one
      roundingMode: 'floor'
    } as Intl.NumberFormatOptions)
  }

  displayValue(ulps: bigint): string {
    return this.#formatter.format(Number(ulps) / this.#one)
  }

  /**
   * Accepts `1234567.8901`, `1'234'567.8901` and `1,234,567.8901`.
   *
   * Throws on anything else rather than returning a number: this converts what
   * the user typed into the amount that will be signed, so a silent 0 or a
   * silently truncated value is the one outcome worth avoiding.
   */
  parseAmount(amount: string | number): bigint {
    const str =
      typeof amount === 'number' ? amount.toFixed(this.#decimals) : amount
    const clean = str.trim().replace(/[,']/g, '')
    if (!/^\d*(\.\d*)?$/.test(clean)) {
      throw new Error(`Invalid amount: ${str}`)
    }

    const [integral, fractional] = clean.split('.')
    if (fractional && fractional.length > this.#decimals) {
      throw new Error(`Amount ${str} has more than ${this.#decimals} decimals`)
    }

    let ulps = integral ? BigInt(integral) * 10n ** BigInt(this.#decimals) : 0n
    if (fractional) {
      ulps += BigInt(fractional.padEnd(this.#decimals, '0'))
    }
    return ulps
  }
}

// native tokens are keyed by decimals, so the handful of Intl.NumberFormat
// instances are built once instead of on every render
const displays = new Map<number, TokenDisplay>()

export function tokenDisplay(decimals: number): TokenDisplay {
  let display = displays.get(decimals)
  if (!display) {
    display = new TokenDisplay(decimals)
    displays.set(decimals, display)
  }
  return display
}
