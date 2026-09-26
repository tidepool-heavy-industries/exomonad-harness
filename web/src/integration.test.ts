import { describe, expect, it } from 'vitest'
import { normalizeSnapshot, type Snapshot } from './protocol'
import { toViewModel } from './integration'

describe('web outcome integration', () => {
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
      jobs: [],
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
