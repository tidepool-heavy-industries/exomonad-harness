# Exomonad friction — correction wave

Version-controlled field notes, not an engine bug tracker. “Fixed” below means
the local prompt or workflow changed; it does not imply an engine fix. Earlier
detail remains in Git history. Do not fabricate watchdog or nudge events.

## Adapter readiness — 2026-09-25

- `friction:` The original FIFO replay fixture had no durable request/response
  turn mapping. Store exposed `children_of` (initially overlooked), but
  `request_items` still intermingled inputs, response items, and tool outputs.
  Root landed exact `ResponsesRequest`/response turn records before
  reassigning the Store-backed provider; the leaf did not guess segmentation.
- `friction:` The provider test initially expected configuration update
  before user input, while Engine actually recorded the reverse order.
  Correcting only the expectation produced a focused 2/2 provider pass.
- `friction:` The vertical test leaf needed test-local `Auth` and `Arc<CellJob>`
  wrappers, then corrected a moved `Item`, strict finalize's
  `{"result":...}` arguments, and JSON-text decoding of durable tool output.
  Its final candidate changed only the owned test file.
- `friction:` The read-only gate audit initially misstated a replay barrier:
  `wait_requested(n)` means request `n` has already been captured. Root
  corrected insertion/check timing to envelopes inserted while responses
  2/3/4 are gated and observed in requests 3/4/5.
- `friction:` The first exact-candidate review accepted a scheduling-sensitive
  five-turn test because it read final-with-pending as a return. Engine
  actually waits and then creates another request. Root withheld integration
  and requested a scheduler-settlement barrier and sixth gated response.
- `friction:` The first same-reviewer recheck of the repair did not checkout
  the assigned commit; its receipt and result still named the old HEAD and
  repeated the old defect. Root marked that verdict invalid and reassigned
  an explicit checkout/HEAD-confirmation recheck.
- `friction:` Root also mistyped the full base OID in reviewer packets:
  `...c5ab...` rather than committed `...c5cab...`. A fresh reviewer
  correctly Blocked instead of inventing a cumulative diff. Root checked
  the actual base object and ownership diff, then resent the exact OID.

## Root: still open

