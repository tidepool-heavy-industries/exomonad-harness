import { describe, expect, it } from 'vitest'
import { createElement } from 'react'
import { fireEvent, render, screen, within } from '@testing-library/react'
import { normalizeSnapshot, type Snapshot, type RequestRecord } from './protocol'
import { createViewProjector, toViewModel } from './integration'
import { customJobJourney, customSuccessJourney } from './fixture'
import App from './App'

describe('web outcome integration', () => {
  it('projects an exact retired actor head without any activity rows', () => {
    const snapshot: Snapshot = { seq: '1', hostRun: 'run', actors: [{
      identity: { run: 'run', actor: '/root/old', incarnation: '1' }, parent: null,
      kind: 'model', lifecycle: 'retired', modelConversation: '/root/old',
      modelHeadRequest: 'old-exact-head',
    }], conversations: [], requests: [], jobs: [], envelopes: [] }
    const view = toViewModel(normalizeSnapshot(snapshot))
    expect(view.timeline).toEqual([])
    expect(view.actors?.[0]).toMatchObject({ modelHeadRequest: 'old-exact-head', lifecycle: 'retired' })
  })

  it('shows Haskell-only host actors apart from model conversations', () => {
    const snapshot: Snapshot = {
      seq: '1',
      actors: [
        { identity: { run: 'run-1', actor: 'root', incarnation: 'first' }, parent: null,
          kind: 'model', lifecycle: 'running', modelConversation: 'root-conversation' },
        { identity: { run: 'run-1', actor: 'reviewer', incarnation: 'second' },
          parent: { run: 'run-1', actor: 'root', incarnation: 'first' },
          kind: 'workflow', lifecycle: 'waiting', modelConversation: null },
      ],
      conversations: [{ parentId: null, forkSourceRequestId: null, id: 'root-conversation', path: '/root', state: 'requesting' }],
      requests: [], jobs: [], envelopes: [],
    };
    const view = toViewModel(normalizeSnapshot(snapshot));
    expect(view.nodes).toHaveLength(1);
    expect(view.actors).toHaveLength(2);
    render(createElement(App, { data: view }));
    const actorTable = screen.getByRole('table', { name: 'Host actor lifecycles' });
    const reviewerRow = within(actorTable).getByRole('row', { name: /reviewer/ });
    expect(within(reviewerRow).getByRole('link', { name: 'reviewer' })).toBeInTheDocument();
    expect(within(reviewerRow).getByRole('cell', { name: 'reviewer · workflow' })).toBeInTheDocument();
    expect(within(reviewerRow).getByRole('cell', { name: /incarnation second/ })).toBeInTheDocument();
    expect(within(reviewerRow).getByRole('cell', { name: 'waiting' })).toBeInTheDocument();
    expect(screen.getByRole('table', { name: 'Conversation tree' }).textContent).not.toContain('reviewer');
  });

  it('reopens a settled custom job with progress and retained output in App', () => {
    const [running, settled, reopened] = customSuccessJourney
    expect(running).toBeDefined()
    expect(settled).toBeDefined()
    expect(reopened).toBeDefined()
    const runningView = toViewModel(normalizeSnapshot(running!))
    expect(runningView.timeline.find(({ id }) => id === 'job-success')).toMatchObject({
      state: 'running', callId: 'call-success-1', toolKind: 'custom',
      toolName: 'run', delivered: false,
    })
    expect(runningView.inbox).toContainEqual(expect.objectContaining({
      id: 'progress-success', state: 'PROGRESS', message: 'custom run evaluating',
    }))
    const settledView = toViewModel(normalizeSnapshot(settled!))
    const reopenedView = toViewModel(normalizeSnapshot(reopened!))
    const retained = settledView.timeline.find(({ id }) => id === 'job-success')
    expect(retained).toMatchObject({
      id: 'job-success', state: 'settled', requestId: 'request-success',
      callId: 'call-success-1', toolKind: 'custom', toolName: 'run',
      delivered: true, output: { status: 'ok', stdout: 'preview ready' },
    })
    expect(reopenedView.timeline.find(({ id }) => id === 'job-success')).toEqual(retained)
    expect(reopenedView.inbox).toEqual(settledView.inbox)

    const app = render(createElement(App, { data: runningView }))
    fireEvent.click(screen.getByRole('link', { name: /timeline/i }))
    let timeline = screen.getByRole('table', { name: 'Conversation activity timeline' })
    expect(timeline.textContent).toContain('call call-success-1')
    expect(timeline.textContent).toContain('tool kind custom')
    expect(timeline.textContent).toContain('delivered false')
    fireEvent.click(screen.getByRole('link', { name: /inbox/i }))
    expect(screen.getByRole('list', { name: 'Inbox messages' }).textContent).toContain('custom run evaluating')

    app.rerender(createElement(App, { data: settledView }))
    fireEvent.click(screen.getByRole('link', { name: /timeline/i }))
    timeline = screen.getByRole('table', { name: 'Conversation activity timeline' })
    expect(timeline.textContent).toContain('settled')
    expect(timeline.textContent).toContain('call call-success-1')
    expect(timeline.textContent).toContain('tool kind custom')
    expect(timeline.textContent).toContain('run')
    expect(timeline.textContent).toContain('delivered true')
    expect(timeline.textContent).toContain('preview ready')

    app.rerender(createElement(App, { data: reopenedView }))
    timeline = screen.getByRole('table', { name: 'Conversation activity timeline' })
    expect(timeline.textContent).toContain('call call-success-1')
    expect(timeline.textContent).toContain('tool kind custom')
    expect(timeline.textContent).toContain('delivered true')
    expect(timeline.textContent).toContain('preview ready')
  })

  it('reopens a cancelled custom job with its retained terminal output', () => {
    const [running, cancelled, reopened, afterLateCompletion] = customJobJourney
    expect(running).toBeDefined()
    expect(cancelled).toBeDefined()
    expect(reopened).toBeDefined()
    expect(afterLateCompletion).toBeDefined()

    const runningState = normalizeSnapshot(running!)
    const runningJob = toViewModel(runningState).timeline.find(({ id }) => id === 'job-custom')
    expect(runningJob).toMatchObject({
      id: 'job-custom',
      state: 'running',
      callId: 'call-custom-1',
      toolKind: 'custom',
      toolName: 'custom:render_preview',
    })
    // The fixture supplies the discriminator as a server-projected field;
    // this only tests projection, not the server's Store-backed derivation.
    expect(runningState.jobs.get('job-custom')?.toolKind).toBe('custom')
    expect(toViewModel(runningState).inbox).toMatchObject([{
      state: 'PROGRESS',
      message: 'custom run started',
    }])

    const cancelledState = normalizeSnapshot(cancelled!)
    const cancelledView = toViewModel(cancelledState)
    expect(cancelledView.timeline.find(({ id }) => id === 'request-custom')).toMatchObject({
      outcome: 'cancelled',
    })
    const retained = cancelledView.timeline.find(({ id }) => id === 'job-custom')
    expect(retained).toMatchObject({
      id: 'job-custom',
      state: 'cancelled',
      callId: 'call-custom-1',
      toolKind: 'custom',
      toolName: 'custom:render_preview',
      delivered: true,
      output: { status: 'cancelled', reason: 'operator requested cancellation' },
    })
    expect(normalizeSnapshot(reopened!).jobs.get('job-custom')?.toolKind).toBe('custom')

    const reopenedView = toViewModel(normalizeSnapshot(reopened!))
    expect(reopenedView.timeline.find(({ id }) => id === 'job-custom')).toEqual(retained)
    expect(reopenedView.inbox).toMatchObject([{
      state: 'PROGRESS',
      message: 'custom run started',
    }])
    expect(afterLateCompletion!.seq).toBe((BigInt(reopened!.seq) + 1n).toString())
    const afterLateView = toViewModel(normalizeSnapshot(afterLateCompletion!))
    expect(afterLateView.timeline.find(({ id }) => id === 'job-custom')).toMatchObject({
      id: 'job-custom',
      state: 'cancelled',
      callId: 'call-custom-1',
      toolKind: 'custom',
      toolName: 'custom:render_preview',
      delivered: true,
      output: { status: 'cancelled', reason: 'operator requested cancellation' },
    })
    expect(afterLateView.inbox).toContainEqual(expect.objectContaining({
      id: 'late-completion',
      message: 'late provider success ignored; cancellation retained',
    }))

    // Exercise the actual App toolJobs/timeline presentation as the browser
    // receives each deterministic server snapshot (including after reopen).
    const app = render(createElement(App, { data: toViewModel(runningState) }))
    fireEvent.click(screen.getByRole('link', { name: /timeline/i }))
    expect(screen.getByRole('table', { name: 'Conversation activity timeline' }).textContent)
      .toContain('custom:render_preview')
    expect(screen.getByRole('table', { name: 'Conversation activity timeline' }).textContent)
      .toContain('tool kind custom')

    app.rerender(createElement(App, { data: cancelledView }))
    let timeline = screen.getByRole('table', { name: 'Conversation activity timeline' })
    expect(timeline.textContent).toContain('cancelled')
    expect(timeline.textContent).toContain('operator requested cancellation')

    app.rerender(createElement(App, { data: reopenedView }))
    timeline = screen.getByRole('table', { name: 'Conversation activity timeline' })
    expect(timeline.textContent).toContain('custom:render_preview')
    expect(timeline.textContent).toContain('operator requested cancellation')

    app.rerender(createElement(App, { data: afterLateView }))
    timeline = screen.getByRole('table', { name: 'Conversation activity timeline' })
    expect(timeline.textContent).toContain('call call-custom-1')
    expect(timeline.textContent).toContain('tool kind custom')
    expect(timeline.textContent).toContain('delivered true')
    expect(timeline.textContent).toContain('cancelled')
    expect(timeline.textContent).toContain('operator requested cancellation')
    expect(timeline.textContent).not.toContain('late provider success')
  })

  it('renders additive outcomes, ordered progress/messages, and distinct child identities', () => {
    const snapshot: Snapshot = {
      seq: '4',
      conversations: [
        { parentId: null, forkSourceRequestId: null, id: 'root', path: '/root', state: 'idle' },
        { parentId: 'root', forkSourceRequestId: null, id: 'child', path: '/root/helper', state: 'idle' },
      ],
      requests: [{ parentId: null, createdAtMs: null, endedAtMs: null, failure: null,
        id: 'request-1', conversationId: 'root', state: 'completed',
        commandId: 'cmd-7', command: 'echo hello', outcome: 'completed', detail: 'echoed hello',
      }],
      jobs: [{ startedAtMs: null, endedAtMs: null,
        id: 'job-9', conversationId: 'root', state: 'settled',
        requestId: 'request-1', callId: 'call-3', toolName: 'slow_tool',
        delivered: false, output: 'result pending delivery',
      }],
      envelopes: [
        { id: 'reply', recipient: '/root', sender: '/root/helper', type: 'FINAL_ANSWER', payload: 'done', ordinal: '3' },
        { id: 'progress', recipient: '/root', sender: '/harness', type: 'PROGRESS', payload: 'working', ordinal: '1' },
        { id: 'message', recipient: '/root/helper', sender: '/root', type: 'MESSAGE', payload: 'do it', ordinal: '2' },
      ],
    }
    const view = toViewModel(normalizeSnapshot(snapshot))
    expect(view.timeline[0]).toMatchObject({
      commandId: 'cmd-7', command: 'echo hello', outcome: 'completed', detail: 'echoed hello',
    })
    expect(view.timeline.find(({ id }) => id === 'job-9')).toMatchObject({
      id: 'job-9', state: 'settled', requestId: 'request-1', callId: 'call-3',
      toolName: 'slow_tool', delivered: false, output: 'result pending delivery',
    })
    expect(view.inbox.map(({ id }) => id)).toEqual(['progress', 'message', 'reply'])
    expect(view.inbox[0]).toMatchObject({ state: 'PROGRESS', sender: '/harness', recipient: '/root' })
    expect(view.inbox[1]).toMatchObject({ state: 'MESSAGE', sender: '/root', recipient: '/root/helper' })
    expect(view.nodes.find(({ id }) => id === 'child')).toMatchObject({ parentId: 'root', name: '/root/helper' })
  })

  it('does not synthesize absent optional outcomes or envelope ordering', () => {
    const old: Snapshot = {
      seq: '1',
      conversations: [{ parentId: null, forkSourceRequestId: null, id: 'root', path: '/root', state: 'idle' }],
      requests: [{ parentId: null, createdAtMs: null, endedAtMs: null, failure: null, id: 'legacy', conversationId: 'root', state: 'completed' }],
      jobs: [],
      envelopes: [{ id: 'legacy-message', recipient: '/root', sender: '/operator', type: 'MESSAGE', payload: 'hi' }],
    }
    const view = toViewModel(normalizeSnapshot(old))
    expect(view.timeline[0]?.outcome).toBeUndefined()
    expect(view.timeline[0]?.commandId).toBeUndefined()
    expect(view.inbox[0]?.ordinal).toBeUndefined()
    expect(view.inbox[0]?.state).toBe('MESSAGE')
  })
})

