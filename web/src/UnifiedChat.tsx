import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { actorOutputKey, isActorOutputOrigin, isStoredActorOutput, type ActorOutputOrigin, type StoredActorOutput } from './actor-output'
import { isStoredActorForm, type StoredActorForm } from './MountedForm'
import HistoryItem from './HistoryItem'
import type { HistoryEntry } from './history-client'
import MountedForm from './MountedForm'
import { outputKey, outputLabels, type LiveOutput, type OutputOrigin } from './live-output'
import { isChatTool, projectTimeline, mergeEntries, boundEntries, type ChatTool, type TimelineRow } from './chat-timeline'
import './UnifiedChat.css'
import { RichViewRenderer } from './rich-view'
import type { HarnessViewModel } from './view-model'

export type UnifiedEntry =
  | { readonly sequence: number; readonly kind: 'message'; readonly requestId: string; readonly position: number; readonly hash: string; readonly item: unknown; readonly tool?: ChatTool | null }
  | { readonly sequence: number; readonly kind: 'oversized'; readonly requestId: string; readonly position: number; readonly hash: string; readonly byteLen: number }
  | { readonly sequence: number; readonly kind: 'output'; readonly output: StoredActorOutput }
  | { readonly sequence: number; readonly kind: 'form'; readonly form: StoredActorForm }
export interface UnifiedPage {
  readonly origin: ActorOutputOrigin; readonly cutoverSequence: number; readonly legacyHistory: boolean
  readonly entries: readonly UnifiedEntry[]; readonly nextAfter: number | null; readonly previousBefore?: number | null
}
const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)
const nat = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0

export function decodeUnifiedPage(value: unknown, origin: ActorOutputOrigin, cursor: number | { before?: number } = {}): UnifiedPage {
  const after = typeof cursor === 'number' ? cursor : 0
  const backward = typeof cursor !== 'number'
  if (!object(value) || !isActorOutputOrigin(value.origin) || actorOutputKey(value.origin) !== actorOutputKey(origin)
    || !nat(value.cutoverSequence) || typeof value.legacyHistory !== 'boolean' || !Array.isArray(value.entries) || value.entries.length > 100
    || !(value.nextAfter === null || nat(value.nextAfter) && value.nextAfter > after)
    || (backward && !(value.previousBefore === null || nat(value.previousBefore) && value.previousBefore > 0))) throw new Error('Unified conversation returned an invalid page.')
  let last = after
  for (const entry of value.entries) {
    if (!object(entry) || !nat(entry.sequence) || entry.sequence <= last || (typeof cursor !== 'number' && cursor.before !== undefined && entry.sequence >= cursor.before)) throw new Error('Unified conversation entries are out of sequence.')
    last = entry.sequence
    if (entry.kind === 'message') {
      if (typeof entry.requestId !== 'string' || !nat(entry.position) || typeof entry.hash !== 'string' || !/^[0-9a-f]{64}$/.test(entry.hash) || !Object.hasOwn(entry, 'item')) throw new Error('Unified conversation contains an invalid message.')
      if (entry.tool != null && (!isChatTool(entry.tool) || !object(entry.item) || entry.item.call_id !== entry.tool.operation.call)) throw new Error('Unified conversation contains invalid tool ownership.')
      if (entry.tool != null && object(entry.item) && ['function_call', 'custom_tool_call'].includes(String(entry.item.type))
        && (entry.tool.invocation.request !== entry.requestId || entry.tool.invocation.position !== entry.position || entry.tool.invocation.hash !== entry.hash)) throw new Error('Unified conversation invocation does not match its issuing occurrence.')
    } else if (entry.kind === 'oversized') {
      if (typeof entry.requestId !== 'string' || !nat(entry.position) || typeof entry.hash !== 'string' || !/^[0-9a-f]{64}$/.test(entry.hash) || !nat(entry.byteLen)) throw new Error('Unified conversation contains an invalid oversized message.')
    } else if (entry.kind === 'output') {
      if (!isStoredActorOutput(entry.output) || actorOutputKey(entry.output.reference.origin) !== actorOutputKey(origin)) throw new Error('Unified conversation contains invalid output.')
    } else if (entry.kind === 'form') {
      if (!isStoredActorForm(entry.form) || actorOutputKey(entry.form.opening.origin) !== actorOutputKey(origin) || entry.form.sequence !== entry.sequence) throw new Error('Unified conversation contains an invalid form.')
    } else throw new Error('Unified conversation contains an unknown entry.')
  }
  if (backward && value.previousBefore !== null && value.previousBefore !== (value.entries[0] as Record<string, unknown> | undefined)?.sequence) throw new Error('Unified conversation cursor does not match its first entry.')
  if (value.nextAfter !== null && value.nextAfter !== last) throw new Error('Unified conversation cursor does not match its last entry.')
  return value as unknown as UnifiedPage
}

