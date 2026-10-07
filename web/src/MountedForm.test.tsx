import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import MountedForm, { clearMountedFormDrafts, type StoredActorForm } from './MountedForm'

const origin = { run: 'run', nativeActor: 4, incarnation: 2 }
const view = { kind: 'text' as const, text: 'Option details' }
const openForm: StoredActorForm = {
  sequence: 9, opening: { origin, mountId: 'mount-1', execution: 'exec', conversation: 'conv', form: { version: 1, root: { kind: 'group', children: [
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

beforeEach(() => { clearMountedFormDrafts(); vi.restoreAllMocks() })

describe('mounted actor forms', () => {
  it('retains edits across rerenders and rejected requests, distinguishing option ids and omitting inactive alternatives', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('rejected', { status: 409 }))
    vi.stubGlobal('fetch', fetchMock)
    const mounted = render(<MountedForm form={openForm} ready active />)
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'draft survives' } })
    fireEvent.click(screen.getAllByLabelText('Same label')[1]!)
    fireEvent.click(screen.getByRole('radio', { name: /B/ }))
    expect(screen.getByLabelText('B field')).toBeTruthy()
    expect(screen.queryByLabelText('A field')).toBeNull()
    mounted.rerender(<MountedForm form={{ ...openForm, errors: [{ field: 'name', message: 'Try again' }] }} ready active />)
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1))
    const payload = JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body)) as { draft: Record<string, unknown> }
    expect(payload.draft).toEqual({ name: 'draft survives', choice: 'second', mode: 'b', onlyB: 'b value' })
    await screen.findByRole('alert')
    expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe('draft survives')
    expect((screen.getAllByLabelText('Same label')[1] as HTMLInputElement).checked).toBe(true)
  })

  it('renders terminal form answers and submitted values read-only', () => {
    const terminal: StoredActorForm = { ...openForm, state: 'answered', draft: { name: 'final' }, answer: { kind: 'markdown', text: '**done**' } }
    render(<MountedForm form={terminal} ready active />)
    expect((screen.getByLabelText('Name') as HTMLInputElement).disabled).toBe(true)
    expect(screen.getByText('Submitted values').tagName).toBe('H4')
    expect(screen.getByText(/"name": "final"/)).toBeTruthy()
    expect(screen.getByText('**done**')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Submit' })).toBeNull()
  })
})
