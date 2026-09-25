# Correction wave — execution contract

Source: `docs/tree.md` correction-wave and wave0-review bullets, PRD §§ provider
trait, agent verbs, settings items, loop and compaction. Human correction
received 2026-09-24; Q1–Q3 in `docs/questions.md` remain settled. This is a
correction wave, not wave1 approval. Server, auth and web are frozen.

The core Luna lead's readback at `babfb4d` agrees with the ordered frontier:

1. **One entry:** `Engine::run(head: Option<RequestId>, new_items: Vec<Item>,
   cancellation, incoming)` is the sole public run entry. It loads durable
   history, admits the mailbox and returns `EngineCompletion`. Remove the
   legacy wrappers and migrate both demo consumers and tests; no shims.
2. **Async jobs:** every provider tool and crate verb except `wait_agent`
   advertises `async: true`. Prove a live slow `sleep`: the model continues
   with the call pending, then `wait_agent` resumes when the original call
   settles. Record redacted request bodies in `docs/findings.md`.
3. **Positional settings:** store `configuration_update` as an item; implement
   `set_effort`, adjacency replacement and the fork strip list. Effort is
   pinned through the item; any request-level field only mirrors its first
   update. Prove live child history = filtered parent prefix + one update,
   recording cache counters. Per Q2, a positive counter is not required.
4. **Compaction:** implement PRD `Compactor`, `CompactContext`, `Summary` and
   `NewWindow { carried }` with `Server` only. Preserve the pending-call
   invariant and record the unanswered-call experiment in findings.

The separate cache-shape probe is findings-only. Hooks and provider-trait
redesign beyond what (a)–(c) require are not in this wave.

## Steering update — 2026-09-24

Root Sol plans and decides seams. Lunas receive bounded implementation, test
or review assignments; they do not write the execution plan. A Luna doing
structural work, or accumulating repeated failed checks without a candidate,
must stop and ping root: split into a Luna subtree with named seams, or return
`Blocked` identifying the seam it cannot settle. Assignments need a stop/ping
condition. Review integration candidates, not findings-only probes; root reads
their evidence.

While one candidate is under review, root advances independent ready work
rather than serializing the wave on that notice. The ordered (a)→(d)
contract still controls shared loop changes and integration dependencies.
The operator has requested a handoff-ready stopping point after the next
Exomonad-improvement wave; unfinished live acceptance must stay explicitly
open rather than being called complete.

Later operator correction: Bash latency came from shared-checkout contention
and a compile-cache miss, not output size. One failed classification step was
fixed in the engine. Remove output-size limits from task packets, batch
independent commands, and prefer fewer, larger bounded children for the
rest of this wave. Root is pausing polling/integration briefly to read
planning material and record engine friction; active leaves were asked to
commit candidates and explain the delay in their own words.

## Ownership and join

Root owns this contract, actor-spec repair, demo consumer wiring
(`crates/harness-demo/src/{main,driver}.rs`), interviews, integration commits
and final checks. Root chooses each shared seam; the core Luna executes bounded
crate changes and must split or stop if the work becomes design-heavy. Contract
files are scaffolded before leaves touch them. Every candidate
is checked against owned paths, reviewed at its exact commit and merged, never
copied. Each frontier ends in `integrate(<label>)`.

The planner-review recipient is the operator's original Astra planning
conversation. The exact artifact, if review is requested, is this file plus
the core readback and `docs/tree.md` correction-wave bullet. The operator's
current correction message is the release scope for this wave; there is no
separate operator hold or unanswered product choice recorded. No later local
repair needs renewed planner approval.

## Evidence still needed

Focused crate checks after each slice; then integrated checks against the
resulting source, live item 2 and item 13 traces, request bodies/cache
counters, the compaction unanswered-call finding, and one interview section
per node. Preparation alone does not close these obligations.

## Status (operator review, 2026-09-24, after the first correction run)

Not done. This wave ends when all four items are integrated on master with
their live traces in `docs/findings.md`, master is green, and
`docs/interviews.md` has one section per node that ran.

| item | state | what remains |
|---|---|---|
| (a) one entry | **done** (`9da6efe`, `a9f7a12`) | nothing |
| (b) async tools | provider stamp integrated on master (`d8097c3`); offline correction-wave test 1/1 passed, fmt passed | live item-2 trace (slow `sleep`, model continues, `wait_agent` resumed) with redacted request bodies in findings remains open |
| (c) settings items | not started; the "provenance seam" the last lead blocked on is decided (Q5, TODO at `Store::append_items`) | `configuration_update` as a store item, drop rule at append, `set_effort`, fork strip list, effort pinned via the item; live item-13 trace with cache counters |
| (d) Compactor | not started | PRD shape, `Server` only, unanswered-call experiment → findings |
| cache probe | `Blocked` evidence merged (`b99337e`, `06148a1`); unblocked by Q4 | two requests through our builder, same key; record both redacted `usage` blocks; field diff vs codex source only if `cached_tokens` is 0 |
| interviews | root, probe and core sections present | one section per node that runs next |

The red offline test on master was accepted between slices and became green
with (b). Live tests that spend inference stay `#[ignore]` and are run by
hand, once, with the trace recorded in findings; never automate a test that
triggers inference.

## Current-run handoff (2026-09-24)

The root integrated (b) at `d8097c3` and the probe's blocked finding at
`06148a1`. On integrated source, `correction_wave` ran 1/1 passing and
workspace fmt passed. This establishes the provider schema flag, **not**
live async continuation. Item-2 live acceptance is still open.

Core returned `Blocked` on the structural (c)/(d) seam after reporting that
its notification inbox was fenced. The operator's no-more-forks constraint
remained in force for that run and is lifted (Q5). The provenance question
is answered by the PRD, not by a design lead: a `configuration_update` enters
the store only via `set_effort`, a `here` fork's re-pin, or a compaction's
fresh pin, and any other one is dropped at append. No SQL migration. The
next lead implements (c) as store drop rule + `set_effort` + strip list +
pin-from-history, in that order, with the live item-13 trace last.

Still open: (c), (d), live item-2 and item-13 traces, unanswered-call
experiment, cache probe reference capture/Q4, interviews from bounded nodes
whose own words were not delivered, and final combined checks. Do not
call this correction wave complete or launch wave1 from it.
