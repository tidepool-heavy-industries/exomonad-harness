# Cache-shape probe evidence — 2026-09-24

## Outcome: Blocked

No live requests were sent. The required reference capture is missing: this
checkout contains the harness request builder and a prior narrative of Codex's
headers/body, but not an exact Codex wire capture establishing ordered header
values and serialized body bytes. The builder in
`crates/harness/src/transport/client.rs` establishes only the harness shape;
reconstructing Codex's shape from prose would not satisfy byte-for-byte
acceptance.

Accordingly, there are no new request bodies or cache counters to report. The
previous live observations recorded in `docs/findings.md` belong to different
requests and are not evidence for this probe. To unblock, supply an
appropriately redacted Codex wire capture with ordered header values and body
bytes, or authorize a different comparison criterion. See `NEXT.md` and
`docs/correction-plan.md` for the probe scope and acceptance.

No code was changed, and no tests were run.
