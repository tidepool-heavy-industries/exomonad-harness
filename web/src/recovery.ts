import { actorIdentityKey, canonicalOperationId, sameInferenceBlock, type HostActorProjection, type HostCommand, type InferenceBlock } from './protocol'
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
