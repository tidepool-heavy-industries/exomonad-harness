import { useMemo, useState } from 'react'
import type { CommandReceipt, HostCommand } from './protocol'
import { canonicalOperationId, isOperationId } from './protocol'
import { isSettledCommand, operationKey, type BrowserCommandRecord } from './pending-commands'
import type { HarnessViewModel } from './view-model'
import type { RetryHostCommand } from './client-contract'
import { viewCommandIsCurrent } from './recovery'
import { pageRows } from './selectors'
import { Pager } from './view-controls'
import { receiptText } from './command-presentation'

function RecoveryEvidence({ receipt }: { receipt: CommandReceipt }) {
  return receipt.outcome === 'recovery_authorized' ? <details><summary>Recovery evidence</summary>
    <p>Exact opaque originals omitted from the next model request:</p>
    {receipt.projection.omitted.length ? <ul>{receipt.projection.omitted.map((reference, index) => <li className="mono" key={index}>{reference}</li>)}</ul> : <p>None.</p>}
    <p>Exchanges with unknown response completeness:</p>
    {receipt.projection.unknownResponseCompleteness.length ? <ul>{receipt.projection.unknownResponseCompleteness.map((request, index) => <li className="mono" key={index}>{request}</li>)}</ul> : <p>None.</p>}
  </details> : null;
}
function receiptIdentity(receipt: CommandReceipt, commands: readonly BrowserCommandRecord[]): string | undefined {
  const target = receipt.target ?? (isOperationId(receipt.commandId)
    ? commands.find(record => isOperationId(record.submission.operation_id)
      && canonicalOperationId(record.submission.operation_id) === canonicalOperationId(receipt.commandId))?.submission.command.target
    : undefined);
  return target ? `run ${target.run} · actor ${target.actor} · incarnation ${target.incarnation}` : undefined;
}
function CommandPayload({ command }: { command: HostCommand }) {
  const [opened, setOpened] = useState(false);
  return <details open={opened}>
    <summary onClick={event => { event.preventDefault(); setOpened(current => !current); }}>Original command JSON</summary>
    {opened && <pre>{JSON.stringify(command, null, 2)}</pre>}
  </details>;
}
export default function HostOperations({ commands, receipts, run, ready, data, onRetry }: {
  commands: readonly BrowserCommandRecord[];
  receipts: readonly CommandReceipt[];
  run?: string;
  ready: boolean;
  data: HarnessViewModel;
  onRetry?: RetryHostCommand;
}) {
  const [feedback, setFeedback] = useState('');
  const [page, setPage] = useState(0);
  const rows = useMemo(() => [
    ...commands.map((record, index) => ({ kind: 'command' as const, record, key: `command:${operationKey(record)}:${index}` })),
    ...receipts.map((receipt, index) => ({ kind: 'receipt' as const, receipt, index, key: `receipt:${receipt.commandId}:${index}` })),
  ], [commands, receipts]);
  const visible = useMemo(() => pageRows(rows, page, 50), [rows, page]);
  if (!rows.length)
    return null;
  return <section aria-label="Retained browser operations">
    <h2>Retained browser operations</h2>
    {visible.rows.map(row => row.kind === 'command' ? <article className="operation" data-operation-id={row.record.submission.operation_id} key={row.key}>
      <strong>{row.record.submission.command.action} · {row.record.authority === 'legacy' ? `Previously observed ${row.record.state}; fresh lookup required` : row.record.authority === 'local' ? 'Locally retained; host admission unknown' : row.record.state}</strong>
      <div className="mono">{row.record.submission.operation_id} · run {row.record.hostRun} · actor {row.record.submission.command.target.actor} · incarnation {row.record.submission.command.target.incarnation}</div>
      <CommandPayload command={row.record.submission.command} />
      <p>Socket send: {row.record.send ?? 'unknown'}{row.record.accepted ? ' · Server acknowledged handoff; terminal outcome unknown.' : ''}</p>
      {row.record.lookup && <p className="hint">Status lookup {row.record.lookup.kind}: {row.record.lookup.reason}</p>}
      {row.record.localRefusal && <p className="error">Local channel refusal: {row.record.localRefusal.reason}</p>}
      {row.record.issues?.map((issue, i) => <p className="error" key={i}>{issue}</p>)}
      {row.record.receipt && <p>{receiptText(row.record.receipt).title}: {receiptText(row.record.receipt).detail}</p>}
      {row.record.inputObservation && <p className="meta">Input {row.record.inputObservation.state}{row.record.inputObservation.state === 'included' ? ` · request ${row.record.inputObservation.request}; execution outcome unobserved` : ''}</p>}
      {row.record.receipt && <RecoveryEvidence receipt={row.record.receipt} />}
      <button disabled={!onRetry || !ready || row.record.hostRun !== run || !viewCommandIsCurrent(row.record.submission.command, data, row.record.submission.operation_id) || isSettledCommand(row.record)} onClick={() => {
        const result = onRetry?.(row.record.submission); if (result)
          setFeedback(result.kind === 'blocked' ? result.reason : `Same operation retained; socket send ${result.send}.`);
      }}>Retry same operation</button>
    </article> : <article className="operation" key={row.key}>
      <strong>{receiptText(row.receipt).title}</strong>
      <div className="mono">{row.receipt.commandId}{receiptIdentity(row.receipt, commands) && ` · ${receiptIdentity(row.receipt, commands)}`}</div>
      <p>{receiptText(row.receipt).detail}</p>
      <RecoveryEvidence receipt={row.receipt} />
    </article>)}<Pager page={visible.page} count={visible.pageCount} total={visible.total} onPage={setPage} />
    <p className="hint">Admission, request inclusion, execution, delivery and terminal outcome are separate observations.</p>
    {feedback && <p role="status">
      {feedback}
    </p>}
    <p className="hint">Original payloads are retained. Reconnect never automatically resubmits an operation.</p>
  </section>;
}
