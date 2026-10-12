import { actorIdentityKey, type HostActorIdentity } from './protocol'

import type { Snapshot, StateEvent } from './protocol'
export type OutputScope = Extract<StateEvent, { kind: 'model.output.started' }>['value']
export type OutputOrigin = OutputScope['origin']
export type OutputItem = Extract<StateEvent, { kind: 'model.output.remove' }>['value']
export type OutputUpdate = Extract<StateEvent, { kind: 'model.output.delta' }>['value']
export type OutputCommit = Extract<StateEvent, { kind: 'model.output.committed' }>['value']
export type LiveOutput = NonNullable<Snapshot['liveOutput']>[number]
export type HistoryRevision = NonNullable<Snapshot['historyRevisions']>[number]
export type OutputChannel = OutputItem['channel']
export const outputLabels: Record<OutputChannel, string> = {assistant:'Assistant', reasoning_summary:'Reasoning summary', reasoning:'Reasoning', tool_arguments:'Tool arguments', tool_input:'Tool input', refusal:'Assistant refusal'}
const MAX_ITEMS = 128
const MAX_BYTES = 64 * 1024
// Preview values are immutable. Restored wire values acquire this derived count
// on their first update; subsequent deltas never re-encode the retained prefix.
const retainedBytes = new WeakMap<LiveOutput, number>()
const encoder = new TextEncoder()
const decoder = new TextDecoder('utf-8', {ignoreBOM:true})

function completePrefix(encoded: Uint8Array, budget: number): number {
  let end = Math.min(encoded.length, budget)
  while (end < encoded.length && (encoded[end]! & 0xc0) === 0x80) end--
  return end
}

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
  const bytes = old ? retainedBytes.get(old) : 0
  const restoring = bytes === undefined
  const remaining = MAX_BYTES - (bytes ?? 0)
  // Restored wire values use the whole-prefix rule once. Every UTF-16 code
  // unit needs at least one UTF-8 byte, so later input cannot fit after this
  // prefix; even an oversized incoming delta has bounded encoding work.
  const incoming = restoring ? old!.text + delta.text : delta.text
  const prefix = incoming.slice(0, remaining)
  const encoded = remaining > 0 ? encoder.encode(prefix) : new Uint8Array()
  const end = completePrefix(encoded, remaining)
  // Preserve the whole-prefix decoder's leading-BOM rule, but keep a BOM
  // at an interior delta boundary. Its omitted bytes are not retained bytes.
  const start = (restoring || !old?.text) && end >= 3 && encoded[0] === 0xef && encoded[1] === 0xbb && encoded[2] === 0xbf ? 3 : 0
  const text = (restoring ? '' : old?.text ?? '') + (end > start ? decoder.decode(encoded.subarray(start, end)) : '')
  const item: LiveOutput = { ...delta, streaming:true, text,
    overflow: delta.overflow || prefix.length < incoming.length || end < encoded.length,
    committedHash: old?.committedHash }
  retainedBytes.set(item, (bytes ?? 0) + end - start)
  next.set(key, item)
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
