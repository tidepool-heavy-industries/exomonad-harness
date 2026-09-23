# exomonad-harness PRD

fmt: compressed. `→` yields/then. `⊥` no dependency. `!` hard rule. `?` open, must be settled by experiment + written down.

## what
- standalone rust crate. runs gpt-6 conversation vs OpenAI Responses API. dispatches tool calls → `Provider` (supplied by crate user).
- crate owns: auth, transport, streaming, retries, item store, forks, resume, compaction strategy trait, effort pin, tool schemas, async dispatch + wait_agent, agent verbs + envelopes, typed hook points, usage, socket protocol, web view.
- provider owns: what tools + hooks mean. the crate knows nothing of any judgment service; it offers many well-typed hooks (see `hooks`) and stores every decision.
- consumer#1 = exomonad via adapter crate on exomonad side. dep direction: exomonad→harness only. harness ⊥ tidepool ! compiles+tests in seconds !
- wave1 output = the REAL tree-shaped harness (store, scheduler, agent verbs, mailbox, checkpoints, compaction trait, hooks, event stream, web view) w/ a demonstrator provider plugged in: minimal tools, each in a shape exomonad will need (see `demo provider`). not a toy, not a scripted transcript; it stays as the acceptance provider and the reference implementation of the trait.

## why
- current: forked codex. no async tools, astra-only effort gate, subprocess/fork, relay socket for host tools, usage via rollout parsing, every feature = patch on moving upstream.
- gpt-6 async tool calling: call no longer blocks turn. haskell cells / children / long cmds = calls that settle when they settle. design is built around this.
- **separate repo on purpose (strategy, not accident).** tidepool+exomonad = GHC worker + Cranelift + multi-crate workspace; a build/test cycle there is minutes and memory-heavy. this crate has no such dep ⇒ `cargo check -p` in seconds, tests in seconds ⇒ a swarm of models can build it by dogfooding (many small check/commit cycles) without the workspace's overhead. later it is consumed BY exomonad (adapter crate, dep direction exomonad→harness only; see `~/dev/tidepool/plans/harness-adoption.md`). nothing here waits on tidepool; nothing in tidepool changes until the adapter step.
- **the builder is building its own next home.** the agents that run this wave run today inside the forked codex; what they build here is the harness they will run inside afterwards (their cells, their spawns, their wait, their compaction). every decision in this file therefore carries its reason (the `why` after each `!`, the `decided from open questions` and `hooks` notes, the api facts behind each rule) so a builder can derive an implementation strategy from the reason rather than the letter: when a rule and a reason pull apart in the code, the reason wins and the rule is amended in a commit that says so. read the reasons as requirements on YOUR future working conditions: a lost delta is a lost thought of yours, a blocking tool is your own stalled turn, a broken cache prefix is your own cost.

## decisions (settled)
- own repo, generic crate, provider trait w/ assoc types. nothing exomonad-specific in crate.
- ∀tool async ! run(tool) → Job (pollable, settles→Output, cancellable→typed Cancelled). job settled before next request ⇒ delivered in that request (reads sync). no sync path !
- stateless conversations ! full item list each request. crate = only source of truth for item sequence. cache absorbs resend. (forks/resume/late outputs/effort pins reason cleanly only if we own exact bytes.)
- models: gpt-6-astra | gpt-6-sol | gpt-6-luna. effort: low | medium | high. no xhigh/max/ultra (cost) !
- effort mid-conversation via `configuration_update` item; request-level `reasoning.effort` pinned to first update in history (cache) !; two updates never adjacent !; after compaction: drop updates, re-pin with fresh update.
- headless core + one event stream + one command channel. view = web page served by same binary, tailscale iface. no TUI (terminal client = trivial consumer of channel, later, if wanted).
- codex = reference reading only ! copy nothing ! write from API docs + scaffolding.
- primitives not helpers ! crate ships: handle/call, wait_agent, cancel, spawn_agent (fork), checkpoint, set-effort, compaction strategy trait, event stream, queries. anything composable from these in few lines stays in provider.
- one process owns run + whole tree of sessions.
- **agent tree = model-facing, in the shape gpt-6 was trained on** (hosted Multi-agent mode + codex v2 use it; we implement it ourselves, see `agent verbs`). NOT the hosted `multi_agent` mode ! (rejects configuration_update, forces per-agent server compaction, one tool list + one fs for all agents, no worktrees, no provider tools, no hooks).
- checkout: worktree per SUBTREE (lead). leaves share the lead's checkout w/ declared `owned`/`mustNot` paths; edit outside `owned` ⇒ veto at admission (deterministic, no jev); harness commits by pathspec on the child's behalf; `request_permissions`-style widening = ask parent.
- transport: HTTP stateless first. WebSocket mode = later lane (`response.steer` mid-turn user input, `response.inject` settled output into a RUNNING response, ≤32 named `stream_id` lanes/connection, cross-lane fork by response id, `generate:false` warmup). loop delivery designed so a settlement CAN be injected into an in-flight response when transport allows; HTTP path delivers at next request.
- streaming end to end ! no polling anywhere. transport streams SSE → scheduler emits deltas (text, reasoning summary, function-call args) as they arrive → event stream carries deltas → page renders them live. a job STARTS the moment its complete call item arrives, before the response finishes (API supports this). job progress/state changes stream the same way. store commits completed items only; deltas live on the event stream, never in the store. writer batches ≤ ~50ms so live queries follow within a frame or two.

