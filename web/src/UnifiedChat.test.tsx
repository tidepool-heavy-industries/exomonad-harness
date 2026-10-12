import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import UnifiedChat, { decodeUnifiedPage } from './UnifiedChat'
import { clearMountedFormDrafts } from './MountedForm'

afterEach(() => { vi.unstubAllGlobals(); clearMountedFormDrafts() })

describe('unified conversation pages', () => {
  const origin = { run: 'r', nativeActor: '3', incarnation: '1' }
  it('preserves server sequence order and rejects timestamp shaped alternatives', () => {
    const page = { origin, cutoverSequence: '4', legacyHistory: true, entries: [
      { sequence: '4', kind: 'message', requestId: 'req', position: '0', hash: 'a'.repeat(64), tool: null, inputOperation: null, item: { type: 'message' } },
      { sequence: '5', kind: 'output', output: { reference: { origin, sequence: '5' }, emission: { origin, id: { displaySlot: '1', pageOrdinal: '1' }, execution: { kind: 'actor_program' }, conversation: null, page: { text: 'value', expansions: [], unavailable: false } } } },
    ], nextAfter: null, previousBefore: null }
    expect(decodeUnifiedPage(page, origin, '0').entries.map(entry => entry.sequence)).toEqual(['4', '5'])
    expect(() => decodeUnifiedPage({ ...page, entries: [...page.entries].reverse() }, origin, '0')).toThrow(/sequence/)
    expect(decodeUnifiedPage({ ...page, nextAfter: '5' }, origin, '0').nextAfter).toBe('5')
    expect(() => decodeUnifiedPage({ ...page, nextAfter: '6' }, origin, '0')).toThrow(/cursor/)
  })
  it('starts at latest and prepends older entries using the exact server cursor', async () => {
    const latest = { origin, cutoverSequence: '0', legacyHistory: false, entries: [
      { sequence: '20', kind: 'message', requestId: 'req', position: '1', hash: '2'.repeat(64), tool: null, inputOperation: null, item: { type: 'message', role: 'assistant', content: 'Latest page' } },
    ], nextAfter: null, previousBefore: '20' }
    const older = { origin, cutoverSequence: '0', legacyHistory: false, entries: [
      { sequence: '10', kind: 'message', requestId: 'req', position: '0', hash: '1'.repeat(64), tool: null, inputOperation: null, item: { type: 'message', role: 'assistant', content: 'Older page' } },
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
      { sequence: '10', kind: 'message', requestId: 'req', position: '0', hash: 'a'.repeat(64), tool: null, inputOperation: null, item: { type: 'message' } },
    ], nextAfter: null, previousBefore: '10' }
    expect(decodeUnifiedPage(page, origin, { before: '20' }).previousBefore).toBe('10')
    expect(() => decodeUnifiedPage({ ...page, previousBefore: '9' }, origin, { before: '20' })).toThrow(/cursor/)
    expect(() => decodeUnifiedPage(page, origin, { before: '10' })).toThrow(/sequence/)
    expect(() => decodeUnifiedPage({ ...page, previousBefore: null, origin: { ...origin, incarnation: '2' } }, origin, {})).toThrow(/invalid page/)
  })
  it('retains exact before cursors above 2^53 and refuses numeric wire substitutes', () => {
    const page = { origin, cutoverSequence: '0', legacyHistory: false, entries: [
      { sequence: '9007199254740993', kind: 'message', requestId: 'req', position: '0', hash: 'a'.repeat(64), item: { type: 'message' }, tool: null, inputOperation: null },
    ], nextAfter: null, previousBefore: '9007199254740993' }
    expect(decodeUnifiedPage(page, origin, { before: '9007199254740994' }).previousBefore).toBe('9007199254740993')
    expect(() => decodeUnifiedPage({ ...page, previousBefore: 9007199254740992 }, origin, {})).toThrow(/invalid page/)
    expect(() => decodeUnifiedPage({ ...page, previousBefore: undefined }, origin, '0')).toThrow(/invalid page/)
    expect(() => decodeUnifiedPage({ ...page, nextAfter: undefined }, origin, '0')).toThrow(/invalid page/)
  })
  it('does not render unassociated live output when the exact model origin is absent', () => {
    render(<UnifiedChat origin={origin} ready={false} active={false} liveOutput={[
      { origin: { kind: 'embedded', run: 'r', actor: '/different', incarnation: 'one' }, requestId: 'other', itemId: 'other', channel: 'assistant', index: '0', version: '1', text: 'unrelated actor output', overflow: false, streaming: true, committedHash: null },
    ]} />)
    expect(screen.queryByText('unrelated actor output')).toBeNull()
    expect(screen.getByText('Live output is unavailable for this actor.')).toBeVisible()
  })
  it('acknowledges only exact input references after their message row is rendered', async () => {
    const target = { run: 'r', actor: '/worker', incarnation: 'one' }
    const inputOperation = { target, operationId: 'same-operation', envelopeId: '9007199254740993' }
    const acknowledgement = vi.fn()
    let resolve!: (value: Response) => void
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(done => { resolve = done })))
    render(<UnifiedChat origin={origin} ready active={false} onRenderedInputs={acknowledgement} />)
    expect(acknowledgement.mock.calls.at(-1)![0]).toEqual([])
    resolve(new Response(JSON.stringify({ origin, cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: null, entries: [
      { sequence: '5', kind: 'message', requestId: 'req', position: '0', hash: 'a'.repeat(64), item: { type: 'message', role: 'user', content: 'identical submitted input' }, tool: null, inputOperation },
      { sequence: '6', kind: 'message', requestId: 'req', position: '1', hash: 'a'.repeat(64), item: { type: 'message', role: 'user', content: 'identical submitted input' }, tool: null, inputOperation: null },
    ] })))
    await waitFor(() => expect(acknowledgement.mock.calls.at(-1)![0]).toEqual([inputOperation]))
    expect(screen.getAllByText('identical submitted input')).toHaveLength(2)
  })
  it('keeps the streaming element through final fetch and live snapshot eviction', async () => {
    const modelOrigin = { kind: 'embedded' as const, run: 'r', actor: '/worker', incarnation: 'one' }
    const live = { origin: modelOrigin, requestId: 'req', itemId: 'id', channel: 'assistant' as const, index: '0', text: 'partial text', streaming: true, version: '1', overflow: false, committedHash: null }
    let entries: unknown[] = []
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ origin, entries, cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: null }))))
    const mounted = render(<UnifiedChat origin={origin} requestId="req" modelOrigin={modelOrigin} ready active={false} liveOutput={[live]} />)
    const partial = await screen.findByText('partial text')
    const row = partial.closest('[data-timeline-key]')
    entries = [{ sequence: '5', kind: 'message', requestId: 'req', position: '0', hash: 'a'.repeat(64), tool: null, inputOperation: null, item: { type: 'message', id: 'id', role: 'assistant', content: 'final text' } }]
    mounted.rerender(<UnifiedChat origin={origin} requestId="req" revision="5" modelOrigin={modelOrigin} ready active={false} liveOutput={[{ ...live, streaming: false, committedHash: 'a'.repeat(64) }]} />)
    expect((await screen.findByText('final text')).closest('[data-timeline-key]')).toBe(row)
    expect(screen.queryByText('partial text')).toBeNull()
    mounted.rerender(<UnifiedChat origin={origin} requestId="req" revision="5" modelOrigin={modelOrigin} ready active={false} liveOutput={[]} />)
    expect(screen.getByText('final text').closest('[data-timeline-key]')).toBe(row)
  })
  it('anchors prepended history and offers new activity without scrolling an unpinned reader', async () => {
    const entry = (sequence: number) => ({ sequence: String(sequence), kind: 'message', requestId: 'req', position: String(sequence), hash: 'a'.repeat(64), tool: null, inputOperation: null, item: { type: 'message', role: 'assistant', content: `message ${sequence}` } })
    let latest = [entry(20), entry(30), entry(40)]
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const before = new URL(String(input), 'http://localhost').searchParams.get('before')
      return new Response(JSON.stringify({ origin, entries: before ? [entry(10)] : latest, cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: before ? null : '20' }))
    }))
    const view = (revision = 0) => <div className="worker-chat-scroll"><UnifiedChat origin={origin} revision={String(revision)} ready active={false} /></div>
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
      { sequence: '20', kind: 'message', requestId: 'head-a', position: '0', hash: '1'.repeat(64), tool: null, inputOperation: null, item: { type: 'message', role: 'assistant', content: 'Model answer' } },
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
