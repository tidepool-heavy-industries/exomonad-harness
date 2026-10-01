import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import App from './App'
import { clearDrafts } from './drafts'
import { clearWorkerChatRetention } from './WorkerChat'
import { createViewProjector } from './integration'
import {
  canonicalOperationId, isOperationId, normalizeSnapshot, sameHostCommand,
  type HostCommand, type HostCommandSubmission, type NormalizedState, type Snapshot,
} from './protocol'
import {
  applyCommandReceipt, applyCommandStatus, allocateOperationId, observeCommand, readCommandLedger,
  retainCommand, writePendingCommands, type BrowserCommandRecord,
} from './pending-commands'
import { createCommandLookups } from './command-lookups'
import { CommandStatusError, getCommandStatus, getSessionStatus, login, logout, SessionApiError } from './session-api'
import { connectHarness, type HarnessConnection } from './ws-client'
import type { DemoSubmissionResult, LocalSubmissionResult, TransportPhase } from './client-contract'

const emptySnapshot: Snapshot = { seq: 0, conversations: [], requests: [], jobs: [], envelopes: [] }
const reconnectDelays = [1_000, 2_000, 5_000, 10_000, 30_000] as const

export function Operator() {
  const [state, setState] = useState<NormalizedState>(() => normalizeSnapshot(emptySnapshot))
  const stateRef = useRef(state)
  const [project] = useState(createViewProjector)
  const [snapshotLoaded, setSnapshotLoaded] = useState(false)
  const [authenticated, setAuthenticated] = useState<boolean | undefined>()
  const [secret, setSecret] = useState('')
  const [failure, setFailure] = useState('')
  const [demoFeedback, setDemoFeedback] = useState('')
  const [checking, setChecking] = useState(true)
  const [transportPhase, setTransportPhase] = useState<TransportPhase>('connecting')
  const phaseRef = useRef<TransportPhase>('connecting')
  const [connectionEpoch, setConnectionEpoch] = useState(0)
  const connectionRef = useRef<HarnessConnection>()
  const [acceptedCommandIds, setAcceptedCommandIds] = useState<string[]>([])
  const [restored] = useState(readCommandLedger)
  const [pendingCommands, setPendingCommands] = useState(restored.records)
  const pendingRef = useRef(restored.records)
  const lookupsRef = useRef<ReturnType<typeof createCommandLookups>>()
  const sessionGeneration = useRef(0)
  const sessionController = useRef<AbortController>()
  const sessionPending = useRef<Promise<void>>()
  const reconnectAttempt = useRef(0)
  const mounted = useRef(true)

  const updatePending = useCallback((records: BrowserCommandRecord[], requireRetention = false): boolean => {
    const persist = () => writePendingCommands(records)
    if (requireRetention) {
      try { records = persist() } catch {
        setFailure('This browser could not retain the operation before sending. The draft has been preserved.')
        return false
      }
    }
    // A later metadata write failure must not undo true observations in memory.
    pendingRef.current = records
    setPendingCommands(records)
    if (!requireRetention) {
      try {
        const persisted = persist()
        pendingRef.current = persisted
        setPendingCommands(persisted)
      } catch {
        setFailure('The latest operation observation is visible, but this browser could not persist its metadata.')
      }
    }
    return true
  }, [])

  const checkSession = useCallback((): Promise<void> => {
    if (sessionPending.current) return sessionPending.current
    const generation = ++sessionGeneration.current
    const controller = new AbortController()
    sessionController.current?.abort()
    sessionController.current = controller
    setChecking(true)
    const pending = getSessionStatus(controller.signal).then((result) => {
      if (!mounted.current || generation !== sessionGeneration.current) return
      setAuthenticated(result.authenticated)
    }, (error: unknown) => {
      if (!mounted.current || generation !== sessionGeneration.current || controller.signal.aborted) return
      if (error instanceof SessionApiError && (error.status === 401 || error.status === 403)) setAuthenticated(false)
      setFailure(error instanceof Error ? error.message : 'Unable to check the browser session.')
    }).finally(() => {
      if (mounted.current && generation === sessionGeneration.current) {
        setChecking(false)
        sessionPending.current = undefined
      }
    })
    sessionPending.current = pending
    return pending
  }, [])

  useEffect(() => {
    mounted.current = true
    if (restored.issues.length) setFailure(restored.issues.join(' '))
    void checkSession()
    return () => {
      mounted.current = false
      sessionGeneration.current += 1
      sessionController.current?.abort()
      sessionPending.current = undefined
    }
  }, [checkSession, restored])

  useEffect(() => {
    if (authenticated !== true) return
    let active = true
    let disconnected = false
    let receivedSnapshot = false
    let priorReceipts: NormalizedState['commandReceipts'] | undefined
    let priorRun: string | undefined
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined
    let connectTimer: ReturnType<typeof setTimeout> | undefined
    const sent = new Map<string, HostCommandSubmission>()
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
    const socket = new WebSocket(`${protocol}//${location.host}/api/ws`)
    const phase = (next: TransportPhase) => {
      if (!active) return
      phaseRef.current = next
      setTransportPhase(next)
      if (next !== 'ready') lookupsRef.current?.pause()
    }
    const lookups = createCommandLookups({
      current: () => pendingRef.current,
      read: getCommandStatus,
      result(record, status) {
        if (!active || disconnected) return
        try {
          updatePending(status
            ? applyCommandStatus(pendingRef.current, record.hostRun, status)
            : observeCommand(pendingRef.current, record.hostRun, record.submission.operation_id,
              { lookup: { kind: 'unavailable', reason: 'The operation is not available from this host (HTTP 404).' } }))
        } catch (error) {
          updatePending(observeCommand(pendingRef.current, record.hostRun, record.submission.operation_id,
            { lookup: { kind: 'error', reason: error instanceof Error ? error.message : 'Command status is invalid.' } }))
        }
      },
      error(record, error) {
        if (!active || disconnected) return
        updatePending(observeCommand(pendingRef.current, record.hostRun, record.submission.operation_id,
          { lookup: { kind: 'error', reason: error instanceof Error ? error.message : 'Command status is unavailable.' } }))
        if (error instanceof CommandStatusError && (error.status === 401 || error.status === 403)) void checkSession()
      },
    })
    lookupsRef.current = lookups
    const reconnect = () => {
      if (!active || disconnected) return
      disconnected = true
      phase('disconnected')
      connectionRef.current = undefined
      lookups.dispose()
      socket.close()
      if (connectTimer !== undefined) clearTimeout(connectTimer)
      // Rechecking auth is observational; temporary network errors keep App mounted.
      void checkSession()
      reconnectTimer = setTimeout(() => {
        if (active) setConnectionEpoch((epoch) => epoch + 1)
      }, reconnectDelays[Math.min(reconnectAttempt.current++, reconnectDelays.length - 1)]!)
    }
    const connection = connectHarness(socket, {
      phase,
      disconnected: reconnect,
      error(message) { if (active) setFailure(message) },
      receive(next) {
        if (!active || disconnected) return
        stateRef.current = next
        setState(next)
        setSnapshotLoaded(true)
        reconnectAttempt.current = 0
        if (connectTimer !== undefined) clearTimeout(connectTimer)
        let records = pendingRef.current
        const changedReceipts = [...next.commandReceipts.entries()]
          .filter(([key, receipt]) => priorReceipts?.get(key) !== receipt || priorRun !== next.hostRun)
          .map(([, receipt]) => receipt)
        for (const receipt of changedReceipts) records = applyCommandReceipt(records, receipt)
        if (records.some((record, index) => record !== pendingRef.current[index])) updatePending(records)
        const run = next.hostRun
        if (run !== undefined) {
          lookups.reconcile(records, run)
          if (!receivedSnapshot) {
            // A new connection may recheck immutable unconfirmed evidence once.
            lookups.reconcile(records.filter((record) => record.state === 'unconfirmed'), run, true)
          }
          for (const receipt of changedReceipts) {
            if (receipt.target === undefined && isOperationId(receipt.commandId)) {
              const matching = records.filter((record) => record.hostRun === run
                && canonicalOperationId(record.submission.operation_id) === canonicalOperationId(receipt.commandId))
              lookups.reconcile(matching, run, true)
            }
          }
        }
        if (run === undefined) lookups.pause()
        priorReceipts = next.commandReceipts
        priorRun = run
        receivedSnapshot = true
      },
      accepted(commandId) {
        if (!active || disconnected) return
        const run = stateRef.current.hostRun
        if (run === undefined) {
          if (phaseRef.current === 'ready') setAcceptedCommandIds((ids) =>
            ids.includes(commandId) ? ids : [...ids, commandId].slice(-128))
          return
        }
        if (!isOperationId(commandId)) return
        const original = sent.get(canonicalOperationId(commandId))
        if (original) {
          updatePending(observeCommand(pendingRef.current, original.command.target.run, commandId, { accepted: true }))
          setAcceptedCommandIds((ids) => ids.includes(canonicalOperationId(commandId)) ? ids : [...ids, canonicalOperationId(commandId)].slice(-128))
        }
        if (phaseRef.current === 'ready') {
          const matching = pendingRef.current.filter((record) => record.hostRun === run
            && canonicalOperationId(record.submission.operation_id) === canonicalOperationId(commandId))
          lookups.reconcile(matching, run, true)
        }
      },
      refused(refusal) {
        if (!active || disconnected) return
        setFailure(refusal.reason)
        const original = refusal.operation_id === null ? undefined : sent.get(canonicalOperationId(refusal.operation_id))
        if (original && refusal.operation_id) {
          updatePending(observeCommand(pendingRef.current, original.command.target.run, refusal.operation_id, { localRefusal: refusal }))
        } else setDemoFeedback(refusal.reason)
      },
    })
    connectionRef.current = {
      send(command) {
        const observation = connection.send(command)
        if (typeof command !== 'string' && observation !== 'not_sent') sent.set(canonicalOperationId(command.operation_id), command)
        return observation
      },
      dispose: connection.dispose,
    }
    connectTimer = setTimeout(() => {
      if (!active || receivedSnapshot) return
      setFailure('The connection did not provide an authoritative snapshot within 10 seconds.')
      reconnect()
    }, 10_000)
    return () => {
      active = false
      connection.dispose()
      connectionRef.current = undefined
      if (lookupsRef.current === lookups) lookupsRef.current = undefined
      lookups.dispose()
      if (connectTimer !== undefined) clearTimeout(connectTimer)
      if (reconnectTimer !== undefined) clearTimeout(reconnectTimer)
      socket.close()
    }
  }, [authenticated, connectionEpoch, checkSession, updatePending])

  const submitHostCommand = (command: HostCommand): LocalSubmissionResult => {
    const run = stateRef.current.hostRun
    if (run === undefined || phaseRef.current !== 'ready' || !connectionRef.current) {
      return { kind: 'blocked', reason: 'A fresh authoritative host snapshot and connected command channel are required.' }
    }
    if (command.target.run !== run) return { kind: 'blocked', reason: 'The operation targets a different host run.' }
    try {
      const submission: HostCommandSubmission = { operation_id: allocateOperationId(), command }
      const records = retainCommand(pendingRef.current, run, submission)
      if (!updatePending(records, true)) return { kind: 'blocked', reason: 'This browser could not retain the operation before sending.' }
      const original = records.find((record) => canonicalOperationId(record.submission.operation_id) === canonicalOperationId(submission.operation_id)
        && record.hostRun === run)!.submission
      const send = connectionRef.current?.send(original) ?? 'not_sent'
      updatePending(observeCommand(pendingRef.current, run, original.operation_id, { send }))
      lookupsRef.current?.reconcile(pendingRef.current, run)
      return { kind: 'retained', operationId: original.operation_id, send }
    } catch (error) {
      const reason = error instanceof Error ? error.message : 'The operation could not be retained.'
      setFailure(reason)
      return { kind: 'blocked', reason }
    }
  }

  const retryHostCommand = (submission: HostCommandSubmission): LocalSubmissionResult => {
    const run = stateRef.current.hostRun
    if (run === undefined || phaseRef.current !== 'ready' || !connectionRef.current || !isOperationId(submission.operation_id))
      return { kind: 'blocked', reason: 'A fresh authoritative host connection is required for explicit retry.' }
    const record = pendingRef.current.find((item) => item.hostRun === run
      && canonicalOperationId(item.submission.operation_id) === canonicalOperationId(submission.operation_id))
    if (!record || !sameHostCommand(record.submission.command, submission.command))
      return { kind: 'blocked', reason: 'This exact retained operation is unavailable in the current host run.' }
    if (!updatePending(pendingRef.current, true)) return { kind: 'blocked', reason: 'This browser could not retain the operation before retrying.' }
    const send = connectionRef.current.send(record.submission)
    updatePending(observeCommand(pendingRef.current, run, record.submission.operation_id, { send }))
    lookupsRef.current?.reconcile([record], run, true)
    return { kind: 'retained', operationId: record.submission.operation_id, send }
  }

  const submitDemoCommand = (command: string): DemoSubmissionResult => {
    if (stateRef.current.hostRun !== undefined || phaseRef.current !== 'ready' || !connectionRef.current)
      return { kind: 'blocked', reason: 'A fresh standalone connection is required.' }
    const send = connectionRef.current.send(command)
    if (send !== 'sent') {
      const reason = send === 'unknown' ? 'The demo send outcome is unknown; inspect the connection before retrying.' : 'The command channel is disconnected.'
      setDemoFeedback(reason)
      return { kind: 'blocked', reason }
    }
    setDemoFeedback('')
    return { kind: 'sent' }
  }

  const submitLogin = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const suppliedSecret = secret
    setSecret('')
    setFailure('')
    sessionController.current?.abort()
    sessionPending.current = undefined
    const generation = ++sessionGeneration.current
    const controller = new AbortController()
    sessionController.current = controller
    setChecking(true)
    try {
      const result = await login(suppliedSecret, controller.signal)
      if (mounted.current && generation === sessionGeneration.current) setAuthenticated(result.authenticated)
    } catch (error) {
      if (!mounted.current || generation !== sessionGeneration.current) return
      if (error instanceof SessionApiError && (error.status === 401 || error.status === 403)) setAuthenticated(false)
      setFailure(error instanceof Error ? error.message : 'Unable to sign in.')
    } finally { if (mounted.current && generation === sessionGeneration.current) setChecking(false) }
  }

  const signOut = async () => {
    setFailure('')
    sessionController.current?.abort()
    sessionPending.current = undefined
    const generation = ++sessionGeneration.current
    const controller = new AbortController()
    sessionController.current = controller
    setChecking(true)
    try {
      await logout(controller.signal)
      clearWorkerChatRetention()
      if (!mounted.current || generation !== sessionGeneration.current) return
      try { clearDrafts() } catch { setFailure('Signed out; some browser draft storage could not be cleared.') }
      setAuthenticated(false)
      phaseRef.current = 'disconnected'
      setTransportPhase('disconnected')
    } catch (error) {
      if (mounted.current && generation === sessionGeneration.current)
        setFailure(error instanceof Error ? error.message : 'Unable to sign out.')
    } finally { if (mounted.current && generation === sessionGeneration.current) setChecking(false) }
  }

  if (checking && authenticated === undefined)
    return <main aria-busy="true"><p role="status">Checking session…</p></main>

  if (authenticated !== true) return (
    <main className="session-screen" aria-labelledby="session-heading">
      <h1 id="session-heading">{authenticated === false ? 'Operator sign in' : 'Check operator session'}</h1>
      <p className="hint">Enter the browser session secret. Trusted-proxy access is checked automatically.</p>
      {failure && <p className="session-error" role="alert">{failure}</p>}
      <form className="session-form" onSubmit={(event) => void submitLogin(event)}>
        <label htmlFor="session-secret">Session secret</label>
        <input id="session-secret" type="password" autoComplete="current-password" value={secret}
          onChange={(event) => setSecret(event.target.value)} />
        <button type="submit" disabled={!secret || checking}>Sign in</button>
      </form>
      <button type="button" onClick={() => void checkSession()} disabled={checking}>
        {checking ? 'Checking…' : 'Check session / retry'}
      </button>
    </main>
  )

  return <>
    <header className="session-bar">
      <span role="status">Session authenticated · {transportPhase}</span>
      <button type="button" onClick={() => void signOut()} disabled={checking}>Sign out</button>
      <button type="button" onClick={() => void checkSession()} disabled={checking}>Recheck session</button>
    </header>
    {failure && <p className="session-error" role="alert">{failure}</p>}
    {snapshotLoaded
      ? <App data={project(state)} onHostCommand={submitHostCommand} onDemoCommand={submitDemoCommand}
        onRetry={retryHostCommand} transportPhase={transportPhase} pendingCommands={pendingCommands}
        acceptedCommandIds={acceptedCommandIds} demoFeedback={demoFeedback} onAuthExpired={() => { void checkSession() }} />
      : <main aria-busy="true"><p role="status">Loading authoritative harness snapshot…</p></main>}
  </>
}