## provider trait
assoc types:
- `Tools`: → tool schemas (name, description, typed args, `output_schema`). exomonad derives from protocol; demo writes by hand. output schemas validated at the boundary and the SAME source generates the web view's TS types.
- `ReplayProvider` (crate, test support): answers model requests from the store and tool calls from recorded outputs ⇒ end-to-end tests w/o API; the same path lets a hook provider answer from stored `decision` evidence.
- `Job`: `Future<Output=Output>`; cancel via token in `CallContext`. every call gets `handle`.
- `Decision`s: one closed enum PER hook (see `hooks` table), never one shared enum.
- `Compactor`: the compaction strategy (see `compaction`); crate ships `Server` + `Select`.
- `State`: per-conversation, copied on fork, persisted.
hooks (typed event in → typed decision out): full catalogue in `hooks`; wave1 implements tool-call-admission, model-stopped, tool-result, compaction; the rest are scaffolded as types + pass-through.
`CallContext` = {handle, call_id, cancel: CancellationToken, progress: sink→event stream (never→model), verbs: JobVerbs}.
`JobVerbs` = the harness verbs callable from INSIDE a running job, in Rust: `spawn_agent`, `send_message`, `followup_task`, `checkpoint`, `set_effort`, `envelope(progress)`. same closed result enums as the model-facing tools. this is the interface exomonad's cell effects will bind to. the crate ships them as Rust methods + a pluggable generic tool capability (a provider tool may forward to any JobVerb); the demo provider pipes them 1:1 (one model-facing tool each, which the crate already provides) and its `run` job uses only `envelope(progress)`. how a script inside a job would reach them is the consumer's business, not the crate's.

## agent verbs (model-facing; crate-provided tools)
- names: `/root`, children `/root/<task_name>/<task_name>`. task_name grammar = lowercase, digits, `_` (trained form) ! our kebab labels accepted on input, canonicalized to `_` in model-facing text. store keeps both.
- `spawn_agent{task_name, from, task}`. `from ∈ {prompt | here | checkpoint(name)}` ! (= hosted `fork_turns: none | all`; no turn counts; checkpoint replaces N). returns `{task_name}` immediately. ONE primitive: from model (tool) and from a cell (effect `spawnAgent :: From -> Contract -> Eff es AgentRef`). retires unfold/errand/withContext.
  - `prompt`: fresh conversation = root instructions + rendered task.
  - `here`: parent items through last completed item + the spawn call + its output. spawns raised inside a cell: child inherits the parent's CLAIM on the pending cell call ⇒ receives the cell's output on the same call_id when it settles; child's Haskell env = the cell's committed snapshot ⇒ child's first request after the cell settles. N spawns in one cell = N siblings, one shared prefix, harness batches.
  - `checkpoint(name)`: fork at named request; claims on settled calls replay stored outputs.
  - fork strip list ! : configuration_updates (child re-pins), watchdog annotations, dropped claims (option ⇒ `Interrupted`). codex strips guardian approvals + developer instructions the same way.
  - `task` = contract as data: {clauses, acceptance, owned, mustNot, introduces, consumes, boundaries}. child sees a rendering (NEW_TASK envelope). admission veto derives from owned/mustNot (crate). the record is passed whole to every hook that concerns the child (see `hooks`); the crate never interprets clauses.
