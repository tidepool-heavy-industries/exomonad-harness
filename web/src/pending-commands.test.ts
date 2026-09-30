import { beforeEach, describe, expect, it } from 'vitest'
import { allocateOperationId, applyCommandStatus, readPendingCommands, retainCommand, writePendingCommands } from './pending-commands'

const command = { action: 'input' as const, target: { run: 'run-1', actor: '/root', incarnation: 'inc-1' }, text: 'continue' }

describe('retained embedded browser operations', () => {
  beforeEach(() => sessionStorage.clear())

  it('uses a browser-compatible UUID and retains the exact operation for explicit retry', () => {
    const operationId = allocateOperationId()
    expect(operationId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
    const submission = { operation_id: operationId, command }
    const retained = retainCommand([], 'run-1', submission)
    writePendingCommands(retained)
    expect(readPendingCommands()).toEqual(retained)
    expect(readPendingCommands()[0]?.submission).toEqual(submission)
  })

  it('reconciles status without changing the original command or operation identity', () => {
    const submission = { operation_id: '11111111-1111-4111-8111-111111111111', command }
    const retained = retainCommand([], 'run-1', submission)
    const next = applyCommandStatus(retained, 'run-1', {
      operationId: submission.operation_id,
      command,
      state: 'input_admitted',
      envelopeId: 42,
      receipt: null,
    })
    expect(next).toEqual([{ ...retained[0]!, state: 'input_admitted' }])
    expect(next[0]?.submission).toEqual(submission)
  })
})
