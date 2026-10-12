import { useRef, useState, type FormEvent } from 'react'
import RecoveryControls from './RecoveryControls'
import { currentRecoveryBlock } from './recovery'
import { MessageContent, MessagePresentation } from './MessagePresentation'
import { actorIdentityKey, type HostCommand } from './protocol'
import { operationKey, type BrowserCommandRecord } from './pending-commands'
import type { HarnessViewModel } from './view-model'
import type { RouteState, SubmitHostCommand } from './client-contract'
import { resolveSelection } from './selectors'
import { actorDraftKey, useDraft } from './drafts'
import { inputStatus, inputPublicationKey, receiptText } from './command-presentation'

function actorControls(data: HarnessViewModel, route: RouteState, unavailable: boolean, ready: boolean, submit?: SubmitHostCommand) {
  const actor = resolveSelection(data, route.selection).actor;
  const target = route.selection.kind === 'actor' ? route.selection.identity : undefined;
  const enabled = Boolean(target && actor && !unavailable && actor.run === data.hostRun && ready && submit && ['running', 'waiting'].includes(actor.lifecycle));
  return { actor, target, enabled };
}
export function OutgoingInputs({ records, rendered }: { records: readonly BrowserCommandRecord[]; rendered: ReadonlySet<string> }) {
  const inputs = records.filter(record => record.submission.command.action === 'input'
    && !(record.state === 'input_admitted' && ['status', 'receipt'].includes(record.authority) && record.inputObservation?.state === 'included'
      && record.envelopeId != null && rendered.has(inputPublicationKey({ target:record.submission.command.target,
        operationId:record.submission.operation_id, envelopeId:record.envelopeId })!)));
  return inputs.length ? <section className="outgoing-inputs" aria-label="Outgoing messages"><div role="list">
    {inputs.map(record => <div key={operationKey(record)} data-local-operation-id={record.submission.operation_id}>
      <MessagePresentation kind="human" title="You" status={inputStatus(record)} details={<>
        <p className="mono">Operation {record.submission.operation_id} · envelope {record.envelopeId ?? 'unobserved'}</p>
        <p className="mono">{record.submission.command.target.actor} · run {record.hostRun} · incarnation {record.submission.command.target.incarnation}</p>
        {record.receipt && <p>{receiptText(record.receipt).title}: {receiptText(record.receipt).detail}</p>}
      </>}>
        <MessageContent text={record.submission.command.action === 'input' ? record.submission.command.text : ''} label="outgoing message" format="plain" />
      </MessagePresentation>
    </div>)}
  </div></section> : null;
}
export function ActorComposer({ data, route, unavailable, ready, onHostCommand, operations }: {
  data: HarnessViewModel; route: RouteState; unavailable: boolean; ready: boolean;
  onHostCommand?: SubmitHostCommand; operations: readonly BrowserCommandRecord[];
}) {
  const { actor, target, enabled } = actorControls(data, route, unavailable, ready, onHostCommand);
  const draft = useDraft(target ? actorDraftKey(target) : undefined);
  const composing = useRef(false);
  const [feedback, setFeedback] = useState('');
  const latestInput = target ? operations.filter(record => record.submission.command.action === 'input'
    && actorIdentityKey(record.submission.command.target) === actorIdentityKey(target)).at(-1) : undefined;
  const send = (command: HostCommand) => {
    const result = onHostCommand?.(command);
    if (!result) return;
    setFeedback(result.kind === 'blocked' ? result.reason : `Operation retained; socket send ${result.send}. Input admission remains a separate host observation.`);
    if (result.kind === 'retained' && command.action === 'input') draft.submitted();
  };
  const sendInput = () => { if (target && enabled && draft.text.trim()) send({ action: 'input', target, text: draft.text }); };
  return <section className="host-controls worker-chat-composer" aria-label="Message composer">
    {actor?.inference && <p className="composer-state" role="status">Inference: {actor.inference.state}{actor.inference.state === 'blocked'
      ? ' · input can queue; recovery requires an explicit decision in Actor controls.' : actor.inference.state === 'recovering' ? ' · recovery authorized; execution unobserved' : ''}</p>}
    {!ready && <p role="status">Host controls await a fresh connected snapshot.</p>}
    {!onHostCommand && <p role="status">Host command channel is unavailable; this view is read-only.</p>}
    <form onSubmit={(event: FormEvent) => { event.preventDefault(); sendInput(); }} aria-label="Send actor message">
      <label htmlFor="actor-message">Message to selected actor</label>
      <textarea id="actor-message" rows={3} value={draft.text} onChange={event => draft.setText(event.target.value)}
        disabled={!target || !actor || unavailable || !['running', 'waiting'].includes(actor.lifecycle)}
        aria-describedby="actor-message-hint" onCompositionStart={() => { composing.current = true; }}
        onCompositionEnd={() => { composing.current = false; }} onKeyDown={event => {
          if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.altKey && !composing.current
            && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) {
            event.preventDefault(); sendInput();
          }
        }} />
      <div className="composer-toolbar">
        <span className="hint" id="actor-message-hint">Enter for a new line · Ctrl/Cmd+Enter to send</span>
        <div className="host-actions">
          <button disabled={!enabled || !draft.text.trim()}>Send input</button>
          <button type="button" disabled={!enabled || !actor?.activeRound} onClick={() => {
            if (target && actor?.activeRound) send({ action: 'interrupt', target, expected_round: actor.activeRound });
          }}>Interrupt</button>
        </div>
      </div>
    </form>
    {latestInput && <p className="composer-delivery" role="status" data-input-state={latestInput.state} data-input-authority={latestInput.authority} data-input-operation-id={latestInput.submission.operation_id} data-input-observation={latestInput.inputObservation?.state}>{inputStatus(latestInput)}</p>}
    {draft.error && <p className="error" role="status">{draft.error}</p>}
    {feedback && <p className="composer-feedback" role="status">{feedback}</p>}
  </section>;
}
export function ActorControlPanel({ data, route, unavailable, ready, onHostCommand, operations }: {
  data: HarnessViewModel; route: RouteState; unavailable: boolean; ready: boolean;
  onHostCommand?: SubmitHostCommand; operations: readonly BrowserCommandRecord[];
}) {
  const { actor, target, enabled } = actorControls(data, route, unavailable, ready, onHostCommand);
  const block = actor ? currentRecoveryBlock(actor) : undefined;
  const [feedback, setFeedback] = useState('');
  const report = (result: import('./client-contract').LocalSubmissionResult) =>
    setFeedback(result.kind === 'blocked' ? result.reason : `Operation ${result.operationId} retained; socket send ${result.send}. Host outcome remains separate.`);
  return <details className="actor-controls"><summary>Actor controls</summary>
    {target && block && <RecoveryControls target={target} block={block} ready={enabled} submit={onHostCommand} onResult={report} operations={operations} />}
    <section className="actor-retire" aria-label="Retire actor">
      <h3>Retire this actor</h3><p>Close this exact actor. Retained history remains readable.</p>
      <button type="button" disabled={!enabled} onClick={() => { if (target && onHostCommand) report(onHostCommand({ action: 'retire', target })); }}>Retire</button>
    </section>
    {feedback && <p role="status">{feedback}</p>}
  </details>;
}
