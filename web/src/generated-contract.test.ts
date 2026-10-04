import { describe, expect, it } from 'vitest';
import {
  isServerFrame, isClientFrame, isActorOutputHistoryPage, isActorDisplayExpansion,
  isHistoryPage, isEmbeddedCommandRecord, isServerEvent,
} from './generated/validators.mjs';
import samples from './generated/wire-samples.json';
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

  it('refuses rounded counters, overflow, alternate decimal spellings and unknown events', () => {
    const frame = frames.find(frame => frame.type === 'snapshot');
    expect(frame).toBeDefined();
    for (const seq of [9007199254740992, '18446744073709551616', '-1', '01', '+1', '1e3']) {
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
