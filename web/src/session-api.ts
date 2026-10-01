import { canonicalOperationId, isEmbeddedCommandRecord, isObject, isOperationId, type EmbeddedCommandRecord } from './protocol'

export class SessionApiError extends Error {
  constructor(readonly status: number, operation: 'check' | 'login' | 'logout') {
    super(status === 401 && operation === 'login'
      ? 'The supplied login secret was not accepted.'
      : status === 404 && operation !== 'check' ? 'Browser login is not enabled on this server.'
        : `Session ${operation} failed (HTTP ${status}).`)
    this.name = 'SessionApiError'
  }
}

export type AuthenticationMode = 'secret' | 'tailscale' | 'disabled'
export interface SessionStatus { authenticated: boolean; authentication: AuthenticationMode; available: boolean }

export function decodeSessionStatus(value: unknown): SessionStatus {
  if (!isObject(value) || typeof value.authenticated !== 'boolean'
    || (value.authentication !== 'secret' && value.authentication !== 'tailscale' && value.authentication !== 'disabled')
    || typeof value.available !== 'boolean') {
    throw new Error('The session endpoint returned an invalid status.')
  }
  return { authenticated: value.authenticated, authentication: value.authentication, available: value.available }
}

/** Every protected read has a deadline, including connection and JSON body. */
async function request<T>(url: string, init: RequestInit, decode: (response: Response) => Promise<T>, signal?: AbortSignal): Promise<T> {
  const controller = new AbortController()
  let abort!: () => void
  let timer: ReturnType<typeof setTimeout> | undefined
  const cancelled = new Promise<never>((_, reject) => {
    abort = () => { controller.abort(); reject(new DOMException('The request was aborted.', 'AbortError')) }
    if (signal?.aborted) { abort(); return }
    signal?.addEventListener('abort', abort, { once: true })
    timer = setTimeout(() => { controller.abort(); reject(new Error('The request timed out after 10 seconds.')) }, 10_000)
  })
  try {
    return await Promise.race([
      cancelled,
      fetch(url, { credentials: 'same-origin', ...init, signal: controller.signal }).then(decode),
    ])
  } finally {
    if (timer !== undefined) clearTimeout(timer)
    signal?.removeEventListener('abort', abort)
  }
}

export async function getSessionStatus(signal?: AbortSignal): Promise<SessionStatus> {
  return request('/api/session', { method: 'GET', headers: { Accept: 'application/json' } }, async (response) => {
    if (!response.ok) throw new SessionApiError(response.status, 'check')
    const result: unknown = await response.json()
    return decodeSessionStatus(result)
  }, signal)
}

export async function login(secret: string, signal?: AbortSignal): Promise<SessionStatus> {
  return request('/api/session', {
    method: 'POST', headers: { Accept: 'application/json', 'Content-Type': 'application/json' }, body: JSON.stringify({ secret }),
  }, async (response) => {
    if (!response.ok) throw new SessionApiError(response.status, 'login')
    const result: unknown = await response.json()
    const status = decodeSessionStatus(result)
    if (!status.authenticated) throw new Error('The login endpoint did not confirm authentication.')
    return status
  }, signal)
}

export async function logout(signal?: AbortSignal): Promise<void> {
  return request('/api/session', { method: 'DELETE' }, async (response) => {
    if (!response.ok) throw new SessionApiError(response.status, 'logout')
  }, signal)
}

export class CommandStatusError extends Error {
  constructor(readonly status: number) { super(`Command status lookup failed (HTTP ${status}).`) }
}

export async function getCommandStatus(operationId: string, signal?: AbortSignal): Promise<EmbeddedCommandRecord | undefined> {
  if (!isOperationId(operationId)) throw new Error('The operation ID is invalid.')
  return request(`/api/commands/${encodeURIComponent(canonicalOperationId(operationId))}`, {
    method: 'GET', headers: { Accept: 'application/json' },
  }, async (response) => {
    if (response.status === 404) return undefined
    if (!response.ok) throw new CommandStatusError(response.status)
    const value: unknown = await response.json()
    if (!isEmbeddedCommandRecord(value) || canonicalOperationId(value.operationId) !== canonicalOperationId(operationId)) {
      throw new Error('The command status endpoint returned an invalid record.')
    }
    return value
  }, signal)
}
