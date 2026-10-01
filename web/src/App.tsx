import { useEffect, useMemo, useState, type FormEvent } from "react";
import NodeWindow from "./NodeWindow";
import type { CommandReceipt, HostCommand, HostCommandSubmission } from "./protocol";
import type { HarnessViewModel } from "./view-model";
import type { Screen } from "./client-contract";

export type AppProps = {
  data: HarnessViewModel;
  demoFeedback?: string;
  onCommand?: (command: string | HostCommand) => void;
  onRetry?: (submission: HostCommandSubmission) => void;
  pendingCommands?: Array<{ submission: HostCommandSubmission; state: string }>;
  acceptedCommandIds?: readonly string[];
};

const commonScreens: Array<{ id: Screen; title: string; shortcut: string }> = [
  { id: "tree", title: "Tree", shortcut: "g t" },
  { id: "timeline", title: "Timeline", shortcut: "g l" },
  { id: "inbox", title: "Inbox", shortcut: "g i" },
];
const standaloneScreens = [...commonScreens, { id: "command" as const, title: "Command", shortcut: "g c" }];
const embeddedScreens = [...commonScreens, { id: "host" as const, title: "Host", shortcut: "g h" }];

const styles = `
  :root { color-scheme: light dark; --bg:#fff; --fg:#171717; --muted:#595959; --line:#b8b8b8; --soft:#f2f2f2; --accent:#075fc7; --pending:#765000; --failed:#a31313; }
  @media (prefers-color-scheme: dark) { :root { --bg:#151515; --fg:#f2f2f2; --muted:#c1c1c1; --line:#626262; --soft:#242424; --accent:#83baff; --pending:#ffd166; --failed:#ff9292; } }
  * { box-sizing:border-box; }
  body { margin:0; background:var(--bg); color:var(--fg); font:13px/1.4 Inter,ui-sans-serif,system-ui,sans-serif; font-variant-numeric:tabular-nums; }
  button,input { font:inherit; }
  button { color:inherit; }
  .harness { min-height:100vh; display:flex; flex-direction:column; }
  .top { display:flex; align-items:center; gap:20px; min-height:48px; padding:8px 16px; border-bottom:1px solid var(--line); }
  .brand { font-weight:600; letter-spacing:.02em; margin-right:auto; }
  .tabs { display:flex; gap:4px; }
  .tab { border:0; background:transparent; padding:7px 10px; cursor:pointer; border-bottom:2px solid transparent; }
  .tab[aria-current=page] { border-color:var(--accent); font-weight:600; }
  .tab kbd { margin-left:6px; color:var(--muted); font:11px ui-monospace,monospace; }
  :focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
  main { width:min(100%,1200px); margin:0 auto; padding:20px 16px; flex:1; }
  h1 { font-size:20px; line-height:1.25; margin:0 0 16px; font-weight:600; }
  h2 { font-size:14px; margin:0; font-weight:600; }
  .hint,.meta { color:var(--muted); }
  .mono { font:12px/1.4 ui-monospace,SFMono-Regular,monospace; }
  .rows { border-top:1px solid var(--line); }
  .row { min-height:36px; display:grid; grid-template-columns:minmax(130px,1.1fr) minmax(90px,.8fr) minmax(120px,1fr); gap:12px; align-items:center; padding:6px 8px; border-bottom:1px solid var(--line); }
  .row:nth-child(even) { background:var(--soft); }
  .tree-row { grid-template-columns:minmax(180px,1fr) minmax(100px,.6fr) minmax(130px,.8fr); }
  .indent { display:flex; align-items:center; min-width:0; }
  .branch { display:inline-block; width:18px; flex:none; color:var(--muted); }
  .state { font-weight:500; }
  .state[data-state*=pending],.state[data-state*=waiting] { color:var(--pending); }
  .state[data-state*=fail],.state[data-state*=error],.state[data-state*=refus] { color:var(--failed); }
  .empty { padding:22px 8px; border-block:1px solid var(--line); }
  .empty strong { display:block; margin-bottom:4px; }
  .toolbar { display:flex; justify-content:space-between; gap:12px; align-items:center; margin-bottom:12px; }
  .command-form { display:flex; gap:8px; max-width:760px; }
  .command-form input { flex:1; min-width:0; padding:10px; color:var(--fg); background:var(--bg); border:1px solid var(--line); }
  .command-form button { padding:8px 14px; background:var(--fg); color:var(--bg); border:1px solid var(--fg); cursor:pointer; }
  .host-controls { max-width:760px; }
  .host-controls label { display:block; margin:12px 0 4px; font-weight:600; }
  .host-controls select,.host-controls textarea { width:100%; padding:9px; color:var(--fg); background:var(--bg); border:1px solid var(--line); }
  .host-actions { display:flex; gap:8px; margin-top:10px; }
  .host-actions button { padding:8px 12px; }
  .message { white-space:pre-wrap; overflow-wrap:anywhere; }
  .message pre { white-space:pre-wrap; overflow-wrap:anywhere; margin:4px 0 12px; }
  .sr-only { position:absolute; width:1px; height:1px; padding:0; margin:-1px; overflow:hidden; clip:rect(0,0,0,0); white-space:nowrap; border:0; }
  @media(max-width:600px) {
    .top { align-items:flex-start; flex-wrap:wrap; gap:4px; padding:8px; }
    .brand { width:100%; }
    .tabs { width:100%; overflow-x:auto; }
    .tab { padding:7px 8px; white-space:nowrap; }
    .tab kbd { display:none; }
    main { padding:16px 8px; }
    .row { grid-template-columns:minmax(0,1fr) minmax(80px,.7fr); gap:6px 10px; }
    .row > :last-child { grid-column:1 / -1; }
    .tree-row { grid-template-columns:minmax(0,1fr) minmax(85px,.7fr); }
    .command-form { flex-wrap:wrap; }
    .command-form input { flex-basis:100%; }
  }
  @media(prefers-reduced-motion:reduce) { *,*::before,*::after { scroll-behavior:auto !important; transition:none !important; animation:none !important; } }
  @media(forced-colors:active) { .tab[aria-current=page] { border-bottom-color:Highlight; } :focus-visible { outline-color:Highlight; } }
`;

