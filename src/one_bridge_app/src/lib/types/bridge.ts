import type { BridgeTx } from '../../declarations/one_bridge_canister/one_bridge_canister.did.js'

export type BridgingStatus =
  | 'Accepted'
  | 'Pending'
  | 'Completed'
  | 'Error'
  | 'Retrying'
  | 'Needs review'
  | 'Needs attention'
  | 'Closed'

// one finalized or in-flight bridge transfer, ready to render
export type BridgeLogInfo = {
  id: bigint
  taskId?: bigint
  fromTransaction: BridgeTx
  canRecheck: boolean
  settled: boolean
  stuck: boolean
  needsReview: boolean
  nextPollAt: number
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
