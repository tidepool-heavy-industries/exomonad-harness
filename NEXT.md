# Completed assignment — wave16 deterministic before-request Inject

The authorized standalone Inject slice is integrated and checked at
product-code source `9123ffba6af1171f072b48c9e95db94786eabc67`.
See [wave16 handoff](docs/wave16-handoff.md) for contracts, exact reviews,
counted checks, raw evidence, automation trial and remaining uncertainty.
At that source the typed Inject serde test and standalone production browser
journey each matched/runnable/executed/passed 1/1; browser coverage includes
the existing child, reconnect and process-loss path. Engine valid/invalid/
restriction checks each passed 1/1 at integrated `91cf294`, and Provider/
Store checks each passed 1/1 at integrated `3f4ec09`. Web tests passed
14/14 on the unchanged web source. The final source's `cargo fmt --check`,
`git diff --check` and trial-ledger JSON check exited 0. Working-tree
evidence explicitly lists dirty run docs and pre-existing helper files;
no clean-tree assertion is made.

`SessionHelpers.Wave16Gate` executed in a child and retained count/source
evidence, but correctly returned unknown strict assurance due an older
expected OID and dirty child checkout. The bounded ReviewFlow routed an
Engine candidate but its reviewer never produced a first observation;
fallback ordinary exact-tip review independently accepted Engine. That
cancelled reviewer reported exit, but its stop outcome retained
ToolService/socket/build/worktree cleanup uncertainty. All implementer
and successful-reviewer interviews are retained in `docs/interviews.md`.
The supervisor owns launch identity and any later scope. Stop at this
completed deterministic slice: no credentialed inference, adapter, new
browser command, live-demo or Tailscale change was run or authorized.

### Where the last run stopped — wave16

- No unmerged branch holds reviewed or passing wave16 product code.
  Acceptance `424bab6` is merged at `394311f`; independently reviewed
  Provider/Store `859f56a` is merged at `3f4ec09`; independently reviewed
  Engine `7dd904f` is merged at `91cf294`; root formatting amendment
  `9123ffb` is the checked product-code source. The earlier ReviewFlow
  reviewer produced no verdict and has no accepted code to merge.
- The run's documentation and helper trial source are committed in the
  explicit stop handoff after product checks. Pre-existing dirty
  `.exomonad/helpers/README.md`,
  `.exomonad/helpers/SessionHelpers.hs` and
  `.exomonad/helpers/SessionHelpers/TestEvidence.hs` edits were preserved
  and not folded into product code.
- Next owner: supervisor for any separate launch/runtime or later scope.
  Host cleanup confirmation for the cancelled ReviewFlow reviewer remains
  unverified; do not infer a clean release.

## Wave16 execution chronology (historical; superseded above)

The shared typed `Inject { item, tools_allowed }` seam is committed at
`594b3d006f49d0fefbbc38114bd7c276f8f57975`; its focused serde check
matched/executed/passed 1/1. An earlier `cargo test --exact` compiled but
matched **zero** tests and is not evidence of a pass. The current contract is
[wave16-contract](docs/wave16-contract.md), PRD.md § “before-request”.
The Engine arm at this scaffold only compiles; it does **not** apply injection.
The source that incorporates this brief will be resolved before wave admission.
Admission source is `f84e8b08d8a8111f896903e5470003f8b90ce6a4`.
Actors 2, 3 and 4 were admitted from it, owning Engine/transport,
Provider/Store and standalone browser acceptance respectively. First expected
replies: checked Engine component candidate, checked Provider/Store component
candidate, and expected-red browser candidate. Root's `web/dist` was absent;
`npm ci && npm run check && npm test && npm run build` was attempted as retained
job `2ad87a61-1f2e-4105-8e69-b369c0a7475d`, without touching the demo.
That job exited 127 before checks because plain `npm` is absent. Root verified
the pinned `nix develop .#web` shell has Node 24.13.0/npm 11.6.2, informed
acceptance and provider owners, and launched prerequisite job
`1379eb23-7029-49aa-ba01-391d40636f55` using that shell. No assertion
result follows from the first preparation attempt.
Pinned preparation job `1379eb23-7029-49aa-ba01-391d40636f55` subsequently
exited 0: `npm ci`, TypeScript check, web tests 14/14 across 5 files, and
production Vite build passed at the f84e8b0 checkout. This prepared root's
`web/dist`; each isolated child still needs assets in its own checkout.
Acceptance owner added test coverage but its first focused run compiled and
matched/executed 1/1 only to fail at missing child-checkout `web/dist`; it did
not reach the expected-red Inject assertion. The focused runner's integration
test target is `test:standalone_browser`, not `standalone_browser` (the brief
named the wrong syntax). Root supplied the pinned Nix build command and
correct target; the owner still owes asset preparation and assertion-level red.