function Empty({ title, help }: { title: string; help: string }) {
  return <div className="empty"><strong>{title}</strong><span className="hint">{help}</span></div>;
}

function Tree({ data }: { data: HarnessViewModel }) {
  const children = useMemo(() => {
    const byParent = new Map<string | undefined, HarnessViewModel["nodes"]>();
    data.nodes.forEach((node) => {
      const parentId = node.parentId ?? undefined;
      byParent.set(parentId, [...(byParent.get(parentId) ?? []), node]);
    });
    return byParent;
  }, [data.nodes]);
  const rows: Array<{ node: HarnessViewModel["nodes"][number]; depth: number }> = [];
  const visit = (parent: string | undefined, depth: number, seen: Set<string>) => {
    (children.get(parent) ?? []).forEach((node) => {
      if (seen.has(node.id)) return;
      rows.push({ node, depth });
      const next = new Set(seen); next.add(node.id); visit(node.id, depth + 1, next);
    });
  };
  visit(undefined, 0, new Set());
  return data.nodes.length === 0 ? <Empty title="No nodes yet" help="Active conversations and their children appear here." /> : (
    <div role="table" aria-label="Conversation tree" className="rows">
      <div className="row tree-row" role="row"><strong role="columnheader">Conversation</strong><strong role="columnheader">State</strong><strong role="columnheader">Model · effort · activity</strong></div>
      {rows.map(({ node, depth }) => <div className="row tree-row" role="row" key={node.id}>
        <div className="indent" role="cell" style={{ paddingInlineStart: Math.min(depth, 8) * 16 }}>
          {depth > 0 && <span className="branch" aria-hidden="true">└─</span>}
          <span title={node.id}>{node.name}</span>
        </div>
        <span role="cell" className="state" data-state={node.state}>{node.state}</span>
        <span role="cell" className="meta">{[node.model, node.effort, node.detail ?? node.updatedAt].filter(Boolean).join(" · ") || "—"}</span>
      </div>)}
    </div>
  );
}

function HostActors({ data }: { data: HarnessViewModel }) {
  const actors = data.actors ?? [];
  if (actors.length === 0) return null;
  return <section aria-label="Host actors">
    <h2>Host actors</h2>
    <div role="table" aria-label="Host actor lifecycles" className="rows">
      <div className="row" role="row"><strong role="columnheader">Actor</strong><strong role="columnheader">Lifecycle</strong><strong role="columnheader">Incarnation · relation</strong></div>
      {actors.map((actor) => <div className="row" role="row" key={actor.id}>
        <strong role="cell">{actor.name} <span className="hint">({actor.kind})</span></strong>
        <span role="cell" className="state" data-state={actor.lifecycle}>{actor.lifecycle}</span>
        <span role="cell" className="meta">{[
          `run ${actor.run}`, `incarnation ${actor.incarnation}`,
          actor.parent ? `parent ${actor.parent}` : undefined,
          actor.modelConversation ? `conversation ${actor.modelConversation}` : undefined,
        ].filter(Boolean).join(" · ")}</span>
      </div>)}
    </div>
  </section>;
}

