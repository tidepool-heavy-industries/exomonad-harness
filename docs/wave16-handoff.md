# Wave16 checked integration — deterministic before-request Inject

Product-code source: `9123ffba6af1171f072b48c9e95db94786eabc67`.
This is an explicit stop slice, not a launch authorization. The supervisor
owns launch identity and any later scope. No credentialed inference,
Exomonad adapter, external tool, new browser command, live-demo change or
Tailscale route change was authorized or run.

The typed `Inject { item, tools_allowed }` decision accepts one exact
nonempty user-message context item and optional final-advertised-name
restriction. Engine rejects invalid item or restriction before transport
and cleans pending claims. It appends the item after hook history once for
that attempt, without persisting it as earlier history or reentering the
hook. Store records the typed decision and opaque evidence with original
request/agent provenance and pre-injection event refs. Browser root
`echo inject-context` deterministically exercises that path with `sleep`
selection. Ordinary echo, child and default Send retain wave15 behavior.

## Source and independent review

Shared contract was checked at `594b3d0`, then documented at `f84e8b0`.
Independent browser test `424bab6` was confirmed red at the old
SendRestricted decision and merged at `394311f`. Provider/Store `859f56a`
was independently accepted at exact HEAD after its provider and Store
focused checks each matched/executed/passed 1/1; root merged it at
`3f4ec09` and checked both there 1/1. The first Engine ReviewFlow
reviewer never became observable; it produced no verdict and was
cancelled with resource cleanup unconfirmed. Fresh fallback reviewer
independently accepted Engine `7dd904f` at exact HEAD after valid,
invalid/cleanup and prior-restriction focused checks each 1/1. Root
merged it at `91cf294`. Root's formatting-only shared-contract
amendment produced the code source above. Component reviews did not
claim combined acceptance.

## Integrated evidence

At `91cf294`, the three changed Engine boundaries and the production
standalone browser journey each matched/runnable/executed/passed 1/1 in
gated retained job `48ed7275-fcc4-4440-a779-0c43ef66a6bf`.
Evidence files under `.exomonad/build/cargo/debug/deps/`:

| Check | Evidence |
| --- | --- |
| Valid once-only Inject, canonical body and retained decision on transport failure | `focused-ixneei24/evidence.json` |
| Invalid Inject before transport and pending-claim cleanup | `focused-wyila2qi/evidence.json` |
| Prior invalid restriction/typed-completion cleanup | `focused-65m817kx/evidence.json` |
| Standalone browser with echo, Inject, child, reconnect and process-loss reopen | `focused-px1qs9_h/evidence.json` |

Provider policy and typed Store reopen checks at `3f4ec09` each matched/
executed/passed 1/1, with `focused-tpwj1r27/evidence.json` and
`focused-fzjht7b0/evidence.json`. Root's pinned Nix preparation at
`f84e8b0` passed TypeScript check, web tests 14/14 and production build.
Web code did not change. At final product-code source `9123ffb`,
retained job `d1a06b11-3a5d-4464-9a4b-d17ffb3c206a` exited 0/clean:
the typed Inject serde boundary and complete standalone production browser
journey each matched/runnable/executed/passed **1/1**. Their raw evidence
is `focused-3h_mki0u/evidence.json` and
`focused-xc_ngq4e/evidence.json`, respectively. Both JSON records name
`9123ffba6af1171f072b48c9e95db94786eabc67` as source. This final
rerun followed a formatting-only change to `hooks.rs`; the earlier Engine
and Provider/Store focused checks remain attached to their stated prior
integration sources, not silently relabelled.

Focused runner JSON at `91cf294` and final source `9123ffb` identifies
each exact source, exit 0 and one matched/runnable/executed/passed per
check. It also reports a dirty working tree consisting of pre-existing
helper files and uncommitted run documentation, not unmerged product
files. `cargo fmt --all -- --check`, `git diff --check` and automation
ledger JSON validation exited 0 after the final formatting amendment.
The browser
uses an isolated test-owned port/data; the old wave12 demo data and
Tailscale routes were not changed.

## Orchestration and limits

Three bounded Luna owners worked in parallel on Engine, Provider/Store
and independent acceptance. Owner/reviewer interviews are in
`docs/interviews.md`; trial calls, retained jobs, outcomes and
non-use opportunities are in `docs/automation-trials.json`; failure
paths are in `docs/exomonad-friction.md`. The specialized
`SessionHelpers.Wave16Gate` executed once in the Provider owner with
1/1 count evidence but correctly returned **unknown** strict verdict
because its expected source was the older scaffold OID and the child
checkout was dirty. ReviewFlow routed a candidate but its reviewer
did not start observably, so it did not save a review frontier; the
ordinary exact-tip review was the fallback. No runtime or token-saving
claim follows from these trials.

After retaining interviews, root retired the three implementation actors
and two successful reviewers (`StoppedNow`); typed cleanup completed for
their fork groups. The ReviewFlow actor finished with
`ReviewStopped ReviewerUnavailable`. Its cancelled reviewer emitted an
exit notice, but the earlier `StoppedRetaining` resource warning was
not superseded by a clean-release receipt; a later cleanup plan refused
`unknown fork group 2`. The affected host resources remain a named
uncertainty, not product behavior.

Remaining uncertainties are resource cleanup from the cancelled
ReviewFlow reviewer and any later credentialed/live scope the supervisor
may separately authorize. Product behavior beyond this deterministic
standalone slice is not claimed.
