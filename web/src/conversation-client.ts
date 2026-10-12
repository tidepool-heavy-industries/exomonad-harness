import { actorOutputKey, isStoredActorOutput, type ActorOutputOrigin } from './actor-output'
import { isStoredActorForm } from './form-model'
import { isChatPage } from './generated/validators.mjs'
import type { ChatPage, ChatEntry } from './generated/chat'

export type UnifiedEntry = ChatEntry
export type UnifiedPage = ChatPage
const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)

/** Generated shape validation is separate from publication/cursor associations. */
export function decodeUnifiedPage(value: unknown, origin: ActorOutputOrigin, cursor: string | { before?: string } = {}): UnifiedPage {
  const after = typeof cursor === 'string' ? BigInt(cursor) : 0n
  const backward = typeof cursor !== 'string'
  if (!isChatPage(value) || actorOutputKey(value.origin) !== actorOutputKey(origin)
    || BigInt(value.cutoverSequence) < 0n || value.entries.length > 100
    || (backward && (value.previousBefore === undefined || value.nextAfter !== null))
    || (value.nextAfter !== null && BigInt(value.nextAfter) <= after)
    || (value.previousBefore !== null && BigInt(value.previousBefore) <= 0n)) throw new Error('Unified conversation returned an invalid page.')
  let last = after
  for (const entry of value.entries) {
    const sequence = BigInt(entry.sequence)
    if (sequence <= last || (typeof cursor !== 'string' && cursor.before !== undefined && sequence >= BigInt(cursor.before))) throw new Error('Unified conversation entries are out of sequence.')
    last = sequence
    if (entry.kind === 'message' || entry.kind === 'oversized') {
      if (!entry.requestId || BigInt(entry.position) < 0n || !/^[0-9a-f]{64}$/.test(entry.hash)) throw new Error('Unified conversation contains an invalid message.')
      if (entry.kind === 'message' && entry.providerItemId !== null && (!entry.providerItemId || !object(entry.item) || entry.item.id !== entry.providerItemId)) throw new Error('Unified conversation contains invalid provider identity.')
      if (entry.kind === 'message' && entry.tool != null) {
        const { operation, invocation } = entry.tool
        if (!operation.request || !operation.call || invocation.request !== operation.request || BigInt(invocation.position) < 0n
          || !/^[0-9a-f]{64}$/.test(invocation.hash) || !object(entry.item) || entry.item.call_id !== operation.call) throw new Error('Unified conversation contains invalid tool ownership.')
        if (['function_call', 'custom_tool_call'].includes(String(entry.item.type)) && (invocation.request !== entry.requestId || invocation.position !== entry.position || invocation.hash !== entry.hash)) throw new Error('Unified conversation invocation does not match its issuing occurrence.')
      }
    } else if (entry.kind === 'output') {
      if (!isStoredActorOutput(entry.output) || actorOutputKey(entry.output.reference.origin) !== actorOutputKey(origin) || entry.output.reference.sequence !== entry.sequence) throw new Error('Unified conversation contains invalid output.')
    } else if (entry.kind === 'form') {
      if (!isStoredActorForm(entry.form) || actorOutputKey(entry.form.opening.origin) !== actorOutputKey(origin) || entry.form.sequence !== entry.sequence) throw new Error('Unified conversation contains an invalid form.')
    }
  }
  if (backward && value.previousBefore !== null && value.previousBefore !== value.entries[0]?.sequence) throw new Error('Unified conversation cursor does not match its first entry.')
  if (value.nextAfter !== null && BigInt(value.nextAfter) !== last) throw new Error('Unified conversation cursor does not match its last entry.')
  return value
}

export async function readUnifiedPage(origin: ActorOutputOrigin, requestId: string | undefined, before: string | undefined, signal: AbortSignal, onAuthExpired?: () => void) {
  const query = new URLSearchParams({ run: origin.run, actor: String(origin.nativeActor), incarnation: String(origin.incarnation), direction: 'backward', limit: '50' })
  if (before !== undefined) query.set('before', String(before))
  if (requestId) query.set('request', requestId)
  const response = await fetch(`/api/chat?${query}`, { credentials: 'same-origin', cache: 'no-store', signal })
  if (response.status === 401 || response.status === 403) onAuthExpired?.()
  if (!response.ok) throw new Error(response.status === 404 ? 'Unified conversation history is unavailable.' : `Conversation history failed (${response.status}).`)
  const raw = await response.text()
  if (new TextEncoder().encode(raw).length > 256 * 1024) throw new Error('Unified conversation exceeded the page limit.')
  return decodeUnifiedPage(JSON.parse(raw) as unknown, origin, { before })
}

