import type { ReactNode } from 'react'
import type { HarnessViewModel } from './view-model'
import type { RouteState, Selection, Screen } from './client-contract'
import { actorIdentityKey, type HostActorIdentity } from './protocol'
import { pageRows, type TreeRow } from './selectors'
import { Empty, Pager } from './view-controls'
type SelectionLink = (selection: Selection, label: string, screen?: Screen) => ReactNode
type Inspect = (requestId: string, trigger?: HTMLElement) => void
const identityOf = (actor: NonNullable<HarnessViewModel['actors']>[number]): HostActorIdentity => ({run: actor.run, actor: actor.name, incarnation: actor.incarnation})
const sameSelection = (left: Selection, right: Selection) => JSON.stringify(left) === JSON.stringify(right)

export default function ConversationTreeView({ actors, conversations, link, inspect, route, setPage, setActorPage }: { actors: ReturnType<typeof pageRows<NonNullable<HarnessViewModel['actors']>[number]>>; conversations: ReturnType<typeof pageRows<TreeRow>>; link: SelectionLink; inspect: Inspect; route: RouteState; setPage: (page: number) => void; setActorPage: (page: number) => void }) {
  return <>
          <section aria-label="Host actors">
            <h2>Host actors</h2>
            <div role="table" aria-label="Host actor lifecycles">
              {actors.rows.map(actor => <div className="row" role="row" key={actorIdentityKey(identityOf(actor))} aria-selected={sameSelection(route.selection, { kind: 'actor', identity: identityOf(actor) })}>
                <span role="cell">
                  {link({ kind: 'actor', identity: identityOf(actor) }, actor.name)} · {actor.kind}
                </span>
                <span role="cell">
                  {actor.lifecycle}
                </span>
                <span role="cell">run {actor.run} · incarnation {actor.incarnation}{actor.parentIdentity && <> · Parent {link({ kind: 'actor', identity: actor.parentIdentity }, actor.parentIdentity.actor)}
                </>}{actor.modelConversation && <> · {link({ kind: 'conversation', conversationId: actor.modelConversation }, actor.modelConversation)}
                </>}
                </span>
              </div>)}
            </div>
            <Pager page={actors.page} count={actors.pageCount} total={actors.total} onPage={setActorPage} />
          </section>
          <section aria-label="Conversations">
            <h2>Conversations</h2>
            <div role="table" aria-label="Conversation tree">
              {conversations.rows.map(({ node, depth, lineage }) => <div className="row tree-row" role="row" key={node.id} aria-selected={route.selection.kind === 'conversation' && route.selection.conversationId === node.id}>
                <span role="cell">
                  <span className="branch" aria-hidden="true">
                    {'· '.repeat(Math.min(depth, 8))}
                  </span>
                  {link({ kind: 'conversation', conversationId: node.id }, node.name)}{lineage === 'orphan' || lineage === 'cycle' ? ` · ${lineage}` : ''}
                </span>
                <span role="cell">
                  {node.state}
                </span>
                <span role="cell" className="meta">
                  {node.id} · {[node.model, node.effort, node.detail ?? node.updatedAt].filter(Boolean).join(' · ')}{node.forkSourceRequestId && <button onClick={event => inspect(node.forkSourceRequestId!, event.currentTarget)}>Inspect fork source</button>}
                </span>
              </div>)}
            </div>
            {!conversations.total && <Empty>No conversations are available.</Empty>}
            <Pager page={conversations.page} count={conversations.pageCount} total={conversations.total} onPage={setPage} />
          </section>
        </>;
}
