import React from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import { fixtureSnapshot } from './fixture'
import { toViewModel } from './integration'
import { normalizeSnapshot } from './protocol'
import './tokens.css'
import { connectHarness } from './ws-client'

function Operator() {
  const [state, setState] = React.useState(() => normalizeSnapshot(fixtureSnapshot))
  const [failure, setFailure] = React.useState<string>()
  const [command, setCommand] = React.useState<(value: string) => void>()
  React.useEffect(() => {
    if (new URLSearchParams(location.search).get('live') !== '1') return
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
    const socket = new WebSocket(`${protocol}//${location.host}/api/ws`)
    setCommand(connectHarness(socket, setState, setFailure))
    return () => socket.close()
  }, [])
  return <>
    {failure && <p role="alert">{failure}</p>}
    <App data={toViewModel(state)} onCommand={command} />
  </>
}

createRoot(document.getElementById('root')!).render(<React.StrictMode><Operator /></React.StrictMode>)
