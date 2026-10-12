import './ToolActivity.css'
import { useState } from 'react'
import HistoryItem, { readable } from './HistoryItem'
import { MessageContent } from './MessagePresentation'
import { PresentationDetails, PresentationMemory } from './presentation-state'
import type { MessageEntry, TimelineRow } from './chat-timeline'
import type { HarnessViewModel } from './view-model'

export type ExchangeObservation = Pick<HarnessViewModel['timeline'][number], 'id' | 'state' | 'detail' | 'failure' | 'startedAtMs' | 'endedAtMs'>
export type ToolJobObservation = HarnessViewModel['timeline'][number]

/** A result's publication is evidence of availability, not of successful execution. */
export default function ToolActivity({ row, streamingActive, exchange, jobs = [] }: {
  row: Extract<TimelineRow, { kind: 'activity' | 'live' }>; streamingActive: boolean; exchange?: ExchangeObservation; jobs?: readonly ToolJobObservation[]
}) {
  const entry = row.kind === 'activity' ? row.entry : undefined
  const view = entry ? readable(entry.item) : undefined
  const parts = row.kind === 'live' ? row.live.map(item => ({ text: item.text, copyText: item.text,
    label: item.channel === 'tool_input' ? 'Input' : 'Arguments' })) : view?.texts ?? []
  const results = row.kind === 'activity' ? row.results : []
  const name = entry && typeof entry.item === 'object' && entry.item !== null && 'name' in entry.item && typeof entry.item.name === 'string'
    ? entry.item.name : 'Tool call'
  const status = row.kind === 'live'
    ? row.live.some(item => item.streaming) && streamingActive ? 'Streaming input' : 'Input preview'
    : results.length ? 'Result retained' : entry?.tool ? 'No loaded result' : 'Operation ownership unavailable'
  const elapsed = exchange?.startedAtMs !== undefined && exchange.endedAtMs !== undefined && exchange.endedAtMs >= exchange.startedAtMs
    ? (exchange.endedAtMs - exchange.startedAtMs) / 1000 : undefined
  return <section className="tool-activity" aria-label={`${name} tool call`}>
    <header className="tool-heading"><h3>{name}</h3><span className="tool-status">{jobs.length ? `${jobs.map(job => `Tool ${job.state}`).join(' · ')} · ` : ''}{status}</span></header>
    <div className="tool-source" translate="no">{parts.map((part, index) => <MessageContent key={index} text={part.text} copyText={part.copyText}
      label={part.label} memoryKey={`part:${index}`} bounded />)}</div>
    {entry && !view && <HistoryItem listItem={false} entry={{ position: entry.position, hash: entry.hash, byteLen: String(new TextEncoder().encode(JSON.stringify(entry.item)).length), item: entry.item }} />}
    {row.kind === 'live' && row.live.some(item => item.overflow) && <p className="hint">Partial live preview. Completed content will be available in retained history.</p>}
    {results.length > 0 && <div className="tool-results"><h4>Result</h4>{results.map(result => {
      const resultView = readable(result.item)
      return <PresentationMemory key={result.sequence}>{resultView ? resultView.texts.map((part, index) =>
        <MessageContent key={index} text={part.text} copyText={part.copyText} label={`Result ${result.sequence}`} memoryKey={`result:${index}`} bounded />)
        : <HistoryItem listItem={false} entry={{ position: result.position, hash: result.hash, byteLen: String(new TextEncoder().encode(JSON.stringify(result.item)).length), item: result.item }} />}
        <ToolEvidence entry={result} label="Result" />
      </PresentationMemory>
    })}</div>}
    <PresentationDetails memoryKey="execution" className="execution-log" summary="Execution log">
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
    </PresentationDetails>
    {entry && <ToolEvidence entry={entry} label="Tool" />}
  </section>
}

function ToolEvidence({ entry, label }: { entry: MessageEntry; label: 'Tool' | 'Result' }) {
  const [rawVisible, setRawVisible] = useState(false)
  return <PresentationDetails memoryKey={`${label}-details`} className="timeline-metadata" summary={`${label} details`}>
    <p>Sequence {entry.sequence} · exchange {entry.requestId} · position {entry.position}</p><p>Hash {entry.hash}</p>
    <button type="button" aria-expanded={rawVisible} onClick={() => setRawVisible(!rawVisible)}>{rawVisible ? 'Hide' : 'Show'} raw {label.toLowerCase()} item</button>
    {rawVisible && <MessageContent text={JSON.stringify(entry.item, null, 2)} label={`Raw ${label.toLowerCase()} item`} />}
  </PresentationDetails>
}
