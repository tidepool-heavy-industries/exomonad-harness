# Visible context trim provider qualification — 2026-10-02

## Current trial: native ChatGPT plan route

The running trial's configuration selects `provider:chatgpt_plan`,
`gpt-6.1-sol`, and Medium effort. Four fresh requests qualified that exact
provider path through the existing `ChatGptPlanAuth`,
`ResponsesRoute::ChatGptPlan`, and production Standard serializer against
`https://api.openai.com/v1/responses`. The existing native registration remained
owned by the authentication service. No credentials, authorization headers, or
Codex-issued reasoning were copied into this run.

The production public serializer advertised tools in the `functions` namespace,
retaining `async:true` on the custom background tool and `async:false` on the
custom editor. A genuine diagnostic call supplied encrypted reasoning. The next
response emitted an asynchronous raw-Haskell custom call (`async:true`), a new
encrypted reasoning item, and a synchronous custom editor (absent `async`).

After the editor, the host replaced only the earlier diagnostic output string
(8,860 bytes to 102 bytes, including the explanatory trim marker and retained
`calibration=47`). Both reasoning items and every native call remained exact
under canonical JSON hashing. The editor's real completion was appended while
the unrelated background call remained unresolved. The next response was
`CONTEXT_OK calibration=47 pending=true`. Appending the eventual checksum result
with the original background call ID then produced
`BACKGROUND_OK checksum=91 calibration=47`.

| ChatGPT plan request | Input tokens | Output tokens | Reasoning tokens | Returned reasoning context |
| --- | ---: | ---: | ---: | --- |
| Diagnostic call | 466 | 153 | 136 | `all_turns` |
| Background and editor calls | 2,487 | 176 | 127 | `all_turns` |
| Edited history and editor completion | 866 | 12 | 0 | `all_turns` |
| Late background completion | 874 | 12 | 0 | `all_turns` |

Requests omitted `reasoning.context`; opt-in completion tracing confirmed the
returned effective mode on every response. All four requests completed; reported
cache and cache-write counters were zero. Prediction-token counters were
unavailable. This independently qualifies the live trial's provider route;
tool execution remained the local diagnostic/checksum fixture, so it does not
replace resident Haskell or browser acceptance.

## Separate Codex endpoint qualification

Earlier real requests used `gpt-6.1-sol` through the owning
`ResponsesClient::create` and production request serializer, against
`https://chatgpt.com/backend-api/codex/responses`. Authentication read the
existing Codex credential file through `CodexFileAuth`; no credentials or
authorization headers were printed or retained. Each request replayed its
complete native history with `store:false`, streaming, and encrypted reasoning
included. No `previous_response_id` or reasoning reset was used.

## Codex Standard curation and pending settlement

The Standard Codex route accepted four requests containing a diagnostic
function call and asynchronous function call, followed by a synchronous editor,
then its completion and a late result for the original pending call. The
diagnostic and editor responses both supplied encrypted reasoning. Replacing
only the earlier diagnostic output string preserved every reasoning item and
native call byte-for-byte under canonical JSON hashing. The subsequent answer
was `CONTEXT_OK calibration=47 pending=true`; the later pending result produced
`BACKGROUND_OK checksum=91 calibration=47`.

A separate Standard run qualified the resident tool shape: a provider-produced
asynchronous **custom** call containing raw Haskell (`async:true`) and a
provider-produced synchronous custom editor (absent `async`, interpreted as
synchronous), each with its own preceding encrypted reasoning. Both appeared
in the same response. The editor was settled before the next inference; the
background call remained without an output. Curation and late original-ID
custom-tool settlement produced the same two expected answers.

| Standard custom request | Input tokens | Output tokens | Reasoning tokens | Returned reasoning context |
| --- | ---: | ---: | ---: | --- |
| Diagnostic call | 386 | 15 | 0 | `all_turns` |
| Background and editor calls | 2,509 | 385 | 334 | `all_turns` |
| Edited history and editor completion | 925 | 12 | 0 | `all_turns` |
| Late background completion | 933 | 12 | 0 | `all_turns` |

Standard requests omitted `reasoning.context`; the table records what the
provider actually returned, observed through opt-in completion tracing. All
reported cache and cache-write counters were zero. Prediction-token counters
were unavailable. These results establish endpoint acceptance and correct
visible answers for this fixture, rather than a general guarantee that edited
facts leave earlier reasoning conclusions valid. Tool execution was a local
diagnostic/checksum fixture; this was not a resident Haskell or browser test.

## Codex Lite capabilities and boundaries

The Codex Lite serializer requests `reasoning.context:all_turns`, sets
`parallel_tool_calls:false`, supplies tools as `additional_tools`, and removes
their `async` declarations. A three-request **settled-history** Lite run accepted
the same visible trim, retained both native reasoning items exactly, and
returned `CONTEXT_OK calibration=47` after the editor completed.

| Lite request | Input tokens | Output tokens | Reasoning tokens | Returned reasoning context |
| --- | ---: | ---: | ---: | --- |
| Diagnostic call | 315 | 162 | 145 | `all_turns` |
| Editor call | 2,297 | 44 | 28 | `all_turns` |
| Edited history and editor completion | 567 | 10 | 0 | `all_turns` |

Replaying a native unresolved call under that Lite profile failed with HTTP 400,
`invalid_request_error`, `param:input`, and `No tool output found for function
call <original ID>`. That raw replay does not prove that production Engine
sends the same shape: Lite calls lacked `async`, so existing admission treated
them as synchronous.

One authorized scratch variation retained the original tool `async` declarations
in Lite `additional_tools`, with every other protocol setting unchanged. It
advertised a custom Haskell async tool, custom synchronous editor, and synchronous
diagnostic function. The endpoint rejected the first request with HTTP 400,
`unsupported_value`, `param:tools`:

> X-OpenAI-Internal-Codex-Responses-Lite does not support async tools because they inject acknowledgments and resume sampling.

That experimental serializer was not committed or made a production default.
No fake acknowledgment, native argument rewrite, or reasoning removal was used
to obtain a successful result. Standard native async behavior was qualified
separately as described above.

## Retained evidence and validation

Transport source baseline: `136f5ca760b6e9f9297ffdc2da8e9a69d48f1f60`.
Completion observability: `e21072554412260932f1309ffdf5c070e6494390`.
Private, bounded request/response JSON, exact item hashes, runner source, and
redacted logs are retained under `/tmp/context-trim-provider-20261002` (mode 0700).
The `standard-custom-async` and `lite-settled-diagnostic` directories contain
the qualified shapes; earlier assumptions and rejected shapes are retained
separately. Two initial fixture responses emitted no reasoning and were not
counted as reasoning qualification; the custom run retained its original first
response and continued from it. An erroneous custom probe attempted inference
before settling a synchronous editor; its 400 was preserved and the existing
editor was then settled in the same history.

Twenty Codex HTTP requests ran: seventeen completed and three returned the
documented HTTP 400 outcomes. The additional four ChatGPT plan requests all
completed, bringing the total to twenty-four HTTP requests. These are provider
gates, not a deployment gate.
The changed harness library compiled with its pinned Rust 1.93 toolchain and
declared cached dependencies. A narrow source test wrapper executed all five
existing SSE tests (five passed, zero failed); it did not run the full Cargo
suite. The source passed pinned Rustfmt check and `git diff --check`.
