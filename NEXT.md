# Current assignment — wave 12 browser harness

Read [the wave-12 brief](docs/wave12-brief.md) and deliver its standalone browser
journey over Tailscale. Simple deterministic echo, test and messaging behavior is
sufficient; real model inference and shell tools are not required. Use the
updated helper and review surfaces. Do not resume completed wave-11 obligations.

## Accepted product state

- Integrated code: `ec012f042774e086634666623e45d40bb366be88`.
- Final wave interviews/handoff: `3bf45e2cc600cb4a2eb5d13af1f3eac165c5dfba`.
- Store recovery, guarded Engine interruption recovery, Driver restart evidence,
  and helper-process kill gates are integrated. Root recorded component-group
  cleanup complete with no pending responses/watches.
- Focused checks on the integrated code passed: Engine recovery 2/2, dynamic
  schema 2/2, harness recovery 5/5 (includes the Engine pair), atomic completion
  2/2, Store process loss 2/2, Driver recovery 2/2, followup lifecycle 2/2,
  shutdown 2/2, process restart 1/1, adapter readiness 1/1. Formatting and
  diff checks passed. Counts overlap and are not a unique-test total.
- Evidence proves offline replay, clean Store/Driver reopen, and killing a
  helper after commit before wake. It does not prove crash of an already-running
  Driver, remote execution exactly once, power-loss durability, or live adapter
  behavior. CLI restart admission remains a separate product capability.

## Branch disposition

- Engine `65bace1`: owned code equivalent to reviewed/integrated `6988d51`;
  no code remains to merge. Lead stopped before typed Delivery; own-words
  interview subsequently recorded.
- Driver `24b61a0`: reviewed code already integrated through `5a5329a`;
  remaining differences were documentation only.
- Durable `d6b9ab8`: redundant pending-envelope API rejected, not a pending gate.

## Run preparation

Running demo source: `2c19e457d7707b1d57666541fa7c1450003f1234`.
The sole live contract is `docs/wave12-contract.md` as corrected at `c9c7281`
(cancelled wait: coarse request failed, outcome cancelled, job cancelled).
Root owns integration, real-browser acceptance, Tailscale launch and handoff.

Integrated preparation:
- HTTP/WS acceptance dependencies and matching lock amendment.
- Expected-red black-box test candidate `9fc9152`, subsequently
  corrected by `1e08bc6`; its missing-producer failure was closed by
  the integrated server and a passing 1/1 production-binary test.
- Reviewed web prep `ef5e612`, merged through current root. Integrated
  npm test 11/11, `npm run check` and `npm run build` passed.
- Reviewed optional web outcome renderer `3bfa370`, accepted for
  preparation only and merged through `7be80b2`; integrated npm test
  14/14, check and build passed. Its producer was subsequently verified
  by the integrated production-binary journey and real Chromium run.
- Corrected browser test `1e08bc6` is merged. Its earlier red
  `commandId`/`outcome` barrier became green on the integrated server:
  1 matched, 1 executed, 1 passed.

Delivered server and verification:
- `wave12/ready-frontier/wave12-server` candidate
  `7ae65fbb75465192ebbcc7ba226b538b04cd931e` was accepted by an
  exact-tip independent reviewer after server 8/8 and authenticated
  production-binary browser 1/1 passed on that tip. It changed only
  owned `crates/harness-demo/src/main.rs` and was merged into root
  `2c19e457d7707b1d57666541fa7c1450003f1234`. Integrated
  server check passed 8 matched/executed, browser check passed 1
  matched/executed, and the existing Driver process-kill test passed
  1 matched/executed on the integrated source.
- Web and acceptance owners' preparation candidates are merged.
  A real local Chromium journey passed login, echo/test, pending
  message/cancel, child reply, fail/recovery and refresh without replay.
  An isolated server on port 4601 passed a SIGKILL/reopen browser
  recovery test. The running port-4600 demo was not restarted.
- Existing tailnet-only Serve URL
  `https://nixos-1.sphynx-bitterling.ts.net/` proxies to the running
  loopback process PID in `/tmp/wave12-live/server.pid`. Local HTTPS
  route returned 200. Remote-device reachability is not verified.
  Secret, launch/stop instructions, evidence and limits are in
  [the wave-12 handoff](docs/wave12-handoff.md).

The original acceptance child was host-cancelled. A bounded Astra
consultation confirmed `Engine::with_transport` and
`StoreAgentToolService` support the required deterministic Engine/child
seams without a library edit. The session helper publication/import
experiment failed; use direct `scripts/cargo-focused-test` until repaired.
The final server candidate received exact-tip independent acceptance
after the reviewer built missing web assets. The failed initial
review test had 1 matched/executed but reached no browser assertions.
The helper experiment files are retained outside Git at
`/tmp/wave12-live/helper-experiment`; notebook import failed despite
publication, and direct focused checks were used.

## Where the last run stopped (live checkpoint; update on delivery)

- No reviewed or passing wave-12 code remains unmerged as of
  `2c19e457d7707b1d57666541fa7c1450003f1234`. Server
  `7ae65fbb75465192ebbcc7ba226b538b04cd931e` was accepted and
  merged; earlier `bcedd45` received Repair and is superseded.
- Reviewed web prep `ef5e612` and `3bfa370`, and acceptance test
  history `9fc9152`/`1e08bc6`, are merged through running source
  `2c19e45`; no passing web/test branch remains unmerged.

## Evidence and constraints

[Wave-11 checkpoint history](docs/wave11-handoff.md),
[interviews](docs/interviews.md), and [friction](docs/exomonad-friction.md)
retain the detailed sequence. Historical checkpoints are not current blockers.

Live adapter inference, credentialed item-2 retry and item-13 trace remain on
hold. No new scheduler/mailbox/durable journal or adapter implementation is
implied. Use focused checks with expected and executed counts, explicit failure
barriers, one compiler worker, pathspec commits and no attribution trailers.
Never stash/reset/checkout paths or restart a shared daemon.
