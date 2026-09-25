# Item 2 live acceptance — 2026-09-24

**Outcome: not measured; acceptance remains open.** No inference-spending
request was made. The existing CLI tree path can form a plausible prompt-driven
scenario (`harness-demo --tree --ask ...`): the demo provider has a slow
`sleep` tool, and `TreeProvider` exposes the agent verbs. But the available
runner does not provide the required auditable trace.

Evidence inspected at integrated source `d0245b3177afa21556c041ce05296a96b50b209a`:

- `crates/harness-demo/src/main.rs`: `tree_ask` starts the driver and waits for
  root completion; `DemoProvider::call_with_context` sleeps and emits a
  `sleep_started` progress event. The CLI does not render/store that progress
  as an acceptance trace.
- `crates/harness/src/transport/client.rs`: `request_body` constructs the
  outbound JSON, but `execute` sends it without recording or exposing it.
  Consequently there is no supported way in this harness to produce the
  requested redacted *actual request bodies* after a live run.
- The available ignored live test is
  `transport::client::tests::live_subscription_response`; it is a simple
  response smoke, not a tool/sleep/`wait_agent` scenario.

The prompt alone would not prove the required ordering (a further model
continuation while sleep is still pending, followed by `wait_agent` receiving
the original settled result). Running it without observable request and job
timing evidence would spend inference but still fail the stated acceptance.
Per the one-run/no-retry constraint, I stopped before making a live request.
No request bodies or tokens were captured, printed, copied, or stored. The
blocking evidence gap is therefore the harness's lack of a live trace surface
for outbound request bodies plus correlated sleep-job timing/result; this
document does not claim item 2 passed.

No source code was changed and no test or build command was run for this
findings-only measurement.
