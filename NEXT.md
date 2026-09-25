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
`Store::list_agents()` plus `Store::unread(path)` is already a non-consuming
typed pending-work query. This is the sole current contract; no second
journal/scheduler or duplicate query is authorized without a missing datum.
An active head with no unread inbox is ambiguous (settled/quiescent versus
interrupted), not definitely Interrupted. Do not replay it; explicitly
classify unsupported resumption when attempted. A committed head/answer
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
| Durable discovery and atomic publication | durable lead | reviewed local `f3551a0948e0c145cc4c997dd441ee3255f2f360`, `recovery_` 3/3, completion 2/2 | root merge and any separate production candidate |
| Request-boundary replay/provenance | Engine lead | pending scaffold checks | reviewed Delivery |
| Restart admission/shutdown | Driver lead | pending scaffold checks | reviewed Delivery |
| Abrupt process-loss Store commit/wake gate | root | `test:process_recovery --filter process_loss_`: 2 expected/matched/executed/passed; child process killed at explicit stdout barrier | combined Driver restart gate still open |

The root's process test kills a separate helper before completion commit and
after Store commit but before any wake, then opens the file in the parent
process. It checks head/answer atomicity, typed serialization, provenance and
CAS idempotence. It does **not** exercise Driver restart or prove power-loss
durability. Durable lead's admission checkpoint: production child owns
`store/mod.rs` and `agent_runtime.rs`; test child owns
`store/recovery_tests.rs`; both start at scaffold `55d2cce...`, with first
reply expected as Outcome Candidate and focused counts.
Driver lead's admission checkpoint: production child owns `driver.rs`, test
child owns `driver/recovery_tests.rs`, both from `55d2cce...`; production
first reply targets `driver_restart`, test reply `recovery_`, with counts and
expected-red status. `main.rs` remains with the Driver lead.
Engine lead's admission checkpoint: production child owns `engine.rs`, test
child owns `engine/recovery_tests.rs`, both from `55d2cce...`; baseline
`dynamic_reply_schema` matched/passed 2/2, new boundary count pending.
Driver inspection reports no production edit yet. `driver_restart` was a
prospective filter and selected 0 matched/0 runnable/0 ignored tests; it is
not a passing check. Existing `restart_with_existing_root_head_does_not_duplicate_prompt`
and the required `followup_lifecycle` baseline remain distinct from new
recovery tests. Driver lead will report executed counts for the latter.
Durable recovery-test correction: use existing Store APIs only; test committed
answer reopen and pending-versus-interrupted claim discovery. Root source
inspection confirms `recover_pending` selects only pending claims and `claims`
maps Interrupted, while CAS HeadMismatch precedes envelope insertion.
Forwarding reached both child owners, but their actual candidate OIDs and
checks are required before incorporation is claimed.
Durable production child returned Blocked/no-change because the caller for
`complete_agent_with_publication` is outside `crates/harness/src`; root resolved
this: `crates/harness-demo/src/driver.rs::start_agent` is the production caller.
Durable acceptance is Store/service persistence and query evidence using
existing APIs; Driver lead owns startup scan/admission. Durable test candidate
`cb87de6` changes only `store/recovery_tests.rs`, `recovery_` 3/3 and
completion baseline 2/2 reported; exact-source review initially returned
Repair solely because its first test invocation exited 137. Lead reran the
same detached candidate with `recovery_` 3 matched/executed/passed and
completion 2 matched/executed/passed, then resubmitted to the same reviewer.
Decision remains pending. Do not
count this report as integrated or the Driver gate as closed.
Seam correction at `f7b60c5db1dab47ef603b72c8785e2d03ca175e5`
supersedes the proposed new non-consuming Store query: `unread(path)` already
provides it. Durable and Driver leads received the correction; receipts are
transport only. Incorporation requires their candidate OIDs and focused
checks. A head with no unread does not prove interruption.
Driver recovery-test owner is using only minimal fixtures local to its
`recovery_tests.rs` (existing helpers live inside sibling `driver.rs::tests`).
No duplicate shutdown tests under new names; novel recovered-admission join
checks may reuse the existing shutdown contract. Durable file Store and
explicit barriers are required where restart is claimed.
Engine reported gap: a persisted Pending non-`wait_agent` claim with a fresh
empty `JobScheduler` currently reaches `JobError::UnknownCall`. Existing
`Store::interrupt_claim` plus typed `JobOutput::Interrupted` permits a
fail-closed classification and replay of that interrupted output, not
resumption of remote execution. This supersedes the earlier blanket Driver
failure-channel expectation for UnknownCall. Do not turn other errors or
forked `wait_agent` into successful replay; only the inherited pending claim
may transition, with its affected-row result checked. Engine candidate OID
and checks remain pending.
Engine production candidate `f3b1a8005e8bca5ef2d6779c4a4e0c599694c718`
received exact-source ReviewDecision Repair. Reviewer confirmed root's
finding: UnknownCall path ignores `interrupt_claim` affected-row count
and can synthesize Interrupted over a concurrent Settled claim. Engine
lead was asked to route same-file repair to retained production implementer:
on zero rows reread, replay Settled stored output or synthesize only
Interrupted, fail closed otherwise; same reviewer must verify repaired tip.
Separate boundary-test candidate remains unaccepted.
Engine boundary-test candidate `19e3cd375a9189b2976fc149a97fb34c3939c2c0`
stopped Blocked after `boundary_replay` 1 matched/1 executed/0 passed in
two rounds. Its assertion that the new completion head owns one claim
contradicts the original-head ownership invariant; it also pre-marks
Interrupted, so misses the intended Pending+empty-scheduler barrier. Root
asked Engine lead to repair the owned test independent of pending production
review, then run it against reviewed production.

