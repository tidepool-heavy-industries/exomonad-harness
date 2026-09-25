# Wave 0 API findings

## Pre-flight (2026-09-23)

Credential: Codex-owned ChatGPT subscription login, read only. No token value,
copy, or refresh was stored. Endpoint:
`POST https://chatgpt.com/backend-api/codex/responses`; headers included
`version: 0.155.1`, `originator: codex_cli_rs`, `Accept: text/event-stream`,
`chatgpt-account-id`, and a stable `session-id`. Bodies set `stream: true`,
`store: false`, `model: gpt-6-sol`, a stable `prompt_cache_key` equal to that
session id, stable instructions (about 1,100 input tokens), one strict
function tool, and request-level `reasoning.effort: low`.

The initial two calls returned **HTTP 200** and `response.completed`. The
first asked for a brief reply with a function tool present. The second kept
the same prefix and added an assistant-role agent envelope, a
`configuration_update` raising effort to medium, and a user request to
address the sender. Both reported `usage.input_tokens_details.cached_tokens:
0` and `cache_write_tokens: 0` (first input tokens 1,187; second 1,219).
Our initial SSE reader inspected only `response.completed.response.output`,
which was empty; that **does not establish absence of an assistant message**.

Two diagnostic calls corrected the SSE observation. The backend emitted the
assistant message in `response.output_item.done`, with
`role: assistant`, `phase: final_answer`. The first diagnostic returned
`PREFLIGHT_INSPECT_OK`. The second, with the agent envelope and positional
effort update, returned `Acknowledged, Peer.` Both returned HTTP 200; the
second again reported zero cached and cache-write tokens (1,205 input).
Thus assistant `phase` is present, the assistant-role envelope and
`configuration_update` are accepted, and the model addressed the sender.
Cache reuse **was not observed**, despite an unchanged >1,024-token prefix
and stable session/key. No 401 occurred.

### Consequence

The SSE parser must build output items from `response.output_item.done`;
`response.completed` supplies status/usage but its `output` may be empty on
this endpoint. Do not gate correctness on a positive cache counter. Retain
cache affinity and record its actual counters; investigate cache controls
separately without claiming a hit.

## Responses transport slice (2026-09-23)

Integrated `d3a0598` on `master`. The transport reads Codex-owned subscription
credentials without refreshing or storing them, sends stateless streaming
requests, and assembles output from `response.output_item.done`; completion
provides the response ID and usage. A 401 returns an authentication error.

Before integration, `cargo fmt -p harness -- --check`, `cargo test -p harness
--offline` (6 passed, 1 live test ignored), `cargo clippy -p harness
--all-targets --offline -- -D warnings`, and `cargo check -p harness --offline`
passed against the submitted commit. After integration, crate fmt, tests, and
Clippy passed again; the explicit live subscription smoke test passed and
confirmed a final-answer item plus response ID. Workspace-wide `cargo fmt
--all -- --check` is not green because of pre-existing formatting in
`crates/harness-demo/src/main.rs`, outside this slice. No cache hit was claimed
from the live smoke test. Store, agent tree, server, and web remain deferred.

## Wave 1 integration progress (2026-09-23)

The first store, agent-tree, server, and web candidates were merged on master.
These are foundations, **not yet an accepted end-to-end harness**. The store
has a reopen-tested SQLite item/request DAG, envelopes, claims, and decisions;
the tree has path parsing, mailbox rendering, in-memory async jobs/claims, and
request admission; the server has HTTP commands, SSE, WebSocket snapshot/event
frames, and static assets; the web has fixture-backed tree/timeline/inbox views
and a WebSocket adapter. The server and web initially disagreed on transport;
follow-up commits aligned their frame shapes. The Rust server is exported from
the crate, but no running driver connects transport, store, scheduler, server
commands, and a real provider yet.

After integrating the WebSocket and web repairs, `cargo fmt -p harness -- --check`,
`cargo test -p harness --offline` (27 passed, 1 ignored live test),
`cargo clippy -p harness --all-targets --offline -- -D warnings`, and
`cargo check -p harness --offline` passed. With Node from Nix and a locked
install, the web passed 5 Vitest tests, `npm run check`, `npm run build`, and
`npm audit --audit-level=moderate` (zero vulnerabilities). The production JS
bundle reported 50.61 kB gzipped. This does not establish browser accessibility,
visual quality, full live event handling, or end-to-end operation.

