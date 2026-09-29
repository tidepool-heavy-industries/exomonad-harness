# Next: connect the accepted harness library to Exomonad

The standalone embedding batch is implemented. See
[the implementation handoff](docs/embedding-ready-handoff.md) for revisions,
interfaces, tests and limitations. Wave22 production candidates have been joined;
their original branches and run evidence remain retained.

## What is ready

- Raw custom and structured calls, durable claims, scoped replay and browser recovery.
- Host-bound conversations, immutable request tool surfaces, durable idempotent
  input admission, and separate inclusion/cleanup evidence.
- Reusable conversation checkpoints with opaque process-local host attachments.
- Plain-text compaction near half configured context capacity, preserving pending
  calls and recording failures/no-progress without repeatedly retrying them.
- Authenticated browser views of model and workflow actors, retained request
  history, and deterministic external-host acceptance through the public library.

This is offline library readiness. It is not real Haskell execution, accepted
live-provider async behavior, or permission to migrate a running wave.

## Next work

1. Complete harness operation-identity isolation before sharing a scheduler
   across conversations. Preserve original provider IDs and the origin of
   inherited pending claims; review Store/scheduler/replay keying together.
   This work can proceed during the Tidepool engine foundation batch.
2. Consume the exact accepted harness revision through its public embedding
   interfaces. Keep actor lifecycle, custody and commands in their existing
   Tidepool owners; keep conversation history and input envelopes in Store.
3. First prove the embedded sequential resident path, including cancellation
   and reconnect, once the engine foundation is accepted. Then implement and
   prove concurrent cells, atomic publication, real source/checkpoint leases,
   exact typed replies and acknowledged cleanup as a separate milestone.
4. Host this library's Router in the main Exomonad process. Connect host actor
   projection and control; do not install the standalone TreeDriver as a second
   supervisor. Check Codex fallback independently.
5. Exercise the browser-operated Sol/Luna worker tree specified in
   [the embedded-host contract](docs/embedded-host-prd.md), then deliberately
   choose a trial wave and record matched binaries, workspace and asset revisions.

The [daily-driver roadmap](docs/daily-driver-plan.md) retains the larger sequence.
Stock Codex stays installed. Read-only Codex credentials, existing browser login
behind loopback/Tailscale HTTPS and one shared instance per run remain the initial
operator profile. Deployment, live credentialed checks and default cutover need
an operator assignment. No launch follows merely from passing these checks.

Server provisioning is separate; the OVH machine may first run the existing
Exomonad setup. Historical assignments are in
[pre-adoption-next](docs/pre-adoption-next.md).
