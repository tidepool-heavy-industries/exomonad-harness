export class SessionApiError extends Error {
  constructor(
    readonly status: number,
    operation: 'check' | 'login' | 'logout',
  ) {
    const message = status === 401
      ? 'The supplied login secret was not accepted.'
      : status === 404 && operation !== 'check'
        ? 'Browser login is not enabled on this server.'
        : `Session ${operation} failed (HTTP ${status}).`
    super(message)
    this.name = 'SessionApiError'
  }
}

export interface SessionStatus {
  authenticated: boolean
}

const endpoint = '/api/session'

export async function getSessionStatus(): Promise<SessionStatus> {
  const response = await fetch(endpoint, {
    method: 'GET',
    credentials: 'same-origin',
    headers: { Accept: 'application/json' },
  })
  if (!response.ok) throw new SessionApiError(response.status, 'check')
  const result: unknown = await response.json()
  if (typeof result !== 'object' || result === null || !('authenticated' in result) ||
      typeof result.authenticated !== 'boolean') {
    throw new Error('The session endpoint returned an invalid status.')
  }
  return { authenticated: result.authenticated }
}

export async function login(secret: string): Promise<SessionStatus> {
  const response = await fetch(endpoint, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
    body: JSON.stringify({ secret }),
  })
  if (!response.ok) throw new SessionApiError(response.status, 'login')
  const result: unknown = await response.json()
  if (typeof result !== 'object' || result === null || !('authenticated' in result) ||
      result.authenticated !== true) {
    throw new Error('The login endpoint did not confirm authentication.')
  }
  return { authenticated: true }
}

export async function logout(): Promise<void> {
  const response = await fetch(endpoint, {
    method: 'DELETE',
    credentials: 'same-origin',
  })
  if (!response.ok) throw new SessionApiError(response.status, 'logout')
}
