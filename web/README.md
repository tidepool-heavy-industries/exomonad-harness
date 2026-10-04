# Operator web view

The production entry point always checks `/api/session`, then connects to the
same-origin `/api/ws` after authentication. It waits for an authoritative snapshot.
Serving the asset directory alone does not provide a working operator session.
`src/fixture.ts` supplies unit-test examples; there is no default fixture mode or
`?live=1` switch.

For the standalone server, prepare the local release executable and production
assets with `scripts/prepare-browser-harness`, then launch with
`scripts/launch-browser-harness ABSOLUTE_SQLITE_PATH LOOPBACK_ADDR`. Preparation
requires the repository's Rust toolchain and pinned Nix Node environment. The
launcher uses the prepared executable and `web/dist`; it does not build them.
The embedded operator uses the host's existing actor capabilities, not the
standalone demo command grammar.

The embedded view starts at Tree. Each worker opens the same-tab `/chat/…` actor
page, which combines its messages, live output, input, interrupt/retire controls,
exact actor details, and host operation receipts. The worker list includes model
and workflow actors. Workflow pages show retained mailbox endpoint observations;
model pages show the exact associated model exchange history. Retired or lost
workers remain readable with commands disabled. Selected operations appear by
default; “All host operations” exposes the retained browser and host audit.
Previous `/host` and `?view=host` links open this same actor page.

## Frontend and rendered browser checks

From the repository root:

```sh
nix build .#buck-node .#buck-npm-cache .#web-chromium --no-link
nix develop .#web -c scripts/verify-frontend-browser
```

This frontend-only check copies the fixed-output npm cache to disposable scratch,
installs locked dependencies offline, selects the declared Buck TypeScript,
Vitest and sealed production asset outputs, typechecks the browser tests, checks
the evidence reader, then executes Playwright. Browser types and validators are
generated from the native Rust DTO exporter through those Buck dependencies. Node 24 and Chromium come from the existing flake pin;
Playwright and axe are development dependencies. Browser installation scripts and
ambient browsers are not used.

Playwright serves those production assets from an owned loopback fixture transport
on port 4387. Synthetic session, history, command status and WebSocket frames
exercise the actual renderer without provider requests, credentials, or a live
host. Fixture command receipts are supplied evidence, not proof of real host
admission, execution or cleanup. The tests cover light/dark themes, desktop and
390px views, axe checks, keyboard/URL navigation, draft and transport recovery,
history bounds, receipt handling, exact targets and large-list bounds. Actual
200% browser zoom is set and read through a test-only Chromium extension; it is
not a smaller viewport or pinch-scale substitution. Timing and heap observations
are retained, while performance gates check structural bounds.

Reports, screenshots, traces and provenance are retained per invocation under
`target/gui-browser/runs/`; `target/gui-browser/latest.json` points to the latest
run. The evidence reader rejects missing, malformed, stale or zero-execution
reports and preserves the original test failure. A failed or empty selection is
not a passing check. To run only a named
browser case after a current production build, use `npm run test:browser --
--grep 'case name'` from `web` inside the pinned shell. `npm run check:browser`
typechecks the browser target. Vitest collects only `src/**/*.test.ts(x)`.

On the shared Linux host, bound the owned browser/server process group, for
example with the user service manager:

```sh
systemd-run --user --wait --pipe --collect --unit=harness-frontend-browser \
  -p MemoryMax=2G --working-directory="$PWD" \
  "$(command -v nix)" develop .#web --command scripts/verify-frontend-browser
```

For ordinary frontend checks, build `//web:check`, `//web:test` or `//web:dist`
through the admitted pinned Buck environment. These targets provide the Rust
schema artifact, generated declarations and standalone validators. The Vite dev server
needs the protected API/WebSocket routes on its origin; static Vite alone does
not authenticate or supply the operator state.

## Backend contract checks and Buck

`nix develop .#web -c scripts/verify-browser-journey` prepares frontend assets,
then runs four focused Rust HTTP/WebSocket contract tests through
`scripts/cargo-focused-test`. It requires Rust for that backend phase.
`--prepare-only` performs only the frontend preparation and requires no Cargo.
These tests retain counted native execution evidence separately from Playwright's
rendered GUI evidence. They do not run Chromium or axe.

On an admitted x86_64 Linux checkout, `buck2 build //web:check //web:test
//web:dist` uses pinned Node and the fixed-output npm cache. It produces separate
TypeScript, Vitest and production asset outputs. Browser execution is a separate
frontend gate. Follow the repository/server build admission rules, configure the
checkout after pin changes, and use `--local-only -c remote.enabled=false` until
remote-execution acceptance is recorded. A new worktree needs its own provisioned
`buck-out` bind mount before Buck is used.


## Generated wire contracts

Rust owns the browser DTOs and their Serde schemas; see
[the contract and bundle boundary](../docs/browser-contract.md). Native integer
IDs, counters and cursors cross JSON as canonical decimal strings. Browser code
compares them with `bigint`, preserving values beyond JavaScript's exact number
range. The generated declarations and standalone validators are build outputs
under `src/generated`, supplied by `//web:browser_contract` to each web action.
They are not checked-in snapshots.

A production dist contains `browser-bundle.json`, with declared Harness source,
actual DTO schema and asset hashes. The composition root supplies the expected
source identity from its matched build inputs, and verifies the bundle before
listening. A manifest from an unrelated source or stale schema is refused.
