# Build the workflow as you work

Exomonad's Haskell notebook is a place to develop the orchestration for this
project. Keep useful values, compose effects, and turn repeated work into small
functions and actors that you and your children can reuse. The aim is useful
parallel work with fewer model rounds spent relaying or reconstructing evidence.

## A test helper worth inheriting

Start from a real recurring task: edit a component, run its focused tests, inspect
failures, and decide what deserves attention. Customize a helper around that
component's target, expected test count and failure modes. `runTests` is a useful
name for a project-owned function; it is not a promised built-in API.

A good composition has these stages:

1. **Select:** accept the component, source identity and intended check. Resolve
   the actual package, target and filter. In this repository,
   `scripts/cargo-focused-test` retains the selected executable and count evidence.
2. **Execute and retain:** use `Cmd` when command results feed later computation.
   Keep exit status, actual executed counts, output completeness and artifact
   paths. Await an existing execution instead of rerunning to retrieve output.
3. **Interpret:** supply Jev the task intent and relevant diagnostic excerpts.
   Ask for comparable alternatives such as implementation failure, fixture issue,
   missing prerequisite, or insufficient evidence. Keep full logs recoverable.
4. **Branch:** code handles known exit/count/availability conditions. Jev can
   choose which evidence deserves inspection or which authorized diagnostic to
   run. Bound further commands and stop when evidence is missing or ambiguous.
5. **Present:** return a compact typed result containing what ran, what happened,
   source and log references, the semantic judgment, and any unresolved question.

For example, one cell could run the selected tests, preserve their logs, classify
an unexpected failure and extract the relevant assertion plus nearby context.
The next model round then receives a useful debugging packet. Jev does not
establish that tests passed, authorize edits, or supply evidence missing from a
truncated log. A classifier failure still returns the command evidence.

Begin with explicit parameters and a small behavior you can exercise. Test a
success, a real failure, and missing evidence before depending on it. Change the
helper when the task teaches you something; do not build a generic framework
before there is a second consumer. A plain shell command is appropriate when
there is no retained state, semantic judgment or effect composition to add.

## Grow and pass down the scratchpad

Notebook definitions are immediately useful for local iteration. Repeated,
related definitions belong in an authored module with explicit imports and
available-effect constraints. Use the source publication mechanism available in
your installed host; a module edit alone does not publish new definitions.
If `reload_helpers` is exposed, `.exomonad/helpers/` is the session helper
surface. Otherwise use the existing workspace module/reload workflow. Treat
reload failure as failure and keep using only a known valid revision.

Give a child the helper's name, purpose, parameters and relevant source revision.
Verify the selected fork mode carries the definitions or modules it needs;
fresh context must not depend on an omitted explanation. A parent's later edits
do not update existing children automatically. Deliver changes explicitly and
ask affected consumers to identify what they incorporated and checked.

When an import fails, retain the exact compiler error and distinguish the module
file in the checkout from the published helper layer and the actor's import
roots. If `reload_helpers` is available, first establish that the intended module is
in this consumer's bound helper branch at its module-relative path. Reload
publishes that branch; a root-layer publication does not update it. Publish the
intended revision and retry the import once. If it still fails, report both observations and use
the direct operation while the owning mechanism is investigated. Do not copy
helpers into another source subsystem to hide a publication failure. A root
revert does not revert a child's inherited dirty snapshot.

Count reuse only when a consumer imports and executes the helper. Report actor,
helper revision, test target/filter, selected/executed counts and command effects
separately from model tool calls. A missing test is a selection failure; a Jev
service failure leaves deterministic command evidence usable. Neither becomes
a passing judgment.

Seeds are invitations to customize: replace generic test selection, choose useful
Jev questions, and adjust output to the component's debugging needs. Keep an
escape hatch to the original evidence. Promote a helper into shared project
code when its consumers justify that ownership; a session experiment need not
become a permanent subsystem.

## Let actors carry repeated coordination

Use installed routing and review modules before writing a custom protocol.
A useful coordinator retains the current candidate, its review basis and the
repair owner. It can route review findings to that owner, request review of the
revised exact commit, and notify the integration owner of acceptance or a real
question. Limit repair attempts and escalate with the accumulated evidence.
Keep shared design decisions and final integration with their assigned owner.

