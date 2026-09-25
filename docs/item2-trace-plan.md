# Item-2 trace seam (root scaffold)

Source: `NEXT.md` standing objective, PRD acceptance item 2, and the measured
gap in `docs/item2-live.md`. No live inference runs until this path is
auditable. The scaffold source is the root integration head after the cache
and item-2 findings.

Ownership:

- `crates/harness-demo/src/trace.rs`: bounded trace leaf. Implement an
  opt-in wrapper around the production `ResponsesTransport`; record the
  redacted request body *made by* `transport::client::request_body` immediately
  before dispatch, model turn completion, and timestamped correlation
  metadata. Do not write credentials, prompt text, token, account, or raw
  body. Fail the opted-in run if trace recording fails. Send root the exact
  consumer signature before root wiring. Offline tests only.
- `crates/harness-demo/src/main.rs` and `driver.rs`: root consumer wiring.
  Enable the wrapper only for a deliberate manual trace mode, report sleep
  start/settle by call handle, and ensure `wait_agent` result can be correlated
  with its request. The ordinary CLI remains unchanged.

Acceptance for the seam: focused offline check verifies redaction and
request/job correlation; manual run uses one credentialed inference scenario
and stores redacted evidence in `docs/findings.md`. This scaffold itself does
not establish item-2 live acceptance.