| Observation and cost | Improvement to test |
| --- | --- |
| The project `AgentSpec` required `Journal`, which coding children lack. Two children failed only after admission. | Preflight the selected spec against every launchable child role's actual effect row, with a diagnostic naming role and missing effect. Do not grant the effect silently. |
| A completed leaf commit was visible in Git before its parent published a candidate. Commit, typed delivery, review, merge and verified integration were easy to confuse. | Show these as separate source-bound states; let a parent ask for the exact candidate rather than infer acceptance from a branch head. |
| A probe's latest commit touched only owned paths, but an earlier commit in its branch still changed `docs/findings.md`. Root caught this by comparing the *cumulative* diff, sent it back, and merged only the repaired branch (`cdbbd367` via `b99337e`). | Make the cumulative base-to-candidate owned-path gate first-class. A clean tip commit is not sufficient. |
| The cache probe had no exact Codex wire capture, so it sent no live requests and returned a documented blocker. It had initially edited the shared findings file before root corrected ownership. | Preflight reference evidence and shared-file ownership before inference spend. Preserve `Blocked` as a real outcome, not a passing probe. |
| `sendMessage` to the core lead returned a delivery receipt, but the operator observed no active tmux work and root saw no progress or reply. The runtime showed the request as `Working` while its workbench was idle. Core later reported its inbox was fenced and it could not receive further host notifications this run. The mechanism remains **undiagnosed**; a receipt proved neither presentation nor action. | Trace notification receipt → inbox fence → actor wake → provider turn → progress/reply. Surface a “stalled despite pending request” state and an explicit recovery action. |
| The root checked a router snapshot on status questions and found only `pending`. | Keep event-driven continuations: notify once on actionable change, and distinguish a meaningful wait from polling or an idle actor. |
| A message asked core to incorporate probe evidence into `docs/findings.md`. Delivery alone does not show that it did so. | Require an incorporation receipt tied to the exact commit and focused check. This run's prompt trial uses that rule manually. |
| The cache blocker required an operator question. The answer may never arrive. | Keep the recommendation and unanswered question in `docs/questions.md`; allow reversible work to proceed while the irreversible live comparison remains blocked. |
| The watchdog classified core's `sendMessage` checkpoint as `destructive_command` (0.9) even though the call only sent text and returned a `NotificationReceipt`. Root had already incorporated the message. This is an observed false positive for that call, not evidence of a destructive tool execution. | Classify the invoked tool and effects before scanning quoted message text for command verbs; retain the original call reference for audit. |
| In this run, the preflight nonce message was submitted but not initially presented. The lead explicitly reported that state, then later quoted the nonce. This prevented a receipt from being mistaken for readback, but required another operator-visible turn. | Make submitted, presented, quoted/answered, and incorporated distinct first-class message states. Notify the owner on presentation failure or `inbox=fenced`, not on repeated pending snapshots. |
| A settings implementation rebased onto root `master` rather than its lead's integration branch. Its three-file work passed focused checks, but the cumulative assignment-base diff contained root docs/demo changes, so the lead correctly refused it. | Give assignments a typed integration target and an `owned-diff` preflight before candidate submission. A safe rebase helper should target the owning lead's checked branch, then validate the cumulative diff before replying. |
| Two exact-commit reviews of an intentionally red settings test returned `Blocked` because the reviewer had `CommitReview` input but the reply recipe required a `Task`. The reviewer had actually matched one offline test and observed the expected failure. The operator updated `.exomonad/prompts/review.md`; re-review is pending, not yet acceptance. | Construct the review `Task` from `CommitReview` in the review helper/prompt, and make an expected-red verdict name owner, closing slice, matched count, and failure cause without implying production passed. |
| On 2026-09-24 `reload_agent_spec` refused after publishing the updated workspace source layer: `prepared engine: missing imported value Project.Shell.presentSelected`. The source file contains that function; the prior typed tool record stayed active while notebook cells saw the new layer. The cause is not yet diagnosed. | Make source-layer publication and spec reload atomic, or expose a one-step rollback/rebuild with the exact stale symbol identity. A refusal should leave both the record and imported source layer at one coherent revision. |
| The Compactor leaf needed `schemars` but owned only `compaction.rs`, not `Cargo.toml`/`Cargo.lock`, and correctly blocked instead of editing outside ownership. | Preflight dependency additions at scaffold time. Give a manifest/lock owner or root-amend those files before assigning a module leaf; show dependency needs in the admission checklist. |
| The item-2 live probe found no supported way to correlate redacted actual request bodies with slow-tool start/settle and `wait_agent` resumption. It spent no inference. Root had to scaffold a demo trace seam before the single manual run. | Treat observability as part of acceptance scaffolding: opt-in, redacted request/job correlation with fail-closed trace writes; never spend a one-shot live run without a trace path. |

## Local workflow corrections already applied

- **Planning and release:** Root authored the correction plan; Q1–Q3 and the
  correction scope were already settled, with no separate planner hold.
  Do not delegate routine seam design or invent another release gate.
- **Review and integration:** Review only source candidates at their exact
  commit. A findings-only probe is read, not reviewed. Merge the child's
  branch, never copy its files; send stale or out-of-scope candidates back.
- **Bounded children:** Task packets now say to stop/ping on an ambiguous seam
  or two failed check rounds without a candidate. This is a prompt trial,
  not an engine-enforced limit.
- **Checks:** Use `&&` for gating compound commands; distinguish tests run,
  tests compiled but not run, zero matches and ignored live tests. The
  intentionally red offline async-schema test names (b) as its owner.
- **Operator notes:** The earlier Bash output-size explanation was a
  hypothesis and was retracted; shared-checkout contention and a compile
  cache miss better explained latency. Packets no longer promote a
  hypothesis to a constraint.
- **Tool guidance:** The current fork skill gives single-label
  `batch`/`subgroup` examples, and the root review recipe now uses an
  absolute group path. These reduce prompt-discovery friction; whether
  diagnostics themselves improved is unverified.
- **This run's prompt trials:** The lead sent admission and settlement
  checkpoints; the settings implementer escalated after two failed engine
  history rounds rather than grinding a third. A test was deliberately
  expected-red under settings (c), with an owner and closing slice, and was
  not called green or merged on root. The old inbox fence has not recurred
  on the current lead; this does not establish a runtime fix.

## Remaining design experiments

1. **Event continuity:** Compose child settlement, progress, mailbox and
   command completion as typed events; retain the pending claim across
   compaction and restart. Trial: slow tool plus child, with one wake for
   the first actionable event and later delivery of the other result.
