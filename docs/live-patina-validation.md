# Live Patina implementation and verification

Base: published `origin/integration/chat-palette-20261012`, `9f64c05`.
Worktree: `harness-live-patina-20261012`; branch: `gui/live-patina-20261012`.

This first adoption implements the bible's petrol surfaces, warm ivory text,
verdigris primary actions, mint selection/focus, brass attention, readable coral
errors, rounded controls, and 14px plain-source code. Sampled muted/border/error
seeds were lifted for contrast. IBM Plex Mono now ships locally with its upstream
license so font availability cannot silently turn source into proportional text.
Syntax highlighting and a wholesale transcript layout redesign are future work.

## Executed checks

The pinned `swarm-build nix develop .#web` environment ran Buck local-only frontend
checks, the browser contract, production assets, and `scripts/verify-frontend-browser`.

- Production/test/browser TypeScript checks passed.
- 363 unit tests in 32 files passed; 7 tooling tests passed.
- 59 browser tests executed and passed, no skips or failures in the confirmation.
- Axe and overflow checks cover desktop, narrow layouts, and actual 200% zoom.
- Behavior includes forms/submitted history, streaming/selection, draft retention,
  disconnect, recovery, navigation, and keyboard input.
- Live Patina text-role contrast on canvas/panel passes 4.5:1; control/accent roles
  pass 3:1. Supporting text on panel is 6.47:1; control boundary is 4.45:1.
- Mint action/hover/selection with deep ink passes 4.5:1. Rendered Submit default,
  hover, pressed and keyboard focus plus Send default/disabled styles are checked.
- Font loading and equal-width narrow/wide glyphs are checked, not just CSS names.

Evidence directory: `/srv/swarm/evidence/inanna/live-patina-20261012`.
Final browser run: `target/gui-browser/runs/20261012T224800-3211886`.
Exact launcher script and logs: `check.sh`, `check.log`, `check-2.log` in evidence.
First run passed 57/59; two new tests incorrectly expected keyboard focus styling
from programmatic focus. They now exercise actual Tab navigation. Direct review
also caught the host's missing monospace font; the confirmation covers its repair.

Independent Sol review: both code-font and action-state evidence gaps resolved;
ship this theme adoption. Detector retained existing Inter and tree-selection
border warnings. No new design concerns require expanding this pass.

Screenshot gallery: https://swarm-01.sphynx-bitterling.ts.net:8445/live-patina-ui/
These are production frontend assets with synthetic transport, not live-engine
acceptance. No live deployment, shared service restart, or Tidepool pin changes.
