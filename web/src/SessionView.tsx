import { useId, useState, type FormEvent, type ReactNode } from 'react'
import type { AuthenticationMode } from './session-api'
import type { TransportPhase } from './client-contract'
import './SessionView.css'

export function SessionScreen({ authentication, authenticationAvailable, authenticated, checking, failure, secret, setSecret, submitLogin, checkSession }: { authentication?: AuthenticationMode; authenticationAvailable?: boolean; authenticated?: boolean; checking: boolean; failure: string; secret: string; setSecret: (value: string) => void; submitLogin: (event: FormEvent<HTMLFormElement>) => Promise<void>; checkSession: () => Promise<void> }) {
  return (
    <main className="session-screen" aria-labelledby="session-heading">
      <h1 id="session-heading">{authentication === 'tailscale' ? 'Tailscale access required'
        : authentication === 'disabled' || authenticationAvailable === false ? 'Operator access unavailable'
          : authenticated === false ? 'Operator sign in' : 'Check operator session'}</h1>
      <p className="hint">{authentication === 'tailscale'
        ? authenticationAvailable === false
          ? 'Tailscale authentication is unavailable on this server. Check the server configuration, then retry.'
          : 'Connect to the authorized Tailscale network, then retry the session check.'
        : authentication === 'disabled' || authenticationAvailable === false
          ? 'Browser operator authentication is unavailable on this server. Check the server configuration, then retry.'
          : authentication === 'secret' ? 'Enter the browser session secret.'
            : 'Check the operator session, then retry if needed.'}</p>
      {failure && <p className="session-error" role="alert">{failure}</p>}
      {authentication === 'secret' && authenticationAvailable !== false && <form className="session-form" onSubmit={(event) => void submitLogin(event)}>
        <label htmlFor="session-secret">Session secret</label>
        <input id="session-secret" name="session-secret" type="password" autoComplete="current-password" value={secret}
          onChange={(event) => setSecret(event.target.value)} />
        <button type="submit" disabled={!secret || checking}>Sign in</button>
      </form>}
      <button type="button" onClick={() => void checkSession()} disabled={checking}>
        {checking ? 'Checking…' : 'Retry session check'}
      </button>
    </main>
  )
}

export function SessionBar({ authentication, transportPhase, checking, signOut, checkSession, failure, children }: { authentication?: AuthenticationMode; transportPhase: TransportPhase; checking: boolean; signOut: () => Promise<void>; checkSession: () => Promise<void>; failure: string; children?: ReactNode }) {
  const [optionsOpen, setOptionsOpen] = useState(false)
  const optionsId = useId()
  return <>
    <section className="session-bar" aria-label="Browser session">
      <span role="status"><span className="session-authentication">Session authenticated{authentication === 'tailscale' ? ' · Tailscale' : authentication === 'disabled' ? ' · No browser authentication' : ''} · </span>{transportPhase === 'ready' ? 'Connected' : transportPhase}</span>
      <button className="session-toggle" type="button" aria-expanded={optionsOpen} aria-controls={optionsId} onClick={() => setOptionsOpen(!optionsOpen)}>Session options</button>
      <div id={optionsId} className={`session-actions${optionsOpen ? ' is-open' : ''}`}>
      {children}
      {authentication === 'secret' && <button type="button" onClick={() => void signOut()} disabled={checking}>Sign out</button>}
      <button type="button" onClick={() => void checkSession()} disabled={checking}>Recheck session</button>
      </div>
    </section>
    {failure && <p className="session-error" role="alert">{failure}</p>}
  </>
}
