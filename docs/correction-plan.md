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

## Ownership and join

Root owns this contract, actor-spec repair, demo consumer wiring
(`crates/harness-demo/src/{main,driver}.rs`), interviews, integration commits
and final checks. Core Luna owns sequential local integration of the crate
loop corrections and forks Luna leaves, at most two at a time. Contract files
are amended at core's scaffold before any leaves touch them. Every candidate
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
