import { useLayoutEffect, useRef, useState, type FormEvent } from 'react'
import RecoveryControls from './RecoveryControls'
import { continuationRefusal, currentRecoveryBlock } from './recovery'
import { MessageContent, MessagePresentation } from './MessagePresentation'
import { actorIdentityKey, type HostCommand } from './protocol'
import { operationKey, type BrowserCommandRecord } from './pending-commands'
import type { HarnessViewModel } from './view-model'
import type { RouteState, RetryHostCommand, SubmitHostCommand } from './client-contract'
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
export function ActorComposer({ data, route, unavailable, ready, onHostCommand, operations, onRetry }: {
  data: HarnessViewModel; route: RouteState; unavailable: boolean; ready: boolean;
  onHostCommand?: SubmitHostCommand; onRetry?: RetryHostCommand; operations: readonly BrowserCommandRecord[];
}) {
  const { actor, target, enabled } = actorControls(data, route, unavailable, ready, onHostCommand);
  const draft = useDraft(target ? actorDraftKey(target) : undefined);
  const composing = useRef(false);
  const [compact, setCompact] = useState(false);
  const compactChanged = useRef(false);
  const messageInput = useRef<HTMLTextAreaElement>(null);
  const reopen = useRef<HTMLButtonElement>(null);
  useLayoutEffect(() => {
    if (compactChanged.current) (compact ? reopen.current : messageInput.current)?.focus({ preventScroll: true });
  }, [compact]);
  function setReadingMode(value: boolean) { compactChanged.current = true; setCompact(value); }
  const interrupt = () => {
    if (target && actor?.activeRound) send({ action: 'interrupt', target, expected_round: actor.activeRound });
  };
  const [feedback, setFeedback] = useState({ text: '', input: false });
  const latestInput = target ? operations.filter(record => record.submission.command.action === 'input'
    && actorIdentityKey(record.submission.command.target) === actorIdentityKey(target)).at(-1) : undefined;
  const send = (command: HostCommand) => {
    const result = onHostCommand?.(command);
    if (!result) return;
    setFeedback({ input: command.action === 'input', text: result.kind === 'blocked' ? result.reason : command.action !== 'input'
      ? 'Interrupt request saved locally. Its host outcome is available in Host operations.' : result.send === 'unknown'
      ? 'Saved locally. Initial delivery to the host was uncertain; check the message status before retrying.'
      : result.send === 'sent' ? 'Sent to the host. Admission is shown in the message status.' : 'Saved locally. Delivery and admission are shown in the message status.' });
    if (result.kind === 'retained' && command.action === 'input') draft.submitted();
  };
  const block = actor ? currentRecoveryBlock(actor) : undefined;
  const sendInput = () => { if (target && enabled && draft.text.trim()) send({ action: 'input', target, text: draft.text }); };
  return <section className={`host-controls worker-chat-composer${draft.text ? ' has-draft' : ''}${compact ? ' is-compact' : ''}`} aria-label="Message composer">
    {target && block && <RecoveryControls target={target} block={block} ready={enabled} submit={onHostCommand} retry={onRetry} continuationFailure={continuationRefusal(target, block, operations)} onResult={result => setFeedback({ input: false, text: result.kind === 'blocked' ? result.reason : '' })} operations={operations} />}
    {actor?.inference && !block && <div className="composer-state">
      <p role="status">{!['running', 'waiting'].includes(actor.lifecycle) ? `Read-only · ${actor.lifecycle}`
        : actor.inference.state === 'blocked' ? 'Response interrupted'
        : actor.inference.state === 'auto_retry' ? `Retrying response · attempt ${actor.inference.attempt} of ${actor.inference.maximum} after ${actor.inference.delayMs / 1000}s`
        : actor.inference.state === 'recovering' ? 'Retry requested'
        : actor.inference.state === 'running' ? 'Working' : 'Ready for input'}</p>
    </div>}
    {!ready && <p className="composer-connection" role="status">Disconnected · draft kept; reconnect to send.</p>}
    {!onHostCommand && <p role="status">Host command channel is unavailable; this view is read-only.</p>}
    <form onSubmit={(event: FormEvent) => { event.preventDefault(); sendInput(); }} aria-label="Send actor message">
    <div className="composer-compact-bar">
      <button ref={reopen} type="button" aria-expanded={!compact} aria-controls="actor-composer-body" onClick={() => setReadingMode(false)}>
        {draft.text ? 'Resume draft' : 'Write message'}{target && <span className="mono"> · {target.actor}</span>}
      </button>
    </div>
    <div className="composer-body" id="actor-composer-body">
      <div className="composer-heading">
        <label id="actor-message-label" htmlFor="actor-message">Message {target ? <span className="mono">{target.actor}</span> : 'to selected actor'}</label>
        <button className="composer-collapse" type="button" aria-expanded={!compact} aria-controls="actor-composer-body" onClick={() => setReadingMode(true)}>Read conversation</button>
        {target && <details className="composer-identity"><summary>Identity</summary><p>Run {target.run} · incarnation {target.incarnation}</p></details>}
      </div>
      <textarea ref={messageInput} id="actor-message" rows={1} value={draft.text} onChange={event => draft.setText(event.target.value)}
        disabled={!target || !actor || unavailable || !['running', 'waiting'].includes(actor.lifecycle)}
        aria-describedby="actor-message-hint" onCompositionStart={() => { composing.current = true; }}
        onCompositionEnd={() => { composing.current = false; }} onKeyDown={event => {
          if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.altKey && !composing.current
            && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) {
            event.preventDefault(); sendInput();
          }
        }} />
    </div>
      <div className="composer-toolbar">
        <span className="hint" id="actor-message-hint">Enter for a new line · Ctrl/Cmd+Enter to send</span>
        <div className="host-actions">
          <button className="composer-send" disabled={!enabled || !draft.text.trim()}>Send input</button>
          <button type="button" aria-describedby="actor-message-label" disabled={!enabled || !actor?.activeRound} onClick={interrupt}>Interrupt</button>
        </div>
      </div>
    </form>
    {latestInput && <p className="composer-delivery" role="status" data-input-state={latestInput.state} data-input-authority={latestInput.authority} data-input-operation-id={latestInput.submission.operation_id} data-input-observation={latestInput.inputObservation?.state}>{inputStatus(latestInput)}</p>}
    {draft.error && <p className="error" role="status">{draft.error}</p>}
    {feedback.text && !(feedback.input && latestInput && ['status', 'receipt'].includes(latestInput.authority))
      && <p className="composer-feedback" role="status">{feedback.text}</p>}
  </section>;
}
export function ActorControlPanel({ data, route, unavailable, ready, onHostCommand }: {
  data: HarnessViewModel; route: RouteState; unavailable: boolean; ready: boolean;
  onHostCommand?: SubmitHostCommand;
}) {
  const { target, enabled } = actorControls(data, route, unavailable, ready, onHostCommand);
  const [feedback, setFeedback] = useState('');
  return <details className="actor-controls"><summary>Actor controls</summary>
    <section className="actor-retire" aria-label="Retire actor">
      <h3>Retire this actor</h3><p>Close this exact actor. Retained history remains readable.</p>
      <button type="button" disabled={!enabled} onClick={() => {
        if (target && onHostCommand) {
          const result = onHostCommand({ action: 'retire', target });
          setFeedback(result.kind === 'blocked' ? result.reason : 'Retirement requested.');
        }
      }}>Retire</button>
    </section>
    {feedback && <p role="status">{feedback}</p>}
  </details>;
}
