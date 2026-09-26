# Agent automation menu

Use an entry when its named module is present in the actor's bound project
source. A commit or this menu alone does not install a module into a live run.
Start with the compiled `Project/FocusedGateExample.hs` source for a complete
focused-check workflow. Customize its inputs or callbacks in SessionHelpers;
pass the callable helper and published source to children.
Wait for completion and then handle its verdict; waiting specifically for green
can turn an ordinary failure into endless polling. Unknown evidence is a result
to investigate using the retained job.
Keep the original response, `Cmd.Job`, source OID, or evidence path so a compact
notice remains traceable. Choose useful helpers; no entry is mandatory.

For qualified `Project.TestEvidence` calls, first `import Project.TestEvidence`.
`Project.CheckResults` re-exports its unqualified names but does not bring that
module qualifier into scope.

| Need | Callable entry and inputs | Result and compiled example |
| --- | --- | --- |
| Let focused checks finish while working | `Project.CheckResults.watchChecks`: owner, `NoticePolicy`, named `FocusedRun` values. Start each run with explicit `Cmd.Memory` through `Project.TestEvidence.startFocused` or `startFocusedIn`. | One completion watcher retains raw command receipts, parsed count/source evidence, and notice attempts; `readChecks` and `checksSummary` inspect it. `Project.CheckResultsChecks.completionRouting` exercises late, failed, dirty, missing, and mismatched results. |
| Know when a child has submitted reviewable source | `Project.Routing.notifyReviewReady` as a `followWork` sink for terminal `Outcome Candidate` events. | An exact terminal source notice; progress checkpoints and unbound or mismatched sources do not claim readiness. `Project.RoutingChecks.reviewReadiness` is the focused example. Use the existing review request after deciding to review. |
| Diagnose a failed focused check | `Project.TestEvidence.collectFocused`, then `diagnoseFocused` on its `FocusedResult`. | Deterministic selection, setup, assertion, runner and source branches, bounded retained excerpt, and existing code reflex. `finishFocused` optionally asks Jev for a failure explanation; neither Jev nor the excerpt determines a pass. `Project.CheckResultsChecks.completionRouting` exercises the branches. |
| Track an assumption that may change | `Project.AssumptionWatch.watchAssumption`: owner, initial typed value, event source, projection and decision callback. `watchIncorporatedBaseline` covers a child's incorporated source. | Bounded before/after history and decision with a retained notification receipt; an unresolved judgment is visible. `Project.AssumptionChecks.changes` exercises deterministic callbacks, including unresolved decisions. `Project.AssumptionExamples.semanticImpact` is a compiled Jev policy example; live Jev behavior has not been validated. |
| Gather independent read-only evidence | `Project.ParallelInvestigate.startProbeBatch` and `observeProbe`: named caller-supplied commands, absolute checkout, memory and `ProbeLimits`. `selectProbes` is optional name lookup before the batch. | Original jobs, refusals, bounded observations and explicitly unrun probes. Optional `chooseNextProbe` selects only among supplied probes. `Project.AutomationRuntimeChecks.commandCustody` and `checks/automation-helper-contract.hs` exercise the boundary. |
| Collect answers before retiring workers | `Project.Interview.collectInterview`: supplied `KnownAnswer` or `AwaitAnswer` items. | Typed answered, waiting, cancellation-pending or unavailable findings; `interviewComplete` refuses empty or incomplete input. `Project.InterviewChecks.collectAnswers` is the focused example. |
| Continue when a prerequisite finishes | Start and retain the preparation `Cmd.Job`; use `Project.PrepareContinue.verifyPrepared` inside `R.on (Cmd.completion job)` with a readiness function returning a typed value. | The continuation receives that value after exact terminal receipt, clean exit and readiness checks; failure keeps the original run receipt. `Project.PrepareContinueChecks.preparationCompletion` compiles and exercises late, failed and unready paths. A long job never requires a foreground polling loop. |
| Recover output from an existing command | `Project.RetainedEvidence.evidenceBudget`, then `recoverRetained` on the original `Cmd.Job`. | Original status and raw stdout/stderr pages with typed EOF, current end, budget, incomplete-page or refusal stops. Each stream has an explicit byte budget; no command is resubmitted. `Project.PrepareContinueChecks.preparationCompletion` checks a failed job and a four-byte cutoff. |
| Notice a command that remains slow | `Project.SlowCommandWatch.watchSlowCommand`: owner, context, existing `Cmd.Job`, observation threshold from attachment (0..30000 ms), diagnostic character limit and callback over typed observation. | At most one slow alert after that bounded observation, then the same job's later completion; the threshold says nothing about the job's prior lifetime. The callback may use Jev without changing job status. `Project.AutomationRuntimeChecks.commandCustody` exercises cross-actor custody and `checks/automation-helper-contract.hs` checks the typed API. |
| Assemble a compact handoff | `Project.HandoffExamples.handoffProposal`: reported candidate, optional reported review, observed check state and remaining obligations. Import the example module explicitly and customize its projection. | Reuses existing summaries without granting integration authority. `Project.AutomationChecks.integration` compiles and exercises this projection. |

Ordinary exact-candidate review requests remain in `Project.Work`, with
event-driven collection in `Project.Routing`. Use `Project.ReviewFlow`
for a bounded component after the launch brief confirms its runtime gate: declare component-only review or required sibling
commits before requesting review, retain root integration authority, and escalate
unresolved contracts. The compiled `checks/review-flow-loop.hs` consumer shows setup, including
`R.withWorktree` authority; `Project.ReviewFlowChecks` exercises acceptance,
repair, missing sibling source and empty-findings correction.
`Project.BrowserScenario` remains unavailable pending its isolated browser check.