Questions must reach an owner while the request is pending. Findings-only work
returns findings. Settlement, acceptance, incorporation and integration remain
separate facts. Exercise rejection, stale candidates and a question during
repair as well as a successful path before trusting automatic continuation.

Choose useful parallel obligations: implementation, failure-case tests, consumer
inspection and independent review. Add depth when a component owner can absorb
its children's coordination. Judge delegation by overlapping work, useful Luna
contributions and defects caught; the root may still do substantial integration.

## Design by executable experiment

For a consequential orchestration question, discuss alternatives with the user
and make one small version executable. A typed record actor or notebook
composition can reveal a missing event, a mistaken state distinction, or a
needless model round before a larger design is committed.

Each run normally includes one bounded experiment. Write down its hypothesis,
workload, success evidence, stopping condition and fallback. Examples include
reusing one customized evidence helper across several children, or letting an
actor handle one complete review/repair cycle without root relay. Count actual
calls, useful overlap, interventions and failures; separate structural savings
from measured improvements. Keep the resulting helper and evidence available
for the next iteration, including cases where the simpler workflow worked better.

## Evaluate automation in use

For an Astra-led evaluation wave, delivery and interface evaluation happen
alongside one another. Read the installed AgentSpec and relevant helper source
before choosing the initial workflows, then test those judgments during real
product work. Inspection can find confusing contracts; execution establishes
whether the interface actually helps its users.

Use parallel feature-area worker trees with Luna implementation and review work.
Each feature owner absorbs its own children's coordination and integrates its
component. Depth is useful when it removes work from the parent or enables
independent progress. Root ownership of shared decisions does not require root
relay of every child result.

Try several forms where applicable:

- Use a helper directly to establish its behavior and evidence boundary.
- Give a child the same published helper and see whether the brief suffices.
- Reuse a context-specific composition across sibling assignments.
- Put a callback or actor between a completion event and the next useful action,
  so the frontier model receives evidence or a decision request already prepared.
- Compare with a simpler operation when the helper's setup is disproportionate.

Keep a compact opportunity record in the existing automation trial/friction
artifacts. Record the actor, task, helper and revision, original command or call
references, attempted composition, outcome, remaining manual steps, and suggested
change. Distinguish unavailable publication, discoverability, API friction,
implementation defects and a genuinely unsuitable task. An unused helper without
an applicable opportunity is not a failure. Keep helpers exposed for three waves;
repair broken paths and prompting before concluding that non-use means no value.

Ask workers before retirement: What repeated work did this remove, and what work
remained? What would you change about the interface, or why did you skip it?
The root synthesizes these answers with its own source review into a detailed
closing critique: strongest examples of value, failed compositions, missed
opportunities, and prioritized changes with concrete proposed interfaces.

Measure actual model rounds separately from tool calls and command jobs. Setup,
recovery, notices and diagnostic calls all count toward cost. A plausible avoided
turn is a hypothesis unless the trace supports it; no savings percentage from
helper-use counts alone. Preserve raw evidence without copying full logs into
every child's context. Product acceptance remains separate from the quality of
the orchestration experiment.

## Reduce root coordination across feature trees

The Astra root's coordination cost is a primary experiment outcome. Give each
feature owner a complete local loop and a clear escalation boundary before
forking. A routine event should update retained state or trigger an authorized
continuation; it should reach the root when the root has something useful to do.

| Repeated root work | Composition to try | Root receives |
| --- | --- | --- |
| Read every child's progress and retell it | Local `Project.Routing.followWork` with a task-specific `WorkSink`; retain full snapshots and choose notification deltas | New decision, integration-ready slice, or component result |
| Start checks, remember handles, collect several outputs | The validated acceptance-plan composition with one candidate and retained per-check evidence | Aggregate terminal result including refusals and unknowns |
| Relay review findings, repair requests and revised candidates | Validated ReviewFlow with a named repair owner and bounded attempts | Reviewed candidate or unresolved contract question |
| Read failure logs and choose the first diagnostic command | Validated completion-driven investigator with supplied probes | Original failure plus bounded diagnostic evidence |
| Reconstruct state after a missed notice or notebook failure | Read the retained router/check/job handle and source receipt | Current evidence without resubmitting work |
| Ask every worker the same closing questions manually | Supply the feedback contract in assignments and collect answers before owner-authorized cleanup | Local synthesis with links to the original answers |

