/**
 * Stable browser-facing JSON contract. These shapes intentionally describe the
 * server API, not Rust structs; wire adapters may evolve independently.
 */
export type EntityId = string;
const commandReceiptLimit = 128;

export interface Snapshot {
  readonly seq: number;
  /** Exact embedded host run; absent means this is the standalone demo UI. */
  readonly hostRun?: string;
  /** Absent in standalone snapshots written before host actor projection. */
  readonly actors?: readonly HostActorProjection[];
  /** Bounded handoff receipts; durable input history remains in the store. */
  readonly commandReceipts?: readonly CommandReceipt[];
  readonly conversations: readonly Conversation[];
  readonly requests: readonly RequestRecord[];
  readonly jobs: readonly Job[];
  readonly envelopes: readonly Envelope[];
}

export interface HostActorIdentity {
  readonly run: string;
  readonly actor: string;
  readonly incarnation: string;
}

export type ClientOperationId = string;
export type EmbeddedRoundId = string;

/** Allocate once for an explicit operation; retain across uncertain transport. */
export interface HostCommandSubmission {
  readonly operation_id: ClientOperationId;
  readonly command: HostCommand;
}

export type HostCommand =
  | { readonly action: "input"; readonly target: HostActorIdentity; readonly text: string }
  | { readonly action: "interrupt"; readonly target: HostActorIdentity; readonly expected_round: EmbeddedRoundId }
  | { readonly action: "retire"; readonly target: HostActorIdentity };

export interface HostActorProjection {
  readonly identity: HostActorIdentity;
  readonly parent: HostActorIdentity | null;
  readonly kind: "model" | "workflow";
  readonly lifecycle: "running" | "waiting" | "retiring" | "retired" | "lost";
  readonly modelConversation: EntityId | null;
  readonly activeRound?: EmbeddedRoundId;
}

export interface EmbeddedCommandRecord {
  readonly operationId: ClientOperationId;
  readonly command: HostCommand;
  readonly state: "queued" | "dispatching" | "input_admitted" | "control_requested" | "refused" | "unconfirmed";
  readonly envelopeId: number | null;
  readonly receipt: CommandReceipt | null;
}

export type CommandReceipt =
  | { readonly commandId: string; readonly outcome: "unconfirmed"; readonly target: HostActorIdentity; readonly reason: string }
  | {
      readonly commandId: string;
      readonly target?: HostActorIdentity;
      readonly outcome: "admitted";
      readonly envelopeId: string;
      readonly wakeError?: string;
    }
  | {
      readonly commandId: string;
      readonly target: HostActorIdentity;
      readonly outcome: "control_requested";
      readonly control: "interrupt" | "retire";
    }
  | {
      readonly commandId: string;
      readonly target?: HostActorIdentity;
      readonly outcome: "refused";
      readonly reason: string;
    };

export function actorIdentityKey(identity: HostActorIdentity): string {
  return JSON.stringify([identity.run, identity.actor, identity.incarnation]);
}

export interface Conversation {
  readonly id: EntityId;
  readonly path: string;
  /** Exact parent node identity, not a derived tree label. */
  readonly parentId?: EntityId | null;
  readonly forkSourceRequestId?: EntityId | null;
  readonly state: "idle" | "requesting" | "paused" | "cancelled";
  readonly version?: number;
}

export interface RequestRecord {
  readonly id: EntityId;
  readonly conversationId: EntityId;
  readonly parentId?: EntityId | null;
  readonly createdAtMs?: number | null;
  readonly endedAtMs?: number | null;
  readonly state: "running" | "completed" | "failed";
  readonly commandId?: EntityId;
  readonly command?: string;
  readonly outcome?: "accepted" | "pending" | "queued" | "presented" | "acted" | "completed" | "cancelled" | "failed";
  readonly detail?: string;
  readonly version?: number;
}

