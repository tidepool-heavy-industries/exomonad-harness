# Chat usability direction

This brief records the operator's choices for the usability pass based on
`bf11247b5fb881a5ba6624a0ab2e29b8f3adb8a4`. It supersedes the older monochrome,
13px transcript, and non-chatbot direction in `web.md` for the chat surface.

- Audience: the daily technical operator. The conversation is the primary working
  surface; code and execution are first-class content, not hidden debug metadata.
- Visual system: Live Patina — High Current, approved by the operator on
  2026-10-12 and implemented with petrol surfaces, verdigris primary actions,
  ivory text, sage metadata, and brass attention cues. Solarized and Catppuccin
  remain historical comparisons. [DESIGN.md](../DESIGN.md) records current roles;
  the [style bible](gui-style-bible.md) retains broader proposals. Chat prose is
  16px with a comfortable measure; code is 14px in the existing monospace stack.
  The existing IBM Plex Mono choice is now self-hosted as regular and semibold
  WOFF2, with upstream source and license retained; no new typeface choice,
  animation libraries, or runtime dependencies. Palette contrast
  and browser acceptance need evidence for this implementation.
- Reading: full prose, bounded code/results, full-source copy. Reasoning and
  technical identifiers remain available through disclosures. Streaming and
  retained versions preserve row identity and disclosure choices. An active text
  selection suspends automatic following; the user can explicitly jump to activity.
- Execution: code/arguments and results are visible together. An expandable log
  beneath each cell shows observed publication and exactly correlated job state
  and timing. Missing results never imply completion or ongoing execution. Compiler
  phases and a complete effect log require the separate telemetry PRD.
- Forms: prominent inline controls, a sticky jump indicator for known open forms
  in loaded history, retained drafts, field-associated validation and deliberate
  focus after submission. Submitted values read as an answer; full form details
  remain expandable. No background focus stealing.
- Layout: retain worker navigation and the anchored composer. At short viewport
  heights (including zoom), let the page scroll so the composer cannot cover an
  unusably small transcript. Horizontal code scrolling remains keyboard reachable.
- Delivery: direct improvement of the current chat, followed by user iteration.
  No variant picker, deployment, pin changes, or cancellation repair in this pass.

Design references: Impeccable `shape` and Operate/Read guidance; Emil Kowalski's
interaction guidance; Vercel Web Interface Guidelines for review. The operator's
brief takes precedence over generic skill defaults.

Pending-form counts describe loaded history, not the actor's complete outstanding
work. An authoritative pending-form index, per-effect logs, and compiler phases
are requested in `gui-execution-telemetry-prd.md` for coordinator implementation.