Independent review found that the pre-repair server/web protocol was
incompatible (now repaired), mixed-source mailbox coalescing reordered
messages, cancellation raced task-handle registration, and async-job recovery
was absent. The latter three are assigned to the tree/store owners. The
review also found command/event routes had no authorization; a server security
follow-up is pending. The demo provider candidate omitted strict tool schemas
and trusted model-supplied edit ownership; repair is pending. No wave-1
live GPT-6 run or restart/recovery acceptance test has yet been completed.

The demo provider was subsequently merged after repairing strict function
schemas and moving edit authority from model arguments to host-supplied owned
paths. A transport test now permits freeform custom tools without the
function-only `strict` field. Workspace fmt, offline tests (28 harness tests
passed, 1 live test ignored; 3 demo tests passed), Clippy, and check passed;
the demo's local zero-duration sleep smoke passed. This is a provider-unit
smoke, **not** a model run. The demo's shell result is truncated to 16 KiB per
stream after capture, so peak child-process output memory is not yet bounded.
Its ask/form tools and a real transport-backed driver remain unimplemented.

The next store/tree follow-ups are integrated. Store schema v2 records UTC-ms
timestamps, per-request usage and session state, migrates v1, and exposes
additional named queries and pending-claim recovery enumeration. Tree code
adds model-facing verb schemas/dispatch hooks, streamed-call admission
primitives, wait/output ordering, and final-answer envelope conversion. The
integration required adding `agent` to the demo's `CallContext` test. Workspace
fmt, offline tests (40 harness passed, 1 live ignored; 3 demo passed),
Clippy and check passed after that change. These are not a live agent tree:
the concrete `AgentToolService`, transport callback wiring, request lineage,
and durable job restoration remain open. An independent review's cancellation
race was subsequently repaired by registering the task handle behind a
launch gate before provider execution can begin. A deterministic
start/cancel test now covers the interleaving. After integration, workspace
fmt, offline tests (41 harness passed, 1 live ignored; 3 demo passed),
Clippy, and check passed. Cancellation still cannot undo side effects that
completed before cancellation.

Server API routes now require an explicit bearer secret and otherwise deny
access; the repaired version passed its focused authorization tests and
integrated harness tests (30 passed, 1 live ignored), fmt and Clippy. A native
browser WebSocket cannot set an Authorization header, so the shipped page
still cannot reach the secured API directly. Operator login/session versus a
trusted authenticating proxy remains an explicit delivery decision.

An optional local browser-session API was integrated. It exchanges a
separate operator secret for a short-lived, HttpOnly, SameSite=Strict cookie
with same-origin login/command/WebSocket checks; bearer-only fail-closed mode
remains available. Integrated tests covered unauthorized and authorized
routes, expiry, logout, and cross-origin rejection. Workspace fmt, offline
tests (56 harness passed, 2 live ignored; 3 demo passed), Clippy and check
passed. This is server capability, not an enabled deployment: the demo has
no `server_with_config` startup yet, and the web login UI is still pending.
Sessions are in-memory and do not survive server restart. Login rate-limiting
and external proxy identity enforcement are not implemented.

The demo CLI now calls the library `Engine` instead of maintaining a second
request loop. Its single-agent `--db … --ask …` path was verified live with
read-only subscription auth and returned `DEMO_CLI_OK` (final response usage:
118 input, 9 output tokens). The temporary SQLite file was removed afterward.
Workspace fmt, offline tests (56 harness passed, 2 live ignored; 7 demo passed),
Clippy and check passed. The CLI deliberately hides unimplemented ask/form
tools, leaves shell opt-in, and denies edits without host-supplied ownership.
It does not resume an old conversation, supervise child agents, or serve web;
the displayed usage is for the final response, not an aggregate.

The first request engine is now integrated as `harness::engine`. Its offline
replay tests cover stateless full-history input, separate durable request
rows/usage across turns, ordered call/output replay on reopen, dispatch from
`response.output_item.done` before completion, malformed calls, and cleanup
on transport error/cancellation. Root integrated **only** the child's engine
files to avoid reverting newer store/tree/server repairs from its branch.
Integrated workspace fmt, offline tests (51 harness passed, 1 live ignored;
3 demo passed), Clippy, and check passed. A local integration fix confined a
test MutexGuard before an await. There was no live model run.

