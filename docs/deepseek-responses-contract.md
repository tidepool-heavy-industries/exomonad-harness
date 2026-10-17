# DeepSeek Responses transport contract

Reviewed 2026-10-17 against the official Responses, thinking-mode and tool-call
guides:

- https://api-docs.deepseek.com/guides/responses_api/
- https://api-docs.deepseek.com/guides/thinking_mode/
- https://api-docs.deepseek.com/guides/tool_calls/

The DeepSeek route selects https://api.deepseek.com/responses with JSON bearer
requests, flat function tools, plaintext reasoning and stateless full history.
It does not inherit the ChatGPT route's namespace wrapper, zstd compression,
private headers, encrypted-reasoning include, cache comparison or cache key.
The endpoint is the official URL by default. An explicitly selected canonical
numeric-loopback HTTP endpoint with a nonzero port and exact /responses path
is also admitted for a local provider; it is retained and matched exactly.
DNS names, URL credentials, query/fragment, noncanonical addresses and other
remote endpoints are refused. ChatGPT keeps its original fixed endpoint.
Redirects are disabled; no host configuration endpoint override is introduced. Unsupported semantics
are refused locally rather than relying on the service's silent ignores.

The official Responses guide limits custom tools to apply_patch. This initial
profile admits function tools only; notebook adaptation belongs to the host's
pinned advertised surface. Incoming calls must satisfy that same profile before
parsing or dispatch can discard unsupported fields. Developer policy requires
known authored-message evidence before positional system-role projection.
The canonical history is unchanged by wire projection.

Responses reasoning effort supports none, low, high and max. Medium is not
silently mapped to high. The thinking guide's reasoning_content examples use
Chat Completions; Responses retains reasoning item content instead. The tool
schema guide describes strict mode under the beta Chat endpoint, while the
Responses guide lists function support without an explicit strict-mode
promise. Local strict schema validation is retained; offline checks do not
prove the service enforces those schemas.

A run retains its nonsecret route, protocol, endpoint, credential source and
limits. Rehydration revalidates that contract; historical absence remains
unknown, never an inferred DeepSeek selection. A credential reference selects
an operator-managed source, not immutable secret bytes or a proven account:
OAuth refresh and explicit key rotation at the same source remain possible.
The actual DeepSeek principal is unknown without a provider-issued identity.
There is no credential-source fallback. Debug/error output excludes values.

DeepSeek defaults are 65,536 output tokens, 15-second connect timeout,
300-second HTTP timeout and four in-flight requests. Run clients share one
admission handle and semaphore. Queue wait precedes credential access and HTTP
execution; cancellation drops the owned permit and cannot dispatch queued work.
Caller cancellation/deadline covers queue wait; HTTP timeout begins at dispatch.
The existing tool scheduler is independent.
