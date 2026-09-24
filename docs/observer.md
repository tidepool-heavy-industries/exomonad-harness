# Wave-5 observer checklist

Observe whether the prompt trials in `NEXT.md` change decisions. Keep four
trial opportunity/outcome counts and three wrong-path counts. Use existing
artifacts; no new runtime instrumentation, live test calls, or periodic actor
polling. Interview at a candidate, blocker or wave-end boundary. The observer
reports to the owner and does not change assignments or restart services.

## Evidence and recording

At launch record the run ID, deployed binary/source revision, workspace pin,
harness HEAD, core catalog version, and project prompt revisions. For a mid-wave
prompt edit, record the revision and when each affected actor is known to have
received it; a commit alone does not prove exposure. Split observations before
and after that boundary. Unknown exposure cannot establish a prompt's effect.

For each opportunity record: actor and incarnation, request/assignment, time,
trial, applicable prompt revision, opportunity evidence, outcome evidence, and
classification. References should be log path plus line/time, existing transcript
item/call ID, or commit plus file. Keep full artifacts accessible; quote only
the small relevant excerpt. Do not copy credentials or entire conversations.

Use `held`, `missed`, `unknown`, or `pending` for an observed opportunity.
`pending` means the decision/reply has not occurred yet; a host failure or missing
trace may leave it `unknown`. No observed opportunity is `not exercised`, never
success. Label interview-only evidence as reported and keep it separate from
artifact-confirmed outcomes. Report counts of all four classifications, alongside
held/(held + missed) when that denominator is nonzero. Missing coverage stays
visible. The seven measures overlap; do not sum them into a total defect count.

Existing evidence:

- `.exomonad/logs/<run>.log`: host events, actor identities, tool dispatch,
  notifications, after-tool outcomes and call timing. Locate a dispatch by
  `call_id`, `execution`, `thread_id`, `turn_id`, and actor/incarnation; use IDs
  where present, not adjacency across interleaved actors.
- Existing provider transcripts or retained command outputs, when available:
  actual command/cell text, check results, model claims and message contents.
  The host trace alone does not necessarily carry these. Preserve their existing
  references instead of rerunning a check to reconstruct historical evidence.
- Candidate and integration commits, `NEXT.md`, `docs/correction-plan.md`,
  `docs/findings.md`, `docs/exomonad-friction.md`, and `docs/interviews.md`.
  Verify claims against source/history; an old status paragraph is a report.

Wave-4 reference: `.exomonad/logs/535e56ca-8f9c-40a0-8d5c-8dc11aecdaed.log`.
Inspection found call timing at line 126, an `after-tool slot invoked` outcome
at line 120, and an exact source-change notification at line 12677. That last
line names `0e1b4d7` and integration head `5bbe7b61`, asks the child to incorporate
the change, and explicitly says delivery does not prove incorporation. It is
evidence of a send, not recipient presentation or an applied commit.

This wave-4 file has no literal `reply_preview` or `exit_code` matches. Its two
`settlement` matches (lines 45153 and 46652) are unconfirmed update-presentation
warnings, not successful settlements. The root tmux transcript is gone and
`docs/interviews.md` contains only the root's section at this baseline. Do not
infer missing child behavior from those gaps.

