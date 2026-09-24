/**
 * Stable browser-facing JSON contract. These shapes intentionally describe the
 * server API, not Rust structs; wire adapters may evolve independently.
 */
export type EntityId = string;

export interface Snapshot {
  readonly seq: number;
  readonly conversations: readonly Conversation[];
  readonly requests: readonly RequestRecord[];
  readonly jobs: readonly Job[];
  readonly envelopes: readonly Envelope[];
}

export interface Conversation {
  readonly id: EntityId;
  readonly path: string;
  readonly state: "idle" | "requesting" | "paused" | "cancelled";
  readonly version?: number;
}

export interface RequestRecord {
  readonly id: EntityId;
  readonly conversationId: EntityId;
  readonly state: "running" | "completed" | "failed";
  readonly version?: number;
}

export interface Job {
  readonly id: EntityId;
  readonly conversationId: EntityId;
  readonly state: "running" | "settled" | "cancelled";
  readonly version?: number;
}

export interface Envelope {
  readonly id: EntityId;
  readonly recipient: string;
  readonly sender: string;
  readonly type: "NEW_TASK" | "MESSAGE" | "FINAL_ANSWER";
  readonly payload: string;
  readonly version?: number;
}

export type StateEvent =
  | { readonly kind: "conversation.upsert"; readonly value: Conversation }
  | { readonly kind: "request.upsert"; readonly value: RequestRecord }
  | { readonly kind: "job.upsert"; readonly value: Job }
  | { readonly kind: "envelope.upsert"; readonly value: Envelope }
  | { readonly kind: "entity.remove"; readonly entity: "conversation" | "request" | "job" | "envelope"; readonly id: EntityId };

export interface SequencedEvent {
  readonly seq: number;
  readonly event: StateEvent;
}

export interface DeltaEvent {
  readonly kind: "delta";
  readonly seq: number;
  readonly itemId: EntityId;
  readonly text: string;
}

export interface NormalizedState {
  readonly seq: number;
  readonly conversations: ReadonlyMap<EntityId, Conversation>;
  readonly requests: ReadonlyMap<EntityId, RequestRecord>;
  readonly jobs: ReadonlyMap<EntityId, Job>;
  readonly envelopes: ReadonlyMap<EntityId, Envelope>;
}

export type ApplyResult =
  | { readonly kind: "applied"; readonly state: NormalizedState }
  | { readonly kind: "resync"; readonly expected: number; readonly received: number };

export function normalizeSnapshot(snapshot: Snapshot): NormalizedState {
  return {
    seq: snapshot.seq,
    conversations: index(snapshot.conversations),
    requests: index(snapshot.requests),
    jobs: index(snapshot.jobs),
    envelopes: index(snapshot.envelopes),
  };
}

/** State events are durable; transient deltas must use a separate buffer. */
export function applyStateEvent(state: NormalizedState, message: SequencedEvent): ApplyResult {
  const expected = state.seq + 1;
  if (!Number.isSafeInteger(message.seq) || message.seq !== expected) {
    return { kind: "resync", expected, received: message.seq };
  }
  const next = { ...state, seq: message.seq };
  switch (message.event.kind) {
    case "conversation.upsert":
      return { kind: "applied", state: { ...next, conversations: set(next.conversations, message.event.value) } };
    case "request.upsert":
      return { kind: "applied", state: { ...next, requests: set(next.requests, message.event.value) } };
    case "job.upsert":
      return { kind: "applied", state: { ...next, jobs: set(next.jobs, message.event.value) } };
    case "envelope.upsert":
      return { kind: "applied", state: { ...next, envelopes: set(next.envelopes, message.event.value) } };
    case "entity.remove": {
      const { entity, id } = message.event;
      switch (entity) {
        case "conversation": {
          const table = new Map(next.conversations); table.delete(id);
          return { kind: "applied", state: { ...next, conversations: table } };
        }
        case "request": {
          const table = new Map(next.requests); table.delete(id);
          return { kind: "applied", state: { ...next, requests: table } };
        }
        case "job": {
          const table = new Map(next.jobs); table.delete(id);
          return { kind: "applied", state: { ...next, jobs: table } };
        }
        case "envelope": {
          const table = new Map(next.envelopes); table.delete(id);
          return { kind: "applied", state: { ...next, envelopes: table } };
        }
      }
    }
  }
}

function index<T extends { readonly id: EntityId }>(values: readonly T[]): ReadonlyMap<EntityId, T> {
  return new Map(values.map((value) => [value.id, value]));
}

function set<T extends { readonly id: EntityId }>(table: ReadonlyMap<EntityId, T>, value: T): ReadonlyMap<EntityId, T> {
  return new Map(table).set(value.id, value);
}
