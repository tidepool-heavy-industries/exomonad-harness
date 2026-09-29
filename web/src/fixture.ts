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

/**
 * Deterministic custom-tool cancellation/reopen journey. The final snapshot is
 * what a newly opened browser receives from the server; it must retain the
 * terminal record rather than re-submit or reconstruct the tool call.
 */
export const customJobJourney: readonly Snapshot[] = [
  {
    seq: 20,
    conversations: [{ id: 'root', path: '/root', state: 'requesting' }],
    requests: [{ id: 'request-custom', conversationId: 'root', state: 'running' }],
    jobs: [{
      id: 'job-custom',
      conversationId: 'root',
      state: 'running',
      toolKind: 'custom',
      requestId: 'request-custom',
      callId: 'call-custom-1',
      toolName: 'custom:render_preview',
    }],
    envelopes: [{
      id: 'progress-custom',
      recipient: '/root',
      sender: '/root',
      type: 'PROGRESS',
      payload: 'custom run started',
      ordinal: 1,
    }],
  },
  {
    seq: 21,
    conversations: [{ id: 'root', path: '/root', state: 'cancelled' }],
    requests: [{
      id: 'request-custom',
      conversationId: 'root',
      state: 'completed',
      outcome: 'cancelled',
    }],
    jobs: [{
      id: 'job-custom',
      conversationId: 'root',
      state: 'cancelled',
      toolKind: 'custom',
      requestId: 'request-custom',
      callId: 'call-custom-1',
      toolName: 'custom:render_preview',
      delivered: true,
      output: { status: 'cancelled', reason: 'operator requested cancellation' },
    }],
    envelopes: [{
      id: 'progress-custom',
      recipient: '/root',
      sender: '/root',
      type: 'PROGRESS',
      payload: 'custom run started',
      ordinal: 1,
    }],
  },
  {
    // Same durable IDs and terminal result, as returned after reopening.
    seq: 21,
    conversations: [{ id: 'root', path: '/root', state: 'cancelled' }],
    requests: [{
      id: 'request-custom',
      conversationId: 'root',
      state: 'completed',
      outcome: 'cancelled',
    }],
    jobs: [{
      id: 'job-custom',
      conversationId: 'root',
      state: 'cancelled',
      toolKind: 'custom',
      requestId: 'request-custom',
      callId: 'call-custom-1',
      toolName: 'custom:render_preview',
      delivered: true,
      output: { status: 'cancelled', reason: 'operator requested cancellation' },
    }],
    envelopes: [{
      id: 'progress-custom',
      recipient: '/root',
      sender: '/root',
      type: 'PROGRESS',
      payload: 'custom run started',
      ordinal: 1,
    }],
  },
  {
    // Expected authoritative server snapshot after a late provider completion:
    // the late success is observed but cannot replace the terminal cancellation.
    // This is a fixture contract, not proof that Engine/server implements it.
    seq: 22,
    conversations: [{ id: 'root', path: '/root', state: 'cancelled' }],
    requests: [{
      id: 'request-custom',
      conversationId: 'root',
      state: 'completed',
      outcome: 'cancelled',
    }],
    jobs: [{
      id: 'job-custom',
      conversationId: 'root',
      state: 'cancelled',
      toolKind: 'custom',
      requestId: 'request-custom',
      callId: 'call-custom-1',
      toolName: 'custom:render_preview',
      delivered: true,
      output: { status: 'cancelled', reason: 'operator requested cancellation' },
    }],
    envelopes: [{
      id: 'progress-custom',
      recipient: '/root',
      sender: '/root',
      type: 'PROGRESS',
      payload: 'custom run started',
      ordinal: 1,
    }, {
      id: 'late-completion',
      recipient: '/root',
      sender: '/harness',
      type: 'MESSAGE',
      payload: 'late provider success ignored; cancellation retained',
      ordinal: 2,
    }],
  },
]

/** Expected server snapshots for a successful custom job and a read-only reopen. */
export const customSuccessJourney: readonly Snapshot[] = [
  {
    seq: 30,
    conversations: [{ id: 'root', path: '/root', state: 'requesting' }],
    requests: [{ id: 'request-success', conversationId: 'root', state: 'running' }],
    jobs: [{
      id: 'job-success',
      conversationId: 'root',
      state: 'running',
      toolKind: 'custom',
      requestId: 'request-success',
      callId: 'call-success-1',
      toolName: 'run',
      delivered: false,
    }],
    envelopes: [{
      id: 'progress-success',
      recipient: '/root',
      sender: '/root',
      type: 'PROGRESS',
      payload: 'custom run evaluating',
      ordinal: 1,
    }],
  },
  {
    seq: 31,
    conversations: [{ id: 'root', path: '/root', state: 'idle' }],
    requests: [{ id: 'request-success', conversationId: 'root', state: 'completed', outcome: 'completed' }],
    jobs: [{
      id: 'job-success',
      conversationId: 'root',
      state: 'settled',
      toolKind: 'custom',
      requestId: 'request-success',
      callId: 'call-success-1',
      toolName: 'run',
      delivered: true,
      output: { status: 'ok', stdout: 'preview ready' },
    }],
    envelopes: [{
      id: 'progress-success',
      recipient: '/root',
      sender: '/root',
      type: 'PROGRESS',
      payload: 'custom run evaluating',
      ordinal: 1,
    }],
  },
  {
    // Reopening projects the same durable identity, progress and settled output.
    seq: 31,
    conversations: [{ id: 'root', path: '/root', state: 'idle' }],
    requests: [{ id: 'request-success', conversationId: 'root', state: 'completed', outcome: 'completed' }],
    jobs: [{
      id: 'job-success',
      conversationId: 'root',
      state: 'settled',
      toolKind: 'custom',
      requestId: 'request-success',
      callId: 'call-success-1',
      toolName: 'run',
      delivered: true,
      output: { status: 'ok', stdout: 'preview ready' },
    }],
    envelopes: [{
      id: 'progress-success',
      recipient: '/root',
      sender: '/root',
      type: 'PROGRESS',
      payload: 'custom run evaluating',
      ordinal: 1,
    }],
  },
]
