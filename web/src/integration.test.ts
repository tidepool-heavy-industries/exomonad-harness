import { describe, expect, it } from 'vitest'
import { createElement } from 'react'
import { fireEvent, render, screen } from '@testing-library/react'
import { normalizeSnapshot, type Snapshot } from './protocol'
import { toViewModel } from './integration'
import { customJobJourney, customSuccessJourney } from './fixture'
import App from './App'

describe('web outcome integration', () => {
  it('shows Haskell-only host actors apart from model conversations', () => {
    const snapshot: Snapshot = {
      seq: 1,
      actors: [
        { identity: { run: 'run-1', actor: 'root', incarnation: 'first' }, parent: null,
          kind: 'model', lifecycle: 'running', modelConversation: 'root-conversation' },
        { identity: { run: 'run-1', actor: 'reviewer', incarnation: 'second' },
          parent: { run: 'run-1', actor: 'root', incarnation: 'first' },
          kind: 'workflow', lifecycle: 'waiting', modelConversation: null },
      ],
      conversations: [{ id: 'root-conversation', path: '/root', state: 'requesting' }],
      requests: [], jobs: [], envelopes: [],
    };
    const view = toViewModel(normalizeSnapshot(snapshot));
    expect(view.nodes).toHaveLength(1);
    expect(view.actors).toHaveLength(2);
    render(createElement(App, { data: view }));
    const actorTable = screen.getByRole('table', { name: 'Host actor lifecycles' });
    expect(actorTable.textContent).toContain('reviewer (workflow)');
    expect(actorTable.textContent).toContain('incarnation second');
    expect(actorTable.textContent).toContain('waiting');
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
    fireEvent.click(screen.getByRole('button', { name: /timeline/i }))
    let timeline = screen.getByRole('table', { name: 'Conversation activity timeline' })
    expect(timeline.textContent).toContain('call call-success-1')
    expect(timeline.textContent).toContain('tool kind custom')
    expect(timeline.textContent).toContain('delivered false')
    fireEvent.click(screen.getByRole('button', { name: /inbox/i }))
    expect(screen.getByRole('list', { name: 'Inbox messages' }).textContent).toContain('custom run evaluating')

    app.rerender(createElement(App, { data: settledView }))
    fireEvent.click(screen.getByRole('button', { name: /timeline/i }))
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
    expect(afterLateCompletion!.seq).toBe(reopened!.seq + 1)
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
    fireEvent.click(screen.getByRole('button', { name: /timeline/i }))
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
      seq: 4,
      conversations: [
        { id: 'root', path: '/root', state: 'idle' },
        { id: 'child', path: '/root/helper', state: 'idle' },
      ],
      requests: [{
        id: 'request-1', conversationId: 'root', state: 'completed',
        commandId: 'cmd-7', command: 'echo hello', outcome: 'completed', detail: 'echoed hello',
      }],
      jobs: [{
        id: 'job-9', conversationId: 'root', state: 'settled',
        requestId: 'request-1', callId: 'call-3', toolName: 'slow_tool',
        delivered: false, output: 'result pending delivery',
      }],
      envelopes: [
        { id: 'reply', recipient: '/root', sender: '/root/helper', type: 'FINAL_ANSWER', payload: 'done', ordinal: 3 },
        { id: 'progress', recipient: '/root', sender: '/harness', type: 'PROGRESS', payload: 'working', ordinal: 1 },
        { id: 'message', recipient: '/root/helper', sender: '/root', type: 'MESSAGE', payload: 'do it', ordinal: 2 },
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

  it('keeps old snapshots valid and does not synthesize outcome or envelope ordering', () => {
    const old: Snapshot = {
      seq: 1,
      conversations: [{ id: 'root', path: '/root', state: 'idle' }],
      requests: [{ id: 'legacy', conversationId: 'root', state: 'completed' }],
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
