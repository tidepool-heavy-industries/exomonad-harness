import type { ReactNode } from 'react'
import type { HarnessViewModel } from './view-model'
import type { RouteState, Selection, Screen } from './client-contract'
import { actorIdentityKey, type HostActorIdentity } from './protocol'
import { pageRows } from './selectors'
import { Empty, Pager } from './view-controls'
type SelectionLink = (selection: Selection, label: string, screen?: Screen) => ReactNode
const identityOf = (actor: NonNullable<HarnessViewModel['actors']>[number]): HostActorIdentity => ({run: actor.run, actor: actor.name, incarnation: actor.incarnation})

export default function InboxView({ inbox, endpoints, endpointNodes, endpointActors, link, route, navigate, setPage }: { inbox: ReturnType<typeof pageRows<HarnessViewModel['inbox'][number]>>; endpoints: string[]; endpointNodes: Map<string, HarnessViewModel['nodes']>; endpointActors: Map<string, NonNullable<HarnessViewModel['actors']>>; link: SelectionLink; route: RouteState; navigate: (route: RouteState) => void; setPage: (page: number) => void }) {
  return <>
          <div className="toolbar filters">
            {(['sender', 'recipient'] as const).map(key => <label key={key}>
              {key === 'sender' ? 'Sender' : 'Recipient'}
              <select value={route.messageFilters[key] ?? ''} onChange={event => navigate({ ...route, messageFilters: { ...route.messageFilters, [key]: event.target.value || undefined } })}>
                <option value="">All endpoints</option>
                {route.messageFilters[key] && !endpoints.includes(route.messageFilters[key]!) && <option>
                  {route.messageFilters[key]}
                </option>}{endpoints.map(endpoint => <option key={endpoint}>
                  {endpoint}
                </option>)}
              </select>
            </label>)}
            <label>Message type<select value={route.messageFilters.type ?? ''} onChange={event => navigate({ ...route, messageFilters: { ...route.messageFilters, type: event.target.value as RouteState['messageFilters']['type'] || undefined } })}>
              <option value="">All types</option>
              {['NEW_TASK', 'MESSAGE', 'FINAL_ANSWER', 'PROGRESS'].map(kind => <option key={kind}>
                {kind}
              </option>)}
            </select>
            </label>
          </div>
          <p className="hint">Filters match supplied endpoint labels. They do not prove an actor incarnation or that a question was answered.</p>
          <div role="list" aria-label="Inbox messages">
            {inbox.rows.map(item => <div className="row" role="listitem" key={item.id}>
              <strong>
                {item.sender}{item.recipient && ` → ${item.recipient}`}
              </strong>
              <span>
                {item.type ?? item.state}{item.ordinal !== undefined && ` · #${item.ordinal}`}
              </span>
              <span className="message">
                {item.message}
                <span className="meta">
                  {item.receivedAt}
                </span>
              </span>
              <div>
                {[...new Set([item.sender, item.recipient].filter((endpoint): endpoint is string => Boolean(endpoint)))].map(endpoint => {
                  return <span key={endpoint}>
                    {(endpointNodes.get(endpoint) ?? []).map(node => <span key={node.id}>
                      {link({ kind: 'conversation', conversationId: node.id }, `Open conversation ${node.name} · ${node.id}`)} </span>)}{(endpointActors.get(endpoint) ?? []).map(actor => <span key={actorIdentityKey(identityOf(actor))}>
                        {link({ kind: 'actor', identity: identityOf(actor) }, `Select actor ${actor.name} · run ${actor.run} · incarnation ${actor.incarnation}`)} </span>)}
                    <button onClick={() => navigate({ ...route, messageFilters: { ...route.messageFilters, sender: endpoint } })}>Filter {endpoint}
                    </button>
                  </span>;
                })}
              </div>
            </div>)}
          </div>
          {!inbox.total && <Empty>No messages match this context and filters.</Empty>}
          <Pager page={inbox.page} count={inbox.pageCount} total={inbox.total} onPage={setPage} />
        </>;
}
