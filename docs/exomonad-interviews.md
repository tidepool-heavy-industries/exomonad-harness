# Exomonad operator interviews

## wave 5 root — 2026-09-24

These are my observations as the root user of the harness during this run.
Times supplied by the operator are labeled as such; I did not instrument
them myself. A delivery receipt, branch commit, or actor status is not
treated here as proof that another actor read a message or finished work.

### 1. Delegation cost

I noticed the fork admission cell was not instantaneous, but I did not time
it; the operator's measurements are **55 s** for my two-lead setup cell and
**39 s** for core's fork cell. I already used one applicative `unfold` to
admit core and probe together, so the latency did not cause me to switch
from one-call-per-child to batching—I had batched before seeing it. Later I
favored fewer, bounded children because of the earlier shared-checkout
contention report and the no-more-forks hold, not because I measured the
fork cell. I want **one batched admission call for a ready frontier**, with
per-child admission results and a fast return that does not wait for each
child's provider startup. Sequential calls should be reserved for a real
dependency, not startup overhead.

### 2. Knowing what children were doing

After admission, `readWork router` showed `core WorkOpen ... result pending`.
I inspected Git and found `a1f976f`, a committed (b) leaf, but no core
candidate publication or integration. On the operator's status question,
another router snapshot still said pending. `status detailed` showed core's
request `handling`/`Working` while its workbench was `Idle`; the leaf actors
were `IdleRetained`. I sent core a `sendMessage` asking for an actionable
checkpoint; it returned a `NotificationReceipt`. `status summary` still
showed core Working, with no active tool turn. The operator then reported
that tmux showed no subagents active. Core eventually reported, via the
operator/TUI path, that its inbox was fenced; ordinary host notifications
could not be received. The operator estimates this ambiguous interval at
about **27 minutes**. I had no status line connecting the receipt to
presentation or explaining the fence.

The line I wanted was: **“core-lead: request pending; provider idle 27m;
notification inbox fenced; last message received by host but not presented;
recover by [specific action].”** Without an established recovery action,
“fenced, cannot present” alone would at least have stopped me treating
another receipt as progress.

### 3. Messages

I expected `sendMessage` to queue for the next safe model/tool boundary
and to wake an idle lead if its request was waiting on work. I did not
expect an arbitrary mid-tool interrupt. The receipt only told me the
transport accepted the message, not that core read it. I wanted separate
states: queued, presented in a provider turn, acknowledged by the child,
and incorporated at an exact commit or decision.

I did want replacement once: I accidentally sent core a literal
`$(git rev-parse HEAD)` placeholder in a message, then sent a second
message with the actual `b99337e` head. A replace-before-presentation
operation would have avoided contradictory queued text. I would not want
silent retraction after presentation; show a superseding correction linked
to the first message instead.

### 4. Corrections reaching children

The probe's `docs/findings.md` ownership correction landed because I sent
both an ordinary message and an `updateRequest`, then saw the probe's own
reply name the new allowed paths and a new commit. The `updateRequest`
receipt itself said `UpdateUnconfirmed`—durably queued, not incorporated.
I checked the **cumulative** diff, discovered that the first repair still
carried an earlier unowned findings commit, and requested a second repair.
The clean `cdbbd367` diff contained only the probe evidence and interview
files; I merged it at `b99337e`. The correction would have failed if I had
trusted the update receipt, the latest commit's path list, or a
`sendMessage` receipt alone. It also could have failed under core's later
inbox fence.

Yes, I want a versioned way to change standing assignment instructions:
new owned paths/acceptance/source, an effective-from boundary, a
presentation acknowledgment, and a separate incorporation acknowledgment.
It should not silently rewrite work already committed under the old
assignment.

### 5. Standing objectives

