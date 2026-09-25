# Correction-wave interviews

## root

- **Scaffold change:** The project `AgentSpec` required `Journal`, but coding children do not receive that effect. Both first-wave children failed before starting. I amended the spec to install the baseline watchdog instead. This loses the project-specific nudge ledger for children; I will not invent events.
- **Missing sibling interface:** The engine's seven entry points and demo's two callers need one agreed signature before a leaf owns either side. I will retain the demo consumer wiring once that signature is checked.
- **API versus docs:** The child effect row lacks `Journal` although the project nudge spec assumed it.
- **Integration cost:** The first delegation wave failed at startup; it cost a retry and a source amendment, not feature progress. A preflight child launch would have surfaced this earlier.
- **Different scaffold next time:** Check the actor spec against the child effect row before admitting the wave, and write the single engine signature into the scaffold before splitting loop from consumers.
- **Nudges:** No child nudge fired because both initial children failed at tool installation. The fallback watchdog can run, but its events are not the project nudge ledger.
# Cache-shape probe leaf — 2026-09-24

## Own-words plan and question

My bounded task was to make exactly two consecutive live requests with
Codex-shaped headers, `prompt_cache_key`, and body order, then record redacted
bodies and cache counters in `docs/findings.md`; alternatively, report the
precise missing capture. The plan in `docs/correction-plan.md` and the
operational boundary in `NEXT.md` both say this is findings-only, one node,
and not a source-code or review task.

I found the harness request builder and prior live-call narrative, but no
exact reference capture of Codex's serialized request. Those artifacts do
not establish byte-for-byte parity. I therefore sent no requests rather
than present a reconstructed shape as a live comparison. What exact redacted
Codex wire capture can be supplied (ordered header values and body bytes)?
The precise Blocked evidence is in `docs/cache-probe-evidence.md`; root owns
incorporating it into the shared `docs/findings.md`.

## Prompt trials

- Incorporating an existing sibling/parent change: not applicable; no such
  change was incorporated.
- Intentionally red test: not applicable.
- Two failed check rounds before ping: not reached; this was an evidence gap,
  not repeated check failures.
- Operator notes as advice unless explicitly constraints, and labeling
  hypotheses: held. I treated the two-request probe criteria as the stated
  acceptance, and did not infer exact wire shape from the narrative.

No code or tests were changed or run. The detailed blocker is recorded in
`docs/cache-probe-evidence.md`.

## Correction core lead — 2026-09-24

Root incorporated this node's own-words interview from its retained commit
`6667ddc`. That commit was excluded from the delivered branch because these
documentation paths are root-owned. Its old local findings commit `b765e9b`
was rebased as `1513e95` and merged by root at `06148a1`.

- **Plan and tree cost:** I planned three ordered slices. The (b) Luna
  produced `a1f976f`, and an independent settings-contract reader found
  the provenance and pinning seams. The host notification inbox then
  became fenced, so later child replies and ordinary review waves could
  not be received normally. I inspected the (b) diff inline, merged it
  in `ffe20e5`, and ran the focused offline acceptance test (1 passed)
  and workspace format check. Root incorporated that branch at `d8097c3`.
  The notification failure made this an exceptional manual review.
- **Missing interface:** Generic `Item(Value)`/Store replay preserves JSON
  but does not identify which `configuration_update` was harness-authored.
  The settings slice needs a provenance decision before pinning effort,
  replacing adjacent updates and stripping/re-pinning child histories.
  No SQL migration is inherently required.
- **Next scaffold:** Represent settings provenance at append time, not by
  trusting wire JSON. Define the initial update and child fork prefix
  once; then separate store/engine and runtime/verb changes behind that
  seam.
- **Prompt trials:** I named exact commits for incorporation. No red test
  was committed by my node. There were no two failed check rounds. The
  operator's inbox report was treated as a run constraint, not silently
  generalized to the project. No nudge events were observed or invented.
- **Result:** I returned `Blocked` for (c)/(d), not a feature completion.
  The live item-2/item-13 and unanswered-call experiments were not run.

## Root continuation — 2026-09-24

- **Tree structure cost:** The probe initially changed a shared findings
  file outside its ownership; checking the cumulative branch diff caught
  the earlier commit even though its tip was in scope. Repair and rebase
  produced `cdbbd367`, merged at `b99337e`. The probe still lacked the
  reference capture, so no live call was justified.
- **Stalled lead:** A notification delivery receipt did not wake core's
  fenced inbox. Operator TUI relay supplied the missing checkpoint. Core
  continued long enough to deliver (b) and findings, but could not safely
  start a new child wave; a `Working` request was not evidence of progress.
- **Different scaffold next time:** Make provenance an explicit
  harness-owned property at append time and fix the initial settings pin
  and fork-prefix contract before splitting (c)/(d). Preflight notification
  routing before admitting that next lead.
- **Prompt trials:** Exact incorporation commits were recorded; the
  intentionally red offline test was treated as a contract and turned
  green by (b), not mislabeled passing early. No repeated failing-check
  loop was observed. The inbox fence was an operator constraint for this
  run; no conjecture about its mechanism was promoted to an engine fact.

## Cache-counter probe leaf — correction second half, 2026-09-24

- **Scaffold and plan:** Q4 changed the earlier byte-parity blocker into one
  yes/no measurement. I used the existing production request builder at
  `d0245b3`; no scaffold code changed under me.
- **Sibling interface:** None. The read-only credential and builder were
  present; the temporary runner did not change repository code.
- **API versus docs:** The production builder returned usage for both calls;
  the second reported 20,736 cached input tokens. This measures the stated
  criterion, not a promise of future cache hits.
- **Tree cost:** Root must integrate this one findings-only document and
  incorporate the result into shared findings. A narrower probe than the
  previous byte-parity task avoided an unnecessary missing-capture blocker.
- **Different scaffold next time:** State the measurement and its available
  inputs in the task before admission, as Q4 now does.
- **Nudges and trials:** No sibling change, intentional red test, or repeated
  failed-check escalation occurred. No nudge firing was reported.

## Item-2 live probe leaf — correction second half, 2026-09-24

- **Scaffold change:** None. I read the integrated async-tool schema and demo
  slow `sleep` path at `d0245b3`.
- **Sibling interface:** The existing CLI can drive the scenario, but it does
  not expose the actual redacted request bodies or correlate them with sleep
  progress and `wait_agent` result. Root owns a trace seam before a live run.
- **API versus docs:** The code has the ingredients, not the auditable trace
  required by item 2. I did not infer continuation from a prompt or mock test.
- **Tree cost:** Root must merge this findings-only document and add tracing
  before spending the single manual live run. My initial document needed a
  rebase onto root's intervening cache-probe integration.
- **Different scaffold next time:** Require a redacted trace surface before
  assigning the credentialed acceptance experiment.
- **Nudges and trials:** No intentional red test or repeated failing-check
  round occurred; no nudge event was reported. No inference was spent.
