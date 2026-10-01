import { useCallback, useEffect, useState } from 'react';
import type { RouteState, Screen } from './client-contract';
const keys = ['view', 'run', 'actor', 'incarnation', 'conversation', 'global', 'request', 'sender', 'recipient', 'type'] as const;
const screens: Screen[] = ['tree', 'timeline', 'inbox', 'host', 'command'];
const kinds = ['NEW_TASK', 'MESSAGE', 'FINAL_ANSWER', 'PROGRESS'] as const;
export const defaultRoute: RouteState = { screen: 'tree', selection: { kind: 'none' }, global: false, messageFilters: {} };
export function parseRoute(url: URL): {
  route: RouteState;
  issue?: string;
} {
  const p = url.searchParams;
  const invalid = keys.some(key => p.getAll(key).length > 1);
  const view = p.get('view');
  const run = p.get('run'), actor = p.get('actor'), incarnation = p.get('incarnation'), conversation = p.get('conversation');
  const hasActor = [run, actor, incarnation].some(x => x !== null);
  const type = p.get('type');
  const issue = invalid || (view !== null && !screens.includes(view as Screen)) || (hasActor && (!run || !actor || !incarnation || conversation !== null)) || conversation === '' || (p.has('global') && p.get('global') !== '1') || (type !== null && !kinds.includes(type as typeof kinds[number]));
  const selection: RouteState['selection'] = hasActor && run && actor && incarnation && conversation === null
    ? { kind: 'actor', identity: { run, actor, incarnation } }
    : conversation ? { kind: 'conversation', conversationId: conversation } : { kind: 'none' };
  return { route: { screen: screens.includes(view as Screen) ? view as Screen : 'tree', selection: issue ? { kind: 'none' } : selection, global: p.get('global') === '1', requestId: p.get('request') || undefined, messageFilters: { sender: p.get('sender') || undefined, recipient: p.get('recipient') || undefined, type: kinds.includes(type as typeof kinds[number]) ? type as typeof kinds[number] : undefined } }, issue: issue ? 'This URL contains an invalid or incomplete selection. Choose an exact context again.' : undefined };
}
export function routeUrl(route: RouteState, base: URL): URL {
  const url = new URL(base);
  keys.forEach(key => url.searchParams.delete(key));
  url.searchParams.set('view', route.screen);
  if (route.selection.kind === 'actor')
    Object.entries(route.selection.identity).forEach(([key, value]) => url.searchParams.set(key, value));
  if (route.selection.kind === 'conversation')
    url.searchParams.set('conversation', route.selection.conversationId);
  if (route.global)
    url.searchParams.set('global', '1');
  if (route.requestId)
    url.searchParams.set('request', route.requestId);
  Object.entries(route.messageFilters).forEach(([key, value]) => {
    if (value)
      url.searchParams.set(key, value);
  });
  return url;
}
export function useRoute() {
  const [parsed, setParsed] = useState(() => parseRoute(new URL(window.location.href)));
  useEffect(() => { const pop = () => setParsed(parseRoute(new URL(window.location.href))); window.addEventListener('popstate', pop); return () => window.removeEventListener('popstate', pop); }, []);
  const navigate = useCallback((route: RouteState, replace = false) => {
    window.history[replace ? 'replaceState' : 'pushState'](null, '', routeUrl(route, new URL(window.location.href)));
    setParsed({ route });
  }, []);
  return { ...parsed, navigate };
}