I would have put this in a standing goal: “Finish the correction wave on
reviewed, merged source; (a)–(d) plus live item-2/item-13 and unanswered-call
evidence; do not call preparation complete; do not fork under an operator
hold; record interviews and unresolved gates honestly.” I wanted a reminder
when core remained idle with a pending request, when (b)'s leaf commit
appeared without review or integration, when a reset threatened a committed
interview, and before giving a final answer that might imply the wave was
done. The reminder should cite the changed fact and the next owner, not
replay the whole plan on every tool call.

### 6. The brief

Load-bearing: `NEXT.md` gave the exact starting slice, intentionally red
offline test, live-inference rule, owner boundaries, prompt trials and done
criteria. The plan's ordered (a)→(d) table and Q1–Q3 prevented me from
inventing a planner hold or treating zero cache counters as failure.
The annotations in `provider.rs`, `engine.rs`, `agent_runtime.rs`, and
`compaction.rs` located the seams. I did not use the frozen web/server/auth
design or the long earlier wave-0 findings to decide this run's code.
The old “last run stopped” instructions in `NEXT.md` remained visible after
I had formatted the files and made (b) green; I added a current checkpoint
so they would not be mistaken for current work.

I had to guess how to recover a fenced notification inbox, whether a
delivered message would wake core, and how to obtain byte-for-byte Codex
wire bytes for the probe. My added sentence would be: **“If a lead's
notification inbox is fenced, stop sending steering as if receipts prove
presentation; record its source and use an explicit recovery or handoff.”**
I would delete the stale instruction **“Format those two files first, as
their own commit”** from the active part of the brief after `a4f3cb9`,
retaining it only as historical evidence.

### 7. Cells and compile

The operator reports 2–4 s recompilation per cell. I noticed that as
friction but cannot name a decision I consciously declined *because of
that compile time*. I used `status` directly for runtime uncertainty and
`bash` for Git evidence because they were the direct surfaces, not as a
measured workaround for Haskell compilation. I would use a non-blocking
cell for slow fork admission or a command-completion join: start it, do
independent documentation/review, then receive a retained typed result.
I would not use non-blocking execution to hide whether an edit or merge
finished before I depend on it.

### 8. Tools built by hand and supplied tools worked around

I hand-built two `Task` packets, one applicative `unfold`, the
`followWork`/`notifyWork` route, `readWork` summaries, and repeated
Git cumulative-diff/check/merge gates. The typed fork and router are
valuable supplied parts; a project primitive should supply the
**base-to-candidate ownership gate with source-bound review/integration
states**, not force me to infer them from branch names.

The harness supplied `sendMessage` and `updateRequest`, but I worked
around missing presentation/incorporation evidence with child replies,
Git diffs and finally operator TUI relay. I used `status` to inspect
actors, then had to interpret `Working` plus `Idle` myself. I did **not**
build or use `reviewCycle` this run; core inspected (b) inline after the
operator barred new forks.

### 9. Watchdog

I did not observe a nudge or hold that prevented a mistake this run.
The `destructive_command` alert on core's `sendMessage` checkpoint was a
false positive: the call sent text and returned a `NotificationReceipt`;
it did not execute the destructive text it mentioned. The earlier
`git reset --hard master` alert was worth inspecting: Git reflog showed
that the reset had dropped committed interview/friction prose at
`6667ddc` even though the worktree had been clean. That was an
after-the-fact prompt, not a pre-execution hold. I could recover the
source from Git, and I incorporated core's own words under root-owned
documentation. No project nudge ledger event was observed or invented.

### 10. Review pattern

The owned-path rule caught a **real** issue: probe tip `77c6372`
looked scoped, but its cumulative diff still changed shared
`docs/findings.md` through ancestor `a1b10c3`. Returning it for an
owned-path-clean branch prevented me from merging an unauthorized edit.
Reviewing a findings-only probe would have been waste; I read its
evidence and merged the corrected documentation. Core's (b) change
was one existing `Provider::all_tools` path with a 1/1 focused test;
the inbox fence and no-more-forks hold made an independent review
cycle impractical, so core inspected structure inline and I inspected
the exact diff before merging. That is weaker than an independent review,
not equivalent to one. The branch/rebase rules cost extra repair and
handoff steps, but protected the shared findings seam. The `git reset`
incident shows that “clean worktree” is not a safe substitute for
tracking committed branch state.

