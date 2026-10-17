import { describe, expect, it } from 'vitest';
import {
  isServerFrame, isClientFrame, isActorOutputHistoryPage, isActorDisplayExpansion,
  isHistoryPage, isEmbeddedCommandRecord, isServerEvent, isStoredActorForm, isChatPage, isClientCommand, isActorFormSubmit, isActorFormDismiss,
} from './generated/validators.mjs';
import samples from './generated/wire-samples.json';
import type { HistoryPage } from './generated/history';
import type { InferenceBlock, Snapshot } from './protocol';
import { isEmbeddedCommandRecord as isAssociatedCommandRecord, isInferenceBlock, sameInferenceBlock } from './protocol';
const rawFrames: unknown[] = samples.server;
const frames = rawFrames.filter(isServerFrame);

describe('Rust-owned browser wire contracts', () => {
  it('accepts every positive wire sample emitted by the Rust owner', () => {
    for (const frame of samples.server) expect(isServerFrame(frame)).toBe(true);
    for (const event of samples.serverEvent) expect(isServerEvent(event)).toBe(true);
    for (const frame of samples.client) expect(isClientFrame(frame)).toBe(true);
    for (const page of samples.actorOutputHistory) expect(isActorOutputHistoryPage(page)).toBe(true);
    for (const input of samples.actorDisplayExpansion) expect(isActorDisplayExpansion(input)).toBe(true);
    for (const page of samples.history) expect(isHistoryPage(page)).toBe(true);
    for (const form of samples.actorForm) expect(isStoredActorForm(form)).toBe(true);
    for (const page of samples.chat) expect(isChatPage(page)).toBe(true);
    for (const command of samples.clientCommand) expect(isClientCommand(command)).toBe(true);
    for (const input of samples.actorFormSubmit) expect(isActorFormSubmit(input)).toBe(true);
    for (const input of samples.actorFormDismiss) expect(isActorFormDismiss(input)).toBe(true);
    for (const record of samples.embeddedCommand) expect(isEmbeddedCommandRecord(record)).toBe(true);
  });

  it('keeps all opaque JSON forms inside typed history and tool-result fields', () => {
    const opaque: unknown[] = [{ provider: [null, 'nested'] }, [null, true, 'retained'], 'text', 23.5, false, null];
    for (const value of opaque) {
      const page: HistoryPage = { requestId: 'request/root', parentId: null, branch: '/root', items: [{ position: '0', hash: 'a'.repeat(64), byteLen: '1', item: value }], nextOffset: null, oversizedItem: null };
      const job: Snapshot['jobs'][number] = { id: 'job', conversationId: 'root', state: 'settled', startedAtMs: null, endedAtMs: null, output: value };
      expect(isHistoryPage(page)).toBe(true);
      expect(isServerFrame({ type: 'event', event: { seq: '1', event: { kind: 'job.upsert', value: job } } })).toBe(true);
      expect(isServerFrame({ type: 'event', event: { seq: '1', event: { kind: 'job.upsert', value } } })).toBe(false);
    }
  });

  it('refuses rounded counters, overflow, alternate decimal spellings and unknown events', () => {
    const frame = frames.find(frame => frame.type === 'snapshot');
    expect(frame).toBeDefined();
    for (const seq of [9007199254740992, '18446744073709551616', '-1', '01', '+1', '1e3', '1\n', '1\r', '1 ', ' 1']) {
      expect(isServerFrame({ ...frame, snapshot: { ...frame!.snapshot, seq } })).toBe(false);
    }
    expect(isServerFrame({ type: 'event', event: { seq: '1', event: { kind: 'unknown', value: {} } } })).toBe(false);
  });

  it('preserves form revisions and recovery generation above the JavaScript integer range', () => {
    const form = samples.actorForm[0]!;
    expect(form.sequence).toBe('9007199254740993');
    expect(form.opening.origin.nativeActor).toBe('18446744073709551615');
    expect(isStoredActorForm({ ...form, revisionSequence: 9007199254740992 })).toBe(false);
    const commands: unknown[] = samples.clientCommand;
    const recovery = commands.filter(isClientCommand).find(command => command.type === 'host');
    if (recovery?.type !== 'host' || recovery.command.action !== 'recover') throw new Error('Missing recovery sample');
    expect(recovery.command.expected_block.generation).toBe('18446744073709551615');
    expect(isClientCommand({ ...recovery, command: { ...recovery.command, expected_block: { ...recovery.command.expected_block, generation: 9007199254740992 } } })).toBe(false);
    expect(isChatPage({ ...samples.chat[0], nextAfter: 9007199254740992 })).toBe(false);
  });

  it('checks operation state and observation association after generated shape validation', () => {
    const record = samples.embeddedCommand[0]!;
    expect(isAssociatedCommandRecord(record)).toBe(true);
    expect(isAssociatedCommandRecord({ ...record, command: { action:'retire', target:record.command.target }, state:'recovery_authorized', receipt:null })).toBe(false);
    expect(isAssociatedCommandRecord({ ...record, inputObservation:{state:'included', request:''} })).toBe(false);
    const first = { request:'request', generation:'9007199254740993', reason:{kind:'request_rejected' as const, failure:{kind:'http' as const, status:400, diagnostic:{code:'code', message:'message'}}} };
    const reordered = { ...first, reason:{kind:'request_rejected' as const, failure:{kind:'http' as const, status:400, diagnostic:{message:'message', code:'code'}}} };
    expect(sameInferenceBlock(first, reordered)).toBe(true);
    expect(sameInferenceBlock(first, {...reordered, reason:{...reordered.reason, failure:{...reordered.reason.failure, diagnostic:{...reordered.reason.failure.diagnostic, param:null}}}})).toBe(true);
    expect(sameInferenceBlock(first, {...reordered, generation:'9007199254740994'})).toBe(false);
  });

  it('compares complete request refusal facts across providers and credential causes', () => {
    type Failure = Extract<InferenceBlock['reason'], { kind: 'request_rejected' }>['failure'];
    const failures: Failure[] = [
      { kind: 'authentication' },
      { kind: 'http', status: 400, diagnostic: null },
      { kind: 'http', status: 401, diagnostic: null },
      { kind: 'http', status: 400, diagnostic: { code: 'different' } },
    ];
    for (const route of ['chat_gpt_plan', 'deep_seek'] as const) {
      failures.push({ kind: 'provider_authentication', route });
      for (const reason of ['missing', 'unreadable', 'unsafe', 'invalid', 'worker_failed'] as const) {
        failures.push({ kind: 'credential_unavailable', route, reason });
      }
    }
    const blocks: InferenceBlock[] = failures.map(failure => ({
      request: 'same-request', generation: '7', reason: { kind: 'request_rejected', failure },
    }));
    for (const block of blocks) expect(isInferenceBlock(block)).toBe(true);
    // Each row changes one owner-issued refusal fact; only the diagonal is equal.
    for (const [left, a] of blocks.entries()) {
      for (const [right, b] of blocks.entries()) {
        expect(sameInferenceBlock(a, b)).toBe(left === right);
      }
    }
  });

  it('keeps actor output history and live event bodies identical', () => {
    const event = frames.filter(frame => frame.type === 'event')
      .find(frame => frame.event.event.kind === 'actor.output.committed');
    expect(event?.event.event.value).toEqual(samples.actorOutputHistory[0]?.outputs[0]);
  });
});
