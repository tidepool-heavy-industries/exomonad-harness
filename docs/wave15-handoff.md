# Wave15 checked integration — restricted before-request tools

Product source: `40bd399e274c95d56374cb36efe09f66eb74b43a`.
Root merged independent acceptance and its corrections, independently
reviewed Engine/transport `f616be77`, then independently reviewed
Provider/Store `5f31285c`. The final provider review confirmed its
exact HEAD, passed Store, policy, capture and production standalone
browser focused checks 1/1 each, and accepted the component.

`BeforeRequestDecision::SendRestricted { tools_allowed: names }` selects
names from the final advertised tools for one transport attempt.
Unknown/duplicate names or exclusion of required typed completion fail
with typed `InvalidToolSelection` before transport and clean pending
claims. `ResponsesRequest.tools` retains full definitions and order;
`tool_choice` is ordered `allowed_tools`, literal `none` for an empty
selection, or `auto` for default Send. The existing Store row durably
records the typed choice, opaque evidence, request and agent. Browser-only
policy uses the explicit Engine invocation command and agent: root echo
selects `sleep`, child selects none, and other commands remain Send.
Ordinary CLI policy is unchanged. The deterministic browser transport's
opt-in capture writes canonical production `request_body` JSONL to a
test-owned path; capture failures propagate as transport errors.

## Final integrated checks

On product source `40bd399e`, each named Cargo check selected 1 runnable,
executed 1 and passed 1 with `scripts/cargo-focused-test --expect 1`.
Evidence files are relative to `.exomonad/build/cargo/debug/deps/`:

| Boundary / filter | Evidence |
| --- | --- |
| Contract `restricted_names_and_evidence_cross_serde_boundary` | `focused-z5_6xtqb/evidence.json` |
| Transport `restricted_tools_serialize_exact_order_and_empty_as_none` | `focused-hvbkq85_/evidence.json` |
| Engine `restricted_invalid_names_and_excluded_finalize_fail_before_transport_and_cleanup_claims` | `focused-i2ldjro6/evidence.json` |
| Default Send `request_is_stateless_and_pins_effort` | `focused-abxiqvy9/evidence.json` |
| Store `restricted_before_request_decision_roundtrips_after_reopen` | `focused-zukf4lpn/evidence.json` |
| Browser provider `browser_provider_policy_uses_explicit_invocation_classification` | `focused-tn02u3hh/evidence.json` |
| Capture `deterministic_capture_appends_production_request_body_jsonl` | `focused-ouh03m9n/evidence.json` |
| Standalone production browser `standalone_missing_assets_and_clean_and_process_loss_reopen` | `focused-p7tyrf9y/evidence.json` |
| Transport-failure Send regression `before_request_send_persists_on_transport_failure` | `focused-u7mjl2ll/evidence.json` |

The standalone test verified an echo completes; captured requests retain
the same full tool definitions while echo `tool_choice` selects only
`sleep` and the child request selects none; Store decisions retain typed
echo/child choice, evidence, request and agent; reopen does not invoke
the hook again. It also retains the child, reconnect and process-loss
journey. `nix develop .#web -c scripts/verify-browser-journey` exited
0 at the integrated source: web tests 14/14, assets built, browser
journey 1 selected/executed/passed. Port 4600 remained HTTP 200 and
was not changed. Evidence JSON records a dirty worktree: the two
pre-existing `.exomonad/helpers` edits and uncommitted wave15 notes,
not unmerged product files.

## Red barriers, review and automation

Test-only acceptance at `b711920` selected/executed 1/1 and failed at
the intended missing-capture assertion. Later expected-red runs on
test corrections reached old-provider evidence assertions; these were
not green tests. Engine first focused run exited 137 before execution,
then its repaired candidate passed 1/1 checks and was accepted at
exact tip. Provider first candidate OID was invalid; later exact-tip
review caught whole-request substring classification. A provider-only
review at `59bf6236` could not prove outgoing restriction because its
checkout lacked the sibling Engine source. After rebase, combined
review at `8db250a5` ran standalone browser 1/1 and failed on child
misclassification. The explicit-invocation repair `5f31285c` passed
fresh exact-tip review including standalone browser 1/1 before merge.

The bounded automation trial ran one existing focused transport
regression on source `a61a9c6`: 1 matched/executed/passed. A
`CheckResults.watchChecks` notice arrived but its first evidence read
failed, so its compact summary remained unknown. `collectFocused`
later read the original evidence file successfully without rerun;
bounded `recoverRetained` on the original job found expired raw output
(HTTP 409). See `docs/automation-trials.json`. Trial behavior is not
product acceptance.

No credentialed inference, external shell tools, Exomonad adapter,
new UI command, second-device check, live port-4600 change, or Tailscale
change ran. Remaining owner is the supervisor for launch/runtime identity
and any later scope; wave15's authorized deterministic browser behavior
is checked at the product source above.