Use only workflows confirmed available in the launch brief. A proposed API is
not a callable tool. Prefer extending the existing sink or callback to creating
another coordinator that owns the same state. Do not send both automatic and
manual notices for the same event unless the latter adds a needed decision.
Keep notification failures and unresolved work visible in retained state.

Choose different useful compositions for different feature areas and explain the
choice. Record root interventions by purpose: shared design, integration, manual
relay, status recovery, or automation repair. This classification belongs in the
retrospective and compact retained notes, not a mandatory message on every event.
Inspect representative event sequences to establish what automation actually did.
A quieter root is useful only if work advances and questions still reach owners.

The closing critique should name the best composition, its setup cost, where it
broke down, and the next concrete interface change. Retain unsuccessful attempts;
compare their actual work and prerequisites before attributing differences to a
helper or a model tier.

### Coordinate baseline changes and review without root relay

A feature owner may receive a new shared baseline while its children still work
from older source. Track the desired baseline separately from delivery,
acknowledgment, actual incorporation and checked resulting commits. Use the
existing accepted-decision/amendment and incorporation contracts to carry these
facts. Coordinate dependent work in dependency order; independent owners can
incorporate in parallel. Preserve dirty work and escalate conflicts that change
shared semantics instead of silently choosing a resolution.

A changed candidate needs evidence for that candidate. Re-evaluate affected checks
and exact-source review after incorporation; retain earlier evidence as earlier
evidence. Do not queue a new incorporation request behind the very delivery that
is waiting on the change: steer its active assignment through the supported
update path and require incorporation evidence. The integration owner remains
responsible for the final combined source. Build the first automation around an
actual baseline change, with named affected consumers and an explicit fallback.

For automated review, name the exact basis/candidate, repair owner, attempt bound
and escalation recipient. Routine within-contract repair can go straight to its
owner. A disputed invariant, changed ownership, incompatible baseline or exhausted
repair budget reaches the parent with findings and alternatives. A review pass
is not integration, and a changed candidate does not inherit its old verdict.

### Nudge for a specific better workflow

Useful nudges offer a different operation at the moment it fits. Examples:

- A feature owner has several independent ready tasks but is doing them serially:
  suggest the concrete forkable obligations and the installed coordination entry.
- An actor repeatedly reads the same pending result: point to its retained handle
  and the available completion-driven composition.
- A parent manually relays review findings: suggest the installed review/repair
  coordinator with the current repair owner and escalation boundary.
- Siblings repeat the same check/read sequence: suggest publishing one contextual
  helper and passing its module, inputs and source with the next assignments.

Treat these as hypotheses grounded in a bounded event slice. Code suppresses
repeated nudges for the same episode; use Jev only if deciding whether a pattern
fits requires semantic judgment. Send one specific suggestion with its evidence,
callable alternative and expected benefit. An actor can explain why it does not
fit. Do not repeatedly nag, interrupt a critical operation, or count acceptance
of a suggestion as successful automation. A quiet actor may simply be waiting on
correctly routed work. Trial a few actionable triggers before broad coverage.

### Use role instructions for semantic invariants

At assignment construction, put stable role/feature invariants in the supported
instruction surface and changing source, authority and work in the typed task.
Examples include preserving original command evidence, treating missing checks
as unknown, keeping candidate/review identities exact, and escalating a semantic
conflict to its owner. Keep these concrete enough to guide a decision.

Verify how the installed host renders role guidance into provider messages before
claiming developer-message delivery. An ordinary notification is a notification;
it does not become a developer instruction by saying so. Preserve the shared
instruction prefix and the role contract when adding guidance: nested
`withInstructions` calls replace rather than concatenate the previous override.
Compose required role text deliberately and verify the actual starting packet.
Changed decisions for an active child require supported request updates and
incorporation evidence, not only an edited prompt file.
