import { createRef } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import UnifiedChat, { decodeUnifiedPage } from './UnifiedChat'
import { clearMountedFormDrafts } from './MountedForm'

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); clearMountedFormDrafts() })

describe('unified conversation pages', () => {
  const origin = { run: 'r', nativeActor: '3', incarnation: '1' }
  it('clears the pending form indicator from the HTTP response and does not regress on a stale history refresh', async () => {
    const form = { sequence: '1', revisionSequence: '1', opening: { origin, mountId: 'response', execution: { kind: 'actor_program' }, conversation: null,
      form: { version: 1, root: { kind: 'text', id: 'name', label: 'Project', initial: null } } }, state: 'open', attemptId: null, draft: null, errors: [], answer: null }
    const page = { origin, entries: [{ sequence: '1', kind: 'form', form }], cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: null }
    vi.stubGlobal('fetch', vi.fn(async (url: string) => new Response(JSON.stringify(url.includes('/submit')
      ? { ...form, revisionSequence: '2', state: 'submitted', draft: { name: 'Harness' } } : page))))
    render(<UnifiedChat origin={origin} ready active />)
    await screen.findByRole('navigation', { name: 'Pending responses' })
    fireEvent.change(screen.getByLabelText('Project'), { target: { value: 'Harness' } })
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(screen.queryByRole('navigation', { name: 'Pending responses' })).toBeNull())
    expect(screen.getByText('Harness')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Refresh conversation' }))
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(3))
    expect(screen.queryByRole('button', { name: 'Submit' })).toBeNull()
    expect(screen.queryByRole('navigation', { name: 'Pending responses' })).toBeNull()
  })

  it('keeps tool input visible and execution disclosure open when a preview becomes retained history', async () => {
    const modelOrigin = { kind: 'embedded' as const, run: 'r', actor: '/worker', incarnation: 'one' }
    const live = { origin: modelOrigin, requestId: 'req', itemId: 'cell', channel: 'tool_input' as const, index: '0', text: 'observe value', streaming: true, version: '1', overflow: false, committedHash: null }
    let entries: unknown[] = []
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ origin, entries, cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: null }))))
    const mounted = render(<UnifiedChat origin={origin} modelOrigin={modelOrigin} ready active={false} liveOutput={[live]} />)
    const source = await screen.findByText('observe value')
    expect(source).not.toBeVisible()
    const group = screen.getByLabelText('Tool activity group') as HTMLDetailsElement
    group.open = true; fireEvent(group, new Event('toggle'))
    expect(source).toBeVisible()
    const disclosure = screen.getByText('Tool evidence').closest('details')!
    disclosure.open = true; fireEvent(disclosure, new Event('toggle'))
    const hash = 'a'.repeat(64)
    entries = [{ sequence: '2', kind: 'message', requestId: 'req', position: '0', hash, providerItemId: 'cell', inputOperation: null,
      tool: { operation: { origin: modelOrigin, request: 'req', call: 'call' }, invocation: { request: 'req', position: '0', hash } },
      item: { type: 'custom_tool_call', id: 'cell', name: 'haskell', call_id: 'call', input: 'observe value' } }]
    mounted.rerender(<UnifiedChat origin={origin} revision="2" modelOrigin={modelOrigin} ready active={false} liveOutput={[live]} />)
    await screen.findByRole('heading', { name: 'haskell' })
    expect(screen.getByLabelText('Input')).toBeVisible()
    expect(screen.getByLabelText('Input').textContent).toBe('observe value')
    expect(screen.getByText('Tool evidence').closest('details')).toHaveAttribute('open')
    expect(screen.getByText('No loaded result')).toBeVisible()
    expect(screen.queryByText('Running')).toBeNull()
    const jobs = [
      { id: 'unrelated', nodeId: 'worker', kind: 'job' as const, label: 'job', state: 'running', requestId: 'other-request', callId: 'call' },
      { id: 'owned', nodeId: 'worker', kind: 'job' as const, label: 'job', state: 'settled', requestId: 'req', callId: 'call', startedAtMs: 1000, endedAtMs: 2500 },
    ]
    mounted.rerender(<UnifiedChat origin={origin} revision="2" modelOrigin={modelOrigin} ready active={false} liveOutput={[live]} jobs={jobs} />)
    expect(screen.getByText(/Host duration: 1.5 s/)).toBeVisible()
    expect(screen.queryByText(/Tool running/)).toBeNull()
  })
  it('retains group and row disclosures when older adjacent tools are prepended', async () => {
    const height = vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(312)
    const scroll = vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(600)
    const modelOrigin = { kind: 'embedded' as const, run: 'r', actor: '/worker', incarnation: 'one' }
    const tool = (sequence: string) => ({ sequence, kind: 'message', requestId: 'req', position: sequence, hash: 'a'.repeat(64), providerItemId: null, inputOperation: null,
      tool: { operation: { origin: modelOrigin, request: 'req', call: sequence }, invocation: { request: 'req', position: sequence, hash: 'a'.repeat(64) } },
      item: { type: 'custom_tool_call', name: 'haskell', call_id: sequence, input: `observe value${sequence}` } })
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const older = new URL(String(input), 'http://localhost').searchParams.has('before')
      return new Response(JSON.stringify({ origin, entries: older ? [tool('1')] : [tool('2'), { ...tool('2'), sequence: '3', item: { type: 'custom_tool_call_output', call_id: '2', output: 'Long retained result\n'.repeat(100) } }], cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: older ? null : '2' }))
    }))
    render(<UnifiedChat origin={origin} modelOrigin={modelOrigin} ready active={false} />)
    await screen.findByRole('heading', { name: 'haskell' })
    const group = screen.getByLabelText('Tool activity group') as HTMLDetailsElement
    group.open = true; fireEvent(group, new Event('toggle'))
    const evidence = screen.getByText('Tool evidence').closest('details')!
    evidence.open = true; fireEvent(evidence, new Event('toggle'))
    fireEvent.click(screen.getByRole('button', { name: 'Expand Result 3' }))
    fireEvent.click(screen.getByRole('button', { name: 'Load older messages' }))
    await waitFor(() => expect(screen.getAllByRole('heading', { name: 'haskell' })).toHaveLength(2))
    expect(screen.getByLabelText('Tool activity group')).toHaveAttribute('open')
    const retained = screen.getAllByText('Tool evidence')[1]!.closest('details')!
    expect(retained).toHaveAttribute('open')
    expect(screen.getAllByLabelText('Input')[1]).toBeVisible()
    expect(screen.getByRole('button', { name: 'Collapse Result 3' })).toHaveAttribute('aria-expanded', 'true')
    height.mockRestore(); scroll.mockRestore()
  })
  it('preserves server sequence order and rejects timestamp shaped alternatives', () => {
    const page = { origin, cutoverSequence: '4', legacyHistory: true, entries: [
      { sequence: '4', kind: 'message', requestId: 'req', position: '0', hash: 'a'.repeat(64), tool: null, providerItemId: null, inputOperation: null, item: { type: 'message' } },
      { sequence: '5', kind: 'output', output: { reference: { origin, sequence: '5' }, emission: { origin, id: { displaySlot: '1', pageOrdinal: '1' }, execution: { kind: 'actor_program' }, conversation: null, page: { text: 'value', expansions: [], unavailable: false } } } },
    ], nextAfter: null, previousBefore: null }
    expect(decodeUnifiedPage(page, origin, '0').entries.map(entry => entry.sequence)).toEqual(['4', '5'])
    expect(() => decodeUnifiedPage({ ...page, entries: [...page.entries].reverse() }, origin, '0')).toThrow(/sequence/)
    expect(decodeUnifiedPage({ ...page, nextAfter: '5' }, origin, '0').nextAfter).toBe('5')
    expect(() => decodeUnifiedPage({ ...page, nextAfter: '6' }, origin, '0')).toThrow(/cursor/)
  })
  it('starts at latest and prepends older entries using the exact server cursor', async () => {
    const latest = { origin, cutoverSequence: '0', legacyHistory: false, entries: [
      { sequence: '20', kind: 'message', requestId: 'req', position: '1', hash: '2'.repeat(64), tool: null, providerItemId: null, inputOperation: null, item: { type: 'message', role: 'assistant', content: 'Latest page' } },
    ], nextAfter: null, previousBefore: '20' }
    const older = { origin, cutoverSequence: '0', legacyHistory: false, entries: [
      { sequence: '10', kind: 'message', requestId: 'req', position: '0', hash: '1'.repeat(64), tool: null, providerItemId: null, inputOperation: null, item: { type: 'message', role: 'assistant', content: 'Older page' } },
    ], nextAfter: null, previousBefore: null }
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const before = new URL(String(input), 'http://localhost').searchParams.get('before')
      return new Response(JSON.stringify(before === '20' ? older : latest), { status: 200 })
    })
    vi.stubGlobal('fetch', fetchMock)
    render(<UnifiedChat origin={origin} ready active={false} />)
    await screen.findByText('Latest page')
    expect(new URL(String(fetchMock.mock.calls[0]![0]), 'http://localhost').searchParams.has('after')).toBe(false)
    expect(fetchMock).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: 'Load older messages' }))
    await screen.findByText('Older page')
    expect(screen.getByText('Latest page')).toBeVisible()
    expect(fetchMock.mock.calls.some(([input]) => new URL(String(input), 'http://localhost').searchParams.get('before') === '20')).toBe(true)
    expect([...screen.getByRole('list', { name: 'Conversation entries' }).children].map(item => item.getAttribute('data-sequence'))).toEqual(['10', '20'])
  })
  it('rejects backward cursors outside the returned page or entries crossing the boundary', () => {
    const page = { origin, cutoverSequence: '0', legacyHistory: false, entries: [
      { sequence: '10', kind: 'message', requestId: 'req', position: '0', hash: 'a'.repeat(64), tool: null, providerItemId: null, inputOperation: null, item: { type: 'message' } },
    ], nextAfter: null, previousBefore: '10' }
    expect(decodeUnifiedPage(page, origin, { before: '20' }).previousBefore).toBe('10')
    expect(() => decodeUnifiedPage({ ...page, previousBefore: '9' }, origin, { before: '20' })).toThrow(/cursor/)
    expect(() => decodeUnifiedPage(page, origin, { before: '10' })).toThrow(/sequence/)
    expect(() => decodeUnifiedPage({ ...page, previousBefore: null, origin: { ...origin, incarnation: '2' } }, origin, {})).toThrow(/invalid page/)
  })
  it('retains exact before cursors above 2^53 and refuses numeric wire substitutes', () => {
    const page = { origin, cutoverSequence: '0', legacyHistory: false, entries: [
      { sequence: '9007199254740993', kind: 'message', requestId: 'req', position: '0', hash: 'a'.repeat(64), item: { type: 'message' }, tool: null, providerItemId: null, inputOperation: null },
    ], nextAfter: null, previousBefore: '9007199254740993' }
    expect(decodeUnifiedPage(page, origin, { before: '9007199254740994' }).previousBefore).toBe('9007199254740993')
    expect(() => decodeUnifiedPage({ ...page, previousBefore: 9007199254740992 }, origin, {})).toThrow(/invalid page/)
    expect(() => decodeUnifiedPage({ ...page, previousBefore: undefined }, origin, '0')).toThrow(/invalid page/)
    expect(() => decodeUnifiedPage({ ...page, nextAfter: undefined }, origin, '0')).toThrow(/invalid page/)
  })
  it('rejects an inconsistent provider identity while retaining historical unknown provenance', () => {
    const entry = { sequence: '1', kind: 'message', requestId: 'request', position: '0', hash: 'a'.repeat(64), tool: null, inputOperation: null, providerItemId: null, item: { type: 'message', id: 'raw-id' } }
    const page = { origin, cutoverSequence: '0', legacyHistory: false, entries: [entry], nextAfter: null, previousBefore: null }
    expect(decodeUnifiedPage(page, origin, {}).entries).toHaveLength(1)
    expect(() => decodeUnifiedPage({ ...page, entries: [{ ...entry, providerItemId: 'different-id' }] }, origin, {})).toThrow(/provider identity/)
    expect(() => decodeUnifiedPage({ ...page, entries: [{ ...entry, providerItemId: '' }] }, origin, {})).toThrow()
  })
  it('does not render unassociated live output when the exact model origin is absent', () => {
    render(<UnifiedChat origin={origin} ready={false} active={false} liveOutput={[
      { origin: { kind: 'embedded', run: 'r', actor: '/different', incarnation: 'one' }, requestId: 'other', itemId: 'other', channel: 'assistant', index: '0', version: '1', text: 'unrelated actor output', overflow: false, streaming: true, committedHash: null },
    ]} />)
    expect(screen.queryByText('unrelated actor output')).toBeNull()
    expect(screen.getByText('Live output is unavailable for this actor.')).toBeVisible()
  })
  it('keeps the streaming element through final fetch and live snapshot eviction', async () => {
    const modelOrigin = { kind: 'embedded' as const, run: 'r', actor: '/worker', incarnation: 'one' }
    const live = { origin: modelOrigin, requestId: 'req', itemId: 'id', channel: 'assistant' as const, index: '0', text: 'partial text', streaming: true, version: '1', overflow: false, committedHash: null }
    let entries: unknown[] = []
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ origin, entries, cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: null }))))
    const mounted = render(<UnifiedChat origin={origin} requestId="req" modelOrigin={modelOrigin} ready active={false} liveOutput={[live]} />)
    const partial = await screen.findByText('partial text')
    const row = partial.closest('[data-timeline-key]')
    entries = [{ sequence: '5', kind: 'message', requestId: 'req', position: '0', hash: 'a'.repeat(64), tool: null, providerItemId: 'id', inputOperation: null, item: { type: 'message', id: 'id', role: 'assistant', content: 'final text' } }]
    mounted.rerender(<UnifiedChat origin={origin} requestId="req" revision="5" modelOrigin={modelOrigin} ready active={false} liveOutput={[{ ...live, streaming: false, committedHash: 'a'.repeat(64) }]} />)
    expect((await screen.findByText('final text')).closest('[data-timeline-key]')).toBe(row)
    expect(screen.queryByText('partial text')).toBeNull()
    mounted.rerender(<UnifiedChat origin={origin} requestId="req" revision="5" modelOrigin={modelOrigin} ready active={false} liveOutput={[]} />)
    expect(screen.getByText('final text').closest('[data-timeline-key]')).toBe(row)
  })
  it('does not resurrect a finalized partial preview when the bounded history window evicts its final row', async () => {
    const modelOrigin = { kind: 'embedded' as const, run: 'r', actor: '/worker', incarnation: 'one' }
    const live = { origin: modelOrigin, requestId: 'req', itemId: 'final-item', channel: 'assistant' as const, index: '0', text: 'partial already finalized', streaming: false, version: '1', overflow: false, committedHash: 'a'.repeat(64) }
    const all = Array.from({ length: 9 }, (_, index) => ({ sequence: String(index + 1), kind: 'message', requestId: 'req', position: String(index), hash: 'a'.repeat(64), tool: null, providerItemId: index === 8 ? 'final-item' : null, inputOperation: null,
      item: { type: 'message', role: 'assistant', content: `message ${index + 1}`, retained_metadata: 'x'.repeat(249_950), ...(index === 8 ? { id: 'final-item' } : {}) } }))
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const before = new URL(String(input), 'http://localhost').searchParams.get('before')
      const available = all.filter(entry => before === null || BigInt(entry.sequence) < BigInt(before))
      const entries = available.slice(-1)
      return new Response(JSON.stringify({ origin, entries, cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: available.length > entries.length ? entries[0]!.sequence : null }))
    }))
    const mounted = render(<UnifiedChat origin={origin} modelOrigin={modelOrigin} ready active={false} liveOutput={[live]} />)
    await waitFor(() => expect(mounted.container.querySelector('[data-sequence="9"]')).toBeInTheDocument())
    expect(screen.queryByText('partial already finalized')).toBeNull()
    for (let sequence = 8; sequence >= 1; sequence--) {
      fireEvent.click(screen.getByRole('button', { name: 'Load older messages' }))
      await waitFor(() => expect(mounted.container.querySelector(`[data-sequence="${sequence}"]`)).toBeInTheDocument())
    }
    expect(mounted.container.querySelector('[data-sequence="9"]')).toBeNull()
    expect(screen.queryByText('partial already finalized')).toBeNull()
    expect(screen.getByRole('list', { name: 'Conversation entries' }).children).toHaveLength(8)
  })
  it('anchors prepended history and offers new activity without scrolling an unpinned reader', async () => {
    const entry = (sequence: number) => ({ sequence: String(sequence), kind: 'message', requestId: 'req', position: String(sequence), hash: 'a'.repeat(64), tool: null, providerItemId: null, inputOperation: null, item: { type: 'message', role: 'assistant', content: `message ${sequence}` } })
    let latest = [entry(20), entry(30), entry(40)]
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const before = new URL(String(input), 'http://localhost').searchParams.get('before')
      return new Response(JSON.stringify({ origin, entries: before ? [entry(10)] : latest, cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: before ? null : '20' }))
    }))
    const viewportRef = createRef<HTMLDivElement>()
    const view = (revision = 0) => <div ref={viewportRef} className="worker-chat-scroll"><UnifiedChat viewportRef={viewportRef} origin={origin} revision={String(revision)} ready active={false} /></div>
    const mounted = render(view())
    const scroller = mounted.container.firstElementChild as HTMLElement
    Object.defineProperties(scroller, { clientHeight: { value: 100 }, scrollHeight: { get: () => scroller.querySelectorAll('[data-timeline-key]').length * 100 + 200 } })
    const rect = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      const index = this.dataset.timelineKey ? [...this.parentElement!.children].indexOf(this) : 0
      const top = this.dataset.timelineKey ? index * 100 - scroller.scrollTop + 20 : 0
      return { x: 0, y: top, top, bottom: top + 100, left: 0, right: 100, width: 100, height: 100, toJSON: () => ({}) }
    })
    try {
      await screen.findByText('message 40')
      scroller.scrollTop = 0; fireEvent.scroll(scroller)
      const row = screen.getByText('message 20').closest<HTMLElement>('[data-timeline-key]')!
      const offset = row.getBoundingClientRect().top
      fireEvent.click(screen.getByRole('button', { name: 'Load older messages' }))
      await screen.findByText('message 10')
      expect(row.getBoundingClientRect().top).toBe(offset)
      expect(scroller.scrollTop).toBe(100)
      latest = [...latest, entry(50)]
      mounted.rerender(view(1))
      await screen.findByRole('button', { name: 'New activity ↓' })
      expect(scroller.scrollTop).toBe(100)
      expect(screen.queryByText('message 50')).toBeNull()
      fireEvent.click(screen.getByRole('button', { name: 'New activity ↓' }))
      await screen.findByText('message 50')
      expect(scroller.scrollTop).toBe(scroller.scrollHeight)
    } finally { rect.mockRestore() }
  })

  it('retains a mounted draft and publication rank through disconnection and a new head', async () => {
    const form = { sequence: '10', revisionSequence: '10', opening: { origin, mountId: 'same-mount', execution: { kind: 'actor_program' }, conversation: null,
      form: { version: 1, root: { kind: 'text', id: 'f0', label: 'Name', initial: null } } },
      state: 'open', attemptId: null, draft: null, errors: [], answer: null }
    const first = { origin, cutoverSequence: '0', legacyHistory: false, entries: [
      { sequence: '10', kind: 'form', form },
      { sequence: '20', kind: 'message', requestId: 'head-a', position: '0', hash: '1'.repeat(64), tool: null, providerItemId: null, inputOperation: null, item: { type: 'message', role: 'assistant', content: 'Model answer' } },
    ], nextAfter: null, previousBefore: null }
    const second = { ...first, entries: [...first.entries,
      { sequence: '30', kind: 'output', output: { reference: { origin, sequence: '30' }, emission: { origin, id: { displaySlot: '1', pageOrdinal: '0' }, execution: { kind: 'actor_program' }, conversation: null,
        page: { text: 'Later display', expansions: [], unavailable: false } } } },
    ] }
    let snapshot: unknown = first
    const fetchMock = vi.fn(async () => new Response(JSON.stringify(snapshot), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    const mounted = render(<UnifiedChat origin={origin} requestId="head-a" revision="10" ready active />)
    await screen.findByLabelText('Name')
    const card = screen.getByRole('region', { name: /^Actor form / })
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'unsent edit' } })
    mounted.rerender(<UnifiedChat origin={origin} requestId="head-a" revision="10" ready={false} active />)
    expect((screen.getByLabelText('Name') as HTMLInputElement).disabled).toBe(true)
    snapshot = second
    mounted.rerender(<UnifiedChat origin={origin} requestId="head-b" revision="30" ready active />)
    await screen.findByText('Later display')
    expect(screen.getByRole('region', { name: /^Actor form / })).toBe(card)
    expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe('unsent edit')
    expect((screen.getByLabelText('Name') as HTMLInputElement).disabled).toBe(false)
    expect([...screen.getByRole('list', { name: 'Conversation entries' }).children].map(item => item.getAttribute('data-sequence'))).toEqual(['10', '20', '30'])
    expect(screen.getAllByRole('listitem')).toHaveLength(3)
  })

})