async function readUnifiedPage(origin: ActorOutputOrigin, requestId: string | undefined, before: number | undefined, signal: AbortSignal, onAuthExpired?: () => void) {
  const query = new URLSearchParams({ run: origin.run, actor: String(origin.nativeActor), incarnation: String(origin.incarnation), direction: 'backward', limit: '50' })
  if (before !== undefined) query.set('before', String(before))
  if (requestId) query.set('request', requestId)
  const response = await fetch(`/api/chat?${query}`, { credentials: 'same-origin', cache: 'no-store', signal })
  if (response.status === 401 || response.status === 403) onAuthExpired?.()
  if (!response.ok) throw new Error(response.status === 404 ? 'Unified conversation history is unavailable.' : `Conversation history failed (${response.status}).`)
  const raw = await response.text()
  if (new TextEncoder().encode(raw).length > 256 * 1024) throw new Error('Unified conversation exceeded the page limit.')
  return decodeUnifiedPage(JSON.parse(raw) as unknown, origin, { before })
}

interface ScrollAnchor { element: HTMLElement; key: string; offset: number }
function captureAnchor(section: HTMLElement | null): ScrollAnchor | undefined {
  const element = section?.closest<HTMLElement>('.worker-chat-scroll')
  if (!element) return undefined
  const top = element.getBoundingClientRect().top
  const row = [...section!.querySelectorAll<HTMLElement>('[data-timeline-key]')].find(item => item.getBoundingClientRect().bottom > top)
  return row ? { element, key: row.dataset.timelineKey!, offset: row.getBoundingClientRect().top - top } : undefined
}
function restoreAnchor(section: HTMLElement | null, anchor: ScrollAnchor | undefined) {
  if (!section || !anchor) return
  const row = [...section.querySelectorAll<HTMLElement>('[data-timeline-key]')].find(item => item.dataset.timelineKey === anchor.key)
  if (row) anchor.element.scrollTop += row.getBoundingClientRect().top - anchor.element.getBoundingClientRect().top - anchor.offset
}

