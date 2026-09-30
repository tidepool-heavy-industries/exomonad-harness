import { useCallback, useEffect, useState, type FormEvent } from 'react'
import App from './App'
import { toViewModel } from './integration'
import { normalizeSnapshot, type HostCommand, type NormalizedState, type Snapshot } from './protocol'
import { getSessionStatus, login, logout } from './session-api'
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
  const [sendCommand, setSendCommand] = useState<(command: string | HostCommand) => void>()
  const [acceptedCommandIds, setAcceptedCommandIds] = useState<string[]>([])

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
          <section aria-labelledby="async-guidance-heading" className="command-guidance">
            <h2 id="async-guidance-heading">Async command scenario</h2>
            <p>Enter <code>async start</code> to start A. It automatically emits B on the next Engine turn while A remains pending; no second browser submission triggers B. <code>echo hello</code> is only an independent responsiveness check.</p>
            <p>Use <code>async release</code> to release A, or <code>async cancel</code> to cancel A.</p>
          </section>
          <App data={toViewModel(state)} onCommand={sendCommand} acceptedCommandIds={acceptedCommandIds} />
        </>
        : <main aria-busy="true"><p role="status">Loading authoritative harness snapshot…</p></main>}
    </>
  )
}