function HostControls({ data, onCommand, pendingCommands, onRetry }: {
  data: HarnessViewModel;
  onCommand?: AppProps["onCommand"];
  pendingCommands: NonNullable<AppProps["pendingCommands"]>;
  onRetry?: AppProps["onRetry"];
}) {
  const actors = (data.actors ?? []).filter((actor) => actor.run === data.hostRun);
  const [selectedKey, setSelectedKey] = useState("");
  const [text, setText] = useState("");
  const selected = actors.find((actor) => actor.id === selectedKey);
  const target = selected && {
    run: selected.run,
    actor: selected.name,
    incarnation: selected.incarnation,
  };
  const canControl = Boolean(target && (selected?.lifecycle === "running" || selected?.lifecycle === "waiting") && onCommand);
  const submitInput = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!target || !canControl || !text.trim()) return;
    onCommand?.({ action: "input", target, text });
    setText("");
  };
  const sendAction = (action: "interrupt" | "retire") => {
    if (!target || !canControl) return;
    if (action === "interrupt") {
      if (!selected?.activeRound) return;
      onCommand?.({ action, target, expected_round: selected.activeRound });
    } else onCommand?.({ action, target });
  };
  const selectedWasRemoved = selectedKey.length > 0 && !selected;
  return <section className="host-controls" aria-labelledby="host-controls-heading">
    <h2 id="host-controls-heading">Embedded host controls</h2>
    <p className="hint">Run <span className="mono">{data.hostRun}</span>. Select an actor explicitly. Controls address its exact run, actor, and incarnation.</p>
    {actors.length === 0 && <p role="status" className="hint">No actors are currently projected for this host run.</p>}
    <label htmlFor="host-actor-target">Target actor</label>
    <select id="host-actor-target" value={selectedKey} onChange={(event) => setSelectedKey(event.target.value)}>
      <option value="">Choose an actor…</option>
      {selectedWasRemoved && <option value={selectedKey}>Previously selected actor is no longer present</option>}
      {actors.map((actor) => <option key={actor.id} value={actor.id}>
        {actor.name} · {actor.kind} · {actor.lifecycle} · incarnation {actor.incarnation}
      </option>)}
    </select>
    {selectedWasRemoved && <p role="status" className="hint">The selected actor disappeared or was replaced. Choose an actor again before sending a command.</p>}
    {selected && selected.lifecycle !== "running" && selected.lifecycle !== "waiting" &&
      <p role="status" className="hint">This actor is {selected.lifecycle}; host controls are unavailable.</p>}
    {!onCommand && <p role="status" className="hint">Host command channel is not connected.</p>}
    <form onSubmit={submitInput}>
      <label htmlFor="host-actor-input">Message to selected actor</label>
      <textarea id="host-actor-input" rows={4} value={text} onChange={(event) => setText(event.target.value)} disabled={!canControl} />
      <div className="host-actions">
        <button type="submit" disabled={!canControl || !text.trim()}>Send input</button>
        <button type="button" disabled={!canControl || !selected?.activeRound} onClick={() => sendAction("interrupt")}>Interrupt</button>
        <button type="button" disabled={!canControl} onClick={() => sendAction("retire")}>Retire</button>
      </div>
    </form>
    <CommandReceipts receipts={data.commandReceipts ?? []} />
    <PendingCommands commands={pendingCommands ?? []} onRetry={onRetry} />
    <HostActors data={data} />
  </section>;
}

function PendingCommands({ commands, onRetry }: { commands: NonNullable<AppProps["pendingCommands"]>; onRetry?: AppProps["onRetry"] }) {
  if (commands.length === 0) return null;
  return <section aria-labelledby="pending-commands-heading">
    <h2 id="pending-commands-heading">Retained browser operations</h2>
    {commands.map(({ submission, state }) => <article className="row" key={submission.operation_id} data-operation-id={submission.operation_id}>
      <span>{submission.command.action}</span>
      <span className="state" data-state={state}>{state}</span>
      <span className="meta mono">{submission.operation_id}</span>
      <button type="button" onClick={() => onRetry?.(submission)} disabled={!onRetry}>Retry same operation</button>
    </article>)}
    <p className="hint">Unconfirmed operations are retained across reloads and are never replayed automatically. Retry reuses the same operation ID.</p>
  </section>;
}

