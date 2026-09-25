# Correction-wave interviews

## root

- **Scaffold change:** The project `AgentSpec` required `Journal`, but coding children do not receive that effect. Both first-wave children failed before starting. I amended the spec to install the baseline watchdog instead. This loses the project-specific nudge ledger for children; I will not invent events.
- **Missing sibling interface:** The engine's seven entry points and demo's two callers need one agreed signature before a leaf owns either side. I will retain the demo consumer wiring once that signature is checked.
- **API versus docs:** The child effect row lacks `Journal` although the project nudge spec assumed it.
- **Integration cost:** The first delegation wave failed at startup; it cost a retry and a source amendment, not feature progress. A preflight child launch would have surfaced this earlier.
- **Different scaffold next time:** Check the actor spec against the child effect row before admitting the wave, and write the single engine signature into the scaffold before splitting loop from consumers.
- **Nudges:** No child nudge fired because both initial children failed at tool installation. The fallback watchdog can run, but its events are not the project nudge ledger.
# Cache-shape probe leaf — 2026-09-24

## Own-words plan and question

My bounded task was to make exactly two consecutive live requests with
Codex-shaped headers, `prompt_cache_key`, and body order, then record redacted
bodies and cache counters in `docs/findings.md`; alternatively, report the
precise missing capture. The plan in `docs/correction-plan.md` and the
operational boundary in `NEXT.md` both say this is findings-only, one node,
and not a source-code or review task.

I found the harness request builder and prior live-call narrative, but no
exact reference capture of Codex's serialized request. Those artifacts do
not establish byte-for-byte parity. I therefore sent no requests rather
than present a reconstructed shape as a live comparison. What exact redacted
Codex wire capture can be supplied (ordered header values and body bytes)?
The precise Blocked evidence is in `docs/cache-probe-evidence.md`; root owns
incorporating it into the shared `docs/findings.md`.

## Prompt trials

- Incorporating an existing sibling/parent change: not applicable; no such
  change was incorporated.
- Intentionally red test: not applicable.
- Two failed check rounds before ping: not reached; this was an evidence gap,
  not repeated check failures.
- Operator notes as advice unless explicitly constraints, and labeling
  hypotheses: held. I treated the two-request probe criteria as the stated
  acceptance, and did not infer exact wire shape from the narrative.

No code or tests were changed or run. The detailed blocker is recorded in
`docs/cache-probe-evidence.md`.

## Correction core lead — 2026-09-24

Root incorporated this node's own-words interview from its retained commit
`6667ddc`. That commit was excluded from the delivered branch because these
documentation paths are root-owned. Its old local findings commit `b765e9b`
was rebased as `1513e95` and merged by root at `06148a1`.

- **Plan and tree cost:** I planned three ordered slices. The (b) Luna
  produced `a1f976f`, and an independent settings-contract reader found
  the provenance and pinning seams. The host notification inbox then
  became fenced, so later child replies and ordinary review waves could
  not be received normally. I inspected the (b) diff inline, merged it
  in `ffe20e5`, and ran the focused offline acceptance test (1 passed)
  and workspace format check. Root incorporated that branch at `d8097c3`.
  The notification failure made this an exceptional manual review.
- **Missing interface:** Generic `Item(Value)`/Store replay preserves JSON
  but does not identify which `configuration_update` was harness-authored.
  The settings slice needs a provenance decision before pinning effort,
  replacing adjacent updates and stripping/re-pinning child histories.
  No SQL migration is inherently required.
- **Next scaffold:** Represent settings provenance at append time, not by
  trusting wire JSON. Define the initial update and child fork prefix
  once; then separate store/engine and runtime/verb changes behind that
  seam.
- **Prompt trials:** I named exact commits for incorporation. No red test
  was committed by my node. There were no two failed check rounds. The
  operator's inbox report was treated as a run constraint, not silently
  generalized to the project. No nudge events were observed or invented.
- **Result:** I returned `Blocked` for (c)/(d), not a feature completion.
  The live item-2/item-13 and unanswered-call experiments were not run.

## Root continuation — 2026-09-24

