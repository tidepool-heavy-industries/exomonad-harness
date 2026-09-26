# Wave 14 launch record

## Source and gates

- Harness branch: `rsi/wave14`, worktree `/home/inanna/dev/exomonad-harness-runs/wave14`.
- Admission HEAD: `5c398bc94d8f8c480ae14fe1032211803b6473a2`;
  accepted master baseline `36cdebf01f7b0792cf6828ee0e0d4ab045e3b61e`.
- Workspace pin: `98aef850d7be1b322aebcea929d62de3a56ab92f`.
- Tidepool launch checkout: `15306f7c4`; production source `42db232c0`.
  Matched `just exomonad-build` passed, followed by incremental init build.
  Workspace `exomonad check` passed; catalog 39, Codex 0.155.1.
- Runtime fixes include atomic tmux insertion `fece13978`, launch failure
  preservation `3cbb987d9`, oracle input fix `42db232c0`.
  Command schema extension remains reverted to protect live runtime sources.
- New worktree submodule gitfile normalized to its absolute existing Git
  directory. Host Git checks succeeded. Root subsequently reported actor-view
  `git status --porcelain` succeeded before forks, showing dirty helper files;
  this is not a clean-tree claim. See NEXT.md checkpoint.

## Runtime identities

- Session: `wave14`.
- Run: `08a7d4c5-4821-4887-8a07-42470a08029b`.
- Root: actor `1@1`, Sol Medium; native thread
  `01a0dc67-2aed-7eb0-95d5-15dedc7ef92f`.
- Logs: `.exomonad/logs/08a7d4c5-4821-4887-8a07-42470a08029b.log`
  and adjacent `.jsonl`; launcher output `/tmp/rsi-wave14-launch.log`.
- Host started 2026-09-26 06:27:58 UTC; root ready 06:29:04 UTC.
  Initial instruction visibly executing approximately 06:31:05 UTC.
  Initial tmux paste required a second Enter before execution; no duplicate task.
- Startup compile request: 40.295 seconds; spec preparation: 22.895 seconds.
- Three children launched 06:34:40–06:34:55 UTC after root's compiling
  shared contract `81ef34ce05c4c9bc823f6d495befd485055cbbc8`.

## Acceptance and experiment

Follow [the bounded brief](wave14-brief.md). Provider/Store, Engine and
independent expected-red production acceptance are separate obligations.
The composed focused-check helper must be consumed by both implementation
owner and reviewer; publication or two root targets alone is insufficient.

Wave13's product and external host gate were accepted before this launch.
Its host and compiler stopped; uncertain process/workspace custody remains
retained, not force-released. No second working wave was launched.

Protected wave12 demo remained HTTP 200 on localhost:4600 at 06:36 UTC.
Its process, data, Tailscale route and resources were not changed.
Early wave14 audit found Jev HTTP 403 authorization refusals and circuit-open
abstentions; ordinary tools continue. No product completion is claimed here.