function CommandReceipts({ receipts }: { receipts: readonly CommandReceipt[] }) {
  if (receipts.length === 0) return null;
  return <section aria-labelledby="command-receipts-heading">
    <h2 id="command-receipts-heading">Recent command handoffs</h2>
    <div role="list" className="rows" aria-label="Command handoff receipts">
      {receipts.map((receipt) => {
        const outcome = receipt.outcome === "admitted"
          ? "Admitted for processing"
          : receipt.outcome === "control_requested"
            ? `${receipt.control === "interrupt" ? "Interrupt" : "Retire"} requested`
            : "Refused";
        const detail = receipt.outcome === "admitted"
          ? [`envelope ${receipt.envelopeId}`, receipt.wakeError ? `wake issue: ${receipt.wakeError}` : undefined].filter(Boolean).join(" · ")
          : receipt.outcome === "control_requested"
            ? "Request sent to the host; this does not report actor completion."
            : receipt.reason;
        return <article role="listitem" className="row" key={receipt.commandId}>
          <strong>{outcome}</strong>
          <span className="mono">{receipt.commandId}</span>
          <span className="meta">{[
            receipt.target
              ? `${receipt.target.actor} · run ${receipt.target.run} · incarnation ${receipt.target.incarnation}`
              : undefined,
            detail,
          ].filter(Boolean).join(" · ")}</span>
        </article>;
      })}
    </div>
  </section>;
}

function Timeline({ data, onInspectRequest }: { data: HarnessViewModel; onInspectRequest?: (requestId: string) => void }) {
  return data.timeline.length === 0 ? <Empty title="No activity yet" help="Requests, jobs and waits will be listed as they happen." /> : (
    <div role="table" aria-label="Conversation activity timeline" className="rows">
      <div className="row" role="row"><strong role="columnheader">Activity</strong><strong role="columnheader">State</strong><strong role="columnheader">Node · start · duration</strong></div>
      {data.timeline.map((item) => <div className="row" role="row" key={item.id}>
        <span role="cell"><span className="mono">{item.kind}</span> · {item.label}
          {item.kind === "request" && onInspectRequest && <button type="button" onClick={() => onInspectRequest(item.id)}>Inspect history</button>}
        </span>
        <span role="cell" className="state" data-state={item.state}>{item.state}</span>
        <span role="cell" className="meta">{[
          `id ${item.id}`,
          item.nodeId,
          item.commandId ? `command ${item.commandId}` : undefined,
          item.outcome ? `outcome ${item.outcome}` : undefined,
          item.requestId ? `request ${item.requestId}` : undefined,
          item.callId ? `call ${item.callId}` : undefined,
          item.toolKind ? `tool kind ${item.toolKind}` : undefined,
          item.toolName,
          item.delivered === undefined ? undefined : `delivered ${item.delivered}`,
          item.startedAt,
          item.duration,
          item.detail,
          item.output === undefined ? undefined : `output ${formatOutput(item.output)}`,
        ].filter(Boolean).join(" · ") || "—"}</span>
      </div>)}
    </div>
  );
}

function formatOutput(output: unknown): string {
  if (typeof output === "string") return output;
  try { return JSON.stringify(output) ?? String(output); }
  catch { return String(output); }
}

function Inbox({ data }: { data: HarnessViewModel }) {
  return data.inbox.length === 0 ? <Empty title="Inbox is clear" help="Operator messages and pending questions appear here. Use “g t” to return to the tree." /> : (
    <div role="list" className="rows" aria-label="Inbox messages">
      {data.inbox.map((item) => <article className="row" role="listitem" key={item.id}>
        <strong>{item.sender}{item.recipient ? ` → ${item.recipient}` : ""}</strong><span className="state" data-state={item.state}>{item.state}{item.ordinal !== undefined ? ` · #${item.ordinal}` : ""}</span>
        <span className="message">{item.message}</span><span className="meta">{item.receivedAt ?? ""}</span>
      </article>)}
    </div>
  );
}

