import { useEffect, useRef, useState } from 'react'
import HistoryItem from './HistoryItem'
import { HistoryReadError, MAX_HISTORY_BYTES, readHistoryPage, type HistoryPage } from './history-client'

const MAX_REQUESTS = 8
const MAX_ITEMS = 200
interface Cursor { requestId: string; offset: number }
interface Loaded { cursor: Cursor; page: HistoryPage }

/** The store retains request-local items; only its parent edges establish lineage. */
export default function ChatHistory({ requestId, refreshKey, ready, onAuthExpired }: {
  requestId: string; refreshKey: string; ready: boolean; onAuthExpired?: () => void
}) {
  const [browsing, setBrowsing] = useState<Cursor>()
  const cursor = browsing ?? { requestId, offset: 0 }
  const [pages, setPages] = useState<Loaded[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [attempt, setAttempt] = useState(0)
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
  function go(next?: Cursor) { setPages([]); setBrowsing(next) }
  return <section className="chat-history" aria-label="Conversation messages" aria-busy={loading}>
    <div className="toolbar"><h2>Conversation</h2><div className="history-controls">
      <button disabled={!ready || loading} onClick={() => setAttempt(value => value + 1)}>Refresh messages</button>
      {browsing && <button disabled={!ready || loading} onClick={() => go()}>Latest messages</button>}
    </div></div>
    {!ready && <p role="status">Host unavailable; retained messages remain visible. Reconnect to refresh or reply.</p>}
    {error && <div role="alert"><p>{error}</p><button disabled={!ready || loading} onClick={() => setAttempt(value => value + 1)}>Retry messages</button></div>}
    {loading && <p role="status">Loading conversation messages…</p>}
    <div role="list" aria-label="Retained conversation items">
      {pages.map(({ page, cursor: source }) => <div className="chat-exchange" key={`${page.requestId}:${source.offset}`}>
        <p className="meta">Exchange {page.requestId}{source.offset > 0 ? ` · offset ${source.offset}` : ''}</p>
        {page.items.map(entry => <HistoryItem key={`${page.requestId}:${entry.position}:${entry.hash}`} entry={entry} />)}
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