2. **Delegation preflight:** Check owned modules and `mod` lines, consumer
   interfaces, effect rows, checked source, runnable acceptance and
   stop/ping condition before forking. Reproduce the `Journal` startup
   failure as a preflight refusal.
3. **Behavioral replay:** Compare old and candidate scheduling, refusals,
   messages and costs against the same addressed events. Use it to test a
   bounded-evidence watchdog selector; report abstentions, not invented
   nudges.
4. **Capability and evidence view:** Show runtime authority, live typed
   bindings, source revision, and what would be lost at fork/compaction.
   Tie observed, inferred, proposed and unverified claims to evidence.
5. **Measured promotion:** Promote a repeated notebook routine to a
   workspace helper or hook only after comparing rounds saved, added
   latency and error rate on later candidates.
6. **Integrated candidate transaction:** Represent source base, owned paths,
   checks with matched counts, exact review, merge and post-merge verification
   as typed stages. Trial: intentionally rebase a clean three-file candidate
   onto the wrong parent and refuse it before commissioning review.
7. **Actionable swarm view:** One concise frontier view should show only
   newly presented messages, child settlements, expected-red ownership, fences
   and source advances. Keep a detailed audit trail available on demand
   without making every status turn replay the whole actor roster.

## Node notes

- **Core lead:** Own-words plan received: (b), then (c), then (d), with
  focused checks and manual live traces. At its later checkpoint, (b) had
  a committed leaf but no review/merge, and (c)/(d) had not started. Core
  reported an inbox fence, continued inline without further forks, delivered
  (b) and a findings note, then returned `Blocked` on the structural (c)/(d)
  seam. Its own-words account (retained source `6667ddc`) says child replies
  were still readable through typed `pollResponse`, while host
  notifications were not. TUI relay restored facts manually but not normal
  routing. An explicit fence drain/replay is a candidate recovery mechanism,
  not something tested here. Its interview is in `docs/interviews.md`.
- **Cache-probe leaf:** Its own account is in `docs/interviews.md`. It
  found no byte-for-byte Codex reference capture, made no live calls, and
  repaired an out-of-scope branch ancestor before integration. The blocker
  is in `docs/cache-probe-evidence.md`.
- **Other leaves:** Interviews remain due from nodes that ran. Do not infer
  their experience from root observations.

## Wave 6 root interview — 2026-09-24, mid-run

These are my own observations, not a claim that the correction wave is done.

1. **First turn.** My first tool call read `NEXT.md` alone. The second read
   `README.md`, `docs/correction-plan.md`, `docs/tree.md` and
   `docs/questions.md` in that order. I then searched PRD/code annotations,
   read the relevant PRD and coordination skills, and looked up the typed
   task/delegation constructors. The first action that advanced the plan was
   the Haskell cell admitting the preflight lead, after those reads and
   lookups (the seventh tool call in my initial sequence). One paragraph in
   `NEXT.md` could have shortened this: “At exact head `d0245b3`, Q4/Q5 are
   settled and the fork hold is lifted; first preflight a new core lead by
   sending a nonce and waiting for its echo, then assign (c) settings
   drop→set_effort→Here snapshot→first-update mirror and (d) Server
   Compactor. Root owns demo wiring, live runs and integration; existing
   modules compile, no new module without a root scaffold. The remaining
   acceptance is item-2/item-13 live traces, unanswered-call evidence,
   cache-counter probe, interviews and final combined checks.” That would
   not replace the PRD, but would make the first *action* clear.

2. **Children's first turns.** I cannot measure how many minutes each child
   spent orienting from checkpoints alone, so I will not invent a duration.
   The cache probe checked its reference inputs and builder before two live
   requests; the item-2 probe inspected CLI/transport visibility before
   declining inference; the trace-design leaf read the builder and demo
   call/sleep/wait paths; the Compactor leaf discovered an unowned dependency
   need. Those were useful reads, but the last could have been preflighted by
   root. A first-call-ready brief should name the exact source commit,
   owned file(s), one production consumer, the first file/line to inspect,
   one focused command and expected test count, the precise acceptance and
   stop/ping condition, plus who owns any shared manifest or module line.
   It should not require a child to rediscover a settled PRD rule.

