# Correction-wave interviews

## Wave 13 root — 2026-09-25 (delivery still open)

- **Scaffold change:** Root committed `docs/wave13-contract.md` at
  `b43cdb1`. Root corrected its invented `GET /api/snapshot` readiness
  route at `a2ccbde` after reading `server.rs`: the actual boundary is
  `GET /api/session` followed by an authenticated `/api/ws` initial
  snapshot. The launch owner reported that correction did not affect
  its candidate because it had no readiness-route check.
- **Sibling gap and tree cost:** Three disjoint Luna obligations were
  admitted together, but host cancellation killed the test and docs
  children and their replacements before they produced candidates.
  Root took those paths over. The split named the right seams, but the
  cancellations cost two replacement admissions and left no useful
  parallel evidence from those branches. A first independent exact-tip
  reviewer was also host-cancelled before a verdict. The one useful
  retained implementation owner allowed a concrete launcher repair
  request instead of copying its files.
- **API versus docs:** The readiness-route error was in root's new
  contract, not server code. The candidate's CLI exposed absolute
  `--assets`, but its wrapper still invoked `cargo run --release` at
  runtime; describing it as standalone would have hidden a Rust and
  source dependency. That defect was returned to the owner, distinct
  from the no-op readiness correction.
- **Rerun scaffold:** Put a small executable start-from-unrelated-cwd
  test in the source before delegating the launcher. Have preparation
  produce a named runnable binary and make the launcher consume that
  explicit binary rather than choosing `cargo run`. Keep an isolated
  port/data fixture and confirm real browser-session/WS snapshot
  behavior before writing a readiness contract.
- **Nudges:** No wave-13 nudge ledger entries were observed by root.
  The module helper published/imported, but neither of two focused
  calls started; cancellation left no output streams. This is failed
  execution/reuse, not a passed test. After the operator identified an
  impossible 8-GiB request in a pool partly reserved by the live demo,
  a 4-GiB helper version executed on two actual root consumers (server
  8/8 and standalone 1/1). Its clean-source predicate stayed false on
  null runner status, so the direct script remained the final gate.
- **Review value and finished boundary:** Exact review accepted the
  no-Cargo launcher `ef86255`, then rejected artifact-path repair
  `19a6818` for leaving a stale release executable launchable after
  failed preparation. A further fail-closed repair `2e7e6f9` was
  independently accepted and merged. On `d2de055`, full preparation
  passed web 14/14 and production browser 1/1, and focused standalone
  1/1, server 8/8 and assets CLI 1/1 passed. A copied release bundle
  started from `/`, survived command lifetime, retained a completed
  request after process loss and stopped on SIGINT after reopen. The
  tool sandbox cannot report a host-visible service/PID, so the
  ordinary-host-shell acceptance remains with the operator.
- **Interview limitations:** Four cancelled acceptance/docs actors
  yielded no own-words interviews. Reviewer findings/checks were
  incorporated as reports, not mislabelled as interviews. The first
  attempt to request their own-words interviews was rejected before
  submission by a workbench compiler dependency mismatch
  (`Tidepool.Effects.Core` lacked `CommandQueueWait`). After the
  Tidepool source revert restored the resident compiler, root
  retried and admitted three reviewer interviews; their sections
  below record the replies that arrived.

## Wave 13 launch owner — 2026-09-25

- **Scaffold change and discovery:** The first launcher called `cargo run
  --release`; root's Request 10 identified that this still required Cargo,
  toolchain and source at runtime. The owner moved release build into
  preparation and changed launch to exec a prebuilt absolute binary with
  early missing-binary/assets checks. The defect was an initial
  implementation omission, not a Cargo API change; focused Rust tests did
  not catch shell launch behavior.
- **Missing sibling input and tree cost:** The owner had no concrete
  end-to-end operator acceptance result before its first candidate.
  Parallel acceptance, docs and launch branches were a sensible split,
  but the host-cancelled acceptance child did not supply the launcher
  check in time. Root needed a serial repair request and another
  review/integration cycle. A shared executable lifecycle test at the
  seam could have caught the runtime-Cargo dependency earlier.
- **API versus docs:** Root's original readiness sentence named a
  nonexistent `GET /api/snapshot`. The real routes are `GET /api/session`
  and authenticated `/api/ws` initial snapshot. The launcher did not
  implement a readiness probe, so the correction required no owner
  code change and is separate from the Cargo repair.
- **Rerun scaffold:** Fix the prepared artifact
  `target/release/harness-demo`, `HARNESS_DEMO_BIN` override, absolute
  DB/assets paths, prerequisite errors and unrelated-cwd mock launch
  before dispatch. Demand an actual release build/startup assertion in
  addition to shell syntax and mock launch.
- **Nudges and friction:** The owner observed no nudge firing. Extending
  `CliOptions` first broke two test initializers; after repair, focused
  checks reported 8 server matches/executions and 1 assets CLI
  match/execution. A first mock-check fixture had a malformed temporary
  shebang and failed; correcting the fixture let shell syntax and
  unrelated-cwd/missing-binary mock checks pass. The release build was
  deliberately not run during the owner's repair.

## Wave 13 launch exact-source reviewer — 2026-09-25

- **Scaffold and missing input:** The reviewer verified absolute,
  serve-only `--assets`, legacy ask/serve compatibility, script
  syntax and focused Rust checks at the exact tip. The missing
  host-environment contract was Cargo's target directory: source
  review did not run preparation, so it could not establish where
  the executable landed. The reviewer needed root's retained
  integrated command exit, environment and artifact paths.
- **API versus docs and tree cost:** The script assumed
  `target/release/harness-demo`, but this environment wrote to
  `.exomonad/build/cargo/release`. Passing focused source tests did
  not make preparation usable. Exact-base diff, production consumer
  inspection, two focused invocations and shell syntax had moderate
  review cost; tool-truncated output required retained count
  summaries. Later shell-inspection jobs queued, delaying evidence.
- **Rerun scaffold:** Record `CARGO_TARGET_DIR`, check the Cargo
  output path, then launch from an unrelated directory with
  absolute DB/assets and no secret in argv. Carry expected
  selected/executed counts into the acceptance packet.
- **Nudge and friction:** The reviewer calls out the nudge not to
  infer real preparation from source review or a successful Cargo
  build. Its first review explicitly left web preparation and
  standalone launch with root; that boundary should be stated
  early rather than presenting source acceptance as end-to-end.

## Wave 13 artifact-path reviewer — 2026-09-25

