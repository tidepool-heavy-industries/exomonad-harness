# Live Patina — GUI style bible

Version 0.1 · 2026-10-12 · Approved design direction; implementation scope below.

This document stands alone. It describes a working conversational agent interface,
its visual language, and the decisions needed to extend it coherently. It is useful
to designers, engineers, and language models. It is not an image-generation prompt.
The operator selected the High Current branch of Live Patina. That direction
supersedes the earlier Solarized and Catppuccin palette choices for future design
work. The application now adopts the petrol/verdigris palette, bright primary
actions and selection/focus, 14px plain-source code, and rounded controls.
The existing IBM Plex Mono choice is now self-hosted in regular and semibold
weights; the typography proposals below remain design direction. See
[DESIGN.md](../DESIGN.md) for the exact implemented tokens and component scope.
The seed table and remaining numerical details below retain their proposal status;
syntax highlighting and other broader aspirations are not established by this
implementation. Current fixture validation is recorded in [the validation note](live-patina-validation.md); the proposed seed table is not a blanket contrast certification. User feedback
can revise any part of this document.

## 1. The job

Harness is a daily workspace for conversing with an agent while it executes tools.
The conversation contains streaming prose, substantial Haskell source, tool inputs,
execution observations, results, inline questions and forms, and recovery actions.
Users must understand what happened, what is happening, and what needs them next.
The transcript is the main workspace, not an event log behind a chat skin.

Design the application for operation and the transcript for reading. Make long
sessions comfortable. Technical depth is first-class, but routine reading should
not require parsing identifiers, timestamps, transport envelopes, or internal jargon.
Synthetic fixtures support GUI development without a functioning engine; they do
not prove live behavior.

## 2. The world: electric life in weathered petrol

Imagine a rainy, inhabited city a century into its strange industrial future:
dieselpunk cyborgs, hissing electrical light, enamel surfaces, smoked glass, brass
worn smooth, copper turning green. New Crobuzon a hundred years later is an
atmospheric reference, not a request for a fictional city illustration.

The emotional balance is **cool depth, warm human presence, precise electrical
energy**. The ground is unmistakably green-blue petrol. Verdigris feels alive
against it. Ivory text and small pools of brass light keep it humane. It should
feel charged, capable, and used by people, never military, bleak, precious, cozy,
or theatrical.

Translate material into interface behavior and color:

- Petrol enamel becomes continuous dark teal planes with clear, quiet edges.
- Smoked glass becomes depth through adjacent tones, without transparency behind
  text or dependency on backdrop blur.
- Patina becomes a family of mineral greens and blue-greens, not random distress.
- Electricity becomes a crisp focus ring, a clear active control, a small running
  indicator. Brightness identifies meaningful activity.
- Brass and warm lamps become restrained attention and waiting cues, not golden
  decoration spread over every border.

Material photographs belong in design studies, not behind the transcript. Reading
surfaces are flat and stable. Do not add scratches, raindrops, gears, rivets, scan
lines, lens bloom, or glowing text. Personality comes from color relationships,
typography, proportion, and precise state details.

## 3. Color system

These seed colors were measured from the selected generated study, using median
RGB from 11×11-pixel swatch patches. Texture makes them approximations to a visual
family, not a pre-certified UI palette. Semantic use below deliberately differs
from some labels in the generated study. Never copy a swatch's label as evidence
that its contrast or interaction role is suitable.

