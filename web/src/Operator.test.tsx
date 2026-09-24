import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { Operator } from './Operator'

const sockets: FakeSocket[] = []
class FakeSocket {
  static readonly OPEN = 1
  readonly readyState = 0
  readonly listeners = new Map<string, EventListener[]>()
  close = vi.fn()
  send = vi.fn()
  constructor(readonly url: string) { sockets.push(this) }
  addEventListener(type: string, listener: EventListener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener])
  }
}

function response(status: number, body?: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  } as Response
}

const fetchMock = vi.fn<typeof fetch>()
beforeEach(() => {
  sockets.length = 0
  fetchMock.mockReset()
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

  it('allows trusted-proxy authentication without login and logs out local cookie sessions', async () => {
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

  it('keeps trusted-proxy access authenticated when cookie logout is unavailable', async () => {
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
    expect(screen.queryByText('Loading authoritative harness snapshot…')).not.toBeInTheDocument()
    expect(screen.queryByText('/root/web_ui/web_ui')).not.toBeInTheDocument()
  })
})
