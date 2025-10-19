const src = globalThis.location?.href || ''

export const APP_VERSION = '0.1.0'
export const IS_LOCAL = src.includes('localhost') || src.includes('127.0.0.1')
export const ENV = IS_LOCAL ? 'local' : 'ic'
export const APP_ORIGIN = IS_LOCAL
  ? 'http://2fvu6-tqaaa-aaaap-akksa-cai.localhost:4943'
  : 'https://dmsg.net'

export const INTERNET_IDENTITY_CANISTER_ID = 'rdmx6-jaaaa-aaaaa-aaadq-cai' // ic & local
export const PANDA_BRIDGE_CANISTER_ID = 'dpjyw-raaaa-aaaar-qbxlq-cai' // ic & local
