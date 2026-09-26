# Wave 12 — a usable standalone browser harness

## Outcome

A human can open the harness in a browser over Tailscale, start or enter a
session, submit work, see progress and agent messages, steer or cancel work,
and read its final result. Deliver a running demonstration and the address
and login procedure the operator can use. The implementation remains standalone.

Simple deterministic behavior is sufficient: echo text, emit a few progress
updates, perform a test action, and send a message between agents. Model inference,
Bash execution and production tool integrations are not required. Clearly label
the deterministic mode in the interface. Drive the real server, engine, event
and persistence paths; do not substitute canned browser-only responses.

## Representative human journey

1. Open the browser address and enter through the existing authentication flow.
2. Start or select a session and send an echo/test request. See that it was
   accepted, observe progress, and read the final reply.
3. Start an intentionally pending action. Send another message while it is
   active; show whether it was queued, presented, or acted on without claiming
   more than the system knows. Cancel pending work and observe a terminal state.
4. Exercise a small parent/child interaction: a child receives a message and
   replies, with enough visible identity and ordering to follow what happened.
5. Refresh or reconnect the browser and recover the session's recorded history
   without duplicating submitted work. Exercise one controlled failure with a
   readable outcome and a successful subsequent request.

Use existing product behavior where it already meets these cases. Define exact
scope for process restart separately from browser reconnect; preserve prior
recovery gates, and do not promise restart semantics the demonstration lacks.

## Build and delegate

Read the relevant PRD and web requirements, then inspect current consumers before
changing interfaces. Resolve the smallest shared contracts and a compiling
integration path before dependent work. A reasonable parallel split is:

- deterministic execution and message/progress behavior through existing owners;
- server/session/control wiring and retained event delivery;
- browser interaction, reconnect and clear pending/failure states;
- independent end-to-end acceptance and failure-case tests.

Adjust ownership to the actual source; do not give two implementers the same
shared file. Favor broad bounded Luna work and independent review. Use useful
component depth where it removes coordination. Sol owns shared decisions and
integration, and keeps working on integration while children run.

Do not build generic hook machinery, new persistence or scheduling systems, or
large tool APIs merely to satisfy this demo. If a product seam needs repair,
make that repair in its owning implementation and prove the consumer works.

## Orchestration experiment

Customize the installed session helper seed for this project's focused checks
and evidence. Give children its module name, inputs and evidence contract; check
that they can actually use it. Compose command execution, retained logs and
optional Jev failure triage where that saves repeated model rounds. Deterministic
code owns exit/count facts. Record helper reuse, calls avoided where measurable,
and any friction. Use ordinary event-driven independent review; automatic
review/repair convergence is not required for this run.

## Acceptance and handoff

- Exercise the human journey in an actual browser against the running server;
  retain exact steps and results, including cancellation, reconnect and failure.
- Supply the operator's reachable Tailscale URL and login instructions. Keep
  secrets out of tracked documents. Prefer the existing loopback server behind
  Tailscale Serve; preserve authentication and avoid public Funnel exposure.
  Inspect existing networking configuration before changes and preserve unrelated
  routes. Distinguish local smoke-test evidence from remote reachability.
- Focused tests identify exact source, target/filter, expected and executed
  counts, and retained output. Keep the relevant existing recovery checks green.
- Record the launch command, source revision, deterministic mode, storage path,
  known limits and stop procedure. Leave the demonstration available for the
  human to try; identify its process/session and owner.
- Interview at least the root after delivery, and collect concrete orchestration
  friction from children. Separate product gaps from workflow observations.

The root may resolve ordinary implementation choices. Ask the operator about a
consequential product ambiguity with a recommendation, while continuing useful
independent work. No credentialed inference or real shell-tool exposure is
needed to meet this milestone.