- **Tree structure cost:** The probe initially changed a shared findings
  file outside its ownership; checking the cumulative branch diff caught
  the earlier commit even though its tip was in scope. Repair and rebase
  produced `cdbbd367`, merged at `b99337e`. The probe still lacked the
  reference capture, so no live call was justified.
- **Stalled lead:** A notification delivery receipt did not wake core's
  fenced inbox. Operator TUI relay supplied the missing checkpoint. Core
  continued long enough to deliver (b) and findings, but could not safely
  start a new child wave; a `Working` request was not evidence of progress.
- **Different scaffold next time:** Make provenance an explicit
  harness-owned property at append time and fix the initial settings pin
  and fork-prefix contract before splitting (c)/(d). Preflight notification
  routing before admitting that next lead.
- **Prompt trials:** Exact incorporation commits were recorded; the
  intentionally red offline test was treated as a contract and turned
  green by (b), not mislabeled passing early. No repeated failing-check
  loop was observed. The inbox fence was an operator constraint for this
  run; no conjecture about its mechanism was promoted to an engine fact.

## Cache-counter probe leaf — correction second half, 2026-09-24

- **Scaffold and plan:** Q4 changed the earlier byte-parity blocker into one
  yes/no measurement. I used the existing production request builder at
  `d0245b3`; no scaffold code changed under me.
- **Sibling interface:** None. The read-only credential and builder were
  present; the temporary runner did not change repository code.
- **API versus docs:** The production builder returned usage for both calls;
  the second reported 20,736 cached input tokens. This measures the stated
  criterion, not a promise of future cache hits.
- **Tree cost:** Root must integrate this one findings-only document and
  incorporate the result into shared findings. A narrower probe than the
  previous byte-parity task avoided an unnecessary missing-capture blocker.
- **Different scaffold next time:** State the measurement and its available
  inputs in the task before admission, as Q4 now does.
- **Nudges and trials:** No sibling change, intentional red test, or repeated
  failed-check escalation occurred. No nudge firing was reported.

## Item-2 live probe leaf — correction second half, 2026-09-24

- **Scaffold change:** None. I read the integrated async-tool schema and demo
  slow `sleep` path at `d0245b3`.
- **Sibling interface:** The existing CLI can drive the scenario, but it does
  not expose the actual redacted request bodies or correlate them with sleep
  progress and `wait_agent` result. Root owns a trace seam before a live run.
- **API versus docs:** The code has the ingredients, not the auditable trace
  required by item 2. I did not infer continuation from a prompt or mock test.
- **Tree cost:** Root must merge this findings-only document and add tracing
  before spending the single manual live run. My initial document needed a
  rebase onto root's intervening cache-probe integration.
- **Different scaffold next time:** Require a redacted trace surface before
  assigning the credentialed acceptance experiment.
- **Nudges and trials:** No intentional red test or repeated failing-check
  round occurred; no nudge event was reported. No inference was spent.

## Item-2 trace-design leaf — correction second half, 2026-09-24

- **Scaffold change:** None. At `8779c47`, root added a compiling `trace.rs`
  boundary; this leaf provided findings only, with no edits, tests or inference.
- **Sibling interface:** A transport wrapper can inspect the production-built
  request body via `request_body`, but cannot observe sleep lifecycle. Root's
  `main.rs` provider hook must emit sleep start/settle into the same sink.
  Correlation requires call IDs and a narrowly redacted `wait_agent` result.
- **API versus docs:** The trace plan's combined wording hid that
  `CallContext.progress` is consumed by `JobScheduler`, not the transport.
- **Tree cost:** Separating request logging from root-owned provider timing
  adds one shared sink contract; root then has to wire and verify both sides
  before the single manual live run.
- **Different scaffold next time:** State the sink signature and the minimal
  redacted request/job fields before parallel trace work.
- **Nudges and trials:** No nudge event, expected-red test or repeated failing
  check was reported.

## Item-2 trace implementer — correction second half, 2026-09-25

- **Scaffold change:** I replaced the compiling `trace.rs` TODO stub from
  `8779c47`. Root advanced the base through prompt/docs and consumer commits;
  I learned of the changing contract through the trace-design finding and
  root messages, then checked each pinned rebase and cumulative diff.
