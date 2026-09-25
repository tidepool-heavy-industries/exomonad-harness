# dogfood requirements (waves 5 + 6, 2026-09-24/25)

fmt: as PRD. `ev` = evidence (run, number, where). `PRD` = the section + rule that already covers it. `GAP` = what the PRD does not yet say and the evidence requires. amendments at the end are proposals; PRD.md is unchanged.

sources: `~/dev/tidepool/plans/next-wave-inputs.md` (from "Delivery fence on a hosted input" onward), `~/dev/tidepool/plans/wave6-root-interview-digest.md`, `docs/exomonad-interviews.md` (wave 5 root, rounds 1+2), `docs/exomonad-friction.md` (wave 6 root). every problem below was seen on the Codex-hosted path; the question per row is whether this crate removes it by construction, covers it with a primitive, or still owes a rule.

## mailbox

**queued input shown only at turn end, one per turn end**
- ev: wave 6, Codex `ext/queue/src/service.rs` delivers only from `dispatch_if_idle` + a 10 s poll that returns while the thread runs, and stops after one item ⇒ `startOrSteer` never steers. notifications, request updates, assignments to a busy actor waited 3.5 to 51 min; 5 of 6 UpdatePending refusals came before the update was shown.
- PRD: `mailbox` delivery classes: "`AtBoundary` (next request, after outputs; default)"; `agent verbs` wait_agent: "ALL function outputs first ..., THEN envelopes in arrival order".
- GAP: "boundary" is not defined against "turn". must say ! boundary = EVERY request of the target (each tool round-trip), never the derived turn end; a request carries ALL unread AtBoundary envelopes, not one. test: 3 envelopes arrive during a 3-call tool chain ⇒ each lands in the next request, none waits for a stop.

**delivery fence (inbound path hung for the rest of the run)**
- ev: wave 5 core lead, 27 min "Working but idle" (operator estimate). chain: pump submits StartOrSteer (35 s deadline) → TUI `before_input` cancels the active `haskell` call instead of queueing → host answers NotSleeping for a computing cell → no production caller of `complete_from_call`, gate never wakes → host marks row Unconfirmed, re-queries every 1 s, EvidenceUnavailable collapsed to Unknown = "still pending", no withdraw, no timeout; every later message queued behind it. recovery = operator paste into the composer.
- PRD: `mailbox`: "everything that reaches a conversation between requests is a CALL OUTPUT ... or an ENVELOPE"; `gpt-6 only`: "no interrupt path on HTTP: user input mid-request = item appended before next request. only abort = cancel ..., always → typed output"; `Steer` is "operator only; WebSocket lane only; HTTP ⇒ downgraded to AtBoundary"; store: envelope rows w/ "delivered_at request".
- GAP: the fence is impossible here only if these hold, and the PRD states none of them as `!`: (1) delivering an envelope never cancels, waits on, or is ordered behind a job (a computing cell and an arriving envelope are independent); (2) delivery is a store write in-process: an envelope row is `unread` or `delivered_at R`, no third "unknown admission" state exists; (3) Steer never cancels a job, only the in-flight response. write them as invariants w/ one test each (envelope arrives while a slow job runs ⇒ delivered at the next request, job untouched).

