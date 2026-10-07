import { useEffect, useState } from 'react'
import { RichViewRenderer, isRichView, type RichView } from './rich-view'

export interface FormOption { readonly id: string; readonly label: string; readonly presentation: RichView }
export type FormNode =
  | { readonly kind: 'pure' }
  | { readonly kind: 'empty' }
  | { readonly kind: 'group'; readonly children: readonly FormNode[] }
  | { readonly kind: 'section'; readonly title: string; readonly child: FormNode }
  | { readonly kind: 'view'; readonly presentation: RichView }
  | { readonly kind: 'text'; readonly id: string; readonly label: string; readonly initial: string | null }
  | { readonly kind: 'int'; readonly id: string; readonly label: string; readonly initial: string | null }
  | { readonly kind: 'number'; readonly id: string; readonly label: string; readonly initial: number | null }
  | { readonly kind: 'bool'; readonly id: string; readonly label: string; readonly initial: boolean | null }
  | { readonly kind: 'choice'; readonly id: string; readonly label: string; readonly options: readonly FormOption[]; readonly initial: string | null }
  | { readonly kind: 'many'; readonly id: string; readonly label: string; readonly options: readonly FormOption[]; readonly initial: readonly string[] | null }
  | { readonly kind: 'alternatives'; readonly id: string; readonly label: string; readonly options: readonly (FormOption & { readonly form: FormNode })[]; readonly initial: string | null }
export interface FormSpec { readonly version: 1; readonly root: FormNode }
export interface FormOrigin { readonly run: string; readonly nativeActor: number; readonly incarnation: number }
export type FormExecution = { readonly kind: 'actor_program' }
  | { readonly kind: 'notebook'; readonly execution: string; readonly inputUnitIndex: number; readonly effectOrdinal: number }
export type FormConversation = { readonly kind: 'standalone'; readonly store: string; readonly actor: string }
  | { readonly kind: 'embedded'; readonly run: string; readonly actor: string; readonly incarnation: string }
export interface StoredActorForm {
  readonly sequence: number; readonly revisionSequence: number
  readonly opening: { readonly origin: FormOrigin; readonly mountId: string; readonly execution: FormExecution; readonly conversation: FormConversation | null; readonly form: FormSpec }
  readonly state: 'open' | 'submitted' | 'answered' | 'dismissed' | 'cancelled' | 'interrupted'
  readonly attemptId: string | null
  readonly draft: Readonly<FormDraft> | null
  readonly errors: readonly { readonly field: string | null; readonly message: string }[]
  readonly answer: RichView | null
}
export type DraftValue = string | number | boolean | string[] | null
export type FormDraft = Record<string, DraftValue>
type Obj = Record<string, unknown>
const obj = (value: unknown): value is Obj => typeof value === 'object' && value !== null && !Array.isArray(value)
const text = (value: unknown, max = 8192): value is string => typeof value === 'string' && new TextEncoder().encode(value).length <= max

