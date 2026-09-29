# Embedded Exomonad host contract

Status: accepted integration target, not an implementation or release claim
(2026-09-28). This document defines how the generic harness library is embedded
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

Harness Store is the durable owner of model conversation envelopes and their
inclusion/claim state. Tidepool remains the authority for live actor messages,
typed values, execution, and resource control. Browser reconnect reads current
state; it does not resubmit work.

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
executions and checkpoints keep the snapshot they captured. A cell that fails
or is cancelled publishes none of its new bindings, but completed effects and
their receipts remain retained and observable. Retrying a call reads its
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
authority context needed for a later admission.

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

## Acceptance

The embedded host is not accepted until one browser run demonstrates a Sol root
managing two independent Haskell-directed workflows, each with Luna delegation,
checks, exact-candidate independent review, at least one repair path, typed
completion, integration, and deliberate resource cleanup. The browser must show
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
