import type { ChatInputOperation } from './generated/chat'
import { actorIdentityKey, canonicalOperationId, isOperationId } from './protocol'
import type { BrowserCommandRecord } from './pending-commands'
import type { CommandReceipt, InferenceBlock } from './protocol'

export function receiptText(receipt: CommandReceipt): {
  title: string;
  detail: string;
} {
  switch (receipt.outcome) {
    case 'account_selected': return { title: 'Model account selected', detail: 'The account setting applies to this run. Send or retry to continue an interrupted response.' };
    case 'recovery_authorized': return { title: 'Recovery authorized', detail: `${receipt.mode} · request ${receipt.block.request} · generation ${receipt.block.generation} · omitted ${receipt.projection.omitted.length} opaque originals · response completeness unknown for ${receipt.projection.unknownResponseCompleteness.length} exchanges. Execution is not established.` };
    case 'admitted': return { title: 'Admitted for processing', detail: `envelope ${receipt.envelopeId}${receipt.wakeError ? ` · wake issue: ${receipt.wakeError}` : ''} · ` + (receipt.continuation?.state === 'refused'
      ? `Your message is queued. ${continuationRefusalText(receipt.continuation.reason)}`
      : receipt.continuation?.state === 'granted' ? 'Your message is queued and continuation was authorized. A response is not yet established.'
      : 'Your message is queued. A response is not yet established.') };
    case 'control_requested': return { title: receipt.control === 'interrupt' ? 'Interrupt requested' : 'Retire requested', detail: 'Host control request; actor completion is not established.' };
    case 'continuation_refused': return { title: 'Response interrupted', detail: continuationRefusalText(receipt.reason) };
    case 'refused': return { title: 'Refused', detail: receipt.reason };
    case 'unconfirmed': return { title: 'Unconfirmed', detail: receipt.reason };
  }
}
export function inputStatus(record: BrowserCommandRecord): string {
  if (record.authority === 'status' || record.authority === 'receipt') {
    if (record.state === 'input_admitted' && record.inputObservation?.state === 'included')
      return 'Included in a model request · execution outcome unobserved';
    if (record.state === 'input_admitted' && record.receipt?.outcome === 'admitted' && record.receipt.continuation?.state === 'refused')
      return `Message queued · ${continuationRefusalText(record.receipt.continuation.reason)}`;
    if (record.state === 'input_admitted') return 'Host queued · request inclusion and execution unobserved';
    if (record.state === 'refused') return 'Host refused';
    if (record.state === 'unconfirmed') return 'Host outcome unconfirmed';
    if (record.state === 'dispatching') return 'Host dispatching · input admission unobserved';
    if (record.state === 'queued') return 'Host command queued · input admission unobserved';
  }
  if (record.accepted) return 'Host command accepted · input admission unobserved';
  if (record.send === 'sent') return 'Sent to host · input admission unobserved';
  if (record.send === 'unknown') return 'Send outcome unknown · retained locally';
  return 'Retained locally · input admission unobserved';
}
export function inputPublicationKey(reference: ChatInputOperation): string | undefined {
  return isOperationId(reference.operationId) ? JSON.stringify([actorIdentityKey(reference.target), canonicalOperationId(reference.operationId), reference.envelopeId]) : undefined;
}

function continuationRefusalText(reason: NonNullable<InferenceBlock['continuationRefusal']>): string {
  switch (reason) {
    case 'exact_projection_unavailable': return 'Some reasoning context could not be carried forward. Review continuation.';
    case 'uncertain_effects': return 'A tool outcome is uncertain. Continuation is blocked until it is resolved.';
    case 'continuity_unproven': return 'The retained conversation could not be verified. Continuation remains blocked.';
    case 'stale': return 'The response changed before continuation could be authorized.';
  }
}
