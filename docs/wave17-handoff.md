# Wave17 checked integration and release slice

## Outcome and source

The checked combined standalone deterministic browser candidate is
`6c28a7918d722beace4715344220d048d7a7d95a` on `rsi/wave17`.
It contains wave16 stop handoff `21cb20e94a84c93caa16e580f5efa156dbd4acbb`
and helper baseline `1b0f7a0` by ancestry, with the exact shared
`.exomonad/workspace` pin
`2599a6434b5f09ab176b29a851b6a680ef4aa45d`.
`7c7f8bb` was the wave16 merge, retaining the active wave17 assignment,
wave16 interviews/trial evidence and the new helpers. Root reconciled
both automation-ledger variants instead of taking either whole file.

The additional product-test repair replaces a raw TCP read after the
tungstenite handshake with typed WebSocket frame consumption. It
preserves the existing browser assertions and process cleanup. A
direct `futures-util` test dependency and its one-line Cargo.lock
entry were committed at `fc652b4` and `b6fea32`. Reviewed test
candidate `f6def4d` was merged at `6749fbe`. The operator release
instructions, including database-parent creation and `umask 077`,
are [wave17-release.md](wave17-release.md); reviewed `07d7c8c` was
merged at `826ef05`. No hook, Engine, Store, web UI or external-tool
semantics changed beyond the wave16 ancestry.

## Decisive integrated verification

At **exact clean source `6c28a7918d722beace4715344220d048d7a7d95a`**,
root used the committed `SessionHelpers.runCheck` and
`SessionHelpers.runBrowserCheck` with that OID passed at each call.
Each focused Cargo filter expected/matched/runnable/executed/passed
**1/1/1/1/1**, exited 0 with `CommandClean`, and its evidence JSON has
`working_tree_status: ""` and that exact source:

| Boundary | Original job | Runner evidence |
| --- | --- | --- |
| Standalone browser echo, Inject, child, reconnect, process-loss reopen | `079345ac-a432-41ac-b593-ceba95ca1663` | `.exomonad/build/cargo/debug/deps/focused-qxpndm_p/evidence.json` |
| Engine valid Inject, attempt-only position, retained decision on transport failure | `fbe22a64-a3e6-4890-9951-adf69dd6a2a9` | `focused-e0bybkpx/evidence.json` |
| Typed Store Inject decision reopen | `886d9647-453e-4d2e-b977-a791f910dd58` | `focused-vgoxqms1/evidence.json` |
| Invalid Inject before transport and pending-claim cleanup | `f25389a7-22eb-4e12-8eef-119fee1c85f4` | `focused-jur5twds/evidence.json` |
| Invalid restricted names / excluded typed completion cleanup | `2a83c8ec-ae44-490b-b2db-c3e5a33e6971` | `focused-0cbe1alo/evidence.json` |

The browser helper prepared assets in this consuming checkout before
the assertion: `npm ci`, TypeScript check, web tests **14/14** across
five files, and production Vite build all succeeded in the original
browser job; preparation exit 0 is separate from the browser test
exit 0. `cargo fmt --all -- --check`, `git diff --check`, ledger JSON
validation, ancestry/pin checks and `git status --short` also
exited cleanly at the checked source. `cargo check -p harness-demo
--test standalone_browser` compiled the **old test source** before
the lockfile commit; it ran zero tests and is not browser acceptance.

The earlier browser job `9928d6c5-88a1-4bc9-9086-2ede08edd96b`
at `7c7f8bb` prepared successfully but selected/executed 1/1 and
failed at `standalone_browser.rs:120` (`event` instead of `snapshot`).
Its original retained job was recovered, not rerun. The worker's
repaired-tip browser job
`cb1cd32c-47d3-4630-bf79-cf10baa70bd0` passed 1/1 at `f6def4d`,
but generated a dirty Cargo.lock and had **unknown strict source
assurance**. Neither earlier result substitutes for the clean
integrated-source pass above.

## Independent review and bounded automation

