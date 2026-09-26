# Agent automation menu

Use an entry when its named module is present in the actor's bound project
source. A commit or this menu alone does not install a module into a live run.
Keep the original response, `Cmd.Job`, source OID, or evidence path so a compact
notice remains traceable. Choose useful helpers; no entry is mandatory.

| Need | Callable entry and inputs | Result and compiled example |
| --- | --- | --- |
| Let focused checks finish while working | `Project.CheckResults.watchChecks`: owner, `NoticePolicy`, named `FocusedRun` values. Start each run with explicit `Cmd.Memory` through `Project.TestEvidence.startFocused` or `startFocusedIn`. | One completion watcher retains raw command receipts, parsed count/source evidence, and notice attempts; `readChecks` and `checksSummary` inspect it. `Project.CheckResultsChecks.completionRouting` exercises late, failed, dirty, missing, and mismatched results. |
| Know when a child has submitted reviewable source | `Project.Routing.notifyReviewReady` as a `followWork` sink for terminal `Outcome Candidate` events. | An exact terminal source notice; progress checkpoints and unbound or mismatched sources do not claim readiness. `Project.RoutingChecks.reviewReadiness` is the focused example. Use the existing review request after deciding to review. |
| Diagnose a failed focused check | `Project.TestEvidence.collectFocused`, then `diagnoseFocused` on its `FocusedResult`. | Deterministic selection, setup, assertion, runner and source branches, bounded retained excerpt, and existing code reflex. `finishFocused` optionally asks Jev for a failure explanation; neither Jev nor the excerpt determines a pass. `Project.CheckResultsChecks.completionRouting` exercises the branches. |
| Track an assumption that may change | `Project.AssumptionWatch.watchAssumption`: initial typed value, event source, projection and decision callback. `watchIncorporatedBaseline` covers a child's incorporated source. | Bounded before/after history and decision with a retained notification receipt; an unresolved judgment is visible. `Project.AssumptionChecks.changes` covers deterministic and semantic policies. |
| Gather independent read-only evidence | `Project.ParallelInvestigate.selectProbes`, `startProbeBatch` and `observeProbe`: named caller-supplied commands, absolute checkout, memory and `ProbeLimits`. | Original jobs, refusals, bounded observations and explicitly unrun probes. Optional `chooseNextProbe` selects only among supplied probes. `Project.AutomationRuntimeChecks.commandCustody` and `checks/automation-helper-contract.hs` exercise the boundary. |
| Collect answers before retiring workers | `Project.Interview.collectInterview`: supplied `KnownAnswer` or `AwaitAnswer` items. | Typed answered, waiting, cancellation-pending or unavailable findings; `interviewComplete` refuses empty or incomplete input. `Project.InterviewChecks.collectAnswers` is the focused example. |
| Continue when a prerequisite finishes | Start and retain the preparation `Cmd.Job`; use `Project.PrepareContinue.verifyPrepared` inside `R.on (Cmd.completion job)` with a readiness function returning a typed value. | The continuation receives that value after exact terminal receipt, clean exit and readiness checks; failure keeps the original run receipt. `Project.PrepareContinueChecks.preparationCompletion` compiles and exercises late, failed and unready paths. A long job never requires a foreground polling loop. |
| Recover output from an existing command | `Project.RetainedEvidence.evidenceBudget`, then `recoverRetained` on the original `Cmd.Job`. | Original status and raw stdout/stderr pages with typed EOF, current end, budget, incomplete-page or refusal stops. Each stream has an explicit byte budget; no command is resubmitted. `Project.PrepareContinueChecks.preparationCompletion` checks a failed job and a four-byte cutoff. |
| Notice a command that remains slow | `Project.SlowCommandWatch.watchSlowCommand`: owner, context, existing `Cmd.Job`, threshold, diagnostic character limit and callback over typed observation. | At most one slow alert after a bounded observation, then the same job's later completion; the callback may use Jev without changing job status. `Project.AutomationRuntimeChecks.commandCustody` exercises cross-actor custody and `checks/automation-helper-contract.hs` checks the typed API. |

The review and repair coordinator remains the ordinary `Project.Review`
workflow. The separate `ReviewFlow` automation has no demonstrated end-to-end
run and is absent from the callable menu. `Project.HandoffExamples` awaits its
combined gate, and `Project.BrowserScenario` awaits its isolated browser run.
Neither is advertised here yet.

## Trial record

`docs/automation-trials.json` records implementation separately from exposure.
Record `first_exposed_wave` only after a launched wave actually receives this
menu with its bound callable modules. Each validated entry stays available for
at least three exposed waves, including an initial wave with no use. For every
wave, record applicable opportunities, discovery, attempted calls, outcome,
fallback, source/evidence references and token or attention measurements when
available. An empty use count alone does not justify removal; inspect discovery
and opportunities first. A known broken path stays unavailable while repaired.
