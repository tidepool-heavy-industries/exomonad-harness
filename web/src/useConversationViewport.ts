import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react'
import { hasChatSelection, useChatSelection } from './chat-selection'

interface ScrollAnchor { element: HTMLElement; key: string; offset: number }

/** The parent explicitly supplies its scroll surface; short layouts scroll the page. */
export function useConversationViewport(viewportRef?: RefObject<HTMLElement>) {
  const section = useRef<HTMLElement>(null)
  const pinned = useRef(true)
  const anchor = useRef<ScrollAnchor>()
  const [newActivity, setNewActivity] = useState(false)
  function surface() {
    const element = viewportRef?.current ?? section.current
    return element && getComputedStyle(element).overflowY === 'visible'
      ? document.scrollingElement as HTMLElement | null : element
  }
  function top(element: HTMLElement) {
    return element === document.scrollingElement ? 0 : element.getBoundingClientRect().top
  }
  function preserveAnchor() {
    const element = surface()
    if (!element || !section.current) return
    const boundary = top(element)
    const row = [...section.current.querySelectorAll<HTMLElement>('[data-timeline-key]')].find(row => row.getBoundingClientRect().bottom > boundary)
    anchor.current = row ? { element, key: row.dataset.timelineKey!, offset: row.getBoundingClientRect().top - boundary } : undefined
  }
  function revealRow(key: string) {
    const row = [...(section.current?.querySelectorAll<HTMLElement>('[data-timeline-key]') ?? [])].find(row => row.dataset.timelineKey === key)
    if (!row) return
    pinned.current = false
    row.scrollIntoView({ block: 'start' })
    row.focus({ preventScroll: true })
  }
  function restoreOrFollow() {
    const saved = anchor.current
    if (saved) {
      const row = [...(section.current?.querySelectorAll<HTMLElement>('[data-timeline-key]') ?? [])].find(row => row.dataset.timelineKey === saved.key)
      if (row) saved.element.scrollTop += row.getBoundingClientRect().top - top(saved.element) - saved.offset
      anchor.current = undefined
    } else if (pinned.current && !hasChatSelection(section.current)) {
      const element = surface()
      if (element) element.scrollTop = element.scrollHeight
      setNewActivity(false)
    }
  }
  useEffect(() => {
    const element = viewportRef?.current ?? section.current
    const observe = () => {
      const current = surface()
      if (!current) return
      pinned.current = current.scrollHeight - current.clientHeight - current.scrollTop <= 48
      if (pinned.current) setNewActivity(false)
    }
    element?.addEventListener('scroll', observe, { passive: true })
    window.addEventListener('scroll', observe, { passive: true })
    return () => {
      element?.removeEventListener('scroll', observe)
      window.removeEventListener('scroll', observe)
    }
  }, [viewportRef])
  return { section, pinned, newActivity, preserveAnchor, restoreOrFollow, revealRow,
    notifyActivity: () => setNewActivity(true), clearActivity: () => setNewActivity(false) }
}
export type ConversationViewport = ReturnType<typeof useConversationViewport>

export function useConversationViewportUpdates(viewport: ConversationViewport, entries: unknown, liveVersion: string) {
  const previousLive = useRef('')
  useChatSelection(viewport.section, entries, liveVersion)
  useLayoutEffect(() => { viewport.restoreOrFollow() }, [entries, liveVersion])
  useEffect(() => {
    if (previousLive.current && previousLive.current !== liveVersion && (!viewport.pinned.current || hasChatSelection(viewport.section.current))) viewport.notifyActivity()
    previousLive.current = liveVersion
  }, [liveVersion])
}
