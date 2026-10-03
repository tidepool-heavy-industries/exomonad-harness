import { useMemo, useState } from 'react'
import type { HistoryEntry } from './history-client'

const PREVIEW_LENGTH = 2000

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

interface ReadableItem {
  readonly label: string
  readonly texts: readonly { label: string; text: string; kind: 'prose' | 'code' }[]
  readonly metadata?: readonly { label: string; value: string }[]
  readonly responseSummary?: string
  readonly unknownBlocks?: boolean
  readonly reasoning?: boolean
}

function structuredJson(text: string): { text: string; summary?: string } | undefined {
  try {
    const value: unknown = JSON.parse(text)
    if (!object(value) && !Array.isArray(value)) return undefined
    const entries = Array.isArray(value) ? value.length : Object.keys(value).length
    const scalarFields = object(value)
      ? Object.entries(value).filter(([, field]) => field === null || ['string', 'number', 'boolean'].includes(typeof field))
      : []
    const preview = scalarFields.slice(0, 2).map(([key, field]) => {
      const rendered = String(field)
      return `${key}: ${rendered.length > 72 ? `${rendered.slice(0, 69)}…` : rendered}`
    }).join(' · ')
    return { text: JSON.stringify(value, null, 2), summary: !Array.isArray(value) && preview
      ? `JSON result · ${entries} fields · ${preview}` : undefined }
  } catch { /* Non-JSON tool text stays exact. */ }
  return undefined
}

function presentJson(text: string): string {
  return structuredJson(text)?.text ?? text
}

function toolMetadata(value: Record<string, unknown>, type: string, callId: string): { label: string; value: string }[] {
  return [
    { label: 'Item type', value: type },
    ...(typeof value.id === 'string' ? [{ label: 'Item ID', value: value.id }] : []),
    { label: 'Call ID', value: callId },
  ]
}

function blocks(value: unknown, types: readonly string[]): { texts: string[]; unknownBlocks: boolean } {
  if (typeof value === 'string') return { texts: [value], unknownBlocks: false }
  if (!Array.isArray(value)) return { texts: [], unknownBlocks: true }
  const texts: string[] = []
  let unknownBlocks = false
  for (const block of value) {
    if (object(block) && typeof block.type === 'string' && types.includes(block.type)
      && typeof (block.type === 'refusal' ? block.refusal : block.text) === 'string') {
      texts.push((block.type === 'refusal' ? block.refusal : block.text) as string)
    } else unknownBlocks = true
  }
  return { texts, unknownBlocks }
}

function readable(value: unknown): ReadableItem | undefined {
  if (!object(value) || typeof value.type !== 'string') return undefined
  if (value.type === 'message' && typeof value.role === 'string') {
    const content = blocks(value.content, ['input_text', 'output_text', 'refusal'])
    const phase = typeof value.phase === 'string' ? value.phase : undefined
    const assistantLabel = phase === 'commentary' || phase === 'final'
      ? `Assistant · ${phase}`
      : phase === undefined ? 'Assistant' : `Assistant · unknown phase (${phase})`
    return { label: value.role === 'assistant' ? assistantLabel : value.role === 'user' ? 'You' : `Message · ${value.role}`,
      texts: content.texts.map((text) => ({ label: 'Text', text, kind: 'prose' })),
      unknownBlocks: content.unknownBlocks || (value.role === 'assistant' && phase !== undefined && phase !== 'commentary' && phase !== 'final') }
  }
  if (value.type === 'reasoning') {
    const summary = blocks(value.summary, ['summary_text'])
    const content = value.content === undefined ? {texts:[],unknownBlocks:false} : blocks(value.content, ['reasoning_text', 'text'])
    return { label: summary.texts.length ? 'Reasoning summary' : 'Reasoning',
      texts: [...summary.texts.map(text => ({label:'Summary',text,kind:'prose' as const})), ...content.texts.map(text => ({label:'Reasoning',text,kind:'prose' as const}))],
      unknownBlocks: summary.unknownBlocks || content.unknownBlocks, reasoning: true }
  }
  if ((value.type === 'function_call' || value.type === 'custom_tool_call')
    && typeof value.name === 'string' && typeof value.call_id === 'string') {
    const input = value.type === 'function_call' ? value.arguments : value.input
    if (typeof input !== 'string' && !(value.type === 'function_call' && object(input))) return undefined
    return { label: `Model tool call · ${value.name}`,
      metadata: toolMetadata(value, value.type, value.call_id),
      texts: [{ label: value.type === 'function_call' ? (typeof input === 'string' ? 'Arguments' : 'Arguments JSON') : 'Input',
        text: typeof input === 'string' ? presentJson(input) : JSON.stringify(input, null, 2), kind: 'code' }] }
  }
  if ((value.type === 'function_call_output' || value.type === 'custom_tool_call_output')
    && typeof value.call_id === 'string' && typeof value.output === 'string') {
    const formatted = structuredJson(value.output)
    return { label: 'Tool response', metadata: toolMetadata(value, value.type, value.call_id), responseSummary: formatted?.summary,
      texts: [{ label: 'Result', text: formatted?.text ?? value.output, kind: 'code' }] }
  }
  if (value.type === 'configuration_update' && object(value.reasoning) && typeof value.reasoning.effort === 'string') {
    return { label: 'Configuration update', texts: [{ label: 'Reasoning effort', text: value.reasoning.effort, kind: 'code' }] }
  }
  return undefined
}

