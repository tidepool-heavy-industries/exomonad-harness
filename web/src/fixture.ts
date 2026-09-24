import type { Snapshot } from './protocol'

/** Replaceable stable-JSON example; deliberately contains no Rust-owned type. */
export const fixtureSnapshot: Snapshot = {
  seq: 12,
  conversations: [
    { id: 'root', path: '/root', state: 'requesting' },
    { id: 'worker', path: '/root/web_ui/web_ui', state: 'paused' },
  ],
  requests: [{ id: 'request-1', conversationId: 'root', state: 'running' }],
  jobs: [{ id: 'job-1', conversationId: 'worker', state: 'running' }],
  envelopes: [
    {
      id: 'envelope-1',
      recipient: '/root',
      sender: '/operator',
      type: 'MESSAGE',
      payload: 'Review the latest web candidate.',
    },
  ],
}
