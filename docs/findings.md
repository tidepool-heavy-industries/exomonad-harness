# Wave 0 API findings

## Pre-flight status (2026-09-23)

The two required live Responses requests have **not** run. `OPENAI_API_KEY` is
not present in this process environment, and the local Codex authentication
file has no OpenAI API key. Codex session credentials are not substituted for
an API key. Consequently, there is no observed HTTP status, assistant `phase`,
sender response, or cached-token count yet. No scaffold was created.

The request/response fields in the PRD remain hypotheses until the live
pre-flight records the request bodies (with credentials redacted), HTTP status,
response item types and phases, assistant text, and usage for each call.
