# Execution observations

The browser contract currently exposes native workbench posture, exact human-form
waits, and pending-form discovery. It does not yet provide a complete execution
trace, compiler phase stream, or per-effect timing history.

Rust owns the DTOs in `server/ws_protocol/contract.rs` and
`contract_extensions.rs`. The browser types and validators are generated from
those DTOs. All native identities, revisions, counters, ordinals, and timestamp
fields in these projections are decimal strings.

## Actor observations

`HostActorProjection.workbench` is the actor's last sampled workbench posture:
`idle`, `running_unit`, `awaiting_effect`, `terminal_transfer`, or `failed`. It
does not enumerate simultaneous executions. Its operation label comes from the
native effect boundary; consumers must not parse that text to infer control or
outcomes. Inference recovery is independently carried by `inference`.

`formWaits` is optional. Absence means the host does not report form waits. A
present sample contains up to 64 independently owned active waits and an
`omitted` count for additional concurrent waits. Each wait identifies its exact
mount, native notebook execution/input unit/effect when available, and the time
the interpreter began waiting. `sinceUnixMs` is UTC Unix milliseconds, measured
by the host. Browser elapsed counters based on it are estimates; it is not a
host-measured effect duration. Repeated observations retain the same start.

Opening a form is not evidence that an interpreter is waiting. The native
`FormAwait` service issues a wait only after an authoritative attempt read
returns pending, and removes it when that await returns or is dropped. Distinct
waits on the same mount retain independent lifetime guards. Actor-program
effects currently retain their explicit actor owner but do not have stable
per-effect execution identity; they must not attach to a nearby notebook cell.

## Pending forms

`GET /api/actor-form/pending?run=...&actor=...&incarnation=...&after=...&limit=...`
reads the durable form owner, independently of loaded chat history. `after`
defaults to 0; `limit` defaults to 32 and accepts 1 through 64. Each page has an
exact origin, a durable `revisionSequence`, compact form records, and an optional
`nextAfter`. Forms and array metadata are bounded to 64 KiB per native page.
Invalid cursor/limit/origin is refused; an unavailable Store returns HTTP 503.
Read failure returns an error, never an empty successful page.

Each record carries the mount, opening journal `sequence`, latest revision,
execution association, optional issued conversation identity, and `open` or
`submitted` state. The opening journal position is the exact chat locator. It
does not require matching labels, source text, paths, or nearby timestamps. The
index excludes form descriptors, drafts, answers, and diagnostics.

`submitted` means awaiting native processing. Only native rejection, commitment,
closure, or restart changes that state. A pending-only index does not establish
the outcome of an absent mount. Retrieve the exact retained mount through
`GET /api/actor-form?run=...&actor=...&incarnation=...&mount=...`; it returns the
canonical full form, including terminal state. An unavailable mount returns HTTP
404. Form retention follows the existing durable Store and has no new expiry
policy.

Pages are current reads, not a frozen multi-page snapshot. Their revision is the
latest durable revision of **all** forms for that actor incarnation, including
settled forms, obtained in the same Store read transaction as the page. Consumers
must restart pagination when revisions differ. Only a complete enumeration at
one revision authorizes removal of previously pending entries, and removal does
not imply a successful answer.

## Live updates and reconnect

Submission and dismissal publish the committed record immediately as the existing
`actor.form.changed` event. Native transitions use the same stream. Each mount's
durable revision fences older observations; transport sequence establishes
delivery order, not native settlement order.

The event sequence and `Snapshot.actorFormRevisions` watermark commit under the
existing `ServerControl` sequence lock. Each origin's watermark only advances.
Watermarks retain at most 128 origins and 64 KiB, independently of durable forms.
Clients fetch the pending index on actor selection and reconnect even when that
origin has no loaded watermark. The existing stream resynchronizes through a
snapshot after a sequence gap or broadcast lag; browser slowness never awaits
native evaluation. Revisions and actor origins distinguish incarnations.

All new reads and submissions use the existing authenticated API policy. No
credentials or provider request payloads are added to these observations.

## Remaining instrumentation

Compiler phases, complete effect lifetimes, attempt/source-occurrence correlation,
non-form wait reasons, measured queue/compile/evaluate/suspended durations, durable
trace pagination/expiry, and instrumentation overhead are not provided by this
slice. Their absence must remain unavailable rather than zero, success, or
invented progress. The native execution journal, admitted compiler timing owner,
and immutable terminal receipt remain the owners for a future trace projection.