describe('indexed retained projection', () => {
  it('preserves authoritative null/unrelated parents, source links, exact identity and null output', () => {
    const parent = { run: 'Opaque/Run', actor: 'same-label', incarnation: 'OLD' }
    const state = normalizeSnapshot({ seq: '1', actors: [{
      identity: { ...parent, incarnation: 'NEW' }, parent, kind: 'workflow', lifecycle: 'waiting', modelConversation: null,
    }], conversations: [
      { parentId: null, forkSourceRequestId: null, id: 'root', path: '/root', state: 'idle' },
      { id: 'child', path: '/unrelated', parentId: 'root', state: 'idle', forkSourceRequestId: 'source' },
      { forkSourceRequestId: null, id: 'other-root', path: '/root/other', parentId: null, state: 'idle' },
    ], requests: [{ failure: null, id: 'same', conversationId: 'root', parentId: 'source', state: 'completed', createdAtMs: '3000', endedAtMs: '4000' }],
    jobs: [{ id: 'same', conversationId: 'root', requestId: 'same', state: 'settled', startedAtMs: '1000', endedAtMs: '3000', output: null }], envelopes: [] })
    const data = toViewModel(state)
    expect(data.actors?.[0]?.parentIdentity).toBe(parent)
    expect(data.nodes.find((node) => node.id === 'child')).toMatchObject({ parentId: 'root', forkSourceRequestId: 'source' })
    expect(data.nodes.find((node) => node.id === 'other-root')?.parentId).toBeNull()
    expect(data.timeline.map((row) => row.kind)).toEqual(['job', 'request'])
    expect(data.timeline[0]).toMatchObject({ id: 'same', output: null, startedAtMs: 1000, endedAtMs: 3000, duration: '2s' })
    expect(data.timeline[1]).toMatchObject({ parentId: 'source', startedAtMs: 3000, duration: '1s' })
    expect(data.timeline[0]?.key).not.toBe(data.timeline[1]?.key)
  })

  it('refreshes history only for the exact durable request and its terminal/delivered jobs', () => {
    const project = createViewProjector()
    let state = normalizeSnapshot({ seq: '1', conversations: [{ parentId: null, forkSourceRequestId: null, id: 'c', path: '/c', state: 'idle', version: '1' }],
      requests: [{ parentId: null, createdAtMs: null, endedAtMs: null, failure: null, id: 'r', conversationId: 'c', state: 'running', version: '1' }],
      jobs: [{ startedAtMs: null, endedAtMs: null, id: 'j', conversationId: 'c', requestId: 'r', state: 'running', version: '1', delivered: false }], envelopes: [] })
    const key = () => project(state).timeline.find((row) => row.kind === 'request')!.historyRefreshKey
    const first = key()
    state = { ...state, seq: '9', conversations: new Map([['c', { ...state.conversations.get('c')!, version: '100' }]]) }
    expect(key()).toBe(first)
    state = { ...state, jobs: new Map([['j', { ...state.jobs.get('j')!, version: '8', output: 'progress' }]]) }
    expect(key()).toBe(first)
    state = { ...state, jobs: new Map([...state.jobs, ['other', { startedAtMs: null, endedAtMs: null, id: 'other', conversationId: 'c', requestId: 'elsewhere', state: 'settled', delivered: true }]]) }
    expect(key()).toBe(first)
    state = { ...state, jobs: new Map([...state.jobs, ['j', { ...state.jobs.get('j')!, state: 'settled' }]]) }
    const terminal = key()
    expect(terminal).not.toBe(first)
    state = { ...state, jobs: new Map([...state.jobs, ['j', { ...state.jobs.get('j')!, delivered: true }]]) }
    const delivered = key()
    expect(delivered).not.toBe(terminal)
    state = { ...state, jobs: new Map([...state.jobs, ['j', { ...state.jobs.get('j')!, version: '99', output: 'metadata update' }]]) }
    expect(key()).toBe(delivered)
    state = { ...state, requests: new Map([['r', { ...state.requests.get('r')!, version: '2' }]]) }
    expect(key()).not.toBe(delivered)
  })

  it('reuses unrelated tables and unchanged rows when relevant Maps change', () => {
    const project = createViewProjector()
    const state = normalizeSnapshot({ seq: '1', conversations: [
      { parentId: null, forkSourceRequestId: null, id: 'a', path: '/a', state: 'idle' }, { parentId: null, forkSourceRequestId: null, id: 'b', path: '/b', state: 'idle' }],
      requests: [{ parentId: null, createdAtMs: null, endedAtMs: null, failure: null, id: 'r', conversationId: 'a', state: 'running' }], jobs: [], envelopes: [] })
    const first = project(state)
    expect(project({ ...state, seq: '2' })).toBe(first)
    const message = project({ ...state, envelopes: new Map([['e', { id: 'e', sender: '/a', recipient: '/b', type: 'MESSAGE', payload: 'hi' }]]) })
    expect(message.nodes).toBe(first.nodes)
    expect(message.timeline).toBe(first.timeline)
    expect(message.actors).toBe(first.actors)
    const updated = project({ ...state, requests: new Map([['r', { ...state.requests.get('r')!, detail: 'changed' }]]) })
    expect(updated.nodes.find((row) => row.id === 'b')).toBe(first.nodes.find((row) => row.id === 'b'))
    expect(updated.nodes.find((row) => row.id === 'a')).not.toBe(first.nodes.find((row) => row.id === 'a'))
  })

  it.each([200, 2000])('indexes %i worker requests without a request scan per conversation', (count) => {
    let scans = 0
    class CountedRequests extends Map<string, RequestRecord> {
      override values() { scans++; return super.values() }
    }
    const state = normalizeSnapshot({ seq: '1', conversations: Array.from({ length: count }, (_, n) => ({ forkSourceRequestId: null,
      id: `c${n}`, path: `/root/c${n}`, parentId: n === 0 ? null : 'c0', state: 'idle' as const,
    })), requests: [], jobs: [], envelopes: [] })
    const requests = new CountedRequests(Array.from({ length: count }, (_, n) => [`r${n}`, { parentId: null, createdAtMs: null, endedAtMs: null, failure: null,
      id: `r${n}`, conversationId: `c${n}`, state: 'running' as const,
    }]))
    const project = createViewProjector()
    const view = project({ ...state, requests })
    expect(view.nodes).toHaveLength(count)
    expect(view.nodes.every((row) => row.detail === 'request running')).toBe(true)
    expect(scans).toBe(2)
    project({ ...state, requests, seq: '100' })
    expect(scans).toBe(2)
  })
})
