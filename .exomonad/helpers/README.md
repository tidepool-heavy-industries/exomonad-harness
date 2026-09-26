# Session helper seed

Edit `.exomonad/helpers/SessionHelpers/TestEvidence.hs` in the actor's helper
draft and publish with `reload_helpers`. Forked actors inherit the draft at
fork time. Specialize a `FocusedSpec` for the component, choose its memory
reservation, and compose the returned run with a background result watcher.

The seed's `runTests` starts one focused check with a 4 GiB reservation and
returns either a setup refusal or a retained `FocusedRun`. Bind that result
before attaching a watcher. `Project.CheckResults.watchChecks` observes completion
while you work; `Project.TestEvidence.collectFocused` reads retained evidence,
and `finishFocused` optionally adds a Jev diagnosis. The shared module owns
parsing and acceptance, so this seed cannot drift into a second test runner.

Remix the seed to capture useful repeated work: select the exact tests, tune
resource use, compose evidence reads and bounded Jev decisions, and present a
compact result. Keep exact source, execution counts, cleanup status and evidence
paths available. A reported pass, source assurance and integrated acceptance are
distinct facts. See `docs/agent-automation-menu.md` for available compositions.
