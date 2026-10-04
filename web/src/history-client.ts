export const HISTORY_PAGE_SIZE = 50
export const MAX_HISTORY_BYTES = 256 * 1024
const READ_TIMEOUT_MS = 10_000
const ITEM_BUDGET = MAX_HISTORY_BYTES - 16 * 1024

import { isHistoryPage } from './generated/validators.mjs'
import { unsignedDecimal } from './decimal'
import type { HistoryPage } from './generated/history'
export type { HistoryPage } from './generated/history'
export type HistoryEntry = HistoryPage['items'][number]
export type OversizedItem = NonNullable<HistoryPage['oversizedItem']>

export class HistoryReadError extends Error {
  constructor(message: string, readonly kind: 'authentication' | 'unavailable' | 'invalid' = 'unavailable') {
    super(message)
    this.name = 'HistoryReadError'
  }
}

function invalid(): never {
  throw new HistoryReadError('Request history returned an invalid page.', 'invalid')
}

/** Validates the Store envelope; Item itself remains transparent arbitrary JSON. */
export function decodeHistoryPage(value: unknown, requestId: string, offset: string, status = 200): HistoryPage {
  if (!unsignedDecimal(offset) || !isHistoryPage(value) || value.requestId !== requestId
    || value.items.length > HISTORY_PAGE_SIZE) invalid()
  const first = BigInt(offset)
  const budget = BigInt(ITEM_BUDGET)
  let last = first - 1n
  let bytes = 0n
  for (const entry of value.items) {
    const position = BigInt(entry.position)
    const length = BigInt(entry.byteLen)
    if (!/^[0-9a-f]{64}$/.test(entry.hash) || length <= 0n || position < first
      || position <= last || length > budget) invalid()
    last = position
    bytes += length
    if (bytes > budget) invalid()
  }
  const oversized = value.oversizedItem
  if (oversized !== null) {
    const position = BigInt(oversized.position)
    if (!/^[0-9a-f]{64}$/.test(oversized.hash) || position < first || position <= last
      || BigInt(oversized.byteLen) <= budget || BigInt(oversized.skipOffset) !== position + 1n
      || value.nextOffset !== oversized.position) invalid()
  }
  if (value.nextOffset !== null && (BigInt(value.nextOffset) < first || BigInt(value.nextOffset) <= last
    || (oversized === null && BigInt(value.nextOffset) <= first))) invalid()
  if (value.items.length === 0 && value.nextOffset !== null && oversized === null) invalid()
  if ((status === 413) !== (value.items.length === 0 && oversized !== null)) invalid()
  return value
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

async function requestPage(requestId: string, offset: string, signal: AbortSignal): Promise<HistoryPage> {
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
export async function readHistoryPage(requestId: string, offset: string, signal: AbortSignal): Promise<HistoryPage> {
  if (!unsignedDecimal(offset)) invalid()
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