- **Finding and missing contract:** The reviewer traced the configured
  target output through `scripts/prepare-browser-harness` into its
  launcher consumer. If the configured build produced no executable,
  preparation exited without removing an older stable binary, while
  the launcher accepted any executable at that stable path. The
  assignment said “missing binary cleanup” but did not state the
  stale-output consequence; inspecting the actual consumer supplied
  it without another sibling result.
- **Docs/API and tree cost:** The preparation and launcher paths agreed;
  no separate API/documentation mismatch appeared. Review was one
  owned-script diff, one production consumer read, syntax and
  whitespace checks. Cargo and the full browser journey were
  intentionally not run in this source-only review.
- **Rerun scaffold and nudge:** State success as “configured release
  executable exists and is staged at the launcher path,” and failure
  as “no stale executable remains launchable.” Put a focused
  stale-output fixture in the acceptance packet rather than relying
  on the vague phrase “cleanup.” The review nudge was to check the
  consuming launcher before treating a missing-source error as
  harmless.

## Wave 13 fail-closed repair reviewer — 2026-09-25

- **Scaffold and checks:** At exact candidate `2e7e6f9`, the reviewer
  read the cumulative preparation diff and launcher. Isolated
  mocked Nix and Cargo failures with an old default binary left no
  launchable default; configured-target success staged an executable.
  A mocked default-target “Cargo reports success but no output”
  case failed and left no old default. Shell syntax and diff checks
  passed. The reviewer did not run real Cargo's fingerprint/cache
  behavior or full Nix/web/browser preparation.
- **Sibling evidence and documentation:** The assignment supplied
  root's earlier web 14/14, browser 1/1 and failed artifact-lookup
  evidence as a boundary, not as new reviewer execution. The
  reviewer found no separate API/docs mismatch; the explicit shell
  contract and launcher path governed this narrow review.
- **Tree cost, rerun scaffold and friction:** The clean exact checkout
  and one-file cumulative diff kept review modest, but hand-built
  temporary shell mocks were needed because the repo had no script
  fixture. A reusable checked-in fixture could count failure,
  configured-target, default-target stale/fresh-cache and stage
  cleanup cases while keeping full preparation separate. One
  Haskell multiline-list cell was rejected before a compact
  binding succeeded; it did not alter the verdict. No repair
  finding remained.

## Wave 12 root — 2026-09-25

- **Tree cost and benefit:** Disjoint server, web and production-binary
  acceptance owners could work from one shared contract. Web preparation
  and an expected-red consumer test were integrated while the server
  was repaired. A bounded read-only Engine/Store consultant resolved
  the missing-seam claim without a library edit. No component-lead layer
  was needed: adding one would have added a relay across the single
  `main.rs` seam rather than useful independent ownership.
- **Where prompting failed:** The first server packet required a real
  Engine path but omitted the existing `Engine::with_transport` example.
  The owner initially declared the credential-free path blocked and
  then produced a bypass. The follow-up contract had to name the
  production consumer, genuine Store child service and no-fake-progress
  invariant. The first review found fake messaging, missing progress,
  duplicate upserts and incomplete cleanup, all consequential.
- **Where the harness helped:** Exact cumulative ownership plus
  `git merge-tree` let reviewed web/test preparation merge independently.
  Typed request IDs prevented old settlement notices from being
  mistaken for current replies. `scripts/cargo-focused-test` enforced
  nonzero selection and exact executed counts. Independent review of
  `7ae65fb` eventually verified both server 8/8 and real-binary 1/1
  after assets were supplied. Integrated checks and Chromium then
  exercised the joined source.
- **Where orchestration cost turns:** A retained `reviewAgain` started
  on stale `bcedd45` rather than `7ae65fb`; the reviewer correctly
  refused to switch its dirty checkout. A fresh exact-tip review
  started correctly but its browser check failed before readiness
  because `web/dist` was missing. A second request to the same
  reviewer built the assets and passed. The session helper seed
  compiled/published but could not be imported in notebook or child;
  measured reuse was zero. This experiment was archived under
  `/tmp/wave12-live/helper-experiment`; direct script calls were the
  fallback.
- **Product versus evidence:** The reviewed branch was not delivered
  until merged at `2c19e45` and rerun 8/8 and 1/1 there. Local
  Chromium login/command/reconnect and a separate process-kill
  recovery passed, while remote-device tailnet reachability remains
  for the operator. The browser script's first selector, transient
  pending-history and final-echo count assertions failed as test
  design issues; they were corrected with exact UI selectors,
  lifecycle-aware expectations and a unique final-answer barrier.
- **Next improvement:** Make `reviewAgain` pin or explicitly report
  its HEAD before work, and make a production-binary acceptance target
  provision or declare web assets rather than failing before the
  assertion body. A helper publication receipt should include a
  consumer import/use smoke check. Preserve the valuable typed
  exact-source and positive-count gates rather than replacing them
  with another review submission wrapper.

### Supervisor's six-question wave-12 retrospective

1. **Parallel value and serial convergence.** **Observed:** One ready
   frontier gave `main.rs` to the server owner, `web/src/` to the web
   owner, and `browser_journey.rs` to an independent acceptance owner;
   the first acceptance actor was host-cancelled and replaced. Web
   `ef5e612` and additive-renderer `3bfa370` were reviewed and merged
   while the server owner repaired its path. The acceptance test
   `1e08bc6` was useful *red* evidence: it selected/executed 1/1 and
   failed at absent `commandId`/`outcome`, not at a guessed UI state.
   The read-only Astra consultation independently found
   `Engine::with_transport` and `StoreAgentToolService` without a
   library edit. **Serial point:** one owner and one production file
   (`main.rs`) then had to absorb the shared contract and the
   `bcedd45` review findings (fake child, absent progress, duplicate
   upserts, cleanup), reach `7ae65fb`, receive exact-tip acceptance,
   and only then merge at `2c19e45` for combined checks and Chromium.
   Explanation: the work was parallel by ownership, but a single
   server producer was the unavoidable integration gate; more lead
   layers would not have removed it.