Admission plan: one Luna Engine/transport owner (`engine.rs`,
`transport/client.rs`), one Luna Provider/Store owner (`harness-demo/src/main.rs`,
`harness/src/store/mod.rs`), and one independent Luna browser acceptance owner
(`harness-demo/tests/standalone_browser.rs`). First expected replies are
checked component candidates for the first two and a confirmed expected-red
barrier for acceptance. Shared sequence: root `echo inject-context` injects one
user message after canonical history with sleep restriction; ordinary echo,
child and Send remain unchanged. Root owns review and integration. The
specialized Haskell helper is `SessionHelpers.Wave16Gate.startWave16Gate`
(owner, name, `FocusedSpec`, fixed 3 GiB), then `readWave16Gate` on terminal
notice with failed/unknown callbacks; published helper source is
`e98a8f2fa0231e2e4973aad03eb4da7a6435891e206628f71e5b9b865556a71d`.

Engine owner submitted candidate `7dd904f8279a17b0189817137e3465e7cb84ac56`
from f84e8b0. Root's cumulative diff shows only owned `engine.rs`.
Owner reports three focused checks each matched/executed/passed 1/1 after
one initial exit-137 build; these are component reports, not yet review.
The bounded `Project.ReviewFlow` trial is active on this component, source
plan `ComponentReview`, repair limit 1, base f84e8b0, candidate 7dd904f,
coordinator actor 5 and forwarding route 6. Hypothesis: one exact-candidate
review or bounded repair reaches a typed acceptance/question without root
relaying routine findings. Stop on accepted or unresolved/one repair;
fallback is ordinary `reviewCommit` at exact tip. Root retains merge authority.

Acceptance owner corrected its isolated preparation with
`nix develop path:..#web` in its own checkout, then the focused
`test:standalone_browser` target matched/executed 1/1 and failed at the
intended missing-producer decision assertion (SendRestricted instead of
Inject), not assets. Test-only candidate `424bab6eb7aa9e405de2f92a904aa65909958b70`
changed only owned `standalone_browser.rs`, and root merged that confirmed-red
barrier at `394311f1567f48bd1472231b4836c9692f7553a9`. It is not green
product acceptance. The independent test still includes child, reconnect and
process-loss assertions beyond the red barrier.
The acceptance owner's typed own-words interview is retained in
`docs/interviews.md`; actor 4 was then retired with `StoppedNow`. Its branch,
worktree and raw focused evidence remain retained.

Provider/Store candidate `859f56a9d0493d24b43437eb35137fd0886e1151`
was independently accepted at that exact HEAD for component-only scope:
reviewer's Provider and Store focused checks each matched/executed/passed
1/1. Root merged it over the red acceptance barrier at
`3f4ec09bc9514689073b29ef3a0f51fce95eb655` and reran both focused
checks there, each 1/1. This is not combined Engine/browser acceptance.
The Provider owner's `startWave16Gate`/`readWave16Gate` trial ran one
original job with 1 matched/runnable/executed/passed, but returned **unknown**
because its expected source was f84e8b0 versus actual candidate 859f56a,
and the child had dirty helper/hooks files. See `docs/automation-trials.json`
for job/evidence. Provider owner and reviewer interviews are retained;
both actors stopped `StoppedNow`.

