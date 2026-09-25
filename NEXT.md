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
This **does not** apply to a distinct inherited Pending non-`wait_agent`
claim whose fresh in-memory `JobScheduler` returns `UnknownCall`: the
accepted Engine contract is to atomically mark only that durable claim
Interrupted and replay its existing typed `JobOutput::Interrupted`, with
zero-row reread preserving any concurrently Settled output. It makes no
exactly-once external execution promise. Forked `wait_agent` and other
unproven states stay fail-closed.

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
| Durable discovery and atomic publication | durable lead | Delivery settled, root `3ae99f6f45507537f04a48a2489fdb091b3c829f`, `recovery_` 3/3, completion 2/2 | complete for Store scope; Driver/Engine separate |
| Request-boundary replay/provenance | Engine lead | exact root review Accepted coherent `6988d5192065876e178e7fa97a3b8205dd21ea94`, engine recovery 2/2, schema 2/2 | root merge/post-merge combined gate; operator policy answer pending |
| Restart admission/shutdown | Driver lead | Delivery settled; root `d23d0fc313b7a171d645628dc670e4a51bc706f9`, recovery 2/2, follow-up 2/2, no-replay 1/1, process restart 1/1 | Engine seam and final combined gate |
| Abrupt process-loss Store commit/wake gate | root | `test:process_recovery --filter process_loss_`: 2 expected/matched/executed/passed; child process killed at explicit stdout barrier | integrated |
| Restarted real Driver after lost wake | root | `bin:harness-demo --filter process_restart_driver`: 1 expected/matched/executed/passed; killed helper then two Driver startups over file Store | final combined integrated-source gate |

The root's process test kills a separate helper before completion commit and
after Store commit but before any wake, then opens the file in the parent
process. It checks head/answer atomicity, typed serialization, provenance and
CAS idempotence. It does **not** exercise Driver restart or prove power-loss
durability. Durable lead's admission checkpoint: production child owns
`store/mod.rs` and `agent_runtime.rs`; test child owns
`store/recovery_tests.rs`; both start at scaffold `55d2cce...`, with first
reply expected as Outcome Candidate and focused counts.
Root added a separate cross-component process-loss gate in
`harness-demo/src/process_restart_tests.rs`, wired by a root-owned `mod`
declaration in `main.rs`: helper process commits the typed child answer,
signals after Store commit before any wake, and is killed; a new Driver over
the same file discovers it. Two sequential Driver startups see one answer,
do not re-admit the child with head/no unread, and join shutdown. This does
not run a real remote provider/Engine claim or prove power-loss durability.
Exact-source Luna reviewer accepted root gate at
`203d92726220863fbff5b760b5bf8ee1d3ae66c0`, verifying `process_restart_driver`
1 expected/matched/runnable/executed/passed, cumulative owned diff and
Driver scan/reap/shutdown paths. Reviewer explicitly limits the claim:
the helper process is killed after committing Store data and a fresh
Driver opens it; this does not kill an already-running Driver or execute
a real remote provider/Engine job. The reviewer noted an unrelated
unused-import warning in the still-stub Driver recovery test module.
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
Engine lead's local `integrate(engine-production)` at
`9caecf153412df77aecb6947634cc77a58874662` contains the repair:
UnknownCall now checks `interrupt_claim` affected rows, rereads a zero-row
claim, returns the actual Settled output or Interrupted only for that
state, and fails closed otherwise. Local `dynamic_reply_schema` 2 matched/
executed/passed, fmt and diff-check passed. Same exact-source reviewer is
checking the repaired tip; direct `engine_recovery_` filter was 0 matched
before the test child was reassigned to add two owned cases. This is not
yet a reviewed/integrated root slice.
The same reviewer returned Repair again on `9caecf1`: design accepted,
but `dynamic_reply_schema` alone did not execute stale-Pending/Settled-output
or genuine Interrupted recovery paths. Engine test owner is adding two
cases in its separate file from `9caecf1`; `engine_recovery_` target is
2 matched/executed/passed on the combined tip, then same-reviewer
reviewAgain. No production-file repair is requested in this round.
**New Engine contradiction to resolve before root merge:** lead branch
`70412aebf64d139a688c623aff1e42b17c502379` replaces the agreed
UnknownCall→Interrupted output with `UnsupportedInFlightJob`, preserving
Pending and returning an error. Its cumulative recovery_tests.rs still
calls removed `recover_missing_job` and expects typed Interrupted, so the
branch appears not to compile, let alone pass `engine_recovery_`. Root
fenced the live Engine request with the explicit distinction between
ambiguous agent head and proven missing in-memory job; update is
`UpdateUnconfirmed`, not yet incorporated. Root requested exact rationale,
reviewer decision and executed result; this
cannot be published as Delivery until shared semantics and tests agree.
Root independently reviewed coherent Engine integration candidate
`6988d5192065876e178e7fa97a3b8205dd21ea94` at the exact HEAD from
baseline `55d2cce...`, cumulative owned diff only `engine.rs` and
`engine/recovery_tests.rs`. Luna reviewer Accepted after executing
`engine_recovery_` 2 matched/executed/passed and `dynamic_reply_schema`
2/2, confirming request-bound interrupt, settled-output reconciliation,
no duplicate claim, ancestry and typed final. Root merge and post-merge
checks follow. The later `70412ae`/`65bace1` Unsupported branch is not
part of this accepted candidate and remains unmerged unless the operator
changes the policy.

