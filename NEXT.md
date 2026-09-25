# NEXT: finish the correction wave, second half

You are the root. This file is where the last run stopped and where you start.
For planning it replaces `README.md`, `docs/correction-plan.md`, `docs/tree.md`
and `docs/questions.md`: what you need from them is quoted below (the one page,
"Settled answers", "Correction items", "Node protocol"), so do not read them
before planning. Open one only to edit it; append a question to
`docs/questions.md` without reading it first. The `TODO(correction-wave …)` and
`FIXME(correction-wave …)` comments in `crates/` are the map into the code;
each names the PRD rule it serves.

## One page (read this first; everything after it is reference)

**Integrated head.** `master`. Resolve it with `git rev-parse master` before any
fork and pass that full OID as every child's base. This page is updated in
`integrate(item2-trace)`; use that commit's resolved OID, not the earlier
`859258f` parent. The previous findings integration is `ee27b86`
(item2-live-gap); the trace scaffold is `8779c47` and the Compactor
dependency amendment is `c427057`.

**Obligations.** Detail per row under "Where the last run stopped".

| item | owner | source / candidate | checks | unverified | next |
|---|---|---|---|---|---|
| (a) one entry | done | `9da6efe`, `a9f7a12` | integrated | nothing | nothing |
| (b) async tools | root | `d8097c3`; trace leaf `fb80016`; consumer `a42920f`; tree schema repair under review | offline demo 35/35; manual attempt: HTTP 400 on request 2 | live `wait_agent` continuation | no retry without operator authority; review tree fix and retain redacted attempt |
| (c) settings items | settings-preflight-v8, with retained prior candidates | store drop `46a5496` merged at `eabf47b`; prior stack `5876336` (unmerged) | drop 1/1 integrated | set_effort, here fork, live trace | verify/rebase prior candidates; implement remaining wiring |
| (d) `Compactor` | Compactor lead `compaction.rs`, with retained prior candidate | prior stack `caddc4c` (unmerged); scaffold `c427057` | `cargo check -p harness` on scaffold | integration, unanswered-call experiment | inspect prior candidate before rebuilding; wait settings prerequisite |
| cache probe | done | `d0245b3` | 0 then 20,736 cached tokens | general cache behavior | nothing |
| interviews | every node | `docs/interviews.md` | root, probe, core present | later nodes | one section per node that runs |
| wave-7 admission | root | base `763764d389763df6ab3ac0cc65b990c4b82fe1b7` | store-drop-v7 owns `crates/harness/src/store/mod.rs`; core-preflight-v7 owns no files | drop integrated; preflight passed | no next action |
| core-compaction-v7 follow-up | preflighted Sol lead | base `763764d389763df6ab3ac0cc65b990c4b82fe1b7` | owns `crates/harness/src/compaction.rs`; children compaction-server-v7 (implementation) and compaction-contract-v7 (read-only) | settled Blocked as component: retained repair e87b47f only compaction.rs, child final review pending; old caddc4c stacked | component can land independent of settings; follow-up below; root owns Engine::run wiring |
| store-drop review | reviewer | candidate `46a5496e38e8493a4de63eeedd007997b0084e3b`, base `763764d389763df6ab3ac0cc65b990c4b82fe1b7` | read-only review of `crates/harness/src/store/mod.rs` | accepted, merged at `eabf47b`, integrated check 1/1 | no next action |
| settings-preflight-v8 admission | root | base `eabf47b6b8b49e501a39960e02f989b8f4e89625` | no files | passed | quoted `amber-orbit-eabf47b`; follow-up sent |
| settings-recovery-v8 follow-up | preflighted Sol lead | base `5c2c8bc89b310d0b77b3e484a2a7f0811f32ec1b` | owns `crates/harness/src/{engine,agent_runtime}.rs`, `store/mod.rs`, `crates/harness/tests/correction_wave.rs`; root owns contracts; wave-1 settings-pin-v8 (Luna Medium, engine/store) and here-candidate-probe-v8 (read-only) | admission checkpoint received; local `26f8634` 2/2 and `a243d1f` 1/1 reported reviewed, but root found public trusted-write API; neither merged | repair visibility and reviewAgain, then next exact integrate commit through progress |
| compaction-component-v9 follow-up | retained Sol lead | base `48807d3a4fc25b692975329f4e0219be60c6a783` | `crates/harness/src/compaction.rs` | pending | first reply: request-18 final exact commit/checks; review and integrate component without waiting for settings; deliver remaining Engine/live gates explicitly |