export interface UnifiedChatProps {
  origin: ActorOutputOrigin; requestId?: string; revision?: number; ready: boolean; active: boolean; onAuthExpired?: () => void; legacy?: ReactNode
  liveOutput?: readonly LiveOutput[]; modelOrigin?: OutputOrigin; requests?: ReadonlyMap<string, Pick<HarnessViewModel['timeline'][number], 'id' | 'state' | 'detail' | 'failure'>>
}
export default function UnifiedChat(props: UnifiedChatProps) { return <UnifiedChatView key={actorOutputKey(props.origin)} {...props} /> }
function UnifiedChatView({ origin, requestId, revision, ready, active, onAuthExpired, legacy, liveOutput = [], requests, modelOrigin }: UnifiedChatProps) {
  const [page, setPage] = useState<UnifiedPage>()
  const [entries, setEntries] = useState<readonly UnifiedEntry[]>([])
  const entriesRef = useRef(entries)
  entriesRef.current = entries
  const [older, setOlder] = useState<number | null>(null)
  const [issue, setIssue] = useState('')
  const [attempt, setAttempt] = useState(0)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [newActivity, setNewActivity] = useState(false)
  const [windowed, setWindowed] = useState(false)
  const section = useRef<HTMLElement>(null)
  const pinned = useRef(true)
  const anchor = useRef<ScrollAnchor>()
  const controllerOlder = useRef<AbortController>()
  const generation = useRef(0)
  const replaceLatest = useRef(false)
  const latestSequence = useRef(0)
  const previousLive = useRef('')
  const identity = actorOutputKey(origin)
  const rows = projectTimeline(origin, entries, liveOutput, modelOrigin)
  const liveVersion = JSON.stringify(liveOutput.map(item => [outputKey(item), item.version, item.streaming]))
  function revealLatest() {
    const element = section.current?.closest<HTMLElement>('.worker-chat-scroll')
    if (element) element.scrollTop = element.scrollHeight
    pinned.current = true
    setNewActivity(false)
  }
  useEffect(() => {
    const element = section.current?.closest<HTMLElement>('.worker-chat-scroll')
    if (!element) return
    const observe = () => { pinned.current = element.scrollHeight - element.clientHeight - element.scrollTop <= 48; if (pinned.current) setNewActivity(false) }
    element.addEventListener('scroll', observe, { passive: true })
    return () => element.removeEventListener('scroll', observe)
  }, [identity])
  useLayoutEffect(() => {
    if (anchor.current) { restoreAnchor(section.current, anchor.current); anchor.current = undefined }
    else if (pinned.current) revealLatest()
  }, [entries, liveVersion])
  useEffect(() => {
    if (previousLive.current && previousLive.current !== liveVersion && !pinned.current) setNewActivity(true)
    previousLive.current = liveVersion
  }, [liveVersion])
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
          const newest = next.entries.at(-1)?.sequence ?? 0
          if (latestSequence.current && newest > latestSequence.current && !pinned.current) setNewActivity(true)
          latestSequence.current = newest
          if (!pinned.current) anchor.current = captureAnchor(section.current)
          setPage(next)
          const current = entriesRef.current
          const overlap = next.entries.some(entry => current.some(old => old.sequence === entry.sequence))
          const newerWindow = current.length > 0 && next.entries.length > 0 && !overlap && next.entries[0]!.sequence > current.at(-1)!.sequence
          if (replaceLatest.current) { replaceLatest.current = false; setEntries(next.entries); setOlder(next.previousBefore ?? null); setWindowed(false) }
          else if (newerWindow && !pinned.current) { setWindowed(true); setNewActivity(true) }
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
      anchor.current = captureAnchor(section.current)
      const merged = mergeEntries(entriesRef.current, next.entries)
      const bounded = boundEntries(merged, 'oldest')
      if (bounded.trimmed) setWindowed(true)
      setEntries(bounded.entries)
      setOlder(next.previousBefore ?? null)
    } catch (error) { if (!controller.signal.aborted) setIssue(error instanceof Error ? error.message : String(error)) }
    finally { if (!controller.signal.aborted) setLoadingOlder(false) }
  }
  function returnToLatest() { replaceLatest.current = true; pinned.current = true; setNewActivity(false); setAttempt(value => value + 1) }
  const failedRequests = [...(requests?.values() ?? [])].filter(item => item.state === 'failed'
    && entries.some(entry => entry.kind === 'message' && entry.requestId === item.id))
  return <section ref={section} className="unified-chat" aria-label="Conversation" aria-busy={ready && !page}>
    <div className="toolbar"><h2>Conversation</h2><button type="button" disabled={!ready} onClick={() => setAttempt(value => value + 1)}>Refresh conversation</button></div>
    {!ready && <p role="status">Host unavailable; retained conversation entries remain visible.</p>}
    {issue && <div role="alert"><p>{issue}</p><button type="button" disabled={!ready} onClick={() => setAttempt(value => value + 1)}>Retry conversation</button></div>}
    {older !== null && <button type="button" className="load-older" disabled={!ready || loadingOlder} onClick={() => void loadOlder()}>{loadingOlder ? 'Loading older messages…' : 'Load older messages'}</button>}
    {page?.legacyHistory && older === null && <details><summary>Legacy message history</summary><p className="meta">Older messages use retained request order. Their order relative to displays and forms is unavailable before sequence {page.cutoverSequence}.</p>{legacy}</details>}
    {failedRequests.map(item => <div className="error" role="alert" key={item.id}><strong>Exchange failed</strong><p>{item.detail ?? 'The model request failed.'}</p><details><summary>Exchange details</summary>{item.id}</details></div>)}
    <div className="unified-entries" role="list" aria-label="Conversation entries">{rows.map(row => <div role="listitem" className="timeline-row" key={row.key} data-timeline-key={row.key} data-sequence={'entry' in row ? row.entry.sequence : undefined}>
      <TimelineContent row={row} streamingActive={ready && active} />
      {row.kind === 'entry' && row.entry.kind === 'output' && <UnifiedOutput entry={row.entry} origin={origin} ready={ready} active={active} onAuthExpired={onAuthExpired} onExpanded={() => setAttempt(value => value + 1)} />}
      {row.kind === 'entry' && row.entry.kind === 'form' && <MountedFormEntry form={row.entry.form} ready={ready} active={active} onAuthExpired={onAuthExpired} />}
      {'entry' in row && <details className="timeline-metadata"><summary>Publication details</summary><p>Sequence {row.entry.sequence}</p>{(row.entry.kind === 'message' || row.entry.kind === 'oversized') && <p>Exchange {row.entry.requestId} · position {row.entry.position}</p>}</details>}
    </div>)}</div>
    {newActivity && <button type="button" className="new-activity" onClick={returnToLatest}>New activity ↓</button>}
    {windowed && <div className="history-controls"><p className="meta">Showing a bounded history window. Additional messages remain in retained history.</p><button type="button" disabled={!ready} onClick={returnToLatest}>Return to latest messages</button></div>}
    {page && !entries.length && !liveOutput.length && !page.legacyHistory && !issue && <p>No conversation entries have been retained yet.</p>}
  </section>
}