export default function App({ data, onCommand, onRetry, pendingCommands = [], acceptedCommandIds = [] }: AppProps) {
  const [screen, setScreen] = useState<Screen>("tree");
  const [command, setCommand] = useState("");
  const [inspectedRequest, setInspectedRequest] = useState<string>();
  const embeddedMode = data.hostRun !== undefined;
  const screens = embeddedMode ? embeddedScreens : standaloneScreens;
  const activeScreen = screens.some((item) => item.id === screen) ? screen : "tree";
  useEffect(() => {
    let prefix = "";
    let timer = 0;
    const handle = (event: KeyboardEvent) => {
      if (event.altKey || event.ctrlKey || event.metaKey || event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLSelectElement || (event.target as HTMLElement).isContentEditable) return;
      if (event.key === "g") { prefix = "g"; window.clearTimeout(timer); timer = window.setTimeout(() => { prefix = ""; }, 900); return; }
      if (prefix === "g") {
        const target: Record<string, Screen> = embeddedMode
          ? { t: "tree", l: "timeline", i: "inbox", h: "host" }
          : { t: "tree", l: "timeline", i: "inbox", c: "command" };
        const nextScreen = target[event.key]
        if (nextScreen !== undefined) { event.preventDefault(); setScreen(nextScreen); }
        prefix = "";
      }
    };
    window.addEventListener("keydown", handle);
    return () => { window.removeEventListener("keydown", handle); window.clearTimeout(timer); };
  }, [embeddedMode]);
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    // Preserve the payload exactly: whitespace after the verb is data in the
    // deterministic command grammar.
    if (command.trim()) { onCommand?.(command); setCommand(""); }
  };
  return <>
    <style>{styles}</style>
    <div className="harness">
      <header className="top">
        <div className="brand">Harness <span className="hint">/ operator</span></div>
        <nav className="tabs" aria-label="Views">
          {screens.map(({ id, title, shortcut }) => <button className="tab" key={id} type="button" aria-current={activeScreen === id ? "page" : undefined} onClick={() => setScreen(id)}>
            {title}<kbd aria-hidden="true">{shortcut}</kbd>
          </button>)}
        </nav>
      </header>
      <main>
        <div className="toolbar"><h1>{screens.find((item) => item.id === activeScreen)?.title}</h1><span className="hint">Keyboard: g then t / l / i / {embeddedMode ? "h" : "c"}</span></div>
        {activeScreen === "tree" && <><HostActors data={data} /><Tree data={data} /></>}
        {activeScreen === "host" && <HostControls data={data} onCommand={onCommand} pendingCommands={pendingCommands} onRetry={onRetry} />}
        {activeScreen === "timeline" && <><Timeline data={data} onInspectRequest={setInspectedRequest} />
          {inspectedRequest && <NodeWindow key={inspectedRequest} requestId={inspectedRequest} onClose={() => setInspectedRequest(undefined)} />}</>}
        {activeScreen === "inbox" && <Inbox data={data} />}
        {activeScreen === "command" && <section aria-labelledby="command-heading">
          <h2 id="command-heading">Send a command</h2>
          <p className="hint">Deterministic mode · commands are sent to the authenticated host. Sending is not acceptance or completion; this client displays only state present in the server snapshot and events.</p>
          <div className="rows" aria-label="Command grammar">
            <p><code>echo TEXT</code> · <code>test</code> · <code>wait</code> · <code>message TEXT</code></p>
            <p><code>cancel</code> · <code>child TEXT</code> · <code>fail</code></p>
            <p className="hint">Use the command form for the documented operations. Outcomes, progress, and messages appear only to the extent represented by received server records.</p>
          </div>
          <div className="toolbar" aria-label="Common deterministic actions">
            {([
              ["Run test", "test"],
              ["Start wait", "wait"],
              ["Cancel wait", "cancel"],
              ["Create example child", "child browser example"],
              ["Fail request", "fail"],
              ["Recover with echo", "echo recovered"],
            ] as const).map(([label, value]) => <button key={value} type="button" disabled={!onCommand} onClick={() => onCommand?.(value)}>{label}</button>)}
          </div>
          <form className="command-form" onSubmit={submit}>
            <label className="sr-only" htmlFor="command-input">Command</label>
            <input id="command-input" value={command} onChange={(event) => setCommand(event.target.value)} autoComplete="off" />
            <button type="submit" disabled={!command.trim() || !onCommand}>Send</button>
          </form>
          {!onCommand && <p className="hint" role="status">Command channel is not connected.</p>}
          <h2>Authoritative activity</h2>
          <p className="hint">Stored records received from the host; refresh reconnects to the server snapshot and does not replay submitted commands.</p>
          {acceptedCommandIds.filter((id) => !data.timeline.some((item) => item.commandId === id)).map((id) => (
            <p className="hint" role="status" key={id}>Server accepted command {id}; this acknowledgment is not a terminal outcome.</p>
          ))}
          <Timeline data={data} onInspectRequest={setInspectedRequest} />
          {inspectedRequest && <NodeWindow key={inspectedRequest} requestId={inspectedRequest} onClose={() => setInspectedRequest(undefined)} />}
          <CommandReceipts receipts={data.commandReceipts ?? []} />
          <HostActors data={data} /><h2>Conversation and child identities</h2><Tree data={data} />
          <h2>Messages and replies</h2><Inbox data={data} />
        </section>}
      </main>
    </div>
  </>;
}
