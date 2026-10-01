import { actorIdentityKey, applyStateEvent, normalizeSnapshot, type SequencedEvent, type Snapshot } from "./protocol";
import { describe, expect, it } from "vitest";

// Recorded-shaped fixture: consumers can replace this with captured server JSON
// without bringing Rust types into the browser package.
const fixture: Snapshot = {
  seq: 41,
  conversations: [{ id: "c/root", path: "/root", state: "requesting" }],
  requests: [{ id: "r/7", conversationId: "c/root", state: "running" }],
  jobs: [],
  envelopes: [{ id: "e/2", recipient: "/root", sender: "/operator", type: "MESSAGE", payload: "continue" }],
};

describe("stable JSON state adapter", () => {
  it("normalizes snapshots and applies contiguous events immutably", () => {
    const initial = normalizeSnapshot(fixture);
    expect(initial.seq).toBe(41);
    expect(initial.requests.has("r/7")).toBe(true);
    const applied = applyStateEvent(initial, {
      seq: 42,
      event: { kind: "job.upsert", value: { id: "j/1", conversationId: "c/root", state: "running" } },
    });
    expect(applied.kind).toBe("applied");
    if (applied.kind === "applied") {
      expect(applied.state.jobs.has("j/1")).toBe(true);
      expect(applied.state.seq).toBe(42);
    }
    expect(initial.seq).toBe(41);
    expect(initial.jobs.has("j/1")).toBe(false);
  });

  it("rejects sequence gaps without applying the event", () => {
    const initial = normalizeSnapshot(fixture);
    const gap = applyStateEvent(initial, {
      seq: 43,
      event: { kind: "conversation.upsert", value: { id: "c/other", path: "/other", state: "idle" } },
    });
    expect(gap).toEqual({ kind: "resync", expected: 42, received: 43 });
    expect(initial.conversations.has("c/other")).toBe(false);
  });

  it("projects host workflow incarnations separately from conversations across reconnect", () => {
    const identity = { run: "run-1", actor: "worker", incarnation: "second" };
    const actor = {
      identity, parent: { run: "run-1", actor: "root", incarnation: "first" },
      kind: "workflow" as const, lifecycle: "waiting" as const, modelConversation: null,
    };
    const initial = normalizeSnapshot({ ...fixture, actors: [actor] });
    const key = actorIdentityKey(identity);
    expect(initial.actors.get(key)).toEqual(actor);
    expect(initial.conversations.size).toBe(1);
    const result = applyStateEvent(initial, {
      seq: 42, event: { kind: "actor.upsert", value: { ...actor, lifecycle: "retired" } },
    });
    expect(result.kind).toBe("applied");
    if (result.kind === "applied") {
      expect(result.state.actors.get(key)?.lifecycle).toBe("retired");
      expect(normalizeSnapshot({ ...fixture, seq: 42, actors: [{ ...actor, lifecycle: "retired" }] }).actors.get(key))
        .toEqual(result.state.actors.get(key));
    }
    expect(initial.actors.get(key)?.lifecycle).toBe("waiting");
  });

  it("updates explicit host mode from a host-run event even with no actor rows", () => {
    const initial = normalizeSnapshot({ ...fixture, hostRun: undefined, actors: [] });
    const result = applyStateEvent(initial, {
      seq: 42,
      event: { kind: "host_run.upsert", value: { run: "run-empty" } },
    });
    expect(result.kind).toBe("applied");
    if (result.kind === "applied") {
      expect(result.state.hostRun).toBe("run-empty");
      expect(result.state.actors.size).toBe(0);
    }
  });

  it("applies bounded command receipts as sequenced observable handoffs", () => {
    const receipt = {
      commandId: "cmd-4",
      target: { run: "run-1", actor: "/root/worker", incarnation: "inc-2" },
      outcome: "control_requested" as const,
      control: "interrupt" as const,
    };
    const initial = normalizeSnapshot(fixture);
    const applied = applyStateEvent(initial, {
      seq: fixture.seq + 1,
      event: { kind: "command.receipt", value: receipt },
    });
    expect(applied.kind).toBe("applied");
    if (applied.kind === "applied") {
      expect(applied.state.commandReceipts.get("cmd-4")).toEqual(receipt);
      expect(applied.state.seq).toBe(fixture.seq + 1);
    }
  });

  it("advances over a valid auxiliary event without requesting a false resync", () => {
    const initial = normalizeSnapshot(fixture);
    const result = applyStateEvent(initial, {
      seq: fixture.seq + 1,
      event: { kind: "control.requested", value: { commandId: "cmd-5" } },
    } as unknown as SequencedEvent);
    expect(result.kind).toBe("applied");
    if (result.kind === "applied") expect(result.state.seq).toBe(fixture.seq + 1);
  });

  it("bounds live receipts and moves a replacement to the newest position", () => {
    let state = normalizeSnapshot(fixture);
    for (let index = 0; index < 129; index += 1) {
      const result = applyStateEvent(state, {
        seq: state.seq + 1,
        event: {
          kind: "command.receipt",
          value: { commandId: `cmd-${index}`, outcome: "refused", reason: "not admitted" },
        },
      });
      expect(result.kind).toBe("applied");
      if (result.kind === "applied") state = result.state;
    }
    expect(state.commandReceipts.size).toBe(128);
    expect([...state.commandReceipts.keys()][0]).toBe("cmd-1");
    expect([...state.commandReceipts.keys()].at(-1)).toBe("cmd-128");

    const replacement = applyStateEvent(state, {
      seq: state.seq + 1,
      event: {
        kind: "command.receipt",
        value: { commandId: "cmd-1", outcome: "admitted", envelopeId: "env-refreshed" },
      },
    });
    expect(replacement.kind).toBe("applied");
    if (replacement.kind === "applied") {
      expect(replacement.state.commandReceipts.size).toBe(128);
      expect([...replacement.state.commandReceipts.keys()][0]).toBe("cmd-2");
      expect([...replacement.state.commandReceipts.keys()].at(-1)).toBe("cmd-1");
      expect(replacement.state.commandReceipts.get("cmd-1")).toMatchObject({
        outcome: "admitted", envelopeId: "env-refreshed",
      });
    }
  });
});

