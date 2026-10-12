import { useEffect, useState, type RefObject } from 'react'
import { formKey, hydratedDraft, activeDraft, readFormDraft, hasFormDraft, discardFormDraft, keepDraft, formRevisionIsStale, isStoredActorForm, type FormDraft, type StoredActorForm } from './form-model'
import { allocateOperationId } from './pending-commands'
import { useFormFocus } from './useFormFocus'
export interface MountedFormProps {
  form: StoredActorForm
  ready: boolean
  active: boolean
  onAuthExpired?: () => void
  onStateChange?: (form: StoredActorForm) => void
}

export function useMountedForm({ form, ready, active, onAuthExpired, onStateChange }: MountedFormProps, section: RefObject<HTMLElement>) {
  const key = formKey(form)
  const [draft, setDraft] = useState<FormDraft>(() => hydratedDraft(form, readFormDraft(key)))
  const [busy, setBusy] = useState(false)
  const [issue, setIssue] = useState('')
  const [latest, setLatest] = useState(form)
  const focusIntent = useFormFocus(section, latest, busy)
  const editable = latest.state === 'open' && active && ready
  const displayedDraft = latest.state === 'open' ? draft : hydratedDraft(latest)
  const dismissible = (latest.state === 'open' || latest.state === 'submitted') && active && ready
  useEffect(() => {
    const stale = formRevisionIsStale(form, latest)
    if (stale) return
    if (latest !== form) setLatest(form)
    if (form.state !== 'open') {
      discardFormDraft(key)
      setDraft(hydratedDraft(form))
    } else if (!hasFormDraft(key)) setDraft(hydratedDraft(form))
  }, [form, key, latest])
  useEffect(() => {
    if (editable) keepDraft(key, draft)
  }, [key, draft, editable])
  async function submit(dismiss: boolean) {
    if ((dismiss ? !dismissible : !editable) || busy) return
    setBusy(true); setIssue('')
    try {
      const { origin, mountId } = latest.opening
      const operationId = allocateOperationId()
      focusIntent.current = dismiss ? undefined : { attemptId: operationId, startRevision: latest.revisionSequence, responseReceived: false, submissionFocused: false }
      const response = await fetch(`/api/actor-form/${dismiss ? 'dismiss' : 'submit'}`, {
        method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ origin, mountId, operationId, ...(dismiss ? {} : { draft: activeDraft(latest.opening.form.root, draft) }) }),
      })
      if (response.status === 401 || response.status === 403) onAuthExpired?.()
      if (!response.ok) throw new Error(response.status === 409 ? 'This form is no longer open. Refresh the conversation.' : 'The form response was rejected. Your draft is still here.')
      const value: unknown = await response.json()
      if (!isStoredActorForm(value) || formKey(value) !== key) throw new Error('The server returned an invalid form response. Your draft is still here.')
      if (focusIntent.current?.attemptId === operationId) focusIntent.current.responseReceived = true
      setLatest(current => formRevisionIsStale(value, current) ? current : value)
      onStateChange?.(value)
      if (value.state !== 'open') discardFormDraft(key)
      else keepDraft(key, draft)
    } catch (error) { focusIntent.current = undefined; setIssue(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(false) }
  }
  return { key, draft, setDraft, busy, issue, latest, editable, displayedDraft, dismissible, submit }
}