### 11. One change

Make **message lifecycle and idle recovery observable and actionable**:
receipt, queued, presented, read/acknowledged, incorporated, or fenced,
with the last provider turn and one supported recovery action. This is
more urgent than shaving a few seconds off compilation or admission.
It would have turned the 27-minute “Working but idle” interval into a
clear handoff instead of repeated status checks and operator tmux
intervention.

### 12. Unasked observation

The cache probe was asked for byte-for-byte Codex parity but lacked an
actual ordered-header/serialized-body reference. It correctly made no
live requests rather than spend inference on an invented comparison.
The blocker should have been checked before fork. I also learned that
`git reset --hard` can discard **committed** node interview work while
`git status --short` is empty; the reflog, not the worktree status, made
that loss visible. Both are examples of why a source- and
evidence-bound state view matters more than a generic “done/pending”
label.

### Round 2: shapes

These are deliberately narrow API sketches, not claims that these names
already exist. `Response`, `Eff`, `GitOid`, `AgentRef`, `WorkProgress`,
`sendMessage`, `updateRequest`, `readWork`, and the typed `unfold`/router
are the current vocabulary. Every read below is retained evidence, not
a command to poll until something changes.

#### A. Message lifecycle

```haskell
data Incorporation = AtCommit GitOid | AtDecision DecisionId
data MessagePhase = Receipt | Queued | Presented TurnId
                  | Acknowledged TurnId | Incorporated (NonEmpty Incorporation)
                  | Fenced FenceReason
data MessageState = MessageState MessageRef NotificationReceipt MessagePhase
messageState :: Member Notifications e => MessageRef -> Eff e MessageState
```

`Receipt` means the service accepted the send; `Queued` means retained for
presentation; `Presented` names the provider turn that contained it.
`Acknowledged` means the child explicitly referenced `MessageRef` in a
reply/ack effect, **not** that the model was forced to agree or even read
every word. `Incorporated` points to one or both an exact commit and a
durable decision record; it is never inferred from presentation. `Fenced`
must retain the unsatisfied message and say whether it can later be
presented. A parent calls `messageState` once on a changed-event wake, not
as a status-poll loop.

#### B. Replace before presentation

```haskell
data ReplaceResult = Replaced MessageRef | LinkedCorrection MessageRef
replaceMessage :: Member Notifications e
               => MessageRef -> Text -> Eff e ReplaceResult
```

The operation atomically replaces a queued, unpresented message, including
one held behind a fence. If already presented, it appends a new message
linked to the original and returns `LinkedCorrection`; it never rewrites
what the child already saw. This would have replaced my literal
`$(git rev-parse HEAD)` message before presentation, if it was still queued.

#### C. Versioned assignment update

```haskell
data AssignmentDelta = AssignmentDelta
  { ownedPaths :: [Path], acceptance :: Text, sourceBase :: GitOid
  , effectiveFrom :: CellBoundary }
data UpdateAck = UpdatePresented AssignmentVersion TurnId
               | UpdateIncorporated AssignmentVersion (NonEmpty Incorporation)
updateRequest :: Response r -> AssignmentDelta -> Eff e UpdateHandle
cellAssignment :: Member ActorContext e
               => Eff e (AssignmentVersion, CellBoundary)
```

`CellBoundary` is the **next** cell admission sequence, never the middle
of a running cell. The cell sees its pinned version and boundary through
`cellAssignment`; a late update changes the next cell's assignment, not
the checks already in flight. `UpdatePresented` is a runtime fact;
`UpdateIncorporated` is a child-attested source/decision fact. A queued
`updateRequest` alone establishes neither.

#### D. Batched admission