The Engine ReviewFlow trial admitted exact candidate `7dd904f` but its
reviewer never obtained a first provider observation. Root stopped that
unresolved reviewer; `stopAgent` returned `StoppedRetaining`, with
ToolService/socket/build/worktree release unconfirmed. Flow state is
`ReviewStopped ReviewerUnavailable`, no verdict/repair. Fresh ordinary
exact-tip review actor 11 is the bounded fallback and is pending; Engine
candidate is **not** reviewed or integrated yet. Host cleanup uncertainty
is in the friction record.

Fallback Engine reviewer independently accepted exact HEAD
`7dd904f8279a17b0189817137e3465e7cb84ac56` for component scope,
after three named checks each matched/executed/passed 1/1. Its first valid
Cargo attempt exited 137, then `CARGO_BUILD_JOBS=1` and 6144 MiB succeeded.
Root's cumulative diff confirmed only owned `engine.rs`; conflict-free merge
over Provider/Store + red acceptance produced combined source
`91cf294eb4fd5426821b7e2137d3ce4cd74cf59b`.
The integrated Engine valid/invalid/restriction tests and production browser
journey are now running as retained command
`48ed7275-fcc4-4440-a779-0c43ef66a6bf` (all four gated by `&&`).
No combined green is claimed until that command's terminal count/source
evidence is read. Root `web/dist` was prepared and present before launch.

The completed wave15 handoff below is retained history, not unfinished work.

# Completed assignment — wave15 restricted before-request tools

Wave15's authorized deterministic-browser product behavior is
independently reviewed and checked at source
`40bd399e274c95d56374cb36efe09f66eb74b43a`.
The before-request hook selects final advertised tool names for one
attempt without changing full schemas. Echo selects `sleep`, child
selects none, default Send remains auto. Invalid/duplicate selection
or excluded typed completion fails before transport and cleans
pending claims; Store records typed choice and opaque evidence with
request/agent provenance. The standalone browser command, child,
reconnect and process-loss journey remains green.

At that exact source nine named focused Cargo checks each selected
and executed 1/1 and passed; web tests passed 14/14 and browser
journey 1/1. See [wave15 handoff](docs/wave15-handoff.md) for exact
commands, evidence and review chronology. Port 4600 returned HTTP
200 and was not changed. The background-check automation trial
ran but its compact summary failed; see `docs/automation-trials.json`.
No credentialed inference, adapter, external tool or live-demo
change was authorized or run.

### Where the last run stopped — wave15

- No unmerged branch holds reviewed or passing wave15 product code.
  Acceptance `441e52fb`, Engine `f616be77`, and Provider/Store
  `5f31285c` are ancestors of integrated product source `40bd399e`.
  Superseded provider tips `b5b61f84` and `8db250a5` were rejected
  after concrete review/production-browser failures; do not merge them.
- Wave15 documentation/checkpoint notes are committed in the explicit
  stop handoff, after the checked product source. Pre-existing unrelated
  `.exomonad/helpers/README.md` and
  `.exomonad/helpers/SessionHelpers/TestEvidence.hs` edits remain
  uncommitted and must not be folded into product code.
- Next owner: supervisor for exact launch/runtime identity and later
  scope. No further live or credentialed behavior is inferred.

## Wave15 execution checkpoint (historical; superseded above)

Shared compiling source `a61a9c6d5f36d7efd91bde779d07c114174e0007`
adds the typed `SendRestricted` names and optional request selection. Its
focused serialization check selected/executed/passed 1/1; no application or
product acceptance follows from that scaffold.

From that exact base, Luna actor 2 owns `engine.rs` and
`transport/client.rs` (first expected reply: checked Engine/transport
candidate), actor 3 owns `harness-demo/src/main.rs`, Store and possibly
`harness-demo/src/tree.rs` (first expected reply: checked provider/Store
candidate), and actor 4 owns standalone browser acceptance (first expected
reply: expected-red test candidate). Acceptance first returned Blocked on
missing deterministic request capture; root resolved an opt-in
`HARNESS_DEMO_CAPTURE_REQUESTS` JSONL capture in the existing deterministic
transport, using canonical `request_body`. Echo selects `ask`; child selects
an empty subset. Acceptance was reassigned on that exact seam. No candidate
is integrated or reviewed yet.

