# Browser contract and bundle boundary

`crates/harness/src/server/ws_protocol/contract.rs` owns the browser projections.
The server publishes the closed `StateEvent` enum and typed snapshots directly.
Store and Engine records remain native; their browser conversions expose only
fields needed by the operator. Actor-output history and committed events use the
same `ActorOutputProjection`. Provider bodies remain opaque only inside the
history-item and tool-result fields.

`WireU64` and `WireI64` serialize native integers as canonical decimal strings.
Their Serde parsers and schemas reject JSON numbers, overflow, leading zeroes,
plus signs, negative zero and trailing whitespace. UUID serialization has its
own canonical schema; inbound UUID schemas match the native UUID parser. UI
bounds and associations remain separate from structural wire validation, and
an output projection never grants native display or actor authority.

The native `//crates/harness:browser_contract` producer emits schemas and positive
wire scenarios encoded from these Rust types. Schemars 1.2.2 generates outbound
schemas for serialization and inbound schemas for deserialization. The declared
`//web:browser_contract` action uses locked json-schema-to-typescript 16.0.0 and
Ajv 8.17.1 to produce declarations and standalone validators. It validates every
Rust scenario before issuing an artifact. `//web:check`, `//web:test` and
`//web:dist` depend on that artifact and the exact offline npm closure.

The dist producer seals fresh assets with one manifest. Its source digest is
issued from the declared Harness Rust, SQL and Cargo inputs; its schema digest
is computed from the actual Rust schema artifact. The composition root passes
`BrowserBundleIdentity` from that same declared source to
`verify_browser_bundle(root, expected_identity)` before listening. Verification
checks the compiled DTO schema digest, exact asset inventory and hashes, then
retains verified immutable asset bytes. No runtime Git lookup or manifest-derived
expected identity is used. Library consumers do not need a mandatory embedded
source artifact; the executable or package composition owns that dependency.

Focused coverage lives in Rust decimal, contract, bundle, server output and
protected HTTP tests; the browser generated-contract tests consume the emitted
positive scenarios. Browser protocol and component tests exercise real reduction,
resynchronization, history, expansion and command behavior with those guards.
Build or link success does not establish test execution.
