# Wave 0 API findings

## Pre-flight (2026-09-23)

Credential: Codex-owned ChatGPT subscription login, read only. No token value,
copy, or refresh was stored. Endpoint:
`POST https://chatgpt.com/backend-api/codex/responses`; headers included
`version: 0.155.1`, `originator: codex_cli_rs`, `Accept: text/event-stream`,
`chatgpt-account-id`, and a stable `session-id`. Bodies set `stream: true`,
`store: false`, `model: gpt-6-sol`, a stable `prompt_cache_key` equal to that
session id, stable instructions (about 1,100 input tokens), one strict
function tool, and request-level `reasoning.effort: low`.

The initial two calls returned **HTTP 200** and `response.completed`. The
first asked for a brief reply with a function tool present. The second kept
the same prefix and added an assistant-role agent envelope, a
`configuration_update` raising effort to medium, and a user request to
address the sender. Both reported `usage.input_tokens_details.cached_tokens:
0` and `cache_write_tokens: 0` (first input tokens 1,187; second 1,219).
Our initial SSE reader inspected only `response.completed.response.output`,
which was empty; that **does not establish absence of an assistant message**.

Two diagnostic calls corrected the SSE observation. The backend emitted the
assistant message in `response.output_item.done`, with
`role: assistant`, `phase: final_answer`. The first diagnostic returned
`PREFLIGHT_INSPECT_OK`. The second, with the agent envelope and positional
effort update, returned `Acknowledged, Peer.` Both returned HTTP 200; the
second again reported zero cached and cache-write tokens (1,205 input).
Thus assistant `phase` is present, the assistant-role envelope and
`configuration_update` are accepted, and the model addressed the sender.
Cache reuse **was not observed**, despite an unchanged >1,024-token prefix
and stable session/key. No 401 occurred.

### Consequence

The SSE parser must build output items from `response.output_item.done`;
`response.completed` supplies status/usage but its `output` may be empty on
this endpoint. Do not gate correctness on a positive cache counter. Retain
cache affinity and record its actual counters; investigate cache controls
separately without claiming a hit.

## Responses transport slice (2026-09-23)

Integrated `d3a0598` on `master`. The transport reads Codex-owned subscription
credentials without refreshing or storing them, sends stateless streaming
requests, and assembles output from `response.output_item.done`; completion
provides the response ID and usage. A 401 returns an authentication error.

Before integration, `cargo fmt -p harness -- --check`, `cargo test -p harness
--offline` (6 passed, 1 live test ignored), `cargo clippy -p harness
--all-targets --offline -- -D warnings`, and `cargo check -p harness --offline`
passed against the submitted commit. After integration, crate fmt, tests, and
Clippy passed again; the explicit live subscription smoke test passed and
confirmed a final-answer item plus response ID. Workspace-wide `cargo fmt
--all -- --check` is not green because of pre-existing formatting in
`crates/harness-demo/src/main.rs`, outside this slice. No cache hit was claimed
from the live smoke test. Store, agent tree, server, and web remain deferred.