The bounded check watcher was exercised on the existing transport regression
at scaffold source: underlying test selected/executed/passed 1/1, but the
watcher summary was unknown after its first evidence `cat` exited 1. A later
read of the original job's evidence succeeded without rerun. See
`docs/automation-trials.json`; that trial is not product acceptance.

Independent acceptance test `6db60d0` is merged at `b7119207`. Root
prepared web assets (web check, tests and build exited 0), then ran the
focused standalone test at that test-only source: 1 selected, 1 executed,
1 failed at the intended missing-capture product assertion. Raw evidence
is `.exomonad/build/cargo/debug/deps/focused-3g_9lckc/evidence.json`.
This is a confirmed expected-red barrier, not a green browser journey.
Root's earlier `ask` selection assumption was wrong: `CliProvider::tools`
filters it out. The actual advertised safe name is `sleep`. Acceptance
owner has the test correction; provider owner has echo `[sleep]`, child
`[]`, default Send elsewhere, and removal of duplicate EngineConfig
tool-list wiring. This is a contract correction at `b7119207`, not a
new UI command.

First implementation replies are not accepted: Engine candidate `a671079b`
has selection/body code but its focused run exited 137 before executing
and its invalid/cleanup test was absent. Provider reported nonexistent
candidate `d453d120`; its actual submitted HEAD `d453d126` has only
browser policy, not the agreed capture seam, Store restricted readback or
the echo/empty cases. Both retained owners received explicit repair
requests. Acceptance remains on its reassigned expected-red test. No
review has yet been commissioned on these incomplete tips.

Subsequent Engine repair `f616be77` passed transport serialization,
invalid-selection/pending-claim cleanup and default-auto focused checks,
each 1/1. Independent exact-HEAD review first returned `Repair` without
a defect; its clarified typed verdict accepted the same exact source,
with reviewer transport and Engine tests 1/1 each. Root merged that
reviewed tip at `37e3d4ae4587fbfd7dc774161705cd6e8ab539e1` and
reran the two changed Engine/transport focused tests there, each
selected/executed/passed 1/1. Provider policy, Store and browser green
remain open; do not mistake this component check for product acceptance.

Provider/Store final submitted candidate `b5b61f84d5b437300e391aa5111ebb1b7458de5c`
received independent exact-HEAD `Repair`: the browser hook classified
by substring over serialized *whole* request items, risking incidental
command words in unrelated content. The retained provider owner has
the precise last-user-command parser/adversarial-test repair. Its
Store, policy and capture checks each ran 1/1 at the rejected tip;
those passes do not close the review finding. The acceptance owner
corrected old evidence/Send assertions in `441e52fb`, merged at
`37f9c74f5d72b92684f34ef4a1b198ace3b02b46`; its focused test
compiled and confirmed red at the intended old-provider evidence
barrier (1 selected/1 executed/1 failed). No reviewed provider
candidate or green combined browser gate exists yet.
Repaired provider tip `59bf6236` passed its three component checks,
but fresh exact-tip review ran from its old `a61a9c6` source, which
lacks sibling Engine `f616be77`, and observed `tool_choice:"auto"`.
This is a source-composition blocker to product review, not an
identified provider-owned defect. The retained provider owner has a
safe-rebase request onto integrated `37f9c74` (preserving dirty
helpers), then must rerun affected focused checks and return an exact
new HEAD for combined review.
Rebased provider candidate `8db250a5` included Engine and passed its
three focused component checks, but independent combined exact-tip
review prepared assets and ran the production standalone browser test:
1 selected, 1 executed, 1 failed at child-empty decision assertion.
The child Engine actually invokes `child-reply`; inferring policy from
RequestPlan history misclassified it as echo-sleep. Root returned this
exact failure to retained provider owner: use explicit invocation
`command` and `agent` at `run_deterministic_engine_completion` to
choose root echo `[sleep]`, root/child request `[]`, others Send;
then new exact-tip review and browser gate. No provider tip merged.

