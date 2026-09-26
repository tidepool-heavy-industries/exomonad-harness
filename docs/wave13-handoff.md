# Wave 13 handoff — standalone deterministic browser harness

## Source and outcome

Reviewed launch `ef86255` and reviewed fail-closed preparation repair
`2e7e6f9` are merged on the wave-13 branch. Production integration source
tested below: `d2de0554a0e8ac18e09aabc5619b30ba31546674`. Final
documentation/checkpoint commits, if any, must name their new OID separately.

The ordinary-host commands are `scripts/prepare-browser-harness` and
`scripts/launch-browser-harness ABSOLUTE_DB LOOPBACK_ADDR
[ABSOLUTE_ASSETS]`, with optional absolute `HARNESS_DEMO_BIN`. Preparation
builds assets and a release executable, then stages it at
`target/release/harness-demo` even when `CARGO_TARGET_DIR` points elsewhere.
Failed preparation invalidates that default executable; the launcher does
not invoke Cargo. It reads the login secret only from
`HARNESS_DEMO_SESSION_SECRET` in the environment. The legacy CLI form without
`--assets` remains accepted from the project root. Browser `--serve` still
uses deterministic Engine/Store and child-message behavior, not credentialed
inference, an Exomonad adapter, or real shell tools.

The durable database is selected by the operator and must be outside a
disposable source/build directory. Copy the release binary, web assets and
launcher to stable absolute host paths before discarding the preparation
checkout. See [operator instructions](standalone-browser.md).

## Checks at `d2de055`

| Check | Selection / execution | Evidence |
| --- | --- | --- |
| `CARGO_BUILD_JOBS=1 scripts/prepare-browser-harness` | Exit 0. Web tests 14/14; `browser_journey` 1 matched, 1 executed, 1 passed; release build completed and staged binary at `target/release/harness-demo`. | Retained command `a6884c74-32b5-40c1-9940-423eda156c10`; `.exomonad/build/cargo/debug/deps/focused-a0234iiw/evidence.json` |
| `scripts/cargo-focused-test --package harness-demo --target test:standalone_browser --filter standalone --expect 1` | 1 matched, 1 executed, 1 passed. Production binary from unrelated directory: missing assets/no DB or listener, authenticated `echo` and child message, clean SIGINT reopen, SIGKILL/reopen same SQLite data. | `.exomonad/build/cargo/debug/deps/focused-y6tez0mi/evidence.json` |
| `scripts/cargo-focused-test --package harness-demo --target bin:harness-demo --filter server --expect 8` | 8 matched, 8 executed, 8 passed. | `.exomonad/build/cargo/debug/deps/focused-th11uwwv/evidence.json` |
| `scripts/cargo-focused-test --package harness-demo --target bin:harness-demo --filter assets_cli_is_absolute_and_serve_only --expect 1` | 1 matched, 1 executed, 1 passed. | `.exomonad/build/cargo/debug/deps/focused-yebnyyqq/evidence.json` |

All four focused evidence JSON files identify `d2de055` as their source.
The runner recorded `working_tree_status: null` in this checkout because
ordinary Git status traverses a broken nested module gitdir. This is
missing cleanliness evidence, not a test assertion failure. Cumulative
ownership checks and scoped Git status were inspected separately.

## Isolated release-script exercise

The release binary, launcher and `web/dist` were copied outside the checkout
to `/tmp/wave13-isolated.c0Shds/{bin,scripts,web/dist}`. Database:
`/tmp/wave13-isolated.c0Shds/data/session.sqlite` (mode 600). From `/`,
the copied launcher started on `127.0.0.1:51807` with absolute
binary/assets/database paths. A synthetic temporary login secret was
provided through the environment, never as a process argument or committed
file. It has been removed; the prior wave-12 secret was not read or changed.

The first release process outlived the launching command. HTTP session
readiness returned 200; login succeeded; `echo release-host` was accepted and
an authenticated WebSocket reconnect snapshot showed its request completed
with the expected detail. The retained launch command's exact scoped
cancellation stopped that first process; its terminal/cleanup receipt was
`CommandUnconfirmed`/`CommandRetained`, so this is **observed process loss**,
not evidence of a clean signal. A second launch over the same database
served the static index, reauthenticated and returned the previous completed
request in a WebSocket snapshot. It was stopped with SIGINT to its exact
namespace PID; the process exited successfully. Port 51807 was confirmed
released with curl and `ss`. The two recorded PIDs (10610 and 10689) are
**namespace-relative**, not host-visible identities.

The protected wave-12 port 4600 still returned HTTP 200 after these checks.
Its service, data, secret and Tailscale Serve configuration were untouched.
No second-device tailnet check ran.

## Open host gate

This tool sandbox cannot reach the host user service manager
(`systemctl --user`: “No data available”), and local SSH did not
authenticate. It therefore cannot report a **host-visible** PID/unit or
claim that a host service outlived the agent's mount/PID namespace. The
operator has been asked in [questions](questions.md) to run one isolated
prepare/launch/stop/reopen from an ordinary host shell and report the exact
service/PID. Until that occurs, namespace-local production checks establish
the implementation and recovery behavior, but the external host-lifetime
acceptance gate remains open. The next owner is the host operator for that
one check; no credentialed inference or adapter work is authorized.

## Focused-gate helper trial

`SessionHelpers.TestEvidence` was imported and published. Its initial
8-GiB memory request could never be admitted in the shared 8-GiB command
pool while the protected demo reserved 1 GiB; both queued jobs were
cancelled before opening streams. After changing to 4 GiB, two **actual**
uses at `cf19ed4` executed: server 8/8
(`focused-hgmvh3_m/evidence.json`) and standalone 1/1
(`focused-_5bvmybi/evidence.json`). Each required `startFocused` and
`finishFocused` rather than one direct script call. Typed evidence paths,
source OIDs, counts and failure retention were useful; no turn saving was
measured. `focusedPassed` stayed false because `working_tree_status` was
null, preserving that uncertainty. No Jev failure triage was needed.
The project script remains the authoritative fallback and final gate.
