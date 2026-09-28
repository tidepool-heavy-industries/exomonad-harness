# Daily-driver adoption roadmap

Status: implementation scaffold, 2026-09-28. No acceptance or deployment claimed.
Goal: use Exomonad's main binary and browser to do real development waves through
one shared exomonad-harness instance, without a Codex process per model actor.
SSH remains an independent operator access path. Hardware provisioning proceeds
in parallel; it is not a reason to postpone the integration design.

## Evidence baseline and reading order

1. NEXT.md, this plan, PRD.md, then the relevant owning implementation.
2. The wave22 run's brief, NEXT and exact-source review/check artifacts.
3. Tidepool `plans/harness-adoption-reconciliation.md` and the original
   `plans/harness-adoption.md` (read-only historical design input).

Canonical harness inspected at `8224d1a8dc7c33d07b2861ad8ef6e6bd6f428aa1`.
Wave22 inspected at `1b72450`; this is a moving run checkpoint, not an accepted
release. Do not cherry-pick this plan's older implementation baseline over it.
Reconcile comments and paths against the accepted newer source during integration.

Verified from source in the canonical checkout:

- `cell_job.rs`: evaluator seam exists, but advertises a JSON function and uses
  CellInput. Wave22 owns its raw custom-tool replacement.
- `provider.rs`: the harness adds async tool declarations; its old TODO asking
  for that implementation is obsolete. Progress here is an unbounded channel;
  check later wave work before assigning a bounded-progress repair.
- `harness-demo/src/main.rs`: browser `serve` is deliberately deterministic;
  interactive `tree_ask` is a separate path. A live browser root is not yet proven.
- `harness-demo/src/tree.rs`: inherited/checkpoint starts are explicitly refused.
  History: `git log -S` locates that guard at `2ebdd30`.
- `transport/auth.rs`: CodexFileAuth reads credentials without refreshing them.
  History: introduced at `fff38ec`. This is not independent credential lifecycle.
- Engine/Store/mailbox, agent verbs, compaction, web transport and offline
  adapter-readiness tests already exist. Audit/reuse them before adding owners.

These are source observations, not newly executed checks. Wave22's current handoff
still has combined Engine/Store/custom-browser acceptance open. Recheck at freeze.

## Workflow and ownership contract (gate C0)

Trace one real operation before splitting implementation: browser input ->
operator identity -> agent mailbox -> model request -> custom Haskell call ->
resident admission/execution -> original-call result -> next model request ->
browser state. Repeat for a fork, cancellation, reload, and process loss.
Record the participating owner, stable identity, durable fact, next event and
failure handback at every boundary. Remove relay steps instead of reproducing
Codex's tmux/process protocol behind a new adapter.

| Concern | Owner to retain / integration responsibility |
| --- | --- |
| Model requests, history, call outputs, mailbox persistence | harness Engine, Store, transport and existing mailbox |
| Tools and hooks' meaning | embedding Provider; Haskell/Jev policy stays in Tidepool |
| Resident admission, execution, compiler/source snapshot | Tidepool actor workbench and runtime session owners |
| Worktree/source isolation and resource release | Tidepool managed checkout and resource owners |
| Agent execution and lifecycle | decide a single transition authority at C0; map harness AgentPath and Exomonad actor incarnation explicitly |
| Browser protocol, snapshots, retained detail | existing harness server and web protocol; embed into main binary |
| Host launch, configuration, prompts, effects, deployment | Tidepool facade composition root |
| Credentials | transport Auth boundary; secret storage/refresh policy explicit |

C0 artifact: a short ownership/transition table and compiling boundary scaffold.
Specify pending-call fork semantics, actor incarnation reuse, acknowledgment,
resource-release responsibilities and malformed-input behavior. Do not introduce
another scheduler, call registry, history log or shadow actor lifecycle.
Serialized changes require a version/migration decision; prefer clean internal APIs.

## Work packages and acceptance

### H0 — finish and freeze standalone custom cells

Owner: existing wave22 component owners and root. Reuse their candidates.
Deliver raw custom input, kind-aware Engine dispatch, Store/replay validation and
production browser progress/cancel/reopen. Preserve exact text including Unicode.
Malformed calls must produce explicit errors; stale/duplicate outputs must not be
accepted as another job's completion. Close component review plus combined gates.