function option(value: unknown): value is FormOption {
  return obj(value) && text(value.id, 256) && value.id.length > 0 && text(value.label) && isRichView(value.presentation)
}
const decimalInteger = (value: unknown): value is string => typeof value === 'string' && /^-?\d+$/.test(value)
function formNode(value: unknown, depth = 0, ids = new Set<string>()): value is FormNode {
  if (!obj(value) || depth > 32 || typeof value.kind !== 'string') return false
  switch (value.kind) {
    case 'pure': case 'empty': return true
    case 'group': return Array.isArray(value.children) && value.children.length <= 1024 && value.children.every(child => formNode(child, depth + 1, ids))
    case 'section': return text(value.title) && formNode(value.child, depth + 1, ids)
    case 'view': return isRichView(value.presentation)
    case 'text': case 'int': case 'number': case 'bool': {
      if (!text(value.id, 128) || value.id.length === 0 || ids.has(value.id) || !text(value.label)) return false
      ids.add(value.id)
      if (value.initial === null) return true
      return value.kind === 'text' ? typeof value.initial === 'string' : value.kind === 'int' ? decimalInteger(value.initial)
        : value.kind === 'bool' ? typeof value.initial === 'boolean' : typeof value.initial === 'number' && Number.isFinite(value.initial)
    }
    case 'choice': case 'many': case 'alternatives': {
      if (!text(value.id, 128) || value.id.length === 0 || ids.has(value.id) || !text(value.label) || !Array.isArray(value.options)
        || value.options.length === 0 || value.options.length > 256) return false
      ids.add(value.id)
      const optionIds = new Set<string>()
      if (!value.options.every(item => obj(item) && option(item) && !optionIds.has(item.id) && !!optionIds.add(item.id)
        && (value.kind !== 'alternatives' || formNode(item.form, depth + 1, ids)))) return false
      if (value.initial === null) return true
      if (value.kind === 'many') return Array.isArray(value.initial) && value.initial.every(item => typeof item === 'string' && optionIds.has(item))
      return typeof value.initial === 'string' && optionIds.has(value.initial)
    }
    default: return false
  }
}
export function isFormSpec(value: unknown): value is FormSpec {
  return obj(value) && value.version === 1 && formNode(value.root)
}
export function isStoredActorForm(value: unknown): value is StoredActorForm {
  if (!obj(value) || !Number.isSafeInteger(value.sequence) || (value.sequence as number) <= 0 || !obj(value.opening)
    || !Number.isSafeInteger(value.revisionSequence) || (value.revisionSequence as number) < 0
    || !obj(value.opening.origin) || !text(value.opening.origin.run, 1024) || !isCounter(value.opening.origin.nativeActor)
    || !isCounter(value.opening.origin.incarnation) || !text(value.opening.mountId, 256) || !isFormExecution(value.opening.execution)
    || !(value.opening.conversation === null || isFormConversation(value.opening.conversation)) || !isFormSpec(value.opening.form)
    || !['open', 'submitted', 'answered', 'dismissed', 'cancelled', 'interrupted'].includes(String(value.state))
    || !(value.attemptId === null || text(value.attemptId, 256)) || !(value.draft === null || isFormDraft(value.draft))
    || !Array.isArray(value.errors) || value.errors.length > 128 || !value.errors.every(error => obj(error)
      && (error.field === null || text(error.field, 256)) && text(error.message, 8192))
    || !(value.answer === null || isRichView(value.answer))) return false
  return true
}
function isFormDraft(value: unknown): value is FormDraft {
  return obj(value) && Object.entries(value).length <= 1024 && Object.entries(value).every(([key, item]) => key.length <= 128
    && (item === null || typeof item === 'string' || typeof item === 'boolean'
      || typeof item === 'number' && Number.isFinite(item)
      || Array.isArray(item) && item.length <= 1024 && item.every(entry => typeof entry === 'string')))
}
function isCounter(value: unknown): value is number { return Number.isSafeInteger(value) && (value as number) >= 0 }
function isFormExecution(value: unknown): value is FormExecution {
  if (!obj(value)) return false
  return value.kind === 'actor_program' || value.kind === 'notebook' && text(value.execution, 256)
    && isCounter(value.inputUnitIndex) && isCounter(value.effectOrdinal)
}
function isFormConversation(value: unknown): value is FormConversation {
  if (!obj(value)) return false
  return value.kind === 'standalone' && text(value.store, 1024) && text(value.actor, 1024)
    || value.kind === 'embedded' && text(value.run, 1024) && text(value.actor, 1024) && text(value.incarnation, 1024)
}

