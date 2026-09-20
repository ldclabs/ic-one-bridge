import type {
  BridgeLog,
  OperationInfo,
  StateInfo
} from '../../declarations/one_bridge_canister/one_bridge_canister.did.js'

export function chainName(target: BridgeLog['from']): string {
  return 'Evm' in target ? target.Evm : 'Sol' in target ? 'SOL' : 'ICP'
}

export function readinessReason(
  state: StateInfo | null,
  chains: string[],
  newBridge = true,
  nativeTransfer = false
): string | null {
  if (!state) return 'Loading bridge status…'
  const runtime = state.runtime[0]
  if (!runtime)
    return newBridge
      ? 'This bridge has not enabled recoverable requests yet. Please wait for its upgrade.'
      : null
  if (newBridge) {
    if (runtime.migration_remaining > 0n)
      return 'Bridge history is being upgraded. Existing funds are retained; new bridges will reopen when migration finishes.'
    if (!runtime.ledger_verified)
      return 'The token ledger is being verified. Please try again once the bridge is ready.'
    if (state.error_rounds >= 42n)
      return 'New bridges are paused while the bridge recovers from provider errors. Recovery runs automatically.'
    if (
      runtime.pending_count + runtime.unresolved_operations >=
      BigInt(runtime.resource_limits.max_pending)
    )
      return 'The bridge queue is full. Existing payments are still being processed.'
  }
  if (chains.some((c) => c !== 'ICP' && c !== 'SOL') && !runtime.keys_ready[0])
    return 'The EVM signing key is not ready. Please wait for initialization.'
  if (chains.includes('SOL')) {
    if (!runtime.keys_ready[1])
      return 'The Solana signing key is not ready. Please wait for initialization.'
    if (!runtime.svm_mint_verified && !(nativeTransfer && !newBridge))
      return 'The Solana token is being verified. Please try again once it is ready.'
  }
  return null
}

export function operationActions(operation: OperationInfo) {
  const phase = operation.phase
  const ownerRecoverable =
    operation.kind === 'deposit' || operation.kind === 'fee withdrawal'
  return {
    resume:
      ownerRecoverable &&
      !('NeedsReview' in phase) &&
      !('Rejected' in phase) &&
      !(operation.kind === 'fee withdrawal' && 'Completed' in phase),
    cancel:
      ownerRecoverable &&
      ('Planning' in phase || 'Prepared' in phase || 'Rejected' in phase) &&
      !operation.reconciliation.length &&
      operation.error[0] !== 'cancelled before execution',
    review:
      'NeedsReview' in phase || operation.kind === 'legacy pending conflict'
  }
}

export function operationStatus(operation: OperationInfo): string {
  if ('NeedsReview' in operation.phase) return 'Governance review required'
  if ('Completed' in operation.phase)
    return operation.kind === 'deposit'
      ? 'Deposit received'
      : 'Payment completed'
  if ('Signed' in operation.phase) return 'Transaction signed'
  if ('Submitted' in operation.phase) return 'Awaiting outcome'
  if ('Rejected' in operation.phase) return 'Not executed'
  if ('Prepared' in operation.phase) return 'Ready to submit'
  return 'Preparing'
}

export function logSettled(log: BridgeLog): boolean {
  // An archived failed/closed task is also terminal; it need not have a payout.
  return log.id.length > 0 || log.finalized_at > 0n || txFinalized(log.to_tx[0])
}

export function txFinalized(tx?: BridgeLog['from_tx']): boolean {
  return tx
    ? 'Icp' in tx
      ? tx.Icp[0]
      : 'Evm' in tx
        ? tx.Evm[0]
        : tx.Sol[0]
    : false
}

export function reconciliationHeld(log: BridgeLog): boolean {
  return log.error.some(
    (error) =>
      error.includes('legacy conflict') ||
      error.includes('already appears in archive') ||
      error.includes('reconcile and close')
  )
}

export function canRecheck(log: BridgeLog): boolean {
  // The canister is authoritative: it rejects any unresolved reconciliation
  // hold even if the public error does not expose its exact cause.
  return !logSettled(log) && !reconciliationHeld(log)
}
