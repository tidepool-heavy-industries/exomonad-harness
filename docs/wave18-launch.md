# Wave18 launch record

## Assignment

Astra High root with parallel Luna feature-area trees. Deliver the standalone
browser harness milestone, prioritizing a deterministic asynchronous tool lifecycle
slice. Evaluate the composed Haskell workflows and return concrete interface feedback.
GPT-6 is the model target; the destination is an Exomonad-specific home with typed
mailboxes, event hooks and Haskell continuations. See [brief](wave18-brief.md).

## Preparation evidence

- Tidepool launch code: `d707d18ed` (matched incremental build passed).
- Published shared workspace: `8a21b71fdb9ca2cc7b241a5f3dd97a83b6c45d84`.
- Shared source and changed template files match byte-for-byte.
- Focused implementation and live Jev evidence: Tidepool
  `docs/reports/wave17-to-18-implementation.md` and `wave17-audit/jev-synthetic-trials.md`.
- Staged standalone release journey passed at harness `6ae6521531439f6bad168a3d55f7f086be5b7623`:
  frontend typecheck,14 web tests, production assets, release build and the
  production-launcher browser journey1/1. Binary SHA256
  `7270adb43dfeea1a469cc9e941f4fecefc652086c7fa6d486f6e3bfa4bcd94fd`.
  Later pin/trial/launch-record changes do not modify product Rust or release scripts.
  Log `/tmp/wave18-startup/browser-release.log`.
- Twelve release/preparation script tests and Cargo formatting passed.

## Predecessor

Wave17 root and worker interviews were retained before stop. Final roster had no
active or queued requests; `exomonad stop` succeeded and the tmux session and host
process were gone. Evidence: `/tmp/wave18-startup/wave17-final-actors.json` and
`wave17-stop.log`. Source/worktrees/commits and existing demo/Tailscale state retained.

## Admission and observed start

- Session: `wave18`; run: `29e61b63-2bc9-45d5-b5d1-b67e20918bea`.
- Checkout: `/home/inanna/dev/exomonad-harness-runs/wave18`, branch `rsi/wave18`.
- Harness launch revision: `5484b2a85f1db21f5f56a259226e64a42ba72615`.
- Root: `1@1`, `gpt-6-astra`, high effort.
- Provider thread: `01a0e011-ac4e-79e0-86f6-67d10bade76f`.
- Frozen Exomonad SHA256:
  `7b9a740735c5946bc28cd7b1692708ef900b32b4404e9749c079d8eb793cdcea`.
- Codex SHA256:
  `6d6f7bb755e380b050e0becbdfded933c19fe1d51f73329ea7c9acf192f4cc82`.
- Matched build, Rust formatting, scaffold pin test (1 passed), and exact launch
  checkout workspace preflight passed. Preflight definition:
  `91a097068cb9439d140e2d68f98f5dbaedd2e59d50160b862057f20a32e327bb`.
  All configured modules compiled and launchable children satisfied required
  effects; preflight did not invoke providers.
- Host started `2026-09-26T23:32:55Z`; root ready `23:34:10Z`;
  first provider turn `23:34:44.357Z`; first tool `23:34:47.850Z`.
- Initial tools read NEXT, the wave18 brief, async source notes, AgentSpec,
  automation menu, local helpers and shared workflow implementations. This proves
  exposure and orientation, not helper usage or child delegation yet.

Host log:
`/home/inanna/dev/exomonad-harness-runs/wave18/.exomonad/logs/29e61b63-2bc9-45d5-b5d1-b67e20918bea.log`.
Root transcript:
`/home/inanna/.codex/sessions/2026/09/26/rollout-2026-09-26T16-34-09-01a0e011-ac4e-79e0-86f6-67d10bade76f.jsonl`.
Build, preflight, release, admission and notification receipts are retained under
`/tmp/wave18-startup/`. Subsequent documentation commits do not change the active
run's launch baseline.