- `send_message{target, message}`: envelope, class AtBoundary, no turn. `followup_task{target, task}`: envelope that starts a request if the target is idle, else AtBoundary; ONE FINAL_ANSWER per followup. (the two verbs = the two internal delivery classes with the same names, see `mailbox`.)
- `wait_agent{}` (was "yield"; only non-async tool; no args): output withheld ! resumed by job settled | envelope | cancel. returns WHICH resumed you, never content ! next request order ! ALL function outputs first (settled jobs on own call_ids, then wait's own `{resumed_by: job <handle> | envelope <sender>}`), THEN envelopes in arrival order (user input is an envelope, so it is covered). matches api guidance "results first, then wait status". debounce ~1s.
- **harness verbs** (crate tools, strict, w/ output schemas; exomonad later exposes the same three as effects — wave1 is standalone, see `integration`):
  - `checkpoint{name}` → `{name, request, tokens, warm_until}`. a checkpoint is ONE thing seen four ways ! fork point (`spawn_agent from: checkpoint`) | explicit cache breakpoint write (root prefix + the 3 most recent checkpoints = the 4 allowed writes) | compaction boundary (`compact keep_since`) | named place in the tree view.
  - `compact{keep_since: <checkpoint>, strategy?}` → `{window_tokens, summary_ref, kept: N}`. summarizes everything BEFORE the checkpoint (provider strategy), keeps everything after it verbatim, re-pins effort. the model manages its own memory instead of hitting a threshold.
  - `set_effort{effort}` → `{effective}`. appends the positional update for the next request (adjacency rule enforced: a second call before a request replaces, never appends).
  - `status` is NOT a verb: usage/budget/warmth arrive as `/harness` envelopes at boundaries when they change.
- **typed refusals** ! every verb's result is a closed enum incl. `Refused{reason, ...}`. cold prefix: the harness knows warmth (30-min ttl × last touch, stored). a verb that would send a cold prefix (`spawn_agent from: checkpoint` on an old one, `followup_task` to a long-idle child) returns `Refused{prefix_cold, cold_since, tokens, estimate}` instead of a request; caller repeats w/ `accept_cost: true`. same shape to the operator (inbox) and to the before-request hook (may waive or deny). frontier settled: harness computes+exposes, hook decides, model sees a typed outcome.
- **typed final answers** ! contract record may carry `reply: JsonSchema`. child's FINAL_ANSWER = forced strict call to `finalize` w/ that schema; harness stores the record AND renders it as the envelope payload. parent's child-reply hook joins fields, not prose. compaction's handoff = the same primitive w/ the provider's Summary type. two features, one mechanism.
- `list_agents{prefix?}`: tree + statuses + last task message (store query).
- `interrupt_agent{target}`: **TODO, not wave0/1** (needs cancel of an in-flight request w/ partial streamed items; semantics in `later`). the tool is declared (schema in the stable tools list, cache) and returns `Refused{not_available}`. wave0/1 ship the two safe cancels: cancel a JOB (typed `Cancelled` on its call_id) and cancel a request BEFORE its first delivered item.
- envelope (trained form, verbatim) ! child→parent and parent→child items are user-role messages:
  ```
  Message Type: NEW_TASK | MESSAGE | FINAL_ANSWER
  Task name: <recipient>
  Sender: <author>
  Payload:
  <payload>
  ```
  child's `final_answer`-phase message ⇒ FINAL_ANSWER envelope in parent's mailbox (NOT a function output) ! tools ⇒ function_call_output on call_id. two channels, never mixed.
- developer item per conversation (versioned model-facing text): identity path, slot count ("There are N available concurrency slots…", driven by scheduler cap), and our own statement of who is capable of what (labels select effort/tools ⇒ the hosted line "all agents equally capable, same tools" is false here; write what is true).
- parent owns shutdown ! a client cancels a subtree only through its root (codex app-server refuses to archive a live internal worker).
- api warns: async tools + parallel tool calls misbehave together in hosted multi-agent mode ⇒ acceptance item 11 exercises slow async tool + spawn in one turn.

## mailbox (two channels, nothing else)
- everything that reaches a conversation between requests is a CALL OUTPUT (exactly one per call_id; tool channel) or an ENVELOPE (many; arrival order; message channel) !
- envelope = `{type: NEW_TASK|MESSAGE|FINAL_ANSWER, recipient: path, sender: path (+ call handle for jobs), payload, class, ts}`. one struct; rendering by sender kind (verified in codex source, `core/src/context/inter_agent_message.rs`) ! agents + jobs ⇒ ASSISTANT-role message carrying the envelope text (hosted mode says "in the analysis channel"; codex approximates w/ assistant role; we do the same) | `/operator` ⇒ USER-role message, plain payload, no envelope header (user input is trained as a user message; the envelope struct is internal) | `/harness` ⇒ developer-role (codex renders budget/time notices as developer). exact texts: `docs/model-facing.md`.
- senders: agent paths | `/operator` (user input IS an envelope; a form = `followup_task` to `/operator` w/ a schema, its answer = the operator's FINAL_ANSWER; the inbox screen = the operator's mailbox) | job progress (`sender: <agent path> call <handle>`; interim results while the single output waits; this is the deferred "streaming outputs" answered w/o breaking one-output-per-call) | `/harness` (usage, budget, warmth, compaction notices).
- delivery classes (closed enum) ! `Steer` (interrupt the running response; operator only; WebSocket lane only; HTTP ⇒ downgraded to AtBoundary) | `AtBoundary` (next request, after outputs; default) | `Hold` (queued until target calls wait_agent or goes idle; low-priority progress, coalesced notices). code sets the default from sender kind; the mailbox-message hook may raise or lower; the crate knows three classes and nothing about how a provider decides.
- coalescing: envelopes of one class from one sender within the debounce window ⇒ one envelope w/ sections (concatenation w/ headers, never a summary).
- store: envelope rows (sender, recipient, class, item hash of the rendered message, delivered_at request). queries: `inbox(path)`, `unread(path)`.

## items
Message{role, phase?} | Reasoning | FunctionCall{call_id,name,args,async:bool} | FunctionCallOutput{call_id,output} | CustomToolCall/Output (freeform, cell tool) | settings items (below) | Compaction (opaque, server-issued).
`phase ∈ {commentary, final_answer}` on assistant messages preserved on replay ! (dropped phase ⇒ preambles read as answers). final_answer = what settles a child's task.
Output ∈ {Value(json), Cancelled{reason}, Interrupted, NoAnswer}. exactly one output per call_id !
item content-addressed (blake3). shared by every request containing it.
replay rule ! every reasoning/call/output item since the last user message is resent with the calls it belongs to.

## settings items (positional; replace request-level fields so the cached prefix survives)
| request-level | positional item |
|---|---|
| `reasoning.effort` | `configuration_update{reasoning:{effort}}` — ONLY field the item has (ref: "Only effort is supported") |
| `tools` | `additional_tools` item; `defer_loading` + tool_search |
| history pruning | `compaction_trigger` (must be FINAL input item) |
- store models these as one item kind `settings` w/ closed variant enum; a future field slots in w/o schema change.
- configuration_update rules ! gpt-6 family; standard single-agent mode (rejected in pro/tournament); never adjacent; not with automatic compaction/truncation; compact endpoint rejects histories containing them; effort string ≤128 bytes (codex cap); harness-authored only — a model or client cannot forge one (provenance flag in store, forged ⇒ dropped). model SEES the item (it is in history) ⇒ it knows its effort changed.
- effort pin restated: request-level `reasoning.effort` = first update in history, forever; later updates positional. fork at other effort = append one update in the child; prefix intact.
- tool availability per request via `tool_choice: {type: allowed_tools, mode, tools}` or `"none"` ! never edit `tools` (cache). <20 tools at turn start (api guidance). per-label tool sets = allowed subsets of one stable list.
- cache: `prompt_cache_options.ttl = "30m"` (only value on gpt-6); never `prompt_cache_retention` (pre-5.6). `prompt_cache_key` = accounting only on gpt-6, not routing. explicit breakpoints (`prompt_cache_options.mode: explicit`, ≤4 writes/request) available: put one after root instructions+tools, one after the contract rendering. min cacheable prefix 1024 visible tokens. cache write 1.25×, read 0.1×.
- reasoning: `reasoning.context: all_turns` default on gpt-6; persisted reasoning reuses only within a model family ⇒ vary EFFORT per fork, not model (a luna child of a sol parent loses reasoning continuity). `none` not on astra. pro mode out of scope.

## store (sqlite, one file, one process)
- request = one call to the model: parent request, appended items since parent, params{model,effort}, response items, usage, ts. key = hash(parent, appended). conversation = path root→leaf. fork = sibling. compaction = request whose summary → range replaced ⇒ DAG.
- item rows by hash; body > threshold → blob dir beside db, row keeps hash+size.
- event = between-requests, ts: job start/settle/cancel, wait/resume, hook decision (→ `decision` row), envelope sent/delivered, effort change, late output after stop, compaction.
- session_state row per conversation.
- writer: ONE task owns the only write conn, batches from channel. loop/forks send rows, never hold db. read pool separate. WAL, synchronous=NORMAL, timed WAL checkpoint (sqlite term, unrelated to the fork-point primitive), short read txns.
- queries (recursive CTE): seen_by(request), pending_at(request), usage_subtree(request), siblings(request), inbox(path), decisions(path), children_of(path). ∀query = a named fn w/ a typed row; the same functions back the web view, the provider's reflection over its own conversation, and later "what happened here" analysis. composition query (which calls co-occur w/ a given tool within a request/job) = later, listed in ideas-later.md.
- db = source of truth. nothing rewritten in place !

## loop
not a loop: a scheduler over three event sources: (1) stream items from the in-flight request, (2) job settlements, (3) commands from the channel. each event appends items and/or decides whether to make the next request.
request → dispatch calls→jobs → deliver settled jobs on original call_ids in the next request → keep requesting while the model keeps calling → model stops (final message, or wait_agent) → job settles after stop ⇒ new request carrying that output → model continues on cached prefix.
"turn" = derived view (interval from user input to a stop w/ no pending jobs), not loop state.
**wait_agent** (crate-provided; the only non-async tool; no arguments; see `agent verbs`): model calls it to pause. output withheld ! conversation = paused until resumed by: job settled | mailbox update | user input | cancel. next request order ! settled job outputs on own call_ids FIRST, then mailbox envelopes, then wait's output `{resumed_by: job <handle> | agent <path> | user_input | cancelled}`, then any user message. debounce: settlements within ~1s ride one request. tool description text (model-facing): "Wait for a pending call, a message from another agent, or new user input. Results arrive on their original calls and messages in their envelopes; this tool returns only what resumed you. Do not wait for results that have already arrived."
ask-user = job. output = the answer, never an ack ! dismissed/timeout ⇒ explicit NoAnswer. (gpt-6 asks non-blocking questions by default; this is the trained shape.)
compaction: see `compaction` section. one experiment remains (server strategy + unanswered call).
forbidden per API docs: async + programmatic tool calling; async + parallel tool calls in hosted multi-agent mode (we test the mix, item 11).
later iteration (not wave1): streaming outputs from one job (N items then the terminal output on the call_id); progress → event stream only for now. later: `response.inject` delivery into a running response (WebSocket lane).

## compaction (pluggable)
- trigger = code ! usage vs model window (`compact_at` fraction, config) | operator command | provider `Decision::Compact` at model-stopped. never jev.
- shape in store: compaction = a request whose parent edge is `compaction` from request R. old window stays queryable; new window = content-addressed items like any other; pending CLAIMS carry over. web view: node boundary.
- one trait, three strategies:
  ```
  trait Compactor { type Summary: JsonSchema+Serialize+DeserializeOwned;
                    async fn compact(&self, cx: CompactContext<'_>) -> Result<NewWindow, CompactError>; }
  CompactContext: items() by address | usage() | pending_calls()
                | typed_turn::<T>(instructions, tool: ToolName) -> T   // tool_choice forced to ONE provider-named tool; its call decoded as T
                | server_compact() -> Vec<Item>                        // compaction_trigger path
  NewWindow { items: Vec<ItemRef | NewItem>, effort: Effort, carried: Vec<CallId> }
  ```
  - `Server` (crate default): strip settings items → `compaction_trigger` last → returned window as-is (never pruned) → fresh configuration_update. opaque, keeps encrypted reasoning, cheapest.
  - `Structured` (demo + exomonad): ONE forced call, no sub-turn ! demo: strict json `handoff` fn w/ params = Summary {progress, decisions, remaining, references}. exomonad: the CELL tool is the handoff; cell must evaluate to a value of the Summary type ⇒ GHC typecheck is the strictness; no standalone summary tool. provider `render(Summary) -> items`.
  - `Select`: keep items by address. crate ships code rules only (user messages, pending calls, settings, last N turns); a provider filter `items -> [ItemRef]` may narrow further (consumer note in `hooks`). composes: select → server.
- deterministic context appended by CODE after the summary, never asked of the model ! bindings live in the resident Haskell env and survive compaction untouched; exomonad's render appends a generated item: live bindings w/ types, worktree OIDs, child paths + pending claims, contract clauses done/undone.
- invariants ∀strategy (harness enforces) ! no configuration_update in new window; fresh pin appended; user's own messages retained verbatim (codex does this; a `Select` rule applied before any strategy, bounded); pending `function_call` items for carried claims present verbatim so a late output on call_id stays valid; opening developer item (versioned text) = "context was compacted; you are the successor; summary follows; live state (bindings, worktrees, children) is listed after it"; trigger request recorded.
- ? experiment kept: does the server-compacted window retain an unanswered function_call? if not, `Server` re-appends carried calls after the compaction item. → `docs/findings.md`.
- codex reference: local path = ordinary turn w/ a prose summary prompt, shown to the successor as a USER message ("Another language model started to solve this problem…"), user messages kept verbatim; remote = compact endpoint; pending calls ⇒ synthetic `aborted`. we differ: typed summary, code-appended state, claims carried.

## gpt-6 only, zero back-compat
- one API, one wire shape: Responses items. no chat-completions, no legacy tool_calls, no per-model capability table. items enum = whole vocabulary.
- no sync dispatch code at all, no fallback for models w/o async.
- one effort mechanism (configuration_update + pin). one compaction path through one trait (automatic server compaction/truncation are never enabled: they reject histories w/ updates; explicit `compaction_trigger` is the server strategy).
- no interrupt path on HTTP: user input mid-request = item appended before next request. only abort = cancel (job or in-flight request), always → typed output. WebSocket lane later adds `response.steer`.
- tool schemas always strict (∀field required, no additionalProperties). derivation built for it.
- reasoning items = history: stateless mode returns `encrypted_content` by default (the `include: reasoning.encrypted_content` flag is legacy, harmless); carry those items in the tree like any other.
- cached prefix by design: instructions, tool list, system items fixed per conversation root; forks share root's cache key; changing them = new root.
- custom tools (freeform text input, async-capable) in scope for the cell tool: haskell source arrives raw, not json-escaped. grammars stay out.
- per-request params in store beyond effort: service_tier, response id (background/resume later). tree can vary tier per node like effort.

## quality bar (∀code, ∀wave)
- type safe ! newtypes for ids (CallId, Handle, RequestId, ItemHash, ConversationId), enums not strings, closed item/output/event/command enums w/ exhaustive matches, typed errors per module (`thiserror`), no `unwrap`/`expect`/`panic` in library code (tests only), no `serde_json::Value` past the wire boundary except tool args by design. TS: `strict`, no `any`, protocol types GENERATED from rust (schemars → json-schema → ts); one source of truth.
- structured ! one responsibility per module; dependency direction: item/model → store → transport → scheduler → server → web; no cycles; `pub` surface minimal and documented; no feature flags for hypothetical needs; no dead code.
- fast ! no blocking calls in async context (sqlite via the writer task / `spawn_blocking` for reads); bounded channels everywhere w/ explicit overflow policy; hot path (stream delta → event → page) allocation-light, no serialization round-trips inside the process; page: virtualized lists, no re-render of the tree on every delta.
- clean ! `cargo fmt`, `clippy -D warnings` + `pedantic` where it is not noise, `eslint` + `prettier`; small functions; names say what, doc comments say why. review reads for these before anything else.
- tests ! focused per module (store keys/queries, scheduler ordering, wait_agent delivery order, envelope routing, claims on fork, schema strictness); recorded transcripts for end-to-end; no broad batteries.
- deterministic ! canonical JSON (stable key order, no float drift) for every hashed or cached byte: item hashes, request bodies, tool list (sorted). replay from store ⇒ byte-identical request. debug assert: a fork's first request = parent prefix + appended. fixtures = recorded transcripts; time via `tokio::time::pause` in tests.
- explicit state ! conversation state = closed enum {idle, requesting, paused(wait_agent), cancelled}; job state enum; transitions in one place; illegal transition = typed error, never silent. ordering: total order of items per conversation; every event carries a monotonic seq; reconnect = snapshot + seq.
- concurrency ! structured: token hierarchy run → conversation → request/job; drop = cancel; no orphan tasks; every spawned task owned by a scope. commands idempotent by id; settlement delivery exactly-once per claimant (dedupe on call_id).
- durable ! rows for items committed before any event references them; store schema versioned w/ numbered migrations from day one; FKs on, NOT NULL, CHECK on enums, UNIQUE on hashes; no string-built SQL, queries = named fns w/ tests. crash → restart: in-flight request → `Interrupted`, jobs resume or settle `Interrupted` per provider, no duplicate rows.
- backpressure ! two lanes on the event stream: lossy delta lane (droppable; final item in store recovers it) and lossless state lane (never dropped; bounded w/ resync marker). slow page never stalls the scheduler.
- limits ! per-job output cap (rest → blob), max pending jobs per conversation, max history size ⇒ compaction trigger, request timeout, stream stall detection (no delta for N s ⇒ failed+retry), blob threshold. all as config w/ defaults, none hardcoded.
- observable ! `tracing` span per request/job/command carrying conversation_id/request_id/call_id; errors carry those ids; error taxonomy retryable vs terminal; latency, tokens, cached_tokens per request stored as data (queryable), not a metrics stack.
- secure ! API key never in store/events/logs/page; viewer identity (tailscale header) recorded on every command + form answer; websocket origin check; file-edit tool path-confined; demo shell marked dev-only; blob dir 0700.
- time ! wall clock UTC ms in store; monotonic for debounce/timeouts; timestamps as data on every row/event.
- model-facing text ! tool descriptions + wait_agent text + envelope format + opening developer items are part of the cached prefix: versioned, reviewed like UI copy, tested for strict schema + stable bytes.
- web quality ! keyboard nav + focus management ∀screen; contrast passes on monochrome + meaning colors; reduced-motion respected; aria labels on graph nodes; input→paint < 50ms; no layout shift while deltas stream; bundle budget stated.
- dev loop ! one `just` entry: fmt, clippy, tests, typecheck, web build; web dev server proxies to core; lockfiles pinned; `cargo deny` (licenses/dupes/advisories); minimal features per dep.

## auth
ChatGPT-subscription login behind `Auth` trait first, because it is the
credential available to this run. Read `~/.codex/auth.json` without modifying,
copying, or refreshing it; Codex owns refresh. Never store or log tokens.
Subscription requests use the streaming-only
`https://chatgpt.com/backend-api/codex/responses` endpoint (`stream: true`,
`store: false`, SSE), with Codex CLI version, originator, account, and stable
session headers. A 401 stops the run for operator attention, not a refresh.
Platform API-key auth (`api.openai.com/v1/responses`) is later behind the same
trait. Subscription tokens lack `api.responses.write`, so the platform endpoint
cannot serve as this run's pre-flight or first transport.

## web view
- served from binary, tailscale iface; `tailscale serve` for https+identity when wanted. multi-viewer. operator view now; arena (generations/handoffs) over many runs later.
- layout: two panes. left = tree sidebar. right = tabs, one per open node, splittable. deep link ∀node,∀request. command palette reaches ∀command.
- screens = combination of TWO familiar UI patterns (one for structure ⊕ one for interaction). each screen names its pair; deviation ⇒ written reason.
  - tree: commit-graph ⊕ trace-view. lanes/branch points; y=time; node's current request = open span.
  - node timeline: trace-view ⊕ commit-log. requests+jobs as spans; span expands → its request.
  - node window: notebook ⊕ chat. turn = cell w/ outputs; form renders inline as output awaiting input; composer streams reply into next cell.
  - inbox: email ⊕ issue-tracker. ∀ awaiting-human across tree; keyboard triage; item carries node, effort, age.
  - palette: launcher ⊕ REPL. commands take args, print results in place; also where operator runs a query.
- forms by effect: provider raises form as job. schema = JSON Schema (same derivation as tools) + uiSchema (widgets/order/hints). page renders; answer settles job as typed Output. ask-user = one-field form.
- live data: ∀panel = query over store. page subscribes; writer knows tables touched per batch → server reruns+diffs affected subscriptions. in-flight text/reasoning/args deltas come from the event stream (not the store) and render as they arrive; the completed item then replaces the delta view.
- design school: functionalist (Rams/Swiss lineage, dense pro tools). strict type scale+grid; monochrome screens; color = meaning only (pending/failed/effort/waiting-on-operator); Tufte for graphs. full brief w/ touchstones, tokens, streaming-text rules, d3 approach (d3 computes, React renders, canvas past ~300 nodes), responsiveness numbers, keyboard map, tooling, anti-patterns: `docs/web.md` (binds web-principles; every screen leaf reads it).
- method: principles doc (school + pattern pair per screen) → tokens before components → ∀screen states {who looks, needs to know, does next} → critique by interview w/ screenshots vs principles → deviations logged w/ reason → principles amended when deviation recurs.

## stack
rust: tokio, tokio-util(cancel); reqwest(rustls)+eventsource-stream; serde/serde_json; schemars; blake3; rusqlite(bundled), WAL; axum(ws+static)+rust-embed; tracing; thiserror; clap; wiremock for API mocks; recorded transcripts as spot checks.
web: vite, ts (`strict`, `noUncheckedIndexedAccess`), react. radix primitives + own tokens (never Radix Themes' default look); d3 for math (`d3-hierarchy`, `d3-shape`, `d3-zoom`, `d3-brush`, `d3-time`, `d3-quadtree`) w/ React rendering the DOM and a canvas backend past ~300 nodes (xyflow only if the hand-rolled lane layout proves worse; decide in web-principles, record why); `@tanstack/react-virtual` + table; react-jsonschema-form w/ our Radix theme; zustand or a reducer; generated protocol types. tooling: eslint (ts strict, jsx-a11y, react-hooks), prettier, vitest + testing-library, ladle (components vs fixtures), playwright (screenshots + axe), size-limit (≤300 kB gz initial). assets embedded ⇒ one binary. datasette beside page for raw queries.

## demo provider (wave1 output = the REAL harness; only the tools are minimal)
- wave1 produces the real tree-shaped harness: store, scheduler, agent verbs, mailbox, checkpoints, compaction trait, hooks, event stream, web view. the provider plugged in is a demonstrator: few tools, each chosen because it has a shape exomonad will need. NOT a throwaway ! it stays as the crate's acceptance provider and the reference implementation of the trait.
- tools (each = one exomonad shape):
  - `run{script}` freeform ASYNC custom tool (raw text, not json-escaped) executing a shell script as a job = the CELL shape: long-running, progress envelopes from `<path> call <handle>` via `JobVerbs::envelope`, single output at the end. (spawning from inside a job is a `JobVerbs` capability the crate provides and tests in Rust; the demo does not wire it into a script.)
  - `edit{path, patch}` strict, path-confined to `task.owned` = the admission-veto shape.
  - `sleep{duration}` async = pure timing: settles inline at 0, pending otherwise; exercises wait_agent, late delivery after stop, cancel.
  - `ask{question, schema?}` = form to `/operator` (`followup_task`); answer = operator FINAL_ANSWER; NoAnswer on dismiss.
  - the harness verbs (`checkpoint`, `compact`, `set_effort`) + the agent six + `wait_agent` come from the crate, not the provider.
- demo `Compactor` = `Structured` w/ a strict json `handoff` tool (Summary {progress, decisions, remaining, references}); `render` appends code-generated state (open jobs, children, checkpoints) after the summary — the same split exomonad uses for bindings/worktrees.
- demo hooks: tool-call-admission (owned/mustNot veto), tool-result (the watchdog nudge ledger), child-reply (pass-through + a `reply` schema check), compaction. all others pass-through.
- demo children = the same provider at another effort or model; a demo run is a real tree (root → leads → leaves) doing a real multi-file task in a scratch git repo, not a scripted transcript.
- shell/edit marked dev-only; no haskell in the picture; exomonad-blind !

## hooks (crate ⊥ any judgment service ! the crate knows nothing of jev; it knows it must offer MANY well-typed hooks. ∀hook: typed event in → typed decision out; default = pass-through; provider may attach an opaque `evidence` blob to any decision, stored beside it)
| hook | event (in) | decision (out) |
|---|---|---|
| tool-call-admission | `ToolCall{path, call_id, name, args, owned, mustNot}` | `Admit | Veto{reason} | RewriteInput{args} | Escalate{to: path, note}` |
| model-stopped | `Stop{path, phase, pending: [CallId], mailbox: [Envelope]}` | `Continue | Inject(item) | SetEffort | Compact | Spawn(..) | Pause` |
| before-request | `RequestPlan{items, tools_allowed, effort}` | `Send | Send{tools_allowed'} | Inject(item)` |
| streamed-item | `Item` as it completes (never deltas) | `Keep | Annotate{text} | StartJob` (default for calls) |
| child-reply (before delivery to parent) | `Reply{child, envelope, diff_vs_base, contract}` | `Deliver | Deliver{annotation} | ReturnToChild{followup_task}` |
| mailbox-message (before delivery) | `Envelope + target state` | `Steer | AtBoundary | Hold{until}` |
| spawn | `SpawnRequest{from, contract}` | `Admit | Veto{reason} | Admit{contract'}` (validate/complete the contract record) |
| fork | `Fork{parent items, claims}` | `strip: [ItemRef], claims: Keep | Drop` |
| compaction | `Compactor` trait (above) + `Select` filter `items -> [ItemRef]` | `NewWindow` |
| schedule | `Runnable{path, priority hint}` | `Priority(u8)` |
| tool-result (after) | `ToolResult` | `NoAnnotation | Annotated{text} | Pruned{text, handle}` (today's `afterTool`) |
- every decision stored as a `decision` row: hook, event ref (item ids), decision, evidence blob (provider-opaque), ts, latency. queryable; replayable (a provider that answers from stored evidence turns node logic into a pure test). the crate never interprets `evidence`.
- wave1 hooks: tool-call-admission, model-stopped, tool-result, compaction. rest = typed events + pass-through decisions in the scaffold so a provider can fill them without a contract change.
- consumer note (exomonad adapter, NOT crate): this is where a judgment service sits. measured rules for that side: it answers WHICH never WHAT-NEXT; code decides that a boundary was crossed, judgment classifies which clause; child-reply hook runs compiled review joins over the contract record (gate as screen, repair request from item ids, verbatim text); mailbox hook uses an attention score mapped by code to Steer/AtBoundary/Hold; `Select` uses locate-then-confirm pruning; the contract record is what every packet compiles from. watchdog nudges for the dogfood run: `docs/nudges.md`.

## out of scope
custom tools w/ grammars (freeform custom tools are IN scope); hosted tools; programmatic tool calling; hosted `multi_agent` mode; Agents API; pro reasoning mode; anything tidepool-specific; TUI.

## decided from open questions (2026-09-22)
- **claims.** a pending job has a set of claimants = conversations holding its call w/o output. fork inherits the parent's claims by default (option on fork: drop → settle `Interrupted` in the fork). settlement delivers the output to every claimant on the same call_id. a claim on an already-settled job delivers the stored output immediately (so forks from a checkpoint replay cleanly). alt considered: only the continued parent keeps the event, forks get nothing — subsumed by "drop claim".
- **checkpoint** primitive: names a request as a fork point; exomonad exposes it as an effect so agents define their own fork points. fork-from-checkpoint = ordinary fork w/ claims rule above. (primitives line: + checkpoint.)
- **request scheduler**: yes. capacity = 4th event source; cap is over ACTIVE requests (most nodes are paused/supervising at any moment, not requesting); shared 429 backoff; ordering root before leaves; per-request priority field. priority factors: depth (root first) | operator waiting on this node | prefix warmth (continue warm prefixes before they go cold). prewarm (`generate:false`, WebSocket lane later; HTTP: a `prompt_cache_options.prewarm` request) when a child is admitted while capacity is free.
- **integration tension, decided**: the harness verbs and hooks will become tightly integrated with tidepool/exomonad (effects, resident env, worktrees). wave1 stays STANDALONE and exomonad-blind: verbs are crate tools the demo uses; hooks are typed pass-throughs; nothing imports tidepool. dogfooding well matters more than integration now; the adapter (`~/dev/tidepool/plans/harness-adoption.md`) maps tools → effects later w/o changing the crate's contract.
- **adapter**: breaking change, new architecture; new trait, not `InteractiveAgentBackend`.
- **subscription auth**: yes, schedule early.

## later (todo)
- cancel of an in-flight request w/ partial streamed items (⇒ `interrupt_agent`): partial items kept as an `Interrupted` request row, never sent back to the model; next request from the last complete item. TODO after wave1.
- multiple operators: `/operator/<tailscale identity>` as sender path; inbox + audit per person; a form may address one operator. TODO.
- extended prompt cache retention: check availability for gpt-6; if so, option on `checkpoint`. want real cache stats from the target first.
- experiments list (compaction leaf, `docs/findings.md`): unanswered call across server compaction | late output for a call after a fork (claims) | cache hit on fork + appended configuration_update on sol/luna | wait_agent with nothing pending | async tool + spawn in one turn (hosted-mode warning) | explicit cache breakpoints vs implicit.
- test strategy vs live API (record+replay).
- program-driven typed turns (Haskell asks the model for a value of type `a`: `typed_turn` w/ a forced strict `finalize` tool = the compaction primitive exposed). maybe unnecessary: a cell can raise an effect that asks the model. decide after wave1.
- WebSocket lane (steer, inject, named lanes, warmup) once HTTP path is byte-stable.
- history: the retired resident-actor design (`loop :: State -> Harness State`, `deliberate`, fenced Haskell in prose) was dropped because a hosted agent's prose could not safely be executed; owning the loop removes that reason. cells as a freeform custom tool are the safe form of the same idea.
