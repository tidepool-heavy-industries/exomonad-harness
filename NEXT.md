# NEXT: wave 10 / RSI iteration 2

Read docs/rsi-iteration-2.md and implement its authorized outcome: the complete
offline follow-up/finalization lifecycle with typed parent answers and durable
seen/unseen input provenance. The root owns shared contract, decomposition,
independent review and checked integration. No planner release is outstanding.

This current assignment controls decomposition and result types over historical
campaign examples in docs/tree.md or prompts. Use two or three bounded Sol Medium
implementation children where the source permits; their result is Outcome
Candidate, not Delivery. Root retains integration. Ordinary event-driven review
uses the new ReviewRequest/ReviewBasis API; automatic-review wrapper repair is
outside the wave. Resolve full source OIDs from Git.

## Obligations

| Obligation | Owner | State / next evidence |
|---|---|---|
| Shared lifecycle/provenance contract and scaffold | root | inspect existing owners, commit before dependent forks |
| Stable follow-up identity and durable provenance | assigned by root | production service/Store behavior |
| Typed completion and parent publication | assigned by root | strict typed answer with seen/unseen references |
| Driver continuation and integrated release gate | root or named owner | real driver, deterministic boundary cases, reopened Store |
| Independent review, integration, interview and stop | root | exact-source verdict, executed integrated checks, handoff |

Keep this table current as assignments settle; ordinary idle turn ends need no
commit. Preserve one implementation owner per shared file and record amendments.

## Standing constraints and retained evidence

- No git stash/reset/checkout -- path; commit by pathspec, no attribution trailers.
- One compiler worker; do not restart the shared daemon.
- No credentialed item-2 retry, item-13 trace or live adapter smoke. Offline
  replay and Exomonad coding actors are authorized.
- Root may make implementation decisions within the stated contract; ask only
  when consequential ambiguity remains after inspecting source and requirements.
- Wave 9 merged first-request gate at 4230179, handoff 9c6e75b. Its manual
  advance_agent_head proves persisted observation, not the production runner.
- Wave 8 adapter-readiness gate remains required; it uses an offline evaluator.
- Review setup: harness 705eaee, workspace 7307478. Provenance recipe passed 6/6
  in Tidepool and harness; the workspace-pin test passed. No full gate claimed.
