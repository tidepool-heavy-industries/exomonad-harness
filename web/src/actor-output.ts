import { isActorOutputHistoryPage, isServerFrame } from './generated/validators.mjs'
import type { ActorOutputHistoryPage } from './generated/actor-output-history'
export type ActorOutputOrigin = ActorOutputHistoryPage['origin']
export type StoredActorOutput = ActorOutputHistoryPage['outputs'][number]
export type ActorOutputReference = StoredActorOutput['reference']
const bytes = (value: unknown) => new TextEncoder().encode(typeof value === 'string' ? value : JSON.stringify(value)).length
export const actorOutputKey = (origin: ActorOutputOrigin) => JSON.stringify([origin.run, origin.nativeActor, origin.incarnation])

/** Generated shapes precede cross-field association and retained display bounds. */
export function isStoredActorOutput(value: unknown): value is StoredActorOutput {
  const frame = { type: 'event', event: { seq: '0', event: { kind: 'actor.output.committed', value } } }
  if (!isServerFrame(frame) || frame.type !== 'event' || frame.event.event.kind !== 'actor.output.committed') return false
  const { reference, emission } = frame.event.event.value
  const page = emission.page
  return BigInt(reference.sequence) > 0n && BigInt(emission.id.displaySlot) > 0n
    && actorOutputKey(emission.origin) === actorOutputKey(reference.origin)
    && emission.origin.run.length > 0 && bytes(emission.origin.run) <= 1024
    && bytes(page.text) <= 32768
    && page.expansions.every(([key]) => BigInt(key) > 0n)
    && new Set(page.expansions.map(([key]) => key)).size === page.expansions.length
    && bytes({ identity: [emission.origin.nativeActor, emission.origin.incarnation, emission.id.displaySlot], expansions: page.expansions, unavailable: page.unavailable }) <= 8192
}
export function decodeActorOutputPage(value: unknown, origin: ActorOutputOrigin, after: string): ActorOutputHistoryPage {
  if (!isActorOutputHistoryPage(value) || actorOutputKey(value.origin) !== actorOutputKey(origin)
    || value.outputs.length > 100 || !value.outputs.every(isStoredActorOutput)
    || value.outputs.some(output => actorOutputKey(output.reference.origin) !== actorOutputKey(origin))
    || (value.nextAfter !== null && BigInt(value.nextAfter) <= BigInt(after))) {
    throw new Error('Actor output history has an invalid page.')
  }
  return value
}
export function retainActorOutput(current: ReadonlyMap<string, ActorOutputReference> | undefined, reference: ActorOutputReference) {
  const next = new Map(current)
  next.delete(actorOutputKey(reference.origin))
  next.set(actorOutputKey(reference.origin), reference)
  while (next.size > 128 || bytes([...next.values()]) > 65536) next.delete(next.keys().next().value!)
  return next
}
