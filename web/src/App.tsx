import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent, type MouseEvent } from 'react';
import NodeWindow from './NodeWindow';
import WorkerChat from './WorkerChat';
import ActiveWorkerTree from './ActiveWorkerTree';
import type { CommandReceipt, HostActorIdentity, HostCommand } from './protocol';
import { actorIdentityKey, canonicalOperationId, isOperationId } from './protocol';
import { isSettledCommand, operationKey, type BrowserCommandRecord } from './pending-commands';
import type { HarnessViewModel } from './view-model';
import type { RouteState, Screen, Selection, SubmitHostCommand, RetryHostCommand, SubmitDemoCommand, TransportPhase } from './client-contract';
import { createViewSelector, formatActivityTime, orderConversationTree, pageRows, resolveSelection, needsActivityClock } from './selectors';
import { actorDraftKey, demoDraftKey, useDraft } from './drafts';
import { routeUrl, useRoute } from './navigation';
export type AppProps = {
  data: HarnessViewModel;
  onHostCommand?: SubmitHostCommand;
  onDemoCommand?: SubmitDemoCommand;
  onRetry?: RetryHostCommand;
  transportPhase?: TransportPhase;
  pendingCommands?: readonly BrowserCommandRecord[];
  acceptedCommandIds?: readonly string[];
  demoFeedback?: string;
  onAuthExpired?: () => void;
};
const screenNames: Record<Screen, string> = { tree: 'Tree', timeline: 'Timeline', inbox: 'Inbox', command: 'Command', chat: 'Chat' };
const identityOf = (actor: NonNullable<HarnessViewModel['actors']>[number]): HostActorIdentity => ({ run: actor.run, actor: actor.name, incarnation: actor.incarnation });
const sameSelection = (left: Selection, right: Selection) => JSON.stringify(left) === JSON.stringify(right);
function Empty({ children }: {
  children: React.ReactNode;
}) {
  return <p className="empty" role="status">
    {children}
  </p>;
}
function Pager({ page, count, total, onPage }: {
  page: number;
  count: number;
  total: number;
  onPage: (page: number) => void;
}) {
  return <div className="pager">
    <span>
      {total} rows · Page {page + 1} of {Math.max(1, count)}
    </span>
    <button disabled={page === 0} onClick={() => onPage(page - 1)}>Previous page</button>
    <button disabled={page + 1 >= count} onClick={() => onPage(page + 1)}>Next page</button>
  </div>;
}
function receiptText(receipt: CommandReceipt): {
  title: string;
  detail: string;
} {
  switch (receipt.outcome) {
    case 'admitted': return { title: 'Admitted for processing', detail: `envelope ${receipt.envelopeId}${receipt.wakeError ? ` · wake issue: ${receipt.wakeError}` : ''}` };
    case 'control_requested': return { title: receipt.control === 'interrupt' ? 'Interrupt requested' : 'Retire requested', detail: 'Host control request; actor completion is not established.' };
    case 'refused': return { title: 'Refused', detail: receipt.reason };
    case 'unconfirmed': return { title: 'Unconfirmed', detail: receipt.reason };
  }
}
function CommandReceipts({ receipts }: {
  receipts: readonly CommandReceipt[];
}) {
  return receipts.length ? <section aria-label="Command handoff receipts">
    <h2>Recent command handoffs</h2>
    {receipts.map((receipt, index) => {
      const text = receiptText(receipt);
      return <article className="row" key={`${receipt.commandId}:${index}`}>
        <strong>
          {text.title}
        </strong>
        <span className="mono">
          {receipt.commandId}
        </span>
        <span>
          {receipt.target && `${receipt.target.actor} · run ${receipt.target.run} · incarnation ${receipt.target.incarnation} · `}{text.detail}
        </span>
      </article>;
    })}
    <p className="hint">Admission, request inclusion, execution, delivery and terminal outcome are separate observations.</p>
  </section> : null;
}
function RetainedCommands({ commands, run, ready, activeTargets, onRetry }: {
  commands: readonly BrowserCommandRecord[];
  run?: string;
  ready: boolean;
  activeTargets: ReadonlySet<string>;
  onRetry?: RetryHostCommand;
}) {
  const [feedback, setFeedback] = useState('');
  if (!commands.length)
    return null;
  return <section aria-label="Retained browser operations">
    <h2>Retained browser operations</h2>
    {commands.map(record => <article className="operation" data-operation-id={record.submission.operation_id} key={operationKey(record)}>
      <strong>
        {record.submission.command.action} · {record.authority === 'legacy' ? `Previously observed ${record.state}; fresh lookup required` : record.authority === 'local' ? 'Locally retained; host admission unknown' : record.state}
      </strong>
      <div className="mono">
        {record.submission.operation_id} · run {record.hostRun}
      </div>
      <pre>
        {JSON.stringify(record.submission.command, null, 2)}
      </pre>
      <p>Socket send: {record.send ?? 'unknown'}{record.accepted ? ' · Server acknowledged handoff; terminal outcome unknown.' : ''}
      </p>
      {record.lookup && <p className="hint">Status lookup {record.lookup.kind}: {record.lookup.reason}
      </p>}
      {record.localRefusal && <p className="error">Local channel refusal: {record.localRefusal.reason}
      </p>}
      {record.issues?.map((issue, i) => <p className="error" key={i}>
        {issue}
      </p>)}
      {record.receipt && <p>
        {receiptText(record.receipt).title}: {receiptText(record.receipt).detail}
      </p>}
      <button disabled={!onRetry || !ready || record.hostRun !== run || !activeTargets.has(actorIdentityKey(record.submission.command.target)) || isSettledCommand(record)} onClick={() => {
        const result = onRetry?.(record.submission); if (result)
          setFeedback(result.kind === 'blocked' ? result.reason : `Same operation retained; socket send ${result.send}.`);
      }}>Retry same operation</button>
    </article>)}{feedback && <p role="status">
      {feedback}
    </p>}
    <p className="hint">Original payloads are retained. Reconnect never automatically resubmits an operation.</p>
  </section>;
}
function ActorComposer({ data, route, unavailable, ready, onHostCommand }: {
  data: HarnessViewModel;
  route: RouteState;
  unavailable: boolean;
  ready: boolean;
  onHostCommand?: SubmitHostCommand;
}) {
  const resolved = resolveSelection(data, route.selection);
  const actor = resolved.actor;
  const target = route.selection.kind === 'actor' ? route.selection.identity : undefined;
  const draft = useDraft(target ? actorDraftKey(target) : undefined);
  const [feedback, setFeedback] = useState('');
  const canControl = Boolean(target && actor && !unavailable && actor.run === data.hostRun && ready && onHostCommand && (actor.lifecycle === 'running' || actor.lifecycle === 'waiting'));
  const send = (command: HostCommand) => {
    const result = onHostCommand?.(command);
    if (!result)
      return;
    setFeedback(result.kind === 'blocked' ? result.reason : `Operation ${result.operationId} retained locally; socket send ${result.send}. Host admission is not established.`);
    if (result.kind === 'retained' && command.action === 'input')
      draft.submitted();
  };
  return <section className="host-controls">
    <h2>Message and controls</h2>
    {!ready && <p role="status">Host controls await a fresh connected snapshot.</p>}{!onHostCommand && <p role="status">Host command channel is unavailable; this view is read-only.</p>}
    <form onSubmit={(event: FormEvent) => {
      event.preventDefault(); if (target && canControl && draft.text.trim())
        send({ action: 'input', target, text: draft.text });
    }}>
      <label htmlFor="actor-message">Message to selected actor</label>
      <textarea id="actor-message" rows={4} value={draft.text} onChange={event => draft.setText(event.target.value)} disabled={!target || !actor || unavailable} />
      <div className="host-actions">
        <button disabled={!canControl || !draft.text.trim()}>Send input</button>
        <button type="button" disabled={!canControl || !actor?.activeRound} onClick={() => {
          if (target && actor?.activeRound)
            send({ action: 'interrupt', target, expected_round: actor.activeRound });
        }}>Interrupt</button>
        <button type="button" disabled={!canControl} onClick={() => {
          if (target)
            send({ action: 'retire', target });
        }}>Retire</button>
      </div>
    </form>
    {draft.error && <p className="error" role="status">
      {draft.error}
    </p>}{feedback && <p role="status">
      {feedback}
    </p>}
  </section>;
}
export default function App({ data, onHostCommand, onDemoCommand, onRetry, transportPhase = 'ready', pendingCommands = [], acceptedCommandIds = [], demoFeedback, onAuthExpired }: AppProps) {
  const { route, issue, navigate } = useRoute();
  const resolved = useMemo(() => resolveSelection(data, route.selection), [data.actors, data.nodes, route.selection]);
  const selector = useMemo(() => createViewSelector(), []);
  const view = useMemo(() => selector(data, route.selection, route.global, route.messageFilters), [selector, data, route.selection, route.global, route.messageFilters]);
  const embedded = data.hostRun !== undefined;
  const screens = useMemo<Screen[]>(() => embedded ? ['tree', 'chat', 'timeline', 'inbox'] : ['tree', 'timeline', 'inbox', 'command'], [embedded]);
  const heading = useRef<HTMLHeadingElement>(null);
  const previousScreen = useRef(route.screen);
  const inspectionTrigger = useRef<HTMLElement | null>(null);
  const previousInspection = useRef(route.requestId);
  const [search, setSearch] = useState('');
  const [page, setPage] = useState(0);
  const [actorPage, setActorPage] = useState(0);
  const [allOperations, setAllOperations] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const demo = useDraft(demoDraftKey);
  const [demoLocalFeedback, setDemoLocalFeedback] = useState('');
  useEffect(() => {
    if (previousScreen.current !== route.screen) {
      heading.current?.focus();
      previousScreen.current = route.screen;
    } setSearch(''); setPage(0); setActorPage(0);
  }, [route.screen]);
  useEffect(() => {
    if (previousInspection.current !== undefined && route.requestId === undefined) {
      if (inspectionTrigger.current?.isConnected)
        inspectionTrigger.current.focus();
      else
        heading.current?.focus();
      inspectionTrigger.current = null;
    }
    previousInspection.current = route.requestId;
  }, [route.requestId]);
  useEffect(() => { setPage(0); setActorPage(0); }, [route.selection, route.global, route.messageFilters]);
  const searchTerm = search.toLowerCase();
  const filteredTimeline = useMemo(() => searchTerm ? view.timeline.filter(item => `${item.label} ${item.id} ${item.detail ?? ''}`.toLowerCase().includes(searchTerm)) : view.timeline, [view.timeline, searchTerm]);
  const activity = useMemo(() => pageRows(filteredTimeline, page, 50), [filteredTimeline, page]);
  const activeVisible = (route.screen === 'timeline' || route.screen === 'command') && activity.rows.some(needsActivityClock);
  useEffect(() => {
    if (!activeVisible)
      return;
    let timer: ReturnType<typeof setInterval> | undefined;
    const visibility = () => {
      if (timer)
        clearInterval(timer); if (!document.hidden) {
          setNow(Date.now());
          timer = setInterval(() => setNow(Date.now()), 1000);
        }
    };
    visibility();
    document.addEventListener('visibilitychange', visibility);
    return () => {
      if (timer)
        clearInterval(timer); document.removeEventListener('visibilitychange', visibility);
    };
  }, [activeVisible]);
  useEffect(() => {
    let prefix = false;
    let timer: ReturnType<typeof setTimeout>;
    const handle = (event: KeyboardEvent) => {
      const element = event.target instanceof Element ? event.target : null;
      if (event.altKey || event.ctrlKey || event.metaKey || element?.closest('input,textarea,select,[contenteditable]:not([contenteditable="false"])')) {
        prefix = false;
        return;
      }
      if (event.key === 'g') {
        prefix = true;
        clearTimeout(timer);
        timer = setTimeout(() => { prefix = false; }, 900);
        return;
      }
      if (!prefix)
        return;
      prefix = false;
      const target = ({ t: 'tree', l: 'timeline', i: 'inbox', h: 'chat', c: embedded ? 'chat' : 'command' } as Record<string, Screen>)[event.key];
      if (target && screens.includes(target)) {
        event.preventDefault();
        navigate({ ...route, screen: target });
      }
    };
    window.addEventListener('keydown', handle);
    return () => { window.removeEventListener('keydown', handle); clearTimeout(timer); };
  }, [route, embedded]);
  const link = useCallback((selection: Selection, label: string, screen = selection.kind === 'actor' && embedded ? 'chat' : route.screen) => {
    const next = { ...route, screen, selection, requestId: selection.kind === 'actor' ? undefined : route.requestId };
    return <a href={routeUrl(next, new URL(window.location.href)).toString()} onClick={(event: MouseEvent<HTMLAnchorElement>) => {
      if (event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey) {
        event.preventDefault();
        navigate(next);
      }
    }}>
      {label}
    </a>;
  }, [route, navigate, embedded]);
  const inspect = useCallback((requestId: string, element?: HTMLElement) => { inspectionTrigger.current = element ?? null; navigate({ ...route, requestId }); }, [route, navigate]);
  const closeInspector = useCallback(() => {
    navigate({ ...route, requestId: undefined });
  }, [route, navigate]);
  const selectedDescription = route.selection.kind === 'actor' ? `${route.selection.identity.actor} · run ${route.selection.identity.run} · incarnation ${route.selection.identity.incarnation}` : route.selection.kind === 'conversation' ? `Conversation ${route.selection.conversationId}` : 'All contexts';
  const orderedNodes = useMemo(() => orderConversationTree(data.nodes), [data.nodes]);
  const nodes = useMemo(() => searchTerm ? orderedNodes.filter(({ node }) => `${node.name} ${node.id}`.toLowerCase().includes(searchTerm)) : orderedNodes, [orderedNodes, searchTerm]);
  const conversations = useMemo(() => pageRows(nodes, page, 100), [nodes, page]);
  const filteredActors = useMemo(() => (data.actors ?? []).filter(actor => !searchTerm || `${actor.name} ${actor.incarnation}`.toLowerCase().includes(searchTerm)), [data.actors, searchTerm]);
  const actors = useMemo(() => pageRows(filteredActors, actorPage, 100), [filteredActors, actorPage]);
  const filteredInbox = useMemo(() => searchTerm ? view.inbox.filter(item => `${item.message} ${item.sender} ${item.recipient ?? ''}`.toLowerCase().includes(searchTerm)) : view.inbox, [view.inbox, searchTerm]);
  const inbox = useMemo(() => pageRows(filteredInbox, page, 50), [filteredInbox, page]);
  const endpoints = useMemo(() => [...new Set(['/operator', ...data.inbox.flatMap(item => [item.sender, ...(item.recipient ? [item.recipient] : [])])])].sort(), [data.inbox]);
  const endpointNodes = useMemo(() => {
    const index = new Map<string, HarnessViewModel['nodes']>();
    for (const node of data.nodes) {
      const rows = index.get(node.name) ?? [];
      rows.push(node);
      index.set(node.name, rows);
    }
    return index;
  }, [data.nodes]);
  const endpointActors = useMemo(() => {
    const index = new Map<string, NonNullable<HarnessViewModel['actors']>>();
    for (const actor of data.actors ?? []) {
      const rows = index.get(actor.name) ?? [];
      rows.push(actor);
      index.set(actor.name, rows);
    }
    return index;
  }, [data.actors]);
  const requestIndex = useMemo(() => new Map(data.timeline.filter(item => item.kind === 'request').map(item => [item.id, item])), [data.timeline]);
  const commandIds = useMemo(() => new Set(data.timeline.flatMap(item => item.commandId ? [item.commandId] : [])), [data.timeline]);
  const sendDemo = (text: string) => {
    if (transportPhase !== 'ready' || !text.trim())
      return;
    const result = onDemoCommand?.(text);
    if (!result) return;
    setDemoLocalFeedback(result.kind === 'blocked' ? result.reason : 'Sent to the demo channel; server result remains separate.');
    if (result.kind === 'sent')
      demo.submitted(text);
  };
  const timeline = useMemo(() => <>
    <div role="table" aria-label="Conversation activity timeline" className="rows">
      <div className="row" role="row">
        <strong role="columnheader">Activity</strong>
        <strong role="columnheader">State</strong>
        <strong role="columnheader">Source and time</strong>
      </div>
      {activity.rows.map(item => {
        const time = formatActivityTime(item, now);
        return <div className="row" role="row" key={item.key ?? `${item.kind}:${item.id}`}>
          <span role="cell">
            {item.kind} · {item.label}{item.kind === 'request' && <button onClick={event => inspect(item.id, event.currentTarget)}>Inspect history</button>}{item.parentId && <button onClick={event => inspect(item.parentId!, event.currentTarget)}>Inspect parent request</button>}
          </span>
          <span role="cell">
            {item.state}{item.outcome && ` · outcome ${item.outcome}`}{item.delivered !== undefined && ` · delivered ${item.delivered}`}
          </span>
          <span role="cell" className="meta">
            {link({ kind: 'conversation', conversationId: item.nodeId }, item.nodeId)} · id {item.id} · {time.start} · {time.duration}{item.commandId && ` · command ${item.commandId}`}{item.requestId && ` · request ${item.requestId}`}{item.callId && ` · call ${item.callId}`}{item.toolKind && ` · tool kind ${item.toolKind}`}{item.toolName && ` · ${item.toolName}`}{item.detail && ` · ${item.detail}`}{item.output !== undefined && <pre>
              {typeof item.output === 'string' ? item.output : JSON.stringify(item.output, null, 2)}
            </pre>}
          </span>
        </div>;
      })}
    </div>
    {!activity.total && <Empty>No activity is available for this context.</Empty>}
    <Pager page={activity.page} count={activity.pageCount} total={activity.total} onPage={setPage} />
  </>, [activity, now, inspect, link]);
  const tree = useMemo(() => <>
          <section aria-label="Host actors">
            <h2>Host actors</h2>
            <div role="table" aria-label="Host actor lifecycles">
              {actors.rows.map(actor => <div className="row" role="row" key={actorIdentityKey(identityOf(actor))} aria-selected={sameSelection(route.selection, { kind: 'actor', identity: identityOf(actor) })}>
                <span role="cell">
                  {link({ kind: 'actor', identity: identityOf(actor) }, actor.name)} · {actor.kind}
                </span>
                <span role="cell">
                  {actor.lifecycle}
                </span>
                <span role="cell">run {actor.run} · incarnation {actor.incarnation}{actor.parentIdentity && <> · Parent {link({ kind: 'actor', identity: actor.parentIdentity }, actor.parentIdentity.actor)}
                </>}{actor.modelConversation && <> · {link({ kind: 'conversation', conversationId: actor.modelConversation }, actor.modelConversation)}
                </>}
                </span>
              </div>)}
            </div>
            <Pager page={actors.page} count={actors.pageCount} total={actors.total} onPage={setActorPage} />
          </section>
          <section aria-label="Conversations">
            <h2>Conversations</h2>
            <div role="table" aria-label="Conversation tree">
              {conversations.rows.map(({ node, depth, lineage }) => <div className="row tree-row" role="row" key={node.id} aria-selected={route.selection.kind === 'conversation' && route.selection.conversationId === node.id}>
                <span role="cell">
                  <span className="branch" aria-hidden="true">
                    {'· '.repeat(Math.min(depth, 8))}
                  </span>
                  {link({ kind: 'conversation', conversationId: node.id }, node.name)}{lineage === 'orphan' || lineage === 'cycle' ? ` · ${lineage}` : ''}
                </span>
                <span role="cell">
                  {node.state}
                </span>
                <span role="cell" className="meta">
                  {node.id} · {[node.model, node.effort, node.detail ?? node.updatedAt].filter(Boolean).join(' · ')}{node.forkSourceRequestId && <button onClick={event => inspect(node.forkSourceRequestId!, event.currentTarget)}>Inspect fork source</button>}
                </span>
              </div>)}
            </div>
            {!conversations.total && <Empty>No conversations are available.</Empty>}
            <Pager page={conversations.page} count={conversations.pageCount} total={conversations.total} onPage={setPage} />
          </section>
        </>, [actors, conversations, link, inspect, route.selection]);
  const inboxScreen = useMemo(() => <>
          <div className="toolbar filters">
            {(['sender', 'recipient'] as const).map(key => <label key={key}>
              {key === 'sender' ? 'Sender' : 'Recipient'}
              <select value={route.messageFilters[key] ?? ''} onChange={event => navigate({ ...route, messageFilters: { ...route.messageFilters, [key]: event.target.value || undefined } })}>
                <option value="">All endpoints</option>
                {route.messageFilters[key] && !endpoints.includes(route.messageFilters[key]!) && <option>
                  {route.messageFilters[key]}
                </option>}{endpoints.map(endpoint => <option key={endpoint}>
                  {endpoint}
                </option>)}
              </select>
            </label>)}
            <label>Message type<select value={route.messageFilters.type ?? ''} onChange={event => navigate({ ...route, messageFilters: { ...route.messageFilters, type: event.target.value as RouteState['messageFilters']['type'] || undefined } })}>
              <option value="">All types</option>
              {['NEW_TASK', 'MESSAGE', 'FINAL_ANSWER', 'PROGRESS'].map(kind => <option key={kind}>
                {kind}
              </option>)}
            </select>
            </label>
          </div>
          <p className="hint">Filters match supplied endpoint labels. They do not prove an actor incarnation or that a question was answered.</p>
          <div role="list" aria-label="Inbox messages">
            {inbox.rows.map(item => <div className="row" role="listitem" key={item.id}>
              <strong>
                {item.sender}{item.recipient && ` → ${item.recipient}`}
              </strong>
              <span>
                {item.type ?? item.state}{item.ordinal !== undefined && ` · #${item.ordinal}`}
              </span>
              <span className="message">
                {item.message}
                <span className="meta">
                  {item.receivedAt}
                </span>
              </span>
              <div>
                {[...new Set([item.sender, item.recipient].filter((endpoint): endpoint is string => Boolean(endpoint)))].map(endpoint => {
                  return <span key={endpoint}>
                    {(endpointNodes.get(endpoint) ?? []).map(node => <span key={node.id}>
                      {link({ kind: 'conversation', conversationId: node.id }, `Open conversation ${node.name} · ${node.id}`)} </span>)}{(endpointActors.get(endpoint) ?? []).map(actor => <span key={actorIdentityKey(identityOf(actor))}>
                        {link({ kind: 'actor', identity: identityOf(actor) }, `Select actor ${actor.name} · run ${actor.run} · incarnation ${actor.incarnation}`)} </span>)}
                    <button onClick={() => navigate({ ...route, messageFilters: { ...route.messageFilters, sender: endpoint } })}>Filter {endpoint}
                    </button>
                  </span>;
                })}
              </div>
            </div>)}
          </div>
          {!inbox.total && <Empty>No messages match this context and filters.</Empty>}
          <Pager page={inbox.page} count={inbox.pageCount} total={inbox.total} onPage={setPage} />
        </>, [inbox, endpoints, endpointNodes, endpointActors, link, route, navigate]);
  const selectedIdentity = route.selection.kind === 'actor' ? actorIdentityKey(route.selection.identity) : undefined;
  const showAllOperations = allOperations || !selectedIdentity;
  const chatCommands = pendingCommands.filter(record => showAllOperations || actorIdentityKey(record.submission.command.target) === selectedIdentity);
  const activeTargets = new Set((data.actors ?? []).filter(actor => actor.run === data.hostRun && ['running', 'waiting'].includes(actor.lifecycle)).map(actor => actorIdentityKey(identityOf(actor))));
  const chatCommandIds = new Set(chatCommands.map(record => canonicalOperationId(record.submission.operation_id)));
  const chatReceipts = (data.commandReceipts ?? []).filter(receipt => showAllOperations || (receipt.target ? actorIdentityKey(receipt.target) === selectedIdentity : isOperationId(receipt.commandId) && chatCommandIds.has(canonicalOperationId(receipt.commandId))));
  const inspected = route.requestId ? requestIndex.get(route.requestId) : undefined;
  return <div className="harness">
    <a className="skip-link" href="#main-content">Skip to content</a>
    <header className="top">
      <span className="brand">Harness <span className="hint">/ operator</span>
      </span>
      <nav className="tabs" aria-label="Views">
        {screens.map(screen => <a className="tab" key={screen} href={routeUrl({ ...route, screen }, new URL(window.location.href)).toString()} aria-current={route.screen === screen ? 'page' : undefined} onClick={(event: MouseEvent<HTMLAnchorElement>) => {
          if (event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey) {
            event.preventDefault();
            navigate({ ...route, screen });
          }
        }}>
          {screenNames[screen]}
        </a>)}
      </nav>
    </header>
    {transportPhase !== 'ready' && <p className="snapshot-status" role="status">{route.screen === 'chat' ? 'Host unavailable · ' : ''}Showing last snapshot · {transportPhase}; controls await a fresh snapshot.</p>}
    {route.screen !== 'chat' && <section className="selection-context" aria-label="Selected context">
      <strong>Selected context:</strong> <span>
        {selectedDescription}
      </span>
      <button disabled={route.selection.kind === 'none' && !issue} onClick={() => navigate({ ...route, selection: { kind: 'none' } })}>Clear selection</button>
      <label>
        <input type="checkbox" checked={route.global} onChange={event => navigate({ ...route, global: event.target.checked })} />Global activity</label>
      {issue && <p className="error" role="status">
        {issue}
      </p>}{resolved.missing && <p className="error" role="status">The selected actor or conversation is unavailable. The exact selection remains preserved; choose a new context explicitly.</p>}{resolved.actor?.kind === 'workflow' && !resolved.conversationId && <p role="status">Workflow actor model history is unavailable.</p>}
    </section>}
    <main id="main-content" tabIndex={-1}>
      <div className="toolbar">
        <h1 ref={heading} tabIndex={-1}>
          {screenNames[route.screen]}
        </h1>
        <span className="hint">Keyboard: g then t / l / i / c
        </span>
      </div>
      {!screens.includes(route.screen) ? <Empty>This view is unavailable in this host mode. Choose a view above.</Empty> : <>
        {((route.screen === 'tree' && !embedded) || ['timeline', 'inbox'].includes(route.screen)) && <div className="toolbar">
          <label>Search <input type="search" value={search} onChange={event => { setSearch(event.target.value); setPage(0); setActorPage(0); }} />
          </label>
          {route.screen === 'tree' && <button disabled={route.selection.kind === 'none'} onClick={() => {
            setSearch(''); if (route.selection.kind === 'actor') {
              const index = (data.actors ?? []).findIndex(actor => sameSelection(route.selection, { kind: 'actor', identity: identityOf(actor) }));
              setActorPage(Math.max(0, Math.floor(index / 100)));
            }
            else if (route.selection.kind === 'conversation') {
              const selectedId = route.selection.conversationId;
              const index = orderedNodes.findIndex(({ node }) => node.id === selectedId);
              setPage(Math.max(0, Math.floor(index / 100)));
            }
          }}>Jump to selected</button>}
        </div>}
        {route.screen === 'tree' && (embedded ? <ActiveWorkerTree data={data} route={route} navigate={navigate} /> : tree)}
        {route.screen === 'timeline' && timeline}
        {route.screen === 'inbox' && inboxScreen}
        {route.screen === 'chat' && <WorkerChat data={data} route={route} navigate={navigate} transportPhase={transportPhase} issue={issue} onAuthExpired={onAuthExpired}>
          <ActorComposer key={route.selection.kind === 'actor' ? actorIdentityKey(route.selection.identity) : 'none'} data={data} route={route} unavailable={Boolean(issue || resolved.missing)} ready={transportPhase === 'ready'} onHostCommand={onHostCommand} />
          <section className="actor-operations" aria-label="Host operations">
            <div className="toolbar"><h2>Host operations</h2>
              {selectedIdentity && <label><input type="checkbox" checked={allOperations} onChange={event => setAllOperations(event.target.checked)} />All host operations</label>}
            </div>
            <RetainedCommands commands={chatCommands} run={data.hostRun} activeTargets={activeTargets} ready={transportPhase === 'ready' && !issue} onRetry={onRetry} />
            <CommandReceipts receipts={chatReceipts} />
            {!chatCommands.length && !chatReceipts.length && <p className="hint">No operations retained for {showAllOperations ? 'this browser or host' : 'this exact actor'}.</p>}
          </section>
        </WorkerChat>}
        {route.screen === 'command' && <section>
          <h2>Send a command</h2>
          <p className="hint">Standalone deterministic demo · sending is separate from server acceptance and completion.</p>
          <p>
            <code>echo TEXT · test · wait · message TEXT · cancel · child TEXT · fail</code>
          </p>
          <div className="toolbar" aria-label="Common deterministic actions">
            {[['Run test', 'test'], ['Start wait', 'wait'], ['Cancel wait', 'cancel'], ['Create example child', 'child browser example'], ['Fail request', 'fail'], ['Recover with echo', 'echo recovered']].map(([label, value]) => <button key={value} disabled={!onDemoCommand || transportPhase !== 'ready'} onClick={() => sendDemo(value!)}>
              {label}
            </button>)}
          </div>
          <form className="command-form" onSubmit={event => {
            event.preventDefault(); if (demo.text.trim())
              sendDemo(demo.text);
          }}>
            <label htmlFor="demo-command">Command</label>
            <input id="demo-command" value={demo.text} onChange={event => demo.setText(event.target.value)} autoComplete="off" />
            <button disabled={!demo.text.trim() || !onDemoCommand || transportPhase !== 'ready'}>Send</button>
          </form>
          {!onDemoCommand && <p role="status">Command channel is unavailable; this view is read-only.</p>}{demo.error && <p role="status" className="error">
            {demo.error}
          </p>}{demoLocalFeedback && <p role="status">
            {demoLocalFeedback}
          </p>}{demoFeedback && <p role="status" className="error">
            {demoFeedback}
          </p>}{demo.lastSubmitted && <div>
            <pre>
              {demo.lastSubmitted}
            </pre>
            <button onClick={demo.restore}>Restore last demo command</button>
          </div>}{acceptedCommandIds.filter(id => !commandIds.has(id)).map(id => <p role="status" key={id}>Server accepted command {id}; this acknowledgment is not a terminal outcome.</p>)}
          <h2>Authoritative activity</h2>
          {timeline}
        </section>}
      </>}{route.requestId && <NodeWindow requestId={route.requestId} conversationId={inspected?.nodeId} hostRun={data.hostRun} refreshKey={inspected?.historyRefreshKey} onAuthExpired={onAuthExpired} onClose={closeInspector} />}
    </main>
  </div>;
}
