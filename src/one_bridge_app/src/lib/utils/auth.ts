import { IS_LOCAL } from '$lib/constants'
import { AuthClient, type AuthClientCreateOptions } from '@icp-sdk/auth/client'
import {
  AnonymousIdentity,
  HttpAgent,
  type HttpAgentOptions,
  type HttpAgentRequest,
  type Identity
} from '@icp-sdk/core/agent'
import { DelegationChain, DelegationIdentity } from '@icp-sdk/core/identity'
import type { Principal } from '@icp-sdk/core/principal'
import { isWindowDefined } from './window'

export const EXPIRATION_MS = 1000 * 60 * 60 // 1 hour

export class IdentityEx implements Identity {
  expiredHook: (() => void) | null = null

  constructor(
    public readonly id: Identity,
    public readonly expiration: number, // in milliseconds
    public readonly username: string = '' // this is name identity if username exists
  ) {
    this.id = id
    this.expiration = id.getPrincipal().isAnonymous()
      ? Number.MAX_SAFE_INTEGER
      : expiration
    this.username = username
  }

  get isExpired() {
    return Date.now() >= this.expiration - 1000 * 60 * 5 // 5 minutes before expiration
  }

  get isAuthenticated() {
    return !this.id.getPrincipal().isAnonymous() && !this.isExpired
  }

  getPrincipal(): Principal {
    return this.id.getPrincipal()
  }

  transformRequest(request: HttpAgentRequest): Promise<unknown> {
    if (this.isExpired) {
      if (this.expiredHook) this.expiredHook()
      throw new Error('Identity expired, please sign in again')
    }

    return this.id.transformRequest(request)
  }
}

export const anonymousIdentity = new IdentityEx(new AnonymousIdentity(), 0)

// should create a new authClient for each login: the identity provider and the
// popup geometry are constructor options in @icp-sdk/auth, and the constructor
// is synchronous so `signIn()` can open the window inside the click's call stack
export function createAuthClient(
  options: AuthClientCreateOptions = {}
): AuthClient {
  // the SSR build imports this module in Node, where the default IndexedDB
  // storage has nothing to open, and nothing calls into the client there
  if (!isWindowDefined) {
    return {} as AuthClient
  }

  return new AuthClient({
    keyType: 'Ed25519',
    ...options,
    idleOptions: {
      disableIdle: true,
      disableDefaultIdleCallback: true,
      ...options.idleOptions
    }
  })
}

export async function loadIdentity(
  client?: AuthClient
): Promise<IdentityEx | null> {
  const authClient = client || createAuthClient()

  // Not authenticated therefore we provide no identity as a result
  if (!authClient.isAuthenticated?.()) {
    return null
  }

  const identity = await authClient.getIdentity()
  return new IdentityEx(identity, expirationOf(identity))
}

// II may grant less than EXPIRATION_MS, never more, so the expiration is read
// from the delegation chain instead of being assumed
export function expirationOf(identity: Identity): number {
  const chain =
    identity instanceof DelegationIdentity ? identity.getDelegation() : null
  return chain ? getDelegationExpiration(chain) : Date.now() + EXPIRATION_MS
}

function getDelegationExpiration(chain: DelegationChain): number {
  let expiration = Date.now() + EXPIRATION_MS
  for (const { delegation } of chain.delegations) {
    // prettier-ignore
    const ex = Number(delegation.expiration / BigInt(1000000))
    if (ex < expiration) {
      expiration = ex
    }
  }
  return expiration
}

export class AuthAgent extends HttpAgent {
  private _id: Identity
  constructor(options: { identity: Identity } & HttpAgentOptions) {
    super(options)
    this._id = options.identity
  }

  get id() {
    return this._id
  }

  isAnonymous() {
    return this._id.getPrincipal().isAnonymous()
  }

  setIdentity(id: Identity) {
    this._id = id
    super.replaceIdentity(id)
  }
}

export function createAgent(identity: Identity): AuthAgent {
  return new AuthAgent({
    identity,
    host: IS_LOCAL ? 'http://localhost:4943/' : 'https://icp-api.io',
    verifyQuerySignatures: false,
    shouldFetchRootKey: IS_LOCAL
  })
}

export const dynAgent = createAgent(anonymousIdentity)
export const anonAgent = new AuthAgent({
  identity: anonymousIdentity,
  host: 'https://icp-api.io',
  verifyQuerySignatures: false,
  shouldFetchRootKey: IS_LOCAL
})
