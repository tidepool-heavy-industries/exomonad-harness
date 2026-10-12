import { isRichView } from './rich-view-model'
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
const retainedDrafts = new Map<string, FormDraft>()
const MAX_DRAFTS = 32
export const formKey = (form: StoredActorForm) => JSON.stringify([form.opening.origin.run, form.opening.origin.nativeActor, form.opening.origin.incarnation, form.opening.mountId])
export function clearMountedFormDrafts() { retainedDrafts.clear() }
export function keepDraft(key: string, draft: FormDraft) {
  retainedDrafts.delete(key); retainedDrafts.set(key, draft)
  while (retainedDrafts.size > MAX_DRAFTS) retainedDrafts.delete(retainedDrafts.keys().next().value!)
}
export function initialDraft(node: FormNode, into: FormDraft = Object.create(null) as FormDraft): FormDraft {
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
export function hydratedDraft(form: StoredActorForm, cached?: FormDraft): FormDraft {
  const draft = { ...initialDraft(form.opening.form.root), ...(form.draft ?? {}), ...(cached ?? {}) }
  return initialDraft(form.opening.form.root, draft)
}
export function activeDraft(node: FormNode, draft: FormDraft, into: FormDraft = Object.create(null) as FormDraft): FormDraft {
  if (['text', 'int', 'number', 'slider', 'bool', 'choice', 'many', 'alternatives', 'alternatives_many'].includes(node.kind)) {
    const field = node as Extract<FormNode, { id: string }>
    const value = draft[field.id]
    if (Object.hasOwn(draft, field.id) && value !== undefined) into[field.id] = value
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

export function readFormDraft(key: string) { return retainedDrafts.get(key) }
export function hasFormDraft(key: string) { return retainedDrafts.has(key) }
export function discardFormDraft(key: string) { retainedDrafts.delete(key) }

/** An HTTP acknowledgement cannot roll back a newer native settlement. */
export function formRevisionIsStale(incoming: StoredActorForm, current: StoredActorForm) {
  return compareDecimal(incoming.revisionSequence, current.revisionSequence) < 0
    || (incoming.revisionSequence === current.revisionSequence && current.state !== 'open' && incoming.state === 'open')
}
