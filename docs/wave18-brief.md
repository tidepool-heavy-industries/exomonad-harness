# Wave18 — usable standalone harness and authored coordination

## Outcome

Deliver a coherent, browser-operable deterministic harness that a human can launch,
use and reopen without Exomonad integration or inference credentials. Start from
the integrated wave17 product and the supervisor's executable release workflow.
Verify what already works before choosing missing feature slices; do not rebuild
passing components just to increase wave size.

The operator should be able to start the documented launcher, identify readiness
and the address, create/use a session, send messages, observe deterministic replies
and child activity, inspect state/history, reconnect and reopen durable state.
Exercise the harness extension points through standalone deterministic stubs with
observable decisions/evidence. Keep interrupted or unavailable behavior honest.
A complete staged-binary journey and clear operator handoff establish this milestone.
No credentialed inference, Tidepool adapter, new external service or Tailscale
reconfiguration is authorized. Existing Tailscale access can be documented after
checking the configured route; do not weaken the server's bind/authentication rules.

## The home we are building

Our longer-term goal is to replace Codex as Exomonad's agent harness and give you
an environment designed specifically for Exomonad: resident Haskell cells,
composable effects, typed actor conversations, parallel worker trees and durable
human interaction. This standalone harness is the foundation of that future home.
Your experience operating the current environment is valuable design evidence.

The other core motivation is taking advantage of newer model capabilities,
especially asynchronous tool calls. Prioritize that foundation: represent tool
work that remains in flight while useful work continues, explicit completion and
result delivery, cancellation, and late results. Preserve call/result identity and
an honest durable lifecycle through reconnect/reopen. Start by inspecting what the
existing Engine and extension seams already provide; build the smallest missing
slice and prove it with deterministic asynchronous tool stubs. Keep provider wire
assumptions explicit and unimplemented until checked against the real protocol.
This priority should shape the shared contract before dependent feature trees fork.

While delivering this milestone, identify where a custom harness could make your
work simpler or enable useful behavior that the current host makes awkward.
Describe concrete interactions, required extension points and preserved semantics;
separate demonstrated needs from speculative ideas. Feed those observations into
your final critique. Prove the standalone foundation with deterministic stubs this
wave; the Exomonad adapter and real inference integration are subsequent work.

## Root and worker organization

You are the Astra delivery and orchestration-design owner. Use parallel Luna
feature-area trees: local owners may delegate implementation, checks and independent
review and integrate their own components. Keep cross-feature semantics and final
integration with the root. A coherent refactoring/code-quality tree is welcome:
remove duplicate mechanisms, clarify ownership and types, and preserve demonstrated
behavior through real consumers. Coordinate shared-file ownership before dispatch.

Inspect current source to choose concrete areas around browser/operator use,
persistence/recovery, standalone extensions, and quality. These are useful
boundaries to investigate, not a mandatory partition. Read the PRD and relevant
consumer contracts; define a small shared request/event example before dependent
forks. Include needed manifest/module/lockfile changes in ownership or identify
the executing prerequisite owner. Use reviewed commits and merge history, not file
copying. Preserve dirty work; never stash/reset/path-checkout; commit by pathspec
without attribution trailers.

## Understand your Haskell environment

Read `.exomonad/AgentSpec.hs`, local `.exomonad/helpers` implementations, and the
main shared workflow modules you will use under `.exomonad/workspace/Project`.
Start with Work, Types, Routing, TestEvidence, CheckResults, FocusedGateExample,
ReviewFlow, BackgroundInvestigator, BaselineIncorporation and WorkflowReminders.
Read their dependencies and other production helpers when useful. Skip test/check
modules initially; consult them to resolve a specific uncertainty. Rust runtime
archaeology is not part of this orientation. Trace actual AgentSpec exposure and
published source rather than assuming that a file alone makes a callable module.

The source is an interface you may understand, critique and compose. Give each
worker the relevant callable names, published revision and input/evidence contract.
Publish project helper changes before forks and deliver later updates explicitly.
Selected Luna contexts receive required values through typed inputs; they do not
inherit arbitrary parent bindings. The baseline collector's handle must be opened
before those forks and carried in its provided typed assignment.

## Core experiment: remove small coordination sequences

A core goal is to make several feature trees progress without root relay of
routine events. Distill actual three-to-five-call bookkeeping sequences into
Haskell effects and small flowchart-style actors. Jev supplies a tightly
instructed semantic branch over permitted alternatives; code owns identities,
state, actions and continuation. Return a useful packet or unresolved question to
the engineering owner. Do not aim to replace a whole implementation assignment.

Try the validated starter kit in different suitable areas:

- Candidate-once check plans: retain every job and refusal, receive one summary.
- Failure investigation: original prepared run plus at most two supplied read-only
  diagnostic probes; preserve the original failed evidence and accessible paths.
- Review/repair: exact candidate, named repair owner, explicit semantic escalation
  criteria, bounded attempts, retained interview before owner-authorized cleanup.
- Baseline coordination: update active requests and collect exact incorporation
  reports; presentation, reported checks and verified acceptance remain separate.
- Contextual reminders: supplied episode facts, precise trigger/exclusions,
  instructed suggestion, fixed recipient and one evaluation per episode. Use
  routing observations where applicable; do not add a broad nagging loop.

Use the automation menu and compiled examples for exact calls. Preserve original
handles. Full evidence stays retained; wake the root for a shared decision,
a useful integrated slice, an actionable failure or final delivery. A missing
notice is not proof that a command or request stopped.

You may adapt session helper modules within your assigned checkout. Keep runtime
and shared-workspace engine fixes as precise supervisor feedback unless explicitly
assigned. Try direct use, delegated use, sibling reuse and callback composition;
compare different approaches where real tasks justify them. Keep useful individual
operations available when composition costs more. No usage quota or artificial
rerun of passing acceptance just for experiment bookkeeping.

## Evidence and closing critique

Record applicable opportunity, actual flow, helper/source and original job/request
references, outcome, setup/recovery cost and what still required a model. Separate
model rounds, tool calls and command jobs. Do not infer saved rounds from helper
counts or compare unrelated workloads as a controlled experiment.

Interview local owners and workers before retiring them: what automation removed
work, what remained, why applicable helpers were skipped, and what concrete
interface would help. Retain positive examples, failed attempts and counterexamples
in the existing automation trial/friction/interview artifacts. Give advertised
helpers three exposed waves before judging non-use; correct defects immediately.

Deliver checked product source, a reproducible operator walkthrough and a detailed
Haskell-interface critique: best compositions, missed opportunities, failure causes,
proposed APIs/policies and prioritized changes. Retain unmerged work and remaining
gates in NEXT.md. Stop after this milestone; the supervisor owns any successor.