Expected-red tests: none recorded. One committed red gets a row here with its
owner and closing slice.

**Active refs and fences.** Children and requests live in `status`, one
delivery line per child. A retained `exomonad/...` branch is a candidate when
a child's reply names its commit **or when the branch, last commit, contents,
and review state are listed under "Where the last run stopped" below**.
Fences are recorded in `docs/exomonad-friction.md`; an `inbox=fenced` line
means stop steering that child (rule below).

**Owner map.** Root: this file, `docs/correction-plan.md`, workspace and crate
manifests and `Cargo.lock`, `crates/harness-demo/src/{main,driver,tree}.rs`
consumer wiring, integration commits, interviews, final checks. Contract files
(list in `docs/tree.md`) change only by a root `amend(<label>)` commit. Each
leaf owns the paths in its Task. Server, auth and web are frozen this wave;
hooks are wave1.

**Permitted live-run commands.** By hand, once each, trace in
`docs/findings.md`; nothing else spends inference:
`cargo test -p harness --lib live_subscription_response -- --ignored`,
`cargo test -p harness --lib live_subscription_engine_final_answer -- --ignored`,
and the item-2 and item-13 manual runs once their trace seams are integrated.

**Five non-negotiable rules.**

1. Never automate a test that spends inference. Live tests stay `#[ignore]`,
   run by hand once, trace in findings (PRD `inference spend !`). A red offline
   test on master between slices is fine and names its owner.
2. Check the cumulative base-to-candidate diff for owned paths
   (`git diff <base>...<tip> --stat`), not the tip commit; an ancestor can
   carry an unowned edit.
3. Review and incorporation name the exact commit. "Applied" without a commit
   is nothing.
4. Integrate = merge the child's branch at its exact commit, or send it back.
   Never extract files from a stale candidate. Scaffold owns every `mod` line.
5. Do not call preparation complete: nothing closes before the final checks on
   integrated source (standing objective below).

## Fork recipe

The PRD is `PRD.md` at the repo root; cite it by heading (`PRD.md` § settings
items). You need not re-read the fork or coordinate skills to write this.
`base` is the full OID of your checked integration head, the commit your
checkout is at when you fork. A child whose `parentAgent` is Nothing cannot
message you, so each obligation carries base, PRD section, test command and
how the child reports. Every obligation is this first-call-ready brief, filled in:

```text
Source: <full 40-hex commit>
Owns: <paths>. Manifests, `mod` lines, Cargo.lock: <owner, root by default>; ask, never edit.
Consumer: <one production caller, file::symbol>
Start at: <file>:<line>
Check: `<one focused command>`; expect <N> matched, <N> passed
Acceptance: <exact sentence>; PRD.md § <section>
Stop and report when: <condition>; the same check fails twice; a file you do not own must change
Report: reportProgress for checkpoints; respond for the result
```

One Luna leaf and one Sol lead in one admission cell, with a fresh group label
per wave:

