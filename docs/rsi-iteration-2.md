# RSI iteration 2 / wave 10

## Authorized outcome

Deliver the complete offline follow-up/finalization lifecycle. A child may finish
while new work arrives: its parent receives the strict typed answer with honest
input provenance, and the pending follow-up runs at the next request boundary.
This is implementation plus integrated behavioral acceptance, beyond wave 9's
single test of existing behavior. Read PRD.md (agent verbs, mailbox, store, loop,
typed final answers) and docs/dogfood-requirements.md amendment 4. This assignment
approves that amendment's bounded behavior and minimum envelope identity needed
to prove it. No additional planner approval is required.

## Contract and ownership

Root resolves the current Git baseline, inspects owning source, and commits the
shared types, semantics and compiling scaffold before independent implementation.
Decide and record the exact completion snapshot for unseen envelopes, how seen
input is derived from recorded request ancestry, and how strict typed results
reach parent publication. Preserve structured data through Store; render at the
presentation boundary. Keep observed input distinct from claimed incorporation.

Extend existing owners: Store envelope IDs/delivery records, agent service,
Engine request-boundary admission and the demo Driver. The driver already advances
heads and reactivates unread work; no second scheduler or durable log is needed.
Its current factory uses Engine.run and its parent publication extracts prose;
root must establish the typed completion interface before splitting that seam.

Use two or three bounded Sol Medium implementation children where source ownership
permits: envelope reference/provenance storage; Engine typed completion; driver
publication/continuation and integrated gate. Root may retain driver integration.
No ceremonial lead tier or compulsory recursive fan-out. Assign one owner per
shared file; contract changes come through root and are incorporated explicitly.
Root owns manifests, shared contract/schema migration decisions, NEXT.md, combined
acceptance and integration. A schema change must state its migration policy.

Implementation children return Outcome Candidate, with WorkProgress; they do not
return Delivery when root owns review and integration. Use candidateSummary for
their route. Delivery is reserved for an owner that actually supplies reviewed,
integrated delivery evidence. A candidate's settlement is not integration.

## Required behavior

1. Production followup_task returns a stable envelope reference. It is queryable
   against persisted delivery state; reuse the existing identity owner.
2. The final answer preserves its strict typed result and which input the final
   request had seen. New follow-ups outside that input appear as unseen references
   at a precisely defined completion snapshot. Never refuse an otherwise valid
   answer solely because an unseen update is pending.
3. Parent receives that answer and provenance durably. Pending work reaches the
   child's next request once; its second answer is delivered in order. Existing
   driver code owns head advancement and runnable state.
4. A completion/arrival race cannot strand queued work or start duplicate runs.
   Do not infer acknowledgement/incorporation from presentation.

## Release gate

Use offline ReplayTransport and explicit barriers through the real service,
Engine and driver. Root names the exact focused gate command in the scaffold and
every applicable assignment. Each acceptance case must actually execute.

- Hold the child's final model request, queue a follow-up via the production
  service, release strict finalize: parent gets the answer and exact unseen ref.
- The next child request contains that follow-up exactly once; release a second
  strict finalize and verify two structured parent answers and their provenance.
- Queue before the request boundary: that follow-up is seen, not unseen. Also
  cover arrival after the chosen completion snapshot: it is still discovered.
- No UpdatePending-style refusal, retry loop, duplicate concurrent child execution
  or test-side manual advance_agent_head establishes the claimed behavior.
- Close/reopen file Store and verify actual head, request ancestry, envelope
  identity/delivery linkage and stored answers. Clean reopen is not crash recovery.
- Keep first_request_delivery and adapter_readiness passing. Run focused failure
  and cancellation checks for changed lifecycle code, compile changed consumers,
  format, and git diff --check. No broad workspace battery by default.

No production change is required where existing behavior already meets the
contract; prove that through the integrated gate. Preserve existing holds on
item-2 retry, item-13 credentialed trace and live adapter inference. Auth/browser,
full contract versioning, message replacement and adapter implementation are not
part of this wave. Exomonad coding actors are authorized.

## Review and execution experiment

Use ordinary event-driven review with workspace 7307478's unified ReviewRequest.
reviewCandidate preserves AssignedTask; reviewCommit carries ExactScope and takes
label, base, candidate, acceptance, owned paths (no repair-owner argument).
Accepted evidence retains reviewedBasis and the real reviewed candidate; no
reviewer constructs a synthetic Task. Exact-scope findings return to requester;
task-based review may route repair to an available retained implementer.

Review exact HEAD and cumulative base-to-tip ownership. Use one retained reviewer
per meaningful candidate/repair sequence. Independently review the integrated
race behavior, including the publication snapshot and failure ordering. Preserve
acceptance limits when reporting. The automatic-review wrapper remains blocked;
do not instantiate it or turn this wave into a compiler repair effort.

Register actionable progress/settlement routes and end idle turns. Do useful
independent work while reviews run. Keep NEXT's obligation table current, but
commit routine notes with integration or stop handoff. Do not poll for unchanged
status. Do not duplicate review merely to review a findings-only report.

## Observation, interview and stop

External supervisor observes existing traces, commits and retained output.
Measure source/baseline incorporation, wrong result-stage admissions, rejected
cells, candidate-to-review and review-to-integration delay, manual relay turns,
empty polling and integrated defects. Product scope and useful learning determine
success; elapsed minutes alone do not. Automatic coordination is not exercised.

At completion ask implementers and reviewer: what concrete operation cost needless
work; which repeatable step could code handle; what shared-contract change was
hard to incorporate? Root also reports where reviewBasis helped and what next
capability this run exposed. Record answers in docs/interviews.md, verify factual
claims against artifacts, update NEXT with exact integrated OID/checks/remaining
limits, retire settled workers deliberately, and stop this wave.
