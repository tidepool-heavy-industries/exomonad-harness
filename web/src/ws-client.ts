import {
  applyStateEvent,
  normalizeSnapshot,
  type NormalizedState,
  type SequencedEvent,
  type Snapshot,
} from './protocol'

/**
 * Server integration boundary: accepts stable JSON frames
 * {type:"snapshot", snapshot} and {type:"event", event:{seq,event}}.
 * A gap is never applied; it requests a fresh snapshot on the same socket.
 */
export function connectHarness(
  socket: WebSocket,
  receive: (state: NormalizedState) => void,
  error: (message: string) => void,
): (command: string) => void {
  let current: NormalizedState | undefined
  socket.addEventListener('message', (message: MessageEvent<string>) => {
    try {
      const frame = JSON.parse(message.data) as
        | { type: 'snapshot'; snapshot: Snapshot }
        | { type: 'event'; event: SequencedEvent }
        | { type: 'error'; reason: string }
      if (frame.type === 'snapshot') {
        current = normalizeSnapshot(frame.snapshot)
        receive(current)
      } else if (frame.type === 'event') {
        if (!current) {
          socket.send(JSON.stringify({ type: 'snapshot.request' }))
          return
        }
        const result = applyStateEvent(current, frame.event)
        if (result.kind === 'resync') {
          socket.send(JSON.stringify({ type: 'snapshot.request' }))
          return
        }
        current = result.state
        receive(current)
      } else error(frame.reason)
    } catch {
      error('Invalid JSON event from server.')
    }
  })
  socket.addEventListener('error', () => error('WebSocket connection failed.'))
  return (command) => {
    if (socket.readyState !== WebSocket.OPEN) {
      error('Command channel is disconnected; retry after reconnecting.')
      return
    }
    socket.send(JSON.stringify({ type: 'command', command }))
  }
}
