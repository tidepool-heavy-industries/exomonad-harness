import { useEffect, useRef, useState } from 'react'
import type { NodeWindowProps } from './client-contract'
import HistoryItem from './HistoryItem'
import { HistoryReadError, readHistoryPage, type HistoryPage } from './history-client'

interface LoadedPage {
  readonly page: HistoryPage
  readonly offset: string
  readonly previous?: PageOffset
}

interface PageOffset {
  readonly offset: string
  readonly previous?: PageOffset
}

interface ReadIntent {
  readonly offset: string
  readonly previous?: PageOffset
  readonly attempt: number
}

function visiblePageIntent(current: ReadIntent, loaded?: LoadedPage): ReadIntent {
  return { offset: loaded?.offset ?? current.offset, previous: loaded ? loaded.previous : current.previous, attempt: current.attempt + 1 }
}

/** One bounded retained page belongs to one exact inspection context. */
export default function NodeWindow({ requestId, conversationId, hostRun, refreshKey, onClose, onAuthExpired }: NodeWindowProps) {
  const context = JSON.stringify([requestId, conversationId, hostRun])
  const heading = useRef<HTMLHeadingElement>(null)
  const [loaded, setLoaded] = useState<LoadedPage>()
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string>()
  const [intent, setIntent] = useState<ReadIntent>({ offset: '0', attempt: 0 })
  const currentContext = useRef(context)
  currentContext.current = context
  const generation = useRef(0)
  const authCallback = useRef(onAuthExpired)
  authCallback.current = onAuthExpired
  const [stateContext, setStateContext] = useState(context)
  const [stateRefreshKey, setStateRefreshKey] = useState(refreshKey)

  useEffect(() => { heading.current?.focus() }, [context])

  // A context change resets paging before rendering any data from the old host.
  if (stateContext !== context) {
    setStateContext(context)
    setLoaded(undefined)
    setError(undefined)
    setIntent({ offset: '0', attempt: 0 })
    setStateRefreshKey(refreshKey)
  } else if (stateRefreshKey !== refreshKey) {
    setStateRefreshKey(refreshKey)
    setIntent((current) => visiblePageIntent(current, loaded))
  }

  useEffect(() => {
    const controller = new AbortController()
    const readGeneration = ++generation.current
    const isCurrent = () => !controller.signal.aborted && generation.current === readGeneration && currentContext.current === context
    setLoading(true)
    setError(undefined)
    void readHistoryPage(requestId, intent.offset, controller.signal).then((page) => {
      if (isCurrent()) setLoaded({ page, offset: intent.offset, previous: intent.previous })
    }).catch((cause: unknown) => {
      if (!isCurrent()) return
      setError(cause instanceof Error ? cause.message : 'Request history failed.')
      if (cause instanceof HistoryReadError && cause.kind === 'authentication') authCallback.current?.()
    }).finally(() => {
      if (isCurrent()) setLoading(false)
    })
    return () => controller.abort()
  }, [context, requestId, intent])

  function reload() {
    setIntent((current) => ({ ...current, attempt: current.attempt + 1 }))
  }

  function refresh() {
    setIntent((current) => visiblePageIntent(current, loaded))
  }

  function next(offset: string) {
    if (!loaded) return
    setIntent({ offset, previous: { offset: loaded.offset, previous: loaded.previous }, attempt: 0 })
  }

  function previous() {
    if (!loaded) return
    const previous = loaded.previous
    if (previous) setIntent({ offset: previous.offset, previous: previous.previous, attempt: 0 })
  }

  const page = loaded?.page
  return <section aria-label="Retained request history" aria-busy={loading}>
    <div className="toolbar"><h2 ref={heading} tabIndex={-1}>Request history · {requestId}</h2><div className="history-controls">
      <button type="button" disabled={loading} onClick={refresh}>Refresh history</button>
      <button type="button" onClick={onClose}>Close history</button>
    </div></div>
    {page && <p className="meta">Branch {page.branch}{page.parentId !== null ? ` · parent ${page.parentId}` : ''} · Offset {loaded?.offset} · {page.items.length} items</p>}
    {error && <div role="alert"><p>{error}</p><button type="button" disabled={loading} onClick={reload}>Retry history</button></div>}
    {loading && <p role="status">Loading retained history…</p>}
    {!loading && page && page.items.length === 0 && !page.oversizedItem && <p>No retained items at this offset.</p>}
    <div role="list" aria-label="Retained request items" className="rows">
      {page?.items.map((entry) => <HistoryItem key={`${context}:${loaded?.offset}:${entry.position}:${entry.hash}`} entry={entry} />)}
    </div>
    {page?.oversizedItem && <p role="status">
      Item {page.oversizedItem.position} is too large to show ({page.oversizedItem.byteLen} bytes; hash {page.oversizedItem.hash}).
      Full large-item retrieval is unavailable in this view.
      <button type="button" disabled={loading} onClick={() => next(page.oversizedItem!.skipOffset)}>Skip this item</button>
    </p>}
    <div className="history-controls">
      <button type="button" disabled={loading || !loaded?.previous} onClick={previous}>Previous page</button>
      {!page?.oversizedItem && page?.nextOffset !== null && page?.nextOffset !== undefined &&
        <button type="button" disabled={loading} onClick={() => next(page.nextOffset!)}>Next page</button>}
    </div>
  </section>
}
