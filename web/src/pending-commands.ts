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

export function isHostCommand(value: unknown): value is HostCommand {
  if (typeof value !== 'object' || value === null || !('action' in value) || !('target' in value)) return false
  const command = value as Record<string, unknown>
  const target = command.target
  if (typeof target !== 'object' || target === null || !('run' in target) || !('actor' in target) || !('incarnation' in target)) return false
  const identity = target as Record<string, unknown>
  if (![identity.run, identity.actor, identity.incarnation].every((part) => typeof part === 'string' && part.length > 0)) return false
  if (command.action === 'input') return typeof command.text === 'string'
  if (command.action === 'retire') return true
  return command.action === 'interrupt' && isOperationId(command.expected_round)
}

export function isOperationId(value: unknown): value is string {
  return typeof value === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)
}

export function isCommandState(value: unknown): value is EmbeddedCommandRecord['state'] {
  return ['queued', 'dispatching', 'input_admitted', 'control_requested', 'refused', 'unconfirmed'].includes(value as string)
}

export function sameHostCommand(left: HostCommand, right: HostCommand): boolean {
  return left.action === right.action
    && left.target.run === right.target.run
    && left.target.actor === right.target.actor
    && left.target.incarnation === right.target.incarnation
    && (left.action !== 'input' || (right.action === 'input' && left.text === right.text))
    && (left.action !== 'interrupt' || (right.action === 'interrupt' && left.expected_round === right.expected_round))
}

function decode(value: unknown): BrowserCommandRecord[] {
  if (!Array.isArray(value)) return []
  return value.filter((item): item is BrowserCommandRecord => {
    if (typeof item !== 'object' || item === null) return false
    const record = item as Record<string, unknown>
    const submission = record.submission
    if (typeof record.hostRun !== 'string' || !isCommandState(record.state) || typeof submission !== 'object' || submission === null) return false
    const candidate = submission as Record<string, unknown>
    return isOperationId(candidate.operation_id) && isHostCommand(candidate.command) && candidate.command.target.run === record.hostRun
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
  if (!isOperationId(submission.operation_id) || !isHostCommand(submission.command) || submission.command.target.run !== hostRun) {
    throw new Error('The operation must address the current exact host run with valid identities.')
  }
  const existing = records.find((record) => record.hostRun === hostRun && record.submission.operation_id === submission.operation_id)
  if (existing) {
    if (!sameHostCommand(existing.submission.command, submission.command)) throw new Error('The operation ID is already retained with different contents.')
    return [...records]
  }
  return [...records, { hostRun, submission, state: 'queued' }]
}

export function applyCommandStatus(records: readonly BrowserCommandRecord[], hostRun: string, status: EmbeddedCommandRecord): BrowserCommandRecord[] {
  return records.map((record) => {
    if (record.hostRun !== hostRun || record.submission.operation_id !== status.operationId) return record
    if (!sameHostCommand(record.submission.command, status.command)) throw new Error('The retained status does not match this operation’s exact target and contents.')
    return { ...record, state: status.state }
  })
}
