# Async tool lifecycle: source basis for wave18

Inspected 2026-09-26. Local Codex revision
`f84db4db036355a79e5d8aac2aa5121e17f60a02` in Tidepool `vendor/codex`.
The supervisor read the owning source; these are references to inspect when
relevant, not an instruction to copy a second runtime into this repository.

## What the current Codex host does

Paths below are relative to `vendor/codex/codex-rs`:

- `core/src/tools/parallel.rs`, `ToolCallRuntime::handle_tool_call_with_source`:
  handlers run in spawned tasks. A read/write gate admits parallel-safe calls
  together and serializes other calls. Each invocation retains its issuing step,
  tool identity and cancellation context. Completion readiness is recorded at
  handler return, independently of later result collection.
- `core/src/session/turn.rs`, `try_run_sampling_request` and `drain_in_flight`:
  completed streamed call items produce futures held in `FuturesOrdered`.
  After the response stream ends, the loop drains those results into conversation
  history before returning to the next sampling step. Concurrent execution here
  does not establish unrestricted model continuation with unfinished direct calls.
- `core/src/tools/code_mode/{delegate.rs,execute_handler.rs,wait_handler.rs}`:
  Code Mode retains yielded cell state and its advertised step context. `notify`
  can inject output during an active turn. This is a separate host-managed cell
  mechanism, useful lifecycle evidence but not proof of provider async support.
- `protocol/src/models.rs`: function/custom outputs retain `call_id`;
  assistant `MessagePhase` distinguishes commentary from final-answer text.
  Preserve identities and terminality explicitly in the new harness.

The local migration reference
`skills/src/assets/samples/openai-docs/references/upgrading-to-gpt-6-astra.md`
points to the actual provider async guide. The supervisor opened that guide too.

## Published provider contract

[OpenAI async tool calling](https://developers.openai.com/api/docs/guides/async-tool-calling)
permits `async: true` on function/custom definitions and marks corresponding calls.
The application owns execution and pending work. It returns each result on the
original `call_id`; the model can continue independent work before that result.
An optional application-defined wait tool waits only for named jobs. Deliver their
completed results before the wait result. Continuations use the latest response
identity. A question to a user stays pending until the answer, rather than being
completed merely because the question was displayed.

## Apply this to the standalone harness

Use the existing Engine, event, Store and extension owners. First inspect their
actual capabilities. Agree on a concrete event sequence before parallel feature
work: launch pending A, continue independent B, complete B before A, deliver each
result exactly once to its matching call, and handle cancellation/late completion
without resurrecting terminal work. Include reconnect/reopen evidence consistent
with the existing persistence contract. These are proposed product acceptance
cases, not additional claims about the provider's cancellation/recovery semantics.

Prove the smallest missing slice with deterministic stubs and a browser-visible
interaction. Keep actual inference and the Exomonad adapter outside this wave.
Use local source and the documented wire contract as evidence; explicitly identify
any remaining semantic decision instead of guessing it.
