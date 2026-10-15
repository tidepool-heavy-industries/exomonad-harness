import { useEffect, useId, useRef, useState } from 'react'
import type { AccountSelection, HostActorIdentity } from './protocol'
import type { RetryHostCommand, SubmitHostCommand } from './client-contract'
import { isSettledCommand, type BrowserCommandRecord } from './pending-commands'
import { sameHostIdentity } from './protocol'
import { AccountSelectionError, getAccountSelection } from './session-api'

export default function AccountSelector({ selection: observedSelection, target, ready, submit, retry, operations, onAuthExpired }: {
  selection: AccountSelection; target?: HostActorIdentity; ready: boolean
  submit: SubmitHostCommand; retry: RetryHostCommand; operations: readonly BrowserCommandRecord[]
  onAuthExpired?: () => void
}) {
  const id = useId()
  const [draft, setDraft] = useState<{ baseline: string; value: string }>()
  const [feedback, setFeedback] = useState('')
  const [inventory, setInventory] = useState<{ baseline: AccountSelection; accounts: AccountSelection['accounts'] }>()
  const [refreshing, setRefreshing] = useState(false)
  const [refreshFailure, setRefreshFailure] = useState('')
  const request = useRef<AbortController>()
  useEffect(() => {
    setRefreshing(false)
    setRefreshFailure('')
    return () => { request.current?.abort(); request.current = undefined }
  }, [observedSelection, ready])
  const selection = inventory?.baseline === observedSelection ? { ...observedSelection, accounts: inventory.accounts } : observedSelection
  const choice = draft?.baseline === selection.selectedId && selection.accounts.some(account => account.id === draft.value) ? draft.value : selection.selectedId
  function refresh() {
    if (!ready || !target || request.current) return
    const controller = new AbortController()
    request.current = controller
    setRefreshing(true)
    setRefreshFailure('')
    void getAccountSelection(controller.signal).then(value => {
      // Ordered host state owns the selection; this read refreshes available choices.
      if (!controller.signal.aborted) setInventory({ baseline: observedSelection, accounts: value.accounts })
    }).catch(error => {
      if (controller.signal.aborted) return
      if (error instanceof AccountSelectionError && error.status === 401) onAuthExpired?.()
      setRefreshFailure(error instanceof Error ? error.message : 'Saved accounts could not be refreshed.')
    }).finally(() => {
      if (request.current === controller) request.current = undefined
      if (!controller.signal.aborted) setRefreshing(false)
    })
  }
  const latest = target ? operations.filter(record => record.submission.command.action === 'select_account'
    && sameHostIdentity(record.submission.command.target, target)).at(-1) : undefined
  const unresolved = latest && !isSettledCommand(latest)
  const observedUnconfirmed = latest?.state === 'unconfirmed' && ['status', 'receipt'].includes(latest.authority)
  const pending = unresolved && !observedUnconfirmed
  const available = selection.accounts.some(account => account.id === choice)
  return <section className="account-selector" aria-label="Run model account">
    <label htmlFor={id}>Model account</label>
    <div className="account-selector-controls">
      <select id={id} value={choice} disabled={!ready || !target || !!pending || !selection.accounts.length}
        onFocus={refresh} onPointerDown={refresh} onKeyDown={event => { if (['ArrowDown', 'Enter', ' '].includes(event.key)) refresh() }}
        onChange={event => { setDraft({ baseline: selection.selectedId, value: event.target.value }); setFeedback('') }}>
        {!selection.accounts.some(account => account.id === selection.selectedId) && <option value={selection.selectedId} disabled>Current account unavailable</option>}
        {selection.accounts.map(account => <option key={account.id} value={account.id}>{account.label}</option>)}
      </select>
      <button type="button" disabled={!ready || !target || !!pending || refreshing || !!refreshFailure || !available || choice === selection.selectedId} onClick={() => {
        if (!ready || !target || pending || refreshing || refreshFailure || !available || choice === selection.selectedId) return
        const result = submit({ action: 'select_account', target, account_id: choice })
        if (result.kind === 'retained') setDraft(undefined)
        setFeedback(result.kind === 'blocked' ? result.reason : '')
      }}>Apply account</button>
      <button type="button" disabled={!ready || !target || refreshing} onClick={refresh}>Refresh saved accounts</button>
    </div>
    <p className="hint">Applies to this run. Recovery remains a separate action.</p>
    {refreshing && <p role="status">Refreshing saved accounts…</p>}
    {refreshFailure && <p role="alert">{refreshFailure}</p>}
    {!selection.accounts.length && <p>No saved model accounts are available.</p>}
    {!target && <p>The current root actor is unavailable.</p>}
    {unresolved && <>
      <p role="status">Account selection is unresolved.{latest.lookup && ` ${latest.lookup.reason}`}</p>
      <button type="button" disabled={!ready || !target} onClick={() => {
        const result = retry(latest.submission)
        setFeedback(result.kind === 'blocked' ? result.reason : '')
      }}>Retry account selection</button>
    </>}
    {(latest?.receipt?.outcome === 'refused' || latest?.receipt?.outcome === 'unconfirmed') && <p role="status">{latest.receipt.reason}</p>}
    {latest?.receipt?.outcome === 'account_selected' && <p role="status">Account selection confirmed.</p>}
    {feedback && <p role="status">{feedback}</p>}
  </section>
}
