import { useEffect, useState } from 'react'
import { RichViewRenderer, isRichView, type RichView } from './rich-view'

import { isStoredActorForm as isFormShape } from './generated/validators.mjs'
import type { StoredActorForm as WireForm } from './generated/actor-form'
import { compareDecimal } from './decimal'
export type StoredActorForm = WireForm
export type FormSpec = StoredActorForm['opening']['form']
export type FormNode = FormSpec['root']
export type FormOption = Extract<FormNode, { kind: 'choice' }>['options'][number]
export type FormOrigin = StoredActorForm['opening']['origin']
export type FormExecution = StoredActorForm['opening']['execution']
export type FormConversation = NonNullable<StoredActorForm['opening']['conversation']>
export type FormDraft = NonNullable<StoredActorForm['draft']>
export type DraftValue = FormDraft[string]
type Obj = Record<string, unknown>
const obj = (value: unknown): value is Obj => typeof value === 'object' && value !== null && !Array.isArray(value)
const text = (value: unknown, max = 128 * 1024): value is string => typeof value === 'string' && new TextEncoder().encode(value).length <= max

function option(value: unknown): value is FormOption {
  return obj(value) && text(value.id, 128) && value.id.length > 0 && text(value.label) && isRichView(value.presentation)
}
const decimalInteger = (value: unknown): value is string => typeof value === 'string' && /^-?\d+$/.test(value)
function formNode(value: unknown, depth = 0, ids = new Set<string>()): value is FormNode {
  if (!obj(value) || depth > 32 || typeof value.kind !== 'string') return false
  switch (value.kind) {
    case 'pure': case 'empty': return true
    case 'group': return Array.isArray(value.children) && value.children.length <= 1024 && value.children.every(child => formNode(child, depth + 1, ids))
    case 'section': return text(value.title) && formNode(value.child, depth + 1, ids)
    case 'view': return isRichView(value.presentation)
    case 'text': case 'int': case 'number': case 'slider': case 'bool': {
      if (!text(value.id, 128) || value.id.length === 0 || ids.has(value.id) || !text(value.label)) return false
      ids.add(value.id)
      if (value.kind === 'slider') return typeof value.minimum === 'number' && Number.isFinite(value.minimum) && typeof value.maximum === 'number' && Number.isFinite(value.maximum) && value.minimum < value.maximum && typeof value.initial === 'number' && Number.isFinite(value.initial) && value.initial >= value.minimum && value.initial <= value.maximum
      if (value.initial === null) return true
      return value.kind === 'text' ? typeof value.initial === 'string' : value.kind === 'int' ? decimalInteger(value.initial)
        : value.kind === 'bool' ? typeof value.initial === 'boolean' : typeof value.initial === 'number' && Number.isFinite(value.initial)
    }
    case 'choice': case 'many': case 'alternatives_many': case 'alternatives': {
      if (!text(value.id, 128) || value.id.length === 0 || ids.has(value.id) || !text(value.label) || !Array.isArray(value.options)
        || value.options.length > 1024 || value.kind !== 'many' && value.kind !== 'alternatives_many' && value.options.length === 0) return false
      ids.add(value.id)
      const optionIds = new Set<string>()
      if (!value.options.every(item => obj(item) && option(item) && !optionIds.has(item.id) && !!optionIds.add(item.id)
        && ((value.kind !== 'alternatives' && value.kind !== 'alternatives_many') || formNode(item.form, depth + 1, ids)))) return false
      if (value.initial === null) return true
      if (value.kind === 'many' || value.kind === 'alternatives_many') return Array.isArray(value.initial) && new Set(value.initial).size === value.initial.length && value.initial.every(item => typeof item === 'string' && optionIds.has(item))
      return typeof value.initial === 'string' && optionIds.has(value.initial)
    }
    default: return false
  }
}
export function isFormSpec(value: unknown): value is FormSpec {
  return obj(value) && value.version === 1 && formNode(value.root)
}
export function isStoredActorForm(value: unknown): value is StoredActorForm {
  return isFormShape(value) && BigInt(value.sequence) > 0n
    && compareDecimal(value.revisionSequence, value.sequence) >= 0
    && value.opening.origin.run.length > 0 && value.opening.mountId.length > 0
    && isFormSpec(value.opening.form) && isFormErrors(value.errors)
    && (value.answer === null || isRichView(value.answer))
}
function isFormErrors(value: unknown): value is StoredActorForm['errors'] {
  if (!Array.isArray(value) || value.length > 1024) return false
  let bytes = 0
  for (const error of value) {
    if (!obj(error) || !(error.field === null || text(error.field, 256)) || !text(error.message)) return false
    bytes += new TextEncoder().encode(error.field ?? '').length + new TextEncoder().encode(error.message).length
    if (bytes > 32 * 1024) return false
  }
  return true
}
function isFormDraft(value: unknown): value is FormDraft {
  return obj(value) && Object.entries(value).length <= 1024 && Object.entries(value).every(([key, item]) => key.length <= 128
    && (item === null || typeof item === 'string' || typeof item === 'boolean'
      || typeof item === 'number' && Number.isFinite(item)
      || Array.isArray(item) && item.length <= 1024 && item.every(entry => typeof entry === 'string')))
}
const retainedDrafts = new Map<string, FormDraft>()
const MAX_DRAFTS = 32
const keyOf = (form: StoredActorForm) => JSON.stringify([form.opening.origin.run, form.opening.origin.nativeActor, form.opening.origin.incarnation, form.opening.mountId])
export function clearMountedFormDrafts() { retainedDrafts.clear() }
function keepDraft(key: string, draft: FormDraft) {
  retainedDrafts.delete(key); retainedDrafts.set(key, draft)
  while (retainedDrafts.size > MAX_DRAFTS) retainedDrafts.delete(retainedDrafts.keys().next().value!)
}
function initialDraft(node: FormNode, into: FormDraft = Object.create(null) as FormDraft): FormDraft {
  if (['text', 'int', 'number', 'slider', 'bool', 'choice', 'many', 'alternatives', 'alternatives_many'].includes(node.kind)) {
    const field = node as Extract<FormNode, { id: string }>
    if (field.initial !== null && !Object.hasOwn(into, field.id)) into[field.id] = field.kind === 'many' || field.kind === 'alternatives_many' ? [...field.initial] : field.initial
    if ((node.kind === 'many' || node.kind === 'alternatives_many') && node.options.length === 0 && !Object.hasOwn(into, field.id)) into[field.id] = []
  }
  if (node.kind === 'group') node.children.forEach(child => initialDraft(child, into))
  if (node.kind === 'section') initialDraft(node.child, into)
  if (node.kind === 'alternatives') {
    const selected = Object.hasOwn(into, node.id) ? into[node.id] : node.initial
    const branch = typeof selected === 'string' ? node.options.find(item => item.id === selected) : undefined
    if (branch) initialDraft(branch.form, into)
  }
  if (node.kind === 'alternatives_many') {
    const selected = into[node.id]
    node.options.filter(item => Array.isArray(selected) && selected.includes(item.id)).forEach(item => initialDraft(item.form, into))
  }
  return into
}
function hydratedDraft(form: StoredActorForm, cached?: FormDraft): FormDraft {
  const draft = { ...initialDraft(form.opening.form.root), ...(form.draft ?? {}), ...(cached ?? {}) }
  return initialDraft(form.opening.form.root, draft)
}
function activeDraft(node: FormNode, draft: FormDraft, into: FormDraft = Object.create(null) as FormDraft): FormDraft {
  if (['text', 'int', 'number', 'slider', 'bool', 'choice', 'many', 'alternatives', 'alternatives_many'].includes(node.kind)) {
    const field = node as Extract<FormNode, { id: string }>
    const value = draft[field.id]
    if (value !== undefined) into[field.id] = value
  }
  if (node.kind === 'group') node.children.forEach(child => activeDraft(child, draft, into))
  if (node.kind === 'section') activeDraft(node.child, draft, into)
  if (node.kind === 'alternatives') {
    const selectedValue = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
    const selected = typeof selectedValue === 'string' ? selectedValue : undefined
    const branch = node.options.find(item => item.id === selected)
    if (branch) activeDraft(branch.form, draft, into)
  }
  if (node.kind === 'alternatives_many') {
    const selected = draft[node.id]
    node.options.filter(item => Array.isArray(selected) && selected.includes(item.id)).forEach(item => activeDraft(item.form, draft, into))
  }
  return into
}

