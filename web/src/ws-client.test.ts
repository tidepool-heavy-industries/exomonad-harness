import { describe, expect, it, vi } from 'vitest'
import { fixtureSnapshot } from './fixture'
import { connectHarness } from './ws-client'

describe('WebSocket API boundary', () => {
  it('consumes snapshot/event frames and sends exact command and resync frames', () => {
    const listeners = new Map<string, (event: Event) => void>()
    const socket = {
      readyState: WebSocket.OPEN,
      addEventListener: (type: string, listener: (event: Event) => void) => listeners.set(type, listener),
      send: vi.fn(),
    } as unknown as WebSocket
    const receive = vi.fn()
    const error = vi.fn()
    const sendCommand = connectHarness(socket, receive, error)
    const dispatch = (frame: unknown) => listeners.get('message')?.(
      new MessageEvent('message', { data: JSON.stringify(frame) }),
    )

    dispatch({ type: 'snapshot', snapshot: fixtureSnapshot })
    expect(receive).toHaveBeenCalledTimes(1)
    dispatch({
      type: 'event',
      event: {
        seq: fixtureSnapshot.seq + 1,
        event: {
          kind: 'conversation.upsert',
          value: { id: 'new', path: '/root/new', state: 'idle' },
        },
      },
    })
    expect(receive).toHaveBeenCalledTimes(2)
    expect(receive.mock.lastCall?.[0].conversations.has('new')).toBe(true)

    dispatch({
      type: 'event',
      event: {
        seq: fixtureSnapshot.seq + 3,
        event: {
          kind: 'conversation.upsert',
          value: { id: 'skipped', path: '/root/skipped', state: 'idle' },
        },
      },
    })
    expect(socket.send).toHaveBeenLastCalledWith(JSON.stringify({ type: 'snapshot.request' }))
    expect(receive).toHaveBeenCalledTimes(2)

    sendCommand('wait_agent')
    expect(socket.send).toHaveBeenLastCalledWith(JSON.stringify({ type: 'command', command: 'wait_agent' }))
    expect(error).not.toHaveBeenCalled()
  })
})
