export type Chain = {
  id: number // EVM chain ID
  name: string
  fullName: string
  nativeToken: string
  explorerUrl: string
  logo: string
  averageFinalitySeconds: number
}

export type BridgeLogInfo = {
  id: bigint
  user: string
  token: string
  from: string
  to: string
  amount: number
  fee: number
  from_tx: string
  to_tx?: string
  to_addr?: string
  created_at: number
  finalized_at: number
  status: 'pending' | 'success' | 'failed'
  error?: string
}