export default function MountedForm({ form, ready, active, onAuthExpired }: { form: StoredActorForm; ready: boolean; active: boolean; onAuthExpired?: () => void }) {
  const key = keyOf(form)
  const [draft, setDraft] = useState<FormDraft>(() => hydratedDraft(form, retainedDrafts.get(key)))
  const [busy, setBusy] = useState(false)
  const [issue, setIssue] = useState('')
  const [latest, setLatest] = useState(form)
  const editable = latest.state === 'open' && active && ready
  const displayedDraft = latest.state === 'open' ? draft : hydratedDraft(latest)
  const dismissible = (latest.state === 'open' || latest.state === 'submitted') && active && ready
  useEffect(() => {
    const stale = compareDecimal(form.revisionSequence, latest.revisionSequence) < 0
      || (form.revisionSequence === latest.revisionSequence && latest.state !== 'open' && form.state === 'open')
    if (stale) return
    if (latest !== form) setLatest(form)
    if (form.state !== 'open') {
      retainedDrafts.delete(key)
      setDraft(hydratedDraft(form))
    } else if (!retainedDrafts.has(key)) setDraft(hydratedDraft(form))
  }, [form, key, latest])
  useEffect(() => {
    if (editable) keepDraft(key, draft)
  }, [key, draft, editable])
  async function submit(dismiss: boolean) {
    if ((dismiss ? !dismissible : !editable) || busy) return
    setBusy(true); setIssue('')
    try {
      const { origin, mountId } = latest.opening
      const response = await fetch(`/api/actor-form/${dismiss ? 'dismiss' : 'submit'}`, {
        method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ origin, mountId, operationId: crypto.randomUUID(), ...(dismiss ? {} : { draft: activeDraft(latest.opening.form.root, draft) }) }),
      })
      if (response.status === 401 || response.status === 403) onAuthExpired?.()
      if (!response.ok) throw new Error(response.status === 409 ? 'This form is no longer open. Refresh the conversation.' : 'The form response was rejected. Your draft is still here.')
      const value: unknown = await response.json()
      if (!isStoredActorForm(value) || keyOf(value) !== key) throw new Error('The server returned an invalid form response. Your draft is still here.')
      setLatest(current => compareDecimal(value.revisionSequence, current.revisionSequence) < 0
        || (value.revisionSequence === current.revisionSequence && current.state !== 'open' && value.state === 'open') ? current : value)
      if (value.state !== 'open') retainedDrafts.delete(key)
      else keepDraft(key, draft)
    } catch (error) { setIssue(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(false) }
  }
  return <section className="mounted-form" aria-label={`Actor form ${latest.sequence}`} data-mount-id={latest.opening.mountId}>
    <h3>Form · {formStatus[latest.state]}</h3>
    {latest.state === 'submitted' && <p role="status">Response submitted. Waiting for the actor to process it.</p>}
    {latest.state !== 'open' && <p className="meta">This response is read-only.</p>}
    <FormNodeView scope={key} node={latest.opening.form.root} draft={displayedDraft} setDraft={setDraft} editable={editable} readOnly={latest.state !== 'open'} />
    {!!latest.errors.length && <div role="alert"><ul className="form-errors" aria-label="Form errors">{latest.errors.map((error, i) => <li key={`${error.field ?? ''}:${i}`}>{error.field && <strong>{fieldLabel(latest.opening.form.root, error.field) ?? 'Field'}: </strong>}{error.message}</li>)}</ul></div>}
    {issue && <p role="alert">{issue}</p>}
    {latest.answer && <div className="form-answer"><h4>Answer</h4><RichViewRenderer view={latest.answer} /></div>}

    {(latest.state === 'open' || latest.state === 'submitted') && <div className="form-actions">
      {latest.state === 'open' && <button type="button" disabled={!editable || busy} onClick={() => void submit(false)}>{busy ? 'Sending…' : 'Submit'}</button>}
      <button type="button" disabled={!dismissible || busy} onClick={() => void submit(true)}>Dismiss</button>
    </div>}
    {!active && latest.state === 'open' && <p role="status">This actor is inactive; the form is read-only.</p>}
  </section>
}

