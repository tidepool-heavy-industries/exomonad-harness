import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import AccountSelector from './AccountSelector'
import type { AccountSelection, HostCommandSubmission } from './protocol'
import * as api from './session-api'

afterEach(() => vi.restoreAllMocks())

const selection: AccountSelection = { selectedId: 'one', accounts: [{ id: 'one', label: 'Account one' }, { id: 'two', label: 'Account two' }] }
const target = { run: 'run-1', actor: '/root', incarnation: '1' }
const submission: HostCommandSubmission = { operation_id: '11111111-1111-4111-8111-111111111111', command: { action: 'select_account', target, account_id: 'two' } }

it('requires an explicit choice and submits only the saved account ID to the root', () => {
  const submit = vi.fn().mockReturnValue({ kind: 'retained', operationId: submission.operation_id, send: 'sent' })
  const props = { selection, target, ready: true, submit, retry: vi.fn(), operations: [] }
  const view = render(<AccountSelector {...props} />)
  expect(screen.getByRole('button', { name: 'Apply account' })).toBeDisabled()
  fireEvent.change(screen.getByRole('combobox', { name: 'Model account' }), { target: { value: 'two' } })
  expect(submit).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole('button', { name: 'Apply account' }))
  expect(submit).toHaveBeenCalledExactlyOnceWith(submission.command)
  expect(screen.getByRole('combobox')).toHaveValue('one')
  view.rerender(<AccountSelector {...props} selection={{ ...selection, selectedId: 'two' }} />)
  expect(screen.getByRole('combobox')).toHaveValue('two')
  expect(screen.getByRole('button', { name: 'Apply account' })).toBeDisabled()
})

it('retains the old observed selection, prevents duplicate submission, and explicitly retries the original operation', () => {
  const retry = vi.fn().mockReturnValue({ kind: 'retained', operationId: submission.operation_id, send: 'sent' })
  const props = { selection, target, ready: true, submit: vi.fn(), retry }
  const record = { hostRun: target.run, submission, authority: 'local' as const, state: 'queued' as const, send: 'unknown' as const }
  const view = render(<AccountSelector {...props} operations={[record]} />)
  expect(screen.getByRole('combobox')).toHaveValue('one')
  expect(screen.getByRole('combobox')).toBeDisabled()
  expect(screen.getByRole('button', { name: 'Apply account' })).toBeDisabled()
  expect(retry).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole('button', { name: 'Retry account selection' }))
  expect(retry).toHaveBeenCalledExactlyOnceWith(submission)
  view.rerender(<AccountSelector {...props} operations={[record]} ready={false} />)
  expect(screen.getByRole('button', { name: 'Retry account selection' })).toBeDisabled()
})

it('shows refusal without changing the observed account and permits a new explicit choice', () => {
  render(<AccountSelector selection={selection} target={target} ready submit={vi.fn()} retry={vi.fn()} operations={[{
    hostRun: target.run, submission, authority: 'receipt', state: 'refused',
    receipt: { outcome: 'refused', commandId: submission.operation_id, target, reason: 'Saved account no longer exists' },
  }]} />)
  expect(screen.getByRole('combobox')).toHaveValue('one')
  expect(screen.getByRole('combobox')).toBeEnabled()
  expect(screen.getByRole('status')).toHaveTextContent('Saved account no longer exists')
})

it('does not offer arbitrary account entry when the inventory is empty', () => {
  render(<AccountSelector selection={{ selectedId: 'one', accounts: [] }} target={target} ready submit={vi.fn()} retry={vi.fn()} operations={[]} />)
  expect(screen.getByRole('combobox')).toBeDisabled()
  expect(screen.getByRole('button', { name: 'Apply account' })).toBeDisabled()
  expect(screen.queryByRole('textbox')).toBeNull()
})

it('permits a fresh explicit account choice after an authoritative unconfirmed outcome', () => {
  render(<AccountSelector selection={selection} target={target} ready submit={vi.fn()} retry={vi.fn()} operations={[{
    hostRun: target.run, submission, authority: 'receipt', state: 'unconfirmed',
    receipt: { outcome: 'unconfirmed', commandId: submission.operation_id, target, reason: 'Interrupted before confirmation' },
  }]} />)
  expect(screen.getByRole('combobox')).toBeEnabled()
  fireEvent.change(screen.getByRole('combobox'), { target: { value: 'two' } })
  expect(screen.getByRole('button', { name: 'Apply account' })).toBeEnabled()
})

it('refreshes saved choices on focus without changing the sequenced selection or submitting a command', async () => {
  const submit = vi.fn()
  const get = vi.spyOn(api, 'getAccountSelection').mockResolvedValue({ selectedId: 'two', accounts: [...selection.accounts, { id: 'new', label: 'New account' }] })
  render(<AccountSelector selection={selection} target={target} ready submit={submit} retry={vi.fn()} operations={[]} />)
  fireEvent.focus(screen.getByRole('combobox'))
  await screen.findByRole('option', { name: 'New account' })
  expect(get).toHaveBeenCalledOnce()
  expect(screen.getByRole('combobox')).toHaveValue('one')
  expect(submit).not.toHaveBeenCalled()
})

it('ignores a late inventory read after a newer authoritative selection arrives', async () => {
  let resolve!: (value: AccountSelection) => void
  vi.spyOn(api, 'getAccountSelection').mockReturnValue(new Promise(done => { resolve = done }))
  const props = { target, ready: true, submit: vi.fn(), retry: vi.fn(), operations: [] }
  const view = render(<AccountSelector {...props} selection={selection} />)
  fireEvent.focus(screen.getByRole('combobox'))
  view.rerender(<AccountSelector {...props} selection={{ ...selection, selectedId: 'two' }} />)
  resolve({ selectedId: 'one', accounts: [...selection.accounts, { id: 'stale', label: 'Stale result' }] })
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh saved accounts' })).toBeEnabled())
  expect(screen.getByRole('combobox')).toHaveValue('two')
  expect(screen.queryByRole('option', { name: 'Stale result' })).toBeNull()
})

it('propagates expired authentication and preserves known inventory on a failed read', async () => {
  vi.spyOn(api, 'getAccountSelection').mockRejectedValue(new api.AccountSelectionError(401))
  const expired = vi.fn()
  render(<AccountSelector selection={selection} target={target} ready submit={vi.fn()} retry={vi.fn()} operations={[]} onAuthExpired={expired} />)
  fireEvent.focus(screen.getByRole('combobox'))
  await screen.findByRole('alert')
  expect(expired).toHaveBeenCalledOnce()
  expect(screen.getAllByRole('option')).toHaveLength(2)
  expect(screen.getByRole('combobox')).toHaveValue('one')
})
