# web view: taste, craft, tooling (for web-principles first, then every screen leaf)

fmt: compressed. this is the design brief the `web-principles` leaf turns into `docs/principles.md` + `web/src/tokens.ts`. `!` hard rule. named products are touchstones to look at, not to copy.

## the feel
- a dense professional instrument, not a dashboard and not a chatbot. touchstones: Linear (density + keyboard), a Bloomberg terminal's information-per-pixel with Swiss typographic discipline instead of its chrome, Observable notebooks (data feels alive, code sits beside output), Tufte's graphics (data-ink, direct labels, small multiples), Rams: as little design as possible.
- the page is about the tree of thinking machines the operator is holding. everything on screen answers one of: what is running · what is waiting on me · what did it cost · what happened here. if a pixel answers none, remove it.
- monochrome by default; color is a signal, never a coat. motion only for continuity, never for delight. no gradients, no shadows deeper than 1px, no rounded-bubble chat styling, no cards-in-a-grid, no spinners, no toasts for state, no modals for anything the page can show in place !

## type + grid (tokens, not css literals !)
- one sans (Inter or IBM Plex Sans), one mono (IBM Plex Mono / JetBrains Mono) for ids, hashes, code, numbers in tables. tabular numerals everywhere numbers align.
- scale: 11 / 12 / 13 (body) / 14 / 16 / 20. line-height 1.35 body, 1.25 dense rows. weights 400/500/600 only.
- ids: mono, truncated to 7 chars + `…` w/ full value in tooltip and copy-on-click. paths (`/root/core/core_store`) in mono, segments separable.
- 4px base, 8px rhythm; row heights 24 (dense) / 32 (default). two panes w/ a draggable splitter (min 240px), state in URL.
- density: one. no "comfortable/compact" toggle in wave1.

## color (oklch ramps; light + dark from the SAME tokens)
- neutral ramp 12 steps; text on background ≥ 7:1 body, ≥ 4.5:1 secondary. every meaning color passes 3:1 against its background in both themes !
- meaning colors, and only these: `pending` (amber), `failed` (red), `waiting-on-operator` (violet), `effort` (3-step blue ramp low→high), `selected/focus` (one accent). cached-prefix ratio, tokens, cost = neutral ink + position/length, never hue.
- the same meaning has the same color on every screen; a legend is a failure (direct-label instead).

## streaming text (the heart of the node window)
- deltas append to ONE text node per message; never a DOM node per token !; batch by `requestAnimationFrame`; caret block at the end while streaming; no reflow of earlier lines (reserve line-height; fixed column widths for meta).
- reasoning summary streams in a collapsed, dimmer block above the answer; auto-collapses on completion; expandable.
- function-call args stream into a mono block w/ the tool name as a label; job progress envelopes render as a quiet inline line under the call; the settled output replaces the streaming view in place, same box, no jump.
- completed item swaps the delta view atomically (same height or grow only; never shrink while the user reads).

## d3 style (d3 computes, React renders; canvas when it must)
- split: d3 for math (scales, `d3-hierarchy`, `d3-shape`, `d3-zoom`, `d3-brush`, `d3-time`), React owns the DOM for ≤ ~300 nodes/spans; above that the graph layer renders to canvas w/ a hit-test index (quadtree) and React draws only the HUD (labels, selection, tooltips). one component, two backends, same data.
- tree screen: lanes = conversations, y = time, x = lane; branch curves `d3.linkVertical` w/ the fork point marked; a node's open request = span growing in real time; idle intervals compressed on the time axis w/ a visible break glyph (∿) so a 40-minute wait doesn't flatten an hour of work; minimap; semantic zoom (labels appear past 1×, hashes past 2×); brush a time range → the timeline tab filters to it; crosshair w/ time readout; hover = highlight lineage (ancestors + descendants), click = pin, shift-click = compare (later).
- timeline: spans as rounded rects (r=2), request spans dark, job spans light, wait spans hatched; a thin usage sparkline per node above its row (tokens per request, cached fraction as fill); direct labels on the longest spans only; sparse gridlines (major ticks), none horizontal.
- axes: `d3-time` ticks, relative ("+4m12s") by default, absolute on hover; tick labels at 11px mono.
- every graphic has a text alternative: the same query rendered as a table one keystroke away (`t`), aria-labels on nodes ("core_store, luna medium, requesting, 12 items pending").

