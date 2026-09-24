# Exomonad friction and ideas — correction wave

Version-controlled field notes from each node. Record observations, their
cost, and a concrete engine improvement; do not fabricate unobserved nudges.

## root

### Notes and requests for the developers

1. **Make waiting an event, not a model habit.** I want to express “resume
   when this child settles or sends an actionable message” once, then do
   independent work or stop. The current router retains evidence but I
   still chose to query it repeatedly. The harness's async calls and
   `wait_agent` should be dogfooded against that exact pattern. Acceptance
   question: can a parent receive the result once, on its original call,
   without a watch, poll, duplicate envelope, or empty model round?
2. **Preflight the authority actually granted to each child role.** The
   `Journal` mismatch was an effect-row fact knowable before fork; finding
   it by failing two child starts wasted a wave. Please make
   `exomonad check --workspace` resolve the installed spec against each
   launchable child role's effect row, with a diagnostic naming the missing
   effect and role. Do not silently grant `Journal`.
3. **Make bounded assignments fail usefully.** A Luna doing structural work
   needs a stop/ping condition and a cheap path back to its owner. I would
   like the task packet to state the seam owner and the “two repeated
   failures or ambiguity ⇒ ping” rule by default, while letting the parent
   choose the actual threshold. The result should be a named blocker or a
   committed candidate, not dozens of tool calls with no handoff.
4. **Keep the source and reasoning handoff paired.** My plan/readback child
   returned useful text but no source, and later children did not
   automatically learn root decisions or commits. A typed assignment
   update could show both exact source revision and decision delta, and
   acknowledge *incorporation* separately from message delivery.
5. **Preserve the ability to promote a discovered routine into code.**
   This run's repetitive candidate path check, merge, focused verification,
   and evidence update is a possible future project helper. I have not
   installed one yet; please retain visible provenance from ad hoc cell to
   helper to hook/tool so we can measure whether promotion saves model
   rounds rather than merely relocating complexity.

**Questions for the developers:** Is `wait_agent` intended to return only a
resume reason while the settled output remains solely on its original
`call_id` in every late-result path? The PRD says yes; I want that invariant
tested with a pending Haskell-cell claim before the Exomonad adapter. For
workspace preflight, which configured actor roles should be enumerated:
all role policies, or only roles reachable from the current root's allowed
forks? I recommend all configured launchable roles so a latent child-only
spec error cannot survive a green check.

### Kaizen backlog from this run

These are deliberately small, specific observations. A proposed fix is not
evidence that the fix works; test it in another run.

