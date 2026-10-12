import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { actorOutputKey, isStoredActorOutput, type ActorOutputOrigin } from './actor-output'
import { isStoredActorForm, type StoredActorForm } from './MountedForm'
import HistoryItem from './HistoryItem'
import { MessageContent, MessagePresentation } from './MessagePresentation'
import type { HistoryEntry } from './history-client'
import MountedForm from './MountedForm'
import { outputKey, outputLabels, originKey, type LiveOutput, type OutputOrigin } from './live-output'
import { projectTimeline, mergeEntries, boundEntries, type TimelineRow } from './chat-timeline'
import { isChatPage } from './generated/validators.mjs'
import type { ChatPage, ChatEntry, ChatInputOperation } from './generated/chat'
import './UnifiedChat.css'
import { RichViewRenderer } from './rich-view'
import ToolActivity, { type ExchangeObservation, type ToolJobObservation } from './ToolActivity'
import { PresentationMemory } from './presentation-state'
import { hasChatSelection, useChatSelection } from './chat-selection'

export type UnifiedEntry = ChatEntry
export type UnifiedPage = ChatPage
const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)

/** Generated shape validation is separate from publication/cursor associations. */
export function decodeUnifiedPage(value: unknown, origin: ActorOutputOrigin, cursor: string | { before?: string } = {}): UnifiedPage {
  const after = typeof cursor === 'string' ? BigInt(cursor) : 0n
  const backward = typeof cursor !== 'string'
  if (!isChatPage(value) || actorOutputKey(value.origin) !== actorOutputKey(origin)
    || BigInt(value.cutoverSequence) < 0n || value.entries.length > 100
    || (backward && (value.previousBefore === undefined || value.nextAfter !== null))
    || (value.nextAfter !== null && BigInt(value.nextAfter) <= after)
    || (value.previousBefore !== null && BigInt(value.previousBefore) <= 0n)) throw new Error('Unified conversation returned an invalid page.')
  let last = after
  for (const entry of value.entries) {
    const sequence = BigInt(entry.sequence)
    if (sequence <= last || (typeof cursor !== 'string' && cursor.before !== undefined && sequence >= BigInt(cursor.before))) throw new Error('Unified conversation entries are out of sequence.')
    last = sequence
    if (entry.kind === 'message' || entry.kind === 'oversized') {
      if (!entry.requestId || BigInt(entry.position) < 0n || !/^[0-9a-f]{64}$/.test(entry.hash)) throw new Error('Unified conversation contains an invalid message.')
      if (entry.kind === 'message' && entry.providerItemId !== null && (!entry.providerItemId || !object(entry.item) || entry.item.id !== entry.providerItemId)) throw new Error('Unified conversation contains invalid provider identity.')
      if (entry.kind === 'message' && entry.tool != null) {
        const { operation, invocation } = entry.tool
        if (!operation.request || !operation.call || invocation.request !== operation.request || BigInt(invocation.position) < 0n
          || !/^[0-9a-f]{64}$/.test(invocation.hash) || !object(entry.item) || entry.item.call_id !== operation.call) throw new Error('Unified conversation contains invalid tool ownership.')
        if (['function_call', 'custom_tool_call'].includes(String(entry.item.type)) && (invocation.request !== entry.requestId || invocation.position !== entry.position || invocation.hash !== entry.hash)) throw new Error('Unified conversation invocation does not match its issuing occurrence.')
      }
    } else if (entry.kind === 'output') {
      if (!isStoredActorOutput(entry.output) || actorOutputKey(entry.output.reference.origin) !== actorOutputKey(origin) || entry.output.reference.sequence !== entry.sequence) throw new Error('Unified conversation contains invalid output.')
    } else if (entry.kind === 'form') {
      if (!isStoredActorForm(entry.form) || actorOutputKey(entry.form.opening.origin) !== actorOutputKey(origin) || entry.form.sequence !== entry.sequence) throw new Error('Unified conversation contains an invalid form.')
    }
  }
  if (backward && value.previousBefore !== null && value.previousBefore !== value.entries[0]?.sequence) throw new Error('Unified conversation cursor does not match its first entry.')
  if (value.nextAfter !== null && BigInt(value.nextAfter) !== last) throw new Error('Unified conversation cursor does not match its last entry.')
  return value
}