2. **Missing facts versus real discovery.** **Observed:** The initial
   packet said “real Engine path” but omitted the existing
   `Engine::with_transport`/`ResponsesTransport` replay example and
   `StoreAgentToolService::{spawn_agent,send_message}`; request 1
   returned Blocked, then `70f3989` implemented a Store-only bypass
   despite 5/5 local server tests. A first-call packet naming that
   constructor, the `--serve` production consumer, and explicit
   “no canned child reply/progress” and cancellation failure barriers
   would likely have changed the first attempt. **Inference, not
   observation:** that packet might have saved the initial Blocked and
   bypass rounds; it would not guarantee a correct implementation.
   Genuine discovery still included how the existing Request and Job
   state enums represented cancellation (`c9c7281`), whether an
   Engine turn actually persisted progress and child delivery, and
   how HTTP/Engine tasks were joined on errors. Exact `bcedd45`
   review and the production-binary test established those facts.
3. **Late notices and refused/unconfirmed steering.** **Observed:**
   old candidate settlements (including `70f3989`) surfaced after
   newer work was active; root repeatedly reconciled request IDs and
   explained that a reply was not an integration. The cancellation
   `updateRequest` returned `UpdateUnconfirmed` for the server
   (queued, not known presented); the acceptance owner's update
   returned `ReplyAlreadySettled`, requiring a fresh request carrying
   `c9c7281`. The server later named the correction in its candidate,
   which is incorporation evidence rather than proof from the update
   receipt. **Cost:** at least one new acceptance request and several
   explicit source/status reconciliation turns; no reliable token or
   wall-clock savings were measured. **Explanation:** a transport
   receipt cannot prove a child read or used a correction; a
   supersession-aware, presented/incorporated event would have reduced
   repeated explanations without silently replacing assignments.
4. **Cleanup sequencing and the needed receipt.** **Observed:** At
   delivery root executed seven `planCleanupFor`/`executeCleanup`
   operations in one Haskell cell, sequentially. The first
   ready-frontier plan covered actors 2 and 3 together, so a later
   plan for actor 3 was empty. Every plan had no pending response or
   watch; several `CleanupReceipt`s said `cleanupReceiptComplete=True`
   while their actor step was `AgentStoppedReleasing`. Later host
   notices separately confirmed actor resources released. **Why
   sequential:** the operations mutate group/actor revisions and
   shared group 1, and a plan can stale; sequential composition was
   the simplest safe decision, not a measured throughput need.
   The `StoppedReleasing` receipt already permitted ending the turn
   without polling or issuing stop again. A clearer *batch* receipt
   separating “request settled,” “actor terminal,” “release pending
   with a notice token,” and “resource release terminal” would have
   made it obvious when root could finish and which releases still
   awaited notice. `cleanupReceiptComplete=True` alone did not mean
   process cleanup was terminal.
5. **Reusable helper and existing payoffs.** A small
   `focusedGate(package,target,filter,expected)` returning the
   retained evidence path, exact selected/executed counts, outcome
   and (only on failure) bounded semantic triage would have been
   used for the server 8/8, production browser 1/1, and existing
   process-restart 1/1 gates, and by owners/reviewers running those
   same boundaries. **Observed experiment:** the customized
   `SessionHelpers.TestEvidence.demoSpec` compiled/published, but
   notebook import and child lookup failed; actual reuse and calls
   avoided were zero. It was not counted as working infrastructure.
   `scripts/cargo-focused-test` itself already paid off by enforcing
   positive selection and retaining JSON evidence. Typed
   `Response`/request IDs, `reviewCommit` at an exact OID,
   cumulative owned-diff/`git merge-tree`, and cleanup plans also
   paid off; an automatic review/repair collector was not necessary.
6. **Top three next-run structural changes and surprise.**
   (a) Make the first-call contract a source-pinned executable
   producer→Store/Engine→wire→browser example with one failure
   invariant and named constructor/consumer, not more prose.
   (b) Make review continuation bind and report its exact checkout
   HEAD and required build artifacts before checks: request 20's
   `reviewAgain` stayed at `bcedd45` rather than `7ae65fb`, and
   fresh request 21 selected/executed the browser test 1/1 but failed
   before any journey assertion because `web/dist` was absent;
   request 22 built assets and passed. (c) Give asynchronous
   assignments a supersession-aware presented/incorporated signal,
   and give cleanup a separate terminal resource-release receipt.
   **Surprise:** the helper publication receipt did not imply an
   importable module; the failed archive move copied its files to
   `/tmp/wave12-live/helper-experiment` but could not remove the
   mounted `.exomonad/helpers` directory (`Device or resource busy`).
   No user work or demo process was removed. This is a workflow
   observation, not a product failure.

## Wave 11 root — 2026-09-25

- **Tree and overlap:** Three Luna component owners each admitted production
  and test children from scaffold `55d2cce`; while they worked, root built
  separate Store and real-Driver process-kill gates. This depth did create
  overlapping code/test/review work, but also several extra correction relays.
- **What depth helped:** Separate test files let Store recovery, Engine
  request-boundary logic and Driver restart admission proceed without racing
  on `store/mod.rs`, `engine.rs` or `driver.rs`. A retained exact-source
  reviewer found Engine's zero-row `interrupt_claim` race; the Driver
  reviewer demanded an explicit no-blind-replay barrier.
- **What depth cost:** A durable lead prematurely settled `Blocked` while
  children ran and required a second Delivery request. The Driver test
  child also settled `Blocked` before its fixture decision. One
  `updateRequest` carrying a critical caller correction stayed
  `UpdateUnconfirmed`, and repeated old assertions that atomic completion
  lacked a production caller created avoidable cross-lane discussion.
  Root's direct `git grep` showed the caller in `harness-demo/driver.rs`.
- **Scaffold defect versus review:** The scaffold supplied compiling test
  modules and existing atomic completion, catching a nonexistent
  `driver_restart` filter (0 matches) early. Review, not the scaffold,
  caught the Engine affected-row race and Driver no-blind-replay test gap.
- **Next time:** Put the exact production caller file:line and the
  non-consuming `unread` contract in the first-call brief. A typed
  candidate must be a changed, checked commit, not a no-change
  inspection; component owners should keep Delivery pending through
  their children and publish reviewed slices promptly.
- **Contract drift and redundant coordination:** Engine's `70412ae`
  briefly recast an inherited Pending non-`wait_agent` claim with a missing
  in-memory job as `UnsupportedInFlightJob`, conflating it with the distinct
  active-head/empty-inbox ambiguity. Its tests still expected typed
  Interrupted. `65bace1` reverted the change and is source-equivalent to
  the exact-reviewed `6988d51`; root had already integrated that candidate
  as `ec012f0` and checked the combined gates. Root's extra policy question,
  correction relays, separate exact review and equivalence check were
  coordination costs rather than new product behavior. A single pinned
  claim-state table and a test against the actual consumer before changing
  the contract would have prevented the detour. The operator audit ruled
  out adding CLI restart admission to the written acceptance matrix.

