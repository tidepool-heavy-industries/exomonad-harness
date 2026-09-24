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
      .map((request) => `request ${request.state}`)
      .join(', '),
  }))
  const timeline: HarnessViewModel['timeline'] = [
    ...[...state.requests.values()].map((request) => ({
      id: request.id,
      nodeId: request.conversationId,
      label: 'Response request',
      kind: 'request' as const,
      state: request.state,
    })),
    ...[...state.jobs.values()].map((job) => ({
      id: job.id,
      nodeId: job.conversationId,
      label: 'Async job',
      kind: 'job' as const,
      state: job.state,
    })),
  ]
  const inbox = [...state.envelopes.values()].map((envelope) => ({
    id: envelope.id,
    sender: envelope.sender,
    message: envelope.payload,
    state: envelope.type === 'FINAL_ANSWER' ? 'answered' : 'unread',
  }))
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