Root integration checkpoint: exact-scope reviewer accepted durable test
candidate `cb87de6`; durable lead merged it as
`f3551a0948e0c145cc4c997dd441ee3255f2f360`, with cumulative owned
diff only `store/recovery_tests.rs` and local `recovery_` 3 matched/executed/
passed, `complete_agent_with_publication_` 2 matched/executed/passed.
Root verified cumulative scope and conflict-free merge-tree; root merge and
post-merge check follow. The separate `9cc0866` pending-query production
candidate is **not** part of this slice and remains under review.
Driver production child's first reply was unchanged baseline `55d2cce...`,
therefore findings only, not a candidate to integrate. `driver_restart`
selected 0 tests; exact existing restart selector matched/executed/passed
1/1/1. Lost wake, duplicate publication and recovered shutdown remain
unverified. Driver lead is retaining the owner for a test-driven repair.
Driver test child also prematurely settled Blocked before the fixture answer
arrived. Lead supplied the decision and reassigned the retained child at the
same source and ownership; next acceptance is novel file-backed pending-inbox
startup recovery with explicit barriers and focused `recovery_` counts.
Its partial commit `3f1548195d383fbf3cb3f08ed5dddf5e73034642`
is **not accepted**: `recovery_` 1 matched/1 executed/0 passed from an
invalid sender; sender correction was not rerun and a compile failure was
also observed. Driver lead retained the test owner for one bounded
repair/rebase to `f7b60c5...` and both `recovery_` and
`followup_lifecycle` checks. No Driver slice has integrated.
Repaired test candidate `a1704d325b2e00319edd26f28247fc6b210ab029`
passed `recovery_` 1 matched/executed/passed at base `55d2cce...`; it did
not contain the optional `f7b60c5...` ancestry and did not run
`followup_lifecycle`. Lead sent the same owner for rebase and both checks;
the candidate remains unreviewed/unintegrated.
Driver lead acknowledged the existing `list_agents()+unread` seam at assigned
`55d2cce...` and no-replay ambiguity. It requested test-child rebase onto
root `f7b60c5...`; root clarified that the root commit changes only disjoint
NEXT/process-test paths, so a rebase is optional if cumulative ownership,
merge-tree, exact-tip review and post-merge checks prove integration. Actual
child OID and checks remain pending.

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