| Seed | Hex | Intended use |
|---|---|---|
| Deep petrol | `#03262a` | Main canvas; the dominant visual field |
| Petrol enamel | `#094044` | Tool/source and composer surfaces |
| Lifted petrol | `#084c4f` | Selected regions and elevated surfaces, sparingly |
| Mineral teal | `#196b69` | Strong surface emphasis, not the default card fill |
| Quiet seam | `#456e6b` | Decorative divisions; not sufficient by assumption for controls |
| Enamel edge | `#36716c` | Border seed to be adjusted for control visibility |
| Warm ivory | `#f4ebd6` | Primary text |
| Pale sage | `#c0d1c3` | Supporting readable text |
| Weathered sage | `#7a9487` | Muted-text seed; lighten where necessary |
| Deep ink | `#001f22` | Text and icons on bright filled controls |
| Electric mint | `#86fae3` | Focus and selected-text treatment, paired with deep ink |
| Current | `#44d6c0` | Primary action; link text and active accents |
| Patina | `#50bda4` | Secondary accent and syntax family |
| Aged brass | `#da9d40` | Warm attention, small highlights |
| Living green | `#45a978` | Success seed |
| Amber | `#d2943a` | Warning seed |
| Fired copper | `#bb593e` | Error seed; lighter text variant will likely be needed |
| Rain blue | `#4fa3b6` | Informational seed |
| Pale current | `#87e4d2` | Running indicator and restrained hover emphasis |
| Warm copper | `#ca8343` | Waiting-state seed |

Build semantic tokens, not component-specific hex values: canvas, surface,
surface-raised, text, text-secondary, text-muted, border-control, focus, action,
action-hover, action-ink, selection, success, warning, error, info, running, waiting.
Status families need separate foreground, background, and border tokens. A color
appropriate as a dot may be unsuitable for a sentence. Error text can be ivory
beside a copper icon until a readable copper foreground is established.

Keep most of the viewport petrol, most of the readable content ivory/sage, and
bright chromatic accents scarce. Avoid navy drift, purple-gray neutral surfaces,
pure-black voids, olive military backgrounds, and indiscriminate neon outlines.
Separate decorative brass from semantic warning through placement, icon, and label.
Do not imply a state with color alone.

Before production adoption, measure all actual foreground/background pairs:
normal text at least 4.5:1, large text at least 3:1, essential control boundaries
and state indicators at least 3:1 against adjacent colors. Check hover, selection,
focus, disabled legibility, syntax, and tinted panels separately. These are
acceptance targets, not claims that these seeds already pass. Adjust within the
color family; readability wins over preserving an exact sampled hex.

## 4. Type, geometry, and density

Use a humanist sans-serif with open counters for interface and prose, and a crisp
monospace with distinct punctuation for code. Initial implementation should reuse
the existing fonts; new font selection is open. Avoid stencil, techno, ornamental,
and faux-terminal typography. Never put the entire interface in monospace.

Start prose at 16px/1.6, supporting UI at 14px/1.4, code at 14px/1.55, and section
headings at 18–22px with moderate weight. Metadata may be 12–13px when optional;
important status and form instructions must not shrink to that level. Sentence
case is normal. Reserve tracking for rare tiny category labels, not paragraphs.
Keep reading measure near 65–80 characters; give code more width when needed.

Use a 4px spacing unit, 8–12px within closely related groups, 16–24px within turns,
and 28–40px between turns. Relationship matters more than rigid arithmetic.
Corners are modest, approximately 6px for controls and 8px for contained tool/form
regions. Use thin purposeful borders and little shadow. No repeated nested cards,
status pills everywhere, huge empty gutters, or dashboard tiles around every fact.

## 5. Conversation structure

A compact worker/session navigation region supports a dominant conversation
column. The header identifies the current conversation and its meaningful state.
It does not compete with the transcript. Supporting views are available on demand.

Group each turn by author and content continuity. Distinguish user and assistant
with labels, spacing, and restrained surface treatment; avoid enclosing every
assistant paragraph in a bubble. Prose, code, execution, and results belong to the
same readable turn. Paragraphs and headings need clear rhythm. Lists and tables
should handle real technical content without turning the whole transcript into a
wall of small labels. Links look interactive and remain keyboard discoverable.

A tool region shows a human-readable tool name, observed state, source/input,
result excerpt or rendered result, and contextual actions. Haskell gets proper
syntax coloring: sage comments, patina identifiers, brass strings, pale-current
keywords, ivory punctuation where needed. Preserve selection and full-source copy.
Bound very long code/output without hiding its existence; offer explicit expansion
and keyboard-reachable horizontal scrolling. Prose wraps; code preserves meaning.