3. **Operator notes and reload.** The numbered note was actionable: I called
   `reload_agent_spec` once, then handled review guidance and the dependency
   ownership issue separately. Reload **did not succeed**: it reported
   `prepared engine: missing imported value Project.Shell.presentSelected`.
   It did publish the new workspace source layer, while the previous typed
   tool record remained active. I can use Haskell cells, but I cannot infer
   that the new agent spec is installed. The note's three numbered actions
   helped; the awkward part is that “reload” was not atomic, so “after the
   reload” became ambiguous. I relayed that exact refusal and asked the
   lead to re-run both exact-commit reviews under the updated prompt; their
   new verdicts are still evidence to collect.

4. **Checkpoints.** Core sent an admission checkpoint immediately after its
   settings fork and another after its compaction fork, naming children,
   base, owned paths and first expected reply. It also sent settlement
   checkpoints. As root I reported admissions to the operator, but I did
   **not** send one explicit protocol-form checkpoint after each of my own
   fork cells (especially the preflight/probe and trace waves); I have no
   parent `sendMessage` target, but my brief updates still omitted parts of
   that template. That is a prompting gap, not evidence that the rule fired.

5. **Fence and status.** I looked at per-child delivery lines repeatedly,
   primarily at preflight and after steering. One line was
   `inbox=open; last_message=ref2 submitted/not-presented; source=d0245b3; next=await-event`
   for the preflight child; it told me the nonce receipt was not readback.
   Later an `inbox=open`/`presented` line for core made a new fence less
   likely. The line was useful when deciding whether to steer, but full
   roster dumps were mostly noise once the state was unchanged.

6. **Blocked replies.** The two reviewers were right not to fabricate an
   `Accepted` value from a mismatched `CommitReview` input; the tool/prompt
   API was wrong, not the expected-red test. It cost two review cycles,
   an acceptance-packet correction and a pending re-review. The Compactor
   implementer was also right not to edit `Cargo.toml`/`Cargo.lock` outside
   ownership. It cost a dependency scaffold amendment and a rebase/reassign;
   I added `schemars` on master at `c427057` and checked `cargo check -p
   harness`. Neither Blocked reply is completion evidence.

7. **Most annoying unasked friction.** A source advance has several
   partially independent identities—operator checkout, child source head,
   assignment base, child branch tip, review seed, and installed workspace
   tool layer. A successful `git rebase master` can still make a candidate
   invalid against its assignment base, and a refused spec reload can still
   publish half of a source-layer change. I want one concise “what revision
   am I actually using, and what would this action publish?” view.

8. **What went well.** I refused to call preparation done: Q4's two-request
   measurement was integrated as a finding, while item 2 spent no inference
   because it lacked an auditable trace. Exact base-to-tip path review also
   kept an unowned settings tip out of integration. The one-run inference
   rule, child ownership rule, typed replies, and Git merge-parent evidence
   made those distinctions possible.

9. **One-page next-run entry.** Put exact integrated head; accepted operator
   decisions and holds; one row per obligation with owner, source/candidate
   OID, checks with matched counts, unverified behavior and next action;
   active child/request references and any inbox fence; the current owner
   map and consumer seams; the one permitted live-run commands and trace
   locations; expected-red test owner/closing slice; and the five
   non-negotiable rules (no inference automation, cumulative ownership diff,
   exact-commit review, branch merge not file copy, no completion claim
   before final integrated checks). Link full PRD/plan; do not paste every
   prior conversation or status dump onto that page.

10. **Background commands versus streaming output.** I would use a
    `--background` command for a long compile, focused test, or one-shot
    live/manual trace while I review a disjoint candidate or answer a child.
    A completion notice must carry the job handle, exact command and
    working/source revision, authoritative exit code or signal, whether the
    process and cleanup are terminal, whether output is complete, a bounded
    diagnostic tail or focus match, and a durable full-output reference
    (OID/job output handle) with a no-rerun read path. The handle must be
    registered for a wake before I leave it unattended. Streaming the last
    lines (proposal a) helps diagnose a blocked wait, but alone still pins
    the model turn and can flood attention; it does not let independent work
    advance. Background work has a real risk: a lead may merge, rebase, or
    report success while a check on the older source is still running.
    Tie the job to its source OID and dependent gate, visibly mark it
    `running` until terminal, and refuse to count it as passing or to mutate
    its checkout concurrently. A notice is evidence of command completion,
    not evidence that the integrated revision was checked.

