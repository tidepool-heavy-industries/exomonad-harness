# Wave16: deterministic before-request injection

## Product outcome

Extend the standalone browser harness's before-request extension point to support
one provider-authored injected item before a transport attempt, with typed decision
and opaque evidence persisted through Store reopen. This is the remaining Inject
branch in PRD's before-request hook, following wave15 tool restriction.

A human using the existing browser echo/test interaction should be able to exercise
a deterministic injected-context path, observe the completed response and inspect
its recorded provenance. Use the existing browser/transport/Store seams; no new UI
command, credentials, judgment service, Exomonad adapter or external tools.

## Shared decisions before delegation

Root owns the typed contract and a small compiling consumer example before forks.
Inspect the current Engine invocation, canonical request serialization and final
advertised tools. Decide and record how Inject composes with SendRestricted, what
item forms are accepted, and how one hook decision maps to one transport attempt.
Default: apply injection once to that attempt without recursively re-entering the
same hook. Invalid input must fail before transport with typed error and pending
claim cleanup. Do not silently fabricate tool-call results or alter earlier items.
Reopen/replay reads recorded decisions rather than invoking the live hook again.
Name exact expected request/evidence shape and required sibling commits in tasks.

## Acceptance

- Ordinary Send and restricted selection preserve wave15 behavior and full schemas.
- The real Engine and canonical request serializer include the injected item in
  the decided position exactly once, with deterministic request/agent provenance.
- Invalid injection, transport failure and pending-claim cleanup have focused tests.
- Typed decision and opaque evidence survive Store reopen without repeating hooks.
- An independent expected-red test becomes green against the integrated production
  browser consumer; retain the existing child/reconnect/process-loss journey.
- Check exact matched/executed counts and tested source. Preparation failure is
  distinct from assertion failure; dirty-source evidence remains explicit.

## Work graph and automation experiment

Sol owns shared decisions and integration. Use parallel Luna obligations for
Engine/transport, provider/Store and independent acceptance when the actual file
boundaries permit. Use depth where a coherent subproblem merits it. Review exact
candidates independently, then check integrated source.

Use the compiled Haskell workflow examples described in the automation menu for
real repeated work: focused start -> completion -> evidence and preparation ->
readiness -> check. Specialize one useful check helper for this wave and provide
its name, arguments and published source to at least one child who needs it.
Do not execute extra checks merely to satisfy the trial. If an API obstructs the
workflow, retain its exact failure and use the original job's evidence.

One bounded review/repair-flow trial should handle a component whose required
source is declared. Root retains integration; escalate contract changes. Use
ordinary review if the trial reports an unresolved condition. A bounded Jev
follow-up may select supplied diagnostic probes for an actual failed check; it
never decides test counts, source identity or review acceptance.

Do not count menu exposure as use. Record opportunity, actual calls, setup cost,
recovery, frontier rounds avoided or shifted, and retained evidence. A helper may
be useful without being the right fit here. No routine one-second polling.

## Finish

Retain implementer/reviewer interviews before retirement and root interview at
completion. Report exact source, independent reviews, integrated checks, automation
outcomes and remaining uncertainties in NEXT.md and the friction record. Stop after
this product slice. The old wave12 demo was stopped by the supervisor; preserve its
data. Use isolated ports/data for acceptance and do not change Tailscale routes.
