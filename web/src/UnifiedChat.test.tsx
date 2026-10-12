import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import UnifiedChat, { decodeUnifiedPage } from './UnifiedChat'
import { clearMountedFormDrafts } from './MountedForm'

afterEach(() => { vi.unstubAllGlobals(); clearMountedFormDrafts() })

describe('unified conversation pages', () => {
  const origin = { run: 'r', nativeActor: 3, incarnation: 1 }
  it('preserves server sequence order and rejects timestamp shaped alternatives', () => {
    const page = { origin, cutoverSequence: 4, legacyHistory: true, entries: [
      { sequence: 4, kind: 'message', requestId: 'req', position: 0, hash: 'a'.repeat(64), item: { type: 'message' } },
      { sequence: 5, kind: 'output', output: { reference: { origin, sequence: 5 }, emission: { origin, id: { displaySlot: 1, pageOrdinal: 1 }, page: { text: 'value', expansions: [], unavailable: false } } } },
    ], nextAfter: null }
    expect(decodeUnifiedPage(page, origin, 0).entries.map(entry => entry.sequence)).toEqual([4, 5])
    expect(() => decodeUnifiedPage({ ...page, entries: [...page.entries].reverse() }, origin, 0)).toThrow(/sequence/)
    expect(decodeUnifiedPage({ ...page, nextAfter: 5 }, origin, 0).nextAfter).toBe(5)
    expect(() => decodeUnifiedPage({ ...page, nextAfter: 6 }, origin, 0)).toThrow(/cursor/)
  })
  it('uses server cursors for pages of varying sizes and retains a previous-page path', async () => {
    const first = { origin, cutoverSequence: 0, legacyHistory: false, entries: [
      { sequence: 10, kind: 'message', requestId: 'req', position: 0, hash: '1'.repeat(64), item: { type: 'message', role: 'assistant', content: 'First page' } },
    ], nextAfter: 10 }
    const second = { origin, cutoverSequence: 0, legacyHistory: false, entries: [
      { sequence: 20, kind: 'message', requestId: 'req', position: 1, hash: '2'.repeat(64), item: { type: 'message', role: 'assistant', content: 'Second page' } },
    ], nextAfter: null }
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const after = new URL(String(input), 'http://localhost').searchParams.get('after')
      return new Response(JSON.stringify(after === '10' ? second : first), { status: 200 })
    })
    vi.stubGlobal('fetch', fetchMock)
    render(<UnifiedChat origin={origin} ready active={false} />)
    await screen.findByText('First page')
    fireEvent.click(screen.getByRole('button', { name: 'Next entries' }))
    await screen.findByText('Second page')
    expect(fetchMock.mock.calls.some(([input]) => new URL(String(input), 'http://localhost').searchParams.get('after') === '10')).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Previous entries' }))
    await screen.findByText('First page')
  })
  it('retains a mounted draft and publication rank through disconnection and a new head', async () => {
    const form = { sequence: 10, revisionSequence: 10, opening: { origin, mountId: 'same-mount', execution: { kind: 'actor_program' }, conversation: null,
      form: { version: 1, root: { kind: 'text', id: 'f0', label: 'Name', initial: null } } },
      state: 'open', attemptId: null, draft: null, errors: [], answer: null }
    const first = { origin, cutoverSequence: 0, legacyHistory: false, entries: [
      { sequence: 10, kind: 'form', form },
      { sequence: 20, kind: 'message', requestId: 'head-a', position: 0, hash: '1'.repeat(64), item: { type: 'message', role: 'assistant', content: 'Model answer' } },
    ], nextAfter: null }
    const second = { ...first, entries: [...first.entries,
      { sequence: 30, kind: 'output', output: { reference: { origin, sequence: 30 }, emission: { origin, id: { displaySlot: 1, pageOrdinal: 0 }, execution: { kind: 'actor_program' }, conversation: null,
        page: { text: 'Later display', expansions: [], unavailable: false } } } },
    ] }
    let snapshot: unknown = first
    const fetchMock = vi.fn(async () => new Response(JSON.stringify(snapshot), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    const mounted = render(<UnifiedChat origin={origin} requestId="head-a" revision={10} ready active />)
    await screen.findByLabelText('Name')
    const card = screen.getByRole('region', { name: /^Actor form / })
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'unsent edit' } })
    mounted.rerender(<UnifiedChat origin={origin} requestId="head-a" revision={10} ready={false} active />)
    expect((screen.getByLabelText('Name') as HTMLInputElement).disabled).toBe(true)
    snapshot = second
    mounted.rerender(<UnifiedChat origin={origin} requestId="head-b" revision={30} ready active />)
    await screen.findByText('Later display')
    expect(screen.getByRole('region', { name: /^Actor form / })).toBe(card)
    expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe('unsent edit')
    expect((screen.getByLabelText('Name') as HTMLInputElement).disabled).toBe(false)
    expect([...screen.getByRole('list', { name: 'Conversation entries' }).children].map(item => item.getAttribute('data-sequence'))).toEqual(['10', '20', '30'])
    expect(screen.getAllByRole('listitem')).toHaveLength(3)
  })

})
