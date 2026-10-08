import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import MountedForm, { clearMountedFormDrafts, isFormSpec, isStoredActorForm, type StoredActorForm } from './MountedForm'

import preparedBoundaries from './fixtures/prepared-form-boundaries.json'

function prepared(name: string) {
  const entry = preparedBoundaries.cases.find(item => item.name === name)!
  if (!isFormSpec(entry.descriptor)) throw new Error(`Invalid prepared descriptor: ${name}`)
  return entry.descriptor
}

const origin = { run: 'run', nativeActor: 4, incarnation: 2 }
const view = { kind: 'text' as const, text: 'Option details' }
const openForm: StoredActorForm = {
  sequence: 9, revisionSequence: 10, opening: { origin, mountId: 'mount-1', execution: { kind: 'actor_program' }, conversation: null, form: { version: 1, root: { kind: 'group', children: [
    { kind: 'text', id: 'name', label: 'Name', initial: null },
    { kind: 'choice', id: 'choice', label: 'Pick one', initial: null, options: [
      { id: 'first', label: 'Same label', presentation: view }, { id: 'second', label: 'Same label', presentation: view },
    ] },
    { kind: 'alternatives', id: 'mode', label: 'Mode', initial: 'a', options: [
      { id: 'a', label: 'A', presentation: view, form: { kind: 'text', id: 'onlyA', label: 'A field', initial: 'a value' } },
      { id: 'b', label: 'B', presentation: view, form: { kind: 'text', id: 'onlyB', label: 'B field', initial: 'b value' } },
    ] },
  ] } } }, state: 'open', attemptId: null, draft: null, errors: [], answer: null,
}
function integerForm(initial: string | null): StoredActorForm {
  return { ...openForm, opening: { ...openForm.opening, mountId: 'int-mount', form: { version: 1, root: {
    kind: 'int', id: 'integer', label: 'Integer', initial,
  } } } }
}
function numberForm(): StoredActorForm {
  return { ...openForm, opening: { ...openForm.opening, mountId: 'number-mount', form: prepared('blank-number-null') } }
}
function radioForm(mountId: string): StoredActorForm {
  return { ...openForm, opening: { ...openForm.opening, mountId, form: prepared('rich-choice') } }
}

beforeEach(() => { clearMountedFormDrafts(); vi.restoreAllMocks() })

