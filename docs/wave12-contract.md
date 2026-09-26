# Wave 12 shared browser contract

Source baseline: `6e3dcd32b686cc84dc4149cd18183bfacb7bc54c`.

Keep the existing authenticated loopback HTTP server and `/api/ws` snapshot/event
channel. The browser sends the existing string command frame. Do not add a
second transport, scheduler, database, or client-only simulated responses.

Deterministic mode commands (case-sensitive; whitespace after the verb is data):

- `echo TEXT`: record a root request, visible progress, then a final answer
  containing TEXT.
- `test`: execute a deterministic success check and report its result.
- `wait`: enter a pending root request until a later `cancel` command; no
  fabricated completion.
- `message TEXT`: while `wait` is active, report whether the message was
  queued, presented, or acted on using the actual state; show it in history.
- `cancel`: stop a pending `wait`, record a terminal cancelled state.
- `child TEXT`: create a visible child identity, deliver a parent message, and
  show its reply with ordering.
- `fail`: cause a controlled, readable failed request; a subsequent `echo`
  must work.

The server owns command interpretation, durable state transitions and event
publication. Wire records retain the existing `Snapshot` arrays and
`*.upsert` event kinds. Every accepted command gets an ID and must be
represented by a durable outcome.

Additive browser fields for this wave, owned by the server:

- Each request record has `commandId` (the ID in `command.accepted`),
  `command` (the submitted string), `outcome` (`accepted`, `pending`,
  `queued`, `presented`, `acted`, `completed`, `cancelled`, or `failed`),
  and optional `detail` (readable reason). Existing `state` remains the
  coarse request lifecycle; do not overload it with delivery states.
  `outcome` describes only what the server has actually observed.
  For a cancelled wait, preserve the existing request `state:"failed"`
  (the request did not complete), set `outcome:"cancelled"`, and mark its
  associated job `state:"cancelled"`. The browser presents the specific
  `outcome`, not the coarse request state, as the human-facing reason.
  This avoids widening the shared Request.state enum merely for this demo.
- Progress is a durable envelope with `type:"PROGRESS"`, `sender:"/harness"`,
  `recipient` identifying the affected agent and human-readable `payload`.
  Message and reply envelopes use existing `MESSAGE` and `FINAL_ANSWER`
  types with their real sender/recipient. Each new envelope has an additive
  integer `ordinal`, monotonically increasing within the persisted session,
  so browser refresh preserves the same order. Do not insert a progress
  envelope only after completion and call that live progress.
- A child is a distinct conversation row with its own `id` and `path`
  under `/root/`; a child-looking final envelope alone is insufficient.

The browser accepts absent additive fields for old snapshots, but when they
exist it displays command ID/outcome, detail, progress and ordered messages
without inferring `presented` or `acted` from `queued`. Tests may assert the
existing record keys (`id`, `conversationId`, `state`, `sender`, `recipient`,
`type`, `payload`) plus the additive fields above. The root supplies HTTP
and WebSocket test dependencies in the demo crate's manifest; the test
owner edits only the integration test.
Snapshot on reconnect must reflect stored history, not replay commands.
Browser refresh is the primary reconnect guarantee. Process restart must
recover settled history and mark interrupted work honestly, never re-execute
it. No remote exactly-once claim.

The browser owns presentation and command affordances using this grammar.
It must label deterministic mode and show command outcomes, pending state,
failure, child identity, and reconnect without inventing event states.
Acceptance tests must drive the real authenticated HTTP/WS binary, including
failure and recovery, and may request corrections from owners rather than
editing their files.

Integration barrier: a `wait` must not block the server command receiver,
and `cancel` must be terminal even if a connection drops. Test that failure
and a later success are distinct durable records. Secrets must not appear in
tracked artifacts. Shell and model inference stay disabled in this mode.
