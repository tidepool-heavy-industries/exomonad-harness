import {
  canonicalOperationId, isCommandReceipt, isCommandRefusal, isCommandState, isHostCommand,
  isObject, isOperationId, sameHostCommand, sameHostIdentity,
  type CommandReceipt, type EmbeddedCommandRecord, type HostCommandRefusal, type HostCommandSubmission,
} from './protocol'

import { signedDecimal } from './decimal'

const storageKey = 'harness.embeddedCommands.v2'
const legacyStorageKey = 'harness.embeddedCommands.v1'

export interface BrowserCommandRecord {
  readonly hostRun: string
  readonly submission: HostCommandSubmission
  /** Local queued and legacy hints are never authoritative host state. */
  readonly authority: 'local' | 'legacy' | 'status' | 'receipt'
  readonly state: EmbeddedCommandRecord['state']
  readonly receipt?: CommandReceipt | null
  readonly envelopeId?: string | null
  readonly accepted?: boolean
  readonly send?: 'sent' | 'not_sent' | 'unknown'
  readonly lookup?: { readonly kind: 'unavailable' | 'error'; readonly reason: string }
  readonly localRefusal?: HostCommandRefusal
  readonly issues?: readonly string[]
}

interface QuarantinedStorage {
  readonly source: 'v1' | 'v2'
  readonly raw: string
  readonly reason: string
}
export interface CommandLedger {
  readonly records: BrowserCommandRecord[]
  readonly available: boolean
  readonly quarantine: readonly QuarantinedStorage[]
  readonly issues: readonly string[]
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

export function operationKey(record: Pick<BrowserCommandRecord, 'hostRun' | 'submission'>): string {
  return JSON.stringify([record.hostRun, canonicalOperationId(record.submission.operation_id)])
}

export function isSettledCommand(record: BrowserCommandRecord): boolean {
  return (record.authority === 'status' || record.authority === 'receipt')
    && ['input_admitted', 'control_requested', 'refused'].includes(record.state)
}

/** Store unconfirmed is immutable evidence, but remains unresolved for retention. */
export function needsCommandLookup(record: BrowserCommandRecord): boolean {
  return record.authority === 'local' || record.authority === 'legacy'
    || record.state === 'queued' || record.state === 'dispatching'
}

function issue(record: BrowserCommandRecord, message: string): BrowserCommandRecord {
  const issues = record.issues ?? []
  return issues.includes(message) ? record : { ...record, issues: [...issues, message] }
}

function decodeRecord(value: unknown, legacy: boolean): BrowserCommandRecord | undefined {
  if (!isObject(value) || typeof value.hostRun !== 'string' || !isCommandState(value.state)
    || !isObject(value.submission) || !isOperationId(value.submission.operation_id)
    || !isHostCommand(value.submission.command) || value.submission.command.target.run !== value.hostRun) return
  if (!legacy && !['local', 'legacy', 'status', 'receipt'].includes(value.authority as string)) return
  if (value.receipt !== undefined && value.receipt !== null && !isCommandReceipt(value.receipt)) return
  if (value.envelopeId !== undefined && value.envelopeId !== null
    && !signedDecimal(value.envelopeId)) return
  if (value.accepted !== undefined && typeof value.accepted !== 'boolean') return
  if (value.send !== undefined && !['sent', 'not_sent', 'unknown'].includes(value.send as string)) return
  if (value.lookup !== undefined && (!isObject(value.lookup) || !['unavailable', 'error'].includes(value.lookup.kind as string)
    || typeof value.lookup.reason !== 'string')) return
  if (value.localRefusal !== undefined && !isCommandRefusal(value.localRefusal)) return
  if (value.issues !== undefined && (!Array.isArray(value.issues) || !value.issues.every((item) => typeof item === 'string'))) return
  // Retain valid legacy evidence and unknown metadata in the original JSON shape.
  const record = { ...value, authority: legacy ? 'legacy' : value.authority } as unknown as BrowserCommandRecord
  if (!legacy && (record.authority === 'status' || record.authority === 'receipt')) {
    if ((record.state === 'input_admitted' && record.submission.command.action !== 'input')
      || (record.state === 'control_requested' && record.submission.command.action === 'input')) return
    if (record.authority === 'receipt' && (!record.receipt || !record.receipt.target)) return
    if (record.receipt && (!receiptMatches(record, record.receipt) || receiptState(record.receipt) !== record.state)) return
    if (record.receipt?.outcome === 'admitted' && record.envelopeId != null
      && record.envelopeId !== record.receipt.envelopeId) return
  }
  return record
}

export function readCommandLedger(): CommandLedger {
  const records: BrowserCommandRecord[] = []
  const quarantine: QuarantinedStorage[] = []
  const issues: string[] = []
  let legacyImported = false
  let available = true
  for (const [key, source] of [[storageKey, 'v2'], [legacyStorageKey, 'v1']] as const) {
    let raw: string | null
    try { raw = sessionStorage.getItem(key) } catch { available = false; issues.push('This browser could not read retained operations.'); continue }
    if (raw === null || (source === 'v1' && legacyImported)) continue
    try {
      const decoded: unknown = JSON.parse(raw)
      const rows = source === 'v1' ? decoded : isObject(decoded) && decoded.version === 2 ? decoded.records : undefined
      if (!Array.isArray(rows)) throw new Error('Invalid ledger format.')
      if (source === 'v2' && isObject(decoded)) legacyImported = decoded.legacyImported === true
      if (source === 'v2' && isObject(decoded) && Array.isArray(decoded.quarantine)) {
        for (const entry of decoded.quarantine) {
          if (isObject(entry) && (entry.source === 'v1' || entry.source === 'v2')
            && typeof entry.raw === 'string' && typeof entry.reason === 'string') quarantine.push(entry as unknown as QuarantinedStorage)
          else quarantine.push({ source, raw: JSON.stringify(entry), reason: 'Invalid quarantine entry.' })
        }
      }
      for (const row of rows) {
        const record = decodeRecord(row, source === 'v1')
        if (!record) { quarantine.push({ source, raw: JSON.stringify(row), reason: 'Invalid retained operation.' }); continue }
        const existing = records.findIndex((item) => operationKey(item) === operationKey(record))
        if (existing < 0) records.push(record)
        else if (!sameHostCommand(records[existing]!.submission.command, record.submission.command)) {
          records[existing] = issue(records[existing]!, 'A stored operation ID has contradictory original contents.')
          quarantine.push({ source, raw: JSON.stringify(row), reason: 'Conflicting retained operation.' })
        }
      }
    } catch { quarantine.push({ source, raw, reason: 'The original ledger could not be decoded.' }) }
  }
  if (quarantine.length) issues.push('Some retained operation data was quarantined; its original content is preserved.')
  return { records, quarantine, issues, available }
}

export function readPendingCommands(): BrowserCommandRecord[] { return readCommandLedger().records }

function compact(records: readonly BrowserCommandRecord[]): BrowserCommandRecord[] {
  const settled = records.filter(isSettledCommand)
  const discard = new Set(settled.slice(0, Math.max(0, settled.length - 128)).map(operationKey))
  return records.filter((record) => !discard.has(operationKey(record)))
}

export function writePendingCommands(records: readonly BrowserCommandRecord[]): BrowserCommandRecord[] {
  const prior = readCommandLedger()
  if (!prior.available) throw new Error('Retained operations could not be read; their existing data was preserved.')
  const keys = new Set(records.map(operationKey))
  const retained = compact([...prior.records.filter((record) => !keys.has(operationKey(record))), ...records])
  const quarantine = prior.quarantine.filter((entry, index, all) =>
    all.findIndex((other) => other.source === entry.source && other.raw === entry.raw && other.reason === entry.reason) === index)
  sessionStorage.setItem(storageKey, JSON.stringify({ version: 2, legacyImported: true, records: retained, quarantine }))
  // Never remove or overwrite v1: recovery can inspect its exact original bytes.
  return retained
}

export function retainCommand(records: readonly BrowserCommandRecord[], hostRun: string, submission: HostCommandSubmission): BrowserCommandRecord[] {
  if (!isOperationId(submission.operation_id) || !isHostCommand(submission.command) || submission.command.target.run !== hostRun) {
    throw new Error('The operation must address the current exact host run with valid identities.')
  }
  const existing = records.find((record) => record.hostRun === hostRun
    && canonicalOperationId(record.submission.operation_id) === canonicalOperationId(submission.operation_id))
  if (existing) {
    if (!sameHostCommand(existing.submission.command, submission.command)) throw new Error('The operation ID is already retained with different contents.')
    return [...records]
  }
  // A caller cannot mutate the retained payload after submission.
  const immutable = JSON.parse(JSON.stringify(submission)) as HostCommandSubmission
  Object.freeze(immutable.command.target)
  Object.freeze(immutable.command)
  Object.freeze(immutable)
  return compact([...records, { hostRun, submission: immutable, authority: 'local', state: 'queued' }])
}

function receiptState(receipt: CommandReceipt): EmbeddedCommandRecord['state'] {
  switch (receipt.outcome) {
    case 'admitted': return 'input_admitted'
    case 'control_requested': return 'control_requested'
    case 'refused': return 'refused'
    case 'unconfirmed': return 'unconfirmed'
  }
}

function receiptMatches(record: BrowserCommandRecord, receipt: CommandReceipt): boolean {
  if (!isOperationId(receipt.commandId)
    || canonicalOperationId(receipt.commandId) !== canonicalOperationId(record.submission.operation_id)) return false
  if (receipt.target != null && !sameHostIdentity(receipt.target, record.submission.command.target)) return false
  switch (receipt.outcome) {
    case 'admitted': return record.submission.command.action === 'input'
    case 'control_requested': return record.submission.command.action === receipt.control
    case 'refused': case 'unconfirmed': return true
  }
}

function mergeEvidence(record: BrowserCommandRecord, state: EmbeddedCommandRecord['state'], authority: 'status' | 'receipt',
  receipt?: CommandReceipt | null, envelopeId?: string | null): BrowserCommandRecord {
  if ((state === 'input_admitted' && record.submission.command.action !== 'input')
    || (state === 'control_requested' && record.submission.command.action === 'input'))
    return issue(record, 'Observed handoff state contradicts this operation’s action.')
  if ((record.authority === 'status' || record.authority === 'receipt')
    && ((record.state === 'dispatching' && state === 'queued')
      || (record.state === 'unconfirmed' && (state === 'queued' || state === 'dispatching')))) return record
  if (receipt && !receiptMatches(record, receipt)) return issue(record, 'Observed receipt contradicts this operation’s target or action.')
  if (receipt && receiptState(receipt) !== state) return issue(record, 'Observed status and receipt report contradictory handoff outcomes.')
  if (isSettledCommand(record) && !['input_admitted', 'control_requested', 'refused'].includes(state)) return record
  if (isSettledCommand(record) && record.state !== state) return issue(record, `Contradictory handoff evidence: retained ${record.state}, observed ${state}.`)
  if (record.envelopeId != null && envelopeId != null && record.envelopeId !== envelopeId)
    return issue(record, 'Contradictory admitted envelope identity.')
  let combined = receipt === undefined ? record.receipt : receipt === null ? record.receipt ?? null : receipt
  if (record.receipt && receipt) {
    if (record.receipt.outcome !== receipt.outcome) {
      if (isSettledCommand(record)) return issue(record, 'Contradictory receipt outcome.')
    } else if (receipt.outcome === 'admitted' && record.receipt.outcome === 'admitted') {
      if (record.receipt.envelopeId !== receipt.envelopeId) return issue(record, 'Contradictory admitted envelope identity.')
      if (record.receipt.wakeError !== undefined && receipt.wakeError !== undefined && record.receipt.wakeError !== receipt.wakeError)
        return issue(record, 'Contradictory admitted wake observations.')
      combined = { ...record.receipt, ...receipt, wakeError: receipt.wakeError ?? record.receipt.wakeError }
    } else if ('reason' in receipt && 'reason' in record.receipt && receipt.reason !== record.receipt.reason) {
      return issue(record, 'Contradictory handoff reasons.')
    }
  }
  return { ...record, state, authority, receipt: combined, envelopeId: envelopeId ?? record.envelopeId, lookup: undefined }
}

export function applyCommandStatus(records: readonly BrowserCommandRecord[], hostRun: string, status: EmbeddedCommandRecord): BrowserCommandRecord[] {
  return records.map((record) => {
    if (record.hostRun !== hostRun || canonicalOperationId(record.submission.operation_id) !== canonicalOperationId(status.operationId)) return record
    if (!sameHostCommand(record.submission.command, status.command)) throw new Error('The retained status does not match this operation’s exact target and contents.')
    return mergeEvidence(record, status.state, 'status', status.receipt, status.envelopeId)
  })
}

/** A missing target is ambiguous across runs and must be checked through GET. */
export function applyCommandReceipt(records: readonly BrowserCommandRecord[], receipt: CommandReceipt): BrowserCommandRecord[] {
  if (!receipt.target || !isOperationId(receipt.commandId)) return [...records]
  return records.map((record) => receiptMatches(record, receipt)
    ? mergeEvidence(record, receiptState(receipt), 'receipt', receipt) : record)
}

export function observeCommand(records: readonly BrowserCommandRecord[], hostRun: string, operationId: string,
  observation: Pick<BrowserCommandRecord, 'accepted' | 'send' | 'lookup' | 'localRefusal'>): BrowserCommandRecord[] {
  return records.map((record) => record.hostRun === hostRun
    && canonicalOperationId(record.submission.operation_id) === canonicalOperationId(operationId)
    ? { ...record, ...observation } : record)
}