- **Sibling/root seam:** Root needed a clonable `TraceSink`, async
  `open`/`record_job`/`flush`, `JobEvent` sleep start/settle, and a
  `TraceTransport` with enabled and disabled constructors of one concrete
  type. My first proposed `Trace::new`/`record_correlation`/
  `TracedTransport` API was superseded. The plan specified behavior but not
  constructor signatures or the exact `wait_agent` output projection.
- **API versus docs:** The root factory could not select traced and ordinary
  transports with the first constructor shape. Review also showed that
  dropping every output prevented redacted `resumed_by` evidence. Existing
  trace-path truncation needed a later create-new correction.
- **Tree cost:** API churn delayed root consumer wiring, and repeated
  rebases were needed while root advanced. An early shared sink/transport
  contract would have avoided this cross-owner repair loop.
- **Different scaffold next time:** Include production-built request and
  `wait_agent` fixtures, redaction/correlation tests, disabled-mode typing,
  and no-clobber trace-path semantics from the start.
- **Nudges and trials:** Exact-API, ownership, builder and rebase nudges were
  useful; one stale assignment repeated the superseded API. Early compile
  failures in error chaining/lifetimes and a later stale raw-call-ID test
  were corrected and rerun. Final owned candidate `fb80016` had 9 matched,
  9 passed trace tests plus check/format/diff-check; it did not establish
  root consumer behavior or the credentialed live trace.

## Root continuation — correction second half, 2026-09-25

- **Plan and outcome so far:** I recovered previously reviewed settings and
  Compactor candidates rather than rebuilding them. The settings stack
  reached master at `f1334bec`; the Compactor component and production
  consumer reached master by `f504dd0`. Here and the live item-2/item-13
  gates remain open; I do not call the wave complete.
- **Shared seams:** The Compactor component alone could pass while no
  production Engine selected it. I retained Engine/store and demo wiring,
  then added a production-factory replay test. Here exposed a different
  missing seam: stored `agent.head_request` is stale during an active spawn
  call. I added `AgentInvocation { request, call_id }` at `e28126e` for the
  retained Here owner; the actual output-readiness gate remains its work.
- **Tree structure cost:** Stacked old branches carried settings ancestors
  into Compactor review, making a component-only diff hard to distinguish
  from an integration candidate. The exact new Compactor component was
  reviewed and merged instead. The original settings-pin reviewer was
  retired before a visibility defect was discovered, requiring an explicit
  reviewer exception and a full cumulative review. A later Here leaf stayed
  in one turn beyond the 15-minute/30-call constraint, delaying presentation
  of the new root contract despite a separate-worktree branch.
- **Checks versus behavior:** A replay test passed once and failed when the
  same reviewer repeated it; the test assumed a 1 ms job had settled before
  the final response. Repair allowed either three or four requests while
  asserting one Compactor boundary, and it passed 20 repetitions in root
  and review. A broad harness library run still has nine failing stale
  settings-pin assertions; focused green tests did not prove master green.
- **Different scaffold next time:** Put a production caller and an
  active-request identity in the initial compiling scaffold, name the
  pending-output persistence barrier, and use a replay fixture that controls
  job completion rather than relying on elapsed milliseconds. Keep one
  reviewer retained until the slice is integrated and its consumers checked.
- **Prompt trials:** No test was committed intentionally red in this
  continuation. I reported zero-match filters as zero, not as passing
  evidence. I have not spent inference beyond the single item-2 manual
  attempt already recorded; the operator hold on its retry remains.

## Adapter-readiness gate — 2026-09-25

- **Root:** I integrated the Store-backed replay provider, then scaffolded
  one final Engine/Store vertical test and delegated its implementation plus
  a read-only gate audit. The tree kept the test owner separate from the
  contract auditor and exact-candidate reviewer, but the first review
  incorrectly accepted a final-response race. I traced Engine's
  final-with-pending fall-through and sent a bounded repair. One attempted
  recheck stayed on the old HEAD, so no repaired candidate was accepted from
  it; an exact-checkout recheck remains pending. No live inference was run.
