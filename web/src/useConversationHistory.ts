import { useEffect, useRef, useState } from 'react'
import { actorOutputKey, type ActorOutputOrigin } from './actor-output'
import type { StoredActorForm } from './form-model'
import { mergeEntries, boundEntries } from './chat-timeline'
import { readUnifiedPage, type UnifiedPage, type UnifiedEntry } from './conversation-client'
import type { ConversationViewport } from './useConversationViewport'

export interface ConversationHistoryOptions {
  origin: ActorOutputOrigin
  requestId?: string
  revision?: string
  ready: boolean
  active: boolean
  onAuthExpired?: () => void
}
type HistoryViewport = Pick<ConversationViewport, 'pinned' | 'preserveAnchor' | 'notifyActivity' | 'clearActivity'>

/** Owns retained data only. Viewport policy is supplied by the mounted chat. */
export function useConversationHistory({ origin, requestId, revision, ready, active, onAuthExpired }: ConversationHistoryOptions, { pinned, preserveAnchor, notifyActivity, clearActivity }: HistoryViewport) {
  const [page, setPage] = useState<UnifiedPage>()
  const [entries, setEntries] = useState<readonly UnifiedEntry[]>([])
  const entriesRef = useRef(entries)
  entriesRef.current = entries
  const [older, setOlder] = useState<string | null>(null)
  const [issue, setIssue] = useState('')
  const [attempt, setAttempt] = useState(0)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [windowed, setWindowed] = useState(false)
  const controllerOlder = useRef<AbortController>()
  const generation = useRef(0)
  const replaceLatest = useRef(false)
  const latestSequence = useRef('0')
  const identity = actorOutputKey(origin)
  function updateForm(form: StoredActorForm) {
    setEntries(current => mergeEntries(current, [{ kind: 'form', sequence: form.sequence, form }]))
  }
  useEffect(() => {
    if (!ready) return
    const controller = new AbortController()
    const currentGeneration = ++generation.current
    controllerOlder.current?.abort()
    setLoadingOlder(false)
    let timer: ReturnType<typeof setTimeout> | undefined
    const load = async () => {
      try {
        const next = await readUnifiedPage(origin, requestId, undefined, controller.signal, onAuthExpired)
        if (!controller.signal.aborted && currentGeneration === generation.current) {
          const newest = next.entries.at(-1)?.sequence ?? '0'
          if (latestSequence.current !== '0' && BigInt(newest) > BigInt(latestSequence.current) && !pinned.current) notifyActivity()
          latestSequence.current = newest
          if (!pinned.current) preserveAnchor()
          setPage(next)
          const current = entriesRef.current
          const overlap = next.entries.some(entry => current.some(old => old.sequence === entry.sequence))
          const newerWindow = current.length > 0 && next.entries.length > 0 && !overlap && BigInt(next.entries[0]!.sequence) > BigInt(current.at(-1)!.sequence)
          if (replaceLatest.current) { replaceLatest.current = false; setEntries(mergeEntries(current.filter(entry => next.entries.some(nextEntry => nextEntry.sequence === entry.sequence)), next.entries)); setOlder(next.previousBefore ?? null); setWindowed(false) }
          else if (newerWindow && !pinned.current) { setWindowed(true); notifyActivity() }
          else if (newerWindow) { setEntries(next.entries); setOlder(next.previousBefore ?? null); setWindowed(false) }
          else {
            const incoming = !pinned.current && current.length ? next.entries.filter(entry => current.some(old => old.sequence === entry.sequence)) : next.entries
            const merged = mergeEntries(current, incoming)
            const bounded = boundEntries(merged, 'latest')
            if (bounded.trimmed) { setWindowed(true); setOlder(bounded.entries[0]!.sequence) }
            else if (!current.length) setOlder(next.previousBefore ?? null)
            setEntries(bounded.entries)
          }
          setIssue('')
        }
      } catch (error) { if (!controller.signal.aborted) setIssue(error instanceof Error ? error.message : String(error)) }
      finally { if (!controller.signal.aborted && active) timer = setTimeout(load, 2000) }
    }
    void load()
    return () => { controller.abort(); controllerOlder.current?.abort(); clearTimeout(timer) }
  }, [identity, requestId, revision, ready, active, attempt, onAuthExpired])
  async function loadOlder() {
    if (older === null || loadingOlder || !ready) return
    const controller = new AbortController()
    controllerOlder.current = controller
    const currentGeneration = generation.current
    const cursor = older
    setLoadingOlder(true); setIssue('')
    try {
      const next = await readUnifiedPage(origin, requestId, cursor, controller.signal, onAuthExpired)
      if (controller.signal.aborted || currentGeneration !== generation.current) return
      pinned.current = false
      preserveAnchor()
      const merged = mergeEntries(entriesRef.current, next.entries)
      const bounded = boundEntries(merged, 'oldest')
      if (bounded.trimmed) setWindowed(true)
      setEntries(bounded.entries)
      setOlder(next.previousBefore ?? null)
    } catch (error) { if (!controller.signal.aborted) setIssue(error instanceof Error ? error.message : String(error)) }
    finally { if (!controller.signal.aborted) setLoadingOlder(false) }
  }
  function returnToLatest() { replaceLatest.current = true; pinned.current = true; clearActivity(); setAttempt(value => value + 1) }
  return { page, entries, older, issue, loadingOlder, windowed, updateForm, loadOlder, returnToLatest, refresh: () => setAttempt(value => value + 1) }
}
