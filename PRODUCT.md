# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

The daily technical operator is the primary user for this GUI evaluation. Desktop
is the primary working environment; narrow layouts and browser zoom must remain
usable. The broader product audience remains an open decision.

## Product Purpose

The GUI must succeed as a daily agent workspace without limiting its broader
purpose to that role. Users read streaming answers and Haskell, inspect tool
execution, respond to inline forms, and steer agents while retaining context.

## Operating Context

The conversation is the primary working surface, with worker navigation and
supporting tree, timeline, inbox, and recovery views. Frontend iteration must be
possible with synthetic browser fixtures while the full engine is unavailable.
Fixture evidence does not establish live engine correctness.

## Capabilities and Constraints

Code and tool results are first-class content. Diagnostic metadata remains
accessible through disclosure. Preserve drafts, selection, reading position,
keyboard access, deliberate focus, and readable submitted responses.

Execution labels must reflect observed evidence: retained output does not by
itself establish successful execution. Missing telemetry is a separate engine
or coordinator dependency, not something the GUI may invent.

## Brand Commitments

Retain the established Solarized Dark identity and current font stacks. The
existing chat brief in docs/gui-usability-brief.md takes precedence over the
older non-chatbot direction in docs/web.md.

## Evidence on Hand

The production GUI and synthetic browser fixtures are recorded at commit
715e111221f8841cb7ac3a95c27ce7b66f1ff89c. Existing screenshots include desktop,
narrow layouts, 200% zoom, forms, code, failures, and recovery. The implementation
ownership guide is docs/gui-structure.md.

## Product Principles

- Make the ongoing conversation understandable while execution continues.
- Keep required responses discoverable and attached to their context.
- Preserve user control of reading, drafting, and focus.
- Communicate only states supported by evidence.
- Keep technical depth available without making it dominate routine use.
