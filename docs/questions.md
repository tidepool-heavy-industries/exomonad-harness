# Operator questions

## Q1 — Wave 0 pre-flight credential (2026-09-23)

**Boundary:** The contract requires two live GPT-6 Responses calls *before*
scaffolding. This workspace has no `OPENAI_API_KEY`, and the Codex login is not
an API key.

**Options:** (A) Configure `OPENAI_API_KEY` in the environment and continue the
pre-flight; (B) stop and report the blocker without scaffolding.

**Asked:** Via operator input. **Answer:** Neither option. Use the existing
ChatGPT-subscription login in `~/.codex/auth.json` read-only, with
`tokens.access_token` and `tokens.account_id`, against the streaming Codex
Responses endpoint. Never write, copy, refresh, print, log, commit, or store
the token. On 401, stop and ask rather than refresh. The operator supplied
the endpoint, required headers/body, and the already-tested implementation at
`~/dev/tidepool/exomonad/harness/src/provider/oauth.rs` as a reference.
This changes PRD auth order; subscription comes first because it is the
credential available now.

## Q2 — Cache-hit acceptance (2026-09-23)

**Asked:** `[root] Wave 0 item 13 requires positive cached-prefix tokens, but
four live subscription calls reported zero cache-write and cached tokens even
with a stable >1,024-token prefix and session key. Should acceptance require
a measured hit, or verify stable prefix/effort history and report cache as
unobserved?`

**Default:** Verify history and report zero. **Blocks:** Final acceptance
verdict for item 13. **Answer:** Verify history and report cache unobserved;
a positive hit is not required.

## Tooling friction (not operator questions)

- Bash calls can be slow while a wide tree contends on the shared machine
  checkout and misses the compile cache. An earlier output-size diagnosis
  was incorrect; the failing classification step was fixed in the engine.
  Bound output for readability, batch independent commands and prefer fewer,
  larger children for the remainder of this wave.
- The native `spawn_agent` was offered but its child could not use Bash,
  Haskell, or status (`hosted tool call is not authorized`); Exomonad
  `spawnWatched` was the working delegation path.
- On this subscription SSE endpoint, `response.completed.response.output`
  was empty while `response.output_item.done` carried the assistant message;
  a completion-only parser would silently lose the answer.
- A batched Haskell `lookup` for agent-list APIs failed with a compiler-worker
  diagnostic rather than returning per-query results; individual lookup or
  `observeAgent` remains available.
- The registered response watch has no incremental progress unless the child
  publishes a progress stream; repeatedly polling `ResponsePending` added no
  information, so root switched to reviewing committed Git evidence.
- Today's Exomonad coding children receive isolated bound worktrees, not the
  shared leaf checkout described for the future harness. Root amended
  tree.md to preserve the ownership reason through contracts and path review;
  integration must merge leaf commits instead of relying on a shared index.
- The `afterTool` watchdog hook is not installed for this running actor, so
  `.exomonad/nudges/<label>.jsonl` cannot be generated from actual hook
  judgments here. Do not fabricate nudge events; report the absence in the
  wave-0 interviews.

## Q3 — Child path form (asked in findings, answered 2026-09-24)

**Asked:** The verb schema uses single-segment child paths despite the PRD's
`/root/core/core-store` example; which is the contract?

**Answer:** Single segment, as built. A child path is `/parent/task_name`.
The `core-` prefix in a label like `core-store` is a naming convention for
unique nudge sets and branch names; it is not nesting and the harness never
parses it. `docs/tree.md` labels section now says so.

## Q4 — Cache probe reference capture (asked 2026-09-24)

**Asked:** Can the operator provide an appropriately redacted Codex request
capture with ordered headers and serialized body bytes for the two-request
byte-for-byte cache probe, or should that probe remain blocked?

**Recommendation:** Keep the probe `Blocked` and defer it rather than infer
wire bytes from prose or spend inference on a comparison without a reference.
No live requests were sent. Evidence: `docs/cache-probe-evidence.md` on the
probe branch; root will incorporate it after an owned-path-clean revision.
**Answer (operator, 2026-09-24):** No capture exists and none is required.
Byte-for-byte parity was never the question; the question is whether
`cached_tokens` reads above zero on the second of two identical-prefix
requests through our own builder. Run that, record both redacted `usage`
blocks. Only if the answer is no, diff our headers and body field order
against Codex's request builder in source (the retired
`~/dev/tidepool/exomonad/harness/src/provider/` tree and the vendored Codex
client), as a field list, and record the diff. The probe is unblocked and
re-scoped in `docs/tree.md`.

## Q5 — Resume authority after fenced core inbox (asked 2026-09-24)

**Asked:** With the no-more-forks hold still in force and core `Blocked` on
the structural (c)/(d) seam, may a fresh core lead be admitted in the next
run after notification routing is preflighted, or should the wave remain
on hold?

**Recommendation:** Authorize a fresh routable lead after the preflight;
scaffold harness-authored settings provenance and the initial-pin/fork
contract before bounded implementation. Do not lift the current-run
no-more-forks constraint by inference. **Answer (operator, 2026-09-24):**
Yes. The hold was a constraint on that run only and is lifted. Admit a
fresh core lead; preflight that it receives a message before giving it
work. One correction to the recommendation: there is no provenance seam to
scaffold first. PRD `settings items` decides it (harness-authored only,
forged ones dropped) and the annotation at `Store::append_items` states the
rule in one sentence. (c) starts as implementation, not design.

[item2-live-retry-2026-09-25] The single authorized manual item-2 run on a42920f made a first successful response and a pending-call second request, but the second request returned HTTP 400; redacted evidence is docs/item2-live-attempt.jsonl and diagnosis is in docs/findings.md. After an offline-reviewed repair of TreeProvider's missing async flags, may root spend exactly one additional credentialed manual item-2 run? default: do not retry; leave live item-2 acceptance open. blocks: live item-2 continuation/wait_agent acceptance, not offline c/d work.
