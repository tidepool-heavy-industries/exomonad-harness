import type { MessageFilters, Selection } from './client-contract'
import type { Envelope } from './protocol'
import { actorIdentityKey } from './protocol'
import type { HarnessViewModel } from './view-model'

type ActorRow = NonNullable<HarnessViewModel['actors']>[number]
type NodeRow = HarnessViewModel['nodes'][number]
type ActivityRow = HarnessViewModel['timeline'][number]

export interface ResolvedSelection {
  actor?: ActorRow
  conversationId?: string
  endpoint?: string
  missing: boolean
}

export function resolveSelection(data: HarnessViewModel, selection: Selection): ResolvedSelection {
  if (selection.kind === 'none') return { missing: false }
  if (selection.kind === 'actor') {
    const id = actorIdentityKey(selection.identity)
    const actor = data.actors?.find((row) => row.id === id && row.run === selection.identity.run
      && row.name === selection.identity.actor && row.incarnation === selection.identity.incarnation)
    if (!actor) return { missing: true }
    const conversationId = actor.modelConversation
    return { actor, conversationId,
      endpoint: conversationId ? data.nodes.find((row) => row.id === conversationId)?.name : undefined,
      missing: false }
  }
  const node = data.nodes.find((row) => row.id === selection.conversationId)
  return node ? { conversationId: node.id, endpoint: node.name, missing: false } : { missing: true }
}

const messageTypes: readonly Envelope['type'][] = ['NEW_TASK', 'MESSAGE', 'FINAL_ANSWER', 'PROGRESS']
function messageType(row: HarnessViewModel['inbox'][number]): Envelope['type'] | undefined {
  return row.type ?? messageTypes.find((kind) => kind === row.state)
}

function sameRows<T>(next: T[], previous: T[] | undefined): T[] {
  return previous && next.length === previous.length && next.every((row, index) => row === previous[index])
    ? previous : next
}

/** Context and explicit message filters are conjoined; labels confer no authority. */
export function selectViewModel(data: HarnessViewModel, selection: Selection,
  global: boolean, filters: MessageFilters): HarnessViewModel {
  return createViewSelector()(data, selection, global, filters)
}

/** One bounded latest projection per mounted consumer, never a global entity store. */
export function createViewSelector(): typeof selectViewModel {
  let previous: HarnessViewModel | undefined
  let previousContext: string | undefined
  let result: HarnessViewModel | undefined
  return (data, selection, global, filters) => {
    const resolved = resolveSelection(data, selection)
    const contextual = !global && selection.kind !== 'none'
    const context = JSON.stringify([contextual, resolved.conversationId, resolved.endpoint,
      resolved.missing, filters.sender, filters.recipient, filters.type])
    const sameContext = previousContext === context
    let nodes = sameContext && previous?.nodes === data.nodes && result ? result.nodes
      : contextual ? data.nodes.filter((row) => row.id === resolved.conversationId) : data.nodes
    let timeline = sameContext && previous?.timeline === data.timeline && result ? result.timeline
      : contextual ? data.timeline.filter((row) => row.nodeId === resolved.conversationId) : data.timeline
    const hasFilters = filters.sender !== undefined || filters.recipient !== undefined || filters.type !== undefined
    let inbox = sameContext && previous?.inbox === data.inbox && result ? result.inbox
      : contextual || hasFilters ? data.inbox.filter((row) =>
        (!contextual || (resolved.endpoint !== undefined
          && (row.sender === resolved.endpoint || row.recipient === resolved.endpoint)))
        && (filters.sender === undefined || row.sender === filters.sender)
        && (filters.recipient === undefined || row.recipient === filters.recipient)
        && (filters.type === undefined || messageType(row) === filters.type)) : data.inbox
    if (sameContext && result) {
      nodes = sameRows(nodes, result.nodes)
      timeline = sameRows(timeline, result.timeline)
      inbox = sameRows(inbox, result.inbox)
    }
    result = nodes === data.nodes && timeline === data.timeline && inbox === data.inbox
      ? data : { ...data, nodes, timeline, inbox }
    previous = data
    previousContext = context
    return result
  }
}

type ParentNode = Pick<NodeRow, 'id' | 'name' | 'parentId'>
type PathIndex = { children: Map<string, PathIndex>; id?: string | null }

