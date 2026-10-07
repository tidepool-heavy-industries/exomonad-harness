import { describe, expect, it } from 'vitest'
import { decodeUnifiedPage } from './UnifiedChat'

describe('unified conversation pages', () => {
  const origin = { run: 'r', nativeActor: 3, incarnation: 1 }
  it('preserves server sequence order and rejects timestamp shaped alternatives', () => {
    const page = { origin, cutoverSequence: 4, legacyHistory: true, entries: [
      { sequence: 4, kind: 'message', requestId: 'req', position: 0, hash: 'a'.repeat(64), item: { type: 'message' } },
      { sequence: 5, kind: 'output', output: { reference: { origin, sequence: 5 }, emission: { origin, id: { displaySlot: 1, pageOrdinal: 1 }, page: { text: 'value', expansions: [], unavailable: false } } } },
    ], nextAfter: null }
    expect(decodeUnifiedPage(page, origin, 0).entries.map(entry => entry.sequence)).toEqual([4, 5])
    expect(() => decodeUnifiedPage({ ...page, entries: [...page.entries].reverse() }, origin, 0)).toThrow(/sequence/)
  })
})
