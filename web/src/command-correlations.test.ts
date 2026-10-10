import { describe, expect, it } from 'vitest'
import { CommandCorrelations } from './command-correlations'
import { readPendingCommands, retainCommand, writePendingCommands } from './pending-commands'

describe('transient command reply correlations', () => {
  it('settles only the matching accepted or refused payload and leaves retained retry evidence intact', () => {
    const correlations = new CommandCorrelations()
    const command = { action: 'input' as const, target: { run: 'run-1', actor: '/root', incarnation: 'inc-1' }, text: 'continue' }
    const accepted = { operation_id: '11111111-1111-4111-8111-111111111111', command }
    const refused = { operation_id: '22222222-2222-4222-8222-222222222222', command: { ...command, text: 'retry later' } }
    const neverSent = { operation_id: '33333333-3333-4333-8333-333333333333', command: { ...command, text: 'not sent' } }
    const ledger = retainCommand([], 'run-1', accepted)
    writePendingCommands(ledger)

    correlations.sent(accepted, 'sent')
    correlations.sent(refused, 'unknown')
    correlations.sent(neverSent, 'not_sent')

    expect(correlations.accepted('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa')).toBeUndefined()
    expect(correlations.refused(refused.operation_id)).toEqual(refused)
    expect(correlations.refused(refused.operation_id)).toBeUndefined()
    expect(correlations.accepted(accepted.operation_id.toUpperCase())).toEqual(accepted)
    expect(correlations.accepted(accepted.operation_id)).toBeUndefined()
    expect(correlations.accepted(neverSent.operation_id)).toBeUndefined()
    expect(readPendingCommands()).toEqual(ledger)
  })
})