Gate: accepted source and counted focused checks, baseline browser journey and
custom browser journey; record remaining uncertainty. No fake evaluator result
is evidence of real Haskell or live provider behavior.

### H1 — real resident execution (Tidepool integration lane)

Depends: H0 interfaces + C0 scaffold. Implement the existing CellJob/Provider seam
against resident workbench admission, not a second subprocess execution service.
Expose raw Haskell and existing typed tools from the actor's actual source view.
Preserve call identity and execution classifications, retained output and detail
references. Distinguish failed compilation, failed execution and unknown effects.

Gate: actual resident bindings persist across calls; a delayed original call can
settle while operator input is admitted. Cancellation during compilation and
execution has an observable outcome and verified resource disposition. Dropping
a harness future alone does not prove compiler, command or resident work stopped.
Late success cannot overwrite cancellation. Reuse offline readiness fixtures,
then substitute real resident execution rather than duplicating the harness loop.

### H2 — shared model-actor lifecycle (joint integration lane)

Depends: C0; fork acceptance also needs H1. Run all model conversations within one
shared harness instance. Keep GHC workers/commands as separately owned processes.
Use existing agent runtime/driver facilities in the accepted revision.

Connect fresh, inherited and checkpoint admissions to the committed conversation,
resident environment, source view, worktree and authority. A child cannot start
from a partially committed parent cell. Multiple children from the same boundary
must share the intended immutable prefix without sharing mutable authority.
Maintain exact typed request/reply, progress, parent messaging, independent review
and integration behavior through existing Haskell consumers.

Gate: one Sol root, component owner, and parallel leaves complete real work.
Demonstrate messages during pending work, follow-up to an idle actor, typed final
answers, exact-source review and integration, local retirement and resource release.
Exercise failed admission, cancellation and a parent stopping with children.
A retired actor is not merely a completed model request. Measure live/retired
memory versus the existing client-process baseline; do not promise constant-cost
parked actors or zero-copy conversation inheritance without measurement.

### H3 — main-binary browser/operator composition

Depends: C0; can prepare in parallel with H1/H2. Embed the reusable harness server,
assets, Store and live driver into Exomonad's CLI/composition root. Keep deterministic
standalone mode explicit and useful for offline tests. Do not silently turn demo
commands into credentialed inference or create another browser-only scheduler.

Provide conversation selection, messages, tool progress/results, child tree,
errors, interruption/stop and recoverable detail. Distinguish command admission,
execution and completion in the UI. Reconnect obtains authoritative state without
resubmitting commands. Reuse production rendering and truncation behavior.

Before network use, decide operator authentication: retain a working baseline
while reconciling shared-secret sessions with trusted Tailscale identity. Verify
session expiry/revocation, HTTP/WebSocket authorization, origin/CSRF handling,
and permission to inspect/control a run. Never trust forwarded identity headers
from arbitrary peers. Keep admin access and secrets outside disposable worktrees.

Gate: browser -> real model -> persistent Haskell -> result, then browser tree
operation. Browser refresh and Tailscale disconnect/reconnect retain state.
SSH remains usable. No public Internet exposure is needed for the first release.

### H4 — real transport and credential lifecycle

Depends: accepted tool/request shapes; can proceed alongside H1/H3. Audit current
transport against local pinned client source and retained provider evidence.
Prove async custom-call ordering, pending-call continuation, finalization,
stream interruption, error/rate-limit behavior and bounded retry. Reuse existing
redacted opt-in tracing; never retain auth headers/tokens in evidence.

Choose an approved credential source through Auth. An initial explicit
CodexFileAuth bridge is acceptable only with documented expiry and recovery;
independent login/refresh is required before claiming no Codex dependency.
Coordinate refresh across agents, surface reauthentication to the operator,
and avoid retry storms. Do not copy credentials to the new host implicitly.

Gate: live one-root and child checks using the intended account/model/effort,
plus deterministic replay of relevant failure cases. Distinguish mock, replay
and live evidence. Verify normalized provider requests/cache use rather than
assuming shared prefixes from local metadata.