export interface Job {
  readonly id: EntityId;
  readonly conversationId: EntityId;
  readonly startedAtMs?: number | null;
  readonly endedAtMs?: number | null;
  readonly state: "running" | "settled" | "cancelled" | "interrupted";
  /** Server-projected kind of the durable invocation; never inferred from name. */
  readonly toolKind?: "function" | "custom";
  /** Present for real Engine tool calls; command-level jobs omit these fields. */
  readonly requestId?: EntityId;
  readonly callId?: string;
  readonly toolName?: string;
  /** Output is persisted in conversation input, not merely provider-ready. */
  readonly delivered?: boolean;
  readonly output?: unknown;
  readonly version?: number;
}

export interface Envelope {
  readonly id: EntityId;
  readonly recipient: string;
  readonly sender: string;
  readonly type: "NEW_TASK" | "MESSAGE" | "FINAL_ANSWER" | "PROGRESS";
  readonly payload: string;
  readonly ordinal?: number;
  readonly version?: number;
}

export type StateEvent =
  | { readonly kind: "host_run.upsert"; readonly value: { readonly run: string } }
  | { readonly kind: "command.receipt"; readonly value: CommandReceipt }
  | { readonly kind: "actor.upsert"; readonly value: HostActorProjection }
  | { readonly kind: "conversation.upsert"; readonly value: Conversation }
  | { readonly kind: "request.upsert"; readonly value: RequestRecord }
  | { readonly kind: "job.upsert"; readonly value: Job }
  | { readonly kind: "envelope.upsert"; readonly value: Envelope }
  | { readonly kind: "entity.remove"; readonly value: { readonly entity: "actor" | "conversation" | "request" | "job" | "envelope"; readonly id: EntityId } };

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
  readonly hostRun?: string;
  readonly actors: ReadonlyMap<EntityId, HostActorProjection>;
  readonly commandReceipts: ReadonlyMap<EntityId, CommandReceipt>;
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
    hostRun: snapshot.hostRun,
    actors: new Map((snapshot.actors ?? []).map((actor) => [actorIdentityKey(actor.identity), actor])),
    commandReceipts: (snapshot.commandReceipts ?? []).reduce<ReadonlyMap<EntityId, CommandReceipt>>(
      (receipts, receipt) => setCommandReceipt(receipts, receipt),
      new Map<EntityId, CommandReceipt>(),
    ),
    conversations: index(snapshot.conversations),
    requests: index(snapshot.requests),
    jobs: index(snapshot.jobs),
    envelopes: index(snapshot.envelopes),
  };
}

