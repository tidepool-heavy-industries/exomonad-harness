import { useEffect, useState } from 'react'

interface HistoryItem {
  readonly position: number;
  readonly hash: string;
  readonly byteLen: number;
  readonly item: unknown;
}

interface OversizedItem {
  readonly position: number;
  readonly hash: string;
  readonly byteLen: number;
  readonly skipOffset: number;
}

interface HistoryPage {
  readonly requestId: string;
  readonly parentId: string | null;
  readonly branch: string;
  readonly items: readonly HistoryItem[];
  readonly nextOffset: number | null;
  readonly oversizedItem: OversizedItem | null;
}

/** Reads retained request Items through the protected, bounded Store route. */
export default function NodeWindow({ requestId, onClose }: { requestId: string; onClose: () => void }) {
  const [items, setItems] = useState<readonly HistoryItem[]>([])
  const [page, setPage] = useState<HistoryPage>()
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string>()

  async function load(offset: number) {
    setLoading(true)
    setError(undefined)
    try {
      const response = await fetch(`/api/history/${encodeURIComponent(requestId)}?offset=${offset}&limit=50`, {
        credentials: 'same-origin',
        cache: 'no-store',
      })
      if (!response.ok && response.status !== 413) {
        throw new Error(response.status === 404 ? 'Request history was not found.'
          : response.status === 503 ? 'Request history is unavailable.'
          : `Request history failed (${response.status}).`)
      }
      const next = await response.json() as HistoryPage
      setPage(next)
      setItems((current) => offset === 0 ? next.items : [...current, ...next.items])
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Request history failed.')
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { void load(0) }, [requestId])

  return <section aria-label="Retained request history">
    <div className="toolbar"><h2>Request history · {requestId}</h2><button type="button" onClick={onClose}>Close history</button></div>
    {page && <p className="meta">Branch {page.branch}{page.parentId ? ` · parent ${page.parentId}` : ''}</p>}
    {error && <p role="alert">{error}</p>}
    {loading && <p role="status">Loading retained history…</p>}
    <div role="list" aria-label="Retained request items" className="rows">
      {items.map((entry) => <article role="listitem" key={`${entry.position}:${entry.hash}`} className="message">
        <strong>Item {entry.position}</strong> <span className="meta">hash {entry.hash} · {entry.byteLen} bytes</span>
        <pre>{JSON.stringify(entry.item, null, 2)}</pre>
      </article>)}
    </div>
    {page?.oversizedItem && <p role="status">
      Item {page.oversizedItem.position} is too large to show ({page.oversizedItem.byteLen} bytes; hash {page.oversizedItem.hash}).
      Full large-item retrieval is unavailable in this view.
      <button type="button" disabled={loading} onClick={() => void load(page.oversizedItem!.skipOffset)}>Skip this item</button>
    </p>}
    {!page?.oversizedItem && page?.nextOffset !== null && page?.nextOffset !== undefined &&
      <button type="button" disabled={loading} onClick={() => void load(page.nextOffset!)}>Next page</button>}
  </section>
}
