# Agent automation trials

## Intent

Small project-owned Haskell machinery takes recurring operational work from
agents. The notebook interface is a stable, composable tool surface; it is not
a general model of project concepts or an expectation that every worker invents
a new DSL. Jev navigates semantic choices within bounded operations. Code owns
identity, execution facts and authority.

These are implementation candidates, not installed API names. Advertise an
entry point only after its example compiles and a real consumer exercises it.
The next-wave prompt may carry a larger explicit helper menu for discovery;
measure usage and remove unhelpful entries after the trial.

## Six concrete trials

| Machinery | Model interaction | Result and stopping boundary |
| --- | --- | --- |
| Command completion | Start a check and subscribe to its result | One useful terminal notice, exact job and evidence; no model polling |
| Grouped checks | Supply several named checks and desired notification policy | One combined result with individual failures/unknowns; do not hide incomplete checks |
| Review readiness | Supply exact candidate and required preparation | Establish actual checkout/prerequisites before review; dirty conflicting work remains preserved |
| Slow-operation observation | Select operations, duration threshold and bounded diagnostics | Gather wait/compile/execution evidence and report an actionable episode; no blind cancellation |
| Dependency-change notice | Observe declared source/decision dependencies for active assignments | Notify affected owner of exact change; delivery is not incorporation |
| Focused failure diagnostics | Inspect a retained failed check through bounded reads and optional Jev choices | Compact evidence packet; zero selection, setup failure and product assertion failure remain distinct |

## Composition and implementation

Extend existing Cmd completion, Project.Routing, focused-test evidence and source
publication owners. No second scheduler, durable log, task registry or parser.
Fix helper delivery into each consumer's actual bound source layer before
claiming helper reuse. Keep code and prompts isolated from running wave sources.
Start with command completion and grouped results. Review readiness, slow-call
and dependency observation follow as slots free and actual primitives are verified.
Use focused failure diagnostics to extend the current test helper rather than
introducing a parallel test runner.

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
a missing opportunity or low utility: inspect those before pruning. Drop prompt
entries with no demonstrated value; retain full evidence outside prompt context.
