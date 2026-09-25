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
| Stable follow-up identity and durable provenance | store child | exact-tip review accepted; integrated as `176a271`; provenance focused test 1/1 on integrated source |
| Typed completion | engine child | repaired exact candidate `2d5218f` re-review accepted; integrated as `c445c5d`; root-focused checks running |
| Driver publication/continuation and integrated release gate | driver child | repaired candidate `cb2620d` reports combined lifecycle 2/2 and malformed-publication 1/1; same-reviewer exact-tip re-review pending, not merged |
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
provenance) as a JSON-encoded standard `output_text` item; typed Store/hook
consumers decode it, while Responses sees only valid message fields.
Root-owned `lifecycle::PublishedAnswer::{to_message_item,from_message_item}`
is the single codec. This amends the earlier proposed extra top-level
`structured` field, which had no wire projection and could be rejected
when Engine sends stored history unchanged. No new durable log or schema migration: existing envelope IDs,
`delivered_request`, request ancestry, and item content are authoritative.
Additional root-owned tool-schema amendment: the model-facing strict
`spawn_agent` and `followup_task` contract schema must carry required
nullable `reply` as a JSON-encoded schema string (`type:
["string","null"]`). Strict tool schemas cannot express arbitrary
schema property names while keeping `additionalProperties:false`.
`Contract` deserializes the encoded string to its internal `Value`;
null is no reply. Store serialization retains parsed JSON. This
closes a production-wire gap the offline ReplayTransport would bypass.

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
Store review accepted exactly `5b2741a` (ancestry/reopen test 1/1;
the reviewer's second service command had no visible matched count and
is not review evidence). Root merged it in `176a271d47f14fc1d3eea1c21bdb55b21a9d3b4e`
and ran `cargo test -q -p harness completion_provenance_uses_ancestry_and_reopen`
on that integrated source: 1 matched, 1 passed. Driver was informed.
Engine corrected `2d12a4a` is under exact-tip review.
Driver later found its implementation still consumed the superseded
`typed_result` field. Root made the seam decision explicit again:
driver MUST parse `completion.turn` through `FinalizeParser::<Value>`;
engine must NOT add the field solely for driver. This corrects the
earlier accepted field contract and unblocks driver-owned repair.
Driver confirmed the correction was incorporated and its branch advanced
through `27f75df0aacefcb5370327fb639fdda76e301c13` with strict
turn parsing and reopened assertions. It incorrectly reported master
still `f9ab1a9`; root corrected the current integrated Store source
`176a271` and asked it to await Engine integration before final gate.
Driver raised a strict-completion concern because public
`run_with_reply_schema` delegates without parsing inline. Root inspected
exact Engine tip: the shared `run_loop`'s only completion return arm
collects finalize calls, rejects count != 1, then calls
`parse_completed_with_result_schema` before `EngineCompletion` returns
(`engine.rs` around lines 717-744). This is code-path evidence, not yet
an independent verdict or integrated gate. Root forwarded the focus to
Engine owner and reviewer and the observed path to driver.
Driver identified that its extra top-level `structured` Item field
would flow unchanged into Responses input (`engine.rs` builds request
from stored history). Root decided on a standard-message JSON-text
codec with strict `PublishedAnswer` decode and a roundtrip test in
`lifecycle.rs`; driver was instructed to use it, not ad-hoc text/prose.
Driver later identified that `verb_tool_schemas()` omitted
`Contract.reply` even though the Rust Contract accepted it; strict
tool input would reject typed reply at production ingress. Root
amended agents.rs with required nullable encoded `reply` and a
focused strict-schema/roundtrip test; check and commit were pending
at this checkpoint.
Exact Engine review at `2d12a4a` confirmed shared-loop exactly-one
validation but returned Repair for two `finalize.rs` bugs:
schema-invalid dynamic result consumes `FinalizeParser`, contrary to
invalid-then-valid behavior; normalized string `pattern` is advertised
but not enforced locally, allowing ReplayTransport violation. Reviewer
ran no completed tests (compiler artifact lock and unknown retained
job); its matched count is 0/0. Root assigned repair to retained
Engine owner, then one re-review by same reviewer. Driver was told
Engine is not integrated; its branch reported rebase onto Store
integration and an uncommitted borrow fix.
Engine repair candidate `2d5218f881b5cfa4e055471548152b99ee63157c`
was submitted with five focused cases each reported 1/1, fmt and diff
checks; diff from incorporated Store base `176a271` touches only
engine.rs/finalize.rs and merge-tree shows no conflict with root codec
amendment `41c48b6`. Root assigned one re-review to the same reviewer,
seeded at exact tip with review base `176a271`. The driver prior request
settled Blocked on the now-resolved Engine/publication seams (it had
not compiled its gate). Root supplied codec commit `41c48b6` and
reassigned retained driver to rebase its owned file, use the codec,
and run the gate after Engine integration.
Same reviewer accepted exact Engine candidate `2d5218f` after five
focused matched/passed 1/1 checks, noting integrated failure/cancel
and Driver gate remained. Root merged its engine.rs/finalize.rs-only
delta into `c445c5d3370e6beb2127cc4a2e0197ddf121df3f` with
no conflict against codec/tool-schema amendments, informed Driver,
and started integrated focused checks. This is not overall wave
acceptance.
Integrated checks on `c445c5d`: `cargo test -q -p harness
dynamic_reply_schema` matched/passed 2/2;
`dynamic_schema_invalid_call_does_not_consume_finalize_parser` 1/1;
`rejects_pattern_schema_instead_of_advertising_unchecked_constraint`
1/1; `--test first_request_delivery` 1/1;
`--test adapter_readiness` 1/1; cargo fmt and git diff --check passed.
The real Driver `followup_lifecycle` gate has not yet run.
Driver's prior release-gate request settled Blocked on its stale view
that master was `41c48b6`; it reported codec use and two written
barrier tests but 0 compiled/matched tests. Root supplied the missing
checked integration source `c445c5d` and immediately reassigned the
retained driver to rebase and execute the focused gate. No driver
candidate is accepted yet.
Driver then submitted exact candidate
`ddce261673a7f5fed964609a355b4d22d4003957` rebased on
`c445c5d`, cumulative diff only driver.rs. It reported
`cargo test -p harness-demo followup_lifecycle` 2/2,
`shutdown_` 2/2, `reaper_joins_all_finished_handles_after_first_failure`
1/1, first_request_delivery 1/1, adapter_readiness 1/1,
fmt and diff checks. Root commissioned exact-tip independent review;
these are candidate reports, not integrated gate evidence.
Driver review of exact `ddce261` returned Repair: it advances
the child head before adding the parent answer envelope; if
publication fails, restart sees a completed head with no parent
answer, so the answer is lost. The reviewer reported no counted
adapter-readiness test (its filter matched 0), despite the owner's
candidate report of 1/1; retain the discrepancy until root reruns
on integrated source. Root contract amendment: one Store transaction
must CAS the child's head and insert the optional parent answer
envelope. `lifecycle::CompletionCommit` is the shared result:
`HeadMismatch` (no envelope inserted) or `Committed { envelope_id }`
(both effects durable). Driver constructs its publication item and
snapshot before calling the atomic method; on committed publication
it sends a wake hint. Crash/retry after commit cannot duplicate the
envelope because head CAS will fail. Store owner must prove rollback
with an envelope-insert failure injection (e.g. a test-only SQLite
abort trigger). No schema migration or second scheduler.
Store request 16 reported the exact atomic API implemented on its
branch rebased to `04fdf30`, with HeadMismatch and trigger-failure
rollback tests written but not yet run. Root relayed the API to
Driver as an unintegrated owner report. Root-visible branch ref
still showed `04fdf30` before a Store commit.
Store submitted exact candidate
`5b7272bac92296d7112b19e628f9e84e8302c7ee`, cumulative diff
only store/mod.rs from `04fdf30`. It reported
`cargo test -p harness complete_agent_with_publication` 2/2,
covering success/idempotence and SQLite trigger-injected insert
failure rolling back head and item. Root commissioned independent
exact-tip review through retained Store reviewer; not merged yet.
Store atomic reviewer first returned Repair claiming no production
caller because `agent_runtime.rs:789` still used
`advance_agent_head`; root verified that occurrence is inside a
unit test, while disjoint committed Driver repair `552a21d` calls
`complete_agent_with_publication` in production at driver.rs ~367.
Root supplied this precise consumer evidence in one same-reviewer
re-review request. No Store code defect has been identified, but
Store slice remains unmerged pending the corrected verdict and
integrated gate.
Same reviewer accepted exact Store atomic candidate `5b7272b` after
correcting the production-consumer evidence. It verified 2/2 Store
focused tests and explicitly left the integrated Driver gate open.
Root merged Store as `850fa8393e776cdd67261137acd4876e2714e864`
and ran `cargo test -q -p harness complete_agent_with_publication`
on integrated source: 2 matched, 2 passed. Root immediately
reassigned retained Driver owner with that exact source for rebase,
atomic repair and combined gate.
Driver repair candidate
`cb2620d70b77a477af062f75b11aebb5b0ad487b` was then
checked on Store integration base `850fa83`; cumulative diff only
driver.rs. Owner reported lifecycle 2/2, malformed-publication
1/1, shutdown 2/2, reaper 1/1, first-request 1/1,
adapter-readiness 1/1, fmt and diff checks. Root assigned one
same-reviewer exact-tip re-review focused on the previous
head-before-answer failure and production cleanup. Candidate
passing checks are not integrated acceptance.
Driver atomic repair checkpoint committed
`552a21d1c1972ba3e8ef36157cf59254b774ba9f` on `04fdf30`,
owned diff only driver.rs. It prepares publication before the single
atomic Store call, handles HeadMismatch/Committed and wakes parent
after commit. A malformed-publication failure test is written;
no driver test has compiled against the unmerged Store method yet.
Driver repair request 17 settled Blocked on the same unresolved Store
integration dependency at exact tip `552a21d`; it correctly reported
zero compiled/matched tests, not a failing or passing gate. Root must
reassign the retained Driver owner immediately after reviewed Store
merge with that integration OID.

