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

Persistent stores use WAL with `synchronous=FULL` on every writer connection.
Schema 16 records this durability contract. Opening an older store first confirms its
inherited WAL with a FULL checkpoint; a blocked checkpoint refuses admission
before retained bindings can authorize attachment. Matching current writers
reopen without that barrier. Stop older writers before migration: a version
marker cannot change an already-open connection's mode. Older bundles refuse
newer schemas, so preserve the matching bundle for unmigrated runs. Schema 17
records provider issuance, indexed output ownership and explicit inference
recovery grants. Legacy response records retain unknown transport provenance;
opening a store never certifies it from the current provider configuration.

A successful binding COMMIT is durable acknowledgement. Checkpoint maintenance
does not revoke it. Any explicit Store COMMIT error or failed transaction
completion closes its serialized connection, fences sibling handles and notifies supervision. The first
failure retains its SQLite cause as `StoreError::RecoveryRequired` (or
`EmbeddedError::RecoveryRequired`); later access returns `StoreError::AdmissionFenced`.
The embedding must discard admission handles and reopen with retained authority
before another operation. Binding readback alone grants no attachment authority.

The bundled SQLite is 3.53.2 through exact rusqlite 0.40.2 and libsqlite3-sys
0.38.2. This includes the WAL-reset fix absent from the older 3.46.0 bundle;
the focused store controls verify the library linked by Buck at runtime.

Embedded inference uses a durable Store gate independently of actor lifetime.
Messages can queue while blocked. An explicit `Recover` command names the exact
run, incarnation, request and block generation; its checked projection, once-only
authorization and receipt commit together in the existing command journal.
Recovery refuses uncertain affected work and never reexecutes effects. Send
atomically admits text once and requests exact continuation; queued input carries
one intent bound to the running attempt. Projection refusal keeps that text queued
and reports a typed reason. Visible history continuation requires an explicit
operator command and records omissions while preserving canonical history and
operation identities. Only transient EOF/read failures without completed current
items or dispatched tools receive automatic retries after one and three seconds;
the Store retains this two-retry budget across restart and manual continuation.
Stop fences scheduled retries and existing grants. Exclusive host startup fences
earlier attempts and unused grants; passive Store reopen and wake notifications
grant no inference. Schema 19 retains retry lineage and input continuation intents;
older bundles refuse the newer schema.

Usage observations serialize as version 2. Cache read/write counters are
independently optional: `null` means unavailable, while zero means the provider
reported zero. The usage reader treats zeros in historical unversioned records
as unavailable because the previous parser synthesized them for missing fields;
positive historical counts remain observed. Input/output `reported` does not
establish cache-counter availability. Existing records are never rewritten.

Native response failures carry bounded metadata through the existing provider
event stream into `transport_failure_summary` Store events (version 1). These
records contain allowlisted headers, diagnostic IDs, event counts and kinds,
terminal disposition, and unfinished-frame byte lengths. Unknown event labels
are hashed; bodies and opaque content are excluded. Diagnostic IDs may be
redacted or truncated and never establish replay provenance. Missing historical
summary events mean unavailable observations, not zero activity. The new event
is additive and does not rewrite existing events.

`response.incomplete` is a typed explicit terminal with an ID and reason,
distinct from clean EOF and HTTP body-read failure. Neither it nor metadata
authorizes inference. The Engine's existing failure assessment, retained items,
operation cleanup and Store continuation grant own that decision. Body timings
start after response headers arrive. An unfinished SSE event is discarded at
EOF; its bytes are never promoted into a completed message or tool invocation.

A host may issue one opaque `RequestActivation` per native request and retain it
in every immutable `Provider::request_snapshot` for that activation. The scheduler
pins each operation's `InvocationRole` and inventories queued and running domain
work across provider responses using its existing jobs registry. Unconfirmed
cleanup remains pending; completed unread output does not. Settlement and
observation controls are excluded. `CallContext::invocations` admits a native
request claim synchronously under the same lock as job registration and physical
start. The callback takes the native request lock second, never awaits or
reenters the scheduler, and may refine inventory only using exact native terminal
facts. Refusal leaves domain admission open; acceptance fences that activation's
new domain starts and returns an exact operation-bound `InvocationTerminal`.

The native owner attaches that token to `ProviderCompletion::activation_terminal`
only for its accepted settlement. This authority is independent of `full_success`,
which gates context edits: a native request transfer may intentionally discard
those edits. Engine commits ordinary output and eligible context edits, executes
remaining response-order controls, then returns before successor
inference or further inbox admission. The reusable actor and a successor
activation's queued input remain owned by the host. Unconfirmed transfer cleanup retains the existing cleanup obligations and stops
inference. Activation-scoped settlement must use `BeforeNextInference`; async
settlement is refused before claim or dispatch. Output JSON, tool names,
schema equality, passive replay and Store reopen issue no terminal authority.

