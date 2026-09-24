// Runs `fn` and reports its error, unless the returned abort was called first:
// a run its caller gave up on has nothing left to report. `fn` checks the
// signal after each await before writing any result.
export function tryRun(
  fn: (signal: AbortSignal) => unknown,
  onerror: (err: unknown) => void
): () => void {
  const controller = new AbortController()
  ;(async () => fn(controller.signal))().catch((err) => {
    if (!controller.signal.aborted) onerror(err)
  })
  return () => controller.abort()
}

export function errMessage(err: any): string {
  if (typeof err?.data === 'string') return readableError(err.data)
  if (err?.data) {
    return JSON.stringify(err.data, (key, value) =>
      typeof value === 'bigint' ? value.toString() : value
    )
  }
  if (err?.message) {
    return readableError(err.message)
  }
  return String(err)
}

function readableError(message: string): string {
  if (message.includes('hourly request budget exhausted')) {
    const reset = new Date((Math.floor(Date.now() / 3_600_000) + 1) * 3_600_000)
    return `The hourly request allowance has been reached. Try again after ${reset.toLocaleTimeString()} (your local time). Existing operations remain recorded; you can continue them after the reset.`
  }
  if (message.includes('public work is paused to preserve cycles'))
    return 'New signing requests are paused to preserve resources for pending payments. Existing payments remain recorded; please try again later.'
  if (message.includes('too many active requests'))
    return 'The bridge is handling the maximum number of requests. Please wait, then continue your saved request.'
  return message
}
