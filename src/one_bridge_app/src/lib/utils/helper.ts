// shorten an id for display; `long` overrides the viewport-based default
export function pruneCanister(canisterId: string, long?: boolean) {
  if (long ?? globalThis.innerWidth >= 640) return canisterId
  return canisterId.slice(0, 7) + '...' + canisterId.slice(-5)
}

export function pruneAddress(id: string, long?: boolean): string {
  if (long ?? globalThis.innerWidth >= 640) {
    return id.length > 27 ? id.slice(0, 13) + '...' + id.slice(-11) : id
  }
  return id.length > 15 ? id.slice(0, 7) + '...' + id.slice(-5) : id
}

export function formatTimeAgo(timestamp: number) {
  const delta = Date.now() - new Date(timestamp).getTime()
  const minutes = Math.max(Math.round(delta / (60 * 1000)), 1)
  if (minutes > 60 * 24) {
    const days = Math.round(minutes / (60 * 24))
    return `${days} days ago`
  } else if (minutes > 60) {
    const hours = Math.round(minutes / 60)
    return `${hours} hours ago`
  }
  return `${minutes} minutes ago`
}
