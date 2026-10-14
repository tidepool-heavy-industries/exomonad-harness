/** Call from the copy control's user gesture, including on HTTP operator hosts. */
export async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text)
      return
    } catch {
      // The browser may expose the API but refuse the write.
    }
  }
  copySelectedText(text)
}

function copySelectedText(text: string): void {
  const active = document.activeElement
  const selection = document.getSelection()
  const ranges = selection ? Array.from({ length: selection.rangeCount }, (_, index) => selection.getRangeAt(index).cloneRange()) : []
  const endpoints = selection?.anchorNode && selection.focusNode
    ? { anchor: selection.anchorNode, anchorOffset: selection.anchorOffset, focus: selection.focusNode, focusOffset: selection.focusOffset } : undefined
  const field = active instanceof HTMLTextAreaElement || active instanceof HTMLInputElement ? active : undefined
  const fieldSelection = field && field.selectionStart !== null
    ? { start: field.selectionStart, end: field.selectionEnd!, direction: field.selectionDirection ?? undefined } : undefined
  const textarea = document.createElement('textarea')
  textarea.value = text
  textarea.readOnly = true
  textarea.tabIndex = -1
  textarea.setAttribute('aria-hidden', 'true')
  Object.assign(textarea.style, { position: 'fixed', top: '0', left: '0', width: '1px', height: '1px', opacity: '0', pointerEvents: 'none' })
  // Supply the retained string directly, rather than textarea-normalized line endings.
  const onCopy = (event: ClipboardEvent) => {
    if (!event.clipboardData) return
    event.clipboardData.setData('text/plain', text)
    event.preventDefault()
  }
  document.addEventListener('copy', onCopy)
  document.body.append(textarea)
  try {
    textarea.focus({ preventScroll: true })
    textarea.select()
    // Legacy copy remains the user-gesture fallback where the secure API is absent.
    if (!document.execCommand('copy')) throw new Error('Clipboard copy refused')
  } finally {
    document.removeEventListener('copy', onCopy)
    textarea.remove()
    if (active instanceof HTMLElement && active.isConnected) active.focus({ preventScroll: true })
    if (fieldSelection && field?.isConnected) field.setSelectionRange(fieldSelection.start, fieldSelection.end, fieldSelection.direction)
    if (selection) {
      selection.removeAllRanges()
      if (ranges.length === 1 && endpoints) {
        selection.setBaseAndExtent(endpoints.anchor, endpoints.anchorOffset, endpoints.focus, endpoints.focusOffset)
      } else {
        for (const range of ranges) selection.addRange(range)
      }
    }
  }
}
