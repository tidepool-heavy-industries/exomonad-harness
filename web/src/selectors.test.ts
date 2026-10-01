import { describe, expect, it } from 'vitest'
import { normalizeSnapshot, actorIdentityKey } from './protocol'
import { toViewModel } from './integration'
import { createViewSelector, formatActivityTime, needsActivityClock, orderConversationTree, pageRows,
  resolveSelection, selectViewModel, sortActivity } from './selectors'
import type { HarnessViewModel } from './view-model'
import type { Selection } from './client-contract'

const identity = { run: 'RUN/opaque', actor: '/worker', incarnation: 'Inc-A' }
const selection: Selection = { kind: 'actor', identity }
function fixture() {
  return toViewModel(normalizeSnapshot({ seq: 1, actors: [
    { identity, parent: null, kind: 'model', lifecycle: 'running', modelConversation: 'CONV' },
    { identity: { ...identity, actor: 'workflow' }, parent: identity,
      kind: 'workflow', lifecycle: 'waiting', modelConversation: null },
  ], conversations: [
    { id: 'CONV', path: '/source/path', state: 'idle', parentId: null },
    { id: 'free', path: '/unattached', state: 'idle' },
  ], requests: [{ id: 'r', conversationId: 'CONV', state: 'running' },
    { id: 'other', conversationId: 'free', state: 'running' }], jobs: [], envelopes: [
    { id: 'operator', sender: '/operator', recipient: '/source/path', type: 'MESSAGE', payload: 'hi' },
    { id: 'answer', sender: '/source/path', recipient: '/operator', type: 'FINAL_ANSWER', payload: 'done' },
    { id: 'progress', sender: '/source/path', recipient: '/peer', type: 'PROGRESS', payload: 'working' },
    { id: 'free', sender: '/operator', recipient: '/unattached', type: 'MESSAGE', payload: 'unrelated' },
  ] }))
}