```haskell
let base = GitOid "<full 40-hex integration head>"
let leafTask = task [label|store-drop|]
      "Source: <full 40-hex integration head>\nOwns: crates/harness/src/store/mod.rs. Manifests, mod lines, Cargo.lock: root; ask, never edit.\nConsumer: crates/harness/src/engine.rs, the store.append_items calls\nStart at: crates/harness/src/store/mod.rs:549 (the TODO above Store::append_items)\nCheck: `cargo test -p harness --lib drops_foreign_configuration_update`; expect 1 matched, 1 passed\nAcceptance: a configuration_update not authored by the harness is dropped at append; PRD.md § settings items\nStop and report when: the drop rule needs a schema change; the same check fails twice; a file you do not own must change\nReport: reportProgress for checkpoints; respond for the result"
      ["crates/harness/src/store/mod.rs"] "drops_foreign_configuration_update: 1 matched, 1 passed" base
let leadTask = task [label|core|]
      "Source: <full 40-hex integration head>\nOwns: crates/harness/src/compaction.rs. Manifests, mod lines, Cargo.lock: root; ask, never edit.\nConsumer: <engine.rs call site; none exists yet, name it before forking>\nStart at: crates/harness/src/compaction.rs:11 (TODO(correction-wave d), the target trait)\nCheck: `cargo test -p harness --lib compaction`; report matched and passed counts\nAcceptance: Compactor, CompactContext, Summary, NewWindow { carried } with Server only; PRD.md § compaction (pluggable)\nStop and report when: the pending-call invariant cannot hold; the same check fails twice; a file you do not own must change\nReport: checkpoints to parentAgent, else reportProgress; respond with the Delivery"
      ["crates/harness/src/compaction.rs"] "Compactor per the PRD shape, focused tests green, one integrate(core) commit" base
((leaf, leafProgress), (lead, leadProgress)) <- unfold (batch "correction" "wave-7") $ (,)
  <$> childWithProgress @WorkProgress @(Outcome Candidate) (withReport Silent (lunaTaskFrom [label|store-drop|] Medium currentCheckout leafTask))
  <*> childWithProgress @WorkProgress @Delivery (withReport Silent (withInstructions (projectPrompt "lead") (solTaskFrom [label|core|] High currentCheckout leadTask)))
```

`solTaskFrom` carries the task prompt; the outer `withInstructions (projectPrompt
"lead")` replaces it with the lead prompt, so a lead knows it owes a Delivery.
End that cell. In the next, one router per result type:

```haskell
leafRouter <- followWork [("store-drop", leaf, leafProgress)] (notifyWork me (withCheckpoints (workMessage candidateSummary)))
leadRouter <- followWork [("core", lead, leadProgress)] (notifyWork me (workMessage deliverySummary))
```

## Settled answers (quoted from `docs/questions.md`)

- Q1 auth: use the subscription login in `~/.codex/auth.json` read-only
  (`tokens.access_token`, `tokens.account_id`) against the streaming Codex
  Responses endpoint. Never write, copy, refresh, print, log, commit or store
  the token. On 401, stop and ask rather than refresh.
- Q2 cache: "Verify history and report cache unobserved; a positive hit is not
  required."
- Q3 paths: "Single segment, as built. A child path is `/parent/task_name`."
  The `core-` prefix is a naming convention, not nesting.
- Q4 probe: "No capture exists and none is required. Byte-for-byte parity was
  never the question; the question is whether `cached_tokens` reads above zero
  on the second of two identical-prefix requests through our own builder."
  Only if no, diff headers and body field order against Codex's builder in
  source, as a field list. Answered yes on `d0245b3` (0 then 20,736).
- Q5 authority: "The hold was a constraint on that run only and is lifted.
  Admit a fresh core lead; preflight that it receives a message before giving
  it work." "There is no provenance seam to scaffold first. PRD `settings
  items` decides it (harness-authored only, forged ones dropped) and the
  annotation at `Store::append_items` states the rule in one sentence. (c)
  starts as implementation, not design."

## Correction items (quoted from `docs/correction-plan.md`)

1. One entry: `Engine::run(head: Option<RequestId>, new_items: Vec<Item>,
   cancellation, incoming)` is the sole public run entry. No shims.
2. Async jobs: every provider tool and crate verb except `wait_agent`
   advertises `async: true`. Prove a live slow `sleep`: the model continues
   with the call pending, then `wait_agent` resumes when the original call
   settles. Record redacted request bodies in `docs/findings.md`.
3. Positional settings: store `configuration_update` as an item; implement
   `set_effort`, adjacency replacement and the fork strip list. Effort is
   pinned through the item; any request-level field only mirrors its first
   update. Prove live child history = filtered parent prefix + one update,
   recording cache counters. A `configuration_update` enters the store only
   via `set_effort`, a `here` fork's re-pin, or a compaction's fresh pin; any
   other one is dropped at append. No SQL migration.
4. Compaction: PRD `Compactor`, `CompactContext`, `Summary` and
   `NewWindow { carried }` with `Server` only. Preserve the pending-call
   invariant and record the unanswered-call experiment in findings.

