# Issued requests and replay

`ResponsesRequest.tools` is an immutable `ToolManifest`. Its public serde
representation remains the ordered tool array, including unknown fields.
Embedded installations construct one validated `EmbeddedToolManifest` and
publish request surfaces with `ToolSurface::from_manifest`. A surface still
pins a fresh host endpoint, version and authority for each request. Engine
composes configured and finalize tools once per run, rebuilding when the
provider manifest changes. It does not retain an endpoint between requests.

`Provider::before_request` receives a borrowed `RequestPlanView`. Consumers
that retain a plan explicitly call `to_owned`; existing owned `RequestPlan`
serde consumers can use `as_view`. Transport serializes the normalized borrowed
request directly. `request_body` exposes the same normalization as a JSON
Value for offline comparisons and opt-in traces.

Engine loads each history window with its existing ordered content hashes.
Unmodified history reuses those hashes for decision evidence and replay.
Projected and hook-injected Items are interned as their exact issued values.
Store seals these references before transport starts, so compaction, late
output and subsequent request history cannot rewrite an issued window.

Completed `model_turn` events use replay format `1`: ordered input and response
Item hashes, instruction and manifest hashes, and the exact request scalar
fields, including `tools_allowed`. Completion and response references commit
in one transaction. The Store's immutable `items` table owns the referenced
bytes. Failed transport can leave interned Items without a completion event;
this does not fabricate a completed replay turn.

This is an explicit internal format break. Replay of an event without format
`1`, including the older embedded request/response payload, returns
`StoreError::UnsupportedReplayFormat` with its event sequence. Opening the Store
and reading ordinary history remain supported; refusal never rewrites the old
event bytes. There is no automatic conversion or fallback to later history.

Call admission and replay kind lookup share Store's exact indexed invocation
query. Every matching recognized invocation is checked; duplicates and malformed
matching calls are refusals. The separate indexed surface query reads only the
latest issuing-surface event. Neither query decodes unrelated history or
completed replay payloads. Host admission, operation identity and original
request/call IDs remain independent checks.

Schema `8` retains the typed terminal outcome on each settled claim alongside
its output hash, in the same transaction. Success stores only a discriminant;
the existing immutable Item owns its payload. Failure retains `ToolFailure`,
while cancellation, interruption and unconfirmed cancellation remain distinct.
Raw output writers must supply an explicit `TerminalOutcome`. Completion
acknowledgment and replay read this same authority; the former
`operation_output` event is no longer written or used to decide completion.
Inherited claimants must retain identical output and terminal evidence.

Older schemas open through an additive migration. Their settled rows have no
terminal marker: typed replay returns `UnsupportedReplayOutcome`, preserving
their original Item bytes rather than interpreting an `error` key as failure
or silently treating an unknown result as success. Ordinary diagnostic Item
reads remain available. Successful tool payloads containing `error` or
`failure` fields remain successful.

Offline tests compare normalized issued requests after reopening, preserve
projected/injected and unknown fields, reject old formats without modifying
bytes, check transaction rollback and validate duplicate/malformed calls. The
two-size cost test reports retained reference bytes against the former full
payload format and counts the actual unchanged-input reencoding path. It does
not measure provider latency or prompt-cache behavior.