it('validates every receipt outcome and rejects unsupported or incomplete handoff evidence', async () => {
  const { isCommandReceipt, isEmbeddedCommandRecord } = await import('./protocol')
  const target = { run: 'Run', actor: '/ROOT', incarnation: 'Inc' }
  const commandId = '11111111-1111-4111-8111-111111111111'
  for (const receipt of [
    { commandId, target, outcome: 'admitted', envelopeId: '42', wakeError: 'wake failed' },
    { commandId, target, outcome: 'control_requested', control: 'interrupt' },
    { commandId, target, outcome: 'refused', reason: 'denied' },
    { commandId, target, outcome: 'unconfirmed', reason: 'ack lost' },
  ]) expect(isCommandReceipt(receipt)).toBe(true)
  for (const receipt of [
    { commandId, target, outcome: 'future_success' },
    { commandId, outcome: 'unconfirmed', reason: 'unknown' },
    { commandId, target, outcome: 'admitted', envelopeId: 42 },
    { commandId, target, outcome: 'control_requested', control: 'kill' },
  ]) {
    expect(isCommandReceipt(receipt)).toBe(false)
    expect(isEmbeddedCommandRecord({ operationId: commandId, command: { action: 'retire', target }, state: 'queued', envelopeId: null, receipt })).toBe(false)
  }
})

it('validates known optional fields while preserving absent parent and explicit null provenance', async () => {
  const { isSnapshot, isSequencedEvent } = await import('./protocol')
  const legacy = { seq: 0, conversations: [{ id: 'c', path: '/c', state: 'idle' }], requests: [], jobs: [], envelopes: [] }
  expect(isSnapshot(legacy)).toBe(true)
  expect(Object.hasOwn(legacy.conversations[0]!, 'parentId')).toBe(false)
  expect(isSnapshot({ ...legacy, conversations: [{ ...legacy.conversations[0], parentId: null }] })).toBe(true)
  const malformed = [
    { kind: 'conversation.upsert', value: { ...legacy.conversations[0], parentId: 123 } },
    { kind: 'job.upsert', value: { id: 'j', conversationId: 'c', state: 'running', delivered: 'false' } },
    { kind: 'job.upsert', value: { id: 'j', conversationId: 'c', state: 'running', toolKind: {} } },
    { kind: 'request.upsert', value: { id: 'r', conversationId: 'c', state: 'running', createdAtMs: Infinity } },
    { kind: 'envelope.upsert', value: { id: 'e', sender: 's', recipient: 'r', type: 'MESSAGE', payload: '', ordinal: -1 } },
  ]
  for (const event of malformed) expect(isSequencedEvent({ seq: 1, event })).toBe(false)
  expect(isSequencedEvent({ seq: 1, event: { kind: 'tokens.observed', value: null } })).toBe(true)
})
