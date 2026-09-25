# nudges: jev packets per label (v2)

fmt: compressed. `N:` noul (floor 0.6 unless noted). `C:` choice (alts + exit). `S:` score (rubric levels; advise below threshold). `A:` advise (annotated onto the child's own result). `E:` escalate (also `sendMessage` parent, `strict` policy only). `⇑` parent.

## what jev is good at here (measured, see jev-laptop notes)
- answers *which*, *does*, *how much* over a state you supply. never *what next* — no "what should the child do now" questions; author the candidate advices, let jev pick among them.
- one call carries the whole battery (≤100 questions, ~200ms). per-item batteries (`each` over files) are nearly free; ask them.
- wording = conditions over named state fields. every choice has an exit. rivals come from evidence, not symmetry.
- numbers are the product: a score's *level* selects the advice text; a noul's mass vs floor selects whether to speak; margins gate escalation.

## install
- slot: `afterTool` in `.exomonad/AgentSpec.hs` (post-call; no veto). tools seen: `haskell` (cell src in `arguments`), `execute` (cmd), `lookup`, `inspect`.
- write own `watch :: ToolCall -> ToolResult -> Eff effs Annotation` (shipped `Project.Watchdog` = nouls only). one `J.ask` per call.
- state: `{label, tool, arguments, result, paths, recent_calls, recent_advice, last_scores}`.
  - `paths` = file paths extracted from `arguments` by regex in the handler (rows for `each`).
  - `recent_calls` = last `historyDepth` completed turns via `reflect` (leaf 2, lead/root 4).
  - `recent_advice` = annotation lines (prefix `⚑`) found in `recent_calls` results — what the child was already told.
  - `last_scores` = this actor's previous score levels from the ledger (below).
- packet = `L0 :& L1 :& L2 :& <set for label> :& L4 :& L5 :& L6 :& L7` (packets compose w/ `:&`); ~40–90 questions.
- label = `contextActorPath` substring, per tree.md. sets:
```
core-*            everyone ++ leaf ++ core ++ core-<module>
server-*          everyone ++ leaf ++ server ++ server-<module>
web-principles    everyone ++ leaf ++ web-principles
web-* (other)     everyone ++ leaf ++ web ++ web-<screen>
core|server|web   everyone ++ lead
root (otherwise)  everyone ++ lead ++ root
```

## handler (deterministic, after the one call)
1. candidates = tripped nouls (mass ≥ floor) ++ choice branches carrying advice ++ scores below threshold (advice text = the level's gap, from the rubric).
2. anti-nag: drop a candidate whose advice already appears in `recent_advice`, unless its score fell ≥1 level vs `last_scores`.
3. select ONE advice: if L6 `show` names a standing candidate → it; else highest severity (score deficit ≥2 levels > noul > score deficit 1). ≤1 `⚑` line per call; escalations are additional, never suppressed by anti-nag.
4. escalate only when `J.settle strict` clears (mass/margin/confidence floors); otherwise the escalation noul degrades to advice.
5. ledger: append `{ts, label, kind, phase, artifact, answers(all), shown, dropped_by_antinag, escalated}` to `.exomonad/nudges/<label>.jsonl` (Journal effect if available, else file by pathspec). feeds: after-run interview ("which nudges fired; right or wrong"), floor tuning, and replay tests (ledger replays ⇒ handler is a pure test).
6. `Abstained` when nothing stands. never annotate on `kind = read`.

## L0 context (choices; no advice; gate everything else)
- C: `kind` "Which describes this call?" `check` | `commit` | `fork` | `reply` (settles assignment) | `edit_contract` | `edit_own` | `read` | `ask` (question to operator) | `other`.
- C: `phase` "Which phase of the assignment does `recent_calls` + this call show?" `orient` (reading scaffold/PRD, no edits yet) | `implement` (edits, no check since) | `verify` (checks/tests after edits) | `deliver` (commit/reply) | `blocked` (repeated failure or waiting) | `unclear`.
- C: `artifact` "What is being written, if anything?" `rust_lib` | `rust_test` | `sql` | `ts_component` | `ts_types` | `doc` | `commit_msg` | `cmd` | `nothing`.

## L1 everyone (hygiene nouls)
- `stages_everything` "git add/commit with `-a`, `-A`, `.` or `--all` instead of named paths?" A: siblings share index; `git commit -F msg -- <paths>`.
- `done_with_stubs` (kind=reply) "`result`/`recent_calls` show a `todo!()` in the delivered module or a failing check after the last edit?" A: finish or state exactly what is left.
- `unwrap_in_lib` (artifact=rust_lib) "non-test rust calls `unwrap`/`expect`/`panic!`?" A: typed error; propagate.
- `stringly_typed` (rust) "passes id/model/effort/kind as `String`/`&str` where a newtype/enum exists?" A: use it.
- `blocking_in_async` "blocking API (std fs/net, rusqlite, thread sleep, `block_on`) inside async w/o `spawn_blocking`/writer task?" A: no blocking in async.
- `unbounded_channel` "unbounded channel or uncapped queue?" A: bounded + overflow policy.
- `orphan_task` "`tokio::spawn` w/o an owning scope/token?" A: token hierarchy run→conversation→request/job.
- `secret_in_output` "logs/stores/emits API key or auth header?" A: redact at transport boundary.
- `hardcoded_limit` "hardcodes a cap/timeout/threshold instead of config w/ default?" A: config field.
- `helper_shipped` (0.75) "adds a public fn in `crates/harness` whose body only chains two+ of wait_agent/cancel/spawn_agent/checkpoint/set-effort/store queries, under a name not on the PRD primitives line?" A: primitives not helpers; delete.
- `noncanonical_json` "hashes or sends JSON w/o canonical key order?" A: canonical serializer for every hashed/cached byte.
- `reinvents_library` (0.7) "hand-writes something nontrivial that a well-tested crate/package already provides (SSE parsing, backoff+jitter, JSON schema derivation, canonical JSON, blake3, SQL migrations, WS framing, virtualized lists, JSON-schema forms) with no stated, specific reason in the code or commit?" A: use the library from the PRD stack; if a reason exists, write it in a doc comment; if not, delete the hand version.
- `repeating_itself`, `ignoring_a_failure` (Project.Watchdog wording). exempt: retrying the same call after a host-side rejection (a held lock, `ReplyUpdatePending`) is the correct move, not a repeat; never advise on it.
- `asks_answered_question` (kind=ask) "the question's answer is stated in PRD.md, tree.md or the scaffold docs?" A: cite the section; don't ask.
- `ask_shape` (kind=ask) "question lacks `[label]`, `default:` or `blocks:`?" A: tree.md `ask` shape.

- `settled_rule_as_blocker` (kind=reply, phase=blocked) "reply returns `Blocked` or asks for a 'design decision' on a point PRD.md decides with a `!` rule or the annotations name as decided (e.g. settings provenance: harness-authored only)?" A: cite the rule; implement it; a PRD rule is a blocker only if the code shows it cannot hold, and then the reply quotes the code.
- `invented_scope` "building identity, auth, sessions, rate limits, or another subsystem the PRD assigns elsewhere (tailscale identity) or does not name?" A: stop; ask; the PRD reason for the assignment stands until amended.
- `interview_missing` "final reply without the interview section, or the wave ends with no `docs/interviews.md` entry?" A: the interview is a deliverable.

## L2 per-file battery (`each` over `paths`; leaf only)
- `owned` "Is <path> inside the module this label owns per tree.md?" (no → feeds `edits_contract` E)
- `contract` "Is <path> in the contract list?" (yes → E)
- `tested` (artifact=rust_lib) "Does this call or `recent_calls` also touch a test for <path>?" (no on ≥2 files → A: focused test per module)
- `generated` "Is <path> a generated file (`web/src/protocol.ts`, schema json) edited by hand?" (yes → A: regenerate)

## L3 sets
### leaf
- `broad_build` "builds/tests whole workspace (`cargo test/build` w/o `-p`, `--workspace`, all-package npm)?" A: `-p <crate>`; gate is parent's.
- `edits_contract` (from L2) E: leaf touched contract / outside module. A: revert; send ⇑ the amendment.
- `check_cadence` "≥4 edit calls in `recent_calls` with no `cargo check`/typecheck between?" A: check after every few edits.

### core (all core leaves)
- `stateful_api` "sends `previous_response_id`, `store: true`, or reads server-side conversation state?" A: stateless.
- `sync_path` "tool run path returning Output directly / blocking wait in a tool / Job not a Future?" A: ∀tool = Job.
- `two_outputs` "second FunctionCallOutput for a call_id that has one?" A: one output per call_id.
- `effort_from_session` "request-level `reasoning.effort` set from current effort instead of first `configuration_update` in history?" A: pin to first update.
- `adjacent_updates` "`configuration_update` directly after another?" A: merge.
- `tidepool_leak` "imports/names tidepool or exomonad?" A: harness ⊥ tidepool.
- `value_past_wire` "`serde_json::Value` used past the transport boundary, other than tool args?" A: typed items.
- `state_transition_scattered` "conversation/job state mutated outside the one transition fn?" A: transitions in one place; illegal = typed error.

### core-transport
- `buffers_stream` "collects the whole response before yielding any delta?" A: deltas as they arrive; start job when call item completes.
- `retry_after_partial` "retries after stream items were delivered, w/o discarding them?" A: retry only before first delivered item.
- `retry_class_wrong` "retries a 4xx other than 408/409/429, or does not retry 429/5xx/timeout?" A: taxonomy retryable vs terminal.
- `no_jitter` "backoff without jitter or without a cap?" A: jittered, capped.
- `cache_key_varies` "`prompt_cache_key`/affinity derived from anything a fork doesn't share (session id, ts, effort)?" A: key = root request hash.
- `sse_by_line` "parses SSE by newline ignoring event boundaries / multi-line data?" A: frames.
- `usage_dropped` "does not capture `usage.cached_tokens`, total tokens, and response id from the completed response?" A: store them per request.
- `stall_undetected` "no timeout for 'no delta for N s' on an open stream?" A: stall detection → failed+retry.
- `retention_param` "sends `prompt_cache_retention`, or a `prompt_cache_options.ttl` other than `30m`?" A: ttl 30m only on gpt-6.
- `tools_edited` "changes the `tools` array between requests of one conversation instead of `tool_choice: allowed_tools`/`none`?" A: stable list, allowed subset.
- `reasoning_dropped` "omits reasoning items (or call/output items) since the last user message from the next request?" A: replay all of them.
- `phase_dropped` "replays an assistant message without its `phase`?" A: preserve phase.
- `pro_mode` "sets `reasoning.mode: pro` or effort `none` on astra?" A: out of scope / 400.
- C: `request_shape` `matches_items` | `invents_item` | `omits_async` (function tool w/o `async: true`, not wait_agent) | `omits_strict` (tool schema not strict) | `effort_unpinned` (request-level effort ≠ first update in history) | `not_a_request`. advice per branch.

### core-store
- `request_key_not_hash` "request keyed by autoincrement/uuid rather than hash(parent, appended)?" A: blake3(parent, appended).
- `write_outside_writer` "INSERT/UPDATE/DELETE or write conn outside the writer task?" A: rows → channel → writer.
- `rewrites_in_place` "UPDATE/DELETE on requests/items/events?" A: append-only.
- `blob_inline` "item bodies inline w/o threshold?" A: > threshold → blob dir.
- `read_across_await` "read txn held across `.await`?" A: short txns.
- `string_sql` "SQL by concat/format, or table w/o FK/NOT NULL/CHECK/UNIQUE?" A: named query fns; constraints in schema.
- `no_migrations` "tables created outside a numbered migration?" A: migrations from day one.
- `event_before_item` "an event row referencing an item/request can commit before that row?" A: items first, in the same batch.
- `missing_index` "recursive query over parent link w/o an index on `parent`?" A: index.
- `batch_unbounded` "writer batches w/o size or ≤50ms time bound?" A: both bounds.
- C: `query_shape` `recursive_cte` | `parent_loop` | `whole_table` | `not_a_query`. advice: CTE over parent link.

### core-loop
- `pending_dropped` "drops pending jobs when the model stops?" A: pending survives; late settle → new request.
- `late_no_request` "late-settled job delivered only when next user input arrives?" A: settle starts a request.
- `wait_order` "wait_agent resumes with its own output before settled outputs and mailbox envelopes?" A: settled outputs first, then envelopes, then wait's, then user msg.
- `wait_answered_early` "answers wait_agent before a resume event?" A: withheld until resumed.
- `wait_returns_content` "wait_agent's output carries a result or message body instead of only what resumed it?" A: which, never content.
- `reply_as_output` "child's final answer delivered to the parent as a function_call_output instead of a FINAL_ANSWER envelope?" A: two channels, never mixed.
- `envelope_shape` "envelope text deviates from `Message Type / Task name / Sender / Payload`?" A: trained form verbatim.
- `spawn_from_missing` "spawn implemented without the `from ∈ {prompt, here, checkpoint}` argument, or with turn counts?" A: PRD agent verbs.
- `spawn_claim_lost` "spawn raised inside a pending call does not give the child a claim on that call?" A: child inherits the claim.
- `fork_unstripped` "forked child carries the parent's configuration_updates or watchdog annotations?" A: strip list.
- `adjacent_updates` "two configuration_updates can become adjacent (fork + re-pin, or compaction re-pin)?" A: never adjacent.
- `update_forged` "accepts a configuration_update from a model or client input?" A: harness-authored only; drop forged.
- `slot_count_missing` "developer item does not state the slot count, or states 'all agents equally capable, same tools'?" A: write what is true from the scheduler cap and labels.
- `subtree_cancel_bypass` "a client command can cancel/delete a child other than through its root?" A: parent owns shutdown.
- `third_channel` "something reaches a conversation between requests that is neither a call output nor an envelope (raw user message path, direct item append)?" A: two channels only; user input is an envelope from `/operator`.
- `envelope_before_output` "an envelope is placed before a function output in the next request?" A: all outputs first, then envelopes.
- `steer_on_http` "Steer class attempted on the HTTP transport instead of downgrading to AtBoundary?" A: downgrade.
- `class_open` "delivery class is a string or has more than Steer/AtBoundary/Hold?" A: closed enum of three.
- `checkpoint_split` "checkpoint, cache breakpoint, compaction boundary or fork point implemented as separate concepts/rows?" A: one row, four uses.
- `cold_prefix_sent` "a cold prefix is sent without a `Refused{prefix_cold}` round or `accept_cost`?" A: typed refusal first.
- `refusal_as_error` "a refusal surfaces as an Err/exception instead of a variant of the verb's result enum?" A: closed result enum.
- `finalize_prose` "typed reply (`reply` schema present) delivered as prose instead of a `finalize` record?" A: forced strict call.
- `cancel_no_output` "cancels a job w/o `Cancelled` on its call_id?" A: typed output always.
- `cancel_race` "job settle and cancel can both produce an output for one call_id?" A: first wins, exactly one.
- `sync_tool_pending` "a tool whose call can outlive the response lacks `async: true`, yet the loop resends its `function_call` without an output?" A: mark it async; the API rejects a bare call otherwise (wave0 shipped this untested live).
- `effort_by_field` "effort set by the request-level `reasoning.effort` field with no `configuration_update` item in history?" A: positional item, harness-authored; the field is only the first update's mirror.
- `entry_point_sprawl` "more than one public run/start entry on the engine, or a doc comment saying legacy/source-compatible?" A: one entry (durable head + new items + mailbox); delete the rest; zero back-compat is a PRD rule.
- `verbs_via_provider` "agent verbs dispatched through the provider trait instead of owned by the engine?" A: crate-owned verbs; the provider supplies tools and hooks only.
- `claims_missing` "fork does not copy the parent's pending claims (or offer drop)?" A: PRD claims rule.
- `claim_settled_not_replayed` "claim on an already-settled job does not deliver the stored output immediately?" A: replay stored output.
- `deliver_state_wrong` "delivery to a paused claimant does not resume it / to an idle one does not start a request / to a requesting one is not queued for the next?" A: per state.
- `debounce_wallclock` "debounce/timeouts use wall clock instead of monotonic?" A: monotonic.
- `scheduler_no_capacity` "requests issued w/o passing the capacity/priority scheduler?" A: 4th event source; root before leaves.
- `delta_serialized_twice` "delta re-serialized/cloned per subscriber inside the process?" A: one Arc'd event, fan-out by reference.
- C: `handle_registry` `unique_per_conversation` | `reused` | `call_id_as_handle` | `none_written`.

### core-compaction
- `guessed_compaction` "asserts whether an unanswered function_call survives the server-compacted window while `recent_calls` shows no experiment?" A: run it; `docs/findings.md`.
- `pin_not_refreshed` "new window starts w/o a fresh `configuration_update`, or still contains an old one?" A: strip, then re-pin.
- `trigger_not_last` "`compaction_trigger` sent with items after it, or alongside a configuration_update?" A: final item; strip updates first.
- `auto_compaction_on` "sets `context_management` or truncation on a request?" A: never; explicit trigger only.
- `compact_output_pruned` "edits or prunes the window the server returned?" A: as-is.
- `pending_lost` "carried claims' `function_call` items absent from the new window?" A: carry them verbatim.
- `summary_not_item` "new window stored outside the request tree (not a request w/ a `compaction` parent edge)?" A: it's a request node.
- `strategy_hardcoded` "compaction logic not behind the `Compactor` trait, or `Server` not the crate default?" A: one trait, three strategies.
- `handoff_subturn` "`Structured` lets the model call other tools before the handoff, or handoff tool not forced via `tool_choice`?" A: one forced call.
- `summary_asks_state` "summary schema asks the model for bindings/worktrees/children that code can list?" A: code appends deterministic state after the summary.
- `user_msgs_dropped` "new window omits the user's own messages?" A: retain verbatim, bounded.
- `opening_item_missing` "new window lacks the versioned opening developer item (compacted; successor; live state follows)?" A: add it.
- `trigger_by_judgment` "compaction triggered by a jev answer rather than usage/command/decision?" A: trigger is code.
- `findings_missing_numbers` (artifact=doc) "findings.md lacks cached_tokens before/after and the exact API response for the late output?" A: numbers + raw response.

### server (all)
- `bypasses_loop` "server mutates store/session directly instead of a command to the scheduler?" A: channel client only.
- `unversioned_wire` "wire message lacks version?" A: version every message.
- `unseq_event` "event lacks monotonic seq?" A: seq; reconnect = snapshot+seq.
- `command_unacked` "command w/o result event carrying its id?" A: ∀command → result.
- `command_not_idempotent` "re-sending a command id applies it twice?" A: dedupe by id.
- `viewer_unrecorded` "command/form answer stored w/o viewer identity?" A: tailscale identity on every command.
- `demo_knows_tidepool` A: demo ⊥ tidepool.

### server-ws
- `polls_instead_of_push` A: push only.
- `one_lane` "deltas and state changes on one queue?" A: lossy delta lane + lossless state lane w/ resync marker.
- `fanout_blocks` "fan-out awaits a slow client w/o bounded channel?" A: bounded per client.
- `no_origin_check` "ws upgrade w/o origin check?" A: check.
- `no_ping` "no ping/pong or idle timeout on ws?" A: both.
- `snapshot_missing` "reconnect has no snapshot endpoint keyed by seq?" A: snapshot+seq.

### server-live
- `rerun_all` "reruns every subscription per batch rather than touched tables?" A: touched tables only.
- `full_resend` "pushes full result set instead of diff?" A: diff by row key.
- `sub_leak` "subscriptions survive socket close?" A: drop.
- `query_from_client_raw` "client sends raw SQL?" A: named queries + params only.

### server-forms
- `handwritten_schema` A: derive like tool schemas.
- `hints_in_schema` A: uiSchema separate.
- `ack_as_answer` A: output = answer; NoAnswer on dismiss/timeout.
- `answer_unvalidated` "form answer settles the job w/o server-side validation against the schema?" A: validate, reject w/ typed error.
- `form_not_inbox` "raised form not emitted as an inbox item w/ node, effort, age?" A: emit.

### server-demo
- `sleep_blocks` A: tokio sleep in the Job future.
- `shell_unbounded` A: cap + blob.
- `sleep_zero_pending` "`sleep 0` comes back pending?" A: must read sync.
- `edit_unconfined` "edit tool writes outside `task.owned`?" A: confine to owned; veto is the admission hook's, not the tool's guess.
- `demo_prod_default` "run/edit tools enabled by default outside dev?" A: dev-only flag.
- `run_not_freeform` "`run` declared as a json function tool instead of an async freeform custom tool?" A: cell shape: raw text, async.
- `run_no_progress` "`run` never sends progress envelopes (`<path> call <handle>`) for a long script?" A: interim results as envelopes, one output at the end.
- `job_verbs_missing` "`CallContext` lacks `JobVerbs` (spawn/send/followup/checkpoint/set_effort/envelope callable from inside a job, in Rust, with the tools' result enums), or the demo wires a script to them?" A: crate provides + tests them in Rust; demo pipes 1:1 to tools and uses only `envelope`.
- `demo_is_toy` "demo provider written as a scripted/throwaway transcript instead of the trait's reference implementation running a real tree?" A: it is the acceptance provider; keep it real.
- `handoff_not_forced` "demo `Compactor` lets the model answer in prose or call other tools during compaction?" A: one forced strict `handoff` call.

### web (all screen leaves)
- `components_before_principles` A: principles+tokens first.
- `kit_default_look` (0.75) "color literal / font-family / px size not a token reference?" A: tokens.
- `single_pattern` (0.75, kind=commit) "commit names fewer than two of {commit-graph, trace-view, commit-log, notebook, chat, email, issue-tracker, launcher, REPL}?" A: both + the boundary.
- `renders_on_complete` A: render deltas live; swap in completed item.
- `panel_not_query` A: query subscription.
- `no_deep_link` A: route ∀node ∀request.
- `color_as_decoration` (0.75) A: color = meaning only.
- `any_or_handwritten_types` A: strict TS; generated types.
- `mouse_only` A: keyboard + visible focus.
- `layout_shift` A: reserve space.
- `tree_rerender_per_delta` "delta updates re-render the whole tree/list?" A: memoize; virtualize.
- `seq_ignored` "client applies events w/o checking seq gaps?" A: gap → resync.
- `state_from_deltas` "client derives durable state from deltas rather than the state lane/queries?" A: deltas = display only.
- `per_token_dom` "streaming text creates a DOM node per delta/token, or updates outside `requestAnimationFrame`?" A: one text node per message; rAF batch.
- `shrinks_on_complete` "completed item replaces the streaming view with a shorter box while the user reads?" A: grow only; same box.
- `spinner` A: skeleton w/ final dimensions. `toast_for_state` A: state in place. `modal_for_state` A: inline; Dialog only for destructive confirms (none in wave1).
- `chat_bubbles` "message styling uses bubbles/avatars/rounded chat cards?" A: notebook cell + chat composer; instrument, not messenger.
- `cards_grid` "dashboard cards in a grid?" A: dense rows + one graphic.
- `legend_not_labels` "chart carries a legend instead of direct labels?" A: direct-label; same color = same meaning everywhere.
- `gridlines_heavy` "horizontal gridlines or more than major ticks?" A: sparse major ticks only.
- `dom_shared` "d3 and React both mutate one DOM subtree?" A: d3 computes, React renders; or a canvas island w/ React HUD.
- `no_canvas_path` (tree/timeline) "graph component has no canvas backend/hit-test for large node counts?" A: one component, two backends, quadtree hit-test.
- `scroll_layout_read` "layout read (`getBoundingClientRect`, `offsetHeight`) in a scroll/resize handler?" A: measure once; rAF.
- `optimistic_fake` "command UI shows the result before its result event arrives?" A: show 'sent'; reconcile by event.
- `no_empty_state` "list/screen renders blank when empty?" A: empty state that teaches + shortcut.
- `refusal_hidden` "a typed refusal (cold prefix, not owned) surfaces as a generic error?" A: quiet notice w/ the accept action; same shape the model sees.
- `url_not_state` "pane width / open tabs / brush not in the URL?" A: URL = pane state; back works.
- `font_shift` "web font without metric-compatible fallback or subsetting?" A: no layout shift.
- `hand_shortcuts` "shortcuts not registered in the palette's command registry?" A: discoverable.
- S: `visual_craft` (ts_component) 1 defaults + literals · 2 tokens, no hierarchy · 3 hierarchy, some decoration · 4 dense, direct-labeled, monochrome + meaning color · 5 + streaming stable, keyboard complete, both themes pass contrast. advise <4.

### web-principles
- `no_pairs` A: pattern pair per screen + school.
- `tokens_not_data` A: tokens = data → css vars.
- `no_bundle_budget` "principles lack a bundle/latency budget?" A: state both.
- `no_contrast_rule` "no contrast rule for meaning colors on monochrome?" A: state it.
- S: `screen_questions` 1–3 (who/knows/does per screen). advise <3.

### web-tree / web-timeline / web-window / web-inbox / web-palette
- tree: `depth_as_y` A: y=time. `prefix_invisible` A: shared lane segment. `pending_invisible` A: open span. `no_effort_mark` "node doesn't show its effort level?" A: show (blue ramp). `idle_uncompressed` "a 40-minute wait takes 40 minutes of axis?" A: compress idle intervals w/ a break glyph. `no_semantic_zoom` A: labels past 1×, hashes past 2×. `no_lineage_hover` "hover doesn't highlight ancestors + descendants?" A: lineage highlight. `no_minimap` A: minimap. `brush_unlinked` "time brush doesn't filter the timeline tab?" A: link.
- timeline: `span_ends_at_stop` A: to settle time. `span_no_expand` A: expands to request. `wait_invisible` "paused (wait_agent) interval not drawn?" A: hatched pause span. `no_sparkline` "no per-node usage sparkline (tokens, cached fraction)?" A: thin sparkline above the row. `no_crosshair` A: crosshair w/ relative time; absolute on hover. `no_table_alt` "graphic has no table alternative one keystroke away?" A: `t`.
- window: `form_modal` A: inline cell. `no_stream_cell` A: stream into cell. `reasoning_hidden` "reasoning summary deltas not shown while streaming?" A: dim collapsed block; auto-collapse. `args_not_mono` "tool-call args / ids / code not in the mono face?" A: mono. `progress_loud` "job progress envelopes render as full messages?" A: quiet inline line under the call. `no_caret` A: caret block while streaming.
- inbox: `inbox_per_node` A: tree-wide. `item_missing_fields` A: node/effort/age. `no_keyboard` A: keyboard. `no_age_sort` "not sorted by age/urgency?" A: sort. `no_grouping` "items not groupable by node/kind?" A: group toggle. `no_quick_answer` "an ask/form can't be answered from the inbox row?" A: answer in place, `enter`.
- palette: `no_args` A: args + results in place. `no_query` A: runs a query. `no_command_registry` "commands hardcoded in the palette rather than a registry ∀command?" A: registry. `no_fuzzy` A: fuzzy match on name + shortcut. `no_recent` A: recent commands first.

### lead (and root)
- C: `fork_readiness` (kind=fork) `ready` | `uncommitted` | `failing` | `unknown` | `not_a_fork`. advice: check → commit → fork.
- S: `assignment` (kind=fork) 1–5: task · +module · +files · +commit · +label +acceptance. advise <5.
- `label_off_table` A: use the table.
- `unrecorded_amendment` (kind=edit_contract) A: `amend(<label>):` commit; tell every child.
- `integration_unchecked` (kind=reply) A: check after merge.
- `sideways_fix` A: ⇑/⇓ only.
- `merge_order_wrong` "merges a child before the children it depends on?" A: dep order.
- `review_skipped` (kind=reply) "`recent_calls` show no read of each child's reply vs scaffold before integrate?" A: one review cycle.
- **on child reply** (tool result = a child's Outcome): S: `child_evidence` 1–5 (claims · checks named · +results · +oids · +left-undone+amendments+interview). `<4` → A: send back for evidence before merging. `child_amendment_pending` "child's reply requests a contract amendment not yet committed?" A: amend first. `child_scope_creep` "child's commits touch files outside its label's module?" E.

### root
- `order_unrecorded` (fork of server/web) A: record core-first choice.
- `scaffold_incomplete` (kind=commit, `scaffold(root)`) "scaffold lacks any of: workspace Cargo.toml, 3 crates, web/package.json, schema/ gen step, one test per crate?" A: tree.md `root`.
- `acceptance_not_recorded` (kind=reply) "final reply lacks the 10-item acceptance transcript w/ store queries?" A: record it.

## L4 quality scores (artifact ≠ nothing; threshold → advise w/ the level's gap)
- S: `type_discipline` (rust) 1 raw strings/Value for ids+kinds · 2 some newtypes · 3 newtypes for ids, strings elsewhere · 4 closed enums + newtypes, some unwrap · 5 fully typed, typed errors, exhaustive matches. advise <4.
- S: `error_handling` (rust) 1 unwrap/panic · 2 `Box<dyn Error>`/anyhow in lib · 3 thiserror but lossy · 4 typed per module w/ ids · 5 + retryable/terminal classified. advise <4.
- S: `async_hygiene` (rust) 1 blocking in async · 2 unbounded channels · 3 bounded, orphan tasks · 4 scoped, no cancel hierarchy · 5 token hierarchy, drop=cancel. advise <4.
- S: `doc_why` 1 none · 2 what only · 3 why on pub items · 4 + invariants stated · 5 + PRD rule cited. advise <3.
- S: `test_focus` (rust_test) 1 none · 2 smoke · 3 asserts the invariant · 4 + edge (empty/duplicate/late) · 5 + replay fixture. advise <3.
- S: `sql_rigor` (sql) 1 no constraints · 2 PKs · 3 FK+NOT NULL · 4 + CHECK/UNIQUE · 5 + indexes for every query path. advise <4.
- S: `ts_strictness` (ts_*) 1 any · 2 loose · 3 strict, hand types · 4 generated types · 5 + exhaustive switch on enums. advise <4.
- S: `a11y` (ts_component) 1 mouse only · 2 tabbable · 3 + visible focus · 4 + aria on graph · 5 + reduced-motion + contrast. advise <3.
- S: `hot_path` (rust_lib in scheduler/transport/ws) 1 clones per subscriber · 2 re-serializes · 3 Arc'd, some copies · 4 zero-copy fan-out · 5 + allocation-free delta path. advise <3.

## L5 trajectory scores (over `recent_calls`)
- S: `progress` 1 churn on same lines · 2 exploring w/o edits (fine in orient) · 3 edits landing · 4 edits + checks · 5 edits + checks + commit. advise ≤1: name what's churning.
- S: `evidence_before_edit` 1 edits w/o reading target · 3 reads then edits · 5 reads scaffold doc + target. advise ≤1.
- S: `check_gap` (calls since last passing check) 1 ≥8 · 3 4–7 · 5 ≤3. advise ≤1.
- N: `blocked_silent` (phase=blocked) "same failure ≥3 times in `recent_calls` w/o an `ask`?" A: ask ⇑ w/ default + blocks. host-side rejections (held lock, `ReplyUpdatePending`) are not failures here.

## L6 selection
- C: `show` "Which single advice, if given now, would most change the quality of the child's next call?" alts = every advice key in this packet (as authored, wording = its condition) + `nothing`. handler uses the pick only if that candidate tripped.

## L7 escalations (strict policy)
- `edits_contract`, `child_scope_creep`, `destructive_command` (Project.Watchdog), `scaffold_diverged` (lead) "child's commits redefine a type the scaffold defines?" E.

## counts (approx per call)
L0 3 · L1 ~17 · L2 4×files (cap `paths` at 6 ⇒ ≤24) · L3 10–35 (web screen leaves are the widest) · L4 2–4 · L5 4 · L6 1 · L7 2–4 ⇒ 40–95. under 100 ! if a packet would exceed, drop L2 rows beyond the first 4 files, then L4 scores already at level 5 last time.