### H5 — sustained sessions, restart and reload

Depends: H1-H4 integration. Exercise compaction with pending calls, child
obligations, typed final answers, retained evidence and surviving resident bindings.
Inspect existing compaction implementation before adding a strategy.

Define restart semantics explicitly: SQLite history is durable; arbitrary resident
heap state is not automatically restored. Mark interrupted/nonrecoverable work
honestly, prevent blind replay of side effects, and provide a deliberate fresh
session/recovery path retaining source, commits and evidence. Decide whether any
safe reconstruction is needed; transparent heap restoration is not a launch goal.

Reload helper/AgentSpec source through the existing atomic publication owner.
Rejected reload keeps the last valid surface. Identify which source version
running calls and children see, preserve authority, and expose changed tool
surfaces at coherent model-request boundaries. UI refresh, authored-source reload
and hot replacement of the host executable are different operations.

Gate: a long development session compacts, reloads helpers, reconnects, and
recovers from a stopped host without fabricated success or duplicate effects.
Include operator interruption under active tools and pending child requests.

### H6 — packaging, daily waves and cutover

Depends: integrated H1-H5 gates for the exercised scope. Build matched runtime,
extractor/worker, pinned harness dependency, workspace and browser assets through
Tidepool's build owner. Do not create an independently drifting deployed UI.
Declare service configuration, durable data, permissions, resource budgets,
backup/restore, startup readiness, orderly shutdown and schema compatibility.
Use OVH when ready; validate on a bounded run before moving existing workloads.

Start ordinary development waves on the new backend once a browser root and
small tree are accepted; do not wait for every future dashboard feature. Observe
roughly five completed waves before calling it the normal daily driver, repairing
between runs. Scale concurrency based on headroom and useful work, not a quota.

Each wave: exact revisions/pins, model settings, run/database/log IDs, useful
product outcome, one bounded orchestration experiment, root interview, and
actor/resource release evidence. Audit slow operations, rejected/confused cells,
lost obligations, useful/missed helpers, redundant frontier turns and Jev packet
quality. Reconstruct the whole workflow before choosing the repair.

Operator decides default cutover. Retain the functioning Codex path as a temporary
recovery option during trials; remove its process/relay/rollout dependencies after
a consumer audit. Do not wholesale-delete Haskell watches or fork APIs because an
old design assumed async tools replace every non-model-actor use.

## Suggested scheduling

- Now: H0 with its existing owners; server provisioning and configuration in parallel.
- Next: C0 contract/scaffold; read-only H4 audit and H3 UI/protocol assessment can start.
- Implementation: H1 resident lane, H2 lifecycle lane, H3 browser lane; one integration
  owner owns shared types and ordering. H4 transport/auth has explicit file ownership.
- Converge: one real root, then a small tree. H5 failure/reload/long-session work can
  be decomposed around independent owners. H6 begins real waves once scoped gates pass.

Use broad useful worker trees, explicit component joins, focused checks and one
expensive compiler slot on the current host. Independent UI/library checks can
run where resource budgets permit. No routine two-hour Tidepool gate per parcel.
Each owner reports exact candidate/base, dependencies, review disposition, executed
counts, compiled-only targets, failure evidence and remaining unverified behavior.

## Decisions to take with Inanna

- Actor transition authority and how paths map to incarnations (C0).
- Initial credential mode and independent login/refresh sequencing (H4).
- Operator identity/session policy and scope of browser control (H3).
- Acceptable loss/recovery of resident state across host restart (H5).
- First live tree's scope, default cutover and retirement of the Codex backend (H6).

User direction already settled: browser operation, Haskell as primary tool surface,
one shared harness instance, ordinary SSH, server provisioning now, no dependency
from the standalone harness back to Tidepool. These need no repeated approval.

## Later extensions

Read-only server/resource views, terminals attached to exact checkouts, service
management and artifact browsing can use the same typed capability owners.
A terminal or service-control module needs explicit authority and lifecycle;
do not add it as a bypass around command/resource ownership. Seamless binary hot
replacement, broad automation-hook expansion and dashboard polish can follow
real use. They are not prerequisites to the first useful browser wave.