```haskell
data Admitted r = Admitted
  { childReply :: Response r, providerReady :: Response ReadyState
  , childProgress :: Progress WorkProgress, childRef :: AgentRef }
forkBatch :: NonEmpty (Branch child Task r)
          -> Eff e (NonEmpty (Either AdmissionError (Admitted r)))
```

Return every per-child admission promptly in one call; provider startup
settles later on each retained `providerReady`. The parent can install one
`followWork` router over admitted `childReply`/progress handles, do
independent work, and observe readiness on a completion event. It must
not call admission “running” or integrate a branch merely because the
`Admitted` value exists. Heterogeneous branches can continue to use
the existing applicative `unfold`; I would not add a second general fork DSL.

#### E. Standing goal

```haskell
data GoalTrigger = IdlePending AgentRef | UnreviewedCommit GitOid
                 | BeforeDiscard GitOid | BeforeFinal
data Goal = Goal { objective :: Text, triggers :: Set GoalTrigger }
data Reminder = Reminder
  { changedFact :: Text, evidence :: EvidenceRef, nextOwner :: AgentRef }
```

The runtime emits one `Reminder` when a relevant fact *changes*, with a
source address and next owner; it does not repeat the whole goal at every
tool boundary. My goal would encode the correction-wave done criteria
and no-more-forks hold; `BeforeFinal` would have reminded me that live
item-2/item-13 and (c)/(d) were still open.

#### F. Ownership gate

```haskell
data Scope = InScope | OutsideOwned [Path]
data Stage = Committed | ReviewPending | Reviewed ReviewRef
           | Merged GitOid | Verified GitOid
data CandidateView = CandidateView
  { base :: GitOid, tip :: GitOid, cumulative :: [(Path, ChangeKind)]
  , scope :: Scope, stage :: Stage }
candidateView :: Assignment -> GitOid -> Eff e CandidateView
```

`cumulative` is the parent-view diff from the **assignment base** to
the exact candidate tip, never just the last commit. `OutsideOwned`
forbids review/merge. `Reviewed` names the review of that exact tip;
`Merged` names the integration commit; `Verified` names the integrated
source checked. The minimal forward path is committed → reviewed →
merged → verified, with a scope refusal before review and a new candidate
tip restarting review. A report-only artifact does not enter this machine.

#### G. Destructive-command hold

```haskell
data DiscardIntent = DiscardIntent
  { expectedTip :: GitOid, target :: GitOid, reason :: Text }
```

Require this intent before `reset --hard` moves a branch backward,
`rebase --onto`/`--skip` drops commits, `branch -D` removes its last ref,
or a force-update overwrites a published ref. Compare `expectedTip` with
the actual ref **before** execution; show the commits that will lose that
ref. `git clean` and checkout of uncommitted edits have no OID to name,
so require an exact path/diff acknowledgment instead. Refusal: **“Ref
is at 4b15434, not expected 8d45d32; reset would drop committed
6667ddc. Inspect/rebase or confirm the actual tip.”** A clean worktree
does not waive this hold.

#### H. Status line

```text
core-lead req=1 pending; provider=idle 27m; inbox=fenced(reason, since);
  last_message=ref9 queued/not-presented; source=8d45d32; next=route-recovery
core-lead req=1 pending; provider=running turn=T; inbox=open;
  last_message=ref9 presented@T/ack-pending; source=8d45d32; next=await-event
```

Both are **one logical per-child line** in `status`, not a derived guess
from `Working` plus `Idle`. `source` is the child's checked head, not
root's head. `next` must name a supported action; if route recovery is
unavailable, say `next=handoff`, not `retry sendMessage`.

#### I. Revision to round one

I would weaken my round-one wording that the runtime *showed* a fenced
inbox: `status` showed Working/Idle and notification receipts; **core
reported** the fence later through the operator/TUI path. I would also
distinguish `providerReady` from a child actually running work. Neither
correction changes my one priority: expose presentation/fence/recovery
state before optimizing fork or cell latency.
