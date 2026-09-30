# Embedded Exomonad host contract

Status: accepted integration target (2026-09-28). Standalone C0/C1/C2
implementation and offline evidence are recorded in
[the implementation handoff](embedding-ready-handoff.md); joint runtime and live
release acceptance remain outstanding. This document defines how the generic
harness library is embedded
by Exomonad. It does not make the standalone crate depend on Tidepool.

Read with [the crate PRD](../PRD.md), which specifies the generic provider and
standalone demonstration, and the [daily-driver roadmap](daily-driver-plan.md),
which sequences Tidepool integration. If a generic or demo behavior conflicts
with this embedded profile, this profile governs embedded mode. The standalone
demo remains useful and may retain model-facing harness verbs for its own
operator-driven tree.

## Product target

One Exomonad process embeds one shared harness library instance and the Tidepool
actor kernel. A human operates a Sol root and its recursive Luna worker trees in
the browser. Actors use raw Haskell notebook cells and AgentSpec tools whose
handlers are compiled Haskell. The harness is a generic model and browser
runtime; it never imports or depends on Tidepool.

The first release is accepted only when a browser-operated worker tree completes
useful work. A single actor is an integration test on that path, not the product
milestone. Codex remains an independently usable fallback during trials.

## Ownership boundaries

| Responsibility | Owner in embedded mode |
| --- | --- |
| Actor identity/incarnation, lifecycle, supervision, admission, authority, typed live-value mailbox | Tidepool actor kernel |
| Model requests, conversation history, call identities, pending claims/results, durable model-input envelopes | Harness Engine and Store |
| Haskell compilation/execution, notebook bindings, source publication | Tidepool resident runtime |
| Commands, checkouts, resource custody and cleanup | Tidepool resource owners |
| Browser protocol, event stream, page and retained conversation presentation | Harness, hosted by Exomonad |
| Launch configuration and composition | Tidepool facade |

There is one authority for actor lifecycle and one durable owner for model-input
envelopes. The kernel admits and authorizes input; the harness Store records it
before the associated conversation is woken and includes it in the model
request. Do not mirror those deliveries into a second durable inbox. Typed
Haskell values remain live kernel messages, not serialized model envelopes.

Each embedded conversation is bound to an exact authorized actor incarnation.
Model-provided identifiers cannot choose another actor or grant authority. A
model final response, a typed assignment reply, delivery acknowledgment, and
actor retirement are separate events. The standalone `TreeDriver` and
`StoreAgentToolService` remain available to the demo, but do not supervise an
embedded Exomonad tree.

## Model-facing calls and pending work

The host supplies each conversation's actual tool manifest. Embedded mode must
not append mandatory harness `spawn_agent`, `send_message`, `wait_agent`,
checkpoint, or other lifecycle verbs. Exomonad exposes its authorized Haskell
surface through the existing compiled AgentSpec tool mechanism. Harness
transport, call tracking, streaming, and browser controls are internal library
interfaces. The standalone demo may expose its own agent verbs.

Every request and tool call retains its original identity through admission,
streaming, execution, cancellation, result publication, replay, and browser
inspection. Checkpoint snapshots can retain a pending call claim. When that call
later settles, its result is attached to that same call identity. Never invent a
success output to unblock a child or report an unfinished parent invocation as
complete. Unknown external effects remain unknown until their owner reports an
outcome; cancelling a future alone does not establish that a process or
resident execution stopped.

`Store::capture_checkpoint_cuts` captures two immutable capabilities in one
transaction from an exact original pending operation. The deferred cut includes
the boundary call and its original claim. The before-call cut ends immediately
before that item, with no invented output and no current-call claim. Both retain
the source operation as provenance; earlier calls and their original pending
claims remain dependencies. Later items in the same response or future requests
are absent. The embedding chooses the before-call capability only after its
own captured group commits; ordinary unfold keeps its completion fence and the
deferred capability. Provider history and the opaque runtime capture are
independent snapshots. Neither capability admits or supervises an actor.

Schema 6 wraps checkpoint host metadata with versioned cut provenance. Existing
records migrate explicitly to deferred cuts; reopening refuses unknown cut or
metadata versions and cannot restore a process-local host attachment.

Harness Store is the durable owner of model conversation envelopes and their
inclusion/claim state. Tidepool remains the authority for live actor messages,
typed values, execution, and resource control. Browser reconnect reads current
state; it does not resubmit work.

### Required library seams

Keep these in the existing Provider, Engine, Store, job and server owners. These
are behavior requirements for their public interfaces, not a second runtime:

