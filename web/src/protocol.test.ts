import { actorIdentityKey, applyStateEvent, normalizeSnapshot, type Snapshot } from "./protocol";
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
});
