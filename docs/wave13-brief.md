# Wave 13 — a browser harness that outlives its development run

## Outcome

A human can prepare and start the deterministic browser harness from an ordinary
host shell, stop it, and reopen the same data without relying on an agent's
working directory, mount namespace or command lifetime. Keep the existing
Engine/Store/child-message behavior. This is standalone harness work: no
Exomonad dependency, credentialed inference or real shell tools.

Wave 12 delivered the browser journey, but its live process still belongs to
the development run's command namespace. Its handoff PID is namespace-relative,
and generated web assets depend on the checkout. The supervisor is retaining
that run to preserve the demo. Do not stop, replace or migrate that live demo;
prove this wave on a separate loopback port and temporary data directory.
Do not modify the existing Tailscale route or claim a second-device check.

## First shared decision and executable contract

Read the existing CLI, `serve`, `ensure_asset_root`, Engine local transport and
StoreAgentToolService consumers before choosing interfaces. Use one existing
owner per lifecycle mechanism. Land a small executable launch/reopen contract
before delegating: explicit assets and data locations, readiness, graceful
stop, process-loss reopen, and the expected failure for missing assets.

Keep existing command-line use working or document an intentional migration.
Provide one documented project command for preparation and one for ordinary
host launch. Resolve paths explicitly; a launch from an unrelated directory
must work. Persisted data stays outside disposable source/build directories.
The launcher must not print or embed login secrets in its command line or
committed files. Report the actual host-visible process/service identity and
how to stop only that instance. Reuse OS process/service management; do not
build another process supervisor or daemon registry.

## Parallel obligations

Root owns shared CLI/path decisions, integration and product acceptance. Use a
broad ready frontier of bounded Luna work with independent exact-source review:

- Launch/assets owner: explicit runtime paths and standalone startup, extending
  the existing demo consumer. Keep source changes out of sibling-owned files.
- Independent acceptance owner: real production binary launched from a different
  directory; missing assets, successful browser API journey, stop/reopen and
  process-loss recovery. Author expected-red checks against the shared contract.
- Operator experience owner: concise startup/recovery instructions and accurate
  preparation/runtime failure reporting. Coordinate any shared CLI edit through
  root rather than competing for `main.rs`.

Delegate deeper only where a component has coherent independent work. Nodes ask
their parent about implementation ambiguity and continue independent work.
Include a verified entry point, exact source and required checks in assignments.

## Verification and bounded orchestration experiment

The new preparation owner is `nix develop .#web -c
scripts/verify-browser-journey`. It supplies pinned Node/npm and builds web assets
before the existing focused Cargo runner. Its Rust toolchain prerequisite is
explicit. Reuse this owner; do not introduce a competing test runner. Distinguish
missing prerequisites, executed failures and passing product assertions.

Trial one context-specific Haskell focused-gate helper for at least two actual
consumers (for example implementation owner and reviewer). Use the existing
SessionHelpers seed and typed command handles. First prove import and one real
call in the actual notebook; only then advertise it. Keep full command/evidence
references, counts and failure results. Jev may triage a failed check; unavailable
Jev must preserve that failure. Compare helper calls/repeated manual steps and
useful output with the ordinary runner. Publication alone is not reuse. If the
smoke check fails, record the failure and use the script; do not stall delivery.

Independent review must identify the exact candidate and whether assets were
prepared there. Final acceptance runs on the integrated revision, proves
startup from an unrelated directory and a real login/command/reconnect journey,
and distinguishes process-loss from clean reopen. Preserve existing credentials
and demo; no external tailnet-device verification is implied.

## Stop and handoff

Record source revision, exact commands/counts, data/assets paths, process
ownership, failure/reopen evidence and experiment outcome. Stop the isolated
test instance and confirm its release. Interview root before retiring the
development actors. Keep dirty worktrees, branches and evidence. A successful
run delivers portable operation and demonstrated failure handling, not merely
another startup script or a passing mock.
