import { useId, useRef, useState } from 'react'
import { RichViewRenderer } from './rich-view'
import type { StoredActorForm } from './form-model'
import { Validation, FormNodeView, ResponseSummary, fieldLabel } from './FormFields'
import { useMountedForm, type MountedFormProps } from './useMountedForm'
import './MountedForm.css'
export { isFormSpec, isStoredActorForm, clearMountedFormDrafts } from './form-model'
export type { StoredActorForm, FormSpec, FormNode, FormOption, FormOrigin, FormExecution, FormConversation, FormDraft, DraftValue } from './form-model'

export type { MountedFormProps } from './useMountedForm'
export default function MountedForm(props: MountedFormProps) {
  const { ready, active } = props
  const section = useRef<HTMLElement>(null)
  const prefix = useId()
  const [showDetails, setShowDetails] = useState(false)
  const { key, setDraft, busy, issue, latest, editable, displayedDraft, dismissible, submit } = useMountedForm(props, section)
  return <form onSubmit={event => { event.preventDefault(); void submit(false) }} aria-label={`Respond to actor form ${latest.sequence}`} aria-busy={busy}><section ref={section} tabIndex={-1} className="mounted-form" aria-label={`Actor form ${latest.sequence}`} data-state={latest.state} data-mount-id={latest.opening.mountId}>
    <h3>Form · {formStatus[latest.state]}</h3>
    {latest.state === 'submitted' && <p role="status">Response submitted. Waiting for the actor to process it.</p>}
    {latest.state !== 'open' && <p className="meta">This response is read-only.</p>}
    {latest.state === 'open' ? <Validation.Provider value={{ prefix, errors: latest.errors }}>
      <FormNodeView scope={key} node={latest.opening.form.root} draft={displayedDraft} setDraft={setDraft} editable={editable && !busy} />
    </Validation.Provider> : <>
      <ResponseSummary node={latest.opening.form.root} draft={displayedDraft} />
      <details open={showDetails} onToggle={event => setShowDetails(event.currentTarget.open)}><summary>Full form details</summary>
        {showDetails && <FormNodeView scope={key} node={latest.opening.form.root} draft={displayedDraft} setDraft={setDraft} editable={false} readOnly />}
      </details>
    </>}
    {!!latest.errors.length && <div role="alert"><ul className="form-errors" aria-label="Form errors">{latest.errors.map((error, i) => <li id={`${prefix}-error-${i}`} key={`${error.field ?? ''}:${i}`}>{error.field && <strong>{fieldLabel(latest.opening.form.root, error.field) ?? 'Field'}: </strong>}{error.message}</li>)}</ul></div>}
    {issue && <p role="alert">{issue}</p>}
    {latest.answer && <div className="form-answer"><h4>Answer</h4><RichViewRenderer view={latest.answer} /></div>}

    {(latest.state === 'open' || latest.state === 'submitted') && <div className="form-actions">
      {latest.state === 'open' && <button type="submit" disabled={!editable || busy}>{busy ? 'Sending…' : 'Submit'}</button>}
      <button type="button" disabled={!dismissible || busy} onClick={() => void submit(true)}>Dismiss</button>
    </div>}
    {!active && latest.state === 'open' && <p role="status">This actor is inactive; the form is read-only.</p>}
    {!ready && active && latest.state === 'open' && <p role="status">Connection unavailable. Your draft is kept here; reconnect to respond.</p>}
  </section></form>
}

const formStatus: Record<StoredActorForm['state'], string> = { open: 'response required', submitted: 'response submitted', answered: 'response accepted', dismissed: 'dismissed', cancelled: 'cancelled', interrupted: 'interrupted' }