**message states: receipt is not presentation**
- ev: wave 5 root: `sendMessage` returned `NotificationReceipt` while core's inbox was fenced; wave 6: preflight nonce `submitted/not-presented` until the lead quoted it; root asked for "receipt, queued, presented, read/acknowledged, incorporated, or fenced" as its one change (interview Q11, round 2 shape A `MessagePhase`).
- PRD: `mailbox` store: "envelope rows (sender, recipient, class, item hash of the rendered message, delivered_at request). queries: `inbox(path)`, `unread(path)`."
- GAP: an envelope has no identity the sender holds. needed: `send_message`/`followup_task` return `{ref}`; states as data: `queued` (row, no delivered_at) | `presented{request}` (delivered_at, a crate fact) | `acknowledged{request}` (recipient's later verb call names the ref) | `incorporated{commit|decision}` (provider-attested, never inferred from presentation). `message_state(ref)` query. fenced is not a state here (see invariants above); `held{class: Hold, since}` is.

**replace before delivery**
- ev: wave 5 root sent core a literal `$(git rev-parse HEAD)` then a second message w/ `b99337e`; both queued, contradictory (interview Q3, shape B).
- PRD: none; `store` "nothing rewritten in place !".
- GAP: `send_message{..., replaces: ref}` → `Replaced{ref'}` when `ref` is unread (old row marked withdrawn by a new row, never delivered; no in-place rewrite) | `LinkedCorrection{ref'}` when already presented (new envelope rendered w/ "corrects <ref>"). never a silent retraction of presented text.

**status noise vs changed facts**
- ev: wave 6 root Q5: the per-child line `inbox=open; last_message=ref2 submitted/not-presented; source=d0245b3; next=await-event` steered decisions; "full roster dumps were mostly noise once state was unchanged". wave 5 wanted `core-lead req=1 pending; provider=idle 27m; inbox=...; last_message=ref9 ...; source=...; next=...` (shape H).
- PRD: `agent verbs`: "`status` is NOT a verb: usage/budget/warmth arrive as `/harness` envelopes at boundaries when they change"; `list_agents{prefix?}`: "tree + statuses + last task message".
- GAP: `list_agents` row lacks: conversation state + since (idle 27m), open task ref, last envelope ref + state, unread count + oldest age, checkout revision. `/harness` child notices carry only the fields that changed since the last notice delivered to that recipient (a diff, never a roster).

## agent verbs

**update pending at reply time (the reply-while-update-pending rule)**
- ev: wave 6: `ReplyError::UpdatePending` refused child replies while a parent `updateRequest` was undelivered; refusal text now says "end your turn now, read the update, then send this again" (tidepool 4dcb4d7d2). 5 of 6 refusals preceded presentation. wave 5: `updateRequest` receipt `UpdateUnconfirmed`, root verified only via the child's reply + cumulative diff.
- PRD: `followup_task{target, task}`: "envelope that starts a request if the target is idle, else AtBoundary; ONE FINAL_ANSWER per followup".
- GAP: no rule for a FINAL_ANSWER produced by a request that did not contain a pending followup for the same task. proposed rule ! never refuse: the final answer is delivered carrying `seen_through: <envelope seq>` and `unseen: [ref]`; the followup is delivered at the child's next request (followup_task starts one, the child is idle); the parent's child-reply hook sees both and may `ReturnToChild`. no retry loop exists because no envelope waits for a turn end.

**versioned assignment update**
- ev: wave 5 probe ownership correction needed `sendMessage` + `updateRequest` + two repair rounds; root wants "new owned paths/acceptance/source, an effective-from boundary, a presentation ack and a separate incorporation ack", never rewriting work committed under the old version (shape C).
- PRD: `spawn_agent`: "`task` = contract as data: {clauses, acceptance, owned, mustNot, introduces, consumes, boundaries}"; admission veto "derives from owned/mustNot".
- GAP: contract has no version. needed: `followup_task` may carry a contract delta → new contract version row; effective-from = the target's next request (a job already admitted keeps the version it was admitted under); tool-call-admission reads the version in force at admission; presentation + incorporation of the version use the message states above.

**parent handle and task rendering (first-turn orientation)**
- ev: waves 5+6 rollouts: median time to first productive action root 28 s / 4.5 calls, Luna 68 s / 10 → 86 s / 13 in wave 6. `inspectFull sessionInput` on 14 of 14 children because activation text truncated the Task ("additional detail omitted"); one child returned Blocked on the truncated text. `parentAgent` was Nothing for selected-context children: 3 to 10 calls per Luna hunting the parent, checkpoints silently dropped. 8 actors looked up reply constructors; ambiguous `Blocked`/`Accepted` failed 5 cells.
- PRD: `agent verbs` names: "children `/root/<task_name>/<task_name>`"; NEW_TASK envelope = "child sees a rendering"; developer item: "identity path, slot count"; typed final answers: "child's FINAL_ANSWER = forced strict call to `finalize` w/ that schema".
- GAP: (1) the developer item names the parent path too (a path makes it derivable; the model should not derive it); parent is independent of `from` (`prompt | here | checkpoint`). (2) ! NEW_TASK renders the whole contract, never truncated; over a size limit ⇒ `spawn_agent` returns `Refused{task_too_large, tokens}`, never a clipped rendering. (3) constructor lookups vanish by construction once the reply shape is the `finalize` strict schema on the wire: acceptance test = a child produces a typed Blocked/Accepted w/o any lookup call.

**root's own checkpoint target**
- ev: wave 6 root Q4: "no parent `sendMessage` target", so it sent no admission checkpoint after its own fork cells; its decisions stayed pane text. interim: NEXT.md obligations table.
- PRD: `mailbox` senders: "`/operator` (user input IS an envelope; a form = `followup_task` to `/operator` ...; the inbox screen = the operator's mailbox)".
- GAP: say ! `/operator` is `/root`'s parent: `send_message{target: /operator}` is valid from `/root` and lands in the inbox screen as a MESSAGE; `/root`'s FINAL_ANSWER goes to `/operator`. developer item for `/root` names it.

**batched admission / fork latency**
- ev: wave 5 fork cells 54.7 s (root) and 39.0 s (core lead): ~40 s memo misses + ~22 s split-compile stale retries + ~2 s per child unlogged capture; root wanted "one batched admission call ... fast return that does not wait for each child's provider startup" (shape D).
- PRD: `spawn_agent`: "returns `{task_name}` immediately"; `here`: "N spawns in one cell = N siblings, one shared prefix, harness batches"; `decided`: "prewarm ... when a child is admitted while capacity is free"; `JobVerbs` spawn_agent callable from inside a job.
- GAP: small. `JobVerbs::spawn_agent` returns `Admitted{path} | Refused{..}` synchronously per child, before any child request; child readiness = its first request row, surfaced as an event the parent can wait on (not a second return value). the compile cost itself is provider-side (see `not the crate's`).

## loop

**turn ends without a reply; standing goal**
- ev: waves 0 and 6: a provider turn ended with the actor's own request still open; workspace code could not observe it (only `afterTool` exists; turn completion is engine-only). wave 5 root's goal text + triggers: idle child w/ pending request, unreviewed leaf commit, before a destructive reset, before a final answer implying completion; "cite the changed fact and the next owner, not replay the whole plan" (shape E).
- PRD: `hooks` model-stopped: `Stop{path, phase, pending: [CallId], mailbox: [Envelope]}` → `Continue | Inject(item) | SetEffort | Compact | Spawn(..) | Pause`; `loop`: "'turn' = derived view".
- GAP: (1) Stop event lacks the open obligation: the conversation's unanswered task ref (NEW_TASK/followup w/o FINAL_ANSWER), children w/ open tasks, and the event seq range since the previous Stop (so a reminder cites the changed fact). (2) default decision for "stopped, phase ≠ final_answer, no pending job, task open" is unstated; proposed: pass-through = the harness delivers `/harness` one-line notice once, a second identical stop ⇒ parent gets a typed `Stopped{no_final_answer}` envelope. (3) a standing goal is provider State; the hook is its trigger. nothing more in the crate.

## decisions (settled) / provider trait: ∀tool async

**background commands and non-blocking cells**
- ev: wave 6 root Q10: wants a background long compile / focused test / live trace while reviewing; notice must carry handle, exact command, source revision, exit code or signal, terminal + cleanup flag, output-complete flag, bounded tail, durable full-output reference w/ a no-rerun read; named risk: merging while a check on the older source runs. wave 5 Q7: "start it, do independent work, then receive a retained typed result"; streaming tails alone "still pin the model turn".
- PRD: `decisions`: "∀tool async ! run(tool) → Job (pollable, settles→Output, cancellable→typed Cancelled) ... no sync path !"; `agent verbs` wait_agent "resumed by job settled | envelope | cancel"; `limits`: "per-job output cap (rest → blob)"; progress = `JobVerbs::envelope(progress)`.
- GAP: (1) job start row records a provider-supplied `source` stamp (checkout OID at start) beside handle + args; its output carries it back. (2) the blob beyond the output cap is addressable: `job_output(handle, range)` query/verb reads it w/o rerun. (3) tool-call-admission event includes pending jobs on the same checkout (handle, name, source) so a provider can refuse a mutating call while a check on that checkout runs. staleness judgment ("does this pass count for revision X") stays provider.

## hooks

**ownership gate (cumulative, base to candidate)**
- ev: wave 5: probe tip `77c6372` touched only owned paths but ancestor `a1b10c3` changed shared `docs/findings.md`; caught only by the root's cumulative diff. wave 6: a settings leaf rebased onto root master; its assignment-base diff carried root docs/demo, lead refused. root: "base-to-candidate ownership gate with source-bound review/integration states, not inferred from branch names" (shape F).
- PRD: `hooks` child-reply: `Reply{child, envelope, diff_vs_base, contract}` → `Deliver | Deliver{annotation} | ReturnToChild{followup_task}`; `decisions` checkout: "edit outside `owned` ⇒ veto at admission".
- GAP: `diff_vs_base` is undefined. say ! base = the contract version's recorded base OID (captured at spawn, re-captured per version); diff = cumulative base..tip (every commit, not the last), per path w/ owned|mustNot|unowned verdict, as data. the review/merge stage machine (committed → reviewed → merged → verified) is provider State, not crate.

**destructive command hold**
- ev: wave 5: `git reset --hard master` dropped committed interview prose at `6667ddc` while `git status --short` was empty; the watchdog alerted after the fact. root wants a pre-execution hold naming the OID to be discarded (shape G).
- PRD: `hooks` tool-call-admission: `ToolCall{path, call_id, name, args, owned, mustNot}` → `Admit | Veto{reason} | RewriteInput{args} | Escalate{to: path, note}`.
- GAP: none in the crate. the veto text is the provider's. (the checkout state the provider needs is its own to read.)

**watchdog (after-tool) and its false positive**
- ev: wave 5: `destructive_command` (0.9) fired on core's `sendMessage` because the message TEXT mentioned a reset; no nudge prevented a mistake this run.
- PRD: `hooks` tool-result: `ToolResult` → `NoAnnotation | Annotated{text} | Pruned{text, handle}` "(today's `afterTool`)"; admission event carries the typed tool name.
- GAP: `ToolResult` event must carry the originating call (name, args, call_id), so a classifier sees "this was send_message" before scanning text. otherwise covered.

## store

**revision identities**
- ev: wave 6 root Q7 ("most annoying unasked friction"): six partially independent identities (operator checkout, child source head, assignment base, child branch tip, review seed, installed tool layer) desync silently; a clean `git rebase master` still invalidates a candidate against its base.
- PRD: `store` queries: `seen_by`, `pending_at`, `usage_subtree`, `siblings`, `inbox`, `decisions`, `children_of`; "∀query = a named fn w/ a typed row".
- GAP: `revision(path)` query: checkout head, contract base (per version), branch tip, provider spec version the conversation runs under, tools-list hash, compaction window id. the crate stores what it records (spec version, tools hash, window, job source stamps); OIDs come from the provider via `State`/job stamps. "what would this action publish" = provider, over this row.

## gpt-6 only / provider trait: cached prefix and reload

**reload is not atomic**
- ev: wave 6 root Q3: `reload_agent_spec` refused (`prepared engine: missing imported value Project.Shell.presentSelected`) AFTER publishing the new source layer; the old typed tool record stayed active; "after the reload" became ambiguous.
- PRD: `gpt-6 only`: "instructions, tool list, system items fixed per conversation root; ... changing them = new root"; settings items: `tools` → "`additional_tools` item".
- GAP: no provider-spec swap is specified. say ! a spec change (tool schemas + hook impls + model-facing text) is one transaction: validated fully, then committed as a new spec version row and applied from the next request (additional_tools or new root, per the cache rule), else refused w/ the old version untouched and the failing symbol named. the conversation learns the change as a `/harness` envelope.

## not the crate's (recorded so no amendment is invented)
- cell memo unsoundness (compiled items tied to value generation; decision: no cache) and split-compile stale retries: provider/runtime latency. the crate's part is only that a slow cell is a Job and never blocks the turn.
- first-call-ready brief, one-page next-run entry, CommitReview prompt branch, manifest ownership in the admission checklist, PRD section paths: brief/workspace text.
- per-child 2 s capture in `prepare_captured`: provider spawn cost.

## ranked PRD amendments (proposals)
1. `mailbox`: fence impossible by construction: AtBoundary = every request, all unread envelopes per request; delivery never cancels/waits on a job; envelope row is unread | delivered_at, no unknown state.
2. `mailbox`: envelope ref returned by send_message/followup_task; states queued | presented{request} | acknowledged{request} | incorporated{commit|decision} | held; `message_state(ref)` query.
3. `hooks` (model-stopped): Stop event carries open task ref, children w/ open tasks, event seq range since last stop; default for "stopped w/o final answer, nothing pending, task open" = one `/harness` notice, then `Stopped{no_final_answer}` to parent.
4. `agent verbs` (followup_task): reply-while-update-pending rule: never refuse; FINAL_ANSWER carries `seen_through` + `unseen`, followup delivered next request, child-reply hook decides.
5. `hooks` (child-reply): `diff_vs_base` = cumulative from the contract version's base OID, per-path owned verdict, as data.
6. `agent verbs` (spawn_agent/followup_task): contract versions w/ effective-from = target's next request; admitted jobs keep their version.
7. `agent verbs` (send_message): `replaces: ref` → `Replaced` if unread | `LinkedCorrection` if presented.
8. `agent verbs` (names/developer item): `/operator` is `/root`'s parent; developer item names the parent path; NEW_TASK renders the whole contract or spawn is `Refused{task_too_large}`.
9. `decisions`/`store`: job start row stamps provider `source`; overflow blob readable by `job_output(handle, range)`; admission event lists pending jobs on the same checkout; `revision(path)` query.
10. `gpt-6 only` (cached prefix): provider spec swap is one transaction w/ a version row, applied from the next request or refused whole.

## removed by construction once amendments 1, 4, 8 land (no exomonad-side workaround survives)
- delivery fence: nothing sits between an envelope row and the next request.
- turn-end-only delivery, one per turn end: boundary = every request.
- UpdatePending refusal loop: replies are never refused for an unseen update.
- parent hunting and truncated-task reads: parent path + whole contract are in the child's first request.
- reply constructor lookups: the reply shape is the strict `finalize` schema.

## offline acceptance (one focused test per amendment; `ReplayProvider`, `tokio::time::pause`, no inference)
1. slow job pending + 3 envelopes during a 3-call chain ⇒ each in the next request; job output unchanged; no row w/o delivered_at after the chain.
2. send → `queued`; next request → `presented{R}`; recipient verb naming the ref → `acknowledged`; provider attestation → `incorporated`.
3. child stops w/ commentary, no pending, task open ⇒ one `/harness` notice; second stop ⇒ parent receives `Stopped{no_final_answer}`.
4. followup arrives during the child's final request ⇒ parent gets FINAL_ANSWER w/ `unseen: [ref]`; child's next request carries the followup; no refusal item anywhere.
5. branch w/ an out-of-owned ancestor commit and a clean tip ⇒ `diff_vs_base` lists the ancestor's path as unowned.
6. contract delta while a job runs ⇒ the job's admission used v1; next call admitted under v2.
7. replace unread ⇒ original never rendered; replace presented ⇒ second envelope names the first.
8. `/root` send_message to `/operator` lands in the operator inbox; child's first request contains parent path + full contract bytes.
9. job overflowing the output cap ⇒ `job_output` returns the tail w/o re-running; mutating call on the same checkout sees the pending job in its admission event.
10. spec swap w/ a missing symbol ⇒ refused, spec version row unchanged, next request's tools hash unchanged.

## adapter must (exomonad side, `~/dev/tidepool/plans/harness-adoption.md`)
- resident Haskell cells = async freeform custom tool calls; a computing cell is a Job; no input path cancels it (the fence's step 2 must not come back).
- actor verbs → crate verbs: fork/unfold/lunaTask → `spawn_agent` (from = prompt | here | checkpoint), `sendMessage` → `send_message`, `updateRequest` → `followup_task` w/ contract delta, `respond` → `finalize` w/ the reply schema, `pollResponse`/watch → `wait_agent`.
- `afterTool` → tool-result hook; watchdog destructive-command class → tool-call-admission `Escalate`/`Veto` (pre-execution hold w/ the discarded OID in the reason).
- the host's turn-end reminder (lane turn-end-reminder) → model-stopped hook; standing goal = provider State.
- the delivery pump (`controller.rs`, input-control socket, Unconfirmed rows, 1 s re-query) → envelopes; delete the pump, map durable notification rows 1:1 to envelope rows w/ refs.
- Git-from-parent-view cumulative diff → the source of `diff_vs_base`; checkout OIDs → job `source` stamps and `revision(path)` fields.
- `reload_agent_spec` → the spec-swap transaction; the prepared-engine symbol check runs before commit.
- status/run-map per-child line → `list_agents` row + `/harness` change notices.
