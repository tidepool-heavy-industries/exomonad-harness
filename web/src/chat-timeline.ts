import { actorOutputKey, type ActorOutputOrigin } from './actor-output'
import { type LiveOutput, type OutputOrigin, originKey, outputKey } from './live-output'
import type { ChatOperation } from './generated/chat'
import { compareDecimal } from './decimal'
import type { UnifiedEntry } from './UnifiedChat'
export type { ChatTool } from './generated/chat'

export type ToolOperation = ChatOperation
export type MessageEntry = Extract<UnifiedEntry, { kind: 'message' }>
export type TimelineRow =
  | { kind: 'entry'; key: string; entry: UnifiedEntry }
  | { kind: 'live'; key: string; live: readonly LiveOutput[] }
  | { kind: 'activity'; key: string; entry: MessageEntry; results: readonly MessageEntry[] }
  | { kind: 'result'; key: string; entry: MessageEntry; invocationKey?: string }

const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)
export function operationKey(operation: ToolOperation): string { return JSON.stringify([originKey(operation.origin), operation.request, operation.call]) }
export function toolKind(entry: MessageEntry): 'invocation' | 'result' | undefined {
  if (!object(entry.item)) return undefined
  return ['function_call', 'custom_tool_call'].includes(String(entry.item.type)) ? 'invocation'
    : ['function_call_output', 'custom_tool_call_output'].includes(String(entry.item.type)) ? 'result' : undefined
}
function providerItemId(entry: MessageEntry): string | undefined { return entry.providerItemId ?? undefined }
function liveMessageKey(output: LiveOutput): string { return JSON.stringify(['model-item', originKey(output.origin), output.requestId, output.itemId]) }
function storedMessageKey(origin: ActorOutputOrigin, entry: MessageEntry): string | undefined {
  const id = providerItemId(entry)
  // Numeric actor IDs belong to displays; model live origins carry the actor path.
  // The caller supplies live origins, so the exact model key is selected below.
  return id ? JSON.stringify([actorOutputKey(origin), entry.requestId, id]) : undefined
}
export function mergeEntries(current: readonly UnifiedEntry[], incoming: readonly UnifiedEntry[]): UnifiedEntry[] {
  const bySequence = new Map(current.map(entry => [entry.sequence, entry]))
  for (const entry of incoming) {
    const retained = bySequence.get(entry.sequence)
    // A form's HTTP response may be newer than an in-flight history snapshot.
    if (retained?.kind === 'form' && entry.kind === 'form') {
      const order = compareDecimal(retained.form.revisionSequence, entry.form.revisionSequence)
      if (order > 0 || order === 0 && retained.form.state !== 'open' && entry.form.state === 'open') continue
    }
    bySequence.set(entry.sequence, entry)
  }
  return [...bySequence.values()].sort((a, b) => compareDecimal(a.sequence, b.sequence))
}
// Presentation budget: eight 50-entry pages and at most eight 256-KiB responses.
// History remains in Store; eviction never implies deletion or completion.
const retainedSizes = new WeakMap<UnifiedEntry, number>()
export function boundEntries(entries: readonly UnifiedEntry[], keep: 'oldest' | 'latest'): { entries: readonly UnifiedEntry[]; trimmed: boolean } {
  const selected: UnifiedEntry[] = []
  let bytes = 0
  const source = keep === 'latest' ? [...entries].reverse() : entries
  for (const entry of source) {
    let size = retainedSizes.get(entry)
    if (size === undefined) { size = new TextEncoder().encode(JSON.stringify(entry)).length; retainedSizes.set(entry, size) }
    if (selected.length === 400 || bytes + size > 2 * 1024 * 1024) break
    bytes += size; selected.push(entry)
  }
  if (keep === 'latest') selected.reverse()
  return { entries: selected, trimmed: selected.length !== entries.length }
}
/** Presentation joins only issuer evidence and exact provider item identity. */
export function projectTimeline(origin: ActorOutputOrigin, entries: readonly UnifiedEntry[], live: readonly LiveOutput[], modelOrigin?: OutputOrigin): TimelineRow[] {
  const groups = new Map<string, LiveOutput[]>()
  for (const output of live) {
    if (!modelOrigin || originKey(output.origin) !== originKey(modelOrigin)) continue
    const key = liveMessageKey(output)
    groups.set(key, [...(groups.get(key) ?? []), output])
  }
  const providerCounts = new Map<string, number>()
  for (const entry of entries) if (entry.kind === 'message') {
    const key = storedMessageKey(origin, entry)
    if (key) providerCounts.set(key, (providerCounts.get(key) ?? 0) + 1)
  }
  const invocations = new Map<string, MessageEntry>()
  const results = new Map<string, MessageEntry[]>()
  for (const entry of entries) if (entry.kind === 'message' && entry.tool) {
    const key = operationKey(entry.tool.operation)
    if (toolKind(entry) === 'invocation') invocations.set(key, entry)
    if (toolKind(entry) === 'result') results.set(key, [...(results.get(key) ?? []), entry])
  }
  const consumed = new Set<string>()
  const rows: TimelineRow[] = entries.map(entry => {
    const occurrence = JSON.stringify(['publication', actorOutputKey(origin), entry.sequence])
    if (entry.kind !== 'message') return { kind: 'entry', key: occurrence, entry }
    const candidates = [...groups].filter(([, output]) => modelOrigin && originKey(output[0]!.origin) === originKey(modelOrigin) && output[0]!.requestId === entry.requestId && output[0]!.itemId === providerItemId(entry))
    const identity = storedMessageKey(origin, entry)
    const matched = identity && providerCounts.get(identity) === 1 && candidates.length === 1 ? candidates[0] : undefined
    if (matched) consumed.add(matched[0])
    const id = providerItemId(entry)
    const key = modelOrigin && id && identity && providerCounts.get(identity) === 1
      ? JSON.stringify(['model-item', originKey(modelOrigin), entry.requestId, id]) : occurrence
    if (toolKind(entry) === 'invocation') return { kind: 'activity', key, entry, results: entry.tool ? results.get(operationKey(entry.tool.operation)) ?? [] : [] }
    if (toolKind(entry) === 'result') {
      const invocation = entry.tool ? invocations.get(operationKey(entry.tool.operation)) : undefined
      const invocationId = invocation ? providerItemId(invocation) : undefined
      const invocationIdentity = invocation ? storedMessageKey(origin, invocation) : undefined
      const invocationKey = invocation ? modelOrigin && invocationId && invocationIdentity && providerCounts.get(invocationIdentity) === 1
        ? JSON.stringify(['model-item', originKey(modelOrigin), invocation.requestId, invocationId])
        : JSON.stringify(['publication', actorOutputKey(origin), invocation.sequence]) : undefined
      return { kind: 'result', key, entry, invocationKey }
    }
    return { kind: 'entry', key, entry }
  })
  for (const [key, output] of groups) if (!consumed.has(key)) rows.push({ kind: 'live', key, live: output.sort((a, b) => compareDecimal(a.version, b.version) || outputKey(a).localeCompare(outputKey(b))) })
  return rows
}
