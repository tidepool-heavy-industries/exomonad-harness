# Daily-driver adoption roadmap

Status: accepted integration target and implementation scaffold, 2026-09-28.
No integrated acceptance or deployment is claimed. Embedded behavior is defined
by [the embedded-host contract](embedded-host-prd.md).
Goal: use Exomonad's main binary and browser to do real development waves through
one shared exomonad-harness instance, without a Codex process per model actor.
SSH remains an independent operator access path. Hardware provisioning proceeds
in parallel; it is not a reason to postpone the integration design.

## Current batch and settled defaults

The standalone library batch has converged wave22 and implemented the deterministic
embedding seams. See [the implementation handoff](embedding-ready-handoff.md)
for exact interfaces, revisions and offline evidence. Tidepool engine, concurrent
resident execution and real host checkpoint integration remain held pending its
separate investigation. No deployment or live cutover is claimed.

Initial operation uses the read-only Codex credential bridge and existing browser
session login behind Tailscale HTTPS. Independent refresh and Tailscale identity
login are later work. Codex remains the development default; no runtime pins,
backend selection, deployments or active sessions change in this batch.

## Evidence baseline and reading order

1. NEXT.md, this plan, PRD.md, then the relevant owning implementation.
2. The wave22 run's brief, NEXT and exact-source review/check artifacts.
3. Tidepool `plans/harness-adoption-reconciliation.md` and the original
   `plans/harness-adoption.md` (read-only historical design input).

This roadmap was reconciled against harness source `d36ef10`. Wave22's final
handback is a separate WIP convergence stream; read its run `NEXT.md` and
`docs/wave22-final-manifest.md`, then preserve each candidate, review and check
artifact separately. No component candidate or handback is an accepted
integrated release. Do not replace its source/evidence with this roadmap or
claim its open combined gate has passed.

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

## Workflow and ownership contract (gate C0, settled)

Trace one real operation before splitting implementation: browser input ->
operator identity -> agent mailbox -> model request -> custom Haskell call ->
resident admission/execution -> original-call result -> next model request ->
browser state. Repeat for a fork, cancellation, reload, and process loss.
Record the participating owner, stable identity, durable fact, next event and
failure handback at every boundary. Remove relay steps instead of reproducing
Codex's tmux/process protocol behind a new adapter.

| Concern | Owner to retain / integration responsibility |
| --- | --- |
| Model requests, history, pending call identities/claims and durable model-input envelopes | harness Engine and Store |
| Actor identity/incarnation, lifecycle, supervision, authority and typed live-value mailbox | Tidepool actor kernel |
| Tools and hooks' meaning | Exomonad's supplied tool manifest and provider; Haskell/Jev policy stays in Tidepool |
| Resident admission, execution, compiler/source snapshot | Tidepool actor workbench and runtime session owners |
| Worktree/source isolation and resource release | Tidepool managed checkout and resource owners |
| Agent execution and lifecycle | Tidepool is the sole transition authority; map each harness conversation to an exact actor incarnation |
| Browser protocol, snapshots, retained detail | existing harness server and web protocol; embed into main binary |
| Host launch, configuration, prompts, effects, deployment | Tidepool facade composition root |
| Credentials | transport Auth boundary; secret storage/refresh policy explicit |

C0 implementation artifact: compile the typed embedding boundary against a
deterministic host stub. Harness Store owns durable model-input envelopes and
their inclusion/claims; the kernel admits and authorizes those inputs. Do not
mirror the same delivery into Tidepool's native durable inbox. Keep exact
incarnation identity, typed live values, cancellation/cleanup evidence, and
resource release with their current owners. Do not introduce a second scheduler,
call registry, history log or actor lifecycle. Serialized changes require an
explicit migration decision; prefer clean internal APIs.

## Work packages and acceptance

### H0 — finish and freeze standalone custom cells

Status: joined and checked in the embedding batch; retained candidates and
combined evidence are listed in the implementation handoff.
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

Depends: C0 scaffold and shared execution identity. Run all model conversations
within one shared harness instance. Keep GHC workers/commands as separately
owned processes. Use existing lifecycle owners, not the standalone tree driver.

Connect fresh and checkpoint admissions to the exact conversation, captured
resident source/bindings, separately selected worktree revision and narrowed
authority. An explicit checkpoint may start children while its issuing cell is
pending and carries that cell's completed private scaffold; it remains valid if
the cell later fails. Ordinary unpublished cell bindings remain private. Multiple
children can share immutable context without sharing mutable authority.
Maintain exact typed request/reply, progress, parent messaging, independent
review and integration through existing Haskell consumers. Embedded model tools
come from Exomonad's supplied AgentSpec surface; do not expose the harness's
standalone lifecycle verbs as a second authority.

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

Initial operator authentication uses existing browser-session login behind
loopback/Tailscale HTTPS; trusted Tailscale identity login is deferred. Verify
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

Use the approved explicit read-only CodexFileAuth bridge through Auth, with
documented expiry and operator reauthentication;
independent login/refresh is required before claiming no Codex dependency.
Surface reauthentication to the operator and avoid retry storms. When independent
refresh is implemented later, coordinate it across agents. Do not copy credentials to the new host implicitly.

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
session/recovery path retaining source, commits and evidence. Automatic resident
heap reconstruction is excluded from the initial release.

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

## Settled decisions and later operator gates

The embedded-host PRD settles kernel lifecycle authority, exact-incarnation
binding, initial credential bridge, browser-session login, and honest loss of
resident state on restart. They are not open implementation choices.

The first connected acceptance is a browser Sol Medium root driving parallel
Luna component workflows, review and repair, integration and cleanup. That test
is separate from authorizing a production wave, provisioning or deployment.
Operator decisions still required later: default cutover after observed embedded
waves, removal of the custom Codex dependency after its consumer audit, and any
expansion to independent credential refresh or server-wide management.

## Later extensions

Read-only server/resource views, terminals attached to exact checkouts, service
management and artifact browsing can use the same typed capability owners.
A terminal or service-control module needs explicit authority and lifecycle;
do not add it as a bypass around command/resource ownership. Seamless binary hot
replacement, broad automation-hook expansion and dashboard polish can follow
real use. They are not prerequisites to the first useful browser wave.
