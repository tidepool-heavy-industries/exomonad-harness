# Exomonad friction and ideas — correction wave

Version-controlled field notes from each node. Record observations, their
cost, and a concrete engine improvement; do not fabricate unobserved nudges.

## root

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