For conditional diagnostics, `Project.ParallelInvestigate.followFailure` observes
the original job and lets Jev choose at most two supplied `CommandProbe` values.
`followFailureWith` accepts a deterministic selection function instead. Each
result retains the original outcome, diagnostic observations and typed stop;
unknown judgment stops without guessing. Invoke it from a completion handler; a
pending original remains unchanged for its existing completion route. Diagnostic
jobs use completion-oriented observation. Shared original jobs do not grant
authority to arm their owner's notice.
The caller supplies read-only commands; this is not an extra sandbox guarantee.
No original command is retried. `Project.FollowupChecks.bounded` is the example.

For preparation followed by a check, `Project.WorkflowExamples.prepareFocused`
composes `verifyPrepared` with `startFocused`: the readiness callback returns the
actual `FocusedSpec`, and the resulting original job can feed `watchChecks`.
Use it from the preparation completion handler. Paths do not grant authority;
run checkout-sensitive preparation in the actor that owns that checkout.

## Trial record

`docs/automation-trials.json` records implementation separately from exposure.
Record `first_exposed_wave` only after a launched wave actually receives this
menu with its bound callable modules. Each validated entry stays available for
at least three exposed waves, including an initial wave with no use. For every
wave, record applicable opportunities, discovery, attempted calls, outcome,
fallback, source/evidence references and token or attention measurements when
available. An empty use count alone does not justify removal; inspect discovery
and opportunities first. A known broken path stays unavailable while repaired.

## Choose an opportunity, not a helper quota

For repeated focused checks, specialize `SessionHelpers.CheckDefinition` and pass
the committed candidate to `runCheck` each time. For browser acceptance,
`runBrowserCheck` prepares assets and runs that definition in the same original
job and checkout. The watcher reads retained evidence and sends the terminal
notice; it does not acquire write authority to the owner's checkout. The command
records prerequisite failure separately and never starts the test after it.
Dirty-source evidence remains unknown even when execution passes.

Use `verifyPrepared` when a continuation already has authority over its next
operation. Use the single-job composition when preparation and test both need
the invoking checkout. The pinned Node environment imports only flake.nix and
flake.lock, avoiding copying a warm target directory into the Nix store.

- Diagnostics/probes: use for unresolved causes or competing bounded probes;
  skip Jev when a missing prerequisite or assertion already identifies the action.
- Assumption/slow watchers: use for a changing assumption or an actionable delay;
  skip additional alerts when completion alone is sufficient.
- Interview collection: use when replies remain outstanding across a larger tree;
  direct retained answers are sufficient for a small completed set.
- Handoff projection: use for recurring summaries; it does not establish acceptance.
- Review readiness: skip a second notice when the settled candidate already supplies
  everything needed. ReviewFlow still needs a successful activation smoke check
  before the next live trial.

Record applicable non-use as well as attempts. Keep the three-wave trial window.
Avoid explicit short yields and repeated empty write_stdin calls for batch work;
use one completion route and read the original job when a decision needs it.

## Wave18 compositions

Read the production modules first; open a checks module only to answer a specific
question. These compositions preserve the individual operations above.

- **One candidate, several checks:** `SessionHelpers.AcceptancePlan` supplies
  `standaloneChecks` and `startAcceptancePlan`. Customize the list to the affected
  product boundary. `startCheckPlan` keeps each original job and setup refusal;
  `readCheckPlan` aggregates preparation, source, counts and retained output.
- **A failed job, one investigation packet:**
  `SessionHelpers.BrowserInvestigation.startBrowserInvestigation` starts browser
  preparation/checking in the invoking checkout and attaches its sole investigator.
  The example uses deterministic probe selection. Customize `watchFailedCheck`'s
  supplied read-only probes and selection callback when semantic choice helps.
  `readInvestigation` retains the original failed check as well as diagnostics;
  `finishInvestigation` refuses to retire pending work.
- **Ordinary repair without parent relay:** `Project.ReviewFlow.reviewFlowWith`
  accepts a bounded semantic policy over the exact candidate, findings and repair
  budget. `semanticReviewChoice` chooses within-contract repair or escalation;
  uncertainty escalates. It cannot turn a repair decision into acceptance.
  Follow the compiled `checks/review-flow-workflow*.hs` sequence for retained
  interview evidence and owner-authorized cleanup after the flow settles.
- **One baseline change, several active owners:**
  `Project.BaselineIncorporation.openBaselineEpisode` creates the collector before
  forks. `lunaBaselineTaskFrom` carries its handle and assignment into selected
  Luna contexts. The request owner calls `beginBaselineEpisode`, then
  `refreshBaselineEpisode` to observe incorporation. Worker reports remain reports,
  not independent checks. Route exact named owners directly; `routeQuestion` uses
  Jev only for ambiguous ownership and retains an unresolved outcome.
- **A specific reminder attached to real progress:**
  `Project.WorkflowReminderExamples.withQuestionReminders` wraps an existing
  `followWork` sink. It submits source-identified question deltas to a fixed-recipient
  reminder actor while preserving the sink's ordinary behavior. Supply the precise
  context, trigger, exclusions and suggested workflow; each immutable episode is
  evaluated once. Changed evidence needs a new episode. No silence-based readiness
  inference, repeated nagging, or second after-tool hook.

For every composed flow, retain its handle and evidence packet before deciding
whether a frontier-model turn is needed. Escalate ambiguous shared decisions;
let deterministic state transitions handle routine bookkeeping. Compare direct,
delegated and sibling reuse where the work presents real opportunities.
