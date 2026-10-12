import { useEffect, useLayoutEffect, useRef, type RefObject } from 'react'

interface Endpoint { row: string; body: number; offset: number }
interface SavedSelection { anchor: Endpoint; focus: Endpoint; text: string }
export function hasChatSelection(section: HTMLElement | null): boolean {
  const selection = window.getSelection()
  return !!section && !!selection && !selection.isCollapsed && !!selection.anchorNode && section.contains(selection.anchorNode)
}

/** Restore an unchanged selection when streamed text nodes or preview views are replaced. */
export function useChatSelection(section: RefObject<HTMLElement>, entries: unknown, liveVersion: string) {
  const saved = useRef<SavedSelection>()
  useEffect(() => {
    function endpoint(node: Node | null, offset: number): Endpoint | undefined {
      const element = node?.nodeType === Node.ELEMENT_NODE ? node as Element : node?.parentElement
      const body = element?.closest('.message-content-body')
      const row = body?.closest<HTMLElement>('[data-timeline-key]')
      if (!node || !body || !row || !section.current?.contains(row)) return
      const range = document.createRange()
      range.selectNodeContents(body); range.setEnd(node, offset)
      return { row: row.dataset.timelineKey!, body: [...row.querySelectorAll('.message-content-body')].indexOf(body), offset: range.toString().length }
    }
    const capture = () => {
      const selection = window.getSelection()
      const anchor = endpoint(selection?.anchorNode ?? null, selection?.anchorOffset ?? 0)
      const focus = endpoint(selection?.focusNode ?? null, selection?.focusOffset ?? 0)
      saved.current = selection && !selection.isCollapsed && anchor && focus ? { anchor, focus, text: selection.toString() } : undefined
    }
    document.addEventListener('selectionchange', capture)
    return () => document.removeEventListener('selectionchange', capture)
  }, [section])
  useLayoutEffect(() => {
    const value = saved.current
    if (!value) return
    function locate(endpoint: Endpoint): [Node, number] | undefined {
      const row = [...(section.current?.querySelectorAll<HTMLElement>('[data-timeline-key]') ?? [])].find(row => row.dataset.timelineKey === endpoint.row)
      const body = row?.querySelectorAll('.message-content-body')[endpoint.body]
      if (!body) return
      const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT)
      let remaining = endpoint.offset
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        const length = node.textContent?.length ?? 0
        if (remaining <= length) return [node, remaining]
        remaining -= length
      }
    }
    const anchor = locate(value.anchor), focus = locate(value.focus)
    if (!anchor || !focus) { saved.current = undefined; return }
    const selection = window.getSelection()
    if (!selection) return
    const candidate = document.createRange()
    candidate.setStart(...anchor); candidate.setEnd(...focus)
    if (candidate.collapsed) { candidate.setStart(...focus); candidate.setEnd(...anchor) }
    // Do not silently select different content after a non-append final revision.
    if (candidate.toString() !== value.text) { saved.current = undefined; return }
    selection.setBaseAndExtent(...anchor, ...focus)
  }, [section, entries, liveVersion])
}
