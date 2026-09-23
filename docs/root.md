# Wave 0 root scaffold

The root owns the common model/contract/item/protocol types and compactor
boundary. The `core` lead owns `crates/harness` implementation and the
`crates/harness-demo` CLI. It scaffolds leaf boundaries before delegation:
`core-transport` owns `src/transport/`; `core-store` owns `src/store/`;
`core-loop` owns `src/turn.rs`; `core-demo` owns the CLI. The lead owns
contract-file amendments and integration.

Root does not scaffold server or web in wave 0. The initial interfaces are
deliberately small; a missing shared type is amended at the lead/root boundary,
never invented independently in sibling modules. All implementation must honor
the findings about subscription SSE and zero observed cache hits.
