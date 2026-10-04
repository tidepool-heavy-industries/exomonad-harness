import { appendOutput, commitOutput, setRevision, outputKey, revisionKey,
  type LiveOutput, type HistoryRevision } from './live-output';
import { actorOutputKey, retainActorOutput, isStoredActorOutput, validActorOutputReference, type ActorOutputReference } from './actor-output';
import { isServerFrame, isClientFrame, isEmbeddedCommandRecord as isCommandRecordShape } from './generated/validators.mjs';
import type { ServerFrame } from './generated/server';
import type { ClientFrame } from './generated/client';
export type { EmbeddedCommandRecord } from './generated/embedded-command';
import type { EmbeddedCommandRecord } from './generated/embedded-command';

/** Browser wire types are projected from the Rust-owned generated contract. */
export type EntityId = string;
export type Snapshot = Extract<ServerFrame, { type: 'snapshot' }>['snapshot'];
export type SequencedEvent = Extract<ServerFrame, { type: 'event' }>['event'];
export type StateEvent = SequencedEvent['event'];
export type HostActorProjection = NonNullable<Snapshot['actors']>[number];
export type HostActorIdentity = HostActorProjection['identity'];
export type HostCommand = Extract<ClientFrame, { type: 'host_command' }>['command'];
export type HostCommandSubmission = Omit<Extract<ClientFrame, { type: 'host_command' }>, 'type'>;
export type ClientOperationId = HostCommandSubmission['operation_id'];
export type EmbeddedRoundId = Extract<HostCommand, { action: 'interrupt' }>['expected_round'];
export type CommandReceipt = NonNullable<Snapshot['commandReceipts']>[number];
export type Conversation = Snapshot['conversations'][number];
export type RequestRecord = Snapshot['requests'][number];
export type Job = Snapshot['jobs'][number];
export type Envelope = Snapshot['envelopes'][number];
export type RequestFailure = NonNullable<RequestRecord['failure']>;
export type HttpDiagnostic = NonNullable<Extract<RequestFailure, { kind: 'http' }>['diagnostic']>;
export type HostCommandRefusal = Omit<Extract<ServerFrame, { type: 'command.refused' }>, 'type'>;
const commandReceiptLimit = 128;
const validationOperation = '00000000-0000-0000-0000-000000000000';
const validationTarget = { run: 'validation', actor: 'validation', incarnation: 'validation' };
// Root validators also admit subvalues without duplicating their field schemas.

export function actorIdentityKey(identity: HostActorIdentity): string {
  return JSON.stringify([identity.run, identity.actor, identity.incarnation]);
}

