import { useLayoutEffect, useRef, type ReactNode, type RefObject } from 'react'
import { actorOutputKey, type ActorOutputOrigin } from './actor-output'
import MountedForm from './MountedForm'
import { outputKey, originKey, type LiveOutput, type OutputOrigin } from './live-output'
import { projectTimeline } from './chat-timeline'
import type { ChatInputOperation } from './generated/chat'
import type { UnifiedEntry } from './conversation-client'
import type { ExchangeObservation, ToolJobObservation } from './ToolActivity'
import { PresentationMemory } from './presentation-state'
import { TimelineContent, UnifiedOutput } from './ConversationContent'
import { useConversationHistory } from './useConversationHistory'
import { useConversationViewport, useConversationViewportUpdates } from './useConversationViewport'
import './UnifiedChat.css'

export { decodeUnifiedPage } from './conversation-client'
export type { UnifiedEntry, UnifiedPage } from './conversation-client'

export interface UnifiedChatProps {
  viewportRef?: RefObject<HTMLElement>
  jobs?: readonly ToolJobObservation[]
  origin: ActorOutputOrigin; requestId?: string; revision?: string; ready: boolean; active: boolean; onAuthExpired?: () => void; legacy?: ReactNode
  liveOutput?: readonly LiveOutput[]; modelOrigin?: OutputOrigin; onRenderedInputs?: (references: readonly ChatInputOperation[]) => void; requests?: ReadonlyMap<string, ExchangeObservation>
}
export default function UnifiedChat(props: UnifiedChatProps) { return <UnifiedChatView key={actorOutputKey(props.origin)} {...props} /> }
function UnifiedChatView({ viewportRef, origin, requestId, revision, ready, active, onAuthExpired, legacy, liveOutput = [], requests, jobs = [], modelOrigin, onRenderedInputs }: UnifiedChatProps) {
  const viewport = useConversationViewport(viewportRef)
  const { section, pinned, newActivity } = viewport
  const { page, entries, older, issue, loadingOlder, windowed, updateForm, loadOlder, returnToLatest, refresh } = useConversationHistory({ origin, requestId, revision, ready, active, onAuthExpired }, viewport)
  const attention = useRef<HTMLElement>(null)
  const lastFormJump = useRef<string>()
  const observedFinals = useRef<ReadonlySet<string>>(new Set())
  const scopedOutput = modelOrigin ? liveOutput.filter(item => originKey(item.origin) === originKey(modelOrigin)) : []
  const projected = projectTimeline(origin, entries, scopedOutput, modelOrigin)
  // Retain only exact keys still present in the already bounded live preview set.
  // Window eviction cannot revoke a final publication already rendered here.
  const residentPreviewKeys = new Set(projectTimeline(origin, [], scopedOutput, modelOrigin).map(row => row.key))
  const knownFinals = new Set([...observedFinals.current].filter(key => residentPreviewKeys.has(key)))
  const rows = projected.filter(row => row.kind !== 'live' || !knownFinals.has(row.key))
  const openForms = entries.filter((entry): entry is Extract<UnifiedEntry, { kind: 'form' }> => entry.kind === 'form' && entry.form.state === 'open')
  useLayoutEffect(() => {
    const element = attention.current
    const update = () => section.current?.style.setProperty('--chat-attention-height', `${element?.getBoundingClientRect().height ?? 0}px`)
    update()
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(update)
    if (element) observer?.observe(element)
    return () => observer?.disconnect()
  }, [active, openForms.length, ready])
  function jumpToForm() {
    const index = openForms.findIndex(entry => entry.form.opening.mountId === lastFormJump.current)
    const next = openForms[(index + 1) % openForms.length]
    if (!next) return
    lastFormJump.current = next.form.opening.mountId
    const form = [...(section.current?.querySelectorAll<HTMLElement>('[data-mount-id]') ?? [])].find(element => element.dataset.mountId === next.form.opening.mountId)
    pinned.current = false
    const target = form?.querySelector<HTMLElement>('input:not(:disabled), textarea:not(:disabled), select:not(:disabled)') ?? form
    form?.scrollIntoView({ block: 'start' })
    target?.focus({ preventScroll: true })
  }
  const renderedInputs = rows.flatMap(row => 'entry' in row && row.entry.kind === 'message' && row.entry.inputOperation ? [row.entry.inputOperation] : [])
  const inputVersion = JSON.stringify(renderedInputs)
  const inputCallback = useRef(onRenderedInputs)
  inputCallback.current = onRenderedInputs
  useLayoutEffect(() => { inputCallback.current?.(renderedInputs) }, [inputVersion])
  const liveVersion = JSON.stringify(scopedOutput.map(item => [outputKey(item), item.version, item.streaming]))
  useConversationViewportUpdates(viewport, entries, liveVersion)
  useLayoutEffect(() => {
    for (const row of projected) if (row.kind !== 'live' && residentPreviewKeys.has(row.key)) knownFinals.add(row.key)
    observedFinals.current = knownFinals
  }, [entries, liveVersion])
  const failedRequests = [...(requests?.values() ?? [])].filter(item => item.state === 'failed'
    && entries.some(entry => entry.kind === 'message' && entry.requestId === item.id))
  return <section ref={section} className="unified-chat" aria-label="Conversation" aria-busy={ready && !page}>
    {active && openForms.length > 0 && <nav ref={attention} className="chat-attention" aria-label="Pending responses">
      <span role="status">{openForms.length} {openForms.length === 1 ? 'response' : 'responses'} needed <small>in loaded history{!ready ? ' · reconnect to respond' : ''}</small></span>
      <button type="button" onClick={jumpToForm}>{openForms.length === 1 ? 'Jump to form' : 'Jump to next form'}</button>
    </nav>}
    <div className="toolbar"><h2>Conversation</h2><button type="button" disabled={!ready} onClick={refresh}>Refresh conversation</button></div>
    {!ready && <p role="status">Host unavailable; retained conversation entries remain visible.</p>}
    {!modelOrigin && liveOutput.length > 0 && <p role="status">Live output is unavailable for this actor.</p>}
    {issue && <div role="alert"><p>{issue}</p><button type="button" disabled={!ready} onClick={refresh}>Retry conversation</button></div>}
    {older !== null && <button type="button" className="load-older" disabled={!ready || loadingOlder} onClick={() => void loadOlder()}>{loadingOlder ? 'Loading older messages…' : 'Load older messages'}</button>}
    {page?.legacyHistory && older === null && <details><summary>Legacy message history</summary><p className="meta">Older messages use retained request order. Their order relative to displays and forms is unavailable before sequence {page.cutoverSequence}.</p>{legacy}</details>}
    {failedRequests.map(item => <div className="error" role="alert" key={item.id}><strong>Exchange failed</strong><p>{item.detail ?? 'The model request failed.'}</p><details><summary>Exchange details</summary>{item.id}</details></div>)}
    <div className="unified-entries" role="list" aria-label="Conversation entries">{rows.map(row => <div role="listitem" className="timeline-row" key={row.key} tabIndex={-1} data-timeline-key={row.key} data-sequence={'entry' in row ? row.entry.sequence : undefined}>
      <PresentationMemory><TimelineContent onRevealActivity={viewport.revealRow} row={row} streamingActive={ready && active} requests={requests} jobs={jobs} modelOrigin={modelOrigin} /></PresentationMemory>
      {row.kind === 'entry' && row.entry.kind === 'output' && <UnifiedOutput entry={row.entry} origin={origin} ready={ready} active={active} onAuthExpired={onAuthExpired} onExpanded={refresh} />}
      {row.kind === 'entry' && row.entry.kind === 'form' && <MountedForm form={row.entry.form} ready={ready} active={active} onAuthExpired={onAuthExpired} onStateChange={updateForm} />}
      {'entry' in row && row.entry.kind !== 'message' && <details className="timeline-metadata"><summary>Publication details</summary><p>Sequence {row.entry.sequence}</p>{(row.entry.kind === 'message' || row.entry.kind === 'oversized') && <p>Exchange {row.entry.requestId} · position {row.entry.position}</p>}</details>}
    </div>)}</div>
    {newActivity && <button type="button" className="new-activity" onClick={returnToLatest}>New activity ↓</button>}
    {windowed && <div className="history-controls"><p className="meta">Showing a bounded history window. Additional messages remain in retained history.</p><button type="button" disabled={!ready} onClick={returnToLatest}>Return to latest messages</button></div>}
    {page && !entries.length && !scopedOutput.length && !page.legacyHistory && !issue && <p>No conversation entries have been retained yet.</p>}
  </section>
}