Root integration checkpoint: exact-scope reviewer accepted durable test
candidate `cb87de6`; durable lead merged it as
`f3551a0948e0c145cc4c997dd441ee3255f2f360`, with cumulative owned
diff only `store/recovery_tests.rs` and local `recovery_` 3 matched/executed/
passed, `complete_agent_with_publication_` 2 matched/executed/passed.
Root verified cumulative scope and conflict-free merge-tree; root merge and
post-merge check follow. The separate `9cc0866` pending-query production
candidate is **not** part of this slice and remains under review.
Root merged durable test slice as
`3ae99f6f45507537f04a48a2489fdb091b3c829f`. On this integrated
source, `recovery_` 3 expected/matched/executed/passed,
`complete_agent_with_publication_` 2/2/2/2 and process `process_loss_`
2/2/2/2. `9cc0866` adds a second public pending query whose production
consumer already uses `unread`; root rejected it from this wave absent a
specific missing datum. It remains an unmerged branch, not delivered work.
Critical stale-callsite correction: `git grep` at root
`3ae99f6f45507537f04a48a2489fdb091b3c829f` proves
`crates/harness-demo/src/driver.rs:371` calls
`Store::complete_agent_with_publication` in production after typed-answer
preparation, then handles `CompletionCommit` and emits a later wake.
The repeated assertion of "no production caller" searched the wrong crate
scope; **do not** add Engine call-site wiring. An update to durable lead
request 10 is queued but `UpdateUnconfirmed`, not incorporated.
Durable lead later proposed accepting unmerged `9cc0866` as a hydrated
pending-envelope snapshot from a test-owner contract. Root's current
consumer inspection finds no production callsite for that body before
admission: Driver::scan and scan_inbox read `unread(path)` metadata;
Engine::append_unread_envelopes hydrates and consumes transactionally.
Root has withheld integration pending a named failing production barrier
and callsite; Driver lead was asked whether one exists. The INNER JOIN
silent-omission defect in the proposed query warrants repair if pursued,
but is not product approval for a redundant API.
Durable lead accepted root resolution and stopped the unneeded production
owner; `stopAgent` returned `StoppedReleasing`, not terminal cleanup yet.
Rejected branch
`exomonad/wave11/component-owners/wave11-durable-lead/durable-recovery-wave1/branches/durable-store-production`
is at `d6b9ab8c912fea8c16c5204c267f278307227857` (prior `9cc0866...`);
it implements the unneeded hydrated query plus missing-item repair, unmerged.
The durable Delivery will name only reviewed test slice `f3551a0`/root
`3ae99f6` and leave Driver admission separate.
Durable Delivery has now settled `Produced (Delivered ... f3551a0 ...)`
with exact review Accepted `cb87de6`, isolated-target checks recovery 3/3,
completion 2/2, follow-up lifecycle 2/2 and source correction naming
Driver::start_agent. Root had already merged and post-checked this slice;
root stopped the settled durable lead with `StoppedNow`. The original
wave-1 cleanup plan included still-running Engine/Driver actors because
all three leads shared one unfold group, so root did not execute group
cleanup prematurely.
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
Driver test child's `cargo fmt -- crates/harness-demo/src/driver/recovery_tests.rs`
also reformatted root-owned `crates/harness/tests/process_recovery.rs` in
the child's isolated working tree, unstaged and not in candidate. Root's
integrated checkout has that file clean; no root restore is needed. Record
as formatting-scope friction, not candidate content.
Root exact integration-tip reviewer Accepted
`5a5329ad741e4e38ee4fc518198053d2cba968b7` from base
`3ae99f6f45507537f04a48a2489fdb091b3c829f`, owned diff only
`driver/recovery_tests.rs`; reviewer executed `recovery_` 2/2,
`followup_lifecycle` 2/2 and existing root-head no-replay selector 1/1.
This is clean file-Store reopen, not process crash. Reviewer first
observed an unrelated command handle, then corrected to the actual
retained test job. Root merged the reviewed Driver slice as
`d23d0fc313b7a171d645628dc670e4a51bc706f9`; post-merge focused
`recovery_` 2 matched/executed/passed, `followup_lifecycle` 2/2,
`restart_with_existing_root_head_does_not_duplicate_prompt` 1/1 and
`process_restart_driver` 1/1, plus diff-check.
Driver Delivery later settled with local `24b61a0ee615499852c739fb9221e717736e9fde`
and focused `recovery_` 2/2, `followup_lifecycle` 2/2, `shutdown_` 2/2,
`process_restart_driver` 1/1, Store process-loss 2/2. Root already
integrated equivalent code via reviewed `5a5329a` into `d23d0fc`; `git
diff` between those tips shows only documentation differences, not
unincorporated code. Root requested Driver lead stop; outcome
`StoppedReleasing`, so cleanup finality awaits notice.

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