## Correction second half — 2026-09-25, root-collected `friction:` lines

- The store-drop implementer could not narrowly rebase onto a divergent
  integration branch: a plain rebase conflicted in unowned `provider.rs`,
  while guarded `--onto` refused to drop 59 commits without `DiscardIntent`.
  Root instead selected an exact source and merged the reviewed slice.
- The settings-pin reviewer was retired before a later public-visibility
  defect was found. The isolated visibility repair review found another
  envelope ingress. Root explicitly authorized the retained independent
  reviewer to inspect the full repaired cumulative tip; the narrower Repair
  was never treated as approval.
- A Compactor component candidate passed its focused test but review found
  three invariant gaps: duplicate user messages, conflicting pending
  call IDs, and a second effort-pin constructor. The same reviewer accepted
  the repaired exact component. A later Engine consumer passed focused
  tests but review found no production caller.
- The production factory replay passed once, then failed when the reviewer
  repeated it because a 1 ms job could remain pending across the final
  response. Root repaired the fixture and ancestry assertion; the same
  reviewer observed 20/20 successful repetitions. Test timing was not
  product evidence until that repair.
- The Here replacement worker kept one active turn beyond the operator's
  15-minute/30-call bound. Its root-contract steering remained submitted
  rather than presented, so the lead was asked to obtain a committed
  checkpoint or preserve the work and replace the leaf. No old Here branch
  was merged on that basis.
- A read-only Here test-design attempt to list tests did not compile and
  was reported as **no pass evidence**. On integrated master a broad
  harness library run compiled 89 tests, passed 78, failed 9, ignored 2;
  the nine stale settings-pin assertions were assigned to the `engine.rs`
  owner rather than silently counted under focused green tests.
- The stopped Here worker's active model-facing test compiled but failed
  twice with a parent Replay `replay exhausted` error before its final
  `f7a766e` commit. It is an unmerged red test, not a green Here proof;
  the rescue owner received the exact failure and must distinguish a
  missing replay response from a production output-readiness defect.

## wave 7 root interview — 2026-09-25

These are my observations as root, not an inference that the remaining live
acceptance gates passed. Times below are PDT commit times unless marked
approximate; commit-to-commit windows include work, review, and integration,
so they are not pure model latency.

### 1. Fastest path

The fastest *reviewed handoff to root merge* was the Server Compactor
component: lead commit `03beea9` at 02:02:09, root `integrate(compaction-component)`
`c3f29d0` at 02:03:38, about 89 seconds. It was possible because the operator's
**integrate first** rule made that reviewed slice my next action, and the
lead prompt says, “Each integrated slice reaches your requester the same
turn.” Earlier, the focused store drop went from implementation `46a5496`
(01:17) to root merge `eabf47b` (01:23), about six minutes. The operator's
stranded-branches note also prevented rebuilding settings/Compactor work
that already existed: I treated those branches as candidates to verify, not
as completed features. The exact branch/commit, cumulative ownership diff,
and matched test count made these short integrations possible.

### 2. Where I waited

- **Children's engineering:** Settings was not one wait: pin `26f8634`
  landed at 01:37, test `a243d1f` at 01:45, and the visibility/effort/
  provenance stack did not reach root until `f1334be` at 02:49. Here was
  the long dependency. From the provisional settings-based Here work around
  02:00 to reviewed root merge `2e456e3` at 04:43 was roughly 2 h 40 m.
  Root's active-invocation contract `e28126e` arrived at 03:14. Actor25
  stalled about 30 minutes; actor28's later active-seam interval was roughly
  03:14–03:47, ending with preserved `f7a766e` and a stop after more than
  133 responses in one turn. Actor34's rescue interval was roughly
  03:47–04:39; it preserved repaired `b4257da` at 04:30 but was stopped
  after roughly 120 responses in its later turn. These are actor-work/
  turn-boundary windows, not compile times.
- **Independent reviews:** The Compactor component reviewer required repair
  of three invariants before `03beea9`. The Engine consumer went from
  `2b5e152` at 03:08 through a missing-production-caller Repair,
  `5c17907` at 03:19 through a flaky-replay Repair, and exact accepted
  `c8c7fef` before `f504dd0` at 03:38. That is a roughly 30-minute
  review/repair/integration window, not 30 minutes idle waiting. Here's
  final candidate `b4257da` at 04:30 reached accepted review and merge
  `2e456e3` at 04:43, about 13 minutes including my dispatch and tests.
  The original settings-pin reviewer was retired before the visibility
  defect; that cost a separate explicitly authorized full-cumulative
  review by actor21 rather than `reviewAgain`.
