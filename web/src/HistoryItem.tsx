import { useMemo, useState } from 'react'
import type { HistoryEntry } from './history-client'

const PREVIEW_LENGTH = 2000

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

interface ReadableItem {
  readonly label: string
  readonly texts: readonly { label: string; text: string }[]
  readonly unknownBlocks?: boolean
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
    return { label: value.role === 'assistant' ? 'Assistant' : value.role === 'user' ? 'You' : `Message · ${value.role}`,
      texts: content.texts.map((text) => ({ label: 'Text', text })), unknownBlocks: content.unknownBlocks }
  }
  if (value.type === 'reasoning') {
    const summary = blocks(value.summary, ['summary_text'])
    const content = value.content === undefined ? {texts:[],unknownBlocks:false} : blocks(value.content, ['reasoning_text', 'text'])
    return { label: summary.texts.length ? 'Reasoning summary' : 'Reasoning',
      texts: [...summary.texts.map(text => ({label:'Summary',text})), ...content.texts.map(text => ({label:'Reasoning',text}))],
      unknownBlocks: summary.unknownBlocks || content.unknownBlocks }
  }
  if ((value.type === 'function_call' || value.type === 'custom_tool_call')
    && typeof value.name === 'string' && typeof value.call_id === 'string') {
    const input = value.type === 'function_call' ? value.arguments : value.input
    if (typeof input !== 'string' && !(value.type === 'function_call' && object(input))) return undefined
    return { label: `${value.type} · ${value.name} · call ${value.call_id}`,
      texts: [{ label: value.type === 'function_call' ? (typeof input === 'string' ? 'Arguments' : 'Arguments JSON') : 'Input',
        text: typeof input === 'string' ? input : JSON.stringify(input, null, 2) }] }
  }
  if ((value.type === 'function_call_output' || value.type === 'custom_tool_call_output')
    && typeof value.call_id === 'string' && typeof value.output === 'string') {
    return { label: `${value.type} · call ${value.call_id}`, texts: [{ label: 'Output', text: value.output }] }
  }
  if (value.type === 'configuration_update' && object(value.reasoning) && typeof value.reasoning.effort === 'string') {
    return { label: 'Configuration update', texts: [{ label: 'Reasoning effort', text: value.reasoning.effort }] }
  }
  return undefined
}

function TextPreview({ text, label }: { readonly text: string; readonly label: string }) {
  const [expanded, setExpanded] = useState(false)
  const long = text.length > PREVIEW_LENGTH
  // Keep a preview boundary from splitting a Unicode surrogate pair.
  const end = /[\uD800-\uDBFF]/.test(text.charAt(PREVIEW_LENGTH - 1))
    && /[\uDC00-\uDFFF]/.test(text.charAt(PREVIEW_LENGTH)) ? PREVIEW_LENGTH - 1 : PREVIEW_LENGTH
  return <div>
    <pre className="history-content" aria-label={label}>{long && !expanded ? text.slice(0, end) : text}</pre>
    {long && <><p className="meta">{expanded ? 'Showing the full fetched text.' : 'Text preview is limited to 2000 characters.'}</p>
      <button type="button" onClick={() => setExpanded(!expanded)}>{expanded ? 'Collapse' : 'Expand'} {label}</button></>}
  </div>
}

/** Plain React text retains wire strings; raw formatting happens only when shown. */
export default function HistoryItem({ entry, listItem = true }: { readonly entry: HistoryEntry; readonly listItem?: boolean }) {
  const view = useMemo(() => readable(entry.item), [entry.item])
  const [showRaw, setShowRaw] = useState(false)
  const rawVisible = showRaw || !view
  const raw = useMemo(() => rawVisible ? JSON.stringify(entry.item, null, 2) : undefined, [entry.item, rawVisible])
  // Opaque reasoning items contain no readable summary and add no chat content.
  if (object(entry.item) && entry.item.type === 'reasoning' && view?.texts.length === 0) return null
  return <div role={listItem ? 'listitem' : undefined} className="message">
    {view ? <>
      <h3>{view.label}</h3>
      {view.texts.map((part, index) => <TextPreview key={index} text={part.text} label={`${part.label} item ${entry.position}${view.texts.length > 1 ? ` block ${index + 1}` : ''}`} />)}
      {view.unknownBlocks && <p>Unknown content is retained. Open Raw to inspect all blocks.</p>}
      <details className="history-controls"><summary>Message details</summary><p className="meta">Item {entry.position} · hash {entry.hash} · {entry.byteLen} bytes</p><button type="button" aria-expanded={showRaw} onClick={() => setShowRaw(!showRaw)}>{showRaw ? 'Hide' : 'Show'} Raw item {entry.position}</button></details>
    </> : <p>Unrecognized item · Raw data</p>}
    {rawVisible && <TextPreview key="raw" text={raw ?? 'Raw data could not be represented as JSON.'} label={`Raw item ${entry.position}`} />}
  </div>
}
