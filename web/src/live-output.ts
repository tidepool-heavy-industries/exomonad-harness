import { actorIdentityKey, isObject, type HostActorIdentity } from './protocol'

export type OutputOrigin = { kind: 'embedded'; run: string; actor: string; incarnation: string }
  | { kind: 'standalone'; store: string; actor: string }
export interface OutputScope { origin: OutputOrigin; requestId: string }
export type OutputChannel = 'assistant' | 'reasoning_summary' | 'reasoning' | 'tool_arguments' | 'tool_input' | 'refusal'
export const outputLabels: Record<OutputChannel, string> = {assistant:'Assistant', reasoning_summary:'Reasoning summary', reasoning:'Reasoning', tool_arguments:'Tool arguments', tool_input:'Tool input', refusal:'Assistant refusal'}
export interface OutputItem extends OutputScope { itemId: string; channel: OutputChannel; index: number }
export interface LiveOutput extends OutputItem { text: string; overflow: boolean; streaming: boolean; version: number; committedHash?: string }
export interface HistoryRevision extends OutputScope { version: number }
export type OutputUpdate = OutputItem & { text: string; overflow: boolean; version: number }
export type OutputCommit = OutputScope & { itemId: string | null; hash: string; version: number }
const MAX_ITEMS = 128
const MAX_BYTES = 64 * 1024

export function originKey(origin: OutputOrigin): string {
  return origin.kind === 'embedded' ? actorIdentityKey(origin) : JSON.stringify([origin.store, origin.actor])
}
export function outputKey(output: OutputItem): string {
  return JSON.stringify([originKey(output.origin), output.requestId, output.itemId, output.channel, output.index])
}
export function revisionKey(output: OutputScope): string { return JSON.stringify([originKey(output.origin), output.requestId]) }
export function belongsTo(output: OutputScope, identity: HostActorIdentity): boolean {
  return output.origin.kind === 'embedded' && originKey(output.origin) === actorIdentityKey(identity)
}
export function appendOutput(outputs: ReadonlyMap<string, LiveOutput> | undefined, delta: OutputUpdate): ReadonlyMap<string, LiveOutput> {
  const next = new Map(outputs)
  const key = outputKey(delta)
  const old = next.get(key)
  const encoded = new TextEncoder().encode((old?.text ?? '') + delta.text)
  // Decode only complete UTF-8 characters at the retained preview boundary.
  let end = Math.min(encoded.length, MAX_BYTES)
  while (end < encoded.length && (encoded[end]! & 0xc0) === 0x80) end--
  const text = new TextDecoder().decode(encoded.subarray(0, end))
  next.set(key, { ...delta, streaming:true, text, overflow: delta.overflow || encoded.length > MAX_BYTES,
    committedHash: old?.committedHash })
  while (next.size > MAX_ITEMS) next.delete(next.keys().next().value!)
  return next
}
export function commitOutput(outputs: ReadonlyMap<string, LiveOutput> | undefined, committed: OutputCommit): ReadonlyMap<string, LiveOutput> {
  const next = new Map(outputs)
  for (const [key, item] of next) if (revisionKey(item) === revisionKey(committed) && item.itemId === committed.itemId) {
    next.set(key, { ...item, streaming:false, committedHash: committed.hash })
  }
  return next
}
export function setRevision(revisions: ReadonlyMap<string, HistoryRevision> | undefined, revision: HistoryRevision): ReadonlyMap<string, HistoryRevision> {
  const next = new Map(revisions)
  const key = revisionKey(revision)
  next.delete(key); next.set(key, revision)
  while (next.size > MAX_ITEMS) next.delete(next.keys().next().value!)
  return next
}
/** A committed partial stays visible until the exact durable hash has been fetched. */
export function visibleOutput(outputs: readonly LiveOutput[], loaded: ReadonlyMap<string, ReadonlySet<string>>): LiveOutput[] {
  return outputs.filter(output => !output.committedHash || !loaded.get(output.requestId)?.has(output.committedHash))
}
export function isOutputScope(value: unknown): value is OutputScope {
  if (!isObject(value) || typeof value.requestId !== 'string' || !value.requestId || !isObject(value.origin)
    || typeof value.origin.actor !== 'string' || !value.origin.actor) return false
  const origin = value.origin
  return origin.kind === 'embedded' ? ['run', 'incarnation'].every(key => typeof origin[key] === 'string' && !!origin[key])
    : origin.kind === 'standalone' && typeof origin.store === 'string' && !!origin.store
}
export function isOutputItem(value: unknown): value is OutputItem {
  return isOutputScope(value) && isObject(value) && typeof value.itemId === 'string' && !!value.itemId
    && typeof value.channel === 'string' && Object.hasOwn(outputLabels, value.channel) && Number.isSafeInteger(value.index) && (value.index as number) >= 0
}
export function isOutputUpdate(value: unknown): value is OutputUpdate {
  return isOutputItem(value) && isObject(value) && typeof value.text === 'string' && typeof value.overflow === 'boolean'
    && Number.isSafeInteger(value.version) && (value.version as number) >= 0
}
export function isLiveOutput(value: unknown): value is LiveOutput {
  return isOutputUpdate(value) && isObject(value) && typeof value.streaming === 'boolean'
    && (value.committedHash === undefined || typeof value.committedHash === 'string')
}
export function isHistoryRevision(value: unknown): value is HistoryRevision {
  return isOutputScope(value) && isObject(value) && Number.isSafeInteger(value.version) && (value.version as number) >= 0
}
export function isOutputCommit(value: unknown): value is OutputCommit {
  return isHistoryRevision(value) && isObject(value) && (value.itemId === null || typeof value.itemId === 'string') && typeof value.hash === 'string'
}