| Boundary | Information and guarantee required by the embedding |
| --- | --- |
| Request construction | Exact conversation/request identity and host-supplied tool manifest/handler version; record the tool kind against the issuing request |
| Call admission | Original request/call identity, raw text or structured arguments without conversion, bounded progress sink and cancellation signal |
| Input delivery | Durable envelope ID, exact target incarnation, admission outcome and actual request-inclusion acknowledgment |
| Checkpoint capture | Immutable conversation prefix with pending claims, plus opaque host attachment; return a usable handle before the enclosing call settles |
| Child attachment | Host-admitted identity and checkpoint; register conversation history without independently admitting or supervising an actor |
| Result settlement | One terminal call result, retained detail reference and host execution classification; distinguish a lost waiter from a stopped operation |
| Browser control | Route control to the bound host owner; project its lifecycle and Haskell-only actors alongside conversations |

Cancellation first requests action from the execution owner. The scheduler must
not discard the only control handle by aborting a provider future and then
claim resource cleanup. Preserve a way to obtain the owner's completion,
acknowledged cancellation, or explicitly unconfirmed outcome. If completion
wins the settlement race, keep that completion; if cancellation wins, late
success cannot overwrite it. Browser/transport detachment alone does not cancel
the admitted resident execution. Exercise this with a deterministic host stub
whose external operation outlives a dropped result waiter.

The host attachment in a checkpoint is an opaque capability, not serialized
Haskell memory. Capture retains its source/scope leases while Store commits the
conversation reference. Failed capture releases its provisional attachment;
successful capture retains it independently of enclosing-call success.

## Concurrent notebook cells

An actor may have multiple raw cells or installed Haskell tools active at once.
This is required to make asynchronous calls useful. Each admission captures a
stable snapshot of the published Haskell declarations, bindings, source version,
and tool surface. A cell's intermediate definitions stay private to that cell;
the cell executes its own Haskell sequence in order.

At effect suspension, the execution yields its machine checkout so other
admitted cells and actor events can progress. Do not hold execution locks over
provider, command, or other external waits. The existing actor scheduler and
runtime remain the scheduling owners; do not add another scheduler or shared
actor-wide active-cell slot.

Only successful cell completion atomically publishes that cell's new bindings
and declarations. It publishes a delta against the current environment, never
its entire starting snapshot. If two successful cells publish the same name,
completion order decides which definition future admissions see. Existing
executions and checkpoints keep the snapshot they captured. Individually valid private declarations may conflict when joined; an invalid
public declaration join fails atomically. Publication has one runtime-owned
commit point ordered against cancellation: cancellation winning first prevents
publication; cancellation arriving afterward cannot undo committed definitions.
A failed cell or rejected join publishes no new definitions. Independently
retained captures survive, and completed effects and their receipts remain
retained and observable. Retrying a call reads its
recorded outcome; it does not rerun work to recreate a result.

Dependent work belongs in one cell or in a later cell admitted after its
producer completes. There is no implicit unresolved-name/future dependency
graph. A checkpoint made inside a cell can explicitly give a child that cell's
completed private scaffold before the cell itself returns.

## Checkpoints and launch context

An inherited spawn/unfold requires a runtime-issued checkpoint handle. Fresh
context remains a separate launch choice. A checkpoint is reusable for multiple
children and captures a specific model conversation prefix, pending-call
references, Haskell/helper source version, published notebook environment, and
the issuing cell's completed private bindings. It records provenance and the
authority context needed for a later admission. This freezes lexical resolution,
not mutable resources or arbitrary filesystem contents; their sharing and
lifetime contracts remain with their owners.

Checkpoint creation is an effect boundary. Once it succeeds, the checkpoint can
be consumed while its issuing tool call is still pending and remains valid if
that cell later fails. Do not wait for a made-up successful cell result before
starting the child. A Haskell workflow actor with no model conversation may
receive the checkpoint and launch model workers from it.

A checkpoint does not select or freeze a project checkout. Each worker launch
separately selects a checkout/revision and records that selection alongside
the checkpoint identity. This permits a reviewer to use the original
requirement context against the implementer's exact candidate revision. A
checkpoint does not grant its originator's whole authority; the execution
owner validates and narrows authority at admission. Captured mutable resource
handles retain their ordinary live-resource meaning; they are not deep-frozen.

The actor descriptor must continue to distinguish context origin, creator, and
supervisor. Keep captured scopes/source alive through their real consumers and
release them through existing custody owners. Persisting checkpoint metadata
does not make a resident heap restorable after process loss. Never replay
uncertain side effects blindly.

## Compaction, reload, and recovery

The initial embedded compactor uses plain text, following the familiar local
Codex flow: ask the model for a handoff summary, then seed the next request with
standing instructions, that summary, and a bounded selection of retained user
messages. Default the configurable threshold to about 50% of the context limit
to leave headroom. Keep the summary text as model-authored text; no typed
builder, structured handoff schema, renderer template, or prepare/commit mode in
this release. The provider compaction strategy remains injectable for later
experiments.

