import { useEffect, useState, type ReactNode } from 'react'
import { belongsTo } from './live-output'
import ChatHistory, { clearChatHistoryRetention } from './ChatHistory'
import { actorOutputKey, type ActorOutputOrigin } from './actor-output'
import UnifiedChat from './UnifiedChat'
import { clearMountedFormDrafts } from './MountedForm'
import type { RouteState, TransportPhase } from './client-contract'
import { routeUrl } from './navigation'
import { actorIdentityKey, type HostActorIdentity } from './protocol'
import { pageRows, resolveSelection } from './selectors'
import type { HarnessViewModel } from './view-model'
import './WorkerChat.css'

type Actor = NonNullable<HarnessViewModel['actors']>[number]
type Request = HarnessViewModel['timeline'][number]
type Head = Pick<Request, 'id'>
interface RetainedChat { conversationId?: string; head?: Head; outputOrigin?: ActorOutputOrigin }
const retainedChats = new Map<string, RetainedChat>()
const MAX_RETAINED_CHATS = 8

/** Clear alongside drafts when the user deliberately signs out. */
export function clearWorkerChatRetention() {
  retainedChats.clear()
  clearChatHistoryRetention()
  clearMountedFormDrafts()
}

function identityOf(actor: Actor): HostActorIdentity {
  return { run: actor.run, actor: actor.name, incarnation: actor.incarnation }
}