/** Projected state events and auxiliary sequence numbers share this stream. */
export function applyStateEvent(state: NormalizedState, message: SequencedEvent): ApplyResult {
  const expected = state.seq + 1;
  if (!Number.isSafeInteger(message.seq) || message.seq !== expected) {
    return { kind: "resync", expected, received: message.seq };
  }
  const next = { ...state, seq: message.seq };
  switch (message.event.kind) {
    case "host_run.upsert":
      return { kind: "applied", state: { ...next, hostRun: message.event.value.run } };
    case "command.receipt":
      return {
        kind: "applied",
        state: {
          ...next,
          commandReceipts: setCommandReceipt(next.commandReceipts, message.event.value),
        },
      };
    case "actor.upsert":
      return { kind: "applied", state: { ...next, actors: setByKey(next.actors, actorIdentityKey(message.event.value.identity), message.event.value) } };
    case "conversation.upsert":
      return { kind: "applied", state: { ...next, conversations: set(next.conversations, message.event.value) } };
    case "request.upsert":
      return { kind: "applied", state: { ...next, requests: set(next.requests, message.event.value) } };
    case "job.upsert":
      return { kind: "applied", state: { ...next, jobs: set(next.jobs, message.event.value) } };
    case "envelope.upsert":
      return { kind: "applied", state: { ...next, envelopes: set(next.envelopes, message.event.value) } };
    case "entity.remove": {
      const { entity, id } = message.event.value;
      switch (entity) {
        case "actor": {
          const table = new Map(next.actors); table.delete(id);
          return { kind: "applied", state: { ...next, actors: table } };
        }
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
    default:
      // Auxiliary server events that do not project snapshot state still own
      // their sequence number and must not trigger a false gap/resync.
      return { kind: "applied", state: next };
  }
}

function index<T extends { readonly id: EntityId }>(values: readonly T[]): ReadonlyMap<EntityId, T> {
  return new Map(values.map((value) => [value.id, value]));
}

function set<T extends { readonly id: EntityId }>(table: ReadonlyMap<EntityId, T>, value: T): ReadonlyMap<EntityId, T> {
  return new Map(table).set(value.id, value);
}

function setByKey<T>(table: ReadonlyMap<EntityId, T>, key: EntityId, value: T): ReadonlyMap<EntityId, T> {
  return new Map(table).set(key, value);
}

function setCommandReceipt(
  table: ReadonlyMap<EntityId, CommandReceipt>,
  receipt: CommandReceipt,
): ReadonlyMap<EntityId, CommandReceipt> {
  const next = new Map(table);
  const key = isOperationId(receipt.commandId) ? canonicalOperationId(receipt.commandId) : receipt.commandId;
  next.delete(key);
  next.set(key, receipt);
  while (next.size > commandReceiptLimit) {
    const oldest = next.keys().next().value;
    if (oldest === undefined) break;
    next.delete(oldest);
  }
  return next;
}

export interface HostCommandRefusal {
  readonly operation_id: string | null;
  readonly code: 'invalid_command' | 'conflict' | 'unavailable' | 'wrong_run';
  readonly reason: string;
}

export function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function isOperationId(value: unknown): value is string {
  return typeof value === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);
}

/** Only operation and round UUIDs have case-insensitive identity. */
export function canonicalOperationId(value: string): string {
  if (!isOperationId(value)) throw new Error('The operation ID is invalid.');
  return value.toLowerCase();
}

export function isHostIdentity(value: unknown): value is HostActorIdentity {
  return isObject(value) && [value.run, value.actor, value.incarnation]
    .every((part) => typeof part === 'string' && part.length > 0);
}

export function sameHostIdentity(left: HostActorIdentity, right: HostActorIdentity): boolean {
  return left.run === right.run && left.actor === right.actor && left.incarnation === right.incarnation;
}

export function isHostCommand(value: unknown): value is HostCommand {
  if (!isObject(value) || !isHostIdentity(value.target)) return false;
  switch (value.action) {
    case 'input': return typeof value.text === 'string';
    case 'interrupt': return isOperationId(value.expected_round);
    case 'retire': return true;
    default: return false;
  }
}

export function sameHostCommand(left: HostCommand, right: HostCommand): boolean {
  return sameHostIdentity(left.target, right.target) && left.action === right.action
    && (left.action !== 'input' || (right.action === 'input' && left.text === right.text))
    && (left.action !== 'interrupt' || (right.action === 'interrupt'
      && canonicalOperationId(left.expected_round) === canonicalOperationId(right.expected_round)));
}

export function isCommandState(value: unknown): value is EmbeddedCommandRecord['state'] {
  return ['queued', 'dispatching', 'input_admitted', 'control_requested', 'refused', 'unconfirmed'].includes(value as string);
}

export function isCommandReceipt(value: unknown): value is CommandReceipt {
  if (!isObject(value) || typeof value.commandId !== 'string' || value.commandId.length === 0) return false;
  if ('target' in value && !isHostIdentity(value.target)) return false;
  switch (value.outcome) {
    case 'admitted':
      return typeof value.envelopeId === 'string' && value.envelopeId.length > 0
        && (value.wakeError === undefined || typeof value.wakeError === 'string');
    case 'control_requested':
      return isHostIdentity(value.target) && (value.control === 'interrupt' || value.control === 'retire');
    case 'unconfirmed': return isHostIdentity(value.target) && typeof value.reason === 'string';
    case 'refused': return typeof value.reason === 'string';
    default: return false;
  }
}

export function isCommandRefusal(value: unknown): value is HostCommandRefusal {
  return isObject(value) && (value.operation_id === null || isOperationId(value.operation_id))
    && ['invalid_command', 'conflict', 'unavailable', 'wrong_run'].includes(value.code as string)
    && typeof value.reason === 'string';
}

export function isEmbeddedCommandRecord(value: unknown): value is EmbeddedCommandRecord {
  return isObject(value) && isOperationId(value.operationId) && isHostCommand(value.command)
    && isCommandState(value.state)
    && (value.envelopeId === null || (Number.isSafeInteger(value.envelopeId) && (value.envelopeId as number) >= 0))
    && (value.receipt === null || isCommandReceipt(value.receipt));
}

function validProjection(kind: string, value: unknown): boolean {
  if (!isObject(value)) return false;
  const id = () => typeof value.id === 'string' && value.id.length > 0;
  switch (kind) {
    case 'host_run.upsert': return typeof value.run === 'string' && value.run.length > 0;
    case 'command.receipt': return isCommandReceipt(value);
    case 'actor.upsert': return isHostIdentity(value.identity) && (value.parent === null || isHostIdentity(value.parent))
      && ['model', 'workflow'].includes(value.kind as string)
      && ['running', 'waiting', 'retiring', 'retired', 'lost'].includes(value.lifecycle as string)
      && (value.modelConversation === null || typeof value.modelConversation === 'string')
      && (value.activeRound === undefined || typeof value.activeRound === 'string');
    case 'conversation.upsert': return id() && typeof value.path === 'string'
      && ['idle', 'requesting', 'paused', 'cancelled'].includes(value.state as string);
    case 'request.upsert': return id() && typeof value.conversationId === 'string'
      && ['running', 'completed', 'failed'].includes(value.state as string);
    case 'job.upsert': return id() && typeof value.conversationId === 'string'
      && ['running', 'settled', 'cancelled', 'interrupted'].includes(value.state as string);
    case 'envelope.upsert': return id() && typeof value.sender === 'string' && typeof value.recipient === 'string'
      && typeof value.payload === 'string' && ['NEW_TASK', 'MESSAGE', 'FINAL_ANSWER', 'PROGRESS'].includes(value.type as string);
    case 'entity.remove': return id() && ['actor', 'conversation', 'request', 'job', 'envelope'].includes(value.entity as string);
    default: return true; // Auxiliary events occupy sequence numbers without projecting a table.
  }
}

export function isSnapshot(value: unknown): value is Snapshot {
  if (!isObject(value) || !Number.isSafeInteger(value.seq) || (value.seq as number) < 0
    || (value.hostRun !== undefined && (typeof value.hostRun !== 'string' || value.hostRun.length === 0))) return false;
  for (const [table, kind] of [['conversations', 'conversation.upsert'], ['requests', 'request.upsert'],
    ['jobs', 'job.upsert'], ['envelopes', 'envelope.upsert']] as const) {
    const rows = value[table];
    if (!Array.isArray(rows) || !rows.every((row) => validProjection(kind, row))) return false;
  }
  return (value.actors === undefined || (Array.isArray(value.actors) && value.actors.every((row) => validProjection('actor.upsert', row))))
    && (value.commandReceipts === undefined || (Array.isArray(value.commandReceipts) && value.commandReceipts.every(isCommandReceipt)));
}

export function isSequencedEvent(value: unknown): value is SequencedEvent {
  return isObject(value) && Number.isSafeInteger(value.seq) && (value.seq as number) >= 0
    && isObject(value.event) && typeof value.event.kind === 'string'
    && validProjection(value.event.kind, value.event.value);
}
