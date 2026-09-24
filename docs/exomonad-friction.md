# Exomonad friction and ideas — correction wave

Version-controlled field notes from each node. Record observations, their
cost, and a concrete engine improvement; do not fabricate unobserved nudges.

## root

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
