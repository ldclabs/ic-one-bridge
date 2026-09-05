export type BridgingStatus = 'Accepted' | 'Pending' | 'Completed' | 'Error'

// one finalized or in-flight bridge transfer, ready to render
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