Keep raw arguments, transport metadata, timing, identifiers, and retained evidence
in a consistently named disclosure. Remember expansion choices while streaming.
The output excerpt must be literal evidence, not an optimistic paraphrase.

## 6. State and required attention

Show lifecycle state beside the tool or turn that owns it. Distinguish queued,
running, output available, completed, failed, interrupted, disconnected, and unknown
only when the available data supports that distinction. Output present does not
prove successful completion. Silence does not prove a process is still running.

Use mint for observed running activity, green for confirmed success, copper for
failure, amber for caution, and warm brass/copper for a response needed. Pair each
with a short label and, where helpful, a distinct icon. Running indicators are
small and local; no ambient pulsing or full-window loading wash.

An unanswered form is a first-class conversation element with a clear question,
labels, useful descriptions, visible control edges, and one obvious submission
action. Show a compact jump-to-response affordance when required input is offscreen.
Counts describe known loaded history unless the backend provides a complete index.
On submission, retain a readable record of the user's answers and the original
context. Field errors belong beside the field and link to it. Never erase answers
or imply successful submission while it is still pending.

Failures explain the concrete problem and offer a specific supported next action.
Keep recovery beside the failed operation; retain original evidence. Destructive
or repeated actions must not masquerade as harmless retry links. A disconnected
session preserves the draft and states whether sending is unavailable.

## 7. Composer, streaming, and motion

The composer is anchored and easy to find, with a clear recipient, multiline draft,
send affordance, and an interrupt action while interruption is available. Preserve
drafts through navigation, disconnects, and reading-mode changes. On narrow screens,
let users explicitly collapse it to read and reopen it to resume their draft.

Follow incoming content only when the user is already following. Scrolling upward
or selecting text suspends automatic following. Offer a jump to new activity;
do not drag the viewport, replace selected text nodes unnecessarily, or steal focus
when a tool finishes or a form arrives. Keep stable identities for streamed rows.
Use normal focus order and deliberate focus after explicit actions.

Transitions are short, typically 120–180ms, and clarify local changes. No entrance
animation for each token, bouncing messages, typewriter delays, or animated layout
shifts. Honor reduced motion. The conversation must feel steady even when the agent
is busy. Electricity is precision and contrast, not constant movement.

## 8. Responsive and accessible behavior

Desktop gives reading and substantial code room. Narrow screens collapse navigation
before squeezing content; controls wrap with meaningful grouping. At 320px and at
200% browser zoom, prose and forms remain usable without page-wide horizontal
scrolling. Code and genuinely wide tables may scroll inside their own regions.
The composer never covers the only usable slice of the conversation.

Use semantic headings, buttons, labels, disclosures, and status announcements.
Visible keyboard focus is a crisp mint ring with separation from the control edge.
Aim for 44px touch targets while allowing compact desktop controls with adequate
hit areas. Announce meaningful state changes, not every streamed token. Keep native
selection, zoom, keyboard editing, and browser navigation working.

## 9. Decision order and review

When rules compete, prioritize truthful state, user control and accessibility,
readability, conversational hierarchy, then visual atmosphere. Preserve an approved
behavior while improving its presentation. The user owns the direction; this bible
is a shared design record, not an agent-written veto on future changes.

Review against a real conversation: long prose, substantial Haskell, an observed
compile failure, a result excerpt, an unanswered form, submitted answers, a retained
draft, and a disconnected state. Inspect desktop and narrow screenshots, then
exercise streaming, scrolling, selection, keyboard focus, copy, submission, and
recovery. Test meaningful interactions rather than exact markup or decorative prose.

The visual recognition test is simple: does the interface read as petrol depth,
living patina, warm human light, and precise current while remaining an excellent
place to read and act? A generated mockup can test that vocabulary. It cannot prove
behavior, contrast, accessibility, or implemented product capabilities.
