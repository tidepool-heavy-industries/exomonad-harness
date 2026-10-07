import { useEffect, useState, type ReactNode } from 'react'
import { actorOutputKey, isActorOutputOrigin, isStoredActorOutput, type ActorOutputOrigin, type StoredActorOutput } from './actor-output'
import { isStoredActorForm, type StoredActorForm } from './MountedForm'
import HistoryItem from './HistoryItem'
import type { HistoryEntry } from './history-client'
import MountedForm from './MountedForm'

export type UnifiedEntry =
  | { readonly sequence: number; readonly kind: 'message'; readonly requestId: string; readonly position: number; readonly hash: string; readonly item: unknown }
  | { readonly sequence: number; readonly kind: 'oversized'; readonly requestId: string; readonly position: number; readonly hash: string; readonly byteLen: number }
  | { readonly sequence: number; readonly kind: 'output'; readonly output: StoredActorOutput }
  | { readonly sequence: number; readonly kind: 'form'; readonly form: StoredActorForm }
export interface UnifiedPage {
  readonly origin: ActorOutputOrigin; readonly cutoverSequence: number; readonly legacyHistory: boolean
  readonly entries: readonly UnifiedEntry[]; readonly nextAfter: number | null
}
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

export default function UnifiedChat({ origin, requestId, revision, ready, active, onAuthExpired, legacy }: {
  origin: ActorOutputOrigin; requestId?: string; revision?: number; ready: boolean; active: boolean; onAuthExpired?: () => void; legacy?: ReactNode
}) {
  const [page, setPage] = useState<UnifiedPage>()
  const [issue, setIssue] = useState('')
  const [attempt, setAttempt] = useState(0)
  const identity = actorOutputKey(origin)
  useEffect(() => {
    if (!ready) return
    const controller = new AbortController()
    const query = new URLSearchParams({ run: origin.run, actor: String(origin.nativeActor), incarnation: String(origin.incarnation), after: '0', limit: '50' })
    if (requestId) query.set('request', requestId)
    let timer: ReturnType<typeof setTimeout> | undefined
    const load = async () => {
      try {
        const response = await fetch(`/api/chat?${query}`, { credentials: 'same-origin', cache: 'no-store', signal: controller.signal })
        if (response.status === 401 || response.status === 403) onAuthExpired?.()
        if (!response.ok) throw new Error(response.status === 404 ? 'Unified conversation history is unavailable.' : `Conversation history failed (${response.status}).`)
        const raw = await response.text()
        if (new TextEncoder().encode(raw).length > 256 * 1024) throw new Error('Unified conversation exceeded the page limit.')
        const next = decodeUnifiedPage(JSON.parse(raw) as unknown, origin, 0)
        if (!controller.signal.aborted) { setPage(next); setIssue('') }
      } catch (error) {
        if (!controller.signal.aborted) setIssue(error instanceof Error ? error.message : String(error))
      } finally {
        if (!controller.signal.aborted && active) timer = setTimeout(load, 2000)
      }
    }
    void load()
    return () => { controller.abort(); clearTimeout(timer) }
  }, [identity, requestId, revision, ready, active, attempt, onAuthExpired])
  return <section className="unified-chat" aria-label="Conversation" aria-busy={ready && !page}>
    <div className="toolbar"><h2>Conversation</h2><button type="button" disabled={!ready} onClick={() => setAttempt(value => value + 1)}>Refresh conversation</button></div>
    {!ready && <p role="status">Host unavailable; retained conversation entries remain visible.</p>}
    {issue && <div role="alert"><p>{issue}</p><button type="button" disabled={!ready} onClick={() => setAttempt(value => value + 1)}>Retry conversation</button></div>}
    {page?.legacyHistory && legacy}
    {page && <div className="unified-entries" role="list" aria-label="Conversation entries">{page.entries.map(entry => <div role="listitem" className="message" key={entry.sequence} data-sequence={entry.sequence}>
      <div className="meta">Sequence {entry.sequence}</div>
      {entry.kind === 'message' ? <HistoryItem entry={{ position: entry.position, hash: entry.hash, byteLen: new TextEncoder().encode(JSON.stringify(entry.item)).length, item: entry.item } satisfies HistoryEntry} /> :
        entry.kind === 'oversized' ? <p role="status">Message {entry.position} is too large to display ({entry.byteLen} bytes).</p> :
        entry.kind === 'output' ? <><h3>Displayed values</h3><pre>{entry.output.emission.page.text}</pre>{entry.output.emission.page.unavailable && <p>Some detail is unavailable from this renderer.</p>}</> :
          <MountedFormEntry key={entry.form.opening.mountId} form={entry.form} ready={ready} active={active} onAuthExpired={onAuthExpired} />}
    </div>)}</div>}
    {page && !page.entries.length && !page.legacyHistory && !issue && <p>No conversation entries have been retained yet.</p>}
  </section>
}

function MountedFormEntry({ form, ready, active, onAuthExpired }: { form: StoredActorForm; ready: boolean; active: boolean; onAuthExpired?: () => void }) {
  return <><h3>Form · {form.state}</h3><MountedForm form={form} ready={ready} active={active} onAuthExpired={onAuthExpired} />
  </>
}
