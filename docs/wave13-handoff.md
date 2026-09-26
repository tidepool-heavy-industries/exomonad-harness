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

## External host gate — completed by supervisor

At 2026-09-26 06:10 UTC the operator copied the exact prepared binary, assets
and launcher to `/tmp/wave13-host-gate-zn4v9dqj` and launched from `/` through
host user systemd. No Cargo ran at launch. Binary and index hashes matched the
prepared bundle; [retained evidence](wave13-host-gate.json) records hashes,
source, invocation IDs, PIDs, namespace, cgroup and outcomes.

The first successful unit was `wave13-host-gate-zn4v9dqj-b.service`, host PID
3669031, parent 1513 (user systemd). Its PID namespace matched the operator's
host shell, and the service remained active after the launch command returned.
On isolated port33367, static/session HTTP, login, authenticated WebSocket
snapshot/reconnect and `echo host-gate` succeeded. Exact unit-main SIGKILL
released the port. Reopening the same database as unit c, host PID3670139,
retained the completed command and its detail. Exact SIGINT then exited0;
the unit became inactive and the port was released.

The first attempt failed before application startup because this host's user
systemd PATH omitted Bash. Successful units supplied
`PATH=/run/current-system/sw/bin:/usr/bin:/bin`. This is a launch prerequisite,
not an application failure. A fresh private secret was never printed, and its
mode0600 environment file was removed after stopping the isolated instances.
The protected port4600 demo remained healthy; no Tailscale configuration or
existing secret/data changed. No second-device tailnet verification is claimed.

The ordinary-host lifetime gate is now closed. The product evidence does not
claim a two-actor helper experiment or clean-source metadata that was unavailable.

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
