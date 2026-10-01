import { actorIdentityKey, type HostActorIdentity } from './protocol'
import type { HarnessViewModel } from './view-model'

export type WorkerActor = NonNullable<HarnessViewModel['actors']>[number]

export interface WorkerTreeNode {
  readonly key: string
  readonly actor: WorkerActor
  readonly parentKey?: string
  readonly depth: number
  readonly x: number
  readonly y: number
  readonly context: boolean
  readonly lineage?: 'orphan' | 'cycle' | 'truncated'
}

export interface WorkerTreeLayout {
  readonly nodes: readonly WorkerTreeNode[]
  readonly edges: readonly { readonly from: WorkerTreeNode; readonly to: WorkerTreeNode }[]
  readonly width: number
  readonly height: number
  readonly omitted: number
}

const terminal = (actor: WorkerActor) => actor.lifecycle === 'retired' || actor.lifecycle === 'lost'
const nodeWidth = 256
const nodeHeight = 80
const columnGap = 64
const rowGap = 24
export const workerTreeNodeLimit = 500

const keyOf = (identity: HostActorIdentity) => actorIdentityKey(identity)
const identityOf = (actor: WorkerActor): HostActorIdentity => ({ run: actor.run, actor: actor.name, incarnation: actor.incarnation })

/** Build an exact-identity actor hierarchy with a hard DOM/layout bound. */
export function layoutActiveWorkers(
  actors: readonly WorkerActor[] | undefined,
  hostRun: string | undefined,
  includeHistory = false,
  limit = workerTreeNodeLimit,
): WorkerTreeLayout {
  if (!hostRun || !actors?.length || limit <= 0)
    return { nodes: [], edges: [], width: nodeWidth, height: nodeHeight, omitted: 0 }

  const current = actors.filter(actor => actor.run === hostRun)
  const byIdentity = new Map<string, WorkerActor>()
  for (const actor of current)
    byIdentity.set(keyOf(identityOf(actor)), actor)

  const active = current.filter(actor => !terminal(actor))
  const selected = new Map<string, { actor: WorkerActor; context: boolean }>()
  const activePriorityLimit = Math.max(1, limit - Math.min(100, Math.floor(limit / 5)))
  for (const actor of active.slice(0, activePriorityLimit))
    selected.set(keyOf(identityOf(actor)), { actor, context: false })

  // Keep terminal ancestors only when needed to connect an active actor to its
  // real parent. Shared paths are visited once, even in very large trees.
  const visitedAncestors = new Set<string>()
  for (const actor of active.slice(0, activePriorityLimit)) {
    let parentIdentity = actor.parentIdentity
    const seen = new Set<string>([keyOf(identityOf(actor))])
    while (parentIdentity) {
      const parentKey = keyOf(parentIdentity)
      if (seen.has(parentKey)) break
      seen.add(parentKey)
      if (visitedAncestors.has(parentKey)) break
      visitedAncestors.add(parentKey)
      const parent = byIdentity.get(parentKey)
      if (!parent) break
      if (terminal(parent) && !selected.has(parentKey)) {
        if (selected.size >= limit) break
        selected.set(parentKey, { actor: parent, context: true })
      }
      if (!selected.has(parentKey) && selected.size >= limit) break
      parentIdentity = parent.parentIdentity
    }
  }

  // Use any room left after ancestor context for additional active workers.
  for (const actor of active.slice(activePriorityLimit)) {
    if (selected.size >= limit) break
    const key = keyOf(identityOf(actor))
    if (!selected.has(key)) selected.set(key, { actor, context: false })
  }

  if (includeHistory) {
    for (const actor of current) {
      if (selected.size >= limit) break
      const key = keyOf(identityOf(actor))
      if (!selected.has(key)) selected.set(key, { actor, context: false })
    }
  }

  const visiblePopulation = includeHistory
    ? current.length
    : active.length
  const visibleSelected = includeHistory
    ? selected.size
    : [...selected.values()].filter(value => !value.context).length
  const omitted = Math.max(0, visiblePopulation - visibleSelected)
  const parentKeys = new Map<string, string | undefined>()
  const lineage = new Map<string, WorkerTreeNode['lineage']>()
  for (const [key, value] of selected) {
    const parent = value.actor.parentIdentity
    if (!parent) continue
    const parentKey = keyOf(parent)
    if (!selected.has(parentKey)) {
      lineage.set(key, byIdentity.has(parentKey) ? 'truncated' : 'orphan')
    } else {
      parentKeys.set(key, parentKey)
    }
  }

  // Parent relationships are a functional graph. Break one exact edge in each
  // detected cycle so layout and rendering always terminate.
  const settled = new Set<string>()
  for (const start of selected.keys()) {
    if (settled.has(start)) continue
    const path: string[] = []
    const positions = new Map<string, number>()
    let key: string | undefined = start
    while (key && selected.has(key) && !settled.has(key)) {
      const earlier = positions.get(key)
      if (earlier !== undefined) {
        const cycleRoot = path[earlier]!
        parentKeys.delete(cycleRoot)
        lineage.set(cycleRoot, 'cycle')
        break
      }
      positions.set(key, path.length)
      path.push(key)
      key = parentKeys.get(key)
    }
    for (const item of path) settled.add(item)
  }

  const children = new Map<string, string[]>()
  const roots: string[] = []
  for (const key of selected.keys()) {
    const parentKey = parentKeys.get(key)
    if (parentKey) {
      const list = children.get(parentKey) ?? []
      list.push(key)
      children.set(parentKey, list)
    } else roots.push(key)
  }

  const depths = new Map<string, number>()
  const orderByDepth = new Map<number, string[]>()
  const queue: Array<readonly [string, number]> = roots.map(key => [key, 0])
  for (let cursor = 0; cursor < queue.length; cursor++) {
    const [key, depth] = queue[cursor]!
    if (depths.has(key)) continue
    depths.set(key, depth)
    const level = orderByDepth.get(depth) ?? []
    level.push(key)
    orderByDepth.set(depth, level)
    for (const child of children.get(key) ?? []) queue.push([child, depth + 1])
  }

  const rows = new Map<string, number>()
  let maxDepth = 0
  for (const [depth, keys] of orderByDepth) {
    maxDepth = Math.max(maxDepth, depth)
    keys.forEach((key, index) => rows.set(key, index))
  }
  // Any unexpected disconnected residue is still shown and labeled rather
  // than silently disappearing.
  for (const key of selected.keys()) {
    if (rows.has(key)) continue
    lineage.set(key, 'cycle')
    rows.set(key, (orderByDepth.get(0)?.length ?? 0))
    const level = orderByDepth.get(0) ?? []
    level.push(key)
    orderByDepth.set(0, level)
  }

  const nodes = [...selected.entries()].map(([key, value]): WorkerTreeNode => {
    const depth = depths.get(key) ?? 0
    return {
      key,
      actor: value.actor,
      parentKey: parentKeys.get(key),
      depth,
      x: 16 + depth * (nodeWidth + columnGap),
      y: 16 + (rows.get(key) ?? 0) * (nodeHeight + rowGap),
      context: value.context,
      lineage: lineage.get(key),
    }
  })
  const byKey = new Map(nodes.map(node => [node.key, node]))
  const edges = nodes.flatMap(node => {
    const parent = node.parentKey ? byKey.get(node.parentKey) : undefined
    return parent ? [{ from: parent, to: node }] : []
  })
  const maxRows = Math.max(1, ...[...orderByDepth.values()].map(level => level.length))
  return {
    nodes,
    edges,
    width: 32 + (maxDepth + 1) * (nodeWidth + columnGap) - columnGap,
    height: 32 + maxRows * (nodeHeight + rowGap) - rowGap,
    omitted,
  }
}
