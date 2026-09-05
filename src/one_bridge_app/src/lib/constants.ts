const src = globalThis.location?.href || ''

export const IS_LOCAL = src.includes('localhost') || src.includes('127.0.0.1')

export const INTERNET_IDENTITY_CANISTER_ID = 'rdmx6-jaaaa-aaaaa-aaadq-cai' // ic & local
export const BRIDGE_CANISTER_ID = 'dpjyw-raaaa-aaaar-qbxlq-cai' // ic & local
