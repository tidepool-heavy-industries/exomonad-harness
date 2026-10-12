import type { ReactNode } from 'react'
import type { TransportPhase, SubmitDemoCommand } from './client-contract'
import type { useDraft } from './drafts'

export default function CommandView({ demo, sendDemo, onDemoCommand, transportPhase, demoLocalFeedback, demoFeedback, acceptedCommandIds, commandIds, timeline }: {
  demo: ReturnType<typeof useDraft>; sendDemo: (text: string) => void; onDemoCommand?: SubmitDemoCommand; transportPhase: TransportPhase; demoLocalFeedback: string; demoFeedback?: string; acceptedCommandIds: readonly string[]; commandIds: ReadonlySet<string>; timeline: ReactNode
}) {
  return <section>
          <h2>Send a command</h2>
          <p className="hint">Standalone deterministic demo · sending is separate from server acceptance and completion.</p>
          <p>
            <code>echo TEXT · test · wait · message TEXT · cancel · child TEXT · fail</code>
          </p>
          <div className="toolbar" aria-label="Common deterministic actions">
            {[['Run test', 'test'], ['Start wait', 'wait'], ['Cancel wait', 'cancel'], ['Create example child', 'child browser example'], ['Fail request', 'fail'], ['Recover with echo', 'echo recovered']].map(([label, value]) => <button key={value} disabled={!onDemoCommand || transportPhase !== 'ready'} onClick={() => sendDemo(value!)}>
              {label}
            </button>)}
          </div>
          <form className="command-form" onSubmit={event => {
            event.preventDefault(); if (demo.text.trim())
              sendDemo(demo.text);
          }}>
            <label htmlFor="demo-command">Command</label>
            <input id="demo-command" value={demo.text} onChange={event => demo.setText(event.target.value)} autoComplete="off" />
            <button disabled={!demo.text.trim() || !onDemoCommand || transportPhase !== 'ready'}>Send</button>
          </form>
          {!onDemoCommand && <p role="status">Command channel is unavailable; this view is read-only.</p>}{demo.error && <p role="status" className="error">
            {demo.error}
          </p>}{demoLocalFeedback && <p role="status">
            {demoLocalFeedback}
          </p>}{demoFeedback && <p role="status" className="error">
            {demoFeedback}
          </p>}{demo.lastSubmitted && <div>
            <pre>
              {demo.lastSubmitted}
            </pre>
            <button onClick={demo.restore}>Restore last demo command</button>
          </div>}{acceptedCommandIds.filter(id => !commandIds.has(id)).map(id => <p role="status" key={id}>Server accepted command {id}; this acknowledgment is not a terminal outcome.</p>)}
          <h2>Authoritative activity</h2>
          {timeline}
        </section>
}