Compaction does not own or recreate live Haskell state. Preserve pending-call
identities, claims, typed mailbox obligations, retained evidence, and current
execution state separately from the summary. Prevent a compaction loop when the
summary does not materially reduce context. Do not report a compacted transcript
as a restorable heap snapshot.

An atomic source/AgentSpec reload affects future admissions. In-flight cells and
checkpoints retain their captured source and tool meaning. If reload fails, the
last valid published surface remains active. After host loss, durable history
and evidence may survive while live actor references and heap state do not;
mark unavailable work honestly and offer deliberate recovery.

Tool dispatch must honor the manifest/handler version exposed by the request
that emitted the call. Publish a reloaded tool surface at the next model-request
boundary; calls from an already issued request keep their original handler
meaning. A notebook cell captures published bindings when admitted under that
request's source view. Do not use an unrelated newer global tool declaration
to reinterpret an old raw or structured call.

## Acceptance

The embedded host is not accepted until one browser run demonstrates a Sol root
managing two independent Haskell-directed workflows, each with Luna delegation,
checks, exact-candidate independent review, typed completion, integration, and
deliberate resource cleanup. Exercise at least one repair across the two
workflows. The browser must show
model and Haskell-only workflow actors, pending work, progress, checkpoint
provenance, retained results, and escalation. Refresh/reconnect must be
observational and preserve the current run state.

Focused deterministic checks must cover:

- Concurrent suspended cells progress independently; different-name binding
  deltas compose, same-name definitions follow successful completion order, and
  old completions do not restore an old whole environment.
- Failed/cancelled cells publish no new bindings while retaining completed
  effect evidence. Cancellation and actual cleanup acknowledgment are distinct.
- A checkpoint starts a child before its issuing cell returns, survives later
  cell failure, contains the cell's completed private scaffold, and is usable by
  a Haskell-only workflow supervisor.
- The same checkpoint supports implementation and later review at a separately
  selected exact checkout revision.
- Pending raw and typed calls keep their identities through checkpoint,
  compaction, delivery, and reconnect, with no fabricated or duplicate output.
- Reload changes future admissions only; failed reload preserves the prior
  valid source/tool surface.
- Process loss distinguishes durable evidence from unavailable heap/live
  resources and does not replay uncertain effects.

Record exact source/workspace pins, run and log IDs, model settings, call
counts, observed concurrency, latency, memory, and cleanup outcomes. Separate
mock, replay, and live-provider evidence. These requirements are accepted
design; none should be described as implemented until its owning test and
integrated browser run pass.

## Delivery order

1. Converge and review the existing standalone custom-cell work separately;
   preserve its candidates and evidence, and do not confuse component tests
   with integrated acceptance.
2. Agree the ownership table and scaffold typed host boundaries before parallel
   implementation. A deterministic harness host stub must exercise the public
   embedding seams without importing Tidepool.
3. In parallel after shared contracts exist, implement resident cell isolation
   and publication, actor/checkpoint lifecycle, and browser embedding. Keep a
   single owner for shared types and each registration surface.
4. Integrate the exact reviewed revisions, run focused failure-path checks, then
   run the full browser tree acceptance before moving ordinary waves.

Initial deployment remains one Exomonad process per run, using existing
machine placement, credential and authenticated browser owners. Server-wide
multi-run hosting, browser terminals, broader server-management modules,
automatic heap restoration, structured compaction, and hot replacement of the
host executable are later work. Stock Codex stays available as an independent
fallback during trials; removal of custom-fork dependencies follows a consumer
audit and accepted browser waves.

## Implementation handoff — 2026-09-28 contract pass

The earlier contract pass changed documentation only. Its inspected harness source:
`6326ef680dc5824d60f321f34d2c6c79aaf7eea4`; Tidepool:
`b2de366b2b52e9120ed07f200bae62501b5681e6`. At that point wave22 candidates were
unjoined; their subsequent join and evidence are recorded in the implementation
handoff.
Tidepool compiler, resident scheduling/publication and checkpoint implementation
are held pending its engine investigation. The Tidepool companion
`plans/harness-integration.md` carries the production-consumer/dependency map and
integration parcels. These requirements do not claim current API acceptance.

### Source-grounded library deltas