describe('exact linked selection', () => {
  it('joins only the supplied model conversation and preserves unattached/workflow rows', () => {
    const data = fixture()
    expect(resolveSelection(data, selection)).toMatchObject({
      actor: { id: actorIdentityKey(identity) }, conversationId: 'CONV', endpoint: '/source/path', missing: false,
    })
    expect(resolveSelection(data, { kind: 'actor', identity: { ...identity, actor: 'workflow' } }))
      .toMatchObject({ actor: { kind: 'workflow' }, missing: false, conversationId: undefined })
    expect(resolveSelection(data, { kind: 'conversation', conversationId: 'free' }))
      .toEqual({ conversationId: 'free', endpoint: '/unattached', endpoints: ['/unattached'], missing: false })
    expect(selectViewModel(data, { kind: 'none' }, false, {})).toBe(data)
    expect(selectViewModel(data, selection, false, {}).actors).toBe(data.actors)
  })

  it('does not substitute an incarnation, normalize opaque IDs, or infer from labels', () => {
    const data = fixture()
    data.actors![0] = { ...data.actors![0]!, id: actorIdentityKey({ ...identity, incarnation: 'Inc-B' }), incarnation: 'Inc-B' }
    expect(resolveSelection(data, selection)).toEqual({ missing: true, endpoints: [] })
    expect(selectViewModel(data, selection, false, {}).timeline).toEqual([])
    expect(resolveSelection(data, { kind: 'conversation', conversationId: 'conv' }).missing).toBe(true)
    expect(resolveSelection(data, { kind: 'actor', identity: { ...identity, run: 'run/opaque' } }).missing).toBe(true)
  })

  it('filters workflow endpoint labels without inventing a model conversation', () => {
    const data = fixture()
    data.inbox.push({ id: 'workflow-message', sender: '/operator', recipient: 'workflow', state: 'MESSAGE', message: 'control' })
    const selected: Selection = { kind: 'actor', identity: { ...identity, actor: 'workflow' } }
    expect(resolveSelection(data, selected)).toMatchObject({ endpoint: 'workflow', conversationId: undefined, missing: false })
    const view = selectViewModel(data, selected, false, {})
    expect(view.inbox.map((row) => row.id)).toEqual(['workflow-message'])
    expect(view.timeline).toEqual([])
    expect(view.nodes).toEqual([])
  })

  it('includes both supplied actor and conversation endpoint labels, never a path-based join', () => {
    const data = fixture()
    data.inbox.push({ id: 'actor-label', sender: '/operator', recipient: '/worker', state: 'MESSAGE', message: 'to actor' })
    expect(resolveSelection(data, selection).endpoints).toEqual(['/worker', '/source/path'])
    expect(selectViewModel(data, selection, false, {}).inbox.map((row) => row.id))
      .toEqual(['operator', 'answer', 'progress', 'actor-label'])
    expect(selectViewModel(data, { kind: 'conversation', conversationId: 'CONV' }, false, {}).inbox.map((row) => row.id))
      .toEqual(['operator', 'answer', 'progress'])
    expect(resolveSelection(data, { kind: 'none' }).endpoints).toEqual([])
  })

  it('conjoins context with sender, recipient and type, including /operator', () => {
    const data = fixture()
    expect(selectViewModel(data, selection, false, {}).inbox.map((row) => row.id))
      .toEqual(['operator', 'answer', 'progress'])
    expect(selectViewModel(data, selection, false, { sender: '/operator', recipient: '/source/path', type: 'MESSAGE' })
      .inbox.map((row) => row.id)).toEqual(['operator'])
    expect(selectViewModel(data, selection, true, { sender: '/operator', type: 'MESSAGE' }).inbox.map((row) => row.id))
      .toEqual(['operator', 'free'])
    expect(selectViewModel(data, selection, false, { sender: '/operator', type: 'PROGRESS' }).inbox).toEqual([])
  })

  it('falls back only to supported legacy envelope types', () => {
    const data = fixture()
    data.inbox = [{ id: 'legacy', sender: '/operator', message: 'old', state: 'MESSAGE' },
      { id: 'unknown', sender: '/operator', message: 'unknown', state: 'answered' }]
    expect(selectViewModel(data, { kind: 'none' }, false, { type: 'MESSAGE' }).inbox.map((row) => row.id)).toEqual(['legacy'])
  })

  it('retains filtered table references across unrelated row/map updates', () => {
    const data = fixture(), select = createViewSelector()
    const first = select(data, selection, false, {})
    const next = select({ ...data, inbox: [...data.inbox, { id: 'x', sender: '/else', message: 'x', state: 'MESSAGE' }],
      timeline: [...data.timeline, { id: 'new', nodeId: 'free', kind: 'job', label: 'other', state: 'running' }] }, selection, false, {})
    expect(next.nodes).toBe(first.nodes)
    expect(next.timeline).toBe(first.timeline)
    expect(next.inbox).toBe(first.inbox)
    expect(select(data, { kind: 'conversation', conversationId: 'free' }, false, {}).timeline).not.toBe(first.timeline)
  })
})