The engine still waits for all pending jobs at the end of each response rather
than allowing the model to continue with outstanding async calls or to pause
through `wait_agent`. That is a PRD acceptance blocker, assigned for a
follow-up engine revision. It also is not yet connected to the server command
channel or a concrete tree driver.

The async follow-up was integrated after review. It tracks pending calls,
continues a request without awaiting slow jobs, handles `wait_agent` as a
withheld call output, appends settled outputs before the wait status, and
wakes from a late settlement or mailbox envelope. Controlled offline tests
cover these paths. Integrated workspace fmt, offline tests (54 harness passed,
1 live ignored; 3 demo passed), Clippy and check passed. This narrows the
loop gap, but it does not establish a live subscription conversation, durable
job restoration after process restart, or a running multi-agent driver. The
agent-runtime implementation and server/browser authentication connection
are still pending.

An initial `StoreAgentToolService` candidate was reviewed but not integrated.
It traversed `requests.parent_id` as if it were agent ancestry; the engine
uses those rows for each model call, so that conflates two different trees
and cannot provide a stable current head for `here` forks. Its rendered
agent envelopes also used a user role rather than the required assistant
role. A separate durable agents table and API has been assigned before
runtime integration. The CLI/demo provider still has no process-resume path.

An explicit ignored live smoke test subsequently ran the integrated engine
against the ChatGPT-subscription SSE endpoint with read-only CodexFileAuth.
It returned a final-answer item in 3.77 seconds. This verifies one simple
stateless request through engine, transport and store; it did not exercise
tool calls, forking, wait/resume, browser delivery, or cache hits. The test
remains ignored in ordinary CI, and no credential value was emitted.

The optional browser login UI is integrated. It probes `/api/session`, signs in
with same-origin credentials, holds the operator secret only in React form
state, and opens the WebSocket only after authentication. The production UI
starts with no fixture data and remains in a loading state until an
**authoritative** WebSocket snapshot arrives; fixture state remains available
only to tests. Integrated web checks passed: 10 Vitest tests, TypeScript check,
production build, and npm audit (0 vulnerabilities). Rust workspace fmt,
offline tests (56 harness passed, 2 live ignored; 7 demo passed), and Clippy
also passed. No live browser-to-server run has been made, and the demo CLI does
not yet start the server. Browser sessions remain optional and in-memory.

Durable agent identity is now a distinct v3 `agents` table, not an inference
from `requests.parent_id`. The table stores canonical path, parent edge,
nullable current request head, contract/source, state and creation time.
Prompt-fresh agents can have a NULL head; a compare-and-swap advances it.
Focused tests cover NULL head/CAS, path rejection, list/children queries and
v1/v2 migration without rewriting v2 timestamps. After store-only extraction
from the reviewed candidate, integrated Rust checks passed: fmt, offline
workspace tests (58 harness passed, 2 live ignored; 7 demo passed), Clippy and
check. This is storage capability only: the agent runtime and process-resume
supervisor are not yet integrated. The existing verb schema currently uses
single-segment child paths despite PRD's doubled-path example; this contract
is under operator clarification.

An atomic store operation now admits a child agent together with its initial
NEW_TASK envelope in one SQLite transaction. A forced envelope-insert failure
rolled back both the agent row and content write in the focused test, closing
the orphan-child crash window found in runtime review. Integrated fmt, offline
workspace tests (59 harness passed, 2 live ignored; 7 demo passed), Clippy and
check passed. The runtime must use this operation; its current candidate is
not yet integrated.

The engine now exposes a successful-completion API with the full persisted
ordered transcript and final request head, while retaining the original
`run` API. A focused recorded tool-call test verifies the initial input,
function call, matching output and final item appear once and in order.
Cancellation/failure still returns an error, not a mislabeled complete history.
The child branch did not export engine, so its reported checks did not compile
this code; root restored the existing ignored live smoke test, fixed a test
MutexGuard lifetime, and ran integrated fmt, offline workspace tests (60 harness
passed, 2 live ignored; 7 demo passed), Clippy and check. The demo server must
consume this API before claiming multi-command continuity.