| Existing owner | Required change and acceptance |
| --- | --- |
| `provider.rs`: Provider, CallContext, all_tools | Preserve raw text/structured arguments without conversion; embedded calls require exact request/call provenance, bounded progress and externally acknowledged cancellation. Override the existing tool-list hook rather than append mandatory native verbs. |
| Engine and Store input/call owners | Persist envelopes before wake, distinguish admission from actual request inclusion, preserve original claims/results across detach and retry. Expose retained detail and honest execution outcomes; no second provider-side job registry. |
| Agent lifecycle and server control | Attach conversations to host-admitted actor identities; route stop/interrupt through the host. Do not run demo TreeDriver/StoreAgentToolService as a competing embedded supervisor. Include Haskell-only actors in the host-backed browser projection. |
| Checkpoint owner | Retain pending-call claims and opaque host attachment independently of enclosing-call success. Attachment failure releases provisional ownership; host loss makes stale live attachments unavailable. Immediate capture is gated on Tidepool runtime work. |
| `compaction.rs`: Compactor and Engine strategy | Add the agreed plain-text summary operation through existing transport/Store owners. Current server-compaction endpoint and forced typed-turn capability are not evidence that text summarization exists. Preserve raw and structured pending calls outside summary text. |
| `transport/auth.rs`: CodexFileAuth | Retain the explicit read-only credential bridge for first release; report expiry and preserve outstanding work instead of retrying indefinitely. Independent login/coordinated refresh comes later. |
| `server.rs`, `server/ws_protocol.rs` | Reuse Router/assets/session authorization and origin checks. Reconnect reads snapshots/events without replaying commands; event retention gaps require authoritative resynchronization. |

No Tidepool dependency is introduced into this library. The deterministic host
stub must implement its public embedding interfaces and exercise the real
Engine/Store/server, not duplicate their state machine in test code. Return the
actual accepted public interface inventory with the implementation hash; the
Tidepool adapter will consume that revision rather than guessed signatures.

### Settled initial operator profile

- One shared harness instance per Exomonad run. Codex remains the development
  default; embedding is selected explicitly for a new run and recorded in run
  metadata. No automatic backend switching or cross-backend side-effect replay.
- Existing browser-session login behind loopback and Tailscale HTTPS, with
  HTTP/WebSocket authorization and origin checks. Retain the eight-hour session
  default and explicitly configured public scheme. Do not infer identity from
  untrusted forwarding headers. Verify expiry and secret rotation in acceptance.
- Explicitly configured read-only Codex credential source; the operator uses
  Codex to refresh credentials when needed. Do not copy credentials implicitly.
  Stock Codex remains installed, and independent credential lifecycle is a later
  milestone rather than a first-release gate.
- The facade supplies `run_root/harness/` under its existing private durable run
  root. Store owns schema/migrations, the facade owns run identity and startup;
  assets are immutable matched build artifacts. Incompatible schemas fail before
  actor admission. No automatic Codex transcript migration or heap restoration.
- Plain-text compaction at approximately 50% configured context capacity:
  standing instructions, model-authored handoff and bounded recent user messages.
  Pending claims and live resources remain separate. Failure/no-progress retains
  valid history and prevents repeated compaction of unchanged input.
- Initial host commands use existing confined Tidepool command/resource owners:
  closed/piped stdin, bounded retained output, original offsets and explicit
  cleanup outcomes. PTY requests are unsupported until that owner adds support;
  do not silently substitute pipes. Browser terminals remain deferred.
- Host readiness means configuration/schema/assets/tool host/server are ready;
  it does not certify a provider request. Shutdown seals admissions and retains
  unresolved work/cleanup evidence before closing shared services. A crash never
  turns retained history into proof of live resident-state recovery.

### Required handback and acceptance

C0/C1/C2 below are integration contract parcels, not the roadmap H1-H6
product milestones. C1 provides library seams; roadmap H1 is the later real
resident integration. C2 contributes to transport and sustained-session milestones.


1. **C0:** converge raw-call provider, Engine, Store/replay and browser candidates.
   Report exact accepted hash, component joins, counted combined checks and
   remaining uncertainty. Preserve the wave22 manifest as WIP evidence.
2. **C1:** implement the required library seams against a deterministic host.
   Cover raw Unicode/structured input, kind mismatch, exact retry, bounded
   progress, input admission vs inclusion, dropped waiter and completion/cancel
   races. Extend the production `tests/adapter_readiness.rs` foundation.
3. **C2:** plain-text compaction and credential failure behavior, preserving
   pending call kinds/claims; no tools or effects replayed to rebuild history.
4. **Joint integration:** once Tidepool's runtime hold clears, prove real resident
   execution and checkpoints, then browser operation and the full reviewed worker
   tree specified above. A simulated evaluator is not real Haskell acceptance.
5. **Compatibility/release:** Tidepool checks Codex launch/input/tools/commands/
   fork/publication/cleanup alongside embedded tests. Record matched runtime,
   worker, workspace, harness and asset revisions. Default cutover is separate.

Each handback includes base/full commit, exclusive commit range, dependencies,
owned interfaces, exact check commands and matched/executed counts, compiled-only
work and unverified behavior. Distinguish mocked, replay, resident and live-provider
evidence. No deployment or new wave is authorized merely by completing these docs.
