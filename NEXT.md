# NEXT: wave 11 / RSI iteration 3 — offline restart recovery

Read docs/rsi-iteration-3.md and deliver its offline restart-recovery outcome.
The operator requests a Sol root with multiple Luna subtrees, with meaningful
nested delegation and useful parallel work. At least two Luna component owners
must each delegate at least two bounded Luna obligations; target three component
owners where source ownership supports it. Use Luna Medium, including retained
exact-source reviewers. Root owns shared contracts and final integration.

This assignment controls model choice, topology and result types over historical
examples in docs/tree.md and role prompts. Component owners return Delivery after
local checked integration; their implementers return Outcome Candidate. Use
ordinary event-driven review with ReviewRequest/ReviewBasis. No initial planner
release is outstanding. The typed review submission tool is not required for this
wave; Haskell remains available. Do not instantiate the broken automatic-review
wrapper or turn this wave into its compiler repair.

## First checkpoint

Launch baseline: `3788fd25ff7b081ac123b66aadf1dab25c0f021a`.
Root scaffold: existing `Store::complete_agent_with_publication` in
`crates/harness/src/store/mod.rs` atomically advances the child head and inserts
the typed parent answer; `CompletionCommit` in `lifecycle.rs` reports CAS loss or
committed envelope ID. `Driver::start` subscribes before durable `scan`, and
`scan` admits active agents with unread inbox; Engine claims unread envelopes
when creating a request. Wake channels are hints, never persisted evidence.
This is the sole current contract; no second journal/scheduler is authorized.
An active head with no unread inbox is unsupported interrupted execution and
must be classified explicitly, not silently replayed. A committed head/answer
followed by lost wake must be discovered by Store-backed restart scanning.

Owners after scaffold: durable lead owns `store/mod.rs`,
`agent_runtime.rs`, `store/recovery_tests.rs`; Engine lead owns `engine.rs`,
`engine/recovery_tests.rs`; Driver lead owns `driver.rs`,
`driver/recovery_tests.rs`, `main.rs`. Root owns `lifecycle.rs`, all module
declarations, `NEXT.md`, and a separate cross-component process-crash gate.
Leads must split production and test files between at least two Luna children.
All briefs cite PRD.md § `store (sqlite, one file, one process)` and
docs/rsi-iteration-3.md § `Acceptance matrix`.

Initial boundary check: `scripts/cargo-focused-test --package harness --target
lib --filter complete_agent_with_publication_` expects 2 matched and passed;
it exercises rollback on envelope insertion failure. `scripts/cargo-focused-test
--package harness-demo --target bin:harness-demo --filter followup_lifecycle`
expects 2 matched and passed; it exercises the wire-safe typed answer and
seen/unseen reference through Store reopen (not process crash).
The process-crash gate must use a child process and explicit barriers, not
task abort or graceful shutdown. Publication count and durable IDs are checked
after restart; no exactly-once external execution promise follows.

Admission checkpoint expected from each Luna lead: nested children, exact
scaffold base OID, disjoint owned paths, first expected reply and focused
target/filter/count. Report local integration OID and incorporation checks.

| Obligation | Owner | Current evidence | Gate |
| --- | --- | --- | --- |
| Durable discovery and atomic publication | durable lead | pending scaffold checks | reviewed Delivery |
| Request-boundary replay/provenance | Engine lead | pending scaffold checks | reviewed Delivery |
| Restart admission/shutdown | Driver lead | pending scaffold checks | reviewed Delivery |
| Abrupt process-loss cross-boundary gate | root | not implemented | executed process test |

Candidate domains: durable recovery, Engine request-boundary recovery, and Driver
restart/admission. Root finalizes exact ownership and retains the cross-component
process-crash gate. Separate test modules make implementation/test children useful
without concurrent edits to the same source file. Do not add public APIs solely
for test access. Retained children receive exact dependency OIDs and report their
incorporated candidate and actual checks.

## Gate and constraints

Prove process-loss/restart around atomic head+answer commit and the later wake
hint; prove durable pending follow-up discovery without duplicate publication or
concurrent runs. Preserve typed answers, seen/unseen provenance and ancestry.
Name unsupported or interrupted states explicitly; do not infer exactly-once
remote provider or arbitrary tool execution from Store transactions. Process
crash tests do not establish power-loss durability. Use explicit barriers, not
sleeps, and distinguish abrupt loss from clean reopen.

Use scripts/cargo-focused-test (README.md § Focused tests) for ordinary libtest
targets, with expected and executed counts. Root runs combined focused boundaries
on integrated source; no broad workspace battery by default. Keep wave-10 behavior
and adapter readiness green. Read PRD.md and relevant current consumers.

- No live adapter inference, credentialed item-2 retry or item-13 trace; holds remain.
- No second scheduler, mailbox or durable journal; no adapter implementation or
  contract-versioning expansion. Offline replay and Exomonad coding actors are allowed.
- Never git stash/reset/checkout -- path; commit by pathspec, no attribution trailers.
- One compiler worker; do not restart the shared daemon.
- Wave 10 integrated code is a1c8cd9; handoff/interview are 76eba2b and 6005c7f.
  Its 13 focused tests passed. The full historical checkpoint trail is in
  docs/wave10-handoff.md. No accepted wave-10 work remains unmerged.
- Workspace pin 5864ae1 carries the revised contract/fork practices; the harness
  review prompt has the corrected Label example. Resolve full OIDs from Git.

## Observe and finish

Keep a short obligation table here after the scaffold. Record source/check
incorporation, actual tree shape and overlap, subtree coordination costs, rejected
cells, stale reminder/reply retries and defects caught by the scaffold versus
review. Interview root, component owners and reviewers. At completion record exact
integrated source, decisive checks, remaining limits and any unmerged work; retire
all descendants deliberately and stop.
