# GUI ownership and review

This pass preserves the Solarized Dark direction in `gui-usability-brief.md`.
The conversation remains the primary workspace. Code and results are visible;
publication identifiers and diagnostic payloads stay accessible through disclosure.

## Ownership

- `Operator` owns authenticated transport, snapshot projection, and retained command
  reconciliation. Session components render authentication and connection state.
  Authentication-expiry callbacks stay stable across observational snapshots.
- `App` owns routes, selection, filters, inspection focus, and cross-view operation
  reconciliation. Separate views render tree, timeline, inbox, and demo commands.
  Actor controls own their local drafts and action feedback.
- `UnifiedChat` composes one actor-scoped transcript. The conversation client owns
  page validation and HTTP reads; the history hook owns its bounded retained
  window, cursors, request generations, and refresh cadence. Rendering does not
  establish execution or command authority.
- The viewport hook receives its parent scroll element explicitly. It preserves
  anchors and selection while browsing, follows only when pinned, and observes
  page scrolling in short/zoomed layouts. Tool-result navigation focuses the exact
  invocation row through the transcript owner, without querying CSS ancestry.
- The form model owns validation, bounded draft retention, active branches, and
  revision comparison. The submission hook reconciles HTTP acknowledgements with
  native settlement. The focus hook retains intent for the exact attempt and
  abandons it when the user acts elsewhere. Field components render that state.
  Native form submission makes Enter use the same admission path as Submit.
- Rich-view validation is independent of React/CSS. Feature styles belong beside
  their renderers; shared palette, typography, preview bounds, and attention
  clearance remain in the existing tokens stylesheet.

Keep these boundaries when adding features. Do not add a second history store,
copy transport state into presentational components, or infer successful execution
from a loaded result. Keep exact actor/operation associations intact.

## Rendered review

`web/e2e/visual-review.spec.ts` uses the existing loopback fixture transport with
production assets. Shared chat fixtures include full prose, substantial Haskell,
a compile failure, and an inline form. Screenshots cover desktop, narrow and
320px layouts, actual 200% tab zoom, streaming and retained publication, validation,
submitted responses, recovery, authentication, and every surrounding view.

Screenshots are retained review artifacts, not pixel goldens. Open them directly
and inspect hierarchy, readable content, focus, containment, and reachable actions.
The helper captures Chromium's actual compositor viewport: CSS-coordinate capture
can produce clipped or blank images under real tab zoom. For long nested content,
scroll to the relevant part and capture each viewport rather than requesting a
stitched element screenshot that includes pixels outside its scroll container.

Run the declared frontend checks and `scripts/verify-frontend-browser` using the
pinned Nix/Buck setup described in the repository and server guides. Record the
exact source revision, working-tree state, counts, logs, and screenshot index.
Fixture screenshots and tests establish frontend behavior, not live host execution.

## Deliberate constraints

No HTTP/WebSocket schema, generated contract, dependency, host cancellation,
release pin, or deployment changes. Existing polling intervals, history bounds,
command identities, acknowledgement/settlement semantics, and actor focus repair
remain intact. Browser tests cover actual keyboard form submission and reading
position during page scrolling, in addition to existing lifecycle race tests.
