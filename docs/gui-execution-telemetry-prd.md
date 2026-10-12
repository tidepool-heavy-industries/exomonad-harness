# Execution visibility for the chat window

Status: proposed host/browser capability; not implemented by the GUI usability pass.
Base: Harness `bf11247b5fb881a5ba6624a0ab2e29b8f3adb8a4`.
Implementation owner: coordinator and host/runtime owners. The GUI branch owns presentation.

## User and outcome

The daily operator reads streaming Haskell, follows execution, answers inline forms,
and directs several actors. Each cell needs an expandable execution log, like a
chat's tool-use log, with meaningful effect names, phases, results, and timings.
It must answer “what is this cell doing?”, “what is it waiting for?”, and “where
did its time go?” without requiring the operator to interpret a protocol dump.

The log sits beneath the cell's source. A compact status remains visible when
collapsed. Expansion reveals ordered effects and phase durations; each effect
can expose arguments, results, diagnostics, and links to related displays/forms.
Prose and code remain readable during updates. Raw protocol inspection is a
separate disclosure. This is an observation capability, not a new execution API.

## Existing evidence and missing capabilities

The browser already receives exact tool invocation/result ownership, transient
model output, request/job timestamps, actor inference recovery state, and retained
forms/displays. `ActorOutputExecution::Notebook` supplies `execution`,
`inputUnitIndex`, and `effectOrdinal`; the origin includes run, native actor, and
incarnation. Reuse these identities. `ActorProgram` currently has no equivalent
per-effect identity. Neither projection supplies a complete effect trace or
compiler phase stream. Request durations must not be relabeled as cell timings.

The current GUI will expose existing observations only. A missing result means
“no loaded result”, not running, failed, or successful. A tool result publication
alone is not proof that its content represents success. Unknown associations
remain unassociated. No matching by label, code text, actor path alone, or time.

## Required observations

| Capability | Required information and behavior |
| --- | --- |
| Cell identity | Stable execution and input-unit identity scoped to the exact actor incarnation; explicit link to the issuing model request/call and retained source occurrence when applicable. Include an attempt identity for retries. |
| Actor-program effects | Equivalent stable execution/effect identity for effects evaluated outside notebook cells, with an explicit owner even when no conversation/cell exists. |
| Compilation | Host-reported phase transitions with meaningful labels, phase start/end, terminal outcome, and source-linked diagnostics where available. Expose only stages the compiler can actually observe; do not simulate a phase ladder. |
| Evaluation | Start, suspension, resumption, completion, failure, interruption, and cancellation observations; distinguish a control request from its settled outcome. |
| Effects | Every evaluated effect has a stable ordinal/identity, operation name, dispatch and settlement state, concise argument/result summary, and bounded expandable detail. Indicate parent/child relationships and overlapping work explicitly rather than implying serial execution from list order. |
| Waiting | Structured reason and owning execution/effect, with a reference to the form, child, tool/job, or external event being awaited where known. Distinguish waiting from inference recovery. |
| Output links | Exact references to retained displays, form mounts, tool results, and diagnostic detail. Link source spans only when explicitly supplied. |
| Timing | Host-measured elapsed durations per phase/effect and timestamp metadata sufficient for ordering and display. Report queue, compile, execute, and suspended time separately when measurable. Specify units and clock domain. |
| Availability | Distinguish unsupported instrumentation, not yet observed, history outside the loaded page, dropped/truncated detail, and incomplete traces. Indicate whether all effects for an execution are covered. |

Live elapsed counters may be browser estimates, labeled as such, and replaced by
host measurements at settlement. Reconnect must not reset elapsed time. Missing
timing is unavailable, not zero. Concurrent effect durations are not additive
wall-clock execution time. Effect arguments/results should use the existing
structured presentation system when possible; exclude credentials and preserve
existing authorization boundaries.

## Retrieval, updates, and pending forms

- Provide bounded actor/execution-scoped history plus live updates and a snapshot
  cutover. Use durable identities and revisions so replay and reconnect neither
  duplicate effects nor regress settled state. Specify ordering within execution
  and what can be ordered across executions; preserve overlap.
- Publish observations without allowing a slow/disconnected browser to block host
  evaluation. Bound event payloads, retained history windows, and client buffers;
  signal dropped observations explicitly. Keep terminal state recoverable.
- Support pagination and direct retrieval of an execution referenced by an older
  cell. Define retention/expiry and return explicit unavailable evidence after
  expiry rather than an apparently empty successful trace.
- Expose an authoritative actor-scoped pending-form index, including mount identity,
  revision, state, and a conversation locator. The browser must be able to locate
  a pending form outside its current bounded transcript. Update it immediately
  after submission/dismissal and reconcile it after reconnect.
- Rust owns DTOs, schemas, and generated browser validators. Native integer IDs,
  ordinals, and cursors retain the existing decimal-string conventions. Schema
  changes and matched browser/native bundle delivery belong to the coordinator.

## Acceptance scenarios

1. A streamed Haskell cell compiles, evaluates several effects, emits a display,
   and finishes. Its log presents exact associations and measured phase timings.
2. Compilation fails: source diagnostics appear at that cell, with no invented
   evaluation/effect activity. A later attempt has its own identity.
3. An effect opens a form and waits. The operator can find it while reading older
   history; submission becomes “awaiting processing” before the effect settles.
4. Effects overlap or a child runs asynchronously. The display preserves each
   lifetime and never reports the sum of durations as total wall-clock time.
5. Interrupt is requested while work runs. The log remains pending until a host
   observation establishes interruption/cancellation or another terminal state.
6. Disconnect during compilation, wait, or result delivery; reconnect reconstructs
   the same cells/effects, outcomes, form states, and timings without duplicates.
7. Two actor incarnations use identical labels/source. Events attach only to their
   explicit owner. A standalone actor-program effect is not attached to a nearby cell.
8. A long run exceeds payload/page limits. The UI reports incomplete coverage and
   can load available older details. Missing evidence never reads as success.
9. An uninstrumented host provides ordinary chat. The conversation remains usable;
   compiler/effect data is marked unavailable rather than fabricated.

## Delivery boundary

The host implementation should return its concrete schema, available compiler
stage vocabulary, retention/size limits, transport ordering guarantees, and
measured instrumentation overhead with tests for the scenarios above. Those are
implementation evidence to review against this PRD, not promises made by the GUI.
No replay, rerun, cancellation repair, pin change, or deployment is requested here.
