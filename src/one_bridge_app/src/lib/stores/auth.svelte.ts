import { INTERNET_IDENTITY_CANISTER_ID, IS_LOCAL } from '$lib/constants'
import {
  anonymousIdentity,
  createAuthClient,
  dynAgent,
  expirationOf,
  EXPIRATION_MS,
  IdentityEx,
  loadIdentity
} from '$lib/utils/auth'
import { popupCenter } from '$lib/utils/window'

// the URL is used verbatim by @icp-sdk/auth, it appends no route of its own,
// which is why it carries `/authorize` and the local one `#authorize`
const IDENTITY_PROVIDER = IS_LOCAL
  ? `http://${INTERNET_IDENTITY_CANISTER_ID}.localhost:4943/#authorize`
  : 'https://id.ai/authorize'

const authClient = createAuthClient()

class AuthStore {
  static async init() {
    // Fetch the root key for local development
    if (IS_LOCAL) {
      await Promise.all([dynAgent.fetchRootKey(), dynAgent.syncTime()])
    }
    const identity = await loadIdentity(authClient)
    if (identity) {
      authStore.#login(identity)
    }
  }

  #identity = $state<IdentityEx>(anonymousIdentity)

  get identity() {
    return this.#identity
  }

  #login(identity: IdentityEx) {
    identity.expiredHook = () => this.logout()
    this.#identity = identity
    dynAgent.setIdentity(identity)
  }

  async signIn(identityProvider = IDENTITY_PROVIDER): Promise<void> {
    // Important: createAuthClient is synchronous so that window.open runs inside
    // the click's call stack
    // https://ffan0811.medium.com/window-open-returns-null-in-safari-and-firefox-after-allowing-pop-up-on-the-browser-4e4e45e7d926
    const signInClient = createAuthClient({
      identityProvider,
      windowOpenerFeatures: popupCenter({
        width: 576,
        height: 625
      })
    })

    const identity = await signInClient.signIn({
      maxTimeToLive: BigInt(EXPIRATION_MS) * 1000000n
    })
    console.log(`Login successful from ${location.origin}`)
    this.#login(new IdentityEx(identity, expirationOf(identity)))
  }

  async logout(url?: string) {
    this.#identity = anonymousIdentity
    dynAgent.setIdentity(anonymousIdentity)
    await authClient.signOut()
    url && window.location.assign(url) // force reload to clear all auth state!!
  }
}

export const authStore = new AuthStore()

AuthStore.init().catch((err) => {
  console.error('Failed to initialize AuthStore', err)
})
