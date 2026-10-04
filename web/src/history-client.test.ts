import { afterEach, describe, expect, it, vi } from 'vitest'
import { decodeHistoryPage, HistoryReadError, MAX_HISTORY_BYTES, readHistoryPage } from './history-client'

const hash = 'a'.repeat(64)
const page = () => ({ requestId: 'request λ/1', parentId: null, branch: '/root',
  items: [{ position: '0', hash, byteLen: '4', item: null }], nextOffset: null, oversizedItem: null })
const signal = () => new AbortController().signal

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers() })

describe('protected retained history client', () => {
  it('reads the exact encoded request at one bounded offset with no-store credentials and abort signal', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify(page())))
    vi.stubGlobal('fetch', fetchMock)
    const abortSignal = signal()
    expect(await readHistoryPage('request λ/1', '0', abortSignal)).toEqual(page())
    expect(fetchMock).toHaveBeenCalledWith('/api/history/request%20%CE%BB%2F1?offset=0&limit=50', {
      credentials: 'same-origin', cache: 'no-store', signal: expect.any(AbortSignal),
    })
  })

  it.each([null, true, false, 0, 27, 'λ\n  exact', ['future', { extra: null }]])('preserves arbitrary JSON Item %j', (item) => {
    const value = page()
    const result = decodeHistoryPage({ ...value, items: [{ ...value.items[0], item }] }, value.requestId, '0')
    expect(result.items[0]?.item).toEqual(item)
  })

  it.each([
    { requestId: 'another-request' }, { parentId: 2 }, { branch: null }, { items: null },
    { nextOffset: -1 }, { nextOffset: '0' }, { oversizedItem: undefined },
    { items: [{ position: -1, hash, byteLen: '4', item: null }] },
    { items: [{ position: '0', hash: 'fake-hash', byteLen: '4', item: null }] },
    { items: [{ position: '0', hash, byteLen: '0', item: null }] },
    { items: [{ position: '0', hash, byteLen: '4' }] },
    { items: [{ position: Number.MAX_SAFE_INTEGER + 1, hash, byteLen: '4', item: null }] },
    { items: [{ position: '0', hash, byteLen: '4', item: null }, { position: '0', hash, byteLen: '4', item: null }] },
    { items: Array.from({ length: 51 }, (_, position) => ({ position: String(position), hash, byteLen: '4', item: null })) },
    { items: [{ position: '0', hash, byteLen: '150000', item: null }, { position: '1', hash, byteLen: '150000', item: null }] },
  ])('rejects malformed/misassociated page %j', (change) => {
    expect(() => decodeHistoryPage({ ...page(), ...change }, page().requestId, '0')).toThrow(HistoryReadError)
  })

  it('retains exact paging positions above safe integers and refuses a rounded numeric control', () => {
    const position = '9007199254740993'
    const value = { ...page(), items: [{ position, hash, byteLen: '4', item: null }], nextOffset: '9007199254740994' }
    expect(decodeHistoryPage(value, value.requestId, position)).toEqual(value)
    expect(() => decodeHistoryPage({ ...value, items: [{ ...value.items[0], position: Number(position) }] }, value.requestId, position)).toThrow(HistoryReadError)
  })

  it('rejects entries before the requested offset and a backwards or empty next page', () => {
    expect(() => decodeHistoryPage(page(), page().requestId, '1')).toThrow(HistoryReadError)
    expect(() => decodeHistoryPage({ ...page(), items: [], nextOffset: '1' }, page().requestId, '0')).toThrow(HistoryReadError)
  })

  it('accepts 413 oversized disclosure with an explicit skip, rejecting inconsistent metadata/status', async () => {
    const blocked = { ...page(), items: [], nextOffset: '50',
      oversizedItem: { position: '50', hash, byteLen: '270000', skipOffset: '51' } }
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify(blocked), { status: 413 })))
    expect(await readHistoryPage(page().requestId, '50', signal())).toEqual(blocked)
    for (const change of [{ skipOffset: '50' }, { byteLen: '20' }, { position: '49' }, { hash: 'bad' }]) {
      expect(() => decodeHistoryPage({ ...blocked, oversizedItem: { ...blocked.oversizedItem, ...change } }, page().requestId, '50', 413)).toThrow(HistoryReadError)
    }
    expect(() => decodeHistoryPage(blocked, page().requestId, '50', 200)).toThrow(HistoryReadError)
    expect(() => decodeHistoryPage(page(), page().requestId, '0', 413)).toThrow(HistoryReadError)
  })

  it.each([401, 403])('types confirmed authentication refusal %i', async (status) => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('', { status })))
    await expect(readHistoryPage(page().requestId, '0', signal())).rejects.toMatchObject({ kind: 'authentication' })
  })

  it.each([404, 503, 500])('keeps history failure %i separate from authentication', async (status) => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('', { status })))
    await expect(readHistoryPage(page().requestId, '0', signal())).rejects.toMatchObject({ kind: 'unavailable' })
  })

  it('rejects malformed JSON and oversized streamed bytes before decoding', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(new Response('<html>sign in</html>'))
      .mockResolvedValueOnce(new Response('λ'.repeat(MAX_HISTORY_BYTES)))
      .mockResolvedValueOnce(new Response('{}', { headers: { 'content-length': String(MAX_HISTORY_BYTES + 1) } }))
    vi.stubGlobal('fetch', fetchMock)
    await expect(readHistoryPage(page().requestId, '0', signal())).rejects.toMatchObject({ kind: 'invalid' })
    await expect(readHistoryPage(page().requestId, '0', signal())).rejects.toThrow('retrieval limit')
    await expect(readHistoryPage(page().requestId, '0', signal())).rejects.toThrow('retrieval limit')
  })

  it('bounds stalled response headers to 10 seconds and leaves the next retry with a fresh deadline', async () => {
    vi.useFakeTimers()
    const fetchMock = vi.fn().mockReturnValueOnce(new Promise(() => {}))
      .mockResolvedValueOnce(new Response(JSON.stringify(page())))
    vi.stubGlobal('fetch', fetchMock)
    const read = readHistoryPage(page().requestId, '0', signal())
    const failure = expect(read).rejects.toMatchObject({ kind: 'unavailable', message: 'Request history timed out. Retry this page.' })
    await vi.advanceTimersByTimeAsync(10_000)
    await failure
    expect(fetchMock.mock.calls[0]?.[1].signal.aborted).toBe(true)
    expect(vi.getTimerCount()).toBe(0)
    expect(await readHistoryPage(page().requestId, '0', signal())).toEqual(page())
    expect(vi.getTimerCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(20_000)
    expect(fetchMock.mock.calls[1]?.[1].signal.aborted).toBe(false)
  })

  it('applies the same deadline to a stalled response body and cancels its reader', async () => {
    vi.useFakeTimers()
    const cancel = vi.fn()
    const body = new ReadableStream<Uint8Array>({ cancel })
    const fetchMock = vi.fn().mockResolvedValue(new Response(body))
    vi.stubGlobal('fetch', fetchMock)
    const read = readHistoryPage(page().requestId, '0', signal())
    const failure = expect(read).rejects.toMatchObject({ kind: 'unavailable', message: 'Request history timed out. Retry this page.' })
    await vi.advanceTimersByTimeAsync(10_000)
    await failure
    expect(cancel).toHaveBeenCalledTimes(1)
    expect(vi.getTimerCount()).toBe(0)
  })

  it('cancels a parent-aborted read immediately and cleans its timer without reporting timeout/authentication', async () => {
    vi.useFakeTimers()
    const parent = new AbortController()
    const fetchMock = vi.fn().mockReturnValue(new Promise(() => {}))
    vi.stubGlobal('fetch', fetchMock)
    const read = readHistoryPage(page().requestId, '0', parent.signal)
    const failure = expect(read).rejects.toMatchObject({ name: 'AbortError' })
    parent.abort()
    await failure
    expect(fetchMock.mock.calls[0]?.[1].signal.aborted).toBe(true)
    expect(vi.getTimerCount()).toBe(0)
  })
})
