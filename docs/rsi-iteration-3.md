# RSI iteration 3 / wave 11 — restart recovery and nested Luna work

## Outcome

Extend wave 10's offline typed follow-up lifecycle through abrupt process loss
and restart. A new real Driver over the file Store discovers durable parent
answers and pending child follow-ups without duplicate publication or stranded
work. Preserve strict typed results, request ancestry and seen/unseen provenance.
Wave 10's clean reopen is useful evidence, not proof of crash recovery.

Use the existing Store, service, Engine and Driver owners. No second scheduler,
mailbox or durable journal. Do not promise exactly-once external provider/tool
execution: this wave proves the stated durable publication and admission behavior
using offline replay. Keep live adapter inference, item-2 retry and item-13 trace
on hold. Contract versioning and adapter implementation remain separate scopes.

## Deliberate topology experiment

The operator requests a Sol root with multiple Luna subtrees. Root owns shared
semantics, initial scaffold, cross-component decisions and final integration.
Admit at least two Luna component owners; target three when the following source
boundaries support them. Each component owner delegates at least two meaningful
Luna obligations before local integration. This is an explicit experiment in
nested coordination, not a new default requirement for every future task.

Candidate decomposition, finalized against current source by root:

- **Durable recovery:** Store recovery observations/transaction behavior and
  service pending-work discovery. Separate production and failure-test ownership.
- **Engine recovery:** request-boundary replay, strict-result/provenance retention
  and interrupted-work classification. Separate production and boundary-test
  ownership; preserve existing behavior where evidence already meets the contract.
- **Driver restart:** startup scan, supported CLI admission, task/reaper lifecycle
  and continuation after restart. Separate production changes and Driver
  recovery/cancellation tests. Root retains the cross-component crash gate.

Use Luna Medium for these component owners and their bounded children; retain
Luna reviewers for exact candidates and repairs. Component owners return Delivery
only after local review, integration and focused checks; their implementers return
Outcome Candidate. Root returns final evidence. Use ordinary event-driven review;
automatic-review wrapper repair and typed review-tool rollout do not block this
wave. Retain Haskell for notebook work and replies.

Delegate ready obligations in one context unfold per frontier. Keep the component
owners doing local integration/contract work while children progress. Do not make
siblings race on driver.rs or store/mod.rs: root establishes separate test module
files and their module declarations when needed. Preserve existing private test
visibility rather than exposing production APIs solely for tests. Record real
dependencies; dependent work waits for its exact source, unrelated work proceeds.

## Root prerequisite and contracts

Resolve and record the actual committed launch baseline. Before dependent forks,
land minimum compiling shared types and fixture wiring with:

- one canonical current API and named owners/production consumers;
- the durable invariant joining completion/head advancement and publication;
- the persisted evidence restart uses, including pending/incomplete work;
- a typed barrier at the transaction/notification boundary and an explicit
  process-loss strategy that cannot be confused with graceful shutdown;
- exact package, target, filter, expected matched count and initial outcome.

A test may initially be expected-red; execute it and name the failure. The root
need not finish recovery implementation before delegation. Reuse adequate
existing contracts and tests. Shared signatures in the brief must match the
committed source; move obsolete signatures out of the current contract section.

Each correction carries the superseded decision, full source OID, affected
consumer and required check. The receiver returns its incorporated candidate OID
and actual check result. Delivery and acknowledgment alone do not close that
dependency. Component owners report reviewed integrated slices promptly.

## Acceptance matrix

Root refines exact barriers against production source without weakening outcomes:

1. Failure before completion commit leaves no advanced head without its answer.
   Recover or explicitly classify interrupted work using the existing owner.
2. Abrupt loss after the atomic head/answer commit but before the wake hint:
   restart discovers the durable answer once without requiring that lost hint.
3. A queued child follow-up survives restart, reaches the next request once and
   produces the next typed answer with correct seen/unseen references.
4. Repeated restart neither republishes the committed answer nor admits concurrent
   duplicate runs. Check durable IDs, ancestry and publication counts directly.
5. Shutdown/cancellation after recovery joins admitted tasks, watchers and reapers;
   failure evidence remains visible. Keep wave-10 and adapter-readiness gates green.

Use explicit barriers/acknowledgments rather than sleeps. State which tests kill
a process, abort a task, inject a Store error or cleanly reopen; these are distinct
failure models. Root owns the final combined gate on the integrated revision.
Use scripts/cargo-focused-test for supported Rust libtest targets, and preserve
expected-red, zero-match, compile-only and executed results separately.

## Observation and stop

Record actual tree shape, which sibling work overlapped, shared-contract wait
time, correction incorporation, source mismatches, zero-match commands, rejected
cells, stale reminder/reply retries and defects found before/during review.
Do not count admitted actors as proof of useful parallelism. Interview root,
component owners and reviewers on what depth helped and what only added relays.
At completion record exact integrated source/checks/limits, retire all descendants
deliberately, and stop. No new inference capability is released by this assignment.
