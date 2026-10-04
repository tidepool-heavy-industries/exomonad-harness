import { describe, expect, it, vi } from 'vitest'
import { fixtureSnapshot } from './fixture'
import { connectHarness } from './ws-client'

function setup() {
  const listeners = new Map<string, EventListener>()
  const socket = {
    readyState: WebSocket.OPEN,
    addEventListener: (type: string, listener: EventListener) => listeners.set(type, listener),
    removeEventListener: (type: string) => listeners.delete(type),
    send: vi.fn(),
  } as unknown as WebSocket
  const callbacks = { receive: vi.fn(), phase: vi.fn(), error: vi.fn(), accepted: vi.fn(), refused: vi.fn(), disconnected: vi.fn() }
  const connection = connectHarness(socket, callbacks)
  const dispatch = (frame: unknown) => listeners.get('message')?.(new MessageEvent('message', { data: JSON.stringify(frame) }))
  return { socket, callbacks, connection, dispatch, listeners }
}

describe('WebSocket snapshot and operation boundary', () => {
  it('gates sending on fresh snapshots, coalesces gaps, and applies only contiguous events', () => {
    const { socket, callbacks, connection, dispatch } = setup()
    expect(connection.send('wait_agent')).toBe('not_sent')
    dispatch({ type: 'snapshot', snapshot: fixtureSnapshot })
    expect(connection.send('wait_agent')).toBe('sent')
    expect(socket.send).toHaveBeenLastCalledWith(JSON.stringify({ type: 'command', command: 'wait_agent' }))
    dispatch({ type: 'event', event: { seq: (BigInt(fixtureSnapshot.seq) + 1n).toString(),
      event: { kind: 'conversation.upsert', value: { parentId: null, forkSourceRequestId: null, id: 'new', path: '/root/new', state: 'idle' } } } })
    expect(callbacks.receive.mock.lastCall?.[0].conversations.has('new')).toBe(true)
    for (const seq of [(BigInt(fixtureSnapshot.seq) + 3n).toString(), (BigInt(fixtureSnapshot.seq) + 4n).toString()])
      dispatch({ type: 'event', event: { seq, event: { kind: 'entity.remove', value: { entity: 'conversation', id: 'new' } } } })
    expect(socket.send).toHaveBeenCalledTimes(2)
    expect(callbacks.phase).toHaveBeenLastCalledWith('resync')
    expect(connection.send('wait_agent')).toBe('not_sent')
    expect(callbacks.receive).toHaveBeenCalledTimes(2)
    dispatch({ type: 'snapshot', snapshot: fixtureSnapshot })
    expect(callbacks.phase).toHaveBeenLastCalledWith('resync')
    expect(callbacks.receive).toHaveBeenCalledTimes(2)
    dispatch({ type: 'snapshot', snapshot: { ...fixtureSnapshot, seq: (BigInt(fixtureSnapshot.seq) + 4n).toString() } })
    expect(connection.send('wait_agent')).toBe('sent')
  })

  it('preserves exact host submissions and distinguishes transport refusal from durable outcomes', () => {
    const { socket, callbacks, connection, dispatch } = setup()
    const submission = { operation_id: 'ABCDEF01-1111-4111-8111-111111111111', command: {
      action: 'interrupt' as const, target: { run: 'RUN', actor: '/Root', incarnation: 'Inc' },
      expected_round: 'ABCDEF02-1111-4111-8111-111111111111' } }
    dispatch({ type: 'snapshot', snapshot: { ...fixtureSnapshot, hostRun: 'RUN' } })
    expect(connection.send(submission)).toBe('sent')
    expect(socket.send).toHaveBeenLastCalledWith(JSON.stringify({ type: 'host_command', ...submission }))
    expect(connection.send({ ...submission, command: { ...submission.command, target: { ...submission.command.target, run: 'run' } } })).toBe('not_sent')
    expect(connection.send('demo')).toBe('not_sent')
    dispatch({ type: 'command.accepted', command_id: submission.operation_id.toLowerCase() })
    expect(callbacks.accepted).toHaveBeenCalledWith(submission.operation_id.toLowerCase())
    for (const code of ['invalid_command', 'conflict', 'unavailable', 'wrong_run']) {
      const refusal = { operation_id: submission.operation_id, code, reason: 'not admitted on channel' }
      dispatch({ type: 'command.refused', ...refusal })
      expect(callbacks.refused).toHaveBeenLastCalledWith({ type: 'command.refused', ...refusal })
    }
    expect(callbacks.error).not.toHaveBeenCalled()
  })

  it('rejects unknown receipts without fabricating terminal state or treating protocol failure as auth loss', () => {
    const { socket, callbacks, dispatch } = setup()
    dispatch({ type: 'snapshot', snapshot: fixtureSnapshot })
    dispatch({ type: 'event', event: { seq: (BigInt(fixtureSnapshot.seq) + 1n).toString(), event: { kind: 'command.receipt',
      value: { commandId: 'op', outcome: 'future_outcome' } } } })
    expect(callbacks.error).toHaveBeenCalledWith('Invalid server frame.')
    expect(callbacks.receive).toHaveBeenCalledTimes(1)
    expect(callbacks.disconnected).not.toHaveBeenCalled()
    expect(socket.send).toHaveBeenCalledExactlyOnceWith(JSON.stringify({ type: 'snapshot.request' }))
    dispatch({ type: 'command.refused', reason: 'untyped' })
    expect(callbacks.refused).not.toHaveBeenCalled()
    expect(socket.send).toHaveBeenCalledTimes(1)
  })

  it('ignores stale socket callbacks after disposal and reports a racing send honestly', () => {
    const { socket, callbacks, connection, dispatch, listeners } = setup()
    dispatch({ type: 'snapshot', snapshot: fixtureSnapshot })
    vi.mocked(socket.send).mockImplementationOnce(() => { throw new Error('closed while sending') })
    expect(connection.send('demo')).toBe('unknown')
    expect(callbacks.disconnected).toHaveBeenCalledTimes(1)
    listeners.get('close')?.(new Event('close'))
    expect(callbacks.disconnected).toHaveBeenCalledTimes(1)
    const stale = listeners.get('message')!
    connection.dispose()
    stale(new MessageEvent('message', { data: JSON.stringify({ type: 'snapshot', snapshot: fixtureSnapshot }) }))
    expect(callbacks.receive).toHaveBeenCalledTimes(1)
    expect(connection.send('demo')).toBe('not_sent')
  })
})
