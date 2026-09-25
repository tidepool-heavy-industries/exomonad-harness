# NEXT: wave 10 / RSI iteration 2

Read docs/rsi-iteration-2.md and implement its authorized outcome: the complete
offline follow-up/finalization lifecycle with typed parent answers and durable
seen/unseen input provenance. The root owns shared contract, decomposition,
independent review and checked integration. No planner release is outstanding.

This current assignment controls decomposition and result types over historical
campaign examples in docs/tree.md or prompts. Use two or three bounded Sol Medium
implementation children where the source permits; their result is Outcome
Candidate, not Delivery. Root retains integration. Ordinary event-driven review
uses the new ReviewRequest/ReviewBasis API; automatic-review wrapper repair is
outside the wave. Resolve full source OIDs from Git.

## Obligations

| Obligation | Owner | State / next evidence |
|---|---|---|
| Shared lifecycle/provenance contract and scaffold | root | `lifecycle.rs` defines envelope-ID provenance and structured parent answer; optional `Contract.reply` is the strict result schema; compile and commit before forks |
| Stable follow-up identity and durable provenance | store child | own `store/mod.rs`, `agent_runtime.rs`; no schema migration intended |
| Typed completion | engine child | own `engine.rs`, `finalize.rs`; strict schema from `Contract.reply` |
| Driver publication/continuation and integrated release gate | driver child | own `harness-demo/src/driver.rs`; parent envelope carries `PublishedAnswer` JSON as structured item payload |
| Independent review, integration, interview and stop | root | exact-source verdict, executed integrated checks, handoff |

Keep this table current as assignments settle; ordinary idle turn ends need no
commit. Preserve one implementation owner per shared file and record amendments.

## Standing constraints and retained evidence

- No git stash/reset/checkout -- path; commit by pathspec, no attribution trailers.
- One compiler worker; do not restart the shared daemon.
- No credentialed item-2 retry, item-13 trace or live adapter smoke. Offline
  replay and Exomonad coding actors are authorized.
- Root may make implementation decisions within the stated contract; ask only
  when consequential ambiguity remains after inspecting source and requirements.
- Wave 9 merged first-request gate at 4230179, handoff 9c6e75b. Its manual
  advance_agent_head proves persisted observation, not the production runner.
- Wave 8 adapter-readiness gate remains required; it uses an offline evaluator.
- Review setup: harness 705eaee, workspace 7307478. Provenance recipe passed 6/6
  in Tidepool and harness; the workspace-pin test passed. No full gate claimed.

## Wave 10 shared contract and admission packet

Baseline for the wave is the scaffold commit following 076684b7ad5d858a0fc119663d2c4ec1b1ada251.
Source authority: `PRD.md` §§ `agent verbs`, `mailbox`, `store`, and
`docs/dogfood-requirements.md` amendment 4; scope/acceptance:
`docs/rsi-iteration-2.md`. Focused combined gate:
`cargo test -p harness-demo followup_lifecycle` (the driver owner writes
and executes matched tests), plus `cargo test -p harness --test
first_request_delivery` and `cargo test -p harness --test
adapter_readiness`.

The final-request ancestry is the sole "seen" test. Store takes an atomic
completion snapshot after the final turn is durable: seen = envelopes whose
`delivered_request` is that request or an ancestor; unseen = then-current
unread envelopes for the child. IDs are sorted, stable `envelopes.id` values.
An arrival after the snapshot stays unread and must wake the next request;
it must not retroactively change the first answer or be stranded. "Seen"
means presented input only, never model acknowledgement/incorporation. The
strict finalize call's `result` is preserved as JSON, not rendered prose.
The parent FINAL_ANSWER envelope stores `PublishedAnswer` (sender, result,
provenance) as structured JSON and only renders at a model presentation
boundary. No new durable log or schema migration: existing envelope IDs,
`delivered_request`, request ancestry, and item content are authoritative.

Planned first admission: three Sol Medium candidates from the scaffold
commit. Store owns `store/mod.rs` and `agent_runtime.rs`, first reply stable
`followup_task` envelope reference and persisted snapshot tests. Engine owns
`engine.rs` and `finalize.rs`, first reply strict schema/typed completion
using the shared contract. Driver owns `harness-demo/src/driver.rs`, first
reply real factory, publication and explicit-barrier lifecycle gate. These
paths are exclusive; report a requested root contract amendment rather than
editing another lane. Root owns `agents.rs`, `lifecycle.rs`, `lib.rs`,
manifests, combined gate and integration. Children return `Outcome Candidate`
with `WorkProgress`, not `Delivery`.