const formStatus: Record<StoredActorForm['state'], string> = { open: 'response required', submitted: 'response submitted', answered: 'response accepted', dismissed: 'dismissed', cancelled: 'cancelled', interrupted: 'interrupted' }

function fieldLabel(node: FormNode, id: string): string | undefined {
  if ('id' in node && node.id === id) return node.label
  if (node.kind === 'group') return node.children.map(child => fieldLabel(child, id)).find(label => label !== undefined)
  if (node.kind === 'section') return fieldLabel(node.child, id)
  if (node.kind === 'alternatives' || node.kind === 'alternatives_many') return node.options.map(option => fieldLabel(option.form, id)).find(label => label !== undefined)
  return undefined
}

function FormNodeView({ scope, node, draft, setDraft, editable, readOnly = false }: { scope: string; node: FormNode; draft: FormDraft; setDraft: (draft: FormDraft) => void; editable: boolean; readOnly?: boolean }) {
  const change = (id: string, value: DraftValue, base = draft) => setDraft({ ...base, [id]: value })
  if (node.kind === 'pure' || node.kind === 'empty') return null
  if (node.kind === 'group') return <div className="form-group">{node.children.map((child, i) => <FormNodeView key={i} scope={scope} node={child} draft={draft} setDraft={setDraft} editable={editable} readOnly={readOnly} />)}</div>
  if (node.kind === 'section') return <fieldset className="form-section"><legend>{node.title}</legend><FormNodeView scope={scope} node={node.child} draft={draft} setDraft={setDraft} editable={editable} readOnly={readOnly} /></fieldset>
  if (node.kind === 'view') return <RichViewRenderer view={node.presentation} />
  if (readOnly && 'id' in node) return <ReadOnlyField scope={scope} node={node} draft={draft} setDraft={setDraft} />
  if (node.kind === 'alternatives') {
    const selectedValue = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
    const selected = typeof selectedValue === 'string' ? selectedValue : ''
    const branch = node.options.find(item => item.id === selected)
    return <fieldset><legend>{node.label}</legend>{node.options.map(item => <label className="form-option" key={item.id}>
      <input type="radio" aria-label={item.label} name={`${scope}:${node.id}`} value={item.id} checked={selected === item.id} disabled={!editable} onChange={() => change(node.id, item.id, initialDraft(item.form, { ...draft }))} />
      <RichViewRenderer view={item.presentation} />
    </label>)}{branch && <FormNodeView scope={scope} node={branch.form} draft={draft} setDraft={setDraft} editable={editable} readOnly={readOnly} />}</fieldset>
  }
  if (node.kind === 'alternatives_many') {
    const value = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
    const selected = Array.isArray(value) ? value : []
    return <fieldset><legend>{node.label}</legend>{node.options.map(item => <div key={item.id}>
      <label className="form-option"><input type="checkbox" aria-label={item.label} checked={selected.includes(item.id)} disabled={!editable}
        onChange={event => change(node.id, event.target.checked ? [...selected, item.id] : selected.filter(id => id !== item.id), initialDraft(item.form, { ...draft }))} />
        <RichViewRenderer view={item.presentation} /></label>
      {selected.includes(item.id) && <FormNodeView scope={scope} node={item.form} draft={draft} setDraft={setDraft} editable={editable} readOnly={readOnly} />}
    </div>)}</fieldset>
  }
  if (node.kind === 'choice' || node.kind === 'many') return <fieldset><legend>{node.label}</legend>{node.options.map(item => {
    const hasValue = Object.hasOwn(draft, node.id)
    const selected = node.kind === 'many' ? (Array.isArray(draft[node.id]) ? draft[node.id] as string[] : !hasValue && Array.isArray(node.initial) ? [...node.initial] : [])
      : typeof draft[node.id] === 'string' ? [draft[node.id] as string] : !hasValue && node.initial ? [node.initial] : []
    return <label className="form-option" key={item.id}><input aria-label={item.label} type={node.kind === 'many' ? 'checkbox' : 'radio'} name={`${scope}:${node.id}`} value={item.id} checked={selected.includes(item.id)} disabled={!editable}
      onChange={event => change(node.id, node.kind === 'many' ? event.target.checked ? [...selected, item.id] : selected.filter(id => id !== item.id) : item.id)} />
      <RichViewRenderer view={item.presentation} /></label>
  })}</fieldset>
  const value = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
  if (node.kind === 'slider') return <label className="form-field">{node.label}<input type="range" min={node.minimum} max={node.maximum} step="any" value={Number(value)} disabled={!editable}
    onChange={event => change(node.id, Number(event.target.value))} /><output>{String(value)}</output></label>
  if (node.kind === 'bool') return <label className="form-toggle"><input type="checkbox" checked={value === true} disabled={!editable} onChange={event => change(node.id, event.target.checked)} />{node.label}</label>
  if (node.kind === 'int') return <label className="form-field">{node.label}<input type="text" inputMode="numeric" value={value === null ? '' : String(value)} disabled={!editable}
    onChange={event => change(node.id, event.target.value)} /></label>
  if (node.kind === 'text') return <label className="form-field">{node.label}<input type="text" value={value === null ? '' : String(value)} disabled={!editable}
    onChange={event => change(node.id, event.target.value)} /></label>
  return <label className="form-field">{node.label}<input type="number" step="any" value={value === null ? '' : String(value)} disabled={!editable}
    onChange={event => { const raw = event.target.value; change(node.id, raw === '' ? null : Number(raw)) }} /></label>
}