Read [the wave15 brief](docs/wave15-brief.md). Wave14's Send-only hook is
integrated at `75a1e014d0bc3d7e8208875c84e628dcc2f4bdca`; this brief
starts from harness master `6c37e5a6bc55987190ac991117f4a773cd29c913`.
The next bounded gap is the PRD's per-request tool restriction: selected
availability reaches `tool_choice` while the full tool schemas remain stable,
and the existing standalone browser path proves the actual request and stored
decision. Root owns the small shared contract and integration, Luna owners
take bounded implementation and independent acceptance, and exact-source
review precedes final focused checks. Trial the validated background check
watcher with retained evidence once; record actual use in the automation
ledger. This planning commit has not launched wave15. The supervisor owns
launch source, runtime/workspace pin and run identities.

The completed wave14 checkpoint and chronology below remain historical
evidence. Its statement that no successor was authorized described the
wave14 close, before this new assignment.

# Completed assignment — wave14 before-request hook

## Wave14 integrated product checkpoint

The authorized Send-only before-request hook is integrated at production
source `75a1e014d0bc3d7e8208875c84e628dcc2f4bdca`. See
`docs/wave14-handoff.md` for the exact contract, review, expected-red and
final green evidence. At that source the Store, Provider forwarding, two
Engine failure paths and standalone production browser focused tests each
selected/executed/passed 1/1. Root prepared web assets, passed web 14/14
and browser journey 1/1. Port 4600 remained HTTP 200 and untouched.
No credentialed inference ran. Source evidence records the pre-existing
dirty helper README; no clean-tree claim.

All root, child and reviewer interviews are incorporated in
`docs/interviews.md`. Supervisor verified the completed launch record and raw
focused evidence; see `docs/wave14-handoff.md` and
`docs/wave14-supervisor-evidence.json`.
No reviewed/passing wave14 code branch remains unmerged after the
`6ff6339`, `d1de787` and `75a1e014` integration commits. Preserve the
pre-existing uncommitted helper README edit.

### Where the last run stopped — wave14

- No unmerged branch holds reviewed or passing wave14 code. Acceptance
  `506e1791`, Provider/Store `cb8e94ba`, and Engine `978c650e` are
  ancestors of integrated source `75a1e014`; their review/expected-red
  states and final checks are in `docs/wave14-handoff.md`.
- The three implementation actors and all three review actors were
  retired through typed group cleanup after interviews. Review actors
  reached stopped-now; the three implementation actors initially entered
  `StoppedReleasing`, then host notices confirmed actors 2, 3 and 4
  stopped with resources released.
  Retirement does not delete their branches, worktrees or evidence.
- The sole unrelated root worktree change is the pre-existing
  `.exomonad/helpers/README.md` edit. Do not fold it into product code.
- Next owner: supervisor for exact launch identities and any subsequent
  scope; no wave15 or live demo authority is inferred.

## Wave14 execution chronology (superseded by integrated checkpoint above)

- Actor-view `git status --porcelain` succeeded before forks; it reported
  pre-existing dirty `.exomonad/helpers/README.md` and
  `.exomonad/helpers/SessionHelpers/TestEvidence.hs`. No unknown-clean status.
- Shared Send-only hook contract and Engine invocation seam:
  `81ef34ce05c4c9bc823f6d495befd485055cbbc8`. Root checked
  `cargo check -p harness --lib` and the focused serde test (1 selected,
  1 executed, 1 passed). The first focused evidence was run before amend;
  a same-named notebook helper run at the amended source also passed 1/1
  but its strict cleanliness predicate was false for the dirty helper files.
- Planned wave14 provider/store, engine, independent expected-red acceptance
  children start at the exact shared contract above. Provider/Store owns
  `provider.rs`, `store/mod.rs`, `harness-demo/src/main.rs` and `tree.rs`;
  Engine owns `engine.rs`; acceptance owns
  `harness-demo/tests/standalone_browser.rs`. First expected replies:
  checked component candidates, or the acceptance test's observed red.
