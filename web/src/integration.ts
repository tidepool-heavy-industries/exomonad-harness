import type { HarnessViewModel } from './view-model'
import { actorIdentityKey, type Job, type NormalizedState } from './protocol'
import { resolveParentIds, formatActivityTime, sortActivity } from './selectors'

type NodeRow = HarnessViewModel['nodes'][number]
type ActivityRow = HarnessViewModel['timeline'][number]
type ActorRow = NonNullable<HarnessViewModel['actors']>[number]
type InboxRow = HarnessViewModel['inbox'][number]
type CachedRow<T> = { source: unknown; dependency: string; row: T }

function reuse<T>(previous: Map<string, CachedRow<T>>, next: Map<string, CachedRow<T>>,
  id: string, source: unknown, dependency: string, create: () => T): T {
  const old = previous.get(id)
  const entry = old && old.source === source && old.dependency === dependency
    ? old : { source, dependency, row: create() }
  next.set(id, entry)
  return entry.row
}

function timestamp(value: number | null | undefined): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function timing(start: number | null | undefined, end: number | null | undefined) {
  const times = { startedAtMs: timestamp(start), endedAtMs: timestamp(end) }
  const rendered = formatActivityTime(times)
  return { ...times, startedAt: rendered.start, duration: rendered.duration }
}

