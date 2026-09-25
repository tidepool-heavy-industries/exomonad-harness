//! Opt-in evidence seam for the single manual PRD item-2 live run.
//!
//! TODO(correction-wave b, trace): record redacted bodies made by the
//! production request builder, plus timestamped sleep start/settle and
//! `wait_agent` observations. Never record credentials or unredacted prompt
//! content. A trace write failure must fail the opted-in run rather than
//! silently turn missing evidence into acceptance. Tests are offline.

// The trace leaf owns this module. Root owns the `mod` line and CLI/provider
// consumer wiring. Keep this compiling stub until the leaf returns a checked
// implementation and its exact public signature.
#[allow(dead_code)]
fn trace_seam_pending() {
    todo!("PRD item 2: opt-in redacted request/job trace")
}
