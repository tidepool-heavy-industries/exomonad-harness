import { useEffect, useState, type ReactNode } from 'react'
import ChatHistory, { clearChatHistoryRetention } from './ChatHistory'
import type { RouteState, TransportPhase } from './client-contract'
import { routeUrl } from './navigation'
import { actorIdentityKey, type HostActorIdentity } from './protocol'
import { pageRows, resolveSelection } from './selectors'
import type { HarnessViewModel } from './view-model'
import './WorkerChat.css'

type Actor = NonNullable<HarnessViewModel['actors']>[number]
type Request = HarnessViewModel['timeline'][number]
type Head = Pick<Request, 'id'>
interface RetainedChat { conversationId: string; head?: Head }
const retainedChats = new Map<string, RetainedChat>()
const MAX_RETAINED_CHATS = 8

/** Clear alongside drafts when the user deliberately signs out. */
export function clearWorkerChatRetention() {
  retainedChats.clear()
  clearChatHistoryRetention()
}

function identityOf(actor: Actor): HostActorIdentity {
  return { run: actor.run, actor: actor.name, incarnation: actor.incarnation }
}

export default function WorkerChat({ data, route, navigate, transportPhase, issue, onAuthExpired, children }: {
  data: HarnessViewModel; route: RouteState; navigate: (route: RouteState) => void
  transportPhase: TransportPhase; issue?: string; onAuthExpired?: () => void; children?: ReactNode
}) {
  const [search, setSearch] = useState('')
  const [page, setPage] = useState(0)
  const resolved = resolveSelection(data, route.selection)
  const identity = route.selection.kind === 'actor' ? route.selection.identity : undefined
  const context = identity ? actorIdentityKey(identity) : undefined
  const retained = context ? retainedChats.get(context) : undefined
  const conversationId = !issue && identity && (resolved.actor?.kind === 'model' || resolved.missing)
    ? resolved.conversationId ?? retained?.conversationId : undefined
  const requests = data.timeline.filter(item => item.kind === 'request' && item.nodeId === conversationId)
  const parents = new Set(requests.flatMap(item => item.parentId ? [item.parentId] : []))
  const currentHead = requests.filter(item => !parents.has(item.id)).at(-1)
  const head = currentHead ?? (retained?.conversationId === conversationId ? retained?.head : undefined)
  const refreshKey = JSON.stringify([data.nodes.find(node => node.id === conversationId)?.version,
    requests.map(item => [item.id, item.historyRefreshKey])])
  useEffect(() => {
    if (!context || !identity || !conversationId) return
    retainedChats.delete(context)
    retainedChats.set(context, { conversationId, head: head ? { id: head.id } : undefined })
    while (retainedChats.size > MAX_RETAINED_CHATS) retainedChats.delete(retainedChats.keys().next().value!)
  }, [context, identity, conversationId, head])
  const workers = (data.actors ?? []).filter(actor => actor.kind === 'model').sort((a, b) =>
    Number(!['running', 'waiting'].includes(a.lifecycle)) - Number(!['running', 'waiting'].includes(b.lifecycle))
    || a.name.localeCompare(b.name) || a.incarnation.localeCompare(b.incarnation))
  const shown = pageRows(workers.filter(actor => `${actor.name} ${actor.incarnation} ${actor.lifecycle}`.toLowerCase().includes(search.toLowerCase())), page, 100)
  function workerLink(target: HostActorIdentity, label: ReactNode) {
    const next: RouteState = { ...route, screen: 'chat', selection: { kind: 'actor', identity: target }, requestId: undefined, global: false }
    return <a href={routeUrl(next, new URL(window.location.href)).toString()}
      aria-current={context === actorIdentityKey(target) ? 'page' : undefined}
      onClick={event => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
        event.preventDefault(); navigate(next)
      }}>{label}</a>
  }
  return <div className="worker-chat">
    <aside className="worker-chat-list" aria-label="Workers">
      <h2>Workers</h2>
      <label>Find worker<input type="search" value={search} onChange={event => { setSearch(event.target.value); setPage(0) }} /></label>
      {identity && !resolved.actor && <p>{workerLink(identity, identity.actor)} · selected exact actor unavailable</p>}
      <ul>{shown.rows.map(actor => <li key={actorIdentityKey(identityOf(actor))}>
        {workerLink(identityOf(actor), actor.name)}
        <span className="meta">{actor.lifecycle} · incarnation {actor.incarnation}</span>
        <span className="meta">Run {actor.run}</span>
      </li>)}</ul>
      {!shown.total && <p>No model workers match.</p>}
      {shown.pageCount > 1 && <div className="history-controls">
        <button disabled={shown.page === 0} onClick={() => setPage(shown.page - 1)}>Previous workers</button>
        <span>Page {shown.page + 1} of {shown.pageCount}</span>
        <button disabled={shown.page + 1 === shown.pageCount} onClick={() => setPage(shown.page + 1)}>Next workers</button>
      </div>}
    </aside>
    <section className="worker-chat-conversation" aria-label="Selected worker Chat">
      {identity && <p className="meta">{identity.actor} · run {identity.run} · incarnation {identity.incarnation}</p>}
      {issue ? <p role="alert">{issue}</p> : !identity ? <p>Select an exact model worker to open its Chat.</p> :
        resolved.actor?.kind === 'workflow' ? <p role="status">This workflow actor has no model Chat. Open Host to inspect its commands.</p> : <>
          {resolved.actor && !['running', 'waiting'].includes(resolved.actor.lifecycle) &&
            <p role="status">This actor is {resolved.actor.lifecycle}. Its Chat is read-only; retained history remains available.</p>}
          {resolved.missing && <p role="status">This exact actor is unavailable. Retained history remains read-only; choose a different worker explicitly.</p>}
          {conversationId && head ? <ChatHistory key={JSON.stringify([context, conversationId])}
            cacheKey={JSON.stringify([context, conversationId])} requestId={head.id}
            requests={new Map(data.timeline.filter(item => item.kind === 'request').map(item => [item.id, item]))}
            refreshKey={refreshKey} ready={transportPhase === 'ready'} onAuthExpired={onAuthExpired} /> :
            <p>{resolved.missing ? 'The host has no retained conversation association for this exact actor.' : 'No retained model exchange is available yet.'}</p>}
        </>}
      {children}
    </section>
  </div>
}
