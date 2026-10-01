import { describe, expect, it } from 'vitest';
import { defaultRoute, parseRoute, routeUrl } from './navigation';
describe('native route codec', () => {
  it('roundtrips opaque identity/context and preserves unknown query fields', () => {
    const route = { ...defaultRoute, screen: 'inbox' as const, selection: { kind: 'actor' as const, identity: { run: 'Run A', actor: '/actor/日本', incarnation: 'Inc.CASE' } }, global: true, requestId: 'request/id', messageFilters: { sender: '/operator', recipient: '/actor/日本', type: 'FINAL_ANSWER' as const } };
    const url = routeUrl(route, new URL('https://host/?theme=custom&theme=two#anchor'));
    expect(parseRoute(url)).toEqual({ route, issue: undefined });
    expect(url.searchParams.getAll('theme')).toEqual(['custom', 'two']);
    expect(url.hash).toBe('#anchor');
  });
  it.each(['?actor=a&run=r', '?actor=a&run=r&incarnation=i&conversation=c', '?view=nope', '?type=BOGUS', '?global=0', '?conversation=', '?actor=a&actor=b&run=r&incarnation=i'])('rejects invalid routes without replacement: %s', query => {
    const parsed = parseRoute(new URL('https://host/' + query));
    expect(parsed.issue).toBeTruthy();
    expect(parsed.route.selection.kind).toBe('none');
  });
  it('replaces selection fields and keeps unrelated native URL context', () => {
    const url = routeUrl({ ...defaultRoute, selection: { kind: 'conversation', conversationId: 'unattached' } }, new URL('https://host/?run=old&actor=old&incarnation=old&plugin=1'));
    expect(url.searchParams.has('actor')).toBe(false);
    expect(url.searchParams.get('conversation')).toBe('unattached');
    expect(url.searchParams.get('plugin')).toBe('1');
  });
});
