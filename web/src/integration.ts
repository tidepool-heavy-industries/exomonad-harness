import type { HarnessViewModel } from './App'
import type { NormalizedState } from './protocol'

/** Translate normalized wire state into the view-only contract. */
export function toViewModel(state: NormalizedState): HarnessViewModel {
  const nodes = [...state.conversations.values()].map((conversation) => ({
    id: conversation.id,
    parentId: parentPath(conversation.path, state),
    name: conversation.path,
    state: conversation.state,
    detail: [...state.requests.values()]
      .filter((request) => request.conversationId === conversation.id)
      .map((request) => [
        request.commandId ? `command ${request.commandId}` : undefined,
        request.command ? `"${request.command}"` : undefined,
        request.outcome ? `outcome ${request.outcome}` : undefined,
        request.detail,
      ].filter(Boolean).join(' · ') || `request ${request.state}`)
      .join(', '),
  }))
  const timeline: HarnessViewModel['timeline'] = [
    ...[...state.requests.values()].map((request) => ({
      id: request.id,
      nodeId: request.conversationId,
      label: request.command ?? 'Response request',
      kind: 'request' as const,
      state: request.state,
      commandId: request.commandId,
      command: request.command,
      outcome: request.outcome,
      detail: request.detail,
    })),
    ...[...state.jobs.values()].map((job) => ({
      id: job.id,
      nodeId: job.conversationId,
      label: 'Async job',
      kind: 'job' as const,
      state: job.state,
    })),
  ]
  const envelopeRows = [...state.envelopes.values()]
    .map((envelope, index) => ({
      id: envelope.id,
      sender: envelope.sender,
      recipient: envelope.recipient,
      message: envelope.payload,
      state: envelope.type,
      ordinal: envelope.ordinal,
      index,
    }))
  const hasCompleteOrder = envelopeRows.every((envelope) => envelope.ordinal !== undefined)
  const inbox = envelopeRows
    .sort((left, right) => hasCompleteOrder
      ? (left.ordinal ?? 0) - (right.ordinal ?? 0)
      : left.index - right.index)
    .map(({ index: _index, ...envelope }) => envelope)
  return { nodes, timeline, inbox }
}

function parentPath(path: string, state: NormalizedState): string | undefined {
  let parent = path.slice(0, path.lastIndexOf('/'))
  const byPath = new Map([...state.conversations.values()].map((item) => [item.path, item.id]))
  while (parent) {
    const found = byPath.get(parent)
    if (found) return found
    parent = parent.slice(0, parent.lastIndexOf('/'))
  }
  return undefined
}
