---
name: Harness Live Patina
description: High Current petrol and verdigris for a working agent conversation
colors:
  paper: "#03262a"
  panel: "#094044"
  ink: "#f4ebd6"
  muted: "#b0c8bb"
  line: "#456e6b"
  control-border: "#7eaaa0"
  accent: "#44d6c0"
  link: "#87e4d2"
  pending: "#da9d40"
  pending-text: "#edbc70"
  failed: "#ffac91"
  waiting: "#edbc70"
  selection: "#86fae3"
  selection-ink: "#001f22"
  focus: "#86fae3"
  action-ink: "#001f22"
  action-hover: "#87e4d2"
typography:
  body:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.45
  prose:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "16px"
    lineHeight: 1.7
  code:
    fontFamily: '"IBM Plex Mono", ui-monospace, SFMono-Regular, Consolas, monospace'
    fontSize: "14px"
    lineHeight: 1.65
  diagnostic:
    fontSize: "12px"
rounded:
  option: "4px"
  control: "6px"
  code: "8px"
spacing:
  compact: "4px"
  standard: "8px"
  group: "12px"
  section: "16px"
  spacious: "24px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.action-ink}"
    rounded: "{rounded.control}"
    padding: "7px 10px"
  button-primary-hover:
    backgroundColor: "{colors.action-hover}"
    textColor: "{colors.action-ink}"
  button-primary-active:
    backgroundColor: "{colors.selection}"
    textColor: "{colors.action-ink}"
  button-secondary:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "7px 10px"
  input:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "8px"
  mounted-form:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "16px 20px"
---

# Design System: Harness Live Patina

## Overview

**Creative North Star: "Live Patina — High Current"**

Cool petrol depth, warm human presence, and precise electrical energy frame a daily agent workspace. The conversation is the working surface; technical content remains readable and controls remain clearly actionable. Material character comes from color relationships and stable planes rather than texture behind text.

This document captures the current CSS implementation based on integration revision `9f64c05`, including the Live Patina edits. The [style bible](docs/gui-style-bible.md) holds the approved broader direction; its proposed syntax palette and other unimplemented details are not current tokens. CSS in `web/src/tokens.css`, `WorkerChat.css`, `MountedForm.css`, and message presentation is the implementation source. The 2026-10-12 confirmation passed 59 browser tests, 363 unit tests, and 7 tooling tests; see [validation](docs/live-patina-validation.md) for evidence and limits.

**Key Characteristics:**

- Continuous petrol surfaces and warm ivory reading text.
- Verdigris filled primary actions with deep ink labels.
- Brass attention cues and bright mint focus and selection.
- Flat reading surfaces, restrained rounded controls, and readable code.

## Colors

The palette joins deep green-blue surfaces with mineral green activity and warm attention cues. Frontmatter records semantic roles, including intentional equal-valued roles that may evolve independently.

### Primary

Verdigris (`accent`) fills Send and inline form submission actions. Pale verdigris (`action-hover`) lightens their hover state. Deep ink (`action-ink`) keeps labels distinct on bright fills. Bright mint (`focus`, `selection`) identifies keyboard focus and selected text, with deep ink selection text. Links use their own pale verdigris role.

### Secondary

Brass (`pending`, `pending-text`, `waiting`) marks required responses and waiting cues. Warm coral (`failed`) identifies evidenced errors. Color does not establish execution state on its own.

### Neutral

Deep petrol (`paper`) is the canvas and composer textarea; petrol enamel (`panel`) supports controls, tool containers, and forms. Ivory (`ink`) carries primary text; pale sage (`muted`) carries supporting text. Quiet seams (`line`) divide surfaces, while lighter enamel edges (`control-border`) identify interactive boundaries.

**The Current Rule.** Reserve filled verdigris for primary actions; keep evidence disclosures and navigation quiet.

## Typography

The existing Inter/system sans stack serves controls and prose; IBM Plex Mono with system monospace fallbacks serves code. IBM Plex Mono regular (400) and semibold (600) are now self-hosted as WOFF2 through `@font-face` with `font-display: swap`, preserving the existing typeface choice without relying on installed fonts or a third-party service. The Inter name remains a system-fallback stack rather than a bundled sans font. Font provenance and the upstream SIL Open Font License are retained in [web/src/fonts](web/src/fonts/README.md). Browser font-loading and equal-glyph-width checks passed for desktop and narrow chat. The compact root UI remains 13px, chat prose is 16px, and rendered plain source uses 14px code with a 1.65 line height. Diagnostic content uses 12px. Legacy history prose remains 14px and some message headers retain smaller sizes; this pass does not normalize all typography.