## Wave 11 durable component owner — 2026-09-25

The owner said two bounded children were enough: a separate
`recovery_tests.rs` child produced the isolated accepted slice, while one
exact-source reviewer strengthened confidence without broadening
production scope. After an initial review test-run exit 137, the rerun
passed and review confirmed close/reopen, idempotence and
Pending-versus-Interrupted assertions without claiming process-kill
durability. Queued/unpresented contract corrections around the redundant
`pending_envelopes` API consumed time and produced an unmerged rejected
branch (`9cc0866`, later `d6b9ab8`) and an unnecessary repair/review loop.
The useful nesting was implementation → one exact-scope reviewer; further
production relay depth did not help. Root additionally observed that a
search limited to `crates/harness/src` missed the actual completion caller
in `harness-demo/driver.rs`, delaying the no-new-API resolution.

## Wave 11 Driver component owner — 2026-09-25

The owner's settled Delivery and checkpoints provide the interview evidence;
a separate own-words interview was requested but not received by settlement.
Nested production and recovery-test children did useful work in parallel:
production inspection showed the existing `list_agents()+unread` scan and
completion caller were already sufficient, while the test child built the
file-backed restart barrier. The production child returned an unchanged
finding, not a candidate; the test child needed a fixture correction,
a bounded retry, and a reviewer-requested no-blind-replay case. The owner
preserved the seam by refusing a Store hydrated-query dependency and
waiting for exact typed review correction after the reviewer initially
named the stale candidate. Depth helped keep Driver production untouched
where it already worked, but cost relays across the Store/Engine contract,
an unnecessary optional rebase, and the isolated rustfmt collateral. Its
final Delivery distinguished clean reopen from root's separate killed-helper
gate and did not claim real provider or Engine pending-claim recovery.

## Wave 11 Engine component owner — 2026-09-25

The owner's own-words interview arrived asynchronously after root retired
the prolonged review loop; no typed Delivery settled. It said splitting
production implementation from independent boundary tests gave parallel
code and adversarial coverage, while a later test verifier quickly
confirmed the correct integrated behavior. The cost was repeated
exact-source review and corrections relayed across stale candidate tips;
one production branch was briefly blocked by putting EngineError mapping
inside a StoreError-only blocking closure. It traced the source confusion
to applying f7's ambiguous empty-inbox agent-head rule to a distinct
inherited Pending non-`wait_agent` claim with scheduler UnknownCall, then
propagating an intermediate fail-closed experiment despite tests still
expecting typed Interrupted replay. Root's clarification restored the
guarded version. The owner reported `65bace1` with `engine_recovery_`
2 matched/executed/passed and `dynamic_reply_schema` 2/2; root
independently reviewed code-equivalent `6988d51`, integrated it and passed
the combined focused gate. The useful depth was disjoint test ownership
plus one reviewer, not multiple candidate/review relays.

## Wave 11 root process-gate reviewer — 2026-09-25

The reviewer reported that exact-source isolation made the failure model
clear: a helper committed a typed answer/head, reached a post-commit
pre-wake barrier, and was killed; fresh Driver startups found the one
answer and joined shutdown. It verified 1 matched/executed/passed and
read Driver scan/start/cleanup. The node cost one isolated checkout and
test run, with no repair round; an unrelated unused-import warning was
the only reported friction. It emphasized that this is not a crash of
an already-running Driver, a real provider job, or power-loss durability.

## Wave 11 Driver integration reviewer — 2026-09-25

The reviewer said exact base/tip isolation prevented it from treating the
integration branch as evidence. The clean file-Store reopen test proved
durable inbox discovery without a live wake, while the active-head/empty
inbox test prevented duplicate model turns through blind replay; recovery
and follow-up ran 2/2 each and existing no-replay ran 1/1. Its reviewer
node cost context and tool overhead: it initially observed an unrelated
command handle, then recovered the actual retained test output. It
distinguished orderly reopen from root's killed-helper process gate.

## Wave 11 Engine integration reviewer — 2026-09-25

The reviewer verified exact `6988d519` and its cumulative diff from the
scaffold. For an inherited Pending claim with fresh-scheduler UnknownCall,
Engine interrupts only that claim and replays a typed Interrupted output;
on a zero-row transition it rereads durable state, preserving an already
Settled output and failing closed on unresolved states. It ran
`engine_recovery_` 2/2 and `dynamic_reply_schema` 2/2, and made no
remote-execution exactly-once claim. Its separate checkout, cumulative
diff and consumer inspection cost one additional review node and test
execution, without implementation change. `friction:` notebook
multi-line binding and detached-signature attempts were rejected before a
simpler combined binding cell succeeded.

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
# Wave 14 root interview — before-request hook

**Observed outcome.** I landed the Send-only contract and serialization
example first, then admitted three disjoint Luna owners: Provider/Store,
Engine and independent expected-red browser acceptance. Three review actors
were used in total: one retained Provider/Store reviewer across repair, the
initial Engine reviewer, and one fresh exact-source replacement after
reviewAgain remained pinned to the old tip. Provider/Store and Engine both
required real repairs found by review. The standalone browser barrier went
red before producer integration and green on integrated source `75a1e014`.
Every final focused check selected/executed/passed 1/1; the web prerequisite
ran 14/14 and the browser journey 1/1.

**What the tree cost.** Parallel owners let the browser test expose missing
product evidence while Engine and Store work progressed, but handoff cost was
real. I had to correct an early browser-chain assumption (production
standalone uses CliProvider, not TreeProvider), an acceptance owner's reading
of `deterministic_command` (it still calls Engine), and the browser test's
`before_request` versus `before-request` key. The first Engine candidate had
no focused test; a later review caught a missing pending-claim cleanup. The
Store reviewer caught a hand-built JSON value that did not represent typed
Send. These corrections were more valuable than a single unchecked merge,
but made two repair waves necessary. I did not measure aggregate model tool
calls or elapsed model time reliably enough to claim savings.

