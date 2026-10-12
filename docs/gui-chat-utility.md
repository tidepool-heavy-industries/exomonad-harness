# Chat utility implementation

Implements items 1–4 from [the Impeccable review](gui-impeccable-review.md), on
`gui/chat-utility-20261012`, based on `4e07d465b909d0ae59de2d12a42c18bd0c85ad10`.

On narrow screens the worker list and session actions disclose on demand. The
composer stays compact until focused or carrying a draft; switching workers
retains their separate drafts. Form navigation aligns the form heading below
the measured pending-response banner. The count remains qualified to loaded
history.

The composer identifies its exact recipient, discloses run/incarnation identity,
and provides a direct route to the existing recovery choices. Opening those
choices only reveals and focuses them. Authorization still requires an explicit
choice against the current recovery block; a changed block invalidates it.
Delivery feedback distinguishes local retention, host delivery and admission.

Input boundaries use a dedicated Solarized contrast token. Compact diagnostic
text uses a 12px floor; coarse-pointer chat controls have 44px minimum height.
Existing typography, palette, streaming selection, code disclosure and retained
history behavior remain the design constraints.

## Validation and review

`web/e2e/chat-utility.spec.ts` checks narrow form space and worker draft retention,
recovery disclosure and stale-choice invalidation, computed input-boundary
contrast, diagnostic sizes, and coarse-pointer targets. Existing interaction and
visual fixtures cover streaming, submitted history, disconnected controls,
keyboard use, and actual 200% browser zoom.

Evidence, commands and logs are retained under
`/srv/swarm/evidence/inanna/chat-utility-20261012`. The
[private screenshot review](https://swarm-01.sphynx-bitterling.ts.net:8445/chat-utility/)
compares the previous and current layout and carries the final verification
record. Browser validation uses synthetic loopback transport and production
assets; it does not establish live engine behavior.

Impeccable's detector reported the existing Inter family and a state-accent
border. Both belong to the established design and are retained. Inbox/Timeline
restructuring, engine changes and release integration remain outside this batch.