function ReadOnlyField({ scope, node, draft, setDraft }: { scope: string; node: Extract<FormNode, { id: string }>; draft: FormDraft; setDraft: (draft: FormDraft) => void }) {
  const value = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
  if (node.kind === 'choice' || node.kind === 'many' || node.kind === 'alternatives' || node.kind === 'alternatives_many') {
    const selected = Array.isArray(value) ? value : typeof value === 'string' ? [value] : []
    const branches = node.kind === 'alternatives' || node.kind === 'alternatives_many' ? node.options.filter(option => selected.includes(option.id)) : []
    return <fieldset><legend>{node.label}</legend><ul className="form-fixed-options">{node.options.map(option => <li key={option.id} aria-label={option.label}>
      <span className="form-selection">{selected.includes(option.id) ? 'Selected' : 'Not selected'}</span><RichViewRenderer view={option.presentation} />
    </li>)}</ul>{!node.options.length && <p>No selections.</p>}{branches.map(branch => <FormNodeView key={branch.id} scope={scope} node={branch.form} draft={draft} setDraft={setDraft} editable={false} readOnly />)}</fieldset>
  }
  return <dl className="form-fixed-field"><dt>{node.label}</dt><dd>{value === null ? 'No response' : node.kind === 'bool' ? value ? 'Yes' : 'No' : String(value)}</dd></dl>
}