- **Vertical test leaf:** Its work stayed in the one owned integration-test
  file. Auth/Arc fixture compilation, JSON-text tool output, and expected
  input ordering each cost a local correction. It reported 1/1 on the first
  candidate, then 1/1 after adding a retained scheduler-settlement barrier
  and a sixth gated finalize response. The tree cost it a separate repair
  request and exact-tip re-review before its file can reach master.
- **Read-only gate audit:** It identified the production Engine/Store
  boundary and cleanup risks without edits or checks. It initially placed
  envelope insertion after `wait_requested(n)` as though request `n`
  could already include it; root corrected the barrier numbering in the
  implementer's task. The separate audit was useful only after the timing
  correction, and cannot certify adapter-host wiring.
- **Exact-candidate reviewer:** It ran the first candidate's focused test
  1/1 and demo check, but misread Engine's final-with-pending branch as a
  return rather than a wait followed by another request. Its first repair
  recheck also reviewed stale HEAD despite being assigned the new commit.
  The tree exposed a source-binding failure: a verdict is not usable unless
  the reviewed HEAD equals the candidate OID.

## RSI iteration 1 / wave 9 — 2026-09-25

- **First-request gate owner (request 1, candidate `6d3b39e`; reported):**
  The bounded leaf was assigned `Delivery = Outcome CheckedDelivery` even
  though root owns independent review and integration. After its reported
  1/1 focused gate, it inspected constructors and sent candidate/progress
  separately rather than settling `Outcome Candidate`. The tree cost one
  avoidable result-routing handshake. It expected a candidate-typed leaf
  reply. Assignment-role/result-stage validation or a typed
  candidate→review→integration flow could let a passing leaf settle while
  preserving exact base/tip; the automatic-review wrapper's generated
  `Project.Routing.WorkEffects` / `State` imports currently block that flow.
  It reports no production repair needed and has paused changes for review.
- **Root:** The typed review base helped: reviewer request 2 preserved
  `aa0c82e` distinctly from exact candidate `6d3b39e`, and its accepted
  verdict was usable without reconstructing a merge base. Event waiting
  routed the owner's candidate and later settlement; the result-type
  mismatch caused an unnecessary relay but no empty polling round. Human
  judgment was still needed to scope manual `advance_agent_head`: the
  file-Store reopen proves persistence after the test's explicit advancement,
  not automatic runner advancement. Root merged at `4230179` and ran the
  integrated first-request and adapter-readiness targets (1/1 each). The
  next product gate remains amendment 4's unseen-follow-up/final-answer
  provenance, separate from the blocked automatic-review wrapper capability.
- **Exact-source reviewer (request 2; reported in interview request 3):**
  Reconstructing a `Task` from `CommitReview` to return acceptance required
  a manual constructor step for `6d3b39e`; it expected the typed review
  helper to preserve base and candidate in the accepted result. A typed
  exact-source review continuation could route candidate delivery without
  manual packet relay. The generated automatic-review wrapper failing to
  compile is a missing capability; an established wave-9 authored-flow
  example would help usage but would not fix that wrapper.

## RSI iteration 2 / wave 10 — 2026-09-25 (in progress)

- **Store/service owner:** A zero-match `cargo test` filter for the service
  test cost a manual name lookup and rerun; it then reported exact
  ancestry/reopen, service, first-request and adapter-readiness tests
  matched/passed 1/1 each. A runner could reject filters matching zero
  tests. The shared `CompletionProvenance` and optional `Contract.reply`
  on its base made compilation straightforward; the semantic care was
  keeping ancestral delivery as *seen*, not incorporated. Its tree
  cost an API relay and correction acknowledgement through root to
  Driver, not a schema migration. It did not claim the Driver gate.
  Root integrated its candidate `5b2741a` at `176a271`.
  On the later atomic-publication slice `5b7272b`, it found the
  hardest seam was coupling head CAS and durable parent envelope
  within one SQLite transaction, not two calls. It repeated a
  compile after strengthening an assertion; a matched-count-aware
  focused runner could rerun only affected cases. The tree cost an
  API checkpoint and another candidate-ready relay through root
  before Driver could exercise the integrated transaction. Its
  two focused Store tests reportedly matched/passed 2/2; this
  slice remains under independent review.
