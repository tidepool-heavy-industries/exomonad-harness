# NEXT: finish the correction wave, second half

You are the root. This file is where the last run stopped and where you start.
Read it, then `README.md` for the reading order, then `docs/correction-plan.md`
for the status table and `docs/tree.md` for the correction-wave bullet and the
node protocol. The `TODO(correction-wave …)` and `FIXME(correction-wave …)`
comments in `crates/` are the map into the code; each names the PRD rule it
serves. `docs/questions.md` Q4 and Q5 are answered; read them before planning.

## Where the last run stopped (2026-09-24, one hour, two runs into this wave)

- (a) one engine entry: done (`9da6efe`, `a9f7a12`).
- (b) async tools: done on master (`a1f976f` via `d8097c3`). `Provider::all_tools`
  stamps `async: true` on everything but `wait_agent`; the offline test in
  `crates/harness/tests/correction_wave.rs` is green. The LIVE item-2 trace is
  still open: slow `sleep` tool, model continues, `wait_agent` resumed by the
  result, redacted request bodies in `docs/findings.md`. One node, one run,
  by hand.
- (c) settings items: not started. The last lead returned `Blocked` on a
  "provenance seam". That seam is decided, not open: PRD `settings items` says
  harness-authored only; the TODO at `Store::append_items` states the drop
  rule in one sentence. Order inside (c): drop rule at append → `set_effort`
  appends the positional item → `here` fork strips by item type and re-pins
  with one fresh update → request-level effort mirrors the first update in the
  sent history → live item-13 trace with cache counters last.
- (d) `Compactor`: not started. Annotation at `compaction.rs` has the target
  trait shape. `Server` only; unanswered-call experiment → findings.
- Cache probe: unblocked and rescoped (Q4). Two requests through our builder,
  same `prompt_cache_key`, record both redacted `usage` blocks. The question is
  whether `cached_tokens` > 0 on the second, yes or no. No wire capture is
  needed; field-diff against Codex's builder in source only if the answer is no.
- Interviews: root, probe, core sections exist in `docs/interviews.md`. Every
  node that runs this time adds its own.

When a slice lands, update its bullet here in the same commit, so this section
never shows finished work as open.

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

## Rules in force

- Never automate a test that spends inference. Live tests stay `#[ignore]`,
  run by hand once, trace in findings (PRD `inference spend !`). A red offline
  test on master between slices is fine and names its owner.
- Integrate = merge the child's branch at its exact commit, or send it back.
  Never extract files from a stale candidate. Scaffold owns every `mod` line.
  Review reads structure before bugs. Root implements only scaffold and seams;
  every bounded leaf is a child. Check the cumulative base-to-candidate diff
  for owned paths (`git diff <base>...<tip> --stat`), not the tip commit; an
  ancestor can carry an unowned edit.
- Incorporation names the exact commit. "Applied" without a commit is nothing.
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
  per child settlement.

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