The demo binary now has an opt-in loopback-only `--db … --serve
127.0.0.1:<port>` driver. It requires a separate operator session secret from
`HARNESS_DEMO_SESSION_SECRET`, serves built `web/dist`, queues one `/root` turn
at a time, publishes web-contract records, stores the complete successful
engine transcript, and marks interrupted in-flight records on restart. The
browser-facing snapshot is a minimal one-agent view, not a full reconstructed
tree/event log. The driver refuses non-loopback plaintext binds and missing
web assets. Integrated fmt, offline Rust tests (60 harness passed, 2 live
ignored; 12 demo passed), Clippy/check, and web tests (10), TypeScript check
and production build passed. A live local server smoke used read-only
subscription auth: unauthenticated POST was 401; browser-session login and two
authenticated command POSTs succeeded, yielding `SERVER_FIRST_OK` then
`SERVER_SECOND_OK` with four persisted history items. Ctrl-C exited cleanly;
the temporary SQLite database was removed. This verified HTTP command/session
and sequential model continuity, **not** a real browser/WebSocket round trip,
streamed token deltas, or child-agent supervision. Local cookie sessions do
not survive restart; HTTPS/reverse-proxy serving is not configured.

`harness::agent_runtime::StoreAgentToolService` is now integrated as a scoped
Store-backed implementation of the model-facing agent verbs. Offline tests
cover prompt/Here/checkpoint heads, atomic NEW_TASK admission with the correct
AtBoundary delivery class, subtree authorization, checkpoint corruption,
message/follow-up persistence, and explicit refusal of service-local wait
(which the Engine manages). Integrated fmt, offline workspace tests (66 harness
passed, 2 live ignored; 12 demo passed), Clippy and check passed. This is a
library service, **not** a running agent tree: the demo provider intentionally
hides agent verbs, the demo driver supervises only `/root`, and no driver yet
reconstructs forked prefixes or starts idle-agent follow-ups. The stored
child-path form is immediate single-segment parent/task, pending resolution of
the PRD's doubled-path notation. The service's generation watch is local to
its instance and only hints the host to rescan the durable table.

A later live loopback WebSocket smoke also passed with the demo server and
read-only subscription credential: browser-session login set an HttpOnly
cookie, the authenticated `/api/ws` connection received an authoritative
snapshot and command acknowledgement, and a submitted command produced a
`FINAL_ANSWER` envelope carrying `WS_LIVE_OK`. The server exited cleanly and
the temporary database was removed. This exercised the server/WebSocket wire
path, not browser rendering or a supervised child tree.

The engine now has a branch-start API accepting an optional durable request
head and only new input items. The new request points to that head; replay
loads the parent chain once, avoiding copied-history duplication. Tests cover
fresh start, missing head refusal, and two sequential requests with exact
parent links and stable session key. Root preserved the ignored live smoke test
that was absent from the child's older branch and ran integrated fmt, offline
workspace tests (69 harness passed, 2 live ignored; 12 demo passed), Clippy
and check. This is a necessary driver primitive, not a complete Here or
checkpoint fork: the host must still choose a committed source boundary and
route envelopes safely.

The Store can now atomically append one agent's unread envelope items to a
request for that **same agent** and mark those envelopes delivered. Tests
verify arrival order, idempotent retry, preservation across reopen, rollback
on failed insert, refusal of missing requests, and cross-agent branch
isolation. Integrated fmt, offline workspace tests (71 harness passed, 2 live
ignored; 12 demo passed), Clippy and check passed. No engine or demo driver
calls this API yet, so live mailbox delivery/recovery remains unverified.

The durable branch-start Engine APIs now consume that Store inbox operation
before their first model request. Replay tests verify a prompt-fresh child sees
its NEW_TASK once, a subsequent request does not repeat it, and supplied new
items precede queued inbox items. Legacy `run` APIs retain prior behavior.
Root preserved the ignored live smoke test from the newer baseline and ran
integrated fmt, offline workspace tests (73 harness passed, 2 live ignored;
12 demo passed), Clippy and check. Mid-run mailbox wake still uses an
in-memory channel and is not yet coupled to durable envelope acknowledgement.

