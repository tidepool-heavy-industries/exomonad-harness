import { afterEach, expect, it, vi } from 'vitest'
import { createCommandLookups } from './command-lookups'
import { applyCommandStatus, retainCommand } from './pending-commands'
import type { EmbeddedCommandRecord } from './protocol'

afterEach(() => vi.useRealTimers())
function records(count: number, run = 'run') {
  return Array.from({ length: count }, (_, index) => retainCommand([], run, {
    operation_id: `00000000-0000-4000-8000-${String(index).padStart(12, '0')}`,
    command: { action: 'retire', target: { run, actor: '/root', incarnation: 'one' } },
  })[0]!)
}

it('admits at most two reads, never queries old runs, and aborts obsolete generations', async () => {
  const retained = [...records(4), ...records(1, 'old')]
  const finishes: ((status: EmbeddedCommandRecord | undefined) => void)[] = []
  const read = vi.fn((_id: string, _signal: AbortSignal) => new Promise<EmbeddedCommandRecord | undefined>((resolve) => finishes.push(resolve)))
  const result = vi.fn()
  const scheduler = createCommandLookups({ current: () => retained, read, result, error: vi.fn() })
  scheduler.reconcile(retained, 'run')
  scheduler.reconcile(retained, 'run')
  expect(read).toHaveBeenCalledTimes(2)
  finishes[0]!(undefined)
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(3))
  scheduler.dispose()
  expect(read.mock.calls[1]?.[1].aborted).toBe(true)
  finishes[1]!(undefined)
  await Promise.resolve()
  expect(result).toHaveBeenCalledTimes(1)
})

it('backs off unavailable observations at 1/2/5/10/30 seconds without replaying commands', async () => {
  vi.useFakeTimers()
  const retained = records(1)
  const read = vi.fn().mockResolvedValue(undefined)
  const scheduler = createCommandLookups({ current: () => retained, read, result: vi.fn(), error: vi.fn() })
  scheduler.reconcile(retained, 'run')
  await vi.advanceTimersByTimeAsync(0)
  for (const [index, delay] of [1000, 2000, 5000, 10000, 30000, 30000].entries()) {
    await vi.advanceTimersByTimeAsync(delay - 1)
    expect(read).toHaveBeenCalledTimes(index + 1)
    await vi.advanceTimersByTimeAsync(1)
    expect(read).toHaveBeenCalledTimes(index + 2)
  }
  scheduler.dispose()
})

it('stops polling authoritative unconfirmed while retaining it, and allows one explicit reconciliation', async () => {
  vi.useFakeTimers()
  let retained = records(1)
  const first = retained[0]!
  const status = { operationId: first.submission.operation_id, command: first.submission.command,
    state: 'unconfirmed' as const, envelopeId: null, receipt: null }
  const read = vi.fn().mockResolvedValue(status)
  const scheduler = createCommandLookups({ current: () => retained, read,
    result: (_record, observed) => { retained = applyCommandStatus(retained, 'run', observed!) }, error: vi.fn() })
  scheduler.reconcile(retained, 'run')
  await vi.advanceTimersByTimeAsync(120000)
  expect(read).toHaveBeenCalledTimes(1)
  expect(retained[0]?.state).toBe('unconfirmed')
  scheduler.reconcile(retained, 'run', true)
  await vi.advanceTimersByTimeAsync(120000)
  expect(read).toHaveBeenCalledTimes(2)
  scheduler.dispose()
})

it('cancels old-run lookups before reading a new current run and rejects stale completions', async () => {
  let retained = [...records(1, 'old'), ...records(1, 'new')]
  const finishes: ((status: EmbeddedCommandRecord | undefined) => void)[] = []
  const read = vi.fn((_id: string, _signal: AbortSignal) => new Promise<EmbeddedCommandRecord | undefined>((resolve) => finishes.push(resolve)))
  const result = vi.fn()
  const scheduler = createCommandLookups({ current: () => retained, read, result, error: vi.fn() })
  scheduler.reconcile(retained, 'old')
  scheduler.reconcile(retained, 'new')
  expect(read.mock.calls[0]?.[1].aborted).toBe(true)
  expect(read).toHaveBeenCalledTimes(2)
  finishes[0]!(undefined)
  await Promise.resolve()
  expect(result).not.toHaveBeenCalled()
  scheduler.pause()
  expect(read.mock.calls[1]?.[1].aborted).toBe(true)
  retained = []
  scheduler.dispose()
})
