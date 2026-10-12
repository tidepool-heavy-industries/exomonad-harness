import type { ChatInputOperation } from './generated/chat'
import { actorIdentityKey, canonicalOperationId, isOperationId } from './protocol'
import type { BrowserCommandRecord } from './pending-commands'
import type { CommandReceipt } from './protocol'

export function receiptText(receipt: CommandReceipt): {
  title: string;
  detail: string;
} {
  switch (receipt.outcome) {
    case 'recovery_authorized': return { title: 'Recovery authorized', detail: `${receipt.mode} · request ${receipt.block.request} · generation ${receipt.block.generation} · omitted ${receipt.projection.omitted.length} opaque originals · response completeness unknown for ${receipt.projection.unknownResponseCompleteness.length} exchanges. Execution is not established.` };
    case 'admitted': return { title: 'Admitted for processing', detail: `envelope ${receipt.envelopeId} · input admitted; model execution is not established${receipt.wakeError ? ` · wake issue: ${receipt.wakeError}` : ''}` };
    case 'control_requested': return { title: receipt.control === 'interrupt' ? 'Interrupt requested' : 'Retire requested', detail: 'Host control request; actor completion is not established.' };
    case 'refused': return { title: 'Refused', detail: receipt.reason };
    case 'unconfirmed': return { title: 'Unconfirmed', detail: receipt.reason };
  }
}
export function inputStatus(record: BrowserCommandRecord): string {
  if (record.authority === 'status' || record.authority === 'receipt') {
    if (record.state === 'input_admitted') return record.inputObservation?.state === 'included'
      ? 'Included in a model request · execution outcome unobserved' : 'Host queued · request inclusion and execution unobserved';
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
