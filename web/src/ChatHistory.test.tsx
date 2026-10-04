import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import ChatHistory, { clearChatHistoryRetention } from './ChatHistory'
import { readHistoryPage, type HistoryPage } from './history-client'
vi.mock('./history-client', async original => ({ ...await original<typeof import('./history-client')>(), readHistoryPage: vi.fn() }))
function page(id: string, offset: string | number): HistoryPage {
  return { requestId: id, parentId: null, branch: 'untrusted path', nextOffset: String(offset) === '0' ? '1' : null, oversizedItem: null,
    items: [{ position: String(offset), hash: 'a'.repeat(64), byteLen: '100', item: { type: 'message', role: 'assistant', content: `${id} slice ${offset}` } }] }
}
function chat(key: string, ready = true) {
  return <ChatHistory cacheKey={key} requestId={key} requests={new Map()} refreshKey="stable" ready={ready} />
}
beforeEach(() => { clearChatHistoryRetention(); vi.clearAllMocks(); vi.mocked(readHistoryPage).mockImplementation(async (id, offset) => page(id, offset)) })
describe('bounded retained Chat slices', () => {
  it('publishes the newest response while a retained ancestor is still pending and preserves it if that read fails', async () => {
    let rejectAncestor!: (reason: unknown) => void
    vi.mocked(readHistoryPage).mockImplementation(async (id) => id === 'newest'
      ? {...page(id,0), parentId:'ancestor', nextOffset:null}
      : await new Promise((_, reject) => {rejectAncestor = reject}))
    render(<ChatHistory requestId="newest" requests={new Map()} refreshKey="1" ready />)
    await screen.findByText('newest slice 0')
    expect(screen.getByText('Loading conversation messages…')).toBeVisible()
    await act(async () => rejectAncestor(new Error('ancestor unavailable')))
    expect(screen.getByText('newest slice 0')).toBeVisible()
    expect(screen.getByText('ancestor unavailable')).toBeVisible()
    expect(screen.getByRole('status')).toHaveTextContent('Showing previously loaded messages. The requested messages could not be loaded.')
    vi.mocked(readHistoryPage).mockImplementation(async id => page(id, 0))
    fireEvent.click(screen.getByRole('button', { name: 'Retry messages' }))
    await waitFor(() => expect(screen.queryByText('Showing previously loaded messages. The requested messages could not be loaded.')).toBeNull())
    expect(screen.getByText('newest slice 0')).toBeVisible()
  })
  it('uses proven request lineage to refresh predecessor commits without restarting progressive loads', async () => {
    const oldOrigin = {kind:'embedded' as const, run:'run', actor:'/root', incarnation:'old'}
    vi.mocked(readHistoryPage).mockImplementation(async (id) => ({...page(id,0), parentId:id === 'newest' ? 'ancestor' : null, nextOffset:null}))
    const props = {requestId:'newest', requests:new Map(), refreshKey:'stable', ready:true}
    const mounted = render(<ChatHistory {...props} historyRevisions={[{origin:oldOrigin, requestId:'ancestor', version: '1'}]} />)
    await screen.findByText('ancestor slice 0')
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledTimes(2))
    mounted.rerender(<ChatHistory {...props} historyRevisions={[{origin:oldOrigin, requestId:'ancestor', version: '2'}]} />)
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledTimes(4))
    expect(screen.getByText('ancestor slice 0')).toBeVisible()
  })
  it('retains the exact older cursor across unmount and reconnect rather than replacing it with another worker', async () => {
    const mounted = render(chat('one'))
    await screen.findByText('one slice 0')
    fireEvent.click(screen.getByRole('button', { name: 'More items in this exchange' }))
    await screen.findByText('one slice 1')
    mounted.unmount()
    const another = render(chat('two'))
    await screen.findByText('two slice 0'); another.unmount()
    const restored = render(chat('one', false))
    expect(screen.getByText('one slice 1')).toBeVisible()
    expect(screen.queryByText('two slice 0')).toBeNull()
    expect(readHistoryPage).toHaveBeenCalledTimes(3)
    restored.rerender(chat('one'))
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledTimes(4))
    expect(readHistoryPage).toHaveBeenLastCalledWith('one', '1', expect.any(AbortSignal))
  })
  it('keeps the readable slice on a failed cursor read and retries that requested cursor', async () => {
    let fail = true
    vi.mocked(readHistoryPage).mockImplementation(async (id, offset) => {
      if (id === 'one' && offset === '1' && fail) throw new Error('page unavailable')
      return {...page(id, offset), nextOffset: BigInt(offset) < 2n ? (BigInt(offset) + 1n).toString() : null}
    })
    render(chat('one'))
    await screen.findByText('one slice 0')
    fireEvent.click(screen.getByRole('button', { name: 'More items in this exchange' }))
    await screen.findByText('page unavailable')
    expect(screen.getByText('one slice 0')).toBeVisible()
    expect(screen.getByText('Showing previously loaded messages. The requested messages could not be loaded.')).toBeVisible()
    fail = false
    fireEvent.click(screen.getByRole('button', { name: 'Retry messages' }))
    await screen.findByText('one slice 1')
    expect(screen.queryByText('Showing previously loaded messages. The requested messages could not be loaded.')).toBeNull()
    expect(readHistoryPage).toHaveBeenLastCalledWith('one', '1', expect.any(AbortSignal))
  })
  it('does not claim retained messages when the initial read fails', async () => {
    vi.mocked(readHistoryPage).mockRejectedValueOnce(new Error('first read unavailable'))
    render(chat('one'))
    await screen.findByText('first read unavailable')
    expect(screen.queryByText('Showing previously loaded messages. The requested messages could not be loaded.')).toBeNull()
    expect(screen.queryByRole('listitem')).toBeNull()
  })
  it('keeps the oversized item visible when its skip cursor fails and retries the skip cursor', async () => {
    let fail = true
    const oversized = { position: '0', hash: 'c'.repeat(64), byteLen: '400000', skipOffset: '1' }
    vi.mocked(readHistoryPage).mockImplementation(async (id, offset) => {
      if (id === 'one' && offset === '1' && fail) throw new Error('skip unavailable')
      return offset === '0' ? { requestId: id, parentId: null, branch: 'main', items: [], nextOffset: '0', oversizedItem: oversized }
        : page(id, offset)
    })
    render(chat('one'))
    const notice = await screen.findByText(new RegExp(oversized.hash))
    fireEvent.click(screen.getByRole('button', { name: 'Skip large item' }))
    await screen.findByText('skip unavailable')
    expect(notice).toBeVisible()
    expect(screen.queryByText('Showing previously loaded messages. The requested messages could not be loaded.')).toBeNull()
    fail = false
    fireEvent.click(screen.getByRole('button', { name: 'Retry messages' }))
    await screen.findByText('one slice 1')
    expect(readHistoryPage).toHaveBeenLastCalledWith('one', '1', expect.any(AbortSignal))
  })
  it('keeps a bounded cursor trail for moving back to newer pages', async () => {
    vi.mocked(readHistoryPage).mockImplementation(async (id, offset) => ({
      ...page(id, offset), nextOffset: BigInt(offset) < 39n ? (BigInt(offset) + 1n).toString() : null,
    }))
    render(chat('one'))
    await screen.findByText('one slice 0')
    for (let offset = 0; offset < 39; offset++) {
      fireEvent.click(screen.getByRole('button', { name: 'More items in this exchange' }))
      await screen.findByText(`one slice ${offset + 1}`)
    }
    for (let offset = 39; offset > 7; offset--) {
      fireEvent.click(screen.getByRole('button', { name: 'Newer messages' }))
      await screen.findByText(`one slice ${offset - 1}`)
    }
    expect(screen.queryByRole('button', { name: 'Newer messages' })).toBeNull()
    expect(screen.getByText('one slice 7')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Latest messages' }))
    await screen.findByText('one slice 0')
    expect(screen.queryByRole('button', { name: 'Newer messages' })).toBeNull()
  })
  it('shows the store supplied hash for an oversized item', async () => {
    const hash = 'b'.repeat(64)
    vi.mocked(readHistoryPage).mockResolvedValue({
      requestId: 'one', parentId: null, branch: 'main', items: [], nextOffset: '0',
      oversizedItem: { position: '0', hash, byteLen: '400000', skipOffset: '1' },
    })
    render(chat('one'))
    expect(await screen.findByText(new RegExp(hash))).toBeVisible()
  })
  it('keeps live assistant prose proportional and live tool input monospace', async () => {
    const origin = { kind: 'embedded' as const, run: 'run', actor: '/root', incarnation: 'one' }
    const liveOutput = [
      { origin, requestId: 'one', itemId: 'assistant', channel: 'assistant' as const, index: '0', text: 'Live answer', overflow: false, streaming: true, version: '1' },
      { origin, requestId: 'one', itemId: 'tool-input', channel: 'tool_arguments' as const, index: '0', text: '{"path":"README"}', overflow: false, streaming: true, version: '1' },
    ]
    render(<ChatHistory cacheKey="one" requestId="one" requests={new Map()} refreshKey="stable" ready liveOutput={liveOutput} />)
    await screen.findByText('one slice 0')
    expect(screen.getByRole('heading', { name: 'Assistant · streaming' })).toBeVisible()
    expect(screen.getByText('Live answer')).toHaveClass('history-content', 'history-prose')
    expect(screen.getByRole('heading', { name: 'Tool arguments · streaming' })).toBeVisible()
    expect(screen.getByText('{"path":"README"}')).toHaveClass('history-content', 'history-code')
  })
  it('evicts older worker slices after four retained conversations while keeping the recent slice readable offline', async () => {
    for (let index = 0; index < 5; index++) {
      const mounted = render(chat(String(index)))
      await screen.findByText(`${index} slice 0`); mounted.unmount()
    }
    const recent = render(chat('4', false))
    expect(screen.getByText('4 slice 0')).toBeVisible(); recent.unmount()
    render(chat('0', false))
    expect(screen.queryByText('0 slice 0')).toBeNull()
    expect(readHistoryPage).toHaveBeenCalledTimes(5)
  })
  it('retains a failure diagnostic with its exact cached request during projection loss', async () => {
    const request = { id: 'failed', state: 'failed', failure: { kind: 'http' as const, status: 400, diagnostic: { message: 'Invalid worker tools' } } }
    const mounted = render(<ChatHistory cacheKey="failed" requestId="failed" requests={new Map([['failed', request]])} refreshKey="1" ready />)
    await screen.findByText('failed slice 0'); mounted.unmount()
    render(chat('failed', false))
    expect(screen.getByRole('alert', { name: 'Failed exchange failed' })).toHaveTextContent('Invalid worker tools')
    expect(screen.getByText('failed slice 0')).toBeVisible()
  })
})
