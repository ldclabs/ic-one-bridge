// the SvelteKit build still imports modules in Node before the SPA is emitted,
// so anything touching `window` at module scope has to guard on this
export const isWindowDefined = typeof window != 'undefined'

// geometry for the Internet Identity popup, centred on the window that opens it
export function popupCenter({
  width,
  height
}: {
  width: number
  height: number
}): string {
  if (!window || !window.top) {
    return ''
  }

  const {
    top: { innerWidth, innerHeight }
  } = window

  const y = innerHeight / 2 + screenY - height / 2
  const x = innerWidth / 2 + screenX - width / 2

  return `width=${width},height=${height},top=${y},left=${x}`
}

export async function copyTextToClipboard(text: string): Promise<boolean> {
  if (navigator.clipboard && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      return false
    }
  }

  // http:// origins other than localhost have no clipboard API
  try {
    const ta = document.createElement('textarea')
    ta.value = text
    ta.setAttribute('readonly', '')
    ta.style.position = 'fixed'
    ta.style.top = '-9999px'
    document.body.appendChild(ta)
    ta.select()
    const ok = document.execCommand('copy')
    document.body.removeChild(ta)
    return ok
  } catch {
    return false
  }
}
