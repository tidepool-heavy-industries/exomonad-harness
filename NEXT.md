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
| Stable follow-up identity and durable provenance | store child | candidate `5b2741a` submitted; exact-source review pending; no schema migration |
| Typed completion | engine child | submitted `5e8d58e`, then corrected tip `2d12a4a` to remove typed_result; exact-tip checked resubmission requested before review |
| Driver publication/continuation and integrated release gate | driver child | owned driver edit and explicit-barrier test in progress; sibling APIs not integrated, no compiled gate yet |
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

First admission checkpoint: three Sol Medium candidates admitted from
`f9ab1a9da48e0f119fdad48052e896f57d117bd2` in `wave10/implementation`.
Store owns `store/mod.rs` and `agent_runtime.rs`, first reply stable
`followup_task` envelope reference and persisted snapshot tests. Engine owns
`engine.rs` and `finalize.rs`, first reply strict schema/typed completion
using the shared contract. Driver owns `harness-demo/src/driver.rs`, first
reply real factory, publication and explicit-barrier lifecycle gate. These
paths are exclusive; report a requested root contract amendment rather than
editing another lane. Root owns `agents.rs`, `lifecycle.rs`, `lib.rs`,
manifests, combined gate and integration. Children return `Outcome Candidate`
with `WorkProgress`, not `Delivery`.
Admission routing currently uses `wave10-any` settlement watch; root reads
every lane at the next checkpoint. No candidate is yet reviewed or integrated.
Driver requested exact APIs; root accepted and sent to owners:
`Store::completion_provenance(&AgentPath, &RequestId) ->
Result<CompletionProvenance>` and
`Engine::run_with_reply_schema(head,new_items,cancel,inbox,schema: Value) ->
Result<EngineCompletion,EngineError>` with `EngineCompletion.typed_result:
Option<Value>`. Transport acknowledgments do not prove incorporation.
Scaffold baseline checks: `cargo test -p harness --test first_request_delivery`
1/1 and `cargo test -p harness --test adapter_readiness` 1/1 passed.
Engine owner confirmed the signature and behavior, including
`finalize::tool_schema_from_result_schema(Value)`. The result schema
parameter is named `result_schema`; `typed_result` is `Some` only for one
strict finalize result. Root relayed this to driver. No candidate yet.
Correction from driver, sent to Engine: `EngineCompletion.typed_result` is
not needed. The required `run_with_reply_schema` returns `EngineCompletion`
after enforcing one strict finalize; driver parses its `turn` through
`FinalizeParser::parse_completed::<Value>`. Prior typed_result-field
instruction is superseded. Store provenance API remains unchanged.
Store owner's admission checkpoint reported its planned APIs:
`Store::completion_provenance(&AgentPath,&RequestId)`,
`Store::envelope(i64)->Result<Option<Envelope>>`, and production
`followup_task` JSON `envelope_id`; no migration. Focused tests were in
progress. Root relayed to driver as a report, not integrated evidence.
Store candidate `5b2741a0a720fa52801657e8fbc4a016e7843369` is
committed, cumulative diff limited to its two owned files; reported focused
checks each matched/passed 1/1 (ancestry/reopen, service, first-request,
adapter-readiness). Root commissioned exact-source independent review and
relayed candidate status to driver. Not yet integrated.
Engine's first response named `5e8d58e90f3d0d35b9bcd6ca94ecb76462246774`
but its branch already advanced to `2d12a4af3c221b30fcb0df85dcec66be9dae28f0`
for the driver correction (no `typed_result` field). Root did not
review the stale candidate; it requested exact-tip checks/resubmission
from the retained engine owner. This is a result-stage correction, not
an integration.
Driver progress reported uncommitted owned changes for strict factory,
`PublishedAnswer` publication, and an explicit-barrier real-driver gate.
It still mentioned `typed_result`, so root corrected it to parse the
strict finalize call from `EngineCompletion.turn` at the intended engine
tip. Its gate has not compiled or executed yet.