- **Engine/finalize owner:** Manual tracing of finalize parser and wire
  schema and repeatedly relaying the path and focused evidence across
  rebased tips cost work; a trace from public entry to terminal check,
  exact checked-OID validation, and matched-count checking could be
  automated. The
  hardest contract change was exposing a dynamic strict JSON result
  while preserving untyped `Engine::run` and avoiding a second answer
  channel. A reviewer found parser-consumption and unchecked-pattern
  bugs in `2d12a4a`, requiring one repair candidate and re-review.
  The tree cost an additional checked revision, but caught behavior
  the first focused tests did not. A further rebase to `669ba8b`
  required fresh checks but has identical Engine/finalize content to
  the repaired `2d5218f`, which was accepted and integrated at
  `c445c5d`. The owner did not claim the Driver gate.
- **Store exact-source reviewer:** It found no redundant Store
  scheduler/inbox path in `5b2741a`. It reported no demonstrably
  needless code operation; an automated pre-review step could pin
  exact HEAD, cumulative ownership, and nonzero matched tests.
  The hardest seam was treating ancestral `delivered_request` as
  *presentation*, not acknowledgement/incorporation. The tree cost
  an extra checkout/test evidence handoff; compiler chatter obscured
  the second filter's matched count. It counted only
  `completion_provenance_uses_ancestry_and_reopen` (1/1), not the
  service test whose exit was 0 but match count unavailable.
- **Driver owner (request 3 checkpoint):** Repeated manual source/status
  relays and stale Store notices cost time; routing an accepted
  dependency commit/API to consumers and validating wire shape before
  adapter-readiness could be coded. The earlier `typed_result` field
  requirement was superseded by strict turn parsing, forcing a driver
  correction commit. The tree cost a period where two written
  lifecycle tests could not compile against root until Engine review
  and integration; the owner did not claim either test ran then.
  Its second dependency was a real root contract gap: an extra
  `structured` field would be forwarded to Responses, and strict
  agent tools omitted `reply`. Root amended both at `41c48b6` and
  `e47a958`. Final driver gate evidence remains pending.
  In its later atomic-repair reply it named a second shared
  contract/review/integration cycle as the tree cost. It proposed a
  checked dependency-commit wake followed by the focused gate,
  rather than repeated manual source/status relays. Replacing
  separate head/envelope calls with one transaction while keeping
  Store and Driver ownership disjoint was the hardest correction.
- **Exact-source reviewer:** Switching from its initial `2d12a4a`
  checkout to assigned `2d5218f`, then re-running focused checks,
  cost needless work. A repeatable schema audit could require every
  normalized constraint to be locally enforced or rejected, and
  require failed dynamic parsing to leave `FinalizeParser` reusable.
  The hardest seam was strict dynamic completion without a second
  answer channel: the result stays in `completion.turn` and old
  `run`/`run_finalized` behavior remains. The tree cost an additional
  activation/evidence relay; the reviewer also reported a later
  mismatch between an operator request-state notice and its workbench
  state, so it could not resubmit a result that was already settled.
  Artifact facts: Repair at `2d12a4a`, Accepted at `2d5218f`;
  the first review completed no counted tests, the re-review reported
  five focused filters matching/passing 1/1 each. It did not claim
  broader integration or live gates.
- **Driver exact-source reviewer (first verdict):** It reported test
  polling loops as needless work and suggested codifying
  candidate/base/check accounting. The hardest seam was plumbing
  strict typed completion and provenance through Engine, Store and
  parent publication. The tree cost came from reasoning across the
  Driver's async task, watch, notifier and reaper cleanup paths.
  Its exact `ddce261` review returned Repair: head CAS preceded
  answer-envelope insertion, so an insertion failure could strand
  the parent answer. It also reported a zero-match
  adapter-readiness filter, not an executed adapter check.
  On exact re-review `cb2620d` it found no redundant
  scheduling/publication path. It again named test waits and
  check accounting as codifiable, atomic head+answer commit as
  the hard contract change, and task/watcher/notifier/reaper
  cleanup as the cost of the tree. The corrected tests matched:
  lifecycle 2/2, malformed-publication 1/1, shutdown 2/2,
  reaper 1/1, first-request 1/1, adapter-readiness 1/1.
  This is review evidence, not a live-provider claim.
