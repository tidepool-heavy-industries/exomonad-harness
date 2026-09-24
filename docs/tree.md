# tree guidance: wave 1

fmt: compressed. read PRD.md first. `!` hard rule. `⇑` escalate to parent. `⇓` fork child.

## waves
- **wave0 (tutorial-level, real but small)**: the same node pattern, a small tree: root ⇓ ONE lead (`core`) ⇓ ≤2 leaves at a time (transport+store first, then loop+demo). output: `harness` crate w/ transport, store, loop (wait_agent, spawn/send/followup/list, envelopes, claims, checkpoint, set_effort), the `Compactor` trait w/ `Server` only, the demo provider as a CLI binary (no server, no web), event stream to stdout/jsonl. acceptance: items 1–5, 11, 13, 15(a: input as `/operator` envelope, no page), 16. purpose: prove the loop + the tree pattern before fan-out; every contract defect found here is cheaper than in wave1.
- **wave0 pre-flight (root, before any scaffold; two live requests, results → `docs/findings.md`)**: (a) does gpt-6 emit `phase` on assistant messages in a stateless Responses call w/ a function tool present? FINAL_ANSWER detection depends on it; if not, the child's reply = its last message after `finalize`/stop. (b) send one assistant-role message in the envelope text + one `configuration_update` in the input; confirm 200 and the model addresses the sender. Record actual cache-write and cached-token counters on both identical-prefix requests; a positive cache hit is not a correctness condition. The subscription endpoint returned zero for both counters despite the stable >1,024-token prefix, while phase and envelope behavior were observed via `response.output_item.done`. Both probes are one request each; do them first.
- **wave0 review (operator, 2026-09-24)**: what landed is a real foundation: store (items, request DAG, envelopes, claims, agents table, atomic admission, migrations), engine w/ wait_agent + late settlement, envelope rendering per PRD, live prompt-fork tree verified, findings.md honest throughout. keep all of it. what did NOT land, and is why the next wave is a correction wave, not wave1: (1) no `configuration_update` anywhere; effort is a request-level field; no `set_effort`, no positional settings family, no fork strip list ⇒ item 13 impossible. (2) no tool carries `async: true`; the "continue around pending calls" path resends a `function_call` w/o its output, which the API rejects unless the tool is async; it has only run against the mock transport ⇒ items 2, 3, 11 unproven live and probably broken. (3) no `Compactor` impl, not even `Server`; the trait lacks `CompactContext`/`Summary`/carried calls. (4) no hooks: `decisions` table takes a string + blob; none of the eleven typed hook points exist on the provider. (5) no `prompt_cache_options`, breakpoints, or `allowed_tools`; cache counters read zero on every live call while codex sees cached tokens from the same endpoint ⇒ anomaly, probe it. (6) no interview answers written. drift in kind: provider trait stringly (`&str` name, JSON in/out, unbounded progress channel, no cancel token, no `JobVerbs`; agent verbs routed through the provider instead of owned by the engine); seven public `run*` entry points w/ "legacy"/"source-compatible" comments in a zero-back-compat repo; engine 2.4k lines / one 700-line impl; demo driver 1.35k lines; bearer secrets + cookie sessions + login UI invented where PRD says tailscale identity; web is a react stub w/ hand-written protocol types.
- **correction wave (next; small tree, same node pattern; server/auth/web FROZEN)**: root ⇓ `core` ⇓ ≤2 leaves at a time. four items, in this order because all touch the loop: (a) collapse the engine to ONE run entry (durable head + new items + mailbox); delete the rest; no compat shims. (b) tools that run as jobs carry `async: true`; prove item 2 LIVE (slow `sleep`, model continues, wait_agent resumed by the result) and record the request bodies. (c) settings items: `configuration_update` as a positional item in the store, `set_effort` verb, fork strip list, effort pinned via the item not the request field; prove item 13 LIVE (child history = parent prefix + one update) and record cache counters. (d) `Compactor` per PRD shape (`CompactContext`, `Summary`, `NewWindow{carried}`) w/ `Server` only; run the unanswered-call experiment → findings.md. plus one findings-only probe, any leaf, no code: two consecutive requests shaped byte-for-byte like codex's (headers, `prompt_cache_key`, body order) vs ours; diff; report whether cached tokens appear. also: provider trait toward PRD shape (typed `CallContext` w/ cancel token, bounded progress, `JobVerbs`) is allowed in this wave if (a)–(c) need it; hooks are NOT (wave1). interview answers are a deliverable this time: `docs/interviews.md`, one section per node.
- **wave1 (broad)**: everything below, forked from the correction wave's integrate commit: compaction `Structured`+`Select`, server (ws+live+forms), web (principles + 5 screens), remaining acceptance items.
- **wave2 (standalone, operator-drivable; still no tidepool)**: the harness stays pluggable and the demo provider becomes something the operator can actually run a small coding task in, alone, from the web page. (a) auth: device-code login + refresh behind `Auth`; the run stops reading codex's `auth.json`. the retired `~/dev/tidepool/exomonad/harness/src/provider/oauth.rs` is a working reference (read-only, same rule as codex: read for pitfalls, close, write own). (b) demo coding tools, UNSANDBOXED for now: `run` with cwd = the subtree's worktree, wall timeout, output cap (exists); `read{path}`; `edit` stays owned-confined. this runs on the operator's own machine with trusted models; sandboxing (rlimits, network off) is deferred, not forgotten, and goes in the provider when it comes. drop the dev-only gate on `run`. (c) `--tree` is the default; the operator drives root from the page; `ask` works end to end. acceptance = the operator runs one real multi-file task in a scratch repo without codex installed and without touching tidepool. interrupt_agent stays TODO.
- **after wave2**: tidepool adapter (`~/dev/tidepool/plans/harness-adoption.md`), only once the operator has driven wave2 for a few runs. nothing in these waves imports tidepool.
- efforts: agents INSIDE the harness under test (demo children) = luna low (they exist to exercise the loop). implementers: root luna medium; leads/leaves medium, raised to high on fork for leaves w/ hard modules (transport stream, loop, store queries) — spawn at another effort is one configuration_update, use it.

