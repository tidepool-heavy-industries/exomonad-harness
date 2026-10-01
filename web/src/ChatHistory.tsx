import { useEffect, useRef, useState } from 'react'
import HistoryItem from './HistoryItem'
import { HistoryReadError, MAX_HISTORY_BYTES, readHistoryPage, type HistoryPage } from './history-client'
import type { HarnessViewModel } from './view-model'

const MAX_REQUESTS = 8
const MAX_ITEMS = 200
interface Cursor { requestId: string; offset: number }
interface Loaded { cursor: Cursor; page: HistoryPage }
interface RetainedSlice { browsing?: Cursor; pages: Loaded[]; failures: Map<string, RequestStatus> }
const retainedSlices = new Map<string, RetainedSlice>()
const MAX_RETAINED_SLICES = 4

/** Message content stays in memory and is cleared on deliberate signout. */
export function clearChatHistoryRetention() { retainedSlices.clear() }

function retainSlice(key: string, slice: RetainedSlice) {
  retainedSlices.delete(key)
  retainedSlices.set(key, slice)
  while (retainedSlices.size > MAX_RETAINED_SLICES) retainedSlices.delete(retainedSlices.keys().next().value!)
}
type RequestStatus = Pick<HarnessViewModel['timeline'][number], 'id' | 'state' | 'detail' | 'failure'>

function FailedExchange({ request }: { request: RequestStatus }) {
  const failure = request.failure
  const diagnostic = failure?.kind === 'http' ? failure.diagnostic : undefined
  return <div className="error" role="alert" aria-label={`Failed exchange ${request.id}`}>
    <strong>Exchange failed</strong>
    <p className="meta">Exchange {request.id}</p>
    {failure?.kind === 'authentication' ? <p>Provider authentication failed. Check the host's provider credentials before sending a new reply.</p> :
      failure?.kind === 'http' ? <>
        <p>Provider returned HTTP {failure.status}.</p>
        {diagnostic && <>
          {diagnostic.message && <p>{diagnostic.message}</p>}
          <dl>{diagnostic.code && <><dt>Code</dt><dd>{diagnostic.code}</dd></>}
            {diagnostic.error_type && <><dt>Type</dt><dd>{diagnostic.error_type}</dd></>}
            {diagnostic.param && <><dt>Parameter</dt><dd>{diagnostic.param}</dd></>}</dl>
        </>}
      </> : <p>{request.detail ?? 'The model request failed.'}</p>}
  </div>
}

