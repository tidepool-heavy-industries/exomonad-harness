# NEXT: finish the correction wave

You are the root. This file is where the last run stopped and where you start.
Read it, then `README.md` for the reading order, then `docs/correction-plan.md`
for the status table and `docs/tree.md` for the correction-wave bullet and the
node protocol. The `TODO(correction-wave …)` and `FIXME(correction-wave …)`
comments in `crates/` are the map into the code; each names the PRD rule it
serves.

## Current run checkpoint (2026-09-24)

- (b) provider tool schemas integrated on master at `d8097c3`.
  `cargo test -p harness --offline --test correction_wave` ran 1/1 passing
  test; `cargo fmt --all --check` passed. The live item-2 trace is **open**.
- (c) and (d) remain open on the core lead's branch. Core reported its inbox
  fenced and is continuing inline; a notification receipt must not be treated
  as proof it resumed or incorporated a change.
- The cache probe's `Blocked` evidence was merged at `b99337e`. It sent no
  live requests; an exact redacted Codex wire capture is missing. Q4 is
  pending in `docs/questions.md`.
- All four items are **not** complete. The remainder of this file records
  the previous run's handoff and still-open acceptance criteria.

## Where the last run stopped (2026-09-24)

- (a) one engine entry: done and integrated (`9da6efe`, `a9f7a12`).
- (b) async tools: a red offline test on master, `crates/harness/tests/correction_wave.rs`.
  It is the contract for this slice, not a failure. The fix is one place:
  `Provider::all_tools` stamps `"async": true` on every tool except `wait_agent`
  (TODO at that function). Then the live item-2 trace.
- (c) settings items and (d) `Compactor`: not started. Annotations sit at
  `engine.rs` (effort as a request field), `agent_runtime.rs` (`here` fork strip
  list, checkpoints as a blob), `compaction.rs` (target trait shape inline).
- Cache probe: rescoped to one node, no children, two live requests, redacted
  bodies and counters in `docs/findings.md`, or `Blocked` naming the missing capture.
- Interviews: root only. Every node that runs adds a section to `docs/interviews.md`.
- `cargo fmt --all --check` fails in `engine.rs` test code and the new test file,
  from the last run's commits. Format those two files first, as their own commit.

## Rules that changed since the run started

- A red OFFLINE test on master between slices is fine. Never automate a test
  that spends inference: live tests stay `#[ignore]`, run by hand once, trace in
  findings (PRD `inference spend !`).
- Integrate means merge the child's branch or send it back; never extract files
  from a stale candidate. Scaffold owns every `mod` line. Review reads structure
  before bugs. Root implements only scaffold and seams; every bounded leaf is a
  child (`docs/tree.md`).
- Server, auth and web are frozen this wave.

## Done means

All four items integrated on master with `integrate(<label>)` commits, the
item-2 and item-13 live traces and the unanswered-call finding in
`docs/findings.md`, the probe's bodies or its `Blocked`, one interview section
per node, and `docs/exomonad-friction.md` extended with this run's notes. Then
rewrite this file for wave 1: what landed, what is open, where to start.

## Prompt trials this run (project-level, promote to core only if they hold)

Rules the last run's friction notes argued for but the engine does not yet
enforce. Follow them here; the interview at the end says whether each held.

- A reply that incorporates a sibling's or parent's change names the exact
  commit that did it; "applied" without a commit is not incorporation.
- A test committed red on purpose names its owner and the slice that turns
  it green in its commit message; integration status lists it as expected
  red, never as passing.
- After two failed check rounds with no candidate, the child pings its owner
  before a third; the owner answers with a split, a seam, or a stop.
- An operator note is advice unless it says it is a constraint; a hypothesis
  is labeled as one in every packet that repeats it.
