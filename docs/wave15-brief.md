# Wave15 brief — restrict tools at the before-request boundary

## Outcome and source boundary

Extend wave14's integrated Send-only hook by one PRD decision:
`Send{tools_allowed'}`. A provider may select a subset of the tools in the
final `RequestPlan` for **one transport attempt**. The full tool definitions
remain stable in `ResponsesRequest.tools`; the selected names determine that
attempt's `tool_choice` (`allowed_tools`, or `none` for an empty subset).
Default `Send` still means the current `auto` behavior. The typed decision and
opaque evidence remain on the existing Store decision row, correlated to the
Engine request before transport. Do not implement `Inject(item)` or another
hook in this wave.

This closes a demonstrated gap: `PRD.md` requires per-request tool
availability without editing the cached tool list (settings section) and
names this before-request decision (hook table), while
`crates/harness/src/hooks.rs` represents only `Send` and
`crates/harness/src/transport/client.rs::request_body` hardcodes
`tool_choice: "auto"`. The browser's deterministic Engine path already
calls the hook but has not shown a decision that changes a request.

Keep the standalone browser usable: an existing browser `echo` command
completes normally, and its Engine request demonstrates the restricted tool
choice through the production browser provider and deterministic transport.
Assert both the outgoing request's full tool definitions plus selected
availability and the Store's typed decision/request/agent provenance. The
browser's existing child, reconnect and process-loss journey stays green.
Use the current browser and Store query surfaces; no new command or panel is
needed to display hook bookkeeping. Keep the demo policy local to that
consumer so the ordinary CLI provider's tool availability is not silently
changed.

## Shared decision before delegation

Root first lands a small compiling contract for the selected **tool names**
and request representation, then names its exact commit for dependents. The
selection is a subset of the final advertised names (including any typed
completion tool); preserve tool order and schemas. Reject unknown or duplicate
names before transport with a typed failure, and run Engine's existing pending
claim cleanup. Decide explicitly how required typed completion behaves if its
tool is excluded; never silently invalidate that contract. An empty selection
means `tool_choice: "none"`; an unchanged `Send` means `"auto"`. Preserve
historical Send rows and do not reinterpret provider-opaque evidence.

The owning path is `Provider::before_request` →
`Engine::evaluate_before_request` / `Engine::run` → `ResponsesRequest` →
`transport::client::request_body`, with `Store::record_decision` as the durable
decision owner. The current browser consumer is
`crates/harness-demo/src/main.rs::run_deterministic_engine_completion`, via
`CliProvider` and `DemoProvider`; `crates/harness-demo/tests/standalone_browser.rs`
is its independent acceptance test. Verify these exact paths before changing
them. No second request builder, tool registry, or browser-only lifecycle.

## Work and review

Sol root owns the shared contract, source-pinned decisions and integration.
After it compiles, admit bounded Luna implementation owners for (1)
Engine/transport application and failure cleanup and (2) provider/Store
decision round-trip and browser demo policy. Admit a separate Luna owner for
expected-red acceptance of the browser path. Keep test ownership independent
of the producer; use separate files or agree on edits before siblings touch
`engine.rs` or `main.rs`. Commission independent exact-tip review for the
implementation candidates, request repairs from the retained owners, and
rerun the final focused gates after integration. Review a concrete commit,
not a branch name or a green result from an earlier source.

Acceptance requires a captured deterministic transport request where the
advertised schemas are unchanged and `tool_choice` contains exactly the
selected names, plus a `none` case. An invalid selection must make **zero**
transport calls and leave the pending claim clean. Store readback preserves
the typed restriction and opaque evidence; reopening the browser data does
not invoke the hook again. The existing browser command completes while its
request and decision agree. Default Send and transport-failure behavior
remain covered. A retry, if attempted, gets a new decision for its own
attempt; do not claim exactly-once hook calls.

Run named focused tests with `scripts/cargo-focused-test --package ...
--target ... --filter ... --expect 1`. Plan one selected/executed assertion
each for `harness` library contract, transport body, Engine invalid/cleanup,
Store round-trip, `harness-demo` provider forwarding and
`standalone_browser` production consumer; root chooses exact new test names
after the shared scaffold. Run the existing browser journey once on the
integrated revision after preparing assets with the repository's web script.
Record actual selected/executed counts, exact source, raw failures, retained
evidence and any dirty working-tree status. Do not call compilation or an
expected-red test a pass.

## One bounded automation trial

Use `Project.TestEvidence.startFocused` (or `startFocusedIn` for an explicit
checkout) with caller-supplied `Cmd.Memory`, then attach
`Project.CheckResults.watchChecks` to a named focused check while the root
continues integration. On a failed or incomplete result, inspect the original
job with `Project.RetainedEvidence.recoverRetained` under a byte budget. The
trial measures whether a late result reaches the owner with its command
receipt, selected/executed counts and source observation; it never resubmits
the test to obtain a summary. Consult `docs/agent-automation-menu.md` and
the compiled check examples. The supervisor launch record identifies the tested
workspace pin; report a missing module as source drift instead of doing a routine
bindings inventory. Capture one opportunity, attempt, outcome, fallback
and evidence in `docs/automation-trials.json`. A notice attempt is not proof
of delivery; unavailable/unknown results stay unknown. If binding or command
admission fails, use the direct focused script and keep product acceptance
independent of the trial.

## Stop and handoff

No Exomonad adapter, credentialed inference, external shell tools, new UI
command, live port-4600 demo change or Tailscale change. Preserve the
unrelated dirty work and all wave14 evidence. The supervisor owns
launch identity and exact runtime/workspace pin. Root records the final source,
checks, selected/executed counts, unresolved behavior, interviews and resource
release; retire only this wave's actors after their evidence is retained.

This document prepares wave15; it does not launch it.
