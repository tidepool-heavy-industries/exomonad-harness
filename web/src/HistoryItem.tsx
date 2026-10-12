import { useMemo } from 'react'
import { PresentationDetails, usePresentationChoice } from './presentation-state'
import type { HistoryEntry } from './history-client'
import { MessageContent, MessagePresentation, type MessageKind } from './MessagePresentation'

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

interface ReadableItem {
  readonly label: string
  readonly kind: MessageKind
  readonly texts: readonly { label: string; text: string; kind: 'prose' | 'code'; format?: 'markdown' | 'plain'; copyText?: string }[]
  readonly metadata?: readonly { label: string; value: string }[]
  readonly responseSummary?: string
  readonly unknownBlocks?: boolean
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
      const displayKey = key.length > 48 ? `${key.slice(0, 45)}…` : key
      const rendered = String(field)
      return `${displayKey}: ${rendered.length > 72 ? `${rendered.slice(0, 69)}…` : rendered}`
    }).join(' · ')
    const summary = Array.isArray(value)
      ? `JSON result · ${entries} item${entries === 1 ? '' : 's'}`
      : `JSON result · ${entries} field${entries === 1 ? '' : 's'}${preview ? ` · ${preview}` : ''}`
    return { text: JSON.stringify(value, null, 2), summary }
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

export function readable(value: unknown): ReadableItem | undefined {
  if (!object(value) || typeof value.type !== 'string') return undefined
  if (value.type === 'message' && typeof value.role === 'string') {
    const content = blocks(value.content, ['input_text', 'output_text', 'refusal'])
    const phase = typeof value.phase === 'string' ? value.phase : undefined
    const assistantLabel = phase === 'commentary' || phase === 'final'
      ? `Assistant · ${phase}`
      : phase === undefined ? 'Assistant' : `Assistant · unknown phase (${phase})`
    return { label: value.role === 'assistant' ? assistantLabel : value.role === 'user' ? 'You' : `Message · ${value.role}`,
      kind: value.role === 'user' ? 'human' : value.role === 'assistant' ? 'assistant' : 'activity',
      texts: content.texts.map((text) => ({ label: 'Text', text, kind: 'prose', format: value.role === 'assistant' ? 'markdown' : 'plain' })),
      unknownBlocks: content.unknownBlocks || (value.role === 'assistant' && phase !== undefined && phase !== 'commentary' && phase !== 'final') }
  }
  if (value.type === 'reasoning') {
    const summary = value.summary === undefined ? { texts: [], unknownBlocks: false } : blocks(value.summary, ['summary_text'])
    const content = value.content === undefined ? {texts:[],unknownBlocks:false} : blocks(value.content, ['reasoning_text', 'text'])
    const summaries = summary.texts.filter(text => text.trim().length > 0)
    return { label: summaries.length ? 'Reasoning summary' : 'Reasoning', kind: 'reasoning',
      texts: [...summaries.map(text => ({label:'Summary',text,kind:'prose' as const,format:'markdown' as const})), ...content.texts.filter(text => text.trim().length > 0).map(text => ({label:'Reasoning',text,kind:'prose' as const}))],
      unknownBlocks: summary.unknownBlocks || content.unknownBlocks }
  }
  if ((value.type === 'function_call' || value.type === 'custom_tool_call')
    && typeof value.name === 'string' && typeof value.call_id === 'string') {
    const input = value.type === 'function_call' ? value.arguments : value.input
    if (typeof input !== 'string' && !(value.type === 'function_call' && object(input))) return undefined
    return { label: `Model tool call · ${value.name}`, kind: 'activity',
      metadata: toolMetadata(value, value.type, value.call_id),
      texts: [{ label: value.type === 'function_call' ? (typeof input === 'string' ? 'Arguments' : 'Arguments JSON') : 'Input',
        text: typeof input === 'string' ? presentJson(input) : JSON.stringify(input, null, 2),
        copyText: typeof input === 'string' ? input : undefined, kind: 'code' }] }
  }
  if ((value.type === 'function_call_output' || value.type === 'custom_tool_call_output')
    && typeof value.call_id === 'string' && typeof value.output === 'string') {
    const formatted = structuredJson(value.output)
    return { label: 'Tool response', kind: 'activity', metadata: toolMetadata(value, value.type, value.call_id), responseSummary: formatted?.summary,
      texts: [{ label: 'Result', text: formatted?.text ?? value.output, copyText: value.output, kind: 'code' }] }
  }
  if (value.type === 'configuration_update' && object(value.reasoning) && typeof value.reasoning.effort === 'string') {
    return { label: 'Configuration update', kind: 'activity', texts: [{ label: 'Reasoning effort', text: value.reasoning.effort, kind: 'code' }] }
  }
  return undefined
}

/** Presentation never replaces the wire item; raw formatting happens only on inspection. */
export default function HistoryItem({ entry, listItem = true, diagnostics = true, publication }: { readonly entry: HistoryEntry; readonly listItem?: boolean; readonly diagnostics?: boolean; readonly publication?: { sequence: string; requestId: string } }) {
  const view = useMemo(() => readable(entry.item), [entry.item])
  const [showRaw, setShowRaw] = usePresentationChoice('raw-item')
  const rawVisible = showRaw || !view
  const raw = useMemo(() => rawVisible ? JSON.stringify(entry.item, null, 2) : undefined, [entry.item, rawVisible])
  // Empty or opaque-only reasoning adds no chat content; unfamiliar blocks remain inspectable.
  if (object(entry.item) && entry.item.type === 'reasoning' && view?.texts.length === 0 && !view.unknownBlocks) return publication
    ? <PresentationDetails memoryKey="evidence" className="chat-message-details" summary="Publication details">
      <p>Sequence {publication.sequence} · exchange {publication.requestId} · position {entry.position}</p>
    </PresentationDetails> : null
  const texts = view?.texts.map((part, index, parts) => <div key={index} className={`history-${part.kind}`}>
    <MessageContent text={part.text} copyText={part.copyText} format={part.format} bounded={part.kind === 'code'} memoryKey={`part:${index}`} label={`${part.label} item ${entry.position}${parts.length > 1 ? ` block ${index + 1}` : ''}`} />
  </div>)
  const rawContent = rawVisible && <MessageContent key="raw" text={raw ?? 'Raw data could not be represented as JSON.'} label={`Raw item ${entry.position}`} />
  return <MessagePresentation listItem={listItem} kind={view?.kind ?? 'activity'} title={view?.label ?? 'Unrecognized item · Raw data'} collapsed={view?.kind === 'reasoning' && view.texts.length > 0}
    details={diagnostics ? <>
      {publication && <p className="meta">Sequence {publication.sequence} · exchange {publication.requestId}</p>}
      <p className="meta">Item {entry.position} · hash {entry.hash} · {entry.byteLen} bytes</p>
      {view?.metadata?.map(({ label, value }) => <p className="meta" key={label}>{label}: <span className="mono">{value}</span></p>)}
      {view && <><button type="button" aria-expanded={showRaw} onClick={() => setShowRaw(!showRaw)}>{showRaw ? 'Hide' : 'Show'} Raw item {entry.position}</button>{rawContent}</>}
    </> : undefined}>
    {view ? <>
      {view.responseSummary ? <details className="history-tool-response"><summary>{view.responseSummary}</summary>{texts}</details> : texts}
      {view.unknownBlocks && <p className="message-content-note">Unknown content is retained. Open Raw to inspect all blocks.</p>}
    </> : rawContent}
  </MessagePresentation>
}