- **Root:** `ReviewBasis::ExactScope` kept base/candidate/owned paths
  explicit across the Store and Engine reviews, including the
  Engine rebase over the integrated Store slice. It avoided
  fabricating a `Task`, but did not prevent a stale candidate
  submission (`5e8d58e`) or initial stale reviewer checkout:
  exact-tip checks were still essential. The next capability
  exposed is source-aware dependency routing to a retained consumer,
  plus a typed, wire-safe publication codec checked at the API
  boundary. Root held the driver gate as unverified rather than
  equating a written test with an executed one. In the final
  integration, the first Driver review caught a real lost-answer
  path that its 2/2 lifecycle test did not cover. The tree cost a
  second Store contract/amendment, Store review/correction, Driver
  rebase and Driver re-review; the precise repair was a single
  Store transaction for head CAS and answer insertion. At
  `a1c8cd9`, root ran the combined focused boundaries (13 matched,
  13 passed), not a broad workspace or live test.

## Post-wave RSI interview — Inanna and supervisor, 2026-09-25

Wave 10 is closed. These are my judgments about the recorded run, not
new test results.

### Three largest avoidable costs and one change

1. **The atomic completion seam arrived a review cycle late.**
   `ddce261` passed both lifecycle cases, but its reviewer found that
   Driver advanced the child head before inserting the parent answer.
   An insert failure could lose the answer permanently. Root then
   scaffolded `CompletionCommit`, sent Store and Driver through another
   implementation/review/integration cycle, and added an injected
   failure check. This was the largest avoidable *product* risk and
   rework, not just orchestration overhead.
2. **The producer/consumer contract changed after forks.** The
   `typed_result` field was requested then superseded by parsing the
   strict `completion.turn`; Driver made a correction commit. Its
   proposed top-level `structured` field would have reached Responses
   unchanged, while the strict model-facing agent verb schema omitted
   `reply`; root landed separate codec and schema amendments
   (`41c48b6`, `e47a958`). These were observed interface gaps.
3. **Source and check evidence needed repeated manual reconciliation.**
   An Engine reply named `5e8d58e` after its branch advanced, reviewers
   initially saw older checkouts, Driver repeatedly reported older root
   heads, and zero-match filters plus lost command-job observation
   obscured whether tests ran. The exact-tip checks avoided incorrect
   integration, but spent reviewer/root turns. This cost was smaller
   than the two product seams above.

**One highest-leverage change:** before forking, root should land a
*compiling vertical boundary contract* through the real service,
Store, Engine and Driver: one strict tool argument crossing the
production schema, a typed publication projected to wire-safe input,
and one transaction-level failure case that aborts envelope insertion
after the proposed head CAS. It should name the exact source OID,
consumer path and matched acceptance command in a short canonical
brief. A brief alone would not catch the atomic bug; the executable
failure boundary is the essential part. This is a recommendation,
not a claim that every later race would disappear.

### Assessment of the three proposals

1. **Typed review submission alongside Haskell:** Useful *if* it binds
   current `ReviewRequest`/`ReviewBasis`, exact candidate HEAD and
   ownership diff, exposes a terminal submission receipt, and rejects
   a verdict on another tip. It might have shortened stale-checkout
   and OID reconciliation. Merely adding a second `respond` button
   duplicates the typed `respond`, `ReviewRequest`/`ReviewBasis`,
   `reviewCommit`, `pollResponse`, and notebook exploration/pagination
   that already worked. It would not have found the head/publication
   bug or wire-schema gap. I would not replace the notebook with it
   or prioritize a wrapper that still cannot compile.
