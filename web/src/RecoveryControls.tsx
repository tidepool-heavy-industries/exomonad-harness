import './RecoveryControls.css'
import { useLayoutEffect, useRef, useState } from 'react'
import { sameHostIdentity, sameInferenceBlock, type HostActorIdentity, type InferenceBlock, type RecoveryMode } from './protocol'
import type { LocalSubmissionResult, RetryHostCommand, SubmitHostCommand } from './client-contract'
import { isSettledCommand, type BrowserCommandRecord } from './pending-commands'

export default function RecoveryControls({ target, block, ready, submit, onResult, operations = [], continuationFailure, retry }: {
  continuationFailure?: InferenceBlock['continuationRefusal']; target: HostActorIdentity; block: InferenceBlock; ready: boolean; submit?: SubmitHostCommand; retry?: RetryHostCommand
  onResult: (result: LocalSubmissionResult) => void; operations?: readonly BrowserCommandRecord[]
}) {
  const [confirmation, setConfirmation] = useState<{ target: HostActorIdentity; block: InferenceBlock }>()
  const section = useRef<HTMLElement>(null)
  const confirmationGroup = useRef<HTMLDivElement>(null)
  const review = useRef<HTMLButtonElement>(null)
  const focusIntent = useRef<'confirm' | 'review' | 'status'>()
  useLayoutEffect(() => {
    const intent = focusIntent.current
    focusIntent.current = undefined
    if (intent) (intent === 'confirm' ? confirmationGroup.current : intent === 'review' ? review.current : section.current)?.focus({ preventScroll: true })
  })
  const [operation, setOperation] = useState<{ target: HostActorIdentity; block: InferenceBlock; id: string }>()
  const observed = operations.find(record => record.submission.operation_id === operation?.id)
  const refused = observed?.state === 'refused' && isSettledCommand(observed)
  const submitted = operation !== undefined && sameHostIdentity(operation.target, target) && sameInferenceBlock(operation.block, block) && !refused
  const pending = operations.filter(record => !isSettledCommand(record) && record.hostRun === target.run
    && sameHostIdentity(record.submission.command.target, target) && record.submission.command.action === 'recover'
    && sameInferenceBlock(record.submission.command.expected_block, block))
  const pendingExact = pending.find(record => record.submission.command.action === 'recover' && record.submission.command.mode === 'exact')
  const pendingVisible = pending.find(record => record.submission.command.action === 'recover' && record.submission.command.mode === 'visible_history')
  const diagnostic = block.reason.kind === 'provider_stream_failure' ? block.reason.diagnostic
    : block.reason.kind === 'request_rejected' && block.reason.failure.kind === 'http' ? block.reason.failure.diagnostic : undefined
  const visibleFallback = continuationFailure === 'exact_projection_unavailable'
  const confirming = visibleFallback && confirmation !== undefined && sameHostIdentity(confirmation.target, target) && sameInferenceBlock(confirmation.block, block)
  function resume(mode: RecoveryMode) {
    const original = mode === 'exact' ? pendingExact : pendingVisible
    if (!ready || !submit || continuationFailure === 'uncertain_effects' || mode === 'exact' && pendingVisible
      || original && !retry || submitted && !original) return
    const result = original ? retry!(original.submission) : submit({ action: 'recover', target, expected_block: block, mode })
    if (result.kind === 'retained') { focusIntent.current = 'status'; setOperation({ target, block, id: result.operationId }); setConfirmation(undefined) }
    onResult(result)
  }
  return <section ref={section} tabIndex={-1} className="inference-recovery" aria-label="Response interrupted">
    <div className="response-interrupted-heading"><h3>Response interrupted</h3>
      <button type="button" disabled={!ready || !submit || !!pendingVisible || (submitted && !pendingExact) || (!!pendingExact && !retry) || continuationFailure === 'uncertain_effects'} onClick={() => resume('exact')}>Retry</button></div>
    {diagnostic?.code === 'subscription_sharing_usage_limit_exceeded' && <p>The provider reports that the account’s subscription sharing usage limit was reached. Select another saved model account, then retry.</p>}

    <details><summary>Details</summary>
      <p>Reason: {block.reason.kind.replaceAll('_', ' ')}</p>
      {block.reason.kind === 'provider_stream_failure' && <>
        <p>Provider stream event: {block.reason.event === 'response_failed' ? 'response.failed' : 'error'}</p>
        {diagnostic?.message && <p>{diagnostic.message}</p>}
        {diagnostic && <dl>
          {diagnostic.code && <><dt>Code</dt><dd>{diagnostic.code}</dd></>}
          {diagnostic.error_type && <><dt>Type</dt><dd>{diagnostic.error_type}</dd></>}
          {diagnostic.param && <><dt>Parameter</dt><dd>{diagnostic.param}</dd></>}
        </dl>}
      </>}
      {block.reason.kind === 'response_incomplete' && <p>Provider response {block.reason.response_id} · incomplete reason {block.reason.reason ?? 'unavailable'}</p>}
      <p className="meta">Request {block.request} · generation {block.generation} · actor {target.actor} · run {target.run} · incarnation {target.incarnation}</p>
    </details>
    {visibleFallback && (!submitted || pendingVisible) && <div className="continuation-confirmation">
      {confirming ? <div ref={confirmationGroup} tabIndex={-1} role="group" aria-label="Continue without some reasoning context?">
        <p>Continue without some reasoning context?</p>
        <p>The next response will use the visible conversation. Your queued messages stay queued, and completed tool effects are not repeated.</p>
        <div className="host-actions"><button type="button" disabled={!ready || !submit} onClick={() => resume('visible_history')}>Continue without this context</button>
          <button type="button" onClick={() => { focusIntent.current = 'review'; setConfirmation(undefined) }}>Cancel</button></div>
      </div> : <><p>Some reasoning context could not be carried forward. Your messages are still queued.</p>
        <button ref={review} type="button" disabled={!ready || !submit} onClick={() => { focusIntent.current = 'confirm'; setConfirmation({ target, block }) }}>Review continuation</button></>}
    </div>}
    {continuationFailure === 'uncertain_effects' && <p role="status">A tool outcome is uncertain. It must be resolved before the response can continue.</p>}
    {continuationFailure === 'continuity_unproven' && <p role="status">The retained conversation could not be verified. Continuation remains blocked.</p>}
    {continuationFailure === 'stale' && <p role="status">The response changed before continuation could be authorized.</p>}
    {refused && !continuationFailure && <p role="status">The response could not continue. Your messages remain queued. See details before retrying.</p>}
    {(submitted || pending.length > 0) && <p role="status">Retry requested. Its outcome is not yet confirmed.</p>}
  </section>
}