Hooks and provider-trait redesign beyond what (a)–(c) require are not in this
wave. The wave ends when all four are integrated on master with their live
traces in `docs/findings.md`, master is green, and `docs/interviews.md` has one
section per node that ran.

## Node protocol (quoted from `docs/tree.md`)

`scaffold → commit → children → children repeat or implement → integrate → review(1 cycle) → report`

- Scaffold commit `scaffold(<label>): <one line>`, body listing modules and
  owner label each; every child module exists as a compiling stub with its
  `mod` line. Must pass `cargo check -p <crate>`.
- Integrate commit `integrate(<label>): <children merged>`, body listing every
  contract amendment. A contract change is a new `amend(<label>): <what>`
  commit that every child merges.
- Contract files (a leaf never edits them; it asks):
  `crates/harness/src/{item,model,provider,hooks,agents,mailbox,compaction/trait,protocol}.rs`,
  `crates/harness/src/store/schema.sql`, `web/src/protocol.ts`,
  `crates/harness/src/text/`.
- Leaves: `cargo check -p <crate>` and `cargo test -p <crate> <name>` only, no
  workspace-wide builds; commit by pathspec (`git commit -F msg -- <paths>`);
  no `todo!()` in a delivered module.
- Operator question shape: `[<label>] <question>` + `default: <what you do if
  no answer>` + `blocks: <what waits>`, appended to `docs/questions.md`.
- Interview (every node, in its reply, then one section per node in
  `docs/interviews.md`): which scaffold item changed under you, who, how you
  learned; what you needed from a sibling that the scaffold did not give;
  where an API differs from its docs; what integration cost your parent that
  a different split avoids; what you would scaffold differently; which nudges
  fired on you, right or wrong.

## Where the last run stopped (2026-09-24, one hour, two runs into this wave)

- (a) one engine entry: done (`9da6efe`, `a9f7a12`).
- (b) async tools: schema done on master (`a1f976f` via `d8097c3`).
  `Provider::all_tools` stamps `async: true` on everything but `wait_agent`;
  the offline correction-wave test is green. The trace seam is integrated in
  `integrate(item2-trace)` from reviewed `fb80016` plus root-owned `main.rs`:
  deliberate tree-only `--trace-jsonl`, production-built redacted request
  structure, allowlisted `wait_agent` resume status, and correlated sleep
  start/settle events. On the combined source, trace tests compiled/matched
  9/9 passed, provider/CLI focused tests 1/1 each, `cargo check -p
  harness-demo`, format and diff checks passed. **LIVE item-2 remains open**:
  the single authorized manual attempt on `a42920f` produced a second request
  with `sleep` still pending, but that request returned HTTP 400 before a
  `wait_agent` continuation. Four redacted events are retained in
  `docs/item2-live-attempt.jsonl`; analysis is in `docs/findings.md`. Root
  found that `TreeProvider::all_tools` bypassed the crate's async stamping;
  its offline repair is in progress. The 400's server-side cause is unknown.
  Do not make another credentialed item-2 run unless the operator explicitly
  grants one additional attempt (question in `docs/questions.md`).
- (c) settings items: the append drop rule is reviewed and integrated at
  `eabf47b6b8b49e501a39960e02f989b8f4e89625` (focused 1/1, cargo check).
  Operator reported retained settings work from the previous run, not yet
  integrated; verify it against current master, do not assume correctness:
  - `exomonad/correction-second-half/preflight/branches/core-preflight-20260924`
    at `58763365674fd5db63c013f6aebc02386d707617`: stacked settings append
    and test integration; prior lead reported integration; not reviewed here.
  - `exomonad/correction-second-half/preflight/core-preflight-20260924/settings-wave/branches/core-settings-impl-20260924`
    at `c0932bb571c3406a296a69087e11e76cee016719`: positional effort
    settings implementation; review state not independently verified here.
  - `exomonad/correction-second-half/preflight/core-preflight-20260924/settings-wave/branches/core-settings-tests-20260924`
    at `922442781597750fb75b49721a0fae2c70a9b047`: settings append test;
    operator reports reviewed green in `.../settings-test-green/branches/review`,
    not independently verified here.
  - `exomonad/correction-second-half/preflight/core-preflight-20260924/here-wave/branches/core-here-snapshot-20260924`
    at `b7b3f294c64ee292c34053be1d0b2735fe99cae8`: Here-fork pending
    claims; operator reports review in sibling `.../here-wave/branches/review`,
    not independently verified here.
  The provenance seam is decided (Q5 above).
  Order inside (c): drop rule at append → `set_effort`
  appends the positional item → `here` fork strips by item type and re-pins
  with one fresh update → request-level effort mirrors the first update in the
  sent history → live item-13 trace with cache counters last.
