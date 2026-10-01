import type { HostActorIdentity, HostCommand, HostCommandSubmission } from './protocol'

/** Client interaction state; normalized server Maps remain authoritative. */
export type Screen = 'tree' | 'timeline' | 'inbox' | 'command' | 'host'

/** Actor identity is opaque and exact; conversation IDs never imply an actor. */
export type Selection =
  | { readonly kind: 'none' }
  | { readonly kind: 'actor'; readonly identity: HostActorIdentity }
  | { readonly kind: 'conversation'; readonly conversationId: string }

export type MessageFilter = 'all' | 'operator'

/** Encoded by native URL APIs, preserving unrelated query parameters. */
export interface RouteState {
  readonly screen: Screen
  readonly selection: Selection
  readonly global: boolean
  readonly requestId?: string
  readonly messageFilter: MessageFilter
}

/** Local retention and socket observation do not establish host admission. */
export type LocalSubmissionResult =
  | { readonly kind: 'blocked'; readonly reason: string }
  | {
      readonly kind: 'retained'
      readonly operationId: string
      readonly send: 'sent' | 'not_sent' | 'unknown'
    }

export type SubmitHostCommand = (command: HostCommand) => LocalSubmissionResult
export type RetryHostCommand = (submission: HostCommandSubmission) => LocalSubmissionResult
export type SubmitDemoCommand = (command: string) => void

export type TransportPhase =
  | 'connecting'
  | 'awaiting_snapshot'
  | 'ready'
  | 'resync'
  | 'disconnected'

/** Retained history is read-only and scoped to the current inspection context. */
export interface NodeWindowProps {
  readonly requestId: string
  readonly conversationId?: string
  readonly hostRun?: string
  /** Changes only for durable version, terminal, or delivered-output evidence. */
  readonly refreshKey?: string
  readonly onClose: () => void
}
