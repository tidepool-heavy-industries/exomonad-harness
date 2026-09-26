# Agent automation menu

Use an entry when its named module is present in the actor's bound project
source. A commit or this menu alone does not install a module into a live run.
Keep the original response, `Cmd.Job`, source OID, or evidence path so a compact
notice remains traceable. Choose useful helpers; no entry is mandatory.

For qualified `Project.TestEvidence` calls, first `import Project.TestEvidence`.
`Project.CheckResults` re-exports its unqualified names but does not bring that
module qualifier into scope. Wave15 exercised this explicit import successfully.

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
event-driven collection in `Project.Routing`. The separate `ReviewFlow`
automation has no demonstrated end-to-end
run and is absent from the callable menu. `Project.BrowserScenario` awaits its isolated browser run and is not advertised here yet.

## Trial record

`docs/automation-trials.json` records implementation separately from exposure.
Record `first_exposed_wave` only after a launched wave actually receives this
menu with its bound callable modules. Each validated entry stays available for
at least three exposed waves, including an initial wave with no use. For every
wave, record applicable opportunities, discovery, attempted calls, outcome,
fallback, source/evidence references and token or attention measurements when
available. An empty use count alone does not justify removal; inspect discovery
and opportunities first. A known broken path stays unavailable while repaired.
