# Wave16 shared contract

Source before implementation: `109491329436b055884ead2a117716170e88c90b`.
Shared contract checkpoint: see `NEXT.md` (resolve its exact Git OID).
This is PRD.md § “before-request” and the wave16 brief's Inject branch.

`BeforeRequestDecision::Inject { item, tools_allowed }` composes the Inject
branch with wave15 restriction. `None` retains auto tool choice; `Some(names)`
uses the existing final-advertised-name validation, including typed completion.
The accepted injected form is exactly a user-role message with a nonempty
string `content` and no other keys besides `type`, `role`, `content`.
This excludes configuration updates, assistant/tool impersonation, fabricated
tool results and ambiguous extras. Invalid input produces a typed Engine error,
cleans the pending claim and does not reach transport. The message is appended
after the canonical history to this transport attempt only, before the single
request body is serialized. It is not appended to durable history or recursively
submitted to the hook. The Store decision records the typed Inject and opaque
evidence on the original request/agent; replay/reopen reads it without invoking
the provider. `event_refs` cover the history presented to the hook, not an
unseen fabricated history.

Deterministic browser sequence: root submits existing `echo inject-context`;
BrowserProvider returns `Inject` with
`{"type":"message","role":"user","content":"standalone injected context"}`,
`tools_allowed:Some(["sleep"])`, and opaque evidence
`{"consumer":"standalone-browser","selection":"echo-inject-sleep",...}`.
The captured canonical body has the original echo history, then one injected
message, full tool schemas unchanged and the sleep-only `tool_choice`.
Other echo remains wave15 `SendRestricted`, child remains empty selection,
and default remains Send. The browser acceptance test must exercise this
through the existing production command path and retain reconnect and
process-loss assertions.

Owner map: root `hooks.rs` and this contract; Engine owner `engine.rs` and
`transport/client.rs`; Provider/Store owner `harness-demo/src/main.rs` and
`harness/src/store/mod.rs`; independent acceptance owner
`harness-demo/tests/standalone_browser.rs`. The no-overlap map requires
each owner to ask rather than edit a sibling path.
