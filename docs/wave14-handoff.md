# Wave 14 before-request hook — integrated source

## Outcome

Production source `75a1e014d0bc3d7e8208875c84e628dcc2f4bdca` has one
typed, Send-only, default pass-through before-request hook on the existing
Engine request path. The final `ResponsesRequest` items, tools and effort
enter `RequestPlan`; the provider's typed Send decision and optional opaque
evidence are recorded in the existing Store decision row against the active
Engine request before transport. `requests.branch` carries agent provenance.
An attempted transport failure leaves the decision durable; reopening/readback
does not invoke the hook. A Store decision-write failure now runs the existing
pending-claim cleanup path, and cancellation can interrupt a stalled hook.
Restriction/injection variants are not supported or advertised.
The hook runs before each Engine transport attempt; this slice makes no
exactly-once promise across retries. No separate retry-focused acceptance
case ran.

The deterministic browser's `CliProvider` forwards the demo provider's
`{"consumer":"standalone-browser"}` evidence. Its isolated Store decision
is tested through the production browser command path. Browser snapshot
`request/{command_id}` and Engine's durable request UUID are intentionally
different identities. Existing browser command, child message, reconnect and
process-loss behavior remain in the same standalone journey. No
credentialed inference, external shell tools or live demo change ran.

## Decisive verification

All focused runner invocations used `scripts/cargo-focused-test --expect 1`.
At exact source `75a1e014` each selected, ran and passed **1/1**:

| Package / target / filter | Evidence |
| --- | --- |
| `harness` `lib` `before_request_decision_store_failure_cleans_pending_claim` | `.exomonad/build/cargo/debug/deps/focused-ulha_e9w/evidence.json` |
| `harness` `lib` `before_request_send_persists_on_transport_failure` | `.exomonad/build/cargo/debug/deps/focused-pibdmwhm/evidence.json` |
| `harness` `lib` `before_request_decision_roundtrips_after_reopen` | `.exomonad/build/cargo/debug/deps/focused-fa9enxig/evidence.json` |
| `harness-demo` `bin:harness-demo` `before_request_forwards_standalone_browser_marker` | `.exomonad/build/cargo/debug/deps/focused-0f4nsfbi/evidence.json` |
| `harness-demo` `test:standalone_browser` `standalone_missing_assets_and_clean_and_process_loss_reopen` | `.exomonad/build/cargo/debug/deps/focused-hb7se6bj/evidence.json` |

The standalone browser test was first confirmed expected-red at
`506e1791` (1 selected/executed/failed, no Store decisions) before producer
integration, then passed on `75a1e014`. After discovering that this root
checkout lacked `web/dist`, `nix develop .#web -c
scripts/verify-browser-journey` prepared assets and passed TypeScript check,
web tests **14/14**, and production `browser_journey` **1/1** at the
integrated source. These checks did not exercise a credentialed model API.
Each final focused evidence JSON records exact source `75a1e014`, exit 0,
and a dirty working-tree status **only** for a pre-existing
`.exomonad/helpers/README.md` edit. That dirt is not claimed clean and was
not included in product commits. Port 4600 returned HTTP 200 after checks;
it and Tailscale were not changed.

## Review and scope

Provider/Store candidate `cb8e94ba` was accepted by exact-tip independent
review after a typed-serialization test repair and merged in `d1de787`.
Engine candidate `978c650e` was accepted by a fresh exact-source review
after a Store-write cleanup repair and merged in `75a1e014`. A retained
reviewer's stale checkout had correctly refused to judge the latter repair;
that refusal was not counted as acceptance. The expected-red acceptance
test was independently authored and merged in `6ff6339`; its integrated
green run is the product consumer check, not its earlier compilation.
Details, failed helper trials and tree costs are in
`docs/exomonad-friction.md` and `docs/interviews.md`.
All six wave-14 workers/reviewers were retired through typed cleanup;
subsequent host notices confirmed resource release for the three
implementation actors. Branches, worktrees and evidence were not deleted.

## Remaining provenance and next owner

`docs/wave14-launch.md` is still a template: the supervisor owns the exact
matched runtime build/source, final launch source, run ID, root thread and
logs. Root asked for those facts in `docs/questions.md`; leave them pending
until the supervisor supplies them. No host launch, second-device check,
credentialed provider inference, full hook catalogue, or live demo migration
is claimed here. The next owner is the supervisor for launch-record identity
and any later authorization; the checked Send-only feature needs no such
identity to establish the local product tests above.
