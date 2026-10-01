import { describe, expect, it } from 'vitest';
import { defaultRoute, parseRoute, routeUrl } from './navigation';

describe('native route codec', () => {
  it('roundtrips exact actor identity in canonical pathname and query', () => {
    const route = {
      ...defaultRoute,
      screen: 'chat' as const,
      selection: { kind: 'actor' as const, identity: { run: 'Run A', actor: '/root/worker 日本', incarnation: 'Inc.CASE' } },
      global: true,
      requestId: 'request/id',
      messageFilters: { sender: '/operator', recipient: '/root/worker 日本', type: 'FINAL_ANSWER' as const },
    };
    const url = routeUrl(route, new URL('https://host/tree?theme=custom&theme=two#anchor'));
    expect(url.pathname).toBe('/chat/root/worker%20%E6%97%A5%E6%9C%AC');
    expect(url.searchParams.get('run')).toBe('Run A');
    expect(url.searchParams.get('incarnation')).toBe('Inc.CASE');
    expect(url.searchParams.has('actor')).toBe(false);
    expect(parseRoute(url)).toEqual({ route, issue: undefined });
    expect(url.searchParams.getAll('theme')).toEqual(['custom', 'two']);
    expect(url.hash).toBe('#anchor');
  });

  it('preserves a slashless actor identity that shares the same readable path', () => {
    const route = { ...defaultRoute, screen: 'chat' as const, selection: { kind: 'actor' as const, identity: { run: 'r', actor: 'root/worker', incarnation: 'i' } } };
    const url = routeUrl(route, new URL('https://host/tree'));
    expect(url.pathname).toBe('/chat/root/worker');
    expect(url.searchParams.get('actor')).toBe('root/worker');
    expect(parseRoute(url)).toEqual({ route, issue: undefined });
  });

  it.each([
    ['tree', '/tree'], ['timeline', '/timeline'], ['inbox', '/inbox'],
    ['host', '/host'], ['command', '/command'], ['chat', '/chat'],
  ] as const)('uses %s as the page route', (screen, pathname) => {
    const route = { ...defaultRoute, screen };
    const url = routeUrl(route, new URL('https://host/'));
    expect(url.pathname).toBe(pathname);
    expect(parseRoute(url)).toEqual({ route, issue: undefined });
  });

  it.each(['tree', 'timeline', 'inbox', 'host', 'command', 'chat'] as const)(
    'preserves exact actor identity on the %s page', screen => {
      const route = {
        ...defaultRoute,
        screen,
        selection: { kind: 'actor' as const, identity: { run: 'run-7', actor: '/root/worker', incarnation: 'inc-2' } },
      };
      const url = routeUrl(route, new URL('https://host/chat/root/worker?run=old&incarnation=old'));
      expect(parseRoute(url)).toEqual({ route, issue: undefined });
      if (screen === 'chat') expect(url.pathname).toBe('/chat/root/worker');
      else {
        expect(url.pathname).toBe(`/${screen}`);
        expect(url.searchParams.getAll('run')).toEqual(['run-7']);
        expect(url.searchParams.getAll('actor')).toEqual(['/root/worker']);
        expect(url.searchParams.getAll('incarnation')).toEqual(['inc-2']);
      }
    },
  );

  it('allows an unselected chat page and reads unambiguous legacy query links', () => {
    expect(parseRoute(new URL('https://host/chat'))).toEqual({ route: { ...defaultRoute, screen: 'chat' } });
    expect(parseRoute(new URL('https://host/?view=chat&run=r&actor=%2Froot%2Fworker&incarnation=i'))).toEqual({
      route: { ...defaultRoute, screen: 'chat', selection: { kind: 'actor', identity: { run: 'r', actor: '/root/worker', incarnation: 'i' } } },
    });
    expect(parseRoute(new URL('https://host/?view=host&run=r&actor=%2Froot%2Fworker&incarnation=i'))).toEqual({
      route: { ...defaultRoute, screen: 'host', selection: { kind: 'actor', identity: { run: 'r', actor: '/root/worker', incarnation: 'i' } } },
    });
  });

  it('keeps a page and issue when its exact actor identity is incomplete', () => {
    const parsed = parseRoute(new URL('https://host/host?run=r&incarnation=i'));
    expect(parsed.issue).toBeTruthy();
    expect(parsed.route.screen).toBe('host');
    expect(parsed.route.selection.kind).toBe('none');
  });

  it.each([
    '/chat/root/worker?run=r',
    '/chat/root/worker?run=r&incarnation=i&actor=%2Froot%2Fother',
    '/chat/root/%ZZ?run=r&incarnation=i',
    '/chat/root//worker?run=r&incarnation=i',
    '/chat/../worker?run=r&incarnation=i',
    '/tree?view=chat',
    '/chat?actor=%2Froot&run=r&incarnation=i&conversation=c',
    '/chat?run=r&incarnation=i',
    '/host?run=r&incarnation=i',
    '/?view=nope',
    '/?view=chat&actor=%2Froot&run=r',
    '/?view=host&run=r&incarnation=i',
    '/?actor=a&actor=b&run=r&incarnation=i&view=chat',
    '/?type=BOGUS',
    '/?global=0',
    '/?conversation=',
  ])('rejects malformed, incomplete, or conflicting routes without replacement: %s', pathname => {
    const parsed = parseRoute(new URL(`https://host${pathname}`));
    expect(parsed.issue).toBeTruthy();
    expect(parsed.route.selection.kind).toBe('none');
  });

  it('replaces route selection and keeps unrelated native URL context', () => {
    const url = routeUrl({ ...defaultRoute, screen: 'chat', selection: { kind: 'conversation', conversationId: 'unattached' } }, new URL('https://host/chat/old?run=old&incarnation=old&theme=custom'));
    expect(url.pathname).toBe('/chat');
    expect(url.searchParams.has('run')).toBe(false);
    expect(url.searchParams.get('conversation')).toBe('unattached');
    expect(url.searchParams.get('theme')).toBe('custom');
  });
});