2. **One canonical API brief plus compiled producer/consumer example:**
   Highest product value, provided it is a single source-pinned
   *executable* contract, not another prose copy of `NEXT.md`.
   An example crossing actual `spawn_agent`/`followup_task` strict
   arguments, `Contract.reply`, Store publication, Driver and
   Responses request projection could have exposed the
   `typed_result` disagreement, extra `structured` field and missing
   strict-schema `reply` before the owners split. Including the
   abort-insert case could also have exposed the atomicity requirement
   cheaply. A happy-path example alone would have repeated the
   `ddce261` blind spot.
3. **Exact package/target/filter and expected matched counts:** Worth
   adding to existing gate packets or runner. The Store owner's
   `service_validates` filter exited 0 with zero matches; Engine's
   `--exact` and the Driver reviewer's first adapter command had
   similar evidence problems. A nonzero-match guard would have
   prevented those false starts, not the product repairs. Some wave
   packets already named commands, and root ultimately recorded
   counts; the missing piece is execution-time enforcement, not a
   new test scheduler or broad battery.

**Priority order:** executable vertical contract/failure seam first;
source-bound candidate/review submission with unambiguous terminal
state second; a small zero-match guard in the existing command path
third. The last two can be incremental and should not displace the
first.

### Reviewer reply confusion and scaffold miss

**Observed:** the Engine reviewer reported submitting its `2d5218f`
Accepted reply, then seeing an operator notice that request 8 was
still open while its workbench showed `current_request=None` and
`respond` unavailable. Root's `pollResponse` showed Ready and used
that verdict; there is no evidence here that the accepted reply
failed. The review prompt also says to retry unchanged after a
genuine `ReplyUpdatePending` rejection, but the recorded interview
does not establish that such a rejection caused these later attempts.
**Inference:** a stale/asynchronous request-state notice, combined
with a “Reply submitted” receipt that did not make terminal state
obvious to the reviewer, best explains the repeated attempts.
The fix is to distinguish submitted, accepted/settled and
unavailable authoritatively and suppress stale “open” steering
after settlement; a second submission surface alone does not fix
conflicting notices. I cannot reconstruct every attempted call from
the interview.

I initially scaffolded types for provenance and publication, but not
the persistence barrier between them. I treated Store's existing
transactional `add_envelope` and Driver's existing head CAS as
separately sufficient, and the two explicit-barrier lifecycle tests
exercised successful delivery, not an envelope-insert failure after
CAS. The first exact Driver review, not the gate, found the gap.
A cheap pre-fork contract would have named “no child head without its
parent answer” and compiled a Store transaction stub or red
trigger-injected rollback test. It needed no live call, timing guess
or additional scheduler.

### Next milestone and experiment

**Proposed next harness milestone (not authorized by this interview):**
offline crash/restart recovery of the completed lifecycle, especially
the boundary after the atomic head+answer commit but before its wake
hint. A new Driver instance over the file Store should discover the
durable parent answer and pending child follow-up once, preserving
strict reply schema and provenance. Wave 10 proved *clean reopen*,
not process-crash recovery. Keep message acknowledgement/
incorporation states, contract versioning and live adapter inference
as distinct later scopes; the item-2 retry, item-13 trace and live
adapter holds remain.

**Wave experiment:** root first lands one source-pinned compiling
service→Store→Engine→Driver fixture and an injected failure/restart
barrier as the shared contract, then admits disjoint Store recovery
and Driver restart owners plus independent exact-source review.
Specify package/target/filter and expected matches at admission;
replay the real strict tool/wire representations, kill or restart at
an explicit barrier rather than sleeping, integrate coherent reviewed
slices, and run the combined focused gate once on the resulting
source. Measure avoided contract amendments, candidate/source
mismatches, zero-match commands and actual product defects, not
elapsed time alone. No credentialed inference is part of this
proposal.

The review prompt's `let repairLabel = "repair-candidate" :: Label`
is a stale example for the current label API; a compile-checked
`[label|repair-candidate|]` (or `labelFromText` for dynamic input)
should replace it next iteration. I saw no evidence it caused wave-10
rework, and I did not edit the prompt. Notebook indentation
auto-repair is deferred as requested; the supervisor reports a
valid multiline placement fix already implemented.