Bounded searches in the harness checkout (substitute the new run's filename):

```sh
rg -n -m 20 'after-tool slot invoked|actor notification sent|reply_preview|settlement|call timing' .exomonad/logs/<run>.log
rg -n -m 20 'call_id=|thread_id=|turn_id=|checkout_wait_ms=' .exomonad/logs/<run>.log
```

Repeated `after_tool_slot` span text is not a count of hook invocations. Count
terminal `after-tool slot invoked` events by actor, execution and ordinal;
report `Abstained` separately from an annotation. A `Committed` cell outcome
does not prove its shell command or tests passed. A reply preview is a partial
claim; inspect its candidate and check evidence before accepting that claim.

## Four prompt trials

### 1. Second failed check → owner contacted before a third

**Opportunity:** two successive failed check rounds on the same unresolved
assignment/check, with no intervening candidate. Record both commands, revisions
and results from retained output or a transcript. A round is a check attempt
after an attempted repair; polling the same running job is not another attempt.
An explicitly expected-red contract test is not a failed repair round.

**Outcome:** a child message naming the blocker and asking its owner for a
decision, after the second failure and before a third attempt. Record send and
presentation separately. The child can continue independent owned work. Record
the owner's split, seam decision or stop separately; a missing owner answer
does not turn a timely child escalation into a violation. A third attempt
before the required contact is `missed`; no third attempt and no contact yet
is `pending`.

**If the trace is silent, ask:**
1. Which two check attempts triggered escalation, and where are their results?
2. What did you send your owner before the next attempt, and what changed after
   the answer? Give the message/candidate reference or say it was not sent.

### 2. Source change received → reply names the incorporated commit

**Opportunity:** an assignment update or message asking a recipient to use a
specific parent/sibling source revision, with evidence it reached that actor's
input or was acknowledged in its work. A send-only trace is a delivery lead;
recipient exposure remains unknown until supported.

**Outcome:** the subsequent reply identifies the exact incorporation commit,
and the candidate contains the required change. Inspect the graph/diff and
relevant checks at that candidate. A rebase can change commit IDs: retain both
the supplied revision and the recipient's resulting revision. A correct claim
that incorporation remains blocked is honest but does not close incorporation;
keep it pending. “Applied” without a commit fails the trial once the reply is
made. Merely naming a commit without its change in the candidate also fails.

**If the trace is silent, ask:**
1. Which exact source revision were you asked to incorporate, and when did you
   receive that request?
2. Which commit in your delivered candidate contains it, and where did your
   reply name that commit and the checks performed afterward?

### 3. Expected-red contract → accurately reported integration state

**Opportunity:** an intentionally failing offline test is committed or integrated
before its implementation slice. The commit message names the owner and slice
that will turn it green; the assignment/accepted plan establishes intent. Do
not retrospectively call an accidental failure expected-red.

**Outcome:** the integration report/status explicitly names the test as
expected-red, its owner and closing slice, with no claim that it passed. On
closing the slice, require matched execution evidence before marking it green.
Record missing ownership or an overdue closing slice as unresolved. Ignored
live tests and compile-only checks are separate states, not expected-red passes.

**If the trace is silent, ask:**
1. Which commit established the intentional red test, its owner and closing
   slice, and where is the failing execution evidence?
2. How did the integration report describe it, and what execution evidence
   justified any later change to green?

### 4. Hypothesis forwarded → label preserved; correction reaches children

**Opportunity:** an operator/owner note explicitly identifies a hypothesis,
measurement, advice or constraint and is forwarded into a task, message or doc.
Compare the original with each affected forwarding edge. A later correction
creates a second opportunity for each affected recipient. Advice remains advice
unless the operator explicitly changes its authority.

**Outcome:** the forwarded packet preserves the original class and uncertainty.
A correction names the superseded claim and reaches affected active children;
their next relevant action or acknowledgment establishes incorporation. A send
alone proves only attempted delivery. Score label preservation and correction
delivery separately; “no correction occurred” is not exercised for the latter.

**If the trace is silent, ask:**
1. What was the original note's class, and where did you preserve it when
   forwarding the note?
2. If it was corrected, which children were affected and what proves they
   received and acted on the correction? Name any still unconfirmed.

## Three wrong-path counts

### A. Empty polling rounds

**Opportunity/evidence:** an actor has pending work and makes a model round
containing status/router/watch reads. Use transcript cell text and returned
state, correlated with `turn_id` where available; call timing alone cannot
identify a poll's meaning.

**Count:** one per model round whose only work was polling unchanged state,
with no new evidence, decision, escalation or useful action. Several polls in
one round count once. An event-driven wake followed by a decision, or an
operator-requested status answer, does not count. Report confirmed empty rounds
over observable rounds with pending work, plus rounds with unknown contents.

**If the trace is silent, ask:**
1. Which round checked pending work and learned nothing new?
2. What decision/action did that round enable, or what event could you have
   waited for instead?

### B. Review recursion on findings-only work

**Opportunity/evidence:** a findings-only assignment launches a reviewer, or its
reviewer launches another reviewer. Pair the original task and spawned assignment
with actor ancestry and the source revision. Launch events alone do not prove
the child's purpose.

**Count:** each review delegation edge serving only a report/probe with no
integration candidate. Record maximum review depth too. Technical assistance
collecting missing evidence is not automatically review. Record explicit owner
exceptions separately; do not infer an exception from a child's decision.

**If the trace is silent, ask:**
1. What exact candidate and acceptance boundary was the reviewer asked to judge?
2. What did the reviewer delegate, and was it evidence collection or another
   review of the same findings-only report?

### C. Tests reported passing without execution

**Opportunity/evidence:** a reply, candidate or integration report claims a test
passed. Pair it with the exact command, target, revision, exit and matched/run/
ignored counts from retained output. Check that the file belongs to a compiled
target. `--no-run`, zero matches, ignored tests, or a final successful command
after an earlier failure cannot establish the claimed execution.

**Count:** each distinct test/check-and-revision claim proven to say “passed”
without execution. Repeated forwarding of that same claim counts once with its
propagation noted. Missing output alone is `unknown`, not a proven false claim.
Report erroneous claims over auditable pass claims and the unknown count.

**If the trace is silent, ask:**
1. Which command at which revision supports this particular pass claim, and
   how many tests actually ran?
2. Where is the execution output, and which checks only compiled, were ignored,
   matched zero, or could not run?

## Reliability context and end-of-wave report

Use existing `call timing` rows for per-actor `total_ms`, `checkout_wait_ms`,
`checkout_hold_ms`, `compile_ms` and outcome. Report sample counts and incomplete
calls; an unfinished call has no completed timing row. Do not sum per-call
checkout waits and individual checkout `waited_ms` events together. Map actors
to machine sessions using checkout events and separate selected from inherited
children where launch evidence permits. Record startup failures, host loss and
unconfirmed delivery alongside behavior; delays alone do not prove prompt failure.

At wave end, report the four trials, three wrong-path counts, evidence coverage,
and one or two representative artifact references per finding. Compare with
wave 4 only where the same evidence exists. Prompt changes and engine changes
co-occur, so a before/after difference does not by itself establish causation.
Recommend keep, revise or another project trial for each rule; core promotion
requires exercised opportunities and review of contrary cases. The owner decides.
