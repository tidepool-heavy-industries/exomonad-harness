# Program-driven bounded model turns

`harness::invocation` composes the existing Engine, Store and JobScheduler for
one program-driven invocation. The host supplies instructions, the complete
function-tool manifest, initial items and an optional result schema. The profile
starts a fresh internal request branch under the parent cell identity. It does
not attach an actor, admit mailbox input or expose harness lifecycle tools.

The host calls `Invocation::next`, handles `Step::Callback` in the original
program continuation, then calls `Callback::complete`. After the provider round completes, callbacks run in provider
item order. They release scheduler capacity while awaiting the continuation, so
a nested invocation can use the same scheduler at capacity one. Invalid
arguments never reach the continuation. Duplicate and reserved tool names fail
preflight. Typed results use the existing finalize schema owner and decoder.

The host creates one `CellBudget` at the first Model invocation in a parent
cell. Every subsequent invocation shares it; `narrow` adds a smaller scope.
Defaults are 16 transport requests, 64 callback attempts, 128000 reported input
plus output tokens and 300 seconds. Requests count at the transport attempt
boundary; the production ResponsesClient currently performs no hidden retries.
Callback attempts count before argument validation. Missing usage remains
explicitly unknown and does not become zero-token evidence. Each receipt has
both invocation-local and cumulative cell counters.

Exhaustion latches the shared budget and cancels active provider futures. A
callback already handed to the program finishes cooperatively; the Engine then
retains its output and returns the exhausted outcome. `InvocationCloseHandle`
can signal the same provider cancellation without acquiring a step lock, while
preserving the admitted callback's completion sender. It does not cancel the
parent program or its other effects.

When enabled, `Step::HookRequested` follows durable output commitment. Its exact
operation and event ordinal identify the retained original output. The host
completes the hook with NoAnnotation, Abstained, Annotated or Pruned. Pruned
changes only that operation's model-visible output; Store retains the original.
Annotation decisions and receipts are retained as existing Store events, with
no schema migration. Wire receipts carry request references and counters;
transcripts remain in Store and are also available to Rust consumers.

Strict schemas support finite inline objects, arrays, strings, booleans,
numbers, integers, nullable values and anyOf. Haskell tagged oneOf variants are
converted to anyOf only after proving distinct required singleton tags, so the
acceptance set is unchanged. Optional object fields become required nullable
fields for the provider. Recursive references, open maps, unchecked validation
keywords and overlapping oneOf schemas are rejected explicitly.

The offline tests in `invocation.rs` run the real Engine against scripted
transports. They cover callback ordering, nested capacity-one scheduling,
shared exhaustion, cooperative close and deadlines, unknown usage, retained
outcomes and exact pruning projections. No credentials or live provider calls
are needed.
