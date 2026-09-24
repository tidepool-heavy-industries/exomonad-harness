import { useEffect, useMemo, useState, type FormEvent } from "react";

/**
 * Presentation-only contract for the web views. The server/protocol adapter
 * owns conversion from protocol events into this view model; this module does
 * not import or re-declare wire types.
 */
export type HarnessViewModel = {
  nodes: Array<{
    id: string;
    parentId?: string;
    name: string;
    model?: string;
    effort?: string;
    state: string;
    detail?: string;
    updatedAt?: string;
  }>;
  timeline: Array<{
    id: string;
    nodeId: string;
    label: string;
    kind: "request" | "job" | "wait";
    state: string;
    startedAt?: string;
    duration?: string;
    detail?: string;
  }>;
  inbox: Array<{
    id: string;
    sender: string;
    message: string;
    state: string;
    receivedAt?: string;
  }>;
};

export type AppProps = {
  data?: HarnessViewModel;
  onCommand?: (command: string) => void;
};

type Screen = "tree" | "timeline" | "inbox" | "command";
const emptyData: HarnessViewModel = { nodes: [], timeline: [], inbox: [] };
const screens: Array<{ id: Screen; title: string; shortcut: string }> = [
  { id: "tree", title: "Tree", shortcut: "g t" },
  { id: "timeline", title: "Timeline", shortcut: "g l" },
  { id: "inbox", title: "Inbox", shortcut: "g i" },
  { id: "command", title: "Command", shortcut: "g c" },
];

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
  .message { white-space:pre-wrap; overflow-wrap:anywhere; }
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
    data.nodes.forEach((node) => byParent.set(node.parentId, [...(byParent.get(node.parentId) ?? []), node]));
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

function Timeline({ data }: { data: HarnessViewModel }) {
  return data.timeline.length === 0 ? <Empty title="No activity yet" help="Requests, jobs and waits will be listed as they happen." /> : (
    <div role="table" aria-label="Conversation activity timeline" className="rows">
      <div className="row" role="row"><strong role="columnheader">Activity</strong><strong role="columnheader">State</strong><strong role="columnheader">Node · start · duration</strong></div>
      {data.timeline.map((item) => <div className="row" role="row" key={item.id}>
        <span role="cell"><span className="mono">{item.kind}</span> · {item.label}</span>
        <span role="cell" className="state" data-state={item.state}>{item.state}</span>
        <span role="cell" className="meta">{[item.nodeId, item.startedAt, item.duration, item.detail].filter(Boolean).join(" · ") || "—"}</span>
      </div>)}
    </div>
  );
}

function Inbox({ data }: { data: HarnessViewModel }) {
  return data.inbox.length === 0 ? <Empty title="Inbox is clear" help="Operator messages and pending questions appear here. Use “g t” to return to the tree." /> : (
    <div role="list" className="rows" aria-label="Inbox messages">
      {data.inbox.map((item) => <article className="row" role="listitem" key={item.id}>
        <strong>{item.sender}</strong><span className="state" data-state={item.state}>{item.state}</span>
        <span className="message">{item.message}</span><span className="meta">{item.receivedAt ?? ""}</span>
      </article>)}
    </div>
  );
}

export default function App({ data = emptyData, onCommand }: AppProps) {
  const [screen, setScreen] = useState<Screen>("tree");
  const [command, setCommand] = useState("");
  useEffect(() => {
    let prefix = "";
    let timer = 0;
    const handle = (event: KeyboardEvent) => {
      if (event.altKey || event.ctrlKey || event.metaKey || event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLSelectElement || (event.target as HTMLElement).isContentEditable) return;
      if (event.key === "g") { prefix = "g"; window.clearTimeout(timer); timer = window.setTimeout(() => { prefix = ""; }, 900); return; }
      if (prefix === "g") {
        const target: Record<string, Screen> = { t: "tree", l: "timeline", i: "inbox", c: "command" };
        if (target[event.key]) { event.preventDefault(); setScreen(target[event.key]); }
        prefix = "";
      }
    };
    window.addEventListener("keydown", handle);
    return () => { window.removeEventListener("keydown", handle); window.clearTimeout(timer); };
  }, []);
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const value = command.trim();
    if (value) { onCommand?.(value); setCommand(""); }
  };
  return <>
    <style>{styles}</style>
    <div className="harness">
      <header className="top">
        <div className="brand">Harness <span className="hint">/ operator</span></div>
        <nav className="tabs" aria-label="Views">
          {screens.map(({ id, title, shortcut }) => <button className="tab" key={id} type="button" aria-current={screen === id ? "page" : undefined} onClick={() => setScreen(id)}>
            {title}<kbd aria-hidden="true">{shortcut}</kbd>
          </button>)}
        </nav>
      </header>
      <main>
        <div className="toolbar"><h1>{screens.find((item) => item.id === screen)?.title}</h1><span className="hint">Keyboard: g then t / l / i / c</span></div>
        {screen === "tree" && <Tree data={data} />}
        {screen === "timeline" && <Timeline data={data} />}
        {screen === "inbox" && <Inbox data={data} />}
        {screen === "command" && <section aria-labelledby="command-heading">
          <h2 id="command-heading">Send a command</h2>
          <p className="hint">Commands are passed to the host; this view does not assume or fabricate a result.</p>
          <form className="command-form" onSubmit={submit}>
            <label className="sr-only" htmlFor="command-input">Command</label>
            <input id="command-input" value={command} onChange={(event) => setCommand(event.target.value)} autoComplete="off" />
            <button type="submit" disabled={!command.trim() || !onCommand}>Send</button>
          </form>
          {!onCommand && <p className="hint" role="status">Command channel is not connected.</p>}
        </section>}
      </main>
    </div>
  </>;
}
