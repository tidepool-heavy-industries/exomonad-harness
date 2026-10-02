import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { Operator } from './Operator'
import { readPendingCommands, writePendingCommands, retainCommand } from './pending-commands'

const sockets: FakeSocket[] = []
class FakeSocket {
  static readonly OPEN = 1
  readyState = 0
  readonly listeners = new Map<string, EventListener[]>()
  close = vi.fn()
  send = vi.fn()
  constructor(readonly url: string) { sockets.push(this) }
  addEventListener(type: string, listener: EventListener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener])
  }
}

function response(status: number, body?: unknown): Response {
  const payload = typeof body === 'object' && body !== null && 'authenticated' in body
    ? { authentication: 'secret', available: true, ...body } : body
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => payload,
  } as Response
}

const fetchMock = vi.fn<typeof fetch>()
beforeEach(() => {
  sockets.length = 0
  sessionStorage.clear()
  fetchMock.mockReset()
  vi.restoreAllMocks()
  vi.stubGlobal('fetch', fetchMock)
  vi.stubGlobal('WebSocket', FakeSocket)
})

describe('optional browser session', () => {
  it('logs in with same-origin credentials, clears the secret, then opens the socket', async () => {
    fetchMock
      .mockResolvedValueOnce(response(200, { authenticated: false }))
      .mockResolvedValueOnce(response(200, { authenticated: true }))
    render(<Operator />)
    const input = await screen.findByLabelText('Session secret')
    fireEvent.change(input, { target: { value: 'private-operator-secret' } })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    await waitFor(() => expect(sockets).toHaveLength(1))
    expect(sockets[0]?.url).toBe(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/api/ws`)
    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/session')
    expect(fetchMock.mock.calls[1]?.[1]).toMatchObject({
      method: 'POST',
      credentials: 'same-origin',
      body: JSON.stringify({ secret: 'private-operator-secret' }),
    })
    expect(screen.queryByLabelText('Session secret')).not.toBeInTheDocument()
    expect(localStorage.length).toBe(0)
  })

  it('shows a recoverable inline error after rejected credentials without opening the socket', async () => {
    fetchMock
      .mockResolvedValueOnce(response(200, { authenticated: false }))
      .mockResolvedValueOnce(response(401))
    render(<Operator />)
    fireEvent.change(await screen.findByLabelText('Session secret'), { target: { value: 'wrong-secret' } })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('secret was not accepted')
    expect(screen.getByRole('heading', { name: 'Operator sign in' })).toBeInTheDocument()
    expect(screen.queryByLabelText('Session secret')).toHaveValue('')
    expect(sockets).toHaveLength(0)
  })

  it('opens authenticated sessions and logs out local cookie sessions', async () => {
    fetchMock
      .mockResolvedValueOnce(response(200, { authenticated: true }))
      .mockResolvedValueOnce(response(204))
    render(<Operator />)
    fireEvent.click(await screen.findByRole('button', { name: 'Sign out' }))

    expect(await screen.findByRole('heading', { name: 'Operator sign in' })).toBeInTheDocument()
    expect(fetchMock.mock.calls[0]?.[1]).toMatchObject({ method: 'GET', credentials: 'same-origin' })
    expect(fetchMock.mock.calls[1]?.[1]).toMatchObject({ method: 'DELETE', credentials: 'same-origin' })
    expect(sockets).toHaveLength(1)
  })

  it('keeps the session authenticated when cookie logout is unavailable', async () => {
    fetchMock
      .mockResolvedValueOnce(response(200, { authenticated: true }))
      .mockResolvedValueOnce(response(404))
    render(<Operator />)
    expect(await screen.findByRole('button', { name: 'Sign out' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Browser login is not enabled')
    expect(screen.getByRole('button', { name: 'Sign out' })).toBeInTheDocument()
    expect(screen.queryByLabelText('Session secret')).not.toBeInTheDocument()
  })

  it('opens an authenticated Tailscale session without browser secret controls', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { authenticated: true, authentication: 'tailscale', available: true }))
    render(<Operator />)

    expect(await screen.findByText(/Session authenticated · Tailscale/)).toBeInTheDocument()
    expect(screen.queryByLabelText('Session secret')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Sign out' })).not.toBeInTheDocument()
    expect(sockets).toHaveLength(1)
  })

  it('shows retry guidance without a secret form when Tailscale denies access', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { authenticated: false, authentication: 'tailscale', available: true }))
    render(<Operator />)

    expect(await screen.findByText(/Connect to the authorized Tailscale network/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry session check' })).toBeInTheDocument()
    expect(screen.queryByLabelText('Session secret')).not.toBeInTheDocument()
    expect(sockets).toHaveLength(0)
  })

  it('shows only loading state until the authoritative WebSocket snapshot replaces it', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { authenticated: true }))
    render(<Operator />)

    expect(await screen.findByText('Loading authoritative harness snapshot…')).toBeInTheDocument()
    expect(screen.queryByText('/root')).not.toBeInTheDocument()
    expect(screen.queryByText('/root/web_ui/web_ui')).not.toBeInTheDocument()
    expect(screen.queryByText('job-1')).not.toBeInTheDocument()
    const socket = sockets[0]
    expect(socket).toBeDefined()
    socket?.listeners.get('message')?.forEach((listener) => listener(new MessageEvent('message', {
      data: JSON.stringify({
        type: 'snapshot',
        snapshot: {
          seq: 31,
          conversations: [{ id: 'live-root', path: '/authoritative/live-root', state: 'requesting' }],
          requests: [],
          jobs: [],
          envelopes: [],
        },
      }),
    })))

    expect(await screen.findByText('/authoritative/live-root')).toBeInTheDocument()
    expect(screen.queryByRole('region', { name: 'Standalone async command scenario' })).not.toBeInTheDocument()
    expect(screen.queryByText('Loading authoritative harness snapshot…')).not.toBeInTheDocument()
    expect(screen.queryByText('/root/web_ui/web_ui')).not.toBeInTheDocument()
  })

  it('reconciles an admitted operation after reload without allowing replay', async () => {
    const submission = {
      operation_id: '11111111-1111-4111-8111-111111111111',
      command: { action: 'input' as const, target: { run: 'run-1', actor: '/root', incarnation: 'inc-1' }, text: 'continue' },
    }
    writePendingCommands(retainCommand([], 'run-1', submission))
    fetchMock
      .mockResolvedValueOnce(response(200, { authenticated: true }))
      .mockResolvedValueOnce(response(200, {
        operationId: submission.operation_id, command: submission.command,
        state: 'input_admitted', envelopeId: 42, receipt: null,
      }))
    render(<Operator />)
    const socket = await waitFor(() => {
      expect(sockets).toHaveLength(1)
      return sockets[0]!
    })
    socket.readyState = FakeSocket.OPEN
    socket.listeners.get('message')?.forEach((listener) => listener(new MessageEvent('message', {
      data: JSON.stringify({
        type: 'snapshot',
        snapshot: {
          seq: 1,
          hostRun: 'run-1',
          actors: [{
            identity: { ...submission.command.target, incarnation: 'replacement' }, parent: null, kind: 'workflow', lifecycle: 'waiting',
            modelConversation: null, activeRound: '22222222-2222-4222-8222-222222222222',
          }],
          conversations: [], requests: [], jobs: [], envelopes: [],
        },
      }),
    })))

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2))
    expect(fetchMock.mock.calls[1]?.[0]).toBe(`/api/commands/${submission.operation_id}`)
    expect(await screen.findByText('Session authenticated · ready')).toBeInTheDocument()
    expect(socket.send).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('link', { name: 'Chat' }))
    const operations = screen.getByRole('region', { name: 'Retained browser operations' })
    expect(await within(operations).findByText('input · input_admitted')).toBeInTheDocument()
    expect(within(operations).getByText(/Reconnect never automatically resubmits an operation/)).toBeInTheDocument()
    const retry = within(operations).getByRole('button', { name: 'Retry same operation' })
    expect(retry).toBeDisabled()
    fireEvent.click(retry)
    expect(socket.send).not.toHaveBeenCalled()
  })

  it('does not show standalone async guidance in an embedded host session', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { authenticated: true }))
    render(<Operator />)
    const socket = await waitFor(() => {
      expect(sockets).toHaveLength(1)
      return sockets[0]!
    })
    socket.listeners.get('message')?.forEach((listener) => listener(new MessageEvent('message', {
      data: JSON.stringify({
        type: 'snapshot',
        snapshot: { seq: 1, hostRun: 'run-1', conversations: [], requests: [], jobs: [], envelopes: [] },
      }),
    })))
    expect(await screen.findByRole('link', { name: 'Chat' })).toBeInTheDocument()
    expect(screen.queryByRole('region', { name: 'Standalone async command scenario' })).not.toBeInTheDocument()
    expect(screen.queryByText('async start')).not.toBeInTheDocument()
  })
})

it('expires authentication on status refusal while preserving the operation and never sending it', async () => {
  const submission = { operation_id: '11111111-1111-4111-8111-111111111111', command: { action: 'retire' as const, target: { run: 'run-1', actor: '/root', incarnation: 'old' } } }
  writePendingCommands(retainCommand([], 'run-1', submission))
  fetchMock.mockResolvedValueOnce(response(200, { authenticated: true })).mockResolvedValueOnce(response(401)).mockResolvedValueOnce(response(200, { authenticated: false }))
  render(<Operator />)
  await waitFor(() => expect(sockets).toHaveLength(1))
  const socket = sockets[0]!
  socket.readyState = FakeSocket.OPEN
  socket.listeners.get('message')?.forEach((listener) => listener(new MessageEvent('message', { data: JSON.stringify({ type: 'snapshot', snapshot: { seq: 1, hostRun: 'run-1', conversations: [], requests: [], jobs: [], envelopes: [] } }) })))
  expect(await screen.findByRole('heading', { name: 'Operator sign in' })).toBeInTheDocument()
  expect(readPendingCommands()[0]?.lookup?.reason).toContain('HTTP 401')
  expect(socket.send).not.toHaveBeenCalled()
  expect(readPendingCommands()[0]?.submission).toEqual(submission)
})

it('keeps authentication when a host operation receives a typed refusal', async () => {
  fetchMock.mockResolvedValueOnce(response(200, { authenticated: true }))
  render(<Operator />)
  await waitFor(() => expect(sockets).toHaveLength(1))
  const socket = sockets[0]!
  socket.listeners.get('message')?.forEach((listener) => listener(new MessageEvent('message', { data: JSON.stringify({ type: 'command.refused', operation_id: null, code: 'conflict', reason: 'Operation contents conflict.' }) })))
  expect(await screen.findByRole('alert')).toHaveTextContent('Operation contents conflict.')
  expect(screen.getByRole('button', { name: 'Sign out' })).toBeInTheDocument()
  expect(screen.queryByRole('heading', { name: 'Operator sign in' })).not.toBeInTheDocument()
})

it('ignores a stale status response after sign out without changing retained input', async () => {
  const submission = { operation_id: '11111111-1111-4111-8111-111111111111', command: { action: 'input' as const, target: { run: 'run-1', actor: '/root', incarnation: 'old' }, text: '  exact λ payload  ' } }
  writePendingCommands(retainCommand([], 'run-1', submission))
  let finishStatus!: (value: Response) => void
  const pendingStatus = new Promise<Response>((resolve) => { finishStatus = resolve })
  fetchMock.mockResolvedValueOnce(response(200, { authenticated: true })).mockImplementationOnce(() => pendingStatus).mockResolvedValueOnce(response(204))
  render(<Operator />)
  await waitFor(() => expect(sockets).toHaveLength(1))
  const socket = sockets[0]!
  socket.listeners.get('message')?.forEach((listener) => listener(new MessageEvent('message', { data: JSON.stringify({ type: 'snapshot', snapshot: { seq: 1, hostRun: 'run-1', conversations: [], requests: [], jobs: [], envelopes: [] } }) })))
  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2))
  fireEvent.click(screen.getByRole('button', { name: 'Sign out' }))
  expect(await screen.findByRole('heading', { name: 'Operator sign in' })).toBeInTheDocument()
  finishStatus(response(401))
  await pendingStatus
  expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  expect(socket.send).not.toHaveBeenCalled()
  expect(readPendingCommands()[0]?.submission).toEqual(submission)
})

it('retains before sending and leaves host effects untouched when tab storage fails', async () => {
  fetchMock.mockResolvedValueOnce(response(200, { authenticated: true }))
  render(<Operator />)
  await waitFor(() => expect(sockets).toHaveLength(1))
  const socket = sockets[0]!
  socket.readyState = FakeSocket.OPEN
  socket.listeners.get('message')?.forEach((listener) => listener(new MessageEvent('message', { data: JSON.stringify({ type: 'snapshot', snapshot: { seq: 1, hostRun: 'run-1', actors: [{ identity: { run: 'run-1', actor: '/root', incarnation: 'one' }, parent: null, kind: 'model', lifecycle: 'waiting', modelConversation: '/root' }], conversations: [], requests: [], jobs: [], envelopes: [] } }) })))
  fireEvent.click(await screen.findByRole('link', { name: 'Chat' }))
  fireEvent.click(within(screen.getByRole('complementary', { name: 'Workers' })).getByRole('link', { name: '/root' }))
  fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: '  λ preserved  ' } })
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('storage unavailable') })
  fireEvent.click(screen.getByRole('button', { name: 'Send input' }))
  expect(await screen.findByRole('alert')).toHaveTextContent('could not retain the operation before sending')
  expect(socket.send).not.toHaveBeenCalled()
})