## Focused tests

### Native Buck2 on x86_64 Linux

The Buck2 graph compiles `harness` and `harness-demo` as native Rust targets.
Reindeer generates the locked third-party Rust targets; Buck does not delegate
the crate build to Cargo. The web `check`, `test`, and `dist` targets run pinned
Node actions against a fixed-output offline npm cache. Cargo remains the
publication and non-Linux development path. The [server acceptance record](docs/buck2-acceptance.md)
lists exact checks, resource use, and pending gates.
Run Buck from an admitted build environment with the checkout's provisioned
`buck-out` bind mount. Stop if the `findmnt` check below fails. Keep remote
execution disabled with `--local-only -c remote.enabled=false` until its
project gates are recorded.
For `scripts/buck-focused-test`, `--local-only` supplies both Buck options.

```sh
nix build .#buck2 .#buck-rust .#buck-cc .#buck-binutils .#buck-node \
  .#buck-python .#buck-npm-cache --no-link
findmnt --mountpoint "$PWD/buck-out"
scripts/buck2-configure.sh
BUCK2="$(nix eval --raw .#packages.x86_64-linux.buck2.outPath)/bin/buck2"
"$BUCK2" build //crates/harness:harness --local-only -c remote.enabled=false
"$BUCK2" build //web:check //web:test //web:dist --local-only -c remote.enabled=false
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


Inference uses native ChatGPT-plan OAuth token sharing and the public
`https://api.openai.com/v1/responses` Standard HTTP endpoint. `ChatGptPlanAuth`
owns selected registration refresh; `ResponsesClient` owns a shared lazy HTTP
pool. Full-history requests use `store:false`, `stream:true`, retained encrypted
reasoning, and zstd request compression with plain JSON fallback if encoding fails.
Empty tools and tool choice are omitted, as is the default parallel-tool flag.
Host async declarations and every returned native replay field remain intact.

`EngineConfig.session_id` is a typed `SessionIdentity` issued before the request
is retained. Its immediate parent thread wins over its exact fork source;
otherwise the current thread is its own affinity. `session-id` carries affinity,
while `thread-id` and `x-client-request-id` carry the current thread.
`prompt_cache_key` uses the same affinity, strips `ses_` only before exactly
64 hexadecimal characters, and clamps to 64 characters. Replays retain the
resolved identity; they never consult current caller configuration or lineage.
`request_body` provides the production normalized JSON for offline diagnostics.

Private Codex credential files, private backend routing and Responses Lite are
unsupported. Old private route/protocol records are rejected rather than relabeled;
start a new public-plan session after native sign-in. Existing public records with
a scalar session ID restore that ID as both thread and affinity.

Synchronous context drafts may stage the next reasoning effort together with
transcript and model edits. Successful publication appends the host-owned
configuration update after the tool output, preserves the earlier request input,
and supersedes effort choices pending at commit admission. Later pending choices
retain their ordinary next-request behavior. Failed or cancelled publication
leaves the context, model, effort and pending choice unchanged.

Store projects closed, evidenced Responses groups into readable, source-linked
notes when their opaque provider state belongs to another model. This projection
is mandatory before transport and is checked against the exact successful output
before a synchronous model change commits. Notes retain supported visible text,
reasoning summaries, tool inputs and results. Pending exchanges, incomplete or
ambiguous response membership, unknown issuing models and unsupported content
refuse portability. Opaque server compaction remains on its evidenced issuing
model. Context drafts cannot remove or convert opaque native groups to notes.
Raw history, claims, context documents and replay receipts retain their original
native items and source references. `getContext` exposes editable message parts
and exact settled result bodies inside native groups, including groups with
other pending calls. Visible edits are occurrence-local overlays: outgoing
same-model requests preserve every retained opaque item, native identity and
position while carrying the edited body (for example, `[Trimmed: repetitive
output]`). Tool inputs, reasoning and native metadata remain read-only. Edited
opaque history refuses a model switch rather than dropping reasoning. Scheduling,
recovery and terminal evidence always read the original canonical items.
Snapshots, forks, saved-context restoration and reopen retain the overlays.
Exact issued replay requests record the projected items and their hashes.
Portability runs no effects.

Context commit receipts use internal format version 3 and accept overlay-free
version 2 receipts, including acknowledged or retried replay restorations. Database
schema 11 adds nullable occurrence overlays; older rows retain identity projection.
Version 1 receipts are rejected with `ContextError::UnsupportedState` when read for acknowledgment,
recovery or replay; retained version 1 runs require explicit migration or
recreation before this candidate can resume them.
