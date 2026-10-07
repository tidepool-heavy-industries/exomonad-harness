import { useEffect, useState, type ReactNode } from 'react'
import { actorOutputKey, isActorOutputOrigin, isStoredActorOutput, type ActorOutputOrigin, type StoredActorOutput } from './actor-output'
import { isStoredActorForm, type StoredActorForm } from './MountedForm'
import HistoryItem from './HistoryItem'
import type { HistoryEntry } from './history-client'
import MountedForm from './MountedForm'
import { visibleOutput, outputKey, outputLabels, type LiveOutput } from './live-output'
import { RichViewRenderer } from './rich-view'
import type { HarnessViewModel } from './view-model'

export type UnifiedEntry =
  | { readonly sequence: number; readonly kind: 'message'; readonly requestId: string; readonly position: number; readonly hash: string; readonly item: unknown }
  | { readonly sequence: number; readonly kind: 'oversized'; readonly requestId: string; readonly position: number; readonly hash: string; readonly byteLen: number }
  | { readonly sequence: number; readonly kind: 'output'; readonly output: StoredActorOutput }
  | { readonly sequence: number; readonly kind: 'form'; readonly form: StoredActorForm }
export interface UnifiedPage {
  readonly origin: ActorOutputOrigin; readonly cutoverSequence: number; readonly legacyHistory: boolean
  readonly entries: readonly UnifiedEntry[]; readonly nextAfter: number | null
}
interface CachedPage { readonly after: number; readonly page: UnifiedPage }
const MAX_CACHED_PAGES = 16
const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)
const nat = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0

export function decodeUnifiedPage(value: unknown, origin: ActorOutputOrigin, after: number): UnifiedPage {
  if (!object(value) || !isActorOutputOrigin(value.origin) || actorOutputKey(value.origin) !== actorOutputKey(origin)
    || !nat(value.cutoverSequence) || typeof value.legacyHistory !== 'boolean' || !Array.isArray(value.entries) || value.entries.length > 100
    || !(value.nextAfter === null || nat(value.nextAfter) && value.nextAfter > after)) throw new Error('Unified conversation returned an invalid page.')
  let last = after
  for (const entry of value.entries) {
    if (!object(entry) || !nat(entry.sequence) || entry.sequence <= last) throw new Error('Unified conversation entries are out of sequence.')
    last = entry.sequence
    if (entry.kind === 'message') {
      if (typeof entry.requestId !== 'string' || !nat(entry.position) || typeof entry.hash !== 'string' || !/^[0-9a-f]{64}$/.test(entry.hash) || !Object.hasOwn(entry, 'item')) throw new Error('Unified conversation contains an invalid message.')
    } else if (entry.kind === 'oversized') {
      if (typeof entry.requestId !== 'string' || !nat(entry.position) || typeof entry.hash !== 'string' || !/^[0-9a-f]{64}$/.test(entry.hash) || !nat(entry.byteLen)) throw new Error('Unified conversation contains an invalid oversized message.')
    } else if (entry.kind === 'output') {
      if (!isStoredActorOutput(entry.output) || actorOutputKey(entry.output.reference.origin) !== actorOutputKey(origin)) throw new Error('Unified conversation contains invalid output.')
    } else if (entry.kind === 'form') {
      if (!isStoredActorForm(entry.form) || actorOutputKey(entry.form.opening.origin) !== actorOutputKey(origin) || entry.form.sequence !== entry.sequence) throw new Error('Unified conversation contains an invalid form.')
    } else throw new Error('Unified conversation contains an unknown entry.')
  }
  if (value.nextAfter !== null && value.nextAfter !== last) throw new Error('Unified conversation cursor does not match its last entry.')
  return value as unknown as UnifiedPage
}

async function readUnifiedPage(origin: ActorOutputOrigin, requestId: string | undefined, after: number, signal: AbortSignal, onAuthExpired?: () => void) {
  const query = new URLSearchParams({ run: origin.run, actor: String(origin.nativeActor), incarnation: String(origin.incarnation), after: String(after), limit: '50' })
  if (requestId) query.set('request', requestId)
  const response = await fetch(`/api/chat?${query}`, { credentials: 'same-origin', cache: 'no-store', signal })
  if (response.status === 401 || response.status === 403) onAuthExpired?.()
  if (!response.ok) throw new Error(response.status === 404 ? 'Unified conversation history is unavailable.' : `Conversation history failed (${response.status}).`)
  const raw = await response.text()
  if (new TextEncoder().encode(raw).length > 256 * 1024) throw new Error('Unified conversation exceeded the page limit.')
  return decodeUnifiedPage(JSON.parse(raw) as unknown, origin, after)
}