function TimelineContent({ row, streamingActive }: { row: TimelineRow; streamingActive: boolean }) {
  if (row.kind === 'live') return <div className="message live-message">{row.live.map(item => <div key={outputKey(item)}><h3>{outputLabels[item.channel]} <span className="meta">{item.committedHash ? 'Retained preview' : item.streaming && streamingActive ? 'Streaming' : 'Incomplete'}</span></h3><pre className="history-content">{item.text}</pre>{item.overflow && <p role="status">Live preview is partial. Completed content remains available in retained history.</p>}</div>)}</div>
  if (row.kind === 'activity') return <div className="message tool-activity"><h3>{toolName(row.entry)} <span className="meta">{row.results.length ? 'Result retained' : row.entry.tool ? 'No loaded result' : 'Operation ownership unavailable'}</span></h3><details><summary>Tool activity</summary><HistoryItem listItem={false} entry={historyEntry(row.entry)} />{row.results.map(result => <div key={result.sequence}><p className="meta">Result published at sequence {result.sequence}</p><HistoryItem listItem={false} entry={historyEntry(result)} /></div>)}</details></div>
  if (row.kind === 'result') return row.invocationKey ? <div className="tool-result-marker"><span>Tool result retained</span><button type="button" onClick={event => { const section = event.currentTarget.closest('.unified-chat'); const card = [...(section?.querySelectorAll<HTMLElement>('[data-timeline-key]') ?? [])].find(item => item.dataset.timelineKey === row.invocationKey); const detail = card?.querySelector('details'); if (detail) detail.open = true; card?.scrollIntoView({ block: 'nearest' }) }}>Show tool activity</button></div> : <div className="message tool-activity"><h3>Tool result</h3><p className="meta">{row.entry.tool ? 'The exact invocation is outside the loaded window.' : 'Operation ownership is unavailable for this historical result.'}</p><HistoryItem listItem={false} entry={historyEntry(row.entry)} /></div>
  if (row.entry.kind === 'message') return <HistoryItem listItem={false} entry={historyEntry(row.entry)} />
  if (row.entry.kind === 'oversized') return <p role="status">Message {row.entry.position} is too large to display ({row.entry.byteLen} bytes).</p>
  return null
}
function historyEntry(entry: Extract<UnifiedEntry, { kind: 'message' }>): HistoryEntry { return { position: entry.position, hash: entry.hash, byteLen: new TextEncoder().encode(JSON.stringify(entry.item)).length, item: entry.item } }
function toolName(entry: Extract<UnifiedEntry, { kind: 'message' }>): string { return object(entry.item) && typeof entry.item.name === 'string' ? entry.item.name : 'Tool activity' }

function UnifiedOutput({ entry, origin, ready, active, onAuthExpired, onExpanded }: {
  entry: Extract<UnifiedEntry, { kind: 'output' }>; origin: ActorOutputOrigin; ready: boolean; active: boolean
  onAuthExpired?: () => void; onExpanded: () => void
}) {
  const [busyKey, setBusyKey] = useState<number>()
  const [issue, setIssue] = useState('')
  const output = entry.output.emission
  async function expand(key: number) {
    setBusyKey(key)
    try {
      const response = await fetch('/api/actor-output/expand', { method: 'POST', credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ origin, displaySlot: output.id.displaySlot, key }) })
      if (response.status === 401 || response.status === 403) onAuthExpired?.()
      if (!response.ok) throw new Error(response.status === 409 ? 'This detail is no longer available. Refresh the conversation.' : 'This detail could not be expanded.')
      setIssue('')
      onExpanded()
    } catch (error) { setIssue(error instanceof Error ? error.message : String(error)) }
    finally { setBusyKey(undefined) }
  }
  return <><h3>Displayed values</h3>{output.page.view ? <RichViewRenderer view={output.page.view} /> : <pre>{output.page.text}</pre>}
    {issue && <p role="alert">{issue}</p>}
    {output.page.unavailable && <p>Some detail is unavailable from this renderer.</p>}
    {output.page.expansions.map(([key, label]) => <button key={key} disabled={!active || !ready || busyKey !== undefined}
      onClick={() => void expand(key)}>{busyKey === key ? 'Loading…' : `Show ${label}`}</button>)}
  </>
}

function MountedFormEntry({ form, ready, active, onAuthExpired }: { form: StoredActorForm; ready: boolean; active: boolean; onAuthExpired?: () => void }) {
  return <><MountedForm form={form} ready={ready} active={active} onAuthExpired={onAuthExpired} />
  </>
}
