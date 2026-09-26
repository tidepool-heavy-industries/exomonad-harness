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
`*.upsert` event kinds; optional descriptive fields are additive. Every
accepted command gets an ID and must be represented by a durable outcome.
Tests may assert the existing record keys (`id`, `conversationId`, `state`,
`sender`, `recipient`, `type`, `payload`) and states, and should not guess
the names of additive progress fields before the server owner publishes
them. The root supplies HTTP and WebSocket test dependencies in the demo
crate's manifest; the test owner edits only the integration test.
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