function WorkflowMessages({ data, identity }: { data: HarnessViewModel; identity: HostActorIdentity }) {
  const [page, setPage] = useState(0)
  const messages = pageRows(data.inbox.filter(message => message.sender === identity.actor || message.recipient === identity.actor), page, 50)
  return <section tabIndex={0} aria-label="Worker mailbox observations">
    <h3>Mailbox messages</h3>
    <p className="hint">Mailbox endpoint labels do not establish an actor incarnation. Workflow actors have no model exchange.</p>
    <div role="list" aria-label="Workflow messages">{messages.rows.map(message => <div className="message" role="listitem" key={message.id}>
      <strong>{message.sender}{message.recipient && ` → ${message.recipient}`}</strong>
      <span className="meta">{message.type ?? message.state}{message.ordinal !== undefined && ` · #${message.ordinal}`}{message.receivedAt && ` · ${message.receivedAt}`}</span>
      <p>{message.message}</p>
    </div>)}</div>
    {!messages.total && <p>No mailbox messages are retained for this worker endpoint yet.</p>}
    {messages.pageCount > 1 && <div className="history-controls">
      <button disabled={messages.page === 0} onClick={() => setPage(messages.page - 1)}>Previous messages</button>
      <span>Page {messages.page + 1} of {messages.pageCount}</span>
      <button disabled={messages.page + 1 === messages.pageCount} onClick={() => setPage(messages.page + 1)}>Next messages</button>
    </div>}
  </section>
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
  const outputOrigin = resolved.actor?.outputOrigin ?? retained?.outputOrigin
  const conversationId = !issue && identity && (resolved.actor?.kind === 'model' || resolved.missing)
    ? resolved.conversationId ?? retained?.conversationId : undefined
  const exactHead = !issue && identity && resolved.actor?.kind === 'model' ? resolved.actor.modelHeadRequest : undefined
  const ambiguousConversation = conversationId !== undefined && new Set((data.actors ?? [])
    .filter(actor => actor.kind === 'model' && actor.modelConversation === conversationId)
    .map(actor => actorIdentityKey(identityOf(actor)))).size > 1
  // Conversation association alone cannot establish head ownership across incarnations.
  const requests = !ambiguousConversation && resolved.actor?.kind === 'model' && resolved.conversationId
    ? data.timeline.filter(item => item.kind === 'request' && item.nodeId === resolved.conversationId) : []
  const parents = new Set(requests.flatMap(item => item.parentId ? [item.parentId] : []))
  const currentHead = exactHead ? { id: exactHead } : requests.filter(item => !parents.has(item.id)).at(-1)
  const head = currentHead ?? (retained?.conversationId === conversationId ? retained?.head : undefined)
  const liveOutput = identity ? (data.liveOutput ?? []).filter(output => belongsTo(output, identity)) : []
  const refreshKey = JSON.stringify([exactHead, data.timeline.find(item => item.kind === 'request' && item.id === exactHead)?.historyRefreshKey,
    data.nodes.find(node => node.id === conversationId)?.version,
    requests.map(item => [item.id, item.historyRefreshKey])])
  useEffect(() => {
    if (!context || !identity || (!conversationId && !outputOrigin)) return
    retainedChats.delete(context)
    retainedChats.set(context, { conversationId: conversationId ?? retained?.conversationId, head: head ? { id: head.id } : retained?.head, outputOrigin })
    while (retainedChats.size > MAX_RETAINED_CHATS) retainedChats.delete(retainedChats.keys().next().value!)
  }, [context, identity, conversationId, head, outputOrigin])
  const workers = [...(data.actors ?? [])].sort((a, b) =>
    Number(!['running', 'waiting'].includes(a.lifecycle)) - Number(!['running', 'waiting'].includes(b.lifecycle))
    || a.name.localeCompare(b.name) || a.incarnation.localeCompare(b.incarnation))
  const shown = pageRows(workers.filter(actor => `${actor.name} ${actor.kind} ${actor.incarnation} ${actor.lifecycle}`.toLowerCase().includes(search.toLowerCase())), page, 100)
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
        <span className="meta">{actor.kind} · {actor.lifecycle} · incarnation {actor.incarnation}</span>
        <span className="meta">Run {actor.run}</span>
      </li>)}</ul>
      {!shown.total && <p>No workers match.</p>}
      {shown.pageCount > 1 && <div className="history-controls">
        <button disabled={shown.page === 0} onClick={() => setPage(shown.page - 1)}>Previous workers</button>
        <span>Page {shown.page + 1} of {shown.pageCount}</span>
        <button disabled={shown.page + 1 === shown.pageCount} onClick={() => setPage(shown.page + 1)}>Next workers</button>
      </div>}
    </aside>
    <section className="worker-chat-conversation" aria-label="Selected worker Chat">
      {identity && <header className="actor-heading">
        <h2>{identity.actor}</h2>
        <p className="meta">run {identity.run} · incarnation {identity.incarnation}</p>
        {resolved.actor && <dl className="actor-details">
          <dt>Actor</dt><dd>{resolved.actor.kind} · {resolved.actor.lifecycle}</dd>
          {resolved.actor.parentIdentity && <><dt>Parent</dt><dd>{workerLink(resolved.actor.parentIdentity, resolved.actor.parentIdentity.actor)} · incarnation {resolved.actor.parentIdentity.incarnation}</dd></>}
          {resolved.conversationId && <><dt>Conversation</dt><dd className="mono">{resolved.conversationId}</dd></>}
          {resolved.actor.activeRound && <><dt>Active round</dt><dd className="mono">{resolved.actor.activeRound}</dd></>}
          {exactHead && <><dt>History head</dt><dd className="mono">{exactHead}</dd></>}
        </dl>}
      </header>}
      {issue ? <p role="alert">{issue}</p> : !identity ? <p>Select a worker to open its messages and controls.</p> :
        <>
          {resolved.actor && !['running', 'waiting'].includes(resolved.actor.lifecycle) &&
            <p role="status">This actor is {resolved.actor.lifecycle}. Its Chat is read-only; retained history remains available.</p>}
          {resolved.missing && <p role="status">This exact actor is unavailable. Retained history remains read-only; choose a different worker explicitly.</p>}
          {ambiguousConversation && !exactHead && <p role="status">The host associates this conversation with multiple exact actors. Its current history head is unavailable; only previously retained exact history can be shown.</p>}
          {outputOrigin && <UnifiedChat key={actorOutputKey(outputOrigin)}
            origin={outputOrigin} requestId={head?.id ?? exactHead}
            revision={(data.actorOutputRevisions ?? []).find(reference => actorOutputKey(reference.origin) === actorOutputKey(outputOrigin))?.sequence}
            ready={transportPhase === 'ready'} active={!!resolved.actor && ['running', 'waiting'].includes(resolved.actor.lifecycle)} onAuthExpired={onAuthExpired}
            liveOutput={liveOutput} requests={new Map(data.timeline.filter(item => item.kind === 'request').map(item => [item.id, item]))}
            legacy={resolved.actor?.kind === 'workflow' ? undefined : (conversationId || exactHead) && head ? <ChatHistory key={JSON.stringify([context, conversationId])}
            cacheKey={JSON.stringify([context, conversationId])} requestId={head.id}
            requests={new Map(data.timeline.filter(item => item.kind === 'request').map(item => [item.id, item]))}
            historyRevisions={(data.historyRevisions ?? []).filter(revision => revision.origin.kind === 'embedded' && revision.origin.run === identity.run)} active={!!resolved.actor && ['running', 'waiting'].includes(resolved.actor.lifecycle)} refreshKey={refreshKey} ready={transportPhase === 'ready'} onAuthExpired={onAuthExpired} /> :
            <p>{ambiguousConversation ? 'No exact history head is available for this actor.' : resolved.missing ? 'The host has no retained conversation association for this exact actor.' : 'No retained model exchange is available yet.'}</p>} />}
          {resolved.actor?.kind === 'workflow' ? <WorkflowMessages key={context} data={data} identity={identity} /> : !outputOrigin ?
            (conversationId || exactHead) && head ? <ChatHistory key={JSON.stringify([context, conversationId])}
              cacheKey={JSON.stringify([context, conversationId])} requestId={head.id}
              requests={new Map(data.timeline.filter(item => item.kind === 'request').map(item => [item.id, item]))}
              historyRevisions={(data.historyRevisions ?? []).filter(revision => revision.origin.kind === 'embedded' && revision.origin.run === identity.run)} active={!!resolved.actor && ['running', 'waiting'].includes(resolved.actor.lifecycle)} liveOutput={liveOutput} refreshKey={refreshKey} ready={transportPhase === 'ready'} onAuthExpired={onAuthExpired} /> :
              <p>{ambiguousConversation ? 'No exact history head is available for this actor.' : resolved.missing ? 'The host has no retained conversation association for this exact actor.' : 'No retained model exchange is available yet.'}</p> : null}
        </>}
      {children}
    </section>
  </div>
}
