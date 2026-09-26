# Current assignment — wave 13 standalone browser operation

Read [the wave-13 brief](docs/wave13-brief.md). Deliver a deterministic browser
harness that a human can prepare, start, stop and reopen from an ordinary host
shell, independently of the agent working directory and command lifetime.
Keep inference and tools deterministic; no Exomonad adapter or credentialed
inference is authorized. Do not resume completed wave-12 implementation.

## Accepted baseline and protected service

Wave 12 integrated product source `2c19e457d7707b1d57666541fa7c1450003f1234`:
server 8/8, production-binary browser journey 1/1, Driver process-kill 1/1,
real Chromium journey and isolated process-kill/reopen checks passed.
Second-device Tailscale reachability remains unverified. Details and limits:
[handoff](docs/wave12-handoff.md), [interviews](docs/interviews.md).

The existing port-4600 demo must stay running. Its PID in the handoff is
namespace-relative; it remains owned by the prior development run. Do not
stop or migrate it, alter its data or secret, or change Tailscale Serve.
Prove this wave on a separate ephemeral loopback port and isolated data.
The supervisor retains the old host solely to preserve that service.

## Preparation and orchestration

- `nix develop .#web -c scripts/verify-browser-journey` prepares pinned Node/npm,
  locked web dependencies and assets before the existing focused Cargo runner.
  Rust remains an explicit prerequisite. It passed on clean `7fd8e02`;
  integrated preparation is `c6a5916`. Product checks belong to the exact source
  tested, not a later merge by implication.
- Prompt corrections `6119ae5` distinguish fresh and retained review checkouts,
  missing prerequisites, expected-red tests and pending update presentation.
- Root owns shared contracts/integration; use bounded parallel Luna work and
  independent exact-source review, with depth where useful.
- The experiment is a context-specific Haskell focused-gate composition reused
  by at least two actual consumers. Prove import and execution before claiming
  reuse; fall back to the script if unavailable. Read the brief for measurements.

Keep existing holds on live adapter inference, credentialed item-2 retry and
item-13 trace. No new scheduler, mailbox or durable journal. Use focused checks
with selected/executed counts, one compiler worker, pathspec commits and no
attribution trailers. Never stash/reset/checkout paths or restart shared daemons.
Interview before retiring actors; preserve the live demo and dirty worktrees.