**Helper experiment.** I republished a 3-GiB `SessionHelpers.TestEvidence`
variant locally and used one Haskell cell to start/finish the scaffold test:
one focused runner and one evidence read, 1/1 passed, with strict
clean-source false because helper files were dirty. Provider/Store's first
import failed with zero command effects; after publication it ran one
start/finish cell with a focused runner and evidence read, 1/1 passed at a
dirty source. The independent reviewer could not import the helper and used
the direct script. Engine's diagnostic trial ran 0/0/0 and its Jev failure
classification encountered a circuit-open 403; it correctly switched to
direct focused checks. The supervisor later clarified that 3 GiB was a
valid experiment reservation; my initial insistence on the committed
4-GiB variant created avoidable steering. Helper consumption was useful
for one owner but not reliably reusable across fork snapshots this run.

**Next improvement.** Publish the helper source once before forking and
name that published revision in every assignment, then distinguish inherited
workspace modules from later root edits. Keep a fresh exact-source review
checkout for repairs rather than relying on a retained checkout to move.
Use one explicit hook-key constant or typed mapping at the persistence seam
so producer and acceptance tests cannot silently spell different keys.
These are proposals, not changes made in wave 14.

## Wave 14 acceptance owner interview

**Report.** The independent owner changed only the standalone browser test
to inspect persisted decisions, correlate Engine request UUIDs with Store
request branch/items, assert marker/Send/provenance and no extra decisions
after reopen, while retaining reconnect/process-loss checks. Its first
candidate `49b5eded` and hook-key repair `506e1791` each ran one selected
and executed standalone test; the corrected pre-producer test failed with
an empty decision list as intended. Its checkout also prepared web assets
and passed web 14/14 and browser journey 1/1. The owner did **not** run
the integrated green test; root ran that later at `75a1e014`.

**Tree cost and friction.** Parallel Provider/Store and Engine work allowed
the expected-red consumer test to develop independently, but the owner
needed three handoff clarifications: exact opaque marker, the difference
between Engine request UUIDs and browser UI IDs, and source proof that
browser commands enter Engine. Root's diff inspection caught the test's
`before_request`/`before-request` mismatch before integration. The owner
reported no Haskell helper or review-handle calls, so no experiment effects
are attributed to this node. Its inference is that a source-backed
consumer/serialized-key note before delegation would reduce coordination
cost; next time it would verify the concrete consumer and hook spelling
before writing the assertion.

## Wave 14 Engine owner interview

**Report.** The owner wired the hook into Engine's final
`ResponsesRequest` path, persisted typed results against the active
Engine request, made a slow hook cancellable, and repaired decision-write
failure routing through `cleanup_pending`. At candidate `978c650e`, the
pending-claim Store-failure test and the
transport-failure/pass-through/provenance/reopen test each matched,
executed and passed 1/1. The owner did not run those tests at integrated
`75a1e014`; root did.

**Tree cost and friction.** Provider/Store and acceptance advanced in
parallel, but review caused two serial Engine repair handoffs: first
coverage/cancellation, then Store-failure cleanup. Parent corrections
clarified request/branch provenance and that the browser command ID is
not the Engine request ID. The owner cannot quantify saved time; it
infers parallel ownership gave independent contract/consumer work while
review exposed lifecycle gaps. The helper import first failed with no
command effects. After reload, a one-cell start/finish attempt issued
three command effects: focused Cargo exited 1 with 0 matched/runnable/
executed, evidence read exited 0, diagnostic tail exited 1; a secondary
Jev diagnosis returned HTTP 403. Direct focused tests, not that
experiment, established its candidate. Total model-call count was not
known. The proposed improvement is to publish and verify helper modules
and name exact focused filters before implementation, keeping the direct
script as delivery fallback.

## Wave 14 fresh Engine reviewer interview

**Report.** The replacement reviewer verified exact HEAD
`978c650e`, inspected the engine.rs-only cumulative diff and PRD/brief,
and ran both focused tests at that tip: each selected, executed and
passed 1/1. The injected Store-failure fixture observed the pending
claim become `Interrupted`; the transport-failure fixture covered
Send pass-through, evidence, request provenance and readback.

