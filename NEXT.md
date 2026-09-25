# NEXT: RSI iteration 1 / wave 9 — completed handoff

Read `docs/rsi-iteration-1.md` for the authorized assignment, acceptance,
coordination experiment and observation questions. This file supersedes the
older correction-wave campaign instructions; prior obligations and evidence
remain in Git and the named docs, not an instruction to resume all of them.

## Standing objective

The offline follow-up-before-first-request integration gate was independently
reviewed and merged at `4230179d467e852d94384e45b40e9bfd6893cbca`.
Integrated focused targets ran 1/1 each; interviews are recorded in
`docs/interviews.md`. Stop this wave. The gate does not close amendment 4's
reply-while-follow-up-is-unseen behavior; no adapter implementation was done.

Resolve master and your actual checkout from Git before assignments. Use one
Sol Medium implementation owner and independent review; no delegation hierarchy
for this bounded slice. Root retains integration. Project reviewCommit now takes
both base and candidate OIDs. Existing event-waiting and exact-HEAD rules apply.
The automatic-review recipe failed on generated wrapper types: use ordinary
reviewed delivery for this wave. See the iteration document for the diagnostic.

## Carried holds and completed work

- Wave 8's offline adapter-readiness release gate is merged at `703005f`.
  It uses a fake evaluator; it does not prove a live Tidepool adapter.
- No credentialed item-2 retry, item-13 trace or live adapter smoke is authorized.
  Offline ReplayTransport tests are allowed. Coding actors for this wave are allowed.
- Auth, browser, web and full harness adoption are outside this bounded assignment.
- Keep one compiler worker; do not restart the shared daemon.
- Never git stash/reset/checkout -- a path. Commit by pathspec; no attribution trailers.

## Current obligations

| Obligation | Owner | Evidence | Next |
|---|---|---|---|
| First-request delivery gate | root | candidate `6d3b39efdb630fbb523d5a8dddb9ae3d0f7cccad` merged at `4230179d467e852d94384e45b40e9bfd6893cbca`; integrated target 1 matched/passed, 0 ignored | complete, caveat below |
| Review and integration | root + reviewer actor 4/request 2 | reviewer accepted exact candidate with 1 matched/passed, no ignored; cumulative diff owned file only; integrated adapter-readiness 1/1, fmt and diff checks pass | complete |
| Coordination experiment | root; external supervisor observes | selected-build automatic-review wrapper blocked before assertions; ordinary route/review used | repair wrapper in a separately authorized workbench effort |
| Interview and stop handoff | root | owner, reviewer and root sections in `docs/interviews.md`; product evidence in `docs/rsi-iteration-1.md` | stop |

Keep this table current with integration or a real stop handoff. Routine turn
ends do not require documentation commits. End idle turns when a registered
route will bring the next actionable event. A findings-only report needs no review.

## Wave 9 admission checkpoint

Base `aa0c82e3bf47a0a26bbad21c7f507da5f9078269` (root master).
Actor 2/request 1 owns only `crates/harness/tests/first_request_delivery.rs`;
first expected reply is an admission checkpoint, then a Delivery with the
named executed gate. `followWork` router actor 3 is registered for progress
and settlement. Root owns exact-source review and integration. The selected
automatic-review recipe is blocked as documented in
`docs/rsi-iteration-1.md`; use ordinary review.
Owner's Delivery reply type is premature for an implementation candidate;
owner settled `Blocked` on that seam. The candidate was published through
WorkProgress. Reviewer actor 4/request 2 accepted the exact candidate; root
merged it at `4230179d467e852d94384e45b40e9bfd6893cbca`.
The test explicitly calls `advance_agent_head` after Engine completion:
reopen proves a durable persisted head, **not** automatic production runner
head advancement. Amendment 4's unseen-follow-up/final-answer provenance
remains unclosed.

## Where the last run stopped

No unmerged wave-9 branch holds reviewed or passing work. Candidate
`6d3b39efdb630fbb523d5a8dddb9ae3d0f7cccad` was merged into root at
`4230179d467e852d94384e45b40e9bfd6893cbca`.

The next product owner should take amendment 4 as a separate assignment:
follow-up arriving while finalization is in flight and unseen by that
request, later delivery and final-answer provenance. The explicit
`advance_agent_head` in this wave's test proves persisted observation
after completion, not automatic production runner head advancement.
Credentialed item-2 retry, item-13 trace and live adapter smoke remain
on hold absent fresh operator authority.
