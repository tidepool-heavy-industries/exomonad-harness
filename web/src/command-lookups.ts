import { needsCommandLookup, operationKey, type BrowserCommandRecord } from './pending-commands'
import type { EmbeddedCommandRecord } from './protocol'

const delays = [1_000, 2_000, 5_000, 10_000, 30_000] as const
interface PendingRead {
  record: BrowserCommandRecord
  attempt: number
  due: number
  forced: boolean
  controller?: AbortController
}

/** Read-only reconciliation. A connection generation owns timers and requests. */
export function createCommandLookups(options: {
  current: () => readonly BrowserCommandRecord[]
  read: (operationId: string, signal: AbortSignal) => Promise<EmbeddedCommandRecord | undefined>
  result: (record: BrowserCommandRecord, status: EmbeddedCommandRecord | undefined) => void
  error: (record: BrowserCommandRecord, error: unknown) => void
}) {
  const pending = new Map<string, PendingRead>()
  let generation = 0
  let disposed = false
  let active = 0
  let currentRun: string | undefined
  let timer: ReturnType<typeof setTimeout> | undefined

  function pump() {
    if (disposed) return
    if (timer !== undefined) { clearTimeout(timer); timer = undefined }
    const now = Date.now()
    for (const [key, entry] of pending) {
      if (active >= 2) break
      if (entry.controller || entry.due > now) continue
      const controller = new AbortController()
      entry.controller = controller
      active += 1
      const admittedGeneration = generation
      void options.read(entry.record.submission.operation_id, controller.signal).then((status) => {
        if (disposed || generation !== admittedGeneration || controller.signal.aborted) return
        options.result(entry.record, status)
      }).catch((error: unknown) => {
        if (disposed || generation !== admittedGeneration || controller.signal.aborted) return
        options.error(entry.record, error)
      }).finally(() => {
        if (disposed || generation !== admittedGeneration) return
        active -= 1
        entry.controller = undefined
        entry.forced = false
        const current = options.current().find((record) => operationKey(record) === key)
        if (!current || !needsCommandLookup(current)) pending.delete(key)
        else {
          entry.record = current
          entry.due = Date.now() + delays[Math.min(entry.attempt++, delays.length - 1)]!
        }
        pump()
      })
    }
    const queued = [...pending.values()].filter((entry) => !entry.controller)
    if (queued.length && active < 2) {
      const next = Math.min(...queued.map((entry) => entry.due))
      timer = setTimeout(pump, Math.max(0, next - Date.now()))
    }
  }

  function pause() {
    generation += 1
    if (timer !== undefined) { clearTimeout(timer); timer = undefined }
    for (const entry of pending.values()) entry.controller?.abort()
    pending.clear()
    active = 0
    currentRun = undefined
  }

  return {
    pause,
    reconcile(records: readonly BrowserCommandRecord[], run: string, force = false) {
      if (disposed) return
      if (run !== currentRun) { pause(); currentRun = run }
      for (const record of records) {
        if (record.hostRun !== run || (!force && !needsCommandLookup(record))) continue
        const key = operationKey(record)
        const existing = pending.get(key)
        if (existing) {
          existing.record = record
          if (force) existing.forced = true
          if (force && !existing.controller) existing.due = Date.now()
        } else pending.set(key, { record, attempt: 0, due: Date.now(), forced: force })
      }
      // Live terminal receipt cancels unnecessary queued reads without spinning on ledger writes.
      for (const [key, entry] of pending) {
        const current = options.current().find((record) => record.hostRun === run && operationKey(record) === key)
        if (!entry.controller && (!current || (!force && !entry.forced && !needsCommandLookup(current)))) pending.delete(key)
      }
      pump()
    },
    dispose() {
      disposed = true
      pause()
    },
  }
}
