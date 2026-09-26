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

The inter-wave helper/review changes are built and the shared workspace is pinned.
Use ordinary event-driven review; automatic review/repair convergence is deferred.
The root owns integration and uses broad bounded Luna delegation with useful depth.
See the brief for the helper composition experiment and acceptance evidence.

Wave-12 working contract: `docs/wave12-contract.md`. First frontier starts from
the shared-contract commit following `6e3dcd32b686cc84dc4149cd18183bfacb7bc54c`.
Expected owners: deterministic server (`crates/harness-demo/src/main.rs`),
browser (`web/src/`), and black-box acceptance
(`crates/harness-demo/tests/browser_journey.rs`). The first expected replies
are code candidates or specific seam questions; each owner reports focused
test expected/executed counts and friction. Root owns integration, helper
publication, live browser acceptance and Tailscale launch. Reconcile the
exact admission checkpoint and branch identities after the fork cell.

## Evidence and constraints

[Wave-11 checkpoint history](docs/wave11-handoff.md),
[interviews](docs/interviews.md), and [friction](docs/exomonad-friction.md)
retain the detailed sequence. Historical checkpoints are not current blockers.

Live adapter inference, credentialed item-2 retry and item-13 trace remain on
hold. No new scheduler/mailbox/durable journal or adapter implementation is
implied. Use focused checks with expected and executed counts, explicit failure
barriers, one compiler worker, pathspec commits and no attribution trailers.
Never stash/reset/checkout paths or restart a shared daemon.
