---
target: chat window utility after items 1–4
total_score: 28
max_score: 40
na_heuristics: 
p0_count: 0
p1_count: 0
target_identity: "file:/srv/swarm/checkouts/harness-chat-critique-2-20261012/web/src/UnifiedChat.tsx"
target_fingerprint: "sha256:7059acae74e0819f538d7493bdba5af1e82dea12271c1b34122f4574fc6c803c"
target_path: /srv/swarm/checkouts/harness-chat-critique-2-20261012/web/src/UnifiedChat.tsx
timestamp: 2026-10-12T21-57-10Z
slug: web-src-unifiedchat-tsx
---
Method: dual-agent (A: /root/critique_design_2 · B: /root/critique_evidence_2), both GPT-6.1 Sol.

Reviewed `web/src/UnifiedChat.tsx` and surrounding GUI at `a93815386c18812705ba54f30cfb6d746bc761b4`. Solarized Dark and existing font stacks remain fixed. Synthetic production-bundle fixtures, not a live engine.

## Design specificity and overall impression

This feels purpose-built for an agent workspace: worker identity, Haskell, paired tool results and inline forms belong together. Readable prose, explicit recipients and honest state language are strong. The next opportunity is faster comprehension of tool results and less diagnostic clutter, rather than a visual redesign.

## Design health

| Heuristic | Score | Main observation |
|---|---:|---|
| System status | 3 | Availability is clear; tool-result meaning requires reading deeper. |
| Match to user language | 3 | Appropriate technical vocabulary, but publication/exchange distinctions need interpretation. |
| Control and freedom | 3 | Explicit jumps and retained drafts; keyboard focus can expand source automatically. |
| Consistency | 3 | Cohesive controls; overlapping details disclosures. |
| Error prevention | 3 | Disabled unavailable actions and clear recipient identity. |
| Recognition over recall | 3 | Pending forms visible; diagnosis spans multiple disclosures. |
| Efficiency | 3 | Shortcuts, copy and search help; inspection adds navigation overhead. |
| Minimalist design | 2 | Repeated metadata and persistent narrow-screen infrastructure. |
| Error recovery | 3 | Draft survives synthetic disconnect; compiler message gives actionable context. |
| Help | 2 | Useful local hints; no concise explanation of delivery/publication terminology. |
| **Total** | **28/40** | **Good; focused utility improvements remain.** |

These are qualitative review scores, not an accessibility certification or test result.

## What works

- Pending forms have a visible count, direct jump and deliberate field focus.
- Prose, source and results remain readable, with full-copy and expansion controls.
- A typed draft survived a synthetic disconnect. Retained content remained visible, and output availability was not falsely presented as successful execution.

## Priority issues

1. **[P2] Tool results need a useful summary beside their availability state.** A compiler-failure result still heads the card with “Result retained”; an operator must scan source/results to discover the problem. Keep that honest publication state, but bring a short result excerpt near the header. Prefer structured outcomes when the host supplies them; never infer authoritative success or failure from arbitrary output text. Suggested flow: **Impeccable clarify**. Evidence: `ToolActivity.tsx`, desktop code/log captures.
2. **[P2] Consolidate repeated diagnostic disclosures.** A single answer has Message details plus Publication details; tool cards add Result details, Execution log and Tool details. The operator must decide which contains the needed evidence. Use one predictable evidence disclosure per conversation object, with labelled internal sections; keep meaningful execution status and copy controls directly available. Preserve result-position navigation and exact provenance. Suggested flow: **Impeccable distill**. Evidence: `UnifiedChat.tsx`, `MessagePresentation.tsx`, `ToolActivity.tsx` and desktop captures.
3. **[P2] Protect narrow-screen reading space during forms and disconnects.** At 390×844, the shell, pending banner and composer consume a substantial share of the viewport; additional disconnect messaging makes it tighter. This is cramped reading, not an established inability to submit. Explore a compact composer when reading or answering a form, preserving the draft and an obvious way back. Consolidate redundant status messages and verify form actions remain fully reachable. Suggested flow: **Impeccable adapt**. Evidence: 390px form/code/disconnect captures; estimates are visual, not new DOM space measurements.

No P0/P1 was established in this bounded review.

## Cognitive load and emotional journey

Moderate load: narrow layouts split attention between an inline form and full composer; tool inspection exposes six or more nearby affordances. The form itself offers only two choices, and top-level navigation has four: neither is inherently overloaded. Reading and form focus feel calm and purposeful. Compiler diagnosis and disconnect warning stacks are the low points; preserved drafts provide reassurance.

## Persona red flags

- **Power user:** provenance lookup requires choosing among similarly named disclosures. Keyboard focus can expand bounded source without an explicit Expand action; this is source-observed, not a demonstrated scroll failure.
- **Keyboard/low-vision operator:** narrow layouts leave less context around required actions. Form focus works. This pass did not execute screen-reader or actual 200% zoom testing.
- **Interrupted mobile user:** drafts survive the inspected disconnect, but warnings take space from the retained conversation. Fine-pointer narrow captures do not establish a touch-target defect; coarse-pointer 44px CSS protections exist.

## Detector/browser evidence

CLI scan of `web/src` returned four warnings: accent borders at `ActiveWorkerTree.css:15`, `SessionView.css:8`, `tokens.css:71`, and Inter at `tokens.css:4`. These are semantic selection/status accents and pinned typography, not reasons to redesign the interface.

Injected browser scans ran in three fresh headless tabs and reported 72, 72 and 73 node-level findings. Repeated gray-on-color, palette/font, line-length, edge-spacing and occlusion heuristics require interpretation; they are not that many independent defects. Solarized/font warnings conflict with the brief. No horizontal document overflow appeared at 1440, 1280 or 390px. Generic scrollIntoView screenshots showed clipping, but the intentional Jump to form flow was separately observed to focus and reveal its heading; no regression is asserted from the generic scroll probe. These scans reinforce inspecting density and readability, not changing the pinned palette.

## Minor observations

- Keep “in loaded history”: it correctly limits the pending count's claim.
- Make horizontal source overflow easier to discover without wrapping or altering copied source.
- Explain delivery/publication vocabulary on demand rather than adding permanent warning text.
- Optional resource 500 responses were retained by B; their cause was not established, so they are not promoted to a product defect.

## Questions to consider

- Which helps daily operation most: tool summaries, unified evidence inspection, or compact narrow reading?
- Should the next batch focus on the two desktop chat improvements, or include the narrow-layout experiment too?
