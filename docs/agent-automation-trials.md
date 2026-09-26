# Agent automation trials

## Intent

Small project-owned Haskell machinery takes recurring operational work from
agents. The notebook interface is a stable, composable tool surface; it is not
a general model of project concepts or an expectation that every worker invents
a new DSL. Jev navigates semantic choices within bounded operations. Code owns
identity, execution facts and authority.

The inventory includes validated APIs and pending candidates. A validated
source revision does not install that module into a live run. Advertise an
entry point only when its example compiles, a real consumer exercises it, and
the launched run binds that source. The next-wave prompt carries an explicit
menu for discovery; measure usage and remove unhelpful entries after the trial.

## Six concrete trials

| Machinery | Model interaction | Result and stopping boundary |
| --- | --- | --- |
| Watch my checks | Named focused runs, owner and notification policy | Completion events produce failures or one combined summary without model polling |
| Prepare my review | Exact terminal candidate and declared prerequisites | Establish actual source before review; preserve conflicting dirty work |
| Diagnose this failure | Retained failed check, context and bounded read operations | Distinguish selection/setup/assertion failures and return a compact evidence packet |
| Carry this repair cycle | Existing implementer/reviewer relationship, scope and repair limit | Route within-contract repairs, escalate unresolved decisions, preserve exact candidate evidence |
| Watch my assumptions | Existing dependency events, projection and relevant-change callback | Notify affected owners of meaningful changes with exact before/after evidence |
| Investigate alongside me | Named caller-supplied read-only probes, context and work budget | Gather independent evidence and use optional Jev choices within supplied operations; return unresolved work explicitly |

## Composition and implementation

Extend existing Cmd completion, Project.Routing, focused-test evidence and source
publication owners. No second scheduler, durable log, task registry or parser.
Fix helper delivery into each consumer's actual bound source layer before
claiming helper reuse. Keep code and prompts isolated from running wave sources.
Start with command completion and grouped results. Reuse the existing review
coordinator; do not add another owner for its repair loop. Dependency observation
and bounded investigation use available event and concurrency primitives.
Use focused failure diagnostics to extend the current test helper rather than
introducing a parallel test runner.

Each automation may be a prebuilt actor configured by functions, context and
policy, or ordinary helpers composed in a notebook. Stable interfaces operate
concrete machinery. Agents primarily use these interfaces; extending the DSL is
not a routine assignment. Choose the strongest reliable narrow version over a
wide interface with unverified behavior. Four workflow automations and two
background capabilities form the trial portfolio.

## Prompt menu shape

Each installed entry gets a short line: when useful, actual callable signature,
inputs/preconditions, returned evidence, and where its compiled example lives.
Describe asynchronous behavior explicitly. Do not load full implementation into
model context. Let agents choose useful helpers; do not force extra successful
test executions to satisfy experiment counts.

## Evidence

Record opportunity, chosen helper, actual actor consumption, model calls and
provider token usage where available, underlying effects, setup/compile cost,
failures/fallbacks and outcome. Count eliminated polling/relay turns separately
from newly affordable useful observations. No usage can mean poor discovery,
a missing opportunity or low utility: inspect those before pruning. Give every validated entry at least three waves of prompt exposure, even if
initially unused. Count exposure from the first wave actually supplied with the
working helper and its menu, not from implementation or publication. Investigate
discovery, applicable opportunities and failures before changing the interface
or retiring it for non-use. Keep unsafe/broken paths unavailable until repaired;
a three-wave trial is not a requirement to repeat a known failure. After the
trial, prune based on evidence and retain results outside prompt context.


## Additional practical machinery

The second batch adds interview collection, prerequisite preparation, retained
command evidence recovery, evidence-backed handoff assembly, isolated browser
scenarios and slow-operation diagnosis. Each reuses the first batch where useful.
These are separate user-facing operations, not twelve required modules/actors.

## Exposure and usage record

`docs/automation-trials.json` owns the trial inventory. At launch, each exposed
entry records the wave, actual module/entrypoint and tested source revision.
`validated_not_exposed` means the isolated source passed its stated check but
has no wave credit; `compiled_runtime_pending` still needs a production
behavior gate; `implementation_pending` is outside the callable menu. All
`first_exposed_wave` values remain null until a launched wave receives both
the menu and the bound module. The current menu and compiled examples are in
`docs/agent-automation-menu.md`.
Afterward record applicable opportunities, evidence of discovery, attempts,
successful use, failures/fallbacks, useful new work, model calls and available
provider token counts. Unknown measurements stay unknown. Preserve artifact/run
references. A wave with no appropriate opportunity still counts as exposure but
not as evidence of low utility. Interview unused helpers explicitly after three
waves before removing their menu entries.

Before the next launch, assemble the model-facing menu from validated entries:
when useful, callable signature, required context/authority, returned evidence,
async behavior and compiled example location. Add short examples where they
save more model work than their prompt cost. Pending implementations belong in
this operator inventory, not in the callable menu. Menu construction and exposure
recording are launch gates; no uncompiled Haskell snippets in prompts.


## Wave16 revisions after the wave15 interviews

Keep the ten advertised operations available through the agreed exposure window.
The worker interview found non-discovery; the root interview found missing complete
compositions and a failed evidence summary. Neither establishes that unused APIs
lack utility. The new role prompts name the compiled focused-gate example directly.

- Command continuation retains the original job and routes completion, including
  Haskell `Cmd.observeCompletion`; explicit observation stays available.
- Focused evidence is captured by the originating command before exit. A watcher
  reads that retained record rather than opening a file in another actor's checkout.
- `FocusedGateExample` composes start, watcher and result callbacks;
  `WorkflowExamples.prepareFocused` composes readiness with starting the real check.
- `ParallelInvestigate.followFailure` optionally selects up to two supplied
  diagnostics. Cancellation, unconfirmed completion and unresolved judgment stop
  explicitly; the original outcome is retained. Read-only is the caller's contract.
- `ReviewFlow` declares component-only or required sibling source and handles one
  empty-findings correction with the same reviewer, then stops if still empty.
  Independent review and root integration remain separate obligations.

Measure actual calls and evidence: setup cost, helper passed to and used by a child,
completion notices versus unchanged polling, evidence collection without rerunning,
review relay avoided or shifted, and optional Jev probes useful versus unnecessary.
The wave15 baseline is 270 Bash starts and 53 empty-input waits across nine native
threads; fourteen short foreground episodes consumed 37 waits. This is opportunity
evidence, not a claim that wave16 saves that many model rounds. Preserve comparable
per-episode counts and distinguish task-size changes from intervention effects.