describe('indexed tree and bounded pages', () => {
  it('uses exact parent/null before legacy paths, with ambiguous paths and orphans visible', () => {
    const nodes: HarnessViewModel['nodes'] = [
      { id: 'child', name: '/unrelated', parentId: 'root', state: 'idle' },
      { id: 'root', name: '/root', parentId: null, state: 'idle' },
      { id: 'explicit-root', name: '/root/not-child', parentId: null, state: 'idle' },
      { id: 'legacy', name: '/root/legacy', state: 'idle' },
      { id: 'orphan', name: '/orphan', parentId: 'absent', state: 'idle' },
      { id: 'dup1', name: '/duplicate', state: 'idle' },
      { id: 'dup2', name: '/duplicate', state: 'idle' },
      { id: 'ambiguous', name: '/duplicate/child', state: 'idle' },
    ]
    const rows = orderConversationTree(nodes)
    expect(rows.map(({ node }) => node.id)).toEqual(['root', 'child', 'legacy', 'explicit-root', 'orphan', 'dup1', 'dup2', 'ambiguous'])
    expect(rows.find(({ node }) => node.id === 'orphan')?.lineage).toBe('orphan')
    expect(rows.find(({ node }) => node.id === 'ambiguous')?.depth).toBe(0)
  })

  it('keeps cycles and their descendants visible exactly once without recursive traversal', () => {
    const rows = orderConversationTree([
      { id: 'c', name: '/c', parentId: 'a', state: 'idle' },
      { id: 'a', name: '/a', parentId: 'b', state: 'idle' },
      { id: 'b', name: '/b', parentId: 'a', state: 'idle' },
      { id: 'self', name: '/self', parentId: 'self', state: 'idle' },
    ])
    expect(rows.filter((row) => row.lineage === 'cycle').map((row) => row.node.id)).toEqual(['a', 'b', 'self'])
    expect(new Set(rows.map((row) => row.node.id)).size).toBe(4)
    expect(rows.find((row) => row.node.id === 'c')).toMatchObject({ depth: 1, lineage: 'child' })
  })

  it('resolves legacy root paths while preserving explicit unknown parents', () => {
    const rows = orderConversationTree([
      { id: 'root', name: '/', state: 'idle' },
      { id: 'child', name: '/child', state: 'idle' },
      { id: 'explicit', name: '/child/explicit', parentId: undefined, state: 'idle' },
    ])
    expect(rows.map((row) => [row.node.id, row.depth])).toEqual([['root', 0], ['child', 1], ['explicit', 0]])
  })

  it.each([200, 2000])('orders a %i-worker deep tree iteratively and bounds pages', (count) => {
    const nodes = Array.from({ length: count }, (_, n) => ({ id: `worker${n}`, name: `/label${n}`,
      state: 'idle', parentId: n === 0 ? null : `worker${n - 1}` }))
    const rows = orderConversationTree([...nodes].reverse())
    expect(rows).toHaveLength(count)
    expect(rows.at(-1)?.depth).toBe(count - 1)
    expect(pageRows(rows, 0, 100).rows).toHaveLength(100)
    expect(pageRows(rows, 9999, 50).page).toBe(Math.ceil(count / 50) - 1)
    expect(pageRows([], -4, 100)).toEqual({ rows: [], page: 0, pageCount: 1, total: 0 })
  })
})

describe('activity timing', () => {
  it('sorts known starts chronologically with stable ties and unknown time last', () => {
    const rows: HarnessViewModel['timeline'] = [
      { id: 'unknown', nodeId: 'c', kind: 'request', state: 'running', label: 'u' },
      { id: 'late', nodeId: 'c', kind: 'request', state: 'completed', label: 'r', startedAtMs: 5000 },
      { id: 'tie', nodeId: 'c', kind: 'job', state: 'settled', label: 'j', startedAtMs: 5000 },
      { id: 'first', nodeId: 'c', kind: 'job', state: 'settled', label: 'j', startedAtMs: 0 },
    ]
    expect(sortActivity(rows).map((row) => row.id)).toEqual(['first', 'late', 'tie', 'unknown'])
    expect(formatActivityTime({})).toEqual({ start: 'Unavailable', duration: 'Unavailable' })
    expect(formatActivityTime({ startedAtMs: 0, endedAtMs: 1500 }).duration).toBe('1.5s')
    expect(formatActivityTime({ startedAtMs: 1000, state: 'running' }, 4000).duration).toBe('3s')
    expect(needsActivityClock({ state: 'running' })).toBe(false)
    expect(needsActivityClock({ startedAtMs: 1000, state: 'pending' })).toBe(true)
    expect(needsActivityClock({ startedAtMs: 1000, endedAtMs: 2000, state: 'running' })).toBe(false)
    expect(formatActivityTime({ startedAtMs: 1000, state: 'completed' }, 4000).duration).toBe('Unavailable')
    expect(formatActivityTime({ startedAtMs: 1000, endedAtMs: 500 }).duration).toBe('Unavailable')
  })
})
