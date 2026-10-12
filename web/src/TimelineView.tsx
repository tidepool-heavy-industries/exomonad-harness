import type { ReactNode } from 'react'
import type { HarnessViewModel } from './view-model'
import type { Selection, Screen } from './client-contract'
import { formatActivityTime, pageRows } from './selectors'
import { Empty, Pager } from './view-controls'
type SelectionLink = (selection: Selection, label: string, screen?: Screen) => ReactNode
type Inspect = (requestId: string, trigger?: HTMLElement) => void

export default function TimelineView({ activity, now, inspect, link, setPage }: { activity: ReturnType<typeof pageRows<HarnessViewModel['timeline'][number]>>; now: number; inspect: Inspect; link: SelectionLink; setPage: (page: number) => void }) {
  return <>
    <div role="table" aria-label="Conversation activity timeline" className="rows">
      <div className="row" role="row">
        <strong role="columnheader">Activity</strong>
        <strong role="columnheader">State</strong>
        <strong role="columnheader">Source and time</strong>
      </div>
      {activity.rows.map(item => {
        const time = formatActivityTime(item, now);
        return <div className="row" role="row" key={item.key ?? `${item.kind}:${item.id}`}>
          <span role="cell">
            {item.kind} · {item.label}{item.kind === 'request' && <button onClick={event => inspect(item.id, event.currentTarget)}>Inspect history</button>}{item.parentId && <button onClick={event => inspect(item.parentId!, event.currentTarget)}>Inspect parent request</button>}
          </span>
          <span role="cell">
            {item.state}{item.outcome && ` · outcome ${item.outcome}`}{item.delivered !== undefined && ` · delivered ${item.delivered}`}
          </span>
          <span role="cell" className="meta">
            {link({ kind: 'conversation', conversationId: item.nodeId }, item.nodeId)} · id {item.id} · {time.start} · {time.duration}{item.commandId && ` · command ${item.commandId}`}{item.requestId && ` · request ${item.requestId}`}{item.callId && ` · call ${item.callId}`}{item.toolKind && ` · tool kind ${item.toolKind}`}{item.toolName && ` · ${item.toolName}`}{item.detail && ` · ${item.detail}`}{item.output !== undefined && <pre>
              {typeof item.output === 'string' ? item.output : JSON.stringify(item.output, null, 2)}
            </pre>}
          </span>
        </div>;
      })}
    </div>
    {!activity.total && <Empty>No activity is available for this context.</Empty>}
    <Pager page={activity.page} count={activity.pageCount} total={activity.total} onPage={setPage} />
  </>;
}
