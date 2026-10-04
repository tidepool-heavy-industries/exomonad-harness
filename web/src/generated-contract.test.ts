import { describe, expect, it } from 'vitest';
import {
  isServerFrame, isClientFrame, isActorOutputHistoryPage, isActorDisplayExpansion,
  isHistoryPage, isEmbeddedCommandRecord, isServerEvent,
} from './generated/validators.mjs';
import samples from './generated/wire-samples.json';
import type { HistoryPage } from './generated/history';
import type { Snapshot } from './protocol';
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

  it('keeps actor output history and live event bodies identical', () => {
    const event = frames.filter(frame => frame.type === 'event')
      .find(frame => frame.event.event.kind === 'actor.output.committed');
    expect(event?.event.event.value).toEqual(samples.actorOutputHistory[0]?.outputs[0]);
  });
});