- Admission occurred: Provider/Store actor 2, Engine actor 3, acceptance actor
  4. Provider/Store candidate `2a9852cc6444a960010151b3d26d8b637224d498`
  passed its own focused Store and demo checks 1/1 each; independent exact-tip
  review actor 5 is pending. Engine initial candidate
  `7b5693a6cad67d6c294947dd8144a123323ab768` compiled only; root
  requested repair from retained actor 3 for cancellation and focused
  failure-path tests. Acceptance actor 4 is pending expected-red; corrected
  consumer seam: browser echo/child do invoke Engine via
  `run_deterministic_engine_turn`, and decision request IDs are Engine UUIDs,
  not the browser snapshot's UI IDs.
- Acceptance actor 4 corrected the hook discriminant to `before-request` at
  `506e1791a19e3bd7a932926ceb9654aa2119997c`; its focused standalone
  test selected/executed 1/1 and failed as expected because Store decisions
  were `[]` without the producer. This is a confirmed red barrier, not a
  reviewed or integrated green test. Provider/Store review requested typed
  Send serialization repair on actor 2; actor 3's Engine test/cancellation
  repair is also still pending.
- Provider/Store repair `cb8e94baf0cb97ffa0e3f2951d7d48b0c2e4b08f`
  passed focused Store and demo checks 1/1 each and was independently
  accepted at that exact tip by retained reviewer actor 5. Its typed
  `Send` Store serialization and historical-record readback are component
  acceptance; merge and post-merge checks establish integration separately.
- Engine candidate `8248c7965b92b8fa3b4883d68959d1871b214268`
  passed its focused 1/1 test but exact-tip reviewer actor 7 found a Store
  insert-failure cleanup omission. Retained actor 3 has the repair.
- Provider/Store candidate `cb8e94ba` was merged with the confirmed-red
  browser barrier at `d1de78746a5df69e24a458f73b9058c32cc3995f`.
  Post-merge Store typed-reopen and demo forwarding focused checks each
  selected/executed/passed 1/1 there; neither proves Engine invocation.
- Engine cleanup repair `978c650ea0fc460a609ed2a4aadf5b459b48d08a`
  passed its own Store-failure/pending-claim and transport-failure focused
  checks 1/1 each. Retained reviewer actor 7 correctly blocked because its
  checkout stayed at the old tip; fresh exact-source reviewer actor 8
  accepted `978c650e` after running both focused tests 1/1 at that tip.
  Root's `web/dist` is absent even though acceptance child's checkout had
  prepared assets; root must prepare its own assets before the production
  standalone browser check.
- Boundary: one Send decision per new transport attempt, associated with its
  durable request and agent; failed transport keeps the recorded decision.
  Readback/reopen alone must not invoke the hook. Retry is not exactly-once.

Read [the wave-14 brief](docs/wave14-brief.md) first. This branch is
`rsi/wave14` from harness master `36cdebf01f7b0792cf6828ee0e0d4ab045e3b61e`;
the pinned shared workspace is
`98aef850d7be1b322aebcea929d62de3a56ab92f`. The supervisor has accepted the matched runtime build and workspace check.
This assignment is authorized for the wave14 root. Record the matched
runtime build, final launch source, run ID, root thread and logs in
[the launch record](docs/wave14-launch.md) only after those facts exist.

The outcome is one typed, default pass-through before-request hook invoked by
the existing Engine, durably recorded with request provenance, and exercised
through the deterministic browser production consumer. Keep the browser's
existing command, child, reconnect and process-loss behavior. Root owns the
small compiling shared contract and invocation boundary before admitting three
bounded Luna obligations: Provider/Store, Engine, and independent expected-red
acceptance. Commission exact-commit review, integrate, then run focused checks
on the integrated revision.

Implementation owner and independent reviewer each trial the existing
`SessionHelpers.TestEvidence` start/wait/finish composition in one notebook
cell on the same named check. Count actual actors, model tool calls, underlying
command effects, selected/executed tests and retained evidence. Use a realistic
memory reservation; the protected port-4600 demo holds 1 GiB of the shared
8 GiB command pool. A missing Git status remains unknown cleanliness.

No Exomonad adapter, credentialed inference, real shell tools, new browser
command, or live demo/Tailscale change is authorized. The supervisor owns host launch and records its exact identities; root may
begin the brief on receiving the initial instruction.