## goal
build ∀ of PRD wave1: core crate, server (ws+live+forms+demo), web. broad on purpose: test of running a tree of trees as much as of the harness. record what the structure cost you.

## what you are building, and for whom
- this harness is your next home. you run today inside a forked codex (blocking tools, watch-and-wake for children, no effort changes mid-thread). the crate in this repo is what you (and agents like you) will run inside next: its `wait_agent` is how you will pause, its `spawn_agent` is how you will delegate, its compaction is how you will survive a long task, its event stream is how your operator will see you think. build it as the tool you want to be held by.
- concrete instance from wave0: the root registered a watch on a child, got no progress, polled `ResponsePending`, learned nothing, and fell back to reading git. in this harness that whole pattern is one async tool call: `spawn_agent` returns a handle, the parent keeps working or calls `wait_agent`, and the wait returns exactly when the child's FINAL_ANSWER (or a progress envelope) arrives. no watch, no poll, no wake protocol. when you build the loop, that is the behavior you are building for yourself.
- it is a separate repo so that YOU can build it: no tidepool/exomonad dependency ⇒ `cargo check -p <crate>` and focused tests finish in seconds ⇒ many small scaffold/check/commit cycles per hour. keep it that way ! (a dep on tidepool, a workspace-wide build, a slow test = a slower you.)
- PRD rules carry their reasons. when the letter of a rule and its reason disagree in the code you are writing, the reason wins: implement to the reason, then `amend(<label>)` the rule and say why. never silently follow a rule you can see defeats its own reason. use the reasons to choose implementation strategy (e.g. "deltas never in the store" exists so the writer is never on the hot path ⇒ design the event lane first, the store second).
- later, exomonad consumes this crate through an adapter (`~/dev/tidepool/plans/harness-adoption.md`, read-only for you). nothing you do here changes tidepool. if you find yourself wanting something from tidepool, that is a contract question ⇒ ask, don't import.