Prose measure is capped through the existing `--prose-measure` (72ch). Fenced code and identified tool source share the 14px code token and a 1.65 line height; inline code retains `.9em`. Identified Haskell tool source and explicitly tagged Haskell Markdown fences share one code frame with wrapping, copy and expansion controls, and Refractor core with only its Haskell grammar. Tool source enters the frame directly, so Markdown fence markers inside source remain literal. Keywords and operators use the link tone, strings use pending text, comments use muted text, and numbers and types use the accent. Source whitespace remains exact and long strings soft-wrap. Live tool previews use the same renderer when provider-issued metadata names `haskell` or `haskell_sync`; unfinished function arguments remain literal JSON until ordinary parsing identifies the source. Open Haskell Markdown fences highlight while streaming. Live parsing and highlighting coalesce at an 80 ms cadence within the output owner’s bounded preview, while completion renders immediately and copying retains the latest exact input. Unnamed tool previews and other languages remain plain. No additional font assets are loaded.

## Layout

The general application uses a centered container capped at 1200px; chat expands to 1440px. Desktop worker navigation occupies a 13–17rem column beside conversation, separated by a 24px gap. At 700px and below, chat becomes one column with a worker switcher and optional compact composer. General rows adapt at 600px; actor metadata expands at 1000px. At viewport heights of 600px or less, chat permits page scrolling rather than squeezing the transcript behind its composer. Coarse-pointer chat controls have a 44px minimum height.

Spacing is primarily 4, 8, 12, 16, and 24px. Mounted forms reduce their padding to 12px on narrow screens. Reading, selection, drafts, disclosures, and intentional focus remain product requirements.

## Elevation & Depth

Transcript depth is mostly tonal: canvas, panel, and quiet borders. Do not introduce transparency or blur behind reading text. The worker tree retains a small existing shadow (`0 2px 5px rgb(0 0 0 / 10%)`); this is not a general card elevation rule. No new animation system is part of Live Patina. Reduced-motion CSS suppresses animation and transition duration and disables smooth scrolling.

## Shapes

Controls and mounted forms have gently curved corners (6px). Form options and human-message surfaces use tighter corners (4px); source blocks use 8px. Other established component-specific radii remain in place. Preserve thin boundaries and do not replace the working interface with decorative material textures.

## Components

### Buttons

Primary Send and form-submit controls use verdigris fills, deep ink labels, and semibold weight. Hover lightens the fill; active uses bright mint. Secondary buttons retain panel fill and an enamel edge; hover uses an accent border. Disabled buttons retain the existing 0.6 opacity and default cursor. Keyboard focus uses a 2px mint outline offset by 2px.

### Inputs / Fields

Fields use panel fill, ivory text, enamel borders, 8px padding, and 6px corners. The composer textarea uses the darker canvas fill. Invalid form inputs use the error border. Radio and checkbox controls inherit the accent color; checked form options use canvas fill and an accent edge. Focus within a form option retains the accent outline.

### Cards / Containers

Tool containers retain panel fill and 6px corners; source blocks use canvas fill and 8px corners. Open mounted forms use a brass border, while their heading remains ivory. Submitted answers retain their readable content and details disclosure. Human message containers retain panel fill and 4px corners.

### Navigation

Current tabs use an accent underline and semibold weight. Selected worker rows use panel fill; the current worker link gains weight and a thicker text underline. Narrow chat navigation uses the existing worker switcher. No new navigation component or generic chip system is introduced.

## Do's and Don'ts

### Do:

- **Do** use semantic CSS roles rather than copying seed swatches into components.
- **Do** retain deep ink text on bright primary actions and selection.
- **Do** preserve quiet evidence controls and readable prose and source.
- **Do** verify contrast and browser behavior after palette changes before claiming acceptance.

### Don't:

- **Don't** treat the style bible's proposed syntax colors as implemented highlighting.
- **Don't** place textures, blur, glowing text, or decorative machinery behind conversation content.
- **Don't** infer completion or activity from a color or a retained result.
