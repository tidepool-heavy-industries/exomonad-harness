import './RecoveryControls.css'
import { useState, type RefObject } from 'react'
import { sameInferenceBlock, type HostActorIdentity, type InferenceBlock, type RecoveryMode } from './protocol'
import type { LocalSubmissionResult, SubmitHostCommand } from './client-contract'
import { isSettledCommand, type BrowserCommandRecord } from './pending-commands'

export default function RecoveryControls({ target, block, ready, submit, onResult, operations = [], sectionRef }: {
  sectionRef?: RefObject<HTMLElement>; target: HostActorIdentity; block: InferenceBlock; ready: boolean; submit?: SubmitHostCommand
  onResult: (result: LocalSubmissionResult) => void; operations?: readonly BrowserCommandRecord[]
}) {
  const [decision, setDecision] = useState<{ block: InferenceBlock; mode: RecoveryMode }>()
  const [operation, setOperation] = useState<{ block: InferenceBlock; id: string }>()
  const observed = operations.find(record => record.submission.operation_id === operation?.id)
  const refused = observed?.state === 'refused' && isSettledCommand(observed)
  const submitted = operation !== undefined && sameInferenceBlock(operation.block, block) && !refused
  const diagnostic = block.reason.kind === 'provider_stream_failure' ? block.reason.diagnostic
    : block.reason.kind === 'request_rejected' && block.reason.failure.kind === 'http' ? block.reason.failure.diagnostic : undefined
  const mode = decision && sameInferenceBlock(decision.block, block) ? decision.mode : undefined
  return <section ref={sectionRef} tabIndex={-1} className="inference-recovery" aria-label="Inference recovery">
    <h3>Recover inference</h3>
    <p>Inference is paused. Choose how to recover before the agent can continue. New messages can still queue.</p>
    {diagnostic?.code === 'subscription_sharing_usage_limit_exceeded' && <p>The provider reports that the account’s subscription sharing usage limit was reached. Select another saved model account, then authorize recovery below.</p>}
    <p>Reason: {block.reason.kind.replaceAll('_', ' ')}</p>
    <details><summary>Recovery details</summary>
      {block.reason.kind === 'provider_stream_failure' && <>
        <p>Provider stream event: {block.reason.event === 'response_failed' ? 'response.failed' : 'error'}</p>
        {diagnostic?.message && <p>{diagnostic.message}</p>}
        {diagnostic && <dl>
          {diagnostic.code && <><dt>Code</dt><dd>{diagnostic.code}</dd></>}
          {diagnostic.error_type && <><dt>Type</dt><dd>{diagnostic.error_type}</dd></>}
          {diagnostic.param && <><dt>Parameter</dt><dd>{diagnostic.param}</dd></>}
        </dl>}
      </>}
      <p className="meta">Request {block.request} · generation {block.generation} · actor {target.actor} · run {target.run} · incarnation {target.incarnation}</p>
    </details>
    <fieldset disabled={!ready || !submit || submitted}>
      <legend>Choose recovery evidence</legend>
      <label><input type="radio" name="recovery-mode" checked={mode === 'exact'} onChange={() => setDecision({ block, mode: 'exact' })} />Exact retained prefix</label>
      <p>Continue only if the host can prove continuity from the exact retained evidence. Otherwise, recovery is refused.</p>
      <label><input type="radio" name="recovery-mode" checked={mode === 'visible_history'} onChange={() => setDecision({ block, mode: 'visible_history' })} />Retained visible history</label>
      <p>Continue from retained visible history, omitting opaque evidence that cannot prove continuity from the next model request. Responses may be incomplete. Canonical originals remain retained; this does not replay effects.</p>
      <button type="button" disabled={!mode} onClick={() => {
        if (!ready || !mode || !submit || submitted) return
        const result = submit({ action: 'recover', target, expected_block: decision!.block, mode })
        if (result.kind === 'retained') setOperation({ block, id: result.operationId })
        onResult(result)
      }}>Authorize recovery</button>
    </fieldset>
    {decision && !mode && <p role="status">The inference block changed. Choose a recovery mode for the current block.</p>}
    {refused && <p role="status">The host refused this operation. Choose and authorize a new recovery decision explicitly.</p>}
    {submitted && <p role="status">Recovery operation retained. Inspect its host receipt or explicitly retry the same operation below.</p>}
  </section>
}
