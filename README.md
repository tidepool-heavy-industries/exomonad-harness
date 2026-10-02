# exomonad-harness

A standalone Rust harness for GPT-6 over the OpenAI Responses API, with
asynchronous tools, durable conversation history and an authenticated web view.
The standalone demo manages its own agent tree. An embedding supplies admitted
actor capabilities and tools while retaining lifecycle authority; the harness
has no Tidepool dependency.

Start at `NEXT.md` and the [implementation handoff](docs/embedding-ready-handoff.md).
The [embedded-host contract](docs/embedded-host-prd.md) defines the Exomonad target;
the [daily-driver roadmap](docs/daily-driver-plan.md) sequences real integration.
The public consumer in `crates/harness-demo/tests/embedded_host.rs` exercises the
embedding without credentials, Tidepool or a second actor supervisor.

Read in this order:

1. `PRD.md` — generic crate and standalone demo contract. Compressed format; `!` marks hard rules; every rule carries its reason.
2. `docs/embedded-host-prd.md` — Exomonad embedding contract and full browser-tree gate.
3. `docs/tree.md` — how the standalone build runs: waves, scaffold-then-spawn, labels, acceptance, interview.
4. `docs/model-facing.md` — the cached-prefix text: developer items, envelopes, tool descriptions, refusals, notices.
5. `docs/web.md` — the design brief for the web view.
6. `docs/nudges.md` — the watchdog packets the run installs on itself.
7. `docs/ideas-later.md` — deferred ideas; not contract.

Produced by the run: `docs/findings.md` (API facts measured live), `docs/questions.md` (every question asked of the operator and its answer), `docs/interviews.md` (one section per node, the interview from tree.md), `docs/principles.md` (web), `.exomonad/nudges/<label>.jsonl` (the nudge ledger).

Companion on the consumer side: `~/dev/tidepool/plans/harness-adoption.md`.

## Focused tests

### Native Buck2 on x86_64 Linux

The Buck2 graph compiles `harness` and `harness-demo` as native Rust targets.
Reindeer generates the locked third-party Rust targets; Buck does not delegate
the crate build to Cargo. The web `check`, `test`, and `dist` targets run pinned
Node actions against a fixed-output offline npm cache. Cargo remains the
publication and non-Linux development path. The [server acceptance record](docs/buck2-acceptance.md)
lists exact checks, resource use, and pending gates.

```sh
nix build .#buck2 .#buck-rust .#buck-cc .#buck-binutils .#buck-node \
  .#buck-python .#buck-npm-cache --no-link
scripts/buck2-configure.sh
BUCK2="$(nix eval --raw .#packages.x86_64-linux.buck2.outPath)/bin/buck2"
"$BUCK2" build //crates/harness:harness --local-only
"$BUCK2" build //web:check //web:test //web:dist --local-only
scripts/buck-focused-test --target //crates/harness:unit_tests \
  --filter hooks::tests::send_plan_and_opaque_evidence_cross_serde_boundary \
  --exact --expect 1 --local-only
```

The script lists the selected libtest cases, rejects zero matches and count
mismatches, then checks the executed count. Select a suite by its separate Buck
target, for example `//crates/harness:adapter_readiness`; the demo suites are
under `//crates/harness-demo:`. Browser tests declare the demo executable, web
assets and launcher through Buck resources. Run those whole suites with
`buck2 test` once the Buck test executor is configured; they exercise local
listeners and spawned processes, so keep them on a host with those capabilities.
The Rust unit and ordinary integration targets can run in an isolated worker.

The Buck2 binary and bundled Prelude are pinned together in `flake.nix` and
`.buckconfig`. `scripts/buck2-configure.sh` writes ignored `.buckconfig.local`
from pinned Nix outputs; rerun it after changing `flake.lock`. To update Rust
dependencies, keep the package manifests and root `Cargo.lock` authoritative,
update `third-party/rust/Cargo.toml` and its lock to the same resolved versions,
then regenerate from the repository root:

```sh
nix build .#buck-reindeer --no-link
REINDEER="$(nix eval --raw .#packages.x86_64-linux.buck-reindeer.outPath)/bin/reindeer"
(cd third-party/rust && "$REINDEER" --config reindeer.toml buckify)
```

Review generated `BUCK` and fixups before committing. The current Reindeer
graph uses Buck's checksum-verified crate downloads; remote execution and
worker network isolation still need acceptance on the configured executor.

### Cargo path

For production-rendered GUI checks, run
`nix build .#buck-npm-cache .#web-chromium --no-link`, then
`nix develop .#web -c scripts/verify-frontend-browser`. This runs pinned offline
frontend checks and Playwright/axe against a synthetic loopback HTTP/WebSocket
transport, without Cargo or provider calls. See `web/README.md` for evidence and
resource bounds.

`nix develop .#web -c scripts/verify-browser-journey` prepares frontend assets
before four focused native HTTP/WebSocket contract tests. Its backend phase
requires Rust; `--prepare-only` requires only pinned Node. Those native checks
are separate from rendered browser evidence.

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


Embedded clients may select the Lite wire contract explicitly with
`ResponsesClient::new(auth).with_protocol(ResponsesProtocol::Lite)`. The default
is Standard. Lite uses the same Responses SSE endpoint and Engine scheduler;
its transport header and input prefix carry namespaced tools and developer
instructions. Strict schemas are admitted before projection, and content-derived
prefix IDs stay stable across retries and exact-context forks. Durable effort
controls remain in Store history; Lite projects the latest effort into the wire
request. Tool calls retain their original names, call IDs and raw items.

The contract follows the retained native Codex source at
`d2d1d7c754a72f51087a54c230185831c649913b`: `core/src/client.rs` builds Lite
requests and the opt-in header, `tools/src/tool_spec.rs` groups the default
`functions` namespace, and `codex-api/src/sse/responses.rs` interprets the same
SSE events. Harness has no Codex code dependency. `request_body_for_protocol`
provides the selected body for offline diagnostics without authentication.
