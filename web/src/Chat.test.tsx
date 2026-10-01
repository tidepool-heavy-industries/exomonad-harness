import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import { clearDrafts } from './drafts'
import { clearWorkerChatRetention } from './WorkerChat'
import { HistoryReadError, type HistoryPage } from './history-client'
import { readHistoryPage } from './history-client'
import type { HarnessViewModel } from './view-model'
import type { BrowserCommandRecord } from './pending-commands'
import { isSnapshot, normalizeSnapshot, type Snapshot } from './protocol'
import { toViewModel } from './integration'
vi.mock('./history-client', async importOriginal => ({ ...await importOriginal<typeof import('./history-client')>(), readHistoryPage: vi.fn() }))
const target = { run: 'run', actor: '/root', incarnation: 'original' }
const actor = { id: JSON.stringify(['run', '/root', 'original']), name: '/root', run: 'run', incarnation: 'original', kind: 'model' as const, parentIdentity: null, lifecycle: 'running', modelConversation: 'root-conv', activeRound: 'round' }
const data: HarnessViewModel = { hostRun: 'run', actors: [actor], nodes: [{ id: 'root-conv', name: '/root', state: 'active' }], timeline: [{ id: 'first', nodeId: 'root-conv', parentId: null, label: 'Earlier request', kind: 'request', state: 'completed', historyRefreshKey: 'first:1' }, { id: 'latest', nodeId: 'root-conv', parentId: 'first', label: 'Latest request', kind: 'request', state: 'completed', historyRefreshKey: 'latest:1' }, { id: 'child', nodeId: 'child-conv', parentId: 'first', label: 'Unrelated child', kind: 'request', state: 'completed' }], inbox: [] }
function page(id: string, parentId: string | null, items: unknown[], nextOffset: number | null = null): HistoryPage { return { requestId: id, parentId, branch: '/root', nextOffset, oversizedItem: null, items: items.map((item, position) => ({ position, hash: 'a'.repeat(64), byteLen: 100, item })) } }
const histories = {
  first: page('first', null, [{ type: 'message', role: 'user', content: [{ type: 'input_text', text: 'Original question' }] }, { type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'Earlier answer' }] }]),
  latest: page('latest', 'first', [{ type: 'function_call', name: 'read_file', call_id: 'call-1', arguments: '{"path":"README"}' }, { type: 'function_call_output', call_id: 'call-1', output: 'File contents' }, { type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'Latest answer' }] }]),
}
beforeEach(() => {
  window.history.replaceState(null, '', '/chat/root?run=run&incarnation=original')
  sessionStorage.clear(); clearDrafts(); clearWorkerChatRetention()
  vi.mocked(readHistoryPage).mockImplementation(async id => histories[id as keyof typeof histories] ?? page(id, null, []))
})
afterEach(() => { vi.clearAllMocks(); clearDrafts(); clearWorkerChatRetention() })
describe('embedded actor chat', () => {
  it('renders the failed request wire diagnostic separately and admits a new reply without changing retained history', async () => {
    const failure = { kind: 'http' as const, status: 400, diagnostic: { code: 'invalid_parameter', error_type: 'invalid_request_error', param: 'tools[0]', message: '<b>Tool schema rejected</b>' } }
    const snapshot: Snapshot = { seq: 1, hostRun: 'run', actors: [{ identity: target, parent: null, kind: 'model', lifecycle: 'running', modelConversation: 'root-conv' }], conversations: [{ id: 'root-conv', path: '/root', state: 'idle' }], requests: [
      { id: 'first', conversationId: 'root-conv', parentId: null, state: 'completed', failure: null },
      { id: 'latest', conversationId: 'root-conv', parentId: 'first', state: 'failed', failure },
      { id: 'child', conversationId: 'child-conv', state: 'failed', failure: { kind: 'http', status: 500, diagnostic: { message: 'Unrelated child failure' } } },
    ], jobs: [], envelopes: [] }
    expect(isSnapshot(snapshot)).toBe(true)
    const failedPage = page('latest', 'first', [])
    const originalHistory = JSON.stringify([failedPage, histories.first])
    vi.mocked(readHistoryPage).mockImplementation(async id => id === 'latest' ? failedPage : histories[id as keyof typeof histories]!)
    const submit = vi.fn().mockReturnValue({ kind: 'retained', operationId: 'next-operation', send: 'sent' })
    const commandId = '00000000-0000-4000-8000-000000000001'
    const admitted: BrowserCommandRecord = { hostRun: 'run', authority: 'receipt', state: 'input_admitted', receipt: { commandId, target, outcome: 'admitted', envelopeId: '1' }, submission: { operation_id: commandId, command: { action: 'input', target, text: 'Original question' } } }
    const mounted = render(<App data={toViewModel(normalizeSnapshot(snapshot))} onHostCommand={submit} pendingCommands={[admitted]} />)
    await screen.findByText('Original question')
    const notice = screen.getByRole('alert', { name: 'Failed exchange latest' })
    expect(notice).toHaveTextContent('Provider returned HTTP 400.')
    expect(notice).toHaveTextContent('invalid_request_error')
    expect(notice).toHaveTextContent('invalid_parameter')
    expect(notice).toHaveTextContent('tools[0]')
    expect(within(notice).getByText('<b>Tool schema rejected</b>')).toBeVisible()
    expect(notice.querySelector('b')).toBeNull()
    expect(screen.queryByText('Unrelated child failure')).toBeNull()
    expect(within(screen.getByRole('list', { name: 'Retained conversation items' })).getAllByRole('listitem')).toHaveLength(histories.first.items.length)
    expect(screen.getByRole('region', { name: 'Retained browser operations' })).toHaveTextContent('input_admitted')
    expect(screen.getByRole('region', { name: 'Retained browser operations' })).not.toHaveTextContent('refused')
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: 'Try again with the corrected schema' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send input' }))
    expect(submit).toHaveBeenCalledWith({ action: 'input', target, text: 'Try again with the corrected schema' })
    expect(screen.getByRole('alert', { name: 'Failed exchange latest' })).toBeVisible()
    expect(screen.queryByText('Try again with the corrected schema')).toBeNull()
    const nextPage = page('next', 'latest', [{ type: 'message', role: 'assistant', content: 'Recovered answer' }])
    vi.mocked(readHistoryPage).mockImplementation(async id => id === 'next' ? nextPage : id === 'latest' ? failedPage : histories[id as keyof typeof histories]!)
    const nextSnapshot: Snapshot = { ...snapshot, seq: 2, requests: [...snapshot.requests, { id: 'next', conversationId: 'root-conv', parentId: 'latest', state: 'completed', failure: null }] }
    mounted.rerender(<App data={toViewModel(normalizeSnapshot(nextSnapshot))} onHostCommand={submit} />)
    await screen.findByText('Recovered answer')
    expect(screen.getByRole('alert', { name: 'Failed exchange latest' })).toBeVisible()
    expect(screen.queryByRole('alert', { name: 'Failed exchange next' })).toBeNull()
    expect(JSON.stringify([failedPage, histories.first])).toBe(originalHistory)
    const unavailable: Snapshot = { ...nextSnapshot, actors: [{ ...snapshot.actors![0]!, lifecycle: 'lost', modelConversation: null }], requests: [] }
    mounted.rerender(<App data={toViewModel(normalizeSnapshot(unavailable))} transportPhase="disconnected" onHostCommand={submit} />)
    expect(screen.getByRole('alert', { name: 'Failed exchange latest' })).toBeVisible()
    expect(screen.getByText('Recovered answer')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Send input' })).toBeDisabled()
    mounted.rerender(<App data={toViewModel(normalizeSnapshot({ ...unavailable, actors: [] }))} onHostCommand={submit} />)
    expect(screen.getByRole('alert', { name: 'Failed exchange latest' })).toBeVisible()
  })
  it('shows authentication failure before its empty history resolves and matches failures while paging older requests', async () => {
    let resolvePage: ((value: HistoryPage) => void) | undefined
    vi.mocked(readHistoryPage).mockImplementationOnce(() => new Promise(resolve => { resolvePage = resolve }))
    const failed = { ...data, timeline: data.timeline.map(request => request.id === 'latest' ? { ...request, state: 'failed', failure: { kind: 'authentication' as const } } : request) }
    render(<App data={failed} onHostCommand={vi.fn()} />)
    expect(screen.getByRole('alert', { name: 'Failed exchange latest' })).toHaveTextContent(/Provider authentication failed/)
    expect(screen.getByRole('alert', { name: 'Failed exchange latest' })).toHaveTextContent(/Check the host's provider credentials/)
    await act(async () => { resolvePage?.(page('latest', 'first', [], 1)) })
    fireEvent.click(screen.getByRole('button', { name: 'Earlier exchanges' }))
    await screen.findByText('Original question')
    expect(screen.queryByRole('alert', { name: 'Failed exchange latest' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Latest messages' }))
    await screen.findByText('Latest answer')
    expect(screen.getByRole('alert', { name: 'Failed exchange latest' })).toBeVisible()
  })
  it('matches an earlier failed exchange by its fetched parent ID without attributing it to the successful head', async () => {
    const olderFailure = { ...data, timeline: data.timeline.map(request => request.id === 'first' ? { ...request, nodeId: 'ancestor-conv', state: 'failed', failure: { kind: 'http' as const, status: 503, diagnostic: { message: 'Earlier provider failure' } } } : request) }
    vi.mocked(readHistoryPage).mockImplementation(async id => id === 'latest' ? page('latest', 'first', [{ type: 'message', role: 'assistant', content: 'Successful newer exchange' }], 1) : histories.first)
    render(<App data={olderFailure} />)
    await screen.findByText('Successful newer exchange')
    expect(screen.queryByRole('alert', { name: 'Failed exchange first' })).toBeNull()
    expect(screen.queryByRole('alert', { name: 'Failed exchange latest' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Earlier exchanges' }))
    await screen.findByText('Original question')
    expect(screen.getByRole('alert', { name: 'Failed exchange first' })).toHaveTextContent('Earlier provider failure')
    expect(screen.queryByRole('alert', { name: 'Failed exchange latest' })).toBeNull()
  })
  it('opens the exact root link with inherited messages, tool inputs and output beside its composer', async () => {
    render(<App data={data} />)
    expect(screen.getByRole('heading', { name: 'Chat', level: 1 })).toBeVisible()
    await screen.findByText('Original question')
    expect(screen.getByText('Latest answer')).toBeVisible()
    expect(screen.getByText('{"path":"README"}')).toBeVisible()
    expect(screen.getByText('File contents')).toBeVisible()
    expect(screen.getByLabelText('Message to selected actor')).toBeVisible()
    expect(window.location.search).toContain('incarnation=original')
    expect(vi.mocked(readHistoryPage).mock.calls.map(call => call[0])).toEqual(['latest', 'first'])
    expect(screen.queryByText('Unrelated child')).toBeNull()
  })
  it('preserves exact drafts, keeps pending operations separate and sends interrupt to the observed round', async () => {
    const submit = vi.fn().mockReturnValueOnce({ kind: 'blocked', reason: 'Cannot retain operation' }).mockReturnValue({ kind: 'retained', operationId: 'operation', send: 'unknown' })
    const pending: BrowserCommandRecord = { hostRun: 'run', authority: 'local', state: 'queued', send: 'unknown', submission: { operation_id: '00000000-0000-4000-8000-000000000001', command: { action: 'input', target, text: 'Awaiting host admission' } } }
    const mounted = render(<App data={data} onHostCommand={submit} />)
    await screen.findByText('Original question')
    const input = screen.getByLabelText('Message to selected actor')
    fireEvent.change(input, { target: { value: ' exact\n reply ' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send input' }))
    expect(input).toHaveValue(' exact\n reply ')
    fireEvent.click(screen.getByRole('button', { name: 'Send input' }))
    expect(submit).toHaveBeenLastCalledWith({ action: 'input', target, text: ' exact\n reply ' })
    expect(input).toHaveValue('')
    mounted.rerender(<App data={data} onHostCommand={submit} pendingCommands={[pending]} />)
    expect(screen.getByRole('region', { name: 'Retained browser operations' })).toHaveTextContent('Awaiting host admission')
    expect(screen.getByRole('list', { name: 'Retained conversation items' })).not.toHaveTextContent('Awaiting host admission')
    fireEvent.click(screen.getByRole('button', { name: 'Interrupt' }))
    expect(submit).toHaveBeenLastCalledWith({ action: 'interrupt', target, expected_round: 'round' })
  })
  it('retains conversation and draft on disconnect and missing actor, never selects a replacement incarnation', async () => {
    const submit = vi.fn()
    const mounted = render(<App data={data} onHostCommand={submit} />)
    await screen.findByText('Latest answer')
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: 'Keep my draft' } })
    mounted.rerender(<App data={{ ...data, actors: [{ ...actor, lifecycle: 'lost', modelConversation: undefined }] }} onHostCommand={submit} />)
    expect(screen.getByText('Latest answer')).toBeVisible()
    expect(screen.getByLabelText('Message to selected actor')).toHaveValue('Keep my draft')
    expect(screen.getByText(/This actor is lost/)).toBeVisible()
    expect(screen.getByText(/Its Chat is read-only; retained history remains available/)).toBeVisible()
    expect(screen.getByRole('button', { name: 'Send input' })).toBeDisabled()
    const replacement = { ...actor, id: JSON.stringify(['run', '/root', 'replacement']), incarnation: 'replacement' }
    mounted.rerender(<App data={{ ...data, actors: [replacement] }} onHostCommand={submit} />)
    expect(screen.getByText('Latest answer')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Send input' })).toBeDisabled()
    expect(window.location.search).toContain('incarnation=original')
    expect(submit).not.toHaveBeenCalled()
  })
  it('retains fetched messages while a new exchange loads, aborts on disconnect and recovers to the new head', async () => {
    const mounted = render(<App data={data} />)
    await screen.findByText('Latest answer')
    let resolveNext: ((page: HistoryPage) => void) | undefined
    vi.mocked(readHistoryPage).mockImplementationOnce(() => new Promise(resolve => { resolveNext = resolve }))
    const next = { ...data, timeline: [...data.timeline, { ...data.timeline[1]!, id: 'next', parentId: 'latest' }] }
    mounted.rerender(<App data={next} />)
    await waitFor(() => expect(readHistoryPage).toHaveBeenLastCalledWith('next', 0, expect.any(AbortSignal)))
    expect(screen.getByText('Latest answer')).toBeVisible()
    const pendingSignal = vi.mocked(readHistoryPage).mock.calls.at(-1)![2]
    mounted.rerender(<App data={next} transportPhase="disconnected" />)
    expect(pendingSignal.aborted).toBe(true)
    await act(async () => { resolveNext?.(page('next', 'latest', [{ type: 'message', role: 'assistant', content: 'Stale response' }])) })
    expect(screen.getByText('Latest answer')).toBeVisible()
    expect(screen.queryByText('Stale response')).toBeNull()
    vi.mocked(readHistoryPage).mockImplementation(async id => id === 'next'
      ? page('next', 'latest', [{ type: 'message', role: 'assistant', content: 'New answer' }])
      : histories[id as keyof typeof histories]!)
    mounted.rerender(<App data={next} />)
    await screen.findByText('New answer')
    expect(screen.getByText('Original question')).toBeVisible()
    expect(screen.getByText('Latest answer')).toBeVisible()
  })
  it('keeps the fetched slice on a failed refresh and retries its exact request', async () => {
    render(<App data={data} />)
    await screen.findByText('Latest answer')
    vi.mocked(readHistoryPage).mockRejectedValueOnce(new HistoryReadError('Host history unavailable'))
    fireEvent.click(screen.getByRole('button', { name: 'Refresh messages' }))
    await screen.findByText('Host history unavailable')
    expect(screen.getByText('Original question')).toBeVisible()
    expect(screen.getByText('Latest answer')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Retry messages' }))
    await waitFor(() => expect(screen.queryByText('Host history unavailable')).toBeNull())
    expect(readHistoryPage).toHaveBeenLastCalledWith('first', 0, expect.any(AbortSignal))
  })
  it('resets history and actor drafts on an explicit identity switch and aborts stale reads', async () => {
    const replacement = { ...actor, id: JSON.stringify(['run', '/other', 'two']), name: '/other', incarnation: 'two', parentIdentity: target, modelConversation: 'other-conv' }
    const both = { ...data, actors: [actor, replacement], timeline: [...data.timeline, { ...data.timeline[1]!, id: 'other-head', nodeId: 'other-conv', parentId: null }] }
    let resolveOld: ((page: HistoryPage) => void) | undefined
    vi.mocked(readHistoryPage).mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve }))
    render(<App data={both} />)
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalled())
    const oldSignal = vi.mocked(readHistoryPage).mock.calls[0]![2]
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: 'Root draft' } })
    fireEvent.click(screen.getByRole('link', { name: replacement.name }))
    expect(oldSignal.aborted).toBe(true)
    expect(screen.getByLabelText('Message to selected actor')).toHaveValue('')
    await act(async () => { resolveOld?.(histories.latest) })
    expect(screen.queryByText('Latest answer')).toBeNull()
    fireEvent.click(screen.getByRole('link', { name: actor.name }))
    expect(screen.getByLabelText('Message to selected actor')).toHaveValue('Root draft')
    await screen.findByText('Latest answer')
  })
  it('refreshes after durable evidence and reconnect, and offers explicit bounded older exchanges', async () => {
    let calls = 0
    vi.mocked(readHistoryPage).mockImplementation(async id => { calls++; return page(id, `parent-${id}`, [{ type: 'message', role: 'assistant', content: `answer ${id} read ${calls}` }]) })
    const mounted = render(<App data={data} />)
    await screen.findByText('answer latest read 1')
    expect(readHistoryPage).toHaveBeenCalledTimes(8)
    fireEvent.click(screen.getByRole('button', { name: 'Earlier exchanges' }))
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledTimes(16))
    expect(screen.queryByText('answer latest read 1')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Latest messages' }))
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledTimes(24))
    mounted.rerender(<App data={data} transportPhase="disconnected" />)
    mounted.rerender(<App data={{ ...data, timeline: data.timeline.map(row => ({ ...row, historyRefreshKey: 'changed' })) }} />)
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledTimes(32))
  })
  it('pages an exchange only on demand, exposes oversized-item skips and returns to latest messages', async () => {
    vi.mocked(readHistoryPage).mockImplementation(async (id, offset) => {
      if (id === 'first') return histories.first
      if (offset === 0) return page('latest', 'first', [{ type: 'message', role: 'assistant', content: 'First slice' }], 1)
      if (offset === 1) return { ...page('latest', 'first', [], 1), oversizedItem: { position: 1, hash: 'a'.repeat(64), byteLen: 300_000, skipOffset: 2 } }
      return page('latest', 'first', [{ type: 'message', role: 'assistant', content: 'After oversized item' }])
    })
    render(<App data={data} />)
    await screen.findByText('First slice')
    expect(readHistoryPage).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: 'More items in this exchange' }))
    await screen.findByText(/too large to display/)
    expect(readHistoryPage).toHaveBeenLastCalledWith('latest', 1, expect.any(AbortSignal))
    fireEvent.click(screen.getByRole('button', { name: 'Skip large item' }))
    await screen.findByText('After oversized item')
    expect(readHistoryPage).toHaveBeenLastCalledWith('latest', 2, expect.any(AbortSignal))
    fireEvent.click(screen.getByRole('button', { name: 'Earlier exchanges' }))
    await screen.findByText('Original question')
    fireEvent.click(screen.getByRole('button', { name: 'Latest messages' }))
    await screen.findByText('First slice')
    expect(readHistoryPage).toHaveBeenLastCalledWith('latest', 0, expect.any(AbortSignal))
  })
  it.each([
    { budget: 'item', itemsPerPage: 50, byteLen: 100, reads: 5, visible: 200 },
    { budget: 'byte', itemsPerPage: 1, byteLen: 200_000, reads: 2, visible: 1 },
  ])('bounds the fetched display by its $budget budget and keeps earlier history reachable', async ({ itemsPerPage, byteLen, reads, visible }) => {
    vi.mocked(readHistoryPage).mockImplementation(async id => {
      const result = page(id, `parent-${id}`, Array.from({ length: itemsPerPage }, (_, i) => ({ type: 'message', role: 'assistant', content: `${id} item ${i}` })))
      return { ...result, items: result.items.map(entry => ({ ...entry, byteLen })) }
    })
    render(<App data={data} />)
    await waitFor(() => expect(within(screen.getByRole('list', { name: 'Retained conversation items' })).getAllByRole('listitem')).toHaveLength(visible))
    expect(readHistoryPage).toHaveBeenCalledTimes(reads)
    expect(screen.getByRole('button', { name: 'Earlier exchanges' })).toBeEnabled()
  })
  it('reports cyclic parent history and retains the already fetched messages', async () => {
    vi.mocked(readHistoryPage).mockImplementation(async id => ({ ...histories[id as keyof typeof histories]!, parentId: id === 'latest' ? 'first' : 'latest' }))
    render(<App data={data} />)
    await screen.findByText('Retained history has a cyclic parent link.')
    expect(screen.getByText('Original question')).toBeVisible()
    expect(screen.getByText('Latest answer')).toBeVisible()
    expect(readHistoryPage).toHaveBeenCalledTimes(2)
  })
  it('reports protected history failure with retry and forwards authentication expiry', async () => {
    const expired = vi.fn()
    vi.mocked(readHistoryPage).mockRejectedValueOnce(new HistoryReadError('Sign in again', 'authentication'))
    render(<App data={data} onAuthExpired={expired} />)
    await screen.findByText('Sign in again')
    expect(expired).toHaveBeenCalledOnce()
    fireEvent.click(screen.getByRole('button', { name: 'Retry messages' }))
    await screen.findByText('Original question')
  })
  it('does not infer roots from labels or follow ambiguous roots, and preserves explicit admin and demo views', () => {
    window.history.replaceState(null, '', '/chat')
    const mounted = render(<App data={{ ...data, actors: [actor, { ...actor, id: 'other', incarnation: 'two' }] }} />)
    expect(screen.getByRole('button', { name: 'Send input' })).toBeDisabled()
    expect(window.location.search).not.toContain('incarnation=')
    mounted.unmount()
    window.history.replaceState(null, '', '/?view=host')
    const host = render(<App data={data} />)
    expect(screen.getByRole('heading', { name: 'Host', level: 1 })).toBeVisible()
    host.unmount()
    window.history.replaceState(null, '', '/')
    render(<App data={{ ...data, hostRun: undefined }} />)
    expect(screen.getByRole('heading', { name: 'Tree', level: 1 })).toBeVisible()
    expect(screen.queryByRole('link', { name: 'Chat' })).toBeNull()
  })
  it('preserves incomplete exact actor links until an explicit selection is made', () => {
    window.history.replaceState(null, '', '/?run=run&actor=/root')
    render(<App data={data} />)
    expect(screen.getByText(/invalid or incomplete selection/)).toBeVisible()
    expect(window.location.search).not.toContain('incarnation=')
    expect(readHistoryPage).not.toHaveBeenCalled()
  })
})