## Where the last run stopped (live wave 10 checkpoint)

- `exomonad/wave10/implementation/branches/wave10-engine`:
  candidate `2d5218f881b5cfa4e055471548152b99ee63157c` is no
  longer unmerged: exact-tip re-review accepted and root integrated it
  as `c445c5d`. Prior reviewed `2d12a4a` received Repair.
- `exomonad/wave10/implementation/branches/wave10-driver`:
  `cb2620d70b77a477af062f75b11aebb5b0ad487b`;
  atomic repair and malformed-publication test checked on `850fa83`,
  same-reviewer exact-tip re-review pending, not integrated.
  Prior `ddce261` gate passed 2/2 but review returned Repair.
- Store atomic-publication repair request is pending on shared
  contract source `04fdf305bc9ca9b78cf871e3b43ca802297e26d2`;
  candidate `5b7272bac92296d7112b19e628f9e84e8302c7ee`
  is no longer unmerged: same-reviewer re-review accepted and root
  integrated it as `850fa83`, with focused Store gate 2/2 on source.
- Store branch `5b2741a` is not unmerged: it is integrated as
  `176a271`. Root's wire-safe codec amendment is `41c48b6`.
Driver progress reported uncommitted owned changes for strict factory,
`PublishedAnswer` publication, and an explicit-barrier real-driver gate.
It still mentioned `typed_result`, so root corrected it to parse the
strict finalize call from `EngineCompletion.turn` at the intended engine
tip. Its gate has not compiled or executed yet.
