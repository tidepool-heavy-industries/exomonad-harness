import { useCallback, useEffect, useState } from 'react';
import type { RouteState, Screen } from './client-contract';

const keys = ['view', 'run', 'actor', 'incarnation', 'conversation', 'global', 'request', 'sender', 'recipient', 'type'] as const;
const screens: readonly Screen[] = ['tree', 'timeline', 'inbox', 'command', 'chat'];
const kinds = ['NEW_TASK', 'MESSAGE', 'FINAL_ANSWER', 'PROGRESS'] as const;
const screenPaths: Record<Exclude<Screen, 'chat'>, string> = {
  tree: '/tree', timeline: '/timeline', inbox: '/inbox', command: '/command',
};
const pathScreens = new Map<string, Screen>(Object.entries(screenPaths).map(([screen, path]) => [path, screen as Screen]));
export const defaultRoute: RouteState = { screen: 'tree', selection: { kind: 'none' }, global: false, messageFilters: {} };

function invalidRoute(screen: Screen): { route: RouteState; issue: string } {
  return {
    route: { screen, selection: { kind: 'none' }, global: false, messageFilters: {} },
    issue: 'This URL contains an invalid or incomplete selection. Choose an exact context again.',
  };
}

function actorFromPath(pathname: string): string | undefined {
  if (!pathname.startsWith('/chat/')) return undefined;
  const encodedSegments = pathname.slice('/chat/'.length).split('/');
  if (encodedSegments.some(segment => segment.length === 0)) return undefined;
  try {
    const segments = encodedSegments.map(segment => decodeURIComponent(segment));
    if (segments.some(segment => !segment || segment.includes('/') || segment.includes('\\') || segment === '.' || segment === '..')) return undefined;
    return `/${segments.join('/')}`;
  } catch {
    return undefined;
  }
}

export function parseRoute(url: URL): { route: RouteState; issue?: string } {
  const p = url.searchParams;
  const duplicate = keys.some(key => p.getAll(key).length > 1);
  const suppliedView = p.get('view');
  const view = suppliedView === 'host' ? 'chat' : suppliedView;
  const run = p.get('run');
  const actor = p.get('actor');
  const incarnation = p.get('incarnation');
  const conversation = p.get('conversation');
  const type = p.get('type');

  let screen: Screen;
  let pathActor: string | undefined;
  let pathIssue = false;
  if (url.pathname === '/') {
    screen = view && screens.includes(view as Screen) ? view as Screen : 'tree';
  } else if (url.pathname === '/chat' || url.pathname === '/host') {
    screen = 'chat';
  } else if (url.pathname.startsWith('/chat/')) {
    screen = 'chat';
    pathActor = actorFromPath(url.pathname);
    pathIssue = pathActor === undefined;
  } else {
    const fromPath = pathScreens.get(url.pathname);
    screen = fromPath ?? 'tree';
    pathIssue = fromPath === undefined;
  }

  const hasIdentityFields = run !== null || actor !== null || incarnation !== null;
  const canonicalActorSelection = pathActor !== undefined;
  const queryActorSelection = !canonicalActorSelection && hasIdentityFields;
  const actorMatchesPath = canonicalActorSelection && actor !== null
    && actorPath(actor) !== undefined
    && (actor.startsWith('/') ? actor.slice(1) : actor) === pathActor?.slice(1);
  const selectionConflict = (canonicalActorSelection && (conversation !== null || (actor !== null && !actorMatchesPath)))
    || (queryActorSelection && conversation !== null)
    || (view !== null && url.pathname !== '/' && view !== screen);
  const missingIdentity = canonicalActorSelection
    ? (!run || !incarnation)
    : queryActorSelection && (!run || !actor || !incarnation);
  const invalid = duplicate
    || pathIssue
    || selectionConflict
    || missingIdentity
    || (view !== null && !screens.includes(view as Screen))
    || (hasIdentityFields && !queryActorSelection && !canonicalActorSelection)
    || (conversation !== null && conversation.length === 0)
    || (p.has('global') && p.get('global') !== '1')
    || (type !== null && !kinds.includes(type as typeof kinds[number]));
  if (invalid) return invalidRoute(screen);

  const selection: RouteState['selection'] = canonicalActorSelection && run && incarnation
    ? { kind: 'actor', identity: { run, actor: actor ?? pathActor!, incarnation } }
    : queryActorSelection && run && actor && incarnation
      ? { kind: 'actor', identity: { run, actor, incarnation } }
      : conversation
        ? { kind: 'conversation', conversationId: conversation }
        : { kind: 'none' };

  return {
    route: {
      screen,
      selection,
      global: p.get('global') === '1',
      requestId: p.get('request') || undefined,
      messageFilters: {
        sender: p.get('sender') || undefined,
        recipient: p.get('recipient') || undefined,
        type: kinds.includes(type as typeof kinds[number]) ? type as typeof kinds[number] : undefined,
      },
    },
  };
}

function actorPath(actor: string): string | undefined {
  const segments = actor.split('/');
  if (segments[0] === '') segments.shift();
  if (segments.length === 0 || segments.some(segment => !segment || segment === '.' || segment === '..' || segment.includes('\\')))
    return undefined;
  return `/chat/${segments.map(segment => encodeURIComponent(segment)).join('/')}`;
}

export function routeUrl(route: RouteState, base: URL): URL {
  const url = new URL(base);
  keys.forEach(key => url.searchParams.delete(key));
  if (route.screen === 'chat' && route.selection.kind === 'actor') {
    const path = actorPath(route.selection.identity.actor);
    url.pathname = path ?? '/chat';
    url.searchParams.set('run', route.selection.identity.run);
    url.searchParams.set('incarnation', route.selection.identity.incarnation);
    // The readable actor path omits its root slash, so retain that bit when
    // the exact actor identity uses the slashless spelling.
    if (!route.selection.identity.actor.startsWith('/')) url.searchParams.set('actor', route.selection.identity.actor);
  } else if (route.selection.kind === 'actor') {
    url.pathname = route.screen === 'chat' ? '/chat' : screenPaths[route.screen];
    const { run, actor, incarnation } = route.selection.identity;
    url.searchParams.set('run', run);
    url.searchParams.set('actor', actor);
    url.searchParams.set('incarnation', incarnation);
  } else {
    url.pathname = route.screen === 'chat' ? '/chat' : screenPaths[route.screen];
    if (route.selection.kind === 'conversation') url.searchParams.set('conversation', route.selection.conversationId);
  }
  if (route.global) url.searchParams.set('global', '1');
  if (route.requestId) url.searchParams.set('request', route.requestId);
  Object.entries(route.messageFilters).forEach(([key, value]) => {
    if (value) url.searchParams.set(key, value);
  });
  return url;
}

export function useRoute() {
  const [parsed, setParsed] = useState(() => parseRoute(new URL(window.location.href)));
  useEffect(() => {
    const pop = () => setParsed(parseRoute(new URL(window.location.href)));
    window.addEventListener('popstate', pop);
    return () => window.removeEventListener('popstate', pop);
  }, []);
  const navigate = useCallback((route: RouteState, replace = false) => {
    const current = new URL(window.location.href);
    const next = routeUrl(route, current);
    if (next.href !== current.href)
      window.history[replace ? 'replaceState' : 'pushState'](null, '', next);
    setParsed({ route });
  }, []);
  return { ...parsed, navigate };
}