const retainedDrafts = new Map<string, FormDraft>()
const MAX_DRAFTS = 32
const keyOf = (form: StoredActorForm) => JSON.stringify([form.opening.origin.run, form.opening.origin.nativeActor, form.opening.origin.incarnation, form.opening.mountId])
export function clearMountedFormDrafts() { retainedDrafts.clear() }
function keepDraft(key: string, draft: FormDraft) {
  retainedDrafts.delete(key); retainedDrafts.set(key, draft)
  while (retainedDrafts.size > MAX_DRAFTS) retainedDrafts.delete(retainedDrafts.keys().next().value!)
}
function initialDraft(node: FormNode, into: FormDraft = {}): FormDraft {
  if (['text', 'int', 'number', 'bool', 'choice', 'many', 'alternatives'].includes(node.kind)) {
    const field = node as Extract<FormNode, { id: string }>
    if (field.initial !== null && !Object.hasOwn(into, field.id)) into[field.id] = Array.isArray(field.initial) ? [...field.initial] : field.initial
  }
  if (node.kind === 'group') node.children.forEach(child => initialDraft(child, into))
  if (node.kind === 'section') initialDraft(node.child, into)
  if (node.kind === 'alternatives') {
    const selected = Object.hasOwn(into, node.id) ? into[node.id] : node.initial
    const branch = typeof selected === 'string' ? node.options.find(item => item.id === selected) : undefined
    if (branch) initialDraft(branch.form, into)
  }
  return into
}
function hydratedDraft(form: StoredActorForm, cached?: FormDraft): FormDraft {
  const draft = { ...initialDraft(form.opening.form.root), ...(form.draft ?? {}), ...(cached ?? {}) }
  return initialDraft(form.opening.form.root, draft)
}
function activeDraft(node: FormNode, draft: FormDraft, into: FormDraft = {}): FormDraft {
  if (['text', 'int', 'number', 'bool', 'choice', 'many', 'alternatives'].includes(node.kind)) {
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
  return into
}

export default function MountedForm({ form, ready, active, onAuthExpired }: { form: StoredActorForm; ready: boolean; active: boolean; onAuthExpired?: () => void }) {
  const key = keyOf(form)
  const [draft, setDraft] = useState<FormDraft>(() => hydratedDraft(form, retainedDrafts.get(key)))
  const [busy, setBusy] = useState(false)
  const [issue, setIssue] = useState('')
  const [latest, setLatest] = useState(form)
  const editable = latest.state === 'open' && active && ready
  const dismissible = (latest.state === 'open' || latest.state === 'submitted') && active && ready
  useEffect(() => {
    const stale = form.revisionSequence < latest.revisionSequence
      || (form.revisionSequence === latest.revisionSequence && latest.state !== 'open' && form.state === 'open')
    if (stale) return
    if (latest !== form) setLatest(form)
    if (!retainedDrafts.has(key)) setDraft(hydratedDraft(form))
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
      setLatest(current => value.revisionSequence < current.revisionSequence
        || (value.revisionSequence === current.revisionSequence && current.state !== 'open' && value.state === 'open') ? current : value)
      if (value.state !== 'open') retainedDrafts.delete(key)
      else keepDraft(key, draft)
    } catch (error) { setIssue(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(false) }
  }
  return <section className="mounted-form" aria-label="Actor form" data-mount-id={latest.opening.mountId}>
    <FormNodeView node={latest.opening.form.root} draft={draft} setDraft={setDraft} editable={editable} />
    {!!latest.errors.length && <ul className="form-errors" aria-label="Form errors">{latest.errors.map((error, i) => <li key={`${error.field ?? ''}:${i}`}>{error.field && <strong>{error.field}: </strong>}{error.message}</li>)}</ul>}
    {issue && <p role="alert">{issue}</p>}
    {latest.answer && <div className="form-answer"><h4>Answer</h4><RichViewRenderer view={latest.answer} /></div>}
    {latest.state !== 'open' && latest.draft && <section className="submitted-form-values"><h4>Submitted values</h4><pre>{JSON.stringify(latest.draft, null, 2)}</pre></section>}
    {(latest.state === 'open' || latest.state === 'submitted') && <div className="form-actions">
      {latest.state === 'open' && <button type="button" disabled={!editable || busy} onClick={() => void submit(false)}>{busy ? 'Sending…' : 'Submit'}</button>}
      <button type="button" disabled={!dismissible || busy} onClick={() => void submit(true)}>Dismiss</button>
    </div>}
    {!active && latest.state === 'open' && <p role="status">This actor is inactive; the form is read-only.</p>}
  </section>
}

function FormNodeView({ node, draft, setDraft, editable }: { node: FormNode; draft: FormDraft; setDraft: (draft: FormDraft) => void; editable: boolean }) {
  const change = (id: string, value: DraftValue, base = draft) => setDraft({ ...base, [id]: value })
  if (node.kind === 'pure' || node.kind === 'empty') return null
  if (node.kind === 'group') return <div className="form-group">{node.children.map((child, i) => <FormNodeView key={i} node={child} draft={draft} setDraft={setDraft} editable={editable} />)}</div>
  if (node.kind === 'section') return <fieldset className="form-section"><legend>{node.title}</legend><FormNodeView node={node.child} draft={draft} setDraft={setDraft} editable={editable} /></fieldset>
  if (node.kind === 'view') return <RichViewRenderer view={node.presentation} />
  if (node.kind === 'alternatives') {
    const selectedValue = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
    const selected = typeof selectedValue === 'string' ? selectedValue : ''
    const branch = node.options.find(item => item.id === selected)
    return <fieldset><legend>{node.label}</legend>{node.options.map(item => <label className="form-option" key={item.id}>
      <input type="radio" aria-label={item.label} name={node.id} value={item.id} checked={selected === item.id} disabled={!editable} onChange={() => change(node.id, item.id, initialDraft(item.form, { ...draft }))} />
      <span>{item.label}</span><RichViewRenderer view={item.presentation} />
    </label>)}{branch && <FormNodeView node={branch.form} draft={draft} setDraft={setDraft} editable={editable} />}</fieldset>
  }
  if (node.kind === 'choice' || node.kind === 'many') return <fieldset><legend>{node.label}</legend>{node.options.map(item => {
    const hasValue = Object.hasOwn(draft, node.id)
    const selected = node.kind === 'many' ? (Array.isArray(draft[node.id]) ? draft[node.id] as string[] : !hasValue && Array.isArray(node.initial) ? [...node.initial] : [])
      : typeof draft[node.id] === 'string' ? [draft[node.id] as string] : !hasValue && node.initial ? [node.initial] : []
    return <label className="form-option" key={item.id}><input aria-label={item.label} type={node.kind === 'many' ? 'checkbox' : 'radio'} name={node.id} value={item.id} checked={selected.includes(item.id)} disabled={!editable}
      onChange={event => change(node.id, node.kind === 'many' ? event.target.checked ? [...selected, item.id] : selected.filter(id => id !== item.id) : item.id)} />
      <span>{item.label}</span><RichViewRenderer view={item.presentation} /></label>
  })}</fieldset>
  const value = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
  if (node.kind === 'bool') return <label className="form-field"><input type="checkbox" checked={value === true} disabled={!editable} onChange={event => change(node.id, event.target.checked)} />{node.label}</label>
  if (node.kind === 'int') return <label className="form-field">{node.label}<input type="text" inputMode="numeric" value={value === null ? '' : String(value)} disabled={!editable}
    onChange={event => change(node.id, event.target.value)} /></label>
  if (node.kind === 'text') return <label className="form-field">{node.label}<input type="text" value={value === null ? '' : String(value)} disabled={!editable}
    onChange={event => change(node.id, event.target.value)} /></label>
  return <label className="form-field">{node.label}<input type="number" step="any" value={value === null ? '' : String(value)} disabled={!editable}
    onChange={event => { const raw = event.target.value; change(node.id, raw === '' ? '' : Number(raw)) }} /></label>
}
