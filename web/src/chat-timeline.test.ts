import { describe, expect, it } from 'vitest'
import { boundEntries, mergeEntries, operationKey, projectTimeline, type ChatTool, type MessageEntry } from './chat-timeline'
import type { LiveOutput } from './live-output'
const origin = { run: 'r', nativeActor: 1, incarnation: 2 }
const modelOrigin = { kind: 'embedded' as const, run: 'r', actor: '/worker', incarnation: 'two' }
const hash = 'a'.repeat(64)
const tool = (request: string, incarnation = 'two'): ChatTool => ({ operation: { origin: { ...modelOrigin, incarnation }, request, call: 'reused-call' }, invocation: { request, position: 0, hash } })
const message = (sequence: number, requestId: string, item: unknown, ownership?: ChatTool): MessageEntry => ({ kind: 'message', sequence, requestId, position: 0, hash, item, tool: ownership })
const call = (sequence: number, requestId: string, ownership?: ChatTool) => message(sequence, requestId, { type: 'custom_tool_call', name: 'run', call_id: 'reused-call', input: 'exact input' }, ownership)
const result = (sequence: number, requestId: string, ownership?: ChatTool) => message(sequence, requestId, { type: 'custom_tool_call_output', call_id: 'reused-call', output: 'exact result' }, ownership)
const live: LiveOutput = { origin: modelOrigin, requestId: 'request', itemId: 'provider-item', channel: 'assistant', index: 0, text: 'partial', overflow: false, streaming: true, version: 1 }

describe('timeline projection evidence', () => {
  it('bounds both rendered entries and retained encoded bytes without changing publication order', () => {
    const many = Array.from({ length: 800 }, (_, index) => message(index + 1, 'request', { type: 'message', content: 'same' }))
    expect(boundEntries(many, 'latest').entries.map(entry => entry.sequence)).toEqual(Array.from({ length: 400 }, (_, index) => index + 401))
    expect(boundEntries(many, 'oldest').entries.map(entry => entry.sequence)).toEqual(Array.from({ length: 400 }, (_, index) => index + 1))
    const large = many.slice(0, 20).map(entry => ({ ...entry, item: { content: 'x'.repeat(250_000) } }))
    const bounded = boundEntries(large, 'latest')
    expect(bounded.trimmed).toBe(true)
    expect(bounded.entries.length).toBeLessThan(20)
    expect(bounded.entries.reduce((bytes, entry) => bytes + new TextEncoder().encode(JSON.stringify(entry)).length, 0)).toBeLessThanOrEqual(2 * 1024 * 1024)
  })
  it('keeps async publication chronology while joining exact issued activity', () => {
    const issued = tool('issuance')
    const entries = [call(1, 'issuance', issued), message(2, 'other', { type: 'message', role: 'assistant', content: 'interleaved' }), result(3, 'claimant', issued)]
    const rows = projectTimeline(origin, entries, [], modelOrigin)
    expect(rows.map(row => row.kind)).toEqual(['activity', 'entry', 'result'])
    expect(rows[0]).toMatchObject({ results: [entries[2]] })
    expect(rows[2]).toMatchObject({ invocationKey: rows[0]!.key })
    expect(rows.map(row => 'entry' in row ? row.entry.sequence : null)).toEqual([1, 2, 3])
  })
  it('does not attach an unloaded or historical unbound result by wire call ID or hash', () => {
    const rows = projectTimeline(origin, [call(1, 'first', tool('first')), result(2, 'second', tool('second')), result(3, 'legacy')], [])
    expect(rows[0]).toMatchObject({ results: [] })
    expect(rows[1]).toMatchObject({ invocationKey: undefined })
    expect(rows[2]).toMatchObject({ invocationKey: undefined })
  })
  it('keeps one stable root from streaming through final history and live eviction', () => {
    const streamed = projectTimeline(origin, [], [live], modelOrigin)
    const final = message(4, 'request', { type: 'message', id: 'provider-item', role: 'assistant', content: 'completed' })
    const joined = projectTimeline(origin, [final], [{ ...live, streaming: false, committedHash: hash }], modelOrigin)
    const evicted = projectTimeline(origin, [final], [], modelOrigin)
    expect(joined).toHaveLength(1)
    expect(streamed[0]!.key).toBe(joined[0]!.key)
    expect(joined[0]!.key).toBe(evicted[0]!.key)
  })
  it('refuses streaming links without the exact model origin and never deduplicates by hash', () => {
    const final = message(4, 'request', { type: 'message', role: 'assistant', content: 'partial' })
    expect(projectTimeline(origin, [final], [{ ...live, committedHash: hash }], modelOrigin)).toHaveLength(2)
    const withId = { ...final, item: { type: 'message', id: 'provider-item' } }
    expect(projectTimeline(origin, [withId], [live])).toHaveLength(2)
    expect(projectTimeline(origin, [withId, { ...withId, sequence: 5, position: 1 }], [live], modelOrigin)).toHaveLength(3)
    expect(projectTimeline(origin, [withId], [{ ...live, origin: { ...modelOrigin, incarnation: 'old' } }], modelOrigin)).toHaveLength(1)
  })
  it('exhaustively separates equal bytes across issuances, incarnations and page histories', () => {
    for (const requestA of ['a', 'b']) for (const requestB of ['a', 'b']) for (const incarnationA of ['one', 'two']) for (const incarnationB of ['one', 'two']) {
      const a = tool(requestA, incarnationA), b = tool(requestB, incarnationB)
      const invocation = call(1, requestA, a), output = result(4, 'claimant', b)
      const unrelated = message(2, 'other', { type: 'message', content: 'equal bytes', id: 'different' })
      for (const history of [[invocation, unrelated], [unrelated, invocation], [invocation, invocation, unrelated]]) {
        const merged = mergeEntries(history, [output])
        const rows = projectTimeline(origin, merged, [])
        const activity = rows.find(row => row.kind === 'activity')!
        expect(activity.kind === 'activity' ? activity.results.length : -1).toBe(operationKey(a.operation) === operationKey(b.operation) ? 1 : 0)
        expect(merged.map(entry => entry.sequence)).toEqual([1, 2, 4])
        expect(new Set(rows.map(row => row.key)).size).toBe(rows.length)
      }
    }
  })
})