The wave-13 notes below are history; their open host gate was closed by the
supervisor's accepted external check.

# Wave13 completed — supervisor acceptance

Production integration `d2de055` passed the root's focused checks and the
supervisor's external ordinary-host lifetime gate. See
[handoff](docs/wave13-handoff.md) and [host evidence](docs/wave13-host-gate.json).
No successor assignment is active yet. Do not start adapter or credentialed
inference work. Preserve the port4600 demo and Tailscale configuration.

The preceding working notes below retain the investigation chronology; their
open-gate statements are superseded by the accepted handoff above.

# Current handoff — wave 13 host-lifetime gate

Read [wave-13 brief](docs/wave13-brief.md) and
[wave-13 handoff](docs/wave13-handoff.md). The standalone deterministic
browser harness code is integrated. The remaining consequential gate is
an **ordinary host-shell service/PID check** outside this tool's PID/mount
namespace. The root asked the operator in [questions](docs/questions.md)
to run that isolated launch/stop/reopen and report its host-visible
identity. Do not infer that namespace-local PIDs are host PIDs.

Integrated product source tested:
`d2de0554a0e8ac18e09aabc5619b30ba31546674`.
Reviewed launch `ef86255` and reviewed fail-closed preparation repair
`2e7e6f9` are merged. Preparation
`CARGO_BUILD_JOBS=1 scripts/prepare-browser-harness` exited 0 at that
source: web tests 14/14, production browser journey 1 matched/executed/
passed, release binary staged. Focused standalone production-binary test
1/1, server 8/8 and assets CLI 1/1 also passed at that source. See
handoff for exact evidence paths, failure-path coverage and the isolated
release-script exercise.

The release-script bundle was copied to `/tmp/wave13-isolated.c0Shds` and
launched from `/` on loopback port 51807 with isolated data. Login,
command completion and WebSocket reconnect worked. After observed
process loss, the same data reopened; SIGINT to the second exact
namespace PID stopped it. Port 51807 was confirmed released. The test
secret was removed. The protected wave-12 demo on port 4600 still
returned HTTP 200 and was not touched; Tailscale Serve was not changed.
No second-device test ran.

The Haskell focused-gate experiment is committed at `fbf08e6`.
Initial 8-GiB jobs never started because the protected demo reserves
1 GiB of the shared 8-GiB pool. After a 4-GiB correction, the helper
executed on two real consumers: server 8/8 and standalone 1/1 at
`cf19ed4`. Its strict clean-source predicate remained false because
runner `working_tree_status` was null here. The direct project script
remains the final check. See [friction](docs/exomonad-friction.md).
After the Tidepool host-source mismatch was reverted, a resident
Haskell smoke cell compiled and three reviewer interviews were
collected. The finished wave-13 child groups were retired through
typed cleanup; their worktrees, branches and evidence were not deleted.

## Accepted constraints and next owner

- Keep the existing port-4600 wave-12 demo and its data, login secret
  and Tailscale route untouched. Its handoff PID is namespace-relative.
- No live Exomonad adapter inference, credentialed item-2 retry, item-13
  trace, real shell tools, new scheduler, mailbox or durable journal.
- Use an isolated loopback port and separate durable data for the
  operator's host check. Record host-visible unit/PID, readiness,
  start/stop/reopen commands and release; do not claim a second-device
  check unless it actually ran.
- Root's tool sandbox could not reach the host user bus or authenticate
  to local SSH. **Next owner: host operator** for the one external
  service-lifetime check; root can incorporate its result and update
  product acceptance. Continue from the checked source, not from a
  cancelled helper command.

## Where the last run stopped

No unmerged branch holds reviewed or passing wave-13 code. Candidate
`19a6818` was rejected for stale-artifact behavior and superseded by
reviewed, merged `2e7e6f9`; do not merge it. Four host-cancelled
acceptance/docs workers and the first cancelled reviewer returned no
candidates. Their worktrees/branches remain retained but contain no
accepted delivery. The working tree may contain documentation/checkpoint
edits; preserve them and inspect scoped Git status before any commit.