/** Index paths once. Ambiguous reused paths never select an arbitrary identity. */
export function resolveParentIds(nodes: readonly ParentNode[]): Map<string, string | null | undefined> {
  const root: PathIndex = { children: new Map() }
  for (const node of nodes) {
    let cursor = root
    for (const part of node.name.split('/').filter(Boolean)) {
      let child = cursor.children.get(part)
      if (!child) { child = { children: new Map() }; cursor.children.set(part, child) }
      cursor = child
    }
    cursor.id = cursor.id === undefined ? node.id : null
  }
  const parents = new Map<string, string | null | undefined>()
  for (const node of nodes) {
    if (Object.hasOwn(node, 'parentId')) { parents.set(node.id, node.parentId); continue }
    let cursor = root
    let parent: string | undefined
    const parts = node.name.split('/').filter(Boolean)
    for (let i = 0; i < parts.length - 1; i++) {
      cursor = cursor.children.get(parts[i]!)!
      if (cursor.id != null) parent = cursor.id
    }
    parents.set(node.id, parent)
  }
  return parents
}

export interface TreeRow { node: NodeRow; depth: number; lineage: 'root' | 'child' | 'orphan' | 'cycle' }

/** Stable pre-order with iterative cycle detection and traversal, including orphans. */
export function orderConversationTree(nodes: readonly NodeRow[]): TreeRow[] {
  const byId = new Map(nodes.map((node) => [node.id, node]))
  const parents = resolveParentIds(nodes)
  const children = new Map<string, NodeRow[]>()
  const roots: NodeRow[] = []
  for (const node of nodes) {
    const parent = parents.get(node.id)
    if (parent == null || !byId.has(parent)) roots.push(node)
    else { const group = children.get(parent); if (group) group.push(node); else children.set(parent, [node]) }
  }
  const complete = new Set<string>()
  const cyclic = new Set<string>()
  for (const node of nodes) {
    const path: string[] = []
    const position = new Map<string, number>()
    let id: string | null | undefined = node.id
    while (id != null && byId.has(id) && !complete.has(id)) {
      const seen = position.get(id)
      if (seen !== undefined) { for (let i = seen; i < path.length; i++) cyclic.add(path[i]!); break }
      position.set(id, path.length); path.push(id); id = parents.get(id)
    }
    for (const visited of path) complete.add(visited)
  }
  const output: TreeRow[] = []
  const visited = new Set<string>()
  const append = (root: NodeRow) => {
    const stack = [{ node: root, depth: 0 }]
    while (stack.length) {
      const entry = stack.pop()!
      if (visited.has(entry.node.id)) continue
      visited.add(entry.node.id)
      const parent = parents.get(entry.node.id)
      output.push({ ...entry, lineage: cyclic.has(entry.node.id) ? 'cycle'
        : parent != null && !byId.has(parent) ? 'orphan' : entry.depth === 0 ? 'root' : 'child' })
      const group = children.get(entry.node.id) ?? []
      for (let i = group.length - 1; i >= 0; i--) stack.push({ node: group[i]!, depth: entry.depth + 1 })
    }
  }
  for (const root of roots) append(root)
  for (const node of nodes) if (!visited.has(node.id)) append(node)
  return output
}

export function pageRows<T>(rows: readonly T[], page: number, pageSize: number): {
  rows: readonly T[]; page: number; pageCount: number; total: number
} {
  const size = Number.isSafeInteger(pageSize) && pageSize > 0 ? pageSize : 1
  const pageCount = Math.max(1, Math.ceil(rows.length / size))
  const current = Math.min(pageCount - 1, Math.max(0, Number.isFinite(page) ? Math.floor(page) : 0))
  return { rows: rows.slice(current * size, (current + 1) * size), page: current, pageCount, total: rows.length }
}

export function sortActivity(rows: readonly ActivityRow[]): ActivityRow[] {
  return [...rows].sort((a, b) => {
    const left = a.startedAtMs, right = b.startedAtMs
    if (left === undefined) return right === undefined ? 0 : 1
    if (right === undefined) return -1
    return left - right
  })
}

export function formatActivityTime(row: Pick<ActivityRow, 'startedAtMs' | 'endedAtMs'>,
  nowMs?: number): { start: string; duration: string } {
  const start = row.startedAtMs
  if (start === undefined || !Number.isFinite(start)) return { start: 'Unavailable', duration: 'Unavailable' }
  const date = new Date(start)
  const rendered = Number.isFinite(date.getTime()) ? date.toISOString() : 'Unavailable'
  const end = row.endedAtMs ?? nowMs
  if (end === undefined || !Number.isFinite(end) || end < start) return { start: rendered, duration: 'Unavailable' }
  const seconds = (end - start) / 1000
  return { start: rendered, duration: `${Number.isInteger(seconds) ? seconds : seconds.toFixed(1)}s` }
}
