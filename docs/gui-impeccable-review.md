# Impeccable GUI evaluation — 12 October 2026

Method: dual-agent (A: `/root/design_review` · B: `/root/technical_review`). Assessment A finished before the parent read detector findings. Both reviewed source and independently opened fresh Chromium contexts. Impeccable v4.3.1; evaluation only.

**Recommendation:** make the active conversation or required response occupy the visual foreground. Start with narrow task space, recovery/recipient clarity, and control readability. Preserve the current Solarized identity and the interaction safeguards already in place.

Reviewed application revision: `715e111221f8841cb7ac3a95c27ce7b66f1ff89c`. Documentation branch: `gui/impeccable-review-20261012`. The GUI must succeed as a daily agent workspace without limiting its broader purpose to that role. Controls use Operate priorities; the transcript uses Read priorities. All recommendations below are proposed changes, not implemented fixes.

## What is working

The interface is recognizably built for this product: actor lineage, Haskell cells paired with results, retained evidence, inline forms, and deliberate recovery. It does not need a new visual identity. Full 16px prose, bounded source with full copy, readable submitted responses, and honest execution labels are strong foundations.

Fresh tests preserve draft state, deliberate focus, text selection during streaming, retained-publication continuity, and reading anchors. Amber pending-form attention is useful. Diagnostic depth remains accessible. Preserve these behaviors through the next visual pass.

## Design critique

These are heuristic judgments, not measured user-study scores. All ten heuristics apply.

| Heuristic | Score /4 | Main finding |
|---|---:|---|
| Visibility of system status | 3 | Status is explicit; recovery is distant from the blocked composer. |
| Match to users' language | 2 | Appropriate technical content is mixed with transport/provenance grammar. |
| User control and freedom | 3 | Strong retention and explicit actions; crowded scrollers cost control. |
| Consistency and standards | 3 | Cohesive visual system; terminology and secondary controls need alignment. |
| Error prevention | 2 | Exact targeting exists, but recipient identity is absent at the input. |
| Recognition rather than recall | 2 | Recipient and recovery location require reconstructing context. |
| Flexibility and efficiency | 3 | Keyboard shortcuts and copy work; narrow chrome costs task time. |
| Aesthetic and minimalist design | 2 | Peripheral controls and diagnostic spacing compete with content. |
| Error recovery | 2 | Good failure detail; recovery entry and consequences need clearer presentation. |
| Help and documentation | 2 | Useful hints; empty and recovery states need clearer next actions. |
| **Total** | **24/40** | **Acceptable; targeted improvements needed.** |

The emotional high point is reading and inspecting real output. The low points are fitting a required response into a crowded narrow screen and discovering how to continue a blocked agent. Submitted-response summaries provide a good end state: recognizable answers and an honest waiting message.

Cognitive load comes mainly from competing task regions and mixed-purpose diagnostic rows, not too many form choices. A power user must parse repeated identifiers in Inbox/Timeline; a keyboard/low-vision operator encounters crowded form layouts and tiny disclosures; someone new to this particular tool must infer the recipient and recovery path. Domain language such as Haskell remains appropriate for the primary audience.

## Ranked changes

P0 means blocking, P1 significant difficulty or an identified accessibility defect, P2 improvement with a workaround. There are **0 P0, 3 P1, and 2 P2 priority groups** below. Confidence is high unless qualified. Effort is relative: S is localized, M crosses a few components, L restructures a view and its interaction checks.

### 1. P1 — Give the active task more usable space

**Evidence:** At 320×844, the rich form fixture leaves about 197px unobscured between the sticky pending-response banner and the composer. Navigation consumes the upper area and the composer about 227px. Jump to form exposes the first field, but the form heading can sit under the banner, including at actual 200% zoom. Submission remains possible; the problem is task visibility and excessive scrolling.

**Change:** On narrow screens, replace the persistent worker list with a selected-worker control that expands the list on demand. Compact healthy session controls. Let an idle composer be compact and expand deliberately for editing, retaining drafts. Reduce redundant pending-form chrome when the corresponding form is visible. Calculate clearance from the actual sticky banner height so the heading, label and focused control remain visible. Preserve keyboard access and DOM/focus order. Enlarge touch hit areas selectively rather than enlarging every desktop control.

**Acceptance:** Review 320px, 390px, desktop, short height and actual 200% zoom. Jump to form shows heading, first label and field unobscured. Form options and Submit remain reachable. Compare usable task height before/after. Drafts, selection, disclosure state and reading anchors survive updates; no background focus stealing.

**Location:** `WorkerChat.css:22,30,47`; `UnifiedChat.css:13`; session/composer presentation. **Effort M; confidence high.** Commands: `$impeccable layout`, `$impeccable adapt`.

