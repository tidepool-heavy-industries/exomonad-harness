import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import UnifiedChat, { decodeUnifiedPage } from './UnifiedChat'

afterEach(() => vi.unstubAllGlobals())

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
})
