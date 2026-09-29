# Embedding-ready implementation handoff

Accepted standalone embedding batch, prepared on `integration/embedding-ready`
for fast-forward into master. No deployment or Tidepool pin.
Base: `0fbcbbbb2630ae59ad75e18ab797b1e11ec0860a`.

## Retained work joined

Production baseline imported from wave22 root `619519b309016c9f205bf66d4faec61744c5cff4`,
preserving current prompts, workspace pin and adoption documents. Product candidates:
provider `880dc75`, Engine `572cb6b` plus recovery `87bf658`, Store `37a4653` and
`2baaf36`, replay `6f6d625` and fixture `3f0752e`, web `517c70b`, server `5d503b4`,
browser journey `62ac759`. Original branches and worktrees remain intact.

Join repairs: request-scoped replay no longer falls back to global call identity;
replay supports raw custom dispatch; retained fixtures compile on the joined APIs;
cancellation fixture verifies durable cancellation output rather than an obsolete
interrupted-claim state; browser pending wait uses the published pending request;
restart tests reauthenticate; failed demo requests advance their durable branch
head before subsequent work. These repairs have production-path regression coverage.

## Initial convergence evidence

- `cargo test -p harness --lib`: 182 passed, 2 ignored.
- Web Nix shell: npm ci, check, test (18 passed), build passed.
- Counted `browser_journey` target: 4 matched/executed/passed after join repairs.
- Other targets passed during workspace traversal; final combined suite remains
  a release gate after embedding changes. No live provider or Haskell execution.

## Implemented public interfaces

- `embedding::{HostActor, AdmissionGuard, HostIdentity, Conversation, ToolSurface}`:
  host capabilities admit exact incarnations. `Conversation::attach` binds history;
  `from_checkpoint` atomically registers checkpoint history and the host binding.
  The host guard protects admission from retirement. The harness does not invent
  a host or supervise it. Low-level Store registration remains a trusted host API.
- `Conversation::engine` binds identity/tools and selects plain-text compaction.
  `Provider::request_snapshot` retains the immutable manifest and dispatcher for
  every call from an issued request. Engine records the surface version, persists
  each claim before execution, and validates a call before interpreting reserved
  operations. The bound provider checks exact durable call identity/input/version.
- `Conversation::input` commits an operation-ID-deduplicated envelope before wake.
  `InputReceipt` distinguishes durable admission from wake failure;
  `input_observation` reports actual request inclusion. Retry preserves its original
  envelope. Typed live host mailboxes remain outside this API.
- `CancellationOwner` reports `Stopped` or `Unconfirmed`. Uncertainty retains the
  waiter. `JobScheduler::provider_completion` preserves late completion separately
  from the immutable emitted result; acknowledgment inspection is passive, while
  `retry_cancellation` explicitly requests another owner action.
- `Store::{capture_checkpoint, attach_checkpoint_child}` and `Checkpoint<T>` retain
  an immutable prefix, original pending claims and an `Arc<T>` host attachment.
  A child can start before the source call returns. Multiple children choose their
  own checkout. Failed or cancelled source work does not invalidate the handle;
  shared call cancellation is observed by every claimant, not cloned or replayed.
  Reopening Store cannot resurrect a live attachment.
- `Engine::with_plain_text_compaction(NonZeroU64)` triggers near half the configured
  capacity. Pending raw/function calls remain exact, outside summary text. Summary
  tool calls are refused. Failed/no-progress attempts retain history and require
  meaningful growth before retry; request IDs, usage and outcomes are recorded.
- Existing `server_with_config` now includes protected paged request history.
  `Snapshot.actors` projects host model/workflow incarnations independently from
  conversations. Browser request inspection renders retained Items and explicitly
  identifies oversized evidence by hash rather than treating it as empty output.

Schema version 4 migrates existing stores transactionally, adding checkpoint,
embedded-binding and input-deduplication records. An incompatible future schema
is refused. The host still owns its private durable root and live authority.

## Combined offline acceptance

Source for the final complete behavioral and lint gate: `fca2693` and its
ancestors. Documentation-only handoff changes follow that revision.

- `CARGO_TARGET_DIR=/home/inanna/dev/exomonad-harness/target cargo test --workspace`:
  **312 passed, 2 pre-existing ignored, 0 failed** across all Rust targets.
  Includes 5 external-host tests, 6 cancellation tests, 5 plain-text-compaction
  integration tests, 4 production browser journeys and the standalone reopen test.
- `nix develop .#web -c bash -c 'cd web && npm run check && npm test && npm run build'`:
  **21 web tests passed**, TypeScript check and matched production assets built.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed with the same
  shared target directory. `cargo fmt --all` and `git diff --check`: passed.
- The public external-host consumer covers raw Unicode, successful structured
  input, wrong kind/actor refusal, original input retry, failed wake, request
  inclusion, handler reload, checkpoint admission refusal and authenticated
  browser reconnect without re-execution. Existing progress tests cover overflow.
- Auth file tests exercise missing/malformed/unsupported/empty credentials,
  secret-free errors, read-only behavior and externally refreshed credentials.
  No real credentials or provider requests were used.

## Explicit limits and next owner

Tidepool consumes this library after its engine hold clears. No runtime/compiler
code, workspace pin, deployment, running session or Codex backend was changed.
Real resident cells, source leases, typed live values and the full reviewed live
worker tree remain joint integration gates. The two ignored tests are opt-in
live-provider probes, not evidence for async provider behavior.

The scheduler currently requires call IDs unique within its shared instance;
collisions are refused before new work starts. Qualifying operation identity
across conversations/requests is required follow-on harness work before shared
embedded use; inherited claims must still resolve their origin operation.
Checkpoints share the original
call, not an independent effect. Process-local cleanup evidence must be projected
and retained by the embedding owner when needed after host loss; durable model
history alone never establishes resource cleanup. History items larger than the
browser page budget remain in Store with a visible hash; a large-artifact download
route is deferred. Browser protocol tests use actual HTTP/WebSocket routes and
deterministic model transport; they are not a live provider or full browser-engine
performance measurement.