[Evidence: narrow form](https://swarm-01.sphynx-bitterling.ts.net:8445/impeccable/browser/artifacts/visual-review-visual-review-chat-and-form-lifecycle-at-320px/pending-form-viewport.png) · [Actual 200% zoom](https://swarm-01.sphynx-bitterling.ts.net:8445/impeccable/browser/artifacts/visual-review-visual-review-actual-200-percent-zoom/zoom-form-viewport.png)

### 2. P1 — Put agent context and recovery where the user acts

**Evidence:** The blocked composer points to recovery in Actor controls, a disclosure after the transcript. It still permits queuing messages. The generic label “Message to selected actor” does not repeat the recipient when its heading has scrolled away.

**Change:** Show the selected recipient at the input, with exact run/incarnation available on demand. Add “Review recovery options” beside a clear paused-state message. That action reveals and focuses the existing current-block controls; it does not select a recovery mode or authorize anything. Explain each mode by its consequences and retain all evidence caveats. Make message feedback distinguish local retention, host admission, and observed execution without leading with socket terminology. Recipient labeling is a smaller P2 subtask within this action-context improvement.

**Acceptance:** From any transcript position, recovery options are one action away. Block changes invalidate the prior choice; no automatic authorization. Queuing a message never implies resumed inference. Switching actors preserves separate drafts and directs Send/Interrupt to the exact selected identity. Visible and accessible labels identify the recipient without exposing long IDs continuously.

**Location:** `ActorControls.tsx:49–59,93`; `WorkerChat.tsx:125`; `RecoveryControls.tsx:18–37`. **Effort M; confidence high.** Commands: `$impeccable clarify`, `$impeccable layout`.

[Evidence: recovery](https://swarm-01.sphynx-bitterling.ts.net:8445/impeccable/browser/artifacts/visual-review-visual-revie-2e2c0--and-realistic-long-content/recovery-viewport.png)

### 3. P1 — Strengthen unfocused input boundaries

**Evidence:** `--line` (#586e75) contrasts 2.42:1 against the panel and 2.79:1 against the canvas. The empty Project name field uses the same fill as its surrounding form, making that border necessary to locate the input. Focus styling does not repair its unfocused identification.

**Change:** Add a dedicated control-boundary token meeting at least 3:1 against its adjacent surfaces. Keep subtle separators separate. Apply to relevant unfocused text inputs and textareas, preserving the palette. Do not mechanically brighten every button border: text can independently identify a button.

**Acceptance:** Measure computed colors for empty unfocused fields on each surface, then verify focus and disabled states separately. Keyboard form use remains unchanged. This is a specific non-text contrast finding, not a blanket accessibility certification. [W3C's guidance on necessary control boundaries](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html#boundaries) explains the distinction.

**Location:** `tokens.css:13,50`; session/search/composer fields. **Effort S; confidence high.** Command: `$impeccable harden`.

### 4. P2 — Make secondary information readable and compact

**Evidence:** Publication details compute to 9.75px; the loaded-history qualifier measures about 10.83px. Repeated Message details and Publication details create tiny labels with disproportionate surrounding space. Some copy controls are about 26.6px tall.

**Change:** Establish a shared 12px diagnostic-label role, retaining existing families and 16px prose. Group related metadata affordances, reduce their vertical overhead, and retain full provenance behind disclosure. Make touch hit areas more forgiving on coarse pointers. Preserve “in loaded history”; it conveys a real limitation.

**Acceptance:** Computed typography matches the role, details remain readable at desktop/narrow/zoom, and metadata expansion survives streaming-to-retained replacement. Full source/results remain visible and copyable. A 44px touch target is a usability aim, not a claim that every smaller control violates WCAG AA.

**Location:** `UnifiedChat.css:4`; `MessagePresentation.css:13,38–39`; `tokens.css:5`. **Effort S–M; confidence high.** Commands: `$impeccable typeset`, `$impeccable distill`, `$impeccable adapt`.

[Evidence: source and diagnostic rhythm](https://swarm-01.sphynx-bitterling.ts.net:8445/impeccable/browser/artifacts/visual-review-visual-revie-89616-nd-form-lifecycle-at-1280px/tool-source-viewport.png)

### 5. P2 — Make Inbox and Timeline lead with the useful content

**Evidence:** Inbox puts messages beside dense association links, identifiers and repeated filters. Timeline mixes IDs, timestamps, tool output and raw JSON into the same scan path. “Delivered true” requires interpretation and is not evidence of success.

**Change:** Lead Inbox with sender→recipient and message, with a clear conversation-opening action. Group filtering separately and disclose technical associations. Lead Timeline with activity, observed state and useful time; keep raw output and exact identifiers accessible. Preserve unresolved endpoint associations and all filtering/deep-link behavior.

**Acceptance:** Who said what and what happened are scannable without parsing identifiers. Narrow DOM order places content before diagnostics. Ambiguous endpoints are never silently resolved and “delivered” is never rewritten as “succeeded.” Full raw output remains available.

**Location:** `InboxView.tsx:34–55`; `TimelineView.tsx:19–29`; shared `.row` in `tokens.css:66`. **Effort L; confidence high for fixture content, medium for larger unseen workloads.** Commands: `$impeccable layout`, `$impeccable clarify`.

[Evidence: Inbox](https://swarm-01.sphynx-bitterling.ts.net:8445/impeccable/browser/artifacts/visual-review-visual-review-shell-views-and-empty-chat/inbox-viewport.png) · [Timeline](https://swarm-01.sphynx-bitterling.ts.net:8445/impeccable/browser/artifacts/visual-review-visual-review-shell-views-and-empty-chat/timeline-viewport.png)

## Technical audit and detector reconciliation

| Dimension | Score /4 | Evidence |
|---|---:|---|
| Accessibility | 2 | Six independent axe samples clean; manual input-boundary contrast finding remains. |
| Performance | 3 | Bounded histories; JS is 184,109 gzip bytes. No latency/field-performance measurement. |
| Responsive design | 2 | No sampled horizontal overflow; active task height and touch targets need work. |
| Theming | 4 | Shared Solarized roles and fonts; fixed dark identity is intentional. |
| Implementation integrity | 4 | Coherent state ownership and honest execution evidence. |
| **Total** | **15/20** | **Good technical foundation, with specific defects.** |

The full CLI detector found four warnings: three side accent borders and the existing Inter stack. They represent selection, workflow/error state and the mandated typography, so all four are dismissed as defects. Layout scan found zero; type scan repeated the font warning. A clean layout scan did not detect the visually evident viewport allocation problem.

The browser detector independently confirmed small operational text. Its Chat70/Tree7/Timeline11/Inbox10 headline counts include palette objections and duplicate/hidden descendants; they are not unique defect totals. Solarized cyan/gray warnings are rejected in context. Code/result line-length warnings do not justify rewriting source. The diagnostic overlay worked in headless Playwright; there is no user-visible interactive overlay.

The design review found recovery discoverability, recipient context and peripheral hierarchy that the CLI detector missed. The technical review added the specific input contrast measurement. Both independently found narrow-space and small-text problems.

## Recommended implementation sequence

1. **First bounded batch:** implement #1–#4 in the chat surface. Begin with the small control-boundary fix, then task-space allocation and recovery/recipient context, then diagnostic typography. Use the existing interaction fixtures as invariants and capture before/after states.
2. **Second batch:** restructure Inbox/Timeline (#5) after reviewing the chat results. Their larger layout changes can proceed independently of engine integration.
3. **Finish each batch with `$impeccable polish`**, one batched desktop/narrow/zoom inspection and at most one confirmation pass. Rerun relevant interaction tests and critique the resulting surface.

These are GUI-local proposals using existing data. Authoritative global pending-form counts, compiler phases, per-effect progress and execution outcomes absent from the current contract remain coordinator/engine work. Do not fabricate them or infer outcomes from arbitrary tool text.

Lower-priority follow-ups: explain the next action in empty chat; profile streaming/render cost before splitting the JS bundle; confirm multilingual font fallback on actual user devices. No evidence justifies new fonts, decorative animation, or a palette replacement.

## Evidence and limits

The focused run executed **16 tests: 16 passed, 0 failed, 0 skipped**, producing **28 fresh viewport captures**. Coverage includes forms, native Enter submission, focus intent, streaming selection, retained publication, disconnection, recovery, short-height reading anchors, and actual 200% browser zoom. Six additional independent axe samples had zero reported violations. No full-suite rerun was needed for this documentation-only evaluation.

Production assets were copied from the previously tested exact revision only after verifying every sealed asset hash. Source under `web/` is unchanged. Browser commands used the pinned Nix web shell through `swarm-build`. Fixture servers on4387/4391/4392 and the diagnostic server on8412 were stopped. Build storage was provisioned for the isolated checkout; no engine build or shared-service restart occurred.

Assessment B observed five HTTP500 console messages without URL provenance in one exploratory probe; subsequent URL-recorded overlay probes had no errors, and all16 focused tests passed. This remains a probe limitation, not a claimed production defect. No live engine, screen-reader session, real-network timing, or complete WCAG certification is established.

Evidence root: `/srv/swarm/evidence/inanna/impeccable-review-20261012`. The [verification record](https://swarm-01.sphynx-bitterling.ts.net:8445/impeccable/verification.json) records source identity, counts, screenshot hashes and the asset manifest. [Assessment A](https://swarm-01.sphynx-bitterling.ts.net:8445/impeccable/assessment-a.md) and [Assessment B](https://swarm-01.sphynx-bitterling.ts.net:8445/impeccable/assessment-b.md) preserve independent reasoning. The private publication retains the screenshots below the report.

## Decisions for the next pass

- First emphasis: **task space** (recommended), **recovery and recipient clarity**, or **readability and control contrast**?
- Implementation scope: **chat batch #1–#4** (recommended), **small clarity/contrast fixes first**, or **all five groups including Inbox/Timeline**?
