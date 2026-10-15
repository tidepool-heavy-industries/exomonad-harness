import { useMemo } from 'react'
import './ToolActivity.css'
import { usePresentationChoice } from './presentation-state'
import HistoryItem, { readable, haskellSource } from './HistoryItem'
import { MessageContent, useStreamingText } from './MessagePresentation'
import { PresentationDetails, PresentationMemory } from './presentation-state'
import type { MessageEntry, TimelineRow } from './chat-timeline'
import type { HarnessViewModel } from './view-model'
import type { LiveOutput } from './live-output'

export type ExchangeObservation = Pick<HarnessViewModel['timeline'][number], 'id' | 'state' | 'detail' | 'failure' | 'startedAtMs' | 'endedAtMs'>
export type ToolJobObservation = HarnessViewModel['timeline'][number]

/** A result's publication is evidence of availability, not of successful execution. */
export default function ToolActivity({ row, streamingActive, exchange, jobs = [] }: {
  row: Extract<TimelineRow, { kind: 'activity' | 'live' }>; streamingActive: boolean; exchange?: ExchangeObservation; jobs?: readonly ToolJobObservation[]
}) {
  const entry = row.kind === 'activity' ? row.entry : undefined
  const view = entry ? readable(entry.item) : undefined
  const results = row.kind === 'activity' ? row.results : []
  const name = entry && typeof entry.item === 'object' && entry.item !== null && 'name' in entry.item && typeof entry.item.name === 'string'
    ? entry.item.name : row.kind === 'live' ? row.live.find(item => item.toolName)?.toolName ?? 'Tool call' : 'Tool call'
  const status = row.kind === 'live'
    ? exchange?.state === 'failed' ? 'Interrupted input preview' : row.live.some(item => item.streaming) && streamingActive ? 'Streaming input' : 'Input preview'
    : results.length ? 'Result retained' : entry?.tool ? 'No loaded result' : 'Operation ownership unavailable'
  const elapsed = exchange?.startedAtMs !== undefined && exchange.endedAtMs !== undefined && exchange.endedAtMs >= exchange.startedAtMs
    ? (exchange.endedAtMs - exchange.startedAtMs) / 1000 : undefined
  const excerpt = results.map(result => resultExcerpt(result.item)).find(Boolean)
  return <section className="tool-activity" aria-label={`${name} tool call`}>
    <header className="tool-heading"><h3>{name}</h3><span className="tool-status">{jobs.length ? `${jobs.map(job => `Tool ${job.state}`).join(' · ')} · ` : ''}{status}</span></header>
    {excerpt && <p className="tool-result-excerpt"><strong>Result excerpt:</strong> {excerpt}</p>}
    <div className="tool-source" translate="no">{row.kind === 'live'
      ? row.live.map((item, index) => <LiveToolContent key={index} item={item} streamingActive={streamingActive} index={index} />)
      : view?.texts.map((part, index) => <MessageContent key={index} text={part.text} copyText={part.copyText} language={part.language}
        label={part.label} memoryKey={`part:${index}`} bounded />)}</div>
    {entry && !view && <HistoryItem listItem={false} diagnostics={false} entry={{ position: entry.position, hash: entry.hash, byteLen: String(new TextEncoder().encode(JSON.stringify(entry.item)).length), item: entry.item }} />}
    {row.kind === 'live' && row.live.some(item => item.overflow) && <p className="hint">Partial live preview. Completed content will be available in retained history.</p>}
    {results.length > 0 && <div className="tool-results"><h4>Result</h4>{results.map(result => {
      const resultView = readable(result.item)
      return <PresentationMemory key={result.sequence}>{resultView ? resultView.texts.map((part, index) =>
        <MessageContent key={index} text={part.text} copyText={part.copyText} label={`Result ${result.sequence}`} memoryKey={`result:${index}`} bounded />)
        : <HistoryItem listItem={false} diagnostics={false} entry={{ position: result.position, hash: result.hash, byteLen: String(new TextEncoder().encode(JSON.stringify(result.item)).length), item: result.item }} />}
      </PresentationMemory>
    })}</div>}
    <PresentationDetails memoryKey="execution" className="execution-log" summary="Tool evidence">
      <h4>Execution log</h4>
      <ol>
        <li>{entry ? 'Tool input recorded.' : 'Tool input preview; execution has not been established.'}</li>
        {jobs.map(job => <li key={job.id}>Tool job {job.state}.
          {job.startedAtMs !== undefined && job.endedAtMs !== undefined && job.endedAtMs >= job.startedAtMs
            ? ` Host duration: ${new Intl.NumberFormat(undefined, { maximumFractionDigits: 3 }).format((job.endedAtMs - job.startedAtMs) / 1000)} s.` : ' Duration unavailable.'}
        </li>)}
        {results.map(result => <li key={result.sequence}>Tool result available.</li>)}
      </ol>
      {!results.length && entry && <p>No result is loaded for this call.{!jobs.length && ' Its execution outcome is unknown.'}</p>}
      {exchange && <p>Model exchange: {exchange.state.replaceAll('_', ' ')}{elapsed !== undefined && ` · ${new Intl.NumberFormat(undefined, { maximumFractionDigits: 3 }).format(elapsed)} s total`}.
        <span className="hint"> Exchange timing includes model activity; it is not this cell’s execution time.</span></p>}
      <p className="hint">Compiler phases and individual effects are not reported by this host interface.</p>
      {entry && <ToolEvidence entry={entry} label="Tool" />}
      {results.map(result => <ToolEvidence key={result.sequence} entry={result} label="Result" />)}
    </PresentationDetails>
  </section>
}

