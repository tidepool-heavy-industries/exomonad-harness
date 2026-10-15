import CommandView from './CommandView'
import { Empty } from './view-controls'
import { inputPublicationKey } from './command-presentation'
import { ActorComposer, ActorControlPanel, OutgoingInputs } from './ActorControls'
import HostOperations from './HostOperations'
import TimelineView from './TimelineView'
import ConversationTreeView from './ConversationTreeView'
import InboxView from './InboxView'
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent } from 'react';
import NodeWindow from './NodeWindow';
import WorkerChat from './WorkerChat';
import ActiveWorkerTree from './ActiveWorkerTree';
import type { ChatInputOperation } from './generated/chat';
import type { HostActorIdentity } from './protocol';
import { actorIdentityKey, canonicalOperationId, isOperationId } from './protocol';
import type { BrowserCommandRecord } from './pending-commands';
import type { HarnessViewModel } from './view-model';
import type { Screen, Selection, SubmitHostCommand, RetryHostCommand, SubmitDemoCommand, TransportPhase } from './client-contract';
import { createViewSelector, orderConversationTree, pageRows, resolveSelection, needsActivityClock } from './selectors';
import { demoDraftKey, useDraft } from './drafts';
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
  const [renderedInputs, setRenderedInputs] = useState<readonly ChatInputOperation[]>([]);
  const onRenderedInputs = useCallback((references: readonly ChatInputOperation[]) => {
    setRenderedInputs(previous => previous.length === references.length
      && previous.every((reference, index) => inputPublicationKey(reference) === inputPublicationKey(references[index]!)) ? previous : references);
  }, []);
  const currentRenderedInputKeys = useMemo(() => new Set(renderedInputs.flatMap(reference => {
    const key = inputPublicationKey(reference); return key === undefined ? [] : [key];
  })), [renderedInputs]);
  const retainedInputKeys = useMemo(() => new Set(pendingCommands.flatMap(record => {
    if (record.submission.command.action !== 'input' || record.envelopeId == null) return [];
    const key = inputPublicationKey({target:record.submission.command.target, operationId:record.submission.operation_id, envelopeId:record.envelopeId});
    return key === undefined ? [] : [key];
  })), [pendingCommands]);
  const [acknowledgedInputKeys, setAcknowledgedInputKeys] = useState<ReadonlySet<string>>(new Set());
  useLayoutEffect(() => {
    setAcknowledgedInputKeys(previous => {
      const next = new Set([...previous, ...currentRenderedInputKeys].filter(key => retainedInputKeys.has(key)));
      return next.size === previous.size && [...next].every(key => previous.has(key)) ? previous : next;
    });
  }, [currentRenderedInputKeys, retainedInputKeys]);
  const renderedInputKeys = useMemo(() => new Set([...currentRenderedInputKeys, ...acknowledgedInputKeys]), [currentRenderedInputKeys, acknowledgedInputKeys]);
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
  useEffect(() => { setAllOperations(false); }, [route.selection]);
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
  const timeline = <TimelineView {...{ activity, now, inspect, link, setPage }} />;
  const tree = <ConversationTreeView {...{ actors, conversations, link, inspect, route, setPage, setActorPage }} />;
  const inboxScreen = <InboxView {...{ inbox, endpoints, endpointNodes, endpointActors, link, route, navigate, setPage }} />;
  const selectedIdentity = route.selection.kind === 'actor' ? actorIdentityKey(route.selection.identity) : undefined;
  useEffect(() => { setRenderedInputs([]); }, [selectedIdentity, route.screen]);
  const showAllOperations = allOperations || !selectedIdentity;
  const chatCommands = pendingCommands.filter(record => showAllOperations || actorIdentityKey(record.submission.command.target) === selectedIdentity);
  const chatCommandIds = new Set(chatCommands.map(record => canonicalOperationId(record.submission.operation_id)));
  const chatReceipts = (data.commandReceipts ?? []).filter(receipt => showAllOperations || (receipt.target ? actorIdentityKey(receipt.target) === selectedIdentity : isOperationId(receipt.commandId) && chatCommandIds.has(canonicalOperationId(receipt.commandId))));
  const inspected = route.requestId ? requestIndex.get(route.requestId) : undefined;
  return <div className={`harness${route.screen === 'chat' ? ' is-chat' : ''}`}>
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
        {route.screen === 'chat' && <WorkerChat data={data} route={route} navigate={navigate} transportPhase={transportPhase} issue={issue} onAuthExpired={onAuthExpired} onRenderedInputs={onRenderedInputs}
          outgoing={<OutgoingInputs records={chatCommands.filter(record => selectedIdentity !== undefined && actorIdentityKey(record.submission.command.target) === selectedIdentity)} rendered={renderedInputKeys} />}
          composer={<ActorComposer onRetry={onRetry} key={route.selection.kind === 'actor' ? actorIdentityKey(route.selection.identity) : 'none'} data={data} route={route} unavailable={Boolean(issue || resolved.missing)} ready={transportPhase === 'ready'} onHostCommand={onHostCommand} operations={pendingCommands} />} >
          <ActorControlPanel key={selectedIdentity ?? 'none'} data={data} route={route} unavailable={Boolean(issue || resolved.missing)} ready={transportPhase === 'ready'} onHostCommand={onHostCommand} />
          <details className="actor-operation-details"><summary>Host operations</summary>
          <section className="actor-operations" aria-label="Host operations">
            <div className="toolbar"><h2>Host operations</h2>
              {selectedIdentity && <label><input type="checkbox" checked={allOperations} onChange={event => setAllOperations(event.target.checked)} />All host operations</label>}
            </div>
            <HostOperations key={JSON.stringify([selectedIdentity ?? 'all', showAllOperations])} commands={chatCommands} receipts={chatReceipts} run={data.hostRun} data={data} ready={transportPhase === 'ready' && !issue} onRetry={onRetry} />
            {!chatCommands.length && !chatReceipts.length && <p className="hint">No operations retained for {showAllOperations ? 'this browser or host' : 'this exact actor'}.</p>}
          </section></details>
        </WorkerChat>}
        {route.screen === 'command' && <CommandView {...{ demo, sendDemo, onDemoCommand, transportPhase, demoLocalFeedback, demoFeedback, acceptedCommandIds, commandIds, timeline }} />}
      </>}{route.requestId && <NodeWindow requestId={route.requestId} conversationId={inspected?.nodeId} hostRun={data.hostRun} refreshKey={inspected?.historyRefreshKey} onAuthExpired={onAuthExpired} onClose={closeInspector} />}
    </main>
  </div>;
}
