// Only public request parameters are stored. Never store identities or signed bytes.
export interface BridgeRequest {
  version: 1
  id: string
  owner: string
  canister: string
  from: string
  to: string
  amount: string
  recipient: string
  createdAt: number
  stage: 'prepared' | 'submitted' | 'accepted'
}

export type BridgeIntent = Pick<
  BridgeRequest,
  'from' | 'to' | 'amount' | 'recipient'
>
export type RequestStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

export function requestKey(canister: string, owner: string): string {
  return `one-bridge:request:v1:${canister}:${owner}`
}

export function readRequest(
  storage: RequestStorage,
  canister: string,
  owner: string
): BridgeRequest | null {
  const raw = storage.getItem(requestKey(canister, owner))
  if (!raw) return null
  const value = JSON.parse(raw) as BridgeRequest
  if (
    value.version !== 1 ||
    value.canister !== canister ||
    value.owner !== owner ||
    !/^[a-f0-9]{64}$/.test(value.id) ||
    !/^\d+$/.test(value.amount) ||
    BigInt(value.amount) <= 0n ||
    typeof value.from !== 'string' ||
    typeof value.to !== 'string' ||
    typeof value.recipient !== 'string' ||
    !Number.isFinite(value.createdAt) ||
    !['prepared', 'submitted', 'accepted'].includes(value.stage)
  ) {
    throw new Error(
      'The saved bridge request cannot be read. Keep this browser data and contact support before submitting again.'
    )
  }
  return value
}

export function saveRequest(
  storage: RequestStorage,
  request: BridgeRequest
): void {
  // A failed write must stop submission: retry safety depends on retaining the ID.
  storage.setItem(
    requestKey(request.canister, request.owner),
    JSON.stringify(request)
  )
}

export function prepareRequest(
  storage: RequestStorage,
  canister: string,
  owner: string,
  intent: BridgeIntent
): BridgeRequest {
  const existing = readRequest(storage, canister, owner)
  if (existing) {
    throw new Error(
      'Review the saved bridge request before starting a new one.'
    )
  }
  const bytes = crypto.getRandomValues(new Uint8Array(32))
  const request: BridgeRequest = {
    version: 1,
    id: Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join(''),
    owner,
    canister,
    ...intent,
    createdAt: Date.now(),
    stage: 'prepared'
  }
  saveRequest(storage, request)
  return request
}

export function requestBytes(request: BridgeRequest): Uint8Array {
  return Uint8Array.from(request.id.match(/../g)!, (byte) => parseInt(byte, 16))
}

export function forgetRequest(
  storage: RequestStorage,
  request: BridgeRequest
): void {
  const current = readRequest(storage, request.canister, request.owner)
  if (current?.id === request.id)
    storage.removeItem(requestKey(request.canister, request.owner))
}

// The same immutable request survives both explicit errors and lost responses.
// A submitted retry bypasses preparation (balance reads and ledger approval).
export async function submitRequest<T>(
  storage: RequestStorage,
  request: BridgeRequest,
  callbacks: {
    assertOwner: () => void
    prepare: () => Promise<void>
    send: (request: BridgeRequest, id: Uint8Array) => Promise<T>
  }
): Promise<T> {
  callbacks.assertOwner()
  const current = readRequest(storage, request.canister, request.owner)
  if (!current || current.id !== request.id)
    throw new Error(
      'The saved request changed in another tab. Refresh before continuing.'
    )
  if (current.stage === 'prepared') {
    await callbacks.prepare()
    callbacks.assertOwner()
    saveRequest(storage, { ...current, stage: 'submitted' })
  }
  callbacks.assertOwner()
  const result = await callbacks.send(current, requestBytes(current))
  // Another account may be active now, but this receipt still belongs to the
  // original owner. Never overwrite a new request or expose it to the new UI.
  const latest = readRequest(storage, current.canister, current.owner)
  if (latest?.id === current.id)
    saveRequest(storage, { ...current, stage: 'accepted' })
  callbacks.assertOwner()
  return result
}

export async function withRequestLock<T>(
  canister: string,
  owner: string,
  work: () => T | Promise<T>
): Promise<T> {
  if (typeof navigator === 'undefined' || !navigator.locks) {
    throw new Error(
      'This browser cannot safely coordinate bridge requests across tabs. Use a current browser on HTTPS or localhost.'
    )
  }
  return navigator.locks.request(`bridge-request:${canister}:${owner}`, work)
}
