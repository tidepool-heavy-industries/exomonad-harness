# Wave14 brief — prove one standalone hook through the browser harness

## Outcome and boundary

A consumer can attach a typed, default pass-through before-request hook to the
existing Engine. The deterministic browser path invokes it, retains its typed
decision and optional evidence through the existing Store, and preserves normal
request behavior. This is the first bounded slice of PRD's hook table, not a
claim that every hook or every before-request transformation is implemented.

Wave13 already proved portable browser startup, login, commands, child messages,
reconnect and process-loss recovery. Reuse those owners and checks. Do not add
another scheduler, log, transport, browser command or UI panel merely to expose
implementation bookkeeping. No Exomonad adapter, credentialed inference, shell
execution, real external tools, live demo migration or Tailscale change.

## First shared contract

Read PRD's provider/hooks sections and the actual Provider, Engine request
construction, Store decision records and deterministic browser consumer. Root
owns one small compiling shared contract before delegation. Use the PRD's
RequestPlan vocabulary and a typed decision with optional opaque evidence.
The bounded implementation may support only Send; do not advertise restriction
or injection variants that silently do nothing. State unsupported scope plainly.
Preserve pass-through providers and unknown historical decision records.

Choose the invocation and persistence boundary from existing request identity
and transaction owners. Define what happens before transport, on transport
failure, on retry and on reopen. Do not invent exactly-once behavior that the
existing request lifecycle cannot provide. Record consequential unresolved
semantics for the supervisor instead of silently widening this assignment.

The executable contract must prove the hook is actually called, its result is
stored with correct request/agent provenance, default Send preserves request
items/tools/effort, and reopening does not invent a second decision for a
request that was merely read back.

## Parallel work

After landing the shared types and boundary, use three bounded Luna obligations:

- Provider/Store: typed consumer surface, default behavior, serialization through
  the existing decision owner and compatibility with existing records.
- Engine: production invocation and correctly correlated persistence; preserve
  cancellation, failure and request ordering at the selected boundary.
- Acceptance: expected-red tests using the deterministic browser production
  consumer and a hook recording distinctive evidence; concise consumer example.

Root owns shared decisions and integration. Keep the test owner independent
of implementation, then commission exact-commit review of the cumulative
candidate. Add depth only for coherent additional ownership. Preserve dirty
work; a reviewer must identify the actual candidate and preparation used.

## One orchestration experiment

Implementation owner and independent reviewer each execute the same focused
check through a context-specific Haskell composition in one notebook cell.
Start with existing SessionHelpers and typed command handles. Compose start,
wait and evidence inspection in ordinary Haskell; do not add a job registry.
Prove import and one execution before sharing it. Give both actors the helper
source revision and record actual incorporation and execution.

Choose realistic memory reservations. The protected demo holds1GiB of the
shared8GiB general command pool: an8GiB request cannot start. Leave room for
parallel checks; do not silently cap a job or claim low RSS frees reservations.
Record model tool calls, underlying command effects, evidence/counts and both
actor identities. A backgrounded call is not completion. Missing Git status
means unknown cleanliness, not pass. Fall back to the project script if the
composition fails; the product does not wait for a successful experiment.

## Acceptance and handoff

Run the smallest focused library/provider/Store/Engine tests that establish
changed behavior, plus the production standalone browser test on the integrated
source. Reuse prepared web assets and rerun web checks only if web source or its
contract changes. Include a transport-failure case and existing-record/reopen
case. Do not rerun unrelated credentialed suites.

Retain exact source, commands, selected/executed counts, failures, hook decision
records, runtime revision, workspace pin and run/log IDs. Interview root and
reviewers about the actual parallel split, shared-contract corrections, hook
boundary and helper consumption. Preserve the port4600 demo and its old host.
Stop only isolated test processes and completed run actors, checking resource
release separately from terminal state.

## Launch status

Prepared by the supervisor after accepting wave13 at harness88ff710. No wave14
run is active yet. Activation requires the next matched runtime build, source
and submodule-path preflight, reviewed launch fixes and a recorded launch.
