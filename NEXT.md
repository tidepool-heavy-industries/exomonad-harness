# Current handoff — wave 11 complete; inter-wave preparation

No new product-wave assignment is active. The supervisor is implementing the
approved Tidepool/Exomonad inter-wave batch before choosing the next standalone
harness milestone. Do not resume completed wave-11 obligations from old notices.

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

## Next preparation

The approved inter-wave implementation is tracked in Tidepool
`plans/rsi-iteration-4.md`: session helper modules and inheritance, Bash/Jev
seeds, authoritative review submission, actor-managed review/repair, focused
execution evidence, wrapper repair and prompting. Sol retains integration.
Product work for the next wave will prove harness extension points with
standalone stubs before any Exomonad adapter; its exact assignment is not yet set.

The focused runner now supports `--expect N`, retained executable identity and
full output evidence; see README.md. New prompt guidance distinguishes pending
questions, terminal Blocked outcomes, and findings-only investigation.

## Evidence and constraints

[Wave-11 checkpoint history](docs/wave11-handoff.md),
[interviews](docs/interviews.md), and [friction](docs/exomonad-friction.md)
retain the detailed sequence. Historical checkpoints are not current blockers.

Live adapter inference, credentialed item-2 retry and item-13 trace remain on
hold. No new scheduler/mailbox/durable journal or adapter implementation is
implied. Use focused checks with expected and executed counts, explicit failure
barriers, one compiler worker, pathspec commits and no attribution trailers.
Never stash/reset/checkout paths or restart a shared daemon.