- **Delivery to a busy child:** Messages to actor25 were not confirmed
  presented during its approximate 30-minute stall. Actor28's contract
  and active-test corrections remained submitted/not-presented inside
  its >133-response turn. Actor34 repeated the failure at >120 responses;
  the short-turn note and the operator relay into the Here owner's pane
  improved visibility, but I did not see proof that the relayed correction
  was incorporated before those turns ended. I treated transport acceptance
  as transport only, stopped/preserved the two workers through their lead,
  and reviewed only `b4257da` after a fresh bounded owner had repaired it.
- **Machine/cells:** I saw no comparable long machine stall. The integrated
  Here full-library build reported about 10.57 seconds to compile, followed
  by a demo check around 1.61 seconds; focused runs generally took seconds.
  Some large diff/test output was truncated and recovered via retained
  output reads. I cannot assign the hours above to CPU time or to a
  specific slow Haskell cell without a recorded duration.

### 3. Operator notes and rules: effect, disagreement, my miss

The stranded-branches correction changed the starting point: NEXT.md had
said (c) unassigned, but I verified retained settings work, used the prior
Compactor as a reference, and did not mistake either for master integration.
The seven rules changed routing: same-turn slice publication (`26f8634`,
`a243d1f`, `03beea9`→`c3f29d0`), one reviewer per candidate and same
reviewer for repairs (Compactor and Engine), integrate-first, Luna Medium
by default, immediate root dependency supply (`e28126e` and exact
`f504dd0` to the Here lead), and branch handoff in NEXT.md. “Do the
Engine::run seam now” moved me from waiting on the Compactor lane to
root-owned Engine/store code `2b5e152`; the reviewer then correctly
refused that component without a production caller, prompting
`5c17907` and replay repair `c8c7fef`. I respected the item-2 retry hold:
the one manual attempt returned HTTP 400, and I made no retry.

I did not follow the stale NEXT.md “unassigned” claim, or treat older
`caddc4c` component review as PRD-wide acceptance: its forced typed-turn
capability and successor/settings invariants were not the final contract.
I also declined a *mechanical rebase just because master moved* when
`b4257da`'s owned code paths were disjoint from only NEXT/docs changes
after base `38e3e14`. A cumulative path diff, `merge-tree` conflict check,
exact review, actual merge `2e456e3`, and post-merge tests gave stronger
evidence with less risk than another rebase of a stopped owner's branch.

My own clear miss was NEXT.md churn. After commit `5045fdc` put the
“commit only with integration or at turn end” rule into the prompt, I
still made **17 `docs:` commits** through this interview's starting
HEAD, many at successive turn ends (`a3cd2ac`, `9f6ada4`, `5cde20e`
and later). The owner prompt also requires the table to stay current
and an admission checkpoint after forks. Frequent asynchronous operator
messages created frequent turn ends; I treated the *allowed* end-of-turn
commit as required and let bookkeeping consume tool rounds. I should
have kept NEXT.md modified across ordinary turns and committed it with
the next integration or a real stop handoff. The short-turn rule did not
fully work either: I noticed and escalated actor28/34's overlong turns,
but only after they had already exceeded the bound by a large margin.

### 4. Making Here one owner and one review

Before assigning the first Here owner, I would have landed the root
contract that active model-facing dispatch carries
`AgentInvocation { request, call_id }` (`e28126e`, 03:14), plus an
explicit persistence barrier: *claim settled is not output appended*.
I would have named one owned `engine.rs`/`agent_runtime.rs`/`store/mod.rs`
slice, one root-owned later `tree.rs` gate, and these acceptance cases in
the first child packet: stale stored head during an active spawn, exact
call→actual output→one pin ordering, a deterministic gap after
`Store::write_output` and before `append_items`, inherited pending
claims, and a full-library run that updates old pin-index assertions.
Actor25 would not have waited for an unstated root seam, and actor28's
`f7a766e` would not have reached a red active fixture and wrong pin
order before handoff. A fixture with explicit channels/barriers rather
than timing would have caught both. At the runtime level, a 15-minute/
30-call turn budget must *force a checkpoint/wake*; text alone failed
twice, with actors28 and 34. One bounded owner could then produce the
equivalent of `b4257da`, receive actor37's one exact review, and merge.