/** One projector per consumer. Caches retain only the latest table entries. */
export function createViewProjector(): (state: NormalizedState) => HarnessViewModel {
  let previous: NormalizedState | undefined
  let result: HarnessViewModel | undefined
  let actorsCache = new Map<string, CachedRow<ActorRow>>()
  let nodesCache = new Map<string, CachedRow<NodeRow>>()
  let requestsCache = new Map<string, CachedRow<ActivityRow>>()
  let jobsCache = new Map<string, CachedRow<ActivityRow>>()
  let inboxCache = new Map<string, CachedRow<InboxRow>>()
  let requestDetails = new Map<string, string>()
  let jobEvidence = new Map<string, string>()
  let parents = new Map<string, string | null | undefined>()

  return (state) => {
    const actorsChanged = previous?.actors !== state.actors
    const conversationsChanged = previous?.conversations !== state.conversations
    const requestsChanged = previous?.requests !== state.requests
    const jobsChanged = previous?.jobs !== state.jobs
    const inboxChanged = previous?.envelopes !== state.envelopes
    const receiptsChanged = previous?.commandReceipts !== state.commandReceipts
    if (result && !actorsChanged && !conversationsChanged && !requestsChanged
      && !jobsChanged && !inboxChanged && !receiptsChanged && previous?.hostRun === state.hostRun) {
      previous = state
      return result
    }
    if (requestsChanged) {
      const grouped = new Map<string, string[]>()
      for (const request of state.requests.values()) {
        const details = [request.commandId ? `command ${request.commandId}` : undefined,
          request.command ? `"${request.command}"` : undefined,
          request.outcome ? `outcome ${request.outcome}` : undefined, request.detail]
          .filter(Boolean).join(' · ') || `request ${request.state}`
        const group = grouped.get(request.conversationId)
        if (group) group.push(details)
        else grouped.set(request.conversationId, [details])
      }
      requestDetails = new Map([...grouped].map(([id, details]) => [id, details.join(', ')]))
    }
    if (jobsChanged) {
      const grouped = new Map<string, unknown[]>()
      for (const job of state.jobs.values()) {
        // Active progress/version changes do not refresh a retained history page.
        if (!job.requestId || (job.state === 'running' && job.delivered !== true)) continue
        const evidence = [job.id, job.state, job.delivered ?? null, job.version ?? null, job.endedAtMs ?? null]
        const group = grouped.get(job.requestId)
        if (group) group.push(evidence)
        else grouped.set(job.requestId, [evidence])
      }
      jobEvidence = new Map([...grouped].map(([id, evidence]) => [id, JSON.stringify(evidence)]))
    }
    if (conversationsChanged) parents = resolveParentIds([...state.conversations.values()].map((node) => ({
      id: node.id, name: node.path,
      ...(Object.hasOwn(node, 'parentId') ? { parentId: node.parentId } : {}),
    })))

    let actors = result?.actors ?? []
    if (actorsChanged) {
      const next = new Map<string, CachedRow<ActorRow>>()
      actors = [...state.actors.values()].map((actor) => {
        const id = actorIdentityKey(actor.identity)
        return reuse(actorsCache, next, id, actor, '', () => ({
          id, name: actor.identity.actor, run: actor.identity.run, incarnation: actor.identity.incarnation,
          parentIdentity: actor.parent,
          parent: actor.parent ? `${actor.parent.actor} · incarnation ${actor.parent.incarnation} · run ${actor.parent.run}` : undefined,
          kind: actor.kind, lifecycle: actor.lifecycle,
          modelConversation: actor.modelConversation ?? undefined, activeRound: actor.activeRound,
        }))
      })
      actorsCache = next
    }
    let nodes = result?.nodes ?? []
    if (conversationsChanged || requestsChanged) {
      const next = new Map<string, CachedRow<NodeRow>>()
      nodes = [...state.conversations.values()].map((conversation) => {
        const parentId = parents.get(conversation.id)
        const detail = requestDetails.get(conversation.id) ?? ''
        return reuse(nodesCache, next, conversation.id, conversation, JSON.stringify([parentId, detail]), () => ({
          id: conversation.id, parentId, name: conversation.path, state: conversation.state,
          forkSourceRequestId: conversation.forkSourceRequestId, version: conversation.version, detail,
        }))
      })
      nodesCache = next
    }
    let timeline = result?.timeline ?? []
    if (requestsChanged || jobsChanged) {
      const nextRequests = new Map<string, CachedRow<ActivityRow>>()
      const nextJobs = new Map<string, CachedRow<ActivityRow>>()
      const rows: ActivityRow[] = []
      for (const request of state.requests.values()) {
        const historyRefreshKey = JSON.stringify([request.id, request.version ?? null,
          request.state, jobEvidence.get(request.id) ?? ''])
        rows.push(reuse(requestsCache, nextRequests, request.id, request, historyRefreshKey, () => ({
          id: request.id, key: JSON.stringify(['request', request.id]), nodeId: request.conversationId,
          label: request.command ?? 'Response request', kind: 'request', state: request.state,
          ...timing(request.createdAtMs, request.endedAtMs), version: request.version,
          historyRefreshKey, parentId: request.parentId,
          commandId: request.commandId, command: request.command, outcome: request.outcome, detail: request.detail,
        })))
      }
      for (const job of state.jobs.values()) rows.push(reuse(jobsCache, nextJobs, job.id, job, '', () => jobRow(job)))
      requestsCache = nextRequests
      jobsCache = nextJobs
      timeline = sortActivity(rows)
    }
    let inbox = result?.inbox ?? []
    if (inboxChanged) {
      const next = new Map<string, CachedRow<InboxRow>>()
      inbox = [...state.envelopes.values()].map((envelope) => reuse(inboxCache, next,
        envelope.id, envelope, '', () => ({
          id: envelope.id, sender: envelope.sender, recipient: envelope.recipient,
          message: envelope.payload, state: envelope.type, type: envelope.type, ordinal: envelope.ordinal,
        })))
      if (inbox.every((row) => row.ordinal !== undefined)) inbox.sort((a, b) => a.ordinal! - b.ordinal!)
      inboxCache = next
    }
    result = { hostRun: state.hostRun, actors, nodes, timeline, inbox,
      commandReceipts: receiptsChanged ? [...state.commandReceipts.values()] : result?.commandReceipts ?? [] }
    previous = state
    return result
  }
}

function jobRow(job: Job): ActivityRow {
  return { id: job.id, key: JSON.stringify(['job', job.id]), nodeId: job.conversationId,
    label: 'Async job', kind: 'job', state: job.state, ...timing(job.startedAtMs, job.endedAtMs),
    version: job.version, requestId: job.requestId, callId: job.callId, toolKind: job.toolKind,
    toolName: job.toolName, delivered: job.delivered, output: job.output }
}

/** Pure one-shot projection for fixtures and consumers without retained state. */
export function toViewModel(state: NormalizedState): HarnessViewModel {
  return createViewProjector()(state)
}
