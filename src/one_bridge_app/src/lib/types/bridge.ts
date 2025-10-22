export type Chain = {
  id: number // EVM chain ID
  name: string
  fullName: string
  nativeToken: string
  explorerUrl: string
  logo: string
  averageFinalitySeconds: number
}

export type BridgingStatus = 'Accepted' | 'Pending' | 'Completed' | 'Error'

export type BridgeLogInfo = {
  id: bigint
  user: string
  token: string
  from: string
  to: string
  amount: string
  fee: string
  fromTx: string
  fromTxUrl: string
  toTx?: string
  toTxUrl?: string
  toAddr?: string
  createdAt: number
  finalizedAt: number
  status: BridgingStatus
  error?: string
}
