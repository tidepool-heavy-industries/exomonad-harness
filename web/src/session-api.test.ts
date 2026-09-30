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