describe('mounted actor forms', () => {
  it('retains edits across rerenders and rejected requests, distinguishing option ids and omitting inactive alternatives', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('rejected', { status: 409 }))
    vi.stubGlobal('fetch', fetchMock)
    const mounted = render(<MountedForm form={openForm} ready active />)
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'draft survives' } })
    fireEvent.change(screen.getByLabelText('A field'), { target: { value: 'edited A' } })
    fireEvent.click(screen.getAllByLabelText('Same label')[1]!)
    fireEvent.click(screen.getByRole('radio', { name: 'B' }))
    expect(screen.getByLabelText('B field')).toBeTruthy()
    expect(screen.queryByLabelText('A field')).toBeNull()
    expect((screen.getByLabelText('B field') as HTMLInputElement).value).toBe('b value')
    fireEvent.change(screen.getByLabelText('B field'), { target: { value: 'edited B' } })
    fireEvent.click(screen.getByRole('radio', { name: 'A' }))
    expect((screen.getByLabelText('A field') as HTMLInputElement).value).toBe('edited A')
    fireEvent.click(screen.getByRole('radio', { name: 'B' }))
    expect((screen.getByLabelText('B field') as HTMLInputElement).value).toBe('edited B')
    mounted.rerender(<MountedForm form={{ ...openForm, errors: [{ field: 'name', message: 'Try again' }] }} ready active />)
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1))
    const payload = JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body)) as { draft: Record<string, unknown> }
    expect(payload.draft).toEqual({ name: 'draft survives', choice: 'second', mode: 'b', onlyB: 'edited B' })
    await screen.findByText(/This form is no longer open/)
    expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe('draft survives')
    expect((screen.getAllByLabelText('Same label')[1] as HTMLInputElement).checked).toBe(true)
  })

  it('renders terminal form answers and submitted values read-only', () => {
    const terminal: StoredActorForm = { ...openForm, state: 'answered', draft: { name: 'final' }, answer: { kind: 'markdown', text: '**done**' } }
    render(<MountedForm form={terminal} ready active />)
    expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull()
    expect(screen.getByText('final')).toBeTruthy()
    expect(screen.getByText('done').tagName).toBe('STRONG')
    expect(screen.queryByRole('button', { name: 'Submit' })).toBeNull()
  })

  it('hydrates a reopened form from its stored server draft when browser retention is empty', () => {
    render(<MountedForm form={{ ...openForm, state: 'submitted', draft: { name: 'server draft' } }} ready active />)
    expect(screen.getByText('server draft')).toBeTruthy()
    expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull()
    expect(isStoredActorForm(openForm)).toBe(true)
    expect(isStoredActorForm({ ...openForm, revisionSequence: -1 })).toBe(false)
  })

  it('accepts newer validation snapshots while ignoring stale or equal revision downgrades', async () => {
    const mounted = render(<MountedForm form={{ ...openForm, state: 'submitted', revisionSequence: 20, draft: { name: 'sent' } }} ready active />)
    mounted.rerender(<MountedForm form={{ ...openForm, state: 'open', revisionSequence: 19, draft: { name: 'stale' } }} ready active />)
    expect(screen.queryByRole('button', { name: 'Submit' })).toBeNull()
    mounted.rerender(<MountedForm form={{ ...openForm, state: 'open', revisionSequence: 20, draft: { name: 'equal stale' } }} ready active />)
    expect(screen.queryByRole('button', { name: 'Submit' })).toBeNull()
    mounted.rerender(<MountedForm form={{ ...openForm, state: 'open', revisionSequence: 21, draft: { name: 'retry draft' } }} ready active />)
    await screen.findByRole('button', { name: 'Submit' })
    expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe('retry draft')
  })

  it('allows dismissing a submitted form while keeping its controls read-only', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('rejected', { status: 409 }))
    vi.stubGlobal('fetch', fetchMock)
    render(<MountedForm form={{ ...openForm, state: 'submitted', draft: { name: 'sent' } }} ready active />)
    expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Submit' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce())
    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/actor-form/dismiss')
    expect(JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body))).not.toHaveProperty('draft')
  })

  it('replaces browser edits with the durable response on the same accepted card and keeps its status current', async () => {
    const mounted = render(<MountedForm form={openForm} ready active />)
    const card = screen.getByRole('region', { name: /^Actor form / })
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'older local edit' } })
    mounted.rerender(<MountedForm form={{ ...openForm, revisionSequence: 12, state: 'answered', draft: { name: 'durable response' }, answer: { kind: 'text', text: 'Accepted' } }} ready active />)
    expect(screen.getByRole('region', { name: /^Actor form / })).toBe(card)
    expect(screen.getByText('durable response')).toBeTruthy()
    expect(screen.queryByDisplayValue('older local edit')).toBeNull()
    expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Submit' })).toBeNull()
    expect(screen.getByRole('heading', { name: 'Form · response accepted' })).toBeTruthy()
  })

  it('uses the posted durable result for the card status before the next conversation refresh', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify({ ...openForm, revisionSequence: 11, state: 'submitted', attemptId: 'attempt', draft: { name: 'posted' } }))))
    render(<MountedForm form={openForm} ready active />)
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'posted' } })
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await screen.findByRole('heading', { name: 'Form · response submitted' })
    expect(screen.getByText('posted')).toBeTruthy()
    expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull()
  })

  it('preserves integers beyond JavaScript safe range exactly through submission', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('rejected', { status: 409 }))
    vi.stubGlobal('fetch', fetchMock)
    const exact = '9007199254740993'
    render(<MountedForm form={integerForm(exact)} ready active />)
    const input = screen.getByLabelText('Integer') as HTMLInputElement
    expect(input.type).toBe('text')
    expect(input.value).toBe(exact)
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce())
    const payload = JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body)) as { draft: Record<string, unknown> }
    expect(payload.draft.integer).toBe(exact)
  })

  it('forwards malformed partial integer lexemes unchanged for server validation', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('rejected', { status: 400 }))
    vi.stubGlobal('fetch', fetchMock)
    render(<MountedForm form={integerForm(null)} ready active />)
    fireEvent.change(screen.getByLabelText('Integer'), { target: { value: '1.5' } })
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce())
    const payload = JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body)) as { draft: Record<string, unknown> }
    expect(payload.draft.integer).toBe('1.5')
  })

  it('isolates radio DOM state and submitted drafts for identical fields in mounted forms', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('rejected', { status: 409 }))
    vi.stubGlobal('fetch', fetchMock)
    render(<><MountedForm form={radioForm('mount-a')} ready active /><MountedForm form={radioForm('mount-b')} ready active /></>)
    const [formA, formB] = screen.getAllByRole('region', { name: /^Actor form / })
    const cardA = within(formA!)
    const cardB = within(formB!)
    fireEvent.click(cardA.getByLabelText(/Quick/))
    fireEvent.click(cardB.getByLabelText(/Careful/))
    expect((cardA.getByLabelText(/Quick/) as HTMLInputElement).checked).toBe(true)
    expect((cardA.getByLabelText(/Careful/) as HTMLInputElement).checked).toBe(false)
    expect((cardB.getByLabelText(/Quick/) as HTMLInputElement).checked).toBe(false)
    expect((cardB.getByLabelText(/Careful/) as HTMLInputElement).checked).toBe(true)
    fireEvent.click(cardA.getByRole('button', { name: 'Submit' }))
    fireEvent.click(cardB.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2))
    const submitted = fetchMock.mock.calls.map(call => JSON.parse(String(call[1]?.body)) as { origin: unknown; mountId: string; draft: Record<string, unknown> })
    expect(submitted.find(item => item.mountId === 'mount-a')?.draft).toEqual({ f0: 'o0' })
    expect(submitted.find(item => item.mountId === 'mount-b')?.draft).toEqual({ f0: 'o1' })
  })

  it('submits an empty number input as null', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('rejected', { status: 409 }))
    vi.stubGlobal('fetch', fetchMock)
    render(<MountedForm form={numberForm()} ready active />)
    fireEvent.change(screen.getByLabelText('Number'), { target: { value: '1.25' } })
    fireEvent.change(screen.getByLabelText('Number'), { target: { value: '' } })
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce())
    const payload = JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body)) as { draft: Record<string, unknown> }
    expect(payload.draft).toEqual(preparedBoundaries.cases.find(item => item.name === 'blank-number-null')!.values)
  })

  it('accepts empty many descriptors and submits their empty selection', async () => {
    const emptyMany: StoredActorForm = { ...openForm, opening: { ...openForm.opening, form: prepared('empty-many') } }
    expect(isFormSpec(emptyMany.opening.form)).toBe(true)
    expect(isStoredActorForm(emptyMany)).toBe(true)
    expect(isFormSpec({ version: 1, root: { kind: 'choice', id: 'choice', label: 'Choose one', options: [], initial: null } })).toBe(false)
    expect(isFormSpec({ version: 1, root: { kind: 'alternatives', id: 'mode', label: 'Mode', options: [], initial: null } })).toBe(false)
    const fetchMock = vi.fn().mockResolvedValue(new Response('rejected', { status: 409 }))
    vi.stubGlobal('fetch', fetchMock)
    render(<MountedForm form={emptyMany} ready active />)
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce())
    const payload = JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body)) as { draft: Record<string, unknown> }
    expect(payload.draft).toEqual(preparedBoundaries.cases.find(item => item.name === 'empty-many')!.values)
  })

  it('accepts Rust-bounded option counts, labels, and form errors', () => {
    const options = Array.from({ length: 300 }, (_, index) => ({ id: `option-${index}`, label: index === 0 ? 'x'.repeat(9000) : `Option ${index}`, presentation: view }))
    const form = { version: 1 as const, root: { kind: 'choice' as const, id: 'choice', label: 'Pick', options, initial: null } }
    expect(isFormSpec(form)).toBe(true)
    expect(isFormSpec({ version: 1, root: { kind: 'choice', id: 'choice', label: 'Pick', options: [{ id: 'i'.repeat(129), label: 'Option', presentation: view }], initial: null } })).toBe(false)
    expect(isStoredActorForm({ ...openForm, errors: Array.from({ length: 1024 }, () => ({ field: null, message: 'x' })) })).toBe(true)
    expect(isStoredActorForm({ ...openForm, errors: Array.from({ length: 1025 }, () => ({ field: null, message: 'x' })) })).toBe(false)
    expect(isStoredActorForm({ ...openForm, errors: [{ field: null, message: 'x'.repeat(32 * 1024 + 1) }] })).toBe(false)
  })
})