/** The store retains request-local items; only its parent edges establish lineage. */
export default function ChatHistory({ requestId, requests, refreshKey, ready, onAuthExpired, cacheKey }: {
  requestId: string; requests: ReadonlyMap<string, RequestStatus>; refreshKey: string; ready: boolean; onAuthExpired?: () => void; cacheKey?: string
}) {
  const initial = useRef(cacheKey ? retainedSlices.get(cacheKey) : undefined)
  const [browsing, setBrowsing] = useState<Cursor | undefined>(initial.current?.browsing)
  const cursor = browsing ?? { requestId, offset: 0 }
  const [pages, setPages] = useState<Loaded[]>(initial.current?.pages ?? [])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [attempt, setAttempt] = useState(0)
  const retainedFailures = useRef(new Map<string, RequestStatus>(initial.current?.failures))
  initial.current = undefined
  const auth = useRef(onAuthExpired)
  auth.current = onAuthExpired
  useEffect(() => {
    if (!ready) { setLoading(false); return }
    const controller = new AbortController()
    setLoading(true)
    setError('')
    const loaded: Loaded[] = []
    void (async () => {
      const visited = new Set<string>()
      let next: Cursor | undefined = cursor
      let bytes = 0
      let count = 0
      while (next && loaded.length < MAX_REQUESTS) {
        if (visited.has(next.requestId)) throw new Error('Retained history has a cyclic parent link.')
        visited.add(next.requestId)
        const page = await readHistoryPage(next.requestId, next.offset, controller.signal)
        if (controller.signal.aborted) return
        const pageBytes = page.items.reduce((total, entry) => total + entry.byteLen, 0)
        if (loaded.length && (bytes + pageBytes > MAX_HISTORY_BYTES || count + page.items.length > MAX_ITEMS)) break
        loaded.push({ cursor: next, page })
        bytes += pageBytes
        count += page.items.length
        // Offset paging stays within one request. Never silently omit a large item.
        if (page.nextOffset !== null || next.offset !== 0) break
        next = page.parentId === null ? undefined : { requestId: page.parentId, offset: 0 }
      }
      if (!controller.signal.aborted) setPages(loaded.reverse())
    })().catch((cause: unknown) => {
      if (controller.signal.aborted) return
      if (loaded.length) setPages(previous => previous.length ? previous : [...loaded].reverse())
      setError(cause instanceof Error ? cause.message : 'Conversation history failed.')
      if (cause instanceof HistoryReadError && cause.kind === 'authentication') auth.current?.()
    }).finally(() => { if (!controller.signal.aborted) setLoading(false) })
    return () => controller.abort()
  }, [cursor.requestId, cursor.offset, refreshKey, ready, attempt])
  const oldest = pages[0]
  const paged = pages.find(({ page }) => page.nextOffset !== null)
  // Retain only evidence for the current slice and its exact cursor during an outage.
  const visibleRequests = new Set([cursor.requestId, ...pages.map(({ page }) => page.requestId)])
  for (const id of visibleRequests) {
    const status = requests.get(id)
    if (status?.state === 'failed') retainedFailures.current.set(id, status)
    else if (status) retainedFailures.current.delete(id)
  }
  for (const id of retainedFailures.current.keys()) if (!visibleRequests.has(id)) retainedFailures.current.delete(id)
  useEffect(() => {
    if (cacheKey) retainSlice(cacheKey, { browsing, pages, failures: new Map(retainedFailures.current) })
  }, [cacheKey, browsing, pages, requests])
  const cursorFailure = !pages.some(({ page }) => page.requestId === cursor.requestId) ? retainedFailures.current.get(cursor.requestId) : undefined
  function go(next?: Cursor) { setPages([]); setBrowsing(next) }
  return <section className="chat-history" aria-label="Conversation messages" aria-busy={loading}>
    <div className="toolbar"><h2>Conversation</h2><div className="history-controls">
      <button disabled={!ready || loading} onClick={() => setAttempt(value => value + 1)}>Refresh messages</button>
      {browsing && <button disabled={!ready || loading} onClick={() => go()}>Latest messages</button>}
    </div></div>
    {!ready && <p role="status">Host unavailable; retained messages remain visible. Reconnect to refresh or reply.</p>}
    {error && <div role="alert"><p>{error}</p><button disabled={!ready || loading} onClick={() => setAttempt(value => value + 1)}>Retry messages</button></div>}
    {loading && <p role="status">Loading conversation messages…</p>}
    {cursorFailure && <FailedExchange request={cursorFailure} />}
    <div role="list" aria-label="Retained conversation items">
      {pages.map(({ page, cursor: source }) => <div className="chat-exchange" key={`${page.requestId}:${source.offset}`}>
        <p className="meta">Exchange {page.requestId}{source.offset > 0 ? ` · offset ${source.offset}` : ''}</p>
        {page.items.map(entry => <HistoryItem key={`${page.requestId}:${entry.position}:${entry.hash}`} entry={entry} />)}
        {retainedFailures.current.has(page.requestId) && <FailedExchange request={retainedFailures.current.get(page.requestId)!} />}
      </div>)}
    </div>
    {!loading && !error && pages.length > 0 && pages.every(({ page }) => page.items.length === 0) && <p>No retained messages in this slice.</p>}
    {paged?.page.oversizedItem && <p role="status">Item {paged.page.oversizedItem.position} is too large to display ({paged.page.oversizedItem.byteLen} bytes).
      <button disabled={!ready || loading} onClick={() => go({ requestId: paged.page.requestId, offset: paged.page.oversizedItem!.skipOffset })}>Skip large item</button></p>}
    <div className="history-controls">
      {paged && !paged.page.oversizedItem && <button disabled={!ready || loading} onClick={() => go({ requestId: paged.page.requestId, offset: paged.page.nextOffset! })}>More items in this exchange</button>}
      {oldest?.page.parentId && <button disabled={!ready || loading} onClick={() => go({ requestId: oldest.page.parentId!, offset: 0 })}>Earlier exchanges</button>}
    </div>
  </section>
}
