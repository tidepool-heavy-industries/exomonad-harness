import {
  applyStateEvent,
  normalizeSnapshot,
  type NormalizedState,
  type SequencedEvent,
  type Snapshot,
  type HostCommand,
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
  accepted: (commandId: string) => void = () => undefined,
): (command: string | HostCommand) => void {
  let current: NormalizedState | undefined
  socket.addEventListener('message', (message: MessageEvent<string>) => {
    try {
      const frame = JSON.parse(message.data) as
        | { type: 'snapshot'; snapshot: Snapshot }
        | { type: 'event'; event: SequencedEvent }
        | { type: 'command.accepted'; command_id: string }
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
      } else if (frame.type === 'command.accepted') {
        if (typeof frame.command_id === 'string' && frame.command_id.length > 0) accepted(frame.command_id)
        else error('Invalid command acceptance frame from server.')
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
    socket.send(JSON.stringify({ type: typeof command === 'string' ? 'command' : 'host_command', command }))
  }
}
