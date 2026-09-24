# Operator web view

Run `npm install`, `npm test`, and `npm run build` from this directory.
The default view renders `src/fixture.ts`; this keeps the screens usable before
the harness server is integrated. Add `?live=1` to connect to the same-origin
WebSocket endpoint `/ws`.

The browser boundary is `src/protocol.ts` and `src/ws-client.ts`. The expected
JSON frames are:

```json
{"type":"snapshot","snapshot":{"seq":1,"conversations":[],"requests":[],"jobs":[],"envelopes":[]}}
{"type":"event","event":{"seq":2,"event":{"kind":"conversation.upsert","value":{"id":"root","path":"/root","state":"requesting"}}}}
{"type":"command","command":"wait_agent"}
```

The event reducer applies only contiguous state-lane sequence numbers and
requests a full snapshot after a gap. The UI is a view-only projection; wire
types intentionally are not imported from Rust. The initial timeline is a
table projection of request/job state (not a duration chart); live event
deltas, richer command results, server protocol conformance, and end-to-end
visual/accessibility evidence still require server integration.
