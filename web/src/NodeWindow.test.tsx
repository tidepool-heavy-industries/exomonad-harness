import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import App from './App'

afterEach(() => vi.unstubAllGlobals())

describe('retained request inspection', () => {
  it('opens a protected page, shows exact Items, and requires an explicit oversized skip', async () => {
    const pages = [
      { requestId: 'request-1', parentId: 'parent-1', branch: '/root',
        items: [{ position: 0, hash: 'hash-0', byteLen: 91,
          item: { type: 'message', role: 'assistant', content: 'model text' } }],
        nextOffset: 1,
        oversizedItem: { position: 1, hash: 'hash-large', byteLen: 270000, skipOffset: 2 } },
      { requestId: 'request-1', parentId: 'parent-1', branch: '/root',
        items: [{ position: 2, hash: 'hash-2', byteLen: 101,
          item: { type: 'custom_tool_call_output', call_id: 'raw-call', output: 'retained result' } }],
        nextOffset: null, oversizedItem: null },
    ]
    const fetchMock = vi.fn()
      .mockResolvedValueOnce({ ok: true, status: 200, json: async () => pages[0] })
      .mockResolvedValueOnce({ ok: true, status: 200, json: async () => pages[1] })
    vi.stubGlobal('fetch', fetchMock)
    render(<App data={{
      nodes: [{ id: 'root', name: '/root', state: 'idle' }],
      timeline: [{ id: 'request-1', nodeId: 'root', label: 'Response request', kind: 'request', state: 'completed' }],
      inbox: [],
    }} />)
    fireEvent.click(screen.getByRole('button', { name: /timeline/i }))
    fireEvent.click(screen.getByRole('button', { name: 'Inspect history' }))
    await waitFor(() => expect(screen.getByRole('list', { name: 'Retained request items' }).textContent).toContain('model text'))
    expect(fetchMock).toHaveBeenCalledWith('/api/history/request-1?offset=0&limit=50', {
      credentials: 'same-origin', cache: 'no-store',
    })
    expect(screen.getByText(/hash-large/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Skip this item' }))
    await waitFor(() => expect(screen.getByRole('list', { name: 'Retained request items' }).textContent).toContain('retained result'))
    expect(fetchMock).toHaveBeenCalledWith('/api/history/request-1?offset=2&limit=50', {
      credentials: 'same-origin', cache: 'no-store',
    })
    expect(screen.getByRole('list', { name: 'Retained request items' }).textContent).toContain('model text')
  })
})
