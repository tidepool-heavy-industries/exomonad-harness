# Wave17: integrate Inject and make browser acceptance repeatable

## Product outcome

Integrate wave16's reviewed deterministic before-request Inject slice into this
updated harness, preserve the new Haskell helpers, and deliver a clean committed
source with independently checked standalone browser acceptance. The useful
product is a reproducible release candidate and a precise browser-operating
handoff. This wave owns the integration; the supervisor has not merged wave16.

Continue to use deterministic echo/test interactions and standalone extension
stubs. No credentials, external tools, Exomonad adapter, new hook semantics,
Tailscale changes or long-lived public server are authorized by this brief.
Use isolated test-owned ports/data. Keep existing retained demo data.

## Exact inputs and integration authority

- Current helper/prompt baseline: `1b0f7a0` (resolve to full OID when recording).
- Wave16 checked product: `9123ffba6af1171f072b48c9e95db94786eabc67`.
- Wave16 stop handoff: `21cb20e` on `rsi/wave16`; merge this descendant so
  retained interviews and evidence accompany the product. Read its
  `docs/wave16-handoff.md` and `docs/wave16-contract.md` with `git show` first.
- Reviewed components: Engine `7dd904f`, Provider/Store `859f56a`, independent
  acceptance `424bab6`. They are already ancestors of the product. Do not
  cherry-pick them a second time or reconstruct source by copying files.
- Final wave16 source had serde and production browser checks each 1/1;
  earlier Engine and Store checks apply to their named earlier sources.
  Its reported dirty paths were helper/docs paths. This is retained evidence,
  not proof that the new combined source passes.

Root may merge and resolve integration conflicts on this run's branch. Keep
this wave17 NEXT/brief, the new helper imports and shared workspace pin
`2599a6434b5f09ab176b29a851b6a680ef4aa45d`. Reconcile the automation ledger by
retaining both exact trial evidence and the later interview, rather than taking
one entire conflicting version. Preserve completed-wave history separately.
Do not move the operator's master branch, push, or edit other run worktrees.

## Work graph and checks

Root owns shared decisions and integration. After publishing the combined source,
use parallel bounded Luna obligations for independent source/replay review,
browser acceptance, and release/operator documentation where the actual write
sets permit. Add depth only for a coherent subproblem. Evidence-only audits need
no invented code change or recursive review. Ask the parent about contract changes.

Acceptance at the final committed candidate:

- Both wave16 product ancestry and the new helper/prompt baseline are retained.
- Focused Engine injection, invalid-input/claim-cleanup, typed Store reopen and
  existing restriction behavior remain correct. Select actual named tests from
  source; record expected, matched and executed counts and exact source.
- A browser check prepares its own assets in its consuming checkout, then exercises
  ordinary echo, injected context, child, reconnect and process-loss reopen.
- Failures in prerequisites remain distinct from product assertion failures;
  passing counts on dirty or mismatched source remain unknown acceptance.
- Format before recording the candidate. Review exact commits independently;
  final combined-source checks are not inferred from component reviews.
- A short release handoff states how to start the standalone deterministic browser
  demo and use echo/inject/child, what persists on reopen, and known limitations.
  Repair concrete integration or workflow defects; avoid unrelated feature growth.

## Automation trial and startup gate

Use `SessionHelpers.runCheck` and `runBrowserCheck` for actual repeated checks;
pass the candidate at invocation. `standaloneBrowser` is the canonical existing
browser definition. Bind returned gates with `<-`, wait for terminal notices,
then inspect typed evidence when making an acceptance decision. Pass the actual
helper name and published source to children that need it. A preparation-success
model round whose only action is starting the known test should be unnecessary.
No extra test run is needed just to inflate helper use.

The runtime now publishes resident actor-launched fork groups without a notebook
callback; the host regression includes activation and typed reply. Native success
is not yet established. Make the first real component review using ReviewFlow a
bounded startup smoke: one candidate, component scope, at most one repair. Retain
reviewer identity, first provider-turn evidence, exact typed verdict and cleanup.
Do not expand ReviewFlow use until this succeeds. Tell the supervisor when the
reviewer is admitted so it can inspect the native trace. If activation or delivery
is unresolved, retain that fact, stop the trial deliberately and use ordinary
exact-commit review; do not let the trial block product integration indefinitely.

Record actual helper calls, opportunities, setup/rejected cells, follow-up tool
calls and useful non-use. Menu exposure is not use, and tool-call counts are not
model-round savings. Preserve the three-exposed-wave trial window. Prefer one
completion route over short-yield/empty-write_stdin polling. Use Jev only for a
real unresolved semantic choice; deterministic prerequisite routing needs none.

## Finish

Interview useful workers before retirement; retain root's own-words kaizen
interview at completion, especially helper choice, setup costs and the review
activation trial. Retain exact integrated source, checks, reviews, command jobs,
automation evidence and cleanup uncertainties in docs/wave17-handoff.md and NEXT.
Stop after this integration/release slice. A completed model turn is not proof
that all actor resources were released. No successor or recurring timer.
