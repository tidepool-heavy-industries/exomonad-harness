import { afterEach, expect, it, vi } from 'vitest'
import { CommandStatusError, getCommandStatus } from './session-api'

afterEach(() => vi.unstubAllGlobals())
const operationId = '11111111-1111-4111-8111-111111111111'
const record = { operationId, command: { action: 'retire', target: { run: 'run', actor: '/root', incarnation: 'one' } }, state: 'queued', envelopeId: null, receipt: null }

it('preserves typed status authentication failure and treats only 404 as absent', async () => {
  const fetcher = vi.fn().mockResolvedValueOnce({ status: 401, ok: false }).mockResolvedValueOnce({ status: 404, ok: false })
  vi.stubGlobal('fetch', fetcher)
  await expect(getCommandStatus(operationId)).rejects.toBeInstanceOf(CommandStatusError)
  await expect(getCommandStatus(operationId)).resolves.toBeUndefined()
  expect(fetcher.mock.calls[0]?.[1]).toMatchObject({ credentials: 'same-origin', method: 'GET' })
})

it('rejects stale operation identity and malformed status payloads', async () => {
  const malformed = [
    { ...record, operationId: '22222222-2222-4222-8222-222222222222' },
    { ...record, state: 'not_a_state' },
    { ...record, command: { action: 'interrupt', target: record.command.target } },
    { ...record, envelopeId: '7' },
  ]
  const fetcher = vi.fn()
  vi.stubGlobal('fetch', fetcher)
  for (const value of malformed) {
    fetcher.mockResolvedValueOnce({ status: 200, ok: true, json: async () => value })
    await expect(getCommandStatus(operationId)).rejects.toThrow(/invalid record/)
  }
})

it('matches UUID case canonically while preserving the exact returned target and round', async () => {
  const operationId = 'ABCDEF01-1111-4111-8111-111111111111'
  const command = { action: 'interrupt', target: { run: 'RUN', actor: '/Root', incarnation: 'Inc' }, expected_round: 'ABCDEF02-1111-4111-8111-111111111111' }
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, status: 200,
    json: async () => ({ ...record, operationId: operationId.toLowerCase(), command }) }))
  expect((await getCommandStatus(operationId))?.command).toEqual(command)
})

it('rejects every malformed receipt and future outcome without inventing uncertainty', async () => {
  for (const receipt of [{ outcome: 'future' }, { commandId: operationId, outcome: 'admitted', envelopeId: 7 },
    { commandId: operationId, outcome: 'unconfirmed', reason: 'unknown' }]) {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, status: 200, json: async () => ({ ...record, receipt }) }))
    await expect(getCommandStatus(operationId)).rejects.toThrow(/invalid record/)
  }
})

it('times out and aborts HTTP reads after 10 seconds, and honors generation cancellation', async () => {
  vi.useFakeTimers()
  const fetcher = vi.fn().mockImplementation(() => new Promise(() => {}))
  vi.stubGlobal('fetch', fetcher)
  const timed = expect(getCommandStatus(operationId)).rejects.toThrow(/10 seconds/)
  await vi.advanceTimersByTimeAsync(10000)
  await timed
  expect(fetcher.mock.calls[0]?.[1].signal.aborted).toBe(true)
  const controller = new AbortController()
  const cancelled = expect(getCommandStatus(operationId, controller.signal)).rejects.toMatchObject({ name: 'AbortError' })
  controller.abort()
  await cancelled
  expect(fetcher.mock.calls[1]?.[1].signal.aborted).toBe(true)
  vi.useRealTimers()
})