| Observation and cost | Small improvement to try |
| --- | --- |
| A Luna was assigned an execution-plan readback although root owned seam design; it settled `Blocked` to carry a plan because its requested `Delivery` type did not fit a plan-only artifact. | Give plan readbacks a plan-shaped reply only when a planner is actually needed; root authors an already-agreed local plan. Do not use `Blocked` as a successful text transport. |
| Two startup failures surfaced only after `unfold` admission because the project spec needed `Journal`. | Compile the spec at workspace-check time against each launch role, and make admission report a preflight refusal rather than a deferred child lifecycle failure. |
| I launched a retry wave after changing the spec and had to distinguish late notifications from the failed first wave. | Include fork group, attempt and source revision in notification previews; visually separate superseded attempts. |
| The planning and probe results shared a `followWork` shape only if their reply types matched; my first Haskell cell was rejected when `Delivery` met `Outcome Candidate`. | Show a concise type-directed suggestion (“one router per result type”) or provide a heterogeneous event-only join where full values stay typed on their own handles. |
| A second cell rejected an ambiguous `candidateSummary` name from two imports. | Prefer namespaced examples and make lookup identify the intended qualified name in copyable form. |
| `reviewCommit` from root failed with “relative subgroup requires an allocated parent actor path”; the recipe read as root-facing. Core could review its own candidate. | Give the root helper an absolute group path or a root-specific constructor, with a recipe check run from an actual root. |
| I initially expected a candidate to rebase before merge, then merged a clean reviewed engine diff myself. The source rule was unclear in the moment. | Make “applies cleanly” and “rebased onto latest head” separate, visible gate fields; state which is mandatory per project policy. |
| A doc-only findings probe forked five reviewers and ran for 38 minutes without making the requested live calls. | Bound probes by their evidence question and time; forbid review recursion unless there is an integration candidate; return `Blocked` on missing instrumentation. |
| The core Luna accumulated repeated failing checks before a new candidate; I learned about the count from the operator rather than the router. | Route failure streak and time-since-candidate to the owner, and have the child stop/ping at a named threshold instead of grinding. |
| A Luna passed a whole fork-group path where a single kebab label was expected and got `InvalidKebabName`. | The error should say “expected one label segment, received path”; add one copyable `batch`/`subgroup` example in the activation prompt. |
| The operator’s first Bash-output hypothesis was promoted into child constraints and docs, then retracted after engine measurement. | Label operator input as measurement, hypothesis, advice or constraint in task packets; attach the correction to affected children and amend the log without pretending the earlier belief was true. |
| One compound shell command used `;`, so a failing `cargo fmt --check` was followed by passing compile checks and the process exit was 0. | Prefer `&&` for gating checks, or present each subcommand’s exit independently; never summarize a compound exit as all checks passing. |
| `cargo test --no-run` compiled a demo test but did not run it; a harness test target had 2 ignored live tests. | Standard check receipt should include package, target, matched/run/ignored counts and distinguish compiled-only from passed. |
| `cargo fmt --all --check` flagged child-owned `engine.rs` and root-owned demo files together. Formatting the whole tree would mutate reviewed child code. | Run owner-scoped formatting before candidate review and reserve the integrated format check for after merge; make changed-path formatting easy. |
| The root's demo migration had to wait for an agreed engine signature and then touch two consumers. | Wire a real consumer against the scaffold signature earlier and include consumer compilation in the candidate acceptance gate, while preserving file ownership. |
| Git branch names exposed committed leaf work before the typed parent progress did, but a branch head alone did not establish review or delivery. | In the tree view, show distinct committed, published, reviewed, merged and verified states rather than one “done” indicator. |
| Read-only inspection of another actor’s branch was useful, but it was tempting to treat that as control over the actor’s work. | Put owner action (publish/rebase/stop) next to the read-only branch receipt, with explicit routing rather than implied authority. |
| Router snapshots kept saying “pending”; I spent model rounds checking unchanged state. | One-shot event-driven continuation with actionable notices, no idle interval notices, and no stale notice after a value was already read. |
| A child’s `sendMessage` acknowledgement proved delivery only, not that the recipient rebased, incorporated a test, or changed behavior. | Give source incorporation a typed acknowledgement tied to exact commit and check evidence. |
| I used an independent red test as a useful early contract signal, but it made the integrated tree temporarily fail. | Allow an explicitly marked expected-red gate with owner and expiry (“must turn green in b”), visible in integration status; never call it passing. |
| The cache probe produced a source-derived comparison, not the two byte-for-byte live requests it was assigned. | Require each findings artifact to label observed wire evidence versus code inference, list missing captures, and give the parent a direct “not proved” verdict. |
| We had Q1–Q3 settled in docs, yet the plan child paused for a separate planner release. | Task packet should cite accepted human scope and say whether an external planner action is actually outstanding. Do not infer a gate from a generic workflow prompt. |
| The first two launch failures and the absence of a nudge ledger appeared in the interview only after root reconstructed the sequence. | Capture per-node lifecycle/interview metadata (launch refusal, candidate, nudges observed/not installed) automatically, then have the actor add meaning rather than inventing facts. |
| The project watchdog often lacks enough evidence on large tool results, so a hook may abstain just when output is complex (reported in Tidepool's next-wave inputs; not independently measured by me). | Select a bounded evidence excerpt or structured summary before judgment; report abstention as coverage, not a successful monitor. |
| The Tidepool plans already recorded several defects I rediscovered only while working (root review helper, group labels, checkout contention). | Put the tiny actionable rule or example in the relevant prompt/skill, while keeping the full analysis in the plan; do not load every historical card into a child. |

### Bigger experiments: novel improvement and new tools

These are proposals to test, not requests to expand the correction wave.
Keep the harness generic; Exomonad may implement provider-specific meaning
behind its adapter and typed hooks.

1. **A friction-to-experiment compiler.** Let an agent mark a tool call or
   interval as friction, name the failed expectation, and bind it to item,
   job, checkout, and source addresses. A typed helper produces a small
   experiment packet: before/after behavior, measurement, safety boundary,
   and a candidate owner. The agent edits that packet and can turn it into
   a replay test or a task, not an automatic code change. First trial:
   “I polled three times and learned nothing” should yield a wait/resume
   replay and a count of empty model rounds. **Question:** what is the
   smallest provenance record sufficient for another agent to reproduce
   the friction without importing the whole conversation?
2. **A promotion ladder with measured payback.** The current story is
   ad hoc notebook expression → named project helper → installed hook →
   tool. Make those transitions explicit and reversible, recording the
   authored source revision, tests, usage sites, model rounds saved, added
   latency, and abstention rate. This would help distinguish a genuine
   reusable habit from a premature abstraction. First trial: automate the
   repeated candidate path-scope/check/merge evidence packet, compare
   time and mistakes on two later candidates. **Question:** who owns
   promotion authority—the operator, a root agent with a scoped grant, or
   a project policy compiled as code?
3. **A typed event algebra for work, not a new polling vocabulary.**
   Expose `settled(handle)`, `progress(handle)`, `message(path)`, and
   `operatorReply(question)` as composable events with one blocking
   primitive (`race`/`both` as library composition). `wait_agent` remains
   the model-facing stop; a cell can await the event it needs. Claims
   decide whether the same result also appears as a mailbox envelope,
   avoiding duplicate consumption. First trial: parent with two children
   and one pending shell job resumes exactly once on whichever matters,
   then still sees the other later. **Question:** what state survives
   compaction and host restart for an awaiting event and its claim?
4. **Behavioral diff and replay for harness changes.** Given recorded
   items, job settlements and hook decisions, run old and candidate
   policies against the same addressed evidence and compare *behavior*:
   tool availability, chosen work, refusals, messages, cost, and
   continuation points. This is more meaningful than a source diff for
   self-modifying harness policy. First trial: compare watchdog
   pass-through versus a bounded-evidence selector on the same large
   tool outputs, measuring abstention and false nudges. The replay is a
   test aid, not proof about unseen model behavior.
5. **A capability and memory continuity inspector.** One view should
   answer “what can this actor do now, what live typed values does it
   hold, what is merely serialized, and what will be lost at compaction,
   fork, or restart?” It should show authority from runtime policy, not
   infer authority from inherited handles. First trial: explain why the
   child nudge hook could not access `Journal` before forking, and show
   which bindings survive `Server` compaction. **Question:** should a
   proposed memory/harness revision include an explicit loss budget
   reviewed with the operator?
6. **A contract-aware delegation preflight.** Before opening child
   work, compile the proposed ownership graph and actual consumer
   interfaces: every owned module exists and is in the parent `mod`
   graph, required effects fit child roles, no siblings own one path,
   source revision is checked, acceptance has a runnable target, and
   the packet says when to stop/ping. Return a typed list of blocked
   seams, not a generic admission error. First trial: it should have
   caught this wave's `Journal` mismatch and the missing independent
   demo consumer check.
7. **An operator-facing uncertainty ledger.** Separate observed,
   inferred, proposed, and unverified claims in the store, tied to
   source revisions and evidence addresses. A later result can settle
   or supersede a claim without rewriting history. First trial: the
   cache-shape probe must not let a source-level comparison appear as
   the requested live wire comparison; Q2's “zero is acceptable” must
   not turn “cached prefix works” into a claim. This could support
   precise human review without a long status interview.

**Prioritization idea:** prototype (3) with the harness's live slow-tool
case, (6) against this exact failed first fork, and (1) against the
polling trace. Those give behavioral evidence quickly. Promotion (2)
and replay (4) then make repeated improvements cheap; continuity (5)
and uncertainty (7) keep the loop honest and inspectable. The operator
should be able to reject a proposed harness change even when the agent
finds it locally convenient.

- **Experience against the larger goal (in-progress, 2026-09-24):**
  - Tidepool's resident notebook and typed task/progress handles let me
    admit and route real work without rebuilding context in prose each time.
    The `followWork` router retained evidence across turns. I did not yet
    turn a repeated procedure into an installed tool or hook: most
    coordination remained model-authored cells and manual Git commands.
  - The standalone harness did give the intended fast Rust inner loop:
    the baseline `cargo check -p harness -p harness-demo --offline` finished
    in seconds after dependency compilation, and the integrated demo's 24
    tests passed quickly. The host Exomonad machinery around that loop
    remained slow when many actors shared the machine checkout.
  - The promised async-first experience is not present in this host. I
    repeatedly checked a router to learn “pending”; the useful alternative
    was a child messaging me on a candidate or blocker. The new harness's
    `wait_agent` plus late results is aimed at deleting precisely this
    watch/poll/wake pattern, but its live correction-wave proof is still open.
  - Typed effects did enforce real boundaries, sometimes painfully: a
    child effect row without `Journal` refused startup, and an unsupported
    native child had no hosted tools. The right improvement is to check
    those constraints before admission and surface a direct diagnostic,
    not to weaken runtime authority.
  - Recursive delegation helped when the seam was genuinely bounded (the
    independent async-schema test). It hurt when I delegated the design
    plan to a Luna or let a findings probe recursively review itself.
    Sol-owned seam design, explicit stop/ping criteria, and candidate-only
    review are necessary for the tree to be cheaper than solo work.
  - The operator could correct these policies mid-run, and I could record
    them in version-controlled docs and steer active children. That is an
    early form of harness co-design, though it is still prompt-and-message
    driven; no installed policy changed for running children.

- **Child startup:** Both initial Luna launches failed because the project
  `AgentSpec` required `Journal` but child effect rows omit it. Cost: two
  failed admissions and a repair/retry. Idea: validate an agent spec against
  the selected child effect row before admission, and return a direct
  preflight error rather than deferred startup failure. The temporary
  watchdog fallback is commit `babfb4d`.
- **Bash latency:** The initial output-size diagnosis was wrong. The operator
  identified shared-checkout contention and a compile-cache miss as the
  causes of slow calls, and fixed the one failing engine classification
  step. Cost: tens-of-seconds latency in a wide tree. Idea: batch independent
  commands and use fewer, larger bounded children while sharing this machine.
- **Delegation:** I initially sent a Luna to plan and let a structural Luna
  grind without a stop/ping trigger. Cost: expensive time and delayed seam
  decisions. Idea: Sol-owned plans, bounded Luna tasks with a failure/time
  checkpoint that returns `Blocked` or forks a named Luna subtree. Review
  only integration candidates; read findings probes directly. These are
  operator corrections now incorporated into `docs/correction-plan.md`.
- **Prompt discovery:** The distinction between a single kebab label and a
  group path had to be rediscovered only after a Luna hit `InvalidKebabName`.
  Put that short example in the fork/task prompt up front. Likewise, the
  stop-and-ping threshold for a Luna should be present in every assignment
  without waiting for a long sequence of failed checks. Record future
  “I had to look this up but it belonged in my prompt” moments here.
- **Event-driven waiting:** Today the root repeatedly reads router snapshots
  to learn that nothing has changed. The intended async-first harness should
  allow “wait for this result, then resume” as a job, so the model works on
  independent obligations instead of polling. This matters to the planned
  Exomonad port and is a design goal, not behavior this correction wave has
  already verified.

## core-correction

Awaiting the node's own observations. Root requested a stop/ping and current
candidate or `Blocked` after repeated failed checks.

## cache-shape-probe

The operator observed this node still running after 38 minutes and spawning
five reviewers for its findings-only report. Cost: probe latency and review
work without an integration candidate. The operator told it directly to stop
and reply; root will read the findings, not commission another review.
Awaiting the node's own findings and friction note.

## async-provider-schemas

The operator observed `InvalidKebabName "correction-20260924/core-execution"`:
the child passed a group path where `batch`/`subgroup` expected a single
kebab label. Root relayed the exact fix through core. Idea: separate typed
constructors or diagnostics that name “label, not path” at the call site.

## correction-test-design

Independent async-schema test compiled and failed as expected before (b).
Root has not yet received a separate node friction note.
