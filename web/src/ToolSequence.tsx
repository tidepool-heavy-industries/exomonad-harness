import { useState, type ReactNode } from 'react'
import type { TimelineRow, MessageEntry } from './chat-timeline'
import { resultExcerpt, literalExcerpt, toolCallName, type ExchangeObservation, type ToolJobObservation } from './ToolActivity'
import { ordinaryItem, readable } from './HistoryItem'
import { compareDecimal } from './decimal'
import { originKey, type OutputOrigin } from './live-output'

export default function ToolSequence({ rows, renderRow, requests, jobs, modelOrigin, streamingActive, memories }: {
  memories?: Map<string, boolean>[];
  streamingActive: boolean; rows: readonly TimelineRow[]; renderRow: (row: TimelineRow) => ReactNode
  requests?: ReadonlyMap<string, ExchangeObservation>; jobs: readonly ToolJobObservation[]; modelOrigin?: OutputOrigin
}) {
  const [localOpen, setLocalOpen] = useState(false)
  const open = memories ? memories.some(memory => memory.get('tool-sequence')) : localOpen
  const results = new Map<string, MessageEntry>()
  const failedJobs = new Set<string>()
  const observedJobs = new Map<string, ToolJobObservation>()
  const interrupted = new Set<string>()
  const inputs: { key: string; name: string; reference: string; preview?: string }[] = []
  let calls = 0
  let streaming = false
  for (const row of rows) {
    if (row.kind === 'activity') {
      calls++
      const source = readable(ordinaryItem(row.entry))?.texts[0]?.text
      inputs.push({ key: row.key, name: toolCallName(row.entry), reference: `Call ${row.entry.sequence}`, preview: source ? literalExcerpt(source) : undefined })
    }
    if (row.kind === 'activity' || row.kind === 'result') {
      const operation = row.entry.tool?.operation
      if (operation && modelOrigin && originKey(operation.origin) === originKey(modelOrigin)) {
        for (const job of jobs) if (job.kind === 'job' && job.requestId === operation.request && job.callId === operation.call) {
          observedJobs.set(job.id, job)
          if (job.state === 'failed') failedJobs.add(job.id)
        }
      }
    }
    if (row.kind === 'result') results.set(row.entry.sequence, row.entry)
    if (row.kind === 'live') {
      calls++; streaming ||= row.live.some(item => item.streaming)
      inputs.push({ key: row.key, name: row.live.find(item => item.toolName)?.toolName ?? 'Tool call', reference: 'Input preview' })
    }
    const ids = row.kind === 'live' ? row.live.map(item => item.requestId) : 'entry' in row && row.entry.kind === 'message' ? [row.entry.requestId] : []
    for (const id of ids) if (requests?.get(id)?.state === 'failed') interrupted.add(id)
  }
  const jobStates = new Map<string, number>()
  for (const job of observedJobs.values()) if (job.state !== 'failed') jobStates.set(job.state, (jobStates.get(job.state) ?? 0) + 1)
  const orderedResults = [...results.values()].sort((left, right) => compareDecimal(left.sequence, right.sequence))
  return <details className="tool-sequence" aria-label="Tool activity group" open={open} onToggle={event => {
    const next = event.currentTarget.open
    for (const memory of memories ?? []) memory.set('tool-sequence', next)
    setLocalOpen(next)
  }}>
    <summary>
      <span className="tool-sequence-title">{calls ? `${calls} tool ${calls === 1 ? 'call' : 'calls'}` : 'Tool results'}
        {results.size > 0 && ` · ${results.size} ${results.size === 1 ? 'result' : 'results'}`}{streaming && streamingActive && !interrupted.size && ' · streaming'}</span>
      <span className="tool-sequence-action">{open ? 'Hide calls and results' : 'Show calls and results'}</span>
      {[...jobStates].map(([state, count]) => <span className="tool-sequence-job" key={state}>{count} {state.replaceAll('_', ' ')} {count === 1 ? 'job' : 'jobs'}</span>)}
      {failedJobs.size > 0 && <span className="tool-sequence-issue">{failedJobs.size} failed {failedJobs.size === 1 ? 'job' : 'jobs'}</span>}
      {interrupted.size > 0 && <span className="tool-sequence-issue">Response interrupted</span>}
      {inputs.slice(-3).map(input => <span className="tool-sequence-input" key={input.key}>
        <span className="tool-call-reference">{input.reference}</span> <strong>{input.name}</strong>{input.preview && <code>{input.preview}</code>}
      </span>)}
      {inputs.length > 3 && <span className="tool-sequence-result">{inputs.length - 3} earlier calls inside</span>}
      {results.size > 3 && <span className="tool-sequence-result">{results.size - 3} earlier result excerpts inside</span>}
      {orderedResults.slice(-3).map(result => {
        const excerpt = resultExcerpt(ordinaryItem(result))
        const publication = rows.find(row => row.kind === 'result' && row.entry.sequence === result.sequence)
        const invocation = publication?.kind === 'result' ? publication.invocation : undefined
        return excerpt ? <span className="tool-sequence-result" key={result.sequence}>
          <span className="tool-call-reference">{invocation ? `Result for call ${invocation.sequence}` : `Result ${result.sequence}`}</span>{invocation && <> <strong>{toolCallName(invocation)}</strong></>}: {excerpt}
        </span> : null
      })}
    </summary>
    <div role="list" aria-label="Tool calls and results">{rows.map(renderRow)}</div>
  </details>
}