async function readUnifiedPage(origin: ActorOutputOrigin, requestId: string | undefined, before: string | undefined, signal: AbortSignal, onAuthExpired?: () => void) {
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
  jobs?: readonly ToolJobObservation[]
  origin: ActorOutputOrigin; requestId?: string; revision?: string; ready: boolean; active: boolean; onAuthExpired?: () => void; legacy?: ReactNode
  liveOutput?: readonly LiveOutput[]; modelOrigin?: OutputOrigin; onRenderedInputs?: (references: readonly ChatInputOperation[]) => void; requests?: ReadonlyMap<string, ExchangeObservation>
}
export default function UnifiedChat(props: UnifiedChatProps) { return <UnifiedChatView key={actorOutputKey(props.origin)} {...props} /> }
function UnifiedChatView({ origin, requestId, revision, ready, active, onAuthExpired, legacy, liveOutput = [], requests, jobs = [], modelOrigin, onRenderedInputs }: UnifiedChatProps) {
  const [page, setPage] = useState<UnifiedPage>()
  const [entries, setEntries] = useState<readonly UnifiedEntry[]>([])
  const entriesRef = useRef(entries)
  entriesRef.current = entries
  const [older, setOlder] = useState<string | null>(null)
  const [issue, setIssue] = useState('')
  const [attempt, setAttempt] = useState(0)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [newActivity, setNewActivity] = useState(false)
  const [windowed, setWindowed] = useState(false)
  const section = useRef<HTMLElement>(null)
  const lastFormJump = useRef<string>()
  const pinned = useRef(true)
  const anchor = useRef<ScrollAnchor>()
  const controllerOlder = useRef<AbortController>()
  const generation = useRef(0)
  const replaceLatest = useRef(false)
  const latestSequence = useRef('0')
  const previousLive = useRef('')
  const observedFinals = useRef<ReadonlySet<string>>(new Set())
  const identity = actorOutputKey(origin)
  const scopedOutput = modelOrigin ? liveOutput.filter(item => originKey(item.origin) === originKey(modelOrigin)) : []
  const projected = projectTimeline(origin, entries, scopedOutput, modelOrigin)
  // Retain only exact keys still present in the already bounded live preview set.
  // Window eviction cannot revoke a final publication already rendered here.
  const residentPreviewKeys = new Set(projectTimeline(origin, [], scopedOutput, modelOrigin).map(row => row.key))
  const knownFinals = new Set([...observedFinals.current].filter(key => residentPreviewKeys.has(key)))
  const rows = projected.filter(row => row.kind !== 'live' || !knownFinals.has(row.key))
  const openForms = entries.filter((entry): entry is Extract<UnifiedEntry, { kind: 'form' }> => entry.kind === 'form' && entry.form.state === 'open')
  function updateForm(form: StoredActorForm) {
    setEntries(current => mergeEntries(current, [{ kind: 'form', sequence: form.sequence, form }]))
  }
  function jumpToForm() {
    const index = openForms.findIndex(entry => entry.form.opening.mountId === lastFormJump.current)
    const next = openForms[(index + 1) % openForms.length]
    if (!next) return
    lastFormJump.current = next.form.opening.mountId
    const form = [...(section.current?.querySelectorAll<HTMLElement>('[data-mount-id]') ?? [])].find(element => element.dataset.mountId === next.form.opening.mountId)
    pinned.current = false
    form?.scrollIntoView({ block: 'center' })
    ;(form?.querySelector<HTMLElement>('input:not(:disabled), textarea:not(:disabled), select:not(:disabled)') ?? form)?.focus({ preventScroll: true })
  }
  const renderedInputs = rows.flatMap(row => 'entry' in row && row.entry.kind === 'message' && row.entry.inputOperation ? [row.entry.inputOperation] : [])
  const inputVersion = JSON.stringify(renderedInputs)
  const inputCallback = useRef(onRenderedInputs)
  inputCallback.current = onRenderedInputs
  useLayoutEffect(() => { inputCallback.current?.(renderedInputs) }, [inputVersion])
  const liveVersion = JSON.stringify(scopedOutput.map(item => [outputKey(item), item.version, item.streaming]))
  useChatSelection(section, entries, liveVersion)
  useLayoutEffect(() => {
    for (const row of projected) if (row.kind !== 'live' && residentPreviewKeys.has(row.key)) knownFinals.add(row.key)
    observedFinals.current = knownFinals
  }, [entries, liveVersion])
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
    else if (pinned.current && !hasChatSelection(section.current)) revealLatest()
  }, [entries, liveVersion])
  useEffect(() => {
    if (previousLive.current && previousLive.current !== liveVersion && (!pinned.current || hasChatSelection(section.current))) setNewActivity(true)
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
          const newest = next.entries.at(-1)?.sequence ?? '0'
          if (latestSequence.current !== '0' && BigInt(newest) > BigInt(latestSequence.current) && !pinned.current) setNewActivity(true)
          latestSequence.current = newest
          if (!pinned.current) anchor.current = captureAnchor(section.current)
          setPage(next)
          const current = entriesRef.current
          const overlap = next.entries.some(entry => current.some(old => old.sequence === entry.sequence))
          const newerWindow = current.length > 0 && next.entries.length > 0 && !overlap && BigInt(next.entries[0]!.sequence) > BigInt(current.at(-1)!.sequence)
          if (replaceLatest.current) { replaceLatest.current = false; setEntries(mergeEntries(current.filter(entry => next.entries.some(nextEntry => nextEntry.sequence === entry.sequence)), next.entries)); setOlder(next.previousBefore ?? null); setWindowed(false) }
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
    {active && openForms.length > 0 && <nav className="chat-attention" aria-label="Pending responses">
      <span role="status">{openForms.length} {openForms.length === 1 ? 'form needs' : 'forms need'} your response <small>in loaded history{!ready ? ' · reconnect to respond' : ''}</small></span>
      <button type="button" onClick={jumpToForm}>{openForms.length === 1 ? 'Jump to form' : 'Jump to next form'}</button>
    </nav>}
    <div className="toolbar"><h2>Conversation</h2><button type="button" disabled={!ready} onClick={() => setAttempt(value => value + 1)}>Refresh conversation</button></div>
    {!ready && <p role="status">Host unavailable; retained conversation entries remain visible.</p>}
    {!modelOrigin && liveOutput.length > 0 && <p role="status">Live output is unavailable for this actor.</p>}
    {issue && <div role="alert"><p>{issue}</p><button type="button" disabled={!ready} onClick={() => setAttempt(value => value + 1)}>Retry conversation</button></div>}
    {older !== null && <button type="button" className="load-older" disabled={!ready || loadingOlder} onClick={() => void loadOlder()}>{loadingOlder ? 'Loading older messages…' : 'Load older messages'}</button>}
    {page?.legacyHistory && older === null && <details><summary>Legacy message history</summary><p className="meta">Older messages use retained request order. Their order relative to displays and forms is unavailable before sequence {page.cutoverSequence}.</p>{legacy}</details>}
    {failedRequests.map(item => <div className="error" role="alert" key={item.id}><strong>Exchange failed</strong><p>{item.detail ?? 'The model request failed.'}</p><details><summary>Exchange details</summary>{item.id}</details></div>)}
    <div className="unified-entries" role="list" aria-label="Conversation entries">{rows.map(row => <div role="listitem" className="timeline-row" key={row.key} data-timeline-key={row.key} data-sequence={'entry' in row ? row.entry.sequence : undefined}>
      <PresentationMemory><TimelineContent row={row} streamingActive={ready && active} requests={requests} jobs={jobs} modelOrigin={modelOrigin} /></PresentationMemory>
      {row.kind === 'entry' && row.entry.kind === 'output' && <UnifiedOutput entry={row.entry} origin={origin} ready={ready} active={active} onAuthExpired={onAuthExpired} onExpanded={() => setAttempt(value => value + 1)} />}
      {row.kind === 'entry' && row.entry.kind === 'form' && <MountedForm form={row.entry.form} ready={ready} active={active} onAuthExpired={onAuthExpired} onStateChange={updateForm} />}
      {'entry' in row && <details className="timeline-metadata"><summary>Publication details</summary><p>Sequence {row.entry.sequence}</p>{(row.entry.kind === 'message' || row.entry.kind === 'oversized') && <p>Exchange {row.entry.requestId} · position {row.entry.position}</p>}</details>}
    </div>)}</div>
    {newActivity && <button type="button" className="new-activity" onClick={returnToLatest}>New activity ↓</button>}
    {windowed && <div className="history-controls"><p className="meta">Showing a bounded history window. Additional messages remain in retained history.</p><button type="button" disabled={!ready} onClick={returnToLatest}>Return to latest messages</button></div>}
    {page && !entries.length && !scopedOutput.length && !page.legacyHistory && !issue && <p>No conversation entries have been retained yet.</p>}
  </section>
}

function TimelineContent({ row, streamingActive, requests, jobs, modelOrigin }: { row: TimelineRow; streamingActive: boolean; requests?: ReadonlyMap<string, ExchangeObservation>; jobs: readonly ToolJobObservation[]; modelOrigin?: OutputOrigin }) {
  if (row.kind === 'activity' || row.kind === 'live' && row.live.every(item => item.channel === 'tool_input' || item.channel === 'tool_arguments')) {
    const operation = row.kind === 'activity' ? row.entry.tool?.operation : undefined
    const matchedJobs = operation && modelOrigin && originKey(operation.origin) === originKey(modelOrigin)
      ? jobs.filter(job => job.kind === 'job' && job.requestId === operation.request && job.callId === operation.call) : []
    return <ToolActivity row={row} streamingActive={streamingActive} exchange={requests?.get(row.kind === 'activity' ? row.entry.requestId : row.live[0]!.requestId)} jobs={matchedJobs} />
  }
  if (row.kind === 'live') return <div className="live-message">{row.live.map(item => <MessagePresentation key={outputKey(item)} listItem={false}
    kind={item.channel === 'reasoning' || item.channel === 'reasoning_summary' ? 'reasoning' : item.channel === 'tool_input' || item.channel === 'tool_arguments' ? 'activity' : 'assistant'}
    title={outputLabels[item.channel]} collapsed={item.channel === 'reasoning' || item.channel === 'reasoning_summary'}
    status={item.committedHash ? 'Retained preview' : item.streaming && streamingActive ? 'Streaming' : 'Incomplete'}>
    <MessageContent text={item.text} label={`${outputLabels[item.channel]} output`} memoryKey={`part:${row.live.indexOf(item)}`} bounded={item.channel === 'tool_input' || item.channel === 'tool_arguments'} format={item.channel === 'assistant' || item.channel === 'refusal' ? 'markdown' : 'plain'} />
    {item.overflow && <p role="status">Live preview is partial. Completed content remains available in retained history.</p>}
  </MessagePresentation>)}</div>
  if (row.kind === 'result') return row.invocationKey ? <div className="tool-result-marker"><span>Tool result retained</span><button type="button" onClick={event => { const section = event.currentTarget.closest('.unified-chat'); const card = [...(section?.querySelectorAll<HTMLElement>('[data-timeline-key]') ?? [])].find(item => item.dataset.timelineKey === row.invocationKey); const detail = card?.querySelector('details'); if (detail) detail.open = true; card?.scrollIntoView({ block: 'nearest' }) }}>Show tool activity</button></div> : <div className="message tool-activity"><h3>Tool result</h3><p className="meta">{row.entry.tool ? 'The exact invocation is outside the loaded window.' : 'Operation ownership is unavailable for this historical result.'}</p><HistoryItem listItem={false} entry={historyEntry(row.entry)} /></div>
  if (row.entry.kind === 'message') return <HistoryItem listItem={false} entry={historyEntry(row.entry)} />
  if (row.entry.kind === 'oversized') return <p role="status">Message {row.entry.position} is too large to display ({row.entry.byteLen} bytes).</p>
  return null
}
function historyEntry(entry: Extract<UnifiedEntry, { kind: 'message' }>): HistoryEntry { return { position: entry.position, hash: entry.hash, byteLen: String(new TextEncoder().encode(JSON.stringify(entry.item)).length), item: entry.item } }

function UnifiedOutput({ entry, origin, ready, active, onAuthExpired, onExpanded }: {
  entry: Extract<UnifiedEntry, { kind: 'output' }>; origin: ActorOutputOrigin; ready: boolean; active: boolean
  onAuthExpired?: () => void; onExpanded: () => void
}) {
  const [busyKey, setBusyKey] = useState<string>()
  const [issue, setIssue] = useState('')
  const output = entry.output.emission
  async function expand(key: string) {
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