**Tree cost and friction.** This reviewer was needed because the retained
reviewer's checkout stayed on the old candidate. It saw no new contract
correction during its own exchange and cannot measure the broader tree's
cost. It infers that a fresh exact-tip checkout avoided a stale-source
verdict. Its unrelated helper README edit was left untouched. An initial
combined check's retained handle became unavailable (`Unknown (command
job)`), so it ran the two prescribed commands individually and retained
their 1/1 evidence. It did not run a helper trial. Proposed improvement:
put exact-tip identity and per-command evidence handles into review
handoffs, and use individual focused commands when observation is
unreliable.

## Wave 14 Provider/Store reviewer interview

**Report.** At repaired exact tip `cb8e94ba`, the reviewer checked the
owned cumulative diff, typed Send survival through Store reopen beside
historical generic decisions, and marker forwarding through
CliProvider/TreeProvider. The Store and demo checks each selected,
executed and passed 1/1. It accepted that component, not the later
integrated Engine/browser product.

**Tree cost and friction.** Parallel implementation and consumer work
allowed independent component review. The first review caught a
hand-built JSON value in the Store test, which proved only generic
roundtrip, not typed Send persistence. Repair and re-review added a
handoff but closed a real evidence gap; net time saving is only an
inference because the reviewer did not observe all sibling timelines.
Its one-cell helper import failed (`Could not find module`) before any
command effect; it used direct focused scripts instead. The retained
checkout initially remained at the old tip, so it explicitly switched
to `cb8e94ba` while preserving an unrelated dirty helper README. One
stale completed-job observation returned `CommandUnavailable`; it did
not replay that job and then observed the correct handle. Proposed
improvement: publish the helper to reviewers before admission and
attach exact source plus real command evidence to the review request.

## Wave 14 Provider/Store owner interview

**Report.** The owner forwarded `before_request` through
DemoProvider→CliProvider→TreeProvider, supplied deterministic opaque
browser evidence, and tested typed Send persistence/reopen alongside
historical generic decisions. At repaired candidate `cb8e94ba`,
the Store test serialized `BeforeRequestDecision::Send` to `"Send"`,
reopened the record, and checked request/branch association. Its
focused Store and demo-chain checks each selected, executed and
passed 1/1.

**Tree cost and friction.** Engine and acceptance were disjoint
siblings, avoiding duplicate implementation, but the reviewer found
the owner's first test had hand-built the wrong JSON representation,
causing a repair and repeated focused check. Root also corrected the
assumption that the deterministic browser used TreeProvider: its
actual direct path uses CliProvider. The owner reports extra handoff
turns and only infers an overall parallel benefit. Its first helper
import failed before any command effect. After `reload_helpers`, a
one-cell start/finish run used the dirty 3-GiB workspace helper:
focused job `a191c28d` matched/ran/passed 1/1 (130 filtered);
`finishFocused` read evidence through command `e1ee974a`. The helper
run recorded a dirty checkout. `cargo fmt` surfaced an unowned
hooks.rs-only formatting change; the owner excluded it from the
candidate. Proposed improvement: establish the actual provider path
and typed serialized value before forking, and check helper/worktree
state before import.

## Wave 15 Engine owner interview

Implemented Engine validation of selected final tool names and transport
`allowed_tools`/`none`/`auto` serialization. Its final candidate
`f616be77` passed focused serialization, invalid-selection/pending-claim
cleanup, and default-auto tests, each 1 matched/executed/passed. It
does not claim browser integration. Disjoint ownership helped merge
preflight against the advancing acceptance test, but a first focused
command exited 137 before tests ran and required a repair handoff.
`friction:` The owner recommends a realistic 3-GiB reservation on
the first focused invocation rather than treating build termination
as test evidence.

## Wave 15 Provider/Store owner interview

Implemented browser-local selection, canonical `request_body` JSONL
capture with typed open/write failure, and restricted Store typed
readback. Final candidate `59bf6236` passed Store, provider policy
and capture focused checks, each 1 matched/executed/passed. It does
not claim integrated standalone-browser acceptance. Independent
review caught whole-history substring misrouting and drove the
latest-user-command repair; source/contract handoffs also corrected
the `ask` assumption to `sleep`. `friction:` The first reported
candidate OID was mistyped and nonexistent. The owner recommends
deriving and verifying the exact reply OID with `git rev-parse HEAD`
and `git show` before submission.

## Wave 15 acceptance owner interview

Owned the standalone browser test: echo completion, captured full tools
and selection, typed Store decision/request/agent/evidence, reopen
invariance, and existing child/reconnect/process-loss journey. Corrected
`ask` to `sleep`, enriched evidence matching and restricted decision
assertions. Its focused test selected/executed 1/1 in four reported
runs, none green: first stopped at missing `web/dist`; later runs
reached intended expected-red missing capture or old-provider evidence
barriers. It claims no integrated acceptance. Separate ownership
allowed a red barrier before producers, but tool-name, evidence and
decision-shape corrections caused repeated handoffs. `friction:`
The child lacked `web/dist`; Nix could not use an untracked
`flake.nix`, so it used a temporary minimal asset fixture. It
recommends freezing advertised names, evidence and decision shape
before forking and providing reproducible asset preparation.

## Wave 15 Engine reviewer interview

Reviewed `f616be77` at exact HEAD; focused transport serialization
and Engine invalid-selection/cleanup checks each matched/executed/passed
1/1. Independent review added a separate check but its first typed
`Repair` contradicted its no-defect prose, costing a clarification
handoff. A combined Cargo invocation exited 137 before sequential
focused commands with explicit memory passed. `friction:` The
reviewer recommends checking verdict against findings before
submission and avoiding a memory-heavy combined build. No integrated
browser acceptance was claimed.

## Wave 15 first Provider reviewer interview

Reviewed `b5b61f84` without edits; Store roundtrip, browser policy
and capture checks each matched/executed/passed 1/1. It caught the
whole-request substring-routing defect before integration and
returned `Repair`; root retained the browser-local wrapper seam
and returned the concrete classifier fix to the owner. A fresh
review was required after repair. `friction:` Finding exact focused
test names required searching `main.rs`; the reviewer recommends
putting exact filter names and expected counts in review packets.
It did not claim later integrated browser gates.

## Wave 15 source-composition reviewer interview

Reviewed exact provider HEAD `59bf6236`. Store restricted reopen,
demo policy and capture checks each matched/executed/passed 1/1.
Its checkout still hardcoded `tool_choice:"auto"` because it began
before sibling Engine/transport `f616be77`; the reviewer initially
reported that observation as `Repair`. Root clarified that it proved
an exact-source composition gap, not a provider-owned defect, and
required a rebase for combined review. The separate standalone test
was not run on this source. `friction:` The extra review cost three
focused runs and a handoff but exposed the source boundary. The
reviewer recommends naming required sibling commits at admission,
or explicitly marking a candidate partial, then reviewing combined
source for product acceptance.

## Wave 15 combined provider reviewer interview

Reviewed exact candidate `8db250a5` on integrated Engine base
`37f9c74`, without edits. Store, transport, provider policy, capture
and Engine invalid/cleanup checks each matched/executed/passed 1/1.
After documented web preparation (separate browser journey 1/1),
standalone browser acceptance matched/executed 1/1 but failed:
the child row had `SendRestricted([sleep])`/`echo-sleep`, not
`child-empty`. A prior run with missing `web/dist` had failed at
the prerequisite, not product assertions. Narrow component tests
quickly checked their parts; exact-source production review caught
a real sequence gap those fixtures missed and required another
provider repair/review. `friction:` Absent `web/dist` caused an
initial non-product failure. The reviewer recommends checking or
preparing assets explicitly before the focused browser assertion.

## Wave 15 final Provider reviewer interview

Reviewed exact candidate `5f31285c` without edits. Store restricted
reopen, explicit browser policy, production capture, and standalone
browser focused tests each matched/executed/passed 1/1; the prepared
browser journey also passed 1/1. The preceding production review had
found child misclassification; the repair moved policy authority to
explicit invocation command and agent, and this review exercised
that consumer. Repeated handoffs and exact-source re-review cost
time, but caught behavior beyond isolated component fixtures.
`friction:` One combined Cargo handle became unavailable and a
later high-memory build exited 137; sequential focused checks with
retained handles and realistic memory completed. The reviewer
recommends asset preparation before Cargo and sequential focused
review checks. Root merge and post-merge checks were not its claim.

## Wave 15 root interview

Root landed the compiling shared contract, assigned disjoint
Engine/transport, Provider/Store, and independent browser acceptance
paths, then integrated exact-source-reviewed candidates. Nine named
focused Cargo checks on product source `40bd399e` each
matched/executed/passed 1/1; web tests passed 14/14 and the separate
browser journey 1/1. The final standalone production consumer
confirmed selected `sleep` versus child `none`, unchanged full
definitions, typed Store provenance and reopen behavior.

Parallel acceptance exposed missing request capture before producers
landed. Independent review caught whole-history classifier
misrouting, and then the real browser child failure; passing
component checks alone would not have closed those defects. The tree
cost contract handoffs: root initially named `ask` before checking
`CliProvider.tools`, reviewed provider-only source lacking sibling
Engine transport, and repeated review after a production failure.
Final explicit invocation context removed that ambiguity.
`friction:` The background CheckResults notice arrived without
counts after an evidence read failure, and later retained raw-output
recovery returned HTTP 409; on-disk evidence was read without
rerunning. Improvement: fix a small executable producer/browser
request example with advertised names and capture seam before
dispatch, then require combined-source browser review.

## Wave 15 root — supervisor follow-up interview, 2026-09-26

This is findings only. The integrated product source remains `40bd399e`;
no implementation or product check was started for this interview.
**Trace evidence** below means a retained job, documented check outcome,
candidate/review reply, or automation record. **Recollection** is my
reason for a choice where the retained artifacts do not prove intent.

1. **Short command waits.** Recollection: I used `bash`
   `yield_time_ms=1000` to get a command handle promptly and kept
   observing that same handle with empty `write_stdin`, so an apparently
   long Cargo/Nix build would not be mistaken for a stopped job or
   restarted merely to obtain output. It was a manual completion loop,
   not a deliberate use of background-completion routing. For commands
   whose result was a dependency, a longer initial wait would have been
   simpler; for independent checks, `background: true` plus a registered
   completion route would have freed the turn. Trace evidence: final
   integrated focused runs and the web preparation have retained job
   IDs and terminal outcomes in `docs/wave15-handoff.md`. The bounded
   watcher did send a late notice, but its evidence read failed and its
   compact result was unknown (`docs/automation-trials.json`). I do not
   find an intermediate *still-running* observation that changed a
   product decision: the decisions came from terminal outcomes and
   evidence. In particular, missing `web/dist` led to prerequisite
   preparation, an exit 137 led to sequential memory-bounded focused
   checks, expected-red product assertions kept gates open, and the
   combined candidate's failed child assertion led to repair. Those
   are terminal findings, not a justification for repeated one-second
   polling. A future loop should wait longer on a blocking job or
   register a reliable completion route, retaining the original handle.

2. **Advertised automation not used.** Trace: the menu and its
   wave-specific statuses are recorded in `docs/automation-trials.json`;
   only `CheckResults.watchChecks` and bounded
   `RetainedEvidence.recoverRetained` were exercised here. Recollection:
   after the watcher failed to read evidence, I did not trust a compact
   automation notice as the sole final acceptance proof, although that
   failure does not establish other helpers are broken.
   `notifyReviewReady` offered little fit because exact candidate/base
   and typed review admission were already in hand; it notices
   readiness, not review. `diagnoseFocused` was unnecessary for
   unambiguous missing-asset, exit-137, and exact assertion failures,
   while the original evidence-read failure also made evidence inputs
   uncertain. `watchAssumption` and `startProbeBatch` did not replace
   the required concrete source/consumer inspection; a typed
   assumption baseline had not been installed before the `ask` error.
   `collectInterview` was not needed to obtain the actual retained
   interview replies. `verifyPrepared` was not adopted because web
   preparation and the dependent browser check were already being
   handled as an explicit sequential gate. `handoffProposal` was
   unnecessary once exact commits, reviews and checks were assembled
   directly; `watchSlowCommand` overlapped the manual job handles but
   would need a trusted completion route. The separate ReviewFlow and
   browser-scenario helpers still had runtime gates and were not
   product-ready. This is a fit/trust judgment, not evidence that each
   helper was imported or tried: the trace records no wave15 import or
   setup attempt for those skipped entries.

3. **Bounded delegation I would trust.** A small Haskell function can
   take an exact source OID, package/target/filter/expected count,
   memory reservation and a retained `Cmd.Job`; read the focused
   evidence, return selection/execution counts, source, exit/cleanup
   and an explicit `EvidenceUnavailable` rather than an empty result.
   Applied here it could have classified the existing
   `request_is_stateless_and_pins_effort` job from its on-disk
   `evidence.json` without rerunning it. A Jev judgment is useful only
   *after* code establishes those facts: it could label a terminal
   failure as prerequisite (missing `web/dist`), build-resource
   interruption (exit 137, zero tests), expected-red assertion, or
   product assertion, then propose one bounded next action. It must
   not turn exit 137 into a test failure or declare a review accepted.
   A second function could compare exact `git rev-parse HEAD`,
   assignment base, owned-path cumulative diff, and required sibling
   commit ancestry before admitting review. These are proposed
   follow-ups, not helpers executed for this interview.

4. **Facts to freeze before delegation.** Trace: the first task
   assumed echo could restrict `ask`; `CliProvider::tools` filters
   `ask`, so the final advertised safe name was `sleep`. The
   provider-only reviewer at `59bf6236` observed `tool_choice:"auto"`
   because its checkout lacked sibling Engine/transport `f616be77`;
   combined review on `8db250a5` then found a real child-route
   failure despite passing component checks. Before forking I should
   have run one compiling production example through the final
   `CliProvider.tools`/`RequestPlan` list and the canonical
   `request_body`: root `echo ...` -> selected `[sleep]`, child ->
   `[]`/`none`, default -> `auto`, with full tool schemas unchanged.
   The task packet should have named the existing deterministic
   transport capture seam, exact evidence keys (`selection` and
   `sleep_advertised`), typed Store decision and request/agent
   provenance, and the fact that `before_request` is infallible while
   Engine validates invalid names before transport. Each reviewer
   needed the actual candidate HEAD **and** the incorporated sibling
   source (or an explicit label that the checkout was partial), plus
   the standalone-browser test and web-asset prerequisite. Recollection:
   that executable shared sequence and source-composition gate would
   likely have removed the `ask` correction and the false attribution
   of a sibling-source gap; it would not by itself prove away the real
   child failure, which required the production consumer test.

## Wave 15 root — API-quality retrospective, 2026-09-26

**Evidence first.** At scaffold `a61a9c6`, I started
`request_is_stateless_and_pins_effort` through
`Project.TestEvidence.startFocused (GiB 3)` and
`CheckResults.watchChecks`; job `049a8eea` selected/executed/passed
1/1. The watcher noticed completion but its first evidence read
failed, yielding unknown counts/source. `collectFocused` later read the
original evidence without rerunning; `recoverRetained` then found raw
output expired (HTTP 409). See `docs/automation-trials.json`.
**Recollection:** the repeated round cost was re-observing handles,
extracting counts/source, verifying the reviewed revision, and
coordinating review/repair. The clearest episode was
provider source `59bf6236`: a reviewer saw `tool_choice:"auto"` in a
checkout missing sibling Engine commit `f616be77`; I had to identify
source composition, obtain a rebase, request combined review, then
route the real child failure at `8db250a5`. A compositional Haskell
source/evidence gate could have prevented the first review round.

“Noticed” is recollection absent an import or call trace; I cannot
attest to an untried API's ergonomics.

| Advertised abstraction | Awareness/use in wave15 | Opportunity; why skipped or what would improve fit |
| --- | --- | --- |
| `CheckResults.watchChecks` | Noticed and called. | Good fit for the independent default-auto check while owners worked. Its notice lost counts/source on one failed evidence read. Prefer a typed incomplete result retaining job and evidence path, plus an on-disk retry, before treating it as final proof. |
| `Routing.notifyReviewReady` | Noticed in menu; not called. | Provider and Engine candidates did need review, but I already had settlement replies and exact OIDs. An actor combining terminal candidate, source ancestry and review admission could save a round; a notice alone would not. |
| `TestEvidence.collectFocused/diagnoseFocused/finishFocused` | Called `collectFocused` after the watcher failure; no recorded `diagnoseFocused` or `finishFocused` call. | The source/count recovery was useful. A `finishFocused` path that preserves explicit evidence-read failure and optionally explains terminal assertions would have reduced manual extraction. I did not ask an API-help tool to implement this; reading the menu is not executing it. |
| `AssumptionWatch.watchAssumption` | Noticed in menu; skipped. | Final advertised names were a consequential assumption: `ask` was filtered, `sleep` present. I had not established a typed initial provider-list baseline or incorporated-source event. An easy `watchIncorporatedBaseline` example bound to the actual provider consumer would make it preferable. |
| `ParallelInvestigate.startProbeBatch` | Noticed in menu; skipped. | There were bounded read-only probes—`CliProvider.tools`, deterministic transport capture, final plan—before delegation. I inspected them ad hoc. A short recipe accepting actual checkout and named commands, returning source-bound results, might have avoided the `ask` correction; no probe batch was run. |
| `Interview.collectInterview` | Noticed in menu; skipped. | Interviews were required before retirement, but replies already arrived as typed settlements. A collector would help if it joined known and pending answers without additional turn-by-turn reads; I cannot attest to its runtime behavior here. |
| `PrepareContinue.verifyPrepared` | Noticed in menu; skipped. | `web/dist` preparation preceded standalone browser checks. I used a sequential gate; a completion-triggered continuation could have released my turn and refused a missing asset distinctly from a product failure. It needs a simple, demonstrably source-bound readiness function. |
| `RetainedEvidence.recoverRetained` | Noticed and called with 1024-byte budget on original job. | Correct custody fit, but HTTP 409 meant raw pages were unavailable. It did not repair the watcher summary; falling back to the evidence JSON was appropriate. Typed expiry and a recoverable evidence-path fallback would be better than a second manual step. |
| `SlowCommandWatch.watchSlowCommand` | Noticed in menu; skipped. | Long Cargo/Nix jobs invited one slow alert, but my short `write_stdin` loop was already underway. A single attach-to-existing-job call returning both slow and completion events would be preferable to polling, not another alert needing manual follow-up. |
| `HandoffExamples.handoffProposal` | Noticed in menu; skipped. | Final candidate/review/check/gate synthesis matched its purpose, but I had already assembled it manually and did not import this example. A projection that fails closed on mismatched candidate OIDs and missing check source would make it worth using. |

**Three wanted compositions (pseudocode, uncompiled).**

1. `focusedGate :: ExactSource -> FocusedSpec -> Memory -> Eff ... (Job, FocusedEvidence)` should start once, attach completion before I leave, and return actual matched/runnable/executed/failed counts, exit/cleanup, source, dirty-path snapshot, evidence path and typed `Unavailable` states. It could extend `TestEvidence.startFocused/collectFocused` plus `CheckResults`, rather than add a parallel test runner. A short callback might route `web/dist` missing to asset preparation or exit 137 with zero executed to a bounded resource retry; 137 is *not* proven OOM, and an expected-red assertion is *not* a pass. This would remove command-polling and evidence-extraction rounds. I still decide whether a failed product assertion demands repair.

2. `reviewable :: Base -> Candidate -> OwnedPaths -> RequiredSiblingOids -> Eff ... ReviewInput` should compare real HEAD, cumulative diff, merge preflight, ancestry and checkout source, returning an explicit partial-source refusal. It could sit inside the existing `Project.Work.reviewCommit` entrypoint or as its preflight, not create another reviewer router. The `59bf6236` false attribution is the concrete saved round. An actor is justified only if it can then receive repair candidates and re-run that preflight without waking me for unchanged facts; otherwise a notebook function is less setup. Independent judgment of production semantics remains with the reviewer and me.

3. `prepareThen :: Cmd.Job -> Readiness -> FocusedSpec -> Eff ... (Preparation, FocusedEvidence)` could compose `PrepareContinue.verifyPrepared` and the focused gate for `web/dist` plus `standalone_browser`. Missing assets would remain a prerequisite result; a product assertion would remain a product result. Its worktree must be explicit, since a green child checkout is not the integrated source. This removes a preparation-completion frontier round, not the final acceptance judgment.

**Inference, not measured savings:** a single cheap typed Jev call after authoritative parsing could choose among *prerequisite*, *build interruption*, *expected-red*, *product regression*, and *unresolved*, with an evidence excerpt and bounded next action. It should never infer source identity, counts, ownership, review approval, or integration from prose. No live Jev judgment of this composition ran in wave15. The setup burden was real: I knew the menu existed, but lacked a copied working root recipe for importing these modules, binding a real checkout/job, and handling evidence-read failure. The failed watcher reduced confidence in compact summaries, not in the idea of actors or Haskell composition.

**Strongest disagreement:** replacing the one-second polling with longer Bash waits alone optimizes waiting, not the expensive evidence/source/review frontier. Nor does one wave's low uptake imply low utility. Next wave, on one existing focused consumer check, exercise an improved `startFocused`→completion→`collectFocused` composition with an induced evidence-read failure. Require one retained job, exact source/counts or explicit unavailable state, and no model wake until a decision is needed; compare rounds against the manual path. Keep the reviewer and product acceptance gates unchanged.
