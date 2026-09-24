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
  readonly #one: bigint
  readonly #decimal: string
  readonly #formatter: Intl.NumberFormat

  constructor(decimals: number) {
    this.#decimals = decimals
    this.#one = 10n ** BigInt(decimals)
    this.#decimal =
      new Intl.NumberFormat(locale)
        .formatToParts(1.1)
        .find((part) => part.type === 'decimal')?.value ?? '.'
    this.#formatter = new Intl.NumberFormat(locale, {
      minimumFractionDigits: 0,
      maximumFractionDigits: 0,
      // never round up: a displayed balance must not exceed the real one
      roundingMode: 'floor'
    } as Intl.NumberFormatOptions)
  }

  displayValue(ulps: bigint): string {
    const negative = ulps < 0n
    const absolute = negative ? -ulps : ulps
    const integral = this.#formatter.format(absolute / this.#one)
    const fraction =
      (absolute % this.#one)
        .toString()
        .padStart(this.#decimals, '0')
        .replace(/0+$/, '') || '0'
    return `${negative ? '-' : ''}${integral}${this.#decimals ? this.#decimal + fraction : ''}`
  }

  /**
   * Accepts `1234567.8901`, `1'234'567.8901` and `1,234,567.8901`.
   *
   * Throws on anything else rather than returning a number: this converts what
   * the user typed into the amount that will be signed, so a silent 0 or a
   * silently truncated value is the one outcome worth avoiding.
   */
  parseAmount(amount: string): bigint {
    const clean = amount.trim().replace(/[,']/g, '')
    if (!/^\d*(\.\d*)?$/.test(clean)) {
      throw new Error(`Invalid amount: ${amount}`)
    }

    const [integral, fractional] = clean.split('.')
    if (fractional && fractional.length > this.#decimals) {
      throw new Error(
        `Amount ${amount} has more than ${this.#decimals} decimals`
      )
    }

    let ulps = integral ? BigInt(integral) * 10n ** BigInt(this.#decimals) : 0n
    if (fractional) {
      ulps += BigInt(fractional.padEnd(this.#decimals, '0'))
    }
    return ulps
  }
}

// keyed by decimals, so the handful of Intl.NumberFormat instances are built
// once instead of on every render or state refresh
const displays = new Map<number, TokenDisplay>()

export function tokenDisplay(decimals: number): TokenDisplay {
  let display = displays.get(decimals)
  if (!display) {
    display = new TokenDisplay(decimals)
    displays.set(decimals, display)
  }
  return display
}