function LiveToolContent({ item, streamingActive, index }: { item: LiveOutput; streamingActive: boolean; index: number }) {
  const preview = useStreamingText(item.text, item.streaming && streamingActive)
  const source = useMemo(() => haskellSource(item.toolName, item.channel === 'tool_input' ? 'custom_tool_call' : 'function_call', preview),
  [preview, item.toolName, item.channel])
  // Unidentified/incomplete JSON stays literal; only the ordinary parser extracts source.
  const copy = item.channel === 'tool_input' || source === undefined || preview !== item.text ? item.text : source
  return <MessageContent text={source ?? preview} copyText={copy} language={source !== undefined ? 'haskell' : undefined}
    label={source !== undefined || item.channel === 'tool_input' ? 'Input' : 'Arguments'} memoryKey={`part:${index}`} bounded />
}

function ToolEvidence({ entry, label }: { entry: MessageEntry; label: 'Tool' | 'Result' }) {
  const [rawVisible, setRawVisible] = usePresentationChoice(`raw:${entry.sequence}`)
  return <div className="timeline-metadata">
    <h4>{label} details</h4>
    <p>Sequence {entry.sequence} · exchange {entry.requestId} · position {entry.position}</p><p>Hash {entry.hash}</p>
    {readable(entry.item)?.metadata?.map(field => <p key={field.label}>{field.label}: {field.value}</p>)}
    <button type="button" aria-expanded={rawVisible} onClick={() => setRawVisible(!rawVisible)}>{rawVisible ? 'Hide' : 'Show'} raw {label.toLowerCase()} item</button>
    {rawVisible && <MessageContent text={JSON.stringify(entry.item, null, 2)} label={`Raw ${label.toLowerCase()} item`} />}
  </div>
}

/** Literal excerpt only: publication never establishes an execution outcome. */
export function resultExcerpt(item: unknown): string | undefined {
  if (!item || typeof item !== 'object' || !('output' in item)) return undefined
  const output = item.output
  const texts = typeof output === 'string' ? [output] : Array.isArray(output)
    ? output.flatMap(block => block && typeof block === 'object' && ['text', 'output_text'].includes(block.type) && typeof block.text === 'string' ? [block.text] : []) : []
  for (const text of texts) {
    // Scan without allocating an array for every line of a large retained result.
    const line = /\S[^\r\n]*/.exec(text)?.[0]?.trim()
    if (line) return line.length > 180 ? `${line.slice(0, 180).replace(/[\uD800-\uDBFF]$/, '')}…` : line
  }
  return undefined
}
