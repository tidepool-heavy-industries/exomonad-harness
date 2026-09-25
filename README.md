# exomonad-harness

A standalone Rust harness for GPT-6 over the OpenAI Responses API: every tool call is asynchronous, agents form a tree with the verbs GPT-6 was trained on, the operator is a peer in the same mailbox, and the whole run is a content-addressed request tree in SQLite with a web view served from the binary. No dependency on Tidepool or Exomonad; Exomonad consumes it later through an adapter.

Start at `NEXT.md`: where the last run stopped and where this one starts.

Read in this order:

1. `PRD.md` — the contract. Compressed format; `!` marks hard rules; every rule carries its reason.
2. `docs/tree.md` — how the build runs: waves, the scaffold-then-spawn pattern at every node, labels, acceptance, the interview.
3. `docs/model-facing.md` — the cached-prefix text: developer items, envelopes, tool descriptions, refusals, notices.
4. `docs/web.md` — the design brief for the web view.
5. `docs/nudges.md` — the watchdog packets the run installs on itself.
6. `docs/ideas-later.md` — deferred ideas; not contract.

Produced by the run: `docs/findings.md` (API facts measured live), `docs/questions.md` (every question asked of the operator and its answer), `docs/interviews.md` (one section per node, the interview from tree.md), `docs/principles.md` (web), `.exomonad/nudges/<label>.jsonl` (the nudge ledger).

Companion on the consumer side: `~/dev/tidepool/plans/harness-adoption.md`.

## Focused tests

Use `scripts/cargo-focused-test` for a named Cargo test target. It reports
selection and execution counts separately, refuses zero runnable matches or a
successful command with no executed tests, and preserves test failures.

```sh
scripts/cargo-focused-test --package harness --target lib --filter dynamic_reply_schema
scripts/cargo-focused-test --package harness --target test:adapter_readiness --filter active_cell_survives_three_boundary_envelopes_and_finalizes_durably
scripts/cargo-focused-test --package harness-demo --target bin:harness-demo --filter followup_lifecycle
```

The filter is a libtest substring. Name the expected count in the assignment and
compare it with the actual result; compilation or listing alone is not a pass.
The guard supports native Rust libtest targets, not custom test harnesses or
cross-target runners. Add `--expect N` to require exactly N runnable and executed
tests. It builds once, freezes the selected executable, and lists and runs that
same artifact from the package directory. Self-spawning tests keep a stable
executable path.

Each invocation prints an `evidence.json` path beside retained `output.log`,
selection logs and the frozen executable in the Cargo target directory. The
record includes package, target, filter, source HEAD/working-tree status,
executable SHA-256, selection, exit status and libtest summaries. Source metadata
is an observation, not a guarantee against concurrent source edits; run acceptance
checks from a stable checkout. These artifacts can be removed with that target
cache after their evidence is no longer needed.
