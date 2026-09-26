# Wave13 completed — supervisor acceptance

Production integration `d2de055` passed the root's focused checks and the
supervisor's external ordinary-host lifetime gate. See
[handoff](docs/wave13-handoff.md) and [host evidence](docs/wave13-host-gate.json).
No successor assignment is active yet. Do not start adapter or credentialed
inference work. Preserve the port4600 demo and Tailscale configuration.

The preceding working notes below retain the investigation chronology; their
open-gate statements are superseded by the accepted handoff above.

# Current handoff — wave 13 host-lifetime gate

Read [wave-13 brief](docs/wave13-brief.md) and
[wave-13 handoff](docs/wave13-handoff.md). The standalone deterministic
browser harness code is integrated. The remaining consequential gate is
an **ordinary host-shell service/PID check** outside this tool's PID/mount
namespace. The root asked the operator in [questions](docs/questions.md)
to run that isolated launch/stop/reopen and report its host-visible
identity. Do not infer that namespace-local PIDs are host PIDs.

Integrated product source tested:
`d2de0554a0e8ac18e09aabc5619b30ba31546674`.
Reviewed launch `ef86255` and reviewed fail-closed preparation repair
`2e7e6f9` are merged. Preparation
`CARGO_BUILD_JOBS=1 scripts/prepare-browser-harness` exited 0 at that
source: web tests 14/14, production browser journey 1 matched/executed/
passed, release binary staged. Focused standalone production-binary test
1/1, server 8/8 and assets CLI 1/1 also passed at that source. See
handoff for exact evidence paths, failure-path coverage and the isolated
release-script exercise.

The release-script bundle was copied to `/tmp/wave13-isolated.c0Shds` and
launched from `/` on loopback port 51807 with isolated data. Login,
command completion and WebSocket reconnect worked. After observed
process loss, the same data reopened; SIGINT to the second exact
namespace PID stopped it. Port 51807 was confirmed released. The test
secret was removed. The protected wave-12 demo on port 4600 still
returned HTTP 200 and was not touched; Tailscale Serve was not changed.
No second-device test ran.

The Haskell focused-gate experiment is committed at `fbf08e6`.
Initial 8-GiB jobs never started because the protected demo reserves
1 GiB of the shared 8-GiB pool. After a 4-GiB correction, the helper
executed on two real consumers: server 8/8 and standalone 1/1 at
`cf19ed4`. Its strict clean-source predicate remained false because
runner `working_tree_status` was null here. The direct project script
remains the final check. See [friction](docs/exomonad-friction.md).
After the Tidepool host-source mismatch was reverted, a resident
Haskell smoke cell compiled and three reviewer interviews were
collected. The finished wave-13 child groups were retired through
typed cleanup; their worktrees, branches and evidence were not deleted.

## Accepted constraints and next owner

- Keep the existing port-4600 wave-12 demo and its data, login secret
  and Tailscale route untouched. Its handoff PID is namespace-relative.
- No live Exomonad adapter inference, credentialed item-2 retry, item-13
  trace, real shell tools, new scheduler, mailbox or durable journal.
- Use an isolated loopback port and separate durable data for the
  operator's host check. Record host-visible unit/PID, readiness,
  start/stop/reopen commands and release; do not claim a second-device
  check unless it actually ran.
- Root's tool sandbox could not reach the host user bus or authenticate
  to local SSH. **Next owner: host operator** for the one external
  service-lifetime check; root can incorporate its result and update
  product acceptance. Continue from the checked source, not from a
  cancelled helper command.

## Where the last run stopped

No unmerged branch holds reviewed or passing wave-13 code. Candidate
`19a6818` was rejected for stale-artifact behavior and superseded by
reviewed, merged `2e7e6f9`; do not merge it. Four host-cancelled
acceptance/docs workers and the first cancelled reviewer returned no
candidates. Their worktrees/branches remain retained but contain no
accepted delivery. The working tree may contain documentation/checkpoint
edits; preserve them and inspect scoped Git status before any commit.