The Store-backed verb service now emits an in-process mailbox-generation hint
after a committed spawn, message, or follow-up envelope. A host subscribes
before scanning durable unread rows and rescans on generation changes; tests
cover all three writes and ensure rejected writes do not notify. Integrated
fmt, offline workspace tests (73 harness passed, 2 live ignored; 12 demo
passed), Clippy and check passed. This hint is local to one service instance,
not a cross-process event log; durable rows remain authoritative.

Durable branch-start Engine runs now accept post-commit mailbox wake hints.
Before every model request they attach queued unread envelopes transactionally;
when `wait_agent` wakes, completed function outputs and the wait status are
committed before unread envelopes. The in-memory hint is not copied into the
request a second time. Focused replay tests cover ordering and deduplication;
legacy transient mailbox mode remains and its wait ordering was corrected.
Root retained the ignored live smoke test from the newer baseline and ran
integrated fmt, offline workspace tests (75 harness passed, 2 live ignored;
12 demo passed), Clippy and check. A one-process driver must still persist an
envelope before signaling and rescan durable inbox on restart; none is wired
yet for child agents.

The demo now contains a tree-provider adapter exposing Store-backed agent
verbs alongside development-gated ordinary tools. Focused tests verify
prompt spawn persists a child and NEW_TASK, messaging persists an envelope,
unrelated agent context is refused, and uncommitted Here/checkpoint forks
leave no Store mutation. The adapter explicitly refuses those fork sources;
it is not yet wired to a supervising tree driver. Integrated fmt, offline
workspace tests (75 harness passed, 2 live ignored; 15 demo passed), Clippy,
and check passed.

The opt-in `--tree` CLI now composes the Store-backed agent verbs with a
one-process prompt-fork driver. A deterministic offline replay executes the
real Engine, TreeProvider, Store, and ResponsesTransport seam through
root spawn → root wait → child FINAL_ANSWER → root final answer. Tests assert
one durable child NEW_TASK, post-commit parent wake, wait-status-before-inbox
ordering, exact branch heads, no spurious request, task reaping, cancellation
drain, and fail-closed no-replay on a stored head. The default CLI/server remain
single-agent. Tree CLI requires a fresh database; process restart/resume,
Here/checkpoint, agent worktree isolation, and browser tree state are not
implemented. The adapter exposes `run` only with the explicit development
shell opt-in; it is not a multi-file task harness with per-agent checkout
ownership.

An opt-in live subscription SSE smoke, using read-only `~/.codex/auth.json`,
returned `TREE_VERIFIED_BLUE`. Inspection of its temporary SQLite database
before removal found `/root` and `/root/helper` agent heads, three root
requests and one child request, a delivered child NEW_TASK, and a delivered
child FINAL_ANSWER to root. This verifies a live prompt-fork/wait/answer path,
not crash recovery or worktree isolation.

## Correction-wave cache-shape probe

The bounded probe was **Blocked**, not accepted: no new live requests were
sent, no redacted request bodies were captured, and no new cache counters
were measured. The exact reference capture of Codex's ordered headers and
serialized body bytes was unavailable; reconstructing it from a narrative
would not establish byte-for-byte parity. Root incorporated the probe's
evidence as `cdbbd367` (`docs/cache-probe-evidence.md`), then integrated it
on master in `b99337e`. Prior counters above are from different requests and
must not be counted toward this probe.

Q4 subsequently replaced byte parity with a two-request counter measurement.
On source `d0245b3177afa21556c041ce05296a96b50b209a`, the bounded probe
made exactly two sequential requests through the production
`ResponsesClient::create` builder with a common key and >1,024-token stable
prefix. Redacted `usage` blocks reported 20,923 input tokens on both requests
and `cached_tokens` of 0 then 20,736. Thus the second request **did** report
cached input tokens. The exact blocks and method are in
`docs/cache-probe-evidence.md`. This is a finding for that pair, not a general
cache guarantee or product approval; per Q4 no Codex field-order diff was
required. No code or broad tests changed.

## Correction-wave item-2 live trace gap

The bounded item-2 probe sent **no** inference request. Inspection at
`d0245b3` found the slow demo `sleep` and async tool schema but no supported
surface to retain redacted outbound request bodies correlated with sleep-job
timing and `wait_agent` resumption. A prompt-driven run without that evidence
would spend inference without proving the acceptance. Details:
`docs/item2-live.md`. Live item 2 is still open; this is not a passing trace.
