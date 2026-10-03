export interface ActorOutputOrigin { readonly run: string; readonly nativeActor: number; readonly incarnation: number }
export interface ActorOutputReference { readonly origin: ActorOutputOrigin; readonly sequence: number }
export interface StoredActorOutput {
  readonly reference: ActorOutputReference;
  readonly emission: {
    readonly origin: ActorOutputOrigin;
    readonly id: { readonly displaySlot: number; readonly pageOrdinal: number };
    readonly page: { readonly text: string; readonly expansions: readonly (readonly [number, string])[]; readonly unavailable: boolean };
  };
}
const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const counter = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
const bytes = (value: unknown) => new TextEncoder().encode(typeof value === 'string' ? value : JSON.stringify(value)).length;
export const actorOutputKey = (origin: ActorOutputOrigin) => JSON.stringify([origin.run, origin.nativeActor, origin.incarnation]);
export function isActorOutputOrigin(value: unknown): value is ActorOutputOrigin {
  return object(value) && typeof value.run === 'string' && value.run.length > 0 && bytes(value.run) <= 1024
    && counter(value.nativeActor) && counter(value.incarnation);
}
export function isActorOutputReference(value: unknown): value is ActorOutputReference {
  return object(value) && isActorOutputOrigin(value.origin) && counter(value.sequence) && value.sequence > 0;
}
export function isStoredActorOutput(value: unknown): value is StoredActorOutput {
  if (!object(value) || !isActorOutputReference(value.reference) || !object(value.emission)) return false;
  const emission = value.emission;
  if (!isActorOutputOrigin(emission.origin) || actorOutputKey(emission.origin) !== actorOutputKey(value.reference.origin)
    || !object(emission.id) || !counter(emission.id.displaySlot) || emission.id.displaySlot === 0
    || !counter(emission.id.pageOrdinal) || !object(emission.page)) return false;
  const page = emission.page;
  return typeof page.text === 'string' && bytes(page.text) <= 32768 && typeof page.unavailable === 'boolean'
    && Array.isArray(page.expansions) && page.expansions.every(key => Array.isArray(key) && key.length === 2
      && counter(key[0]) && key[0] > 0 && typeof key[1] === 'string')
    && new Set(page.expansions.map(key => key[0])).size === page.expansions.length
    && bytes({ identity: [emission.origin.nativeActor, emission.origin.incarnation, emission.id.displaySlot], expansions: page.expansions, unavailable: page.unavailable }) <= 8192;
}
export function retainActorOutput(current: ReadonlyMap<string, ActorOutputReference> | undefined, reference: ActorOutputReference) {
  const next = new Map(current);
  next.delete(actorOutputKey(reference.origin));
  next.set(actorOutputKey(reference.origin), reference);
  while (next.size > 128 || bytes([...next.values()]) > 65536) next.delete(next.keys().next().value!);
  return next;
}
