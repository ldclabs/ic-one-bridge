export interface TryRunResult<T> {
  controller: AbortController
  abort: () => void
  finally(onfinally?: (res: T | null) => any): Promise<any>
}

export function tryRun<T>(
  fn: (signal: AbortSignal, abortingQue: (() => void)[]) => T | Promise<T>,
  onerror?: (err: any) => void
): TryRunResult<T> {
  const controller = new AbortController()
  const abortingQue: (() => void)[] = []
  const rt = (async () => {
    try {
      return await fn(controller.signal, abortingQue)
    } catch (err: any) {
      if (controller.signal.aborted) return null
      if (onerror) {
        onerror(err)
      } else {
        console.error(err)
      }
      return null
    }
  })()

  return {
    controller,
    abort: (reason = 'tryRun aborted') => {
      controller.abort(reason)
      abortingQue.forEach((aborting) => aborting())
    },
    finally: (onfinally) => rt.then((res) => (onfinally ? onfinally(res) : res))
  }
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