## responsiveness (numbers, measured in CI where cheap)
- input → paint < 50ms; delta → paint < 1 frame after rAF; 60fps while streaming into one window and the tree animates one span.
- event application: state-lane events reduce into a normalized store keyed by ids (conversation, request, job, envelope); delta lane into per-item transient buffers; NO derived durable state from deltas !; seq gap ⇒ resync from snapshot.
- virtualize every list > 50 rows (`@tanstack/react-virtual`); memoize row components by id+version; no layout reads in scroll handlers; layout of > 2k nodes in a worker (`d3-hierarchy` is pure; it moves).
- bundle: initial ≤ 300 kB gz; screens code-split by tab; `size-limit` in CI; fonts subset + `font-display: swap` w/ metric-compatible fallback (no layout shift).
- subscriptions: one WebSocket; query subscriptions diffed server-side; client applies patches; optimistic UI for commands (a command shows as "sent" instantly, reconciled by its result event; never fakes the result).

## interaction craft
- keyboard first ! `⌘K` palette; `j/k` move, `enter` open, `esc` back, `/` search, `g t` tree · `g i` inbox · `g w` window · `g p` palette, `[`/`]` prev/next node, `.` focus composer. every shortcut discoverable in the palette. visible focus ring (2px accent, offset 2) on everything focusable.
- empty states teach: an empty inbox says what would appear here and the shortcut to the tree. loading = skeletons with the final dimensions, never spinners.
- errors inline where they happened, w/ the typed reason and a retry; refusals (cold prefix, not owned) render as a quiet notice w/ the accept action, same shape as the model sees.
- deep links ∀ node ∀ request ∀ envelope; the URL is the pane state (left width, open tabs, active tab, time brush). back button works.
- forms (rjsf) render inline in the notebook cell that raised them: our Radix theme (no rjsf default look); enum ≤ 4 → segmented control, else select; live validation; `enter` submits, `esc` dismisses w/ confirm (a dismiss is `NoAnswer` to the model and the page says so).
- respect `prefers-reduced-motion` (kills every transition), `prefers-color-scheme`, high-contrast; zoom to 200% without loss.

## components + tooling
- Radix primitives unstyled + our tokens; never Radix Themes' default look. use: Tabs, ScrollArea, Popover, Tooltip (delay 300ms, none on touch), ContextMenu, Toggle, Separator. Dialog only for destructive confirms (there are none in wave1 ⇒ no Dialog).
- state: one small store (zustand or a reducer) fed by the protocol client; types GENERATED from rust (`schemars` → json-schema → `json-schema-to-typescript`), never hand-written !
- vite · TS `strict` + `noUncheckedIndexedAccess` · eslint (typescript-eslint strict, jsx-a11y, react-hooks) · prettier · vitest + testing-library for logic and components · ladle for components against recorded event fixtures · playwright for screen screenshots and `axe` a11y checks against the same fixtures · `size-limit`.
- evidence per screen leaf: a recorded fixture (one acceptance session's event log) + screenshots at 1280×800 light and dark + the a11y report, committed under `web/evidence/<screen>/`. the lead reviews screenshots against `principles.md` before merging.

## method (unchanged, restated for the leaf)
principles doc (school + pattern pair per screen) → tokens before components → ∀screen states {who looks, needs to know, does next} → build against fixtures → critique by interview w/ screenshots vs principles → deviations logged w/ reason → principles amended when a deviation recurs.

## anti-patterns (each is a nudge)
cards-in-a-grid · chat bubbles · spinner · toast for state · modal for state · legend instead of direct labels · heavy gridlines · gradient/shadow decoration · DOM node per token · layout read in a scroll handler · d3 and React both touching one DOM subtree · hand-written protocol types · color literal outside tokens · mouse-only action · unreserved space during streaming · fake optimistic results.
