import { useEffect, useState } from 'react'
import { actorOutputKey, isActorOutputOrigin, isStoredActorOutput, type ActorOutputOrigin, type StoredActorOutput } from './actor-output'

export default function ActorOutput({ origin, revision, ready, active, onAuthExpired }: {
  origin: ActorOutputOrigin; revision?: number; ready: boolean; active: boolean; onAuthExpired?: () => void
}) {
  const [after, setAfter] = useState(0)
  const [previous, setPrevious] = useState<number[]>([])
  const [rows, setRows] = useState<StoredActorOutput[]>([])
  const [next, setNext] = useState<number | null>(null)
  const [issue, setIssue] = useState<string>()
  const [busy, setBusy] = useState<string>()
  const [refresh, setRefresh] = useState(0)
  const identity = actorOutputKey(origin)
  useEffect(() => {
    if (!ready) return
    const cancel = new AbortController()
    const query = new URLSearchParams({ run: origin.run, actor: String(origin.nativeActor), incarnation: String(origin.incarnation), after: String(after), limit: '50' })
    setIssue(undefined)
    void fetch(`/api/actor-output?${query}`, { credentials: 'same-origin', signal: cancel.signal }).then(async response => {
      if (response.status === 401 || response.status === 403) { onAuthExpired?.(); throw new Error('Sign in to read actor output.') }
      if (!response.ok) throw new Error('Actor output history is unavailable.')
      const raw = await response.text()
      if (new TextEncoder().encode(raw).length > 256 * 1024) throw new Error('Actor output history exceeded its page bound.')
      const page = JSON.parse(raw) as Record<string, unknown>
      if (!isActorOutputOrigin(page.origin) || actorOutputKey(page.origin) !== identity || !Array.isArray(page.outputs)
        || page.outputs.length > 100 || !page.outputs.every(isStoredActorOutput)
        || page.outputs.some(output => actorOutputKey(output.reference.origin) !== identity)
        || !(page.nextAfter === null || Number.isSafeInteger(page.nextAfter) && (page.nextAfter as number) > after)) {
        throw new Error('Actor output history has an invalid page.')
      }
      if (!cancel.signal.aborted) { setRows(page.outputs); setNext(page.nextAfter as number | null) }
    }).catch(error => { if (!cancel.signal.aborted) setIssue(String(error.message ?? error)) })
    return () => cancel.abort()
  }, [identity, after, revision, ready, refresh, onAuthExpired])
  async function expand(output: StoredActorOutput, key: number) {
    const token = `${output.reference.sequence}:${key}`
    setBusy(token); setIssue(undefined)
    try {
      const response = await fetch('/api/actor-output/expand', { method: 'POST', credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ origin, displaySlot: output.emission.id.displaySlot, key }) })
      if (response.status === 401 || response.status === 403) onAuthExpired?.()
      if (!response.ok) throw new Error(response.status === 409 ? 'This detail is no longer available. Refresh the values to see the current expansion keys.' : 'This detail could not be expanded.')
      setRefresh(value => value + 1)
    } catch (error) { setIssue(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(undefined) }
  }
  return <section aria-label="Displayed values">
    <h3>Displayed values</h3>
    {issue && <p role="alert">{issue}</p>}
    {!rows.length && !issue && <p>No values have been displayed yet.</p>}
    <div role="list">{rows.map(output => <div role="listitem" key={output.reference.sequence}>
      <pre>{output.emission.page.text}</pre>
      {output.emission.page.unavailable && <p>Some detail is unavailable from this renderer.</p>}
      {output.emission.page.expansions.map(([key, label]) => <button key={key} disabled={!active || !ready || busy !== undefined}
        onClick={() => void expand(output, key)}>{busy === `${output.reference.sequence}:${key}` ? 'Loading…' : `Show ${label}`}</button>)}
    </div>)}</div>
    <div className="history-controls">
      <button disabled={!previous.length} onClick={() => { setAfter(previous.at(-1)!); setPrevious(previous.slice(0, -1)) }}>Previous values</button>
      <button disabled={next === null} onClick={() => { setPrevious([...previous, after]); setAfter(next!) }}>Next values</button>
      <button disabled={!ready} onClick={() => setRefresh(value => value + 1)}>Refresh values</button>
    </div>
  </section>
}
