# ideas for later (deferred; not wave1; not contract)

fmt: compressed. each idea: what, why it is cheap here, what it needs. nothing here binds a leaf.

## conversations as values (git-shaped operations over the item tree)
all fall out of: content-addressed items + conversation = path + byte-identical replay. none needs a new store table.
- **diff two conversations**: siblings from one checkpoint differ by appended items ⇒ item-level diff = query. web: side-by-side of two forks.
- **cherry-pick an item**: inject a sibling's cell output / envelope into another child as a developer item; the item row already exists.
- **speculative forks / local tournament**: fork one checkpoint at two efforts or two contract renderings; child-reply hook or operator picks; loser retained + queryable. cache makes fork #2 ~free at the prefix. needs: a `compare` view + a hook decision `Prefer(child)`.
- **replay against a new model/prompt**: resend a stored conversation w/ one change (model snapshot, tool description text, developer item) and measure divergence ⇒ regression tests for model-facing text against real history. needs: replay job type + divergence query (first differing item).
- **edit and rerun** from the web view: fork any past request with an edited user message / injected item; notebook pattern on a conversation. needs: command `fork_with{request, replace|inject}`.
- **bisect** a failure across a fork chain: which appended item first made a check fail. code, no model.

## reflection as typed queries (consumer-side, once harness + swarm share a process)
- today's "last n turns as text" becomes queries over the store: items by address, envelopes in/out, `decision` rows w/ evidence, jobs w/ timings, usage + cached fraction per request; a parent gets the same views over its children. crate side needs nothing new beyond the store queries already listed plus a `children_of(path)` view.
- composition query: for a named helper/tool, which other calls precede or follow it within one cell or one request, ranked. this is what turns "I noticed X is always used with reads" into a number, and is the first step of the helper → helper method → hook → tool pipeline (`~/dev/tidepool/plans/harness-adoption.md`).

## other deferred
- consult forks across families (fork to astra at a checkpoint for one decision, MESSAGE back, stop): a pattern to document, not a primitive.
- coalescing envelopes from many children into one sectioned envelope per debounce window (code, not summary).
- standing subscriptions: lead asks to be messaged when a child's check passes / path changes; harness evaluates rules; replaces polling `list_agents`.
- budgets as data per node (tokens/$/wall) w/ inherited fractions; threshold ⇒ developer notice; model-stopped hook may lower effort or compact.
- durability across days: parked nodes cost nothing; resume from inbox after restart (restart semantics already in PRD).
