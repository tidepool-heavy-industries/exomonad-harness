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