export default function UnifiedChat({ origin, requestId, revision, ready, active, onAuthExpired, legacy, liveOutput = [], requests }: {
  origin: ActorOutputOrigin; requestId?: string; revision?: number; ready: boolean; active: boolean; onAuthExpired?: () => void; legacy?: ReactNode
  liveOutput?: readonly LiveOutput[]; requests?: ReadonlyMap<string, Pick<HarnessViewModel['timeline'][number], 'id' | 'state' | 'detail' | 'failure'>>
}) {
  const [pages, setPages] = useState<readonly CachedPage[]>([])
  const [pageIndex, setPageIndex] = useState(0)
  const [issue, setIssue] = useState('')
  const [attempt, setAttempt] = useState(0)
  const [loadingNext, setLoadingNext] = useState(false)
  const identity = actorOutputKey(origin)
  const after = pages[pageIndex]?.after ?? 0
  const page = pages[pageIndex]?.page
  useEffect(() => {
    if (!ready) return
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout> | undefined
    const load = async () => {
      try {
        const next = await readUnifiedPage(origin, requestId, after, controller.signal, onAuthExpired)
        if (!controller.signal.aborted) {
          setPages(current => {
            if (current[pageIndex]?.after === after) {
              const updated = [...current]
              updated[pageIndex] = { after, page: next }
              return updated
            }
            return pageIndex === current.length ? [...current, { after, page: next }] : current
          })
          setIssue('')
        }
      } catch (error) {
        if (!controller.signal.aborted) setIssue(error instanceof Error ? error.message : String(error))
      } finally {
        if (!controller.signal.aborted && active) timer = setTimeout(load, 2000)
      }
    }
    void load()
    return () => { controller.abort(); clearTimeout(timer) }
  }, [identity, requestId, after, pageIndex, revision, ready, active, attempt, onAuthExpired])
  async function loadNext() {
    if (!page?.nextAfter || loadingNext || !ready) return
    if (pageIndex + 1 < pages.length) { setPageIndex(index => index + 1); return }
    const cursor = page.nextAfter
    const controller = new AbortController()
    setLoadingNext(true); setIssue('')
    try {
      const next = await readUnifiedPage(origin, requestId, cursor, controller.signal, onAuthExpired)
      setPages(current => {
        if (current[pageIndex]?.page.nextAfter !== cursor) return current
        const extended = [...current.slice(0, pageIndex + 1), { after: cursor, page: next }]
        return extended.length > MAX_CACHED_PAGES ? extended.slice(1) : extended
      })
      setPageIndex(index => Math.min(MAX_CACHED_PAGES - 1, index + 1))
    } catch (error) { setIssue(error instanceof Error ? error.message : String(error)) }
    finally { setLoadingNext(false) }
  }
  const loadedHashes = new Map<string, ReadonlySet<string>>()
  for (const cached of pages) for (const entry of cached.page.entries) if (entry.kind === 'message') {
    const hashes = new Set(loadedHashes.get(entry.requestId) ?? [])
    hashes.add(entry.hash); loadedHashes.set(entry.requestId, hashes)
  }
  const provisional = visibleOutput(liveOutput, loadedHashes)
  const visibleEntries = page?.entries ?? []
  const failedRequests = [...(requests?.values() ?? [])].filter(item => item.state === 'failed'
    && page?.entries.some(entry => entry.kind === 'message' && entry.requestId === item.id))
  return <section className="unified-chat" aria-label="Conversation" aria-busy={ready && !page}>
    <div className="toolbar"><h2>Conversation</h2><button type="button" disabled={!ready} onClick={() => setAttempt(value => value + 1)}>Refresh conversation</button></div>
    {!ready && <p role="status">Host unavailable; retained conversation entries remain visible.</p>}
    {issue && <div role="alert"><p>{issue}</p><button type="button" disabled={!ready} onClick={() => setAttempt(value => value + 1)}>Retry conversation</button></div>}
    {page?.legacyHistory && legacy}
    {failedRequests.map(item => <div className="error" role="alert" key={item.id}><strong>Exchange failed</strong><p className="meta">Exchange {item.id}</p><p>{item.detail ?? 'The model request failed.'}</p></div>)}
    {page && <div className="unified-entries" role="list" aria-label="Conversation entries">{visibleEntries.map(entry => <div role="listitem" className="message" key={entry.sequence} data-sequence={entry.sequence}>
      <div className="meta">Sequence {entry.sequence}</div>
      {entry.kind === 'message' ? <HistoryItem entry={{ position: entry.position, hash: entry.hash, byteLen: new TextEncoder().encode(JSON.stringify(entry.item)).length, item: entry.item } satisfies HistoryEntry} /> :
        entry.kind === 'oversized' ? <p role="status">Message {entry.position} is too large to display ({entry.byteLen} bytes).</p> :
        entry.kind === 'output' ? <UnifiedOutput entry={entry} origin={origin} ready={ready} active={active} onAuthExpired={onAuthExpired} onExpanded={() => setAttempt(value => value + 1)} /> :
          <MountedFormEntry key={entry.form.opening.mountId} form={entry.form} ready={ready} active={active} onAuthExpired={onAuthExpired} />}
    </div>)}</div>}
    {page && <div className="history-controls">
      <button type="button" disabled={pageIndex === 0} onClick={() => setPageIndex(index => Math.max(0, index - 1))}>Previous entries</button>
      <button type="button" disabled={pageIndex + 1 >= pages.length && page.nextAfter === null || !ready || loadingNext} onClick={() => void loadNext()}>
        {loadingNext ? 'Loading entries…' : 'Next entries'}
      </button>
    </div>}
    {provisional.length > 0 && <div role="list" aria-label="Provisional live output">{provisional.map(item => <div role="listitem" className="message" key={outputKey(item)}>
      <h3>{outputLabels[item.channel]}{item.committedHash ? '' : item.streaming && ready && active ? ' · streaming' : ' · incomplete'}</h3>
      <pre className="history-content">{item.text}</pre>{item.overflow && <p role="status">Live preview is partial. Completed content remains available in retained history.</p>}
    </div>)}</div>}
    {page && !page.entries.length && !page.legacyHistory && !issue && <p>No conversation entries have been retained yet.</p>}
  </section>
}

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
  return <><h3>Form · {form.state}</h3><MountedForm form={form} ready={ready} active={active} onAuthExpired={onAuthExpired} />
  </>
}
