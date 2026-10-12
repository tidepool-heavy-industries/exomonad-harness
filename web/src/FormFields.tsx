import { createContext, useContext } from 'react'
import { RichViewRenderer } from './rich-view'
import { initialDraft, type StoredActorForm, type FormNode, type FormDraft, type DraftValue } from './form-model'

export const Validation = createContext<{ prefix: string; errors: StoredActorForm['errors'] }>({ prefix: '', errors: [] })

export function fieldLabel(node: FormNode, id: string): string | undefined {
  if ('id' in node && node.id === id) return node.label
  if (node.kind === 'group') return node.children.map(child => fieldLabel(child, id)).find(label => label !== undefined)
  if (node.kind === 'section') return fieldLabel(node.child, id)
  if (node.kind === 'alternatives' || node.kind === 'alternatives_many') return node.options.map(option => fieldLabel(option.form, id)).find(label => label !== undefined)
  return undefined
}

export function FormNodeView({ scope, node, draft, setDraft, editable, readOnly = false }: { scope: string; node: FormNode; draft: FormDraft; setDraft: (draft: FormDraft) => void; editable: boolean; readOnly?: boolean }) {
  const validation = useContext(Validation)
  const errorIds = validation.errors.flatMap((error, index) => 'id' in node && error.field === node.id ? [`${validation.prefix}-error-${index}`] : [])
  const fieldProps = { name: 'id' in node ? `${scope}:${node.id}` : undefined, autoComplete: 'off',
    'aria-invalid': errorIds.length ? true as const : undefined, 'aria-describedby': errorIds.length ? errorIds.join(' ') : undefined }
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
      <input {...fieldProps} type="radio" aria-label={item.label} name={`${scope}:${node.id}`} value={item.id} checked={selected === item.id} disabled={!editable} onChange={() => change(node.id, item.id, initialDraft(item.form, { ...draft }))} />
      <RichViewRenderer view={item.presentation} />
    </label>)}{branch && <FormNodeView scope={scope} node={branch.form} draft={draft} setDraft={setDraft} editable={editable} readOnly={readOnly} />}</fieldset>
  }
  if (node.kind === 'alternatives_many') {
    const value = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
    const selected = Array.isArray(value) ? value : []
    return <fieldset><legend>{node.label}</legend>{node.options.map(item => <div key={item.id}>
      <label className="form-option"><input {...fieldProps} type="checkbox" aria-label={item.label} checked={selected.includes(item.id)} disabled={!editable}
        onChange={event => change(node.id, event.target.checked ? [...selected, item.id] : selected.filter(id => id !== item.id), initialDraft(item.form, { ...draft }))} />
        <RichViewRenderer view={item.presentation} /></label>
      {selected.includes(item.id) && <FormNodeView scope={scope} node={item.form} draft={draft} setDraft={setDraft} editable={editable} readOnly={readOnly} />}
    </div>)}</fieldset>
  }
  if (node.kind === 'choice' || node.kind === 'many') return <fieldset><legend>{node.label}</legend>{node.options.map(item => {
    const hasValue = Object.hasOwn(draft, node.id)
    const selected = node.kind === 'many' ? (Array.isArray(draft[node.id]) ? draft[node.id] as string[] : !hasValue && Array.isArray(node.initial) ? [...node.initial] : [])
      : typeof draft[node.id] === 'string' ? [draft[node.id] as string] : !hasValue && node.initial ? [node.initial] : []
    return <label className="form-option" key={item.id}><input {...fieldProps} aria-label={item.label} type={node.kind === 'many' ? 'checkbox' : 'radio'} name={`${scope}:${node.id}`} value={item.id} checked={selected.includes(item.id)} disabled={!editable}
      onChange={event => change(node.id, node.kind === 'many' ? event.target.checked ? [...selected, item.id] : selected.filter(id => id !== item.id) : item.id)} />
      <RichViewRenderer view={item.presentation} /></label>
  })}</fieldset>
  const value = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
  if (node.kind === 'slider') return <label className="form-field">{node.label}<input {...fieldProps} type="range" min={node.minimum} max={node.maximum} step="any" value={Number(value)} disabled={!editable}
    onChange={event => change(node.id, Number(event.target.value))} /><output>{String(value)}</output></label>
  if (node.kind === 'bool') return <label className="form-toggle"><input {...fieldProps} type="checkbox" checked={value === true} disabled={!editable} onChange={event => change(node.id, event.target.checked)} />{node.label}</label>
  if (node.kind === 'int') return <label className="form-field">{node.label}<input {...fieldProps} type="text" inputMode="numeric" value={value === null ? '' : String(value)} disabled={!editable}
    onChange={event => change(node.id, event.target.value)} /></label>
  if (node.kind === 'text') return <label className="form-field">{node.label}<input {...fieldProps} type="text" value={value === null ? '' : String(value)} disabled={!editable}
    onChange={event => change(node.id, event.target.value)} /></label>
  return <label className="form-field">{node.label}<input {...fieldProps} type="number" step="any" value={value === null ? '' : String(value)} disabled={!editable}
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

export function ResponseSummary({ node, draft }: { node: FormNode; draft: FormDraft }) {
  if (node.kind === 'group') return <div className="form-response-summary">{node.children.map((child, index) => <ResponseSummary key={index} node={child} draft={draft} />)}</div>
  if (node.kind === 'section') return <ResponseSummary node={node.child} draft={draft} />
  if (!('id' in node)) return null
  const value = Object.hasOwn(draft, node.id) ? draft[node.id] : node.initial
  const selected = Array.isArray(value) ? value : typeof value === 'string' ? [value] : []
  const choices = 'options' in node ? node.options.filter(option => selected.includes(option.id)) : undefined
  const display = choices ? choices.map(option => option.label).join(', ') || 'No selections' : value == null ? 'No response' : typeof value === 'boolean' ? value ? 'Yes' : 'No' : String(value)
  return <><dl className="form-fixed-field"><dt>{node.label}</dt><dd>{display}</dd></dl>
    {(node.kind === 'alternatives' || node.kind === 'alternatives_many') && node.options.filter(option => selected.includes(option.id)).map(option => <ResponseSummary key={option.id} node={option.form} draft={draft} />)}
  </>
}
