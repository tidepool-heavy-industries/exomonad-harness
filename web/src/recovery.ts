import { actorIdentityKey, canonicalOperationId, sameInferenceBlock, type HostActorProjection, type HostCommand, type InferenceBlock } from './protocol'
import type { BrowserCommandRecord } from './pending-commands'
import type { HarnessViewModel } from './view-model'

type ActorState = Pick<HostActorProjection, 'kind' | 'modelHeadRequest' | 'inference' | 'activeRound'> & { readonly lifecycle: string }

/** A recovery decision refers to an exact frozen block, independently of lifecycle. */
export function currentRecoveryBlock(actor: ActorState): InferenceBlock | undefined {
  return actor.kind === 'model' && ['running', 'waiting'].includes(actor.lifecycle)
    && actor.inference?.state === 'blocked' && actor.modelHeadRequest === actor.inference.block.request
    ? actor.inference.block : undefined
}

export function commandIsCurrent(command: HostCommand, actor: ActorState | undefined,
  operationId?: string): boolean {
  if (!actor || !['running', 'waiting'].includes(actor.lifecycle)) return false
  if (command.action === 'recover') {
    if (actor.kind !== 'model') return false
    const inference = actor.inference
    return !!inference && ((inference.state === 'blocked' && actor.modelHeadRequest === command.expected_block.request && sameInferenceBlock(inference.block, command.expected_block))
      || (inference.state === 'recovering' && operationId !== undefined
        && canonicalOperationId(inference.operationId) === canonicalOperationId(operationId)
        && sameInferenceBlock(inference.block, command.expected_block)))
  }
  return command.action !== 'interrupt' || actor.activeRound === command.expected_round
}

export function viewCommandIsCurrent(command: HostCommand, data: HarnessViewModel, operationId?: string): boolean {
  if (command.target.run !== data.hostRun) return false
  const actor = data.actors?.find(candidate => actorIdentityKey({ run: candidate.run, actor: candidate.name, incarnation: candidate.incarnation }) === actorIdentityKey(command.target))
  return commandIsCurrent(command, actor, operationId)
}

/** Only owner-issued refusal evidence for this exact live block permits fallback. */
export function continuationRefusal(target: HostCommand['target'], block: InferenceBlock, records: readonly BrowserCommandRecord[]) {
  if (block.continuationRefusal != null) return block.continuationRefusal
  for (const record of [...records].reverse()) {
    if (!['receipt', 'status'].includes(record.authority) || record.lookup || record.issues?.length || record.hostRun !== target.run
      || actorIdentityKey(record.submission.command.target) !== actorIdentityKey(target)) continue
    const receipt = record.receipt
    const refused = receipt?.outcome === 'continuation_refused' ? receipt
      : receipt?.outcome === 'admitted' && receipt.continuation?.state === 'refused' ? receipt.continuation : undefined
    if (refused && sameInferenceBlock(refused.block, block)) return refused.reason
  }
}
