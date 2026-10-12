# Chat utility and palette

The operator selected Catppuccin Mocha on 2026-10-12. Solarized was an initial
idea, not a permanent brand constraint; agent-written guidance cannot make it
one. Existing fonts remain unchanged for this focused comparison.

The default uses the named colors from the upstream
[Catppuccin palette](https://github.com/catppuccin/catppuccin#-palette), following
its [role guidance](https://github.com/catppuccin/catppuccin/blob/main/docs/style-guide.md).
No palette package or runtime dependency was introduced.

| UI role | Mocha color |
|---|---|
| Canvas / code and control surfaces | Base / Mantle |
| Primary / secondary text | Text / Subtext0 |
| Decorative separators | Surface1 |
| Input and button boundaries | Overlay1 |
| Actions, links, keyboard focus | Blue |
| Required response | Yellow |
| Evidenced error | Red |
| Waiting | Mauve |
| Text selection | Surface1 with Text foreground |

The fixture-only comparison sets `data-palette="solarized"` on the document
root. Removing that attribute renders the selected default. There is no product
palette selector or persisted preference. Both versions use identical content,
layout, typography and interaction paths, so screenshots isolate palette effects.

Tool cards now show a literal first nonempty result line near their heading.
“Result excerpt” is content, not an inferred execution status. Result availability
and observed job states retain their original meaning. Evidence sits under one
disclosure per tool; ordinary messages include publication in Message details.
Exact source/result data and result-position navigation remain accessible.

At narrow widths, Read conversation deliberately collapses the composer to a
Write message/Resume draft strip with Interrupt still accessible. Reopening
focuses the retained textarea. The draft stays mounted; reading mode neither
submits nor discards it. Desktop retains the full composer. Recovery entry,
delivery feedback and errors stay visible in either mode.

Validation uses production assets with synthetic loopback HTTP/WebSocket data:
`web/e2e/chat-palette.spec.ts` renders both palettes at desktop/narrow widths,
checks semantic text/control contrast, verifies draft/focus preservation across
reading mode and disconnect, and exercises evidence access. The existing suite
covers submitted forms, streaming continuity, real browser zoom and recovery.
Evidence: `/srv/swarm/evidence/inanna/chat-palette-20261012`.
No live-engine validation, deployment or pin changes are part of this pass.

## Verification outcome

363 unit tests, 59 browser tests and 7 tooling tests passed, with production,
unit-test and browser TypeScript checks. The 390×844 fixture gains 144px of
conversation viewport height when switching from a drafted composer to reading
mode. Mocha's mapped secondary text is at least 7.37:1 and control borders at
least 4.44:1 against canvas/panel surfaces. Playwright/axe captures include both
palettes, submitted responses, interruption/disconnect, and actual 200% zoom.

The fresh worktree needed `//web:browser_contract` output copied into ignored
`web/src/generated` for browser TypeScript checking; the admitted wrapper and
initial failed runs are retained with the evidence. The final wrapper exited 0.
Impeccable's single detector pass reported the existing Inter family and three
selection/status accent borders. These are not demonstrated utility defects;
font changes and unrelated tree styling were left for separate decisions.

[Rendered review and palette comparison](https://swarm-01.sphynx-bitterling.ts.net:8445/chat-palette/).
