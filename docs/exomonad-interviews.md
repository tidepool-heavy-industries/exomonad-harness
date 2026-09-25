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