export interface NormalizedState {
  readonly actorOutputRevisions?: ReadonlyMap<string, ActorOutputReference>;
  readonly liveOutput?: ReadonlyMap<string, LiveOutput>;
  readonly historyRevisions?: ReadonlyMap<string, HistoryRevision>;
  readonly seq: string;
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
  | { readonly kind: "resync"; readonly expected: string; readonly received: string };

export function normalizeSnapshot(snapshot: Snapshot): NormalizedState {
  return {
    seq: snapshot.seq,
    actorOutputRevisions: new Map((snapshot.actorOutputRevisions ?? []).map(item => [actorOutputKey(item.origin), item])),
    liveOutput: new Map((snapshot.liveOutput ?? []).map(item => [outputKey(item), item])),
    historyRevisions: new Map((snapshot.historyRevisions ?? []).map(item => [revisionKey(item), item])),
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

/** Every admitted state event advances the exact native sequence. */
export function applyStateEvent(state: NormalizedState, message: SequencedEvent): ApplyResult {
  const expected = (BigInt(state.seq) + 1n).toString();
  if (message.seq !== expected) {
    return { kind: "resync", expected, received: message.seq };
  }
  const next = { ...state, seq: message.seq };
  switch (message.event.kind) {
    case "actor.output.committed": return { kind: 'applied', state: { ...next,
      actorOutputRevisions: retainActorOutput(next.actorOutputRevisions, message.event.value.reference) } };
    case "model.output.stopped": {
      const outputs = new Map(next.liveOutput);
      for (const [key, item] of outputs) if (revisionKey(item) === revisionKey(message.event.value)) outputs.set(key, {...item, streaming:false});
      return {kind:'applied', state:{...next, liveOutput:outputs}};
    }
    case "model.output.started": {
      const output = message.event.value;
      if (output.origin.kind !== 'embedded') return {kind:'applied', state:next};
      const key = actorIdentityKey(output.origin);
      const actor = next.actors.get(key);
      return {kind:'applied', state: actor ? {...next, actors:setByKey(next.actors, key, {...actor, modelHeadRequest:output.requestId})} : next};
    }
    case "model.output.delta": return {kind:'applied', state:{...next, liveOutput:appendOutput(next.liveOutput, message.event.value)}};
    case "model.output.committed": return {kind:'applied', state:{...next,
      liveOutput:commitOutput(next.liveOutput, message.event.value), historyRevisions:setRevision(next.historyRevisions, message.event.value)}};
    case "model.output.remove": {
      const outputs = new Map(next.liveOutput); outputs.delete(outputKey(message.event.value));
      return {kind:'applied', state:{...next, liveOutput:outputs}};
    }
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

export function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
export function isOperationId(value: unknown): value is string {
  return isClientFrame({ type: 'host_command', operation_id: value,
    command: { action: 'retire', target: validationTarget } });
}
/** Only operation and round UUIDs have case-insensitive identity. */
export function canonicalOperationId(value: string): string {
  if (!isOperationId(value)) throw new Error('The operation ID is invalid.');
  let digits = value.toLowerCase();
  if (digits.startsWith('urn:uuid:')) digits = digits.slice(9);
  else if (digits.startsWith('{')) digits = digits.slice(1, -1);
  digits = digits.replaceAll('-', '');
  return [digits.slice(0, 8), digits.slice(8, 12), digits.slice(12, 16), digits.slice(16, 20), digits.slice(20)].join('-');
}
export function isHostIdentity(value: unknown): value is HostActorIdentity {
  const frame = { type: 'host_command', operation_id: validationOperation, command: { action: 'retire', target: value } };
  return isClientFrame(frame) && frame.type === 'host_command'
    && Object.values(frame.command.target).every(part => part.length > 0);
}
export function sameHostIdentity(left: HostActorIdentity, right: HostActorIdentity): boolean {
  return left.run === right.run && left.actor === right.actor && left.incarnation === right.incarnation;
}
export function isHostCommand(value: unknown): value is HostCommand {
  const frame = { type: 'host_command', operation_id: validationOperation, command: value };
  return isClientFrame(frame) && frame.type === 'host_command' && isHostIdentity(frame.command.target);
}
export function sameHostCommand(left: HostCommand, right: HostCommand): boolean {
  return sameHostIdentity(left.target, right.target) && left.action === right.action
    && (left.action !== 'input' || (right.action === 'input' && left.text === right.text))
    && (left.action !== 'interrupt' || (right.action === 'interrupt'
      && canonicalOperationId(left.expected_round) === canonicalOperationId(right.expected_round)));
}
export function isCommandState(value: unknown): value is EmbeddedCommandRecord['state'] {
  return isCommandRecordShape({ operationId: validationOperation,
    command: { action: 'retire', target: validationTarget },
    state: value, envelopeId: null, receipt: null });
}
export function isCommandReceipt(value: unknown): value is CommandReceipt {
  const frame = { type: 'event', event: { seq: '0', event: { kind: 'command.receipt', value } } };
  if (!isServerFrame(frame) || frame.type !== 'event' || frame.event.event.kind !== 'command.receipt') return false;
  const receipt = frame.event.event.value;
  return receipt.commandId.length > 0 && (receipt.target === undefined || isHostIdentity(receipt.target));
}
export function isCommandRefusal(value: unknown): value is HostCommandRefusal {
  return isObject(value) && isServerFrame({ ...value, type: 'command.refused' });
}
/** Shape is generated; operation/target/receipt association is client policy. */
export function isEmbeddedCommandRecord(value: unknown): value is EmbeddedCommandRecord {
  if (!isCommandRecordShape(value)) return false;
  if (value.state === 'input_admitted' && value.command.action !== 'input') return false;
  if (value.state === 'control_requested' && value.command.action === 'input') return false;
  const receipt = value.receipt;
  if (receipt === null) return true;
  if (!isOperationId(receipt.commandId) || canonicalOperationId(receipt.commandId) !== canonicalOperationId(value.operationId)
    || (receipt.target !== undefined && !sameHostIdentity(receipt.target, value.command.target))) return false;
  switch (receipt.outcome) {
    case 'admitted': return value.state === 'input_admitted' && value.command.action === 'input'
      && (value.envelopeId === null || value.envelopeId === receipt.envelopeId);
    case 'control_requested': return value.state === 'control_requested' && value.command.action === receipt.control;
    case 'refused': return value.state === 'refused';
    case 'unconfirmed': return value.state === 'unconfirmed';
  }
}
export function isSnapshot(value: unknown): value is Snapshot {
  const frame = { type: 'snapshot', snapshot: value };
  if (!isServerFrame(frame) || frame.type !== 'snapshot') return false;
  const snapshot = frame.snapshot;
  return (snapshot.liveOutput?.length ?? 0) <= 128
    && (snapshot.historyRevisions?.length ?? 0) <= 128
    && (snapshot.actorOutputRevisions?.length ?? 0) <= 128
    && new TextEncoder().encode(JSON.stringify(snapshot.actorOutputRevisions ?? [])).length <= 65536
    && (snapshot.actorOutputRevisions ?? []).every(validActorOutputReference)
    && (snapshot.actors ?? []).every(value => validStateEvent({ kind: 'actor.upsert', value }))
    && snapshot.conversations.every(value => validStateEvent({ kind: 'conversation.upsert', value }))
    && snapshot.requests.every(value => validStateEvent({ kind: 'request.upsert', value }))
    && snapshot.jobs.every(value => validStateEvent({ kind: 'job.upsert', value }))
    && snapshot.envelopes.every(value => validStateEvent({ kind: 'envelope.upsert', value }))
    && (snapshot.liveOutput ?? []).every(value => validStateEvent({ kind: 'model.output.delta', value }));
}
export function isSequencedEvent(value: unknown): value is SequencedEvent {
  const frame = { type: 'event', event: value };
  return isServerFrame(frame) && frame.type === 'event' && validStateEvent(frame.event.event);
}

/** Identity association and UI retention are separate from generated wire shape. */
function validStateEvent(event: StateEvent): boolean {
  const nullableIdentity = (value: string | null | undefined) => value == null || value.length > 0;
  switch (event.kind) {
    case 'actor.output.committed': return isStoredActorOutput(event.value);
    case 'model.output.started': case 'model.output.stopped': case 'model.output.delta':
    case 'model.output.committed': case 'model.output.remove': {
      const { origin, requestId } = event.value;
      return requestId.length > 0 && origin.actor.length > 0
        && (origin.kind === 'embedded' ? origin.run.length > 0 && origin.incarnation.length > 0 : origin.store.length > 0)
        && (!('itemId' in event.value) || nullableIdentity(event.value.itemId));
    }
    case 'actor.upsert': return isHostIdentity(event.value.identity)
      && (event.value.parent === null || isHostIdentity(event.value.parent))
      && nullableIdentity(event.value.modelConversation) && nullableIdentity(event.value.modelHeadRequest);
    case 'conversation.upsert': return event.value.id.length > 0
      && nullableIdentity(event.value.parentId) && nullableIdentity(event.value.forkSourceRequestId);
    case 'request.upsert': return event.value.id.length > 0 && nullableIdentity(event.value.parentId)
      && withinDiagnosticBounds(event.value.failure);
    case 'job.upsert': case 'envelope.upsert': case 'entity.remove': return event.value.id.length > 0;
    case 'host_run.upsert': return event.value.run.length > 0;
    case 'command.receipt': return event.value.commandId.length > 0;
  }
}

function withinDiagnosticBounds(failure: RequestFailure | null): boolean {
  if (failure?.kind !== 'http' || failure.diagnostic === null) return true;
  const diagnostic = failure.diagnostic;
  const within = (text: string | undefined, limit: number) => text === undefined || Array.from(text).length <= limit;
  return within(diagnostic.code, 256) && within(diagnostic.error_type, 256)
    && within(diagnostic.param, 256) && within(diagnostic.message, 2048);
}
