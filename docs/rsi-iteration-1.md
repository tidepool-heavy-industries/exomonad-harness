# RSI iteration 1 / wave 9

## Product assignment: delivery before the first request

Read PRD.md sections **agent verbs**, **mailbox**, **store**, and **loop**.
Build an offline file-Store integration gate for the following behavior:

1. A registered child has no head/model request. Through the production agent
   service, its parent queues a `followup_task`; the envelope is durable before
   any model call. Distinguish the child's initial task from this follow-up.
2. Start the target through the production Engine. Capture the first outgoing
   request and assert that the specific follow-up appears exactly once.
3. Return a strict typed final result through the real finalize path.
4. Close and reopen Store. Verify the target's persisted head, delivery state
   and exact envelope identity. Reopening proves durable observation, not
   recovery of a running Engine.

Use ReplayTransport, offline auth and explicit barriers. No sleeps establish
ordering. Call the actual `followup_task` service; inserting an envelope by
hand does not prove the verb works. Assert no model request existed when the
follow-up was queued. An empty filtered run is not a passing check.

The service already persists follow-ups without checking head_request, and
`dispatched_message_and_followup_are_persisted_in_arrival_order` covers queueing. The Engine unit test
`run_from_head_admits_unread_inbox_once_before_first_prompt` covers raw inbox
admission in an in-memory Store, without the follow-up service or typed finalize.
Inspect existing tests before adding coverage. If equivalent end-to-end coverage
already exists, return that evidence; do not duplicate it or invent a code fix.

This gate does NOT close dogfood amendment 4: that is a follow-up arriving
during finalization, unseen by the finalizing request, with later delivery and
final-answer provenance. Retain that larger obligation explicitly.

## Ownership and release

Root owns the acceptance contract, NEXT.md, manifests, integration and interview.
Use one Sol Medium implementer owning `crates/harness/tests/first_request_delivery.rs`.
Root first supplies a compiling scaffold if the existing node protocol requires
one. If production repair is necessary, establish the narrow ownership amendment
before edits; keep the same implementer where practical. Reviewer owns no files.
All source revisions come from Git; use full OIDs in typed packets.

Focused gate: `cargo test -p harness --test first_request_delivery` with at least
one named executed test and no ignored acceptance case. Run it on the final
integrated revision, plus the existing adapter_readiness target, cargo check for
the demo consumer if production surface changed, formatting and diff checks.
An independent reviewer checks exact candidate HEAD, cumulative base-to-tip scope,
the real service/Engine path, evidence timing and the reopen boundary.

After checked integration, collect the short interviews below and stop with a
handoff. No unbounded correction campaign follows. Existing item-2, item-13 and
live adapter inference holds remain; replay tests spend no inference. The
Exomonad coding actors themselves are authorized by this wave assignment.

## Coordination experiment

Use the typed base in `reviewCommit` (arguments: label, base, candidate,
acceptance, owned paths, repair owner). `reviewCandidate` already obtains base
from Task. Reviewer acceptance must preserve Task base separately from candidate.
Verify ancestry and use the explicit base-to-tip diff, never a copied OID or an
inferred master merge base.

The existing workspace `plans/continuation.md` and executable
`checks/review-continuation.hs` are the starting point for automatic review.
They already compose exact submission evidence, retained review, outcomes and
repair. Do not create another scheduler or registry.

Use this composition only after its offline recipe passes on the selected build.
A useful first independent review of the scaffold/acceptance may establish the
retained reviewer; do not commission a dummy review just to obtain a handle.
If no useful reviewer is available, the root admits the first review normally;
subsequent real repairs are opportunities for automatic routing.

For a live flow, route actionable outcomes with `notifyWork` to the owner;
`keepWork` alone does not wake it. The existing flow captures a fixed Task:
if an accepted decision or source amendment changes that Task, stop admitting
through the old flow and explicitly establish a new assignment/flow after its
pending requests settle. Do not replay uncertain admissions or silently reuse
stale accepted decisions. Keep integration under the root's judgment.

If the recipe fails, preserve its exact diagnostic as the experiment's blocker
and use normal reviewed delivery for the product task. Do not claim automatic
coordination was exercised. A broken primitive may justify the next substantial
engine experiment, rather than more instructions.

## Observer and interview

Reuse docs/observer.md evidence rules. For this iteration track four measures:

| Opportunity | Evidence and outcome |
|---|---|
| A review is requested | Base/candidate packet, actual reviewer HEAD, executed checks, invalid-packet or checkout repairs |
| Parent has pending work and no useful local action | Route registration, idle turn end, actionable event and next useful parent action; count empty polling rounds |
| Concurrent behavior is implemented/reviewed | Acceptance invariant, barrier placement, initial candidate defects, missed failure paths and repair rounds |
| Candidate or repair settles into an authored flow | Exact receipt, automatic request admission, routed verdict and parent action; count model relay turns and preserve source/refusal failures |

Record actor/request IDs, times, source/prompt exposure and references. Use
held/missed/unknown/pending, and not-exercised when there was no opportunity.
Do not measure success by elapsed time alone or infer prompt exposure from a
commit. Mid-wave changes are allowed; record recipient incorporation boundaries.

At candidate/blocker/wave end ask each relevant actor:
1. Which operation or rule cost unnecessary work? Give one concrete trace,
   assignment or commit, and what you expected instead.
2. Which recurring step could have been handled by code, and what new workflow
   would that enable? Name a missing capability separately from a missing example.

Root also reports whether the typed review base and event waiting helped, where
human/model judgment remained necessary, and whether the useful next milestone
should be amendment 4 or a different capability exposed by this run.

The external supervisor records measurements in Tidepool's RSI iteration notes.
Root records its product evidence and interview in this repository. No observer
management tree, periodic actor polling, or new runtime instrumentation.

## Selected-build preflight: 2026-09-25

Workspace c6d3104: Project.SkillChecks.reviewProvenance passed all four assertions
on a fresh one-worker compiler with 7168 MiB rotation threshold. The independent
review skill call seeds the exact candidate, carries distinct typed base and tip,
and returns acceptance preserving both.

Project.RoutingChecks.automaticReview is BLOCKED before assertions: generated
wrapper lines name unexported Project.Routing.WorkEffects and unimported
Control.Monad.Freer.State.State. **Use ordinary reviewed delivery for wave 9.**
Do not start a compiler-fix detour or instantiate the failed review continuation.
The authored-coordination experiment is not exercised this wave; this is evidence
for a follow-up workbench capability repair. Existing ordinary inline followWork /
notifyWork routing remains the event-waiting mechanism.

The supervisor stopped unused previous hosts and compiler daemons with operator
authorization. Wave 9 launches a fresh one-worker compiler. Source, worktrees and
prior run artifacts were preserved.
