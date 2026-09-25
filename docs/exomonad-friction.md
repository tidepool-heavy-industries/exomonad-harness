# Exomonad friction — correction wave

Version-controlled field notes, not an engine bug tracker. “Fixed” below means
the local prompt or workflow changed; it does not imply an engine fix. Earlier
detail remains in Git history. Do not fabricate watchdog or nudge events.

## Root: still open

| Observation and cost | Improvement to test |
| --- | --- |
| The project `AgentSpec` required `Journal`, which coding children lack. Two children failed only after admission. | Preflight the selected spec against every launchable child role's actual effect row, with a diagnostic naming role and missing effect. Do not grant the effect silently. |
| A completed leaf commit was visible in Git before its parent published a candidate. Commit, typed delivery, review, merge and verified integration were easy to confuse. | Show these as separate source-bound states; let a parent ask for the exact candidate rather than infer acceptance from a branch head. |
| A probe's latest commit touched only owned paths, but an earlier commit in its branch still changed `docs/findings.md`. Root caught this by comparing the *cumulative* diff, sent it back, and merged only the repaired branch (`cdbbd367` via `b99337e`). | Make the cumulative base-to-candidate owned-path gate first-class. A clean tip commit is not sufficient. |
| The cache probe had no exact Codex wire capture, so it sent no live requests and returned a documented blocker. It had initially edited the shared findings file before root corrected ownership. | Preflight reference evidence and shared-file ownership before inference spend. Preserve `Blocked` as a real outcome, not a passing probe. |
| `sendMessage` to the core lead returned a delivery receipt, but the operator observed no active tmux work and root saw no progress or reply. The runtime showed the request as `Working` while its workbench was idle. Core later reported its inbox was fenced and it could not receive further host notifications this run. The mechanism remains **undiagnosed**; a receipt proved neither presentation nor action. | Trace notification receipt → inbox fence → actor wake → provider turn → progress/reply. Surface a “stalled despite pending request” state and an explicit recovery action. |
| The root checked a router snapshot on status questions and found only `pending`. | Keep event-driven continuations: notify once on actionable change, and distinguish a meaningful wait from polling or an idle actor. |
| A message asked core to incorporate probe evidence into `docs/findings.md`. Delivery alone does not show that it did so. | Require an incorporation receipt tied to the exact commit and focused check. This run's prompt trial uses that rule manually. |
| The cache blocker required an operator question. The answer may never arrive. | Keep the recommendation and unanswered question in `docs/questions.md`; allow reversible work to proceed while the irreversible live comparison remains blocked. |

## Local workflow corrections already applied

- **Planning and release:** Root authored the correction plan; Q1–Q3 and the
  correction scope were already settled, with no separate planner hold.
  Do not delegate routine seam design or invent another release gate.
- **Review and integration:** Review only source candidates at their exact
  commit. A findings-only probe is read, not reviewed. Merge the child's
  branch, never copy its files; send stale or out-of-scope candidates back.
- **Bounded children:** Task packets now say to stop/ping on an ambiguous seam
  or two failed check rounds without a candidate. This is a prompt trial,
  not an engine-enforced limit.
- **Checks:** Use `&&` for gating compound commands; distinguish tests run,
  tests compiled but not run, zero matches and ignored live tests. The
  intentionally red offline async-schema test names (b) as its owner.
- **Operator notes:** The earlier Bash output-size explanation was a
  hypothesis and was retracted; shared-checkout contention and a compile
  cache miss better explained latency. Packets no longer promote a
  hypothesis to a constraint.
- **Tool guidance:** The current fork skill gives single-label
  `batch`/`subgroup` examples, and the root review recipe now uses an
  absolute group path. These reduce prompt-discovery friction; whether
  diagnostics themselves improved is unverified.

## Remaining design experiments

1. **Event continuity:** Compose child settlement, progress, mailbox and
   command completion as typed events; retain the pending claim across
   compaction and restart. Trial: slow tool plus child, with one wake for
   the first actionable event and later delivery of the other result.
2. **Delegation preflight:** Check owned modules and `mod` lines, consumer
   interfaces, effect rows, checked source, runnable acceptance and
   stop/ping condition before forking. Reproduce the `Journal` startup
   failure as a preflight refusal.
3. **Behavioral replay:** Compare old and candidate scheduling, refusals,
   messages and costs against the same addressed events. Use it to test a
   bounded-evidence watchdog selector; report abstentions, not invented
   nudges.
4. **Capability and evidence view:** Show runtime authority, live typed
   bindings, source revision, and what would be lost at fork/compaction.
   Tie observed, inferred, proposed and unverified claims to evidence.
5. **Measured promotion:** Promote a repeated notebook routine to a
   workspace helper or hook only after comparing rounds saved, added
   latency and error rate on later candidates.

## Node notes

- **Core lead:** Own-words plan received: (b), then (c), then (d), with
  focused checks and manual live traces. At its later checkpoint, (b) had
  a committed leaf but no review/merge, and (c)/(d) had not started. Core
  reported an inbox fence, continued inline without further forks, delivered
  (b) and a findings note, then returned `Blocked` on the structural (c)/(d)
  seam. Its own-words account (retained source `6667ddc`) says child replies
  were still readable through typed `pollResponse`, while host
  notifications were not. TUI relay restored facts manually but not normal
  routing. An explicit fence drain/replay is a candidate recovery mechanism,
  not something tested here. Its interview is in `docs/interviews.md`.
- **Cache-probe leaf:** Its own account is in `docs/interviews.md`. It
  found no byte-for-byte Codex reference capture, made no live calls, and
  repaired an out-of-scope branch ancestor before integration. The blocker
  is in `docs/cache-probe-evidence.md`.
- **Other leaves:** Interviews remain due from nodes that ran. Do not infer
  their experience from root observations.
