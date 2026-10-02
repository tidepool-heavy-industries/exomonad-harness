import { useMemo, useState, type MouseEvent } from 'react'
import { routeUrl } from './navigation'
import type { RouteState } from './client-contract'
import { layoutActiveWorkers } from './active-worker-tree'
import type { HarnessViewModel } from './view-model'
import './ActiveWorkerTree.css'

export interface ActiveWorkerTreeProps {
  readonly data: HarnessViewModel
  readonly route: RouteState
  readonly navigate: (route: RouteState) => void
}

function followLink(event: MouseEvent<HTMLAnchorElement>, route: RouteState, navigate: (route: RouteState) => void) {
  if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
  event.preventDefault()
  navigate(route)
}

export default function ActiveWorkerTree({ data, route, navigate }: ActiveWorkerTreeProps) {
  const [includeHistory, setIncludeHistory] = useState(false)
  const layout = useMemo(() => layoutActiveWorkers(data.actors, data.hostRun, includeHistory), [data.actors, data.hostRun, includeHistory])
  const urlFor = (destination: RouteState) => {
    const url = routeUrl(destination, new URL(window.location.href))
    return url.pathname + url.search
  }

  return <section className="active-worker-tree" aria-labelledby="active-worker-tree-title">
    <div className="active-worker-tree-heading">
      <div>
        <h2 id="active-worker-tree-title">Active workers</h2>
        <p className="hint">Parent links use exact actor identities from run {data.hostRun ?? 'unavailable'}.</p>
      </div>
      <label className="worker-history-toggle">
        <input type="checkbox" checked={includeHistory} onChange={event => setIncludeHistory(event.currentTarget.checked)} />
        Include retired and lost workers
      </label>
    </div>
    {!data.hostRun && <p className="empty" role="status">The host run is unavailable, so no actor hierarchy can be shown.</p>}
    {data.hostRun && !layout.nodes.length && <p className="empty" role="status">No workers are available for this run.</p>}
    {layout.omitted > 0 && <p className="hint" role="status">Showing {layout.nodes.length} workers; {layout.omitted} more are omitted to keep the tree responsive.</p>}
    {layout.nodes.length > 0 && <div className="worker-tree-scroll" role="region" aria-label="Worker hierarchy" tabIndex={0}>
      <div className="worker-tree-canvas" style={{ width: layout.width, height: layout.height }}>
        <svg className="worker-tree-edges" width={layout.width} height={layout.height} aria-hidden="true">
          {layout.edges.map(({ from, to }) => {
            const startX = from.x + 256
            const startY = from.y + 40
            const endX = to.x
            const endY = to.y + 40
            const middleX = Math.round((startX + endX) / 2)
            return <path key={`${from.key}->${to.key}`} d={`M ${startX} ${startY} C ${middleX} ${startY}, ${middleX} ${endY}, ${endX} ${endY}`} />
          })}
        </svg>
        {layout.nodes.map(node => {
          const actor = node.actor
          const destination: RouteState = { ...route, screen: 'chat', requestId: undefined, selection: { kind: 'actor', identity: { run: actor.run, actor: actor.name, incarnation: actor.incarnation } } }
          const href = urlFor(destination)
          const content = <>
            <strong className="worker-tree-name">{actor.name}</strong>
            <span className="worker-tree-meta">{actor.kind} · {actor.lifecycle}{node.context ? ' · unavailable ancestor' : ''}</span>
            <span className="worker-tree-meta">run {actor.run} · incarnation {actor.incarnation}</span>
            {node.lineage && <span className="worker-tree-lineage">{node.lineage === 'orphan' ? 'Parent actor is not available in this run' : node.lineage === 'cycle' ? 'Cycle in exact parent references' : 'Parent omitted by tree size limit'}</span>}
            {actor.kind === 'workflow' && <span className="worker-tree-lineage">Workflow actors do not have model chat.</span>}
          </>
          return <article className={`worker-tree-node${node.context ? ' is-context' : ''}${actor.kind === 'workflow' ? ' is-workflow' : ''}`} key={node.key} style={{ left: node.x, top: node.y }}>
            <a href={href} aria-label={actor.kind === 'model' ? `Open chat with ${actor.name}, ${actor.lifecycle}` : `Open actor page for workflow actor ${actor.name}, ${actor.lifecycle}`} onClick={event => followLink(event, destination, navigate)}>
              {content}
            </a>
          </article>
        })}
      </div>
    </div>}
  </section>
}