### 5. Prompt edits I would make (proposals, not edits made here)

- **`owner.md`:** Replace the existing line “Keep `NEXT.md`'s obligations
  table current, but commit it only with an `integrate(...)` commit or
  when your turn ends” with:
  > Keep NEXT.md current in the working tree; leave routine checkpoint edits uncommitted across turn ends. Commit them with the next integrate(...) or an explicit stop handoff, not merely because a turn ended.
  I would also add:
  > A code candidate need not rebase over disjoint documentation-only commits: prove cumulative ownership, no merge conflict, exact-tip review, and post-merge checks; otherwise return it to its owner.
- **`lead.md`:** Add immediately after the current short-turn rule:
  > If a child exceeds 15 minutes or 30 tool calls without a turn boundary, request an immediate committed checkpoint; if the correction remains unpresented at the next checkpoint, stop while retaining its branch and reassign once. Do not relay new repair requests into that still-busy turn.
  This is deliberately stronger than a reminder; the actor28/34 notes
  were transported but not timely incorporated.
- **`task.md`:** Replace “Before replying, rebase onto your parent's
  current head (its integration branch) and re-run your checks there”
  with:
  > Before replying, compare the current integration head with your base. Rebase and rerun checks if code overlaps or the candidate will not merge; if only disjoint paths advanced, publish the exact checked tip, base OID, cumulative owned-path diff, and merge preflight so the parent can merge then verify.
  Add:
  > On the 15-minute/30-call limit, commit a safe checkpoint and end the turn even if a test or repair is unfinished; name it pending, never green.
  The old unconditional rebase line pulled actor34 into another long,
  unnecessary turn over docs-only commits.

### 6. First adapter-readiness slice

I would not start by implementing all ten proposed amendments in
`docs/dogfood-requirements.md`. The first vertical slice is **one root
actor, one resident Haskell cell Job, one request-boundary operator
envelope, and one strict typed `finalize` result**, exercised only with
deterministic replay. No fork/unfold, browser, or live inference is
needed to prove that slice. Its critical path is ranked amendment 1
(*AtBoundary* means each request, all unread envelopes included, never
cancel a computing cell), the minimal part of amendment 8 that gives
`/root` a `/operator` parent and a whole first-task contract, and the
resident evaluator protocol. Otherwise the delivery fence, parent-hunting,
and 3.5–51 minute steering delays simply reappear in the adapter.
The exomonad adapter must make the resident cell an async cancellable
Job with retained output, map `respond` to forced-schema `finalize`,
and keep a computing job alive while operator input waits for the next
request. Amendment 2's envelope reference states make that behavior
auditable; amendment 4 (never refuse a reply for an unseen followup)
is the next multi-request gate, before adding children.

As root I would first pin the `CellJob`/request-boundary envelope interface
and an offline `ReplayProvider` fixture. Then one applicative wave of
disjoint bounded leaves: (1) mailbox amendment-1 implementation and
the three-envelopes/three-request replay, (2) resident-Haskell async
Job/typed-finalize adapter, and (3) an independent read-only contract/test
check. I would retain the Engine consumer and integrated replay myself.
The release test is not “tools compile”: start a long cell, deliver three
envelopes at successive request boundaries without canceling/restarting
it, get one strict final result, and restart/query its durable state.

### 7. Unasked observations

The item-13 read-only audit proposed new trace correlation tooling
because JSONL events lack a Store request ID. Code inspection of
`Engine::run_loop` at `engine.rs:432–442` found an existing
request-correlated `responses_usage` Store event with `cached_tokens`.
That avoided another trace-building detour; no live item-13 result is
claimed. A zero-match filtered test can exit 0, and a focused green
crate run can coexist with nine failing library tests; I had to keep
matched counts and compiled-file scope explicit until the integrated
95-pass run at `2e456e3`. Finally, the Compactor's offline
unanswered-call experiment is recorded, but no credentialed live
Compactor call was authorized; item-2 retry and item-13 live trace are
also separate from the (c)/(d) code integrations. The operator's
interview hold means I am stopping here, not opening those gates now.