## invariant: same pattern at every node
```
scaffold → commit → ⇓ children (inherit ctx) → children repeat or implement → integrate → review(1 cycle) → report
```
1. **scaffold commit** ! before any ⇓. contains: shared types, trait sigs, module boundaries, stub bodies `todo!()` + doc comment stating what body must do + which PRD rule binds it, short spec `docs/<subtree>.md`. every module a child will own already exists as a stub WITH its `mod` line in the parent file, so no child ever needs to edit an unowned file to compile (wave3: two children did 8 and 21 backup-and-restore cycles on `main.rs` for one `mod` line). MUST compile (`cargo check -p <crate>` / `npm run typecheck`) !
   commit msg: `scaffold(<label>): <one line>` + body listing modules ⇓ and owner label each.
2. **⇓ spawn** one child per module of the scaffold: Exomonad `coding currentCheckout` with a typed Response (inherited ctx; fresh context only for cheap questions). Checkout: LEADS and LEAVES receive isolated bound worktrees from the parent's scaffold commit in the current Exomonad runtime. This differs from the future harness's shared-leaf checkout because the runtime controls allocation; the reason—prevent overlapping edits—still holds. `task.owned` = the leaf's module paths, `task.mustNot` = contract files + sibling modules; parent reviews changed paths and refuses out-of-scope work. The future harness must implement admission-time veto for its shared leaves. Label per table below !
3. **repeat**: child whose module divides ⇒ own scaffold commit → ⇓. leaf ⇒ implements vs inherited stubs.
4. **integrate**: parent merges children in dep order → tree compiles → focused checks pass → commit `integrate(<label>): <children merged>`; body lists every contract amendment made during the wave. integrate means MERGE the child's branch or send it back ! never extract owned files from a stale candidate: if the child's base is old, the child rebases and re-replies. (wave0–3: 22 of 36 branches left unmerged, zero `integrate` commits, files copied in from older bases ⇒ seven engine entry points that nobody's review saw as one seam.)
5. **review**: one cycle. parent reads each child's result vs scaffold. contract defect found ⇒ ⇑, never sideways. first question, before any bug: does this add a second way to do something that already exists (entry point, channel, table, helper)? PRD `structured !` and `library vs driver` are read before correctness.

shared ctx travels ONLY via: (a) scaffold commits in git, (b) inherited ctx window ! sibling↔sibling boundary knowledge forbidden !
need shared type scaffold lacks ⇒ both children ⇑ → parent amends scaffold in NEW commit `amend(<label>): <what>` → tells every child → each merges. never edit a sibling's file. never edit contract files from a leaf !

## labels (select worktree branch AND watchdog nudge set; see nudges.md)
label = last segment of the agent path = the `task_name` given to `spawn_agent`. a child path is always `/parent/task_name`, ONE new segment, exactly as built in wave0. the `core-` prefix in `core-store` is a naming convention so nudge sets and branch names stay unique across the tree; it is not path nesting and the harness does not parse it. model-facing path uses the trained grammar (lowercase, digits, `_`): `/root/core/core_store`. kebab form below is the branch/ledger name; harness canonicalizes.
```
root                 /root
├─ core            crates/harness
│  ├─ core-transport   src/transport/, Auth, ResponsesClient, stream, retries, cache key
│  ├─ core-store       src/store/, schema.sql, writer task, read pool, queries seen_by/pending_at/usage_subtree
│  ├─ core-loop        src/turn.rs, dispatch, pending, late-settle→new request, effort pin, wait_agent, agent verbs (spawn/send/followup/interrupt/list), envelope, claims
│  └─ core-compaction  src/compaction.rs: `Compactor` trait, `Server` + `Select` strategies, invariants; demo supplies `Structured`; experiment → docs/findings.md
├─ server          crates/harness-server, crates/harness-demo
│  ├─ server-ws        axum ws, event stream, command channel, static
│  ├─ server-live      live query subscriptions via writer table versions
│  ├─ server-forms     form job, schema+uiSchema derivation, settle→Output
│  └─ server-demo      DemoProvider: run (freeform async, cell-shaped, spawns from inside a job), edit (owned-confined), sleep, ask (form to /operator); Structured handoff; bin
└─ web             web/
   ├─ web-principles  docs/principles.md (from docs/web.md), tokens (oklch ramps, type scale, meaning colors), route+tab model, keyboard map, protocol client + store, generated TS protocol types, fixture loader + ladle/playwright harness  (FIRST; others fork after it merges)
   ├─ web-tree        commit-graph ⊕ trace-view
   ├─ web-timeline    trace-view ⊕ commit-log
   ├─ web-window      notebook ⊕ chat
   ├─ web-inbox       email ⊕ issue-tracker
   └─ web-palette     launcher ⊕ REPL
```
contract files (leaf edit ⇒ ⇑, never direct): `crates/harness/src/{item,model,provider,hooks,agents,mailbox,compaction/trait,protocol}.rs`, `crates/harness/src/store/schema.sql`, `web/src/protocol.ts`(generated), `crates/harness/src/text/` (model-facing text), `docs/principles.md` after it merges. ∀leaf `task.mustNot` lists these.

## root (level 0)
- root scaffolds, spawns, integrates, reviews. root does not implement ! (wave3 root wrote code, reviewed it, and integrated it, and spent 43% of its active time polling; a root that codes is a root that is not watching the tree.) the one exception: a stub or `mod` line the scaffold missed, as an `amend(root)` commit.
- read PRD.md, this file, nudges.md, model-facing.md (the cached-prefix text: copy its VERBATIM blocks into `crates/harness/src/text/` w/ the bytes test; edit only `OURS` lines, w/ a reason). no draft stubs exist; root writes scaffold commit from PRD `provider trait`/`items`/`store`/`loop` sections: workspace `Cargo.toml`, crates `harness`/`harness-server`/`harness-demo`, `web/package.json`+`src/main.tsx` stub, `schema/` (protocol json-schema gen step documented not run), one test per crate (protocol version round-trip) so leaves extend an existing harness.
- order ! core's scaffold is what server+web build against ⇒ either (a) merge `core` scaffold commit before ⇓ server/web, or (b) ⇓ server/web from core's scaffold commit directly. choose, record why in scaffold commit body.
- ⇓ core, server, web. inherited ctx.

## leads (level 1)
- core: scaffold module boundaries inside crate (trait `Auth`, `Store` api, `Session`/`run_turn` sigs, `compact` sig) → ⇓ 4 leaves.
- server: scaffold server handlers + `DemoProvider` skeleton + form job type → ⇓ 4 leaves. server-demo may start first (others need it for acceptance).
- web: ⇓ web-principles alone → merge → ⇓ 5 screen leaves from that merge. ∀screen leaf first commit names its pattern pair ! evidence = screenshots vs recorded event fixture.

## leaves
- implement vs stubs. own module only (`task.owned`). `cargo check -p <crate>`, `cargo test -p <crate> <name>` only ! no workspace-wide builds !
- commit by pathspec only ! `git commit -F msg -- <paths>` (the future harness's shared leaves share an index; pathspec discipline remains useful in today's isolated Exomonad worktrees, never `-a`/`-A`).
- no `todo!()` in delivered module ! unfinished ⇒ say exactly what is left, in report.
- codex: reference only ! read for pitfalls, close, write own.
- quality bar (PRD) applies to every commit ! newtypes, closed enums, typed errors, no unwrap in lib, no blocking in async, bounded channels, generated TS types, strict TS. parent review reads for these first.
- reply = `Outcome`: {commits, checks run (names+result), left undone, contract amendments requested, interview answers}.

## ask (∀node)
stop and ask the operator when: contract ambiguous and both readings ⇒ different code | API ≠ docs and the choice is not yours | scope cut needed | anything you'd otherwise guess at a boundary.
shape ! one question per stop: `[<label>] <question>` + `default: <what you do if no answer>` + `blocks: <what waits>`. continue non-blocked work while waiting. never a question whose answer is in PRD/tree.md/scaffold.
log ∀ asked question + answer in `docs/questions.md` (append, by pathspec) — the shape of these is a deliverable of this wave.

## root integration = acceptance
demo provider through server + page (a REAL tree doing a real multi-file task in a scratch repo, recorded), each item checked in page + store query + transcript:
1. fast tool reads sync
2. slow tool, model continues, then wait_agent → resumed by the result
3. model stops w/ call pending → result starts new request
4. fork at different effort → request shows cached prefix tokens
5. cancel → typed Cancelled
6. form raised by tool → answered in page → job settles
7. compaction w/ one job pending (+ findings.md)
8. text streams into the page before the response completes; a job starts before the response completes
9. kill -9 mid-request w/ one job pending → restart → conversation shows `Interrupted` request, job resumed or `Interrupted`, no duplicate rows, page reconnects via snapshot+seq
10. replay: rebuild the request body for a stored request → byte-identical to what was sent
11. slow async tool + `spawn_agent{from: here}` in the SAME turn → both resume the parent through wait_agent; child's FINAL_ANSWER arrives as envelope, tool as function output; child received the parent's pending call output via claim
12. compaction: `Structured` (demo forced handoff) w/ one pending call + one live child → new window has no configuration_update, fresh pin, carried call verbatim, opening developer item, user messages retained; then `Server` once (findings.md records whether the unanswered call survived)
13. spawn at another effort → child's history = parent prefix + one configuration_update, parent's updates stripped; request records cache counters. Verify stable history/prefix and report a zero cache counter as unobserved, not failure: live subscription probes returned zero writes and hits despite a stable >1,024-token prefix, so the invariant under harness control is preserved prefix identity, not a backend cache hit.
14. veto: leaf edits a path outside `task.owned` → typed Veto output, event logged, parent notified
15. operator as path: user input arrives as a MESSAGE envelope from `/operator`; a form = `followup_task` to `/operator`, answered in the page ⇒ operator's FINAL_ANSWER settles it; inbox shows it while pending
16. delivery classes: a job's progress envelope (Hold) is not delivered until wait_agent; a sibling's `send_message` (AtBoundary) arrives at the next request after all function outputs; order verified in the recorded request body
17. cold prefix: `spawn_agent from: checkpoint` on a prefix older than the ttl (clock paused in test) → `Refused{prefix_cold, ...}`; repeat w/ `accept_cost: true` → request sent; inbox shows the same refusal shape to the operator
18. `checkpoint{name}` then `compact{keep_since: name}` → items after the checkpoint verbatim in the new window, summary covers only what preceded it; the request shows an explicit cache breakpoint at the checkpoint
19. typed reply: child contract carries `reply` schema → child's FINAL_ANSWER is a `finalize` call; parent's envelope payload = the rendered record; store has the typed record

## interview (∀node incl. leaves, in final reply)
- which scaffold item changed under you; who; how did you learn.
- what you needed from a sibling that scaffold didn't give.
- where API ≠ its docs.
- what integration cost your parent that a different split avoids.
- what you'd scaffold differently rerunning your subtree.
- which nudges fired on you; right or wrong. (ledger: `.exomonad/nudges/<label>.jsonl` is a deliverable of the wave; the interview reads it.)
