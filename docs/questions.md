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