- (d) `Compactor`: the current lead is inspecting retained prior work before
  rebuilding. `exomonad/correction-second-half/preflight/core-preflight-20260924/compaction-fresh/branches/core-compactor-fresh-20260924`
  at `caddc4c231f3cb21f2dbefd57c9b2c2f2e845d89`: stacked settings plus
  Compactor; operator reports review at sibling `.../compaction-fresh/branches/review-1`
  ("Preserve user and claimed calls in server compaction"), not independently
  verified here. It also changes unowned settings paths, so await the settings
  integration/rebase. `Server` only; unanswered-call experiment → findings.
- Cache probe: measured on source `d0245b3` by the bounded probe. Two requests
  through our builder with one key returned `cached_tokens` 0 then 20,736;
  redacted usage blocks are in `docs/cache-probe-evidence.md`. This answers
  Q4 yes for that pair, not general cache behavior or product approval.
- Interviews: root, probe, core sections exist in `docs/interviews.md`. Every
  node that runs this time adds its own.

When a slice lands, update its row on the one page and its bullet here in the
same commit, so neither shows finished work as open.

## What went wrong last run, so you avoid it

- The core lead's message inbox stopped delivering (an Exomonad-side fault,
  carded there). Root spent 27 min reading delivery receipts as progress.
  Rule: **preflight the lead**. Send the new lead one message and require a
  reply that quotes it before giving it work. A receipt proves transport, not
  reading.
- The lead blocked on a rule the PRD already states. New nudge
  `settled_rule_as_blocker` in `docs/nudges.md`. If a PRD `!` rule seems
  impossible, the reply quotes the code that makes it so; otherwise implement it.
- The probe blocked on a criterion (byte-for-byte parity) nobody needed. Read
  the acceptance sentence as written in `docs/tree.md`; it is now a measurement.

## Rules in force (besides the five above)

- Review reads structure before bugs. Root implements only scaffold and seams;
  every bounded leaf is a child.
- Server, auth and web are frozen this wave. Hooks are wave1.
- Operator notes are advice unless they say constraint. The no-more-forks hold
  from the last run is lifted (Q5).
- `status` shows one delivery line per child. If a lead's line shows
  `inbox=fenced`, stop steering it; receipts do not prove presentation. Record
  the fence and its source in `docs/exomonad-friction.md`. The host resubmits
  on its own (`next=resubmitting`); if the fence is still there at the next
  checkpoint, hand the work to a fresh preflighted lead.
- Expand every value before sending a message. A correction names the message
  it corrects; do not rely on the child reading only the latest one.
- Before forking a probe, confirm its reference inputs exist. A probe without
  its reference is `Blocked` before it starts.
- Every lead sends its parent an admission checkpoint right after a fork cell
  (children, base commit, owned paths, first expected reply) and one checkpoint
  per child settlement: `sendMessage` through `parentAgent`, or reportProgress
  when `parentAgent` is Nothing.

## Standing objective (re-read before any final answer)

Finish the correction wave on reviewed, merged source: (a) to (d) plus the live
item-2 and item-13 traces and the unanswered-call evidence. Do not call
preparation complete. Do not fork under an operator hold. Record interviews and
unresolved gates honestly.

## Prompt trials this run

Project-level rules; the observer (`docs/observer.md`) scores each one from the
log or transcripts.

- After two failed check rounds with no candidate, does the child ping its owner before a third?
- Is a test committed red on purpose named as expected-red, with owner and closing slice, in every integration status?
- When a child's status line shows fenced, does the parent stop steering and hand off within one turn (once the fence outlasts a checkpoint)?
- Does every lead send the admission checkpoint unprompted?