An evidence-only Luna audit found no source-scoped Inject/restriction
defect at `7c7f8bb`. Its own Store attempt exited 137 without
compilation/count evidence; root's named focused checks supplied the
combined-source results instead.

The first native `Project.ReviewFlow` smoke used real release-doc
candidate `bba9416`, `ComponentReview`, one-repair limit, coordinator
actor 11 and exact-tip reviewer **actor 13**. Actor 13 reached its
first provider turn and returned typed `Accepted`; root observed the
typed final flow state. Root rejected that content verdict because
the copied database path lacked its `.local` parent on a clean
checkout. Ordinary fresh exact-tip review of repair `9358cf2`
returned `Repair` for missing private-path umask. Fresh review of
`07d7c8c` accepted the one-file instructions after checking the
CLI, preparation script and privacy/startup sequence, but ran no
server/browser test. Independent exact-tip review accepted browser
test repair `f6def4d`: it ran `git diff --check` and a **compile-only**
`cargo test --no-run` (zero tests executed), inspected frame and
cleanup paths, and did not rerun the browser. Root merged only the
reviewed tips, then ran the clean integrated gate. ReviewFlow
activation and typed delivery succeeded; review quality and
model-round savings are not inferred. See
[automation trials](automation-trials.json), [friction](exomonad-friction.md)
and [interviews](interviews.md).

## Preservation and limits

The operator's master branch was not moved or pushed. No credentials,
model inference, adapter, successor, recurring timer, external tool,
Tailscale or protected demo data were touched. The production browser
test used isolated test-owned ports/data. A read-only HTTP probe of
`127.0.0.1:4600` in this namespace returned connection failure
(`000`), so protected-demo availability was **not verified** here;
no stop/start or configuration change was attempted. No manual
copy/paste launch of [the release instructions](wave17-release.md)
was run. `umask 077` protects newly created demo files, but does
not repair permissions on a pre-existing `.local` directory/database;
an operator reusing such a path must verify its permissions.

**Earlier helper-draft preservation:** startup found three dirty
tracked paths: `.exomonad/helpers/README.md` (an expanded focused
test helper seed), `.exomonad/helpers/SessionHelpers.hs` (draft
re-export of only `SessionHelpers.TestEvidence`, omitting the
committed BrowserChecks and FocusedGateExample exports), and
`.exomonad/helpers/SessionHelpers/BrowserChecks.hs` (draft deletion).
Before the merge, root created the retained stash object
`79a179cc23c38217722df87a6f797f52975c16fa`
(`wave17-preexisting-helper-draft`) containing exactly those
three tracked path changes. Root applied it once, then restored
the committed helper baseline in the working tree for the checks.
The stash was **not dropped**; it is unrelated draft evidence,
not integrated product source. The operator subsequently banned
stash/reset/path-checkout commands; none were repeated afterward.
The committed BrowserChecks and new workspace pin remain intact.

The three Luna frontier workers and the ReviewFlow, final-doc and
browser-test reviewers were interviewed. The first ordinary doc
reviewer was retired after its typed finding; no separate interview
was obtained from it. Workers and ordinary reviewers were retired
through typed cleanup; two frontier worker stop steps
initially returned `StoppedReleasing`, not immediate resource-release
proof. The ReviewFlow record actor finished with its typed result;
its reviewer later returned `AlreadyStopped` to root's stop request,
but root could not plan cleanup of that record actor's group
(`actor 1@1 does not own fork group 2`). Host resource release for
that group is therefore not asserted from the refused cleanup plan.
The wave16 ReviewFlow resource uncertainty remains as documented in
the wave16 handoff. No unmerged branch holds reviewed or passing
wave17 product/test code: `07d7c8c` and `f6def4d` are ancestors of
the checked source. An unmerged pre-existing helper draft remains
preserved in the stash above, not approved for integration.

The supervisor remains the next owner for any external launch or later
scope. This run stops at the deterministic standalone integration and
release handoff.
