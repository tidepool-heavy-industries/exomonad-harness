import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import App from './App'
import { toViewModel } from './integration'
import { normalizeSnapshot, type HostCommand, type HostCommandSubmission, type NormalizedState, type Snapshot } from './protocol'
import { applyCommandStatus, allocateOperationId, readPendingCommands, retainCommand, writePendingCommands, type BrowserCommandRecord } from './pending-commands'
import { getCommandStatus, getSessionStatus, login, logout } from './session-api'
import { connectHarness } from './ws-client'

const emptySnapshot: Snapshot = {
  seq: 0,
  conversations: [],
  requests: [],
  jobs: [],
  envelopes: [],
}

export function Operator() {
  const [state, setState] = useState<NormalizedState>(() => normalizeSnapshot(emptySnapshot))
  const [snapshotLoaded, setSnapshotLoaded] = useState(false)
  const [authenticated, setAuthenticated] = useState<boolean | undefined>()
  const [secret, setSecret] = useState('')
  const [failure, setFailure] = useState('')
  const [checking, setChecking] = useState(true)
  const [sendCommand, setSendCommand] = useState<(command: string | HostCommandSubmission) => void>()
  const [acceptedCommandIds, setAcceptedCommandIds] = useState<string[]>([])
  const [pendingCommands, setPendingCommands] = useState<BrowserCommandRecord[]>([])
  const pendingRef = useRef<BrowserCommandRecord[]>([])

  const updatePending = (records: BrowserCommandRecord[]) => {
    try {
      writePendingCommands(records)
    } catch {
      setFailure('This browser could not retain the operation in this tab. The command was not sent.')
      return false
    }
    pendingRef.current = records
    setPendingCommands(records)
    return true
  }

  const checkSession = useCallback(async () => {
    setChecking(true)
    setFailure('')
    try {
      const result = await getSessionStatus()
      setAuthenticated(result.authenticated)
    } catch (error) {
      setAuthenticated(false)
      setFailure(error instanceof Error ? error.message : 'Unable to check the browser session.')
    } finally {
      setChecking(false)
    }
  }, [])

  useEffect(() => {
    void checkSession()
  }, [checkSession])

  useEffect(() => {
    if (authenticated !== true) return
    const restored = readPendingCommands()
    pendingRef.current = restored
    setPendingCommands(restored)
    let active = true
    setState(normalizeSnapshot(emptySnapshot))
    setAcceptedCommandIds([])
    setSnapshotLoaded(false)
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
    const socket = new WebSocket(`${protocol}//${location.host}/api/ws`)
    const send = connectHarness(
      socket,
      (next) => {
        if (!active) return
        setState(next)
        setSnapshotLoaded(true)
      },
      (message) => {
        if (!active) return
        setFailure(`${message} Recheck the session or sign in again.`)
        setAuthenticated(false)
      },
      (commandId) => {
        if (!active) return
        setAcceptedCommandIds((current) => current.includes(commandId) ? current : [...current, commandId])
      },
    )
    setSendCommand(() => send)
    socket.addEventListener('close', () => {
      if (!active) return
      setFailure('Connection closed. Recheck the session or sign in again.')
      setAuthenticated(false)
    })
    return () => {
      active = false
      setSendCommand(undefined)
      socket.close()
    }
  }, [authenticated])

  useEffect(() => {
    const run = state.hostRun
    if (authenticated !== true || !snapshotLoaded || !run) return
    let active = true
    const reconcile = async () => {
      const records = pendingRef.current.filter((record) => record.hostRun === run)
      for (const record of records) {
        try {
          const status = await getCommandStatus(record.submission.operation_id)
          if (!active) return
          const next = status
            ? applyCommandStatus(pendingRef.current, run, status)
            : pendingRef.current.map((item) => item.hostRun === run && item.submission.operation_id === record.submission.operation_id
              ? { ...item, state: 'unconfirmed' as const }
              : item)
          updatePending(next)
        } catch {
          // Keep the retained operation available for an explicit same-ID retry.
        }
      }
    }
    void reconcile()
    return () => { active = false }
  }, [authenticated, snapshotLoaded, state.hostRun])

  const submitHostCommand = (command: string | HostCommand) => {
    if (typeof command === 'string') { sendCommand?.(command); return }
    const run = state.hostRun
    if (!run) { setFailure('The authoritative host run is not available.'); return }
    const submission: HostCommandSubmission = { operation_id: allocateOperationId(), command }
    // Commit the exact command and ID before sending, so transport uncertainty
    // can be reconciled after reload without inventing a second operation.
    if (!updatePending(retainCommand(pendingRef.current, run, submission))) return
    sendCommand?.(submission)
  }

  const retryHostCommand = (submission: HostCommandSubmission) => {
    const record = pendingRef.current.find((item) => item.submission.operation_id === submission.operation_id)
    if (!record || record.hostRun !== state.hostRun) {
      setFailure('This retained operation belongs to a different host run and cannot be retried here.')
      return
    }
    if (!updatePending(pendingRef.current.map((item) => item.submission.operation_id === submission.operation_id
      ? { ...item, state: 'dispatching' }
      : item))) return
    sendCommand?.(submission)
  }

  const submitLogin = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const suppliedSecret = secret
    setSecret('')
    setFailure('')
    try {
      const result = await login(suppliedSecret)
      setAuthenticated(result.authenticated)
    } catch (error) {
      setAuthenticated(false)
      setFailure(error instanceof Error ? error.message : 'Unable to sign in.')
    }
  }

  const signOut = async () => {
    setFailure('')
    try {
      await logout()
      setAuthenticated(false)
    } catch (error) {
      // In trusted-proxy mode there is no browser cookie to revoke. Preserve
      // the authenticated session and keep the limitation visible inline.
      setFailure(error instanceof Error ? error.message : 'Unable to sign out.')
    }
  }

  if (checking && authenticated === undefined) {
    return <main aria-busy="true"><p role="status">Checking session…</p></main>
  }

  if (authenticated !== true) {
    return (
      <main className="session-screen" aria-labelledby="session-heading">
        <h1 id="session-heading">Operator sign in</h1>
        <p className="hint">Enter the browser session secret. Trusted-proxy access is checked automatically.</p>
        {failure && <p className="session-error" role="alert">{failure}</p>}
        <form className="session-form" onSubmit={(event) => void submitLogin(event)}>
          <label htmlFor="session-secret">Session secret</label>
          <input
            id="session-secret"
            type="password"
            autoComplete="current-password"
            value={secret}
            onChange={(event) => setSecret(event.target.value)}
          />
          <button type="submit" disabled={!secret || checking}>Sign in</button>
        </form>
        <button type="button" onClick={() => void checkSession()} disabled={checking}>
          {checking ? 'Checking…' : 'Check session / retry'}
        </button>
      </main>
    )
  }

  return (
    <>
      <header className="session-bar">
        <span role="status">Session authenticated</span>
        <button type="button" onClick={() => void signOut()}>Sign out</button>
      </header>
      {failure && <p className="session-error" role="alert">{failure}</p>}
      {snapshotLoaded
        ? <>
          {state.hostRun === undefined && <section aria-labelledby="async-guidance-heading" className="command-guidance">
            <h2 id="async-guidance-heading">Standalone async command scenario</h2>
            <p>Enter <code>async start</code> to start A. It automatically emits B on the next Engine turn while A remains pending; no second browser submission triggers B. <code>echo hello</code> is only an independent responsiveness check.</p>
            <p>Use <code>async release</code> to release A, or <code>async cancel</code> to cancel A.</p>
          </section>}
          <App data={toViewModel(state)} onCommand={submitHostCommand} onRetry={retryHostCommand}
            pendingCommands={pendingCommands.filter((record) => record.hostRun === state.hostRun)} acceptedCommandIds={acceptedCommandIds} />
        </>
        : <main aria-busy="true"><p role="status">Loading authoritative harness snapshot…</p></main>}
    </>
  )
}
