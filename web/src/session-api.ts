import type { EmbeddedCommandRecord } from './protocol'
import { isCommandState, isHostCommand, isOperationId } from './pending-commands'

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

export class CommandStatusError extends Error {
  constructor(readonly status: number) {
    super(`Command status lookup failed (HTTP ${status}).`)
  }
}

export async function getCommandStatus(operationId: string): Promise<EmbeddedCommandRecord | undefined> {
  if (!isOperationId(operationId)) throw new Error('The operation ID is invalid.')
  const response = await fetch(`/api/commands/${encodeURIComponent(operationId)}`, {
    method: 'GET', credentials: 'same-origin', headers: { Accept: 'application/json' },
  })
  if (response.status === 404) return undefined
  if (!response.ok) throw new CommandStatusError(response.status)
  const value: unknown = await response.json()
  if (typeof value !== 'object' || value === null || !('operationId' in value) ||
      !('command' in value) || !('state' in value) || !('envelopeId' in value) || !('receipt' in value) ||
      value.operationId !== operationId || !isHostCommand(value.command) || !isCommandState(value.state) ||
      (value.envelopeId !== null && (!Number.isSafeInteger(value.envelopeId) || (value.envelopeId as number) < 0)) ||
      (value.receipt !== null && (typeof value.receipt !== 'object' || value.receipt === null))) {
    throw new Error('The command status endpoint returned an invalid record.')
  }
  return value as EmbeddedCommandRecord
}
