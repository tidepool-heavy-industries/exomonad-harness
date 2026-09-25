# Cache-counter probe evidence — 2026-09-24

## Outcome: second request reported cached input tokens

Ran exactly two consecutive live requests through this checkout's production
`ResponsesClient::create` / `client::execute` builder, sequentially. Both used
model `gpt-6-sol`, low effort, no tools, identical instructions and identical
user-prefix input (a stable repeated text prefix; each response reported
20,923 input tokens), and the same `prompt_cache_key` value
`cache-counter-probe-20260924`. The prefix is well above 1,024 tokens. No
credential, authorization header, or request body was printed or recorded.
The read-only `CodexFileAuth` source was `~/.codex/auth.json`.

Redacted usage blocks, as returned by the transport:

```json
{"input_tokens":20923,"output_tokens":5,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0}}
```

```json
{"input_tokens":20923,"output_tokens":5,"input_tokens_details":{"cached_tokens":20736,"cache_write_tokens":0}}
```

Conclusion: **yes** — the second request's `cached_tokens` was greater than
zero (20,736). Per Q4, no field/header-order comparison against the retired
Codex builder was needed or performed. This is a measurement only, not product
approval or a general cache-behavior guarantee.

Probe source revision: `d0245b3177afa21556c041ce05296a96b50b209a`.
No repository code was changed and no broad live tests were run. The temporary
runner compiled against the harness crate and executed these two requests.
