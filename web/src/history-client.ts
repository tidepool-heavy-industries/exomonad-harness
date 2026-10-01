export const HISTORY_PAGE_SIZE = 50
export const MAX_HISTORY_BYTES = 256 * 1024
const READ_TIMEOUT_MS = 10_000
const ITEM_BUDGET = MAX_HISTORY_BYTES - 16 * 1024

export interface HistoryEntry {
  readonly position: number
  readonly hash: string
  readonly byteLen: number
  readonly item: unknown
}

export interface OversizedItem {
  readonly position: number
  readonly hash: string
  readonly byteLen: number
  readonly skipOffset: number
}

export interface HistoryPage {
  readonly requestId: string
  readonly parentId: string | null
  readonly branch: string
  readonly items: readonly HistoryEntry[]
  readonly nextOffset: number | null
  readonly oversizedItem: OversizedItem | null
}

export class HistoryReadError extends Error {
  constructor(message: string, readonly kind: 'authentication' | 'unavailable' | 'invalid' = 'unavailable') {
    super(message)
    this.name = 'HistoryReadError'
  }
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function natural(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
}

function metadata(value: unknown): value is Record<string, unknown> & { position: number; hash: string; byteLen: number } {
  return record(value) && natural(value.position) && typeof value.hash === 'string'
    && /^[0-9a-f]{64}$/.test(value.hash) && natural(value.byteLen) && value.byteLen > 0
}

function invalid(): never {
  throw new HistoryReadError('Request history returned an invalid page.', 'invalid')
}

/** Validates the Store envelope; Item itself remains transparent arbitrary JSON. */
export function decodeHistoryPage(value: unknown, requestId: string, offset: number, status = 200): HistoryPage {
  if (!natural(offset) || !record(value) || value.requestId !== requestId
    || !(value.parentId === null || typeof value.parentId === 'string')
    || typeof value.branch !== 'string' || !Array.isArray(value.items)
    || value.items.length > HISTORY_PAGE_SIZE
    || !(value.nextOffset === null || natural(value.nextOffset))) invalid()
  let last = offset - 1
  let bytes = 0
  for (const entry of value.items) {
    if (!metadata(entry) || !Object.hasOwn(entry, 'item') || entry.position < offset
      || entry.position <= last || entry.byteLen > ITEM_BUDGET) invalid()
    last = entry.position
    bytes += entry.byteLen
    if (bytes > ITEM_BUDGET) invalid()
  }
  const oversized = value.oversizedItem
  if (oversized !== null) {
    if (!metadata(oversized) || oversized.position < offset || oversized.position <= last
      || oversized.byteLen <= ITEM_BUDGET || !natural(oversized.skipOffset)
      || oversized.skipOffset !== oversized.position + 1
      || value.nextOffset !== oversized.position) invalid()
  }
  if (value.nextOffset !== null && (value.nextOffset < offset || value.nextOffset <= last
    || (oversized === null && value.nextOffset <= offset))) invalid()
  if (value.items.length === 0 && value.nextOffset !== null && oversized === null) invalid()
  if ((status === 413) !== (value.items.length === 0 && oversized !== null)) invalid()
  return value as unknown as HistoryPage
}

async function boundedJson(response: Response, signal: AbortSignal): Promise<unknown> {
  const declaredLength = response.headers.get('content-length')
  if (declaredLength !== null && Number(declaredLength) > MAX_HISTORY_BYTES) {
    await response.body?.cancel()
    throw new HistoryReadError('Request history exceeded the page retrieval limit.', 'invalid')
  }
  if (!response.body) invalid()
  const reader = response.body.getReader()
  const cancel = () => { void reader.cancel().catch(() => {}) }
  signal.addEventListener('abort', cancel, { once: true })
  const decoder = new TextDecoder('utf-8', { fatal: true })
  let bytes = 0
  let text = ''
  try {
    while (true) {
      signal.throwIfAborted()
      const part = await reader.read()
      if (part.done) break
      bytes += part.value.byteLength
      if (bytes > MAX_HISTORY_BYTES) {
        await reader.cancel()
        throw new HistoryReadError('Request history exceeded the page retrieval limit.', 'invalid')
      }
      text += decoder.decode(part.value, { stream: true })
    }
    text += decoder.decode()
    return JSON.parse(text) as unknown
  } catch (cause) {
    if (cause instanceof HistoryReadError || (cause instanceof Error && cause.name === 'AbortError')) throw cause
    invalid()
  } finally {
    signal.removeEventListener('abort', cancel)
    reader.releaseLock()
  }
}

async function requestPage(requestId: string, offset: number, signal: AbortSignal): Promise<HistoryPage> {
  signal.throwIfAborted()
  const response = await fetch(`/api/history/${encodeURIComponent(requestId)}?offset=${offset}&limit=${HISTORY_PAGE_SIZE}`, {
    credentials: 'same-origin', cache: 'no-store', signal,
  })
  signal.throwIfAborted()
  if (response.status === 401 || response.status === 403) {
    throw new HistoryReadError('Sign in again to read request history.', 'authentication')
  }
  if (!response.ok && response.status !== 413) {
    throw new HistoryReadError(response.status === 404 ? 'Request history was not found.'
      : response.status === 503 ? 'Request history is unavailable.'
      : `Request history failed (${response.status}).`)
  }
  return decodeHistoryPage(await boundedJson(response, signal), requestId, offset, response.status)
}

/** One deadline covers response headers and body; parent cancellation stays silent. */
export async function readHistoryPage(requestId: string, offset: number, signal: AbortSignal): Promise<HistoryPage> {
  if (!natural(offset)) invalid()
  signal.throwIfAborted()
  const controller = new AbortController()
  let timer: ReturnType<typeof setTimeout> | undefined
  let cancel = () => {}
  const deadline = new Promise<never>((_, reject) => {
    cancel = () => {
      controller.abort()
      reject(new DOMException('Request history read was aborted.', 'AbortError'))
    }
    signal.addEventListener('abort', cancel, { once: true })
    timer = setTimeout(() => {
      controller.abort()
      reject(new HistoryReadError('Request history timed out. Retry this page.'))
    }, READ_TIMEOUT_MS)
  })
  try {
    return await Promise.race([requestPage(requestId, offset, controller.signal), deadline])
  } finally {
    clearTimeout(timer)
    signal.removeEventListener('abort', cancel)
  }
}
