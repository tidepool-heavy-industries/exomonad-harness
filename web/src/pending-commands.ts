import type { EmbeddedCommandRecord, HostCommand, HostCommandSubmission } from './protocol'

const storageKey = 'harness.embeddedCommands.v1'

export interface BrowserCommandRecord {
  readonly hostRun: string
  readonly submission: HostCommandSubmission
  readonly state: EmbeddedCommandRecord['state']
}

export function allocateOperationId(): string {
  const bytes = new Uint8Array(16)
  if (!globalThis.crypto?.getRandomValues) throw new Error('Secure browser randomness is unavailable.')
  globalThis.crypto.getRandomValues(bytes)
  bytes[6] = (bytes[6]! & 0x0f) | 0x40
  bytes[8] = (bytes[8]! & 0x3f) | 0x80
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

function isHostCommand(value: unknown): value is HostCommand {
  if (typeof value !== 'object' || value === null || !('action' in value) || !('target' in value)) return false
  const command = value as Record<string, unknown>
  const target = command.target
  if (typeof target !== 'object' || target === null || !('run' in target) || !('actor' in target) || !('incarnation' in target)) return false
  const identity = target as Record<string, unknown>
  if (![identity.run, identity.actor, identity.incarnation].every((part) => typeof part === 'string')) return false
  if (command.action === 'input') return typeof command.text === 'string'
  if (command.action === 'retire') return true
  return command.action === 'interrupt' && typeof command.expected_round === 'string'
}

function decode(value: unknown): BrowserCommandRecord[] {
  if (!Array.isArray(value)) return []
  return value.filter((item): item is BrowserCommandRecord => {
    if (typeof item !== 'object' || item === null) return false
    const record = item as Record<string, unknown>
    const submission = record.submission
    if (typeof record.hostRun !== 'string' || typeof record.state !== 'string' || typeof submission !== 'object' || submission === null) return false
    const candidate = submission as Record<string, unknown>
    return typeof candidate.operation_id === 'string' && isHostCommand(candidate.command)
  })
}

export function readPendingCommands(): BrowserCommandRecord[] {
  try { return decode(JSON.parse(sessionStorage.getItem(storageKey) ?? '[]')) }
  catch { return [] }
}

export function writePendingCommands(records: readonly BrowserCommandRecord[]): void {
  sessionStorage.setItem(storageKey, JSON.stringify(records))
}

export function retainCommand(records: readonly BrowserCommandRecord[], hostRun: string, submission: HostCommandSubmission): BrowserCommandRecord[] {
  const next = records.filter((record) => record.submission.operation_id !== submission.operation_id)
  next.push({ hostRun, submission, state: 'queued' })
  return next
}

export function applyCommandStatus(records: readonly BrowserCommandRecord[], hostRun: string, status: EmbeddedCommandRecord): BrowserCommandRecord[] {
  return records.map((record) => record.hostRun === hostRun && record.submission.operation_id === status.operationId
    ? { ...record, state: status.state }
    : record)
}
