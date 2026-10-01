import {
  applyStateEvent, isCommandRefusal, isHostCommand, isObject, isOperationId, isSequencedEvent,
  isSnapshot, normalizeSnapshot, type HostCommandRefusal, type HostCommandSubmission, type NormalizedState,
} from './protocol'
import type { TransportPhase } from './client-contract'

export type SendObservation = 'sent' | 'not_sent' | 'unknown'
export interface HarnessConnection {
  send(command: string | HostCommandSubmission): SendObservation
  dispose(): void
}

/** One socket owns its fresh snapshot gate and coalesced gap request. */
export function connectHarness(socket: WebSocket, callbacks: {
  receive: (state: NormalizedState) => void
  phase: (phase: TransportPhase) => void
  error: (message: string) => void
  accepted: (commandId: string) => void
  refused: (refusal: HostCommandRefusal) => void
  disconnected: () => void
}): HarnessConnection {
  let current: NormalizedState | undefined
  let phase: TransportPhase = 'connecting'
  let requestedSnapshot = false
  let disposed = false
  let lost = false
  let resyncTimer: ReturnType<typeof setTimeout> | undefined
  const listeners: [string, EventListener][] = []
  const transition = (next: TransportPhase) => { phase = next; callbacks.phase(next) }
  const requestSnapshot = () => {
    if (disposed || requestedSnapshot) return
    transition(current ? 'resync' : 'awaiting_snapshot')
    if (socket.readyState !== WebSocket.OPEN) { disconnect(); return }
    try {
      socket.send(JSON.stringify({ type: 'snapshot.request' }))
      requestedSnapshot = true
      resyncTimer = setTimeout(() => {
        if (disposed || lost || !requestedSnapshot) return
        callbacks.error('The authoritative resync snapshot timed out after 10 seconds.')
        disconnect()
      }, 10_000)
    }
    catch { disconnect() }
  }
  const disconnect = () => {
    if (disposed || lost) return
    lost = true
    if (resyncTimer !== undefined) clearTimeout(resyncTimer)
    transition('disconnected')
    callbacks.disconnected()
  }
  const add = (type: string, listener: EventListener) => {
    listeners.push([type, listener]); socket.addEventListener(type, listener)
  }
  add('open', () => { if (!disposed) transition('awaiting_snapshot') })
  add('message', ((message: MessageEvent<string>) => {
    if (disposed || lost) return
    try {
      const frame: unknown = JSON.parse(message.data)
      if (!isObject(frame)) throw new Error('Invalid server frame.')
      switch (frame.type) {
        case 'snapshot':
          if (!isSnapshot(frame.snapshot)) throw new Error('Invalid authoritative snapshot from server.')
          if (current && frame.snapshot.hostRun === current.hostRun && frame.snapshot.seq < current.seq) break
          current = normalizeSnapshot(frame.snapshot)
          requestedSnapshot = false
          if (resyncTimer !== undefined) { clearTimeout(resyncTimer); resyncTimer = undefined }
          transition('ready')
          callbacks.receive(current)
          break
        case 'event': {
          if (!isSequencedEvent(frame.event)) throw new Error('Invalid projected event from server.')
          if (!current || phase !== 'ready') { requestSnapshot(); break }
          const result = applyStateEvent(current, frame.event)
          if (result.kind === 'resync') { requestSnapshot(); break }
          current = result.state
          callbacks.receive(current)
          break
        }
        case 'command.accepted':
          if (typeof frame.command_id !== 'string' || !frame.command_id.length) throw new Error('Invalid command acceptance frame from server.')
          callbacks.accepted(frame.command_id)
          break
        case 'command.refused':
          if (!isCommandRefusal(frame)) throw new Error('Invalid command refusal frame from server.')
          callbacks.refused(frame)
          break
        default: throw new Error('Unsupported server frame.')
      }
    } catch (error) {
      callbacks.error(error instanceof Error ? error.message : 'Invalid JSON event from server.')
      requestSnapshot()
    }
  }) as EventListener)
  add('error', () => {
    if (disposed) return
    callbacks.error('WebSocket connection failed.')
    disconnect()
  })
  add('close', disconnect)
  callbacks.phase(phase)

  return {
    send(command) {
      if (disposed || lost || phase !== 'ready' || socket.readyState !== WebSocket.OPEN) return 'not_sent'
      if (typeof command !== 'string' && (!isOperationId(command.operation_id) || !isHostCommand(command.command)
        || current?.hostRun !== command.command.target.run)) return 'not_sent'
      if (typeof command === 'string' && current?.hostRun !== undefined) return 'not_sent'
      try {
        socket.send(JSON.stringify(typeof command === 'string' ? { type: 'command', command }
          : { type: 'host_command', operation_id: command.operation_id, command: command.command }))
        return 'sent'
      } catch {
        // A throwing browser send does not prove whether bytes reached the transport.
        disconnect()
        return 'unknown'
      }
    },
    dispose() {
      disposed = true
      if (resyncTimer !== undefined) clearTimeout(resyncTimer)
      for (const [type, listener] of listeners) socket.removeEventListener?.(type, listener)
    },
  }
}
