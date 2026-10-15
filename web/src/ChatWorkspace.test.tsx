import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import App from './App'
import { clearDrafts } from './drafts'
import { clearWorkerChatRetention } from './WorkerChat'
import type { BrowserCommandRecord } from './pending-commands'
import type { HarnessViewModel } from './view-model'
import type { ChatPage, ChatEntry } from './generated/chat'

const target = { run: 'run', actor: '/worker', incarnation: 'original' }
const origin = { run: 'run', nativeActor: '7', incarnation: '1' }
const data: HarnessViewModel = { hostRun: 'run', actors: [{ id: JSON.stringify(['run', '/worker', 'original']), name: target.actor, run: target.run,
  incarnation: target.incarnation, kind: 'model', lifecycle: 'waiting', modelConversation: 'conversation',
  modelHeadRequest: 'head', outputOrigin: origin }], nodes: [], timeline: [], inbox: [] }
const operationId = '11111111-1111-4111-8111-111111111111'
const otherOperationId = '22222222-2222-4222-8222-222222222222'
const envelope = '9007199254740993'
const input: BrowserCommandRecord = { hostRun: 'run', authority: 'status', state: 'input_admitted', envelopeId: envelope,
  submission: { operation_id: operationId, command: { action: 'input', target, text: 'Equal text' } },
  inputObservation: { state: 'included', request: 'head' } }
const response = (entries: ChatEntry[]): Response => ({ ok: true, status: 200,
  text: async () => JSON.stringify({ origin, cutoverSequence: '0', legacyHistory: false, nextAfter: null, previousBefore: null, entries } satisfies ChatPage),
}) as Response
const entry = (sequence: string, reference: NonNullable<Extract<ChatEntry, { kind: 'message' }>['inputOperation']>): Extract<ChatEntry, { kind: 'message' }> => ({
  kind: 'message', sequence, requestId: 'head', position: sequence, hash: 'a'.repeat(64), tool: null, providerItemId: null, inputOperation: reference,
  item: { type: 'message', role: 'user', content: 'Equal text' },
})

beforeEach(() => {
  window.history.replaceState(null, '', '/chat/worker?run=run&incarnation=original')
  sessionStorage.clear(); clearDrafts(); clearWorkerChatRetention()
})
afterEach(() => { vi.unstubAllGlobals(); clearDrafts(); clearWorkerChatRetention() })

it('retires included pending bubbles independently of the loaded history window, including remount and replay', async () => {
  let deliver!: (value: Response) => void
  const held = new Promise<Response>(resolve => { deliver = resolve })
  const fetchMock = vi.fn().mockReturnValueOnce(held)
  vi.stubGlobal('fetch', fetchMock)
  const unknown: BrowserCommandRecord = { ...input, authority: 'local', state: 'queued', inputObservation: undefined,
    submission: { ...input.submission, operation_id: otherOperationId } }
  const mounted = render(<App data={data} pendingCommands={[input, unknown]} />)
  const bubble = (id: string) => document.querySelector(`[data-local-operation-id="${id}"]`)
  expect(bubble(operationId)).toBeNull()
  expect(bubble(otherOperationId)).not.toBeNull()
  await act(async () => { deliver(response([{ ...entry('90', { target, operationId: otherOperationId, envelopeId: '9' }),
    inputOperation: null, item: { type: 'message', role: 'assistant', content: 'A later reply in the latest history window' } }])); await held })
  expect(bubble(operationId)).toBeNull()
  expect(bubble(otherOperationId)).not.toBeNull()
  expect(screen.getByText('A later reply in the latest history window')).toBeVisible()
  expect(document.querySelector('.composer-delivery')).not.toBeNull() // The distinct unknown submission remains visible.
  mounted.unmount()
  fetchMock.mockResolvedValue(response([entry('1', { target, operationId, envelopeId: envelope })]))
  render(<App data={data} pendingCommands={[input]} />)
  await waitFor(() => expect(document.querySelector('[data-sequence="1"]')).not.toBeNull())
  expect(bubble(operationId)).toBeNull()
  expect(document.querySelector('.composer-delivery')).toBeNull()
  fireEvent.click(screen.getByText('Host operations', { selector: 'summary' }))
  expect(screen.getByRole('region', { name: 'Retained browser operations' })).toHaveTextContent('Input included · request head')
})

it('keeps legacy inclusion unconfirmed until fresh authoritative status arrives', async () => {
  vi.stubGlobal('fetch', vi.fn(() => new Promise(() => {})))
  const mounted = render(<App data={data} pendingCommands={[{ ...input, authority: 'legacy' }]} />)
  expect(screen.getByLabelText('outgoing message').textContent).toBe('Equal text')
  mounted.rerender(<App data={data} pendingCommands={[input]} />)
  expect(screen.queryByLabelText('outgoing message')).toBeNull()
})

it('preserves exact multiline drafts and ignores composition Enter before explicit Ctrl/Cmd+Enter', () => {
  vi.stubGlobal('fetch', vi.fn(() => new Promise(() => {})))
  const submit = vi.fn(() => ({ kind: 'retained' as const, operationId, send: 'sent' as const }))
  render(<App data={data} onHostCommand={submit} />)
  const textarea = screen.getByRole('textbox', { name: /^Message / })
  const text = 'First line\n  Unicode 🐚 and spaces  '
  fireEvent.change(textarea, { target: { value: text } })
  fireEvent.keyDown(textarea, { key: 'Enter' })
  fireEvent.compositionStart(textarea)
  fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
  fireEvent.compositionEnd(textarea)
  fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true, isComposing: true })
  fireEvent.keyDown(textarea, { key: 'Enter', metaKey: true, keyCode: 229 })
  expect(submit).not.toHaveBeenCalled()
  expect(textarea).toHaveValue(text)
  fireEvent.keyDown(textarea, { key: 'Enter', metaKey: true })
  expect(submit).toHaveBeenCalledExactlyOnceWith({ action: 'input', target, text })
  expect(textarea).toHaveValue('')
})

it('joins an already rendered exact input when its authoritative inclusion status arrives later', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response([entry('1', { target, operationId, envelopeId: envelope })])))
  const local: BrowserCommandRecord = { ...input, authority: 'local', state: 'queued', envelopeId: undefined, inputObservation: undefined }
  const mounted = render(<App data={data} pendingCommands={[local]} />)
  await waitFor(() => expect(document.querySelector('[data-sequence="1"]')).not.toBeNull())
  expect(document.querySelector(`[data-local-operation-id="${operationId}"]`)).not.toBeNull()
  mounted.rerender(<App data={data} pendingCommands={[input]} />)
  await waitFor(() => expect(document.querySelector(`[data-local-operation-id="${operationId}"]`)).toBeNull())
})