function TextPreview({ text, label, kind }: { readonly text: string; readonly label: string; readonly kind: 'prose' | 'code' }) {
  const [expanded, setExpanded] = useState(false)
  const long = text.length > PREVIEW_LENGTH
  // Keep a preview boundary from splitting a Unicode surrogate pair.
  const end = /[\uD800-\uDBFF]/.test(text.charAt(PREVIEW_LENGTH - 1))
    && /[\uDC00-\uDFFF]/.test(text.charAt(PREVIEW_LENGTH)) ? PREVIEW_LENGTH - 1 : PREVIEW_LENGTH
  return <div>
    <pre className={`history-content history-${kind}`} aria-label={label}>{long && !expanded ? text.slice(0, end) : text}</pre>
    {long && <><p className="meta">{expanded ? 'Showing the full fetched text.' : 'Text preview is limited to 2000 characters.'}</p>
      <button type="button" onClick={() => setExpanded(!expanded)}>{expanded ? 'Collapse' : 'Expand'} {label}</button></>}
  </div>
}

/** Plain React text retains wire strings; raw formatting happens only when shown. */
export default function HistoryItem({ entry }: { readonly entry: HistoryEntry }) {
  const view = useMemo(() => readable(entry.item), [entry.item])
  const [showRaw, setShowRaw] = useState(false)
  const rawVisible = showRaw || !view
  const raw = useMemo(() => rawVisible ? JSON.stringify(entry.item, null, 2) : undefined, [entry.item, rawVisible])
  // Purely opaque reasoning has no readable or unknown blocks to inspect.
  if (object(entry.item) && entry.item.type === 'reasoning' && view?.texts.length === 0 && !view.unknownBlocks) return null
  return <div role="listitem" className="message">
    {view ? <>
      <h3>{view.label}</h3>
      {view.reasoning && view.texts.length > 0 ? <details className="history-reasoning">
        <summary>{view.label} · show readable text</summary>
        {view.texts.map((part, index) => <TextPreview key={index} text={part.text} kind={part.kind} label={`${part.label} item ${entry.position}${view.texts.length > 1 ? ` block ${index + 1}` : ''}`} />)}
      </details> : view.responseSummary ? <details className="history-tool-response">
        <summary>{view.responseSummary}</summary>
        {view.texts.map((part, index) => <TextPreview key={index} text={part.text} kind={part.kind} label={`${part.label} item ${entry.position}${view.texts.length > 1 ? ` block ${index + 1}` : ''}`} />)}
      </details> : view.texts.map((part, index) => <TextPreview key={index} text={part.text} kind={part.kind} label={`${part.label} item ${entry.position}${view.texts.length > 1 ? ` block ${index + 1}` : ''}`} />)}
      {view.unknownBlocks && <p>Unknown content is retained. Open Raw to inspect all blocks.</p>}
      <details className="history-controls"><summary>Message details</summary><p className="meta">Item {entry.position} · hash {entry.hash} · {entry.byteLen} bytes</p>
        {view.metadata?.map(({ label, value }) => <p className="meta" key={label}>{label}: <span className="mono">{value}</span></p>)}
        <button type="button" aria-expanded={showRaw} onClick={() => setShowRaw(!showRaw)}>{showRaw ? 'Hide' : 'Show'} Raw item {entry.position}</button></details>
    </> : <p>Unrecognized item · Raw data</p>}
    {rawVisible && <TextPreview key="raw" text={raw ?? 'Raw data could not be represented as JSON.'} kind="code" label={`Raw item ${entry.position}`} />}
  </div>
}
